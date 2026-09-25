use crate::Tracing;
use crate::app::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

#[test]
#[timeout("10 minutes")]
async fn test_effect_http_router_deployed() {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    assert!(
        ctx.cli([flag::YES, cmd::NEW, "effect-http", flag::TEMPLATE, "effect"])
            .await
            .success_or_dump()
    );
    ctx.cd("effect-http");
    fs::write_str(
        ctx.cwd_path_join("src/counter-agent.ts"),
        include_str!("effect_http_router.ts"),
    )
    .unwrap();
    fs::write_str(ctx.cwd_path_join("asset.txt"), "immutable effect asset").unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: effect-http
        environments:
          local:
            server: local
            componentPresets: quick
        components:
          effect-http:main:
            templates: effect
        agents:
          EffectStatic:
            files:
              - sourcePath: ./asset.txt
                targetPath: /assets/value.txt
                permissions: read-only
        httpApi:
          deployments:
            local:
              - domain: localhost:9006
                scheme: http
                openapiEndpoint: /
                agents:
                  EffectWeb: {{}}
                  EffectRaw: {{}}
                  EffectStatic: {{}}
                  EffectCatalog: {{}}
        "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    let output = tokio::process::Command::new("node")
        .arg(crate::workspace_path().join("cli/golem-cli/tests/app/effect_http_router.mjs"))
        .arg(format!("http://localhost:{}", ctx.custom_request_port()))
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
