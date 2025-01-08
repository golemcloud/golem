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

use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

use crate::cli::{Cli, CliConfig, CliLive};
use crate::{RefKind, Tracing};
use anyhow::anyhow;
use assert2::assert;
use golem_cli::model::component::ComponentView;
use golem_cli::model::text::fmt::TextFormat;
use golem_cli::model::text::worker::WorkerGetView;
use golem_cli::model::{Format, IdempotencyKey, WorkersMetadataResponseView};
use golem_client::model::{PublicOplogEntry, UpdateRecord};
use golem_common::model::TargetWorkerId;
use golem_common::uri::oss::url::{ComponentUrl, WorkerUrl};
use golem_common::uri::oss::urn::WorkerUrn;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use indoc::formatdoc;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;
use test_r::core::{DynamicTestRegistration, TestProperties, TestType};
use tracing::debug;

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &EnvBasedTestDependencies) -> CliLive {
    CliLive::make("worker", Arc::new(deps.clone())).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_name_short", "CLI_short_name", true, RefKind::Name);
    make(r, "_name_long", "CLI_long_name", false, RefKind::Name);
    make(r, "_url_short", "CLI_short_url", true, RefKind::Url);
    make(r, "_url_long", "CLI_long_url", false, RefKind::Url);
    make(r, "_urn_short", "CLI_short_urn", true, RefKind::Urn);
    make(r, "_urn_long", "CLI_long_urn", false, RefKind::Urn);
}

fn make(
    r: &mut DynamicTestRegistration,
    suffix: &'static str,
    name: &'static str,
    short: bool,
    ref_kind: RefKind,
) {
    add_test!(
        r,
        format!("worker_new_instance{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_new_instance((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_and_await{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_and_await((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_and_await_wave_params{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_and_await_wave_params((
                deps,
                name.to_string(),
                cli.with_args(short),
                ref_kind,
            ))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_no_params{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_no_params((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_drop{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_drop((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_json_params{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_json_params((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_wave_params{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_wave_params((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_connect{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_connect((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_connect_failed{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_connect_failed((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_interrupt{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_interrupt((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_simulated_crash{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_simulated_crash((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_list{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_list((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_update{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_update((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_indexed_resource{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_indexed_resource((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_without_name{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_without_name((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_without_name_ephemeral{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_without_name_ephemeral((
                deps,
                name.to_string(),
                cli.with_args(short),
                ref_kind,
            ))
        }
    );
    add_test!(
        r,
        format!("worker_get_oplog{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_get_oplog((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_search_oplog{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_search_oplog((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
}

pub fn add_component_from_file(
    deps: &(impl TestDependencies + Send + Sync + 'static),
    component_name: &str,
    cli: &CliLive,
    file: &str,
) -> anyhow::Result<ComponentView> {
    let env_service = deps.component_directory().join(file);
    let cfg = &cli.config;

    cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        component_name,
        env_service.to_str().unwrap(),
    ])
}

pub fn add_ephemeral_component_from_file(
    deps: &(impl TestDependencies + Send + Sync + 'static),
    component_name: &str,
    cli: &CliLive,
    file: &str,
) -> anyhow::Result<ComponentView> {
    let env_service = deps.component_directory().join(file);
    let cfg = &cli.config;

    cli.run(&[
        "component",
        "add",
        "--ephemeral",
        &cfg.arg('c', "component-name"),
        component_name,
        env_service.to_str().unwrap(),
    ])
}

pub fn add_environment_service_component(
    deps: &(impl TestDependencies + Send + Sync + 'static),
    component_name: &str,
    cli: &CliLive,
) -> anyhow::Result<ComponentView> {
    add_component_from_file(deps, component_name, cli, "environment-service.wasm")
}

fn component_ref_key(cfg: &CliConfig, ref_kind: RefKind) -> String {
    match ref_kind {
        RefKind::Name => cfg.arg('c', "component-name"),
        RefKind::Url | RefKind::Urn => cfg.arg('C', "component"),
    }
}

fn component_ref_value(component: &ComponentView, ref_kind: RefKind) -> String {
    match ref_kind {
        RefKind::Name => component.component_name.to_string(),
        RefKind::Url => ComponentUrl {
            name: component.component_name.to_string(),
        }
        .to_string(),
        RefKind::Urn => component.component_urn.to_string(),
    }
}

fn worker_new_instance(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component =
        add_environment_service_component(deps, &format!("{name} worker new instance"), &cli)?;
    let worker_name = format!("{name}_worker_new_instance");
    let cfg = &cli.config;

    let worker_urn: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;

    assert_eq!(worker_urn.id.component_id, component.component_urn.id);
    assert_eq!(worker_urn.id.worker_name, Some(worker_name));
    Ok(())
}

fn worker_ref(
    cfg: &CliConfig,
    ref_kind: RefKind,
    component: &ComponentView,
    worker_name: &str,
) -> Vec<String> {
    let worker_name = worker_name.to_owned();

    match ref_kind {
        RefKind::Name => {
            vec![
                component_ref_key(cfg, ref_kind),
                component_ref_value(component, ref_kind),
                cfg.arg('w', "worker-name"),
                worker_name,
            ]
        }
        RefKind::Url => {
            let url = WorkerUrl {
                component_name: component.component_name.clone(),
                worker_name: Some(worker_name),
            };

            vec![cfg.arg('W', "worker"), url.to_string()]
        }
        RefKind::Urn => {
            let urn = WorkerUrn {
                id: TargetWorkerId {
                    component_id: component.component_urn.id.clone(),
                    worker_name: Some(worker_name),
                },
            };

            vec![cfg.arg('W', "worker"), urn.to_string()]
        }
    }
}

fn worker_invoke_and_await(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component =
        add_environment_service_component(deps, &format!("{name} worker_invoke_and_await"), &cli)?;
    let worker_name = format!("{name}_worker_invoke_and_await");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
        &cfg.arg('e', "env"),
        "TEST_ENV=test-value",
        "test-arg",
    ])?;
    let args_key: IdempotencyKey = IdempotencyKey::fresh();

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke-and-await".to_owned(),
        cfg.arg('f', "function"),
        "golem:it/api.{get-arguments}".to_owned(),
        cfg.arg('j', "parameters"),
        "[]".to_owned(),
        cfg.arg('k', "idempotency-key"),
        args_key.0,
    ];

    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));

    let args = cli.run_json(&cli_args)?;

    let expected_args = json!(
        {
          "typ": {
            "items": [
              {
                "err": {
                  "type": "Str"
                },
                "ok": {
                  "inner": {
                    "type": "Str"
                  },
                  "type": "List"
                },
                "type": "Result"
              }
            ],
            "type": "Tuple"
          },
          "value": [
            {
              "ok": [
                "test-arg"
              ]
            }
          ]
        }
    );

    assert_eq!(args, expected_args);

    let env_key: IdempotencyKey = IdempotencyKey::fresh();

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke-and-await".to_owned(),
        cfg.arg('f', "function"),
        "golem:it/api.{get-environment}".to_owned(),
        cfg.arg('k', "idempotency-key"),
        env_key.0,
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let env = cli.run_json(&cli_args)?;

    let path = serde_json_path::JsonPath::parse("$.value[0].ok")?;
    let node = path.query(&env).exactly_one()?;

    assert!(
        node.as_array()
            .expect("$.value[0].ok is array")
            .contains(&json!(["TEST_ENV", "test-value"])),
        "Env contains TEST_ENV=test-value. Env: {env}"
    );

    Ok(())
}

fn worker_invoke_and_await_wave_params(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_component_from_file(
        deps,
        &format!("{name} worker_invoke_and_await_wave_params"),
        &cli,
        "key-value-service.wasm",
    )?;
    let worker_name = format!("{name}_worker_invoke_and_await_wave_params");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke-and-await".to_owned(),
        cfg.arg('f', "function"),
        "golem:it/api.{set}".to_owned(),
        cfg.arg('a', "arg"),
        r#""bucket name""#.to_owned(),
        cfg.arg('a', "arg"),
        r#""key name""#.to_owned(),
        cfg.arg('a', "arg"),
        r#"[1, 2, 3]"#.to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let res_set = cli.with_format(Format::Text).run_string(&cli_args)?;
    assert_eq!(res_set, "Empty result.\n");

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke-and-await".to_owned(),
        cfg.arg('f', "function"),
        "golem:it/api.{get}".to_owned(),
        cfg.arg('a', "arg"),
        r#""bucket name""#.to_owned(),
        cfg.arg('a', "arg"),
        r#""key name""#.to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let res_get = cli.with_format(Format::Text).run_string(&cli_args)?;
    assert_eq!(
        res_get,
        formatdoc!(
            "
            Invocation results in WAVE format:
            - some([1, 2, 3])

            "
        )
    );

    Ok(())
}

fn worker_invoke_drop(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_component_from_file(
        deps,
        &format!("{name} worker_invoke_drop"),
        &cli,
        "counters.wasm",
    )?;

    let worker_name = format!("{name}_worker_invoke_and_await");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
        &cfg.arg('e', "env"),
        "TEST_ENV=test-value",
        "test-arg",
    ])?;
    let args_key: IdempotencyKey = IdempotencyKey::fresh();

    let mut cli_args = vec![
        "worker".to_string(),
        "invoke-and-await".to_string(),
        cfg.arg('f', "function"),
        "rpc:counters-exports/api.{[constructor]counter}".to_string(),
        cfg.arg('j', "parameters"),
        "[{\"typ\" : { \"type\": \"Str\" }, \"value\" : \"counter1\"}]".to_string(),
        cfg.arg('k', "idempotency-key"),
        args_key.0.clone(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let result = cli.run_json(&cli_args)?;

    println!("JSON: {result}");

    // result is a JSON response containing a tuple with a single element holding the resource handle:
    // {"result": {
    //   "typ":  {"items":[{"mode":{"type":"Owned"},"resource_id":0,"type":"Handle"}],"type":"Tuple"},
    //   "value":["urn:worker:fcb5d2d4-d6db-4eca-99ec-6260ae9270db/CLI_short_name_worker_invoke_and_await/0"]}
    // }
    // we only need this inner element:
    let counter1 = result
        .as_object()
        .unwrap()
        .get("value")
        .unwrap()
        .as_array()
        .unwrap()
        .first()
        .unwrap();

    let json_parameter_list = Value::Array(vec![counter1.clone()]);

    let args_key1: IdempotencyKey = IdempotencyKey::fresh();

    let mut cli_args = vec![
        "worker".to_string(),
        "invoke-and-await".to_string(),
        cfg.arg('f', "function"),
        "rpc:counters-exports/api.{[drop]counter}".to_string(),
        cfg.arg('j', "parameters"),
        json_parameter_list.to_string(),
        cfg.arg('k', "idempotency-key"),
        args_key1.0.clone(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_json(&cli_args)?;

    Ok(())
}

fn worker_invoke_no_params(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component =
        add_environment_service_component(deps, &format!("{name} worker_invoke_no_params"), &cli)?;
    let worker_name = format!("{name}_worker_invoke_no_params");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke".to_owned(),
        cfg.arg('f', "function"),
        "golem:it/api.{get-arguments}".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    Ok(())
}

fn worker_invoke_json_params(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_environment_service_component(
        deps,
        &format!("{name} worker_invoke_json_params"),
        &cli,
    )?;
    let worker_name = format!("{name}_worker_invoke_json_params");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;
    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke".to_owned(),
        cfg.arg('f', "function"),
        "golem:it/api.{get-arguments}".to_owned(),
        cfg.arg('j', "parameters"),
        "[]".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    Ok(())
}

fn worker_invoke_wave_params(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_component_from_file(
        deps,
        &format!("{name} worker_invoke_wave_params"),
        &cli,
        "key-value-service.wasm",
    )?;
    let worker_name = format!("{name}_worker_invoke_wave_params");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;
    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke".to_owned(),
        cfg.arg('f', "function"),
        "golem:it/api.{set}".to_owned(),
        cfg.arg('a', "arg"),
        r#""bucket name""#.to_owned(),
        cfg.arg('a', "arg"),
        r#""key name""#.to_owned(),
        cfg.arg('a', "arg"),
        r#"[1, 2, 3]"#.to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    Ok(())
}

fn worker_connect(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let cfg = &cli.config;

    let stdout_service = deps.component_directory().join("write-stdout.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_connect"),
        stdout_service.to_str().unwrap(),
    ])?;
    let worker_name = format!("{name}_worker_connect");
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;

    let mut cli_args = vec!["worker".to_owned(), "connect".to_owned()];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let mut child = cli.run_stdout(&cli_args)?;

    let (tx, rx) = std::sync::mpsc::channel();

    let stdout = child
        .stdout
        .take()
        .ok_or(anyhow!("Can't get golem cli stdout"))?;

    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            tx.send(line.unwrap()).unwrap()
        }
    });

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke-and-await".to_owned(),
        cfg.arg('f', "function"),
        "run".to_owned(),
        cfg.arg('j', "parameters"),
        "[]".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let _ = cli.run_json(&cli_args)?;

    let line = rx.recv_timeout(Duration::from_secs(5))?;
    let json: serde_json::Value = serde_json::from_str(&line)?;

    assert_eq!(
        json.as_object()
            .unwrap()
            .get("message")
            .unwrap()
            .as_str()
            .unwrap(),
        "Sample text written to the output"
    );

    child.kill()?;

    Ok(())
}

fn worker_connect_failed(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let cfg = &cli.config;

    let stdout_service = deps.component_directory().join("write-stdout.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_connect_failed"),
        stdout_service.to_str().unwrap(),
    ])?;
    let worker_name = format!("{name}_worker_connect_failed");

    let mut cli_args = vec!["worker".to_owned(), "connect".to_owned()];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let mut child = cli.run_stdout(&cli_args)?;

    let exit = child.wait().unwrap();

    assert!(!exit.success(), "!{exit}.success()");

    Ok(())
}

fn worker_interrupt(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let cfg = &cli.config;

    let interruption_service = deps.component_directory().join("interruption.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_interrupt"),
        interruption_service.to_str().unwrap(),
    ])?;
    let worker_name = format!("{name}_worker_interrupt");
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;
    let mut cli_args = vec!["worker".to_owned(), "interrupt".to_owned()];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    Ok(())
}

fn worker_simulated_crash(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let cfg = &cli.config;

    let interruption_service = deps.component_directory().join("interruption.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_simulated_crash"),
        interruption_service.to_str().unwrap(),
    ])?;
    let worker_name = format!("{name}_worker_simulated_crash");
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;
    let mut cli_args = vec!["worker".to_owned(), "simulated-crash".to_owned()];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    Ok(())
}

fn worker_list(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_environment_service_component(deps, &format!("{name} worker_list"), &cli)?;
    let cfg = &cli.config;

    let workers_count = 10;
    let mut worker_urns = vec![];

    for i in 0..workers_count {
        let worker_name = format!("{name}_worker-{i}");
        let worker_urn: WorkerUrn = cli.run(&[
            "worker",
            "add",
            &cfg.arg('w', "worker-name"),
            &worker_name,
            &component_ref_key(cfg, ref_kind),
            &component_ref_value(&component, ref_kind),
        ])?;

        worker_urns.push(worker_urn);
    }

    for worker_urn in worker_urns {
        let result: WorkersMetadataResponseView = cli.run_trimmed(&[
            "worker",
            "list",
            &component_ref_key(cfg, ref_kind),
            &component_ref_value(&component, ref_kind),
            &cfg.arg('f', "filter"),
            format!("name = {}", worker_urn.id.worker_name.unwrap_or_default()).as_str(),
            &cfg.arg('f', "filter"),
            "version >= 0",
            "--precise",
            "true",
        ])?;

        assert_eq!(result.workers.len(), 1);
        assert!(result.cursor.is_none());
    }

    let result: WorkersMetadataResponseView = cli.run_trimmed(&[
        "worker",
        "list",
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
        &cfg.arg('f', "filter"),
        "version >= 0",
        &cfg.arg('f', "filter"),
        format!("name like {}_worker", name).as_str(),
        &cfg.arg('n', "count"),
        (workers_count / 2).to_string().as_str(),
    ])?;

    assert!(result.workers.len() >= workers_count / 2);
    assert!(result.cursor.is_some());

    let cursor = format!(
        "{}/{}",
        result.cursor.as_ref().unwrap().layer,
        result.cursor.as_ref().unwrap().cursor
    );
    let result2: WorkersMetadataResponseView = cli.run_trimmed(&[
        "worker",
        "list",
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
        &cfg.arg('f', "filter"),
        "version >= 0",
        &cfg.arg('f', "filter"),
        format!("name like {}_worker", name).as_str(),
        &cfg.arg('n', "count"),
        (workers_count - result.workers.len()).to_string().as_str(),
        &cfg.arg('S', "cursor"),
        &cursor,
    ])?;

    assert_eq!(result2.workers.len(), workers_count - result.workers.len());

    if let Some(cursor2) = result2.cursor {
        let cursor2 = format!("{}/{}", cursor2.layer, cursor2.cursor);
        let result3: WorkersMetadataResponseView = cli.run_trimmed(&[
            "worker",
            "list",
            &component_ref_key(cfg, ref_kind),
            &component_ref_value(&component, ref_kind),
            &cfg.arg('f', "filter"),
            "version >= 0",
            &cfg.arg('f', "filter"),
            format!("name like {}_worker", name).as_str(),
            &cfg.arg('n', "count"),
            workers_count.to_string().as_str(),
            &cfg.arg('S', "cursor"),
            &cursor2,
        ])?;
        assert_eq!(result3.workers.len(), 0);
    }

    Ok(())
}

fn worker_update(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let cfg = &cli.config;
    let component_v1 = deps.component_directory().join("update-test-v1.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_update"),
        component_v1.to_str().unwrap(),
    ])?;
    let worker_name = format!("{name}_worker_update");

    let workers_list = || -> Result<WorkersMetadataResponseView, anyhow::Error> {
        cli.run_trimmed(&[
            "worker",
            "list",
            &component_ref_key(cfg, ref_kind),
            &component_ref_value(&component, ref_kind),
            &cfg.arg('f', "filter"),
            format!("name like {}_worker", name).as_str(),
            "--precise",
            "true",
        ])
    };

    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
    ])?;
    let original_updates = workers_list()?.workers[0].updates.len();
    let component_v2 = deps.component_directory().join("update-test-v2.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "update",
        &component_ref_key(cfg, ref_kind),
        &component_ref_value(&component, ref_kind),
        component_v2.to_str().unwrap(),
    ])?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "update".to_owned(),
        cfg.arg('m', "mode"),
        "auto".to_owned(),
        cfg.arg('t', "target-version"),
        "1".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;
    let worker_updates_after_update = workers_list()?.workers[0].updates[0].clone();
    let target_version = match worker_updates_after_update {
        UpdateRecord::PendingUpdate(pu) => pu.target_version,
        UpdateRecord::SuccessfulUpdate(su) => su.target_version,
        UpdateRecord::FailedUpdate(_) => panic!("Update failed"),
    };

    loop {
        let mut cli_args = vec!["worker".to_owned(), "get".to_owned()];
        cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
        let result: WorkerGetView = cli.run(&cli_args)?;

        if result.0.component_version == target_version {
            break;
        } else {
            debug!("Waiting for worker to update...");
            sleep(Duration::from_secs(2));
        }
    }

    let mut cli_args = vec!["worker".to_owned(), "oplog".to_owned()];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let result: Vec<(u64, PublicOplogEntry)> = cli.run(&cli_args)?;
    result.print();

    assert_eq!(result.len(), 3); // create, enqueue update, successful update
    assert_eq!(original_updates, 0);
    assert_eq!(target_version, 1);
    Ok(())
}

fn worker_invoke_indexed_resource(
    (deps, name, cli, ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_component_from_file(
        deps,
        &format!("{name}_worker_invoke_indexed_resource"),
        &cli,
        "counters.wasm",
    )?;
    let worker_name = format!("{name}_worker_invoke_indexed_resource");
    let cfg = &cli.config;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke".to_owned(),
        cfg.arg('f', "function"),
        r#"rpc:counters-exports/api.{counter("counter1").inc-by}"#.to_owned(),
        cfg.arg('a', "arg"),
        "1".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke".to_owned(),
        cfg.arg('f', "function"),
        r#"rpc:counters-exports/api.{counter("counter1").inc-by}"#.to_owned(),
        cfg.arg('a', "arg"),
        "2".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke".to_owned(),
        cfg.arg('f', "function"),
        r#"rpc:counters-exports/api.{counter("counter2").inc-by}"#.to_owned(),
        cfg.arg('a', "arg"),
        "5".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke-and-await".to_owned(),
        cfg.arg('f', "function"),
        r#"rpc:counters-exports/api.{counter("counter1").get-value}"#.to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let result = cli.run_json(&cli_args)?;

    assert_eq!(
        result,
        json!({"typ":{"items":[{"type":"U64"}],"type":"Tuple"},"value":[3]})
    );

    let mut cli_args = vec!["worker".to_owned(), "oplog".to_owned()];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let result: Vec<(u64, PublicOplogEntry)> = cli.run(&cli_args)?;
    result.print();

    Ok(())
}

fn worker_invoke_without_name(
    (deps, name, cli, _ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_environment_service_component(
        deps,
        &format!("{name} worker_invoke_without_name"),
        &cli,
    )?;
    let cfg = &cli.config;

    let url = WorkerUrl {
        component_name: component.component_name.clone(),
        worker_name: None,
    };

    let result: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{get-environment}",
    ])?;

    let path = serde_json_path::JsonPath::parse("$.value[0].ok")?;
    let _node = path.query(&result).exactly_one()?;

    Ok(())
}

fn worker_invoke_without_name_ephemeral(
    (deps, name, cli, _ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_ephemeral_component_from_file(
        deps,
        &format!("{name} worker_invoke_without_name_ephemeral"),
        &cli,
        "environment-service.wasm",
    )?;
    let cfg = &cli.config;

    let url = WorkerUrl {
        component_name: component.component_name.clone(),
        worker_name: None,
    };

    let result: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{get-environment}",
    ])?;

    let path = serde_json_path::JsonPath::parse("$.value[0].ok")?;
    let _node = path.query(&result).exactly_one()?;

    Ok(())
}

fn worker_get_oplog(
    (deps, name, cli, _ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_component_from_file(
        deps,
        &format!("{name} worker_get_oplog"),
        &cli,
        "runtime-service.wasm",
    )?;
    let cfg = &cli.config;

    let url = WorkerUrl {
        component_name: component.component_name.clone(),
        worker_name: Some("test1".to_string()),
    };

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{generate-idempotency-keys}",
    ])?;

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{generate-idempotency-keys}",
    ])?;

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{generate-idempotency-keys}",
    ])?;

    let result: Vec<(u64, PublicOplogEntry)> =
        cli.run(&["worker", "oplog", &cfg.arg('W', "worker"), &url.to_string()])?;

    result.print();

    // Whether there is an "enqueued invocation" entry or just directly started invocation
    // depends on timing
    assert!(result.len() >= 12 && result.len() <= 14);

    Ok(())
}

fn worker_search_oplog(
    (deps, name, cli, _ref_kind): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
        RefKind,
    ),
) -> anyhow::Result<()> {
    let component = add_component_from_file(
        deps,
        &format!("{name} worker_search_oplog"),
        &cli,
        "shopping-cart.wasm",
    )?;
    let cfg = &cli.config;

    let url = WorkerUrl {
        component_name: component.component_name.clone(),
        worker_name: Some("search-oplog-1".to_string()),
    };

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{initialize-cart}",
        &cfg.arg('a', "arg"),
        r#""test-user-1""#,
    ])?;

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{add-item}",
        &cfg.arg('a', "arg"),
        r#"{ product-id: "G1000", name: "Golem T-Shirt M", price: 100.0, quantity: 5 }"#,
    ])?;

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{add-item}",
        &cfg.arg('a', "arg"),
        r#"{ product-id: "G1001", name: "Golem Cloud Subscription 1y", price: 999999.0, quantity: 1 }"#,
    ])?;

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{add-item}",
        &cfg.arg('a', "arg"),
        r#"{ product-id: "G1002", name: "Mud Golem", price: 11.0, quantity: 10 }"#,
    ])?;

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{update-item-quantity}",
        &cfg.arg('a', "arg"),
        r#""G1002""#,
        &cfg.arg('a', "arg"),
        r#"20"#,
    ])?;

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{get-cart-contents}",
    ])?;

    let _: Value = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        &cfg.arg('f', "function"),
        "golem:it/api.{checkout}",
    ])?;

    let _oplog: Vec<(u64, PublicOplogEntry)> =
        cli.run(&["worker", "oplog", &cfg.arg('W', "worker"), &url.to_string()])?;

    let result1: Vec<(u64, PublicOplogEntry)> = cli.run(&[
        "worker",
        "oplog",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        "--query",
        "G1002",
    ])?;
    let result2: Vec<(u64, PublicOplogEntry)> = cli.run(&[
        "worker",
        "oplog",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        "--query",
        "imported-function",
    ])?;
    let result3: Vec<(u64, PublicOplogEntry)> = cli.run(&[
        "worker",
        "oplog",
        &cfg.arg('W', "worker"),
        &url.to_string(),
        "--query",
        "product-id:G1001 OR product-id:G1000",
    ])?;

    assert_eq!(result1.len(), 4); // two invocations and two log messages
    assert_eq!(result2.len(), 2); // get_preopened_directories, get_random_bytes
    assert_eq!(result3.len(), 2); // two invocations
    Ok(())
}
