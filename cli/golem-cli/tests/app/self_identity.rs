use super::{TestContext, cmd, flag, go_component_dir};
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

#[test]
#[timeout("15 minutes")]
async fn go_self_identity_follows_durable_history() {
    self_identity_follows_durable_history("go").await;
}

/// Puts the language's fixture in place of the template's agents and returns
/// the component directory relative to the application root.
fn install_fixture(ctx: &TestContext, language: &str) -> String {
    if language != "go" {
        fs::write_str(
            ctx.cwd_path_join("src/counter-agent.ts"),
            fs::read_to_string(ctx.test_data_path_join(format!("self-identity/{language}.ts")))
                .unwrap(),
        )
        .unwrap();
        return String::new();
    }
    let component = go_component_dir(ctx);
    let module = fs::read_to_string(component.join("go.mod"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("module ").map(|m| m.trim().to_string()))
        .expect("the component's go.mod names its module");
    fs::remove(component.join("agents")).unwrap();
    fs::write_str(
        component.join("identity/identity.go"),
        fs::read_to_string(ctx.test_data_path_join("self-identity/go.go")).unwrap(),
    )
    .unwrap();
    fs::write_str(
        component.join("main.go"),
        format!("package main\n\nimport _ \"{module}/identity\"\n\nfunc main() {{}}\n"),
    )
    .unwrap();
    component
        .strip_prefix(ctx.cwd_path())
        .unwrap()
        .to_string_lossy()
        .to_string()
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

/// The `status` of the agent at `address`, after checking that every way of reading its identity
/// names the agent with ID `id`.
async fn status(ctx: &TestContext, address: &str, id: &str) -> Value {
    let result = invoke(ctx, address, "status", &[]).await;
    let status: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(status["current"], id);
    assert_eq!(status["metadata"], id);
    assert_eq!(status["addressed"], id);
    status
}

/// The address of the agent of `agent_type` with ID `id`.
fn by_id(agent_type: &str, id: &str) -> String {
    format!("{agent_type}[{id}]")
}

async fn fork_cutoff(ctx: &TestContext, agent: &str) -> String {
    let output = ctx
        .cli([cmd::AGENT, "oplog", agent, flag::FORMAT, "json"])
        .await;
    assert!(output.success_or_dump());
    output
        .stdout_json::<Value>()
        .into_iter()
        .filter_map(|entry| entry["index"].as_u64())
        .max()
        .expect("the agent has durable history")
        .to_string()
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
    let component_dir = install_fixture(&ctx, language);
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
            dir: "{component_dir}"
            templates: {language}
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
    ctx.start_server().await;
    assert!(ctx.cli([flag::YES, cmd::DEPLOY]).await.success_or_dump());

    for agent_type in ["Identity", "SnapshotIdentity"] {
        let parent = format!("{agent_type}(\"parent\")");
        let parent_id = invoke(&ctx, &parent, "observe", &[]).await;
        assert!(uuid::Uuid::parse_str(&parent_id).is_ok(), "{parent_id}");
        let cutoff = fork_cutoff(&ctx, &parent).await;
        let child_id = invoke(&ctx, &parent, "forkAt", &[json!(cutoff).to_string()]).await;
        assert_ne!(child_id, parent_id);
        let child = by_id(agent_type, &child_id);
        let inherited = status(&ctx, &child, &child_id).await;
        assert_eq!(inherited["configured"], "parent");
        assert_eq!(inherited["saved"], parent_id);
        assert_eq!(inherited["trace"], json!([parent_id]));
        assert_eq!(invoke(&ctx, &child, "observe", &[]).await, child_id);
        assert_eq!(invoke(&ctx, &child, "observe", &[]).await, child_id);

        // Reconstruct both inherited parent reads and recorded child reads.
        ctx.server_process.take().unwrap().kill().await.unwrap();
        ctx.startup_ports = None;
        ctx.start_server().await;
        let restarted = status(&ctx, &child, &child_id).await;
        assert_eq!(restarted["saved"], parent_id);
        assert_eq!(restarted["trace"], json!([parent_id, child_id, child_id]));
        if agent_type == "SnapshotIdentity" {
            assert_eq!(restarted["snapshotIdentity"], child_id);
        }
        let cutoff = fork_cutoff(&ctx, &child).await;
        let grandchild_id = invoke(&ctx, &child, "forkAt", &[json!(cutoff).to_string()]).await;
        let second_generation =
            status(&ctx, &by_id(agent_type, &grandchild_id), &grandchild_id).await;
        assert_eq!(
            second_generation["trace"],
            json!([parent_id, child_id, child_id])
        );
        assert_eq!(second_generation["saved"], parent_id);
        assert_eq!(
            status(&ctx, &parent, &parent_id).await["trace"],
            json!([parent_id])
        );

        // A self-fork resumes the same invocation on both sides of the cut.
        let original: Value =
            serde_json::from_str(&invoke(&ctx, &child, "forkSelf", &[]).await).unwrap();
        assert_eq!(original["before"], child_id);
        assert_eq!(original["after"], child_id);
        assert_eq!(original["tag"], "original");
        let forked_id = original["target"].as_str().unwrap();
        let forked = status(&ctx, &by_id(agent_type, forked_id), forked_id).await;
        let transition: Value = serde_json::from_str(forked["lastFork"].as_str().unwrap()).unwrap();
        assert_eq!(transition["before"], child_id);
        assert_eq!(transition["after"], forked_id);
        assert_eq!(transition["tag"], "forked");
    }
}
