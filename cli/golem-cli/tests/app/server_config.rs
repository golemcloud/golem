use crate::Tracing;
use crate::app::TestContext;
use golem_client::LOCAL_WELL_KNOWN_TOKEN;
use reqwest::StatusCode;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

const ADMIN_TOKEN: &str = "server-config-test-admin-token";
const WEBHOOK_KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const SESSION_TOKEN_KEY: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";

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
    for variable in [
        "GOLEM__REGISTRY_SERVICE__INITIAL_ACCOUNTS__ROOT__TOKEN",
        "GOLEM__WORKER_SERVICE__WEBHOOK_CALLBACK_HANDLER__HMAC_KEY",
        "GOLEM__WORKER_EXECUTOR__AGENT_WEBHOOKS_SERVICE__HMAC_KEY",
        "GOLEM__WORKER_SERVICE__INVOCATION_SESSION_TOKENS__ACTIVE_KEY__KEY",
    ] {
        assert!(output.contains(variable), "{output}");
    }
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
    assert_eq!(cached_metadata.len(), 6);
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
                [router]
                address = "0.0.0.0"

                [registry_service.initial_accounts.root]
                token = "{ADMIN_TOKEN}"

                [worker_service.webhook_callback_handler]
                hmac_key = "{WEBHOOK_KEY}"

                [worker_executor.agent_webhooks_service]
                hmac_key = "{WEBHOOK_KEY}"

                [worker_service.invocation_session_tokens.active_key]
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

#[test]
#[timeout("10m")]
async fn server_rejects_stale_and_launcher_owned_settings(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    ctx.add_env_var("GOLEM__RESOURCE_USAGE_METERING__COMPUTE", "true");

    let output = ctx.run_server_expecting_startup_failure().await;
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.contains("resource_usage_metering"), "{output}");

    let mut ctx = TestContext::new();
    ctx.add_env_var("GOLEM__WORKER_EXECUTOR__GRPC__PORT", "9000");

    let output = ctx.run_server_expecting_startup_failure().await;
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.contains("worker_executor.grpc.port"), "{output}");
}
