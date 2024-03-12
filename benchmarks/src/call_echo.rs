use crate::benchmark::{Benchmark, BenchmarkConfig};
use crate::context::{Context, ContextInfo, EnvConfig, K8sNamespace, K8sRoutingType, Runtime};
use anyhow::{anyhow, Result};
use std::io::BufRead;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use golem_client::model::{VersionedWorkerId, WorkerId};
use serde_json::Value;
use testcontainers::clients;
use golem_cli::clients::template::TemplateView;
use golem_cli::model::InvocationKey;
use crate::cli::{Cli, CliLive};

pub mod benchmark;
pub mod cli;
pub mod context;

#[derive(Debug, Clone)]
pub struct CallEchoConfig {
    iterations: u16,
    n_templates: u16,
    n_workers_per_template: u16,
    n_calls_per_worker: u16,
    n_worker_executors: u16,
}

#[derive(Debug, Clone)]
pub struct CallEchoState {
    run_id: u16,
    workers: Vec<WorkerId>,
}

fn run(docker: &clients::Cli) -> Result<()> {
    let benchamrk = Benchmark {
        make_context: Box::new(make_context),
        wait_for_startup: Box::new(wait_for_startup),
        init_state: Box::new(init_state),
        prepare: Box::new(prepare),
        validate_state: Box::new(validate_state),
        warmup: Box::new(warmup),
        workload: Box::new(workload),
        cleanup: Box::new(cleanup),
    };

    let config = CallEchoConfig {
        iterations: 3,
        n_templates: 10,
        n_workers_per_template: 10,
        n_calls_per_worker: 10,
        n_worker_executors: 5,
    };

    let benchmark_config = BenchmarkConfig {
        iterations: config.iterations
    };

    let res = benchamrk.run(docker, config, benchmark_config)?;

    println!("Result: {}", serde_json::to_string_pretty(&res)?);

    Ok(())
}

fn main() -> Result<()> {
    env_logger::init();

    let docker = clients::Cli::default();

    run(&docker)?;

    // for _ in std::io::stdin().lock().lines() {}

    drop(docker);

    Ok(())
}

async fn make_context_async<'docker_client>(
    docker: &'docker_client clients::Cli,
    config: CallEchoConfig,
) -> Result<Context<'docker_client>> {
    let mut env_conf = EnvConfig::from_env();
    env_conf.runtime = Runtime::K8S{
        namespace: K8sNamespace("benchmark".to_string()),
        routing: K8sRoutingType::Ingress
    };
    env_conf.schema = "http".to_string();
    env_conf.n_worker_executors = config.n_worker_executors;

    Context::start(docker, env_conf).await
}

fn make_context<'docker_client>(
    docker: &'docker_client clients::Cli,
    config: CallEchoConfig,
) -> Result<Context<'docker_client>> {
    return tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime")
        .block_on(make_context_async(docker, config));
}

fn wait_for_startup(context: ContextInfo) -> Result<()> {
    fn create_template_unsafe(context: &ContextInfo) -> Result<TemplateView> {
        let cli = CliLive::make(context)?.with_long_args();
        let cfg = &cli.config;
        let template: TemplateView = cli.run(&[
            "template",
            "add",
            &cfg.arg('t', "template-name"),
            "echo_wait",
            context
                .env
                .wasm_root
                .join("option-service.wasm")
                .to_str().ok_or(anyhow!("Can't print path."))?,
        ])?;

        Ok(template)
    }

    fn create_template(context: &ContextInfo) -> TemplateView {
        let mut n = 0;
        loop {
            match create_template_unsafe(context) {
                Ok(template) => return template,
                Err(e) => {
                    n += 1;
                    println!("Failed to create template: {e:?}. Retry #{n}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    fn start_worker_unsafe(context: &ContextInfo, template: &TemplateView) -> Result<VersionedWorkerId> {
        let cli = CliLive::make(context)?.with_long_args();
        let cfg = &cli.config;
        let worker_id: VersionedWorkerId = cli.run(&[
            "worker",
            "add",
            &cfg.arg('w', "worker-name"),
            "wait",
            &cfg.arg('T', "template-id"),
            &template.template_id,
        ])?;

        Ok(worker_id)
    }

    fn start_worker(context: &ContextInfo, template: &TemplateView) -> VersionedWorkerId {
        let mut n = 0;
        loop {
            match start_worker_unsafe(context, template) {
                Ok(worker_id) => return worker_id,
                Err(e) => {
                    n += 1;
                    println!("Failed to start worker: {e:?}. Retry #{n}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    fn get_invocation_key(context: &ContextInfo, worker_id: &WorkerId) -> InvocationKey {
        let mut n = 0;
        loop {
            match get_invocation_key_unsafe(context, worker_id) {
                Ok(key) => return key,
                Err(e) => {
                    n += 1;
                    println!("Failed to get invocation key: {e:?}. Retry #{n}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    fn get_invocation_key_invoke_and_await_unsafe(context: &ContextInfo, worker_id: &WorkerId, function: &str, params: &str) -> Result<Value> {
        let key = get_invocation_key(context, worker_id);
        invoke_and_await_result(context, worker_id, function, params, &key)
    }

    fn get_invocation_key_invoke_and_await_with_retry(context: &ContextInfo, worker_id: &WorkerId, function: &str, params: &str) -> Value {
        let mut n = 0;
        loop {
            match get_invocation_key_invoke_and_await_unsafe(context, worker_id, function, params) {
                Ok(res) => return res,
                Err(e) => {
                    n += 1;
                    println!("Failed to invoke: {e:?}. Retry #{n}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    println!("Initial template creation.");
    let template = create_template(&context);
    println!("Initial worker creation.");
    let worker_id = start_worker(&context, &template);

    for i in 1..=100 {
        println!("Initial worker call {i}/100");
        let _ = get_invocation_key_invoke_and_await_with_retry(&context, &worker_id.worker_id, "golem:it/api/echo", r#"["Hello"]"#);
    }

    Ok(())
}

fn get_invocation_key_unsafe(context: &ContextInfo, worker_id: &WorkerId) -> Result<InvocationKey> {
    let cli = CliLive::make(context)?.with_long_args();
    let cfg = &cli.config;
    let key: InvocationKey = cli.run(&[
        "worker",
        "invocation-key",
        &cfg.arg('T', "template-id"),
        &worker_id.template_id.to_string(),
        &cfg.arg('w', "worker-name"),
        &worker_id.worker_name,
    ])?;

    Ok(key)
}

fn invoke_and_await_result(context: &ContextInfo, worker_id: &WorkerId, function: &str, params: &str, key: &InvocationKey) -> Result<Value> {
    let cli = CliLive::make(context)?.with_long_args();
    let cfg = &cli.config;
    let res = cli.run_json(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('T', "template-id"),
        &worker_id.template_id.to_string(),
        &cfg.arg('w', "worker-name"),
        &worker_id.worker_name,
        &cfg.arg('f', "function"),
        &function,
        &cfg.arg('j', "parameters"),
        &params,
        &cfg.arg('k', "invocation-key"),
        &key.0,
    ])?;

    Ok(res)
}

fn init_state() -> Option<CallEchoState> {
    None
}

fn prepare(
    context: ContextInfo,
    config: CallEchoConfig,
    old_state: Option<CallEchoState>,
) -> Result<CallEchoState> {
    let run_id = old_state.map(|s| s.run_id + 1).unwrap_or(0);

    println!("Creating env for workload run {run_id}");

    let cli = CliLive::make(&context)?.with_long_args();
    let cfg = &cli.config;

    let mut workers = Vec::with_capacity((config.n_templates * config.n_workers_per_template) as usize);

    for t_n in 0..config.n_templates {
        let template_name = format!("run{run_id}t{t_n}");

        let template: TemplateView = cli.run(&[
            "template",
            "add",
            &cfg.arg('t', "template-name"),
            &template_name,
            context
                .env
                .wasm_root
                .join("option-service.wasm")
                .to_str().ok_or(anyhow!("Can't print path."))?,
        ])?;

        for w_n in 0..config.n_workers_per_template {
            let worker_id: VersionedWorkerId = cli.run(&[
                "worker",
                "add",
                &cfg.arg('w', "worker-name"),
                &format!("{template_name}w{w_n}"),
                &cfg.arg('T', "template-id"),
                &template.template_id,
            ])?;

            workers.push(worker_id.worker_id);
        }
    }

    println!("Created env for workload run {run_id}");

    Ok(CallEchoState{
        run_id,
        workers,
    })
}

fn validate_state(context: ContextInfo, state: CallEchoState) -> Result<()> {
    Ok(())
}

fn warmup(context: ContextInfo, state: CallEchoState) -> Result<()> {
    Ok(())
}

fn workload(context: ContextInfo, state: CallEchoState) -> Result<Duration> {
    let mut joins: Vec<JoinHandle<Result<()>>> = Vec::with_capacity(state.workers.len());

    let start = Instant::now();

    for worker in state.workers {
        let context = context.clone();
        joins.push(std::thread::spawn(move || {
            let key = get_invocation_key_unsafe(&context, &worker)?;
            let _ = invoke_and_await_result(&context, &worker, "golem:it/api/echo", r#"["Hello"]"#, &key)?;

            Ok(())
        }));
    }

    for join in joins {
        join.join().unwrap()?;
    }

    Ok(start.elapsed())
}

fn cleanup(context: Context) -> Result<()> {

    let stop = async move {
        drop(context)
    };

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed building the Runtime")
        .block_on(stop);

    Ok(())
}
