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

/// Runs a storage operation under the retry policy, handing a fence back instead of panicking
/// on it.
///
/// A fenced write is not a storage failure: the storage is healthy and refused the write on
/// purpose, because this executor no longer owns the agent's shard. Retrying cannot change that,
/// and panicking would take the whole executor down over one agent that simply moved. Every other
/// permanent failure still panics, so the fail-stop contract is unchanged for everything else -
/// including the primary-key collision that has always been the crude fence.
pub(crate) async fn retry_storage_op_fenceable<T, F, Fut>(
    retry_config: &golem_common::model::RetryConfig,
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
            Err(err @ IndexedStorageError::Fenced { .. }) => return Err(err),
            Err(IndexedStorageError::Transient(msg)) => {
                if let Some(delay) = get_delay(retry_config, attempts) {
                    crate::metrics::oplog::record_oplog_storage_retry(op_name);
                    tracing::warn!(
                        op = op_name,
                        key = key,
                        attempt = attempts,
                        delay_ms = delay.as_millis() as u64,
                        "Transient indexed storage error, retrying: {msg}"
                    );
                    tokio::time::sleep(delay).await;
                } else {
                    panic!(
                        "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts: Transient storage error: {msg}"
                    );
                }
            }
            Err(err) => {
                panic!("Indexed storage operation '{op_name}' failed for key '{key}': {err}");
            }
        }
    }
}

/// Records the epoch this executor is allowed to write `key` with, and reports the fence when
/// the stored record is already ahead of it.
///
/// Monotonic on the storage side once a record exists, so a re-grant at a higher epoch takes the
/// key over while an executor holding a stale one cannot claim it back. A key with no record
/// (new, deleted, or from before the record existed) is claimed by whichever epoch opens it
/// first. Written before the key's first entry - an absent record fences too, which is what
/// closes the window between creating a key and recording who owns it. The primary oplog and
/// each compressed archive level record their own key.
pub(crate) async fn record_owning_epoch(
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    retry_config: &golem_common::model::RetryConfig,
    namespace: IndexedStorageNamespace,
    agent_id: &AgentId,
    key: &str,
    shard_epoch: ShardEpoch,
) -> Option<OplogFence> {
    let svc_name = epoch_record_service(&namespace);
    let outcome = retry_storage_op_fenceable(retry_config, "set_key_epoch", key, || {
        let namespace = namespace.clone();
        async move {
            indexed_storage
                .set_key_epoch(svc_name, "set_key_epoch", namespace, key, shard_epoch)
                .await
        }
    })
    .await;
    match epoch_record_verdict(&namespace, agent_id, outcome) {
        Ok(fence) => fence,
        Err(error) => unreachable!("a fenceable storage write failed without a fence: {error}"),
    }
}

/// The service name an epoch record of `namespace` is labelled with.
pub(crate) fn epoch_record_service(namespace: &IndexedStorageNamespace) -> &'static str {
    match namespace {
        IndexedStorageNamespace::CompressedOpLog { .. } => "compressed_oplog",
        IndexedStorageNamespace::BlobOplogManifest { .. } => "blob_oplog",
        _ => "oplog",
    }
}

/// What the outcome of an epoch record of `namespace` gives: no fence when it was stored, the
/// fence when a newer owner's record refused it, and any other storage failure as it is.
pub(crate) fn epoch_record_verdict(
    namespace: &IndexedStorageNamespace,
    agent_id: &AgentId,
    outcome: Result<(), IndexedStorageError>,
) -> Result<Option<OplogFence>, IndexedStorageError> {
    let svc_name = epoch_record_service(namespace);
    let metric_op = match namespace {
        IndexedStorageNamespace::CompressedOpLog { .. }
        | IndexedStorageNamespace::BlobOplogManifest { .. } => "archive_record",
        _ => "record",
    };
    let refusal = match outcome {
        Ok(()) => {
            record_oplog_epoch_fence(metric_op, false);
            return Ok(None);
        }
        Err(refusal @ IndexedStorageError::Fenced { .. }) => refusal,
        Err(error) => return Err(error),
    };
    record_oplog_epoch_fence(metric_op, true);
    let fence = OplogFence::refused(agent_id.clone(), refusal);
    tracing::warn!(
        agent_id = %agent_id,
        store = svc_name,
        expected_epoch = fence.expected_epoch.0,
        actual_epoch = ?fence.actual_epoch.map(|epoch| epoch.0),
        "Oplog opened at a stale shard epoch: the shard has a new owner"
    );
    Ok(Some(fence))
}

/// Records the verdict of an epoch-checked storage write: accepted, or refused because a newer
/// owner's generation is recorded. A write that failed for another reason reached no verdict and
/// records nothing.
pub(crate) fn record_epoch_verdict<T>(op: &'static str, outcome: &Result<T, IndexedStorageError>) {
    match outcome {
        Ok(_) => record_oplog_epoch_fence(op, false),
        Err(IndexedStorageError::Fenced { .. }) => record_oplog_epoch_fence(op, true),
        Err(_) => {}
    }
}

/// Refuses a new write once `fence` has latched, before anything reaches the storage.
pub(crate) fn refuse_if_fenced(fence: Option<&OplogFence>) -> Result<(), OplogError> {
    match fence {
        Some(fence) => Err(OplogError::Fenced(fence.clone())),
        None => Ok(()),
    }
}
