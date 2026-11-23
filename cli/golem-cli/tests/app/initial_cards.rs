use crate::Tracing;
use crate::app::{Output, TestContext, cmd, flag};
use golem_cli::fs;
use golem_cli::model::invoke_result_view::InvokeResultView;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);

const AGENT_ID_WARNING: &str = "the initialCard does not allow the agent to read GOLEM_AGENT_ID, which the Golem SDKs need, so the agent fails at its first call or at snapshot load. Grant env(?agent) @ ?agent : read : GOLEM_AGENT_ID in the card, and do not exclude it in the upper bound.";
/// The 1.6.x SDKs fail with their own text when `GOLEM_AGENT_ID` is not visible: the Rust SDK with
/// "GOLEM_AGENT_ID environment variable must be set" and the TypeScript SDK with
/// "GOLEM_AGENT_ID is not set". Both name the variable.
const AGENT_ID_NOT_READABLE: &str = "GOLEM_AGENT_ID";
const FILESYSTEM_READ: &str = "filesystem(?agent) @ ?agent : read : /**";
const AGENT_ID_READ: &str = "env(?agent) @ ?agent : read : GOLEM_AGENT_ID";
const COMPONENT_CARD_RESOURCE: &str = ": read : /component-card/**";
const COMPONENT_CARD_READ: &str = "filesystem(?agent) @ ?agent : read : /component-card/**";
const AGENT_CARD_RESOURCE: &str = ": read : /agent-card/**";
const AGENT_CARD_READ: &str = "filesystem(?agent) @ ?agent : read : /agent-card/**";

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

#[test]
#[timeout("20 minutes")]
async fn component_initial_card_warns_on_deploy_and_is_installed_in_the_agents_that_inherit_it(
    _tracing: &Tracing,
) {
    let mut ctx = TestContext::new();
    let app_name = "initial-card-component";
    ctx.start_server().await;

    let output = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(output.success_or_dump());
    ctx.cd(app_name);
    let manifest = fs::read_to_string(ctx.cwd_path_join("golem.yaml")).unwrap();

    write_component_card(&ctx, &manifest, &[FILESYSTEM_READ]);
    let deploy = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deploy.success_or_dump());
    assert!(
        contains(
            &deploy,
            &format!("Agent type CounterAgent: {AGENT_ID_WARNING}")
        ),
        "deploy did not warn about the component card that CounterAgent inherits"
    );

    write_component_card(&ctx, &manifest, &[COMPONENT_CARD_READ, AGENT_ID_READ]);
    let deploy = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deploy.success_or_dump());
    assert!(
        !contains(&deploy, AGENT_ID_WARNING),
        "deploy warned about a component card that has the grant"
    );
    let grants = installed_initial_grants(&ctx, "CounterAgent(\"inherits\")").await;
    assert!(
        grants.len() == 2 && has_resource(&grants, COMPONENT_CARD_RESOURCE),
        "the agent does not hold the component card: {grants:?}"
    );

    let with_component_card = fs::read_to_string(ctx.cwd_path_join("golem.yaml")).unwrap();
    write_counter_agent_card(
        &ctx,
        &with_component_card,
        &[AGENT_CARD_READ, AGENT_ID_READ],
    );
    let deploy = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deploy.success_or_dump());
    let grants = installed_initial_grants(&ctx, "CounterAgent(\"overrides\")").await;
    assert!(
        grants.len() == 2
            && has_resource(&grants, AGENT_CARD_RESOURCE)
            && !has_resource(&grants, COMPONENT_CARD_RESOURCE),
        "the agent card does not replace the component card: {grants:?}"
    );
}

/// Calls `agent` once, so that Golem creates it, and returns the lower-bound grants of the one
/// card in its wallet.
async fn installed_initial_grants(ctx: &TestContext, agent: &str) -> Vec<String> {
    let invoke = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "increment"])
        .await;
    assert!(invoke.success_or_dump());
    let output = ctx
        .cli(["card", cmd::LIST, "--agent", agent, flag::FORMAT, "json"])
        .await;
    assert!(output.success_or_dump());
    let wallet = output
        .stdout_json::<serde_json::Value>()
        .into_iter()
        .next()
        .expect("the agent wallet list has no JSON output");
    let cards = wallet["cards"].as_array().expect("the wallet has no cards");
    assert_eq!(cards.len(), 1, "the wallet of {agent}: {wallet}");
    cards[0]["lowerPositive"]
        .as_array()
        .expect("the card has no lower bound")
        .iter()
        .map(|grant| grant.as_str().unwrap().to_string())
        .collect()
}

fn has_resource(grants: &[String], resource: &str) -> bool {
    grants
        .iter()
        .any(|grant| grant.starts_with("filesystem(") && grant.ends_with(resource))
}

#[test]
#[timeout("20 minutes")]
async fn manifest_trace_with_build_shows_the_component_initial_card_of_the_agents_that_inherit_it(
    _tracing: &Tracing,
) {
    let mut ctx = TestContext::new();
    let app_name = "initial-card-trace";
    let component_name = format!("{app_name}:rust-main");

    let output = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(output.success_or_dump());
    ctx.cd(app_name);
    let manifest = fs::read_to_string(ctx.cwd_path_join("golem.yaml")).unwrap();
    write_component_card(&ctx, &manifest, &[FILESYSTEM_READ, AGENT_ID_READ]);

    let trace = manifest_trace(&ctx, &component_name, false).await;
    assert!(
        trace["agents"]
            .as_object()
            .is_none_or(|agents| agents.is_empty()),
        "the trace of a component that is not built has agents: {trace}"
    );

    let trace = manifest_trace(&ctx, &component_name, true).await;
    let card = &trace["agents"]["CounterAgent"]["initialCard"];
    assert_eq!(
        card["value"]["lowerBound"]["positive"],
        serde_json::json!([FILESYSTEM_READ, AGENT_ID_READ]),
        "CounterAgent does not have the component card: {trace}"
    );
    assert_eq!(
        card["trace"][0]["id"],
        serde_json::json!({ "component": component_name }),
        "the trace does not name the component as the source of the card: {trace}"
    );

    // The agent traces read the final wasm, not the raw compiler output.
    let wasm_path = |property: &str| {
        let path = ctx.cwd_path_join(
            trace["properties"][property]["value"]
                .as_str()
                .unwrap_or_else(|| panic!("the trace has no {property}: {trace}")),
        );
        assert!(
            path.exists(),
            "the built {property} {path:?} does not exist"
        );
        path
    };
    let raw_wasm = wasm_path("componentWasm");
    let final_wasm = wasm_path("outputWasm");
    let moved = |path: &std::path::Path| path.with_extension("wasm.moved");

    std::fs::rename(&raw_wasm, moved(&raw_wasm)).unwrap();
    let trace = manifest_trace(&ctx, &component_name, false).await;
    assert_eq!(
        trace["agents"]["CounterAgent"]["initialCard"]["value"]["lowerBound"]["positive"],
        serde_json::json!([FILESYSTEM_READ, AGENT_ID_READ]),
        "the trace without the raw wasm does not show the agent card: {trace}"
    );
    std::fs::rename(moved(&raw_wasm), &raw_wasm).unwrap();

    std::fs::rename(&final_wasm, moved(&final_wasm)).unwrap();
    let trace = manifest_trace(&ctx, &component_name, false).await;
    assert!(
        trace["agents"]
            .as_object()
            .is_none_or(|agents| agents.is_empty()),
        "the trace without the final wasm has agents: {trace}"
    );
}

/// Returns the JSON manifest trace of the component.
async fn manifest_trace(ctx: &TestContext, component_name: &str, build: bool) -> serde_json::Value {
    let mut args = vec![
        flag::YES,
        "component",
        "manifest-trace",
        component_name,
        flag::FORMAT,
        "json",
    ];
    if build {
        args.push("--build");
    }
    let output = ctx.cli(args).await;
    assert!(output.success_or_dump());
    output
        .stdout_json::<serde_json::Value>()
        .into_iter()
        .find(|trace| trace["componentName"] == component_name)
        .unwrap_or_else(|| panic!("missing manifest trace for {component_name}"))
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
        "the call error does not name GOLEM_AGENT_ID"
    );
    let agent = ctx.cli([cmd::AGENT, cmd::GET, hidden]).await;
    assert!(agent.success_or_dump());
    assert!(
        contains(&agent, AGENT_ID_NOT_READABLE),
        "the agent error does not name GOLEM_AGENT_ID"
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

/// Writes the card into the component of the generated manifest, after its `templates` line.
fn write_component_card(ctx: &TestContext, manifest: &str, grants: &[&str]) {
    let grants = grants
        .iter()
        .map(|grant| format!("          - \"{grant}\"\n"))
        .collect::<String>();
    let card = format!("    initialCard:\n      lowerBound:\n        positive:\n{grants}");
    let mut written = String::new();
    let mut inserted = false;
    for line in manifest.lines() {
        written.push_str(line);
        written.push('\n');
        if !inserted && line.starts_with("    templates: ") {
            written.push_str(&card);
            inserted = true;
        }
    }
    assert!(
        inserted,
        "the template manifest has no component templates line"
    );
    fs::write_str(ctx.cwd_path_join("golem.yaml"), written).unwrap();
}

fn contains(output: &Output, text: &str) -> bool {
    output.stdout_contains(text) || output.stderr_contains(text)
}
