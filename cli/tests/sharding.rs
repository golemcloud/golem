use crate::cli::{Cli, CliLive};
use crate::context::shard_manager::ShardManager;
use crate::context::worker::WorkerExecutor;
use crate::context::{Context, ContextInfo, EnvConfig};
use async_recursion::async_recursion;
use golem_cli::clients::template::TemplateView;
use golem_cli::model::InvocationKey;
use golem_client::model::VersionedWorkerId;
use libtest_mimic::{Arguments, Conclusion, Failed, Trial};
use rand::prelude::*;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use testcontainers::clients;

pub mod cli;
pub mod context;

fn run(context: ContextInfo) -> Conclusion {
    let args = Arguments::from_args();

    let context = Arc::new(context);

    let mut tests = Vec::new();

    tests.append(&mut all(context.clone()));

    libtest_mimic::run(&args, tests)
}

fn main() -> Result<(), Failed> {
    env_logger::init();


    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let docker = clients::Cli::default();
        let context = Context::start(&docker, EnvConfig::from_env_with_shards(0)).unwrap();

        let context_info = context.info();

        tx.send(context_info);

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed building the Runtime")
            .block_on(make_env_unstable(context));

        drop(docker);
    });

    let context_info = rx.recv().unwrap();

    let res = run(context_info);

    res.exit()
}

pub fn all(context: Arc<ContextInfo>) -> Vec<Trial> {
    let cli = CliLive::make(&context).unwrap().with_long_args();
    let ctx = (context, cli);
    vec![Trial::test_in_context(
        format!("service_is_responsive_to_shard_changes"),
        ctx.clone(),
        service_is_responsive_to_shard_changes,
    )]
}

enum Command {
    StartShard,
    StopShard,
    RestartShardManager,
}

async fn start_shard<'docker_client>(context: &mut Context<'docker_client>) {
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
                false,
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

async fn stop_shard<'docker_client>(context: &mut Context<'docker_client>) {
    let len = context.worker_executors.worker_executors.len();

    if len == 0 {
        return;
    }

    let mut rng = thread_rng();
    let i = rng.gen_range(0..len);
    let we = context.worker_executors.worker_executors.remove(i);
    drop(we) // Not needed. Just making it explicit;
}

async fn reload_shard_manager<'docker_client>(context: &mut Context<'docker_client>) {
    let old_shard_manager = context.shard_manager.take();
    drop(old_shard_manager); // Important! We should stop the old one first.
    match ShardManager::start(context.docker, &context.env, &context.redis.info()) {
        Ok(shard_manager) => context.shard_manager = Some(shard_manager),
        Err(e) => {
            println!("!!! Failed to start shard manager: {e:?}");
        }
    }
}

async fn make_env_unstable<'docker_client>(context: Context<'docker_client>) {
    let mut context = context;

    println!("!!! Starting Golem Sharding Tester");

    async fn worker<'docker_client>(context: &mut Context<'docker_client>) {
        let mut commands = vec![
            Command::StartShard,
            Command::StopShard,
            Command::RestartShardManager,
        ];
        let mut rng = rand::thread_rng();
        commands.shuffle(&mut rng);
        match commands[0] {
            Command::StartShard => {
                println!("!!! Golem Sharding Tester starting shard");
                start_shard(context).await;
                println!("!!! Golem Sharding Tester started shard");
            }
            Command::StopShard => {
                println!("!!! Golem Sharding Tester stopping shard");
                stop_shard(context).await;
                println!("!!! Golem Sharding Tester stopped shard");
            }
            Command::RestartShardManager => {
                println!("!!! Golem Sharding Tester reloading shard manager");
                reload_shard_manager(context).await;
                println!("!!! Golem Sharding Tester reloaded shard manager");
            }
        }
    }

    loop {
        let mut rng = rand::thread_rng();
        let n = rng.gen_range(1..10);
        tokio::time::sleep(Duration::from_secs(n)).await;
        worker(&mut context).await;
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

async fn get_invocation_key(
    template_id: String,
    worker_name: String,
    cli: CliLive,
) -> Result<InvocationKey, Failed> {
    tokio::task::spawn_blocking(move || {
        let cfg = &cli.config;

        let key: InvocationKey = cli.run(&[
            "worker",
            "invocation-key",
            &cfg.arg('T', "template-id"),
            &template_id,
            &cfg.arg('w', "worker-name"),
            &worker_name,
        ])?;

        Ok(key)
    })
    .await?
}

async fn get_invocation_key_with_retry(
    template_id: &str,
    worker_name: &str,
    cli: &CliLive,
) -> Result<InvocationKey, Failed> {
    loop {
        match get_invocation_key(
            template_id.to_string(),
            worker_name.to_string(),
            cli.clone(),
        )
        .await
        {
            Ok(key) => return Ok(key),
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn invoke_and_await_result(
    template_id: String,
    worker_name: String,
    function: String,
    params: String,
    key: InvocationKey,
    cli: CliLive,
) -> Result<Value, Failed> {
    tokio::task::spawn_blocking(move || {
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
    })
    .await?
}

#[async_recursion]
async fn invoke_and_await_result_with_retry(
    template_id: &str,
    worker_name: &str,
    function: &str,
    params: &str,
    key: InvocationKey,
    cli: &CliLive,
) -> Result<Value, Failed> {
    loop {
        match invoke_and_await_result(
            template_id.to_string(),
            worker_name.to_string(),
            function.to_string(),
            params.to_string(),
            key.clone(),
            cli.clone(),
        )
        .await
        {
            Ok(res) => return Ok(res),
            Err(e) => {
                if e.message()
                    .iter()
                    .any(|m| m.contains("Invalid invocation key"))
                {
                    return get_invocation_key_invoke_and_await_with_retry(
                        template_id.to_string(),
                        worker_name.to_string(),
                        function.to_string(),
                        params.to_string(),
                        cli.clone(),
                    )
                    .await;
                } else {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}

async fn get_invocation_key_invoke_and_await_with_retry(
    template_id: String,
    worker_name: String,
    function: String,
    params: String,
    cli: CliLive,
) -> Result<Value, Failed> {
    let key = get_invocation_key_with_retry(&template_id, &worker_name, &cli).await?;
    let res = invoke_and_await_result_with_retry(
        &template_id,
        &worker_name,
        &function,
        &params,
        key,
        &cli,
    )
    .await;
    println!("*** WORKER {worker_name} INVOKED ***");
    res
}

async fn service_is_responsive_to_shard_changes_async(
    context: Arc<ContextInfo>,
    cli: CliLive,
) -> Result<(), Failed> {
    let template_name = "echo-service-1".to_string();

    let cfg = &cli.config;

    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        context.env.wasm_root.join("option-service.wasm").to_str().unwrap(),
    ])?;

    let mut workers_with_names = Vec::new();

    for n in 1..=4 {
        println!("*** WORKER {n} STARTING ***");
        let worker_name = format!("echo-service-{n}");
        let worker_id = upload_and_start_worker(
            &template,
            &worker_name,
            &cli,
        )?;
        println!("*** WORKER {n} STARTED ***");
        workers_with_names.push((worker_id, worker_name))
    }

    println!("*** ALL WORKERS STARTED ***");

    async fn invoke_and_await_workers(
        workers: &[(VersionedWorkerId, String)],
        cli: &CliLive,
    ) -> Result<(), Failed> {
        let mut tasks = Vec::new();

        for (worker, name) in workers {
            let template_id = worker.worker_id.template_id.to_string();
            tasks.push(tokio::spawn(
                get_invocation_key_invoke_and_await_with_retry(
                    template_id,
                    name.to_string(),
                    "golem:it/api/echo".to_string(),
                    r#"["Hello"]"#.to_string(),
                    cli.clone(),
                ),
            ));
        }

        for task in tasks {
            let _ = task.await??;
        }

        Ok(())
    }

    for c in 0..2 {
        if c != 0 {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        println!("*** INVOKING WORKERS {c} ***");
        invoke_and_await_workers(&workers_with_names, &cli).await?;
        println!("*** INVOKING WORKERS {c} DONE ***");
    }

    println!("*** TEST COMPLETED ***");

    Ok(())
}

fn service_is_responsive_to_shard_changes(
    (context, cli): (Arc<ContextInfo>, CliLive),
) -> Result<(), Failed> {
    return tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime")
        .block_on(service_is_responsive_to_shard_changes_async(context, cli));
}
