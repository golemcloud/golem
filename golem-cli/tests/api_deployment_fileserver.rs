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
use golem_cli::model::Format;
use golem_client::model::{ApiDeployment, HttpApiDefinitionRequest, HttpApiDefinitionResponseData};
use golem_common::model::ComponentId;
use golem_common::uri::oss::urn::WorkerUrn;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use std::io::Write;
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
        format!("api_deployment_fileserver_simple{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            api_deployment_file_server_simple((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("api_deployment_fileserver_complex{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            api_deployment_fileserver_complex((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("api_deployment_fileserver_stateful_worker{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            api_deployment_fileserver_stateful_worker((
                deps,
                name.to_string(),
                cli.with_args(short),
            ))
        }
    );
}

fn api_deployment_file_server_simple(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> anyhow::Result<()> {
    let host = format!(
        "localhost:{}",
        deps.worker_service().private_custom_request_port()
    );
    let component_name = format!("api_deployment_fileserver_simple{name}");
    let golem_yaml = deps.component_directory().join("file-server/golem.yaml");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('a', "app"),
        golem_yaml.to_str().unwrap(),
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    let component_id = component.component_urn.id;

    let temp_dir = tempfile::tempdir()?;
    let api_definition = make_file_server_api_definition_simple(&component_id)?;
    let api_path = temp_dir.path().join("api-file-server.json");
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

    // read a read-only file
    let res1 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/foo.txt"))?;
    assert_eq!(res1.status().as_u16(), 200);
    assert_eq!(res1.headers().get("content-type").unwrap(), "text/plain");
    assert_eq!(res1.text()?, "foo\n");

    // read a read-write file
    let res2 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/bar.txt"))?;
    assert_eq!(res2.status().as_u16(), 200);
    assert_eq!(res2.headers().get("content-type").unwrap(), "text/plain");
    assert_eq!(res2.text()?, "bar\n");

    // read a non-existing file
    let res3 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/notfound.txt"))?;
    assert_eq!(res3.status().as_u16(), 404);

    // try to read a directory. This will fail with a 400 as the mime type of 'dir' cannot be inferred.
    let res4 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/dir"))?;
    assert_eq!(res4.status().as_u16(), 400);

    let res5 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/dir/foo.txt"))?;
    assert_eq!(res5.status().as_u16(), 200);
    assert_eq!(res5.headers().get("content-type").unwrap(), "text/plain");
    assert_eq!(res5.text()?, "foo\n");

    Ok(())
}

fn api_deployment_fileserver_complex(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> anyhow::Result<()> {
    let host = format!(
        "localhost:{}",
        deps.worker_service().private_custom_request_port()
    );
    let component_name = format!("api_deployment_fileserver_complex{name}");
    let golem_yaml = deps.component_directory().join("file-server/golem.yaml");

    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('a', "app"),
        golem_yaml.to_str().unwrap(),
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    let component_id = component.component_urn.id;

    let temp_dir = tempfile::tempdir()?;

    let api_definition = make_file_server_api_definition_complex(&component_id)?;
    let api_path = temp_dir.path().join("api-file-server.json");

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

    let res1 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/foo.txt"))?;
    assert_eq!(res1.status().as_u16(), 201);
    assert_eq!(
        res1.headers().get("content-type").unwrap(),
        "application/json"
    );
    assert_eq!(res1.text()?, "foo\n");

    // read a non-existing file
    let res2 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/notfound.txt"))?;
    assert_eq!(res2.status().as_u16(), 404);

    // read a directory. This will fail with a 400 here as we have an explicit mime type.
    let res3 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/dir"))?;
    assert_eq!(res3.status().as_u16(), 400);

    Ok(())
}

fn api_deployment_fileserver_stateful_worker(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> anyhow::Result<()> {
    let host = format!(
        "localhost:{}",
        deps.worker_service().private_custom_request_port()
    );
    let component_name = format!("api_deployment_fileserver_stateful_worker{name}");
    let golem_yaml = deps.component_directory().join("file-server/golem.yaml");

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

    let _ = cli.with_format(Format::Text).run_string(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "run",
    ])?;

    let temp_dir = tempfile::tempdir()?;

    let api_definition =
        make_file_server_api_definition_with_worker_name(&component_id, &worker_name)?;
    let api_path = temp_dir.path().join("api-file-server.json");

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

    let res1 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/foo.txt"))?;
    assert_eq!(res1.status().as_u16(), 200);
    assert_eq!(res1.headers().get("content-type").unwrap(), "text/plain");
    assert_eq!(res1.text()?, "foo\n");

    let res2 = reqwest::blocking::get(format!("http://{host}/files/{component_id}/bar.txt"))?;
    assert_eq!(res2.status().as_u16(), 200);
    assert_eq!(res2.headers().get("content-type").unwrap(), "text/plain");
    assert_eq!(res2.text()?, "hello world");

    Ok(())
}

fn make_file_server_api_definition_simple(
    component_id: &ComponentId,
) -> anyhow::Result<HttpApiDefinitionRequest> {
    Ok(serde_yaml::from_str(&format!(
        r#"
        id: "{component_id}"
        version: "0.1.0"
        draft: true
        routes:
        - method: Get
          path: "/files/{component_id}/{{+file}}"
          binding:
            bindingType: file-server
            componentId:
              componentId: '{component_id}'
              version: 0
            response: 'let file: string = request.path.file; "/files/${{file}}"'
    "#,
    ))?)
}

fn make_file_server_api_definition_complex(
    component_id: &ComponentId,
) -> anyhow::Result<HttpApiDefinitionRequest> {
    Ok(serde_yaml::from_str(&format!(
        r#"
        id: "{component_id}"
        version: "0.1.0"
        draft: true
        routes:
        - method: Get
          path: "/files/{component_id}/{{file}}"
          binding:
            bindingType: file-server
            componentId:
              componentId: '{component_id}'
              version: 0
            response: 'let file: string = request.path.file; {{ headers: {{Content-Type: "application/json"}}, status: 201u64, file-path: "/files/${{file}}" }}'
    "#,
    ))?)
}

fn make_file_server_api_definition_with_worker_name(
    component_id: &ComponentId,
    worker_name: &str,
) -> anyhow::Result<HttpApiDefinitionRequest> {
    Ok(serde_yaml::from_str(&format!(
        r#"
        id: "{component_id}"
        version: "0.1.0"
        draft: true
        routes:
        - method: Get
          path: "/files/{component_id}/{{file}}"
          binding:
            bindingType: file-server
            componentId:
              componentId: '{component_id}'
              version: 0
            workerName: 'let name: string = "{worker_name}"; name'
            response: 'let file: string = request.path.file; "/files/${{file}}"'
    "#,
    ))?)
}
