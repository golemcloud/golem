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

#[async_trait]
pub(super) trait BackgroundTransfer {
    async fn read_source(&self) -> Result<Vec<(OplogIndex, OplogEntry)>, String>;
    async fn append_target(&self, entries: &[(OplogIndex, OplogEntry)]) -> Result<(), OplogError>;
    async fn verify_target(&self, entries: &[(OplogIndex, OplogEntry)]) -> Result<(), String>;
    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex) -> Result<(), OplogError>;

    /// Copies, verifies, then trims, keeping the source until the copy is verified. A failed step
    /// ends the transfer: `OplogError::Fenced` when the oplog has a newer owner, so an unwritten
    /// target is not verified and the source is not trimmed of entries that were never archived,
    /// and `OplogError::Maintenance` for a storage failure the next attempt retries.
    async fn run(&self) -> Result<(), OplogError> {
        let entries = self.read_source().await.map_err(OplogError::Maintenance)?;
        match entries.last() {
            Some(last_entry) => {
                let last_dropped_id = last_entry.0;
                self.append_target(&entries).await?;
                self.verify_target(&entries)
                    .await
                    .map_err(OplogError::Maintenance)?;
                self.drop_source_prefix(last_dropped_id).await?;
            }
            None => {
                warn!("No entries to transfer from the primary oplog");
            }
        }
        Ok(())
    }
}

impl BackgroundTransferFromPrimary {
    pub fn new(
        last_transferred_idx: OplogIndex,
        primary: Arc<dyn Oplog>,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    ) -> Self {
        Self {
            last_transferred_idx,
            primary,
            lower,
        }
    }
}

#[async_trait]
impl BackgroundTransfer for BackgroundTransferFromPrimary {
    async fn read_source(&self) -> Result<Vec<(OplogIndex, OplogEntry)>, String> {
        let entries = self
            .primary
            .try_read_source(OplogIndex::INITIAL, self.last_transferred_idx.as_u64())
            .await?;
        validate_transfer_source(entries, self.last_transferred_idx, OplogReadSource::Primary)
            .map_err(|error| error.to_string())
    }

    async fn append_target(&self, entries: &[(OplogIndex, OplogEntry)]) -> Result<(), OplogError> {
        self.lower.first().append(entries).await.map(|_| ())
    }

    async fn verify_target(&self, entries: &[(OplogIndex, OplogEntry)]) -> Result<(), String> {
        self.lower
            .first()
            .verify_persisted(entries)
            .await
            .map_err(|error| error.to_string())
    }

    /// The primary latches a refused trim itself and reports it typed on the next write through
    /// it, so only a storage failure is returned here.
    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex) -> Result<(), OplogError> {
        self.primary
            .try_drop_prefix(last_dropped_id)
            .await
            .map_err(OplogError::Maintenance)?;
        Ok(())
    }
}

impl BackgroundTransferBetweenLowers {
    pub fn new(
        source: usize,
        last_transferred_idx: OplogIndex,
        lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
    ) -> Self {
        let source_layer = lower[source].clone();
        let target_layer = lower[source + 1].clone();

        Self {
            last_transferred_idx,
            source_layer,
            target_layer,
        }
    }
}

pub(crate) async fn transfer_between_lower_layers(
    source: usize,
    last_transferred_idx: OplogIndex,
    lower: NEVec<Arc<dyn OplogArchive + Send + Sync>>,
) -> Result<(), OplogError> {
    BackgroundTransferBetweenLowers::new(source, last_transferred_idx, lower)
        .run()
        .await
}

#[async_trait]
impl BackgroundTransfer for BackgroundTransferBetweenLowers {
    async fn read_source(&self) -> Result<Vec<(OplogIndex, OplogEntry)>, String> {
        let entries = self
            .source_layer
            .read_source(OplogIndex::INITIAL, self.last_transferred_idx.as_u64())
            .await
            .map_err(|error| error.to_string())?;
        validate_transfer_source(
            entries,
            self.last_transferred_idx,
            OplogReadSource::Other("transfer source archive"),
        )
        .map_err(|error| error.to_string())
    }

    async fn append_target(&self, entries: &[(OplogIndex, OplogEntry)]) -> Result<(), OplogError> {
        self.target_layer.append(entries).await.map(|_| ())
    }

    async fn verify_target(&self, entries: &[(OplogIndex, OplogEntry)]) -> Result<(), String> {
        self.target_layer
            .verify_persisted(entries)
            .await
            .map_err(|error| error.to_string())
    }

    async fn drop_source_prefix(&self, last_dropped_id: OplogIndex) -> Result<(), OplogError> {
        self.source_layer
            .drop_prefix(last_dropped_id)
            .await
            .map(|_| ())
    }
}

fn validate_transfer_source(
    entries: BTreeMap<OplogIndex, OplogEntry>,
    expected_end: OplogIndex,
    source: OplogReadSource,
) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogReadError> {
    let Some((&start, _)) = entries.first_key_value() else {
        return Ok(Vec::new());
    };
    let (&end, _) = entries.last_key_value().unwrap();
    if end != expected_end || end.as_u64() - start.as_u64() + 1 != entries.len() as u64 {
        return Err(OplogReadError::corruption(
            source,
            format!(
                "transfer source returned {} entries in range [{start}..={end}], expected a contiguous suffix ending at {expected_end}",
                entries.len()
            ),
        ));
    }
    Ok(entries.into_iter().collect())
}
