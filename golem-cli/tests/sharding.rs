use crate::cli::{Cli, CliLive};
use golem_cli::clients::template::TemplateView;
use golem_cli::model::InvocationKey;
use golem_client::model::VersionedWorkerId;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use libtest_mimic::{Arguments, Conclusion, Failed, Trial};
use rand::prelude::*;
use serde_json::Value;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

pub mod cli;

fn run(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Conclusion {
    let args = Arguments::from_args();

    let mut tests = Vec::new();

    tests.append(&mut all(deps.clone()));

    libtest_mimic::run(&args, tests)
}

fn main() -> Result<(), Failed> {
    env_logger::init();

    let deps: Arc<dyn TestDependencies + Send + Sync + 'static> =
        Arc::new(EnvBasedTestDependencies::new(10));
    let cluster = deps.worker_executor_cluster(); // forcing startup by getting it
    info!("Using cluster with {:?} worker executors", cluster.size());

    let (stop_tx, stop_rx) = std::sync::mpsc::channel();

    let deps_clone = deps.clone();
    let context_handler = std::thread::spawn(move || {
        make_env_unstable(deps_clone, stop_rx);
    });

    let res = run(deps.clone());

    stop_tx.send(()).unwrap();
    context_handler.join().unwrap();

    drop(cluster);
    drop(deps);

    res.exit()
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let cli = CliLive::make(deps.clone()).unwrap().with_long_args();
    let ctx = (deps.clone(), cli);
    vec![Trial::test_in_context(
        "service_is_responsive_to_shard_changes".to_string(),
        ctx.clone(),
        service_is_responsive_to_shard_changes,
    )]
}

enum Command {
    StartShard,
    StopShard,
    RestartShardManager,
}

fn start_shard(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) {
    let mut stopped = deps.worker_executor_cluster().stopped_indices();
    if !stopped.is_empty() {
        let mut rng = thread_rng();
        stopped.shuffle(&mut rng);

        deps.worker_executor_cluster().start(stopped[0]);
    }
}

fn stop_shard(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) {
    let mut started = deps.worker_executor_cluster().started_indices();
    if !started.is_empty() {
        let mut rng = thread_rng();
        started.shuffle(&mut rng);

        deps.worker_executor_cluster().stop(started[0]);
    }
}

fn reload_shard_manager(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) {
    deps.shard_manager().kill();
    deps.shard_manager().restart();
}

fn make_env_unstable(
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    stop_rx: Receiver<()>,
) {
    println!("!!! Starting Golem Sharding Tester");

    fn worker(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) {
        let mut commands = [
            Command::StartShard,
            Command::StopShard,
            Command::RestartShardManager,
        ];
        let mut rng = thread_rng();
        commands.shuffle(&mut rng);
        match commands[0] {
            Command::StartShard => {
                println!("!!! Golem Sharding Tester starting shard");
                start_shard(deps.clone());
                println!("!!! Golem Sharding Tester started shard");
            }
            Command::StopShard => {
                println!("!!! Golem Sharding Tester stopping shard");
                stop_shard(deps.clone());
                println!("!!! Golem Sharding Tester stopped shard");
            }
            Command::RestartShardManager => {
                println!("!!! Golem Sharding Tester reloading shard manager");
                reload_shard_manager(deps.clone());
                println!("!!! Golem Sharding Tester reloaded shard manager");
            }
        }
    }

    while stop_rx.try_recv().is_err() {
        let mut rng = thread_rng();
        let n = rng.gen_range(1..10);
        std::thread::sleep(Duration::from_secs(n));
        worker(deps.clone());
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
    let key = get_invocation_key_with_retry(template_id, worker_name, cli)?;
    let res =
        invoke_and_await_result_with_retry(template_id, worker_name, function, params, &key, cli);
    println!("*** WORKER {worker_name} INVOKED ***");
    res
}

fn service_is_responsive_to_shard_changes(
    (context, cli): (Arc<dyn TestDependencies + Send + Sync + 'static>, CliLive),
) -> Result<(), Failed> {
    let template_name = "echo-service-1".to_string();

    let cfg = &cli.config;

    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        context
            .template_directory()
            .join("option-service.wasm")
            .to_str()
            .unwrap(),
    ])?;

    let mut workers_with_names = Vec::new();

    for n in 1..=4 {
        println!("*** WORKER {n} STARTING ***");
        let worker_name = format!("echo-service-{n}");
        let worker_id = upload_and_start_worker(&template, &worker_name, &cli)?;
        println!("*** WORKER {n} STARTED ***");
        workers_with_names.push((worker_id, worker_name))
    }

    println!("*** ALL WORKERS STARTED ***");

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

    for c in 0..2 {
        if c != 0 {
            std::thread::sleep(Duration::from_secs(10));
        }
        println!("*** INVOKING WORKERS {c} ***");
        invoke_and_await_workers(&workers_with_names, &cli)?;
        println!("*** INVOKING WORKERS {c} DONE ***");
    }

    println!("*** TEST COMPLETED ***");

    Ok(())
}
