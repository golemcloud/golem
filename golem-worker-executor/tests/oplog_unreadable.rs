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

use crate::Tracing;

use bytes::Bytes;
use futures::stream::BoxStream;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{OplogEntry, OplogIndex, PublicOplogEntry};
use golem_common::model::{AgentId, AgentStatus, IdempotencyKey, OwnedAgentId};
use golem_common::{agent_id, data_value};
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
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, agent_oplog_length, archived_oplog_chunks,
    start_with_oplog_config, start_with_overrides, truncate_archived_oplog_chunk,
};
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
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
inherit_test_dep!(Tracing);

/// The compressed archive level the agents' oplogs move to; with no blob level they stay there.
const ARCHIVE_LEVEL: usize = 1;

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

/// Asserts that the invocation failed with the cause of the unreadable oplog: the agent, the
/// archive layer and the decode error.
fn assert_fails_with_the_decode_error<T: std::fmt::Debug>(
    result: anyhow::Result<T>,
    agent_id: &AgentId,
) {
    let error = result.expect_err("an agent whose oplog does not read must not run");
    let error = format!("{error:#}");
    assert!(
        error.contains(&agent_id.to_string())
            && error.contains("archive layer 1")
            && error.contains("failed to decode compressed oplog chunk")
            && error.contains("Input ended unexpectedly"),
        "the failure does not name the agent, the archive layer and the decode error: {error}"
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

    let unreadable = agent_id!("Counter", "unreadable-archive");
    let healthy = agent_id!("Counter", "healthy-archive");
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
    let unreadable_owned = OwnedAgentId::new(context.default_environment_id, &unreadable_id);
    let healthy_owned = OwnedAgentId::new(context.default_environment_id, &healthy_id);
    wait_until_archived(deps, &context, &unreadable_owned).await?;
    wait_until_archived(deps, &context, &healthy_owned).await?;
    drop(executor);

    truncate_archived_oplog_chunk(deps, &context, &unreadable_owned, ARCHIVE_LEVEL, 0).await?;
    let corrupt = archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?;

    // The executor starts on the data, and recovers every agent it can read.
    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;

    assert_fails_with_the_decode_error(
        executor
            .invoke_and_await_agent(&component, &unreadable, "increment", data_value!())
            .await,
        &unreadable_id,
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

    // Nothing deleted or rewrote the unreadable history.
    drop(executor);
    assert_eq!(
        archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?,
        corrupt
    );
    Ok(())
}

/// The `Create` of the agent reads, and a later archived chunk does not: the agent is found, its
/// start replays into the chunk and fails with the cause, and the agent is not run.
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

    let unreadable = agent_id!("Counter", "unreadable-later-chunk");
    let healthy = agent_id!("Counter", "healthy-later-chunk");
    let unreadable_id = executor
        .start_agent(&component.id, unreadable.clone())
        .await?;
    let healthy_id = executor.start_agent(&component.id, healthy.clone()).await?;
    let unreadable_owned = OwnedAgentId::new(context.default_environment_id, &unreadable_id);
    let healthy_owned = OwnedAgentId::new(context.default_environment_id, &healthy_id);
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
    let corrupt = archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?;

    let executor = start_with_oplog_config(deps, &context, Some(archiving_config())).await?;

    assert_fails_with_the_decode_error(
        executor
            .invoke_and_await_agent(&component, &unreadable, "increment", data_value!())
            .await,
        &unreadable_id,
    );

    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &healthy, "increment", data_value!())
            .await?
            .into_typed::<u32>()?,
        5
    );

    drop(executor);
    assert_eq!(
        archived_oplog_chunks(deps, &context, &unreadable_owned, ARCHIVE_LEVEL).await?,
        corrupt
    );
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

const ARCHIVE_UP: u8 = 0;
const ARCHIVE_UNAVAILABLE: u8 = 1;
const ARCHIVE_CORRUPT: u8 = 2;
const ARCHIVE_UNKNOWN_VERSION: u8 = 3;

/// How a [`FlakyArchiveStorage`] answers the reads of archived oplog chunks, and how many error
/// entries it archived since the history failed.
///
/// The fault applies only to the history: the chunks whose last entry is at or before
/// `history_end`. A status rebuild reads that history.
#[derive(Debug, Default)]
struct ArchiveFault {
    agent_path: OnceLock<String>,
    mode: AtomicU8,
    history_end: AtomicU64,
    archived_errors: AtomicUsize,
}

impl ArchiveFault {
    fn fail_history(&self, mode: u8, history_end: OplogIndex) {
        self.history_end
            .store(u64::from(history_end), Ordering::SeqCst);
        self.archived_errors.store(0, Ordering::SeqCst);
        self.mode.store(mode, Ordering::SeqCst);
    }

    fn restore(&self) {
        self.mode.store(ARCHIVE_UP, Ordering::SeqCst);
    }

    /// The error entries archived since the history failed. The archive receives every committed
    /// entry, so this shows a recorded error without a read of the history.
    fn archived_errors(&self) -> usize {
        self.archived_errors.load(Ordering::SeqCst)
    }

    fn observe_write(&self, namespace: &BlobStorageNamespace, data: &[u8]) {
        if !matches!(namespace, BlobStorageNamespace::CompressedOplog { .. }) {
            return;
        }
        let entries = golem_common::serialization::deserialize::<CompressedOplogChunk>(data)
            .expect("the archive stores compressed oplog chunks")
            .decompress()
            .expect("an archived chunk decompresses");
        let errors = entries
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::Error { .. }))
            .count();
        self.archived_errors.fetch_add(errors, Ordering::SeqCst);
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
            ARCHIVE_UNAVAILABLE => Some(Err(anyhow::anyhow!(
                "injected 503: the oplog archive is down"
            ))),
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
        self.fault.observe_write(&namespace, data);
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
        self.fault.observe_write(&namespace, data);
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
    agent: ParsedAgentId,
    agent_id: AgentId,
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
        let agent = agent_id!("GolemHostApi", name);
        let agent_id = executor.start_agent(&component.id, agent.clone()).await?;
        fault.agent_path.set(agent_path_segment(&agent_id)).unwrap();
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
        let owned_agent_id = OwnedAgentId::new(self.context.default_environment_id, &self.agent_id);
        let (entered, release) = self.executor.gate_next_oplog_function_end(
            &self.agent_id,
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
        self.executor.commit_oplog(&self.agent_id).await?;
        wait_until_archived(deps, &self.context, &owned_agent_id).await?;
        let before_failure = self.executor.oplog_max_index(&self.agent_id).await?;
        self.fault.fail_history(mode, before_failure);
        release
            .send(())
            .map_err(|_| anyhow::anyhow!("the End must still wait"))?;
        Ok((invocation.await?, before_failure))
    }

    /// Whether the oplog after `index` records an error.
    async fn records_an_error_after(&self, index: OplogIndex) -> anyhow::Result<bool> {
        Ok(self
            .executor
            .get_oplog(&self.agent_id, index.next())
            .await?
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Error(_))))
    }
}

/// A commit acknowledges its entries before it rebuilds the status, and the caller that took the
/// acknowledgement no longer waits for the rebuild. When the rebuild cannot read the archive, the
/// owner of the agent still finds out: it chooses no work from the detached status, records a
/// recovery failure, and recovers when the archive answers again. The invocation that committed
/// stays successful.
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

    // While the history is down, the owner fails to repair the status and records the failure,
    // without any new request reaching the agent.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while agent.fault.archived_errors() == 0 {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the owner must record the failed rebuild as a recovery failure"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

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
/// archived history does not decode. The owner stops the agent for good with the cause: the
/// failure is recorded, a later invocation fails with the cause without running, and the
/// committed invocation stays successful. Metadata reports the unreadable history, then the
/// terminal failure once reads recover, without starting the agent again.
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
        let owned_agent_id =
            OwnedAgentId::new(agent.context.default_environment_id, &agent.agent_id);
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
            error.contains(&agent.agent_id.to_string()) && error.contains("failed to decode"),
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
            error.contains(&agent.agent_id.component_id.to_string())
                && error.contains("early-receipt-corrupt-archive")
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
        assert!(
            agent.records_an_error_after(before_failure).await?,
            "the owner must record the failed rebuild"
        );
        let oplog = agent
            .executor
            .get_oplog(&agent.agent_id, before_failure.next())
            .await?;
        assert!(
            !oplog.iter().any(|entry| matches!(
                entry.entry,
                PublicOplogEntry::AgentInvocationStarted(_)
                    | PublicOplogEntry::RecoverySucceeded(_)
            )),
            "metadata observation must not restart the failed agent: {oplog:#?}"
        );

        let healthy = agent_id!("GolemHostApi", "healthy-next-to-corrupt-archive");
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
        assert_eq!(healthy_result, Ok(healthy.to_string()));
    }
    Ok(())
}

/// As [`a_corrupt_archive_after_an_early_commit_receipt_fails_the_agent`], with a result cache of
/// one entry and a second invocation queued behind the first. The stop that fails the agent fails
/// the queued invocation, whose result takes the only cache entry. The detached status still names
/// the committed invocation as current, and the stop must not fail it: no second completion is
/// published for it, and its result reads as the success once the archive answers again.
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
    let owned_agent_id = OwnedAgentId::new(agent.context.default_environment_id, &agent.agent_id);
    let worker = agent
        .executor
        .active_agents()
        .try_get(&owned_agent_id)
        .await
        .expect("the agent is loaded after its first invocation");
    let mut events = worker.events().subscribe();

    let (result, _) = agent
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
                    &agent.agent_id,
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
            } if agent_id == &agent.agent_id => Some((idempotency_key.clone(), result.is_ok())),
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
            } if agent_id == &agent.agent_id && idempotency_key == &committed => {
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

    agent.fault.restore();
    assert_eq!(
        agent
            .executor
            .get_worker_metadata(&agent.agent_id)
            .await?
            .status,
        AgentStatus::Failed
    );
    let lookup = worker.lookup_invocation_result(&committed).await;
    assert!(
        matches!(lookup, LookupResult::Complete(Ok(_))),
        "the committed invocation must read as its success: {lookup:?}"
    );
    Ok(())
}

/// A queued invocation whose pending entry is in archived history that does not decode. The status
/// rebuilds, so only the hydration of the queued invocation reads the history. The owner records
/// the failure and stops the agent with the cause, instead of retrying it without end: the queued
/// invocation fails with the decode error, and no invocation starts after the failure.
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
    let owned_agent_id = OwnedAgentId::new(agent.context.default_environment_id, &agent.agent_id);
    let worker = agent
        .executor
        .active_agents()
        .try_get(&owned_agent_id)
        .await
        .expect("the agent is loaded after its first invocation");
    let mut events = worker.events().subscribe();
    let queued = IdempotencyKey::fresh();

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
            } if agent_id == &agent.agent_id && idempotency_key == &queued => Some(result.clone()),
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

    agent.fault.restore();
    assert!(
        agent.records_an_error_after(before_failure).await?,
        "the owner must record the failed hydration"
    );
    let oplog = agent
        .executor
        .get_oplog(&agent.agent_id, before_failure.next())
        .await?;
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))),
        "no invocation may start after the failed hydration: {oplog:#?}"
    );
    assert_eq!(
        agent
            .executor
            .get_worker_metadata(&agent.agent_id)
            .await?
            .status,
        AgentStatus::Failed
    );
    Ok(())
}
