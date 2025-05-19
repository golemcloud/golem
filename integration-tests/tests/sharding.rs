// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

test_r::enable!();

#[test_r::sequential]
mod tests {
    use test_r::{flaky, test, test_dep, timeout};

    use async_trait::async_trait;
    use golem_wasm_rpc::IntoValueAndType;
    use rand::prelude::*;
    use rand::rng;
    use std::env;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::task::JoinSet;
    use tracing::{error, info, Instrument};

    use golem_api_grpc::proto::golem::worker;
    use golem_common::model::{IdempotencyKey, WorkerId};
    use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
    use golem_test_framework::config::{
        EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    };
    use golem_test_framework::dsl::TestDslUnsafe;

    pub struct Tracing;

    impl Tracing {
        pub fn init() -> Self {
            init_tracing_with_default_debug_env_filter(
                &TracingConfig::test("sharding-tests").with_env_overrides(),
            );
            Self
        }
    }

    #[test_dep]
    pub async fn create_deps(_tracing: &Tracing) -> EnvBasedTestDependencies {
        let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
            number_of_shards_override: Some(16),
            ..EnvBasedTestDependenciesConfig::new()
        })
        .await;

        deps.redis_monitor().assert_valid();

        deps
    }

    #[test_dep]
    pub fn tracing() -> Tracing {
        Tracing::init()
    }

    fn coordinated_scenario_retries() -> usize {
        let retries = env::var("COORDINATED_SCENARIO_RETRIES")
            .ok()
            .and_then(|str| str.parse::<usize>().ok())
            .unwrap_or(1);

        info!("COORDINATED_SCENARIO_RETRIES: {retries}");

        retries
    }

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

        pub fn case_4() -> Vec<Step> {
            Step::invoke_and_await(
                vec![Step::StopAllWorkerExecutor, Step::RestartShardManager],
                vec![
                    Step::StopWorkerExecutors(4),
                    Step::StartWorkerExecutors(3),
                    Step::Sleep(Duration::from_secs(1)),
                    Step::StopWorkerExecutors(2),
                ],
            )
        }
    }

    #[test]
    #[timeout(120000)]
    #[flaky(5)]
    #[ignore] // TEMPORARILY IGNORED AS IT IS VERY FLAKY ON CI
    async fn coordinated_scenario_01_01(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
        for _ in 0..coordinated_scenario_retries() {
            coordinated_scenario(
                deps,
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

    #[test]
    #[timeout(240000)]
    #[flaky(5)]
    #[ignore] // TEMPORARILY IGNORED AS IT IS VERY FLAKY ON CI
    async fn coordinated_scenario_01_02(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
        for _ in 0..coordinated_scenario_retries() {
            coordinated_scenario(
                deps,
                5,
                30,
                vec![
                    Scenario::case_1(Duration::from_secs(5)),
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

    #[test]
    #[timeout(240000)]
    #[flaky(5)]
    #[ignore] // TEMPORARILY IGNORED AS IT IS VERY FLAKY ON CI
    async fn coordinated_scenario_02_01(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
        for _ in 0..coordinated_scenario_retries() {
            coordinated_scenario(
                deps,
                5,
                10,
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

    #[test]
    #[timeout(120000)]
    #[flaky(5)]
    #[ignore] // TEMPORARILY IGNORED AS IT IS VERY FLAKY ON CI
    async fn coordinated_scenario_03_01(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
        for _ in 0..coordinated_scenario_retries() {
            coordinated_scenario(
                deps,
                5,
                10,
                vec![
                    Scenario::case_3(Duration::from_secs(3)),
                    Scenario::case_4(),
                    Scenario::case_3(Duration::from_secs(3)),
                ]
                .into_iter()
                .flatten()
                .collect(),
            )
            .await;
        }
    }

    #[test]
    #[timeout(120000)]
    #[flaky(5)]
    #[ignore] // TEMPORARILY IGNORED AS IT IS VERY FLAKY ON CI
    async fn service_is_responsive_to_shard_changes(
        deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) {
        deps.reset(16).await;
        let worker_ids = deps.create_component_and_start_workers(4).await;

        let deps_clone = deps.clone();
        let (stop_tx, stop_rx) = mpsc::channel(128);
        let chaos = tokio::spawn(
            async move {
                unstable_environment(&deps_clone, stop_rx).await;
            }
            .in_current_span(),
        );

        info!("All workers started");

        for c in 0..2 {
            if c != 0 {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            info!("Invoking workers ({c})");
            deps.invoke_and_await_workers(&worker_ids)
                .await
                .expect("Invocations failed");
            info!("Invoking workers done ({c})");
        }

        info!("Sharding test completed");
        stop_tx.send(()).await.unwrap();
        chaos.await.unwrap();
    }

    async fn coordinated_scenario(
        deps: &EnvBasedTestDependencies,
        number_of_shard: usize,
        number_of_workers: usize,
        steps: Vec<Step>,
    ) {
        deps.reset(number_of_shard).await;
        let worker_ids = deps
            .create_component_and_start_workers(number_of_workers)
            .await;

        let (worker_command_tx, worker_command_rx) = mpsc::channel(128);
        let (worker_event_tx, mut worker_event_rx) = mpsc::channel(128);
        let (env_command_tx, env_command_rx) = mpsc::channel(128);
        let (env_event_tx, env_event_rx) = mpsc::channel(128);

        let deps_clone = deps.clone();
        let chaos = tokio::spawn(
            async move {
                coordinated_environment(&deps_clone, env_command_rx, env_event_tx).await;
            }
            .in_current_span(),
        );

        let mut checked_command_sender = CheckedCommandSender::new(env_command_tx, env_event_rx);

        let deps_clone = deps.clone();
        let invoker = tokio::task::spawn(
            async move {
                worker_invocation(&deps_clone, worker_command_rx, worker_event_tx).await;
            }
            .in_current_span(),
        );

        for step in steps {
            let formatted_step = format!("{:?}", step);
            info!("Step: {} - Started", formatted_step);
            match step {
                Step::StartWorkerExecutors(n) => {
                    checked_command_sender
                        .send(EnvCommand::StartWorkerExecutors(n))
                        .await;
                }
                Step::StopWorkerExecutors(n) => {
                    checked_command_sender
                        .send(EnvCommand::StopWorkerExecutors(n))
                        .await;
                }
                Step::StopAllWorkerExecutor => {
                    checked_command_sender
                        .send(EnvCommand::StopAllWorkerExecutor)
                        .await;
                }
                Step::RestartShardManager => {
                    checked_command_sender
                        .send(EnvCommand::RestartShardManager)
                        .await;
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
        checked_command_sender.send(EnvCommand::Stop).await;
        chaos.await.unwrap();
        invoker.await.unwrap();
    }

    #[derive(Debug)]
    enum Command {
        StartShard,
        StopShard,
        RestartShardManager,
    }

    // Sharding test specific env functions
    #[async_trait]
    trait DepsOps {
        async fn reset(&self, number_of_shards: usize);
        async fn create_component_and_start_workers(&self, n: usize) -> Vec<WorkerId>;
        async fn invoke_and_await_workers(
            &self,
            workers: &[WorkerId],
        ) -> Result<(), worker::v1::worker_error::Error>;
        async fn start_all_worker_executors(&self);
        async fn stop_random_worker_executor(&self);
        async fn start_random_worker_executor(&self);
        async fn start_random_worker_executors(&self, n: usize);
        async fn stop_random_worker_executors(&self, n: usize);
        async fn stop_all_worker_executors(&self);
        async fn start_shard_manager(&self, number_of_shards: Option<usize>);
        async fn stop_shard_manager(&self);
        async fn restart_shard_manager(&self);
        fn flush_redis_db(&self);
    }

    fn randomize<T>(slice: &mut [T]) {
        let mut rng = rng();
        slice.shuffle(&mut rng);
    }

    #[async_trait]
    impl DepsOps for EnvBasedTestDependencies {
        async fn reset(&self, number_of_shards: usize) {
            info!("Reset started");
            self.stop_all_worker_executors().await;
            self.stop_shard_manager().await;
            self.flush_redis_db();
            self.start_shard_manager(Some(number_of_shards)).await;
            self.start_all_worker_executors().await;
            info!("Reset done");
        }

        async fn create_component_and_start_workers(&self, n: usize) -> Vec<WorkerId> {
            info!("Storing component");
            let component_id = self.component("option-service").store().await;
            info!("ComponentId: {}", component_id);

            let mut worker_ids = Vec::new();

            for i in 1..=n {
                info!("Worker {i} starting");
                let worker_name = format!("sharding-test-{i}");
                let worker_id = self.start_worker(&component_id, &worker_name).await;
                info!("Worker {i} started");
                worker_ids.push(worker_id);
            }

            info!("All workers started");

            worker_ids
        }

        async fn invoke_and_await_workers(
            &self,
            workers: &[WorkerId],
        ) -> Result<(), worker::v1::worker_error::Error> {
            let mut tasks = JoinSet::new();
            for worker_id in workers {
                let self_clone = self.clone();
                tasks.spawn({
                    let worker_id = worker_id.clone();
                    async move {
                        let idempotency_key = IdempotencyKey::fresh();
                        (
                            worker_id.clone(),
                            self_clone
                                .invoke_and_await_with_key(
                                    &worker_id,
                                    &idempotency_key,
                                    "golem:it/api.{echo}",
                                    vec![Some("Hello".to_string()).into_value_and_type()],
                                )
                                .await,
                        )
                    }
                    .in_current_span()
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
                        error!("Worker invoke error: {worker_id}, {err:?}");
                        panic!("Worker invoke error: {worker_id}, {err:?}");
                    }
                }
            }

            Ok(())
        }

        async fn start_all_worker_executors(&self) {
            let stopped = self.worker_executor_cluster().stopped_indices().await;
            let mut tasks = JoinSet::new();
            for idx in stopped {
                let self_clone = self.clone();
                tasks.spawn(
                    async move {
                        self_clone.worker_executor_cluster().start(idx).await;
                    }
                    .in_current_span(),
                );
            }
            while let Some(result) = tasks.join_next().await {
                result.unwrap()
            }
        }

        async fn stop_random_worker_executor(&self) {
            self.stop_random_worker_executors(1).await;
        }

        async fn start_random_worker_executor(&self) {
            self.start_random_worker_executors(1).await
        }

        async fn start_random_worker_executors(&self, n: usize) {
            let mut stopped = self.worker_executor_cluster().stopped_indices().await;
            if !stopped.is_empty() {
                {
                    let mut rng = rng();
                    stopped.shuffle(&mut rng);
                }

                let to_start = &stopped[0..n];

                let mut tasks = JoinSet::new();
                for idx in to_start {
                    let self_clone = self.clone();
                    tasks.spawn(
                        {
                            let idx = idx.to_owned();
                            async move {
                                self_clone.worker_executor_cluster().start(idx).await;
                            }
                        }
                        .in_current_span(),
                    );
                }
                while let Some(result) = tasks.join_next().await {
                    result.unwrap()
                }
            }
        }

        async fn stop_random_worker_executors(&self, n: usize) {
            let mut started = self.worker_executor_cluster().started_indices().await;
            if !started.is_empty() {
                randomize(&mut started);

                let to_stop = &started[0..n];

                for idx in to_stop {
                    self.worker_executor_cluster().stop(*idx).await;
                }
            }
        }

        async fn stop_all_worker_executors(&self) {
            let started = self.worker_executor_cluster().started_indices().await;
            for idx in started {
                self.worker_executor_cluster().stop(idx).await;
            }
        }

        async fn start_shard_manager(&self, number_of_shards: Option<usize>) {
            self.shard_manager().restart(number_of_shards).await;
        }

        async fn stop_shard_manager(&self) {
            self.shard_manager().kill().await;
        }

        async fn restart_shard_manager(&self) {
            self.shard_manager().kill().await;
            self.shard_manager().restart(None).await;
        }

        fn flush_redis_db(&self) {
            self.redis().flush_db(0);
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

    struct CheckedCommandSender {
        env_command_tx: mpsc::Sender<EnvCommand>,
        env_event_rx: mpsc::Receiver<EnvEvent>,
    }

    impl CheckedCommandSender {
        fn new(
            env_command_tx: mpsc::Sender<EnvCommand>,
            env_event_rx: mpsc::Receiver<EnvEvent>,
        ) -> Self {
            Self {
                env_command_tx,
                env_event_rx,
            }
        }

        async fn send(&mut self, command: EnvCommand) {
            let response_event = command.response_event();
            self.env_command_tx.send(command).await.unwrap();
            if let Some(response_event) = response_event {
                let event = self.env_event_rx.recv().await.unwrap();
                assert_eq!(event, response_event);
            };
        }
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

    async fn coordinated_environment(
        deps: &EnvBasedTestDependencies,
        mut command_rx: mpsc::Receiver<EnvCommand>,
        event_tx: mpsc::Sender<EnvEvent>,
    ) {
        info!("Starting Golem Sharding Tester");

        loop {
            let command = command_rx.recv().await.unwrap();
            let formatted_command = format!("{:?}", command);
            let response_event = command.response_event();

            info!("Command: {} - Started", formatted_command);
            match command {
                EnvCommand::StartWorkerExecutors(n) => {
                    deps.start_random_worker_executors(n).await;
                }
                EnvCommand::StopWorkerExecutors(n) => {
                    deps.stop_random_worker_executors(n).await;
                }
                EnvCommand::StopAllWorkerExecutor => {
                    deps.stop_all_worker_executors().await;
                }
                EnvCommand::StartShardManager => {
                    deps.start_shard_manager(None).await;
                }
                EnvCommand::StopShardManager => {
                    deps.stop_shard_manager().await;
                }
                EnvCommand::RestartShardManager => {
                    deps.restart_shard_manager().await;
                }
                EnvCommand::Stop => break,
            }

            if let Some(response_event) = response_event {
                event_tx.send(response_event).await.unwrap();
            }

            info!("Command: {} - Done", formatted_command);
        }
    }

    async fn worker_invocation(
        deps: &EnvBasedTestDependencies,
        mut command_rx: mpsc::Receiver<WorkerCommand>,
        event_tx: mpsc::Sender<WorkerEvent>,
    ) {
        while let WorkerCommand::InvokeAndAwaitWorkers { name, worker_ids } =
            command_rx.recv().await.unwrap()
        {
            deps.invoke_and_await_workers(&worker_ids)
                .await
                .expect("Worker invocation failed");
            event_tx
                .send(WorkerEvent::InvokeAndAwaitWorkersCompleted(name))
                .await
                .unwrap();
        }
    }

    async fn unstable_environment(
        deps: &EnvBasedTestDependencies,
        mut stop_rx: mpsc::Receiver<()>,
    ) {
        info!("Starting Golem Sharding Tester");

        async fn worker(deps: &EnvBasedTestDependencies) {
            let mut commands = [
                Command::StartShard,
                Command::StopShard,
                Command::RestartShardManager,
            ];
            {
                let mut rng = rng();
                commands.shuffle(&mut rng);
            }
            let command = &commands[0];
            let formatted_command = format!("{:?}", command);
            info!("Command: {} - Started", formatted_command);
            match command {
                Command::StartShard => {
                    deps.start_random_worker_executor().await;
                }
                Command::StopShard => {
                    deps.stop_random_worker_executor().await;
                }
                Command::RestartShardManager => {
                    deps.restart_shard_manager().await;
                }
            }
            info!("Command: {} - Done", formatted_command);
        }

        fn random_seconds() -> u64 {
            let mut rng = rng();
            rng.random_range(1..10)
        }

        while stop_rx.try_recv().is_err() {
            std::thread::sleep(Duration::from_secs(random_seconds()));
            worker(deps).await;
        }
    }
}
