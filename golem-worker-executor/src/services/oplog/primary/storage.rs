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

use super::*;

/// Runs a storage operation under the retry policy, panicking on anything it cannot retry away.
///
/// Deletions and prefix drops keep this shape: a permanent failure there is a broken
/// deployment, and failing fast is the long-standing contract.
pub(super) async fn retry_storage_op<T, F, Fut>(
    retry_config: &RetryConfig,
    op_name: &str,
    key: &str,
    op: F,
) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    retry_storage_op_result(retry_config, op_name, key, op)
        .await
        .unwrap_or_else(|error| panic!("{error}"))
}

pub(super) async fn retry_storage_op_result<T, F, Fut>(
    retry_config: &RetryConfig,
    op_name: &str,
    key: &str,
    mut op: F,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match op().await {
            Ok(val) => return Ok(val),
            Err(IndexedStorageError::Transient(msg)) => {
                if let Some(delay) = get_delay(retry_config, attempts) {
                    record_oplog_storage_retry(op_name);
                    warn!(
                        op = op_name,
                        key = key,
                        attempt = attempts,
                        delay_ms = delay.as_millis() as u64,
                        "Transient indexed storage error, retrying: {msg}"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    return Err(format!(
                        "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts: Transient storage error: {msg}"
                    ));
                }
            }
            Err(err) => {
                return Err(format!(
                    "Indexed storage operation '{op_name}' failed for key '{key}': {err}"
                ));
            }
        }
    }
}

/// Retries transient failures of an oplog maintenance operation and returns anything else, a
/// fence included, for the caller to classify.
pub(super) async fn retry_maintenance_storage_op<T, F, Fut>(
    retry_config: &RetryConfig,
    op_name: &str,
    key: &str,
    mut op: F,
) -> Result<T, IndexedStorageError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match op().await {
            Ok(val) => return Ok(val),
            Err(error @ IndexedStorageError::Transient(_)) => {
                if let Some(delay) = get_delay(retry_config, attempts) {
                    record_oplog_storage_retry(op_name);
                    warn!(
                        op = op_name,
                        key = key,
                        attempt = attempts,
                        delay_ms = delay.as_millis() as u64,
                        "Transient indexed storage error during oplog maintenance, retrying: {error}"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn stored_batch_matches(mut actual: Vec<(u64, Vec<u8>)>, expected: &[(u64, Bytes)]) -> bool {
    actual.sort_unstable_by_key(|(id, _)| *id);
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(
            |((actual_id, actual_value), (expected_id, expected_value))| {
                actual_id == expected_id && actual_value.as_slice() == expected_value.as_ref()
            },
        )
}

/// Reads back the indices of an append whose outcome is unknown. `Ok(None)` means storage holds
/// none of them; a failed read is an error, never an absent batch.
async fn stored_oplog_batch_matches(
    retry_config: &RetryConfig,
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    namespace: &IndexedStorageNamespace,
    key: &str,
    expected: &[(u64, Bytes)],
) -> Result<Option<bool>, IndexedStorageError> {
    let first_id = expected.first().expect("non-empty oplog batch").0;
    let last_id = expected.last().expect("non-empty oplog batch").0;
    retry_maintenance_storage_op(retry_config, "append_reconcile", key, || {
        let namespace = namespace.clone();
        async move {
            indexed_storage
                .with_entity("oplog", "append_reconcile", "entry")
                .read_raw(namespace, key, first_id, last_id)
                .await
                .map(|actual| {
                    if actual.is_empty() {
                        None
                    } else {
                        Some(stored_batch_matches(actual, expected))
                    }
                })
        }
    })
    .await
}

impl SerializedOplogAppend {
    fn entries(&self) -> &[(u64, Bytes)] {
        match self {
            Self::Entry(entry) => std::slice::from_ref(entry),
            Self::Batch(entries) => entries,
        }
    }

    async fn write(
        &self,
        indexed_storage: &(dyn IndexedStorage + Send + Sync),
        namespace: &IndexedStorageNamespace,
        api_name: &'static str,
        key: &str,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<(), IndexedStorageError> {
        let storage = indexed_storage.with_entity("oplog", api_name, "entry");
        match self {
            Self::Entry((id, value)) => {
                storage
                    .append_raw(namespace.clone(), key, *id, value.to_vec(), shard_epoch)
                    .await
            }
            Self::Batch(entries) => {
                storage
                    .append_many_raw(namespace, key, entries.clone(), shard_epoch)
                    .await
            }
        }
    }
}

/// The refusal of a write of the oplog of `agent_id` whose asserted epoch is `expected`, while the
/// storage holds `actual`.
fn fenced(agent_id: &AgentId, expected: ShardEpoch, actual: Option<ShardEpoch>) -> OplogError {
    OplogError::Fenced(OplogFence {
        agent_id: *agent_id,
        expected_epoch: expected,
        actual_epoch: actual,
    })
}

/// The failure of an append of the oplog of `agent_id` whose outcome is unknown, because the read
/// back of its entries failed with `read_error`. The entries can be stored or absent, so the agent
/// fails: for good, or until a later read can pass when the storage did not answer.
fn unconfirmed_append(
    agent_id: &AgentId,
    key: &str,
    entries: &[(u64, Bytes)],
    read_error: IndexedStorageError,
) -> OplogError {
    let first = entries.first().map_or(0, |(id, _)| *id);
    let last = entries.last().map_or(0, |(id, _)| *id);
    let detail = format!(
        "could not read back the unconfirmed append of oplog entries {first}..={last} for key '{key}': {read_error}"
    );
    let error = match read_error {
        IndexedStorageError::Transient(_) => {
            OplogReadError::source_unavailable(OplogReadSource::Primary, detail)
        }
        _ => OplogReadError::source_failure(OplogReadSource::Primary, detail),
    };
    OplogError::Unconfirmed(UnreadableOplog::new(agent_id, error))
}

/// What the attempts of an append have shown about its batch.
#[derive(Debug)]
enum AppendPhase {
    /// No attempt can have stored the batch.
    Unwritten,
    /// An attempt may have stored the batch. The last read back, if any, found none of it.
    MayHaveCommitted,
    /// An attempt may have stored the batch, and the last read back failed with this error.
    Unknown(IndexedStorageError),
}

/// What an attempt of an append observed.
enum AppendEvent {
    /// The write failed in a way that can hide a stored batch.
    WriteIndeterminate,
    /// The read back found none of the batch.
    ReadBackAbsent,
    /// The read back failed.
    ReadBackFailed(IndexedStorageError),
}

impl AppendPhase {
    /// The one transition of the append phase. A batch that may have been stored stays so; the
    /// last read back decides whether its outcome is unknown.
    fn transition(self, event: AppendEvent) -> Self {
        match (self, event) {
            (Self::Unwritten, AppendEvent::WriteIndeterminate) => Self::MayHaveCommitted,
            (phase, AppendEvent::WriteIndeterminate) => phase,
            (_, AppendEvent::ReadBackAbsent) => Self::MayHaveCommitted,
            (_, AppendEvent::ReadBackFailed(error)) => Self::Unknown(error),
        }
    }

    fn may_have_committed(&self) -> bool {
        !matches!(self, Self::Unwritten)
    }
}

/// Writes `append` to the oplog of `agent_id` under the retry policy. A write whose outcome is
/// unknown is read back. A fence gives `OplogError::Fenced`. A read back that fails, after its own
/// retries and the retries of the write, gives `OplogError::Unconfirmed`: the entries are neither
/// confirmed nor taken as absent. Stored entries that differ from the append, and write failures
/// that no read explains, stop the executor.
#[allow(clippy::too_many_arguments)]
pub(super) async fn retry_oplog_append(
    retry_config: &RetryConfig,
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    namespace: &IndexedStorageNamespace,
    agent_id: &AgentId,
    op_name: &str,
    api_name: &'static str,
    key: &str,
    append: SerializedOplogAppend,
    shard_epoch: Option<ShardEpoch>,
) -> Result<(), OplogError> {
    let mut attempts = 0u32;
    let mut phase = AppendPhase::Unwritten;
    loop {
        attempts += 1;
        let error = match append
            .write(indexed_storage, namespace, api_name, key, shard_epoch)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };

        let retryable = match &error {
            // The storage refused the write because the shard has a new owner. Not a transient
            // failure to retry and not an indeterminate one to reconcile: it is a deliberate
            // refusal, so hand it back and let the caller give up this one agent instead of
            // aborting.
            IndexedStorageError::Fenced {
                expected, actual, ..
            } => return Err(fenced(agent_id, *expected, *actual)),
            IndexedStorageError::Indeterminate(_) => {
                phase = phase.transition(AppendEvent::WriteIndeterminate);
                true
            }
            IndexedStorageError::Transient(_) => true,
            IndexedStorageError::Conflict(msg) => {
                if !phase.may_have_committed() {
                    panic!(
                        "Indexed storage operation '{op_name}' conflicted for key '{key}' without a preceding indeterminate write; possible concurrent oplog writer: {msg}"
                    );
                }
                false
            }
            IndexedStorageError::InvalidResume(_) | IndexedStorageError::Other(_) => {
                if !phase.may_have_committed() {
                    panic!("Indexed storage operation '{op_name}' failed for key '{key}': {error}");
                }
                false
            }
        };

        if phase.may_have_committed() {
            match stored_oplog_batch_matches(
                retry_config,
                indexed_storage,
                namespace,
                key,
                append.entries(),
            )
            .await
            {
                Ok(Some(true)) => return Ok(()),
                Ok(Some(false)) => {
                    // The stored content differs from what this attempt sent - the only
                    // legitimate way that happens is a new owner having already written those
                    // same indices. Repeat the write once as a probe: the backends check the
                    // epoch inside the same transaction as the insert, so a shard that has
                    // moved on is fenced before the insert is even attempted. A same-epoch
                    // conflict instead fails the probe's insert (still fatal, below) - the
                    // mismatch is unexplained and not safe to paper over.
                    if let Some(epoch) = shard_epoch
                        && let Err(IndexedStorageError::Fenced {
                            expected, actual, ..
                        }) = append
                            .write(indexed_storage, namespace, api_name, key, Some(epoch))
                            .await
                    {
                        return Err(fenced(agent_id, expected, actual));
                    }
                    panic!(
                        "Indexed storage operation '{op_name}' failed for key '{key}' and the indeterminate write did not match storage: {error}"
                    )
                }
                Ok(None) => phase = phase.transition(AppendEvent::ReadBackAbsent),
                // The outcome stays unknown. A repeated write that conflicts reconciles again.
                Err(error) => {
                    warn!(
                        op = op_name,
                        key = key,
                        error = %error,
                        "Could not read back an indeterminate oplog write"
                    );
                    phase = phase.transition(AppendEvent::ReadBackFailed(error));
                }
            }
        }

        if retryable && let Some(delay) = get_delay(retry_config, attempts) {
            record_oplog_storage_retry(op_name);
            warn!(
                op = op_name,
                key = key,
                attempt = attempts,
                delay_ms = delay.as_millis() as u64,
                error = %error,
                "Retryable indexed storage write failed, retrying"
            );
            tokio::time::sleep(delay).await;
            continue;
        }

        match phase {
            AppendPhase::Unknown(reconcile_error) => {
                warn!(
                    op = op_name,
                    key = key,
                    attempt = attempts,
                    error = %error,
                    read_error = %reconcile_error,
                    "The outcome of an oplog write stays unknown; the agent fails"
                );
                return Err(unconfirmed_append(
                    agent_id,
                    key,
                    append.entries(),
                    reconcile_error,
                ));
            }
            AppendPhase::MayHaveCommitted => panic!(
                "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts and the indeterminate write did not match storage: {error}"
            ),
            AppendPhase::Unwritten => panic!(
                "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts: {error}"
            ),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::services::oplog::tests::{
        default_execution_status, default_last_known_status, make_agent_metadata,
    };
    use crate::storage::indexed::ScanResume;
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use futures::StreamExt;
    use golem_common::model::regions::OplogRegion;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::sync::Mutex as StdMutex;
    use test_r::test;

    /// An indexed storage that, once `unconfirm` is set, reports the outcome of each append as
    /// unknown, and fails every read with the error that `unconfirm` gives. It stores the append
    /// unless `drop_appends` is set.
    #[derive(Debug, Default)]
    pub(crate) struct UnconfirmedAppends {
        inner: InMemoryIndexedStorage,
        read_error: StdMutex<Option<IndexedStorageError>>,
        next_read_error: StdMutex<Option<IndexedStorageError>>,
        drop_appends: std::sync::atomic::AtomicBool,
    }

    impl UnconfirmedAppends {
        pub(crate) fn unconfirm(&self, read_error: IndexedStorageError) {
            *self.read_error.lock().unwrap() = Some(read_error);
        }

        /// From now on an append with an unknown outcome stores nothing.
        pub(crate) fn drop_appends(&self) {
            self.drop_appends
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        /// Fails the next read with `error`, and changes no append.
        pub(crate) fn fail_next_read(&self, error: IndexedStorageError) {
            *self.next_read_error.lock().unwrap() = Some(error);
        }

        fn lands(&self) -> bool {
            self.read_error().is_none()
                || !self.drop_appends.load(std::sync::atomic::Ordering::SeqCst)
        }

        pub(crate) fn confirm(&self) {
            *self.read_error.lock().unwrap() = None;
        }

        fn read_error(&self) -> Option<IndexedStorageError> {
            self.read_error.lock().unwrap().clone()
        }

        fn unknown_outcome(&self) -> Result<(), IndexedStorageError> {
            match self.read_error() {
                Some(_) => Err(IndexedStorageError::Indeterminate(
                    "injected connection loss after the write".to_string(),
                )),
                None => Ok(()),
            }
        }
    }

    #[async_trait]
    impl IndexedStorage for UnconfirmedAppends {
        async fn number_of_replicas(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
        ) -> Result<u8, IndexedStorageError> {
            self.inner.number_of_replicas(svc_name, api_name).await
        }

        async fn wait_for_replicas(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            replicas: u8,
            timeout: Duration,
        ) -> Result<u8, IndexedStorageError> {
            self.inner
                .wait_for_replicas(svc_name, api_name, replicas, timeout)
                .await
        }

        async fn exists(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<bool, IndexedStorageError> {
            self.inner.exists(svc_name, api_name, namespace, key).await
        }

        async fn scan_stable(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageMetaNamespace,
            prefix: Option<&str>,
            resume: Option<ScanResume>,
            count: u64,
        ) -> Result<(Option<ScanResume>, Vec<String>), IndexedStorageError> {
            self.inner
                .scan_stable(svc_name, api_name, namespace, prefix, resume, count)
                .await
        }

        async fn append(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            id: u64,
            value: Vec<u8>,
            expected_epoch: Option<ShardEpoch>,
        ) -> Result<(), IndexedStorageError> {
            if !self.lands() {
                return self.unknown_outcome();
            }
            self.inner
                .append(
                    svc_name,
                    api_name,
                    entity_name,
                    namespace,
                    key,
                    id,
                    value,
                    expected_epoch,
                )
                .await?;
            self.unknown_outcome()
        }

        async fn append_many(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: &IndexedStorageNamespace,
            key: &str,
            pairs: Arc<[(u64, Bytes)]>,
            expected_epoch: Option<ShardEpoch>,
        ) -> Result<(), IndexedStorageError> {
            if !self.lands() {
                return self.unknown_outcome();
            }
            self.inner
                .append_many(
                    svc_name,
                    api_name,
                    entity_name,
                    namespace,
                    key,
                    pairs,
                    expected_epoch,
                )
                .await?;
            self.unknown_outcome()
        }

        async fn length(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<u64, IndexedStorageError> {
            self.inner.length(svc_name, api_name, namespace, key).await
        }

        async fn delete(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<(), IndexedStorageError> {
            self.inner.delete(svc_name, api_name, namespace, key).await
        }

        async fn read(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            start_id: u64,
            end_id: u64,
        ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
            if let Some(error) = self
                .read_error()
                .or_else(|| self.next_read_error.lock().unwrap().take())
            {
                return Err(error);
            }
            self.inner
                .read(
                    svc_name,
                    api_name,
                    entity_name,
                    namespace,
                    key,
                    start_id,
                    end_id,
                )
                .await
        }

        async fn first(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
            self.inner
                .first(svc_name, api_name, entity_name, namespace, key)
                .await
        }

        async fn last(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
            self.inner
                .last(svc_name, api_name, entity_name, namespace, key)
                .await
        }

        async fn last_id(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
        ) -> Result<Option<u64>, IndexedStorageError> {
            self.inner
                .last_id(svc_name, api_name, entity_name, namespace, key)
                .await
        }

        async fn closest(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            entity_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            id: u64,
        ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
            self.inner
                .closest(svc_name, api_name, entity_name, namespace, key, id)
                .await
        }

        async fn drop_prefix(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            last_dropped_id: u64,
            expected_epoch: Option<ShardEpoch>,
        ) -> Result<(), IndexedStorageError> {
            self.inner
                .drop_prefix(
                    svc_name,
                    api_name,
                    namespace,
                    key,
                    last_dropped_id,
                    expected_epoch,
                )
                .await
        }

        async fn delete_empty_with_epoch(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            expected_epoch: Option<ShardEpoch>,
        ) -> Result<bool, IndexedStorageError> {
            self.inner
                .delete_empty_with_epoch(svc_name, api_name, namespace, key, expected_epoch)
                .await
        }

        async fn set_key_epoch(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            epoch: ShardEpoch,
        ) -> Result<(), IndexedStorageError> {
            self.inner
                .set_key_epoch(svc_name, api_name, namespace, key, epoch)
                .await
        }

        async fn delete_with_epoch(
            &self,
            svc_name: &'static str,
            api_name: &'static str,
            namespace: IndexedStorageNamespace,
            key: &str,
            expected_epoch: Option<ShardEpoch>,
        ) -> Result<(), IndexedStorageError> {
            self.inner
                .delete_with_epoch(svc_name, api_name, namespace, key, expected_epoch)
                .await
        }
    }

    pub(crate) async fn service(storage: Arc<UnconfirmedAppends>) -> PrimaryOplogService {
        PrimaryOplogService::new(
            storage,
            Arc::new(InMemoryBlobStorage::new()),
            100,
            100,
            1024,
            32,
            RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_jitter_factor: None,
            },
        )
        .await
    }

    pub(crate) async fn create(
        service: &PrimaryOplogService,
        agent_id: AgentId,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        create_owned(service, &OwnedAgentId::new(EnvironmentId::new(), &agent_id)).await
    }

    async fn create_owned(
        service: &PrimaryOplogService,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        let agent_id = owned_agent_id.agent_id;
        let account_id = AccountId::new();
        let environment_id = owned_agent_id.environment_id();
        service
            .create_fresh(
                &mut service.lock_lifecycle(&agent_id).await,
                owned_agent_id,
                AgentMode::Durable,
                OplogEntry::jump(
                    None,
                    OplogRegion {
                        start: OplogIndex::NONE,
                        end: OplogIndex::NONE,
                    },
                ),
                make_agent_metadata(agent_id, account_id, environment_id),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await
    }

    pub(crate) async fn add_all(oplog: &Arc<dyn Oplog>, sizes: std::ops::RangeInclusive<u64>) {
        futures::stream::iter(sizes)
            .then(|size| {
                let oplog = oplog.clone();
                async move { oplog.add(OplogEntry::grow_memory(size)).await.unwrap() }
            })
            .collect::<Vec<_>>()
            .await;
    }

    /// The memory deltas of `entries`, which identify the test entries without their timestamps.
    fn deltas(entries: &[OplogEntry]) -> Vec<u64> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                OplogEntry::GrowMemory { delta, .. } => Some(*delta),
                _ => None,
            })
            .collect()
    }

    /// Opens the oplog of `owned_agent_id` again after `previous` was used, and reads its whole
    /// history from the storage.
    async fn history_on_a_fresh_handle(
        service: &PrimaryOplogService,
        owned_agent_id: &OwnedAgentId,
        previous: Arc<dyn Oplog>,
    ) -> Vec<OplogEntry> {
        let agent_id = owned_agent_id.agent_id;
        let last = service
            .get_last_index(owned_agent_id, AgentMode::Durable)
            .await
            .expect("last index");
        let reopened = service
            .open(
                &mut service.lock_lifecycle(&agent_id).await,
                owned_agent_id,
                AgentMode::Durable,
                Some(last),
                make_agent_metadata(agent_id, AccountId::new(), owned_agent_id.environment_id()),
                default_last_known_status(),
                default_execution_status(AgentMode::Durable),
                None,
            )
            .await
            .expect("reopen");
        assert!(
            !Arc::ptr_eq(&reopened, &previous),
            "the open-oplog cache does not hand out the unconfirmed handle"
        );
        assert_eq!(reopened.unconfirmed_append(), None);
        reopened
            .read_exact(OplogIndex::INITIAL, last.as_u64())
            .await
            .expect("fresh history")
            .into_values()
            .collect()
    }

    /// A commit whose append has an unknown outcome, and whose read-back does not answer, fails
    /// the agent with a read that a later attempt can pass. It names the entries of the append.
    /// The handle latches the append and does not give its entries as history.
    #[test]
    async fn an_unconfirmed_append_with_an_unavailable_read_back_fails_the_agent() {
        let storage = Arc::new(UnconfirmedAppends::default());
        let service = service(storage.clone()).await;
        let agent_id = crate::test_agent_id("unconfirmed-append-unavailable");
        let oplog = create(&service, agent_id).await.expect("create");
        add_all(&oplog, 2..=6).await;
        oplog.commit(CommitLevel::Always).await.expect("commit");

        storage.unconfirm(IndexedStorageError::Transient(
            "injected read-back outage".to_string(),
        ));
        let seventh = OplogEntry::grow_memory(7).rounded();
        oplog.add(seventh.clone()).await.unwrap();
        oplog.add(OplogEntry::grow_memory(8)).await.unwrap();
        let error = oplog
            .commit(CommitLevel::Always)
            .await
            .expect_err("an unconfirmed append is not a commit");

        let OplogError::Unconfirmed(unreadable) = &error else {
            panic!("expected an unconfirmed append, got {error:?}");
        };
        assert_eq!(unreadable.agent_id, agent_id);
        let OplogReadError::SourceUnavailable { source, detail } = &unreadable.error else {
            panic!("expected an unavailable read-back, got {unreadable:?}");
        };
        assert_eq!(*source, OplogReadSource::Primary);
        assert!(detail.contains("oplog entries 7..=8"), "{detail}");
        assert!(
            matches!(
                error.agent_failure(),
                Some(WorkerExecutorError::RecoveryRequired { .. })
            ),
            "{error:?}"
        );
        assert_eq!(oplog.unconfirmed_append(), Some(unreadable.clone()));
        assert_eq!(oplog.current_oplog_index().await, OplogIndex::from_u64(8));
        assert_eq!(
            oplog.read(OplogIndex::from_u64(7)).await,
            Err(unreadable.clone()),
            "the buffered copy {seventh:?} is not history"
        );
        assert_eq!(
            oplog.read_exact(OplogIndex::from_u64(6), 2).await,
            Err(unreadable.clone())
        );
        storage.confirm();
        assert_eq!(
            deltas(&[oplog.read(OplogIndex::from_u64(6)).await.unwrap()]),
            vec![6]
        );
    }

    /// Gate: the batch of an unconfirmed append landed. The handle refuses the commit and gives
    /// no history of the batch; a fresh handle reads the stored batch from the storage.
    #[test]
    async fn an_unconfirmed_batch_that_landed_is_history_only_on_a_fresh_handle() {
        let storage = Arc::new(UnconfirmedAppends::default());
        let service = service(storage.clone()).await;
        let owned_agent_id = OwnedAgentId::new(
            EnvironmentId::new(),
            &crate::test_agent_id("unconfirmed-batch-landed"),
        );
        let oplog = create_owned(&service, &owned_agent_id)
            .await
            .expect("create");
        add_all(&oplog, 2..=3).await;
        oplog.commit(CommitLevel::Always).await.expect("commit");
        storage.unconfirm(IndexedStorageError::Transient(
            "read-back outage".to_string(),
        ));
        add_all(&oplog, 41..=42).await;

        let error = oplog
            .commit(CommitLevel::Always)
            .await
            .expect_err("an unconfirmed append is not a commit");

        let OplogError::Unconfirmed(unreadable) = &error else {
            panic!("expected an unconfirmed append, got {error:?}");
        };
        assert_eq!(oplog.unconfirmed_append().as_ref(), Some(unreadable));
        storage.confirm();
        assert_eq!(
            oplog.read(OplogIndex::from_u64(4)).await.as_ref(),
            Err(unreadable)
        );
        assert_eq!(
            deltas(&history_on_a_fresh_handle(&service, &owned_agent_id, oplog).await),
            vec![2, 3, 41, 42]
        );
    }

    /// Gate: the batch of an unconfirmed append did not land. The handle refuses the commit and
    /// gives no history of the batch; a fresh handle finds that the storage holds none of it.
    #[test]
    async fn an_unconfirmed_batch_that_did_not_land_is_absent_on_a_fresh_handle() {
        let storage = Arc::new(UnconfirmedAppends::default());
        let service = service(storage.clone()).await;
        let owned_agent_id = OwnedAgentId::new(
            EnvironmentId::new(),
            &crate::test_agent_id("unconfirmed-batch-lost"),
        );
        let oplog = create_owned(&service, &owned_agent_id)
            .await
            .expect("create");
        add_all(&oplog, 2..=3).await;
        oplog.commit(CommitLevel::Always).await.expect("commit");
        storage.unconfirm(IndexedStorageError::Other("read-back failure".to_string()));
        storage.drop_appends();
        add_all(&oplog, 51..=52).await;

        let error = oplog
            .commit(CommitLevel::Always)
            .await
            .expect_err("an unconfirmed append is not a commit");

        let OplogError::Unconfirmed(unreadable) = &error else {
            panic!("expected an unconfirmed append, got {error:?}");
        };
        assert_eq!(oplog.unconfirmed_append().as_ref(), Some(unreadable));
        storage.confirm();
        assert_eq!(
            oplog.read(OplogIndex::from_u64(4)).await.as_ref(),
            Err(unreadable)
        );
        assert_eq!(
            deltas(&history_on_a_fresh_handle(&service, &owned_agent_id, oplog).await),
            vec![2, 3]
        );
    }

    /// Gate: a commit stored its batch and only its report failed. That is not an unconfirmed
    /// append: the handle latches nothing, gives the batch as history, and accepts more writes.
    #[test]
    async fn a_commit_whose_report_alone_fails_latches_nothing() {
        let storage = Arc::new(UnconfirmedAppends::default());
        let service = service(storage.clone()).await;
        let agent_id = crate::test_agent_id("unconfirmed-report-only");
        let oplog = create(&service, agent_id).await.expect("create");
        // The replica wait stores 61 with no listener, so the report of the next commit reads it
        // back from the storage.
        add_all(&oplog, 61..=61).await;
        oplog
            .wait_for_replicas(1, Duration::from_secs(1))
            .await
            .expect("flush");
        add_all(&oplog, 62..=63).await;
        storage.fail_next_read(IndexedStorageError::Other(
            "report read failure".to_string(),
        ));

        let error = oplog
            .commit(CommitLevel::Always)
            .await
            .expect_err("the report does not read");

        assert!(matches!(error, OplogError::Unreadable(_)), "{error:?}");
        assert_eq!(oplog.unconfirmed_append(), None);
        assert_eq!(
            deltas(&[oplog.read(OplogIndex::from_u64(4)).await.unwrap()]),
            vec![63]
        );
        oplog
            .add(OplogEntry::grow_memory(64))
            .await
            .expect("open for writes");
    }

    /// A set latch never changes, and only a fence or an unconfirmed append sets it.
    #[test]
    fn the_write_latch_is_set_once() {
        let agent_id = crate::test_agent_id("latch-transition");
        let fence = OplogFence {
            agent_id,
            expected_epoch: ShardEpoch(3),
            actual_epoch: Some(ShardEpoch(4)),
        };
        let unconfirmed = UnreadableOplog::new(
            &agent_id,
            OplogReadError::source_failure(OplogReadSource::Primary, "read back of 9..=9"),
        );
        let latched = WriteLatch::Unconfirmed(unconfirmed.clone());

        assert_eq!(
            WriteLatch::transition(
                None,
                LatchEvent::AppendFailed(&OplogError::Unconfirmed(unconfirmed.clone()))
            ),
            Some(latched.clone())
        );
        assert_eq!(
            WriteLatch::transition(Some(&latched), LatchEvent::TrimFenced(fence.clone())),
            None
        );
        assert_eq!(
            WriteLatch::transition(None, LatchEvent::Opened(fence.clone())),
            Some(WriteLatch::Fenced(fence))
        );
        assert_eq!(
            WriteLatch::transition(
                None,
                LatchEvent::AppendFailed(&OplogError::Payload("no payload".to_string()))
            ),
            None
        );
    }

    /// A replica wait flushes the buffer. When that append has an unknown outcome and its read-back
    /// does not answer, the wait fails the agent with the typed error instead of a panic.
    #[test]
    async fn a_flush_of_an_unconfirmed_append_fails_the_agent() {
        let storage = Arc::new(UnconfirmedAppends::default());
        let service = service(storage.clone()).await;
        let agent_id = crate::test_agent_id("unconfirmed-flush");
        let oplog = create(&service, agent_id).await.expect("create");
        storage.unconfirm(IndexedStorageError::Transient(
            "injected read-back outage".to_string(),
        ));
        add_all(&oplog, 2..=4).await;

        let error = oplog
            .wait_for_replicas(1, Duration::from_secs(1))
            .await
            .expect_err("an unconfirmed flush reached no replica");

        let OplogError::Unconfirmed(unreadable) = &error else {
            panic!("expected an unconfirmed append, got {error:?}");
        };
        assert_eq!(unreadable.agent_id, agent_id);
        let OplogReadError::SourceUnavailable { detail, .. } = &unreadable.error else {
            panic!("expected an unavailable read-back, got {unreadable:?}");
        };
        assert!(detail.contains("oplog entries 2..=4"), "{detail}");
        assert_eq!(oplog.fence(), None);
    }

    /// After an append with an unknown outcome, a later commit on the same handle does not write
    /// the entries again. It returns the same typed error, also when the storage reads again.
    #[test]
    async fn a_commit_after_an_unconfirmed_append_returns_the_same_error() {
        let storage = Arc::new(UnconfirmedAppends::default());
        let service = service(storage.clone()).await;
        let agent_id = crate::test_agent_id("unconfirmed-second-commit");
        let oplog = create(&service, agent_id).await.expect("create");
        storage.unconfirm(IndexedStorageError::Other(
            "injected read-back failure".to_string(),
        ));
        oplog.add(OplogEntry::grow_memory(5)).await.unwrap();
        oplog.add(OplogEntry::grow_memory(6)).await.unwrap();
        let first = oplog
            .commit(CommitLevel::Always)
            .await
            .expect_err("an unconfirmed append is not a commit");
        storage.confirm();

        let second = oplog
            .commit(CommitLevel::Always)
            .await
            .expect_err("a later commit does not confirm the append");

        assert!(
            matches!(
                &second,
                OplogError::Unconfirmed(UnreadableOplog {
                    agent_id: failed,
                    error: OplogReadError::SourceFailure { detail, .. },
                }) if *failed == agent_id && detail.contains("oplog entries 2..=3")
            ),
            "{second:?}"
        );
        assert_eq!(second, first);
        assert_eq!(
            oplog
                .add(OplogEntry::grow_memory(7))
                .await
                .expect_err("latched"),
            first
        );
    }

    /// A create whose initial append has an unknown outcome, and whose read-back fails for good,
    /// fails the agent as `OplogFailed` instead of taking the oplog as created or as absent.
    #[test]
    async fn an_unconfirmed_initial_append_with_a_failed_read_back_fails_the_create() {
        let storage = Arc::new(UnconfirmedAppends::default());
        let service = service(storage.clone()).await;
        let agent_id = crate::test_agent_id("unconfirmed-initial-append");
        storage.unconfirm(IndexedStorageError::Other(
            "injected read-back failure".to_string(),
        ));

        let error = create(&service, agent_id)
            .await
            .expect_err("an unconfirmed initial append is not a create");

        let WorkerExecutorError::OplogFailed {
            agent_id: failed,
            details,
            ..
        } = &error
        else {
            panic!("expected an unconfirmed append, got {error:?}");
        };
        assert_eq!(*failed, agent_id);
        assert!(details.contains("oplog entries 1..=1"), "{details}");
        assert!(details.contains("injected read-back failure"), "{details}");
    }
}
