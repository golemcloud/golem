use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use crate::Tracing;
use assert2::assert;
use chrono::{DateTime, FixedOffset};
use golem_cloud_cli::cloud::model::text::token::{TokenListView, UnsafeTokenView};
use test_r::core::{DynamicTestRegistration, TestProperties, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(CloudEnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &CloudEnvBasedTestDependencies) -> CliLive {
    CliLive::make("token", deps).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_short", "CLI_short", true);
    make(r, "_long", "CLI_long", false);
}

fn make(r: &mut DynamicTestRegistration, suffix: &'static str, name: &'static str, short: bool) {
    add_test!(
        r,
        format!("token_add{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            token_add((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("token_list{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            token_list((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("token_delete{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            token_delete((deps, name.to_string(), cli.with_args(short)))
        }
    );
}

fn token_add(
    (_deps, _name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let token: UnsafeTokenView =
        cli.run(&["token", "add", "--expires-at", "2050-01-01T00:00:00Z"])?;

    assert_eq!(
        token.0.data.expires_at,
        "2050-01-01T00:00:00Z".parse::<DateTime<FixedOffset>>()?
    );

    Ok(())
}

fn token_list(
    (_deps, _name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let token: UnsafeTokenView = cli.run(&["token", "add"])?;

    let tokens: TokenListView = cli.run(&["token", "list"])?;

    assert!(tokens.0.iter().any(|t| t.id == token.0.data.id));

    Ok(())
}

fn token_delete(
    (_deps, _name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let token: UnsafeTokenView = cli.run(&["token", "add"])?;

    let tokens: TokenListView = cli.run(&["token", "list"])?;

    assert!(tokens.0.iter().any(|t| t.id == token.0.data.id));

    cli.run_unit(&["token", "delete", &token.0.data.id.to_string()])?;

    let tokens: TokenListView = cli.run(&["token", "list"])?;

    assert!(tokens.0.iter().all(|t| t.id != token.0.data.id));

    Ok(())
}
