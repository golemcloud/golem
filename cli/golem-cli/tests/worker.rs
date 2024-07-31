use crate::cli::{Cli, CliLive};
use golem_cli::model::component::ComponentView;
use golem_cli::model::{Format, IdempotencyKey};
use golem_client::model::{UpdateRecord, WorkerId, WorkersMetadataResponse};
use golem_test_framework::config::TestDependencies;
use indoc::formatdoc;
use libtest_mimic::{Failed, Trial};
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::time::Duration;

fn make(
    suffix: &str,
    name: &str,
    cli: CliLive,
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
) -> Vec<Trial> {
    let ctx = (deps, name.to_string(), cli);
    vec![
        Trial::test_in_context(
            format!("worker_new_instance{suffix}"),
            ctx.clone(),
            worker_new_instance,
        ),
        Trial::test_in_context(
            format!("worker_invoke_and_await{suffix}"),
            ctx.clone(),
            worker_invoke_and_await,
        ),
        Trial::test_in_context(
            format!("worker_invoke_and_await_wave_params{suffix}"),
            ctx.clone(),
            worker_invoke_and_await_wave_params,
        ),
        Trial::test_in_context(
            format!("worker_invoke_no_params{suffix}"),
            ctx.clone(),
            worker_invoke_no_params,
        ),
        Trial::test_in_context(
            format!("worker_invoke_drop{suffix}"),
            ctx.clone(),
            worker_invoke_drop,
        ),
        Trial::test_in_context(
            format!("worker_invoke_json_params{suffix}"),
            ctx.clone(),
            worker_invoke_json_params,
        ),
        Trial::test_in_context(
            format!("worker_invoke_wave_params{suffix}"),
            ctx.clone(),
            worker_invoke_wave_params,
        ),
        Trial::test_in_context(
            format!("worker_connect{suffix}"),
            ctx.clone(),
            worker_connect,
        ),
        Trial::test_in_context(
            format!("worker_connect_failed{suffix}"),
            ctx.clone(),
            worker_connect_failed,
        ),
        Trial::test_in_context(
            format!("worker_interrupt{suffix}"),
            ctx.clone(),
            worker_interrupt,
        ),
        Trial::test_in_context(
            format!("worker_simulated_crash{suffix}"),
            ctx.clone(),
            worker_simulated_crash,
        ),
        Trial::test_in_context(format!("worker_list{suffix}"), ctx.clone(), worker_list),
        Trial::test_in_context(format!("worker_update{suffix}"), ctx.clone(), worker_update),
        Trial::test_in_context(
            format!("worker_invoke_indexed_resource{suffix}"),
            ctx.clone(),
            worker_invoke_indexed_resource,
        ),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make("worker_short", deps.clone())
            .unwrap()
            .with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make("worker_long", deps.clone())
            .unwrap()
            .with_long_args(),
        deps.clone(),
    );

    short_args.append(&mut long_args);
    short_args
}

pub fn make_component_from_file(
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    component_name: &str,
    cli: &CliLive,
    file: &str,
) -> Result<ComponentView, Failed> {
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
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    component_name: &str,
    cli: &CliLive,
) -> Result<ComponentView, Failed> {
    make_component_from_file(deps, component_name, cli, "environment-service.wasm")
}

fn worker_new_instance(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id =
        make_component(deps, &format!("{name} worker new instance"), &cli)?.component_id;
    let worker_name = format!("{name}_worker_new_instance");
    let cfg = &cli.config;
    let worker_id: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;

    assert_eq!(worker_id.component_id.to_string(), component_id);
    assert_eq!(worker_id.worker_name, worker_name);
    Ok(())
}

fn worker_invoke_and_await(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id =
        make_component(deps, &format!("{name} worker_invoke_and_await"), &cli)?.component_id;
    let worker_name = format!("{name}_worker_invoke_and_await");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('e', "env"),
        "TEST_ENV=test-value",
        "test-arg",
    ])?;
    let args_key: IdempotencyKey = IdempotencyKey::fresh();
    let args = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api.{get-arguments}",
        &cfg.arg('j', "parameters"),
        "[]",
        &cfg.arg('k', "idempotency-key"),
        &args_key.0,
    ])?;

    let expected_args = json!([{"ok": ["test-arg"]}]);

    assert_eq!(args, expected_args);

    let env_key: IdempotencyKey = IdempotencyKey::fresh();

    let env = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api.{get-environment}",
        &cfg.arg('k', "idempotency-key"),
        &env_key.0,
    ])?;

    let path = serde_json_path::JsonPath::parse("$[0].ok")?;

    let node = path.query(&env).exactly_one()?;

    assert!(
        node.as_array()
            .expect("env.[0].ok is array")
            .contains(&json!(["TEST_ENV", "test-value"])),
        "Env contains TEST_ENV=test-value. Env: {env}"
    );

    Ok(())
}

fn worker_invoke_and_await_wave_params(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id = make_component_from_file(
        deps,
        &format!("{name} worker_invoke_and_await_wave_params"),
        &cli,
        "key-value-service.wasm",
    )?
    .component_id;
    let worker_name = format!("{name}_worker_invoke_and_await_wave_params");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;
    let res_set = cli.with_format(Format::Text).run_string(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api.{set}",
        &cfg.arg('a', "arg"),
        r#""bucket name""#,
        &cfg.arg('a', "arg"),
        r#""key name""#,
        &cfg.arg('a', "arg"),
        r#"[1, 2, 3]"#,
    ])?;
    assert_eq!(res_set, "Empty result.\n");

    let res_get = cli.with_format(Format::Text).run_string(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api.{get}",
        &cfg.arg('a', "arg"),
        r#""bucket name""#,
        &cfg.arg('a', "arg"),
        r#""key name""#,
    ])?;
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
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id = make_component_from_file(
        deps,
        &format!("{name} worker_invoke_drop"),
        &cli,
        "counters.wasm",
    )?
    .component_id;

    let worker_name = format!("{name}_worker_invoke_and_await");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('e', "env"),
        "TEST_ENV=test-value",
        "test-arg",
    ])?;
    let args_key: IdempotencyKey = IdempotencyKey::fresh();
    let result = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "rpc:counters/api.{[constructor]counter}",
        &cfg.arg('j', "parameters"),
        "[{\"str\": \"counter1\"}]",
        &cfg.arg('k', "idempotency-key"),
        &args_key.0,
    ])?;
    let handle_str = match result {
        serde_json::Value::Array(vec) => match vec[0].clone() {
            serde_json::Value::String(str) => str,
            _ => panic!("Expected handle string"),
        },
        _ => panic!("Expected handle string"),
    };

    let handle_json = format!("[{{\"handle\" : \"{}\"}}]", handle_str);
    let args_key1: IdempotencyKey = IdempotencyKey::fresh();

    cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "rpc:counters/api.{[drop]counter}",
        &cfg.arg('j', "parameters"),
        handle_json.as_str(),
        &cfg.arg('k', "idempotency-key"),
        &args_key1.0,
    ])?;

    Ok(())
}

fn worker_invoke_no_params(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id =
        make_component(deps, &format!("{name} worker_invoke_no_params"), &cli)?.component_id;
    let worker_name = format!("{name}_worker_invoke_no_params");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api.{get-arguments}",
    ])?;

    Ok(())
}

fn worker_invoke_json_params(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id =
        make_component(deps, &format!("{name} worker_invoke_json_params"), &cli)?.component_id;
    let worker_name = format!("{name}_worker_invoke_json_params");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api.{get-arguments}",
        &cfg.arg('j', "parameters"),
        "[]",
    ])?;

    Ok(())
}

fn worker_invoke_wave_params(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id = make_component_from_file(
        deps,
        &format!("{name} worker_invoke_wave_params"),
        &cli,
        "key-value-service.wasm",
    )?
    .component_id;
    let worker_name = format!("{name}_worker_invoke_wave_params");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api.{set}",
        &cfg.arg('a', "arg"),
        r#""bucket name""#,
        &cfg.arg('a', "arg"),
        r#""key name""#,
        &cfg.arg('a', "arg"),
        r#"[1, 2, 3]"#,
    ])?;

    Ok(())
}

fn worker_connect(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let stdout_service = deps.component_directory().join("write-stdout.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_connect"),
        stdout_service.to_str().unwrap(),
    ])?;
    let component_id = component.component_id;
    let worker_name = format!("{name}_worker_connect");
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;

    let mut child = cli.run_stdout(&[
        "worker",
        "connect",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
    ])?;

    let (tx, rx) = std::sync::mpsc::channel();

    let stdout = child
        .stdout
        .take()
        .ok_or::<Failed>("Can't get golem cli stdout".into())?;

    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            tx.send(line.unwrap()).unwrap()
        }
    });

    let _ = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "run",
        &cfg.arg('j', "parameters"),
        "[]",
    ])?;

    let line = rx.recv_timeout(Duration::from_secs(5))?;

    assert_eq!(line, "Sample text written to the output");

    child.kill()?;

    Ok(())
}

fn worker_connect_failed(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let stdout_service = deps.component_directory().join("write-stdout.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_connect_failed"),
        stdout_service.to_str().unwrap(),
    ])?;
    let component_id = component.component_id;
    let worker_name = format!("{name}_worker_connect_failed");

    let mut child = cli.run_stdout(&[
        "worker",
        "connect",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
    ])?;

    let exit = child.wait().unwrap();

    assert!(!exit.success(), "!{exit}.success()");

    Ok(())
}

fn worker_interrupt(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let interruption_service = deps.component_directory().join("interruption.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_interrupt"),
        interruption_service.to_str().unwrap(),
    ])?;
    let component_id = component.component_id;
    let worker_name = format!("{name}_worker_interrupt");
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "interrupt",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;

    Ok(())
}

fn worker_simulated_crash(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let interruption_service = deps.component_directory().join("interruption.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_simulated_crash"),
        interruption_service.to_str().unwrap(),
    ])?;
    let component_id = component.component_id;
    let worker_name = format!("{name}_worker_simulated_crash");
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "simulated-crash",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;

    Ok(())
}

fn worker_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id = make_component(deps, &format!("{name} worker_list"), &cli)?.component_id;
    let cfg = &cli.config;

    let workers_count = 10;
    let mut worker_ids = vec![];

    for i in 0..workers_count {
        let worker_name = format!("{name}_worker-{i}");
        let worker_id: WorkerId = cli.run(&[
            "worker",
            "add",
            &cfg.arg('w', "worker-name"),
            &worker_name,
            &cfg.arg('C', "component-id"),
            &component_id,
        ])?;

        worker_ids.push(worker_id);
    }

    for worker_id in worker_ids {
        let result: WorkersMetadataResponse = cli.run(&[
            "worker",
            "list",
            &cfg.arg('C', "component-id"),
            &component_id,
            &cfg.arg('f', "filter"),
            format!("name = {}", worker_id.worker_name).as_str(),
            &cfg.arg('f', "filter"),
            "version >= 0",
            "--precise",
            "true",
        ])?;

        assert_eq!(result.workers.len(), 1);
        assert!(result.cursor.is_none());
    }

    let result: WorkersMetadataResponse = cli.run(&[
        "worker",
        "list",
        &cfg.arg('C', "component-id"),
        &component_id,
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
    let result2: WorkersMetadataResponse = cli.run(&[
        "worker",
        "list",
        &cfg.arg('C', "component-id"),
        &component_id,
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
        let result3: WorkersMetadataResponse = cli.run(&[
            "worker",
            "list",
            &cfg.arg('C', "component-id"),
            &component_id,
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
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;
    let component_v1 = deps.component_directory().join("update-test-v1.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_update"),
        component_v1.to_str().unwrap(),
    ])?;
    let component_id = component.component_id;
    let worker_name = format!("{name}_worker_update");

    let workers_list = || -> Result<WorkersMetadataResponse, Failed> {
        cli.run(&[
            "worker",
            "list",
            &cfg.arg('C', "component-id"),
            &component_id,
            &cfg.arg('f', "filter"),
            format!("name like {}_worker", name).as_str(),
            "--precise",
            "true",
        ])
    };

    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;
    let original_updates = workers_list()?.workers[0].updates.len();
    let component_v2 = deps.component_directory().join("update-test-v2.wasm");
    let component: ComponentView = cli.run(&[
        "component",
        "update",
        &cfg.arg('c', "component-name"),
        &format!("{name} worker_update"),
        component_v2.to_str().unwrap(),
    ])?;
    let component_id = component.component_id;

    cli.run_unit(&[
        "worker",
        "update",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('m', "mode"),
        "auto",
        &cfg.arg('t', "target-version"),
        "1",
    ])?;
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
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id = make_component_from_file(
        deps,
        &format!("{name}_worker_invoke_indexed_resource"),
        &cli,
        "counters.wasm",
    )?
    .component_id;
    let worker_name = format!("{name}_worker_invoke_indexed_resource");
    let cfg = &cli.config;

    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        r#"rpc:counters/api.{counter("counter1").inc-by}"#,
        &cfg.arg('a', "arg"),
        "1",
    ])?;
    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        r#"rpc:counters/api.{counter("counter1").inc-by}"#,
        &cfg.arg('a', "arg"),
        "2",
    ])?;
    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        r#"rpc:counters/api.{counter("counter2").inc-by}"#,
        &cfg.arg('a', "arg"),
        "5",
    ])?;
    let result = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        r#"rpc:counters/api.{counter("counter1").get-value}"#,
    ])?;

    assert_eq!(result, json!([3]));

    Ok(())
}
