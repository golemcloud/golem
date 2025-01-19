use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use crate::Tracing;
use assert2::assert;
use golem_cloud_cli::cloud::model::text::project::{
    ProjectAddView, ProjectGetView, ProjectListView,
};
use test_r::core::{DynamicTestRegistration, TestProperties, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(CloudEnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &CloudEnvBasedTestDependencies) -> CliLive {
    CliLive::make("project", deps).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_short", "CLI_short", true);
    make(r, "_long", "CLI_long", false);
}

fn make(r: &mut DynamicTestRegistration, suffix: &'static str, name: &'static str, short: bool) {
    add_test!(
        r,
        format!("project_get_default{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            project_get_default((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("project_list{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            project_list((deps, name.to_string(), cli.with_args(short)))
        }
    );
}

fn project_get_default(
    (_deps, _name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let project: ProjectGetView = cli.run(&["project", "get-default"])?;

    assert_eq!(project.0.name, "default-project");

    Ok(())
}

fn project_list(
    (_deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let cfg = &cli.config;

    let name = format!("project list {name}");

    let projects: ProjectListView = cli.run(&["project", "list"])?;

    assert!(projects.0.iter().all(|p| p.name != name));

    let project: ProjectAddView =
        cli.run(&["project", "add", &cfg.arg('p', "project-name"), &name])?;

    assert_eq!(project.0.name, name);

    let projects: ProjectListView = cli.run(&["project", "list"])?;

    assert!(projects.0.iter().any(|p| p.name == name));

    Ok(())
}
