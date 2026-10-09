use super::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use test_r::{test, timeout};

#[test]
#[timeout("15 minutes")]
async fn multipart_recovery_and_manual_update_effect() {
    recovery_and_update(
        "effect",
        "src/counter-agent.ts",
        include_str!("multipart_snapshot_effect.ts"),
    )
    .await;
}

#[test]
#[timeout("15 minutes")]
async fn multipart_recovery_and_manual_update_typescript() {
    recovery_and_update(
        "ts",
        "src/counter-agent.ts",
        include_str!("multipart_snapshot_ts.ts"),
    )
    .await;
}

#[test]
#[timeout("15 minutes")]
async fn multipart_recovery_and_manual_update_rust() {
    recovery_and_update(
        "rust",
        "src/counter_agent.rs",
        include_str!("multipart_snapshot_rust.rs"),
    )
    .await;
}

#[test]
#[timeout("15 minutes")]
async fn multipart_recovery_and_manual_update_scala() {
    recovery_and_update(
        "scala",
        "src/main/scala/MultipartIndex.scala",
        include_str!("multipart_snapshot_scala.scala"),
    )
    .await;
}

#[test]
#[timeout("15 minutes")]
async fn multipart_recovery_and_manual_update_moonbit() {
    recovery_and_update(
        "moonbit",
        "multipart_index.mbt",
        include_str!("multipart_snapshot_moonbit.mbt"),
    )
    .await;
}

async fn recovery_and_update(template: &str, path: &str, source: &str) {
    let mut ctx = TestContext::new();
    ctx.start_server().await;
    let app = format!("multipart-{template}");
    let component_name = format!("{app}:main");
    assert!(
        ctx.cli([
            flag::YES,
            cmd::NEW,
            &app,
            flag::TEMPLATE,
            template,
            flag::COMPONENT_NAME,
            &component_name
        ])
        .await
        .success_or_dump()
    );
    ctx.cd(&app);
    if template == "scala" {
        std::fs::remove_dir_all(ctx.cwd_path_join("src/main/scala")).unwrap();
        fs::create_dir_all(ctx.cwd_path_join("src/main/scala")).unwrap();
    }
    fs::write_str(ctx.cwd_path_join(path), source).unwrap();
    let preset = if matches!(template, "rust" | "moonbit") {
        "debug"
    } else {
        "quick"
    };
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: {app}
        environments:
          local:
            server: local
            componentPresets: {preset}
        components:
          {component_name}:
            templates: {template}
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    let agent = r#"MultipartIndex("索引 🦀")"#;
    for byte in 0..10 {
        assert!(
            ctx.cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                agent,
                "append",
                &byte.to_string()
            ])
            .await
            .success_or_dump()
        );
    }
    let oplog = ctx.cli([cmd::AGENT, "oplog", agent]).await;
    assert!(oplog.success_or_dump());
    assert!(oplog.stdout_contains("SNAPSHOT"));
    // These bytes must come from suffix replay, not from the automatic snapshot.
    for byte in [255, 13, 10] {
        assert!(
            ctx.cli([
                flag::YES,
                cmd::AGENT,
                cmd::INVOKE,
                agent,
                "append",
                &byte.to_string()
            ])
            .await
            .success_or_dump()
        );
    }
    // simulate-crash is a no-op for an idle worker; kill the executor process instead.
    ctx.server_process.take().unwrap().kill().await.unwrap();
    ctx.startup_ports = None;
    ctx.start_server().await;
    inspect(&ctx, agent, 0).await;

    fs::write_str(
        ctx.cwd_path_join(path),
        source.replace("revision-0", "revision-1"),
    )
    .unwrap();
    assert!(ctx.cli([cmd::DEPLOY, flag::YES]).await.success_or_dump());
    let component = ctx
        .cli([
            cmd::COMPONENT,
            cmd::GET,
            &component_name,
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(component.success_or_dump());
    let view = component
        .stdout_json::<serde_json::Value>()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        view["componentRevision"], 1,
        "must deploy a distinct revision"
    );
    assert!(
        ctx.cli([cmd::AGENT, "update", agent, "manual", "1", "--await"])
            .await
            .success_or_dump()
    );
    inspect(&ctx, agent, 1).await;
    let metadata = ctx
        .cli([cmd::AGENT, cmd::GET, agent, flag::FORMAT, "json"])
        .await;
    assert!(metadata.success_or_dump());
    let view = metadata
        .stdout_json::<serde_json::Value>()
        .into_iter()
        .next()
        .unwrap();
    assert!(
        view["metadata"]["updates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|update| update["type"] == "SuccessfulUpdate"
                && update["targetRevision"] == 1
                && update["mode"] == "manual")
    );
    ctx.server_process.take().unwrap().kill().await.unwrap();
    ctx.startup_ports = None;
    ctx.start_server().await;
    inspect(&ctx, agent, 1).await;
}

async fn inspect(ctx: &TestContext, agent: &str, revision: u32) {
    let bytes = (0..=255)
        .chain(0..10)
        .chain([255, 13, 10])
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let output = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "inspect"])
        .await;
    assert!(output.success_or_dump());
    // Only the loader sets restored=true: init + full replay cannot satisfy this.
    assert!(output.stdout_contains(format!("revision-{revision}|13|true|{bytes}")));
}
