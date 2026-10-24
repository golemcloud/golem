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

impl PrimaryOplogState {
    /// Computes a payload reference and, for an external (large) payload, spawns its blob upload and
    /// registers it for the commit barrier — **without** awaiting the upload.
    ///
    /// Intentionally a non-`async` `fn`: computing the reference, spawning the upload, and
    /// registering the [`PendingUpload`] happen as one non-yielding step inside the actor's
    /// `AddStart` job handler, which then builds and `push`es the `Start` from this reference with
    /// no `.await` in between, so concurrent calls' `Start` entries stay in initiation order. That
    /// no-`.await` window is enforced at compile time by the returned `!Send` [`ReserveGuard`].
    pub(super) fn reserve_raw_payload(&mut self, data: Vec<u8>) -> ReservedPayload {
        if data.len() > self.max_payload_size {
            let payload_id = PayloadId::new();
            let md5_hash = md5::compute(&data).to_vec();
            let path = format!("{}/{}", hex::encode(&md5_hash), payload_id.0);
            let data_len = data.len() as u64;

            let blob_storage = self.blob_storage.clone();
            let environment_id = self.owned_agent_id.environment_id();
            let agent_id = self.owned_agent_id.agent_id();
            let agent_mode = self.agent_mode;
            let account_id = self.account_id.to_string();
            let environment_id_label = environment_id.to_string();

            let upload = async move {
                blob_storage
                    .with("oplog", "upload_payload")
                    .put_raw(
                        BlobStorageNamespace::OplogPayload {
                            environment_id,
                            agent_id,
                            agent_mode,
                        },
                        Path::new(&path),
                        &data,
                    )
                    .await
                    .map_err(|e| format!("Failed uploading oplog data to the blob store {e}"))?;
                record_storage_bytes_written(
                    STORAGE_TYPE_OPLOG,
                    &account_id,
                    &environment_id_label,
                    data_len,
                );
                Ok::<(), String>(())
            };

            let upload = tokio::spawn(upload)
                .map(|joined| {
                    joined.unwrap_or_else(|join_err| {
                        Err(format!("oplog payload upload task failed: {join_err}"))
                    })
                })
                .boxed()
                .shared();
            let pending = PendingUpload::spawned(upload);
            self.pending_uploads.push(pending.clone());
            ReservedPayload::new(
                RawOplogPayload::External {
                    payload_id,
                    md5_hash,
                },
                pending,
            )
        } else {
            ReservedPayload::new(
                RawOplogPayload::SerializedInline(data),
                PendingUpload::already_durable(),
            )
        }
    }

    async fn append(
        &mut self,
        entries: Vec<OplogEntry>,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        record_oplog_call("append");

        // Already refused once: fail fast rather than re-asking the storage. Only entries buffered
        // before the fence latched can reach here, and they go back where they were.
        if let Some(fence) = self.fence.get() {
            let fence = fence.clone();
            self.retain_refused(entries);
            return Err(OplogError::Fenced(fence));
        }

        // Commit barrier: every deferred external payload reserved during this session must be
        // durably written to blob storage before the entries (which may reference it) are persisted
        // to indexed storage. `append` flushes the whole buffer, so waiting on all outstanding
        // uploads is correct. A permanent upload failure is treated like a permanent storage
        // failure (see `retry_storage_op`): there is no safe way to commit a dangling reference.
        if !self.pending_uploads.is_empty() {
            let pending = std::mem::take(&mut self.pending_uploads);
            let mut result = Ok(());
            for upload in pending {
                result = result.and(upload.wait().await);
            }
            if let Err(err) = result {
                panic!(
                    "Oplog payload upload failed for key '{}', cannot commit referencing entries: {err}",
                    self.key
                );
            }
        }

        if entries.is_empty() {
            return Ok(BTreeMap::new());
        }

        let entry_count = entries.len() as u64;
        let mut pairs = Vec::with_capacity(entries.len());
        let mut last_idx = self.last_committed_idx;
        for entry in entries {
            let oplog_idx = last_idx.next();
            pairs.push((oplog_idx.into(), entry));
            last_idx = oplog_idx;
        }
        let mut bytes_written = 0u64;
        let mut serialized_pairs = Vec::with_capacity(pairs.len());
        for (id, entry) in &pairs {
            let value = serialize(entry)
                .unwrap_or_else(|err| panic!("Failed to serialize oplog entry: {err}"));
            bytes_written += value.len() as u64;
            serialized_pairs.push((*id, Bytes::from(value)));
        }
        let serialized_pairs: Arc<[(u64, Bytes)]> = serialized_pairs.into();
        let namespace = self.namespace.clone();
        let appended = retry_oplog_append(
            &self.retry_config,
            self.indexed_storage.as_ref(),
            &namespace,
            "append",
            "append",
            &self.key,
            SerializedOplogAppend::Batch(serialized_pairs),
            self.shard_epoch,
        )
        .await
        .map_err(|err| Self::as_oplog_error(&self.owned_agent_id, err));
        if self.shard_epoch.is_some() {
            record_oplog_epoch_fence("append", matches!(appended, Err(OplogError::Fenced(_))));
        }
        if let Err(error) = appended {
            if let OplogError::Fenced(fence) = &error {
                // The commit barrier above already awaited every payload the batch referenced, so
                // those blobs are durable and stay behind with no stored entry pointing at them.
                // The epoch the storage holds is reported, so a shard manager whose state lost
                // history can mint above it. Warned only when it latches: each write after that
                // fails fast on the latch without reaching the storage.
                if self.fence.set(fence.clone()).is_ok() {
                    warn!(
                        agent_id = %self.owned_agent_id,
                        expected_epoch = fence.expected_epoch.0,
                        actual_epoch = ?fence.actual_epoch.map(|epoch| epoch.0),
                        "Oplog append fenced: the shard has a new owner, refusing further writes"
                    );
                }
                self.retain_refused(pairs.into_iter().map(|(_, entry)| entry));
            }
            return Err(error);
        }

        record_storage_bytes_written(
            STORAGE_TYPE_OPLOG,
            &self.account_id_label,
            &self.environment_id_label,
            bytes_written,
        );
        record_storage_objects_written(
            STORAGE_TYPE_OPLOG,
            &self.account_id_label,
            &self.environment_id_label,
            entry_count,
        );

        self.last_committed_idx = last_idx;
        Ok(BTreeMap::from_iter(
            pairs
                .into_iter()
                .map(|(idx, entry)| (OplogIndex::from_u64(idx), entry)),
        ))
    }

    /// Refuses a new write once the fence has latched, before anything is buffered or reserved.
    ///
    /// Without it an add below the commit threshold would only buffer, answer with an index, and
    /// report a write that can never reach the storage.
    pub(super) fn refuse_if_fenced(&self) -> Result<(), OplogError> {
        refuse_if_fenced(self.fence.get())
    }

    /// Puts entries a fenced append turned away back at the head of the buffer, where `commit`
    /// drained them from.
    ///
    /// Every index this oplog has handed out stays readable from it: `last_oplog_idx` is not
    /// rolled back, and the reader maps the buffer from `last_committed_idx`. A reader that took
    /// `current_oplog_index` before the refusal and reads after it would otherwise find a gap and
    /// fail-stop the executor. The entries are never sent again, because every later append fails
    /// on the latch, and the buffer cannot grow, because every later add is refused.
    fn retain_refused(&mut self, entries: impl IntoIterator<Item = OplogEntry>) {
        let mut restored: VecDeque<OplogEntry> = entries.into_iter().collect();
        restored.append(&mut self.buffer);
        self.buffer = restored;
    }

    /// Commits if the buffer is over the threshold. Separated out so the actor arms can fold a
    /// threshold-commit failure into the job that triggered it.
    pub(super) async fn maybe_commit(&mut self) -> Result<(), OplogError> {
        if self.over_commit_threshold() {
            self.auto_commit().await?;
        }
        Ok(())
    }

    /// Commits the buffer outside an explicit commit: on the commit threshold or for a replica
    /// wait. While a listener collects the entries with [`CommitLevel::ReportOnly`], they stay in
    /// memory for that report, up to the latest [`MAX_RETAINED_RECEIPT_BATCHES`] commits; the
    /// report reads older and unretained entries back from storage.
    pub(super) async fn auto_commit(&mut self) -> Result<(), OplogError> {
        let committed = self.commit(CommitLevel::Always).await?;
        if committed.is_empty() {
            return Ok(());
        }
        if self.retained_auto_commits.len() == MAX_RETAINED_RECEIPT_BATCHES {
            self.retained_auto_commits.pop_front();
        }
        self.retained_auto_commits.push_back(committed);
        if !self.auto_commit_listener.notify() {
            self.retained_auto_commits.clear();
        }
        Ok(())
    }

    /// Names the agent on a refused write, so the worker that hit it can be given up by id.
    fn as_oplog_error(owned_agent_id: &OwnedAgentId, err: IndexedStorageError) -> OplogError {
        OplogError::Fenced(OplogFence::refused(owned_agent_id.agent_id(), err))
    }

    /// Pushes an entry into the in-memory buffer and advances the oplog index,
    /// without checking the commit threshold. Callers must run [`maybe_commit`]
    /// afterwards. Used by `add_pair` to buffer a `Start`/`End` pair before a
    /// single commit-threshold check, so the pair is never split by a commit.
    pub(super) fn push(&mut self, entry: OplogEntry) -> OplogIndex {
        let is_hint = entry.is_hint();
        let next_index = self.last_oplog_idx.next();
        self.durable_stream_sessions.apply_entry(next_index, &entry);
        self.buffer.push_back(entry);
        self.last_oplog_idx = next_index;
        if !is_hint {
            self.last_added_non_hint_entry = Some(self.last_oplog_idx);
        }
        self.last_oplog_idx
    }

    /// Snapshots everything needed to serve reads off the actor task, so read I/O never blocks
    /// the actor's job loop (see [`OplogJob::Reader`]).
    pub(super) fn reader(&self) -> OplogReader {
        OplogReader {
            indexed_storage: self.indexed_storage.clone(),
            retry_config: self.retry_config.clone(),
            namespace: self.namespace.clone(),
            key: self.key.clone(),
            last_committed_idx: self.last_committed_idx,
            buffer: self.buffer.clone(),
            replicas: self.replicas,
        }
    }

    /// Whether the buffer has grown past the commit threshold and a commit should be scheduled.
    /// The commit itself always runs inside the actor, before the triggering job replies (see
    /// the note on [`OplogJob`]).
    fn over_commit_threshold(&self) -> bool {
        self.buffer.len() > self.max_operations_before_commit as usize
    }

    pub(super) async fn commit(
        &mut self,
        _level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogError> {
        record_oplog_call("commit");

        let entries = self.buffer.drain(..).collect::<Vec<OplogEntry>>();
        self.append(entries).await
    }

    pub(super) async fn committed_since_last_report(
        &mut self,
        previously_committed_through: OplogIndex,
        mut newly_committed: BTreeMap<OplogIndex, OplogEntry>,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let committed_through = self.last_committed_idx;
        let mut retained: BTreeMap<_, _> = std::mem::take(&mut self.retained_auto_commits)
            .into_iter()
            .flatten()
            .collect();
        let read_through = retained
            .first_key_value()
            .map_or(previously_committed_through, |(first, _)| first.previous());
        let mut entries = if self.last_reported_commit_idx < read_through {
            let start = self.last_reported_commit_idx.next();
            let count = u64::from(read_through) - u64::from(self.last_reported_commit_idx);
            let entries = self.reader().read_source(start, count).await;
            exact_from_source(OplogReadSource::Primary, start, count, entries)
                .unwrap_or_else(|error| panic!("Oplog read failed: {error}"))
        } else {
            BTreeMap::new()
        };
        entries.append(&mut retained);
        entries.append(&mut newly_committed);
        self.last_reported_commit_idx = committed_through;
        entries
    }

    /// Trims the entries an archive transfer has copied, asserting the epoch this oplog was opened
    /// with in the same step. A refusal removes nothing and latches the fence: the oplog belongs to
    /// a newer owner, whose own transfer decides what leaves it, so there is nothing to retry. Any
    /// other failure is returned for the archive maintenance to retry later.
    pub(super) async fn try_drop_prefix(&self, last_dropped_id: OplogIndex) -> Result<(), String> {
        record_oplog_call("drop_prefix");
        if self.fence.get().is_some() {
            return Ok(());
        }

        let is = self.indexed_storage.clone();
        let namespace = self.namespace.clone();
        let key = self.key.clone();
        let dropped_id: u64 = last_dropped_id.into();
        let shard_epoch = self.shard_epoch;
        let trimmed = retry_maintenance_storage_op(&self.retry_config, "drop_prefix", &key, || {
            let is = is.clone();
            let ns = namespace.clone();
            let key = key.clone();
            async move {
                is.with("oplog", "drop_prefix")
                    .drop_prefix(ns, &key, dropped_id, shard_epoch)
                    .await
            }
        })
        .await;
        if self.shard_epoch.is_some() {
            record_epoch_verdict("drop_prefix", &trimmed);
        }
        match trimmed {
            Ok(()) => Ok(()),
            Err(error @ IndexedStorageError::Fenced { .. }) => {
                let fence = OplogFence::refused(self.owned_agent_id.agent_id(), error);
                if self.fence.set(fence.clone()).is_ok() {
                    warn!(
                        agent_id = %self.owned_agent_id,
                        expected_epoch = fence.expected_epoch.0,
                        actual_epoch = ?fence.actual_epoch.map(|epoch| epoch.0),
                        "Oplog trim fenced: the shard has a new owner, refusing further writes"
                    );
                }
                Ok(())
            }
            Err(error) => Err(format!(
                "Indexed storage operation 'drop_prefix' failed for key '{key}': {error}"
            )),
        }
    }
}
