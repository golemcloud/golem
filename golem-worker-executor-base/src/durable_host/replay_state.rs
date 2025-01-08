// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::error::GolemError;
use crate::services::oplog::{Oplog, OplogOps, OplogService};
use golem_common::model::oplog::{AtomicOplogIndex, LogLevel, OplogEntry, OplogIndex};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_common::model::{IdempotencyKey, OwnedWorkerId};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::Value;
use metrohash::MetroHash128;
use std::collections::HashSet;
use std::hash::Hasher;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

#[derive(Clone)]
pub struct ReplayState {
    owned_worker_id: OwnedWorkerId,
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    oplog: Arc<dyn Oplog + Send + Sync>,
    replay_target: AtomicOplogIndex,
    /// The oplog index of the last replayed entry
    last_replayed_index: AtomicOplogIndex,
    internal: Arc<RwLock<InternalReplayState>>,
    has_seen_logs: Arc<AtomicBool>,
}

#[derive(Clone)]
struct InternalReplayState {
    pub deleted_regions: DeletedRegions,
    pub next_deleted_region: Option<OplogRegion>,
    /// Hashes of log entries persisted since the last read non-hint oplog entry
    pub log_hashes: HashSet<(u64, u64)>,
}

impl ReplayState {
    pub async fn new(
        owned_worker_id: OwnedWorkerId,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        oplog: Arc<dyn Oplog + Send + Sync>,
        deleted_regions: DeletedRegions,
        last_oplog_index: OplogIndex,
    ) -> Self {
        let next_deleted_region = deleted_regions.find_next_deleted_region(OplogIndex::NONE);
        let mut result = Self {
            owned_worker_id,
            oplog_service,
            oplog,
            last_replayed_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
            replay_target: AtomicOplogIndex::from_oplog_index(last_oplog_index),
            internal: Arc::new(RwLock::new(InternalReplayState {
                deleted_regions,
                next_deleted_region,
                log_hashes: HashSet::new(),
            })),
            has_seen_logs: Arc::new(AtomicBool::new(false)),
        };
        result.move_replay_idx(OplogIndex::INITIAL).await; // By this we handle initial deleted regions applied by manual updates correctly
        result
    }

    pub fn switch_to_live(&mut self) {
        self.last_replayed_index.set(self.replay_target.get());
    }

    pub fn last_replayed_index(&self) -> OplogIndex {
        self.last_replayed_index.get()
    }

    pub fn replay_target(&self) -> OplogIndex {
        self.replay_target.get()
    }

    pub async fn deleted_regions(&self) -> DeletedRegions {
        let internal = self.internal.read().await;
        internal.deleted_regions.clone()
    }

    pub async fn add_deleted_region(&mut self, region: OplogRegion) {
        let mut internal = self.internal.write().await;
        internal.deleted_regions.add(region);
    }

    pub async fn is_in_deleted_region(&self, oplog_index: OplogIndex) -> bool {
        let internal = self.internal.read().await;
        internal.deleted_regions.is_in_deleted_region(oplog_index)
    }

    /// Returns whether we are in live mode where we are executing new calls.
    pub fn is_live(&self) -> bool {
        self.last_replayed_index.get() == self.replay_target.get()
    }

    /// Returns whether we are in replay mode where we are replaying old calls.
    pub fn is_replay(&self) -> bool {
        !self.is_live()
    }

    /// Reads the next oplog entry, and skips every hint entry following it.
    /// Returns the oplog index of the entry read, no matter how many more hint entries
    /// were read.
    pub async fn get_oplog_entry(&mut self) -> (OplogIndex, OplogEntry) {
        let read_idx = self.last_replayed_index.get().next();
        let entry = self.internal_get_next_oplog_entry().await;

        // Skipping hint entries and recording log entries
        let mut logs = HashSet::new();
        while self.is_replay() {
            let saved_replay_idx = self.last_replayed_index.get();
            let internal = self.internal.read().await;
            let saved_next_deleted_region = internal.next_deleted_region.clone();
            drop(internal);
            let entry = self.internal_get_next_oplog_entry().await;
            if !entry.is_hint() {
                self.last_replayed_index.set(saved_replay_idx);
                let mut internal = self.internal.write().await;
                // TODO: cache the last hint entry to avoid reading it again
                internal.next_deleted_region = saved_next_deleted_region;
                break;
            } else if let OplogEntry::Log {
                level,
                context,
                message,
                ..
            } = &entry
            {
                let hash = Self::hash_log_entry(*level, context, message);
                logs.insert(hash);
            }
        }

        self.has_seen_logs
            .store(!logs.is_empty(), Ordering::Relaxed);
        let mut internal = self.internal.write().await;
        internal.log_hashes = logs;

        (read_idx, entry)
    }

    /// Returns true if the given log entry has been seen since the last non-hint oplog entry.
    pub async fn seen_log(&self, level: LogLevel, context: &str, message: &str) -> bool {
        if self.has_seen_logs.load(Ordering::Relaxed) {
            let hash = Self::hash_log_entry(level, context, message);
            let internal = self.internal.read().await;
            internal.log_hashes.contains(&hash)
        } else {
            false
        }
    }

    /// Removes a seen log from the set. If the set becomes empty, `seen_log` becomes a cheap operation
    pub async fn remove_seen_log(&self, level: LogLevel, context: &str, message: &str) {
        let hash = Self::hash_log_entry(level, context, message);
        let mut internal = self.internal.write().await;
        internal.log_hashes.remove(&hash);
        self.has_seen_logs
            .store(!internal.log_hashes.is_empty(), Ordering::Relaxed);
    }

    fn hash_log_entry(level: LogLevel, context: &str, message: &str) -> (u64, u64) {
        let mut hasher = MetroHash128::new();
        hasher.write_u8(level as u8);
        hasher.write(context.as_bytes());
        hasher.write(message.as_bytes());
        hasher.finish128()
    }

    /// Gets the next oplog entry, no matter if it is hint or not, following jumps
    async fn internal_get_next_oplog_entry(&mut self) -> OplogEntry {
        let read_idx = self.last_replayed_index.get().next();

        let oplog_entries = self.read_oplog(read_idx, 1).await;
        let oplog_entry = oplog_entries.into_iter().next().unwrap();
        self.move_replay_idx(read_idx).await;

        oplog_entry
    }

    async fn move_replay_idx(&mut self, new_idx: OplogIndex) {
        self.last_replayed_index.set(new_idx);
        self.get_out_of_deleted_region().await;
    }

    pub async fn lookup_oplog_entry(
        &mut self,
        begin_idx: OplogIndex,
        check: impl Fn(&OplogEntry, OplogIndex) -> bool,
    ) -> Option<OplogIndex> {
        self.lookup_oplog_entry_with_condition(begin_idx, check, |_, _| true)
            .await
    }

    pub async fn lookup_oplog_entry_with_condition(
        &mut self,
        begin_idx: OplogIndex,
        end_check: impl Fn(&OplogEntry, OplogIndex) -> bool,
        for_all_intermediate: impl Fn(&OplogEntry, OplogIndex) -> bool,
    ) -> Option<OplogIndex> {
        let replay_target = self.replay_target.get();
        let mut start = self.last_replayed_index.get().next();

        const CHUNK_SIZE: u64 = 1024;
        while start < replay_target {
            let entries = self
                .oplog_service
                .read(&self.owned_worker_id, start, CHUNK_SIZE)
                .await;
            for (idx, entry) in &entries {
                // TODO: handle deleted regions
                if end_check(entry, begin_idx) {
                    return Some(*idx);
                } else if !for_all_intermediate(entry, begin_idx) {
                    return None;
                }
            }
            start = start.range_end(entries.len() as u64).next();
        }

        None
    }

    pub async fn get_oplog_entry_exported_function_invoked(
        &mut self,
    ) -> Result<Option<(String, Vec<Value>, IdempotencyKey)>, GolemError> {
        loop {
            if self.is_replay() {
                let (_, oplog_entry) = self.get_oplog_entry().await;
                match &oplog_entry {
                    OplogEntry::ExportedFunctionInvoked {
                        function_name,
                        idempotency_key,
                        ..
                    } => {
                        let request: Vec<golem_wasm_rpc::protobuf::Val> = self
                            .oplog
                            .get_payload_of_entry(&oplog_entry)
                            .await
                            .expect("failed to deserialize function request payload")
                            .unwrap();
                        let request = request
                            .into_iter()
                            .map(|val| {
                                val.try_into()
                                    .expect("failed to decode serialized protobuf value")
                            })
                            .collect::<Vec<Value>>();
                        break Ok(Some((
                            function_name.to_string(),
                            request,
                            idempotency_key.clone(),
                        )));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(GolemError::unexpected_oplog_entry(
                            "ExportedFunctionInvoked",
                            format!("{:?}", oplog_entry),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    pub async fn get_oplog_entry_exported_function_completed(
        &mut self,
    ) -> Result<Option<TypeAnnotatedValue>, GolemError> {
        loop {
            if self.is_replay() {
                let (_, oplog_entry) = self.get_oplog_entry().await;
                match &oplog_entry {
                    OplogEntry::ExportedFunctionCompleted { .. } => {
                        let response: TypeAnnotatedValue = self
                            .oplog
                            .get_payload_of_entry(&oplog_entry)
                            .await
                            .expect("failed to deserialize function response payload")
                            .unwrap();

                        break Ok(Some(response));
                    }
                    entry if entry.is_hint() => {}
                    _ => {
                        break Err(GolemError::unexpected_oplog_entry(
                            "ExportedFunctionCompleted",
                            format!("{:?}", oplog_entry),
                        ));
                    }
                }
            } else {
                break Ok(None);
            }
        }
    }

    pub(crate) async fn get_out_of_deleted_region(&mut self) {
        if self.is_replay() {
            let mut internal = self.internal.write().await;
            let update_next_deleted_region = match &internal.next_deleted_region {
                Some(region) if region.start == (self.last_replayed_index.get().next()) => {
                    let target = region.end.next(); // we want to continue reading _after_ the region
                    debug!(
                        "Worker reached deleted region at {}, jumping to {} (oplog size: {})",
                        region.start,
                        target,
                        self.replay_target.get()
                    );
                    self.last_replayed_index.set(target.previous()); // so we set the last replayed index to the end of the region

                    true
                }
                _ => false,
            };

            if update_next_deleted_region {
                internal.next_deleted_region = internal
                    .deleted_regions
                    .find_next_deleted_region(self.last_replayed_index.get());
            }
        }
    }

    async fn read_oplog(&self, idx: OplogIndex, n: u64) -> Vec<OplogEntry> {
        self.oplog_service
            .read(&self.owned_worker_id, idx, n)
            .await
            .into_values()
            .collect()
    }
}
