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
use crate::services::HasOplog;
use crate::services::oplog::{EphemeralOplog, MultiLayerOplog, Oplog, OplogService};
use crate::services::promise::PromiseService;
use crate::services::shard::ShardService;
use crate::services::worker::WorkerService;
use crate::services::worker_activator::WorkerActivator;
use crate::storage::scheduler::{ClaimedScheduledAction, SchedulerStorage};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::model::agent::Principal;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{
    AgentFingerprint, AgentInvocation, OwnedAgentId, ScheduleId, ScheduledAction, ShardId,
};
use golem_common::serialization::serialize;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::future::Future;
use std::ops::{Add, Deref};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Level, debug, error, info, span, warn};

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

    async fn activate_worker(&self, owned_agent_id: &OwnedAgentId);
    async fn open_oplog(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError>;

    // enqueue an invocation to the worker
    async fn enqueue_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError>;
}

#[async_trait]
impl<Ctx: WorkerCtx> SchedulerWorkerAccess for Arc<dyn WorkerActivator<Ctx>> {
    async fn active_worker_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        self.deref().active_worker_fingerprint(owned_agent_id).await
    }

    async fn activate_worker(&self, owned_agent_id: &OwnedAgentId) {
        self.deref().activate_worker(owned_agent_id).await;
    }

    async fn open_oplog(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
        let worker = self
            .get_or_create_suspended(
                owned_agent_id,
                None,
                Vec::new(),
                None,
                None,
                &InvocationContextStack::fresh(),
                Principal::anonymous(),
            )
            .await?;
        Ok(worker.oplog())
    }

    async fn enqueue_invocation(
        &self,
        owned_agent_id: &OwnedAgentId,
        invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        let worker = self
            .get_or_create_suspended(
                owned_agent_id,
                None,
                Vec::new(),
                None,
                None,
                &InvocationContextStack::fresh(),
                Principal::anonymous(),
            )
            .await?;

        worker.clone().invoke(invocation).await?;

        Worker::start_if_needed(worker).await?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct SchedulerServiceDefault {
    scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
    background_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    shard_service: Arc<dyn ShardService>,
    promise_service: Arc<dyn PromiseService>,
    worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
    oplog_service: Arc<dyn OplogService>,
    worker_service: Arc<dyn WorkerService>,
    claim_batch_size: u32,
    lease_ttl: Duration,
    max_batches_per_tick: u32,
}

impl SchedulerServiceDefault {
    pub fn new(
        scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
        shard_service: Arc<dyn ShardService>,
        promise_service: Arc<dyn PromiseService>,
        worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
        oplog_service: Arc<dyn OplogService>,
        worker_service: Arc<dyn WorkerService>,
        process_interval: Duration,
        claim_batch_size: u32,
        lease_ttl: Duration,
        max_batches_per_tick: u32,
        shutdown_token: CancellationToken,
    ) -> Arc<Self> {
        let svc = Self {
            scheduler_storage,
            background_handle: Arc::new(Mutex::new(None)),
            shard_service,
            promise_service,
            oplog_service,
            worker_service,
            worker_access,
            claim_batch_size,
            lease_ttl,
            max_batches_per_tick,
        };
        let svc = Arc::new(svc);
        let background_handle = {
            let svc_weak = Arc::downgrade(&svc);
            tokio::spawn(
                async move {
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
                            let r = svc.process(Utc::now()).await;
                            if let Err(err) = r {
                                error!(err, "Error in scheduler background task");
                            }
                        } else {
                            warn!("Skipping schedule, shard service is not ready")
                        }
                    }
                }
                .instrument(span!(parent: None, Level::INFO, "Scheduler loop")),
            )
        };
        *svc.background_handle.lock().unwrap() = Some(background_handle);

        svc
    }

    async fn process(&self, now: DateTime<Utc>) -> Result<(), String> {
        let tick_start = std::time::Instant::now();
        let assignment = self
            .shard_service
            .current_assignment()
            .map_err(|err| err.to_string())?;

        for _ in 0..self.max_batches_per_tick {
            let claimed = self
                .scheduler_storage
                .claim_due(now, &assignment, self.claim_batch_size, self.lease_ttl)
                .await?;

            let claimed_count = claimed.len();
            if claimed.is_empty() {
                break;
            }

            crate::metrics::scheduler::set_scheduler_queue_depth(claimed_count);

            // ! Do not exit early from this loop because of failed actions, as it will cause all other actions to be skipped.
            // ! Retryable failures are left unacknowledged and retried after lease expiry.
            for claimed_action in claimed {
                // Observe the lag between scheduled_at (due_at) and actual fire time.
                let lag = now.signed_duration_since(claimed_action.due_at);
                let lag_secs = lag.num_milliseconds().max(0) as f64 / 1000.0;
                crate::metrics::scheduler::record_scheduled_action_lag(Duration::from_secs_f64(
                    lag_secs,
                ));

                if self
                    .process_claimed_action(claimed_action.clone(), now)
                    .await
                {
                    let acked = self
                        .scheduler_storage
                        .ack(&claimed_action.schedule_id, claimed_action.lease_owner)
                        .await?;
                    if !acked {
                        warn!(
                            schedule_id = %claimed_action.schedule_id,
                            lease_owner = %claimed_action.lease_owner,
                            "Failed to acknowledge scheduled action because the lease was lost"
                        );
                    }
                }
            }

            if claimed_count < self.claim_batch_size as usize {
                break;
            }
        }

        crate::metrics::scheduler::record_scheduler_tick_duration(tick_start.elapsed());
        Ok(())
    }

    async fn with_lease_renewal<T, F>(
        &self,
        claimed_action: &ClaimedScheduledAction,
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
                            &claimed_action.schedule_id,
                            claimed_action.lease_owner,
                            lease_until,
                        )
                        .await?;

                    if !renewed {
                        return Err(format!(
                            "lease for scheduled action {} was lost before processing completed",
                            claimed_action.schedule_id
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
        match claimed_action.action.clone() {
            ScheduledAction::CompletePromise {
                account_id: _,
                promise_id,
                environment_id,
            } => {
                let owned_agent_id = OwnedAgentId::new(environment_id, &promise_id.agent_id);

                let result = self
                    .promise_service
                    .complete(promise_id.clone(), vec![])
                    .await;

                // TODO: We probably need more error handling here as not completing a promise that is expected to complete can lead to deadlocks.
                match result {
                    Ok(_) => {
                        // activate worker so it starts processing the newly completed promises
                        // TODO: this is probably redundant with the wakeup in PromiseService. check and fix
                        {
                            let span = span!(
                                Level::INFO,
                                "scheduler",
                                agent_id = owned_agent_id.agent_id.to_string()
                            );

                            self.worker_access
                                .activate_worker(&owned_agent_id)
                                .instrument(span)
                                .await;
                        }

                        record_scheduled_promise_completed();
                        true
                    }
                    Err(e) => {
                        error!(
                            agent_id = owned_agent_id.to_string(),
                            promise_id = promise_id.to_string(),
                            "Failed to complete promise: {e}"
                        );
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

                if self.oplog_service.exists(&owned_agent_id, agent_mode).await {
                    let current_last_index = self
                        .oplog_service
                        .get_last_index(&owned_agent_id, agent_mode)
                        .await;
                    if current_last_index == last_oplog_index {
                        // Need to create the `Worker` instance to avoid race conditions
                        match self.worker_access.open_oplog(&owned_agent_id).await {
                            Ok(oplog) => {
                                let start = Instant::now();
                                let archive_result = self
                                    .with_lease_renewal(&claimed_action, async {
                                        match MultiLayerOplog::try_archive(&oplog).await {
                                            Some(r) => Some(r),
                                            None => EphemeralOplog::try_archive(&oplog).await,
                                        }
                                    })
                                    .await;
                                let archive_result = match archive_result {
                                    Ok(result) => result,
                                    Err(error) => {
                                        warn!(
                                            schedule_id = %claimed_action.schedule_id,
                                            agent_id = owned_agent_id.to_string(),
                                            "Stopped scheduled oplog archival because lease renewal failed: {error}"
                                        );
                                        return false;
                                    }
                                };
                                if let Some(more) = archive_result {
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
                                    } else {
                                        info!(
                                            agent_id = owned_agent_id.to_string(),
                                            "Deleting cached status of fully archived worker"
                                        );
                                        // The oplog is fully archived, so we can also delete the cached worker status
                                        self.worker_service
                                            .remove_cached_status(&owned_agent_id)
                                            .await;
                                    }
                                }
                            }
                            Err(error) => {
                                error!(
                                    agent_id = owned_agent_id.to_string(),
                                    "Failed to activate worker for archiving: {error}"
                                );
                                return false;
                            }
                        }
                    }

                    // TODO: metrics
                }
                true
            }
            ScheduledAction::Invoke {
                account_id: _,
                owned_agent_id,
                invocation,
                target_worker_fingerprint,
            } => {
                // A mismatch means the original worker was deleted and recreated — drop the stale
                // invocation silently.
                let stale = match self
                    .worker_access
                    .active_worker_fingerprint(&owned_agent_id)
                    .await
                {
                    Some(fingerprint) => fingerprint != target_worker_fingerprint,
                    None => match self.worker_service.get(&owned_agent_id).await {
                        Some(meta) => {
                            meta.initial_worker_metadata.fingerprint != target_worker_fingerprint
                        }
                        None => true,
                    },
                };

                if stale {
                    info!(
                        agent_id = owned_agent_id.to_string(),
                        "Dropping stale scheduled invocation: target worker was deleted and recreated"
                    );
                    true
                } else {
                    // We don't really care that it completes here, but it needs to be persisted in the invocation queue.
                    let result = self
                        .worker_access
                        .enqueue_invocation(&owned_agent_id, *invocation)
                        .await;

                    if let Err(e) = result {
                        error!(
                            agent_id = owned_agent_id.to_string(),
                            "Failed to invoke worker with scheduled invocation: {e}"
                        );
                        false
                    } else {
                        true
                    }
                }
            }
            ScheduledAction::Resume {
                agent_created_by: _,
                owned_agent_id,
            } => {
                self.worker_access.activate_worker(&owned_agent_id).await;
                true
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

        // Observe the serialized size of the action before inserting.
        if let Ok(serialized) = serialize(&action) {
            crate::metrics::scheduler::record_scheduled_action_size(
                crate::metrics::scheduler::action_kind_label(&action),
                serialized.len(),
            );
        }

        self.scheduler_storage
            .insert(schedule_id, time, shard_id, &action)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to add schedule for action {action} in scheduler storage: {err}")
            });
        schedule_id
    }

    async fn cancel(&self, id: ScheduleId) {
        self.scheduler_storage
            .cancel(&id)
            .await
            .unwrap_or_else(|err| {
                panic!("failed to remove schedule {id} from scheduler storage: {err}")
            });
    }
}

#[cfg(test)]
mod tests {
    use crate::services::oplog::{Oplog, OplogService, PrimaryOplogService};
    use crate::services::promise::PromiseServiceMock;
    use crate::services::scheduler::{
        SchedulerService, SchedulerServiceDefault, SchedulerWorkerAccess,
    };
    use crate::services::shard::{ShardService, ShardServiceDefault};
    use crate::services::worker::{GetWorkerMetadataResult, WorkerService};
    use crate::storage::indexed::memory::InMemoryIndexedStorage;
    use crate::storage::scheduler::SchedulerStorage;
    use crate::storage::scheduler::memory::InMemorySchedulerStorage;
    use async_trait::async_trait;
    use chrono::DateTime;
    use golem_common::model::AgentStatusRecord;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{AgentMode, Principal, UntypedDataValue};
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::{
        AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, OwnedAgentId, PromiseId,
        ScheduleId, ScheduledAction, ShardAssignment, ShardId,
    };
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::collections::HashSet;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use test_r::test;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    struct SchedulerWorkerAccessMock;

    #[async_trait]
    impl SchedulerWorkerAccess for SchedulerWorkerAccessMock {
        async fn active_worker_fingerprint(
            &self,
            _owned_agent_id: &OwnedAgentId,
        ) -> Option<AgentFingerprint> {
            None
        }

        async fn activate_worker(&self, _owned_agent_id: &OwnedAgentId) {}

        async fn open_oplog(
            &self,
            _owned_agent_id: &OwnedAgentId,
        ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
            unimplemented!()
        }

        async fn enqueue_invocation(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _invocation: AgentInvocation,
        ) -> Result<(), WorkerExecutorError> {
            unimplemented!()
        }
    }

    struct ActiveWorkerAccessMock {
        fingerprint: AgentFingerprint,
        enqueue_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SchedulerWorkerAccess for ActiveWorkerAccessMock {
        async fn active_worker_fingerprint(
            &self,
            _owned_agent_id: &OwnedAgentId,
        ) -> Option<AgentFingerprint> {
            Some(self.fingerprint)
        }

        async fn activate_worker(&self, _owned_agent_id: &OwnedAgentId) {}

        async fn open_oplog(
            &self,
            _owned_agent_id: &OwnedAgentId,
        ) -> Result<Arc<dyn Oplog>, WorkerExecutorError> {
            unimplemented!()
        }

        async fn enqueue_invocation(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _invocation: AgentInvocation,
        ) -> Result<(), WorkerExecutorError> {
            self.enqueue_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct WorkerServiceMock;

    #[async_trait]
    impl WorkerService for WorkerServiceMock {
        async fn get(&self, _owned_agent_id: &OwnedAgentId) -> Option<GetWorkerMetadataResult> {
            unimplemented!()
        }

        async fn get_running_workers_in_shards(&self) -> Vec<GetWorkerMetadataResult> {
            unimplemented!()
        }

        async fn remove(&self, _owned_agent_id: &OwnedAgentId) {}

        async fn remove_cached_status(&self, _owned_agent_id: &OwnedAgentId) {}

        async fn get_agent_mode(&self, _owned_agent_id: &OwnedAgentId) -> Option<AgentMode> {
            None
        }

        async fn write_cached_status(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _previous_status: Option<&AgentStatusRecord>,
            status_value: AgentStatusRecord,
        ) -> Result<AgentStatusRecord, String> {
            Ok(status_value)
        }

        async fn read_status_checkpoint(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _agent_mode: AgentMode,
        ) -> Option<AgentStatusRecord> {
            None
        }

        async fn write_status_checkpoint(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _previous_checkpoint: Option<&AgentStatusRecord>,
            checkpoint: AgentStatusRecord,
        ) -> Result<AgentStatusRecord, String> {
            Ok(checkpoint)
        }

        async fn set_assignment_tracking(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _status_value: &AgentStatusRecord,
        ) {
        }
    }

    fn create_shard_service_mock() -> Arc<dyn ShardService> {
        let result = Arc::new(ShardServiceDefault::new());
        result.register(1, &HashSet::from_iter(vec![ShardId::new(0)]));
        result
    }

    fn create_promise_service_mock() -> Arc<PromiseServiceMock> {
        Arc::new(PromiseServiceMock::new())
    }

    fn create_worker_access_mock() -> Arc<dyn SchedulerWorkerAccess + Send + Sync> {
        Arc::new(SchedulerWorkerAccessMock)
    }

    async fn create_oplog_service_mock() -> Arc<dyn OplogService> {
        Arc::new(
            PrimaryOplogService::new(
                Arc::new(InMemoryIndexedStorage::new()),
                Arc::new(InMemoryBlobStorage::new()),
                1,
                1,
                1024,
                golem_common::model::RetryConfig::default(),
            )
            .await,
        )
    }

    fn create_worker_service_mock() -> Arc<dyn WorkerService> {
        Arc::new(WorkerServiceMock)
    }

    async fn create_scheduler(
        scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
        promise_service: Arc<PromiseServiceMock>,
    ) -> Arc<SchedulerServiceDefault> {
        SchedulerServiceDefault::new(
            scheduler_storage,
            create_shard_service_mock(),
            promise_service,
            create_worker_access_mock(),
            create_oplog_service_mock().await,
            create_worker_service_mock(),
            Duration::from_secs(1000),
            100,
            Duration::from_secs(30),
            10,
            CancellationToken::new(),
        )
    }

    fn promise(agent_id: AgentId, idx: u64) -> PromiseId {
        PromiseId {
            agent_id,
            oplog_idx: OplogIndex::from_u64(idx),
        }
    }

    fn agent(name: &str) -> AgentId {
        AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: name.to_string(),
        }
    }

    fn complete_promise_action(promise_id: PromiseId) -> ScheduledAction {
        ScheduledAction::CompletePromise {
            account_id: AccountId::new(),
            environment_id: EnvironmentId::new(),
            promise_id,
        }
    }

    fn agent_method_invocation() -> AgentInvocation {
        AgentInvocation::AgentMethod {
            idempotency_key: IdempotencyKey::fresh(),
            method_name: "run".to_string(),
            input: UntypedDataValue::Tuple(vec![]),
            invocation_context: InvocationContextStack::fresh(),
            principal: Principal::anonymous(),
        }
    }

    #[test]
    async fn schedule_returns_uuid_backed_id_and_cancel_removes_entry() {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let promise_service = create_promise_service_mock();
        let svc = create_scheduler(storage.clone(), promise_service).await;

        let action = complete_promise_action(promise(agent("inst1"), 101));
        let schedule_id = svc
            .schedule(DateTime::from_str("2023-07-17T10:05:00Z").unwrap(), action)
            .await;

        svc.cancel(schedule_id).await;

        let assignment = ShardAssignment {
            number_of_shards: 1,
            shard_ids: HashSet::from_iter([ShardId::new(0)]),
        };
        let claimed = storage
            .claim_due(
                DateTime::from_str("2023-07-17T10:06:00Z").unwrap(),
                &assignment,
                10,
                Duration::from_secs(30),
            )
            .await
            .unwrap();

        assert!(claimed.is_empty());
    }

    #[test]
    fn schedule_id_uses_same_deterministic_derivation_as_rpc_idempotency_keys() {
        let base = IdempotencyKey::new("caller-provided-idempotency-key".to_string());
        let oplog_index = OplogIndex::from_u64(42);

        let rpc_idempotency_key = IdempotencyKey::derived(&base, oplog_index);
        let schedule_id = ScheduleId::from_idempotency_key(&rpc_idempotency_key);
        let same_rpc_idempotency_key = IdempotencyKey::derived(&base, oplog_index);
        let different_rpc_idempotency_key =
            IdempotencyKey::derived(&base, OplogIndex::from_u64(43));

        assert_eq!(schedule_id.id.to_string(), rpc_idempotency_key.value);
        assert_eq!(
            schedule_id,
            ScheduleId::from_idempotency_key(&same_rpc_idempotency_key)
        );
        assert_ne!(
            schedule_id,
            ScheduleId::from_idempotency_key(&different_rpc_idempotency_key)
        );
    }

    #[test]
    async fn schedule_with_id_is_idempotent() {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let promise_service = create_promise_service_mock();
        let svc = create_scheduler(storage.clone(), promise_service).await;

        let action = complete_promise_action(promise(agent("inst1"), 101));
        let schedule_id = ScheduleId::fresh();
        let due_at = DateTime::from_str("2023-07-17T10:05:00Z").unwrap();

        assert_eq!(
            svc.schedule_with_id(schedule_id, due_at, action.clone())
                .await,
            schedule_id
        );
        assert_eq!(
            svc.schedule_with_id(schedule_id, due_at, action.clone())
                .await,
            schedule_id
        );

        let assignment = ShardAssignment {
            number_of_shards: 1,
            shard_ids: HashSet::from_iter([ShardId::new(0)]),
        };
        let claimed = storage
            .claim_due(
                DateTime::from_str("2023-07-17T10:06:00Z").unwrap(),
                &assignment,
                10,
                Duration::from_secs(30),
            )
            .await
            .unwrap();

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].schedule_id, schedule_id);
    }

    #[test]
    async fn process_completes_entries_older_than_previous_hour() {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let promise_service = create_promise_service_mock();
        let svc = create_scheduler(storage, promise_service.clone()).await;

        let promise_id = promise(agent("inst1"), 101);
        svc.schedule(
            DateTime::from_str("2023-07-17T07:05:00Z").unwrap(),
            complete_promise_action(promise_id.clone()),
        )
        .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        let completed_promises = promise_service.all_completed().await;
        assert!(completed_promises.contains(&promise_id));
    }

    #[test]
    async fn scheduled_invoke_uses_active_worker_fingerprint_before_worker_service_lookup() {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let promise_service = create_promise_service_mock();
        let fingerprint = AgentFingerprint::new();
        let enqueue_count = Arc::new(AtomicUsize::new(0));
        let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
            Arc::new(ActiveWorkerAccessMock {
                fingerprint,
                enqueue_count: enqueue_count.clone(),
            });
        let svc = SchedulerServiceDefault::new(
            storage,
            create_shard_service_mock(),
            promise_service,
            worker_access,
            create_oplog_service_mock().await,
            create_worker_service_mock(),
            Duration::from_secs(1000),
            100,
            Duration::from_secs(30),
            10,
            CancellationToken::new(),
        );

        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("inst1"));
        svc.schedule(
            DateTime::from_str("2023-07-17T07:05:00Z").unwrap(),
            ScheduledAction::Invoke {
                account_id: AccountId::new(),
                owned_agent_id,
                invocation: Box::new(agent_method_invocation()),
                target_worker_fingerprint: fingerprint,
            },
        )
        .await;

        svc.process(DateTime::from_str("2023-07-17T10:15:00Z").unwrap())
            .await
            .unwrap();

        assert_eq!(enqueue_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    async fn leases_prevent_duplicate_claims_until_expiry() {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let action = complete_promise_action(promise(agent("inst1"), 101));
        let due_at = DateTime::from_str("2023-07-17T10:05:00Z").unwrap();
        storage
            .insert(ScheduleId::fresh(), due_at, ShardId::new(0), &action)
            .await
            .unwrap();

        let assignment = ShardAssignment {
            number_of_shards: 1,
            shard_ids: HashSet::from_iter([ShardId::new(0)]),
        };
        let now = DateTime::from_str("2023-07-17T10:06:00Z").unwrap();

        let first = storage
            .claim_due(now, &assignment, 10, Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(first.len(), 1);

        let second = storage
            .claim_due(now, &assignment, 10, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(second.is_empty());

        let after_expiry = storage
            .claim_due(
                DateTime::from_str("2023-07-17T10:06:31Z").unwrap(),
                &assignment,
                10,
                Duration::from_secs(30),
            )
            .await
            .unwrap();
        assert_eq!(after_expiry.len(), 1);
    }

    #[test]
    async fn stale_ack_does_not_delete_reclaimed_entry() {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let action = complete_promise_action(promise(agent("inst1"), 101));
        let due_at = DateTime::from_str("2023-07-17T10:05:00Z").unwrap();
        storage
            .insert(ScheduleId::fresh(), due_at, ShardId::new(0), &action)
            .await
            .unwrap();

        let assignment = ShardAssignment {
            number_of_shards: 1,
            shard_ids: HashSet::from_iter([ShardId::new(0)]),
        };
        let first = storage
            .claim_due(
                DateTime::from_str("2023-07-17T10:06:00Z").unwrap(),
                &assignment,
                10,
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .pop()
            .unwrap();
        let second = storage
            .claim_due(
                DateTime::from_str("2023-07-17T10:06:31Z").unwrap(),
                &assignment,
                10,
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .pop()
            .unwrap();

        assert!(
            !storage
                .ack(&first.schedule_id, first.lease_owner)
                .await
                .unwrap()
        );
        assert!(
            storage
                .ack(&second.schedule_id, second.lease_owner)
                .await
                .unwrap()
        );
    }

    #[test]
    async fn lease_extension_prevents_reclaim_until_extended_deadline() {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let action = complete_promise_action(promise(agent("inst1"), 101));
        let due_at = DateTime::from_str("2023-07-17T10:05:00Z").unwrap();
        storage
            .insert(ScheduleId::fresh(), due_at, ShardId::new(0), &action)
            .await
            .unwrap();

        let assignment = ShardAssignment {
            number_of_shards: 1,
            shard_ids: HashSet::from_iter([ShardId::new(0)]),
        };
        let claimed = storage
            .claim_due(
                DateTime::from_str("2023-07-17T10:06:00Z").unwrap(),
                &assignment,
                10,
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .pop()
            .unwrap();

        assert!(
            storage
                .extend_lease(
                    &claimed.schedule_id,
                    claimed.lease_owner,
                    DateTime::from_str("2023-07-17T10:07:00Z").unwrap(),
                )
                .await
                .unwrap()
        );

        assert!(
            storage
                .claim_due(
                    DateTime::from_str("2023-07-17T10:06:31Z").unwrap(),
                    &assignment,
                    10,
                    Duration::from_secs(30),
                )
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            storage
                .claim_due(
                    DateTime::from_str("2023-07-17T10:07:01Z").unwrap(),
                    &assignment,
                    10,
                    Duration::from_secs(30),
                )
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    async fn shard_filtering_uses_current_assignment() {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let action = complete_promise_action(promise(agent("inst1"), 101));
        let routing_hash = ShardId::hash_agent_id(&action.owned_agent_id().agent_id);
        let shard = ShardId::from_routing_hash(routing_hash, 2);
        let other_shard = ShardId::new((shard.value() + 1) % 2);
        let due_at = DateTime::from_str("2023-07-17T10:05:00Z").unwrap();
        storage
            .insert(ScheduleId::fresh(), due_at, shard, &action)
            .await
            .unwrap();

        let unassigned = ShardAssignment {
            number_of_shards: 2,
            shard_ids: HashSet::from_iter([other_shard]),
        };
        let assigned = ShardAssignment {
            number_of_shards: 2,
            shard_ids: HashSet::from_iter([shard]),
        };
        let now = DateTime::from_str("2023-07-17T10:06:00Z").unwrap();

        assert!(
            storage
                .claim_due(now, &unassigned, 10, Duration::from_secs(30))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            storage
                .claim_due(now, &assigned, 10, Duration::from_secs(30))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn shard_id_from_routing_hash_handles_negative_hashes() {
        assert_eq!(ShardId::from_routing_hash(-i64::MAX, 10), ShardId::new(7));
    }
}
