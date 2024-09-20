use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use assert2::assert;
use golem_cloud_cli::cloud::model::text::account::GrantGetView;
use golem_cloud_cli::cloud::model::text::account::{
    AccountAddView, AccountGetView, AccountUpdateView,
};
use golem_cloud_cli::cloud::model::Role;
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
            format!("account_get_self{suffix}"),
            ctx.clone(),
            account_get_self,
        ),
        Trial::test_in_context(format!("account_add{suffix}"), ctx.clone(), account_add),
        Trial::test_in_context(
            format!("account_update{suffix}"),
            ctx.clone(),
            account_update,
        ),
        Trial::test_in_context(
            format!("account_deletee{suffix}"),
            ctx.clone(),
            account_delete,
        ),
        Trial::test_in_context(format!("account_grant{suffix}"), ctx.clone(), account_grant),
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

fn account_get_self(
    (_deps, _name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let account: AccountGetView = cli.run(&["account", "get"])?;

    assert_eq!(account.0.name, "Initial User");

    Ok(())
}

fn account_add(
    (_deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
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
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
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
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
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
    (_deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let name = format!("account grant {name}");
    let email = format!("account_grant_{name}@example.com");

    let account: AccountAddView = cli.run(&[
        "account",
        "add",
        &cfg.arg('n', "account-name"),
        &name,
        &cfg.arg('e', "account-email"),
        &email,
    ])?;

    let roles: GrantGetView = cli.run(&[
        "account",
        "grant",
        "get",
        &cfg.arg('A', "account-id"),
        &account.0.id,
    ])?;

    assert_eq!(roles.0.len(), 0);

    cli.run_unit(&[
        "account",
        &cfg.arg('A', "account-id"),
        &account.0.id,
        "grant",
        "add",
        "Admin",
    ])?;

    let roles: GrantGetView = cli.run(&[
        "account",
        "grant",
        "get",
        &cfg.arg('A', "account-id"),
        &account.0.id,
    ])?;

    assert_eq!(roles.0.len(), 1);
    assert!(roles.0.contains(&Role::Admin));

    cli.run_unit(&[
        "account",
        &cfg.arg('A', "account-id"),
        &account.0.id,
        "grant",
        "delete",
        "Admin",
    ])?;

    let roles: GrantGetView = cli.run(&[
        "account",
        "grant",
        "get",
        &cfg.arg('A', "account-id"),
        &account.0.id,
    ])?;

    assert_eq!(roles.0.len(), 0);
    assert!(!roles.0.contains(&Role::Admin));

    Ok(())
}
