use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

#[test]
#[timeout("10 minutes")]
async fn effect_concurrent_stream_with_pending_websocket_readers() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([
            flag::YES,
            cmd::NEW,
            "stream-probe",
            flag::TEMPLATE,
            "effect"
        ])
        .await
        .success_or_dump()
    );
    ctx.cd("stream-probe");
    fs::write_str(
        ctx.cwd_path_join("src/counter-agent.ts"),
        include_str!(
            "../../../../sdks/effect/integration-test/components/concurrent-stream/src/main.ts"
        ),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("probe.mjs"),
        include_str!(
            "../../../../sdks/effect/integration-test/test-infra/concurrent-stream-probe.mjs"
        ),
    )
    .unwrap();
    let package_path = ctx.cwd_path_join("package.json");
    let mut package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&package_path).unwrap()).unwrap();
    package["devDependencies"]["ws"] = "8.18.0".into();
    fs::write_str(
        package_path,
        serde_json::to_string_pretty(&package).unwrap(),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: stream-probe
        environments:
          local:
            server: local
            componentPresets: quick
        components:
          stream-probe:main:
            templates: effect
        httpApi:
          deployments:
            local:
              - domain: localhost:9006
                agents:
                  ConcurrentStreamProbe: {{}}
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());

    // Warm the component without readers, then exercise both contended variants repeatedly.
    for (readers, runs, timeout_ms) in [
        ("0", "1", "60000"),
        ("1", "10", "10000"),
        ("2", "20", "10000"),
    ] {
        let output = tokio::process::Command::new("node")
            .args(["probe.mjs", readers, runs])
            .current_dir(ctx.cwd_path())
            .env(
                "PROBE_BASE_URL",
                format!("http://localhost:{}", ctx.custom_request_port()),
            )
            .env("PROBE_TIMEOUT_MS", timeout_ms)
            .env("PROBE_APPEND_DELAY_MS", "0")
            .env("PROBE_WAKE_AFTER_MS", "0")
            .env("PROBE_HOLD_MS", "0")
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "probe with {readers} readers failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
