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

impl MultiLayerOplogService {
    pub fn new(
        primary: Arc<dyn OplogService>,
        lower: NEVec<Arc<dyn OplogArchiveService>>,
        entry_count_limit: u64,
        max_operations_before_commit_ephemeral: u64,
    ) -> Self {
        Self {
            primary,
            lower,
            oplogs: OpenOplogs::new("multi-layer oplog"),
            transfer_fibers: Arc::new(Mutex::new(HashMap::new())),
            archive_failures: Arc::new(Mutex::new(HashMap::new())),
            entry_count_limit,
            max_operations_before_commit_ephemeral,
        }
    }

    pub(crate) fn begin_archive_attempt(
        &self,
        owned_agent_id: &OwnedAgentId,
        source: ArchiveSource,
    ) -> bool {
        self.archive_attempt_ready(owned_agent_id, source)
    }

    pub(crate) fn record_archive_failure(
        &self,
        owned_agent_id: &OwnedAgentId,
        source: ArchiveSource,
    ) {
        let mut failures = self.archive_failures.lock().unwrap();
        let key = (owned_agent_id.clone(), source);
        let failure_count = failures
            .get(&key)
            .map_or(1, |state| state.failures.saturating_add(1));
        failures.insert(
            key,
            ArchiveFailureState {
                failures: failure_count,
                retry_at: Instant::now() + archive_retry_delay(failure_count),
            },
        );
    }

    pub(crate) fn record_archive_success(
        &self,
        owned_agent_id: &OwnedAgentId,
        source: ArchiveSource,
    ) {
        self.archive_failures
            .lock()
            .unwrap()
            .remove(&(owned_agent_id.clone(), source));
    }

    pub(super) fn archive_retry_due(
        &self,
        owned_agent_id: &OwnedAgentId,
        source: ArchiveSource,
    ) -> bool {
        self.archive_failures
            .lock()
            .unwrap()
            .get(&(owned_agent_id.clone(), source))
            .is_some_and(|state| Instant::now() >= state.retry_at)
    }

    pub(super) fn archive_attempt_ready(
        &self,
        owned_agent_id: &OwnedAgentId,
        source: ArchiveSource,
    ) -> bool {
        self.archive_failures
            .lock()
            .unwrap()
            .get(&(owned_agent_id.clone(), source))
            .is_none_or(|state| Instant::now() >= state.retry_at)
    }

    pub(crate) async fn wait_until_archive_ready(
        &self,
        owned_agent_id: &OwnedAgentId,
        source: ArchiveSource,
    ) {
        loop {
            let wait = self
                .archive_failures
                .lock()
                .unwrap()
                .get(&(owned_agent_id.clone(), source))
                .and_then(|state| state.retry_at.checked_duration_since(Instant::now()));
            match wait {
                Some(wait) if !wait.is_zero() => tokio::time::sleep(wait).await,
                _ => return,
            }
        }
    }

    async fn filter_ids_existing_on_lower_layers(
        &self,
        unfiltered_ids: Vec<OwnedAgentId>,
        agent_mode: AgentMode,
        from: usize,
    ) -> Result<Vec<OwnedAgentId>, WorkerExecutorError> {
        let mut ids = Vec::new();
        for id in unfiltered_ids {
            let mut exists_in_lower = false;
            for lower_layer in &self.lower.iter().as_slice()[from..] {
                if lower_layer
                    .exists(&id, agent_mode)
                    .await
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
                {
                    exists_in_lower = true;
                    break;
                }
            }

            if !exists_in_lower {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    pub(crate) fn register_transfer(&self, agent_id: AgentId, transfer_fiber: &TransferFiber) {
        self.transfer_fibers
            .lock()
            .unwrap()
            .insert(agent_id, Arc::downgrade(transfer_fiber));
    }

    async fn abort_transfer(&self, agent_id: &AgentId) {
        let transfer_fiber = self
            .transfer_fibers
            .lock()
            .unwrap()
            .remove(agent_id)
            .and_then(|transfer_fiber| transfer_fiber.upgrade());

        if let Some(transfer_fiber) = transfer_fiber {
            Self::cancel_transfer(&transfer_fiber)
                .await
                .expect("Oplog transfer cleanup failed");
        }
    }

    pub(crate) async fn cancel_transfer(transfer_fiber: &TransferFiber) -> Result<(), String> {
        let closed = {
            let mut state = transfer_fiber.lock().unwrap();
            state.cancelled = true;
            if let Some(transfer) = &state.transfer_fiber {
                transfer.abort();
            }
            state.closed.clone()
        };
        match closed {
            Some(closed) => closed.await,
            None => Ok(()),
        }
    }

    pub(crate) fn transfer_closed(transfer_fiber: &TransferFiber) -> OplogCloseCompletion {
        transfer_fiber
            .lock()
            .unwrap()
            .closed
            .clone()
            .expect("transfer must be constructed")
    }

    pub(crate) async fn start_transfer(
        transfer_fiber: &TransferFiber,
        start: Sender<()>,
        transfer: tokio::task::JoinHandle<()>,
    ) {
        let abort = transfer.abort_handle();
        let closed = async move {
            match transfer.await {
                Ok(()) => Ok(()),
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
        .boxed()
        .shared();
        let cancelled = {
            let mut transfer_fiber = transfer_fiber.lock().unwrap();
            transfer_fiber.transfer_fiber = Some(abort.clone());
            transfer_fiber.closed = Some(closed.clone());
            transfer_fiber.cancelled
        };

        if cancelled {
            abort.abort();
            closed.await.expect("Oplog transfer cleanup failed");
        } else {
            let _ = start.send(());
        }
    }

    pub(crate) fn abort_transfer_in_drop(&self, transfer_fiber: &TransferFiber) {
        let transfer = {
            let mut transfer_fiber = transfer_fiber.lock().unwrap();
            transfer_fiber.cancelled = true;
            transfer_fiber.transfer_fiber.take()
        };

        if let Some(transfer) = transfer {
            transfer.abort();
        }
    }

    pub(crate) fn unregister_transfer(&self, agent_id: &AgentId, transfer_fiber: &TransferFiber) {
        let transfer_fiber = Arc::downgrade(transfer_fiber);
        let mut transfer_fibers = self.transfer_fibers.lock().unwrap();
        if transfer_fibers
            .get(agent_id)
            .is_some_and(|registered| registered.ptr_eq(&transfer_fiber))
        {
            transfer_fibers.remove(agent_id);
        }
    }

    #[cfg(test)]
    pub(in crate::services::oplog) fn has_registered_transfer(
        &self,
        agent_id: &AgentId,
        transfer_fiber: &TransferFiber,
    ) -> bool {
        let transfer_fiber = Arc::downgrade(transfer_fiber);
        self.transfer_fibers
            .lock()
            .unwrap()
            .get(agent_id)
            .is_some_and(|registered| registered.ptr_eq(&transfer_fiber))
    }
}

impl Clone for MultiLayerOplogService {
    fn clone(&self) -> Self {
        Self {
            primary: self.primary.clone(),
            lower: self.lower.clone(),
            oplogs: self.oplogs.clone(),
            transfer_fibers: self.transfer_fibers.clone(),
            archive_failures: self.archive_failures.clone(),
            entry_count_limit: self.entry_count_limit,
            max_operations_before_commit_ephemeral: self.max_operations_before_commit_ephemeral,
        }
    }
}

#[async_trait]
impl OplogService for MultiLayerOplogService {
    async fn lock_lifecycle(&self, agent_id: &AgentId) -> OplogLifecycleGuard {
        self.primary.lock_lifecycle(agent_id).await
    }

    fn set_stream_session_index(
        &self,
        index: Arc<crate::services::oplog::StreamSessionIndexService>,
    ) {
        self.primary.set_stream_session_index(index);
    }

    fn stream_session_index(
        &self,
    ) -> Option<Arc<crate::services::oplog::StreamSessionIndexService>> {
        self.primary.stream_session_index()
    }

    async fn create_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
        initial_worker_metadata: AgentMetadata,
    ) -> Result<Arc<dyn Oplog>, String> {
        self.primary
            .create_staged(
                owned_agent_id,
                agent_mode,
                stage_id,
                initial_worker_metadata,
            )
            .await
    }

    async fn staged_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<bool, String> {
        self.primary
            .staged_exists(owned_agent_id, agent_mode, stage_id)
            .await
    }

    async fn publish_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        publication: StagePublication,
        expected_last_index: OplogIndex,
    ) -> Result<bool, String> {
        self.primary
            .publish_staged(owned_agent_id, agent_mode, publication, expected_last_index)
            .await
    }

    async fn discard_staged(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        stage_id: uuid::Uuid,
    ) -> Result<(), String> {
        self.primary
            .discard_staged(owned_agent_id, agent_mode, stage_id)
            .await
    }

    async fn create(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    Some(initial_entry),
                    self.primary.clone(),
                    self.clone(),
                    Some(OplogIndex::INITIAL),
                    false,
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                    shard_epoch,
                ),
            )
            .await
    }

    async fn create_fresh(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: OplogEntry,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    Some(initial_entry),
                    self.primary.clone(),
                    self.clone(),
                    Some(OplogIndex::INITIAL),
                    true,
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                    shard_epoch,
                ),
            )
            .await
    }

    async fn open(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: Option<OplogIndex>,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        self.oplogs
            .get_or_open(
                lifecycle,
                &owned_agent_id.agent_id,
                CreateOplogConstructor::new(
                    owned_agent_id.clone(),
                    agent_mode,
                    None,
                    self.primary.clone(),
                    self.clone(),
                    last_oplog_index,
                    false,
                    initial_worker_metadata,
                    last_known_status,
                    execution_status,
                    shard_epoch,
                ),
            )
            .await
    }

    async fn get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        let mut result = self
            .primary
            .get_last_index(owned_agent_id, agent_mode)
            .await?;
        if result == OplogIndex::NONE {
            for (level, layer) in self.lower.iter().enumerate() {
                let idx = layer
                    .get_last_index(owned_agent_id, agent_mode)
                    .await
                    .map_err(|error| unreadable_archive(owned_agent_id, level, error))?;
                if idx != OplogIndex::NONE {
                    result = idx;
                    break;
                }
            }
        }
        Ok(result)
    }

    async fn try_get_last_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, String> {
        let mut result = self
            .primary
            .try_get_last_index(owned_agent_id, agent_mode)
            .await?;
        if result == OplogIndex::NONE {
            for layer in &self.lower {
                let idx = layer
                    .get_last_index(owned_agent_id, agent_mode)
                    .await
                    .map_err(|error| error.to_string())?;
                if idx != OplogIndex::NONE {
                    result = idx;
                    break;
                }
            }
        }
        Ok(result)
    }

    async fn assert_owning_epoch(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: ShardEpoch,
    ) -> Result<(), OplogError> {
        self.primary
            .assert_owning_epoch(owned_agent_id, agent_mode, expected_epoch)
            .await
    }

    async fn delete(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        expected_epoch: Option<ShardEpoch>,
    ) -> Result<(), OplogError> {
        lifecycle.assert_agent(&owned_agent_id.agent_id);
        self.abort_transfer(&owned_agent_id.agent_id).await;
        if let Err(error) = self
            .primary
            .delete(lifecycle, owned_agent_id, agent_mode, expected_epoch)
            .await
        {
            crate::metrics::oplog::record_archive_maintenance_failure("delete");
            warn!(agent_id = %owned_agent_id, error = %error, "Failed to delete primary oplog storage; deletion remains fenced for retry");
            return Err(error);
        }
        for (level, layer) in self.lower.iter().enumerate() {
            if let Err(error) = layer.delete(owned_agent_id, agent_mode).await {
                crate::metrics::oplog::record_archive_maintenance_failure("delete");
                warn!(agent_id = %owned_agent_id, level, error = %error, "Failed to delete archived oplog storage; deletion remains fenced for retry");
                return Err(OplogError::Maintenance(error.to_string()));
            }
        }
        self.archive_failures
            .lock()
            .unwrap()
            .retain(|(failed_agent_id, _), _| failed_agent_id != owned_agent_id);
        Ok(())
    }

    async fn read_exact(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        let unreadable = |error| UnreadableOplog::new(&owned_agent_id.agent_id, error);
        let mut read = OplogRead::new(idx, n).map_err(unreadable)?;

        if let Some((start, count)) = read.next_range() {
            let entries = self
                .primary
                .read_source(owned_agent_id, agent_mode, start, count)
                .await?;
            read.add_source(OplogReadSource::Primary, entries)
                .map_err(unreadable)?;
        }

        for (level, layer) in self.lower.iter().enumerate() {
            if let Some((start, count)) = read.next_range() {
                let entries = layer
                    .read_source(owned_agent_id, agent_mode, start, count)
                    .await
                    .map_err(unreadable)?;
                read.add_source(OplogReadSource::Archive(level), entries)
                    .map_err(unreadable)?;
            } else {
                break;
            }
        }

        read.finish().map_err(unreadable)
    }

    async fn read_source(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        idx: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        let mut result = self
            .primary
            .read_source(owned_agent_id, agent_mode, idx, n)
            .await?;
        for layer in &self.lower {
            if result.len() >= n as usize {
                break;
            }
            let entries = layer
                .read_source(owned_agent_id, agent_mode, idx, n)
                .await
                .map_err(|error| UnreadableOplog::new(&owned_agent_id.agent_id, error))?;
            for (index, entry) in entries {
                result.entry(index).or_insert(entry);
            }
        }
        Ok(result)
    }

    async fn exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, UnreadableOplog> {
        if self.primary.exists(owned_agent_id, agent_mode).await? {
            return Ok(true);
        }

        for (level, layer) in self.lower.iter().enumerate() {
            if agent_mode == AgentMode::Ephemeral {
                crate::metrics::ephemeral::record_lower_oplog_existence_read();
            }
            if layer
                .exists(owned_agent_id, agent_mode)
                .await
                .map_err(|error| unreadable_archive(owned_agent_id, level, error))?
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn try_exists(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<bool, String> {
        if self.primary.try_exists(owned_agent_id, agent_mode).await? {
            return Ok(true);
        }
        for layer in &self.lower {
            if agent_mode == AgentMode::Ephemeral {
                crate::metrics::ephemeral::record_lower_oplog_existence_read();
            }
            if layer
                .exists(owned_agent_id, agent_mode)
                .await
                .map_err(|error| error.to_string())?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn scan_for_component(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        let state = decode_scan_cursor(&cursor, modes)?;
        let layer = state.layer;
        if layer > self.lower.len().get() {
            return Err(WorkerExecutorError::invalid_request(format!(
                "Invalid oplog layer in scan cursor: {layer}"
            )));
        }
        let active_mode = state.mode;

        match layer {
            0 => {
                let (new_cursor, unfiltered_ids) = self
                    .primary
                    .scan_for_component(environment_id, component_id, modes, cursor.clone(), count)
                    .await?;
                let ids = self
                    .filter_ids_existing_on_lower_layers(unfiltered_ids, active_mode, 0)
                    .await?;

                if new_cursor.is_finished() {
                    Ok((first_scan_cursor(1, modes)?, ids))
                } else {
                    Ok((new_cursor, ids))
                }
            }
            layer if layer <= self.lower.len().get() => {
                let (new_cursor, unfiltered_ids) = self.lower[layer - 1]
                    .scan_for_component(environment_id, component_id, modes, cursor.clone(), count)
                    .await?;

                let ids = self
                    .filter_ids_existing_on_lower_layers(unfiltered_ids, active_mode, layer)
                    .await?;
                if new_cursor.is_finished() && (layer + 1) <= self.lower.len().get() {
                    Ok((first_scan_cursor(layer + 1, modes)?, ids))
                } else if new_cursor.is_finished() {
                    Ok((ScanCursor::default(), ids))
                } else {
                    Ok((new_cursor, ids))
                }
            }
            _ => unreachable!(),
        }
    }

    async fn upload_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        self.primary
            .upload_raw_payload(owned_agent_id, agent_mode, data)
            .await
    }

    async fn download_raw_payload(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.primary
            .download_raw_payload(owned_agent_id, agent_mode, payload_id, md5_hash)
            .await
    }

    async fn download_raw_payload_classified(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.primary
            .download_raw_payload_classified(owned_agent_id, agent_mode, payload_id, md5_hash)
            .await
    }
}
