use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use golem_cloud_cli::cloud::model::text::{ProjectVecView, ProjectView};
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
        Trial::test_in_context(
            format!("project_set_default{suffix}"),
            ctx.clone(),
            project_set_default,
        )
        .with_ignored_flag(true),
        Trial::test_in_context(
            format!("project_get_default{suffix}"),
            ctx.clone(),
            project_get_default,
        ),
        Trial::test_in_context(format!("project_list{suffix}"), ctx.clone(), project_list),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make("project_short", deps.clone())
            .unwrap()
            .with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make("project_long", deps.clone())
            .unwrap()
            .with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

fn project_set_default(
    (_deps, _name, _cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    todo!("Not implemented")
}

fn project_get_default(
    (_deps, _name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let project: ProjectView = cli.run(&["project", "get-default"])?;

    assert_eq!(project.name, "default-project");

    Ok(())
}

fn project_list(
    (_deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let name = format!("project list {name}");

    let projects: ProjectVecView = cli.run(&["project", "list"])?;

    assert!(projects.0.iter().all(|p| p.name != name));

    let project: ProjectView =
        cli.run(&["project", "add", &cfg.arg('p', "project-name"), &name])?;

    assert_eq!(project.name, name);

    let projects: ProjectVecView = cli.run(&["project", "list"])?;

    assert!(projects.0.iter().any(|p| p.name == name));

    Ok(())
}
