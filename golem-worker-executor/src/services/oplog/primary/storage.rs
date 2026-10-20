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
/// Reads, deletions and prefix drops keep this shape: a permanent failure there is a broken
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

async fn stored_oplog_batch_matches(
    retry_config: &RetryConfig,
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    namespace: &IndexedStorageNamespace,
    key: &str,
    expected: &[(u64, Bytes)],
) -> Option<bool> {
    let first_id = expected.first().expect("non-empty oplog batch").0;
    let last_id = expected.last().expect("non-empty oplog batch").0;
    retry_storage_op(retry_config, "append_reconcile", key, || {
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

pub(super) async fn retry_oplog_append(
    retry_config: &RetryConfig,
    indexed_storage: &(dyn IndexedStorage + Send + Sync),
    namespace: &IndexedStorageNamespace,
    op_name: &str,
    api_name: &'static str,
    key: &str,
    append: SerializedOplogAppend,
    shard_epoch: Option<ShardEpoch>,
) -> Result<(), IndexedStorageError> {
    let mut attempts = 0u32;
    let mut write_may_have_committed = false;
    loop {
        attempts += 1;
        let error = match append
            .write(indexed_storage, namespace, api_name, key, shard_epoch)
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };

        // The storage refused the write because the shard has a new owner. Not a transient
        // failure to retry and not an indeterminate one to reconcile: it is a deliberate refusal,
        // so hand it back and let the caller give up this one agent instead of aborting.
        if matches!(error, IndexedStorageError::Fenced { .. }) {
            return Err(error);
        }

        let retryable = match &error {
            IndexedStorageError::Indeterminate(_) => {
                write_may_have_committed = true;
                true
            }
            IndexedStorageError::Transient(_) => true,
            IndexedStorageError::Conflict(msg) => {
                if !write_may_have_committed {
                    panic!(
                        "Indexed storage operation '{op_name}' conflicted for key '{key}' without a preceding indeterminate write; possible concurrent oplog writer: {msg}"
                    );
                }
                false
            }
            IndexedStorageError::InvalidResume(_) | IndexedStorageError::Other(_) => {
                if !write_may_have_committed {
                    panic!("Indexed storage operation '{op_name}' failed for key '{key}': {error}");
                }
                false
            }
            // Returned above; named here only because the guard does not make this exhaustive.
            IndexedStorageError::Fenced { .. } => unreachable!("a fence returns before this match"),
        };

        if write_may_have_committed {
            match stored_oplog_batch_matches(
                retry_config,
                indexed_storage,
                namespace,
                key,
                append.entries(),
            )
            .await
            {
                Some(true) => return Ok(()),
                Some(false) => {
                    // The stored content differs from what this attempt sent - the only
                    // legitimate way that happens is a new owner having already written those
                    // same indices. Repeat the write once as a probe: the backends check the
                    // epoch inside the same transaction as the insert, so a shard that has
                    // moved on is fenced before the insert is even attempted. A same-epoch
                    // conflict instead fails the probe's insert (still fatal, below) - the
                    // mismatch is unexplained and not safe to paper over.
                    if let Some(epoch) = shard_epoch
                        && let Err(fenced @ IndexedStorageError::Fenced { .. }) = append
                            .write(indexed_storage, namespace, api_name, key, Some(epoch))
                            .await
                    {
                        return Err(fenced);
                    }
                    panic!(
                        "Indexed storage operation '{op_name}' failed for key '{key}' and the indeterminate write did not match storage: {error}"
                    )
                }
                None => {}
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

        if write_may_have_committed {
            panic!(
                "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts and the indeterminate write did not match storage: {error}"
            );
        } else {
            panic!(
                "Indexed storage operation '{op_name}' failed for key '{key}' after {attempts} attempts: {error}"
            );
        }
    }
}
