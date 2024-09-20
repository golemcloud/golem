use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use assert2::assert;
use golem_cloud_cli::cloud::model::text::project::{ProjectPolicyAddView, ProjectPolicyGetView};
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
        Trial::test_in_context(format!("policy_add{suffix}"), ctx.clone(), policy_add),
        Trial::test_in_context(format!("policy_get{suffix}"), ctx.clone(), policy_get),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make("policy_short", deps.clone())
            .unwrap()
            .with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make("policy_long", deps.clone())
            .unwrap()
            .with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

fn policy_add(
    (_deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
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
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
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
