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

/// Reads the stored entries in `[start..=end]` and decodes them.
///
/// A storage failure is retried while the retry policy allows. An entry that does not decode is
/// corruption of the primary oplog, and the error names its index and the decoder reason.
pub(super) async fn read_persisted_oplog_entries(
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    retry_config: &RetryConfig,
    op_name: &'static str,
    namespace: IndexedStorageNamespace,
    key: &str,
    start: u64,
    end: u64,
) -> Result<Vec<(u64, OplogEntry)>, OplogReadError> {
    let entries = retry_primary_read(retry_config, op_name, key, || {
        let indexed_storage = indexed_storage.clone();
        let namespace = namespace.clone();
        async move {
            indexed_storage
                .with_entity("oplog", "read", "entry")
                .read_raw(namespace, key, start, end)
                .await
        }
    })
    .await?;

    join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
        entries
            .into_iter()
            .map(|(idx, bytes)| {
                decode::<OplogEntry>(&bytes)
                    .map(|entry| (idx, entry))
                    .map_err(|error| (idx, error))
            })
            .collect::<Result<Vec<_>, (u64, DecodeError)>>()
    }))
    .await
    .map_err(|error| {
        OplogReadError::source_failure(
            OplogReadSource::Primary,
            format!("oplog deserialization task failed: {error}"),
        )
    })?
    .map_err(|(idx, error)| primary_decode_error(idx, &error))
}

/// An entry that does not decode gives the same error on every read, so it is corruption.
fn primary_decode_error(idx: u64, error: &DecodeError) -> OplogReadError {
    OplogReadError::corruption(
        OplogReadSource::Primary,
        format!("failed to decode oplog entry {idx}: {error}"),
    )
}

/// Runs a read of the primary storage under the retry policy. A storage that did not answer
/// after the retries is unavailable; any other failure is a source failure. The error names the
/// operation and the storage key.
pub(super) async fn retry_primary_read<T, F, Fut>(
    retry_config: &RetryConfig,
    op_name: &'static str,
    key: &str,
    op: F,
) -> Result<T, OplogReadError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, IndexedStorageError>>,
{
    retry_maintenance_storage_op(retry_config, op_name, key, op)
        .await
        .map_err(|error| primary_read_error(op_name, key, error))
}

fn primary_read_error(op_name: &str, key: &str, error: IndexedStorageError) -> OplogReadError {
    let detail = format!("indexed storage operation '{op_name}' failed for key '{key}': {error}");
    match error {
        IndexedStorageError::Transient(_) => {
            OplogReadError::source_unavailable(OplogReadSource::Primary, detail)
        }
        _ => OplogReadError::source_failure(OplogReadSource::Primary, detail),
    }
}

impl OplogReader {
    pub(super) async fn read(&self, oplog_index: OplogIndex) -> Result<OplogEntry, OplogReadError> {
        record_oplog_call("read");

        let buffer_start = self.last_committed_idx.next();
        if oplog_index >= buffer_start
            && let Some(error) = &self.unconfirmed_tail
        {
            return Err(error.clone());
        }
        if oplog_index >= buffer_start {
            let offset = u64::from(oplog_index).saturating_sub(u64::from(buffer_start));
            if let Ok(offset) = usize::try_from(offset)
                && let Some(entry) = self.buffer.get(offset)
            {
                return Ok(entry.clone());
            }
        }

        let idx: u64 = oplog_index.into();
        let entries = read_persisted_oplog_entries(
            self.indexed_storage.clone(),
            &self.retry_config,
            "read",
            self.namespace.clone(),
            &self.key,
            idx,
            idx,
        )
        .await?;

        entries
            .into_iter()
            .next()
            .map(|(_, entry)| entry)
            .ok_or(OplogReadError::Gap {
                start: oplog_index,
                end: oplog_index,
            })
    }

    pub(super) async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogReadError> {
        record_oplog_call("read_exact");

        let Some(last_idx) = checked_range_end(oplog_index, n)? else {
            return Ok(BTreeMap::new());
        };
        if last_idx > self.last_committed_idx
            && let Some(error) = &self.unconfirmed_tail
        {
            return Err(error.clone());
        }

        let mut result: BTreeMap<OplogIndex, OplogEntry> = if oplog_index <= self.last_committed_idx
        {
            read_persisted_oplog_entries(
                self.indexed_storage.clone(),
                &self.retry_config,
                "read_exact",
                self.namespace.clone(),
                &self.key,
                oplog_index.into(),
                min(last_idx, self.last_committed_idx).into(),
            )
            .await?
            .into_iter()
            .map(|(idx, entry)| (OplogIndex::from_u64(idx), entry))
            .collect()
        } else {
            BTreeMap::new()
        };

        if last_idx >= self.last_committed_idx {
            // There can be some uncommitted entries in the buffer
            if !self.buffer.is_empty() {
                let requested_start: u64 = oplog_index.into();
                let requested_end: u64 = last_idx.into();
                let buffer_start: u64 = self.last_committed_idx.next().into();
                let buffer_end = buffer_start + self.buffer.len() as u64 - 1;
                let overlap_start = max(requested_start, buffer_start);
                let overlap_end = min(requested_end, buffer_end);

                if overlap_start <= overlap_end {
                    let offset = (overlap_start - buffer_start) as usize;
                    let count = (overlap_end - overlap_start + 1) as usize;
                    for idx in 0..count {
                        result.insert(
                            OplogIndex::from_u64(overlap_start + idx as u64),
                            self.buffer[offset + idx].clone(),
                        );
                    }
                }
            }
        }

        Ok(result)
    }

    pub(super) async fn length(&self) -> Result<u64, OplogReadError> {
        record_oplog_call("length");
        let indexed_storage = self.indexed_storage.as_ref();
        let key = self.key.as_str();
        retry_primary_read(&self.retry_config, "length", key, || {
            let namespace = self.namespace.clone();
            async move {
                indexed_storage
                    .with("oplog", "length")
                    .length(namespace, key)
                    .await
            }
        })
        .await
    }
}
