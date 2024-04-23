use crate::api_definition::{golem_def, make_golem_file, make_shopping_cart_component};
use crate::cli::{Cli, CliLive};
use golem_client::model::{ApiDeployment, HttpApiDefinition};
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
            format!("api_deployment_deploy{suffix}"),
            ctx.clone(),
            api_deployment_deploy,
        ),
        Trial::test_in_context(
            format!("api_deployment_get{suffix}"),
            ctx.clone(),
            api_deployment_get,
        ),
        Trial::test_in_context(
            format!("api_deployment_list{suffix}"),
            ctx.clone(),
            api_deployment_list,
        ),
        Trial::test_in_context(
            format!("api_deployment_delete{suffix}"),
            ctx.clone(),
            api_deployment_delete,
        ),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make(deps.clone()).unwrap().with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make(deps.clone()).unwrap().with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

pub fn make_definition(
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    cli: &CliLive,
    id: &str,
) -> Result<HttpApiDefinition, Failed> {
    let component = make_shopping_cart_component(deps, id, cli)?;
    let def = golem_def(id, &component.component_id);
    let path = make_golem_file(&def)?;

    cli.run(&["api-definition", "add", path.to_str().unwrap()])
}

fn api_deployment_deploy(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let definition = make_definition(deps, &cli, &format!("api_deployment_deploy{name}"))?;
    let host = format!("deploy-host{name}");
    let cfg = &cli.config;

    let deployment: ApiDeployment = cli.run(&[
        "api-deployment",
        "deploy",
        &cfg.arg('i', "id"),
        &definition.id,
        &cfg.arg('V', "version"),
        &definition.version,
        &cfg.arg('H', "host"),
        &host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    assert_eq!(deployment.site.subdomain, "sdomain");
    assert_eq!(deployment.site.host, host);
    assert_eq!(deployment.api_definition_id, definition.id);
    assert_eq!(deployment.version, definition.version);

    Ok(())
}

fn api_deployment_get(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let definition = make_definition(deps, &cli, &format!("api_deployment_get{name}"))?;
    let host = format!("get-host{name}");
    let cfg = &cli.config;

    let created: ApiDeployment = cli.run(&[
        "api-deployment",
        "deploy",
        &cfg.arg('i', "id"),
        &definition.id,
        &cfg.arg('V', "version"),
        &definition.version,
        &cfg.arg('H', "host"),
        &host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    let res: ApiDeployment = cli.run(&["api-deployment", "get", &format!("sdomain.{host}")])?;

    assert_eq!(created, res);

    Ok(())
}

fn api_deployment_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let definition = make_definition(deps, &cli, &format!("api_deployment_list{name}"))?;
    let host = format!("list-host{name}");
    let cfg = &cli.config;

    let created: ApiDeployment = cli.run(&[
        "api-deployment",
        "deploy",
        &cfg.arg('i', "id"),
        &definition.id,
        &cfg.arg('V', "version"),
        &definition.version,
        &cfg.arg('H', "host"),
        &host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    let res: Vec<ApiDeployment> = cli.run(&[
        "api-deployment",
        "list",
        &cfg.arg('i', "id"),
        &definition.id,
    ])?;

    assert_eq!(res.len(), 1);
    assert_eq!(*res.first().unwrap(), created);

    Ok(())
}

fn api_deployment_delete(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let definition = make_definition(deps, &cli, &format!("api_deployment_delete{name}"))?;
    let host = format!("delete-host{name}");
    let cfg = &cli.config;

    let _: ApiDeployment = cli.run(&[
        "api-deployment",
        "deploy",
        &cfg.arg('i', "id"),
        &definition.id,
        &cfg.arg('V', "version"),
        &definition.version,
        &cfg.arg('H', "host"),
        &host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    cli.run_unit(&["api-deployment", "delete", &format!("sdomain.{host}")])?;

    let res: Vec<ApiDeployment> = cli.run(&[
        "api-deployment",
        "list",
        &cfg.arg('i', "id"),
        &definition.id,
    ])?;

    assert_eq!(res.len(), 0);

    Ok(())
}
