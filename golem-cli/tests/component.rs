use crate::cli::{Cli, CliLive};
use assert2::assert;
use golem_cli::model::component::ComponentView;
use golem_common::uri::oss::url::ComponentUrl;
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
            format!("component_add_and_get{suffix}"),
            ctx.clone(),
            component_add_and_get,
        ),
        Trial::test_in_context(
            format!("component_add_and_get_urn{suffix}"),
            ctx.clone(),
            component_add_and_get_urn,
        ),
        Trial::test_in_context(
            format!("component_add_and_get_url{suffix}"),
            ctx.clone(),
            component_add_and_get_url,
        ),
        Trial::test_in_context(
            format!("component_update{suffix}"),
            ctx.clone(),
            component_update,
        ),
        Trial::test_in_context(
            format!("component_update_urn{suffix}"),
            ctx.clone(),
            component_update_urn,
        ),
        Trial::test_in_context(
            format!("component_update_url{suffix}"),
            ctx.clone(),
            component_update_url,
        ),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI short",
        CliLive::make("component_short", deps.clone())
            .unwrap()
            .with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI long",
        CliLive::make("component_long", deps.clone())
            .unwrap()
            .with_long_args(),
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
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: Vec<ComponentView> = cli.run_trimmed(&["component", "list"])?;
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
    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name_other,
        env_service.to_str().unwrap(),
    ])?;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: Vec<ComponentView> = cli.run_trimmed(&[
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
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "update",
        &cfg.arg('c', "component-name"),
        &component.component_name,
        env_service.to_str().unwrap(),
    ])?;
    Ok(())
}

fn component_update_urn(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} component update urn");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "update",
        &cfg.arg('C', "component"),
        &component.component_urn.to_string(),
        env_service.to_str().unwrap(),
    ])?;
    Ok(())
}

fn component_update_url(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} component update url");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let component_url = ComponentUrl {
        name: component.component_name.to_string(),
    };

    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "update",
        &cfg.arg('C', "component"),
        &component_url.to_string(),
        env_service.to_str().unwrap(),
    ])?;
    Ok(())
}

fn component_add_and_get(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} component add and get");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: ComponentView = cli.run_trimmed(&[
        "component",
        "get",
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    assert!(res == component, "{res:?} = ({component:?})");
    Ok(())
}

fn component_add_and_get_urn(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} component add and get urn");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let res: ComponentView = cli.run_trimmed(&[
        "component",
        "get",
        &cfg.arg('C', "component"),
        &component.component_urn.to_string(),
    ])?;
    assert!(res == component, "{res:?} = ({component:?})");
    Ok(())
}

fn component_add_and_get_url(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} component add and get url");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let component_url = ComponentUrl {
        name: component.component_name.to_string(),
    };

    let res: ComponentView = cli.run_trimmed(&[
        "component",
        "get",
        &cfg.arg('C', "component"),
        &component_url.to_string(),
    ])?;
    assert!(res == component, "{res:?} = ({component:?})");
    Ok(())
}
