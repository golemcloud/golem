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

use crate::Tracing;
use anyhow::Context;
use golem_common::base_model::worker_filter::{AgentFilter, FilterComparator};
use golem_common::config::DbPostgresConfig;
use golem_common::model::agent::{AgentMetadataDto, AgentMode, AgentTypeName, InvocationOperation};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AgentFingerprint, AgentStatus, IdempotencyKey, OwnedAgentId, PromiseId, ScanCursor,
    ScheduledAction, ShardLeaseClock, ShardLeaseRevision,
};
use golem_common::schema::{FromSchema, SchemaValue};
use golem_common::{agent_key, data_value};
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace, agent_path_segment,
};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::golem_config::{
    IndexedStorageConfig, IndexedStoragePostgresConfig, KeyValueStorageConfig,
    KeyValueStoragePostgresConfig, SchedulerStorageConfig, SchedulerStoragePostgresConfig,
};
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use sqlx::PgPool;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

/// All per-agent SQL tables, independently enumerated from the three storage schemas.
const TABLES: [&str; 6] = [
    "indexed.index_storage",
    "indexed.indexed_key_epoch",
    "keyvalue.kv_storage",
    "keyvalue.set_storage",
    "keyvalue.sorted_set_storage",
    "scheduler.scheduled_actions",
];

struct ArchiveStores {
    _lease_renewal: tokio_util::task::AbortOnDropHandle<()>,
    _postgres: DockerPostgresRdb,
    _s3: crate::oplog_blob_archive::S3Test,
    pool: PgPool,
    blobs: Arc<dyn BlobStorage>,
    shard: Arc<Mutex<Option<Arc<dyn ShardService>>>>,
    overrides: TestExecutorOverrides,
}

impl ArchiveStores {
    async fn new(population: bool) -> anyhow::Result<Self> {
        let postgres = DockerPostgresRdb::new(&uuid::Uuid::new_v4().to_string(), false).await;
        let connection = postgres.public_connection_string();
        let url = url::Url::parse(&connection)?;
        let pg = DbPostgresConfig {
            host: url.host_str().unwrap().to_string(),
            port: url.port().unwrap(),
            database: url.path().trim_start_matches('/').to_string(),
            username: url.username().to_string(),
            password: url.password().unwrap().to_string(),
            max_connections: 10,
            schema: None,
            acquire_timeout: None,
        };
        let pool = PgPool::connect(&connection).await?;
        let s3 = crate::oplog_blob_archive::new_s3_test();
        let blobs: Arc<dyn BlobStorage> = Arc::new(s3.s3_storage().await);
        let shard: Arc<Mutex<Option<Arc<dyn ShardService>>>> = Arc::new(Mutex::new(None));
        let lease_renewal = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
            let shard = shard.clone();
            async move {
                loop {
                    let service = shard.lock().unwrap().clone();
                    if let Some(service) = service
                        && service.try_get_current_assignment().is_some()
                    {
                        Self::renew_assignment(service.as_ref())
                            .expect("test lease renewal failed");
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }));
        let overrides = TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                let in_schema = |schema: &str| DbPostgresConfig {
                    schema: Some(schema.to_string()),
                    ..pg.clone()
                };
                config.indexed_storage =
                    IndexedStorageConfig::Postgres(IndexedStoragePostgresConfig {
                        postgres: in_schema("indexed"),
                        drop_prefix_delete_batch_size: 1024,
                        max_concurrent_ops: None,
                    });
                config.key_value_storage =
                    KeyValueStorageConfig::Postgres(KeyValueStoragePostgresConfig {
                        postgres: in_schema("keyvalue"),
                    });
                config.scheduler_storage =
                    SchedulerStorageConfig::Postgres(SchedulerStoragePostgresConfig {
                        postgres: in_schema("scheduler"),
                    });
                config.oplog.indexed_storage_layers = 2;
                config.oplog.blob_storage_layers = 1;
                config.oplog.entry_count_limit = 32;
                config.oplog.archive_interval = Duration::from_millis(100);
                config.oplog.shard_lease_max_ttl = Duration::from_secs(10);
                config.oplog.skew_margin = Duration::from_millis(100);
                config.oplog.seal_put_timeout = Duration::from_secs(1);
                config.oplog.server_commit_bound = Duration::from_secs(1);
                config.scheduler.refresh_interval = Duration::from_millis(50);
                config.suspend.suspend_after = if population {
                    Duration::from_secs(600)
                } else {
                    Duration::from_millis(100)
                };
                config.suspend.wait_suspend_grace = config.suspend.suspend_after;
                config.suspend.wait_suspend_check_interval = Duration::from_millis(50);
            })),
            wrap_blob_storage: Some(Arc::new({
                let blobs = blobs.clone();
                move |_| blobs.clone()
            })),
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    *shard.lock().unwrap() = Some(inner.clone());
                    inner
                }
            })),
            ..Default::default()
        };
        Ok(Self {
            _lease_renewal: lease_renewal,
            _postgres: postgres,
            _s3: s3,
            pool,
            blobs,
            shard,
            overrides,
        })
    }

    fn renew(&self) -> anyhow::Result<()> {
        let shard = self.shard.lock().unwrap().clone().unwrap();
        Self::renew_assignment(shard.as_ref())
    }

    fn renew_assignment(shard: &dyn ShardService) -> anyhow::Result<()> {
        let assignment = shard.current_assignment()?;
        shard.update_lease(
            &assignment.shard_epochs,
            ShardLeaseClock {
                sent_at: Instant::now(),
                wall_anchor: SystemTime::now(),
                ttl: Duration::from_secs(10),
            },
            ShardLeaseRevision {
                incarnation: uuid::Uuid::new_v4(),
                number: 1,
            },
            assignment.executor_count,
        )?;
        Ok(())
    }

    async fn counts(&self) -> anyhow::Result<[i64; 6]> {
        let mut result = [0; 6];
        for (index, table) in TABLES.iter().enumerate() {
            result[index] = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&self.pool)
                .await?;
        }
        Ok(result)
    }

    async fn wait_counts(&self, expected: [i64; 6]) -> anyhow::Result<[i64; 6]> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.renew()?;
            let counts = self.counts().await?;
            if counts == expected {
                return Ok(counts);
            }
            if Instant::now() >= deadline {
                let keys: Vec<(String, String)> = sqlx::query_as(
                    "SELECT namespace, key FROM keyvalue.kv_storage ORDER BY namespace, key",
                )
                .fetch_all(&self.pool)
                .await?;
                println!("Population timeout KV keys: {keys:?}");
            }
            assert!(
                Instant::now() < deadline,
                "expected {expected:?}, got {counts:?} for {TABLES:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn sealed(&self, id: &OwnedAgentId, mode: AgentMode) -> anyhow::Result<usize> {
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                self.renew()?;
                let entries = self
                    .blobs
                    .with("test", "wait_sealed")
                    .list_page(
                        BlobStorageNamespace::SealRecords {
                            environment_id: id.environment_id,
                            type_tag: id.agent_id.type_id().unwrap().get(),
                            agent_mode: mode,
                        },
                        &Path::new("seals").join(agent_path_segment(&id.agent_id)),
                        None,
                        1000,
                    )
                    .await?;
                let rows: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM indexed.index_storage WHERE env = $1 AND agent = $2",
                )
                .bind(id.environment_id.0)
                .bind(id.agent_id.as_uuid())
                .fetch_one(&self.pool)
                .await?;
                if rows == 0
                    && entries
                        .iter()
                        .any(|entry| entry.path.extension().is_some_and(|ext| ext == "sealed"))
                {
                    let chunks = self
                        .blobs
                        .with("test", "sealed_chunks")
                        .list_blobs_below(
                            BlobStorageNamespace::CompressedOplog {
                                environment_id: id.environment_id,
                                type_tag: id.agent_id.type_id().unwrap().get(),
                                agent_mode: mode,
                                level: 0,
                            },
                            &Path::new("chunks").join(agent_path_segment(&id.agent_id)),
                        )
                        .await?;
                    return anyhow::Ok(chunks.len());
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await?
    }

    async fn collected(
        &self,
        agents: &[(OwnedAgentId, AgentMode)],
        fingerprint: Option<AgentFingerprint>,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                self.renew()?;
                let mut keys = 0;
                for (id, mode) in agents {
                    keys += self
                        .blobs
                        .with("test", "wait_collected")
                        .list_page(
                            BlobStorageNamespace::SealRecords {
                                environment_id: id.environment_id,
                                type_tag: id.agent_id.type_id().unwrap().get(),
                                agent_mode: *mode,
                            },
                            &Path::new("seals").join(agent_path_segment(&id.agent_id)),
                            None,
                            1000,
                        )
                        .await?
                        .iter()
                        .filter(|entry| {
                            fingerprint.is_none_or(|fp| {
                                entry
                                    .path
                                    .file_stem()
                                    .unwrap()
                                    .to_string_lossy()
                                    .ends_with(&fp.to_string())
                            })
                        })
                        .count();
                }
                if keys == 0 {
                    return anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await?
    }
}

fn promise(value: &SchemaValue) -> PromiseId {
    PromiseId::from_value(value).expect("expected a promise ID returned by the guest")
}

async fn list_agents(
    executor: &TestWorkerExecutor,
    id: &OwnedAgentId,
    agent_type: &str,
    mode: AgentMode,
) -> anyhow::Result<Vec<AgentMetadataDto>> {
    let mut cursor = ScanCursor::default();
    let mut listed = Vec::new();
    loop {
        let (next, page) = executor
            .get_workers_metadata(
                &id.environment_id,
                &AgentTypeName(agent_type.to_string()),
                Some(AgentFilter::new_mode(FilterComparator::Equal, mode)),
                cursor,
                100,
                true,
            )
            .await?;
        listed.extend(page);
        match next {
            Some(next) => cursor = next,
            None => return Ok(listed),
        }
    }
}

#[test]
#[timeout("8m")]
async fn sealed_postgres_lifecycle_survives_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host: &PrecompiledComponent,
    #[tagged_as("agent_counters")] counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let stores = ArchiveStores::new(false).await?;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, stores.overrides.clone()).await?;
    stores.renew()?;
    let component = executor
        .component_dep(&context.default_environment_id, host)
        .store()
        .await?;
    let key = agent_key!("GolemHostApi", "sealed-postgres");
    let id = executor.start_agent(&component.id, key.clone()).await?;
    let value = executor
        .invoke_and_await_agent(&component, &key, "create_promise", data_value!())
        .await?
        .into_return_value()
        .unwrap();
    let external = promise(&value);
    let invocation = IdempotencyKey::fresh();
    let params = crate::raw_params(vec![SchemaValue::U64(8300), value]);
    executor
        .invoke_agent_with_key(
            &component,
            &key,
            &invocation,
            "sleep_then_local_calls_then_await_promise",
            params.clone(),
        )
        .await?;
    let timer = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let actions: Vec<Vec<u8>> =
                sqlx::query_scalar("SELECT action FROM scheduler.scheduled_actions")
                    .fetch_all(&stores.pool)
                    .await?;
            for bytes in actions {
                let action: ScheduledAction =
                    golem_common::serialization::deserialize(&bytes).map_err(anyhow::Error::msg)?;
                if let ScheduledAction::CompletePromise { promise_id, .. } = action
                    && promise_id.agent_id == id.agent_id
                    && promise_id != external
                {
                    return anyhow::Ok(promise_id);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("waiting for the real timer's CompletePromise action")??;
    let namespace = format!("promises:{}:{}", id.storage_key(), timer.fingerprint);
    let pending_timer: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM keyvalue.kv_storage WHERE namespace = $1 AND key = $2)",
    )
    .bind(&namespace)
    .bind(timer.storage_key(id.environment_id))
    .fetch_one(&stores.pool)
    .await?;
    assert!(
        pending_timer,
        "the real timer must persist its pending promise"
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let completed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM keyvalue.kv_storage WHERE namespace = $1 AND key = $2)",
            )
            .bind(&namespace)
            .bind(format!("{}:completed", timer.storage_key(id.environment_id)))
            .fetch_one(&stores.pool)
            .await?;
            if completed {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("waiting for the timer promise's completed KV row")??;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let metadata = executor.get_worker_metadata(&id).await?;
            if metadata.status == AgentStatus::Suspended
                && metadata.last_oplog_index.as_u64() > 8192
            {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("waiting for external-promise suspension after the large history")??;
    let chunks = stores.sealed(&id, AgentMode::Durable).await?;
    assert!(chunks > 1, "large history must exercise multiple chunks");
    stores.wait_counts([0, 0, 1, 0, 0, 0]).await?;
    let history = executor.stored_oplog(&id.agent_id).await;
    assert!(history.len() > 8192);
    let public_history = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    assert_eq!(
        stores.counts().await?,
        [0, 0, 1, 0, 0, 0],
        "only the external pending promise may remain"
    );
    let pending: Vec<(String, String)> =
        sqlx::query_as("SELECT namespace, key FROM keyvalue.kv_storage")
            .fetch_all(&stores.pool)
            .await?;
    assert_eq!(
        pending,
        vec![(
            format!("promises:{}:{}", id.storage_key(), external.fingerprint),
            external.storage_key(id.environment_id)
        )]
    );
    let metadata = executor.get_worker_metadata(&id).await?;
    assert_eq!(
        list_agents(&executor, &id, "GolemHostApi", AgentMode::Durable).await?,
        vec![metadata.clone()]
    );
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);

    let executor = start_with_overrides(deps, &context, stores.overrides.clone()).await?;
    stores.renew()?;
    assert_eq!(
        list_agents(&executor, &id, "GolemHostApi", AgentMode::Durable).await?,
        vec![metadata.clone()]
    );
    assert_eq!(executor.get_worker_metadata(&id).await?, metadata);
    assert_eq!(executor.stored_oplog(&id.agent_id).await, history);
    assert_eq!(
        executor.get_oplog(&id, OplogIndex::INITIAL).await?,
        public_history
    );
    assert_eq!(
        stores.counts().await?,
        [0, 0, 1, 0, 0, 0],
        "read-only APIs must not restore rows"
    );
    executor
        .complete_promise(&id.environment_id, &external, vec![42, 7])
        .await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &key,
            &invocation,
            "sleep_then_local_calls_then_await_promise",
            params,
        )
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(Vec::<u8>::from_value(&result).unwrap(), vec![42, 7]);
    executor
        .wait_for_status(&id, AgentStatus::Idle, Duration::from_secs(60))
        .await?;
    executor.delete_worker(&id).await?;
    let replacement = executor.start_agent(&component.id, key.clone()).await?;
    assert_eq!(replacement, id);
    assert_ne!(
        executor.agent_fingerprint(&replacement.agent_id).await?,
        external.fingerprint
    );
    let fresh_history = executor.stored_oplog(&replacement.agent_id).await;
    assert!(fresh_history.iter().all(|entry| !history.contains(entry)));
    let mut fresh_metadata = executor.get_worker_metadata(&replacement).await?;
    fresh_metadata.created_at = fresh_metadata.created_at.to_millis().into();
    stores
        .collected(
            &[(id.clone(), AgentMode::Durable)],
            Some(external.fingerprint),
        )
        .await?;
    assert_eq!(
        executor.stored_oplog(&replacement.agent_id).await,
        fresh_history
    );
    let mut collected_metadata = executor.get_worker_metadata(&replacement).await?;
    collected_metadata.created_at = collected_metadata.created_at.to_millis().into();
    assert_eq!(collected_metadata, fresh_metadata);
    let fresh_promise = executor
        .invoke_and_await_agent(&component, &key, "create_promise", data_value!())
        .await?
        .into_return_value()
        .unwrap();
    assert_eq!(
        promise(&fresh_promise).fingerprint,
        fresh_metadata.fingerprint
    );
    executor.delete_worker(&replacement).await?;
    stores
        .collected(&[(replacement, AgentMode::Durable)], None)
        .await?;
    stores.wait_counts([0; 6]).await?;

    let counter_component = executor
        .component_dep(&context.default_environment_id, counters)
        .store()
        .await?;
    let ephemeral_key = agent_key!("EphemeralCounter", "sealed-postgres-ephemeral");
    let invocation = IdempotencyKey::fresh();
    let ephemeral = crate::agent::ephemeral_agent_id(
        &executor,
        counter_component.environment_id,
        &ephemeral_key,
        &invocation,
        "increment",
    );
    executor
        .invoke_and_await_agent_with_key(
            &counter_component,
            &ephemeral_key,
            &invocation,
            "increment",
            data_value!(),
        )
        .await?;
    stores.sealed(&ephemeral, AgentMode::Ephemeral).await?;
    let ephemeral_history = executor.get_oplog(&ephemeral, OplogIndex::INITIAL).await?;
    assert!(!ephemeral_history.is_empty());
    let ephemeral_metadata = executor.get_worker_metadata(&ephemeral).await?;
    assert_eq!(
        list_agents(
            &executor,
            &ephemeral,
            "EphemeralCounter",
            AgentMode::Ephemeral
        )
        .await?,
        vec![ephemeral_metadata.clone()]
    );
    stores.wait_counts([0; 6]).await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_overrides(deps, &context, stores.overrides.clone()).await?;
    stores.renew()?;
    assert_eq!(
        executor.get_oplog(&ephemeral, OplogIndex::INITIAL).await?,
        ephemeral_history
    );
    assert_eq!(
        executor.get_worker_metadata(&ephemeral).await?,
        ephemeral_metadata
    );
    assert_eq!(
        list_agents(
            &executor,
            &ephemeral,
            "EphemeralCounter",
            AgentMode::Ephemeral
        )
        .await?,
        vec![ephemeral_metadata]
    );
    assert_eq!(stores.counts().await?, [0; 6]);
    executor.delete_worker(&ephemeral).await?;
    stores
        .collected(&[(ephemeral, AgentMode::Ephemeral)], None)
        .await?;
    stores.wait_counts([0; 6]).await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    Ok(())
}

#[test]
#[timeout("10m")]
async fn sealed_postgres_population_has_constant_rows(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host: &PrecompiledComponent,
    #[tagged_as("agent_counters")] counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let stores = ArchiveStores::new(true).await?;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, stores.overrides.clone()).await?;
    stores.renew()?;
    let component = executor
        .component_dep(&context.default_environment_id, host)
        .store()
        .await?;
    let counter_component = executor
        .component_dep(&context.default_environment_id, counters)
        .store()
        .await?;
    let mut active = Vec::new();
    for n in 0..3 {
        let key = agent_key!("Clocks", format!("active-{n}"));
        let id = executor.start_agent(&component.id, key.clone()).await?;
        executor
            .invoke_agent(&component, &key, "sleep_for", data_value!(600.0f64))
            .await?;
        executor
            .wait_for_status(&id, AgentStatus::Running, Duration::from_secs(10))
            .await?;
        active.push(id);
    }
    // Running timers need no durable wake until suspension; wait for constructor archive acks.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            stores.renew()?;
            let actions: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM scheduler.scheduled_actions")
                    .fetch_one(&stores.pool)
                    .await?;
            if actions == 0 {
                break anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("waiting for constructor ArchiveOplog actions to be acknowledged")??;
    // The active controls' constructor results are indexed asynchronously.
    tokio::time::timeout(Duration::from_secs(30), async {
        for id in &active {
            let worker = executor.active_agents().try_get(id).await.unwrap();
            loop {
                let metadata = worker.get_latest_worker_metadata().await;
                if let Some((key, _)) = metadata
                    .last_known_status
                    .invocation_results
                    .iter()
                    .find(|(_, result)| result.operation == Some(InvocationOperation::Constructor))
                {
                    let namespace = format!(
                        "agent-invocation-result-index:{}:{}",
                        id.storage_key(), metadata.fingerprint
                    );
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM keyvalue.kv_storage WHERE namespace = $1 AND key = $2)",
                    )
                    .bind(namespace)
                    .bind(format!("ir:{key}"))
                    .fetch_one(&stores.pool)
                    .await?;
                    if exists {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        anyhow::Ok(())
    })
    .await
    .context("waiting for the active constructors' result indexes")??;
    let baseline = stores.counts().await?;
    println!(
        "sealed_population=0 tables={TABLES:?} rows={baseline:?} sealed_pending_obligations=0"
    );
    let mut agents = Vec::new();
    let mut chunk_counts = BTreeSet::new();
    let mut ephemeral_lifecycle = Duration::ZERO;
    const LIST_CALLERS: [&str; 8] = [
        "prepare",
        "verify_record",
        "read_record",
        "read_attempt",
        "moved_data",
        "deletion",
        "gc_tombstones",
        "gc_revalidate",
    ];
    let mut ephemeral_seal_operations = [0u64; 9];
    let seal_operations = || {
        use golem_service_base::metrics::storage::LOGICAL_OPERATIONS_TOTAL;
        std::array::from_fn::<_, 9, _>(|index| {
            if index == 0 {
                LOGICAL_OPERATIONS_TOTAL
                    .with_label_values(&[
                        "blob",
                        "put_if_absent_once",
                        "oplog_seal",
                        "write_record",
                        "",
                    ])
                    .get()
            } else {
                LOGICAL_OPERATIONS_TOTAL
                    .with_label_values(&[
                        "blob",
                        "list_page",
                        "oplog_seal",
                        LIST_CALLERS[index - 1],
                        "",
                    ])
                    .get()
            }
        })
    };
    for size in [10, 50, 200] {
        while agents.len() < size {
            stores.renew()?;
            let n = agents.len();
            if n % 2 == 0 {
                let key = agent_key!("GolemHostApi", format!("population-{n}"));
                let id = executor.start_agent(&component.id, key.clone()).await?;
                let value = executor
                    .invoke_and_await_agent(&component, &key, "create_promise", data_value!())
                    .await?
                    .into_return_value()
                    .unwrap();
                let completed = promise(&value);
                executor
                    .complete_promise(&id.environment_id, &completed, vec![n as u8])
                    .await?;
                let calls = [0, 40, 160][n / 2 % 3];
                executor
                    .invoke_and_await_agent(
                        &component,
                        &key,
                        "local_calls_then_await_promise",
                        crate::raw_params(vec![SchemaValue::U64(calls), value]),
                    )
                    .await?;
                chunk_counts.insert(stores.sealed(&id, AgentMode::Durable).await?);
                agents.push((id, AgentMode::Durable));
            } else {
                let key = agent_key!("EphemeralCounter", format!("population-{n}"));
                let invocation = IdempotencyKey::fresh();
                let id = crate::agent::ephemeral_agent_id(
                    &executor,
                    counter_component.environment_id,
                    &key,
                    &invocation,
                    "increment",
                );
                let started = Instant::now();
                let before = seal_operations();
                executor
                    .invoke_and_await_agent_with_key(
                        &counter_component,
                        &key,
                        &invocation,
                        "increment",
                        data_value!(),
                    )
                    .await?;
                chunk_counts.insert(stores.sealed(&id, AgentMode::Ephemeral).await?);
                ephemeral_lifecycle += started.elapsed();
                let after = seal_operations();
                for index in 0..ephemeral_seal_operations.len() {
                    ephemeral_seal_operations[index] += after[index] - before[index];
                }
                agents.push((id, AgentMode::Ephemeral));
            }
        }
        // Release precedes acknowledgement of its archive action.
        let counts = stores.wait_counts(baseline).await?;
        for id in &active {
            assert_eq!(
                executor.get_worker_metadata(id).await?.status,
                AgentStatus::Running
            );
        }
        println!(
            "sealed_population={size} tables={TABLES:?} rows={counts:?} sealed_pending_obligations=0 ephemeral_invocation_to_observed_seal_total_ms={} ephemeral_seal_record_puts={} ephemeral_seal_record_lists={} list_callers={LIST_CALLERS:?} lists_by_caller={:?} ephemeral_retirements={}",
            ephemeral_lifecycle.as_millis(),
            ephemeral_seal_operations[0],
            ephemeral_seal_operations[1..].iter().sum::<u64>(),
            &ephemeral_seal_operations[1..],
            size / 2
        );
        assert_eq!(counts, baseline, "sealed population must add no SQL rows");
    }
    assert!(
        chunk_counts.len() > 1,
        "population must vary archive chunk counts: {chunk_counts:?}"
    );
    let idle_before = seal_operations();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let idle_after = seal_operations();
    println!(
        "sealed_population_idle_ms=5000 operations=[put, {LIST_CALLERS:?}] deltas={:?}",
        std::array::from_fn::<_, 9, _>(|i| idle_after[i] - idle_before[i])
    );
    for (id, _) in &agents {
        stores.renew()?;
        executor.delete_worker(id).await?;
    }
    stores.collected(&agents, None).await?;
    stores.wait_counts(baseline).await?;
    for id in active {
        executor.delete_worker(&id).await?;
    }
    executor.shutdown_and_wait_for_invocation_loops().await?;
    Ok(())
}
