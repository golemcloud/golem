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

impl CreateOplogConstructor {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        initial_entry: Option<OplogEntry>,
        primary: Arc<dyn OplogService>,
        service: MultiLayerOplogService,
        last_oplog_index: Option<OplogIndex>,
        fresh: bool,
        initial_worker_metadata: AgentMetadata,
        last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        shard_epoch: Option<ShardEpoch>,
    ) -> Self {
        Self {
            owned_agent_id,
            agent_mode,
            initial_entry,
            primary,
            service,
            last_oplog_index,
            fresh,
            initial_worker_metadata,
            last_known_status,
            execution_status,
            shard_epoch,
        }
    }

    /// The last index stored for the agent in any layer. A layer whose index does not read fails
    /// the open.
    async fn stored_last_index(
        &self,
        agent_mode: AgentMode,
    ) -> Result<OplogIndex, UnreadableOplog> {
        self.service
            .get_last_index(&self.owned_agent_id, agent_mode)
            .await
    }
}

#[async_trait]
impl OplogConstructor for CreateOplogConstructor {
    fn shard_epoch(&self) -> Option<ShardEpoch> {
        self.shard_epoch
    }

    async fn create_oplog(
        self,
        lifecycle: &mut OplogLifecycleGuard,
        close: Box<dyn FnOnce() + Send + Sync>,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        // A failed open caches nothing, so its slot is released at once.
        let failed = |close: Box<dyn FnOnce() + Send + Sync>,
                      error: WorkerExecutorError|
         -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
            close();
            Err(error)
        };
        let agent_mode = self.agent_mode;
        let account_id = self.initial_worker_metadata.created_by;
        let fingerprint = self.initial_worker_metadata.fingerprint;

        match agent_mode {
            AgentMode::Durable => {
                let last_oplog_index = match self.last_oplog_index {
                    Some(idx) => idx,
                    None => match self.stored_last_index(agent_mode).await {
                        Ok(idx) => idx,
                        Err(error) => return failed(close, error.into()),
                    },
                };
                let primary = if let Some(initial_entry) = self.initial_entry {
                    if self.fresh {
                        self.primary
                            .create_fresh(
                                lifecycle,
                                &self.owned_agent_id,
                                agent_mode,
                                initial_entry,
                                self.initial_worker_metadata,
                                self.last_known_status,
                                self.execution_status,
                                self.shard_epoch,
                            )
                            .await
                    } else {
                        self.primary
                            .create(
                                lifecycle,
                                &self.owned_agent_id,
                                agent_mode,
                                initial_entry,
                                self.initial_worker_metadata,
                                self.last_known_status,
                                self.execution_status,
                                self.shard_epoch,
                            )
                            .await
                    }
                } else {
                    self.primary
                        .open(
                            lifecycle,
                            &self.owned_agent_id,
                            agent_mode,
                            Some(last_oplog_index),
                            self.initial_worker_metadata,
                            self.last_known_status,
                            self.execution_status,
                            self.shard_epoch,
                        )
                        .await
                };
                let primary = match primary {
                    Ok(primary) => primary,
                    Err(error) => return failed(close, error),
                };
                MultiLayerOplog::new(
                    self.owned_agent_id,
                    agent_mode,
                    account_id,
                    primary,
                    self.service,
                    close,
                    self.shard_epoch,
                )
                .await
                .map_err(WorkerExecutorError::from)
            }
            AgentMode::Ephemeral => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

                let lower = EphemeralOplog::build_lower_layers(
                    &self.service.lower,
                    &self.owned_agent_id,
                    agent_mode,
                    account_id,
                    self.service.entry_count_limit,
                    &tx,
                    &self.service,
                    self.fresh,
                    self.shard_epoch,
                )
                .await;
                let lower = match lower {
                    Ok(lower) => lower,
                    Err(error) => return failed(close, error.into()),
                };

                // Read after the layers record the epoch, as the primary oplog reads its last index
                // after its claim: an index a caller read before can be behind by whatever an
                // older owner's writer appended in between, and this handle's first append would
                // collide with it.
                let stored_last_index = || self.stored_last_index(agent_mode);
                let last_oplog_index = match self.last_oplog_index {
                    None => stored_last_index().await,
                    Some(idx)
                        if self.initial_entry.is_none()
                            && self.shard_epoch.is_some()
                            && lower.iter().all(|layer| layer.fence().is_none()) =>
                    {
                        stored_last_index()
                            .await
                            .map(|stored| OplogIndex::from_u64(idx.as_u64().max(stored.as_u64())))
                    }
                    Some(idx) => Ok(idx),
                };
                let last_oplog_index = match last_oplog_index {
                    Ok(idx) => idx,
                    Err(error) => return failed(close, error.into()),
                };

                if let Some(initial_entry) = self.initial_entry {
                    // A refusal latches on the layer, and the ephemeral oplog built over it below
                    // reports it: the handle is finished, as a stale create of a primary oplog is.
                    match lower
                        .first()
                        .append(&[(OplogIndex::INITIAL, initial_entry)])
                        .await
                    {
                        Ok(_) | Err(OplogError::Fenced(_)) => {}
                        // Nothing of the oplog is persisted, so the create can be tried again.
                        Err(error) => {
                            return failed(
                                close,
                                WorkerExecutorError::runtime(format!(
                                    "failed to persist the initial ephemeral oplog entry of {}: {error}",
                                    self.owned_agent_id
                                )),
                            );
                        }
                    }
                }

                let transfer_fiber = new_transfer_fiber();
                self.service
                    .register_transfer(self.owned_agent_id.agent_id.clone(), &transfer_fiber);
                let (start_tx, start_rx) = tokio::sync::oneshot::channel();
                let transfer = EphemeralOplog::spawn_background_transfer(
                    self.owned_agent_id.clone(),
                    lower.clone(),
                    self.service.clone(),
                    rx,
                    start_rx,
                );
                MultiLayerOplogService::start_transfer(&transfer_fiber, start_tx, transfer).await;

                Ok(Arc::new(
                    EphemeralOplog::new(
                        self.owned_agent_id,
                        agent_mode,
                        fingerprint,
                        last_oplog_index,
                        self.service.max_operations_before_commit_ephemeral,
                        self.primary.clone(),
                        lower,
                        tx,
                        transfer_fiber,
                        self.service,
                        close,
                    )
                    .await,
                ))
            }
        }
    }
}
