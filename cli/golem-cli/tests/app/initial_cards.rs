use crate::Tracing;
use crate::app::{Output, TestContext, cmd, flag};
use golem_cli::fs;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

const AGENT_ID_WARNING: &str = "the initialCard does not allow the agent to read GOLEM_AGENT_ID, which the Golem SDKs need, so the agent fails at its first call or at snapshot load. Add env(?agent) @ ?agent : read : GOLEM_AGENT_ID to the card.";
const AGENT_ID_NOT_READABLE: &str = "GOLEM_AGENT_ID is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_ID";
const FILESYSTEM_READ: &str = "filesystem(?agent) @ ?agent : read : /**";
const AGENT_ID_READ: &str = "env(?agent) @ ?agent : read : GOLEM_AGENT_ID";

#[test]
#[timeout("20 minutes")]
async fn rust_initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call(
    _tracing: &Tracing,
) {
    initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call("rust").await;
}

#[test]
#[timeout("20 minutes")]
async fn typescript_initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call(
    _tracing: &Tracing,
) {
    initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call("ts").await;
}

async fn initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call(
    template: &str,
) {
    let mut ctx = TestContext::new();
    let app_name = format!("initial-card-{template}");
    ctx.start_server().await;

    let output = ctx
        .cli([flag::YES, cmd::NEW, &app_name, flag::TEMPLATE, template])
        .await;
    assert!(output.success_or_dump());
    ctx.cd(&app_name);
    let manifest = fs::read_to_string(ctx.cwd_path_join("golem.yaml")).unwrap();
    assert!(
        !manifest.lines().any(|line| line.starts_with("agents:")),
        "the template manifest already declares agents"
    );

    write_counter_agent_card(&ctx, &manifest, &[FILESYSTEM_READ]);
    let deploy = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deploy.success_or_dump());
    assert!(
        contains(&deploy, AGENT_ID_WARNING),
        "deploy did not warn about the card"
    );

    let hidden = "CounterAgent(\"hidden\")";
    let invoke = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, hidden, "increment"])
        .await;
    assert!(!invoke.success(), "the first call succeeded");
    assert!(
        contains(&invoke, AGENT_ID_NOT_READABLE),
        "the call error does not name the card grant"
    );
    let agent = ctx.cli([cmd::AGENT, cmd::GET, hidden]).await;
    assert!(agent.success_or_dump());
    assert!(
        contains(&agent, AGENT_ID_NOT_READABLE),
        "the agent error does not name the card grant"
    );

    write_counter_agent_card(&ctx, &manifest, &[FILESYSTEM_READ, AGENT_ID_READ]);
    let deploy = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deploy.success_or_dump());
    assert!(
        !contains(&deploy, AGENT_ID_WARNING),
        "deploy warned about a card that has the grant"
    );

    let shown = "CounterAgent(\"shown\")";
    let invoke = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, shown, "increment"])
        .await;
    assert!(invoke.success_or_dump());
    assert!(invoke.stdout_contains("1"));
}

fn write_counter_agent_card(ctx: &TestContext, manifest: &str, grants: &[&str]) {
    let grants = grants
        .iter()
        .map(|grant| format!("          - \"{grant}\"\n"))
        .collect::<String>();
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        format!(
            "{manifest}\nagents:\n  CounterAgent:\n    initialCard:\n      lowerBound:\n        positive:\n{grants}"
        ),
    )
    .unwrap();
}

fn contains(output: &Output, text: &str) -> bool {
    output.stdout_contains(text) || output.stderr_contains(text)
}
