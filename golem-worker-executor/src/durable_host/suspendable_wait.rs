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

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use chrono::{DateTime, Utc};
use golem_common::model::Timestamp;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, EphemeralSleepTooLongError};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};

use crate::durable_host::WakeupScheduler;
use crate::metrics::ephemeral::{dec_promise_waiting, inc_promise_waiting};
use crate::services::golem_config::SuspendConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkOutcome {
    Ready,
    SuspendWorker(Timestamp),
    /// The worker's interrupt signal fired while parked (a real interrupt or the synthetic
    /// invocation-deadline wakeup). The caller must abandon its durable call handle(s) and
    /// propagate the kind directly so it classifies as `TrapType::Interrupt`.
    Interrupted(InterruptKind),
    EphemeralTooLong {
        requested_nanos: u64,
        max_nanos: u64,
    },
}

pub(crate) struct SuspendableWaitContext {
    pub(crate) wait_id: u64,
    pub(crate) agent_mode: AgentMode,
    pub(crate) suspend: SuspendConfig,
    pub(crate) wait_deadline: Option<DateTime<Utc>>,
    pub(crate) suspendable_waits: SuspendableWaitRegistry,
    pub(crate) wakeup_scheduler: WakeupScheduler,
}

pub(crate) async fn park_suspendable_wait<R, Ready, F, Q, N>(
    context: SuspendableWaitContext,
    interrupt: Pin<Box<dyn Future<Output = InterruptKind> + Send>>,
    ready: R,
    final_ready: F,
    safe_to_suspend: Q,
    remaining: N,
) -> Result<ParkOutcome, WorkerExecutorError>
where
    R: FnMut() -> Ready,
    Ready: Future<Output = ()>,
    F: FnMut() -> bool,
    Q: FnMut() -> bool,
    N: FnMut() -> Option<Duration>,
{
    let _registration = (context.agent_mode == AgentMode::Durable).then(|| {
        SuspendableWaitRegistration::new(
            context.wait_id,
            context.wait_deadline,
            context.suspendable_waits.clone(),
        )
    });
    park_registered_wait(
        context,
        interrupt,
        ready,
        final_ready,
        safe_to_suspend,
        remaining,
    )
    .await
}

pub(crate) async fn park_registered_wait<R, Ready, F, Q, N>(
    context: SuspendableWaitContext,
    mut interrupt: Pin<Box<dyn Future<Output = InterruptKind> + Send>>,
    mut ready: R,
    mut final_ready: F,
    mut safe_to_suspend: Q,
    mut remaining: N,
) -> Result<ParkOutcome, WorkerExecutorError>
where
    R: FnMut() -> Ready,
    Ready: Future<Output = ()>,
    F: FnMut() -> bool,
    Q: FnMut() -> bool,
    N: FnMut() -> Option<Duration>,
{
    let requested_nanos = remaining().map(std_duration_to_nanos).unwrap_or(u64::MAX);

    if context.agent_mode == AgentMode::Ephemeral {
        let max_nanos = std_duration_to_nanos(context.suspend.ephemeral_max_sleep);
        if context.wait_deadline.is_some() {
            if requested_nanos >= max_nanos {
                return Ok(ParkOutcome::EphemeralTooLong {
                    requested_nanos,
                    max_nanos,
                });
            }
            return tokio::select! {
                _ = ready() => Ok(ParkOutcome::Ready),
                kind = &mut interrupt => Ok(ParkOutcome::Interrupted(kind)),
            };
        }

        let _promise_waiting = PromiseWaiting::new(true);
        tokio::select! {
            _ = ready() => Ok(ParkOutcome::Ready),
            kind = &mut interrupt => Ok(ParkOutcome::Interrupted(kind)),
            _ = tokio::time::sleep(context.suspend.ephemeral_max_sleep) => {
                Ok(ParkOutcome::EphemeralTooLong {
                    requested_nanos: max_nanos,
                    max_nanos,
                })
            }
        }
    } else {
        let mut first_tick = true;
        loop {
            let tick_after = if first_tick {
                first_tick = false;
                context.suspend.wait_suspend_grace
            } else {
                context.suspend.wait_suspend_check_interval
            };

            tokio::select! {
                _ = ready() => return Ok(ParkOutcome::Ready),
                kind = &mut interrupt => return Ok(ParkOutcome::Interrupted(kind)),
                _ = tokio::time::sleep(tick_after) => {}
            }

            if final_ready() {
                return Ok(ParkOutcome::Ready);
            }

            if let Some(remaining) = remaining()
                && remaining < context.suspend.suspend_after
            {
                return tokio::select! {
                    _ = ready() => Ok(ParkOutcome::Ready),
                    kind = &mut interrupt => Ok(ParkOutcome::Interrupted(kind)),
                };
            }

            if safe_to_suspend() {
                if final_ready() {
                    return Ok(ParkOutcome::Ready);
                }
                if poll_ready_once(ready()).await {
                    return Ok(ParkOutcome::Ready);
                }
                tokio::task::yield_now().await;
                if final_ready() {
                    return Ok(ParkOutcome::Ready);
                }
                if poll_ready_once(ready()).await {
                    return Ok(ParkOutcome::Ready);
                }
                // The yield above lets the store's event loop drive other guest tasks, which may
                // have started new live host calls; suspending now would drop them mid-flight,
                // so re-check and keep parking if suspension is no longer safe.
                if !safe_to_suspend() {
                    continue;
                }

                let mut scheduled_deadline = next_wakeup(
                    Utc::now(),
                    &context.suspendable_waits.lock().unwrap(),
                    context.suspend.wait_suspend_check_interval,
                );
                let suspend_at = Timestamp::now_utc();
                loop {
                    context
                        .wakeup_scheduler
                        .sleep_until(scheduled_deadline)
                        .await?;
                    // Scheduling the wakeup awaits (oplog index read, promise creation, schedule
                    // write) — another window in which waits or unsafe work can appear. Re-check
                    // both before suspending, and schedule again if a new wait needs an earlier
                    // wakeup. Already-scheduled later wakeups are harmless.
                    if final_ready() {
                        return Ok(ParkOutcome::Ready);
                    }
                    if !safe_to_suspend() {
                        break;
                    }

                    let next_deadline = next_wakeup(
                        Utc::now(),
                        &context.suspendable_waits.lock().unwrap(),
                        context.suspend.wait_suspend_check_interval,
                    );
                    if next_deadline >= scheduled_deadline {
                        return Ok(ParkOutcome::SuspendWorker(suspend_at));
                    }
                    scheduled_deadline = next_deadline;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuspendableWait {
    Deadline(Option<DateTime<Utc>>),
    Rpc(Duration),
}

pub(crate) type SuspendableWaitRegistry = Arc<Mutex<BTreeMap<u64, SuspendableWait>>>;

fn next_wakeup(
    now: DateTime<Utc>,
    waits: &BTreeMap<u64, SuspendableWait>,
    fallback: Duration,
) -> DateTime<Utc> {
    waits
        .values()
        .filter_map(|wait| match wait {
            SuspendableWait::Deadline(deadline) => *deadline,
            SuspendableWait::Rpc(resume_after) => {
                Some(now + chrono::Duration::from_std(*resume_after).unwrap())
            }
        })
        .min()
        .unwrap_or_else(|| now + chrono::Duration::from_std(fallback).unwrap())
}

async fn poll_ready_once<F>(future: F) -> bool
where
    F: Future<Output = ()>,
{
    let mut future = pin!(future);
    std::future::poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Ready(()) => Poll::Ready(true),
        Poll::Pending => Poll::Ready(false),
    })
    .await
}

pub(crate) struct SuspendableWaitRegistration {
    wait_id: u64,
    wait: SuspendableWait,
    suspendable_waits: SuspendableWaitRegistry,
}

impl SuspendableWaitRegistration {
    pub(crate) fn new(
        wait_id: u64,
        deadline: Option<DateTime<Utc>>,
        suspendable_waits: SuspendableWaitRegistry,
    ) -> Self {
        let wait = SuspendableWait::Deadline(deadline);
        suspendable_waits.lock().unwrap().insert(wait_id, wait);
        Self {
            wait_id,
            wait,
            suspendable_waits,
        }
    }

    pub(crate) fn rpc(
        wait_id: u64,
        resume_after: Duration,
        suspendable_waits: SuspendableWaitRegistry,
    ) -> Self {
        let wait = SuspendableWait::Rpc(resume_after);
        suspendable_waits.lock().unwrap().insert(wait_id, wait);
        Self {
            wait_id,
            wait,
            suspendable_waits,
        }
    }
}

impl Drop for SuspendableWaitRegistration {
    fn drop(&mut self) {
        let mut suspendable_waits = self.suspendable_waits.lock().unwrap();
        if suspendable_waits.get(&self.wait_id) == Some(&self.wait) {
            suspendable_waits.remove(&self.wait_id);
        }
    }
}

pub(crate) struct PromiseWaiting(bool);

impl PromiseWaiting {
    pub(crate) fn new(enabled: bool) -> Self {
        if enabled {
            inc_promise_waiting();
        }
        Self(enabled)
    }
}

impl Drop for PromiseWaiting {
    fn drop(&mut self) {
        if self.0 {
            dec_promise_waiting();
        }
    }
}

pub(crate) fn ephemeral_sleep_too_long_error(
    requested_nanos: u64,
    max_nanos: u64,
) -> wasmtime::Error {
    wasmtime::Error::from_anyhow(anyhow::anyhow!(WorkerExecutorError::InvocationFailed {
        error: AgentError::EphemeralSleepTooLong(EphemeralSleepTooLongError {
            requested_nanos,
            max_nanos,
        }),
        stderr: String::new(),
    }))
}

pub(crate) fn std_duration_to_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

pub(crate) fn chrono_duration_to_nanos(duration: chrono::Duration) -> u64 {
    duration
        .to_std()
        .map(std_duration_to_nanos)
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::oplog::{CommitLevel, Oplog, OrderedOplogStart};
    use crate::services::promise::{PromiseHandle, PromiseService};
    use crate::services::scheduler::SchedulerService;
    use async_trait::async_trait;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::{OplogEntry, OplogIndex, PayloadId, RawOplogPayload};
    use golem_common::model::{AgentId, OwnedAgentId, PromiseId, ScheduleId, ScheduledAction};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::{Debug, Formatter};
    use std::future::pending;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use test_r::test;

    struct UnusedPromiseService;

    #[async_trait]
    impl PromiseService for UnusedPromiseService {
        async fn create(
            &self,
            _agent_id: &AgentId,
            _oplog_idx: OplogIndex,
        ) -> Result<PromiseId, WorkerExecutorError> {
            unreachable!("promise service is unused by this test")
        }

        async fn poll(&self, _promise_id: PromiseId) -> Result<PromiseHandle, WorkerExecutorError> {
            unreachable!("promise service is unused by this test")
        }

        async fn complete(
            &self,
            _promise_id: PromiseId,
            _data: Vec<u8>,
        ) -> Result<bool, WorkerExecutorError> {
            unreachable!("promise service is unused by this test")
        }

        async fn cleanup(&self) {}
    }

    struct UnusedSchedulerService;

    #[async_trait]
    impl SchedulerService for UnusedSchedulerService {
        async fn schedule(&self, _time: DateTime<Utc>, _action: ScheduledAction) -> ScheduleId {
            unreachable!("scheduler is unused by promise waits")
        }

        async fn schedule_with_id(
            &self,
            _schedule_id: ScheduleId,
            _time: DateTime<Utc>,
            _action: ScheduledAction,
        ) -> ScheduleId {
            unreachable!("scheduler is unused by promise waits")
        }

        async fn cancel(&self, _id: ScheduleId) {
            unreachable!("scheduler is unused by this test")
        }
    }

    struct UnusedOplog;

    impl Debug for UnusedOplog {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("UnusedOplog").finish()
        }
    }

    #[async_trait]
    impl Oplog for UnusedOplog {
        async fn add(&self, _entry: OplogEntry) -> OplogIndex {
            unreachable!("oplog is unused by promise waits")
        }

        fn enqueue_add(&self, _entry: OplogEntry) -> crate::services::oplog::OplogAddReceipt {
            Box::pin(async { unreachable!("oplog is unused by promise waits") })
        }

        async fn add_pair(
            &self,
            _start: OplogEntry,
            _make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
        ) -> (OplogIndex, OplogIndex) {
            unreachable!("oplog is unused by promise waits")
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            unreachable!("oplog is unused by this test")
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            unreachable!("oplog is unused by this test")
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            unreachable!("oplog is unused by promise waits")
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            unreachable!("oplog is unused by this test")
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            unreachable!("oplog is unused by this test")
        }

        async fn read(&self, _oplog_index: OplogIndex) -> OplogEntry {
            unreachable!("oplog is unused by this test")
        }

        async fn read_exact(
            &self,
            _oplog_index: OplogIndex,
            _n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            unreachable!("oplog is unused by this test")
        }

        async fn length(&self) -> u64 {
            unreachable!("oplog is unused by this test")
        }

        async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
            unreachable!("oplog is unused by this test")
        }

        async fn download_raw_payload(
            &self,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unreachable!("oplog is unused by this test")
        }

        async fn add_start_with_reserved_raw_payload(
            &self,
            _serialized_request: Vec<u8>,
            _build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
        ) -> Result<OrderedOplogStart, String> {
            unreachable!("oplog is unused by this test")
        }

        async fn add_start_with_indexed_reserved_raw_payload(
            &self,
            _build_request: crate::services::oplog::IndexedReservedStartBuilder,
        ) -> Result<OrderedOplogStart, String> {
            unreachable!("oplog is unused by this test")
        }
    }

    fn unused_wakeup_scheduler() -> WakeupScheduler {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "unused".to_string(),
        };
        WakeupScheduler {
            promise_service: Arc::new(UnusedPromiseService),
            scheduler_service: Arc::new(UnusedSchedulerService),
            oplog: Arc::new(UnusedOplog),
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent_id),
            created_by: AccountId::new(),
        }
    }

    struct StubPromiseService;

    #[async_trait]
    impl PromiseService for StubPromiseService {
        async fn create(
            &self,
            agent_id: &AgentId,
            oplog_idx: OplogIndex,
        ) -> Result<PromiseId, WorkerExecutorError> {
            Ok(PromiseId {
                agent_id: agent_id.clone(),
                oplog_idx,
            })
        }

        async fn poll(&self, _promise_id: PromiseId) -> Result<PromiseHandle, WorkerExecutorError> {
            unreachable!("promise polling is unused by this test")
        }

        async fn complete(
            &self,
            _promise_id: PromiseId,
            _data: Vec<u8>,
        ) -> Result<bool, WorkerExecutorError> {
            unreachable!("promise completion is unused by this test")
        }

        async fn cleanup(&self) {}
    }

    /// A scheduler whose `schedule` simulates a new live (not suspendable-parked) host call
    /// appearing while the wakeup is being scheduled, by flipping the shared safety flag.
    struct FlippingSchedulerService {
        safe: Arc<AtomicBool>,
    }

    #[async_trait]
    impl SchedulerService for FlippingSchedulerService {
        async fn schedule(&self, _time: DateTime<Utc>, _action: ScheduledAction) -> ScheduleId {
            self.safe.store(false, Ordering::Release);
            ScheduleId::fresh()
        }

        async fn schedule_with_id(
            &self,
            _schedule_id: ScheduleId,
            _time: DateTime<Utc>,
            _action: ScheduledAction,
        ) -> ScheduleId {
            unreachable!("schedule_with_id is unused by wakeup scheduling")
        }

        async fn cancel(&self, _id: ScheduleId) {
            unreachable!("cancel is unused by this test")
        }
    }

    struct ResumingSchedulerService {
        resumed_at: Arc<Mutex<Option<Timestamp>>>,
    }

    #[async_trait]
    impl SchedulerService for ResumingSchedulerService {
        async fn schedule(&self, _time: DateTime<Utc>, _action: ScheduledAction) -> ScheduleId {
            tokio::time::sleep(Duration::from_millis(1)).await;
            *self.resumed_at.lock().unwrap() = Some(Timestamp::now_utc());
            ScheduleId::fresh()
        }

        async fn schedule_with_id(
            &self,
            _schedule_id: ScheduleId,
            _time: DateTime<Utc>,
            _action: ScheduledAction,
        ) -> ScheduleId {
            unreachable!("schedule_with_id is unused by wakeup scheduling")
        }

        async fn cancel(&self, _id: ScheduleId) {
            unreachable!("cancel is unused by this test")
        }
    }

    struct RegisteringRpcSchedulerService {
        waits: SuspendableWaitRegistry,
        scheduled: Arc<Mutex<Vec<DateTime<Utc>>>>,
    }

    #[async_trait]
    impl SchedulerService for RegisteringRpcSchedulerService {
        async fn schedule(&self, time: DateTime<Utc>, _action: ScheduledAction) -> ScheduleId {
            let first = {
                let mut scheduled = self.scheduled.lock().unwrap();
                scheduled.push(time);
                scheduled.len() == 1
            };
            if first {
                self.waits
                    .lock()
                    .unwrap()
                    .insert(2, SuspendableWait::Rpc(Duration::from_secs(5)));
            }
            ScheduleId::fresh()
        }

        async fn schedule_with_id(
            &self,
            _schedule_id: ScheduleId,
            _time: DateTime<Utc>,
            _action: ScheduledAction,
        ) -> ScheduleId {
            unreachable!("schedule_with_id is unused by wakeup scheduling")
        }

        async fn cancel(&self, _id: ScheduleId) {
            unreachable!("cancel is unused by this test")
        }
    }

    struct StubOplog;

    impl Debug for StubOplog {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("StubOplog").finish()
        }
    }

    #[async_trait]
    impl Oplog for StubOplog {
        async fn add(&self, _entry: OplogEntry) -> OplogIndex {
            unreachable!("oplog writes are unused by wakeup scheduling")
        }

        fn enqueue_add(&self, _entry: OplogEntry) -> crate::services::oplog::OplogAddReceipt {
            Box::pin(async { unreachable!("oplog writes are unused by wakeup scheduling") })
        }

        async fn add_pair(
            &self,
            _start: OplogEntry,
            _make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
        ) -> (OplogIndex, OplogIndex) {
            unreachable!("oplog writes are unused by wakeup scheduling")
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            unreachable!("oplog is unused by this test")
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            unreachable!("oplog is unused by this test")
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            OplogIndex::NONE
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            unreachable!("oplog is unused by this test")
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            unreachable!("oplog is unused by this test")
        }

        async fn read(&self, _oplog_index: OplogIndex) -> OplogEntry {
            unreachable!("oplog is unused by this test")
        }

        async fn read_exact(
            &self,
            _oplog_index: OplogIndex,
            _n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            unreachable!("oplog is unused by this test")
        }

        async fn length(&self) -> u64 {
            unreachable!("oplog is unused by this test")
        }

        async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
            unreachable!("oplog is unused by this test")
        }

        async fn download_raw_payload(
            &self,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unreachable!("oplog is unused by this test")
        }

        async fn add_start_with_reserved_raw_payload(
            &self,
            _serialized_request: Vec<u8>,
            _build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
        ) -> Result<OrderedOplogStart, String> {
            unreachable!("oplog is unused by this test")
        }

        async fn add_start_with_indexed_reserved_raw_payload(
            &self,
            _build_request: crate::services::oplog::IndexedReservedStartBuilder,
        ) -> Result<OrderedOplogStart, String> {
            unreachable!("oplog is unused by this test")
        }
    }

    fn flipping_wakeup_scheduler(safe: Arc<AtomicBool>) -> WakeupScheduler {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "flipping".to_string(),
        };
        WakeupScheduler {
            promise_service: Arc::new(StubPromiseService),
            scheduler_service: Arc::new(FlippingSchedulerService { safe }),
            oplog: Arc::new(StubOplog),
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent_id),
            created_by: AccountId::new(),
        }
    }

    fn resuming_wakeup_scheduler(resumed_at: Arc<Mutex<Option<Timestamp>>>) -> WakeupScheduler {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "resuming".to_string(),
        };
        WakeupScheduler {
            promise_service: Arc::new(StubPromiseService),
            scheduler_service: Arc::new(ResumingSchedulerService { resumed_at }),
            oplog: Arc::new(StubOplog),
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent_id),
            created_by: AccountId::new(),
        }
    }

    fn registering_rpc_wakeup_scheduler(
        waits: SuspendableWaitRegistry,
        scheduled: Arc<Mutex<Vec<DateTime<Utc>>>>,
    ) -> WakeupScheduler {
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "registering-rpc".to_string(),
        };
        WakeupScheduler {
            promise_service: Arc::new(StubPromiseService),
            scheduler_service: Arc::new(RegisteringRpcSchedulerService { waits, scheduled }),
            oplog: Arc::new(StubOplog),
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent_id),
            created_by: AccountId::new(),
        }
    }

    #[test]
    async fn durable_promise_wait_ready_race_does_not_suspend() {
        let ready = Arc::new(AtomicBool::new(false));
        let context = SuspendableWaitContext {
            wait_id: 1,
            agent_mode: AgentMode::Durable,
            suspend: SuspendConfig {
                suspend_after: Duration::from_secs(10),
                ephemeral_max_sleep: Duration::from_secs(60),
                wait_suspend_grace: Duration::ZERO,
                wait_suspend_check_interval: Duration::from_secs(10),
                rpc_suspend_after: Duration::from_secs(30),
                rpc_resume_after: Duration::from_secs(5),
            },
            wait_deadline: None,
            suspendable_waits: Arc::new(Mutex::new(BTreeMap::new())),
            wakeup_scheduler: unused_wakeup_scheduler(),
        };

        let outcome = park_suspendable_wait(
            context,
            Box::pin(pending::<InterruptKind>()),
            || {
                let ready = ready.clone();
                async move {
                    if !ready.load(Ordering::Acquire) {
                        pending::<()>().await;
                    }
                }
            },
            || ready.load(Ordering::Acquire),
            || {
                ready.store(true, Ordering::Release);
                true
            },
            || None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, ParkOutcome::Ready);
    }

    #[test]
    async fn durable_promise_wait_ready_after_final_pending_poll_does_not_suspend() {
        let ready = Arc::new(AtomicBool::new(false));
        let polls = Arc::new(AtomicUsize::new(0));
        let context = SuspendableWaitContext {
            wait_id: 1,
            agent_mode: AgentMode::Durable,
            suspend: SuspendConfig {
                suspend_after: Duration::from_secs(10),
                ephemeral_max_sleep: Duration::from_secs(60),
                wait_suspend_grace: Duration::ZERO,
                wait_suspend_check_interval: Duration::from_secs(10),
                rpc_suspend_after: Duration::from_secs(30),
                rpc_resume_after: Duration::from_secs(5),
            },
            wait_deadline: None,
            suspendable_waits: Arc::new(Mutex::new(BTreeMap::new())),
            wakeup_scheduler: unused_wakeup_scheduler(),
        };

        let outcome = park_suspendable_wait(
            context,
            Box::pin(pending::<InterruptKind>()),
            || {
                let ready = ready.clone();
                let polls = polls.clone();
                async move {
                    if ready.load(Ordering::Acquire) {
                        return;
                    }

                    let poll = polls.fetch_add(1, Ordering::AcqRel);
                    if poll >= 1 {
                        ready.store(true, Ordering::Release);
                    }
                    pending::<()>().await;
                }
            },
            || ready.load(Ordering::Acquire),
            || true,
            || None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, ParkOutcome::Ready);
    }

    #[test]
    async fn new_live_host_call_during_wakeup_scheduling_prevents_suspend() {
        let safe = Arc::new(AtomicBool::new(true));
        let context = SuspendableWaitContext {
            wait_id: 1,
            agent_mode: AgentMode::Durable,
            suspend: SuspendConfig {
                suspend_after: Duration::from_secs(10),
                ephemeral_max_sleep: Duration::from_secs(60),
                wait_suspend_grace: Duration::ZERO,
                wait_suspend_check_interval: Duration::from_millis(10),
                rpc_suspend_after: Duration::from_secs(30),
                rpc_resume_after: Duration::from_secs(5),
            },
            wait_deadline: None,
            suspendable_waits: Arc::new(Mutex::new(BTreeMap::new())),
            wakeup_scheduler: flipping_wakeup_scheduler(safe.clone()),
        };

        let outcome = park_suspendable_wait(
            context,
            Box::pin(pending::<InterruptKind>()),
            || {
                let safe = safe.clone();
                async move {
                    // Becomes ready only once the simulated new host call has appeared
                    if safe.load(Ordering::Acquire) {
                        pending::<()>().await;
                    }
                }
            },
            || false,
            || safe.load(Ordering::Acquire),
            || None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, ParkOutcome::Ready);
    }

    #[test]
    async fn safety_revoked_during_pre_suspend_yield_prevents_suspend() {
        let safe_checks = Arc::new(AtomicUsize::new(0));
        let context = SuspendableWaitContext {
            wait_id: 1,
            agent_mode: AgentMode::Durable,
            suspend: SuspendConfig {
                suspend_after: Duration::from_secs(10),
                ephemeral_max_sleep: Duration::from_secs(60),
                wait_suspend_grace: Duration::ZERO,
                wait_suspend_check_interval: Duration::from_millis(10),
                rpc_suspend_after: Duration::from_secs(30),
                rpc_resume_after: Duration::from_secs(5),
            },
            wait_deadline: None,
            suspendable_waits: Arc::new(Mutex::new(BTreeMap::new())),
            wakeup_scheduler: unused_wakeup_scheduler(),
        };

        let outcome = park_suspendable_wait(
            context,
            Box::pin(pending::<InterruptKind>()),
            || {
                let safe_checks = safe_checks.clone();
                async move {
                    // Becomes ready only after the post-yield safety re-check has run
                    if safe_checks.load(Ordering::Acquire) < 2 {
                        pending::<()>().await;
                    }
                }
            },
            || false,
            // Safe on the first check, then a new live host call appears (simulating one started
            // by another guest task during the pre-suspend yield)
            || safe_checks.fetch_add(1, Ordering::AcqRel) == 0,
            || None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, ParkOutcome::Ready);
    }

    #[test]
    fn suspendable_wait_registration_removal_does_not_remove_replaced_wait() {
        let waits = Arc::new(Mutex::new(BTreeMap::new()));

        let registration = SuspendableWaitRegistration::new(1, None, waits.clone());
        waits.lock().unwrap().insert(
            1,
            SuspendableWait::Deadline(Some(Utc::now() + chrono::Duration::seconds(30))),
        );
        drop(registration);

        assert_eq!(
            waits
                .lock()
                .unwrap()
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([1])
        );
    }

    #[test]
    fn rpc_caps_long_sleep_wakeup_and_shorter_sleep_wins() {
        let now = Utc::now();
        let mut waits = BTreeMap::from([
            (
                1,
                SuspendableWait::Deadline(Some(now + chrono::Duration::hours(1))),
            ),
            (2, SuspendableWait::Rpc(Duration::from_secs(5))),
        ]);
        assert_eq!(
            next_wakeup(now, &waits, Duration::from_secs(10)),
            now + chrono::Duration::seconds(5)
        );

        waits.insert(
            1,
            SuspendableWait::Deadline(Some(now + chrono::Duration::seconds(2))),
        );
        assert_eq!(
            next_wakeup(now, &waits, Duration::from_secs(10)),
            now + chrono::Duration::seconds(2)
        );
    }

    #[test]
    fn removing_rpc_restores_sleep_deadline() {
        let now = Utc::now();
        let waits = Arc::new(Mutex::new(BTreeMap::from([(
            1,
            SuspendableWait::Deadline(Some(now + chrono::Duration::hours(1))),
        )])));
        let rpc = SuspendableWaitRegistration::rpc(2, Duration::from_secs(5), waits.clone());
        assert_eq!(
            next_wakeup(now, &waits.lock().unwrap(), Duration::from_secs(10)),
            now + chrono::Duration::seconds(5)
        );
        drop(rpc);
        assert_eq!(
            next_wakeup(now, &waits.lock().unwrap(), Duration::from_secs(10)),
            now + chrono::Duration::hours(1)
        );
    }

    #[test]
    async fn eligibility_is_rechecked_until_suspension_becomes_safe() {
        let checks = Arc::new(AtomicUsize::new(0));
        let scheduler_safe = Arc::new(AtomicBool::new(true));
        let context = SuspendableWaitContext {
            wait_id: 1,
            agent_mode: AgentMode::Durable,
            suspend: SuspendConfig {
                suspend_after: Duration::ZERO,
                ephemeral_max_sleep: Duration::from_secs(60),
                wait_suspend_grace: Duration::ZERO,
                wait_suspend_check_interval: Duration::from_millis(1),
                rpc_suspend_after: Duration::from_secs(30),
                rpc_resume_after: Duration::from_secs(5),
            },
            wait_deadline: None,
            suspendable_waits: Arc::new(Mutex::new(BTreeMap::new())),
            wakeup_scheduler: flipping_wakeup_scheduler(scheduler_safe),
        };
        let outcome = park_suspendable_wait(
            context,
            Box::pin(pending::<InterruptKind>()),
            || pending::<()>(),
            || false,
            || checks.fetch_add(1, Ordering::AcqRel) > 0,
            || None,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, ParkOutcome::SuspendWorker(_)));
        assert!(checks.load(Ordering::Acquire) >= 3);
    }

    #[test]
    async fn suspension_timestamp_precedes_resume_during_wakeup_scheduling() {
        let resumed_at = Arc::new(Mutex::new(None));
        let context = SuspendableWaitContext {
            wait_id: 1,
            agent_mode: AgentMode::Durable,
            suspend: SuspendConfig {
                suspend_after: Duration::ZERO,
                ephemeral_max_sleep: Duration::from_secs(60),
                wait_suspend_grace: Duration::ZERO,
                wait_suspend_check_interval: Duration::from_secs(10),
                rpc_suspend_after: Duration::from_secs(30),
                rpc_resume_after: Duration::from_secs(5),
            },
            wait_deadline: None,
            suspendable_waits: Arc::new(Mutex::new(BTreeMap::new())),
            wakeup_scheduler: resuming_wakeup_scheduler(resumed_at.clone()),
        };

        let outcome = park_suspendable_wait(
            context,
            Box::pin(pending::<InterruptKind>()),
            || pending::<()>(),
            || false,
            || true,
            || None,
        )
        .await
        .unwrap();
        let ParkOutcome::SuspendWorker(suspend_at) = outcome else {
            panic!("expected suspension, got {outcome:?}");
        };
        let resumed_at = resumed_at.lock().unwrap().expect("resume was simulated");
        assert!(suspend_at < resumed_at);
    }

    #[test]
    async fn rpc_registered_during_wakeup_scheduling_gets_an_earlier_wakeup() {
        let waits = Arc::new(Mutex::new(BTreeMap::new()));
        let scheduled = Arc::new(Mutex::new(Vec::new()));
        let context = SuspendableWaitContext {
            wait_id: 1,
            agent_mode: AgentMode::Durable,
            suspend: SuspendConfig {
                suspend_after: Duration::ZERO,
                ephemeral_max_sleep: Duration::from_secs(60),
                wait_suspend_grace: Duration::ZERO,
                wait_suspend_check_interval: Duration::from_secs(10),
                rpc_suspend_after: Duration::from_secs(30),
                rpc_resume_after: Duration::from_secs(5),
            },
            wait_deadline: Some(Utc::now() + chrono::Duration::hours(1)),
            suspendable_waits: waits.clone(),
            wakeup_scheduler: registering_rpc_wakeup_scheduler(waits, scheduled.clone()),
        };

        let outcome = park_suspendable_wait(
            context,
            Box::pin(pending::<InterruptKind>()),
            || pending::<()>(),
            || false,
            || true,
            || None,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, ParkOutcome::SuspendWorker(_)));
        let scheduled = scheduled.lock().unwrap();
        assert_eq!(scheduled.len(), 2);
        assert!(scheduled[1] < scheduled[0]);
    }
}
