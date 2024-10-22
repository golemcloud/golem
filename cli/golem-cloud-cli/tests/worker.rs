use crate::cli::{Cli, CliConfig, CliLive};
use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use crate::{RefKind, Tracing};
use anyhow::anyhow;
use assert2::assert;
use golem_cli::model::component::ComponentView;
use golem_cli::model::{Format, IdempotencyKey, WorkersMetadataResponseView};
use golem_client::model::UpdateRecord;
use golem_common::uri::oss::url::{ComponentUrl, WorkerUrl};
use golem_common::uri::oss::urn::WorkerUrn;
use indoc::formatdoc;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::time::Duration;
use test_r::core::{DynamicTestRegistration, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(CloudEnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &CloudEnvBasedTestDependencies) -> CliLive {
    CliLive::make("worker", deps).unwrap()
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
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_new_instance((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_and_await{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_and_await((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_and_await_wave_params{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
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
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_no_params((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_drop{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_drop((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_json_params{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_json_params((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_wave_params{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_wave_params((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_connect{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_connect((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_connect_failed{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_connect_failed((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_interrupt{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_interrupt((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_simulated_crash{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_simulated_crash((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_list{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_list((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_update{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_update((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
    add_test!(
        r,
        format!("worker_invoke_indexed_resource{suffix}"),
        TestType::IntegrationTest,
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            worker_invoke_indexed_resource((deps, name.to_string(), cli.with_args(short), ref_kind))
        }
    );
}

pub fn make_component_from_file(
    deps: &(impl TestDependencies + Send + Sync + 'static),
    component_name: &str,
    cli: &CliLive,
    file: &str,
) -> Result<ComponentView, anyhow::Error> {
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

pub fn make_component(
    deps: &(impl TestDependencies + Send + Sync + 'static),
    component_name: &str,
    cli: &CliLive,
) -> Result<ComponentView, anyhow::Error> {
    make_component_from_file(deps, component_name, cli, "environment-service.wasm")
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
) -> Result<(), anyhow::Error> {
    let component = make_component(deps, &format!("{name} worker new instance"), &cli)?;
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
                id: golem_common::model::TargetWorkerId {
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
) -> Result<(), anyhow::Error> {
    let component = make_component(deps, &format!("{name} worker_invoke_and_await"), &cli)?;
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
) -> Result<(), anyhow::Error> {
    let component = make_component_from_file(
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
) -> Result<(), anyhow::Error> {
    let component = make_component_from_file(
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
        "rpc:counters/api.{[constructor]counter}".to_string(),
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
        "rpc:counters/api.{[drop]counter}".to_string(),
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
) -> Result<(), anyhow::Error> {
    let component = make_component(deps, &format!("{name} worker_invoke_no_params"), &cli)?;
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
) -> Result<(), anyhow::Error> {
    let component = make_component(deps, &format!("{name} worker_invoke_json_params"), &cli)?;
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
) -> Result<(), anyhow::Error> {
    let component = make_component_from_file(
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
) -> Result<(), anyhow::Error> {
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
    let json: Value = serde_json::from_str(&line)?;

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
) -> Result<(), anyhow::Error> {
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

    let exit = child.wait()?;

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
) -> Result<(), anyhow::Error> {
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
) -> Result<(), anyhow::Error> {
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
) -> Result<(), anyhow::Error> {
    let component = make_component(deps, &format!("{name} worker_list"), &cli)?;
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
        let result: WorkersMetadataResponseView = cli.run(&[
            "worker",
            "list",
            &component_ref_key(cfg, ref_kind),
            &component_ref_value(&component, ref_kind),
            &cfg.arg('f', "filter"),
            format!("name = {}", worker_urn.id.worker_name.unwrap()).as_str(),
            &cfg.arg('f', "filter"),
            "version >= 0",
            "--precise",
            "true",
        ])?;

        assert_eq!(result.workers.len(), 1);
        assert!(result.cursor.is_none());
    }

    let result: WorkersMetadataResponseView = cli.run(&[
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
    let result2: WorkersMetadataResponseView = cli.run(&[
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
        let result3: WorkersMetadataResponseView = cli.run(&[
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
) -> Result<(), anyhow::Error> {
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
        cli.run(&[
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
) -> Result<(), anyhow::Error> {
    let component = make_component_from_file(
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
        r#"rpc:counters/api.{counter("counter1").inc-by}"#.to_owned(),
        cfg.arg('a', "arg"),
        "1".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke".to_owned(),
        cfg.arg('f', "function"),
        r#"rpc:counters/api.{counter("counter1").inc-by}"#.to_owned(),
        cfg.arg('a', "arg"),
        "2".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke".to_owned(),
        cfg.arg('f', "function"),
        r#"rpc:counters/api.{counter("counter2").inc-by}"#.to_owned(),
        cfg.arg('a', "arg"),
        "5".to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    cli.run_unit(&cli_args)?;

    let mut cli_args = vec![
        "worker".to_owned(),
        "invoke-and-await".to_owned(),
        cfg.arg('f', "function"),
        r#"rpc:counters/api.{counter("counter1").get-value}"#.to_owned(),
    ];
    cli_args.append(&mut worker_ref(cfg, ref_kind, &component, &worker_name));
    let result = cli.run_json(&cli_args)?;

    assert_eq!(
        result,
        json!({"typ":{"items":[{"type":"U64"}],"type":"Tuple"},"value":[3]})
    );

    Ok(())
}
