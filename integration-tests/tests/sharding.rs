use std::ops::Deref;
use std::time::Duration;

use ctor::{ctor, dtor};
use golem_wasm_rpc::Value;
use rand::prelude::*;
use tokio::task::JoinSet;
use tracing::info;

use golem_api_grpc::proto::golem::worker;
use golem_common::model::{IdempotencyKey, WorkerId};
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use golem_test_framework::dsl::TestDslUnsafe;

struct Scenario;

impl Scenario {
    pub fn case_1(sleep_duration: Duration) -> Vec<Step> {
        Step::invoke_and_await(
            vec![Step::StopAllWorkerExecutor],
            vec![
                Step::RestartShardManager,
                Step::Sleep(sleep_duration),
                Step::StartWorkerExecutors(4),
            ],
        )
    }

    pub fn case_2() -> Vec<Step> {
        Step::invoke_and_await(
            vec![
                Step::StopAllWorkerExecutor,
                Step::RestartShardManager,
                Step::StartWorkerExecutors(4),
                Step::RestartShardManager,
            ],
            vec![],
        )
    }

    pub fn case_3(sleep_duration: Duration) -> Vec<Step> {
        Step::invoke_and_await(
            vec![
                Step::StopAllWorkerExecutor,
                Step::RestartShardManager,
                Step::StartWorkerExecutors(4),
                Step::StopWorkerExecutors(3),
                Step::Sleep(sleep_duration),
            ],
            vec![],
        )
    }
}

#[tokio::test]
async fn coordinated_scenario_01_01() {
    coordinated_scenario(5, 4, Scenario::case_1(Duration::from_secs(3))).await;
}

#[tokio::test]
async fn coordinated_scenario_01_02() {
    coordinated_scenario(16, 4, Scenario::case_1(Duration::from_secs(3))).await;
}

#[tokio::test]
async fn coordinated_scenario_01_04() {
    coordinated_scenario(5, 4, Scenario::case_1(Duration::from_secs(0))).await;
}

#[tokio::test]
async fn coordinated_scenario_01_05() {
    coordinated_scenario(5, 4, Scenario::case_1(Duration::from_secs(10))).await;
}

#[tokio::test]
async fn coordinated_scenario_01_06() {
    coordinated_scenario(5, 30, Scenario::case_1(Duration::from_secs(3))).await;
}

#[tokio::test]
async fn coordinated_scenario_02_01() {
    coordinated_scenario(5, 4, Scenario::case_2()).await;
}

#[tokio::test]
async fn coordinated_scenario_02_02() {
    coordinated_scenario(16, 4, Scenario::case_2()).await;
}

#[tokio::test]
async fn coordinated_scenario_02_04() {
    coordinated_scenario(5, 4, Scenario::case_2()).await;
}

#[tokio::test]
async fn coordinated_scenario_02_05() {
    coordinated_scenario(5, 4, Scenario::case_2()).await;
}

#[tokio::test]
async fn coordinated_scenario_02_06() {
    coordinated_scenario(5, 30, Scenario::case_2()).await;
}

#[tokio::test]
async fn coordinated_scenario_03_01() {
    coordinated_scenario(5, 4, Scenario::case_3(Duration::from_secs(3))).await;
}

#[tokio::test]
async fn coordinated_scenario_03_02() {
    coordinated_scenario(16, 4, Scenario::case_3(Duration::from_secs(3))).await;
}

#[tokio::test]
async fn coordinated_scenario_03_04() {
    coordinated_scenario(5, 4, Scenario::case_3(Duration::from_secs(0))).await;
}

#[tokio::test]
async fn coordinated_scenario_03_05() {
    coordinated_scenario(5, 4, Scenario::case_3(Duration::from_secs(10))).await;
}

#[tokio::test]
async fn coordinated_scenario_03_06() {
    coordinated_scenario(5, 30, Scenario::case_3(Duration::from_secs(3))).await;
}

#[tokio::test]
async fn coordinated_scenario_04_01() {
    for _ in 0..3 {
        coordinated_scenario(
            5,
            4,
            vec![
                Scenario::case_1(Duration::from_secs(3)),
                Scenario::case_2(),
                Scenario::case_3(Duration::from_secs(3)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        )
        .await;
    }
}

#[tokio::test]
async fn coordinated_scenario_04_02() {
    for _ in 0..3 {
        coordinated_scenario(
            5,
            30,
            vec![
                Scenario::case_1(Duration::from_secs(3)),
                Scenario::case_2(),
                Scenario::case_3(Duration::from_secs(3)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        )
        .await;
    }
}

#[tokio::test]
async fn coordinated_scenario_04_03() {
    for _ in 0..3 {
        coordinated_scenario(
            5,
            30,
            vec![
                Scenario::case_2(),
                Scenario::case_3(Duration::from_secs(3)),
                Scenario::case_1(Duration::from_secs(3)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        )
        .await;
    }
}

#[tokio::test]
async fn service_is_responsive_to_shard_changes() {
    Deps::reset(16).await;
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

async fn coordinated_scenario(number_of_shard: usize, number_of_workers: usize, steps: Vec<Step>) {
    Deps::reset(number_of_shard).await;
    let worker_ids = Deps::create_component_and_start_workers(number_of_workers).await;

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
    async fn reset(number_of_shards: usize) {
        info!("Reset started");
        Deps::stop_all_worker_executors();
        Deps::stop_shard_manager();
        Deps::flush_redis_db();
        Deps::start_shard_manager(number_of_shards).await;
        Deps::start_all_worker_executors().await;
        info!("Reset done");
    }

    async fn create_component_and_start_workers(n: usize) -> Vec<WorkerId> {
        info!("Storing component");
        let component_id = DEPS.store_component("option-service").await;
        info!("ComponentId: {}", component_id);

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
        let mut tasks = JoinSet::new();
        for worker_id in workers {
            tasks.spawn({
                let worker_id = worker_id.clone();
                async move {
                    let idempotency_key = IdempotencyKey::fresh();
                    (
                        worker_id.clone(),
                        DEPS.invoke_and_await_with_key(
                            &worker_id,
                            &idempotency_key,
                            "golem:it/api.{echo}",
                            vec![Value::Option(Some(Box::new(Value::String(
                                "Hello".to_string(),
                            ))))],
                        )
                        .await,
                    )
                }
            });
        }

        info!("Workers invoked");
        while let Some(result) = tasks.join_next().await {
            let (worker_id, result) = result.unwrap();
            match result {
                Ok(_) => {
                    info!("Worker invoke success: {worker_id}")
                }
                Err(err) => {
                    panic!("Worker invoke error: {worker_id}, {err:?}");
                }
            }
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
        DEPS.shard_manager().blocking_restart(None);
    }

    async fn start_shard_manager(number_of_shards: usize) {
        DEPS.shard_manager().restart(Some(number_of_shards)).await;
    }

    fn stop_shard_manager() {
        DEPS.shard_manager().kill();
    }

    fn restart_shard_manager() {
        DEPS.shard_manager().kill();
        DEPS.shard_manager().blocking_restart(None);
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

impl Step {
    pub fn invoke_and_await(before_invoke: Vec<Step>, before_await: Vec<Step>) -> Vec<Step> {
        let step_count = before_invoke.len() + 1 + before_await.len();
        let mut step_descriptions = Vec::with_capacity(step_count);
        let mut steps = Vec::with_capacity(step_count);

        {
            for step in &before_invoke {
                step_descriptions.push(format!("{step:?}"));
            }

            step_descriptions.push("Invoke".to_string());

            for step in &before_await {
                step_descriptions.push(format!("{step:?}"));
            }

            step_descriptions.push("Wait".to_string());
        }
        let description = step_descriptions.join(", ");

        {
            for step in before_invoke {
                steps.push(step);
            }

            steps.push(Step::InvokeAndAwaitWorkersAsync(description));

            for step in before_await {
                steps.push(step);
            }

            steps.push(Step::WaitForInvokeAndAwaitResult)
        }

        steps
    }
}

#[allow(dead_code)]
#[derive(Debug)]
enum EnvCommand {
    StartWorkerExecutors(usize),
    StopWorkerExecutors(usize),
    StopAllWorkerExecutor,
    StartShardManager,
    StopShardManager,
    RestartShardManager,
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
