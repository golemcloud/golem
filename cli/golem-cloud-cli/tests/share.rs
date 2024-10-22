use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use crate::Tracing;
use assert2::assert;
use golem_cloud_cli::cloud::model::text::account::AccountAddView;
use golem_cloud_cli::cloud::model::text::project::{
    ProjectAddView, ProjectPolicyAddView, ProjectPolicyGetView, ProjectShareView,
};
use golem_cloud_client::model::ProjectAction;
use test_r::core::{DynamicTestRegistration, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(CloudEnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &CloudEnvBasedTestDependencies) -> CliLive {
    CliLive::make("share", deps).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_short", "CLI_short", true);
    make(r, "_long", "CLI_long", false);
}

fn make(r: &mut DynamicTestRegistration, suffix: &'static str, name: &'static str, short: bool) {
    add_test!(
        r,
        format!("share_policy{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            share_policy((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("share_actions{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            share_actions((deps, name.to_string(), cli.with_args(short)))
        }
    );
}

fn share_policy(
    (_deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let cfg = &cli.config;
    let name = format!("share policy {name}");
    let email = format!("share_policy_{name}@example.com");

    let account: AccountAddView = cli.run(&[
        "account",
        "add",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
    ])?;

    let project: ProjectAddView =
        cli.run(&["project", "add", &cfg.arg('p', "project-name"), &name])?;

    let policy: ProjectPolicyAddView = cli.run(&[
        "project-policy",
        "add",
        "--project-policy-name",
        &name,
        "ViewComponent",
        "DeleteWorker",
    ])?;

    let res: ProjectShareView = cli.run(&[
        "share",
        &cfg.arg('P', "project"),
        &project.0.project_urn.to_string(),
        "--recipient-account-id",
        &account.0.id,
        "--project-policy-id",
        &policy.0.id.to_string(),
    ])?;

    assert_eq!(res.0.data.grantee_account_id, account.0.id);
    assert_eq!(res.0.data.grantor_project_id, project.0.project_urn.id.0);
    assert_eq!(res.0.data.project_policy_id, policy.0.id);

    Ok(())
}

fn share_actions(
    (_deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let cfg = &cli.config;
    let name = format!("share policy {name}");
    let email = format!("share_policy_{name}@example.com");

    let account: AccountAddView = cli.run(&[
        "account",
        "add",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
    ])?;

    let project: ProjectAddView =
        cli.run(&["project", "add", &cfg.arg('p', "project-name"), &name])?;

    let res: ProjectShareView = cli.run(&[
        "share",
        &cfg.arg('P', "project"),
        &project.0.project_urn.to_string(),
        "--recipient-account-id",
        &account.0.id,
        &cfg.arg('A', "project-actions"),
        "UpdateWorker",
        &cfg.arg('A', "project-actions"),
        "DeleteComponent",
    ])?;

    assert_eq!(res.0.data.grantee_account_id, account.0.id);
    assert_eq!(res.0.data.grantor_project_id, project.0.project_urn.id.0);

    let policy: ProjectPolicyGetView = cli.run(&[
        "project-policy",
        "get",
        &res.0.data.project_policy_id.to_string(),
    ])?;

    assert_eq!(policy.0.project_actions.actions.len(), 2);
    assert!(policy
        .0
        .project_actions
        .actions
        .contains(&ProjectAction::UpdateWorker));
    assert!(policy
        .0
        .project_actions
        .actions
        .contains(&ProjectAction::DeleteComponent));

    Ok(())
}
