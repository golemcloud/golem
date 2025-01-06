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
use assert2::assert;
use golem_cli::model::component::ComponentView;
use golem_common::uri::oss::url::ComponentUrl;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use itertools::Itertools;
use std::sync::Arc;
use test_r::core::{DynamicTestRegistration, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &EnvBasedTestDependencies) -> CliLive {
    CliLive::make("component", Arc::new(deps.clone())).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_short", "CLI_short", true);
    make(r, "_long", "CLI_long", false);
}

fn make(r: &mut DynamicTestRegistration, suffix: &'static str, name: &'static str, short: bool) {
    add_test!(
        r,
        format!("component_add_and_find_all{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_find_all((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_and_find_by_name{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_find_by_name((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_and_get{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_get((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_and_get_urn{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_get_urn((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_and_get_url{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_get_url((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_from_project_file{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_from_project_file((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_update{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_update((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_update_urn{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_update_urn((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_update_url{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_update_url((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_update_from_project_file{suffix}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_update_from_project_file((deps, name.to_string(), cli.with_args(short)))
        }
    );
}

fn component_add_and_find_all(
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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

fn component_add_from_project_file(
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
    let component_name = format!("component_add_from_project_file_{name}");
    let golem_yaml = deps
        .component_directory()
        .join("cli-project-yaml/golem.yaml");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('a', "app"),
        golem_yaml.to_str().unwrap(),
        &cfg.arg('c', "component-name"),
        &component_name,
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

fn component_update_from_project_file(
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
    let component_name = format!("component_update_from_project_file_{name}");
    let golem_yaml = deps
        .component_directory()
        .join("cli-project-yaml/golem.yaml");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('a', "app"),
        golem_yaml.to_str().unwrap(),
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "update",
        &cfg.arg('a', "app"),
        golem_yaml.to_str().unwrap(),
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    let res: Vec<ComponentView> = cli.run_trimmed(&[
        "component",
        "list",
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    assert!(res.contains(&component), "{res:?}.contains({component:?})");
    assert!(
        res.iter().map(|x| x.component_version).contains(&1),
        "{res:?}.contains({component:?})"
    );
    assert_eq!(res.len(), 2, "{res:?}.len() == 2");
    Ok(())
}

fn component_update(
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    assert_eq!(res, component, "{res:?} = ({component:?})");
    Ok(())
}

fn component_add_and_get_urn(
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    assert_eq!(res, component, "{res:?} = ({component:?})");
    Ok(())
}

fn component_add_and_get_url(
    (deps, name, cli): (&EnvBasedTestDependencies, String, CliLive),
) -> Result<(), anyhow::Error> {
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
    assert_eq!(res, component, "{res:?} = ({component:?})");
    Ok(())
}
