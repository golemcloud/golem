use crate::Tracing;
use crate::app::{TestContext, cmd};
use golem_client::LOCAL_WELL_KNOWN_TOKEN;
use reqwest::StatusCode;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

const ADMIN_TOKEN: &str = "server-config-test-admin-token";
const WEBHOOK_KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const SESSION_TOKEN_KEY: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";

#[cfg(unix)]
#[test]
#[timeout("10m")]
async fn server_warns_about_manifest_ports_before_successful_shutdown(_tracing: &Tracing) {
    use futures_util::StreamExt;
    use std::time::Duration;

    futures_util::stream::iter([("text", false), ("json", false), ("text", true)])
        .for_each(|(format, quiet)| async move {
            let mut ctx = TestContext::new();
            std::fs::write(
                ctx.working_dir.join("golem.yaml"),
                format!(
                    r#"
manifestVersion: {}
app: server-port-warning
environments:
  local:
    server: local
localServer:
  customRequestPort: 1
  mcpPort: 2
"#,
                    golem_cli::versions::sdk::MANIFEST,
                ),
            )
            .unwrap();
            std::fs::write(
                ctx.working_dir.join("settings.toml"),
                "[filesystem_snapshots]\nenabled = false\n\
                 [logging.stdout]\nenabled = false\n\
                 [logging.stderr]\nenabled = false\n",
            )
            .unwrap();
            ctx.add_server_args(["--local", "--format", format, "--config", "settings.toml"]);
            if quiet {
                ctx.add_server_args(["-q"]);
            }
            let stdout = ctx.working_dir.join("server.stdout").into_boxed_path();
            let stderr = ctx.working_dir.join("server.stderr").into_boxed_path();
            let mut command = tokio::process::Command::new(&ctx.golem_path);
            std::env::vars_os()
                .filter_map(|(name, _)| name.into_string().ok())
                .filter(|name| name.starts_with("GOLEM__") || name.starts_with("GOLEM_ADVANCED__"))
                .for_each(|name| {
                    command.env_remove(name);
                });
            ctx.server_process = Some(
                command
                    .args(ctx.server_run_args())
                    .current_dir(&ctx.working_dir)
                    .envs(&ctx.env)
                    .env("GOLEM_SERVER_NO_LIMIT_CHANGE", "true")
                    .env_remove("RUST_LOG")
                    .env_remove("GOLEM_LOG_FILTER")
                    .stdout(std::fs::File::create(&stdout).unwrap())
                    .stderr(std::fs::File::create(&stderr).unwrap())
                    .kill_on_drop(true)
                    .spawn()
                    .unwrap(),
            );
            ctx.wait_for_server_startup().await;
            assert_ne!(ctx.custom_request_port(), 1);
            assert_ne!(ctx.mcp_port(), 2);
            let warnings = [
                format!("port {} differs from port 1", ctx.custom_request_port()).into_boxed_str(),
                format!("port {} differs from port 2", ctx.mcp_port()).into_boxed_str(),
            ];
            let destination = if format == "text" { &stdout } else { &stderr };
            if !quiet {
                let seen_before_shutdown = futures_util::stream::iter(0..300)
                    .then(|_| async {
                        let contents = std::fs::read_to_string(destination)
                            .unwrap()
                            .into_boxed_str();
                        let seen = warnings
                            .iter()
                            .all(|warning| contents.contains(warning.as_ref()));
                        if !seen {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        seen
                    })
                    .any(std::future::ready)
                    .await;
                assert!(
                    seen_before_shutdown,
                    "missing port warning on {format} output"
                );
            }

            let process = ctx.server_process.as_mut().unwrap();
            let signal = tokio::process::Command::new("kill")
                .args(["-TERM", &process.id().unwrap().to_string()])
                .status()
                .await
                .unwrap();
            assert!(signal.success());
            let status = tokio::time::timeout(Duration::from_secs(60), process.wait())
                .await
                .expect("server did not shut down after SIGTERM")
                .unwrap();
            ctx.server_process = None;
            assert!(status.success(), "{status}");
            let stdout = std::fs::read_to_string(&stdout).unwrap().into_boxed_str();
            let stderr = std::fs::read_to_string(&stderr).unwrap().into_boxed_str();
            warnings.iter().for_each(|warning| {
                assert_eq!(
                    stdout.contains(warning.as_ref()),
                    !quiet && format == "text",
                    "{stdout}"
                );
                assert_eq!(
                    stderr.contains(warning.as_ref()),
                    !quiet && format != "text",
                    "{stderr}"
                );
            });
        })
        .await;
}

async fn current_token_status(ctx: &TestContext, token: &str) -> StatusCode {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!(
            "http://127.0.0.1:{}/v1/me/token",
            ctx.router_port()
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
}

#[test]
#[timeout("10m")]
async fn server_refuses_well_known_token_on_non_loopback_address(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    ctx.add_server_args(["--router-addr", "0.0.0.0"]);

    let output = ctx.run_server_expecting_startup_failure().await;

    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.contains("Refusing to listen on addresses that are not loopback addresses"),
        "{output}"
    );
    assert!(output.contains("router.address = 0.0.0.0"), "{output}");

    ctx.add_server_args(["-q"]);
    let output = ctx.run_server_expecting_startup_failure().await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Refusing to listen on addresses that are not loopback addresses"),
        "{stderr}"
    );
    assert!(stderr.contains("router.address = 0.0.0.0"), "{stderr}");
}

#[test]
#[timeout("10m")]
async fn unauthenticated_requests_get_json_unauthorized(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    ctx.start_server().await;

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in [
        "/v1/me/token".to_string(),
        format!(
            "/v1/runtime/envs/{}/agents/{}",
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4()
        ),
    ] {
        let response = client
            .get(format!("http://127.0.0.1:{}{path}", ctx.router_port()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["code"], "AUTH_UNAUTHORIZED", "{path}: {body}");
    }

    ctx.add_env_var("GOLEM_AUTH_TOKEN", "rotated-token");
    let output = ctx.cli([cmd::ACCOUNT, "usage", "show"]).await;

    assert!(!output.success());
    assert!(output.stderr_contains("401"), "{}", output.stderr_text());
    assert!(
        output.stderr_contains("Could not authenticate user using token (AUTH_UNAUTHORIZED)"),
        "{}",
        output.stderr_text()
    );
}

#[test]
#[timeout("10m")]
async fn configured_admin_token_replaces_well_known_token(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    let cached_metadata = std::fs::read_dir(ctx.data_dir.path().join("builtin-artifacts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            (path, bytes, modified)
        })
        .collect::<Vec<_>>();
    assert_eq!(cached_metadata.len(), 7);
    assert_eq!(
        current_token_status(&ctx, LOCAL_WELL_KNOWN_TOKEN).await,
        StatusCode::OK
    );
    ctx.stop_server().await;

    let config_file = ctx.working_dir.join("golem-server.toml");
    std::fs::write(
        &config_file,
        format!(
            r#"
                [advanced.server.router]
                address = "0.0.0.0"

                [advanced.registry_service.initial_accounts.root]
                token = "{ADMIN_TOKEN}"

                [advanced.worker_service.webhook_callback_handler]
                hmac_key = "{WEBHOOK_KEY}"

                [advanced.worker_executor.agent_webhooks_service]
                hmac_key = "{WEBHOOK_KEY}"

                [advanced.worker_service.invocation_session_tokens.active_key]
                id = "server-config-test"
                key = "{SESSION_TOKEN_KEY}"
            "#
        ),
    )
    .unwrap();
    ctx.add_server_args(["--config", config_file.to_str().unwrap()]);
    ctx.start_server().await;

    for (path, bytes, modified) in cached_metadata {
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
    }
    assert_eq!(
        current_token_status(&ctx, LOCAL_WELL_KNOWN_TOKEN).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        current_token_status(&ctx, ADMIN_TOKEN).await,
        StatusCode::OK
    );
}
