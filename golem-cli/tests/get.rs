// Copyright 2024-2025 Golem Cloud
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

use test_r::{inherit_test_dep, test, test_dep};

use crate::api_definition::{
    make_json_file, make_shopping_cart_component, native_api_definition_request,
    to_api_definition_with_type_info,
};
use crate::api_deployment::make_definition;
use crate::cli::{Cli, CliLive};
use crate::worker::add_environment_service_component;
use crate::Tracing;
use golem_cli::model::component::ComponentView;
use golem_cli::model::WorkerMetadataView;
use golem_client::model::{ApiDeployment, HttpApiDefinitionResponseData, RibOutputTypeInfo};
use golem_common::uri::oss::url::{
    ApiDefinitionUrl, ApiDeploymentUrl, ComponentUrl, WorkerFunctionUrl, WorkerUrl,
};
use golem_common::uri::oss::urn::{
    ApiDefinitionUrn, ApiDeploymentUrn, WorkerFunctionUrn, WorkerUrn,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_wasm_ast::analysis::analysed_type::{record, str, u64};
use golem_wasm_ast::analysis::NameTypePair;
use std::sync::Arc;

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &EnvBasedTestDependencies) -> CliLive {
    CliLive::make("gateway_api_definition", Arc::new(deps.clone()))
        .unwrap()
        .with_long_args()
}

#[test]
fn top_level_get_api_definition(
    deps: &EnvBasedTestDependencies,
    cli: &CliLive,
    _tracing: &Tracing,
) -> Result<(), anyhow::Error> {
    let component_name = "top_level_get_api_definition";
    let component = make_shopping_cart_component(deps, component_name, cli)?;
    let component_id = component.component_urn.id.0.to_string();
    let path = "/{user-id}/get-cart-contents";
    let def = native_api_definition_request(component_name, &component_id, None, path);
    let path = make_json_file(&def.id, &def)?;

    let _: HttpApiDefinitionResponseData =
        cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let url = ApiDefinitionUrl {
        name: component_name.to_string(),
        version: "0.1.0".to_string(),
    };

    let res: HttpApiDefinitionResponseData = cli.run(&["get", &url.to_string()])?;

    let rib_output_type_info = RibOutputTypeInfo {
        analysed_type: record(vec![
            NameTypePair {
                name: "body".to_string(),
                typ: str(),
            },
            NameTypePair {
                name: "headers".to_string(),
                typ: record(vec![
                    NameTypePair {
                        name: "ContentType".to_string(),
                        typ: str(),
                    },
                    NameTypePair {
                        name: "userid".to_string(),
                        typ: str(),
                    },
                ]),
            },
            NameTypePair {
                name: "status".to_string(),
                typ: u64(),
            },
        ]),
    };

    let expected =
        to_api_definition_with_type_info(def.clone(), res.created_at, rib_output_type_info.clone());
    assert_eq!(res, expected);

    let urn = ApiDefinitionUrn {
        id: component_name.to_string(),
        version: "0.1.0".to_string(),
    };

    let res: HttpApiDefinitionResponseData = cli.run(&["get", &urn.to_string()])?;
    let expected =
        to_api_definition_with_type_info(def.clone(), res.created_at, rib_output_type_info);
    assert_eq!(res, expected);

    Ok(())
}

#[test]
fn top_level_get_api_deployment(
    deps: &EnvBasedTestDependencies,
    cli: &CliLive,
    _tracing: &Tracing,
) -> Result<(), anyhow::Error> {
    let path = "/{user-id}/get-cart-contents";
    let definition = make_definition(deps, cli, "top_level_get_api_deployment", None, path)?;
    let host = "get-host-top-level-get";
    let cfg = &cli.config;

    let created: ApiDeployment = cli.run(&[
        "api-deployment",
        "deploy",
        &cfg.arg('d', "definition"),
        &format!("{}/{}", definition.id, definition.version),
        &cfg.arg('H', "host"),
        host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    let site = format!("sdomain.{host}");

    let url = ApiDeploymentUrl { site: site.clone() };

    let res: ApiDeployment = cli.run(&["get", &url.to_string()])?;

    assert_eq!(created, res);

    let urn = ApiDeploymentUrn { site: site.clone() };

    let res: ApiDeployment = cli.run(&["get", &urn.to_string()])?;

    assert_eq!(created, res);

    Ok(())
}

#[test]
fn top_level_get_component(
    deps: &EnvBasedTestDependencies,
    cli: &CliLive,
    _tracing: &Tracing,
) -> Result<(), anyhow::Error> {
    let component_name = "top_level_get_component";
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        component_name,
        env_service.to_str().unwrap(),
    ])?;

    let url = ComponentUrl {
        name: component.component_name.to_string(),
    };

    let res: ComponentView = cli.run_trimmed(&["get", &url.to_string()])?;
    assert_eq!(res, component);

    let res: ComponentView = cli.run_trimmed(&["get", &component.component_urn.to_string()])?;
    assert_eq!(res, component);

    Ok(())
}

#[test]
fn top_level_get_worker(
    deps: &EnvBasedTestDependencies,
    cli: &CliLive,
    _tracing: &Tracing,
) -> Result<(), anyhow::Error> {
    let component = add_environment_service_component(deps, "top_level_get_worker", cli)?;
    let worker_name = "top_level_get_worker";
    let cfg = &cli.config;

    let worker_urn: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        worker_name,
        "--component",
        &component.component_urn.to_string(),
    ])?;

    let url = WorkerUrl {
        component_name: component.component_name.to_string(),
        worker_name: Some(worker_name.to_string()),
    };

    let worker: WorkerMetadataView = cli.run(&["get", &url.to_string()])?;

    assert_eq!(worker.worker_urn, worker_urn);

    let worker: WorkerMetadataView = cli.run(&["get", &worker_urn.to_string()])?;

    assert_eq!(worker.worker_urn, worker_urn);

    Ok(())
}

#[test]
fn top_level_get_worker_function(
    deps: &EnvBasedTestDependencies,
    cli: &CliLive,
    _tracing: &Tracing,
) -> Result<(), anyhow::Error> {
    let component = add_environment_service_component(deps, "top_level_get_worker_function", cli)?;
    let worker_name = "top_level_get_worker_function";
    let cfg = &cli.config;

    let worker_urn: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        worker_name,
        "--component",
        &component.component_urn.to_string(),
    ])?;

    let function_name = "golem:it/api.{get-environment}";

    let url = WorkerFunctionUrl {
        component_name: component.component_name.to_string(),
        worker_name: worker_name.to_string(),
        function: function_name.to_string(),
    };

    let res = cli.run_string(&["get", &url.to_string()])?;

    assert_eq!(
        res,
        "golem:it/api.{get-environment}() -> result<list<tuple<string, string>>, string>\n"
    );

    let urn = WorkerFunctionUrn {
        id: worker_urn.id.try_into_worker_id().unwrap(),
        function: function_name.to_string(),
    };

    let res = cli.run_string(&["get", &urn.to_string()])?;

    assert_eq!(
        res,
        "golem:it/api.{get-environment}() -> result<list<tuple<string, string>>, string>\n"
    );

    Ok(())
}
