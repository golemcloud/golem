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

use test_r::{inherit_test_dep, test, test_dep};

use crate::api_definition::{
    golem_def, make_golem_file, make_shopping_cart_component, to_definition,
};
use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use crate::worker::make_component;
use crate::Tracing;
use golem_cli::model::component::ComponentView;
use golem_cli::model::WorkerMetadataView;
use golem_client::model::HttpApiDefinitionResponseData;
use golem_cloud_cli::cloud::model::text::account::AccountGetView;
use golem_cloud_cli::cloud::model::text::project::{ProjectAddView, ProjectGetView};
use golem_common::model::AccountId;
use golem_common::uri::cloud::url::AccountUrl;
use golem_common::uri::cloud::urn::AccountUrn;
use golem_common::uri::oss::url::{ApiDefinitionUrl, ComponentUrl, WorkerFunctionUrl, WorkerUrl};
use golem_common::uri::oss::urn::{ApiDefinitionUrn, WorkerFunctionUrn, WorkerUrn};

inherit_test_dep!(CloudEnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &CloudEnvBasedTestDependencies) -> CliLive {
    CliLive::make("get", deps).unwrap()
}

#[test]
fn top_level_get_account(cli: &CliLive, _tracing: &Tracing) -> Result<(), anyhow::Error> {
    let account: AccountGetView = cli.run(&["account", "get"])?;

    let res1: AccountGetView = cli.run(&[
        "get",
        &AccountUrn {
            id: AccountId {
                value: account.0.id.clone(),
            },
        }
        .to_string(),
    ])?;

    assert_eq!(res1, account);

    let res2: AccountGetView = cli.run(&[
        "get",
        &AccountUrl {
            name: account.0.id.clone(),
        }
        .to_string(),
    ])?;

    assert_eq!(res2, account);

    Ok(())
}

#[test]
fn top_level_get_project(cli: &CliLive, _tracing: &Tracing) -> Result<(), anyhow::Error> {
    let name = "top level get project";

    let project: ProjectAddView = cli.run(&["project", "add", "--project-name", name])?;

    let res1: ProjectGetView = cli.run(&["get", &project.0.project_urn.to_string()])?;

    assert_eq!(res1.0, project.0);

    let res2: ProjectGetView = cli.run(&["get", &project.0.project_urn.to_string()])?;

    assert_eq!(res2.0, project.0);

    Ok(())
}

#[test]
fn top_level_get_api_definition(
    deps: &CloudEnvBasedTestDependencies,
    cli: &CliLive,
    _tracing: &Tracing,
) -> Result<(), anyhow::Error> {
    let component_name = "top_level_get_api_definition";
    let component = make_shopping_cart_component(deps, component_name, cli)?;
    let component_id = component.component_urn.id.0.to_string();
    let (api_definition_request, rib_output_type) = golem_def(component_name, &component_id);
    let path = make_golem_file(&api_definition_request)?;

    let _: HttpApiDefinitionResponseData =
        cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let url = ApiDefinitionUrl {
        name: component_name.to_string(),
        version: "0.1.0".to_string(),
    };

    let res: HttpApiDefinitionResponseData = cli.run(&["get", &url.to_string()])?;

    let expected = to_definition(
        api_definition_request.clone(),
        res.created_at,
        rib_output_type.clone(),
    );
    assert_eq!(res, expected);

    let urn = ApiDefinitionUrn {
        id: component_name.to_string(),
        version: "0.1.0".to_string(),
    };

    let res: HttpApiDefinitionResponseData = cli.run(&["get", &urn.to_string()])?;
    let expected = to_definition(
        api_definition_request.clone(),
        res.created_at,
        rib_output_type,
    );
    assert_eq!(res, expected);

    Ok(())
}

#[test]
fn top_level_get_component(
    deps: &CloudEnvBasedTestDependencies,
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
    deps: &CloudEnvBasedTestDependencies,
    cli: &CliLive,
    _tracing: &Tracing,
) -> Result<(), anyhow::Error> {
    let component = make_component(deps, "top_level_get_worker", cli)?;
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
    deps: &CloudEnvBasedTestDependencies,
    cli: &CliLive,
    _tracing: &Tracing,
) -> Result<(), anyhow::Error> {
    let component = make_component(deps, "top_level_get_worker_function", cli)?;
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
