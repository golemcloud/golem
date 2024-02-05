use crate::cli::{Cli, CliLive};
use crate::context::ContextInfo;
use golem_cli::clients::template::TemplateView;
use golem_cli::model::InvocationKey;
use golem_client::model::VersionedWorkerId;
use libtest_mimic::{Failed, Trial};
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::time::Duration;

fn make(suffix: &str, name: &str, cli: CliLive, context: Arc<ContextInfo>) -> Vec<Trial> {
    let ctx = (context, name.to_string(), cli);
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
        Trial::test_in_context(format!("worker_invoke{suffix}"), ctx.clone(), worker_invoke),
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
    ]
}

pub fn all(context: Arc<ContextInfo>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make(&context.golem_service)
            .unwrap()
            .with_short_args(),
        context.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make(&context.golem_service)
            .unwrap()
            .with_long_args(),
        context.clone(),
    );

    short_args.append(&mut long_args);

    short_args
}

fn make_template(
    context: &ContextInfo,
    template_name: &str,
    cli: &CliLive,
) -> Result<TemplateView, Failed> {
    let env_service = context.env.wasi_root.join("environment-service.wasm");
    let cfg = &cli.config;
    Ok(cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        env_service.to_str().unwrap(),
    ])?)
}

fn worker_new_instance(
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let template_id =
        make_template(&context, &format!("{name} worker new instance"), &cli)?.template_id;
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
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let template_id =
        make_template(&context, &format!("{name} worker invocation key"), &cli)?.template_id;
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
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let template_id =
        make_template(&context, &format!("{name} worker_invoke_and_await"), &cli)?.template_id;
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
        &cfg.arg('j', "parameters"),
        "[]",
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

fn worker_invoke((context, name, cli): (Arc<ContextInfo>, String, CliLive)) -> Result<(), Failed> {
    let template_id = make_template(&context, &format!("{name} worker_invoke"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_invoke");
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

fn worker_connect((context, name, cli): (Arc<ContextInfo>, String, CliLive)) -> Result<(), Failed> {
    let cfg = &cli.config;

    let stdout_service = context.env.wasi_root.join("write-stdout.wasm");
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

    Ok(())
}

fn worker_connect_failed(
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let stdout_service = context.env.wasi_root.join("write-stdout.wasm");
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
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let interruption_service = context.env.wasi_root.join("interruption.wasm");
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
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let cfg = &cli.config;

    let interruption_service = context.env.wasi_root.join("interruption.wasm");
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
