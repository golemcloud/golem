// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::api_definition::{golem_def, make_golem_file, make_shopping_cart_component};
use crate::cli::{Cli, CliLive};
use assert2::assert;
use golem_client::model::{ApiDeployment, HttpApiDefinitionWithTypeInfo};
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
        CliLive::make("api_deployment_short", deps.clone())
            .unwrap()
            .with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make("api_deployment_long", deps.clone())
            .unwrap()
            .with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

pub fn make_definition(
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    cli: &CliLive,
    id: &str,
) -> Result<HttpApiDefinitionWithTypeInfo, Failed> {
    let component = make_shopping_cart_component(deps, id, cli)?;
    let component_id = component.component_urn.id.0.to_string();
    let def = golem_def(id, &component_id);
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
        &cfg.arg('d', "definition"),
        &format!("{}/{}", definition.id, definition.version),
        &cfg.arg('H', "host"),
        &host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    let api_definition_info = deployment.api_definitions.first().unwrap();

    assert_eq!(deployment.site.subdomain, Some("sdomain".to_string()));
    assert_eq!(deployment.site.host, host);
    assert_eq!(api_definition_info.id, definition.id);
    assert_eq!(api_definition_info.version, definition.version);

    let updated_def: HttpApiDefinitionWithTypeInfo = cli.run(&[
        "api-definition",
        "get",
        &cfg.arg('i', "id"),
        &deployment.api_definitions.first().unwrap().id,
        &cfg.arg('V', "version"),
        &deployment.api_definitions.first().unwrap().version,
    ])?;

    assert!(definition.draft);
    assert!(!updated_def.draft, "deploy makes definition immutable");

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
        &cfg.arg('d', "definition"),
        &format!("{}/{}", definition.id, definition.version),
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
        &cfg.arg('d', "definition"),
        &format!("{}/{}", definition.id, definition.version),
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
        &cfg.arg('d', "definition"),
        &format!("{}/{}", definition.id, definition.version),
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
