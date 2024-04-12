use crate::cli::{Cli, CliLive};
use golem_cli::model::invoke_result_view::InvokeResultView;
use golem_cli::model::template::TemplateView;
use golem_cli::model::InvocationKey;
use golem_client::model::{VersionedWorkerId, WorkersMetadataResponse};
use golem_test_framework::config::TestDependencies;
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
            format!("worker_get_invocation_key{suffix}"),
            ctx.clone(),
            worker_get_invocation_key,
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
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let short_cli = CliLive::make(deps.clone()).unwrap().with_short_args();
    let mut short_args = make("_short", "CLI_short", short_cli.clone(), deps.clone());

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make(deps.clone()).unwrap().with_long_args(),
        deps.clone(),
    );

    short_args.append(&mut long_args);
    short_args
}

fn make_template_from_file(
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    template_name: &str,
    cli: &CliLive,
    file: &str,
) -> Result<TemplateView, Failed> {
    let env_service = deps.template_directory().join(file);
    let cfg = &cli.config;
    cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        env_service.to_str().unwrap(),
    ])
}

fn make_template(
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    template_name: &str,
    cli: &CliLive,
) -> Result<TemplateView, Failed> {
    make_template_from_file(deps, template_name, cli, "environment-service.wasm")
}

fn worker_new_instance(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id =
        make_template(deps, &format!("{name} worker new instance"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_new_instance");
    let cfg = &cli.config;
    let worker_id: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;

    assert_eq!(worker_id.worker_id.template_id.to_string(), template_id);
    assert_eq!(worker_id.worker_id.worker_name, worker_name);
    Ok(())
}

fn worker_get_invocation_key(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id =
        make_template(deps, &format!("{name} worker invocation key"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_invocation_key");
    let cfg = &cli.config;
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;
    let _: InvocationKey = cli.run(&[
        "worker",
        "invocation-key",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
    ])?;
    Ok(())
}

fn worker_invoke_and_await(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id =
        make_template(deps, &format!("{name} worker_invoke_and_await"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_invoke_and_await");
    let cfg = &cli.config;
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('e', "env"),
        "TEST_ENV=test-value",
        "test-arg",
    ])?;
    let args_key: InvocationKey = cli.run(&[
        "worker",
        "invocation-key",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
    ])?;
    let args = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api/get-arguments",
        &cfg.arg('j', "parameters"),
        "[]",
        &cfg.arg('k', "invocation-key"),
        &args_key.0,
    ])?;

    let expected_args = json!([{"ok": ["test-arg"]}]);

    assert_eq!(args, expected_args);

    let env_key: InvocationKey = cli.run(&[
        "worker",
        "invocation-key",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
    ])?;

    let env = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api/get-environment",
        &cfg.arg('k', "invocation-key"),
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
    let template_id = make_template_from_file(
        deps,
        &format!("{name} worker_invoke_and_await_wave_params"),
        &cli,
        "key-value-service.wasm",
    )?
    .template_id;
    let worker_name = format!("{name}_worker_invoke_and_await_wave_params");
    let cfg = &cli.config;
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;
    let res_set: InvokeResultView = cli.run(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('H', "human-readable"),
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api/set",
        &cfg.arg('p', "param"),
        r#""bucket name""#,
        &cfg.arg('p', "param"),
        r#""key name""#,
        &cfg.arg('p', "param"),
        r#"[1, 2, 3]"#,
    ])?;
    assert_eq!(res_set, InvokeResultView::Wave(Vec::new()));

    let res_get: InvokeResultView = cli.run(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('H', "human-readable"),
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api/get",
        &cfg.arg('p', "param"),
        r#""bucket name""#,
        &cfg.arg('p', "param"),
        r#""key name""#,
    ])?;
    assert_eq!(
        res_get,
        InvokeResultView::Wave(vec!["some([1, 2, 3])".to_string()])
    );

    Ok(())
}

fn worker_invoke_no_params(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id =
        make_template(deps, &format!("{name} worker_invoke_no_params"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_invoke_no_params");
    let cfg = &cli.config;
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api/get-arguments",
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
    let template_id =
        make_template(deps, &format!("{name} worker_invoke_json_params"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_invoke_json_params");
    let cfg = &cli.config;
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api/get-arguments",
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
    let template_id = make_template_from_file(
        deps,
        &format!("{name} worker_invoke_wave_params"),
        &cli,
        "key-value-service.wasm",
    )?
    .template_id;
    let worker_name = format!("{name}_worker_invoke_wave_params");
    let cfg = &cli.config;
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "invoke",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api/set",
        &cfg.arg('p', "param"),
        r#""bucket name""#,
        &cfg.arg('p', "param"),
        r#""key name""#,
        &cfg.arg('p', "param"),
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

    let stdout_service = deps.template_directory().join("write-stdout.wasm");
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &format!("{name} worker_connect"),
        stdout_service.to_str().unwrap(),
    ])?;
    let template_id = template.template_id;
    let worker_name = format!("{name}_worker_connect");
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;

    let mut child = cli.run_stdout(&[
        "worker",
        "connect",
        &cfg.arg('T', "template-id"),
        &template_id,
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
        &cfg.arg('T', "template-id"),
        &template_id,
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

    let stdout_service = deps.template_directory().join("write-stdout.wasm");
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &format!("{name} worker_connect_failed"),
        stdout_service.to_str().unwrap(),
    ])?;
    let template_id = template.template_id;
    let worker_name = format!("{name}_worker_connect_failed");

    let mut child = cli.run_stdout(&[
        "worker",
        "connect",
        &cfg.arg('T', "template-id"),
        &template_id,
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

    let interruption_service = deps.template_directory().join("interruption.wasm");
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &format!("{name} worker_interrupt"),
        interruption_service.to_str().unwrap(),
    ])?;
    let template_id = template.template_id;
    let worker_name = format!("{name}_worker_interrupt");
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "interrupt",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
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

    let interruption_service = deps.template_directory().join("interruption.wasm");
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &format!("{name} worker_simulated_crash"),
        interruption_service.to_str().unwrap(),
    ])?;
    let template_id = template.template_id;
    let worker_name = format!("{name}_worker_simulated_crash");
    let _: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;
    cli.run_unit(&[
        "worker",
        "simulated-crash",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
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
    let template_id = make_template(deps, &format!("{name} worker_list"), &cli)?.template_id;
    let cfg = &cli.config;

    let workers_count = 10;
    let mut worker_ids = vec![];

    for i in 0..workers_count {
        let worker_name = format!("{name}_worker-{i}");
        let worker_id: VersionedWorkerId = cli.run(&[
            "worker",
            "add",
            &cfg.arg('w', "worker-name"),
            &worker_name,
            &cfg.arg('T', "template-id"),
            &template_id,
        ])?;

        worker_ids.push(worker_id);
    }

    for worker_id in worker_ids {
        let result: WorkersMetadataResponse = cli.run(&[
            "worker",
            "list",
            &cfg.arg('T', "template-id"),
            &template_id,
            &cfg.arg('f', "filter"),
            format!("name = {}", worker_id.worker_id.worker_name).as_str(),
            &cfg.arg('f', "filter"),
            "version >= 0",
            &cfg.arg('p', "precise"),
            "true",
        ])?;

        assert_eq!(result.workers.len(), 1);
        assert!(result.cursor.is_none());
    }

    let result: WorkersMetadataResponse = cli.run(&[
        "worker",
        "list",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('f', "filter"),
        "version >= 0",
        &cfg.arg('f', "filter"),
        format!("name like {}_worker", name).as_str(),
        &cfg.arg('n', "count"),
        (workers_count / 2).to_string().as_str(),
    ])?;

    assert!(result.workers.len() >= workers_count / 2);
    assert!(result.cursor.is_some());

    let result2: WorkersMetadataResponse = cli.run(&[
        "worker",
        "list",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('f', "filter"),
        "version >= 0",
        &cfg.arg('f', "filter"),
        format!("name like {}_worker", name).as_str(),
        &cfg.arg('n', "count"),
        (workers_count - result.workers.len()).to_string().as_str(),
        &cfg.arg('c', "cursor"),
        result.cursor.unwrap().to_string().as_str(),
    ])?;

    assert_eq!(result2.workers.len(), workers_count - result.workers.len());

    if let Some(cursor2) = result2.cursor {
        let result3: WorkersMetadataResponse = cli.run(&[
            "worker",
            "list",
            &cfg.arg('T', "template-id"),
            &template_id,
            &cfg.arg('f', "filter"),
            "version >= 0",
            &cfg.arg('f', "filter"),
            format!("name like {}_worker", name).as_str(),
            &cfg.arg('n', "count"),
            workers_count.to_string().as_str(),
            &cfg.arg('c', "cursor"),
            cursor2.to_string().as_str(),
        ])?;
        assert_eq!(result3.workers.len(), 0);
    }

    Ok(())
}
