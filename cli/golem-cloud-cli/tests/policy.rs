use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use crate::Tracing;
use assert2::assert;
use golem_cloud_cli::cloud::model::text::project::{ProjectPolicyAddView, ProjectPolicyGetView};
use golem_cloud_client::model::ProjectAction;
use test_r::core::{DynamicTestRegistration, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(CloudEnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &CloudEnvBasedTestDependencies) -> CliLive {
    CliLive::make("policy", deps).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_short", "CLI_short", true);
    make(r, "_long", "CLI_long", false);
}

fn make(r: &mut DynamicTestRegistration, suffix: &'static str, name: &'static str, short: bool) {
    add_test!(
        r,
        format!("policy_add{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            policy_add((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("policy_get{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            policy_get((deps, name.to_string(), cli.with_args(short)))
        }
    );
}

fn policy_add(
    (_deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let name = format!("policy add {name}");

    let policy: ProjectPolicyAddView = cli.run(&[
        "project-policy",
        "add",
        "--project-policy-name",
        &name,
        "ViewComponent",
        "DeleteWorker",
    ])?;

    assert_eq!(policy.0.name, name);
    assert!(policy
        .0
        .project_actions
        .actions
        .contains(&ProjectAction::ViewComponent));
    assert!(policy
        .0
        .project_actions
        .actions
        .contains(&ProjectAction::DeleteWorker));

    Ok(())
}

fn policy_get(
    (_deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let name = format!("policy list {name}");

    let policy: ProjectPolicyAddView = cli.run(&[
        "project-policy",
        "add",
        "--project-policy-name",
        &name,
        "ViewComponent",
        "DeleteWorker",
    ])?;

    let policy: ProjectPolicyGetView =
        cli.run(&["project-policy", "get", &policy.0.id.to_string()])?;

    assert_eq!(policy.0.name, name);
    assert_eq!(policy.0.project_actions.actions.len(), 2);
    assert!(policy
        .0
        .project_actions
        .actions
        .contains(&ProjectAction::ViewComponent));
    assert!(policy
        .0
        .project_actions
        .actions
        .contains(&ProjectAction::DeleteWorker));

    Ok(())
}
