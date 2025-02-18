use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use crate::Tracing;
use assert2::assert;
use golem_cloud_cli::cloud::model::text::account::{
    AccountAddView, AccountGetView, AccountUpdateView,
};
use test_r::core::{DynamicTestRegistration, TestProperties, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(CloudEnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &CloudEnvBasedTestDependencies) -> CliLive {
    CliLive::make("account", deps).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_short", "CLI_short", true);
    make(r, "_long", "CLI_long", false);
}

fn make(r: &mut DynamicTestRegistration, suffix: &'static str, name: &'static str, short: bool) {
    add_test!(
        r,
        format!("account_get_self{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            account_get_self((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("account_add{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            account_add((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("account_update{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            account_update((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("account_delete{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            account_delete((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("account_grant{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            account_grant((deps, name.to_string(), cli.with_args(short)))
        }
    );
}

fn account_get_self(
    (_deps, _name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let account: AccountGetView = cli.run(&["account", "get"])?;

    assert_eq!(account.0.name, "Initial User");

    Ok(())
}

fn account_add(
    (_deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let cfg = &cli.config;

    let name = format!("account add {name}");
    let email = format!("account_add_{name}@example.com");

    let account: AccountAddView = cli.run(&[
        "account",
        "add",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
    ])?;

    assert_eq!(account.0.name, name);
    assert_eq!(account.0.email, email);

    let account: AccountGetView =
        cli.run(&["account", "get", &cfg.arg('A', "account-id"), &account.0.id])?;

    assert_eq!(account.0.name, name);
    assert_eq!(account.0.email, email);

    Ok(())
}

fn account_update(
    (_deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let cfg = &cli.config;

    let name = format!("account update init {name}");
    let email = format!("account_update_init_{name}@example.com");

    let account: AccountAddView = cli.run(&[
        "account",
        "add",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
    ])?;

    assert_eq!(account.0.name, name);
    assert_eq!(account.0.email, email);

    let name = format!("account update new {name}");
    let email = format!("account_update_new_{name}@example.com");

    let account: AccountUpdateView = cli.run(&[
        "account",
        "update",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
        &cfg.arg('A', "account-id"),
        &account.0.id,
    ])?;

    assert_eq!(account.0.name, name);
    assert_eq!(account.0.email, email);

    let account: AccountGetView =
        cli.run(&["account", "get", &cfg.arg('A', "account-id"), &account.0.id])?;

    assert_eq!(account.0.name, name);
    assert_eq!(account.0.email, email);

    Ok(())
}

fn account_delete(
    (_deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let cfg = &cli.config;

    let name = format!("account delete {name}");
    let email = format!("account_delete_{name}@example.com");

    let account: AccountAddView = cli.run(&[
        "account",
        "add",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
    ])?;

    assert_eq!(account.0.name, name);
    assert_eq!(account.0.email, email);

    cli.run_unit(&[
        "account",
        "delete",
        &cfg.arg('A', "account-id"),
        &account.0.id,
    ])?;

    let get_res = cli.run_unit(&["account", "get", &cfg.arg('A', "account-id"), &account.0.id]);

    assert!(get_res.is_err());

    Ok(())
}

fn account_grant(
    (_deps, _name, _cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    // TODO: Fix this test. Currently it just tests that admin can use the CLI, and nothing else

    // let cfg = &cli.config;

    // let name = format!("account grant {name}");
    // let email = format!("account_grant_{name}@example.com");

    // let account: AccountAddView = cli.run(&[
    //     "account",
    //     "add",
    //     &cfg.arg('n', "account-name"),
    //     &name,
    //     &cfg.arg('e', "account-email"),
    //     &email,
    // ])?;

    // let roles: GrantGetView = cli.run(&[
    //     "account",
    //     "grant",
    //     "get",
    //     &cfg.arg('A', "account-id"),
    //     &account.0.id,
    // ])?;

    // assert_eq!(roles.0.len(), 7);

    // cli.run_unit(&[
    //     "account",
    //     &cfg.arg('A', "account-id"),
    //     &account.0.id,
    //     "grant",
    //     "add",
    //     "Admin",
    // ])?;

    // let roles: GrantGetView = cli.run(&[
    //     "account",
    //     "grant",
    //     "get",
    //     &cfg.arg('A', "account-id"),
    //     &account.0.id,
    // ])?;

    // assert_eq!(roles.0.len(), 1);
    // assert!(roles.0.contains(&Role::Admin));

    // cli.run_unit(&[
    //     "account",
    //     &cfg.arg('A', "account-id"),
    //     &account.0.id,
    //     "grant",
    //     "delete",
    //     "Admin",
    // ])?;

    // let roles: GrantGetView = cli.run(&[
    //     "account",
    //     "grant",
    //     "get",
    //     &cfg.arg('A', "account-id"),
    //     &account.0.id,
    // ])?;

    // assert_eq!(roles.0.len(), 0);
    // assert!(!roles.0.contains(&Role::Admin));

    Ok(())
}
