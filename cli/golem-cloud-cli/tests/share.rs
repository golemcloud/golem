use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use golem_cloud_cli::cloud::model::text::{
    AccountViewAdd, ProjectGrantView, ProjectPolicyView, ProjectView,
};
use golem_cloud_client::model::ProjectAction;
use libtest_mimic::{Failed, Trial};
use std::sync::Arc;

fn make(
    suffix: &str,
    name: &str,
    cli: CliLive,
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
) -> Vec<Trial> {
    let ctx = (deps, name.to_string(), cli);
    vec![
        Trial::test_in_context(format!("share_policy{suffix}"), ctx.clone(), share_policy),
        Trial::test_in_context(format!("share_actions{suffix}"), ctx.clone(), share_actions),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make("account_short", deps.clone())
            .unwrap()
            .with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make("account_long", deps.clone())
            .unwrap()
            .with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

fn share_policy(
    (_deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;
    let name = format!("share policy {name}");
    let email = format!("share_policy_{name}@example.com");

    let account: AccountViewAdd = cli.run(&[
        "account",
        "add",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
    ])?;

    let project: ProjectView =
        cli.run(&["project", "add", &cfg.arg('p', "project-name"), &name])?;

    let policy: ProjectPolicyView = cli.run(&[
        "project-policy",
        "add",
        "--project-policy-name",
        &name,
        "ViewComponent",
        "DeleteWorker",
    ])?;

    let res: ProjectGrantView = cli.run(&[
        "share",
        &cfg.arg('P', "project-id"),
        &project.0.project_id.to_string(),
        "--recipient-account-id",
        &account.0.id,
        "--project-policy-id",
        &policy.0.id.to_string(),
    ])?;

    assert_eq!(res.0.data.grantee_account_id, account.0.id);
    assert_eq!(res.0.data.grantor_project_id, project.0.project_id);
    assert_eq!(res.0.data.project_policy_id, policy.0.id);

    Ok(())
}

fn share_actions(
    (_deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;
    let name = format!("share policy {name}");
    let email = format!("share_policy_{name}@example.com");

    let account: AccountViewAdd = cli.run(&[
        "account",
        "add",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
    ])?;

    let project: ProjectView =
        cli.run(&["project", "add", &cfg.arg('p', "project-name"), &name])?;

    let res: ProjectGrantView = cli.run(&[
        "share",
        &cfg.arg('P', "project-id"),
        &project.0.project_id.to_string(),
        "--recipient-account-id",
        &account.0.id,
        &cfg.arg('A', "project-actions"),
        "UpdateWorker",
        &cfg.arg('A', "project-actions"),
        "DeleteComponent",
    ])?;

    assert_eq!(res.0.data.grantee_account_id, account.0.id);
    assert_eq!(res.0.data.grantor_project_id, project.0.project_id);

    let policy: ProjectPolicyView = cli.run(&[
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
