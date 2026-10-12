use super::{TestContext, cmd, flag};
use golem_cli::{fs, versions};
use indoc::formatdoc;
use serde_json::{Value, json};
use test_r::{test, timeout};

#[test]
#[timeout("15 minutes")]
async fn typescript_self_identity_follows_durable_history() {
    self_identity_follows_durable_history("ts").await;
}

#[test]
#[timeout("15 minutes")]
async fn effect_self_identity_follows_durable_history() {
    self_identity_follows_durable_history("effect").await;
}

async fn invoke(ctx: &TestContext, agent: &str, method: &str, args: &[String]) -> String {
    let mut command = vec![cmd::AGENT, cmd::INVOKE, agent, method];
    command.extend(args.iter().map(String::as_str));
    command.extend([flag::FORMAT, "json", "--no-stream"]);
    let output = ctx.cli(command).await;
    assert!(output.success_or_dump());
    let events = output.stdout_json::<Value>();
    let result = events.iter().find(|event| event["$type"] == "agent.invoke");
    result
        .and_then(|result| result["resultJson"]["value"]["value"].as_str())
        .unwrap_or("")
        .to_string()
}

async fn status(ctx: &TestContext, agent: &str) -> Value {
    let result = invoke(ctx, agent, "status", &[]).await;
    let status: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(status["current"], agent);
    assert_eq!(status["metadata"], agent);
    assert_eq!(status["addressed"], agent);
    status
}

async fn self_identity_follows_durable_history(language: &str) {
    let mut ctx = TestContext::new();
    let app = format!("self-identity-{language}");
    let component = format!("{app}:probe");
    let new = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            &app,
            flag::TEMPLATE,
            language,
            flag::COMPONENT_NAME,
            &component,
        ])
        .await;
    assert!(new.success_or_dump());
    ctx.cd(&app);
    fs::write_str(
        ctx.cwd_path_join("src/counter-agent.ts"),
        fs::read_to_string(ctx.test_data_path_join(format!("self-identity/{language}.ts")))
            .unwrap(),
    )
    .unwrap();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: {app}
        environments:
          local:
            server: local
        components:
          {component}:
            dir: ""
            templates: {language}
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
    ctx.start_server().await;
    assert!(ctx.cli([flag::YES, cmd::DEPLOY]).await.success_or_dump());

    for agent_type in ["Identity", "SnapshotIdentity"] {
        let parent = format!("{agent_type}(\"parent\")");
        let child = format!("{agent_type}(\"child\")");
        let grandchild = format!("{agent_type}(\"grandchild\")");
        assert_eq!(invoke(&ctx, &parent, "observe", &[]).await, parent);
        let cutoff = invoke(&ctx, &parent, "checkpoint", &[]).await;
        invoke(
            &ctx,
            &parent,
            "forkNamed",
            &[json!("child").to_string(), json!(cutoff).to_string()],
        )
        .await;
        let inherited = status(&ctx, &child).await;
        assert_eq!(inherited["configured"], "parent");
        assert_eq!(inherited["saved"], parent);
        assert_eq!(inherited["trace"], json!([parent]));
        assert_eq!(invoke(&ctx, &child, "observe", &[]).await, child);
        assert_eq!(invoke(&ctx, &child, "observe", &[]).await, child);

        // Reconstruct both inherited parent reads and recorded child reads.
        ctx.server_process.take().unwrap().kill().await.unwrap();
        ctx.startup_ports = None;
        ctx.start_server().await;
        let restarted = status(&ctx, &child).await;
        assert_eq!(restarted["saved"], parent);
        assert_eq!(restarted["trace"], json!([parent, child, child]));
        if agent_type == "SnapshotIdentity" {
            assert_eq!(restarted["snapshotIdentity"], child);
        }
        let cutoff = invoke(&ctx, &child, "checkpoint", &[]).await;
        invoke(
            &ctx,
            &child,
            "forkNamed",
            &[json!("grandchild").to_string(), json!(cutoff).to_string()],
        )
        .await;
        let second_generation = status(&ctx, &grandchild).await;
        assert_eq!(second_generation["trace"], json!([parent, child, child]));
        assert_eq!(second_generation["saved"], parent);
        assert_eq!(status(&ctx, &parent).await["trace"], json!([parent]));

        // A self-fork resumes the same invocation on both sides of the cut.
        let original: Value =
            serde_json::from_str(&invoke(&ctx, &child, "forkSelf", &[]).await).unwrap();
        assert_eq!(original["before"], child);
        assert_eq!(original["after"], child);
        assert_eq!(original["tag"], "original");
        let phantom = original["target"].as_str().unwrap();
        let forked = status(&ctx, phantom).await;
        let transition: Value = serde_json::from_str(forked["lastFork"].as_str().unwrap()).unwrap();
        assert_eq!(transition["before"], child);
        assert_eq!(transition["after"], phantom);
        assert_eq!(transition["tag"], "forked");
        if language == "ts" {
            assert!(transition["phantom"].as_str().is_some());
        }
    }
}
