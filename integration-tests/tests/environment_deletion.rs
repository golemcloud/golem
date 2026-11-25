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

use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_test_framework::config::{
    DbType, EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use std::sync::Once;
use std::time::Duration;
use test_r::{define_matrix_dimension, test_dep};

test_r::enable!();

// Matrix dimension for the DB backend. Each test below declares
// `#[dimension(db)]` on its `deps: &EnvBasedTestDependencies` parameter,
// producing one test per case named `<test>_postgres` / `<test>_sqlite`,
// each carrying the `db_postgres` / `db_sqlite` auto-tag (selectable via
// `:tag:db_postgres` / `:tag:db_sqlite`).
define_matrix_dimension!(db: EnvBasedTestDependencies -> "postgres", "sqlite");

#[derive(Debug)]
pub struct Tracing;

// The global subscriber can be installed only once per process, and a retry of a failed test
// creates its dependencies again in the same process.
static TRACING_INIT: Once = Once::new();

#[cfg(unix)]
#[path = "platform/unix.rs"]
mod platform;
#[cfg(not(unix))]
#[path = "platform/non_unix.rs"]
mod platform;

impl Tracing {
    pub fn init() -> Self {
        TRACING_INIT.call_once(|| {
            platform::enable_stack_overflow_backtrace();
            init_tracing_with_default_debug_env_filter(
                &TracingConfig::test_pretty_without_time("integration-tests").with_env_overrides(),
            );
        });
        Self
    }
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
pub async fn create_deps(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 1,
        oplog_archive_interval: Some(Duration::from_secs(2)),
        db_type: DbType::Postgres,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing test dependencies");

    deps.redis_monitor().assert_valid();

    deps
}

#[test_dep(tagged_as = "postgres")]
pub async fn create_deps_postgres(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 1,
        oplog_archive_interval: Some(Duration::from_secs(2)),
        db_type: DbType::Postgres,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing test dependencies");

    deps.redis_monitor().assert_valid();

    deps
}

#[test_dep(tagged_as = "sqlite")]
pub async fn create_deps_sqlite(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 1,
        oplog_archive_interval: Some(Duration::from_secs(2)),
        db_type: DbType::Sqlite,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing test dependencies");

    deps.redis_monitor().assert_valid();

    deps
}

/// Some tests stop and start the worker executors that every test of the binary shares, so the
/// tests run one at a time.
#[test_r::sequential]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use axum::Router;
    use axum::routing::post;
    use futures::{StreamExt, TryStreamExt};
    use golem_client::api::{RegistryServiceClient, WorkerClient, WorkerError};
    use golem_common::model::agent::AgentMode;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::{AgentStatus, OwnedAgentId, PromiseId};
    use golem_common::schema::FromSchema;
    use golem_common::{agent_key, data_value};
    use golem_test_framework::dsl::{TestDsl, TestDslExtended};
    use pretty_assertions::{assert_eq, assert_matches};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use test_r::{test, timeout};
    use tracing::Instrument;
    use tracing::debug;
    use tracing::info;

    /// Verify that deleting an environment stops a self-scheduling agent.
    ///
    /// The agent sends an HTTP POST to a local test server on every `tick`, then
    /// schedules itself 500 ms into the future.  Once the environment is deleted
    /// the scheduler can no longer activate the agent (component metadata fetch
    /// returns NotFound for a deleted environment) and the ping counter stops
    /// advancing.
    #[test]
    #[tracing::instrument]
    #[timeout("2m")]
    async fn environment_deletion_stops_self_scheduling_agent(
        #[dimension(db)] deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        let user = deps.user().await?;
        let registry_client = user.registry_service_client().await;
        let (_, env) = user.app_and_env().await?;

        let ping_count = Arc::new(AtomicU64::new(0));
        let ping_count_clone = ping_count.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();

        let http_server = tokio::spawn(
            async move {
                let route = Router::new().route(
                    "/ping",
                    post(move || {
                        let counter = ping_count_clone.clone();
                        async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            "ok"
                        }
                    }),
                );
                axum::serve(listener, route).await.unwrap();
            }
            .in_current_span(),
        );

        let component = user
            .component(&env.id, "golem_it_agent_rpc_rust_release")
            .name("golem-it:agent-rpc-rust")
            .store()
            .await?;

        let agent_name = "http-polling-self-scheduler-test";
        let parsed_agent_id = agent_key!("HttpPollingSelfScheduler", agent_name);
        user.start_agent(&component.id, parsed_agent_id.clone())
            .await?;

        user.invoke_agent(
            &component,
            &parsed_agent_id,
            "tick",
            golem_common::data_value!("127.0.0.1", port as u16),
        )
        .await?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if ping_count.load(Ordering::SeqCst) >= 3 {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                http_server.abort();
                anyhow::bail!(
                    "timed out waiting for the agent loop to start (got {} pings)",
                    ping_count.load(Ordering::SeqCst)
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        registry_client
            .delete_environment(&env.id.0, env.revision.into())
            .await?;

        tokio::time::sleep(Duration::from_secs(2)).await;
        let count_after_delete = ping_count.load(Ordering::SeqCst);

        tokio::time::sleep(Duration::from_secs(3)).await;
        let count_stable = ping_count.load(Ordering::SeqCst);

        http_server.abort();

        assert_eq!(
            count_after_delete, count_stable,
            "agent kept pinging after environment deletion ({count_after_delete} → {count_stable})"
        );

        Ok(())
    }

    /// Regression test for executor panic when the oplog archiver fires for a
    /// worker whose environment has been deleted.
    ///
    /// Panic path (stack frames from production trace):
    ///   SchedulerServiceDefault background loop
    ///     → ScheduledAction::ArchiveOplog handler
    ///     → SchedulerWorkerAccess::open_oplog
    ///     → Worker::get_or_create_suspended
    ///     → Worker::new
    ///     → DefaultWorkerService::get
    ///     → component_service.get_metadata(revision)   ← returns ComponentNotFound
    ///     → unwrap_or_else(|err| panic!(...))           ← BOOM
    ///
    /// The archiver is scheduled when a worker transitions to Idle/Failed/Exited.
    /// With GOLEM__OPLOG__ARCHIVE_INTERVAL=2s (set in the test framework env) the
    /// archive action fires within a few seconds after the worker goes idle.
    /// Deleting the environment before that window expires reproduces the panic.
    #[test]
    #[tracing::instrument]
    #[timeout("2m")]
    async fn oplog_archive_in_deleted_environment_does_not_panic(
        #[dimension(db)] deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        let user = deps.user().await?;
        let registry_client = user.registry_service_client().await;
        let (_, env) = user.app_and_env().await?;

        let component = user
            .component(&env.id, "it_agent_counters_release")
            .name("it:agent-counters")
            .store()
            .await?;

        // Invoke a method so the worker completes and transitions to Idle, which
        // schedules an ArchiveOplog action 2 s from now (archive_interval=2s in tests).
        user.invoke_and_await_agent(
            &component,
            &agent_key!("Counter", "archive-panic-test"),
            "increment",
            data_value!(),
        )
        .await?;

        // Delete the environment while the ArchiveOplog action is pending.
        registry_client
            .delete_environment(&env.id.0, env.revision.into())
            .await?;

        debug!("deleted environment {}", env.id);

        // Wait long enough for the archive action to fire (archive_interval=2s +
        // scheduler process_interval≤2s → up to 4s; 8s is a comfortable margin).
        tokio::time::sleep(Duration::from_secs(8)).await;

        // Verify the executor is still alive
        assert!(deps.worker_executor_cluster().is_running().await);

        // Verify the executor is still alive by making an unrelated call on a
        // fresh environment. If the scheduler task panicked the executor would
        // be unresponsive. Only having a single executor guarantees we are hitting the instance.
        let liveness_user = deps.user().await?;
        let (_, liveness_env) = liveness_user.app_and_env().await?;

        let liveness_component = liveness_user
            .component(&liveness_env.id, "it_agent_counters_release")
            .name("it:agent-counters")
            .store()
            .await?;

        liveness_user
        .invoke_and_await_agent(
            &liveness_component,
            &agent_key!("Counter", "liveness-check"),
            "increment",
            data_value!(),
        )
        .await
        .map_err(|e| anyhow::anyhow!(
            "executor is no longer responsive after ArchiveOplog fired on deleted environment: {e}"
        ))?;

        Ok(())
    }

    #[test]
    #[tracing::instrument]
    #[timeout("2m")]
    async fn invoke_existing_agent_in_deleted_environment_fails(
        #[dimension(db)] deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        let user = deps.user().await?;
        let registry_client = user.registry_service_client().await;
        let (_, env) = user.app_and_env().await?;

        let component = user
            .component(&env.id, "it_agent_counters_release")
            .name("it:agent-counters")
            .store()
            .await?;

        let parsed_agent_id = agent_key!("Counter", "env-delete-existing");

        user.invoke_and_await_agent(&component, &parsed_agent_id, "increment", data_value!())
            .await?;

        registry_client
            .delete_environment(&env.id.0, env.revision.into())
            .await?;

        // Allow the invalidation event to propagate to the executor.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let error = user
            .invoke_and_await_agent(&component, &parsed_agent_id, "increment", data_value!())
            .await
            .unwrap_err();

        let downcasted = error
            .downcast_ref::<golem_client::Error<golem_client::api::AgentError>>()
            .unwrap();

        assert_matches!(
            downcasted,
            golem_client::Error::Item(golem_client::api::AgentError::Error404(_))
        );

        Ok(())
    }

    /// Invoking a method on a non-existing agent whose environment has been deleted
    /// should also return a 404.
    #[test]
    #[tracing::instrument]
    #[timeout("2m")]
    async fn invoke_new_agent_in_deleted_environment_fails(
        #[dimension(db)] deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        let user = deps.user().await?;
        let registry_client = user.registry_service_client().await;
        let (_, env) = user.app_and_env().await?;

        let component = user
            .component(&env.id, "it_agent_counters_release")
            .name("it:agent-counters")
            .store()
            .await?;

        registry_client
            .delete_environment(&env.id.0, env.revision.into())
            .await?;

        // Allow the invalidation event to propagate to the executor.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let error = user
            .invoke_and_await_agent(
                &component,
                &agent_key!("Counter", "env-delete-new"),
                "increment",
                data_value!(),
            )
            .await
            .unwrap_err();

        let downcasted = error
            .downcast_ref::<golem_client::Error<golem_client::api::AgentError>>()
            .unwrap();

        assert_matches!(
            downcasted,
            golem_client::Error::Item(golem_client::api::AgentError::Error404(_))
        );

        Ok(())
    }

    #[test]
    #[tracing::instrument]
    #[timeout("2m")]
    async fn complete_promise_in_deleted_environment_results_in_404(
        #[dimension(db)] deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        let user = deps.user().await?;
        let registry_client = user.registry_service_client().await;
        let (_, env) = user.app_and_env().await?;

        let component = user
            .component(&env.id, "golem_it_agent_promise")
            .name("golem-it:agent-promise")
            .store()
            .await?;

        let agent_id = agent_key!("PromiseAgent", "promise-env-delete-test");
        let worker = user.start_agent(&component.id, agent_id.clone()).await?;

        let result = user
            .invoke_and_await_agent(&component, &agent_id, "getPromise", data_value!())
            .await?;

        let promise_id_value = result
            .into_return_value()
            .ok_or_else(|| anyhow!("expected promise id return value"))?;
        let promise_id =
            <PromiseId as FromSchema>::from_value(&promise_id_value).map_err(|e| anyhow!("{e}"))?;

        // Suspend the agent on the promise.
        user.invoke_agent(
            &component,
            &agent_id,
            "awaitPromise",
            data_value!(promise_id.clone()),
        )
        .await?;

        user.wait_for_status(&worker, AgentStatus::Suspended, Duration::from_secs(10))
            .await?;

        // Delete the environment while the agent is suspended.
        registry_client
            .delete_environment(&env.id.0, env.revision.into())
            .await?;

        // Invalidation is asynchronous. Observe deletion without completing the promise,
        // so the completion below still targets a pending promise rather than a consumed one.
        tokio::time::timeout(Duration::from_secs(10), async {
            while user.get_worker_metadata_opt(&worker).await?.is_some() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|_| {
            anyhow!("environment deletion did not become visible through agent metadata")
        })??;

        info!("Completing pending promise");

        let error = user
            .complete_promise(&component.environment_id, &promise_id, b"ignored".to_vec())
            .await
            .unwrap_err();

        let downcasted = error
            .downcast_ref::<golem_client::Error<golem_client::api::WorkerError>>()
            .unwrap();

        assert_matches!(
            downcasted,
            golem_client::Error::Item(golem_client::api::WorkerError::Error404(_))
        );

        Ok(())
    }

    /// The key-value storage that the worker executors of `deps` keep their records in.
    async fn executor_key_value_storage(
        deps: &EnvBasedTestDependencies,
    ) -> anyhow::Result<
        Arc<dyn golem_worker_executor::storage::keyvalue::KeyValueStorage + Send + Sync>,
    > {
        use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
        use golem_test_framework::components::rdb::DbInfo;
        use golem_worker_executor::services::golem_config::KeyValueStoragePostgresConfig;
        use golem_worker_executor::storage::keyvalue::postgres::PostgresKeyValueStorage;
        use golem_worker_executor::storage::keyvalue::sqlite::SqliteKeyValueStorage;

        // The schema and the database name that the test framework gives the worker executors.
        const NAMESPACE: &str = "golem_worker_executor";
        match deps.rdb().info() {
            DbInfo::Sqlite(path) => Ok(Arc::new(
                SqliteKeyValueStorage::configured(&DbSqliteConfig {
                    database: path
                        .join(format!("{NAMESPACE}.db"))
                        .to_string_lossy()
                        .into_owned(),
                    max_connections: 1,
                    foreign_keys: true,
                })
                .await
                .map_err(|error| anyhow!(error))?,
            )),
            DbInfo::Postgres(postgres) => Ok(Arc::new(
                PostgresKeyValueStorage::configured(&KeyValueStoragePostgresConfig {
                    postgres: DbPostgresConfig {
                        host: postgres.public_host.clone(),
                        database: postgres.database_name.clone(),
                        username: postgres.username.clone(),
                        password: postgres.password.clone(),
                        port: postgres.public_port,
                        max_connections: 1,
                        schema: Some(NAMESPACE.to_string()),
                        acquire_timeout: None,
                    },
                })
                .await
                .map_err(|error| anyhow!(error))?,
            )),
            DbInfo::Mysql(_) => anyhow::bail!("worker executors do not store key-values in MySQL"),
        }
    }

    /// The last index of the status that the worker executors stored for the agent incarnation
    /// `fingerprint`, or `None` when they stored no status.
    async fn stored_status_index(
        storage: Arc<dyn golem_worker_executor::storage::keyvalue::KeyValueStorage + Send + Sync>,
        owned_agent_id: OwnedAgentId,
        fingerprint: golem_common::model::AgentFingerprint,
    ) -> anyhow::Result<Option<OplogIndex>> {
        use golem_worker_executor::storage::keyvalue::KeyValueStorageNamespace;

        // The field of the stored status that holds its last index.
        const CORE_FIELD: &str = "core";
        storage
            .get_all(
                "test",
                "stored_status_index",
                "agent_status",
                KeyValueStorageNamespace::AgentStatus {
                    owned_agent_id,
                    fingerprint,
                },
            )
            .await?
            .into_iter()
            .find(|(name, _)| name == CORE_FIELD)
            .map(|(_, bytes)| {
                golem_common::serialization::deserialize::<golem_common::model::AgentStatusRecord>(
                    &bytes,
                )
                .map(|status| status.oplog_idx)
                .map_err(|error| anyhow!(error))
            })
            .transpose()
    }

    /// Waits until the status that the worker executors stored for the agent incarnation
    /// `fingerprint` covers `last`, the last index of its oplog. The executors store the status in
    /// the background, and a fold after a restart continues from the stored status. The wait reads
    /// the stored status up to `attempts` times, 100 ms apart.
    async fn wait_for_stored_status(
        storage: Arc<dyn golem_worker_executor::storage::keyvalue::KeyValueStorage + Send + Sync>,
        owned_agent_id: OwnedAgentId,
        fingerprint: golem_common::model::AgentFingerprint,
        last: OplogIndex,
        attempts: u32,
    ) -> anyhow::Result<()> {
        let reads = futures::stream::iter(0..attempts).then({
            let owned_agent_id = owned_agent_id.clone();
            move |attempt| {
                let (storage, owned_agent_id) = (storage.clone(), owned_agent_id.clone());
                async move {
                    if attempt > 0 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    stored_status_index(storage, owned_agent_id, fingerprint).await
                }
            }
        });
        // The boxed stream hides the closure types. Without it, the `Send` check of the test future
        // fails with a higher-ranked lifetime error.
        let reached = reads
            .try_skip_while(move |stored| {
                futures::future::ready(Ok(!stored.is_some_and(|index| index >= last)))
            })
            .boxed()
            .try_next()
            .await?;
        reached.map(|_| ()).ok_or_else(|| {
            anyhow!(
                "the stored status of {owned_agent_id} did not reach {last} in {attempts} reads"
            )
        })
    }

    /// The first page of up to 100 entries of the oplog of `owned_agent_id` through the REST API, from
    /// `from`, or without a `from`.
    async fn oplog_page(
        client: &golem_client::api::WorkerClientLive,
        owned_agent_id: &OwnedAgentId,
        from: Option<OplogIndex>,
    ) -> Result<golem_client::model::GetOplogResponse, golem_client::Error<WorkerError>> {
        client
            .get_oplog(
                &owned_agent_id.environment_id.0,
                &owned_agent_id.agent_id.0,
                from.map(|from| from.as_u64()),
                100,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
    }

    /// Whether `text` names `retained_start` as the start of an expired oplog. A start with more
    /// digits, such as 30 for 3, does not match.
    fn names_expired_start(text: &str, retained_start: OplogIndex) -> bool {
        let needle = format!("starts at index {retained_start}");
        text.match_indices(needle.as_str())
            .any(|(at, _)| !text[at + needle.len()..].starts_with(|c: char| c.is_ascii_digit()))
    }

    /// `GET .../oplog` on an agent whose retained start is 3 returns the entries from the start with
    /// their original indexes, also without a `from`. A `from` of 1 or 2, before the start, returns
    /// the `OPLOG_EXPIRED` error body that names the start. A page without a `from` of an agent whose
    /// start is 1 starts at index 1. The test stops and starts the worker executors, so it runs in
    /// this binary, whose dependencies own the executor processes.
    ///
    /// The worker service reads the metadata of the agent before it reads the oplog, and the
    /// executor folds the status of the agent forward from its stored status. Before the stop, the
    /// test waits until the stored status covers the last entry of the oplog, so the fold reads no
    /// entry below the start.
    #[test]
    #[tracing::instrument]
    #[timeout("4m")]
    async fn get_oplog_starts_at_the_retained_start(
        #[dimension(db)] deps: &EnvBasedTestDependencies,
        _tracing: &Tracing,
    ) -> anyhow::Result<()> {
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "golem_it_host_api_tests_release")
            .name("golem-it:host-api-tests")
            .store()
            .await?;
        let parsed_agent_id = agent_key!("GolemHostApi", "oplog-retained-start");
        let parsed_control_id = agent_key!("GolemHostApi", "oplog-start-at-1");
        let agent_id = user
            .start_agent(&component.id, parsed_agent_id.clone())
            .await?;
        let control_id = user
            .start_agent(&component.id, parsed_control_id.clone())
            .await?;
        user.invoke_and_await_agent(
            &component,
            &parsed_agent_id,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?;
        user.invoke_and_await_agent(
            &component,
            &parsed_control_id,
            "generate_idempotency_keys",
            data_value!(),
        )
        .await?;
        let history = user.get_oplog(&agent_id, OplogIndex::INITIAL).await?;
        let retained_start = OplogIndex::from_u64(3);
        let last = history
            .last()
            .map(|entry| entry.oplog_index)
            .unwrap_or(OplogIndex::NONE);
        assert!(
            last > retained_start,
            "the oplog has entries after the start: {history:?}"
        );

        let storage = executor_key_value_storage(deps).await?;
        let fingerprint = user.get_worker_metadata(&agent_id).await?.fingerprint;
        wait_for_stored_status(storage.clone(), agent_id.clone(), fingerprint, last, 300).await?;
        let cluster = deps.worker_executor_cluster();
        let started = cluster.started_indices().await;
        futures::future::join_all(started.iter().map(|index| cluster.stop(*index))).await;
        golem_worker_executor::services::oplog::retention::advance_oplog_retention(
            storage.as_ref(),
            &agent_id,
            AgentMode::Durable,
            retained_start,
        )
        .await?;
        drop(storage);
        futures::future::join_all(started.iter().map(|index| cluster.start(*index))).await;

        let client = user
            .deps
            .worker_service()
            .worker_http_client(&user.token)
            .await;
        let page = oplog_page(&client, &agent_id, None).await?;
        assert_eq!(page.first_index_in_chunk, retained_start.as_u64());
        assert_eq!(
            page.entries.first().map(|entry| entry.oplog_index()),
            Some(retained_start),
            "a page without a `from` gives its entries from the retained start"
        );
        let page = oplog_page(&client, &control_id, None).await?;
        assert_eq!(
            page.entries.first().map(|entry| entry.oplog_index()),
            Some(OplogIndex::INITIAL),
            "a page without a `from` of an agent whose start is 1 gives its entries from index 1"
        );

        let page = user.get_oplog(&agent_id, retained_start).await?;
        let retained = history
            .iter()
            .filter(|entry| entry.oplog_index >= retained_start)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            page[..retained.len()],
            retained[..],
            "the entries keep their original indexes"
        );

        let expired = futures::future::join_all(
            [OplogIndex::INITIAL, retained_start.previous()]
                .into_iter()
                .map(|from| oplog_page(&client, &agent_id, Some(from))),
        )
        .await;
        expired.into_iter().for_each(|result| match result {
            Err(golem_client::Error::Item(WorkerError::Error400(body))) => {
                assert_eq!(
                    body.code,
                    golem_common::base_model::api::error_code::OPLOG_EXPIRED
                );
                assert!(
                    body.errors
                        .iter()
                        .any(|error| names_expired_start(error, retained_start)),
                    "{body:?}"
                );
            }
            other => panic!("expected the OPLOG_EXPIRED error body, got {other:?}"),
        });
        Ok(())
    }
}
