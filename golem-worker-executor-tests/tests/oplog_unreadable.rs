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

//! An archived oplog chunk that does not decode fails its agent, and nothing else: the executor
//! starts on the data, the other agents run, and the chunk stays for an operator to inspect. An
//! archive that does not read while the agent runs reaches its owner, also when the status rebuild
//! that hits it follows a commit whose caller already took the acknowledgement.
//!
//! An archive that reads is all an agent needs: a keyed agent whose whole oplog is archived, and
//! whose derived status caches archiving removed, resolves by key and by ID with its state.

use crate::Tracing;
use crate::unreadable_call_paths::is_own_read_failure;

use bytes::Bytes;
use futures::stream::BoxStream;
use golem_common::model::agent::{AgentMode, AgentTypeName, ParsedAgentKey};
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    OplogEntry, OplogErrorKind, OplogIndex, OplogPayload, PublicOplogEntry,
};
use golem_common::model::worker::UpdateRecord;
use golem_common::model::{
    AgentId, AgentStatus, IdempotencyKey, OwnedAgentId, ScanCursor, ShardAssignment, ShardId,
};
use golem_common::{agent_key, data_value};
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobMissingError, BlobRangeStream, BlobStorage, BlobStorageBackend,
    BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath, PutIfAbsent,
    agent_path_segment,
};
use golem_test_framework::dsl::{AgentResult, TestDsl};
use golem_worker_executor::model::LookupResult;
use golem_worker_executor::services::HasEvents;
use golem_worker_executor::services::events::Event;
use golem_worker_executor::services::golem_config::OplogConfig;
use golem_worker_executor::services::oplog::CompressedOplogChunk;
use golem_worker_executor::storage::indexed::sqlite::SqliteIndexedStorage;
use golem_worker_executor::storage::indexed::{IndexedStorageLabelledApi, IndexedStorageNamespace};
use golem_worker_executor::storage::keyvalue::sqlite::SqliteKeyValueStorage;
use golem_worker_executor::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use golem_worker_executor::storage::scheduler::SchedulerStorage;
use golem_worker_executor::storage::scheduler::sqlite::SqliteSchedulerStorage;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, agent_oplog_length, archived_oplog_chunks,
    scheduler_sqlite_storage_config, sqlite_storage_config, start_with_oplog_config,
    start_with_overrides, truncate_archived_oplog_chunk,
};
use pretty_assertions::assert_eq;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// The compressed archive level the agents' oplogs move to; with no blob level they stay there.
const ARCHIVE_LEVEL: usize = 1;

#[test]
#[timeout("4m")]
async fn an_unknown_primary_version_fails_only_its_agent_and_a_cache_version_is_rebuilt(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let config = OplogConfig {
        indexed_storage_layers: 1,
        blob_storage_layers: 0,
        ..Default::default()
    };
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(move |runtime| {
            runtime.oplog = config.clone();
            runtime.suspend.suspend_after = Duration::from_secs(300);
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let unreadable = agent_key!("Counter", "unknown-primary-version");
    let healthy = agent_key!("Counter", "unknown-cache-version");
    let unreadable_id = executor
        .start_agent(&component.id, unreadable.clone())
        .await?;
    let healthy_id = executor.start_agent(&component.id, healthy.clone()).await?;
    let (_, healthy_result) = futures::try_join!(
        executor.invoke_and_await_agent(&component, &unreadable, "increment", data_value!()),
        executor.invoke_and_await_agent(&component, &healthy, "increment", data_value!()),
    )?;
    assert_eq!(healthy_result.into_typed::<u32>()?, 1);
    futures::try_join!(
        executor.wait_for_status(&unreadable_id, AgentStatus::Idle, Duration::from_secs(30)),
        executor.wait_for_status(&healthy_id, AgentStatus::Idle, Duration::from_secs(30)),
    )?;
    executor.release().await?;

    let storage_config = sqlite_storage_config(deps, &context);
    let indexed_config =
        golem_worker_executor::derive_disjoint_sqlite_config(&storage_config, "indexed");
    let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
    let namespace = IndexedStorageNamespace::OpLog {
        owned_agent_id: unreadable_id.clone(),
        agent_mode: AgentMode::Durable,
    };
    let key = unreadable_id.storage_key();
    let (_, OplogEntry::Create { parameters, .. }) = indexed
        .with_entity("test", "read_identity", "entry")
        .first::<OplogEntry>(
            IndexedStorageNamespace::OpLog {
                owned_agent_id: healthy_id.clone(),
                agent_mode: AgentMode::Durable,
            },
            &healthy_id.storage_key(),
        )
        .await?
        .expect("the healthy agent must have a Create entry")
    else {
        panic!("the first oplog entry must be Create");
    };
    let cache_namespace = KeyValueStorageNamespace::AgentStatus {
        owned_agent_id: healthy_id.clone(),
        fingerprint: golem_common::model::AgentFingerprint(parameters.instance_id),
    };
    let kv = SqliteKeyValueStorage::configured(&storage_config).await?;
    kv.with_entity("test", "corrupt_version", "status")
        .set_raw(cache_namespace.clone(), "core", &[255])
        .await
        .map_err(anyhow::Error::msg)?;

    let executor = start_with_overrides(deps, &context, overrides).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &healthy, "count", data_value!())
            .await?
            .into_typed::<u32>()?,
        1
    );
    let generation = Arc::downgrade(&executor.active_agent(&healthy_id).await.unwrap().primary());
    let loads = executor.instance_load_count(&healthy_id.agent_id);
    assert!(loads > 0);
    assert!(executor.worker_is_loaded(&healthy_id).await);
    assert!(executor.active_agent(&unreadable_id).await.is_none());
    executor
        .remove_cached_status(&unreadable_id.agent_id)
        .await?;
    assert!(executor.active_agent(&unreadable_id).await.is_none());

    let mut corrupt = indexed
        .with_entity("test", "corrupt_version", "entry")
        .read_raw(namespace.clone(), &key, 2, 2)
        .await?
        .pop()
        .expect("the invocation must have stored a second entry")
        .1;
    assert_eq!(corrupt[0], 3);
    corrupt[0] = 255;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", indexed_config.database)).await?;
    assert_eq!(
        sqlx::query("UPDATE index_storage SET value = ? WHERE namespace = 'durable-worker-oplog' AND key = ? AND id = 2")
            .bind(&corrupt)
            .bind(&key)
            .execute(&pool)
            .await?
            .rows_affected(),
        1
    );
    pool.close().await;
    let before_failure = StoredOplog::read(deps, &context, &unreadable_id).await?;

    let error = executor
        .invoke_and_await_agent(&component, &unreadable, "increment", data_value!())
        .await
        .expect_err("the unknown oplog version must fail its agent");
    let message = format!("{error:#}");
    assert!(
        message.contains("unknown serialization version: 255"),
        "{message}"
    );
    assert!(message.contains("entry 2"), "{message}");
    assert!(
        message.contains(&unreadable_id.agent_id.to_string()),
        "{message}"
    );
    assert!(generation.ptr_eq(&Arc::downgrade(
        &executor.active_agent(&healthy_id).await.unwrap().primary()
    )));
    assert!(executor.worker_is_loaded(&healthy_id).await);
    assert_eq!(executor.instance_load_count(&healthy_id.agent_id), loads);
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        2
    );
    assert!(generation.ptr_eq(&Arc::downgrade(
        &executor.active_agent(&healthy_id).await.unwrap().primary()
    )));
    assert!(executor.worker_is_loaded(&healthy_id).await);
    assert_eq!(executor.instance_load_count(&healthy_id.agent_id), loads);
    let rebuilt = kv
        .with_entity("test", "read_cache", "status")
        .get_raw(cache_namespace, "core")
        .await
        .map_err(anyhow::Error::msg)?
        .expect("the status cache must have been rebuilt");
    assert!(
        golem_common::serialization::decode::<golem_common::model::AgentStatusRecord>(&rebuilt)
            .is_ok()
    );
    assert_eq!(
        indexed
            .with_entity("test", "read_corrupt", "entry")
            .read_raw(namespace, &key, 2, 2)
            .await?,
        vec![(2, corrupt)]
    );
    // The startup read fails before the executor admits the invocation, so the oplog of the
    // agent stays as it was: no pending invocation and no outcome.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    before_failure.assert_adds_only_admitted_work(
        &StoredOplog::read(deps, &context, &unreadable_id).await?,
        0,
    )?;
    Ok(())
}

/// Archives the oplogs of idle agents within a second, into the compressed level only.
fn archiving_config() -> OplogConfig {
    OplogConfig {
        indexed_storage_layers: 2,
        blob_storage_layers: 0,
        archive_interval: Duration::from_secs(1),
        entry_count_limit: 5,
        ..Default::default()
    }
}

/// Checks that the invocation failure names the agent, the archive chunk and the codec version.
fn assert_fails_with_the_decode_error<T: std::fmt::Debug>(
    result: anyhow::Result<T>,
    agent_id: &AgentId,
    chunk_end: u64,
) {
    let error = result.expect_err("an agent whose oplog does not read must not run");
    let error = format!("{error:#}");
    let agent = agent_id.to_string();
    assert!(
        (error.contains(&agent) || error.contains(&agent.escape_debug().to_string()))
            && error.contains("archive layer 1")
            && error.contains(&format!(
                "failed to decode compressed oplog chunk ending at {chunk_end}:"
            ))
            && error.contains("invalid data for serialization version 3"),
        "the failure does not name the agent, the archive chunk and the codec version: {error}"
    );
}

/// Waits until the primary oplog of the agent is empty, so its whole history, `Create` included,
/// is in the archive.
async fn wait_until_archived(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    owned_agent_id: &OwnedAgentId,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while agent_oplog_length(deps, context, owned_agent_id).await? > 0 {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the oplog of {owned_agent_id} was not archived"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}

/// Waits until scheduler storage holds no archive action, due or not, leased or not.
async fn wait_until_archive_actions_finish(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<()> {
    let scheduler =
        SqliteSchedulerStorage::configured(&scheduler_sqlite_storage_config(deps, context))
            .await
            .map_err(anyhow::Error::msg)?;
    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while scheduler
        .count_due(jiff::Timestamp::MAX, &assignment)
        .await?
        > 0
    {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "archive actions did not finish"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}

/// The stored history of a durable agent, as bytes: the primary oplog entries and the chunks of
/// the archive level.
#[derive(Debug)]
struct StoredOplog {
    primary: Vec<(u64, Vec<u8>)>,
    archive: Vec<(u64, Vec<u8>)>,
}

impl StoredOplog {
    async fn read(
        deps: &WorkerExecutorTestDependencies,
        context: &TestContext,
        owned_agent_id: &OwnedAgentId,
    ) -> anyhow::Result<Self> {
        let indexed = SqliteIndexedStorage::configured(
            &golem_worker_executor::derive_disjoint_sqlite_config(
                &sqlite_storage_config(deps, context),
                "indexed",
            ),
        )
        .await?;
        let primary = indexed
            .with_entity("test", "read_primary", "entry")
            .read_raw(
                IndexedStorageNamespace::OpLog {
                    owned_agent_id: owned_agent_id.clone(),
                    agent_mode: AgentMode::Durable,
                },
                &owned_agent_id.storage_key(),
                0,
                i64::MAX as u64,
            )
            .await?;
        Ok(Self {
            primary,
            archive: archived_oplog_chunks(deps, context, owned_agent_id, ARCHIVE_LEVEL).await?,
        })
    }

    /// Checks that `after` keeps every primary entry and archive chunk of this history with the
    /// same bytes, and returns the entries that `after` adds, by oplog index. An entry that
    /// moved from the primary into a new archive chunk is returned once.
    fn added_entries(&self, after: &Self) -> anyhow::Result<BTreeMap<u64, OplogEntry>> {
        anyhow::ensure!(
            after.primary.starts_with(&self.primary),
            "the failed read changed or removed primary entries: before {:?}, after {:?}",
            self.primary,
            after.primary
        );
        anyhow::ensure!(
            after.archive.starts_with(&self.archive),
            "the failed read changed or removed archive chunks: before {:?}, after {:?}",
            self.archive,
            after.archive
        );
        let primary = after.primary[self.primary.len()..]
            .iter()
            .map(|(index, bytes)| {
                golem_common::serialization::decode::<OplogEntry>(bytes)
                    .map(|entry| (*index, entry))
                    .map_err(|error| anyhow::anyhow!("added primary entry {index}: {error}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let archive = after.archive[self.archive.len()..]
            .iter()
            .map(|(last, bytes)| {
                let chunk: CompressedOplogChunk = golem_common::serialization::decode(bytes)
                    .map_err(|error| anyhow::anyhow!("added archive chunk {last}: {error}"))?;
                let first = last + 1 - chunk.count;
                Ok(chunk
                    .decompress()?
                    .into_iter()
                    .zip(first..)
                    .map(|(entry, index)| (index, entry))
                    .collect::<Vec<_>>())
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(archive.into_iter().flatten().chain(primary).collect())
    }

    /// Checks that `after` keeps this history and adds only the `PendingAgentInvocation` entries
    /// of the `admitted` invocations, which the executor took before the read failed. The failed
    /// read must not add an `Error`, a cancellation, a terminal entry or any other entry.
    fn assert_adds_only_admitted_work(&self, after: &Self, admitted: usize) -> anyhow::Result<()> {
        let added = self.added_entries(after)?;
        anyhow::ensure!(
            added.len() == admitted
                && added
                    .values()
                    .all(|entry| matches!(entry, OplogEntry::PendingAgentInvocation { .. })),
            "the failed read must add only the {admitted} admitted invocations: {added:#?}"
        );
        Ok(())
    }

    /// Checks that `after` keeps this history and adds the `PendingAgentInvocation` entries of the
    /// `admitted` invocations, then the one `Recovery` error of `agent_id` that holds each of
    /// `parts`. The failed read must not add a cancellation, a terminal entry or any other entry.
    fn assert_adds_admitted_work_and_the_failure(
        &self,
        after: &Self,
        admitted: usize,
        agent_id: &AgentId,
        parts: &[&str],
    ) -> anyhow::Result<()> {
        let added = self.added_entries(after)?.into_values().collect::<Vec<_>>();
        anyhow::ensure!(
            added.len() == admitted + 1
                && added[..admitted]
                    .iter()
                    .all(|entry| matches!(entry, OplogEntry::PendingAgentInvocation { .. }))
                && is_own_read_failure(&added[admitted], agent_id, parts),
            "the failed read must add only the {admitted} admitted invocations and the Recovery error of {agent_id}: {added:#?}"
        );
        Ok(())
    }
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn an_undecodable_archive_chunk_fails_only_its_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let unreadable = agent_key!("Counter", "unreadable-archive");
    let healthy = agent_key!("Counter", "healthy-archive");
    let unreadable_id = executor
        .start_agent(&component.id, unreadable.clone())
        .await?;
    let healthy_id = executor.start_agent(&component.id, healthy.clone()).await?;
    for agent in [&unreadable, &healthy] {
        for _ in 0..3 {
            executor
                .invoke_and_await_agent(&component, agent, "increment", data_value!())
                .await?;
        }
    }
    let unreadable_owned = unreadable_id.clone();
    let healthy_owned = healthy_id.clone();
    wait_until_archived(deps, &context, &unreadable_owned).await?;
    wait_until_archived(deps, &context, &healthy_owned).await?;
    drop(executor);

    truncate_archived_oplog_chunk(deps, &context, &unreadable_owned, ARCHIVE_LEVEL, 0).await?;
    let before_failure = StoredOplog::read(deps, &context, &unreadable_owned).await?;

    // The executor starts on the data, and recovers every agent it can read.
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;

    assert_fails_with_the_decode_error(
        executor
            .invoke_and_await_agent(&component, &unreadable, "increment", data_value!())
            .await,
        &unreadable_id.agent_id,
        before_failure.archive[0].0,
    );

    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        4
    );
    assert_eq!(
        executor.get_worker_metadata(&healthy_id).await?.status,
        AgentStatus::Idle
    );

    // The startup read fails before the executor admits the invocation, so the primary and the
    // archive stay as they were: no pending invocation and no outcome.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    before_failure.assert_adds_only_admitted_work(
        &StoredOplog::read(deps, &context, &unreadable_owned).await?,
        0,
    )?;
    Ok(())
}

/// The `Create` of the agent reads, and a later archived chunk does not: the agent is found, its
/// cached status admits the invocation and startup, whose replay scan fails with the cause, and the
/// agent is not run. The chunk does not decode, so the read fails for good and fails the agent.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn an_undecodable_later_archive_chunk_fails_only_its_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    let unreadable = agent_key!("Counter", "unreadable-later-chunk");
    let healthy = agent_key!("Counter", "healthy-later-chunk");
    let unreadable_id = executor
        .start_agent(&component.id, unreadable.clone())
        .await?;
    let healthy_id = executor.start_agent(&component.id, healthy.clone()).await?;
    let unreadable_owned = unreadable_id.clone();
    let healthy_owned = healthy_id.clone();
    // Two rounds of invocations, each archived on its own, leave two chunks per agent.
    for _ in 0..2 {
        for agent in [&unreadable, &healthy] {
            for _ in 0..2 {
                executor
                    .invoke_and_await_agent(&component, agent, "increment", data_value!())
                    .await?;
            }
        }
        wait_until_archived(deps, &context, &unreadable_owned).await?;
        wait_until_archived(deps, &context, &healthy_owned).await?;
    }
    // The primary can empty before the final archive action invalidates the cache. Count
    // future-due and leased actions too, so none can erase the metadata warmed below.
    wait_until_archive_actions_finish(deps, &context).await?;
    // Archival removes the status cache. Retire the shell and read metadata while the archive
    // still reads, so the next startup reaches replay instead of failing a cold status fold.
    assert!(executor.stop_worker_if_idle(&unreadable_owned).await?);
    executor.retire_unloaded_worker(&unreadable_owned).await?;
    executor.get_worker_metadata(&unreadable_id).await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);

    let chunks = archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?;
    anyhow::ensure!(
        chunks.len() >= 2,
        "expected two archived chunks, got {}",
        chunks.len()
    );
    truncate_archived_oplog_chunk(
        deps,
        &context,
        &unreadable_owned,
        ARCHIVE_LEVEL,
        chunks.len() - 1,
    )
    .await?;
    let before_failure = StoredOplog::read(deps, &context, &unreadable_owned).await?;

    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;

    assert_eq!(
        executor.get_worker_metadata(&unreadable_id).await?.status,
        AgentStatus::Idle
    );
    assert_fails_with_the_decode_error(
        tokio::time::timeout(
            Duration::from_secs(30),
            executor.invoke_and_await_agent(&component, &unreadable, "increment", data_value!()),
        )
        .await?,
        &unreadable_id.agent_id,
        chunks.last().unwrap().0,
    );

    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        5
    );

    // The cached status admits the invocation, so its `PendingAgentInvocation` is the one entry
    // that the executor adds before replay fails. The failed replay fails the agent: it adds the
    // `Recovery` error that names the agent and the chunk, no cancellation or terminal entry, and
    // keeps every primary entry and archive chunk as it was.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    before_failure.assert_adds_admitted_work_and_the_failure(
        &StoredOplog::read(deps, &context, &unreadable_owned).await?,
        1,
        &unreadable_id.agent_id,
        &[
            "archive layer 1",
            &format!(
                "failed to decode compressed oplog chunk ending at {}:",
                chunks.last().unwrap().0
            ),
            "invalid data for serialization version 3",
        ],
    )?;
    Ok(())
}

#[test]
#[timeout("4m")]
async fn unreadable_replay_payloads_do_not_settle_an_automatic_update(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let check = async |corrupt_started| -> anyhow::Result<()> {
        let context = TestContext::new(last_unique_id);
        let config = OplogConfig {
            indexed_storage_layers: 1,
            blob_storage_layers: 0,
            ..Default::default()
        };
        let executor = start_with_oplog_config(deps, &context, Some(config.clone())).await?;
        let component = executor
            .component_dep(&context.default_environment_id, agent_counters)
            .store()
            .await?;
        let agent = agent_key!("Counter", "unreadable-replay-payload");
        let healthy = agent_key!("Counter", "healthy-next-to-unreadable-update");
        let agent_id = executor.start_agent(&component.id, agent.clone()).await?;
        let healthy_id = executor.start_agent(&component.id, healthy.clone()).await?;
        let key = IdempotencyKey::fresh();
        let invocation_id = executor
            .external_invocation_key(&agent_id.agent_id, &key, "increment")
            .await?;
        assert_eq!(
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &agent,
                    &key,
                    "increment",
                    data_value!()
                )
                .await?
                .into_typed::<u32>()?,
            1
        );
        executor
            .wait_for_status(&agent_id, AgentStatus::Idle, Duration::from_secs(30))
            .await?;
        // An interrupt of an idle agent is ignored, so the agent must run when it is interrupted.
        // An automatic update of an interrupted agent with disabled wake-up only queues.
        executor
            .invoke_agent(&component, &agent, "busy_for", data_value!(120_000u32))
            .await?;
        executor
            .wait_for_status(&agent_id, AgentStatus::Running, Duration::from_secs(30))
            .await?;
        executor.interrupt(&agent_id).await?;
        executor
            .wait_for_status(&agent_id, AgentStatus::Interrupted, Duration::from_secs(30))
            .await?;
        let target = executor
            .update_component(&component.id, "it_agent_counters_release")
            .await?;
        executor
            .auto_update_worker(&agent_id, target.revision, true)
            .await?;
        let before_failure = executor.oplog_max_index(&agent_id).await?;
        executor.release().await?;

        let storage_config = sqlite_storage_config(deps, &context);
        let indexed_config =
            golem_worker_executor::derive_disjoint_sqlite_config(&storage_config, "indexed");
        let indexed = SqliteIndexedStorage::configured(&indexed_config).await?;
        let namespace = IndexedStorageNamespace::OpLog {
            owned_agent_id: agent_id.clone(),
            agent_mode: AgentMode::Durable,
        };
        let storage_key = agent_id.storage_key();
        let rows = indexed
            .with_entity("test", "read_replay_payload", "entry")
            .read_raw(namespace, &storage_key, 1, before_failure.as_u64())
            .await?;
        let (index, original, corrupted) = rows
            .into_iter()
            .find_map(|(index, bytes)| {
                let mut entry = golem_common::serialization::decode::<OplogEntry>(&bytes).unwrap();
                match &mut entry {
                    OplogEntry::AgentInvocationStarted {
                        invocation_id: id,
                        payload,
                        ..
                    } if corrupt_started && *id == invocation_id => {
                        *payload = OplogPayload::SerializedInline {
                            bytes: vec![255],
                            cached: None,
                        };
                    }
                    OplogEntry::AgentInvocationFinished {
                        invocation_id: id,
                        result,
                        ..
                    } if !corrupt_started && *id == invocation_id => {
                        *result = OplogPayload::SerializedInline {
                            bytes: vec![255],
                            cached: None,
                        };
                    }
                    _ => return None,
                }
                Some((
                    index,
                    bytes,
                    golem_common::serialization::serialize(&entry).unwrap(),
                ))
            })
            .expect("the completed invocation must have a Started and a Finished entry");
        let pool =
            sqlx::SqlitePool::connect(&format!("sqlite://{}", indexed_config.database)).await?;
        let replace = async |bytes: &[u8]| -> anyhow::Result<()> {
            assert_eq!(
                sqlx::query("UPDATE index_storage SET value = ? WHERE namespace = 'durable-worker-oplog' AND key = ? AND id = ?")
                    .bind(bytes)
                    .bind(&storage_key)
                    .bind(i64::try_from(index)?)
                    .execute(&pool)
                    .await?
                    .rows_affected(),
                1
            );
            Ok(())
        };
        replace(&corrupted).await?;

        let executor = start_with_oplog_config(deps, &context, Some(config)).await?;
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
                .await?
                .into_typed::<u32>()?,
            1
        );
        let mut events = executor.event_bus(&healthy_id.agent_id).await?.subscribe();
        executor.resume(&agent_id, true).await?;
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            events.wait_for(|event| match event {
                Event::WorkerLoaded {
                    agent_id: id,
                    result,
                    ..
                } if id == &agent_id.agent_id => Some(result.clone()),
                _ => None,
            }),
        )
        .await??;
        let error = result.expect_err("the unreadable replay entry must prevent startup");
        let message = error.to_string();
        assert!(
            message.contains(&format!("oplog index {index}"))
                && message.contains("unknown serialization version: 255"),
            "{message}"
        );
        assert_eq!(
            executor
                .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
                .await?
                .into_typed::<u32>()?,
            2
        );
        assert!(!executor.worker_is_loaded(&agent_id).await);
        // The start records the resume and the strategy of the update attempt before it reads
        // the history. The payload does not decode, so the read fails for good and fails the
        // agent: its `Recovery` error follows, and the update gets no `FailedUpdate`.
        let added = indexed
            .with_entity("test", "read_after_failure", "entry")
            .read_raw(
                IndexedStorageNamespace::OpLog {
                    owned_agent_id: agent_id.clone(),
                    agent_mode: AgentMode::Durable,
                },
                &storage_key,
                before_failure.next().as_u64(),
                u64::MAX,
            )
            .await?
            .into_iter()
            .map(|(_, bytes)| golem_common::serialization::decode::<OplogEntry>(&bytes))
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            matches!(
                added.as_slice(),
                [
                    OplogEntry::Resumed { .. },
                    OplogEntry::PendingUpdate {
                        update_attempt_index: Some(_),
                        ..
                    },
                    failure,
                ] if is_own_read_failure(
                    failure,
                    &agent_id.agent_id,
                    &[
                        &format!("oplog index {index}"),
                        "unknown serialization version: 255",
                    ],
                )
            ),
            "a failed read must add only the Recovery error to the durable tail: {added:?}"
        );
        let after_start = OplogIndex::from_u64(before_failure.as_u64() + 3);
        replace(&original).await?;
        let metadata = executor.get_worker_metadata(&agent_id).await?;
        assert_eq!(metadata.status, AgentStatus::Failed);
        assert_eq!(metadata.component_revision, component.revision);
        assert!(matches!(
            metadata.updates.as_slice(),
            [UpdateRecord::PendingUpdate(_)]
        ));
        assert_eq!(executor.oplog_max_index(&agent_id).await?, after_start);
        Ok(())
    };
    check(true).await?;
    check(false).await?;
    Ok(())
}

/// Keyed agents that are archived with their whole oplog, and whose derived status caches
/// archiving removed, are reached again by key and by ID after a restart: the oplog alone
/// resolves an agent, and the agent keeps its state and its incarnation.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn archived_keyed_agents_resolve_by_key_and_by_id_from_their_oplog(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;

    // Different counts, so that a swapped or a fresh agent shows in the next result.
    let keys = [
        agent_key!("Counter", "archived-1"),
        agent_key!("Counter", "archived-2"),
        agent_key!("Counter", "archived-3"),
    ];
    let mut agents = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        let agent_id = executor.start_agent(&component.id, key.clone()).await?;
        for _ in 0..=index {
            executor
                .invoke_and_await_agent(&component, key, "increment", data_value!())
                .await?;
        }
        let fingerprint = executor.agent_fingerprint(&agent_id.agent_id).await?;
        agents.push((agent_id, fingerprint));
    }

    for (agent_id, _) in &agents {
        wait_until_archived(deps, &context, agent_id).await?;
    }
    // Archiving removes the caches only when its last action moves the whole oplog to the last
    // layer, so wait for every archive action, due or not.
    wait_until_archive_actions_finish(deps, &context).await?;
    // Nothing reads the agents from here on: a metadata read or an identity resolution would
    // write the caches again.
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);

    let kv = SqliteKeyValueStorage::configured(&sqlite_storage_config(deps, &context)).await?;
    for (agent_id, fingerprint) in &agents {
        for namespace in [
            KeyValueStorageNamespace::AgentStatus {
                owned_agent_id: agent_id.clone(),
                fingerprint: *fingerprint,
            },
            KeyValueStorageNamespace::AgentStatusCheckpoint {
                owned_agent_id: agent_id.clone(),
                fingerprint: *fingerprint,
            },
            KeyValueStorageNamespace::AgentInvocationResultIndex {
                owned_agent_id: agent_id.clone(),
                fingerprint: *fingerprint,
            },
        ] {
            let cached = kv.keys("test", "archive", namespace.clone()).await?;
            assert!(
                cached.is_empty(),
                "{namespace:?} is still cached for an archived agent: {cached:?}"
            );
        }
        // The agent-mode hint key of `DefaultWorkerService::agent_mode_key`.
        let agent_mode_key = format!("worker:agent_mode:{}", agent_id.storage_key());
        assert!(
            !kv.exists(
                "test",
                "archive",
                KeyValueStorageNamespace::Worker {
                    owned_agent_id: agent_id.clone(),
                },
                &agent_mode_key,
            )
            .await?,
            "the agent mode of {agent_id} is still cached"
        );
    }

    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;
    let [first, second, _] = &agents[..] else {
        unreachable!("three agents were started")
    };

    // By ID first, then by key.
    assert_eq!(
        executor
            .invoke_and_await_agent_by_id(&component, &first.0, None, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        2
    );
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &keys[0], "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        3
    );

    // An upsert first, then by ID.
    assert_eq!(
        executor.start_agent(&component.id, keys[1].clone()).await?,
        second.0
    );
    assert_eq!(
        executor
            .invoke_and_await_agent_by_id(&component, &second.0, None, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        3
    );

    // By key only.
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &keys[2], "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        4
    );

    for (agent_id, fingerprint) in &agents {
        assert_eq!(
            executor.agent_fingerprint(&agent_id.agent_id).await?,
            *fingerprint
        );
    }
    Ok(())
}

/// Keeps only the blob archive below the primary oplog, and moves every committed entry into it.
fn blob_archiving_config() -> OplogConfig {
    OplogConfig {
        indexed_storage_layers: 1,
        blob_storage_layers: 1,
        entry_count_limit: 1,
        ..Default::default()
    }
}

#[test]
#[timeout("2 minutes")]
async fn an_ephemeral_constructor_completes_while_its_archive_write_is_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] agent_rpc_rust: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let release = tokio_util::sync::CancellationToken::new();
    let _release_on_drop = release.clone().drop_guard();
    let fault = Arc::new(ArchiveFault {
        write_release: Some(release.clone()),
        ..Default::default()
    });
    let wrapper_fault = fault.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog = blob_archiving_config();
                config.invocation_results.recent_capacity = 0;
                config.invocation_results.hydrated_cache_capacity = 0;
                config.invocation_results.bloom_bits = 1;
                config.invocation_results.bloom_hashes = 1;
            })),
            wrap_blob_storage: Some(Arc::new(move |inner| {
                Arc::new(FlakyArchiveStorage {
                    inner,
                    fault: wrapper_fault.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_rpc_rust)
        .store()
        .await?;
    let agent = agent_key!("EphemeralStreamingRpcTarget", "pending-constructor-archive");
    let agent_id = crate::agent::ephemeral_agent_id(
        &executor,
        component.environment_id,
        &agent,
        &IdempotencyKey::fresh(),
        "produce_gated_siblings",
    );
    let creation = executor.create_unkeyed_agent(
        &component.id,
        Some(&agent_id.agent_id),
        agent,
        HashMap::new(),
    );
    let (created, ()) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(creation, fault.write_blocked.notified())
    })
    .await
    .map_err(|_| anyhow::anyhow!("creation did not finish with its archive write held"))?;
    assert_eq!(created?, agent_id);
    release.cancel();
    Ok(())
}

const ARCHIVE_UP: u8 = 0;
const ARCHIVE_UNAVAILABLE: u8 = 1;
const ARCHIVE_CORRUPT: u8 = 2;
const ARCHIVE_UNKNOWN_VERSION: u8 = 3;

/// How a [`FlakyArchiveStorage`] answers the reads of archived oplog chunks.
///
/// The fault applies only to the history: the chunks whose last entry is at or before
/// `history_end`. A status rebuild reads that history.
#[derive(Debug, Default)]
struct ArchiveFault {
    agent_path: OnceLock<String>,
    mode: AtomicU8,
    history_end: AtomicU64,
    unavailable_read: tokio::sync::Notify,
    write_release: Option<tokio_util::sync::CancellationToken>,
    write_blocked: tokio::sync::Notify,
}

impl ArchiveFault {
    fn fail_history(&self, mode: u8, history_end: OplogIndex) {
        self.history_end
            .store(u64::from(history_end), Ordering::SeqCst);
        self.mode.store(mode, Ordering::SeqCst);
    }

    fn restore(&self) {
        self.mode.store(ARCHIVE_UP, Ordering::SeqCst);
    }

    async fn observe_write(&self, namespace: &BlobStorageNamespace, data: &[u8]) {
        if !matches!(namespace, BlobStorageNamespace::CompressedOplog { .. }) {
            return;
        }
        let entries = golem_common::serialization::deserialize::<CompressedOplogChunk>(data)
            .expect("the archive stores compressed oplog chunks")
            .decompress()
            .expect("an archived chunk decompresses");
        if let Some(release) = &self.write_release
            && matches!(
                namespace,
                BlobStorageNamespace::CompressedOplog {
                    agent_mode: AgentMode::Ephemeral,
                    ..
                }
            )
            && entries
                .iter()
                .any(|entry| matches!(entry, OplogEntry::AgentInvocationFinished { .. }))
        {
            self.write_blocked.notify_one();
            release.cancelled().await;
        }
    }

    /// The answer to a read of the object at `path` in `namespace` that replaces the stored one.
    /// An unavailable archive fails the read as a backend whose outage outlived its retries, so
    /// the failure is transient. A corrupt archive gives bytes that do not decode.
    fn read(
        &self,
        namespace: &BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Option<anyhow::Result<Option<Vec<u8>>>> {
        if !matches!(namespace, BlobStorageNamespace::CompressedOplog { .. }) {
            return None;
        }
        if !self
            .agent_path
            .get()
            .is_some_and(|agent| path.starts_with(agent))
        {
            return None;
        }
        // Archived chunk objects are named `<last index>-<uuid>`.
        let chunk_end = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split('-').next())
            .and_then(|index| index.parse::<u64>().ok());
        if chunk_end.is_none_or(|end| end > self.history_end.load(Ordering::SeqCst)) {
            return None;
        }
        match self.mode.load(Ordering::SeqCst) {
            ARCHIVE_UNAVAILABLE => {
                self.unavailable_read.notify_one();
                Some(Err(anyhow::anyhow!(
                    "injected 503: the oplog archive is down"
                )))
            }
            ARCHIVE_CORRUPT => {
                let mut bytes = vec![golem_common::serialization::SERIALIZATION_VERSION_V3];
                bytes.extend_from_slice(b"not a compressed oplog chunk");
                Some(Ok(Some(bytes)))
            }
            ARCHIVE_UNKNOWN_VERSION => Some(Ok(Some(b"not a compressed oplog chunk".to_vec()))),
            _ => None,
        }
    }
}

/// A blob storage whose archived oplog chunks fail to read on demand. Every other operation goes
/// to the storage that it wraps.
#[derive(Debug)]
struct FlakyArchiveStorage {
    inner: Arc<dyn BlobStorage>,
    fault: Arc<ArchiveFault>,
}

#[async_trait::async_trait]
impl BlobStorageBackend for FlakyArchiveStorage {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if let Some(answer) = self.fault.read(&namespace, path) {
            return answer;
        }
        self.inner
            .get_raw(target_label, op_label, namespace, path)
            .await
    }

    async fn get_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BoxStream<'static, anyhow::Result<Bytes>>>> {
        self.inner
            .get_stream(target_label, op_label, namespace, path)
            .await
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<Option<BlobRangeStream>> {
        self.inner
            .get_range_stream(target_label, op_label, namespace, path, offset, length)
            .await
    }

    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner
            .get_raw_slice(target_label, op_label, namespace, path, start, end)
            .await
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.inner
            .get_metadata(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.fault.observe_write(&namespace, data).await;
        self.inner
            .put_raw(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.fault.observe_write(&namespace, data).await;
        self.inner
            .put_raw_if_absent(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = anyhow::Result<Vec<u8>>, Error = anyhow::Error>,
    ) -> anyhow::Result<()> {
        self.inner
            .put_stream(target_label, op_label, namespace, path, stream)
            .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner
            .delete(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_many_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[NormalizedBlobPath<'_>],
    ) -> anyhow::Result<()> {
        let paths = paths
            .iter()
            .map(|path| path.to_path_buf())
            .collect::<Vec<_>>();
        self.inner
            .delete_many(target_label, op_label, namespace, &paths)
            .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner
            .create_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner
            .list_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner
            .list_blobs_below(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<u64>> {
        self.inner
            .delete_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<ExistsResult> {
        self.inner
            .exists(target_label, op_label, namespace, path)
            .await
    }

    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        match self
            .inner
            .copy_between(
                target_label,
                op_label,
                from_namespace,
                from,
                to_namespace,
                to,
            )
            .await
        {
            Ok(()) => Ok(true),
            Err(error) if error.is::<BlobMissingError>() => Ok(false),
            Err(error) => Err(error),
        }
    }
}

/// An agent on an executor whose only archive is the blob archive of `fault`.
struct ArchiveFaultAgent {
    context: TestContext,
    fault: Arc<ArchiveFault>,
    executor: TestWorkerExecutor,
    component: ComponentDto,
    agent: ParsedAgentKey,
    agent_id: OwnedAgentId,
}

impl ArchiveFaultAgent {
    async fn start(
        last_unique_id: &LastUniqueId,
        deps: &WorkerExecutorTestDependencies,
        host_api_tests: &PrecompiledComponent,
        name: &str,
        result_cache_capacity: Option<usize>,
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let fault = Arc::new(ArchiveFault::default());
        let wrapper_fault = fault.clone();
        let executor = start_with_overrides(
            deps,
            &context,
            TestExecutorOverrides {
                configure: Some(Arc::new(move |config| {
                    config.oplog = blob_archiving_config();
                    if let Some(capacity) = result_cache_capacity {
                        config.invocation_results.hydrated_cache_capacity = capacity;
                    }
                })),
                wrap_blob_storage: Some(Arc::new(move |inner| {
                    Arc::new(FlakyArchiveStorage {
                        inner,
                        fault: wrapper_fault.clone(),
                    })
                })),
                ..Default::default()
            },
        )
        .await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let agent = agent_key!("GolemHostApi", name);
        let agent_id = executor.start_agent(&component.id, agent.clone()).await?;
        fault
            .agent_path
            .set(agent_path_segment(&agent_id.agent_id))
            .unwrap();
        let this = Self {
            context,
            fault,
            executor,
            component,
            agent,
            agent_id,
        };
        this.invoke().await?;
        Ok(this)
    }

    async fn invoke(&self) -> anyhow::Result<AgentResult> {
        self.executor
            .invoke_and_await_agent(
                &self.component,
                &self.agent,
                "get_self_metadata_result",
                data_value!(),
            )
            .await
    }

    /// Runs an invocation whose successful commit acknowledges its entries early, and then has to
    /// rebuild the status from archived history that does not read in `mode`.
    ///
    /// The invocation is held at the end of its only host call. Meanwhile the buffered entries
    /// are committed past the status task, so the next receipt of the status task has a gap, and
    /// the entries move to the archive. Then `while_held` runs, the archived history fails, and
    /// the invocation is released. Returns the result of that invocation and the last oplog index
    /// of the failed history.
    async fn succeed_over_a_failing_rebuild(
        self: &Arc<Self>,
        deps: &WorkerExecutorTestDependencies,
        mode: u8,
        while_held: impl AsyncFnOnce(&Self) -> anyhow::Result<()>,
    ) -> anyhow::Result<(anyhow::Result<AgentResult>, OplogIndex)> {
        let owned_agent_id = self.agent_id.clone();
        let (entered, release) = self.executor.gate_next_oplog_function_end(
            &self.agent_id.agent_id,
            HostFunctionName::GolemApiGetSelfMetadata,
            false,
        );
        let invocation = {
            let this = self.clone();
            tokio::spawn(async move { this.invoke().await })
        };
        entered
            .await
            .map_err(|_| anyhow::anyhow!("the host call must reach its End"))?;
        while_held(self.as_ref()).await?;
        self.executor.commit_oplog(&self.agent_id.agent_id).await?;
        wait_until_archived(deps, &self.context, &owned_agent_id).await?;
        let before_failure = self.executor.oplog_max_index(&self.agent_id).await?;
        self.fault.fail_history(mode, before_failure);
        release
            .send(())
            .map_err(|_| anyhow::anyhow!("the End must still wait"))?;
        Ok((invocation.await?, before_failure))
    }

    /// Checks the oplog after `index`: the preceding invocation ends its held host call, finishes
    /// and settles, and then the one `Recovery` error records that the history of the agent does
    /// not decode. The oplog has no other entry there: no start, cancellation or terminal of
    /// another invocation, and no second error.
    async fn assert_prior_invocation_completed_and_the_agent_failed(
        &self,
        index: OplogIndex,
    ) -> anyhow::Result<()> {
        let oplog = self
            .executor
            .get_oplog(&self.agent_id, index.next())
            .await?;
        let agent = self.agent_id.agent_id.to_string();
        // The held host call writes its `End` after the release, so the `End` comes first.
        assert!(
            matches!(
                oplog.as_slice(),
                [end, finished, settled, failure]
                    if matches!(&end.entry, PublicOplogEntry::End(end) if end.start_index <= index)
                        && matches!(finished.entry, PublicOplogEntry::AgentInvocationFinished(_))
                        && matches!(settled.entry, PublicOplogEntry::AgentInvocationSettled(_))
                        && matches!(
                            &failure.entry,
                            PublicOplogEntry::Error(failure)
                                if failure.kind == OplogErrorKind::Recovery
                                    && failure.error.contains(&format!("The oplog of {agent} failed"))
                                    && failure.error.contains("failed to decode")
                        )
            ),
            "only the preceding invocation may end its host call, finish and settle, and only the Recovery error of {agent} may follow: {oplog:#?}"
        );
        Ok(())
    }
}

/// A commit acknowledges its entries before it rebuilds the status, and the caller that took the
/// acknowledgement no longer waits for the rebuild. When the rebuild cannot read the archive, the
/// owner of the agent still finds out. It chooses no work from the detached status and recovers
/// when the archive answers again. The invocation that committed stays successful.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn an_unavailable_archive_after_an_early_commit_receipt_recovers_the_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let agent = Arc::new(
        ArchiveFaultAgent::start(
            last_unique_id,
            deps,
            host_api_tests,
            "early-receipt-unavailable-archive",
            None,
        )
        .await?,
    );

    let (result, _) = agent
        .succeed_over_a_failing_rebuild(deps, ARCHIVE_UNAVAILABLE, async |_| Ok(()))
        .await?;
    result.expect("an invocation whose entries were committed must stay successful");

    // The owner tries to read the history without any new request reaching the agent.
    tokio::time::timeout(
        Duration::from_secs(60),
        agent.fault.unavailable_read.notified(),
    )
    .await?;

    agent.fault.restore();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let metadata = agent.executor.get_worker_metadata(&agent.agent_id).await?;
        if metadata.status == AgentStatus::Idle {
            break;
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the agent did not recover after the archive answered again: {:?}",
            metadata.status
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    agent.invoke().await?;
    Ok(())
}

/// As [`an_unavailable_archive_after_an_early_commit_receipt_recovers_the_agent`], but the
/// archived history does not decode, so the read fails for good. The owner stops the agent with
/// the cause, and the committed invocation stays successful. Metadata reports the unreadable
/// history, then the failed agent once reads recover, without starting the agent again. The oplog
/// gets the `Recovery` error of the agent and no other outcome.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_corrupt_archive_after_an_early_commit_receipt_fails_the_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for mode in [ARCHIVE_CORRUPT, ARCHIVE_UNKNOWN_VERSION] {
        let agent = Arc::new(
            ArchiveFaultAgent::start(
                last_unique_id,
                deps,
                host_api_tests,
                "early-receipt-corrupt-archive",
                None,
            )
            .await?,
        );
        let owned_agent_id = agent.agent_id.clone();
        let worker = agent
            .executor
            .active_agents()
            .try_get(&owned_agent_id)
            .await
            .expect("the agent is loaded after its first invocation");

        let (result, before_failure) = agent
            .succeed_over_a_failing_rebuild(deps, mode, async |_| Ok(()))
            .await?;
        result.expect("an invocation whose entries were committed must stay successful");

        let error = agent
            .invoke()
            .await
            .expect_err("an agent whose status does not rebuild must not run");
        let error = format!("{error:#}");
        assert!(
            error.contains(&agent.agent_id.agent_id.to_string())
                && error.contains("failed to decode"),
            "the failure does not name the agent and the decode error: {error}"
        );
        if mode == ARCHIVE_UNKNOWN_VERSION {
            assert!(
                error.contains("unknown serialization version: 110"),
                "{error}"
            );
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while worker.is_loaded().await {
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "the agent whose status does not rebuild must stop"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let error = agent
            .executor
            .get_worker_metadata(&agent.agent_id)
            .await
            .expect_err("metadata must not return a detached status while history is corrupt");
        let error = format!("{error:#}");
        assert!(
            error.contains(&agent.agent_id.agent_id.to_string())
                && error.contains("failed to decode"),
            "the metadata failure does not name the agent and the decode error: {error}"
        );
        assert!(!worker.is_loaded().await);

        agent.fault.restore();
        assert_eq!(
            agent
                .executor
                .get_worker_metadata(&agent.agent_id)
                .await?
                .status,
            AgentStatus::Failed
        );
        assert!(!worker.is_loaded().await);
        agent
            .assert_prior_invocation_completed_and_the_agent_failed(before_failure)
            .await?;

        let healthy = agent_key!("GolemHostApi", "healthy-next-to-corrupt-archive");
        agent
            .executor
            .start_agent(&agent.component.id, healthy.clone())
            .await?;
        let healthy_result = agent
            .executor
            .invoke_and_await_agent(
                &agent.component,
                &healthy,
                "get_self_metadata_result",
                data_value!(),
            )
            .await?
            .into_typed::<Result<String, String>>()?;
        let healthy_id = agent
            .executor
            .keyed_agent_id(agent.context.default_environment_id, &healthy)?;
        assert_eq!(healthy_result, Ok(healthy_id.agent_id.to_string()));
    }
    Ok(())
}

/// As [`a_corrupt_archive_after_an_early_commit_receipt_fails_the_agent`], with a result cache of
/// one entry and a second invocation queued behind the first. The stop tells the queued waiter
/// about the read failure. The detached status still names the committed invocation as current,
/// but the stop must not publish a second completion for it, and its result reads as the success
/// once the archive answers again.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_failed_rebuild_does_not_fail_a_committed_invocation_whose_result_left_the_cache(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let agent = Arc::new(
        ArchiveFaultAgent::start(
            last_unique_id,
            deps,
            host_api_tests,
            "early-receipt-evicted-result",
            Some(1),
        )
        .await?,
    );
    let owned_agent_id = agent.agent_id.clone();
    let worker = agent
        .executor
        .active_agents()
        .try_get(&owned_agent_id)
        .await
        .expect("the agent is loaded after its first invocation");
    let mut events = worker.events().subscribe();

    let (result, before_failure) = agent
        .succeed_over_a_failing_rebuild(deps, ARCHIVE_CORRUPT, async |agent| {
            agent
                .executor
                .invoke_agent(
                    &agent.component,
                    &agent.agent,
                    "get_self_metadata_result",
                    data_value!(),
                )
                .await?;
            // The enqueue folds every entry before it, so the receipt gap needs one more entry
            // committed past the status task.
            agent
                .executor
                .commit_oplog_entry_bypassing_worker_status(
                    &agent.agent_id.agent_id,
                    OplogEntry::grow_memory(0),
                )
                .await
                .map(|_| ())
        })
        .await?;
    result.expect("an invocation whose entries were committed must stay successful");
    let (committed, succeeded) = events
        .wait_for(|event| match event {
            Event::InvocationCompleted {
                agent_id,
                idempotency_key,
                result,
            } if agent_id == &agent.agent_id.agent_id => Some((*idempotency_key, result.is_ok())),
            _ => None,
        })
        .await?;
    assert!(
        succeeded,
        "the first completion must be the committed success"
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while agent.executor.worker_is_loaded(&owned_agent_id).await {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the agent whose status does not rebuild must stop"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let republished = tokio::time::timeout(
        Duration::from_millis(500),
        events.wait_for(|event| match event {
            Event::InvocationCompleted {
                agent_id,
                idempotency_key,
                result,
            } if agent_id == &agent.agent_id.agent_id && idempotency_key == &committed => {
                Some(result.clone())
            }
            _ => None,
        }),
    )
    .await;
    assert!(
        republished.is_err(),
        "the stop published a second completion for the committed invocation: {republished:?}"
    );

    // The read failure fails the agent. The committed invocation finished, and the queued one
    // did not start.
    agent.fault.restore();
    assert_eq!(
        agent
            .executor
            .get_worker_metadata(&agent.agent_id)
            .await?
            .status,
        AgentStatus::Failed
    );
    agent
        .assert_prior_invocation_completed_and_the_agent_failed(before_failure)
        .await?;
    let lookup = worker.lookup_invocation_result(&committed).await?;
    assert!(
        matches!(lookup, LookupResult::Complete(Ok(_))),
        "the committed invocation must read as its success: {lookup:?}"
    );
    Ok(())
}

/// The published status knows the queued invocation, but its pending payload does not decode, so
/// the read fails for good. The agent fails and tells the waiter of the queued invocation, without
/// starting or settling it: the oplog gets the `Recovery` error of the agent and no other outcome,
/// and a later lookup of the queued key returns the error of the waiter.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_queued_invocation_whose_entry_does_not_decode_fails_the_agent(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let agent = Arc::new(
        ArchiveFaultAgent::start(
            last_unique_id,
            deps,
            host_api_tests,
            "undecodable-queued-invocation",
            None,
        )
        .await?,
    );
    let owned_agent_id = agent.agent_id.clone();
    let worker = agent
        .executor
        .active_agents()
        .try_get(&owned_agent_id)
        .await
        .expect("the agent is loaded after its first invocation");
    let mut events = worker.events().subscribe();
    let healthy = agent_key!("GolemHostApi", "healthy-next-to-unreadable-pickup");
    let healthy_id = agent
        .executor
        .start_agent(&agent.component.id, healthy.clone())
        .await?;
    let queued = IdempotencyKey::fresh();
    let stored_queued = agent
        .executor
        .external_invocation_key(
            &agent.agent_id.agent_id,
            &queued,
            "get_self_metadata_result",
        )
        .await?;

    // The enqueue folds every entry before it, so the status of the first invocation does not
    // need the history.
    let (result, before_failure) = agent
        .succeed_over_a_failing_rebuild(deps, ARCHIVE_CORRUPT, async |agent| {
            agent
                .executor
                .invoke_agent_with_key(
                    &agent.component,
                    &agent.agent,
                    &queued,
                    "get_self_metadata_result",
                    data_value!(),
                )
                .await
        })
        .await?;
    result.expect("the invocation before the queued one does not read the history");

    let queued_result = tokio::time::timeout(
        Duration::from_secs(60),
        events.wait_for(|event| match event {
            Event::InvocationCompleted {
                agent_id,
                idempotency_key,
                result,
            } if agent_id == &agent.agent_id.agent_id && idempotency_key == &stored_queued => {
                Some(result.clone())
            }
            _ => None,
        }),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the queued invocation must fail, not stay pending"))??;
    let error = queued_result.expect_err("a queued invocation that does not hydrate must not run");
    let error = error.to_string();
    assert!(
        error.contains("failed to decode"),
        "the queued invocation must fail with the decode error: {error}"
    );

    assert!(!worker.is_loaded().await);
    assert_eq!(
        agent
            .executor
            .invoke_and_await_agent(
                &agent.component,
                &healthy,
                "get_self_metadata_result",
                data_value!(),
            )
            .await?
            .into_typed::<Result<String, String>>()?,
        Ok(healthy_id.agent_id.to_string())
    );

    agent.fault.restore();
    agent
        .assert_prior_invocation_completed_and_the_agent_failed(before_failure)
        .await?;
    assert_eq!(
        agent
            .executor
            .get_worker_metadata(&agent.agent_id)
            .await?
            .status,
        AgentStatus::Failed
    );
    // The stop of the failed agent answers a later lookup of the queued key on this executor with
    // the error that its waiter received. The oplog has no outcome of the queued invocation.
    let lookup = worker.lookup_invocation_result(&stored_queued).await?;
    assert!(
        matches!(
            &lookup,
            LookupResult::Complete(Err(error)) if error.to_string().contains("failed to decode")
        ),
        "a later lookup must return the decode error of the waiter: {lookup:?}"
    );
    Ok(())
}

/// An agent whose whole history is archived is still the agent its key and its ID name: a keyed
/// invocation joins it instead of creating another agent, and an invocation by ID reaches it.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn archived_agents_are_reached_by_key_and_by_id(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] agent_counters: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_counters)
        .store()
        .await?;
    let by_key = agent_key!("Counter", "archived-by-key");
    let by_id = agent_key!("Counter", "archived-by-id");
    let by_key_id = executor.start_agent(&component.id, by_key.clone()).await?;
    let by_id_id = executor.start_agent(&component.id, by_id.clone()).await?;
    for (agent, increments) in [(&by_key, 3), (&by_id, 2)] {
        for _ in 0..increments {
            executor
                .invoke_and_await_agent(&component, agent, "increment", data_value!())
                .await?;
        }
    }
    wait_until_archived(deps, &context, &by_key_id).await?;
    wait_until_archived(deps, &context, &by_id_id).await?;
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);

    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &by_key, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        4
    );
    assert_eq!(
        executor
            .invoke_and_await_agent_by_id(&component, &by_id_id, None, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        3
    );
    let (cursor, agents) = executor
        .get_workers_metadata(
            &component.environment_id,
            &AgentTypeName("Counter".to_string()),
            None,
            ScanCursor::default(),
            100,
            true,
        )
        .await?;
    assert!(cursor.is_none());
    assert_eq!(
        agents
            .into_iter()
            .map(|agent| agent.agent_id)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([by_key_id.agent_id, by_id_id.agent_id])
    );
    executor.shutdown_and_wait_for_invocation_loops().await?;
    Ok(())
}
