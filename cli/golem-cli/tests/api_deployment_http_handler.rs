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

use crate::cli::{Cli, CliLive};
use crate::Tracing;
use golem_cli::model::component::ComponentView;
use golem_client::model::{ApiDeployment, HttpApiDefinitionRequest, HttpApiDefinitionResponseData};
use golem_common::model::ComponentId;
use golem_common::uri::oss::urn::WorkerUrn;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use std::io::Write;
use std::sync::Arc;
use test_r::{inherit_test_dep, test, test_dep};

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &EnvBasedTestDependencies) -> CliLive {
    CliLive::make("api_deployment", Arc::new(deps.clone())).unwrap()
}

#[test]
async fn api_deployment_http_handler(
    deps: &EnvBasedTestDependencies,
    cli: &CliLive,
) -> anyhow::Result<()> {
    let host = format!(
        "localhost:{}",
        deps.worker_service().private_custom_request_port()
    );
    let component_name = "wasi-http-incoming-request-handler-echo".to_string();
    let golem_yaml = deps
        .component_directory()
        .join("wasi-http-incoming-request-handler-echo/golem.yaml");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('a', "app"),
        golem_yaml.to_str().unwrap(),
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    let component_urn = component.component_urn;
    let component_id = component_urn.id.clone();

    let worker_name = format!("{component_name}-worker");

    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
    ])?;

    let temp_dir = tempfile::tempdir()?;
    let api_definition = make_http_handler_api_definition(&component_id, &worker_name)?;
    let api_path = temp_dir.path().join("api-definition.json");
    {
        let mut api_file = std::fs::File::create_new(&api_path)?;
        let mut writer = std::io::BufWriter::new(&mut api_file);
        serde_json::to_writer(&mut writer, &api_definition)?;
        writer.flush()?;
    }

    let definition: HttpApiDefinitionResponseData =
        cli.run(&["api-definition", "add", api_path.to_str().unwrap()])?;

    let _: ApiDeployment = cli.run(&[
        "api-deployment",
        "deploy",
        &cfg.arg('d', "definition"),
        &format!("{}/{}", definition.id, definition.version),
        &cfg.arg('H', "host"),
        &host,
    ])?;

    let res = {
        let client = reqwest::Client::new();

        client
            .post(format!("http://{host}/test?foo=baz"))
            .header("test-header", "test-header-value")
            .body("\"test-body\"")
            .send()
            .await
    }?;
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(
        res.headers().get("echo-test-header").unwrap(),
        "\"test-header-value\""
    );
    assert_eq!(res.text().await?, "\"test-body\"");

    Ok(())
}

fn make_http_handler_api_definition(
    component_id: &ComponentId,
    worker_name: &str,
) -> anyhow::Result<HttpApiDefinitionRequest> {
    Ok(serde_yaml::from_str(&format!(
        r#"
        id: "{component_id}"
        version: "0.1.0"
        draft: true
        routes:
        - method: Post
          path: "/test"
          binding:
            bindingType: http-handler
            componentId:
              componentId: '{component_id}'
              version: 0
            workerName: 'let name: string = "{worker_name}"; name'
    "#,
    ))?)
}
