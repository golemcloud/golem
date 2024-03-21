use crate::cli::{Cli, CliLive};
use crate::context::shard_manager::ShardManager;
use crate::context::worker::WorkerExecutor;
use crate::context::{Context, ContextInfo, EnvConfig};
use golem_cli::clients::template::TemplateView;
use golem_cli::model::InvocationKey;
use golem_client::model::VersionedWorkerId;
use libtest_mimic::Failed;
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
use testcontainers::clients;

pub mod cli;
pub mod context;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Scenario {
    pub workers_count: usize,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Step {
    StartShards(usize),
    StopShards(usize),
    StopAllShards,
    RestartShardManager,
    Sleep(Duration),
    InvokeAndAwaitWorkersAsync(String),
    WaitForInvokeAndAwaitResult,
}

fn read_scenario() -> Scenario {
    let file_path_str =
        std::env::var("GOLEM_TEST_SCENARIO").unwrap_or("./test-files/scenario.json".to_string());
    println!("Reading scenario from {file_path_str}");

    let path = PathBuf::from(file_path_str);

    let file = std::fs::File::open(&path).unwrap();

    serde_json::from_reader(file).unwrap()
}

#[test]
fn service_is_responsive_to_shard_changes() -> Result<(), Failed> {
    env_logger::init();

    let (context_tx, context_rx) = std::sync::mpsc::channel();
    let (env_command_tx, env_command_rx) = std::sync::mpsc::channel();
    let (env_event_tx, env_event_rx) = std::sync::mpsc::channel();

    let context_handler = std::thread::spawn(move || {
        let docker = clients::Cli::default();
        let context = Context::start(&docker, EnvConfig::from_env_with_shards(3)).unwrap();

        let context_info = context.info();

        context_tx.send(context_info).unwrap();

        env_handler(context, env_command_rx, env_event_tx);

        drop(docker);
    });

    let context_info = context_rx.recv().unwrap();

    let cli = CliLive::make(&context_info)?.with_long_args();

    service_is_responsive_to_shard_changes_run(
        context_info,
        read_scenario(),
        env_command_tx,
        env_event_rx,
        cli,
    )?;

    context_handler.join().unwrap();

    Ok(())
}

enum EnvCommand {
    StartShards(usize),
    StopShards(usize),
    StopAllShards,
    RestartShardManager,
    Stop,
}

enum EnvEvent {
    StartShardsDone,
    StopShardsDone,
    StopAllShardsDone,
    RestartShardManagerDone,
}

fn start_shard(context: &mut Context) {
    let used_ids: HashSet<u16> = context
        .worker_executors
        .worker_executors
        .iter()
        .map(|we| we.shard_id)
        .collect();
    let mut ids = (0..10)
        .into_iter()
        .filter(|i| !used_ids.contains(i))
        .collect::<Vec<_>>();
    let mut rng = thread_rng();
    ids.shuffle(&mut rng);

    match ids.get(0) {
        Some(id) => {
            match WorkerExecutor::start(
                context.docker,
                *id,
                &context.env,
                &context.redis.info(),
                &context.golem_worker_service.info(),
                &context.golem_template_service.info(),
                &context.shard_manager.as_ref().unwrap().info(),
            ) {
                Ok(we) => context.worker_executors.worker_executors.push(we),
                Err(e) => {
                    println!("Failed to start worker: {e:?}");
                }
            }
        }
        None => {}
    }
}

fn start_shards(context: &mut Context, n: usize) {
    for _ in 1..=n {
        start_shard(context)
    }
}

fn stop_shard(context: &mut Context) {
    let len = context.worker_executors.worker_executors.len();

    if len == 0 {
        return;
    }

    let mut rng = thread_rng();
    let i = rng.gen_range(0..len);
    let we = context.worker_executors.worker_executors.remove(i);
    drop(we) // Not needed. Just making it explicit;
}

fn stop_shards(context: &mut Context, n: usize) {
    for _ in 1..=n {
        stop_shard(context)
    }
}

fn stop_all_shards(context: &mut Context) {
    stop_shards(context, context.worker_executors.worker_executors.len())
}

fn reload_shard_manager(context: &mut Context) {
    let old_shard_manager = context.shard_manager.take();
    drop(old_shard_manager); // Important! We should stop the old one first.
    match ShardManager::start(context.docker, &context.env, &context.redis.info()) {
        Ok(shard_manager) => context.shard_manager = Some(shard_manager),
        Err(e) => {
            println!("!!! Failed to start shard manager: {e:?}");
        }
    }
}

fn env_handler(context: Context, command_rx: Receiver<EnvCommand>, event_tx: Sender<EnvEvent>) {
    let mut context = context;

    println!("!!! Starting Golem Sharding Tester");

    loop {
        match command_rx.recv().unwrap() {
            EnvCommand::StartShards(n) => {
                println!("!!! Golem Sharding Tester starting shards({n})");
                start_shards(&mut context, n);
                println!("!!! Golem Sharding Tester started shards({n})");
                event_tx.send(EnvEvent::StartShardsDone).unwrap();
            }
            EnvCommand::StopShards(n) => {
                println!("!!! Golem Sharding Tester stopping shards{n}");
                stop_shards(&mut context, n);
                println!("!!! Golem Sharding Tester stopped shard{n}");
                event_tx.send(EnvEvent::StopShardsDone).unwrap();
            }
            EnvCommand::StopAllShards => {
                println!("!!! Golem Sharding Tester stopping all shards");
                stop_all_shards(&mut context);
                println!("!!! Golem Sharding Tester stopped all shard");
                event_tx.send(EnvEvent::StopAllShardsDone).unwrap();
            }
            EnvCommand::RestartShardManager => {
                println!("!!! Golem Sharding Tester reloading shard manager");
                reload_shard_manager(&mut context);
                println!("!!! Golem Sharding Tester reloaded shard manager");
                event_tx.send(EnvEvent::RestartShardManagerDone).unwrap();
            }
            EnvCommand::Stop => break,
        }
    }
}

fn upload_and_start_worker(
    template: &TemplateView,
    worker_name: &str,
    cli: &CliLive,
) -> Result<VersionedWorkerId, Failed> {
    let cfg = &cli.config;

    let worker_id: VersionedWorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template.template_id,
    ])?;

    Ok(worker_id)
}

fn get_invocation_key(
    template_id: &str,
    worker_name: &str,
    cli: &CliLive,
) -> Result<InvocationKey, Failed> {
    let cfg = &cli.config;

    let key: InvocationKey = cli.run(&[
        "worker",
        "invocation-key",
        &cfg.arg('T', "template-id"),
        template_id,
        &cfg.arg('w', "worker-name"),
        worker_name,
    ])?;

    Ok(key)
}

fn get_invocation_key_with_retry(
    template_id: &str,
    worker_name: &str,
    cli: &CliLive,
) -> Result<InvocationKey, Failed> {
    loop {
        match get_invocation_key(template_id, worker_name, cli) {
            Ok(key) => return Ok(key),
            Err(_) => {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn invoke_and_await_result(
    template_id: &str,
    worker_name: &str,
    function: &str,
    params: &str,
    key: &InvocationKey,
    cli: &CliLive,
) -> Result<Value, Failed> {
    let cfg = &cli.config;

    cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        &function,
        &cfg.arg('j', "parameters"),
        &params,
        &cfg.arg('k', "invocation-key"),
        &key.0,
    ])
}

fn invoke_and_await_result_with_retry(
    template_id: &str,
    worker_name: &str,
    function: &str,
    params: &str,
    key: &InvocationKey,
    cli: &CliLive,
) -> Result<Value, Failed> {
    loop {
        match invoke_and_await_result(template_id, worker_name, function, params, key, cli) {
            Ok(res) => return Ok(res),
            Err(e) => {
                if e.message()
                    .iter()
                    .any(|m| m.contains("Invalid invocation key"))
                {
                    return get_invocation_key_invoke_and_await_with_retry(
                        template_id,
                        worker_name,
                        function,
                        params,
                        cli,
                    );
                } else {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
}

fn get_invocation_key_invoke_and_await_with_retry(
    template_id: &str,
    worker_name: &str,
    function: &str,
    params: &str,
    cli: &CliLive,
) -> Result<Value, Failed> {
    let key = get_invocation_key_with_retry(&template_id, &worker_name, &cli)?;
    let res =
        invoke_and_await_result_with_retry(template_id, worker_name, function, params, &key, cli);
    println!("*** WORKER {worker_name} INVOKED ***");
    res
}

fn service_is_responsive_to_shard_changes_run(
    context: ContextInfo,
    scenario: Scenario,
    env_command_tx: Sender<EnvCommand>,
    env_event_rx: Receiver<EnvEvent>,
    cli: CliLive,
) -> Result<(), Failed> {
    let template_name = "echo-service-1".to_string();

    let cfg = &cli.config;

    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        context
            .env
            .wasm_root
            .join("option-service.wasm")
            .to_str()
            .unwrap(),
    ])?;

    let mut workers_with_names = Vec::new();

    for n in 1..=scenario.workers_count {
        println!("*** WORKER {n} STARTING ***");
        let worker_name = format!("echo-service-{n}");
        let worker_id = upload_and_start_worker(&template, &worker_name, &cli)?;
        println!("*** WORKER {n} STARTED ***");
        workers_with_names.push((worker_id, worker_name))
    }

    println!("*** ALL WORKERS STARTED ***");

    let (worker_command_tx, worker_command_rx) = std::sync::mpsc::channel();
    let (worker_event_tx, worker_event_rx) = std::sync::mpsc::channel();

    let workers_handler = std::thread::spawn(move || {
        let workers_with_names = workers_with_names;
        let cli = cli;

        loop {
            match worker_command_rx.recv().unwrap() {
                WorkerCommand::InvokeAndAwaitWorkers(name) => {
                    println!("*** INVOKING WORKERS: {name} ***");
                    invoke_and_await_workers(&workers_with_names, &cli).unwrap();
                    println!("*** INVOKING WORKERS {name} DONE ***");
                    worker_event_tx
                        .send(WorkerEvent::InvokeAndAwaitWorkersCompleted(name))
                        .unwrap();
                }
                WorkerCommand::Stop => break,
            }
        }
    });

    for step in scenario.steps {
        match step {
            Step::StartShards(n) => {
                env_command_tx.send(EnvCommand::StartShards(n)).unwrap();
                let _ = env_event_rx.recv().unwrap();
            }
            Step::StopShards(n) => {
                env_command_tx.send(EnvCommand::StopShards(n)).unwrap();
                let _ = env_event_rx.recv().unwrap();
            }
            Step::StopAllShards => {
                env_command_tx.send(EnvCommand::StopAllShards).unwrap();
                let _ = env_event_rx.recv().unwrap();
            }
            Step::RestartShardManager => {
                env_command_tx
                    .send(EnvCommand::RestartShardManager)
                    .unwrap();
                let _ = env_event_rx.recv().unwrap();
            }
            Step::Sleep(duration) => {
                std::thread::sleep(duration);
            }
            Step::InvokeAndAwaitWorkersAsync(name) => {
                worker_command_tx
                    .send(WorkerCommand::InvokeAndAwaitWorkers(name))
                    .unwrap();
            }
            Step::WaitForInvokeAndAwaitResult => {
                let _ = worker_event_rx.recv().unwrap();
            }
        }
    }

    worker_command_tx.send(WorkerCommand::Stop).unwrap();
    env_command_tx.send(EnvCommand::Stop).unwrap();

    workers_handler.join().unwrap();

    Ok(())
}

enum WorkerCommand {
    InvokeAndAwaitWorkers(String),
    Stop,
}

enum WorkerEvent {
    InvokeAndAwaitWorkersCompleted(String),
}

fn invoke_and_await_workers(
    workers: &[(VersionedWorkerId, String)],
    cli: &CliLive,
) -> Result<(), Failed> {
    let mut tasks = Vec::new();

    for (worker, name) in workers {
        let name = name.clone();
        let template_id = worker.worker_id.template_id.to_string();
        let cli = cli.clone();
        tasks.push(std::thread::spawn(move || {
            get_invocation_key_invoke_and_await_with_retry(
                &template_id,
                &name,
                "golem:it/api/echo",
                r#"["Hello"]"#,
                &cli,
            )
        }));
    }

    for task in tasks {
        let _ = task.join().unwrap()?;
    }

    Ok(())
}
