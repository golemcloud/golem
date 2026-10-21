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

pub(super) async fn read_persisted_oplog_entries(
    indexed_storage: Arc<dyn IndexedStorage + Send + Sync>,
    namespace: IndexedStorageNamespace,
    key: String,
    start: u64,
    end: u64,
) -> Result<Vec<(u64, OplogEntry)>, IndexedStorageError> {
    let entries = indexed_storage
        .with_entity("oplog", "read", "entry")
        .read_raw(namespace, &key, start, end)
        .await?;

    join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
        entries
            .into_iter()
            .map(|(idx, bytes)| deserialize(&bytes).map(|entry| (idx, entry)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(IndexedStorageError::Other)
    }))
    .await
    .map_err(|error| {
        IndexedStorageError::Other(format!("oplog deserialization task failed: {error}"))
    })?
}

impl OplogReader {
    pub(super) async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        record_oplog_call("read");

        let buffer_start = self.last_committed_idx.next();
        if oplog_index >= buffer_start {
            let offset = u64::from(oplog_index).saturating_sub(u64::from(buffer_start));
            if let Ok(offset) = usize::try_from(offset)
                && let Some(entry) = self.buffer.get(offset)
            {
                return entry.clone();
            }
        }

        let entries: Vec<(u64, OplogEntry)> = {
            let is = self.indexed_storage.clone();
            let namespace = self.namespace.clone();
            let key = self.key.clone();
            let idx: u64 = oplog_index.into();
            retry_storage_op(&self.retry_config, "read", &key, || {
                let is = is.clone();
                let ns = namespace.clone();
                let key = key.clone();
                async move { read_persisted_oplog_entries(is, ns, key, idx, idx).await }
            })
            .await
        };

        entries
            .into_iter()
            .next()
            .map(|(_, entry)| entry)
            .unwrap_or_else(|| {
                panic!(
                    "Missing oplog entry {oplog_index} for {} in indexed storage",
                    self.key
                )
            })
    }

    pub(super) async fn read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.try_read_source(oplog_index, n)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
    }

    pub(super) async fn try_read_source(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, String> {
        record_oplog_call("read_exact");

        let Some(last_idx) =
            checked_range_end(oplog_index, n).map_err(|error| error.to_string())?
        else {
            return Ok(BTreeMap::new());
        };

        let mut result: BTreeMap<OplogIndex, OplogEntry> = if oplog_index <= self.last_committed_idx
        {
            let is = self.indexed_storage.clone();
            let namespace = self.namespace.clone();
            let key = self.key.clone();
            let start: u64 = oplog_index.into();
            let end: u64 = min(last_idx, self.last_committed_idx).into();
            retry_storage_op_result(&self.retry_config, "read_exact", &key, || {
                let is = is.clone();
                let ns = namespace.clone();
                let key = key.clone();
                async move { read_persisted_oplog_entries(is, ns, key, start, end).await }
            })
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

    pub(super) async fn length(&self) -> u64 {
        self.try_length()
            .await
            .unwrap_or_else(|error| panic!("{error}"))
    }

    pub(super) async fn try_length(&self) -> Result<u64, String> {
        record_oplog_call("length");

        Ok({
            let is = self.indexed_storage.clone();
            let namespace = self.namespace.clone();
            let key = self.key.clone();
            retry_storage_op_result(&self.retry_config, "length", &key, || {
                let is = is.clone();
                let ns = namespace.clone();
                let key = key.clone();
                async move { is.with("oplog", "length").length(ns, &key).await }
            })
            .await?
        })
    }
}
