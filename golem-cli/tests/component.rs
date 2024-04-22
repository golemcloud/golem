use crate::cli::{Cli, CliLive};
use golem_cli::model::component::ComponentView;
use golem_test_framework::config::TestDependencies;
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
            format!("component_add_and_find_all{suffix}"),
            ctx.clone(),
            component_add_and_find_all,
        ),
        Trial::test_in_context(
            format!("component_add_and_find_by_name{suffix}"),
            ctx.clone(),
            component_add_and_find_by_name,
        ),
        Trial::test_in_context(
            format!("component_update{suffix}"),
            ctx.clone(),
            component_update,
        ),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI short",
        CliLive::make(deps.clone()).unwrap().with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI long",
        CliLive::make(deps.clone()).unwrap().with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

fn component_add_and_find_all(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} component add and find all");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: Vec<ComponentView> = cli.run(&["component", "list"])?;
    assert!(res.contains(&component), "{res:?}.contains({component:?})");
    Ok(())
}

fn component_add_and_find_by_name(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name_other = format!("{name} component add and find by name other");
    let component_name = format!("{name} component add and find by name");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let _: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name_other,
        env_service.to_str().unwrap(),
    ])?;
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: Vec<ComponentView> = cli.run(&[
        "component",
        "list",
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    assert!(res.contains(&component), "{res:?}.contains({component:?})");
    assert_eq!(res.len(), 1, "{res:?}.len() == 1");
    Ok(())
}

fn component_update(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} component update");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let _: ComponentView = cli.run(&[
        "component",
        "update",
        &cfg.arg('C', "component-id"),
        &component.component_id,
        env_service.to_str().unwrap(),
    ])?;
    Ok(())
}
