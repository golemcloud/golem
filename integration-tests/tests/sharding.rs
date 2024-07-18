use std::ops::Deref;
use std::time::Duration;

use ctor::{ctor, dtor};
use golem_wasm_rpc::Value;
use rand::prelude::*;
use tracing::info;

use golem_api_grpc::proto::golem::worker;
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use golem_test_framework::dsl::TestDslUnsafe;

struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        let config = TracingConfig::test("sharding-tests").with_env_overrides();
        init_tracing_with_default_debug_env_filter(&config);
        Self
    }
}

#[ctor]
pub static DEPS: EnvBasedTestDependencies = {
    let deps = EnvBasedTestDependencies::blocking_new_from_config(EnvBasedTestDependenciesConfig {
        number_of_shards_override: Some(16),
        ..EnvBasedTestDependenciesConfig::new()
    });

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
async fn service_is_responsive_to_shard_changes() {
    Deps::reset().await;
    let worker_ids = Deps::create_component_and_start_workers(4).await;

    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let chaos = std::thread::spawn(|| {
        unstable_environment(stop_rx);
    });

    info!("All workers started");

    for c in 0..2 {
        if c != 0 {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        info!("Invoking workers ({c})");
        Deps::invoke_and_await_workers(&worker_ids)
            .await
            .expect("Invocations failed");
        info!("Invoking workers done ({c})");
    }

    info!("Sharding test completed");
    stop_tx.send(()).unwrap();
    chaos.join().unwrap();
}

#[tokio::test]
async fn coordinated_scenario1() {
    coordinated_scenario(vec![
        Step::StopAllWorkerExecutor,
        Step::InvokeAndAwaitWorkersAsync(
            "Invoke, RestartShardManager, StartWorkerExecutors".to_string(),
        ),
        Step::RestartShardManager,
        Step::Sleep(Duration::from_secs(3)),
        Step::StartWorkerExecutors(4),
        Step::WaitForInvokeAndAwaitResult,
        Step::StopAllWorkerExecutor,
        Step::RestartShardManager,
        Step::StartWorkerExecutors(4),
        Step::RestartShardManager,
        Step::InvokeAndAwaitWorkersAsync(
            "StartWorkerExecutors, RestartShardManager, Invoke".to_string(),
        ),
        Step::WaitForInvokeAndAwaitResult,
        Step::StopAllWorkerExecutor,
        Step::RestartShardManager,
        Step::StartWorkerExecutors(4),
        Step::StopWorkerExecutors(3),
        Step::Sleep(Duration::from_secs(3)),
        Step::InvokeAndAwaitWorkersAsync(
            "StartWorkerExecutors(4), StopWorkerExecutors(3), Invoke".to_string(),
        ),
        Step::WaitForInvokeAndAwaitResult,
    ])
    .await;
}

async fn coordinated_scenario(steps: Vec<Step>) {
    Deps::reset().await;
    let worker_ids = Deps::create_component_and_start_workers(4).await;

    let (worker_command_tx, worker_command_rx) = tokio::sync::mpsc::channel(128);
    let (worker_event_tx, mut worker_event_rx) = tokio::sync::mpsc::channel(128);
    let (env_command_tx, env_command_rx) = std::sync::mpsc::channel();
    let (env_event_tx, env_event_rx) = std::sync::mpsc::channel();

    let chaos = std::thread::spawn(|| {
        coordinated_environment(env_command_rx, env_event_tx);
    });

    let send_env_command = |command: EnvCommand| {
        let response_event = command.response_event();
        env_command_tx.send(command).unwrap();
        if let Some(response_event) = response_event {
            let event = env_event_rx.recv().unwrap();
            assert_eq!(event, response_event);
        }
    };

    let invoker = tokio::task::spawn(async {
        worker_invocation(worker_command_rx, worker_event_tx).await;
    });

    for step in steps {
        let formatted_step = format!("{:?}", step);
        info!("Step: {} - Started", formatted_step);
        match step {
            Step::StartWorkerExecutors(n) => {
                send_env_command(EnvCommand::StartWorkerExecutors(n));
            }
            Step::StopWorkerExecutors(n) => {
                send_env_command(EnvCommand::StopWorkerExecutors(n));
            }
            Step::StopAllWorkerExecutor => {
                send_env_command(EnvCommand::StopAllWorkerExecutor);
            }
            Step::RestartShardManager => {
                send_env_command(EnvCommand::RestartShardManager);
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
        info!("Step: {} - Done", formatted_step);
    }

    info!("Sharding test completed");
    worker_command_tx.send(WorkerCommand::Stop).await.unwrap();
    send_env_command(EnvCommand::Stop);
    chaos.join().unwrap();
    invoker.await.unwrap();
}

#[derive(Debug)]
enum Command {
    StartShard,
    StopShard,
    RestartShardManager,
}

// Sharding test specific env functions
struct Deps;

impl Deps {
    async fn reset() {
        info!("Reset started");
        Deps::stop_all_worker_executors();
        Deps::stop_shard_manager();
        Deps::flush_redis_db();
        Deps::start_shard_manager().await;
        Deps::start_all_worker_executors().await;
        info!("Reset done");
    }

    async fn create_component_and_start_workers(n: usize) -> Vec<WorkerId> {
        info!("Storing component");
        let component_id = DEPS.store_component("option-service").await;

        let mut worker_ids = Vec::new();

        for i in 1..=n {
            info!("Worker {i} starting");
            let worker_name = format!("sharding-test-{i}");
            let worker_id = DEPS.start_worker(&component_id, &worker_name).await;
            info!("Worker {i} started");
            worker_ids.push(worker_id);
        }

        info!("All workers started");

        worker_ids
    }

    async fn invoke_and_await_workers(
        workers: &[WorkerId],
    ) -> Result<(), worker::worker_error::Error> {
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

    async fn start_all_worker_executors() {
        let stopped = DEPS.worker_executor_cluster().stopped_indices();
        for idx in stopped {
            DEPS.worker_executor_cluster().start(idx).await;
        }
    }

    fn start_random_worker_executor() {
        Deps::start_random_worker_executors(1);
    }

    fn start_random_worker_executors(n: usize) {
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

    fn stop_random_worker_executor() {
        Deps::stop_random_worker_executors(1);
    }

    fn stop_random_worker_executors(n: usize) {
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

    fn stop_all_worker_executors() {
        let started = DEPS.worker_executor_cluster().started_indices();
        for idx in started {
            DEPS.worker_executor_cluster().stop(idx);
        }
    }

    fn blocking_start_shard_manager() {
        DEPS.shard_manager().blocking_restart();
    }

    async fn start_shard_manager() {
        DEPS.shard_manager().restart().await;
    }

    fn stop_shard_manager() {
        DEPS.shard_manager().kill();
    }

    fn restart_shard_manager() {
        DEPS.shard_manager().kill();
        DEPS.shard_manager().blocking_restart();
    }

    fn flush_redis_db() {
        DEPS.redis().flush_db(0);
    }
}

#[derive(Debug, Clone)]
enum Step {
    StartWorkerExecutors(usize),
    StopWorkerExecutors(usize),
    StopAllWorkerExecutor,
    RestartShardManager,
    Sleep(Duration),
    InvokeAndAwaitWorkersAsync(String),
    WaitForInvokeAndAwaitResult,
}

#[derive(Debug)]
enum EnvCommand {
    StartWorkerExecutors(usize),
    StopWorkerExecutors(usize),
    StopAllWorkerExecutor,
    StartShardManager,
    StopShardManager,
    RestartShardManager,
    FlushRedis,
    Stop,
}

impl EnvCommand {
    fn response_event(&self) -> Option<EnvEvent> {
        match self {
            EnvCommand::StartWorkerExecutors(_) => Some(EnvEvent::StartWorkerExecutorsDone),
            EnvCommand::StopWorkerExecutors(_) => Some(EnvEvent::StopWorkerExecutorsDone),
            EnvCommand::StopAllWorkerExecutor => Some(EnvEvent::StopAllWorkerExecutorDone),
            EnvCommand::StartShardManager => Some(EnvEvent::StartShardManagerDone),
            EnvCommand::StopShardManager => Some(EnvEvent::StopShardManagerDone),
            EnvCommand::RestartShardManager => Some(EnvEvent::RestartShardManagerDone),
            EnvCommand::FlushRedis => Some(EnvEvent::FlushRedisDone),
            EnvCommand::Stop => None,
        }
    }
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, PartialEq)]
enum EnvEvent {
    StartWorkerExecutorsDone,
    StopWorkerExecutorsDone,
    StopAllWorkerExecutorDone,
    StartShardManagerDone,
    StopShardManagerDone,
    RestartShardManagerDone,
    FlushRedisDone,
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
        let command = command_rx.recv().unwrap();
        let formatted_command = format!("{:?}", command);
        let response_event = command.response_event();

        info!("Command: {} - Started", formatted_command);
        match command {
            EnvCommand::StartWorkerExecutors(n) => {
                Deps::start_random_worker_executors(n);
            }
            EnvCommand::StopWorkerExecutors(n) => {
                Deps::stop_random_worker_executors(n);
            }
            EnvCommand::StopAllWorkerExecutor => {
                Deps::stop_all_worker_executors();
            }
            EnvCommand::StartShardManager => {
                Deps::blocking_start_shard_manager();
            }
            EnvCommand::StopShardManager => {
                Deps::stop_shard_manager();
            }
            EnvCommand::RestartShardManager => {
                Deps::restart_shard_manager();
            }
            EnvCommand::FlushRedis => {
                Deps::flush_redis_db();
            }
            EnvCommand::Stop => break,
        }

        if let Some(response_event) = response_event {
            event_tx.send(response_event).unwrap();
        }

        info!("Command: {} - Done", formatted_command);
    }
}

async fn worker_invocation(
    mut command_rx: tokio::sync::mpsc::Receiver<WorkerCommand>,
    event_tx: tokio::sync::mpsc::Sender<WorkerEvent>,
) {
    while let WorkerCommand::InvokeAndAwaitWorkers { name, worker_ids } =
        command_rx.recv().await.unwrap()
    {
        Deps::invoke_and_await_workers(&worker_ids)
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
        let command = &commands[0];
        let formatted_command = format!("{:?}", command);
        info!("Command: {} - Started", formatted_command);
        match command {
            Command::StartShard => {
                Deps::start_random_worker_executor();
            }
            Command::StopShard => {
                Deps::stop_random_worker_executor();
            }
            Command::RestartShardManager => {
                Deps::restart_shard_manager();
            }
        }
        info!("Command: {} - Done", formatted_command);
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
