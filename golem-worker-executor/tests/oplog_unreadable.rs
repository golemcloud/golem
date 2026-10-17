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
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::{AgentId, AgentStatus, OwnedAgentId};
use golem_common::{agent_id, data_value};
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobMissingError, BlobRangeStream, BlobStorage, BlobStorageBackend,
    BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath, PutIfAbsent,
};
use golem_test_framework::dsl::{AgentResult, TestDsl};
use golem_worker_executor::services::golem_config::OplogConfig;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, agent_oplog_length, archived_oplog_chunks,
    start_with_oplog_config, start_with_overrides, truncate_archived_oplog_chunk,
};
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
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

/// How a [`FlakyArchiveStorage`] answers the reads of archived oplog chunks, and how many it
/// refused.
#[derive(Debug, Default)]
struct ArchiveFault {
    mode: AtomicU8,
    refused: AtomicUsize,
}

impl ArchiveFault {
    fn set(&self, mode: u8) {
        self.mode.store(mode, Ordering::SeqCst);
    }

    fn refused(&self) -> usize {
        self.refused.load(Ordering::SeqCst)
    }

    /// The answer to a read in `namespace` that replaces the stored one. An unavailable archive
    /// fails the read as a backend whose outage outlived its retries, so the failure is transient.
    /// A corrupt archive gives bytes of the current serialization version that do not decode.
    fn read(&self, namespace: &BlobStorageNamespace) -> Option<anyhow::Result<Option<Vec<u8>>>> {
        if !matches!(namespace, BlobStorageNamespace::CompressedOplog { .. }) {
            return None;
        }
        match self.mode.load(Ordering::SeqCst) {
            ARCHIVE_UNAVAILABLE => {
                self.refused.fetch_add(1, Ordering::SeqCst);
                Some(Err(anyhow::anyhow!(
                    "injected 503: the oplog archive is down"
                )))
            }
            ARCHIVE_CORRUPT => {
                self.refused.fetch_add(1, Ordering::SeqCst);
                let mut bytes = vec![golem_common::serialization::SERIALIZATION_VERSION_V3];
                bytes.extend_from_slice(b"not a compressed oplog chunk");
                Some(Ok(Some(bytes)))
            }
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
        if let Some(answer) = self.fault.read(&namespace) {
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
    ) -> anyhow::Result<Self> {
        let context = TestContext::new(last_unique_id);
        let fault = Arc::new(ArchiveFault::default());
        let wrapper_fault = fault.clone();
        let executor = start_with_overrides(
            deps,
            &context,
            TestExecutorOverrides {
                configure: Some(Arc::new(|config| config.oplog = blob_archiving_config())),
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
    /// the entries move to the archive. The archive fails, and the invocation is released.
    /// Returns the result of that invocation and the last oplog index before the archive failed.
    async fn succeed_over_a_failing_rebuild(
        self: &Arc<Self>,
        deps: &WorkerExecutorTestDependencies,
        mode: u8,
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
        self.executor.commit_oplog(&self.agent_id).await?;
        wait_until_archived(deps, &self.context, &owned_agent_id).await?;
        let before_failure = self.executor.oplog_max_index(&self.agent_id).await?;
        self.fault.set(mode);
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
        )
        .await?,
    );

    let (result, before_failure) = agent
        .succeed_over_a_failing_rebuild(deps, ARCHIVE_UNAVAILABLE)
        .await?;
    result.expect("an invocation whose entries were committed must stay successful");

    // The owner tries to repair the status again while the archive is down, without any new
    // request reaching the agent.
    let refused = agent.fault.refused();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while agent.fault.refused() <= refused {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "the owner did not try to repair the detached status"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    agent.fault.set(ARCHIVE_UP);
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
    assert!(
        agent.records_an_error_after(before_failure).await?,
        "the owner must record the failed rebuild as a recovery failure"
    );
    agent.invoke().await?;
    Ok(())
}

/// As [`an_unavailable_archive_after_an_early_commit_receipt_recovers_the_agent`], but the
/// archived history does not decode. The owner stops the agent for good with the cause: the
/// failure is recorded, a later invocation fails with the cause without running, and the
/// committed invocation stays successful.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn a_corrupt_archive_after_an_early_commit_receipt_fails_the_agent(
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
            "early-receipt-corrupt-archive",
        )
        .await?,
    );

    let (result, before_failure) = agent
        .succeed_over_a_failing_rebuild(deps, ARCHIVE_CORRUPT)
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

    agent.fault.set(ARCHIVE_UP);
    assert!(
        agent.records_an_error_after(before_failure).await?,
        "the owner must record the failed rebuild"
    );
    let oplog = agent
        .executor
        .get_oplog(&agent.agent_id, before_failure.next())
        .await?;
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))),
        "no invocation may start after the failed rebuild: {oplog:#?}"
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
