use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use chrono::{DateTime, FixedOffset};
use golem_cloud_cli::cloud::model::text::{TokenVecView, UnsafeTokenView};
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
            format!("token_add_for_another_account{suffix}"),
            ctx.clone(),
            token_add_for_another_account,
        )
        .with_ignored_flag(true),
        Trial::test_in_context(format!("token_add{suffix}"), ctx.clone(), token_add),
        Trial::test_in_context(format!("token_list{suffix}"), ctx.clone(), token_list),
        Trial::test_in_context(format!("token_delete{suffix}"), ctx.clone(), token_delete),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make("token_short", deps.clone())
            .unwrap()
            .with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make("token_long", deps.clone())
            .unwrap()
            .with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

fn token_add_for_another_account(
    (_deps, _name, _cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    todo!("Not implemented, required for tests")
}

fn token_add(
    (_deps, _name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let token: UnsafeTokenView =
        cli.run(&["token", "add", "--expires-at", "2050-01-01T00:00:00Z"])?;

    assert_eq!(
        token.0.data.expires_at,
        "2050-01-01T00:00:00Z"
            .parse::<DateTime<FixedOffset>>()
            .unwrap()
    );

    Ok(())
}

fn token_list(
    (_deps, _name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let token: UnsafeTokenView = cli.run(&["token", "add"])?;

    let tokens: TokenVecView = cli.run(&["token", "list"])?;

    assert!(tokens.0.iter().any(|t| t.id == token.0.data.id));

    Ok(())
}

fn token_delete(
    (_deps, _name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let token: UnsafeTokenView = cli.run(&["token", "add"])?;

    let tokens: TokenVecView = cli.run(&["token", "list"])?;

    assert!(tokens.0.iter().any(|t| t.id == token.0.data.id));

    cli.run_unit(&["token", "delete", &token.0.data.id.to_string()])?;

    let tokens: TokenVecView = cli.run(&["token", "list"])?;

    assert!(tokens.0.iter().all(|t| t.id != token.0.data.id));

    Ok(())
}
