use std::ops::Deref;
use std::time::Duration;

use ctor::{ctor, dtor};
use golem_wasm_rpc::Value;
use rand::prelude::*;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use golem_api_grpc::proto::golem::worker;
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;

struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        // let console_layer = console_subscriber::spawn().with_filter(
        //     EnvFilter::try_new("trace").unwrap()
        //);
        let ansi_layer = tracing_subscriber::fmt::layer()
            .with_ansi(true)
            .with_filter(
                EnvFilter::try_new("debug,cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn,fred=warn").unwrap()
            );

        tracing_subscriber::registry()
            // .with(console_layer) // Uncomment this to use tokio-console. Also needs RUSTFLAGS="--cfg tokio_unstable"
            .with(ansi_layer)
            .init();

        Self
    }
}

#[ctor]
pub static DEPS: EnvBasedTestDependencies = {
    let deps = EnvBasedTestDependencies::blocking_new(10);

    deps.redis_monitor().assert_valid();
    println!(
        "Started a cluster of {} worker executors",
        deps.worker_executor_cluster().size()
    );

    deps
};

#[dtor]
unsafe fn drop_deps() {
    let base_deps_ptr = DEPS.deref() as *const EnvBasedTestDependencies;
    let base_deps_ptr = base_deps_ptr as *mut EnvBasedTestDependencies;
    (*base_deps_ptr).kill_all()
}

#[ctor]
pub static TRACING: Tracing = Tracing::init();

#[tokio::test]
#[tracing::instrument]
#[ignore] // TODO: Re-enable when sharding manager is fixed
async fn service_is_responsive_to_shard_changes() {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let chaos = std::thread::spawn(|| {
        unstable_environment(stop_rx);
    });

    let component_id = DEPS.store_component("option-service").await;

    let mut worker_ids = Vec::new();

    for n in 1..=4 {
        info!("Worker {n} starting");
        let worker_name = format!("sharding-test-1-{n}");
        let worker_id = DEPS.start_worker(&component_id, &worker_name).await;
        info!("Worker {n} started");
        worker_ids.push(worker_id);
    }

    info!("All workers started");

    for c in 0..2 {
        if c != 0 {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        info!("Invoking workers ({c})");
        invoke_and_await_workers(&worker_ids)
            .await
            .expect("Invocations failed");
        info!("Invoking workers done ({c})");
    }

    info!("Sharding test completed");
    stop_tx.send(()).unwrap();
    chaos.join().unwrap();
}

#[tokio::test]
#[tracing::instrument]
#[ignore] // TODO: Re-enable when sharding manager is fixed
async fn coordinated_scenario1() {
    coordinated_scenario(
        1,
        vec![
            Step::StopAllShards,
            Step::InvokeAndAwaitWorkersAsync(
                "Invoke, RestartShardManager, StartShards".to_string(),
            ),
            Step::RestartShardManager,
            Step::Sleep(Duration::from_secs(3)),
            Step::StartShards(4),
            Step::WaitForInvokeAndAwaitResult,
            Step::StopAllShards,
            Step::RestartShardManager,
            Step::StartShards(4),
            Step::RestartShardManager,
            Step::InvokeAndAwaitWorkersAsync(
                "StartShards, RestartShardManager, Invoke".to_string(),
            ),
            Step::WaitForInvokeAndAwaitResult,
            Step::StopAllShards,
            Step::RestartShardManager,
            Step::StartShards(4),
            Step::StopShards(3),
            Step::Sleep(Duration::from_secs(3)),
            Step::InvokeAndAwaitWorkersAsync("StartShards(4), StopShards(3), Invoke".to_string()),
            Step::WaitForInvokeAndAwaitResult,
        ],
    )
    .await;
}
async fn coordinated_scenario(id: usize, steps: Vec<Step>) {
    let (worker_command_tx, worker_command_rx) = tokio::sync::mpsc::channel(128);
    let (worker_event_tx, mut worker_event_rx) = tokio::sync::mpsc::channel(128);
    let (env_command_tx, env_command_rx) = std::sync::mpsc::channel();
    let (env_event_tx, env_event_rx) = std::sync::mpsc::channel();

    let chaos = std::thread::spawn(|| {
        coordinated_environment(env_command_rx, env_event_tx);
    });

    let invoker = tokio::task::spawn(async {
        worker_invocation(worker_command_rx, worker_event_tx).await;
    });

    let component_id = DEPS.store_component("option-service").await;

    let mut worker_ids = Vec::new();

    for n in 1..=4 {
        info!("Worker {n} starting");
        let worker_name = format!("sharding-test-{id}-{n}");
        let worker_id = DEPS.start_worker(&component_id, &worker_name).await;
        info!("Worker {n} started");
        worker_ids.push(worker_id);
    }

    info!("All workers started");

    for step in steps {
        let formatted_step = format!("{:?}", step);
        info!("Executing step: {}", formatted_step);
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
                tokio::time::sleep(duration).await;
            }
            Step::InvokeAndAwaitWorkersAsync(name) => {
                worker_command_tx
                    .send(WorkerCommand::InvokeAndAwaitWorkers {
                        name,
                        worker_ids: worker_ids.clone(),
                    })
                    .await
                    .unwrap();
            }
            Step::WaitForInvokeAndAwaitResult => {
                let evt = worker_event_rx.recv().await.unwrap();
                info!("Invoke and await completed: {evt:?}");
            }
        }
        info!("Executed step: {}", formatted_step);
    }

    info!("Sharding test completed");
    worker_command_tx.send(WorkerCommand::Stop).await.unwrap();
    env_command_tx.send(EnvCommand::Stop).unwrap();
    chaos.join().unwrap();
    invoker.await.unwrap();
}

enum Command {
    StartShard,
    StopShard,
    RestartShardManager,
}

fn start_shard() {
    let mut stopped = DEPS.worker_executor_cluster().stopped_indices();
    if !stopped.is_empty() {
        let mut rng = thread_rng();
        stopped.shuffle(&mut rng);

        DEPS.worker_executor_cluster().blocking_start(stopped[0]);
    }
}

fn start_shards(n: usize) {
    let mut stopped = DEPS.worker_executor_cluster().stopped_indices();
    if !stopped.is_empty() {
        let mut rng = thread_rng();
        stopped.shuffle(&mut rng);

        let to_start = &stopped[0..n];

        for idx in to_start {
            DEPS.worker_executor_cluster().blocking_start(*idx);
        }
    }
}

fn stop_shard() {
    let mut started = DEPS.worker_executor_cluster().started_indices();
    if !started.is_empty() {
        let mut rng = thread_rng();
        started.shuffle(&mut rng);

        DEPS.worker_executor_cluster().stop(started[0]);
    }
}

fn stop_shards(n: usize) {
    let mut started = DEPS.worker_executor_cluster().started_indices();
    if !started.is_empty() {
        let mut rng = thread_rng();
        started.shuffle(&mut rng);

        let to_stop = &started[0..n];

        for idx in to_stop {
            DEPS.worker_executor_cluster().stop(*idx);
        }
    }
}

fn stop_all_shards() {
    let started = DEPS.worker_executor_cluster().started_indices();
    for idx in started {
        DEPS.worker_executor_cluster().stop(idx);
    }
}

fn reload_shard_manager() {
    DEPS.shard_manager().kill();
    DEPS.shard_manager().blocking_restart();
}

async fn invoke_and_await_workers(workers: &[WorkerId]) -> Result<(), worker::worker_error::Error> {
    let mut tasks = Vec::new();

    for worker_id in workers {
        let worker_id_clone = worker_id.clone();
        tasks.push((
            worker_id,
            tokio::spawn(async move {
                let idempotency_key = IdempotencyKey::fresh();
                DEPS.invoke_and_await_with_key(
                    &worker_id_clone,
                    &idempotency_key,
                    "golem:it/api.{echo}",
                    vec![Value::Option(Some(Box::new(Value::String(
                        "Hello".to_string(),
                    ))))],
                )
                .await
            }),
        ));
    }

    for (worker_id, task) in tasks {
        info!("Awaiting worker: {}", worker_id);
        let _ = task.await.unwrap()?;
        info!("Worker finished: {}", worker_id);
    }

    Ok(())
}

#[derive(Debug, Clone)]
enum Step {
    StartShards(usize),
    StopShards(usize),
    StopAllShards,
    RestartShardManager,
    Sleep(Duration),
    InvokeAndAwaitWorkersAsync(String),
    WaitForInvokeAndAwaitResult,
}

enum EnvCommand {
    StartShards(usize),
    StopShards(usize),
    StopAllShards,
    RestartShardManager,
    Stop,
}

#[allow(clippy::enum_variant_names)]
enum EnvEvent {
    StartShardsDone,
    StopShardsDone,
    StopAllShardsDone,
    RestartShardManagerDone,
}

enum WorkerCommand {
    InvokeAndAwaitWorkers {
        name: String,
        worker_ids: Vec<WorkerId>,
    },
    Stop,
}

#[derive(Debug)]
enum WorkerEvent {
    #[allow(dead_code)]
    InvokeAndAwaitWorkersCompleted(String),
}

fn coordinated_environment(
    command_rx: std::sync::mpsc::Receiver<EnvCommand>,
    event_tx: std::sync::mpsc::Sender<EnvEvent>,
) {
    info!("Starting Golem Sharding Tester");

    loop {
        match command_rx.recv().unwrap() {
            EnvCommand::StartShards(n) => {
                info!("Golem Sharding Tester starting shards({n})");
                start_shards(n);
                info!("Golem Sharding Tester started shards({n})");
                event_tx.send(EnvEvent::StartShardsDone).unwrap();
            }
            EnvCommand::StopShards(n) => {
                info!("Golem Sharding Tester stopping shards{n}");
                stop_shards(n);
                info!("Golem Sharding Tester stopped shard{n}");
                event_tx.send(EnvEvent::StopShardsDone).unwrap();
            }
            EnvCommand::StopAllShards => {
                info!("Golem Sharding Tester stopping all shards");
                stop_all_shards();
                info!("Golem Sharding Tester stopped all shard");
                event_tx.send(EnvEvent::StopAllShardsDone).unwrap();
            }
            EnvCommand::RestartShardManager => {
                info!("Golem Sharding Tester reloading shard manager");
                reload_shard_manager();
                info!("Golem Sharding Tester reloaded shard manager");
                event_tx.send(EnvEvent::RestartShardManagerDone).unwrap();
            }
            EnvCommand::Stop => break,
        }
    }
}

async fn worker_invocation(
    mut command_rx: tokio::sync::mpsc::Receiver<WorkerCommand>,
    event_tx: tokio::sync::mpsc::Sender<WorkerEvent>,
) {
    while let WorkerCommand::InvokeAndAwaitWorkers { name, worker_ids } =
        command_rx.recv().await.unwrap()
    {
        invoke_and_await_workers(&worker_ids)
            .await
            .expect("Worker invocation failed");
        event_tx
            .send(WorkerEvent::InvokeAndAwaitWorkersCompleted(name))
            .await
            .unwrap();
    }
}

fn unstable_environment(stop_rx: std::sync::mpsc::Receiver<()>) {
    info!("Starting Golem Sharding Tester");

    fn worker() {
        let mut commands = [
            Command::StartShard,
            Command::StopShard,
            Command::RestartShardManager,
        ];
        let mut rng = thread_rng();
        commands.shuffle(&mut rng);
        match commands[0] {
            Command::StartShard => {
                info!("Golem Sharding Tester starting shard");
                start_shard();
                info!("Golem Sharding Tester started shard");
            }
            Command::StopShard => {
                info!("Golem Sharding Tester stopping shard");
                stop_shard();
                info!("Golem Sharding Tester stopped shard");
            }
            Command::RestartShardManager => {
                info!("Golem Sharding Tester reloading shard manager");
                reload_shard_manager();
                info!("Golem Sharding Tester reloaded shard manager");
            }
        }
    }

    fn random_seconds() -> u64 {
        let mut rng = thread_rng();
        rng.gen_range(1..10)
    }

    while stop_rx.try_recv().is_err() {
        std::thread::sleep(Duration::from_secs(random_seconds()));
        worker();
    }
}
