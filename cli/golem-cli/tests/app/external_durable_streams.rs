// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::app::{TestContext, cmd, flag};
use crate::workspace_path;
use golem_cli::model::invoke_result_view::InvokeResultView;
use golem_cli::{fs, versions};
use golem_common::schema::FromSchema;
use indoc::formatdoc;
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use test_r::{test, timeout};
use tokio::process::{Child, Command};

const TEST_TOKEN: &str = "integration-test-capability";

struct ReferenceServer {
    process: Child,
    url: String,
    client: reqwest::Client,
    _directory: TempDir,
}

impl ReferenceServer {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // This is the server release at durable-streams/durable-streams protocol
        // baseline 461b40267aabd644558f9b19dbb9507dd5f691cf. npm ci pins transitives.
        for (name, contents) in [
            (
                "package.json",
                include_str!("durable_streams_reference/package.json"),
            ),
            (
                "package-lock.json",
                include_str!("durable_streams_reference/package-lock.json"),
            ),
            (
                "server.mjs",
                include_str!("durable_streams_reference/server.mjs"),
            ),
        ] {
            fs::write_str(directory.path().join(name), contents).unwrap();
        }
        let install = tokio::time::timeout(
            Duration::from_secs(120),
            Command::new("npm")
                .args(["ci", "--ignore-scripts", "--no-audit", "--no-fund"])
                .current_dir(directory.path())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("reference server npm ci timed out")
        .expect("failed to run npm ci");
        assert!(
            install.status.success(),
            "reference server npm ci failed: {}",
            String::from_utf8_lossy(&install.stderr)
        );
        let ready = directory.path().join("ready.json");
        let mut process = Command::new("node")
            .arg("server.mjs")
            .arg(&ready)
            .env("GOLEM_DS_TEST_TOKEN", TEST_TOKEN)
            .current_dir(directory.path())
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to start reference server");
        let url = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(
                    process.try_wait().unwrap().is_none(),
                    "reference server exited before readiness"
                );
                if let Ok(contents) = fs::read_to_string(&ready)
                    && let Ok(ready) = serde_json::from_str::<Value>(&contents)
                {
                    break ready["url"].as_str().unwrap().to_owned();
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("reference server startup timed out");
        Self {
            process,
            url,
            _directory: directory,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }

    async fn create(&self, path: &str, content_type: &str) -> String {
        let url = format!("{}{path}", self.url);
        let response = self
            .client
            .put(&url)
            .header("content-type", content_type)
            .bearer_auth(TEST_TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        url
    }

    async fn requests(&self) -> Vec<Value> {
        self.client
            .get(format!("{}/__golem_test/requests", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn wait_for_live(&self, path: &str, transport: &str) {
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                if self.requests().await.iter().any(|request| {
                    request["method"] == "GET" && request["path"] == path
                        && request["live"] == transport
                        // Exercise an actual empty/open long-poll response before data arrives.
                        && (transport != "long-poll" || request["status"] == 204)
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("guest never reached the selected live transport");
    }

    async fn stop(mut self) {
        drop(self.process.stdin.take());
        let status = tokio::time::timeout(Duration::from_secs(10), self.process.wait())
            .await
            .expect("reference server shutdown timed out")
            .unwrap();
        assert!(
            status.success(),
            "reference server shutdown failed: {status}"
        );
    }
}

async fn invoke<T: FromSchema>(
    ctx: &TestContext,
    agent: &str,
    method: &str,
    values: Vec<Value>,
) -> T {
    let mut args = vec![
        flag::YES.to_owned(),
        cmd::AGENT.to_owned(),
        cmd::INVOKE.to_owned(),
        format!("ExternalDurableStreams(\"{agent}\")"),
        method.to_owned(),
    ];
    args.extend(values.iter().map(Value::to_string));
    args.extend([
        "--no-stream".to_owned(),
        flag::FORMAT.to_owned(),
        "json".to_owned(),
    ]);
    let output = tokio::time::timeout(Duration::from_secs(60), ctx.cli(args))
        .await
        .expect("external durable stream invocation timed out");
    assert!(output.success_or_dump());
    let results: Vec<InvokeResultView> = output.stdout_json();
    assert_eq!(results.len(), 1, "expected one invocation result");
    let value = results[0]
        .result_json
        .as_ref()
        .expect("missing typed invocation result");
    T::from_value(value.value()).expect("unexpected guest result schema")
}

async fn invoke_template<T: FromSchema>(ctx: &TestContext, method: &str, values: Vec<Value>) -> T {
    let mut args = vec![
        flag::YES.to_owned(),
        cmd::AGENT.to_owned(),
        cmd::INVOKE.to_owned(),
        "StreamingAgent(\"external\")".to_owned(),
        method.to_owned(),
    ];
    args.extend(values.iter().map(Value::to_string));
    args.extend([
        "--no-stream".to_owned(),
        flag::FORMAT.to_owned(),
        "json".to_owned(),
    ]);
    let output = tokio::time::timeout(Duration::from_secs(90), ctx.cli(args))
        .await
        .expect("template Durable Streams invocation timed out");
    assert!(output.success_or_dump());
    let results: Vec<InvokeResultView> = output.stdout_json();
    let value = results[0]
        .result_json
        .as_ref()
        .expect("missing typed invocation result");
    T::from_value(value.value()).expect("unexpected template result schema")
}

async fn wait_for_closed_json(client: &reqwest::Client, url: &str) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let response = client.get(format!("{url}?offset=-1")).send().await.unwrap();
            if response
                .headers()
                .get("stream-closed")
                .and_then(|v| v.to_str().ok())
                == Some("true")
            {
                return response.json().await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("template output stream did not close")
}

async fn streaming_template_walkthrough(language: &str, append: &str, read: &str) {
    let mut ctx = TestContext::new();
    let app_name = format!("{language}-durable-streams-walkthrough");
    fs::create_dir_all(ctx.cwd_path_join(&app_name)).unwrap();
    ctx.cd(&app_name);
    ctx.add_env_var("DURABLE_STREAM_TOKEN", TEST_TOKEN);
    assert!(
        ctx.cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            &format!("{language}/streaming"),
        ])
        .await
        .success_or_dump()
    );
    let readme = fs::read_to_string(ctx.cwd_path_join("README.md")).unwrap();
    assert!(readme.contains(&format!("ORIGIN=http://{app_name}.localhost:9006")));
    assert!(!readme.contains("ORIGIN=http://app-name.localhost:9006"));
    assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
    ctx.start_server().await;
    let manifest_path = ctx.cwd_path_join("golem.yaml");
    let mut manifest: serde_yaml::Value =
        serde_yaml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["localServer"]["customRequestPort"] = ctx.custom_request_port().into();
    fs::write_str(&manifest_path, serde_yaml::to_string(&manifest).unwrap()).unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    let hostname = format!("{app_name}.localhost");
    // Only the test-process setup client needs an address override. Guest calls
    // below use the executor's production transport with the original authority.
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve(&hostname, "127.0.0.1:0".parse().unwrap())
        .build()
        .unwrap();
    let origin = format!("http://{hostname}:{}", ctx.custom_request_port());
    let base = format!("{origin}/durable-stream-agents/protocol/echo/invocations/template-session");
    let source = format!("{base}/streams/input");
    assert_eq!(
        client.put(&source).send().await.unwrap().status(),
        reqwest::StatusCode::CREATED
    );
    assert!(
        client
            .post(&source)
            .header("content-type", "application/json")
            .body("\"shared\"")
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let fork = source.replace("/invocations/", "/forks/template-fork/invocations/");
    assert_eq!(
        client
            .put(&fork)
            .header("stream-forked-from", source.strip_prefix(&origin).unwrap(),)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CREATED
    );
    for (url, body) in [(&source, "\"source-only\""), (&fork, "\"fork-only\"")] {
        assert!(
            client
                .post(url)
                .header("content-type", "application/json")
                .header("stream-closed", "true")
                .body(body)
                .send()
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    let source_result = format!("{base}/streams/$result");
    let fork_result = source_result.replace("/invocations/", "/forks/template-fork/invocations/");
    assert_eq!(
        wait_for_closed_json(&client, &source_result).await,
        [json!("echo:shared"), json!("echo:source-only")]
    );
    assert_eq!(
        wait_for_closed_json(&client, &fork_result).await,
        [json!("echo:shared"), json!("echo:fork-only")]
    );

    let guest_base =
        format!("{origin}/durable-stream-agents/protocol/echo/invocations/guest-local-session");
    let guest_source = format!("{guest_base}/streams/input");
    assert_eq!(
        client.put(&guest_source).send().await.unwrap().status(),
        reqwest::StatusCode::CREATED
    );
    // Closing the small batch before reading avoids the separate open-stream
    // flag interoperability issue and keeps the entire payload in one batch.
    let offset: Option<String> = invoke_template(
        &ctx,
        append,
        vec![
            json!(guest_source),
            json!("guest-local-producer"),
            json!(["local-first", "local-tail"]),
            json!(true),
        ],
    )
    .await;
    assert!(offset.is_some());
    let values: Vec<String> = invoke_template(&ctx, read, vec![json!(guest_source)]).await;
    assert_eq!(values, ["local-first", "local-tail"]);
    assert_eq!(
        wait_for_closed_json(&client, &format!("{guest_base}/streams/$result")).await,
        [json!("echo:local-first"), json!("echo:local-tail")]
    );

    let server = ReferenceServer::start().await;
    let external = server.create("/template", "application/json").await;
    let append_args = vec![
        json!(external),
        json!("stable-template-producer"),
        json!(["once"]),
        json!(false),
    ];
    let first: Option<String> = invoke_template(&ctx, append, append_args.clone()).await;
    assert!(first.is_some());
    let duplicate: Option<String> = invoke_template(&ctx, append, append_args).await;
    assert_eq!(duplicate, None);
    let _: Option<String> = invoke_template(
        &ctx,
        append,
        vec![
            json!(external),
            json!("closer"),
            json!(["tail"]),
            json!(true),
        ],
    )
    .await;
    let values: Vec<String> = invoke_template(&ctx, read, vec![json!(external)]).await;
    assert_eq!(values, ["once", "tail"]);
    let requests = server.requests().await;
    assert!(
        requests
            .iter()
            .any(|r| r["producerId"] == "stable-template-producer")
    );
    for method in ["GET", "POST"] {
        assert!(requests.iter().any(|r| {
            r["path"] == "/template" && r["method"] == method && r["authenticated"] == true
        }));
    }
    server.stop().await;
}

#[test]
#[timeout("40 minutes")]
async fn rust_streaming_template_durable_streams_walkthrough() {
    streaming_template_walkthrough("rust", "append_external", "read_external").await;
}

#[test]
#[timeout("40 minutes")]
async fn typescript_streaming_template_durable_streams_walkthrough() {
    streaming_template_walkthrough("ts", "appendExternal", "readExternal").await;
}

#[test]
#[timeout("40 minutes")]
async fn effect_streaming_template_durable_streams_walkthrough() {
    streaming_template_walkthrough("effect", "appendExternal", "readExternal").await;
}

#[test]
#[timeout("40 minutes")]
async fn scala_streaming_template_durable_streams_walkthrough() {
    streaming_template_walkthrough("scala", "appendExternal", "readExternal").await;
}

#[test]
#[timeout("40 minutes")]
async fn moonbit_streaming_template_durable_streams_walkthrough() {
    streaming_template_walkthrough("moonbit", "append_external", "read_external").await;
}

#[test]
#[timeout("10 minutes")]
async fn external_durable_streams_golem_gateway_e2e() {
    let mut ctx = TestContext::new();
    for name in ["external-durable-streams", "agent-sdk-rust"] {
        let fixture = workspace_path().join(format!(
            "test-components/golem_it_{}_release.wasm",
            name.replace('-', "_")
        ));
        assert!(
            fixture.is_file(),
            "build test-components/{name} before running this test"
        );
        fs::copy(&fixture, ctx.cwd_path_join(format!("{name}.wasm"))).unwrap();
    }
    ctx.start_server().await;
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: external-durable-streams-gateway
        environments:
          local:
            server: local
        components:
          golem-it:external-durable-streams:
            componentWasm: external-durable-streams.wasm
            outputWasm: external-durable-streams-final.wasm
          golem-it:agent-sdk-rust:
            componentWasm: agent-sdk-rust.wasm
            outputWasm: agent-sdk-rust-final.wasm
        httpApi:
          deployments:
            local:
              - domain: localhost:{port}
                agents:
                  DurableStreamAgent: {{}}
    "#, version = versions::sdk::MANIFEST, port = ctx.custom_request_port()},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let origin = format!("http://localhost:{}", ctx.custom_request_port());

    for transport in ["catch-up", "long-poll"] {
        let session =
            format!("{origin}/durable-stream-agents/{transport}/echo/invocations/session");
        let input = format!("{session}/streams/input");
        let output = format!("{session}/streams/output");
        let created = client.put(&input).send().await.unwrap();
        assert_eq!(created.status(), reqwest::StatusCode::CREATED);
        assert!(!created.headers().contains_key("stream-closed"));
        for method in [reqwest::Method::HEAD, reqwest::Method::GET] {
            let metadata = client
                .request(method.clone(), &session)
                .send()
                .await
                .unwrap();
            assert_eq!(metadata.status(), reqwest::StatusCode::OK);
            assert!(!metadata.headers().contains_key("stream-closed"));
            if method == reqwest::Method::GET {
                assert_eq!(metadata.json::<Value>().await.unwrap()["closed"], false);
            }
        }
        let appended: Result<Vec<Option<String>>, String> = invoke(
            &ctx,
            &format!("writer-{transport}"),
            "append_json",
            vec![
                json!(input),
                json!("open-producer"),
                json!(["\"open\""]),
                json!(false),
            ],
        )
        .await;
        assert!(appended.unwrap()[0].is_some());
        // A bounded guest read returns data before closure. Returning early as EOF
        // would lose the item; waiting for EOF would time out this invocation.
        let open: Result<Vec<String>, String> = invoke(
            &ctx,
            &format!("open-reader-{transport}"),
            "consume_json",
            vec![json!(input), json!("-1"), json!(transport), json!(1)],
        )
        .await;
        assert_eq!(open.unwrap(), ["\"open\""]);
        let head = client.head(&input).send().await.unwrap();
        assert_eq!(head.status(), reqwest::StatusCode::OK);
        assert!(!head.headers().contains_key("stream-closed"));
        let offset = head.headers()["stream-next-offset"]
            .to_str()
            .unwrap()
            .to_owned();

        let closed: Result<Vec<Option<String>>, String> = invoke(
            &ctx,
            &format!("closer-{transport}"),
            "append_json",
            vec![
                json!(input),
                json!("close-producer"),
                json!([]),
                json!(true),
            ],
        )
        .await;
        assert!(closed.unwrap()[0].is_some());
        // The limit exceeds the item count, so these calls must observe EOF.
        let eof: Result<Vec<String>, String> = invoke(
            &ctx,
            &format!("eof-reader-{transport}"),
            "consume_json",
            vec![json!(input), json!(offset), json!(transport), json!(100)],
        )
        .await;
        assert!(eof.unwrap().is_empty());
        let echoed: Result<Vec<String>, String> = invoke(
            &ctx,
            &format!("output-reader-{transport}"),
            "consume_json",
            vec![json!(output), json!("-1"), json!(transport), json!(100)],
        )
        .await;
        assert_eq!(echoed.unwrap(), ["\"open\""]);
        let metadata = client.head(&session).send().await.unwrap();
        assert_eq!(metadata.status(), reqwest::StatusCode::OK);
        assert_eq!(metadata.headers()["stream-closed"], "true");
        let manifest = client.get(&session).send().await.unwrap();
        assert_eq!(manifest.headers()["stream-closed"], "true");
        assert_eq!(manifest.json::<Value>().await.unwrap()["closed"], true);
    }

    // Forward real gateway responses unchanged. Observe a live request before
    // appending, and its data response before closing, not the initial catch-up.
    let (responses, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let gateway = origin.clone();
    let forwarder = client.clone();
    let proxy = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let responses = responses.clone();
        let gateway = gateway.clone();
        let forwarder = forwarder.clone();
        async move {
            let long_poll = request
                .uri()
                .query()
                .is_some_and(|query| query.split('&').any(|part| part == "live=long-poll"));
            if long_poll {
                let _ = responses.send(None);
            }
            let mut headers = request.headers().clone();
            headers.remove(reqwest::header::HOST);
            let response = forwarder
                .get(format!("{gateway}{}", request.uri()))
                .headers(headers)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let headers = response.headers().clone();
            let body = response.bytes().await.unwrap();
            if long_poll && status.is_success() && !headers.contains_key("stream-closed") {
                let _ = responses.send(Some((status, body.clone())));
            }
            let mut response = axum::response::Response::new(axum::body::Body::from(body));
            *response.status_mut() = status;
            *response.headers_mut() = headers;
            response
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let proxy_task = tokio::spawn(async move { axum::serve(listener, proxy).await.unwrap() });
    let path = "/durable-stream-agents/live/echo/invocations/session/streams/input";
    let input = format!("{origin}{path}");
    assert_eq!(
        client.put(&input).send().await.unwrap().status(),
        reqwest::StatusCode::CREATED
    );
    let proxy_input = format!("http://127.0.0.1:{port}{path}");
    let read = invoke::<Result<Vec<String>, String>>(
        &ctx,
        "live-reader",
        "consume_json",
        vec![
            json!(proxy_input),
            json!("-1"),
            json!("long-poll"),
            json!(100),
        ],
    );
    let write = async {
        let started = tokio::time::timeout(Duration::from_secs(30), observed.recv())
            .await
            .expect("guest did not send a long-poll request")
            .unwrap();
        assert!(started.is_none());
        let appended: Result<Vec<Option<String>>, String> = invoke(
            &ctx,
            "live-writer",
            "append_json",
            vec![
                json!(input),
                json!("live-producer"),
                json!(["\"live\""]),
                json!(false),
            ],
        )
        .await;
        assert!(appended.unwrap()[0].is_some());
        let (status, body) = tokio::time::timeout(Duration::from_secs(30), observed.recv())
            .await
            .expect("guest did not read the append through long-poll while open")
            .unwrap()
            .unwrap();
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!(["live"])
        );
        let closed: Result<Vec<Option<String>>, String> = invoke(
            &ctx,
            "live-closer",
            "append_json",
            vec![
                json!(input),
                json!("live-close-producer"),
                json!([]),
                json!(true),
            ],
        )
        .await;
        assert!(closed.unwrap()[0].is_some());
    };
    let (read, ()) = tokio::join!(read, write);
    assert_eq!(read.unwrap(), ["\"live\""]);
    proxy_task.abort();
    let _ = proxy_task.await;
}

#[test]
#[timeout("10 minutes")]
async fn external_durable_streams_reference_server_e2e() {
    let fixture =
        workspace_path().join("test-components/golem_it_external_durable_streams_release.wasm");
    assert!(
        fixture.is_file(),
        "build test-components/external-durable-streams before running this test"
    );
    let mut ctx = TestContext::new();
    fs::copy(&fixture, ctx.cwd_path_join("external-durable-streams.wasm")).unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: external-durable-streams-reference
        environments:
          local:
            server: local
        components:
          golem-it:external-durable-streams:
            componentWasm: external-durable-streams.wasm
            outputWasm: external-durable-streams-final.wasm
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    ctx.start_server().await;
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    let server = ReferenceServer::start().await;

    // The reference server JSON.parse/JSON.stringify path rounds integers above
    // Number.MAX_SAFE_INTEGER. Host codec tests cover unsafe numeric lexemes;
    // third-party roundtrips here assert exact safe-large values and nested arrays.
    let nested = "[9007199254740991,{\"left\":[1,2],\"right\":17}]";
    let final_json = "{\"tail\":9007199254740989}";
    let json_url = server.create("/json", "application/json").await;
    let append = vec![
        json!(json_url),
        json!("json-duplicate"),
        json!([nested]),
        json!(false),
    ];
    let first: Result<Vec<Option<String>>, String> =
        invoke(&ctx, "writer-json", "append_json", append.clone()).await;
    let first = first.unwrap();
    assert_eq!(first.len(), 1);
    assert!(first[0].as_ref().is_some_and(|offset| !offset.is_empty()));
    let duplicate: Result<Vec<Option<String>>, String> =
        invoke(&ctx, "writer-json-retry", "append_json", append).await;
    assert_eq!(duplicate.unwrap(), [None]);
    let closed: Result<Vec<Option<String>>, String> = invoke(
        &ctx,
        "writer-json-final",
        "append_json",
        vec![
            json!(json_url),
            json!("json-final"),
            json!([final_json]),
            json!(true),
        ],
    )
    .await;
    assert_eq!(closed.unwrap().len(), 1);
    let read: Result<Vec<String>, String> = invoke(
        &ctx,
        "reader-json",
        "consume_json",
        vec![json!(json_url), json!("-1"), json!("catch-up"), json!(100)],
    )
    .await;
    let read = read
        .unwrap()
        .into_iter()
        .map(|raw| serde_json::from_str::<Value>(&raw).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        read,
        vec![
            json!([9007199254740991u64, {"left": [1, 2], "right": 17}]),
            json!({"tail": 9007199254740989u64})
        ]
    );
    let rejected: Result<Vec<Option<String>>, String> = invoke(
        &ctx,
        "writer-json-closed",
        "append_json",
        vec![
            json!(json_url),
            json!("after-close"),
            json!(["99"]),
            json!(false),
        ],
    )
    .await;
    assert!(rejected.is_err(), "a closed stream accepted new data");

    let bytes_url = server.create("/bytes", "application/octet-stream").await;
    let appended: Result<Vec<Option<String>>, String> = invoke(
        &ctx,
        "writer-bytes",
        "append_bytes",
        vec![
            json!(bytes_url),
            json!("bytes-final"),
            json!([[0, 255, 19], [4, 8]]),
            json!(true),
        ],
    )
    .await;
    assert_eq!(appended.unwrap().len(), 2);
    let read: Result<Vec<u8>, String> = invoke(
        &ctx,
        "reader-bytes",
        "consume_bytes",
        vec![json!(bytes_url), json!("-1"), json!("catch-up"), json!(100)],
    )
    .await;
    assert_eq!(read.unwrap(), [0, 255, 19, 4, 8]);

    let empty_url = server.create("/close-only", "application/json").await;
    let closed: Result<Vec<Option<String>>, String> = invoke(
        &ctx,
        "writer-close-only",
        "append_json",
        vec![
            json!(empty_url),
            json!("close-only"),
            json!([]),
            json!(true),
        ],
    )
    .await;
    let closed = closed.unwrap();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].as_ref().is_some_and(|offset| !offset.is_empty()));
    let read: Result<Vec<String>, String> = invoke(
        &ctx,
        "reader-close-only",
        "consume_json",
        vec![json!(empty_url), json!("-1"), json!("catch-up"), json!(100)],
    )
    .await;
    assert!(read.unwrap().is_empty());

    for transport in ["long-poll", "sse"] {
        for bytes in [false, true] {
            let mode = if bytes { "bytes" } else { "json" };
            let name = format!("{mode}-{transport}");
            let path = format!("/{name}");
            let url = server
                .create(
                    &path,
                    if bytes {
                        "application/octet-stream"
                    } else {
                        "application/json"
                    },
                )
                .await;
            let producer = format!("producer-{name}");
            let write = async {
                server.wait_for_live(&path, transport).await;
                let values = if bytes {
                    json!([[0, 255, 19], [4, 8]])
                } else {
                    json!([nested, final_json])
                };
                let result: Result<Vec<Option<String>>, String> = invoke(
                    &ctx,
                    &format!("writer-{name}"),
                    if bytes { "append_bytes" } else { "append_json" },
                    vec![json!(url), json!(producer), values, json!(true)],
                )
                .await;
                assert_eq!(result.unwrap().len(), 2);
            };
            let read = async {
                let args = vec![json!(url), json!("-1"), json!(transport), json!(100)];
                if bytes {
                    let result: Result<Vec<u8>, String> =
                        invoke(&ctx, &format!("reader-{name}"), "consume_bytes", args).await;
                    assert_eq!(result.unwrap(), [0, 255, 19, 4, 8]);
                } else {
                    let result: Result<Vec<String>, String> =
                        invoke(&ctx, &format!("reader-{name}"), "consume_json", args).await;
                    assert_eq!(result.unwrap(), [nested, final_json]);
                }
            };
            tokio::join!(read, write);
        }
    }

    let requests = server.requests().await;
    let duplicate_posts = requests
        .iter()
        .filter(|r| r["producerId"] == "json-duplicate")
        .collect::<Vec<_>>();
    assert_eq!(duplicate_posts.len(), 2);
    assert_eq!(duplicate_posts[0]["status"], 200);
    assert_eq!(duplicate_posts[1]["status"], 204);
    for request in duplicate_posts {
        assert_eq!(request["producerEpoch"], "0");
        assert_eq!(request["producerSeq"], "0");
    }
    let bytes_close = requests
        .iter()
        .find(|r| r["producerId"] == "bytes-final" && r["producerSeq"] == "1")
        .unwrap();
    assert_eq!(bytes_close["closed"], "true");
    let close_only = requests
        .iter()
        .find(|r| r["producerId"] == "close-only")
        .unwrap();
    assert_eq!(close_only["producerSeq"], "0");
    assert_eq!(close_only["closed"], "true");
    assert_eq!(close_only["status"], 204);
    server.stop().await;
}
