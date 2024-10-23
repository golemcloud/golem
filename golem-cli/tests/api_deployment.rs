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
use crate::Tracing;
use assert2::assert;
use golem_cli::model::component::ComponentView;
use golem_client::model::{ApiDeployment, HttpApiDefinitionWithTypeInfo};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use std::sync::Arc;
use test_r::core::{DynamicTestRegistration, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &EnvBasedTestDependencies) -> CliLive {
    CliLive::make("api_deployment", Arc::new(deps.clone())).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_short", "CLI_short", true);
    make(r, "_long", "CLI_long", false);
}

fn make(r: &mut DynamicTestRegistration, suffix: &'static str, name: &'static str, short: bool) {
    add_test!(
        r,
        format!("api_deployment_deploy{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            api_deployment_deploy((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("api_deployment_get{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            api_deployment_get((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("api_deployment_list{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            api_deployment_list((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("api_deployment_delete{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            api_deployment_delete((deps, name.to_string(), cli.with_args(short)))
        }
    );
}

pub fn make_definition(
    deps: &EnvBasedTestDependencies,
    cli: &CliLive,
    component_name: &str,
) -> Result<HttpApiDefinitionWithTypeInfo, anyhow::Error> {
    let component = make_shopping_cart_component(deps, component_name, cli)?;
    let component_id = component.component_urn.id.0.to_string();
    let def = golem_def(component_name, &component_id);
    let path = make_golem_file(&def)?;

    cli.run(&["api-definition", "add", path.to_str().unwrap()])
}

fn api_deployment_deploy(
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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

    // We try an update the same component urn with a wrong wasm other than shopping-cart
    // to make it incompatible, and this shouldn't succeed!
    let component_id_in_def = definition
        .routes
        .first()
        .unwrap()
        .binding
        .component_id
        .component_id;

    // Updating the component after a deployment with incompatible changes should fail
    let component_urn = format!("urn:component:{}", component_id_in_def);
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let result: Result<ComponentView, _> = cli.run_trimmed(&[
        "component",
        "update",
        &cfg.arg('C', "component"),
        &component_urn,
        env_service.to_str().unwrap(),
    ]);

    assert!(
        result.is_err(),
        "api deployment disallows incompatible component updates"
    );
    Ok(())
}

fn api_deployment_get(
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
