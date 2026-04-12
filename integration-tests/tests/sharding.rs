// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
    use async_trait::async_trait;
    use axum::Router;
    use axum::routing::post;
    use bytes::Bytes;
    use golem_api_grpc::proto::golem::worker;
    use golem_client::api::RegistryServiceClient;
    use golem_common::model::base64::Base64;
    use golem_common::model::component::ComponentDto;
    use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
    use golem_common::model::oplog::PublicOplogEntry;
    use golem_common::model::plugin_registration::{
        OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
    };
    use golem_common::model::{AgentStatus, IdempotencyKey, OplogIndex};
    use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
    use golem_common::{agent_id, data_value};
    use golem_test_framework::components::rdb::DbInfo;
    use golem_test_framework::config::{
        EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    };
    use golem_test_framework::dsl::{TestDsl, TestDslExtended};

    use golem_common::model::agent::ParsedAgentId;
    use rand::prelude::*;
    use rand::rng;
    use std::collections::{BTreeMap, HashSet};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use test_r::{flaky, test, test_dep, timeout};
    use tokio::sync::mpsc;
    use tokio::task::{JoinHandle, JoinSet};
    use tracing::{Instrument, error, info};
    use uuid::Uuid;

    pub struct Tracing;

    impl Tracing {
        pub fn init() -> Self {
            #[cfg(unix)]
            unsafe {
                backtrace_on_stack_overflow::enable()
            };
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
        .await
        .unwrap();

        deps.redis_monitor().assert_valid();

        deps
    }

    #[test_dep]
    pub fn tracing() -> Tracing {
        Tracing::init()
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
    #[timeout(240000)]
    #[flaky(5)]
    async fn coordinated_scenario_01_01(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
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

    #[test]
    #[timeout(240000)]
    #[flaky(5)]
    async fn coordinated_scenario_01_02(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
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

    #[test]
    #[timeout(240000)]
    #[flaky(5)]
    async fn coordinated_scenario_02_01(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
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

    #[test]
    #[timeout(240000)]
    #[flaky(5)]
    async fn coordinated_scenario_03_01(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
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

    #[test]
    #[timeout(240000)]
    #[flaky(5)]
    async fn service_is_responsive_to_shard_changes(
        deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) {
        deps.reset(16).await;
        let (component, agent_ids) = deps.create_component_and_start_workers(4).await;

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
            deps.invoke_and_await_workers(&component, &agent_ids)
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
        let (component, agent_ids) = deps
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
            let formatted_step = format!("{step:?}");
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
                            component: Box::new(component.clone()),
                            agent_ids: agent_ids.clone(),
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
        async fn create_component_and_start_workers(
            &self,
            n: usize,
        ) -> (ComponentDto, Vec<ParsedAgentId>);
        async fn invoke_and_await_workers(
            &self,
            component: &ComponentDto,
            agent_ids: &[ParsedAgentId],
        ) -> Result<(), worker::v1::agent_error::Error>;
        async fn start_all_worker_executors(&self);
        async fn stop_random_worker_executor(&self);
        async fn start_random_worker_executor(&self);
        async fn start_random_worker_executors(&self, n: usize);
        async fn stop_random_worker_executors(&self, n: usize);
        async fn stop_all_worker_executors(&self);
        async fn start_shard_manager(&self, number_of_shards: Option<usize>);
        async fn stop_shard_manager(&self);
        async fn restart_shard_manager(&self);
        async fn clean_shard_manager_state(&self);
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
            self.clean_shard_manager_state().await;
            self.flush_redis_db();
            self.start_shard_manager(Some(number_of_shards)).await;
            self.start_all_worker_executors().await;
            info!("Reset done");
        }

        async fn create_component_and_start_workers(
            &self,
            n: usize,
        ) -> (ComponentDto, Vec<ParsedAgentId>) {
            let admin = self.admin().await;
            let (_, env) = admin.app_and_env().await.unwrap();
            info!("Storing component");
            let component = admin
                .component(&env.id, "it_agent_counters_release")
                .name("it:agent-counters")
                .store()
                .await
                .unwrap();
            info!("ComponentId: {}", component.id);

            let mut agent_ids = Vec::new();

            for i in 1..=n {
                info!("Worker {i} starting");
                let agent_id = agent_id!("Counter", format!("sharding-test-{i}"));
                admin
                    .start_agent(&component.id, agent_id.clone())
                    .await
                    .unwrap();
                info!("Worker {i} started");
                agent_ids.push(agent_id);
            }

            info!("All workers started");

            (component, agent_ids)
        }

        async fn invoke_and_await_workers(
            &self,
            component: &ComponentDto,
            agent_ids: &[ParsedAgentId],
        ) -> Result<(), worker::v1::agent_error::Error> {
            let mut tasks = JoinSet::new();
            for agent_id in agent_ids {
                let self_clone = self.admin().await;
                tasks.spawn({
                    let agent_id = agent_id.clone();
                    let component = component.clone();
                    async move {
                        let idempotency_key = IdempotencyKey::fresh();
                        (
                            component.id,
                            agent_id.clone(),
                            self_clone
                                .invoke_and_await_agent_with_key(
                                    &component,
                                    &agent_id,
                                    &idempotency_key,
                                    "increment",
                                    data_value!(),
                                )
                                .await,
                        )
                    }
                    .in_current_span()
                });
            }

            info!("Workers invoked");
            let mut pending_count = agent_ids.len();
            let mut completed_agents: Vec<String> = Vec::new();
            loop {
                match tokio::time::timeout(Duration::from_secs(180), tasks.join_next()).await {
                    Ok(Some(result)) => {
                        let (_component_id, agent_id, result) = result.unwrap();
                        match result {
                            Ok(_) => {
                                pending_count -= 1;
                                completed_agents.push(agent_id.to_string());
                                info!(
                                    "Worker invoke success: {agent_id}, pending: {pending_count}",
                                );
                            }
                            Err(err) => {
                                error!("Worker invoke error: {agent_id}, {err:?}");
                                panic!("Worker invoke error: {agent_id}, {err:?}");
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        let all_agents: Vec<String> =
                            agent_ids.iter().map(|a| a.to_string()).collect();
                        let pending: Vec<&String> = all_agents
                            .iter()
                            .filter(|a| !completed_agents.contains(a))
                            .collect();
                        tasks.abort_all();
                        panic!(
                            "Timed out waiting for worker invocations. Still pending ({pending_count}): {}",
                            pending
                                .iter()
                                .map(|a| a.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                }
            }
            info!("All workers finished invocation");

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

        async fn clean_shard_manager_state(&self) {
            match self.rdb().info() {
                DbInfo::Sqlite(_) => {
                    let sqlite_files = [
                        "golem_shard_manager.db",
                        "golem_shard_manager.db-shm",
                        "golem_shard_manager.db-wal",
                    ];

                    for file in sqlite_files {
                        let _ =
                            tokio::fs::remove_file(self.temp_directory().join("sqlite").join(file))
                                .await;
                    }
                }
                DbInfo::Postgres(pg) => {
                    let pool = sqlx::PgPool::connect(&pg.public_connection_string())
                        .await
                        .expect("Failed to connect to Postgres");
                    sqlx::query("DELETE FROM golem_shard_manager.shard_manager_state")
                        .execute(&pool)
                        .await
                        .expect("Failed to clean shard_manager_state");
                    let _ = sqlx::query("DELETE FROM golem_shard_manager.quota_leases")
                        .execute(&pool)
                        .await;
                    let _ = sqlx::query("DELETE FROM golem_shard_manager.quota_resources")
                        .execute(&pool)
                        .await;
                    pool.close().await;
                }
                DbInfo::Mysql(_) => {
                    panic!("Mysql is not supported in sharding tests");
                }
            }
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
            component: Box<ComponentDto>,
            agent_ids: Vec<ParsedAgentId>,
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
            let formatted_command = format!("{command:?}");
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
        while let WorkerCommand::InvokeAndAwaitWorkers {
            name,
            component,
            agent_ids,
        } = command_rx.recv().await.unwrap()
        {
            deps.invoke_and_await_workers(&component, &agent_ids)
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
            let formatted_command = format!("{command:?}");
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

    // ========================================================================
    // Oplog processor plugin helpers for sharding tests
    // ========================================================================

    #[derive(Clone, Debug, serde::Deserialize)]
    struct BatchCallback {
        #[allow(dead_code)]
        source_worker_id: String,
        #[allow(dead_code)]
        account_id: String,
        #[allow(dead_code)]
        component_id: String,
        #[allow(dead_code)]
        first_entry_index: u64,
        #[allow(dead_code)]
        entry_count: u64,
        invocations: Vec<InvocationRecord>,
    }

    #[derive(Clone, Debug, serde::Deserialize)]
    struct InvocationRecord {
        oplog_index: u64,
        fn_name: String,
    }

    fn invocation_count(batches: &[BatchCallback]) -> usize {
        batches.iter().map(|b| b.invocations.len()).sum()
    }

    fn extract_function_names(batches: &[BatchCallback]) -> Vec<String> {
        let mut all: Vec<_> = batches.iter().flat_map(|b| b.invocations.iter()).collect();
        all.sort_by_key(|i| i.oplog_index);
        all.into_iter().map(|i| i.fn_name.clone()).collect()
    }

    fn assert_unique_oplog_indices(batches: &[BatchCallback]) {
        let indices: Vec<u64> = batches
            .iter()
            .flat_map(|b| b.invocations.iter().map(|i| i.oplog_index))
            .collect();
        let unique: HashSet<u64> = indices.iter().copied().collect();
        assert_eq!(
            indices.len(),
            unique.len(),
            "Duplicate oplog indices found: {:?}",
            indices
        );
    }

    async fn start_callback_server() -> (String, Arc<Mutex<Vec<BatchCallback>>>, JoinHandle<()>) {
        let received: Arc<Mutex<Vec<BatchCallback>>> = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = tokio::spawn(async move {
            let route = Router::new().route(
                "/callback",
                post(move |body: Bytes| {
                    async move {
                        let body_str = String::from_utf8(body.to_vec()).unwrap();
                        let batch: BatchCallback = serde_json::from_str(&body_str).unwrap();
                        let mut batches = received_clone.lock().unwrap();
                        batches.push(batch);
                        "ok"
                    }
                    .in_current_span()
                }),
            );
            axum::serve(listener, route).await.unwrap();
        });

        (
            format!("http://localhost:{port}/callback"),
            received,
            handle,
        )
    }

    async fn wait_for_invocations(
        received: &Arc<Mutex<Vec<BatchCallback>>>,
        expected_count: usize,
        timeout: Duration,
    ) -> Vec<BatchCallback> {
        let start = tokio::time::Instant::now();
        loop {
            let batches = received.lock().unwrap().clone();
            let count = invocation_count(&batches);
            if count >= expected_count {
                return batches;
            }
            if start.elapsed() > timeout {
                panic!(
                    "Timed out waiting for callbacks (got {} of {} expected, {} batches: {:?})",
                    count,
                    expected_count,
                    batches.len(),
                    batches,
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn wait_for_oplog_completions(
        user: &(impl TestDsl + Send + Sync + ?Sized),
        worker_id: &golem_common::model::AgentId,
        expected_count: usize,
        timeout: Duration,
        received: &Arc<Mutex<Vec<BatchCallback>>>,
    ) {
        let start = tokio::time::Instant::now();
        loop {
            let oplog = user
                .get_oplog(worker_id, OplogIndex::INITIAL)
                .await
                .unwrap_or_default();

            let completed = oplog
                .iter()
                .filter(|e| matches!(&e.entry, PublicOplogEntry::AgentInvocationFinished(_)))
                .count();

            if completed >= expected_count {
                return;
            }

            if start.elapsed() > timeout {
                let batches = received.lock().unwrap().clone();
                panic!(
                    "Timed out waiting for oplog completions \
                     (got {} of {} expected AgentInvocationFinished entries; \
                     {} callback invocations across {} batches: {:?})",
                    completed,
                    expected_count,
                    invocation_count(&batches),
                    batches.len(),
                    batches,
                );
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    // ========================================================================
    // I1: Shard reassignment — no entry loss
    // ========================================================================

    #[test]
    #[timeout(240000)]
    #[flaky(3)]
    async fn oplog_processor_shard_reassignment_no_loss(
        deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) {
        deps.reset(16).await;

        let user = deps.admin().await;
        let (_, env) = user.app_and_env().await.unwrap();
        let client = user.registry_service_client().await;

        let (callback_url, received, _server) = start_callback_server().await;

        let plugin_component = user
            .component(&env.id, "oplog_processor_release")
            .store()
            .await
            .unwrap();
        let plugin = client
            .create_plugin(
                &user.account_id.0,
                &PluginRegistrationCreation {
                    name: format!("oplog-processor-{}", Uuid::new_v4()),
                    version: "v1".to_string(),
                    description: "A test".to_string(),
                    icon: Base64(Vec::new()),
                    homepage: "none".to_string(),
                    spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                        component_id: plugin_component.id,
                        component_revision: plugin_component.revision,
                    }),
                },
            )
            .await
            .unwrap();
        let grant = client
            .create_environment_plugin_grant(
                &env.id.0,
                &EnvironmentPluginGrantCreation {
                    plugin_registration_id: plugin.id,
                },
            )
            .await
            .unwrap();

        let mut params = BTreeMap::new();
        params.insert("callback-url".to_string(), callback_url);

        let component = user
            .component(&env.id, "it_agent_counters_release")
            .name("it:agent-counters")
            .with_parametrized_plugin(&grant.id, 0, params)
            .store()
            .await
            .unwrap();

        let repo_id = agent_id!("Repository", "worker1");
        let worker_id = user
            .start_agent(&component.id, repo_id.clone())
            .await
            .unwrap();

        for i in 0..3 {
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(format!("G{i}"), format!("Item {i}")),
            )
            .await
            .unwrap();
        }

        // Two-phase setup gate: wait for oplog persistence, then for callbacks.
        wait_for_oplog_completions(&user, &worker_id, 4, Duration::from_secs(120), &received).await;
        let _ = wait_for_invocations(&received, 4, Duration::from_secs(120)).await;

        // Shard reassignment: stop 2 executors, allow reassignment, then start them back.
        deps.stop_random_worker_executors(2).await;
        // Give the shard manager time to detect executor loss and reassign shards.
        tokio::time::sleep(Duration::from_secs(5)).await;
        deps.start_random_worker_executors(2).await;

        // Wait for the worker to become usable again after shard movement.
        user.wait_for_statuses(
            &worker_id,
            &[AgentStatus::Idle, AgentStatus::Running],
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        for i in 3..6 {
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(format!("G{i}"), format!("Item {i}")),
            )
            .await
            .unwrap();
        }

        let batches = wait_for_invocations(&received, 7, Duration::from_secs(120)).await;
        let fn_names = extract_function_names(&batches);
        // Exactly-once: exactly 1 init + 6 adds, no duplicates across shard reassignment.
        // Current bug: no checkpoint, so shard reassignment causes re-delivery.
        assert_eq!(
            fn_names
                .iter()
                .filter(|f| f.as_str() == "agent-initialization")
                .count(),
            1
        );
        assert_eq!(fn_names.iter().filter(|f| f.as_str() == "add").count(), 6);
        assert_unique_oplog_indices(&batches);
    }

    // ========================================================================
    // I2: Shard move with in-flight batch
    // ========================================================================

    #[test]
    #[timeout(240000)]
    #[flaky(3)]
    async fn oplog_processor_shard_move_inflight(
        deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) {
        deps.reset(16).await;

        let user = deps.admin().await;
        let (_, env) = user.app_and_env().await.unwrap();
        let client = user.registry_service_client().await;

        let (callback_url, received, _server) = start_callback_server().await;

        let plugin_component = user
            .component(&env.id, "oplog_processor_release")
            .store()
            .await
            .unwrap();
        let plugin = client
            .create_plugin(
                &user.account_id.0,
                &PluginRegistrationCreation {
                    name: format!("oplog-processor-{}", Uuid::new_v4()),
                    version: "v1".to_string(),
                    description: "A test".to_string(),
                    icon: Base64(Vec::new()),
                    homepage: "none".to_string(),
                    spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                        component_id: plugin_component.id,
                        component_revision: plugin_component.revision,
                    }),
                },
            )
            .await
            .unwrap();
        let grant = client
            .create_environment_plugin_grant(
                &env.id.0,
                &EnvironmentPluginGrantCreation {
                    plugin_registration_id: plugin.id,
                },
            )
            .await
            .unwrap();

        let mut params = BTreeMap::new();
        params.insert("callback-url".to_string(), callback_url);

        let component = user
            .component(&env.id, "it_agent_counters_release")
            .name("it:agent-counters")
            .with_parametrized_plugin(&grant.id, 0, params)
            .store()
            .await
            .unwrap();

        let repo_id = agent_id!("Repository", "worker1");
        let worker_id = user
            .start_agent(&component.id, repo_id.clone())
            .await
            .unwrap();

        for i in 0..2 {
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(format!("G{i}"), format!("Item {i}")),
            )
            .await
            .unwrap();
        }

        wait_for_oplog_completions(&user, &worker_id, 3, Duration::from_secs(120), &received).await;

        // Stop all executors immediately — before flush timer fires
        deps.stop_all_worker_executors().await;
        deps.start_all_worker_executors().await;
        tokio::time::sleep(Duration::from_secs(5)).await;

        user.wait_for_statuses(
            &worker_id,
            &[AgentStatus::Idle, AgentStatus::Running],
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        for i in 2..4 {
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(format!("G{i}"), format!("Item {i}")),
            )
            .await
            .unwrap();
        }

        wait_for_oplog_completions(&user, &worker_id, 5, Duration::from_secs(120), &received).await;
        let batches = wait_for_invocations(&received, 5, Duration::from_secs(120)).await;
        let fn_names = extract_function_names(&batches);
        // Exactly-once: exactly 1 init + 4 adds, no duplicates across shard move.
        // Current bug: no checkpoint, in-flight batch may be re-delivered.
        assert_eq!(
            fn_names
                .iter()
                .filter(|f| f.as_str() == "agent-initialization")
                .count(),
            1
        );
        assert_eq!(fn_names.iter().filter(|f| f.as_str() == "add").count(), 4);
        assert_unique_oplog_indices(&batches);
    }

    // ========================================================================
    // I3: Locality recovery — delivery continues after shard reassignment
    // ========================================================================

    #[test]
    #[timeout(240000)]
    #[flaky(3)]
    async fn oplog_processor_locality_recovery(
        deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) {
        deps.reset(16).await;

        let user = deps.admin().await;
        let (_, env) = user.app_and_env().await.unwrap();
        let client = user.registry_service_client().await;

        let (callback_url, received, _server) = start_callback_server().await;

        let plugin_component = user
            .component(&env.id, "oplog_processor_release")
            .store()
            .await
            .unwrap();
        let plugin = client
            .create_plugin(
                &user.account_id.0,
                &PluginRegistrationCreation {
                    name: format!("oplog-processor-{}", Uuid::new_v4()),
                    version: "v1".to_string(),
                    description: "A test".to_string(),
                    icon: Base64(Vec::new()),
                    homepage: "none".to_string(),
                    spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                        component_id: plugin_component.id,
                        component_revision: plugin_component.revision,
                    }),
                },
            )
            .await
            .unwrap();
        let grant = client
            .create_environment_plugin_grant(
                &env.id.0,
                &EnvironmentPluginGrantCreation {
                    plugin_registration_id: plugin.id,
                },
            )
            .await
            .unwrap();

        let mut params = BTreeMap::new();
        params.insert("callback-url".to_string(), callback_url);

        let component = user
            .component(&env.id, "it_agent_counters_release")
            .name("it:agent-counters")
            .with_parametrized_plugin(&grant.id, 0, params)
            .store()
            .await
            .unwrap();

        let repo_id = agent_id!("Repository", "worker1");
        let worker_id = user
            .start_agent(&component.id, repo_id.clone())
            .await
            .unwrap();

        for i in 0..3 {
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(format!("G{i}"), format!("Item {i}")),
            )
            .await
            .unwrap();
        }

        // Two-phase setup gate: wait for oplog persistence, then for callbacks
        wait_for_oplog_completions(&user, &worker_id, 4, Duration::from_secs(120), &received).await;
        let _ = wait_for_invocations(&received, 4, Duration::from_secs(60)).await;

        deps.stop_random_worker_executors(2).await;
        // Give the shard manager time to detect executor loss and reassign shards
        tokio::time::sleep(Duration::from_secs(5)).await;
        deps.start_random_worker_executors(2).await;

        user.wait_for_statuses(
            &worker_id,
            &[AgentStatus::Idle, AgentStatus::Running],
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        for i in 3..6 {
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(format!("G{i}"), format!("Item {i}")),
            )
            .await
            .unwrap();
        }

        let batches = wait_for_invocations(&received, 7, Duration::from_secs(120)).await;
        let fn_names = extract_function_names(&batches);
        // Exactly-once: exactly 1 init + 6 adds, no duplicates after locality recovery.
        // Current bug: no checkpoint, so shard reassignment causes re-delivery.
        assert_eq!(
            fn_names
                .iter()
                .filter(|f| f.as_str() == "agent-initialization")
                .count(),
            1
        );
        assert_eq!(fn_names.iter().filter(|f| f.as_str() == "add").count(), 6);
        assert_unique_oplog_indices(&batches);
    }

    // ========================================================================
    // I4: End-to-end stress with crashes
    // ========================================================================

    #[test]
    #[timeout(360000)]
    #[flaky(5)]
    async fn oplog_processor_stress_with_crashes(
        deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) {
        deps.reset(16).await;

        let user = deps.admin().await;
        let (_, env) = user.app_and_env().await.unwrap();
        let client = user.registry_service_client().await;

        let (callback_url, received, _server) = start_callback_server().await;

        let plugin_component = user
            .component(&env.id, "oplog_processor_release")
            .store()
            .await
            .unwrap();
        let plugin = client
            .create_plugin(
                &user.account_id.0,
                &PluginRegistrationCreation {
                    name: format!("oplog-processor-{}", Uuid::new_v4()),
                    version: "v1".to_string(),
                    description: "A test".to_string(),
                    icon: Base64(Vec::new()),
                    homepage: "none".to_string(),
                    spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                        component_id: plugin_component.id,
                        component_revision: plugin_component.revision,
                    }),
                },
            )
            .await
            .unwrap();
        let grant = client
            .create_environment_plugin_grant(
                &env.id.0,
                &EnvironmentPluginGrantCreation {
                    plugin_registration_id: plugin.id,
                },
            )
            .await
            .unwrap();

        let mut params = BTreeMap::new();
        params.insert("callback-url".to_string(), callback_url);

        let component = user
            .component(&env.id, "it_agent_counters_release")
            .name("it:agent-counters")
            .with_parametrized_plugin(&grant.id, 0, params)
            .store()
            .await
            .unwrap();

        let repo_id = agent_id!("Repository", "worker1");
        let worker_id = user
            .start_agent(&component.id, repo_id.clone())
            .await
            .unwrap();

        for i in 0..20 {
            user.invoke_and_await_agent(
                &component,
                &repo_id,
                "add",
                data_value!(format!("G{i}"), format!("Item {i}")),
            )
            .await
            .unwrap();

            // Crash at iteration 7
            if i == 7 {
                info!("I4: Triggering simulated crash at iteration {i}");
                user.simulated_crash(&worker_id).await.unwrap();
                user.wait_for_statuses(
                    &worker_id,
                    &[AgentStatus::Idle, AgentStatus::Running],
                    Duration::from_secs(30),
                )
                .await
                .unwrap();
            }

            // Stop/start executor at iteration 14
            if i == 14 {
                info!("I4: Stopping/starting executor at iteration {i}");
                deps.stop_random_worker_executors(1).await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                deps.start_random_worker_executors(1).await;
                tokio::time::sleep(Duration::from_secs(5)).await;
                user.wait_for_statuses(
                    &worker_id,
                    &[AgentStatus::Idle, AgentStatus::Running],
                    Duration::from_secs(60),
                )
                .await
                .unwrap();
            }
        }

        // Exactly-once: exactly 1 init + 20 adds, no duplicates despite crash + executor restart.
        // Current bug: no checkpoint, crash and executor stop/start cause re-delivery.
        let batches = wait_for_invocations(&received, 21, Duration::from_secs(120)).await;
        let fn_names = extract_function_names(&batches);
        assert_eq!(
            fn_names
                .iter()
                .filter(|f| f.as_str() == "agent-initialization")
                .count(),
            1
        );
        assert_eq!(fn_names.iter().filter(|f| f.as_str() == "add").count(), 20);
        assert_unique_oplog_indices(&batches);
    }
}
