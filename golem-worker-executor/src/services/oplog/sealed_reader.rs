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

use super::reader::{OplogRead, OplogReadError, OplogReadSource};
use super::seal::SealRecord;
use super::{CompressedOplogChunk, OplogArchiveError};
use golem_common::model::OwnedAgentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::serialization::decode;
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace,
};
use std::collections::BTreeMap;
use std::path::Path;

/// Reads a frozen manifest without opening an indexed stream or recording an epoch. The exact
/// range merger is the same one used for resident multi-layer oplogs.
pub(crate) async fn read_sealed(
    storage: &dyn BlobStorage,
    owned_agent_id: &OwnedAgentId,
    record: &SealRecord,
    start: OplogIndex,
    count: u64,
) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogReadError> {
    let mut read = OplogRead::new(start, count)?;
    for (&level, manifest) in &record.manifest {
        let source = OplogReadSource::Archive(level);
        let mut entries = BTreeMap::new();
        if let Some((start, count)) = read.next_range() {
            let mut end = start.range_end(count);
            while let Some((&last, chunk_ref)) = manifest.range(end..).next() {
                let bytes = storage
                    .with("oplog_seal", "read_chunk")
                    .get_raw(
                        BlobStorageNamespace::CompressedOplog {
                            environment_id: owned_agent_id.environment_id(),
                            type_tag: owned_agent_id.agent_id.type_tag(),
                            agent_mode: record.agent_mode,
                            level,
                        },
                        Path::new(&chunk_ref.object),
                    )
                    .await
                    .map_err(|error| {
                        OplogReadError::of_archive(
                            source,
                            OplogArchiveError::of_blob(
                                &error,
                                format!(
                                    "failed to read sealed chunk {}: {error}",
                                    chunk_ref.object
                                ),
                            ),
                        )
                    })?
                    .ok_or_else(|| {
                        OplogReadError::corruption(
                            source,
                            format!("sealed chunk {} is missing", chunk_ref.object),
                        )
                    })?;
                let chunk: CompressedOplogChunk = decode(&bytes)
                    .map_err(|error| OplogReadError::corruption(source, error.to_string()))?;
                if chunk.count != chunk_ref.count {
                    return Err(OplogReadError::corruption(
                        source,
                        "chunk count differs from sealed manifest",
                    ));
                }
                let suffix: Vec<_> = chunk
                    .decode(source, last)?
                    .into_iter()
                    .filter(|(idx, _)| *idx >= start && *idx <= end)
                    .collect();
                let Some((first, _)) = suffix.first() else {
                    break;
                };
                let first = *first;
                entries.extend(suffix);
                if first == start {
                    break;
                }
                end = first.previous();
            }
        }
        read.add_source(source, entries)?;
    }
    read.finish()
}
