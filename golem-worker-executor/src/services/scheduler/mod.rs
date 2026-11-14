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

use crate::metrics::oplog::record_scheduled_archive;
use crate::metrics::promises::record_scheduled_promise_completed;
use crate::services::oplog::{ArchiveWait, OplogService};
use crate::services::shard::ShardService;
use crate::services::worker_activator::{ExactExistingDelivery, WorkerActivator};
use crate::storage::scheduler::{ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError};
use crate::worker::INACTIVE_EPHEMERAL_AGENT_ERROR;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::StreamExt;
use golem_common::model::InvocationId;
use golem_common::model::RetryConfig;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentFingerprint, OwnedAgentId, ScheduleId, ScheduledAction, ShardId};
use golem_common::retries::get_delay;
use golem_common::serialization::serialize;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
use std::future::Future;
use std::ops::{Add, Deref};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, debug_span, error, info, info_span, warn};

#[async_trait]
pub trait SchedulerService: Send + Sync {
    async fn schedule(&self, time: DateTime<Utc>, action: ScheduledAction) -> ScheduleId;

    async fn schedule_with_id(
        &self,
        schedule_id: ScheduleId,
        time: DateTime<Utc>,
        action: ScheduledAction,
    ) -> ScheduleId;

    async fn cancel(&self, id: ScheduleId);
}

/// A lighter trait than `WorkerActivator` that only provides the required functionality
/// for `SchedulerServiceDefault`, making it easier to test (by being independent of `WorkerCtx`).
#[async_trait]
pub trait SchedulerWorkerAccess {
    async fn active_worker_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint>;

    /// See [`WorkerActivator::worker_is_cached`].
    async fn worker_is_cached(&self, owned_agent_id: &OwnedAgentId) -> bool;

    async fn activate_worker(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError>;
    async fn archive_oplog(
        &self,
        owned_agent_id: &OwnedAgentId,
        last_oplog_index: OplogIndex,
        wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError>;

    async fn expire_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_agent_fingerprint: AgentFingerprint,
        public_session_id: String,
        session_key: InvocationId,
        expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError>;

    async fn enqueue_exact_existing(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_worker_fingerprint: AgentFingerprint,
        delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError>;
}

#[async_trait]
impl<Ctx: WorkerCtx> SchedulerWorkerAccess for Arc<dyn WorkerActivator<Ctx>> {
    async fn active_worker_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        self.deref().active_worker_fingerprint(owned_agent_id).await
    }

    async fn worker_is_cached(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.deref().worker_is_cached(owned_agent_id).await
    }

    async fn activate_worker(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        self.deref().activate_worker(owned_agent_id).await
    }

    async fn archive_oplog(
        &self,
        owned_agent_id: &OwnedAgentId,
        last_oplog_index: OplogIndex,
        wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        self.deref()
            .archive_oplog(owned_agent_id, last_oplog_index, wait)
            .await
    }

    async fn expire_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_agent_fingerprint: AgentFingerprint,
        public_session_id: String,
        session_key: InvocationId,
        expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        self.deref()
            .expire_durable_stream_session(
                owned_agent_id,
                target_agent_fingerprint,
                public_session_id,
                session_key,
                expected_deadline_millis,
            )
            .await
    }

    async fn enqueue_exact_existing(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_worker_fingerprint: AgentFingerprint,
        delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError> {
        self.deref()
            .enqueue_exact_existing(owned_agent_id, target_worker_fingerprint, delivery)
            .await
    }
}

#[derive(Clone)]
pub struct SchedulerServiceDefault {
    scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
    background_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    shard_service: Arc<dyn ShardService>,
    worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    oplog_service: Arc<dyn OplogService>,
    claim_batch_size: u32,
    lease_ttl: Duration,
    max_batches_per_tick: u32,
    storage_retry: RetryConfig,
    max_concurrent_action_processing: usize,
}

impl SchedulerServiceDefault {
    pub fn new(
        scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService>,
        worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
        oplog_service: Arc<dyn OplogService>,
        process_interval: Duration,
        claim_batch_size: u32,
        lease_ttl: Duration,
        max_batches_per_tick: u32,
        storage_retry: RetryConfig,
        max_concurrent_action_processing: u32,
        shutdown_token: CancellationToken,
    ) -> Arc<Self> {
        let svc = Self {
            scheduler_storage,
            background_handle: Arc::new(Mutex::new(None)),
            shard_service,
            oplog_service,
            worker_access,
            claim_batch_size,
            lease_ttl,
            max_batches_per_tick,
            storage_retry,
            max_concurrent_action_processing: max_concurrent_action_processing.max(1) as usize,
        };
        let svc = Arc::new(svc);
        let background_handle = {
            let svc_weak = Arc::downgrade(&svc);
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown_token.cancelled() => {
                            info!("Shutdown requested, stopping scheduler background loop");
                            break;
                        }
                        _ = tokio::time::sleep(process_interval) => {}
                    }
                    let svc = match svc_weak.upgrade() {
                        Some(s) => s,
                        None => {
                            info!("Scheduler service dropped, stopping background loop");
                            break;
                        }
                    };
                    if svc.shard_service.is_ready() {
                        // A failed tick is logged inside `process`, where its span is
                        // still open.
                        let _ = svc.process(Utc::now()).await;
                    } else {
                        warn!("Skipping schedule, shard service is not ready")
                    }
                }
            })
        };
        *svc.background_handle.lock().unwrap() = Some(background_handle);

        svc
    }

    /// Runs a scheduler storage operation, retrying transient errors (such as
    /// connection pool exhaustion) according to `storage_retry`. Panics on a
    /// non-transient error, or after the configured retries are exhausted.
    async fn retry_storage_op<T, F, Fut>(&self, op_name: &str, target: &str, mut op: F) -> T
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, SchedulerStorageError>>,
    {
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            match op().await {
                Ok(val) => return val,
                Err(SchedulerStorageError::Transient(msg)) => {
                    if let Some(delay) = get_delay(&self.storage_retry, attempts) {
                        warn!(
                            op = op_name,
                            attempt = attempts,
                            delay_ms = delay.as_millis() as u64,
                            "Transient scheduler storage error for {target}, retrying: {msg}"
                        );
                        tokio::time::sleep(delay).await;
                    } else {
                        panic!(
                            "scheduler storage operation '{op_name}' failed for {target} after {attempts} attempts: Transient storage error: {msg}"
                        );
                    }
                }
                Err(SchedulerStorageError::Other(msg)) => {
                    panic!("scheduler storage operation '{op_name}' failed for {target}: {msg}");
                }
            }
        }
    }

    /// Performs one scheduler tick, spanned per tick rather than per loop: a span
    /// wrapping the loop would never close, so it would never be exported.
    async fn process(&self, now: DateTime<Utc>) -> Result<(), String> {
        async {
            let result = async {
                let tick_start = std::time::Instant::now();
                let assignment = self
                    .shard_service
                    .current_assignment()
                    .map_err(|err| err.to_string())?;

                for _ in 0..self.max_batches_per_tick {
                    let claimed = self
                        .scheduler_storage
                        .claim_due(now, &assignment, self.claim_batch_size, self.lease_ttl)
                        .instrument(debug_span!("scheduler_storage_claim_due"))
                        .await?;

                    let claimed_count = claimed.len();
                    crate::metrics::scheduler::set_scheduler_queue_depth(claimed_count);
                    if claimed.is_empty() {
                        break;
                    }

                    self.process_claimed_actions(claimed, now).await?;

                    if claimed_count < self.claim_batch_size as usize {
                        break;
                    }
                }

                match self
                    .scheduler_storage
                    .count_due(Utc::now(), &assignment)
                    .instrument(debug_span!("scheduler_storage_count_due"))
                    .await
                {
                    Ok(backlog) => {
                        crate::metrics::scheduler::set_scheduler_due_action_backlog_after_tick(
                            backlog,
                        )
                    }
                    Err(error) => {
                        warn!(%error, "Failed to count due scheduled actions after tick")
                    }
                }

                crate::metrics::scheduler::record_scheduler_tick_duration(tick_start.elapsed());
                Ok(())
            }
            .await;
            // Logged inside the span, so a failed tick is visible in the trace and
            // not only in the logs.
            if let Err(error) = &result {
                error!(error, "Scheduler tick failed");
            }
            result
        }
        .instrument(info_span!("scheduler_tick"))
        .await
    }

    async fn process_claimed_actions(
        &self,
        claimed: Vec<ClaimedScheduledAction>,
        now: DateTime<Utc>,
    ) -> Result<(), SchedulerStorageError> {
        let mut actions_by_agent: HashMap<OwnedAgentId, Vec<ClaimedScheduledAction>> =
            HashMap::new();
        for action in claimed {
            actions_by_agent
                .entry(action.action.owned_agent_id())
                .or_default()
                .push(action);
        }

        let results = futures::stream::iter(actions_by_agent.into_values())
            .map(|actions| async move {
                // A durable agent's scheduled actions must retain claim order.
                for action in actions {
                    self.process_and_ack_claimed_action(action, now).await?;
                }
                Ok::<(), SchedulerStorageError>(())
            })
            .buffer_unordered(self.max_concurrent_action_processing)
            .collect::<Vec<_>>()
            .await;

        for result in results {
            result?;
        }

        Ok(())
    }

    async fn process_and_ack_claimed_action(
        &self,
        claimed_action: ClaimedScheduledAction,
        now: DateTime<Utc>,
    ) -> Result<(), SchedulerStorageError> {
        let action_kind = crate::metrics::scheduler::action_kind_label(&claimed_action.action);
        let action_start_time = Utc::now();
        let lag = action_start_time.signed_duration_since(claimed_action.due_at);
        let lag_secs = lag.num_milliseconds().max(0) as f64 / 1000.0;
        crate::metrics::scheduler::record_scheduled_action_lag(Duration::from_secs_f64(lag_secs));

        let schedule_id = claimed_action.schedule_id;
        let lease_owner = claimed_action.lease_owner;
        let action_start = std::time::Instant::now();
        let processed = self.process_claimed_action(claimed_action, now).await;
        crate::metrics::scheduler::record_scheduled_action_processing(
            action_kind,
            action_start.elapsed(),
        );
        if !processed {
            return Ok(());
        }

        let acked = self
            .scheduler_storage
            .ack(&schedule_id, lease_owner)
            .await
            .map_err(|error| {
                warn!(
                    schedule_id = %schedule_id,
                    lease_owner = %lease_owner,
                    error = %error,
                    "Failed to acknowledge scheduled action; aborting scheduler tick"
                );
                error
            })?;
        if !acked {
            warn!(
                schedule_id = %schedule_id,
                lease_owner = %lease_owner,
                "Failed to acknowledge scheduled action because the lease was lost"
            );
        }
        Ok(())
    }

    async fn with_lease_renewal<T, F>(
        &self,
        schedule_id: ScheduleId,
        lease_owner: uuid::Uuid,
        operation: F,
    ) -> Result<T, String>
    where
        F: Future<Output = T>,
    {
        let renewal_interval = (self.lease_ttl / 3).max(Duration::from_millis(1));
        tokio::pin!(operation);

        loop {
            tokio::select! {
                result = &mut operation => {
                    return Ok(result);
                }
                _ = tokio::time::sleep(renewal_interval) => {
                    let lease_until = Utc::now().add(self.lease_ttl);
                    let renewed = self.scheduler_storage
                        .extend_lease(
                            &schedule_id,
                            lease_owner,
                            lease_until,
                        )
                        .await?;

                    if !renewed {
                        return Err(format!(
                            "lease for scheduled action {} was lost before processing completed",
                            schedule_id
                        ));
                    }
                }
            }
        }
    }

    async fn process_claimed_action(
        &self,
        claimed_action: ClaimedScheduledAction,
        now: DateTime<Utc>,
    ) -> bool {
        let schedule_id = claimed_action.schedule_id;
        let lease_owner = claimed_action.lease_owner;
        match claimed_action.action {
            ScheduledAction::CompletePromise {
                account_id: _,
                promise_id,
                environment_id,
                target_worker_fingerprint,
            } => {
                let owned_agent_id = OwnedAgentId::new(environment_id, &promise_id.agent_id);
                let result = self
                    .worker_access
                    .enqueue_exact_existing(
                        &owned_agent_id,
                        target_worker_fingerprint,
                        ExactExistingDelivery::CompletePromise(promise_id),
                    )
                    .await;
                match result {
                    Ok(true) => {
                        record_scheduled_promise_completed();
                        true
                    }
                    Ok(false) => {
                        info!(agent_id = %owned_agent_id, "Dropping stale scheduled promise completion");
                        true
                    }
                    Err(error) => {
                        error!(agent_id = %owned_agent_id, error = %error, "Failed to deliver scheduled promise completion");
                        false
                    }
                }
            }
            ScheduledAction::ArchiveOplog {
                account_id,
                owned_agent_id,
                agent_mode,
                last_oplog_index,
                next_after,
            } => {
                debug!("Running scheduled archive oplog for {account_id}/{owned_agent_id}");

                let exists = match self
                    .oplog_service
                    .try_exists(&owned_agent_id, agent_mode)
                    .await
                {
                    Ok(exists) => exists,
                    Err(error) => {
                        crate::metrics::oplog::record_archive_maintenance_failure(
                            "scheduled_exists",
                        );
                        error!(agent_id = %owned_agent_id, error = %error, "Failed to check oplog before archival");
                        return false;
                    }
                };
                if exists {
                    let start = Instant::now();
                    let archive_result = self
                        .with_lease_renewal(
                            schedule_id,
                            lease_owner,
                            self.worker_access.archive_oplog(
                                &owned_agent_id,
                                last_oplog_index,
                                ArchiveWait::Queued,
                            ),
                        )
                        .await;
                    match archive_result {
                        Ok(Ok(Some(more))) => {
                            record_scheduled_archive(start.elapsed(), more);
                            if more {
                                self.schedule(
                                    now.add(next_after),
                                    ScheduledAction::ArchiveOplog {
                                        account_id,
                                        owned_agent_id,
                                        agent_mode,
                                        last_oplog_index,
                                        next_after,
                                    },
                                )
                                .await;
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(error)) => {
                            error!(agent_id = %owned_agent_id, error = %error, "Failed to archive worker oplog");
                            return false;
                        }
                        Err(error) => {
                            warn!(schedule_id = %schedule_id, agent_id = %owned_agent_id, error = %error,
                                "Stopped scheduled oplog archival because lease renewal failed");
                            return false;
                        }
                    }
                }
                true
            }
            ScheduledAction::WakeInvocation {
                owned_agent_id,
                idempotency_key,
                target_worker_fingerprint,
            } => {
                match self
                    .worker_access
                    .enqueue_exact_existing(
                        &owned_agent_id,
                        target_worker_fingerprint,
                        ExactExistingDelivery::WakeInvocation(idempotency_key),
                    )
                    .await
                {
                    Ok(true) => true,
                    Ok(false) => {
                        info!(
                            agent_id = %owned_agent_id,
                            "Dropping stale scheduled invocation"
                        );
                        true
                    }
                    // An ephemeral agent that began executing never starts again.
                    Err(WorkerExecutorError::InvalidRequest { details })
                        if details == INACTIVE_EPHEMERAL_AGENT_ERROR =>
                    {
                        info!(
                            agent_id = %owned_agent_id,
                            "Dropping scheduled invocation of an inactive ephemeral agent"
                        );
                        true
                    }
                    Err(error) => {
                        error!(
                            agent_id = %owned_agent_id,
                            error = %error,
                            "Failed to enqueue scheduled invocation"
                        );
                        false
                    }
                }
            }
            ScheduledAction::ExpireDurableStreamSession {
                owned_agent_id,
                target_agent_fingerprint,
                public_session_id,
                session_key,
                expected_deadline_millis,
            } => {
                match self
                    .worker_access
                    .expire_durable_stream_session(
                        &owned_agent_id,
                        target_agent_fingerprint,
                        public_session_id.clone(),
                        session_key,
                        expected_deadline_millis,
                    )
                    .await
                {
                    Ok(()) => true,
                    Err(error) => {
                        error!(
                            agent_id = %owned_agent_id,
                            public_session_id = %public_session_id,
                            error = %error,
                            "Failed to expire durable stream session"
                        );
                        false
                    }
                }
            }
            ScheduledAction::Resume {
                agent_created_by: _,
                owned_agent_id,
                target_worker_fingerprint,
            } => {
                match self
                    .worker_access
                    .enqueue_exact_existing(
                        &owned_agent_id,
                        target_worker_fingerprint,
                        ExactExistingDelivery::Resume,
                    )
                    .await
                {
                    Ok(true) => true,
                    Ok(false) => {
                        info!(agent_id = %owned_agent_id, "Dropping stale scheduled resume");
                        true
                    }
                    Err(error) => {
                        error!(agent_id = %owned_agent_id, error = %error, "Failed to resume worker");
                        false
                    }
                }
            }
        }
    }
}

impl Drop for SchedulerServiceDefault {
    fn drop(&mut self) {
        if let Some(handle) = self.background_handle.lock().unwrap().take() {
            handle.abort();
        }
    }
}

#[async_trait]
impl SchedulerService for SchedulerServiceDefault {
    async fn schedule(&self, time: DateTime<Utc>, action: ScheduledAction) -> ScheduleId {
        self.schedule_with_id(ScheduleId::fresh(), time, action)
            .await
    }

    async fn schedule_with_id(
        &self,
        schedule_id: ScheduleId,
        time: DateTime<Utc>,
        action: ScheduledAction,
    ) -> ScheduleId {
        let assignment = self.shard_service.current_assignment().unwrap_or_else(|err| {
            panic!("failed to read current shard assignment while scheduling action {action}: {err}")
        });
        let routing_hash = ShardId::hash_agent_id(&action.owned_agent_id().agent_id);
        let shard_id = ShardId::from_routing_hash(routing_hash, assignment.number_of_shards);

        let serialized = serialize(&action).unwrap_or_else(|error| {
            panic!("failed to serialize scheduled action {action}: {error}")
        });
        crate::metrics::scheduler::record_scheduled_action_size(
            crate::metrics::scheduler::action_kind_label(&action),
            serialized.len(),
        );

        self.retry_storage_op("insert", &format!("action {action}"), || {
            self.scheduler_storage
                .insert(schedule_id, time, shard_id, &serialized)
        })
        .await;
        schedule_id
    }

    async fn cancel(&self, id: ScheduleId) {
        self.retry_storage_op("cancel", &format!("schedule {id}"), || {
            self.scheduler_storage.cancel(&id)
        })
        .await;
    }
}

#[cfg(test)]
mod tests;
