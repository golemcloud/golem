use crate::Tracing;
use crate::app::{Output, TestContext, cmd, flag};
use golem_cli::fs;
use golem_cli::model::invoke_result_view::InvokeResultView;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

const AGENT_ID_WARNING: &str = "the initialCard does not allow the agent to read GOLEM_AGENT_ID, which the Golem SDKs need, so the agent fails at its first call or at snapshot load. Grant env(?agent) @ ?agent : read : GOLEM_AGENT_ID in the card, and do not exclude it in the upper bound.";
const AGENT_ID_NOT_READABLE: &str = "GOLEM_AGENT_ID is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_ID";
const FILESYSTEM_READ: &str = "filesystem(?agent) @ ?agent : read : /**";
const AGENT_ID_READ: &str = "env(?agent) @ ?agent : read : GOLEM_AGENT_ID";

#[test]
#[timeout("20 minutes")]
async fn rust_initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call(
    _tracing: &Tracing,
) {
    initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call(
        "rust",
        "Rust syntax",
    )
    .await;
}

#[test]
#[timeout("20 minutes")]
async fn typescript_initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call(
    _tracing: &Tracing,
) {
    initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call(
        "ts",
        "TypeScript syntax",
    )
    .await;
}

async fn initial_card_without_agent_id_read_warns_on_deploy_and_fails_at_first_call(
    template: &str,
    result_format: &str,
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
        contains(
            &deploy,
            &format!("Agent type CounterAgent: {AGENT_ID_WARNING}")
        ),
        "deploy did not warn about the card of CounterAgent"
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
        .cli([
            flag::YES,
            cmd::AGENT,
            cmd::INVOKE,
            shown,
            "increment",
            "--no-stream",
            flag::FORMAT,
            "json",
        ])
        .await;
    assert!(invoke.success_or_dump());
    let results: Vec<InvokeResultView> = invoke.stdout_json();
    assert_eq!(results.len(), 1, "expected one invocation result");
    assert_eq!(results[0].result.as_deref(), Some("1"));
    assert_eq!(results[0].result_format.as_deref(), Some(result_format));
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
