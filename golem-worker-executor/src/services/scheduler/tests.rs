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

use golem_common::model::InvocationId;

use crate::services::oplog::tests::ReadCountingIndexedStorage;
use crate::services::oplog::{ArchiveWait, OplogService, PrimaryOplogService};
use crate::services::promise::{PromiseService, PromiseServiceMock};
use crate::services::scheduler::{
    SchedulerService, SchedulerServiceDefault, SchedulerWorkerAccess,
};
use crate::services::shard::{ShardService, ShardServiceDefault};
use crate::services::worker_activator::ExactExistingDelivery;
use crate::span_test_support::{Tracing, get_tracing_dependency as test_r_get_dep_tracing};
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::indexed::{IndexedStorage, IndexedStorageNamespace};
use crate::storage::scheduler::memory::InMemorySchedulerStorage;
use crate::storage::scheduler::sqlite::SqliteSchedulerStorage;
use crate::storage::scheduler::{ClaimedScheduledAction, SchedulerStorage, SchedulerStorageError};
use async_trait::async_trait;

use futures::FutureExt;
use golem_common::config::DbSqliteConfig;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AgentFingerprint, AgentId, OwnedAgentId, PromiseId, RetryConfig, ScheduleId, ScheduledAction,
    ShardAssignment, ShardEpoch, ShardId,
};
use golem_common::serialization::serialize;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use proptest::prelude::*;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::test;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// Resume scheduling tests share a process-global histogram with exact-count assertions.
static RESUME_METRIC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct SchedulerWorkerAccessMock {
    promise_service: Arc<PromiseServiceMock>,
}

type ExpiryDelivery = (OwnedAgentId, AgentFingerprint, String, InvocationId, u64);

struct ExpiryWorkerAccessMock {
    deliveries: Arc<Mutex<Vec<ExpiryDelivery>>>,
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerWorkerAccess for ExpiryWorkerAccessMock {
    async fn active_worker_fingerprint(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        unimplemented!()
    }

    async fn worker_is_cached(&self, _owned_agent_id: &OwnedAgentId) -> bool {
        unimplemented!()
    }

    async fn activate_worker(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        unimplemented!()
    }

    async fn archive_oplog(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _last_oplog_index: OplogIndex,
        _wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn expire_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_agent_fingerprint: AgentFingerprint,
        public_session_id: String,
        session_key: InvocationId,
        expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        self.deliveries.lock().unwrap().push((
            owned_agent_id.clone(),
            target_agent_fingerprint,
            public_session_id,
            session_key,
            expected_deadline_millis,
        ));
        Ok(())
    }

    async fn enqueue_exact_existing(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_worker_fingerprint: AgentFingerprint,
        _delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError> {
        unimplemented!()
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerWorkerAccess for SchedulerWorkerAccessMock {
    async fn expire_durable_stream_session(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_agent_fingerprint: AgentFingerprint,
        _public_session_id: String,
        _session_key: InvocationId,
        _expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        unimplemented!()
    }

    async fn active_worker_fingerprint(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        None
    }

    async fn worker_is_cached(&self, _owned_agent_id: &OwnedAgentId) -> bool {
        unimplemented!()
    }

    async fn activate_worker(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn archive_oplog(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _last_oplog_index: OplogIndex,
        _wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn enqueue_exact_existing(
        &self,
        owned_agent_id: &OwnedAgentId,
        _target_worker_fingerprint: AgentFingerprint,
        delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError> {
        if let ExactExistingDelivery::CompletePromise(id) = delivery {
            self.promise_service
                .complete_without_wakeup(owned_agent_id.environment_id, id, vec![])
                .await?;
        }
        Ok(true)
    }
}

struct ActiveWorkerAccessMock {
    fingerprint: AgentFingerprint,
    enqueue_count: Arc<AtomicUsize>,
    missing: bool,
    failures: AtomicUsize,
    fail_before_completion: bool,
    promise_service: Arc<PromiseServiceMock>,
    deliveries: Arc<Mutex<Vec<&'static str>>>,
}

struct RecordingActiveWorkerAccess {
    fingerprint: AgentFingerprint,
    executions: Arc<Mutex<Vec<String>>>,
    complete_not_before: Option<jiff::Timestamp>,
    started: CancellationToken,
}

impl RecordingActiveWorkerAccess {
    async fn wait_until_completion(&self) {
        self.started.cancel();
        if let Some(deadline) = self.complete_not_before {
            tokio::time::timeout(Duration::from_secs(5), async {
                while jiff::Timestamp::now().as_millisecond() < deadline.as_millisecond() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("wall clock must reach the delivery completion deadline");
            assert!(jiff::Timestamp::now().as_millisecond() >= deadline.as_millisecond());
        }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerWorkerAccess for RecordingActiveWorkerAccess {
    async fn expire_durable_stream_session(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_agent_fingerprint: AgentFingerprint,
        _public_session_id: String,
        _session_key: InvocationId,
        _expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        unimplemented!()
    }

    async fn active_worker_fingerprint(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        Some(self.fingerprint)
    }

    async fn worker_is_cached(&self, _owned_agent_id: &OwnedAgentId) -> bool {
        unimplemented!()
    }

    async fn activate_worker(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        panic!("scheduled actions must not use unpinned activation")
    }

    async fn archive_oplog(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _last_oplog_index: OplogIndex,
        _wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        self.wait_until_completion().await;
        self.executions.lock().unwrap().push("archive".to_string());
        Ok(Some(true))
    }

    async fn enqueue_exact_existing(
        &self,
        _owned_agent_id: &OwnedAgentId,
        target_worker_fingerprint: AgentFingerprint,
        delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError> {
        if target_worker_fingerprint != self.fingerprint {
            return Ok(false);
        }
        let ExactExistingDelivery::WakeInvocation(idempotency_key) = delivery else {
            unreachable!()
        };
        self.wait_until_completion().await;
        self.executions
            .lock()
            .unwrap()
            .push(idempotency_key.to_string());
        Ok(true)
    }
}

/// Fails every wake delivery, first transiently when asked, then with the terminal error of an
/// ephemeral agent that can no longer start.
struct FailingWakeWorkerAccess {
    terminal: bool,
    fail_first_transiently: bool,
    attempts: Arc<AtomicUsize>,
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerWorkerAccess for FailingWakeWorkerAccess {
    async fn expire_durable_stream_session(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_agent_fingerprint: AgentFingerprint,
        _public_session_id: String,
        _session_key: InvocationId,
        _expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        unimplemented!()
    }

    async fn active_worker_fingerprint(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        panic!("ephemeral schedules must not look up a target fingerprint")
    }

    async fn worker_is_cached(&self, _owned_agent_id: &OwnedAgentId) -> bool {
        unimplemented!()
    }

    async fn activate_worker(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        unreachable!()
    }

    async fn archive_oplog(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _last_oplog_index: OplogIndex,
        _wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        unreachable!()
    }

    async fn enqueue_exact_existing(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_worker_fingerprint: AgentFingerprint,
        _delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_first_transiently && attempt == 0 {
            return Err(WorkerExecutorError::invalid_request(
                "temporarily unavailable",
            ));
        }
        if self.terminal {
            return Err(WorkerExecutorError::invalid_request(
                crate::worker::INACTIVE_EPHEMERAL_AGENT_ERROR,
            ));
        }
        Ok(true)
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerWorkerAccess for ActiveWorkerAccessMock {
    async fn expire_durable_stream_session(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_agent_fingerprint: AgentFingerprint,
        _public_session_id: String,
        _session_key: InvocationId,
        _expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        unimplemented!()
    }

    async fn active_worker_fingerprint(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        Some(self.fingerprint)
    }

    async fn worker_is_cached(&self, _owned_agent_id: &OwnedAgentId) -> bool {
        unimplemented!()
    }

    async fn activate_worker(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        panic!("scheduled actions must not use unpinned activation")
    }

    async fn archive_oplog(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _last_oplog_index: OplogIndex,
        _wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn enqueue_exact_existing(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_worker_fingerprint: AgentFingerprint,
        delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError> {
        if self.missing || target_worker_fingerprint != self.fingerprint {
            return Ok(false);
        }
        self.enqueue_count.fetch_add(1, Ordering::SeqCst);
        let fail = self
            .failures
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if fail && self.fail_before_completion {
            return Err(WorkerExecutorError::runtime("injected completion failure"));
        }
        let kind = match delivery {
            ExactExistingDelivery::WakeInvocation(_) => "wake_invocation",
            ExactExistingDelivery::Resume => "resume",
            ExactExistingDelivery::CompletePromise(id) => {
                self.promise_service
                    .complete_without_wakeup(owned_agent_id.environment_id, id, vec![])
                    .await?;
                "complete_promise"
            }
        };
        self.deliveries.lock().unwrap().push(kind);
        if fail {
            return Err(WorkerExecutorError::runtime("injected start failure"));
        }
        Ok(true)
    }
}

struct FailingActivationWorkerAccess {
    attempts: Arc<AtomicUsize>,
    promise_service: Arc<PromiseServiceMock>,
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerWorkerAccess for FailingActivationWorkerAccess {
    async fn expire_durable_stream_session(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_agent_fingerprint: AgentFingerprint,
        _public_session_id: String,
        _session_key: InvocationId,
        _expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        unimplemented!()
    }

    async fn active_worker_fingerprint(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        None
    }

    async fn worker_is_cached(&self, _owned_agent_id: &OwnedAgentId) -> bool {
        unimplemented!()
    }

    async fn activate_worker(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(WorkerExecutorError::runtime("injected activation failure"))
    }

    async fn archive_oplog(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _last_oplog_index: OplogIndex,
        _wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn enqueue_exact_existing(
        &self,
        owned_agent_id: &OwnedAgentId,
        _target_worker_fingerprint: AgentFingerprint,
        delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError> {
        if let ExactExistingDelivery::CompletePromise(id) = delivery {
            self.promise_service
                .complete_without_wakeup(owned_agent_id.environment_id, id, vec![])
                .await?;
        }
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(WorkerExecutorError::runtime("injected delivery failure"))
    }
}

struct DelayedActiveWorkerAccessMock {
    fingerprint: AgentFingerprint,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerWorkerAccess for DelayedActiveWorkerAccessMock {
    async fn expire_durable_stream_session(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_agent_fingerprint: AgentFingerprint,
        _public_session_id: String,
        _session_key: InvocationId,
        _expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        unimplemented!()
    }

    async fn active_worker_fingerprint(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        Some(self.fingerprint)
    }

    async fn worker_is_cached(&self, _owned_agent_id: &OwnedAgentId) -> bool {
        unimplemented!()
    }

    async fn activate_worker(
        &self,
        _owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn archive_oplog(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _last_oplog_index: OplogIndex,
        _wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn enqueue_exact_existing(
        &self,
        _owned_agent_id: &OwnedAgentId,
        target_worker_fingerprint: AgentFingerprint,
        _delivery: ExactExistingDelivery,
    ) -> Result<bool, WorkerExecutorError> {
        if target_worker_fingerprint != self.fingerprint {
            return Ok(false);
        }
        let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(true)
    }
}

fn create_shard_service_mock() -> Arc<dyn ShardService> {
    let result = Arc::new(ShardServiceDefault::new());
    result.install_unexpiring(
        1,
        &HashMap::from([(ShardId::new(0), ShardEpoch::default())]),
    );
    result
}

fn create_promise_service_mock() -> Arc<PromiseServiceMock> {
    Arc::new(PromiseServiceMock::new())
}

fn create_worker_access_mock() -> Arc<dyn SchedulerWorkerAccess + Send + Sync> {
    Arc::new(SchedulerWorkerAccessMock {
        promise_service: create_promise_service_mock(),
    })
}

async fn create_oplog_service_mock() -> Arc<dyn OplogService> {
    Arc::new(
        PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            1024,
            32,
            golem_common::model::RetryConfig::default(),
        )
        .await,
    )
}

async fn create_scheduler(
    scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
    promise_service: Arc<PromiseServiceMock>,
) -> Arc<SchedulerServiceDefault> {
    create_scheduler_with(
        scheduler_storage,
        Arc::new(SchedulerWorkerAccessMock { promise_service }),
    )
    .await
}

async fn create_scheduler_with_worker_access(
    scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
    worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
) -> Arc<SchedulerServiceDefault> {
    create_scheduler_with(scheduler_storage, worker_access).await
}

async fn create_scheduler_with(
    scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
    worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync>,
) -> Arc<SchedulerServiceDefault> {
    SchedulerServiceDefault::new(
        scheduler_storage,
        create_shard_service_mock(),
        worker_access,
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        64,
        CancellationToken::new(),
    )
}

async fn create_scheduler_with_oplog(
    scheduler_storage: Arc<dyn SchedulerStorage + Send + Sync>,
    oplog_service: Arc<dyn OplogService>,
) -> Arc<SchedulerServiceDefault> {
    SchedulerServiceDefault::new(
        scheduler_storage,
        create_shard_service_mock(),
        create_worker_access_mock(),
        oplog_service,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        64,
        CancellationToken::new(),
    )
}

/// Scheduler storage wrapper whose `insert` fails with a transient error the
/// first `transient_failures` times, then delegates to the inner storage.
#[derive(Debug)]
struct FlakyInsertSchedulerStorage {
    inner: InMemorySchedulerStorage,
    transient_failures: AtomicU32,
    insert_attempts: Arc<AtomicUsize>,
    inserted_actions: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerStorage for FlakyInsertSchedulerStorage {
    async fn insert(
        &self,
        schedule_id: ScheduleId,
        due_at: jiff::Timestamp,
        shard_id: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError> {
        self.insert_attempts.fetch_add(1, Ordering::SeqCst);
        self.inserted_actions.lock().unwrap().push(action.to_vec());
        let remaining =
            self.transient_failures
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        if remaining.is_ok() {
            return Err(SchedulerStorageError::Transient(
                "simulated pool timeout".to_string(),
            ));
        }
        self.inner
            .insert(schedule_id, due_at, shard_id, action)
            .await
    }

    async fn cancel(&self, schedule_id: &ScheduleId) -> Result<(), SchedulerStorageError> {
        self.inner.cancel(schedule_id).await
    }

    async fn claim_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
        limit: u32,
        lease_ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
        self.inner
            .claim_due(now, assignment, limit, lease_ttl)
            .await
    }

    async fn count_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
    ) -> Result<u64, SchedulerStorageError> {
        self.inner.count_due(now, assignment).await
    }

    async fn extend_lease(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        lease_until: jiff::Timestamp,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner
            .extend_lease(schedule_id, lease_owner, lease_until, now)
            .await
    }

    async fn ack(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        self.inner.ack(schedule_id, lease_owner, now).await
    }
}

/// Probe after both the service's 30-second lease and the retry tests' as-of claims,
/// so an empty result proves deletion rather than an action that is still leased.
async fn claim_all(storage: &Arc<InMemorySchedulerStorage>) -> Vec<ClaimedScheduledAction> {
    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    storage
        .claim_due(
            jiff::Timestamp::now() + jiff::SignedDuration::from_secs(120),
            &assignment,
            10,
            Duration::from_secs(30),
        )
        .await
        .unwrap()
}

// Retry-focused tests advance only the storage claim clock. The action and acknowledgement
// still run through the service, without waiting for a real 30-second lease to expire.
async fn process_retry(svc: &SchedulerServiceDefault, storage: &InMemorySchedulerStorage) {
    let claimed = storage
        .claim_due(
            jiff::Timestamp::now() + jiff::SignedDuration::from_secs(60),
            &ShardAssignment::unexpiring(1, [ShardId::new(0)]),
            10,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert_eq!(
        claimed.len(),
        1,
        "failed action must be available for retry"
    );
    svc.process_claimed_actions(claimed, jiff::Timestamp::now())
        .await
        .unwrap();
}

fn promise(agent_id: AgentId, idx: u64) -> PromiseId {
    PromiseId {
        agent_id,
        fingerprint: AgentFingerprint(Uuid::from_u128(u128::from(idx))),
        oplog_idx: OplogIndex::from_u64(idx),
    }
}

#[derive(Debug, Default)]
struct RecordingSchedulerStorage {
    inner: Arc<InMemorySchedulerStorage>,
    failed_ack_ids: Arc<Mutex<HashSet<Uuid>>>,
    failed_acknowledgements: Arc<Mutex<HashSet<Uuid>>>,
    claimed_batches: Arc<Mutex<Vec<Vec<ClaimedScheduledAction>>>>,
    claim_times: Mutex<Vec<jiff::Timestamp>>,
    insert_after_first_claim: Mutex<Option<(ScheduleId, ScheduledAction)>>,
    ack_failures: AtomicUsize,
    claims_after_ack_failure: AtomicUsize,
    expire_renewal: AtomicUsize,
    renewal_refused: CancellationToken,
    hold_ack_until_renewal_refused: bool,
}

#[async_trait]
#[golem_async_trait::region_simple]
impl SchedulerStorage for RecordingSchedulerStorage {
    async fn insert(
        &self,
        schedule_id: ScheduleId,
        due_at: jiff::Timestamp,
        shard_id: ShardId,
        action: &[u8],
    ) -> Result<(), SchedulerStorageError> {
        self.inner
            .insert(schedule_id, due_at, shard_id, action)
            .await
    }

    async fn cancel(&self, schedule_id: &ScheduleId) -> Result<(), SchedulerStorageError> {
        self.inner.cancel(schedule_id).await
    }

    async fn claim_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
        limit: u32,
        lease_ttl: Duration,
    ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
        if self.ack_failures.load(Ordering::SeqCst) > 0 {
            self.claims_after_ack_failure.fetch_add(1, Ordering::SeqCst);
        }
        let claimed = self
            .inner
            .claim_due(now, assignment, limit, lease_ttl)
            .await?;
        self.claimed_batches.lock().unwrap().push(claimed.clone());
        self.claim_times.lock().unwrap().push(now);
        let insert = self.insert_after_first_claim.lock().unwrap().take();
        if let Some((schedule_id, action)) = insert {
            // Make the new action strictly later than the first claim even after storage
            // truncates timestamps to milliseconds. A reused claim time cannot see it.
            let due_at = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let current = jiff::Timestamp::now();
                    if current.as_millisecond() > now.as_millisecond() {
                        break current;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("wall clock must advance beyond the first claim");
            assert!(due_at.as_millisecond() > now.as_millisecond());
            self.inner
                .insert(
                    schedule_id,
                    due_at,
                    ShardId::new(0),
                    &serialize(&action).unwrap(),
                )
                .await?;
        }
        Ok(claimed)
    }

    async fn count_due(
        &self,
        now: jiff::Timestamp,
        assignment: &ShardAssignment,
    ) -> Result<u64, SchedulerStorageError> {
        self.inner.count_due(now, assignment).await
    }

    async fn extend_lease(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        lease_until: jiff::Timestamp,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        if self
            .expire_renewal
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            == Ok(1)
        {
            // Simulate a renewal delayed past expiry, without reclaiming the action.
            // Its requested deadline is at least as late as the previous lease deadline.
            while jiff::Timestamp::now().as_millisecond() < lease_until.as_millisecond() {
                tokio::task::yield_now().await;
            }
            let renewed = self
                .inner
                .extend_lease(
                    schedule_id,
                    lease_owner,
                    lease_until,
                    jiff::Timestamp::now(),
                )
                .await?;
            assert!(
                !renewed,
                "delayed renewal must not revive the expired lease"
            );
            self.renewal_refused.cancel();
            return Ok(renewed);
        }
        self.inner
            .extend_lease(schedule_id, lease_owner, lease_until, now)
            .await
    }

    async fn ack(
        &self,
        schedule_id: &ScheduleId,
        lease_owner: Uuid,
        now: jiff::Timestamp,
    ) -> Result<bool, SchedulerStorageError> {
        if self.hold_ack_until_renewal_refused {
            self.renewal_refused.cancelled().await;
        }
        if self.failed_ack_ids.lock().unwrap().remove(&schedule_id.id) {
            self.ack_failures.fetch_add(1, Ordering::SeqCst);
            self.failed_acknowledgements
                .lock()
                .unwrap()
                .insert(schedule_id.id);
            return Err(SchedulerStorageError::Transient(
                "simulated transient acknowledgement failure".to_string(),
            ));
        }
        self.inner.ack(schedule_id, lease_owner, now).await
    }
}

#[test]
async fn schedule_retries_transient_storage_errors() {
    let insert_attempts = Arc::new(AtomicUsize::new(0));
    let inserted_actions = Arc::new(Mutex::new(Vec::new()));
    let storage = Arc::new(FlakyInsertSchedulerStorage {
        inner: InMemorySchedulerStorage::new(),
        transient_failures: AtomicU32::new(2),
        insert_attempts: insert_attempts.clone(),
        inserted_actions: inserted_actions.clone(),
    });
    let promise_service = create_promise_service_mock();
    let svc = create_scheduler(storage.clone(), promise_service).await;

    let action = complete_promise_action(promise(agent("inst1"), 101));
    let expected_action = serialize(&action).unwrap();
    let schedule_id = ScheduleId::fresh();
    let due_at = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(1);

    // Two transient failures are retried; the third attempt succeeds without panicking.
    assert_eq!(
        svc.schedule_with_id(schedule_id, due_at, action).await,
        schedule_id
    );
    assert_eq!(insert_attempts.load(Ordering::SeqCst), 3);
    assert_eq!(*inserted_actions.lock().unwrap(), vec![expected_action; 3]);

    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    let claimed = storage
        .claim_due(
            jiff::Timestamp::now(),
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
async fn schedule_size_is_observed_once_for_duplicates_and_terminal_failures() {
    let _metric_guard = RESUME_METRIC_LOCK.lock().await;
    let action = ScheduledAction::Resume {
        agent_created_by: AccountId::new(),
        owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent("metric")),
        target_worker_fingerprint: AgentFingerprint::new(),
    };
    let labels = [
        crate::metrics::storage::executor_id(),
        crate::metrics::scheduler::action_kind_label(&action),
    ];
    let histogram =
        crate::metrics::scheduler::SCHEDULED_ACTION_SIZE_BYTES.with_label_values(&labels);
    let before = histogram.get_sample_count();
    let due_at = jiff::Timestamp::now();

    let storage = Arc::new(InMemorySchedulerStorage::new());
    let svc = create_scheduler(storage, create_promise_service_mock()).await;
    let schedule_id = ScheduleId::fresh();
    svc.schedule_with_id(schedule_id, due_at, action.clone())
        .await;
    svc.schedule_with_id(schedule_id, due_at, action.clone())
        .await;
    assert_eq!(histogram.get_sample_count(), before + 2);

    let failing_storage = Arc::new(FlakyInsertSchedulerStorage {
        inner: InMemorySchedulerStorage::new(),
        transient_failures: AtomicU32::new(3),
        insert_attempts: Arc::new(AtomicUsize::new(0)),
        inserted_actions: Arc::new(Mutex::new(Vec::new())),
    });
    let failing_svc = create_scheduler(failing_storage, create_promise_service_mock()).await;
    let result =
        AssertUnwindSafe(failing_svc.schedule_with_id(ScheduleId::fresh(), due_at, action))
            .catch_unwind()
            .await;

    assert!(result.is_err());
    assert_eq!(histogram.get_sample_count(), before + 3);
}

fn agent(name: &str) -> AgentId {
    crate::test_agent_id(name)
}

fn complete_promise_action(promise_id: PromiseId) -> ScheduledAction {
    ScheduledAction::CompletePromise {
        account_id: AccountId::new(),
        environment_id: EnvironmentId::new(),
        target_worker_fingerprint: promise_id.fingerprint,
        promise_id,
    }
}

fn scheduled_action_variants() -> Vec<ScheduledAction> {
    let environment_id = EnvironmentId::new();
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent("scheduled-action"));
    vec![
        complete_promise_action(promise(owned_agent_id.agent_id, 101)),
        ScheduledAction::ArchiveOplog {
            account_id: AccountId::new(),
            owned_agent_id: owned_agent_id.clone(),
            agent_mode: AgentMode::Durable,
            last_oplog_index: OplogIndex::from_u64(42),
            next_after: Duration::from_secs(60),
        },
        ScheduledAction::WakeInvocation {
            owned_agent_id: owned_agent_id.clone(),
            idempotency_key: InvocationId(Uuid::new_v4()),
            target_worker_fingerprint: AgentFingerprint::new(),
        },
        ScheduledAction::Resume {
            agent_created_by: AccountId::new(),
            owned_agent_id: owned_agent_id.clone(),
            target_worker_fingerprint: AgentFingerprint::new(),
        },
    ]
}

async fn assert_storage_roundtrips_all_action_variants(
    storage: Arc<dyn SchedulerStorage + Send + Sync>,
) {
    let due_at = jiff::Timestamp::from_str("2023-07-17T10:05:00Z").unwrap();
    let shard_id = ShardId::new(0);
    let actions = scheduled_action_variants();
    let mut expected = HashMap::new();
    for action in actions {
        let schedule_id = ScheduleId::fresh();
        storage
            .insert(schedule_id, due_at, shard_id, &serialize(&action).unwrap())
            .await
            .unwrap();
        expected.insert(schedule_id, action);
    }

    let claimed = storage
        .claim_due(
            jiff::Timestamp::from_str("2023-07-17T10:06:00Z").unwrap(),
            &ShardAssignment::unexpiring(1, [shard_id]),
            expected.len() as u32,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

    assert_eq!(claimed.len(), expected.len());
    for claimed_action in claimed {
        assert_eq!(
            claimed_action.action,
            expected.remove(&claimed_action.schedule_id).unwrap()
        );
    }
    assert!(expected.is_empty());
}

#[test]
async fn in_memory_scheduler_storage_roundtrips_all_action_variants() {
    assert_storage_roundtrips_all_action_variants(Arc::new(InMemorySchedulerStorage::new())).await;
}

#[test]
async fn sqlite_scheduler_storage_roundtrips_all_action_variants() {
    let tempdir = tempfile::tempdir().unwrap();
    let storage = SqliteSchedulerStorage::configured(&DbSqliteConfig {
        database: tempdir
            .path()
            .join("scheduler.db")
            .to_string_lossy()
            .into_owned(),
        max_connections: 1,
        foreign_keys: false,
    })
    .await
    .unwrap();

    assert_storage_roundtrips_all_action_variants(Arc::new(storage)).await;
}

/// The scheduler's span must cover one tick, not the whole background loop. A
/// loop-lifetime span never closes, so it is never exported and it retains
/// every event recorded inside it for as long as the process runs.
#[test]
async fn process_records_one_closed_span_per_tick(tracing: &Tracing) {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let promise_service = create_promise_service_mock();
    let svc = create_scheduler(storage, promise_service).await;

    let recorder = crate::span_test_support::record_spans(tracing);
    svc.process(jiff::Timestamp::now()).await.unwrap();

    recorder.assert_closed_span("scheduler_tick");
    recorder.assert_all_closed();
}

#[test]
async fn acknowledgement_uses_completion_time_not_original_tick() {
    for expire_before_completion in [false, true] {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let fingerprint = AgentFingerprint::new();
        let executions = Arc::new(Mutex::new(Vec::new()));
        let oplog_service = create_oplog_service_mock().await;
        let tick = jiff::Timestamp::now();
        let lease_ttl = if expire_before_completion {
            Duration::from_millis(20)
        } else {
            Duration::from_secs(30)
        };
        let lease_until = tick + jiff::SignedDuration::try_from(lease_ttl).unwrap();
        let svc = SchedulerServiceDefault::new(
            storage.clone(),
            create_shard_service_mock(),
            Arc::new(RecordingActiveWorkerAccess {
                fingerprint,
                executions: executions.clone(),
                complete_not_before: expire_before_completion.then_some(lease_until),
                started: CancellationToken::new(),
            }),
            oplog_service,
            Duration::from_secs(1000),
            1,
            lease_ttl,
            1,
            RetryConfig::max_attempts_3(),
            1,
            CancellationToken::new(),
        );
        let invocation_id = InvocationId(Uuid::new_v4());
        let schedule_id = svc
            .schedule(
                tick,
                ScheduledAction::WakeInvocation {
                    owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent("ack-clock")),
                    idempotency_key: invocation_id,
                    target_worker_fingerprint: fingerprint,
                },
            )
            .await;
        let claimed = storage
            .claim_due(
                tick,
                &ShardAssignment::unexpiring(1, [ShardId::new(0)]),
                1,
                lease_ttl,
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(tick.as_millisecond() < lease_until.as_millisecond());

        // Exercise acknowledgement independently of batch renewal: the tick precedes
        // expiry, but a delivery completing after expiry must not delete the action.
        svc.process_and_ack_claimed_action(
            claimed.into_iter().next().unwrap(),
            tick,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(*executions.lock().unwrap(), vec![invocation_id.to_string()]);
        if expire_before_completion {
            assert!(jiff::Timestamp::now().as_millisecond() >= lease_until.as_millisecond());
            let remaining = claim_all(&storage).await;
            assert_eq!(remaining.len(), 1);
            assert_eq!(remaining[0].schedule_id, schedule_id);
            assert_eq!(remaining[0].attempt_count, 2);
        } else {
            assert!(claim_all(&storage).await.is_empty());
        }
    }
}

#[test]
#[test_r::timeout("10s")]
async fn batch_lease_renewal_covers_running_and_queued_actions() {
    for scenario in ["same-agent", "distinct-agents", "archive"] {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent(scenario));
        let fingerprint = AgentFingerprint::new();
        let executions = Arc::new(Mutex::new(Vec::new()));
        let oplog_service = create_oplog_service_with_presence(&owned_agent_id).await;
        let tick = jiff::Timestamp::now();
        let lease_ttl = Duration::from_millis(300);
        let complete_not_before = tick + jiff::SignedDuration::from_millis(900);
        let svc = SchedulerServiceDefault::new(
            storage.clone(),
            create_shard_service_mock(),
            Arc::new(RecordingActiveWorkerAccess {
                fingerprint,
                executions: executions.clone(),
                complete_not_before: Some(complete_not_before),
                started: CancellationToken::new(),
            }),
            oplog_service,
            Duration::from_secs(1000),
            3,
            lease_ttl,
            2,
            RetryConfig::max_attempts_3(),
            1,
            CancellationToken::new(),
        );
        let mut expected = Vec::new();
        if scenario == "archive" {
            svc.schedule(
                tick - jiff::SignedDuration::from_secs(10),
                ScheduledAction::ArchiveOplog {
                    account_id: AccountId::new(),
                    owned_agent_id,
                    agent_mode: AgentMode::Durable,
                    last_oplog_index: OplogIndex::from_u64(1),
                    next_after: Duration::from_secs(3600),
                },
            )
            .await;
            expected.push("archive".to_string());
        } else {
            for index in 0..3 {
                let invocation_id = InvocationId(Uuid::new_v4());
                expected.push(invocation_id.to_string());
                svc.schedule(
                    tick - jiff::SignedDuration::from_secs(10)
                        + jiff::SignedDuration::from_millis(index),
                    ScheduledAction::WakeInvocation {
                        owned_agent_id: if scenario == "same-agent" {
                            owned_agent_id.clone()
                        } else {
                            OwnedAgentId::new(
                                owned_agent_id.environment_id,
                                &agent(&format!("agent-{index}")),
                            )
                        },
                        idempotency_key: invocation_id,
                        target_worker_fingerprint: fingerprint,
                    },
                )
                .await;
            }
        }

        let (processed, ()) = tokio::join!(svc.process(tick), async {
            // Probe after the original lease has expired, while the first action is
            // still running. Both serial and concurrency-limited queues must stay leased.
            let probe_at = tick + jiff::SignedDuration::from_millis(600);
            while jiff::Timestamp::now().as_millisecond() < probe_at.as_millisecond() {
                tokio::task::yield_now().await;
            }
            assert!(jiff::Timestamp::now() < complete_not_before);
            assert!(
                storage
                    .claim_due(
                        jiff::Timestamp::now(),
                        &ShardAssignment::unexpiring(1, [ShardId::new(0)]),
                        10,
                        lease_ttl,
                    )
                    .await
                    .unwrap()
                    .is_empty(),
                "{scenario}: another executor reclaimed queued or running work"
            );
        });
        processed.unwrap();
        svc.process(jiff::Timestamp::now()).await.unwrap();
        let mut actual = executions.lock().unwrap().clone();
        if scenario == "distinct-agents" {
            actual.sort();
            expected.sort();
        }
        assert_eq!(
            actual, expected,
            "{scenario}: actions must run once, in agent order"
        );
        assert!(
            claim_all(&storage).await.is_empty(),
            "{scenario}: action was not acknowledged"
        );
    }
}

#[test]
#[test_r::timeout("10s")]
async fn lease_renewal_refuses_expired_work_and_cancels_in_flight_archival() {
    for expire_before_start in [false, true] {
        let storage = Arc::new(RecordingSchedulerStorage {
            // Accept preflight, then let the first heartbeat reach storage after expiry.
            expire_renewal: AtomicUsize::new(if expire_before_start { 0 } else { 2 }),
            ..Default::default()
        });
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("lost-lease"));
        let executions = Arc::new(Mutex::new(Vec::new()));
        let started = CancellationToken::new();
        let svc = SchedulerServiceDefault::new(
            storage.clone(),
            create_shard_service_mock(),
            Arc::new(RecordingActiveWorkerAccess {
                fingerprint: AgentFingerprint::new(),
                executions: executions.clone(),
                // Cancellation must drop this future, not wait for completion.
                complete_not_before: Some(
                    jiff::Timestamp::now() + jiff::SignedDuration::from_hours(24),
                ),
                started: started.clone(),
            }),
            create_oplog_service_with_presence(&owned_agent_id).await,
            Duration::from_secs(1000),
            1,
            Duration::from_millis(300),
            1,
            RetryConfig::max_attempts_3(),
            1,
            CancellationToken::new(),
        );
        let tick = jiff::Timestamp::now();
        let id = svc
            .schedule(
                tick - jiff::SignedDuration::from_secs(10),
                ScheduledAction::ArchiveOplog {
                    account_id: AccountId::new(),
                    owned_agent_id,
                    agent_mode: AgentMode::Durable,
                    last_oplog_index: OplogIndex::from_u64(1),
                    next_after: Duration::from_secs(3600),
                },
            )
            .await;
        let claim_time = if expire_before_start {
            tick - jiff::SignedDuration::from_secs(1)
        } else {
            tick
        };
        let claimed = storage
            .inner
            .claim_due(
                claim_time,
                &ShardAssignment::unexpiring(1, [ShardId::new(0)]),
                1,
                svc.lease_ttl,
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        tokio::time::timeout(
            Duration::from_secs(3),
            svc.process_claimed_actions(claimed, tick),
        )
        .await
        .expect("lost lease must stop the batch without waiting for archival")
        .unwrap();
        assert_eq!(started.is_cancelled(), !expire_before_start);
        assert!(
            executions.lock().unwrap().is_empty(),
            "lost lease must prevent completion"
        );
        let remaining = claim_all(&storage.inner).await;
        assert_eq!(
            remaining.len(),
            1,
            "lost lease must leave the action unacknowledged"
        );
        assert_eq!(remaining[0].schedule_id, id);
        assert_eq!(remaining[0].attempt_count, 2);
    }
}

#[test]
#[test_r::timeout("10s")]
async fn acknowledgement_error_is_not_discarded_when_renewal_fails_during_ack() {
    let id = ScheduleId::fresh();
    let storage = Arc::new(RecordingSchedulerStorage {
        failed_ack_ids: Arc::new(Mutex::new(HashSet::from([id.id]))),
        expire_renewal: AtomicUsize::new(2),
        hold_ack_until_renewal_refused: true,
        ..Default::default()
    });
    let fingerprint = AgentFingerprint::new();
    let executions = Arc::new(Mutex::new(Vec::new()));
    let svc = SchedulerServiceDefault::new(
        storage.clone(),
        create_shard_service_mock(),
        Arc::new(RecordingActiveWorkerAccess {
            fingerprint,
            executions: executions.clone(),
            complete_not_before: None,
            started: CancellationToken::new(),
        }),
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        1,
        Duration::from_millis(300),
        1,
        RetryConfig::max_attempts_3(),
        1,
        CancellationToken::new(),
    );
    let invocation_id = InvocationId(Uuid::new_v4());
    svc.schedule_with_id(
        id,
        jiff::Timestamp::now() - jiff::SignedDuration::from_secs(10),
        ScheduledAction::WakeInvocation {
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent("ack-lease-loss")),
            idempotency_key: invocation_id,
            target_worker_fingerprint: fingerprint,
        },
    )
    .await;

    let result = tokio::time::timeout(Duration::from_secs(3), svc.process(jiff::Timestamp::now()))
        .await
        .expect("renewal must continue while acknowledgement is pending");
    assert!(storage.renewal_refused.is_cancelled());
    assert!(
        result.is_err(),
        "lease loss must not hide an acknowledgement error"
    );
    assert_eq!(*executions.lock().unwrap(), vec![invocation_id.to_string()]);
    assert_eq!(storage.ack_failures.load(Ordering::SeqCst), 1);
    let remaining = claim_all(&storage.inner).await;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].schedule_id, id);
}

async fn create_oplog_service_with_presence(
    owned_agent_id: &OwnedAgentId,
) -> Arc<dyn OplogService> {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    // Only existence is needed; the recording worker mock performs the archive operation.
    indexed_storage
        .append(
            "test",
            "seed_archive_presence",
            "oplog",
            IndexedStorageNamespace::OpLog {
                owned_agent_id: owned_agent_id.clone(),
                agent_mode: AgentMode::Durable,
            },
            &owned_agent_id.storage_key(),
            1,
            vec![],
            None,
        )
        .await
        .unwrap();
    Arc::new(
        PrimaryOplogService::new(
            indexed_storage,
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            1024,
            32,
            RetryConfig::default(),
        )
        .await,
    )
}

#[test]
async fn each_batch_claims_with_fresh_time_but_archive_followup_uses_original_tick() {
    let storage = Arc::new(RecordingSchedulerStorage::default());
    let fingerprint = AgentFingerprint::new();
    let executions = Arc::new(Mutex::new(Vec::new()));
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("batch-clock"));
    let oplog_service = create_oplog_service_with_presence(&owned_agent_id).await;
    let svc = SchedulerServiceDefault::new(
        storage.clone(),
        create_shard_service_mock(),
        Arc::new(RecordingActiveWorkerAccess {
            fingerprint,
            executions: executions.clone(),
            complete_not_before: None,
            started: CancellationToken::new(),
        }),
        oplog_service,
        Duration::from_secs(1000),
        1,
        Duration::from_secs(30),
        2,
        RetryConfig::max_attempts_3(),
        1,
        CancellationToken::new(),
    );
    let tick = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(1);
    let next_after = Duration::from_secs(7200);
    let archive_action = ScheduledAction::ArchiveOplog {
        account_id: AccountId::new(),
        owned_agent_id: owned_agent_id.clone(),
        agent_mode: AgentMode::Durable,
        last_oplog_index: OplogIndex::from_u64(42),
        next_after,
    };
    let second_id = ScheduleId::fresh();
    *storage.insert_after_first_claim.lock().unwrap() = Some((second_id, archive_action.clone()));
    let invocation_id = InvocationId(Uuid::new_v4());
    let first_id = svc
        .schedule(
            tick,
            ScheduledAction::WakeInvocation {
                owned_agent_id,
                idempotency_key: invocation_id,
                target_worker_fingerprint: fingerprint,
            },
        )
        .await;

    svc.process(tick).await.unwrap();

    let batches = storage.claimed_batches.lock().unwrap().clone();
    let claim_times = storage.claim_times.lock().unwrap().clone();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].len(), 1);
    assert_eq!(batches[1].len(), 1);
    assert_eq!(batches[0][0].schedule_id, first_id);
    assert_eq!(batches[1][0].schedule_id, second_id);
    assert!(claim_times[0] > tick);
    assert!(batches[1][0].due_at.as_millisecond() > claim_times[0].as_millisecond());
    assert!(claim_times[1].as_millisecond() >= batches[1][0].due_at.as_millisecond());
    assert_eq!(
        *executions.lock().unwrap(),
        vec![invocation_id.to_string(), "archive".to_string()]
    );

    let followup_due = tick + jiff::SignedDuration::try_from(next_after).unwrap();
    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    // This probe is also well beyond both initial leases: neither original may remain.
    assert!(
        storage
            .inner
            .claim_due(
                followup_due - jiff::SignedDuration::from_millis(1),
                &assignment,
                10,
                Duration::from_secs(30),
            )
            .await
            .unwrap()
            .is_empty()
    );
    let followup = storage
        .inner
        .claim_due(followup_due, &assignment, 10, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(followup.len(), 1);
    assert_eq!(
        followup[0].due_at.as_millisecond(),
        followup_due.as_millisecond()
    );
    assert_eq!(followup[0].action, archive_action);
    assert_ne!(followup[0].schedule_id, first_id);
    assert_ne!(followup[0].schedule_id, second_id);
}

#[test]
async fn schedule_returns_uuid_backed_id_and_cancel_removes_entry() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let promise_service = create_promise_service_mock();
    let svc = create_scheduler(storage.clone(), promise_service).await;

    let action = complete_promise_action(promise(agent("inst1"), 101));
    let schedule_id = svc
        .schedule(
            jiff::Timestamp::now() - jiff::SignedDuration::from_mins(1),
            action,
        )
        .await;

    svc.cancel(schedule_id).await;

    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    let claimed = storage
        .claim_due(
            jiff::Timestamp::now(),
            &assignment,
            10,
            Duration::from_secs(30),
        )
        .await
        .unwrap();

    assert!(claimed.is_empty());
}

#[test]
async fn schedule_with_id_is_idempotent() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let promise_service = create_promise_service_mock();
    let svc = create_scheduler(storage.clone(), promise_service).await;

    let action = complete_promise_action(promise(agent("inst1"), 101));
    let schedule_id = ScheduleId::fresh();
    let due_at = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(1);

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

    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    let claimed = storage
        .claim_due(
            jiff::Timestamp::now(),
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
        jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
        complete_promise_action(promise_id.clone()),
    )
    .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();

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
            missing: false,
            failures: AtomicUsize::new(0),
            fail_before_completion: false,
            promise_service: promise_service.clone(),
            deliveries: Arc::default(),
        });
    let svc = SchedulerServiceDefault::new(
        storage.clone(),
        create_shard_service_mock(),
        worker_access,
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        64,
        CancellationToken::new(),
    );

    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("inst1"));
    for target_worker_fingerprint in [fingerprint, AgentFingerprint::new()] {
        svc.schedule(
            jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
            ScheduledAction::WakeInvocation {
                owned_agent_id: owned_agent_id.clone(),
                idempotency_key: InvocationId(Uuid::new_v4()),
                target_worker_fingerprint,
            },
        )
        .await;
    }

    svc.process(jiff::Timestamp::now()).await.unwrap();

    assert_eq!(enqueue_count.load(Ordering::SeqCst), 1);
    assert!(claim_all(&storage).await.is_empty());
}

#[test]
async fn scheduled_invoke_delivery_failure_is_not_acknowledged() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let attempts = Arc::new(AtomicUsize::new(0));
    let svc = SchedulerServiceDefault::new(
        storage.clone(),
        create_shard_service_mock(),
        Arc::new(FailingActivationWorkerAccess {
            attempts: attempts.clone(),
            promise_service: create_promise_service_mock(),
        }),
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        64,
        CancellationToken::new(),
    );
    svc.schedule(
        jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
        ScheduledAction::WakeInvocation {
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent("delivery-failure")),
            idempotency_key: InvocationId(Uuid::new_v4()),
            target_worker_fingerprint: AgentFingerprint::new(),
        },
    )
    .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(claim_all(&storage).await.len(), 1);
}

#[test]
async fn scheduled_stream_expiry_preserves_all_fences() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let deliveries = Arc::new(Mutex::new(Vec::new()));
    let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
        Arc::new(ExpiryWorkerAccessMock {
            deliveries: deliveries.clone(),
        });
    let svc = SchedulerServiceDefault::new(
        storage,
        create_shard_service_mock(),
        worker_access,
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        64,
        CancellationToken::new(),
    );
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("expiry"));
    let fingerprint = AgentFingerprint::new();
    let session_key = test_invocation_id("internal-key".into());
    svc.schedule(
        jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
        ScheduledAction::ExpireDurableStreamSession {
            owned_agent_id: owned_agent_id.clone(),
            target_agent_fingerprint: fingerprint,
            public_session_id: "public-id".into(),
            session_key,
            expected_deadline_millis: 1_689_577_500_000,
        },
    )
    .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();

    assert_eq!(
        deliveries.lock().unwrap().as_slice(),
        &[(
            owned_agent_id,
            fingerprint,
            "public-id".into(),
            session_key,
            1_689_577_500_000,
        )]
    );
}

/// Nothing else wakes an agent whose resume was already acknowledged, so an activation that
/// failed must leave the action in place to be claimed again.
#[test]
async fn failed_resume_is_not_acknowledged() {
    let _metric_guard = RESUME_METRIC_LOCK.lock().await;
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let attempts = Arc::new(AtomicUsize::new(0));
    let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
        Arc::new(FailingActivationWorkerAccess {
            attempts: attempts.clone(),
            promise_service: create_promise_service_mock(),
        });
    let svc = create_scheduler_with_worker_access(storage.clone(), worker_access).await;

    let schedule_id = svc
        .schedule(
            jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
            ScheduledAction::Resume {
                agent_created_by: AccountId::new(),
                owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent("inst1")),
                target_worker_fingerprint: AgentFingerprint::new(),
            },
        )
        .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    let claimed = claim_all(&storage).await;
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].schedule_id, schedule_id);
}

#[test]
async fn failed_archive_existence_check_is_not_acknowledged() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let oplog_storage = Arc::new(ReadCountingIndexedStorage::new());
    let oplog_service: Arc<dyn OplogService> = Arc::new(
        PrimaryOplogService::new(
            oplog_storage.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            1024,
            32,
            RetryConfig {
                max_attempts: 1,
                min_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                multiplier: 1.0,
                max_jitter_factor: None,
            },
        )
        .await,
    );
    let svc = create_scheduler_with_oplog(storage.clone(), oplog_service).await;
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("archive-exists"));
    oplog_storage.fail_next_exists();

    let schedule_id = svc
        .schedule(
            jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
            ScheduledAction::ArchiveOplog {
                account_id: AccountId::new(),
                owned_agent_id,
                agent_mode: AgentMode::Durable,
                last_oplog_index: OplogIndex::from_u64(42),
                next_after: Duration::from_secs(60),
            },
        )
        .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();

    let claimed = claim_all(&storage).await;
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].schedule_id, schedule_id);
}

/// The counterpart: a resume that did activate its worker is acknowledged and gone.
#[test]
async fn successful_resume_is_acknowledged() {
    let _metric_guard = RESUME_METRIC_LOCK.lock().await;
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let svc =
        create_scheduler_with_worker_access(storage.clone(), create_worker_access_mock()).await;

    svc.schedule(
        jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
        ScheduledAction::Resume {
            agent_created_by: AccountId::new(),
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent("inst1")),
            target_worker_fingerprint: AgentFingerprint::new(),
        },
    )
    .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();

    assert!(claim_all(&storage).await.is_empty());
}

/// Completing the promise is only half of the action: an agent that was not woken is still
/// suspended on it, so a failed activation keeps the action claimable. The retry re-completes
/// the (already completed) promise, which is a no-op.
#[test]
async fn failed_activation_after_promise_completion_is_not_acknowledged() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let attempts = Arc::new(AtomicUsize::new(0));
    let promise_service = create_promise_service_mock();
    let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
        Arc::new(FailingActivationWorkerAccess {
            attempts: attempts.clone(),
            promise_service: promise_service.clone(),
        });
    let svc = create_scheduler_with(storage.clone(), worker_access).await;

    let promise_id = promise(agent("inst1"), 101);
    svc.schedule(
        jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
        complete_promise_action(promise_id.clone()),
    )
    .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();

    assert!(promise_service.all_completed().await.contains(&promise_id));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(claim_all(&storage).await.len(), 1);
}

#[test]
async fn scheduled_actions_for_distinct_agents_are_processed_concurrently() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let fingerprint = AgentFingerprint::new();
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
        Arc::new(DelayedActiveWorkerAccessMock {
            fingerprint,
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: max_in_flight.clone(),
        });
    let svc = SchedulerServiceDefault::new(
        storage,
        create_shard_service_mock(),
        worker_access,
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        2,
        CancellationToken::new(),
    );

    let due_at = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3);
    for name in ["agent-1", "agent-2"] {
        svc.schedule(
            due_at,
            ScheduledAction::WakeInvocation {
                owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent(name)),
                idempotency_key: InvocationId(Uuid::new_v4()),
                target_worker_fingerprint: fingerprint,
            },
        )
        .await;
    }

    svc.process(jiff::Timestamp::now()).await.unwrap();

    assert_eq!(max_in_flight.load(Ordering::SeqCst), 2);
}

#[test]
async fn scheduled_actions_for_the_same_agent_are_processed_serially() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let fingerprint = AgentFingerprint::new();
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
        Arc::new(DelayedActiveWorkerAccessMock {
            fingerprint,
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: max_in_flight.clone(),
        });
    let svc = SchedulerServiceDefault::new(
        storage,
        create_shard_service_mock(),
        worker_access,
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        2,
        CancellationToken::new(),
    );

    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("agent-1"));
    let due_at = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3);
    for _ in 0..2 {
        svc.schedule(
            due_at,
            ScheduledAction::WakeInvocation {
                owned_agent_id: owned_agent_id.clone(),
                idempotency_key: InvocationId(Uuid::new_v4()),
                target_worker_fingerprint: fingerprint,
            },
        )
        .await;
    }

    svc.process(jiff::Timestamp::now()).await.unwrap();

    assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn scheduler_preserves_claim_order_and_aborts_after_ack_failures(
        actions_per_agent in prop::collection::vec(1usize..5, 1..5),
        claim_batch_size in 1u32..8,
        max_batches_per_tick in 1u32..5,
        max_concurrent_action_processing in 1u32..8,
        ack_failure_choices in prop::collection::vec(any::<bool>(), 0..20),
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let failed_ack_ids = Arc::new(Mutex::new(HashSet::new()));
            let failed_acknowledgements = Arc::new(Mutex::new(HashSet::new()));
            let claimed_batches = Arc::new(Mutex::new(Vec::new()));
            let storage = Arc::new(RecordingSchedulerStorage {
                inner: Arc::new(InMemorySchedulerStorage::new()),
                failed_ack_ids: failed_ack_ids.clone(),
                failed_acknowledgements: failed_acknowledgements.clone(),
                claimed_batches: claimed_batches.clone(),
                ack_failures: AtomicUsize::new(0),
                claims_after_ack_failure: AtomicUsize::new(0),
                ..Default::default()
            });
            let executions = Arc::new(Mutex::new(Vec::new()));
            let fingerprint = AgentFingerprint::new();
            let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
                Arc::new(RecordingActiveWorkerAccess {
                    fingerprint,
                    executions: executions.clone(),
                    complete_not_before: None,
                    started: CancellationToken::new(),
                });
            let svc = SchedulerServiceDefault::new(
                storage.clone(),
                create_shard_service_mock(),
                worker_access,
                create_oplog_service_mock().await,
                Duration::from_secs(1000),
                claim_batch_size,
                Duration::from_secs(30),
                max_batches_per_tick,
                RetryConfig::max_attempts_3(),
                max_concurrent_action_processing,
                CancellationToken::new(),
            );

            let due_at = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3);
            let mut scheduled = HashMap::new();
            let mut schedule_ids = Vec::new();
            for (agent_index, action_count) in actions_per_agent.iter().enumerate() {
                let owned_agent_id = OwnedAgentId::new(
                    EnvironmentId::new(),
                    &agent(&format!("agent-{agent_index}")),
                );
                for _ in 0..*action_count {
                    let execution_id = InvocationId(Uuid::new_v4());
                    let schedule_id = svc
                        .schedule(
                            due_at,
                            ScheduledAction::WakeInvocation {
                                owned_agent_id: owned_agent_id.clone(),
                                idempotency_key: execution_id,
                                target_worker_fingerprint: fingerprint,
                            },
                        )
                        .await;
                    scheduled.insert(schedule_id.id, (owned_agent_id.clone(), execution_id.to_string()));
                    schedule_ids.push(schedule_id.id);
                }
            }

            for (index, schedule_id) in schedule_ids.iter().enumerate() {
                if ack_failure_choices
                    .get(index % ack_failure_choices.len().max(1))
                    .copied()
                    .unwrap_or(false)
                {
                    failed_ack_ids.lock().unwrap().insert(*schedule_id);
                }
            }

            let result = svc
                .process(jiff::Timestamp::now())
                .await;
            let ack_failures = storage.ack_failures.load(Ordering::SeqCst);
            prop_assert_eq!(result.is_err(), ack_failures > 0);
            prop_assert_eq!(
                storage.claims_after_ack_failure.load(Ordering::SeqCst),
                0,
                "scheduler claimed another batch after an acknowledgement failure"
            );

            let claimed_batches = claimed_batches.lock().unwrap().clone();
            let failed_acknowledgements = failed_acknowledgements.lock().unwrap().clone();
            let executions = executions.lock().unwrap().clone();
            let execution_set: HashSet<_> = executions.iter().cloned().collect();
            prop_assert_eq!(
                execution_set.len(),
                executions.len(),
                "an action executed twice before its lease expired"
            );

            let mut claimed_by_agent: HashMap<OwnedAgentId, Vec<String>> = HashMap::new();
            let mut executed_by_agent: HashMap<OwnedAgentId, Vec<String>> = HashMap::new();
            let failed_execution_ids: HashSet<_> = failed_acknowledgements
                .iter()
                .map(|schedule_id| scheduled[schedule_id].1.clone())
                .collect();
            for batch in &claimed_batches {
                for claimed in batch {
                    let (owned_agent_id, execution_id) = &scheduled[&claimed.schedule_id.id];
                    claimed_by_agent
                        .entry(owned_agent_id.clone())
                        .or_default()
                        .push(execution_id.clone());
                }
            }
            for execution_id in executions {
                let (owned_agent_id, _) = scheduled
                    .values()
                    .find(|(_, id)| id == &execution_id)
                    .unwrap();
                executed_by_agent
                    .entry(owned_agent_id.clone())
                    .or_default()
                    .push(execution_id);
            }
            for (owned_agent_id, claimed) in &claimed_by_agent {
                let expected_len = claimed
                    .iter()
                    .position(|execution_id| failed_execution_ids.contains(execution_id))
                    .map(|index| index + 1)
                    .unwrap_or(claimed.len());
                let executions = executed_by_agent
                    .get(owned_agent_id)
                    .cloned()
                    .unwrap_or_default();
                prop_assert_eq!(
                    executions,
                    &claimed[..expected_len],
                    "agent {} actions executed out of claim order",
                    owned_agent_id
                );
            }

            for batch in claimed_batches {
                for failed in batch.iter().filter(|action| {
                    failed_acknowledgements.contains(&action.schedule_id.id)
                }) {
                    let (failed_agent_id, _) = &scheduled[&failed.schedule_id.id];
                    let sibling_agents: HashSet<_> = batch
                        .iter()
                        .map(|action| scheduled[&action.schedule_id.id].0.clone())
                        .filter(|agent_id| agent_id != failed_agent_id)
                        .collect();
                    for sibling_agent_id in sibling_agents {
                        let sibling = batch
                            .iter()
                            .find(|action| {
                                scheduled[&action.schedule_id.id].0 == sibling_agent_id
                            })
                            .unwrap();
                        let (_, execution_id) = &scheduled[&sibling.schedule_id.id];
                        prop_assert!(
                            execution_set.contains(execution_id),
                            "acknowledgement failure cancelled sibling agent lane"
                        );
                    }
                }
            }

            Ok(())
        }).unwrap();
    }
}

#[test]
async fn terminal_scheduled_ephemeral_failure_is_acknowledged_without_redelivery() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let enqueue_count = Arc::new(AtomicUsize::new(0));
    let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
        Arc::new(FailingWakeWorkerAccess {
            terminal: true,
            fail_first_transiently: false,
            attempts: enqueue_count.clone(),
        });
    let svc = SchedulerServiceDefault::new(
        storage.clone(),
        create_shard_service_mock(),
        worker_access,
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        64,
        CancellationToken::new(),
    );

    svc.schedule(
        jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
        ScheduledAction::WakeInvocation {
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent("ephemeral")),
            idempotency_key: InvocationId(Uuid::new_v4()),
            target_worker_fingerprint: AgentFingerprint::new(),
        },
    )
    .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();
    assert!(claim_all(&storage).await.is_empty());
    svc.process(jiff::Timestamp::now()).await.unwrap();

    assert_eq!(enqueue_count.load(Ordering::SeqCst), 1);
}

#[test]
async fn transient_invalid_request_does_not_drop_scheduled_ephemeral_invocation() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let enqueue_count = Arc::new(AtomicUsize::new(0));
    let worker_access: Arc<dyn SchedulerWorkerAccess + Send + Sync> =
        Arc::new(FailingWakeWorkerAccess {
            terminal: false,
            fail_first_transiently: true,
            attempts: enqueue_count.clone(),
        });
    let svc = SchedulerServiceDefault::new(
        storage.clone(),
        create_shard_service_mock(),
        worker_access,
        create_oplog_service_mock().await,
        Duration::from_secs(1000),
        100,
        Duration::from_secs(30),
        10,
        RetryConfig::max_attempts_3(),
        64,
        CancellationToken::new(),
    );

    svc.schedule(
        jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
        ScheduledAction::WakeInvocation {
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent("ephemeral")),
            idempotency_key: InvocationId(Uuid::new_v4()),
            target_worker_fingerprint: AgentFingerprint::new(),
        },
    )
    .await;

    svc.process(jiff::Timestamp::now()).await.unwrap();
    assert_eq!(enqueue_count.load(Ordering::SeqCst), 1);
    process_retry(&svc, &storage).await;

    assert_eq!(enqueue_count.load(Ordering::SeqCst), 2);
    assert!(claim_all(&storage).await.is_empty());
}

#[test]
async fn leases_prevent_duplicate_claims_until_expiry() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let action = complete_promise_action(promise(agent("inst1"), 101));
    let due_at = jiff::Timestamp::from_str("2023-07-17T10:05:00Z").unwrap();
    storage
        .insert(
            ScheduleId::fresh(),
            due_at,
            ShardId::new(0),
            &serialize(&action).unwrap(),
        )
        .await
        .unwrap();

    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    let now = jiff::Timestamp::from_str("2023-07-17T10:06:00Z").unwrap();

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
            jiff::Timestamp::from_str("2023-07-17T10:06:31Z").unwrap(),
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
    let due_at = jiff::Timestamp::from_str("2023-07-17T10:05:00Z").unwrap();
    storage
        .insert(
            ScheduleId::fresh(),
            due_at,
            ShardId::new(0),
            &serialize(&action).unwrap(),
        )
        .await
        .unwrap();

    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    let first = storage
        .claim_due(
            jiff::Timestamp::from_str("2023-07-17T10:06:00Z").unwrap(),
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
            jiff::Timestamp::from_str("2023-07-17T10:06:31Z").unwrap(),
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
            .ack(
                &first.schedule_id,
                first.lease_owner,
                jiff::Timestamp::from_str("2023-07-17T10:06:31Z").unwrap()
            )
            .await
            .unwrap()
    );
    assert!(
        storage
            .ack(
                &second.schedule_id,
                second.lease_owner,
                jiff::Timestamp::from_str("2023-07-17T10:06:31Z").unwrap()
            )
            .await
            .unwrap()
    );
}

#[test]
async fn lease_extension_prevents_reclaim_until_extended_deadline() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let action = complete_promise_action(promise(agent("inst1"), 101));
    let due_at = jiff::Timestamp::from_str("2023-07-17T10:05:00Z").unwrap();
    storage
        .insert(
            ScheduleId::fresh(),
            due_at,
            ShardId::new(0),
            &serialize(&action).unwrap(),
        )
        .await
        .unwrap();

    let assignment = ShardAssignment::unexpiring(1, [ShardId::new(0)]);
    let claimed = storage
        .claim_due(
            jiff::Timestamp::from_str("2023-07-17T10:06:00Z").unwrap(),
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
                jiff::Timestamp::from_str("2023-07-17T10:07:00Z").unwrap(),
                jiff::Timestamp::from_str("2023-07-17T10:06:15Z").unwrap(),
            )
            .await
            .unwrap()
    );

    assert!(
        storage
            .claim_due(
                jiff::Timestamp::from_str("2023-07-17T10:06:31Z").unwrap(),
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
                jiff::Timestamp::from_str("2023-07-17T10:07:01Z").unwrap(),
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
    let due_at = jiff::Timestamp::from_str("2023-07-17T10:05:00Z").unwrap();
    storage
        .insert(
            ScheduleId::fresh(),
            due_at,
            shard,
            &serialize(&action).unwrap(),
        )
        .await
        .unwrap();

    let unassigned = ShardAssignment::unexpiring(2, [other_shard]);
    let assigned = ShardAssignment::unexpiring(2, [shard]);
    let now = jiff::Timestamp::from_str("2023-07-17T10:06:00Z").unwrap();

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
async fn due_backlog_includes_leased_actions_until_acknowledged() {
    let storage = Arc::new(InMemorySchedulerStorage::new());
    let action = complete_promise_action(promise(agent("inst1"), 101));
    let shard = ShardId::new(0);
    let assignment = ShardAssignment::unexpiring(1, [shard]);
    let due_at = jiff::Timestamp::from_str("2023-07-17T10:05:00Z").unwrap();
    let now = jiff::Timestamp::from_str("2023-07-17T10:06:00Z").unwrap();
    let schedule_id = ScheduleId::fresh();
    storage
        .insert(schedule_id, due_at, shard, &serialize(&action).unwrap())
        .await
        .unwrap();

    assert_eq!(storage.count_due(now, &assignment).await.unwrap(), 1);

    let claimed = storage
        .claim_due(now, &assignment, 1, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(storage.count_due(now, &assignment).await.unwrap(), 1);

    assert!(
        storage
            .ack(&schedule_id, claimed[0].lease_owner, now)
            .await
            .unwrap()
    );
    assert_eq!(storage.count_due(now, &assignment).await.unwrap(), 0);
}

#[test]
fn shard_id_from_routing_hash_handles_negative_hashes() {
    assert_eq!(ShardId::from_routing_hash(-i64::MAX, 10), ShardId::new(7));
}

fn exact_actions(
    owned_agent_id: &OwnedAgentId,
    fingerprint: AgentFingerprint,
) -> Vec<ScheduledAction> {
    vec![
        ScheduledAction::WakeInvocation {
            owned_agent_id: owned_agent_id.clone(),
            idempotency_key: InvocationId(Uuid::new_v4()),
            target_worker_fingerprint: fingerprint,
        },
        ScheduledAction::Resume {
            agent_created_by: AccountId::new(),
            owned_agent_id: owned_agent_id.clone(),
            target_worker_fingerprint: fingerprint,
        },
        ScheduledAction::CompletePromise {
            account_id: AccountId::new(),
            environment_id: owned_agent_id.environment_id,
            promise_id: promise(owned_agent_id.agent_id, 101),
            target_worker_fingerprint: fingerprint,
        },
    ]
}

#[test]
async fn all_exact_deliveries_acknowledge_missing_stale_and_matching_targets() {
    let _metric_guard = RESUME_METRIC_LOCK.lock().await;
    for (missing, matches) in [(true, true), (false, false), (false, true)] {
        let storage = Arc::new(InMemorySchedulerStorage::new());
        let fingerprint = AgentFingerprint::new();
        let deliveries = Arc::new(Mutex::new(vec![]));
        let count = Arc::new(AtomicUsize::new(0));
        let promise_service = create_promise_service_mock();
        let worker_access = Arc::new(ActiveWorkerAccessMock {
            fingerprint,
            enqueue_count: count.clone(),
            missing,
            failures: AtomicUsize::new(0),
            fail_before_completion: false,
            promise_service: promise_service.clone(),
            deliveries: deliveries.clone(),
        });
        let svc = create_scheduler_with_worker_access(storage.clone(), worker_access).await;
        let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("exact-target"));
        let expected = if matches {
            fingerprint
        } else {
            AgentFingerprint::new()
        };
        for action in exact_actions(&owned_agent_id, expected) {
            svc.schedule(
                jiff::Timestamp::now() - jiff::SignedDuration::from_hours(3),
                action,
            )
            .await;
        }
        svc.process(jiff::Timestamp::now()).await.unwrap();
        assert!(claim_all(&storage).await.is_empty());
        if !missing && matches {
            assert_eq!(count.load(Ordering::SeqCst), 3);
            let mut delivered = deliveries.lock().unwrap().clone();
            delivered.sort_unstable();
            assert_eq!(
                delivered,
                vec!["complete_promise", "resume", "wake_invocation"]
            );
            assert!(
                promise_service
                    .all_completed()
                    .await
                    .contains(&promise(owned_agent_id.agent_id, 101))
            );
        } else {
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(deliveries.lock().unwrap().is_empty());
            assert!(promise_service.all_completed().await.is_empty());
        }
    }
}

#[test]
async fn all_exact_deliveries_retry_failures_and_acknowledge_successful_retry() {
    let _metric_guard = RESUME_METRIC_LOCK.lock().await;
    let owned_agent_id = OwnedAgentId::new(EnvironmentId::new(), &agent("retry-target"));
    let fingerprint = AgentFingerprint::new();
    for action in exact_actions(&owned_agent_id, fingerprint) {
        for fail_before_completion in [true, false] {
            let storage = Arc::new(InMemorySchedulerStorage::new());
            let promise_service = create_promise_service_mock();
            let deliveries = Arc::new(Mutex::new(vec![]));
            let attempts = Arc::new(AtomicUsize::new(0));
            let worker_access = Arc::new(ActiveWorkerAccessMock {
                fingerprint,
                enqueue_count: attempts.clone(),
                missing: false,
                failures: AtomicUsize::new(1),
                fail_before_completion,
                promise_service: promise_service.clone(),
                deliveries: deliveries.clone(),
            });
            let svc = create_scheduler_with_worker_access(storage.clone(), worker_access).await;
            let now = jiff::Timestamp::now();
            let id = svc.schedule(now, action.clone()).await;
            svc.process(now).await.unwrap();
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
            assert_eq!(
                storage
                    .count_due(now, &ShardAssignment::unexpiring(1, [ShardId::new(0)]))
                    .await
                    .unwrap(),
                1
            );
            if matches!(action, ScheduledAction::CompletePromise { .. }) {
                assert_eq!(
                    promise_service.all_completed().await.is_empty(),
                    fail_before_completion
                );
            }
            process_retry(&svc, &storage).await;
            assert_eq!(attempts.load(Ordering::SeqCst), 2);
            assert!(
                claim_all(&storage).await.is_empty(),
                "retry not acknowledged for {id}"
            );
            assert_eq!(
                deliveries.lock().unwrap().len(),
                if fail_before_completion { 1 } else { 2 }
            );
            if matches!(action, ScheduledAction::CompletePromise { .. }) {
                assert!(
                    promise_service
                        .all_completed()
                        .await
                        .contains(&promise(owned_agent_id.agent_id, 101))
                );
            }
        }
    }
}

fn test_invocation_id(value: String) -> InvocationId {
    InvocationId(Uuid::new_v5(&Uuid::NAMESPACE_OID, value.as_bytes()))
}

mod lease_renewal_isolation {
    use super::*;
    use test_r::test;

    #[derive(Debug)]
    struct SlowRenewalStorage {
        inner: InMemorySchedulerStorage,
        slow_id: ScheduleId,
        slow_calls: AtomicUsize,
        delay: Duration,
    }

    #[async_trait]
    #[golem_async_trait::region_simple]
    impl SchedulerStorage for SlowRenewalStorage {
        async fn insert(
            &self,
            id: ScheduleId,
            due: jiff::Timestamp,
            shard: ShardId,
            action: &[u8],
        ) -> Result<(), SchedulerStorageError> {
            self.inner.insert(id, due, shard, action).await
        }

        async fn cancel(&self, id: &ScheduleId) -> Result<(), SchedulerStorageError> {
            self.inner.cancel(id).await
        }

        async fn claim_due(
            &self,
            now: jiff::Timestamp,
            assignment: &ShardAssignment,
            limit: u32,
            ttl: Duration,
        ) -> Result<Vec<ClaimedScheduledAction>, SchedulerStorageError> {
            self.inner.claim_due(now, assignment, limit, ttl).await
        }

        async fn count_due(
            &self,
            now: jiff::Timestamp,
            assignment: &ShardAssignment,
        ) -> Result<u64, SchedulerStorageError> {
            self.inner.count_due(now, assignment).await
        }

        async fn ack(
            &self,
            id: &ScheduleId,
            owner: Uuid,
            now: jiff::Timestamp,
        ) -> Result<bool, SchedulerStorageError> {
            self.inner.ack(id, owner, now).await
        }

        async fn extend_lease(
            &self,
            id: &ScheduleId,
            owner: Uuid,
            until: jiff::Timestamp,
            now: jiff::Timestamp,
        ) -> Result<bool, SchedulerStorageError> {
            if *id == self.slow_id && self.slow_calls.fetch_add(1, Ordering::SeqCst) == 1 {
                // Only one action's first heartbeat waits. The wait is shorter than
                // the 600ms lease, and every other storage operation is immediate.
                tokio::time::sleep(self.delay).await;
            }
            self.inner.extend_lease(id, owner, until, now).await
        }
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn one_slow_renewal_must_not_expire_healthy_sibling() {
        assert_healthy_sibling_completes(Duration::from_millis(500)).await;
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn control_immediate_renewals_keep_healthy_sibling_alive() {
        assert_healthy_sibling_completes(Duration::ZERO).await;
    }

    async fn assert_healthy_sibling_completes(delay: Duration) {
        let slow_id = ScheduleId::fresh();
        let storage = Arc::new(SlowRenewalStorage {
            inner: InMemorySchedulerStorage::new(),
            slow_id,
            slow_calls: AtomicUsize::new(0),
            delay,
        });
        let executions = Arc::new(Mutex::new(Vec::new()));
        let now = jiff::Timestamp::now();
        let fingerprint = AgentFingerprint::new();
        let svc = SchedulerServiceDefault::new(
            storage.clone(),
            create_shard_service_mock(),
            Arc::new(RecordingActiveWorkerAccess {
                fingerprint,
                executions: executions.clone(),
                complete_not_before: Some(now + jiff::SignedDuration::from_millis(1500)),
                started: CancellationToken::new(),
            }),
            create_oplog_service_mock().await,
            Duration::from_secs(1000),
            2,
            Duration::from_millis(600),
            1,
            RetryConfig::max_attempts_3(),
            2,
            CancellationToken::new(),
        );
        let healthy_invocation = InvocationId(Uuid::new_v4());
        for (id, name, invocation) in [
            (slow_id, "slow-renewal", InvocationId(Uuid::new_v4())),
            (ScheduleId::fresh(), "healthy-renewal", healthy_invocation),
        ] {
            svc.schedule_with_id(
                id,
                now - jiff::SignedDuration::from_secs(1),
                ScheduledAction::WakeInvocation {
                    owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent(name)),
                    idempotency_key: invocation,
                    target_worker_fingerprint: fingerprint,
                },
            )
            .await;
        }

        svc.process(now).await.unwrap();

        assert!(
            executions
                .lock()
                .unwrap()
                .contains(&healthy_invocation.to_string()),
            "a sibling with immediate renewals lost its lease because another renewal waited less than one lease TTL"
        );
    }
}
