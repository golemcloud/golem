// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::{RawOutput, TestContext, cmd, flag};
use crate::workspace_path;
use axum::{
    Router,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use golem_cli::{fs, versions};
use golem_common::schema::ExternalTypedSchemaValue;
use golem_schema::render::json_value::to_json_value;
use indoc::{formatdoc, indoc};
use serde::Deserialize;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use test_r::{test, timeout};

const OWNER: &str = r#"BashOwner("acceptance")"#;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BashResult {
    stdout: String,
    stderr: String,
    exit_code: u8,
    cwd: String,
}

// Start an isolated server with three owners: full access, no sibling binding, and denied sibling files.
async fn context() -> TestContext {
    let mut ctx = TestContext::new();
    let component_dir = ctx.cwd_path_join("component");
    fs::create_dir_all(component_dir.join("src")).unwrap();
    fs::copy(
        ctx.test_data_path_join("builtin-bash/src/lib.rs"),
        component_dir.join("src/lib.rs"),
    )
    .unwrap();
    fs::write_str(
        component_dir.join("Cargo.toml"),
        formatdoc! {r#"
            [package]
            name = "builtin_bash_fixture"
            version = "0.0.1"
            edition = "2024"

            [lib]
            crate-type = ["cdylib"]

            [profile.release]
            opt-level = "s"
            lto = true

            [dependencies]
            futures-concurrency = "7.6.3"
            golem-rust = {{ path = "{sdk}", features = ["export_golem_agentic"] }}
        "#, sdk = workspace_path().join("sdks/rust/golem-rust").display()},
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
            manifestVersion: {manifest_version}
            app: builtin-bash-acceptance

            componentTemplates:
              rust-test:
                build:
                - command: cargo build --target wasm32-wasip2 --release
                  sources:
                  - "{{{{ componentDir }}}}/src"
                  - "{{{{ componentDir }}}}/Cargo.toml"
                  targets:
                  - "{{{{ cargoTarget }}}}/wasm32-wasip2/release/builtin_bash_fixture.wasm"
                componentWasm: "{{{{ cargoTarget }}}}/wasm32-wasip2/release/builtin_bash_fixture.wasm"
                outputWasm: "{{{{ golemTempDir }}}}/agents/builtin_bash_fixture.wasm"
            components:
              builtin-bash:owner:
                dir: component
                templates: rust-test
                presets:
                  release: {{}}
            tools:
              bash:
                release:
                  account: builtin-tool-owner@golem.cloud
                  name: bash
                  version: "0.2.0"
              fixture: {{}}
            agents:
              BashOwner:
                tools:
                  bash:
                    filesystemAccess: allowed
                  fixture:
                    filesystemAccess: allowed
              BashOnlyOwner:
                tools:
                  bash:
                    filesystemAccess: allowed
              DeniedFilesOwner:
                tools:
                  bash:
                    filesystemAccess: allowed
                  fixture:
                    filesystemAccess: denied
            environments:
              local:
                server: local
                componentPresets: release
        "#, manifest_version = versions::sdk::MANIFEST},
    )
    .unwrap();
    ctx.start_server().await;
    let deployed = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(deployed.success_or_dump());
    // External tool invocation needs an existing owner; calling name creates each agent.
    for owner in [
        OWNER,
        r#"BashOnlyOwner("isolated")"#,
        r#"DeniedFilesOwner("denied")"#,
    ] {
        let created = ctx
            .cli([flag::YES, cmd::AGENT, cmd::INVOKE, owner, "name"])
            .await;
        assert!(created.success_or_dump());
    }
    ctx
}

// Every call is a fresh shell starting in `cwd` (empty for the default). Each call gets a new
// request key.
async fn invoke(ctx: &TestContext, owner: &str, cwd: &str, script: &str) -> BashResult {
    invoke_with_key(ctx, owner, cwd, script, &uuid::Uuid::new_v4().to_string()).await
}

async fn invoke_raw(
    ctx: &TestContext,
    owner: &str,
    cwd: &str,
    script: &str,
    key: &str,
) -> RawOutput {
    let started = Instant::now();
    let output = tokio::time::timeout(
        Duration::from_secs(90),
        ctx.cli_with_input(
            [
                flag::YES,
                "--format",
                "json",
                cmd::TOOL,
                cmd::INVOKE,
                "--agent",
                owner,
                "--idempotency-key",
                key,
                "bash",
                "--",
                "run",
                "--cwd",
                cwd,
                "--",
                script,
            ],
            &[],
        ),
    )
    .await
    .expect("bound bash invocation did not complete");
    println!("bash invocation {key} on {owner}: {:?}", started.elapsed());
    output
}

async fn invoke_with_key(
    ctx: &TestContext,
    owner: &str,
    cwd: &str,
    script: &str,
    key: &str,
) -> BashResult {
    let output = invoke_raw(ctx, owner, cwd, script, key).await;
    assert!(output.success(), "{}", output.stderr_text());
    let response: Value = serde_json::from_slice(output.stdout()).unwrap();
    decode_result(&response["response"])
}

// RPC success and the script exit code are separate; decode the shell result from its envelope.
fn decode_result(response: &Value) -> BashResult {
    let result = &response["result"];
    assert_eq!(result["type"], "Success", "{response:#}");
    let typed: ExternalTypedSchemaValue = serde_json::from_value(result["result"].clone()).unwrap();
    let typed = typed.as_inner();
    let value = to_json_value(typed.graph(), &typed.graph().root, typed.value()).unwrap();
    serde_json::from_value(value).unwrap()
}

// Enqueue a bash run under a fixed key and return once it is accepted, without awaiting it.
async fn trigger(ctx: &TestContext, cwd: &str, script: &str, key: &str) {
    trigger_on(ctx, OWNER, cwd, script, key).await;
}

async fn trigger_on(ctx: &TestContext, owner: &str, cwd: &str, script: &str, key: &str) {
    let output = ctx
        .cli_with_input(
            [
                flag::YES,
                cmd::TOOL,
                cmd::INVOKE,
                "--agent",
                owner,
                "--trigger",
                "--idempotency-key",
                key,
                "bash",
                "--",
                "run",
                "--cwd",
                cwd,
                "--",
                script,
            ],
            &[],
        )
        .await;
    assert!(output.success(), "{}", output.stderr_text());
}

// Observe an existing invocation by key; input-free lookup never starts a replacement.
async fn lookup(ctx: &TestContext, key: &str) -> Value {
    lookup_on(ctx, OWNER, key).await
}

async fn lookup_on(ctx: &TestContext, owner: &str, key: &str) -> Value {
    let output = ctx
        .cli_with_input(
            [
                flag::YES,
                "--format",
                "json",
                cmd::TOOL,
                cmd::INVOKE,
                "--agent",
                owner,
                "--lookup",
                "--idempotency-key",
                key,
                "bash",
                "--",
                "run",
            ],
            &[],
        )
        .await;
    assert!(output.success(), "{}", output.stderr_text());
    let response: Value = serde_json::from_slice(output.stdout()).unwrap();
    response["response"].clone()
}

async fn await_completion(ctx: &TestContext, key: &str) -> BashResult {
    decode_result(&await_completion_response(ctx, key).await)
}

async fn await_completion_on(ctx: &TestContext, owner: &str, key: &str) -> BashResult {
    decode_result(&await_completion_response_on(ctx, owner, key).await)
}

// Like `await_completion`, but returns the raw envelope instead of assuming `result.type` is
// `Success` -- for a call whose outcome (a decodable `BashResult` vs. a trapped `Failure`) is
// itself part of what the test is proving.
async fn await_completion_response(ctx: &TestContext, key: &str) -> Value {
    await_completion_response_on(ctx, OWNER, key).await
}

async fn await_completion_response_on(ctx: &TestContext, owner: &str, key: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let response = lookup_on(ctx, owner, key).await;
        if response["status"] == "complete" {
            return response;
        }
        assert_eq!(response["status"], "pending", "{response:#}");
        assert!(
            Instant::now() < deadline,
            "{key} did not complete: {response:#}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn simulate_crash(ctx: &TestContext) {
    simulate_crash_on(ctx, OWNER).await;
}

async fn simulate_crash_on(ctx: &TestContext, owner: &str) {
    let crash = ctx
        .cli([flag::YES, cmd::AGENT, "simulate-crash", owner])
        .await;
    assert!(crash.success_or_dump());
}

// `hello`, then a `.` every 20 ms, never ending.
async fn trickle() -> axum::body::Body {
    use futures_util::StreamExt;
    let rest = futures_util::stream::unfold((), |()| async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Some((
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b".")),
            (),
        ))
    });
    let first = futures_util::stream::once(async {
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"hello"))
    });
    axum::body::Body::from_stream(first.chain(rest))
}

// Returns the request body verbatim, so a test can see exactly what curl/wget sent (joined -d
// values, a stripped @file, a POST's payload) instead of only observing local process exit codes.
async fn echo(body: axum::body::Bytes) -> axum::body::Bytes {
    body
}

// A gzip response that decodes to well over whttp's 64 MiB cap (`DEFAULT_MAX_BODY`) but is tiny
// on the wire: repeated bytes compress to almost nothing, so this transfers instantly even though
// the decoded size would not fit in memory if the cap did not stop it first.
async fn compressed_huge() -> impl axum::response::IntoResponse {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let chunk = vec![0u8; 1024 * 1024];
    // 65 MiB uncompressed: one MiB over whttp's 64 MiB decoded-body cap.
    for _ in 0..65 {
        encoder.write_all(&chunk).unwrap();
    }
    let compressed = encoder.finish().unwrap();
    ([(axum::http::header::CONTENT_ENCODING, "gzip")], compressed)
}

// A fixture checkpoint request, parked until the test releases it.
struct CheckpointArrival {
    idempotency_key: String,
    release: tokio::sync::oneshot::Sender<()>,
}

// Serve GET /checkpoint: report every request, then answer 204 only after the test releases it.
// GET /arrivals answers how many checkpoint requests have arrived, so a script can wait until its
// sibling's request has left rather than only until the sibling has started.
async fn checkpoint_server() -> (
    SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<CheckpointArrival>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (arrivals, arrived) = tokio::sync::mpsc::unbounded_channel();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let count = Arc::new(AtomicUsize::new(0));
    let arrivals_count = {
        let count = count.clone();
        get(move || async move { count.load(Ordering::SeqCst).to_string() })
    };
    let checkpoint = get(move |headers: HeaderMap| async move {
        let idempotency_key = headers
            .get("idempotency-key")
            .and_then(|key| key.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let (release, released) = tokio::sync::oneshot::channel();
        count.fetch_add(1, Ordering::SeqCst);
        let _ = arrivals.send(CheckpointArrival {
            idempotency_key,
            release,
        });
        match released.await {
            Ok(()) => StatusCode::NO_CONTENT,
            // The request abandoned by a crashed attempt is never released.
            Err(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    });
    let server = tokio::spawn(async move {
        let routes = Router::new()
            .route("/checkpoint", checkpoint)
            .route("/arrivals", arrivals_count);
        axum::serve(listener, routes)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    (address, arrived, stop, server)
}

async fn next_arrival(
    arrived: &mut tokio::sync::mpsc::UnboundedReceiver<CheckpointArrival>,
) -> CheckpointArrival {
    tokio::time::timeout(Duration::from_secs(120), arrived.recv())
        .await
        .expect("fixture did not reach its checkpoint")
        .expect("checkpoint server stopped")
}

// Crash the owner while its sibling is parked in the checkpoint request, release the request the
// recovered sibling sends, and wait for the original bash invocation to finish.
async fn crash_at_checkpoint(
    ctx: &TestContext,
    arrived: &mut tokio::sync::mpsc::UnboundedReceiver<CheckpointArrival>,
    key: &str,
) -> BashResult {
    // The parked request proves the sibling already appended `before` and is inside its call.
    let abandoned = next_arrival(arrived).await;
    let pending = lookup(ctx, key).await;
    assert_eq!(pending["status"], "pending", "{pending:#}");
    simulate_crash(ctx).await;
    // Recovery re-sends the incomplete GET under its original durable call, so the key matches.
    let resent = next_arrival(arrived).await;
    assert!(!abandoned.idempotency_key.is_empty());
    assert_eq!(resent.idempotency_key, abandoned.idempotency_key);
    drop(abandoned);
    resent.release.send(()).unwrap();
    await_completion(ctx, key).await
}

fn assert_result(result: &BashResult, stdout: &str, stderr: &str, exit_code: u8) {
    assert_eq!(result.stdout, stdout, "{result:?}");
    assert_eq!(result.stderr, stderr, "{result:?}");
    assert_eq!(result.exit_code, exit_code, "{result:?}");
}

#[test]
#[timeout("20 minutes")]
async fn bound_bash_executes_scripts_tools_and_recovers() {
    let ctx = context().await;

    // Ask the CLI for the published run contract, including the cwd option.
    let help = ctx
        .cli_with_input(
            [
                cmd::TOOL,
                cmd::INVOKE,
                "--agent",
                OWNER,
                "bash",
                "--",
                "run",
                "--help",
            ],
            &[],
        )
        .await;
    assert!(help.success(), "{}", help.stderr_text());
    assert!(help.stdout_text().contains("--cwd"));
    assert!(help.stdout_text().contains("--timeout"));

    // A script past the call's time limit is stopped (TERM, then KILL a second later): exit 124.
    let limited = ctx
        .cli_with_input(
            [
                flag::YES,
                "--format",
                "json",
                cmd::TOOL,
                cmd::INVOKE,
                "--agent",
                OWNER,
                "bash",
                "--",
                "run",
                "--timeout",
                "1",
                "--",
                "echo start; while :; do x=1; done",
            ],
            &[],
        )
        .await;
    assert!(limited.success(), "{}", limited.stderr_text());
    let response: Value = serde_json::from_slice(limited.stdout()).unwrap();
    let result = decode_result(&response["response"]);
    assert_result(
        &result,
        "start\n",
        "bash: the call exceeded its 1 s time limit\n",
        124,
    );

    // Keep stdout, stderr and exit 7 separate while the outer tool RPC still succeeds.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        "printf 'hello'; printf 'diagnostic' >&2; exit 7",
    )
    .await;
    assert_result(&result, "hello", "diagnostic", 7);
    assert!(result.cwd.starts_with('/'));

    // head reads one line and closes the pipe; failed upstream writes must end the endless loop.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        "while :; do echo x; done | cat | head -n 1",
    )
    .await;
    assert_result(&result, "x\n", "", 0);
    // A new call must not inherit leftover output or unfinished stages from that pipeline.
    let result = invoke(&ctx, OWNER, "", "printf fresh | cat").await;
    assert_result(&result, "fresh", "", 0);

    // Discover a sibling command and render its metadata-driven help inside bash.
    let result = invoke(&ctx, OWNER, "", "fixture transfer --help").await;
    assert!(result.stdout.contains("prefix"), "{result:?}");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stderr, "");
    // Pipe local output into the sibling tool, then pipe its completed output back to local cat.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        "printf 'sibling-stream' | fixture transfer 'prefix:' | cat",
    )
    .await;
    assert_result(&result, "prefix:sibling-stream", "", 0);
    // Preserve the sibling's named error and its declared shell exit code, 42.
    let result = invoke(&ctx, OWNER, "", "fixture fail").await;
    assert_result(
        &result,
        "",
        "tool error: selected: \"fixture failure\"\n",
        42,
    );
    // Command substitution must collect sibling output before the enclosing printf runs.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        "printf '%s' \"$(printf sub | fixture transfer prefix:)\"",
    )
    .await;
    assert_result(&result, "prefix:sub", "", 0);

    // Invoke the bound bash tool from bash itself and extract stdout from the nested result.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        r#"bash run --cwd / "printf nested" | jq .stdout"#,
    )
    .await;
    assert_result(&result, "\"nested\"\n", "", 0);

    // A small provider output attachment must arrive intact.
    let result = invoke(&ctx, OWNER, "", "fixture emit 16 | cat").await;
    assert_result(&result, "xxxxxxxxxxxxxxxx", "", 0);
    // Exceed the 16 MiB limit by one byte, first on tool input and then on tool output.
    for (script, diagnostic) in [
        (
            "{ for i in {1..256}; do printf '%65536s' x; done; printf x; } | fixture transfer '' >/dev/null",
            Some("16 MiB command input limit"),
        ),
        ("fixture emit 16777217 >/dev/null", None),
    ] {
        let limited = invoke(&ctx, OWNER, "", script).await;
        assert_ne!(
            limited.exit_code, 0,
            "attachment limit was not enforced: {limited:?}"
        );
        assert_eq!(limited.stdout, "");
        assert!(!limited.stderr.is_empty(), "{limited:?}");
        if let Some(diagnostic) = diagnostic {
            assert!(limited.stderr.contains(diagnostic), "{limited:?}");
        }
        // An attachment failure must not leave stale input or poison the next invocation.
        let result = invoke(&ctx, OWNER, "", "printf fresh | fixture transfer ''").await;
        assert_result(&result, "fresh", "", 0);
    }

    // The sibling writes a file that bash reads, then reads back bash's appended bytes.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        indoc! {r#"
        mkdir -p /tmp/builtin-bash
        fixture write /tmp/builtin-bash/shared from-tool >/dev/null
        cat /tmp/builtin-bash/shared
        printf /from-shell >> /tmp/builtin-bash/shared
        fixture read /tmp/builtin-bash/shared
    "#},
    )
    .await;
    assert_result(&result, "from-toolfrom-tool/from-shell", "", 0);

    // While the sibling waits, local read must time out (142), then cat receives completed output.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        indoc! {r#"
            fixture delayed /tmp/builtin-bash/peer-progress | {
                read -t 0.01 pending
                printf 'before=%s\n' "$?"
                cat
            }
        "#},
    )
    .await;
    assert_result(&result, "before=142\ncompleted", "", 0);

    // head closes immediately; the accepted sibling call must still finish its file write.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        indoc! {r#"
        fixture delayed /tmp/builtin-bash/after-delay | head -c 0
        cat /tmp/builtin-bash/after-delay
    "#},
    )
    .await;
    assert_result(&result, "completed", "", 0);

    // A call ends in the directory it changed to, and leaves a variable and a function in a file.
    let first = invoke(
        &ctx,
        OWNER,
        "",
        indoc! {r#"
        cd /tmp/builtin-bash
        export SAVED='two words'
        remembered() { printf 'function:%s' "$SAVED"; }
        declare -p SAVED >settings.sh
        declare -f remembered >>settings.sh
    "#},
    )
    .await;
    assert_result(&first, "", "", 0);
    assert_eq!(first.cwd, "/tmp/builtin-bash");
    // The next call is a fresh shell in the returned directory: the variable and function are gone.
    let second = invoke(
        &ctx,
        OWNER,
        &first.cwd,
        r#"printf '%s|%s|' "$PWD" "${SAVED-unset}"; type remembered >/dev/null 2>&1 || printf no-function"#,
    )
    .await;
    assert_result(&second, "/tmp/builtin-bash|unset|no-function", "", 0);
    // What the first call wrote to a file comes back when a later call sources it.
    let sourced = invoke(&ctx, OWNER, &first.cwd, ". ./settings.sh; remembered").await;
    assert_result(&sourced, "function:two words", "", 0);

    // A directory that is not absolute, or does not exist, fails before the script can run.
    for (cwd, reason) in [
        ("tmp", "cwd tmp: not an absolute path"),
        (
            "/tmp/builtin-bash/missing",
            "cwd /tmp/builtin-bash/missing: No such file or directory",
        ),
    ] {
        let invalid = invoke_raw(
            &ctx,
            OWNER,
            cwd,
            "printf bad >/tmp/builtin-bash/invalid-cwd",
            &uuid::Uuid::new_v4().to_string(),
        )
        .await;
        assert!(invalid.success(), "{}", invalid.stderr_text());
        let invalid: Value = serde_json::from_slice(invalid.stdout()).unwrap();
        assert_eq!(
            invalid["response"]["result"]["type"], "Failure",
            "{invalid:#}"
        );
        assert!(invalid.to_string().contains("invalid-cwd"), "{invalid:#}");
        assert!(invalid.to_string().contains(reason), "{invalid:#}");
    }
    // The rejection is also in the agent's log: the tool installs the wasi logger itself, since
    // golem-rust does so only in an agent's constructor.
    let oplog = ctx
        .cli([cmd::AGENT, "oplog", OWNER, "--query", "\"cwd rejected\""])
        .await;
    assert!(oplog.success_or_dump());
    let oplog = oplog.stdout().collect::<Vec<_>>().join("\n");
    assert!(oplog.contains("cwd rejected: "), "{oplog}");
    // Check the owner filesystem to prove the rejected scripts had no file effect.
    let result = invoke(&ctx, OWNER, "", "test ! -e /tmp/builtin-bash/invalid-cwd").await;
    assert_result(&result, "", "", 0);

    // Create the same directory in a different owner whose only bound tool is bash.
    let isolated = invoke(
        &ctx,
        r#"BashOnlyOwner("isolated")"#,
        "",
        "mkdir -p /tmp/builtin-bash",
    )
    .await;
    assert_result(&isolated, "", "", 0);
    // Starting in a directory another owner used grants nothing of its: fixture stays missing.
    let unbound = invoke(
        &ctx,
        r#"BashOnlyOwner("isolated")"#,
        &first.cwd,
        "fixture fail",
    )
    .await;
    assert_eq!(unbound.exit_code, 127, "{unbound:?}");
    assert_eq!(unbound.stdout, "");
    assert!(unbound.stderr.contains("fixture"), "{unbound:?}");
    // Bash may write this owner's file, but its filesystem-denied sibling must leave it unchanged.
    let denied = invoke(
        &ctx,
        r#"DeniedFilesOwner("denied")"#,
        "",
        indoc! {r#"
        mkdir -p /tmp/builtin-bash
        printf owner-data >/tmp/builtin-bash/guarded
        fixture write /tmp/builtin-bash/guarded forbidden
        code=$?
        cat /tmp/builtin-bash/guarded
        exit "$code"
    "#},
    )
    .await;
    assert_eq!(denied.exit_code, 23, "{denied:?}");
    assert_eq!(denied.stdout, "owner-data");
    assert!(denied.stderr.starts_with("tool error: file:"), "{denied:?}");

    // Background work runs; the job's file effect is visible after wait.
    let background = invoke(
        &ctx,
        OWNER,
        "",
        "printf ok >/tmp/builtin-bash/background & wait; cat /tmp/builtin-bash/background",
    )
    .await;
    assert_result(&background, "ok", "", 0);

    // Process substitution is buffered: `<(list)` runs before the command, `>(list)` after it.
    let substituted = invoke(
        &ctx,
        OWNER,
        "",
        "cat <(printf data); printf out > >(cat); echo; diff <(echo a) <(echo a) && echo same",
    )
    .await;
    assert_result(&substituted, "dataout\nsame\n", "", 0);

    // A command bash-tool refuses refuses the whole script before any of it runs.
    let refused = invoke(
        &ctx,
        OWNER,
        "",
        "printf bad >/tmp/builtin-bash/refused; umask",
    )
    .await;
    assert_result(&refused, "", "bash: umask is unsupported in bash-tool\n", 2);
    let result = invoke(&ctx, OWNER, "", "test ! -e /tmp/builtin-bash/refused").await;
    assert_result(&result, "", "", 0);

    // HTTP command help must succeed without making a network request.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        "curl --help >/dev/null && wget --help >/dev/null",
    )
    .await;
    assert_result(&result, "", "", 0);

    // HTTP requests stay local to the test and exercise guest-side curl and wget.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let http = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/payload", get(|| async { "network-body\n" }))
                .route(
                    "/missing",
                    get(|| async { (StatusCode::NOT_FOUND, "missing-body\n") }),
                )
                .route("/trickle", get(trickle))
                .route("/echo", axum::routing::post(echo))
                .route("/compressed-huge", get(compressed_huge)),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    // Fetch the controlled response with each HTTP command and pipe its body through cat.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        &format!(
            "curl -s http://{address}/payload | cat; wget -q -O - http://{address}/payload | cat"
        ),
    )
    .await;
    // Relative download paths must resolve against the shell's directory after cd.
    let downloaded = invoke(
        &ctx,
        OWNER,
        "",
        &formatdoc! {r#"
            mkdir -p /tmp/builtin-bash/downloads
            cd /tmp/builtin-bash/downloads
            curl -s -o curl-output http://{address}/payload &&
            wget -q -O wget-output http://{address}/payload &&
            cat curl-output wget-output
        "#},
    )
    .await;
    // Downloaded files belong to the owner and outlast the call that made them.
    let files = invoke(
        &ctx,
        OWNER,
        "",
        "cat /tmp/builtin-bash/downloads/curl-output /tmp/builtin-bash/downloads/wget-output",
    )
    .await;
    // curl -f must suppress the 404 body and return its diagnostic with exit code 22.
    let curl_error = invoke(
        &ctx,
        OWNER,
        "",
        &format!("curl -f http://{address}/missing"),
    )
    .await;
    // Without -f, an HTTP error status is a transfer that worked, as in curl: the 404's body is
    // written, nothing is said, and the status is 0.
    let curl_status = invoke(
        &ctx,
        OWNER,
        "",
        &format!("curl http://{address}/missing; echo status=$?"),
    )
    .await;
    // wget must also suppress the 404 body, using its own diagnostic and GNU wget's exit code for
    // an error response from the server, 8.
    let wget_error = invoke(
        &ctx,
        OWNER,
        "",
        &format!("wget -O - http://{address}/missing"),
    )
    .await;
    // Both HTTP commands must still work after the failed requests.
    let following = invoke(
        &ctx,
        OWNER,
        "",
        &format!(
            "curl -s http://{address}/payload | cat; wget -q -O - http://{address}/payload | cat"
        ),
    )
    .await;
    // A body that never ends: only a client that forwards each chunk as it arrives lets the
    // reader see the prefix, and its next write then fails and ends the pipeline.
    let streamed = invoke(
        &ctx,
        OWNER,
        "",
        &format!(
            "curl -s http://{address}/trickle 2>/dev/null | head -c 5; echo; \
             wget -q -O - http://{address}/trickle 2>/dev/null | head -c 5; echo"
        ),
    )
    .await;

    // HTTP behavior proven end to end through a real Golem server, through Golem's
    // durable wasi:http: a POST's body, -d joining and -d @file's CR/LF stripping, -m bounding
    // the whole transfer and -m 0 meaning unlimited, and the --compressed decoded-size cap. (An
    // earlier run saw joining, the stripping, -m and the cap fail here: its server embedded a
    // bash.wasm built before those fixes, which the unit tests exercised against the source.)

    // A basic POST round trip: the server echoes exactly what curl sent as the request body.
    let posted = invoke(
        &ctx,
        OWNER,
        "",
        &format!("curl -s -X POST -d 'hello=world' http://{address}/echo"),
    )
    .await;

    // Repeated -d values join with `&`, and -d @file drops the file's CR and LF bytes, both as
    // curl does; --data-binary @file sends the file as it is.
    let joined = invoke(
        &ctx,
        OWNER,
        "",
        &format!("curl -s -d a=1 -d b=2 -d c=3 http://{address}/echo"),
    )
    .await;
    let from_file = invoke(
        &ctx,
        OWNER,
        "",
        &format!(
            "mkdir -p /tmp/builtin-bash && printf 'x=1\\r\\ny=2\\n' > /tmp/builtin-bash/form && \
             curl -s -d @/tmp/builtin-bash/form http://{address}/echo && echo && \
             curl -s --data-binary @/tmp/builtin-bash/form http://{address}/echo | od -c"
        ),
    )
    .await;

    // -m bounds the whole transfer, not only the wait for the first byte: a body that never ends
    // and never goes idle (a byte every 20 ms) is cut at the deadline.
    let limited = invoke(
        &ctx,
        OWNER,
        "",
        &format!("curl -s -m 1 http://{address}/trickle > /dev/null; echo status=$?"),
    )
    .await;

    // -m 0 means unlimited (not a zero-length timeout that fires immediately): reading a bounded
    // prefix of the same never-ending stream under -m 0 must still succeed. This goes through a
    // different mechanism than the -m 1 case above (the consumer, `head`, closes the pipe early --
    // the same pattern the `streamed` case above already proved works), not curl's own deadline.
    let unbounded = invoke(
        &ctx,
        OWNER,
        "",
        &format!("curl -s -m 0 http://{address}/trickle | head -c 25; echo; echo status=$?"),
    )
    .await;

    // --compressed caps the decoded body as the raw one is: 65 MiB of zeros decodes past whttp's
    // 64 MiB limit although it is tiny on the wire.
    let decoded = invoke(
        &ctx,
        OWNER,
        "",
        &format!(
            "curl -s --compressed http://{address}/compressed-huge | wc -c; \
             echo \"status=${{PIPESTATUS[0]}}\""
        ),
    )
    .await;

    let _ = stop.send(());
    http.await.unwrap();
    assert_result(&streamed, "hello\nhello\n", "", 0);
    assert_result(&result, "network-body\nnetwork-body\n", "", 0);
    assert_result(&downloaded, "network-body\nnetwork-body\n", "", 0);
    assert_eq!(downloaded.cwd, "/tmp/builtin-bash/downloads");
    assert_result(&files, "network-body\nnetwork-body\n", "", 0);
    assert_result(
        &curl_error,
        "",
        "curl: (22) The requested URL returned error: 404\n",
        22,
    );
    assert_result(&curl_status, "missing-body\nstatus=0\n", "", 0);
    // An agent has no HOME, so wget says, as GNU's does, that it cannot keep its HSTS store.
    assert_result(
        &wget_error,
        "",
        "ERROR: could not open HSTS store. HSTS will be disabled.\n\
         wget: server returned status 404\n",
        8,
    );
    assert_result(&following, "network-body\nnetwork-body\n", "", 0);

    // HTTP behavior, proven end to end.
    assert_result(&posted, "hello=world", "", 0);
    assert_result(&joined, "a=1&b=2&c=3", "", 0);
    assert_result(
        &from_file,
        "x=1y=2\n0000000   x   =   1  \\r  \\n   y   =   2  \\n\n0000011\n",
        "",
        0,
    );
    // curl's own status for -m running out; -s hides its message.
    assert_result(&limited, "status=28\n", "", 0);
    // Nothing of the oversized body reaches stdout, and curl fails (whttp's cap, exit 4).
    assert_result(&decoded, "0\nstatus=4\n", "", 0);
    // -m 0 must not time out at all: the same stream, read only up to a fixed prefix by `head`
    // rather than by curl giving up, still succeeds (head's own status, 0).
    assert!(unbounded.stdout.starts_with("hello."), "{unbounded:?}");
    let unbounded_body = unbounded.stdout.strip_suffix("\nstatus=0\n").unwrap();
    assert_eq!(unbounded_body.len(), 25, "{unbounded:?}");
    assert!(
        unbounded_body.chars().skip(5).all(|c| c == '.'),
        "{unbounded:?}"
    );
    assert_eq!(unbounded.stderr, "", "{unbounded:?}");
    assert_eq!(unbounded.exit_code, 0, "{unbounded:?}");

    // Complete an append using a fixed request key so recovery can identify the same invocation.
    let script = "printf x >> /tmp/builtin-bash/replay; cat /tmp/builtin-bash/replay";
    let before = invoke_with_key(&ctx, OWNER, "", script, "bash-replay").await;
    assert_result(&before, "x", "", 0);
    // Crash the owner after completion; this does not test a crash during a pending sibling call.
    let crash = ctx
        .cli([flag::YES, cmd::AGENT, "simulate-crash", OWNER])
        .await;
    assert!(crash.success_or_dump());
    // The same key must recover the original result without appending a second x.
    let recovered = invoke_with_key(&ctx, OWNER, "", script, "bash-replay").await;
    assert_result(&recovered, "x", "", 0);
    assert_eq!(recovered.cwd, before.cwd);
    // A new request confirms exactly one append and a working pipeline after recovery.
    let result = invoke(
        &ctx,
        OWNER,
        "",
        "cat /tmp/builtin-bash/replay; printf '|clean' | cat",
    )
    .await;
    assert_result(&result, "x|clean", "", 0);
}

#[test]
#[timeout("20 minutes")]
async fn bound_bash_recovers_a_crash_while_a_sibling_is_pending() {
    let ctx = context().await;
    let (address, mut arrived, stop, server) = checkpoint_server().await;

    // The directory the recovered pipeline must start in, as a caller passes it.
    let incoming = invoke(
        &ctx,
        OWNER,
        "",
        "mkdir -p /tmp/builtin-bash/recovery; cd /tmp/builtin-bash/recovery",
    )
    .await;
    assert_result(&incoming, "", "", 0);

    // Crash while the piped sibling is pending, then finish with a nonzero exit.
    let piped_key = "bash-pending-piped";
    let piped_script = formatdoc! {r#"
        fixture checkpoint "$PWD/piped" {address} | cat
        saved="${{PIPESTATUS[*]}}"
        printf '%s' "$saved" >pipestatus
        printf '|pipestatus=%s' "$saved"
        printf diagnostic >&2
        exit 9
    "#};
    trigger(&ctx, &incoming.cwd, &piped_script, piped_key).await;
    let piped = crash_at_checkpoint(&ctx, &mut arrived, piped_key).await;
    assert_result(&piped, "completed|pipestatus=0 0", "diagnostic", 9);
    assert_eq!(piped.cwd, "/tmp/builtin-bash/recovery");

    // head exits before the sibling completes. An uninterrupted run is the oracle for the crash run.
    let closed_script = |file: &str| {
        formatdoc! {r#"
            fixture checkpoint "$PWD/{file}" {address} | head -c 0
            printf 'pipestatus=%s' "${{PIPESTATUS[*]}}"
        "#}
    };
    let control_script = closed_script("control");
    let (control, ()) = tokio::join!(invoke(&ctx, OWNER, &incoming.cwd, &control_script), async {
        next_arrival(&mut arrived).await.release.send(()).unwrap()
    },);
    assert!(
        control.stdout.ends_with(" 0"),
        "head must succeed: {control:?}"
    );
    // Reader closure must not cancel the accepted sibling call, before or after the crash.
    let closed_key = "bash-pending-closed";
    trigger(&ctx, &incoming.cwd, &closed_script("closed"), closed_key).await;
    let closed = crash_at_checkpoint(&ctx, &mut arrived, closed_key).await;
    assert_result(&closed, &control.stdout, &control.stderr, control.exit_code);

    // Crash after completion: the same keys return the recorded results without a new request.
    simulate_crash(&ctx).await;
    for (key, expected) in [(piped_key, &piped), (closed_key, &closed)] {
        let recorded = await_completion(&ctx, key).await;
        assert_result(
            &recorded,
            &expected.stdout,
            &expected.stderr,
            expected.exit_code,
        );
        assert_eq!(recorded.cwd, expected.cwd);
    }
    // A fresh call replays the owner and sees each effect exactly once.
    let effects = invoke(
        &ctx,
        OWNER,
        &piped.cwd,
        "cat piped control closed; printf 'pipestatus=%s' \"$(cat pipestatus)\"",
    )
    .await;
    assert_result(
        &effects,
        "before\nafter\nbefore\nafter\nbefore\nafter\npipestatus=0 0",
        "",
        0,
    );

    // Two requests per crashed invocation and one for the control; replay sent none.
    let _ = stop.send(());
    server.await.unwrap();
    assert!(arrived.recv().await.is_none(), "replay repeated a request");
}

#[test]
#[timeout("20 minutes")]
async fn bound_bash_background_jobs_signal_and_cancel_siblings() {
    let ctx = context().await;
    let (address, mut arrived, stop, server) = checkpoint_server().await;
    let incoming = invoke(
        &ctx,
        OWNER,
        "",
        "mkdir -p /tmp/builtin-bash/jobs; cd /tmp/builtin-bash/jobs",
    )
    .await;
    assert_result(&incoming, "", "", 0);

    // Each call is a new process: `$$` is a plausible process number, and a new one every call.
    let first = invoke(&ctx, OWNER, &incoming.cwd, "echo $$").await;
    let second = invoke(&ctx, OWNER, &incoming.cwd, "echo $$").await;
    assert_ne!(first.stdout, second.stdout);
    for call in [&first, &second] {
        let pid: i32 = call.stdout.trim().parse().unwrap();
        assert!((1_000..=4_194_303).contains(&pid), "{call:?}");
    }

    // kill cancels a job's pending sibling call: `after` is never written. The script waits for
    // the request itself: the sibling writes `before` first, and a kill in between would cancel it
    // before its request is sent.
    let killed = formatdoc! {r#"
        fixture checkpoint "$PWD/killed" {address} & p=$!
        until [ "$(curl -s http://{address}/arrivals)" = 1 ]; do sleep 0.01; done
        kill $p; wait $p; echo status=$?
    "#};
    let (killed, parked) = tokio::join!(
        invoke(&ctx, OWNER, &incoming.cwd, &killed),
        next_arrival(&mut arrived)
    );
    assert_result(&killed, "status=143\n", "", 0);
    drop(parked);

    // The end of a run stops a leftover job and cancels its pending call.
    let leftover = formatdoc! {r#"
        fixture checkpoint "$PWD/leftover" {address} &
        until [ "$(curl -s http://{address}/arrivals)" = 2 ]; do sleep 0.01; done
        echo done
    "#};
    let (leftover, parked) = tokio::join!(
        invoke(&ctx, OWNER, &incoming.cwd, &leftover),
        next_arrival(&mut arrived)
    );
    assert_eq!(leftover.stdout, "done\n", "{leftover:?}");
    assert!(
        leftover.stderr.starts_with("bash: stopped job [1] (pid "),
        "{leftover:?}"
    );
    let tail = format!(", hangup): fixture checkpoint \"$PWD/leftover\" {address}\n");
    assert!(leftover.stderr.ends_with(&tail), "{leftover:?}");
    drop(parked);

    // Several sibling calls in flight at once through the real RPC path: each job keeps its own
    // status (the failing one its declared 42) and its own effects.
    let concurrent = indoc! {r#"
        fixture write "$PWD/one" first >/dev/null & a=$!
        fixture write "$PWD/two" second >/dev/null & b=$!
        fixture fail 2>/dev/null & c=$!
        wait $a; echo a=$?; wait $b; echo b=$?; wait $c; echo c=$?
        cat one; echo; cat two; echo
    "#};
    let concurrent = invoke(&ctx, OWNER, &incoming.cwd, concurrent).await;
    assert_result(&concurrent, "a=0\nb=0\nc=42\nfirst\nsecond\n", "", 0);

    let effects = invoke(&ctx, OWNER, &incoming.cwd, "cat killed leftover").await;
    assert_result(&effects, "before\nbefore\n", "", 0);

    let _ = stop.send(());
    server.await.unwrap();
    // One request each for the killed and leftover jobs.
    assert!(
        arrived.recv().await.is_none(),
        "unexpected extra checkpoint request"
    );
}

// Quarantined: after `simulate-crash`, an owner parked on a background job's pending sibling call
// is sometimes never reconstructed, so the recovered call's request never arrives (3 of 7 runs,
// locally and in CI). Nothing in the bash tool runs in that window; it looks like the executor's
// handling of a crash while the owner is parked, which is being investigated separately. The
// foreground variant, `bound_bash_recovers_a_crash_while_a_sibling_is_pending`, passes reliably.
#[ignore = "flaky: a parked owner is sometimes not reconstructed after simulate-crash"]
#[test]
#[timeout("20 minutes")]
async fn bound_bash_recovers_a_crash_while_a_background_job_waits() {
    let ctx = context().await;
    let (address, mut arrived, stop, server) = checkpoint_server().await;
    let incoming = invoke(
        &ctx,
        OWNER,
        "",
        "mkdir -p /tmp/builtin-bash/job-crash; cd /tmp/builtin-bash/job-crash",
    )
    .await;
    assert_result(&incoming, "", "", 0);

    // A crash while a background job waits on a sibling recovers the same result, once.
    let recovered_key = "bash-job-pending";
    let script = formatdoc! {r#"
        fixture checkpoint "$PWD/background" {address} & wait $!; echo status=$?
    "#};
    trigger(&ctx, &incoming.cwd, &script, recovered_key).await;
    let recovered = crash_at_checkpoint(&ctx, &mut arrived, recovered_key).await;
    // The job shares the script's stdout, so the sibling's output precedes the waiter's line.
    assert_result(&recovered, "completedstatus=0\n", "", 0);

    let effects = invoke(&ctx, OWNER, &incoming.cwd, "cat background").await;
    assert_result(&effects, "before\nafter\n", "", 0);

    let _ = stop.send(());
    server.await.unwrap();
    // Two requests for the crashed call; replaying the completed call sent none.
    assert!(
        arrived.recv().await.is_none(),
        "unexpected extra checkpoint request"
    );
}

// a parked GET/POST checkpoint over raw HTTP, distinct from `checkpoint_server`'s sibling
// RPC channel -- curl's own outbound wasi:http calls go through golem-worker-executor's own HTTP
// durability layer (`is_idempotent_http_method`), not the fixture tool's checkpoint mechanism, so
// proving crash-recovery resend behavior needs a server on that same path.
struct HttpArrival {
    method: &'static str,
    // Golem's own durability layer injects this into every outbound wasi:http request, derived
    // from the call's own oplog `Start` index and re-derived the same way on a resend -- so a
    // second arrival with the same value is the original call being replayed, not some unrelated
    // second request.
    idempotency_key: String,
    release: tokio::sync::oneshot::Sender<()>,
}

async fn park(
    method: &'static str,
    headers: HeaderMap,
    arrivals: tokio::sync::mpsc::UnboundedSender<HttpArrival>,
) -> StatusCode {
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|key| key.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let (release, released) = tokio::sync::oneshot::channel();
    let _ = arrivals.send(HttpArrival {
        method,
        idempotency_key,
        release,
    });
    match released.await {
        Ok(()) => StatusCode::NO_CONTENT,
        // The request abandoned by a crashed attempt is never released.
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn http_checkpoint_server() -> (
    SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<HttpArrival>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (arrivals, arrived) = tokio::sync::mpsc::unbounded_channel();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let get_arrivals = arrivals.clone();
    let post_arrivals = arrivals;
    let server = tokio::spawn(async move {
        let checkpoint = get(move |headers: HeaderMap| park("GET", headers, get_arrivals.clone()))
            .post(move |headers: HeaderMap| park("POST", headers, post_arrivals.clone()));
        let routes = Router::new().route("/checkpoint", checkpoint);
        axum::serve(listener, routes)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    (address, arrived, stop, server)
}

async fn next_http_arrival(
    arrived: &mut tokio::sync::mpsc::UnboundedReceiver<HttpArrival>,
) -> HttpArrival {
    tokio::time::timeout(Duration::from_secs(120), arrived.recv())
        .await
        .expect("HTTP request did not reach the checkpoint")
        .expect("checkpoint server stopped")
}

#[test]
#[timeout("20 minutes")]
async fn bound_bash_http_crash_recovery_resends_an_incomplete_request() {
    let ctx = context().await;
    let (address, mut arrived, stop, server) = http_checkpoint_server().await;

    // GET: crash while the request is parked (never answered), then release the one that
    // arrives after recovery. Golem's wasi:http durability treats GET as idempotent, so
    // recovery re-issues it rather than trying to replay a result that was never recorded.
    let get_key = "bash-http-get-replay";
    let get_script = format!("curl -s http://{address}/checkpoint >/dev/null; echo status=$?");
    trigger(&ctx, "", &get_script, get_key).await;
    let first_get = next_http_arrival(&mut arrived).await;
    assert_eq!(first_get.method, "GET");
    assert!(!first_get.idempotency_key.is_empty());
    let pending = lookup(&ctx, get_key).await;
    assert_eq!(pending["status"], "pending", "{pending:#}");
    simulate_crash(&ctx).await;
    let resent_get = next_http_arrival(&mut arrived).await;
    assert_eq!(resent_get.method, "GET", "recovery must re-send the GET");
    // Golem's own durability layer derives this header from the call's oplog `Start` index and
    // re-derives the same value on a resend, so a match here proves this is the original call
    // being replayed, not a second, unrelated request.
    assert_eq!(resent_get.idempotency_key, first_get.idempotency_key);
    drop(first_get);
    resent_get.release.send(()).unwrap();
    let get_result = await_completion(&ctx, get_key).await;
    assert_result(&get_result, "status=0\n", "", 0);

    // POST: Golem does not assume a POST is idempotent, so an interrupted one is recovered by
    // running it again from the start of its request: the server sees it a second time, with a
    // new idempotency-key (the documented at-least-once for POST/PATCH). Recovery must also leave
    // the owner healthy: an earlier build raced a durable timer against the request, and Golem's
    // recovery then failed and left the agent rejecting every call about 8 times in 10. Five crash
    // cycles, each on a fresh owner and followed by an ordinary call on it, guard against that.
    // (A second interrupted POST on the SAME owner currently crashes the executor -- an executor
    // defect reported separately -- so each cycle uses its own owner.)
    for cycle in 0..5 {
        let owner = format!(r#"BashOwner("post-crash-{cycle}")"#);
        let created = ctx
            .cli([flag::YES, cmd::AGENT, cmd::INVOKE, owner.as_str(), "name"])
            .await;
        assert!(created.success_or_dump());
        let post_key = format!("bash-http-post-replay-{cycle}");
        let post_script =
            format!("curl -s -X POST http://{address}/checkpoint >/dev/null; echo status=$?");
        trigger_on(&ctx, &owner, "", &post_script, &post_key).await;
        let first_post = next_http_arrival(&mut arrived).await;
        assert_eq!(first_post.method, "POST", "cycle {cycle}");
        assert!(!first_post.idempotency_key.is_empty(), "cycle {cycle}");
        let pending = lookup_on(&ctx, &owner, &post_key).await;
        assert_eq!(pending["status"], "pending", "cycle {cycle}: {pending:#}");
        simulate_crash_on(&ctx, &owner).await;
        drop(first_post.release);
        let resent_post = next_http_arrival(&mut arrived).await;
        assert_eq!(resent_post.method, "POST", "cycle {cycle}");
        assert_ne!(
            resent_post.idempotency_key, first_post.idempotency_key,
            "cycle {cycle}: a re-run POST gets a new key"
        );
        resent_post.release.send(()).unwrap();
        let post_result = await_completion_on(&ctx, &owner, &post_key).await;
        assert_result(&post_result, "status=0\n", "", 0);
        let follow_up = invoke(&ctx, &owner, "", &format!("echo after-crash-{cycle}")).await;
        assert_result(&follow_up, &format!("after-crash-{cycle}\n"), "", 0);
    }

    // A script that needs exactly-once sends its own key; Golem keeps it on the resent request,
    // so the server can tell the two sends are one operation.
    let keyed_key = "bash-http-post-keyed";
    let keyed_script = format!(
        "curl -s -X POST -H 'Idempotency-Key: order-42' http://{address}/checkpoint >/dev/null; \
         echo status=$?"
    );
    let keyed_owner = r#"BashOwner("post-crash-keyed")"#;
    let created = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, keyed_owner, "name"])
        .await;
    assert!(created.success_or_dump());
    trigger_on(&ctx, keyed_owner, "", &keyed_script, keyed_key).await;
    let first_keyed = next_http_arrival(&mut arrived).await;
    assert_eq!(first_keyed.idempotency_key, "order-42");
    let pending = lookup_on(&ctx, keyed_owner, keyed_key).await;
    assert_eq!(pending["status"], "pending", "{pending:#}");
    simulate_crash_on(&ctx, keyed_owner).await;
    drop(first_keyed.release);
    let resent_keyed = next_http_arrival(&mut arrived).await;
    assert_eq!(resent_keyed.method, "POST");
    assert_eq!(resent_keyed.idempotency_key, "order-42");
    resent_keyed.release.send(()).unwrap();
    let keyed_result = await_completion_on(&ctx, keyed_owner, keyed_key).await;
    assert_result(&keyed_result, "status=0\n", "", 0);

    let _ = stop.send(());
    server.await.unwrap();
    assert!(
        arrived.recv().await.is_none(),
        "unexpected extra checkpoint request beyond the one resend each"
    );
}
