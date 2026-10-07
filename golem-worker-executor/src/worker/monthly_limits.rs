use super::{PendingWorkerInterrupt, UnloadReason, UnloadRequest, Worker, WorkerInstance};
#[cfg(feature = "test-utils")]
use crate::services::HasActiveAgents;
use crate::services::HasConfig;
use crate::services::resource_limits::MonthlyCapacity;
use crate::services::resource_usage_metering::{
    self, ResourceUsageMeteringWindow, ResourceUsageSettlement,
};
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use golem_common::model::{AgentFingerprint, Timestamp};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[cfg(feature = "test-utils")]
mod test_clock;
#[cfg(feature = "test-utils")]
pub use test_clock::{MonthlyClockForTest, MonthlyTimerPollForTest};
#[cfg(test)]
mod tests;

pub(super) struct MonthlyWindowTarget {
    fingerprint: AgentFingerprint,
    start_attempt: Uuid,
    resident_generation: u64,
    active: Mutex<bool>,
    #[cfg(feature = "test-utils")]
    panic: CancellationToken,
    #[cfg(feature = "test-utils")]
    exited: CancellationToken,
}

#[cfg(feature = "test-utils")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonthlyProposalForTest {
    pub fingerprint: AgentFingerprint,
    pub start_attempt: Uuid,
    pub resident_generation: u64,
    pub window_identity: usize,
    pub policy_revision: u64,
    pub period: golem_common::model::account_usage::AccountUsagePeriod,
    pub fuel_generation: Option<u64>,
    pub exhaustion: Option<&'static str>,
}

#[cfg(feature = "test-utils")]
impl MonthlyProposalForTest {
    fn new(target: &Arc<MonthlyWindowTarget>, capacity: MonthlyCapacity) -> Self {
        Self {
            fingerprint: target.fingerprint,
            start_attempt: target.start_attempt,
            resident_generation: target.resident_generation,
            window_identity: Arc::as_ptr(target) as usize,
            policy_revision: capacity.policy_revision,
            period: capacity.period,
            fuel_generation: capacity.fuel_generation,
            exhaustion: capacity.exhaustion.map(|value| value.reason()),
        }
    }
}

#[cfg(feature = "test-utils")]
pub struct MonthlyAcceptanceForTest {
    pub proposal: MonthlyProposalForTest,
    pub completed: tokio::sync::oneshot::Receiver<bool>,
}

pub(super) struct ExecutionWindow {
    window: Option<ResourceUsageMeteringWindow>,
    monitor: Option<MonthlyMonitor>,
    stop_progress: super::StopProgress,
}

struct MonthlyMonitor {
    target: Arc<MonthlyWindowTarget>,
    cancelled: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), WorkerExecutorError>>,
}

impl ExecutionWindow {
    #[cfg(test)]
    pub(super) fn unmonitored(window: ResourceUsageMeteringWindow) -> Self {
        Self {
            window: Some(window),
            monitor: None,
            stop_progress: Arc::default(),
        }
    }

    pub(super) async fn new<Ctx: WorkerCtx>(
        worker: &Arc<Worker<Ctx>>,
        window: ResourceUsageMeteringWindow,
        start_attempt: uuid::Uuid,
    ) -> Self {
        if !worker.resource_entry.monthly_metering_enabled() {
            return Self {
                window: Some(window),
                monitor: None,
                stop_progress: worker.stop_progress.clone(),
            };
        }
        let monitor = worker.resource_entry.monthly_metering_enabled().then(|| {
            if let Some(flusher) = window.usage_flusher() {
                worker
                    .owner_runtime_resources
                    .register_resource_usage_flusher(flusher);
            }
            let target = Arc::new(MonthlyWindowTarget {
                fingerprint: worker.initial_worker_metadata.fingerprint,
                start_attempt,
                resident_generation: worker
                    .resident_generation
                    .load(std::sync::atomic::Ordering::Acquire),
                active: Mutex::new(true),
                #[cfg(feature = "test-utils")]
                panic: CancellationToken::new(),
                #[cfg(feature = "test-utils")]
                exited: CancellationToken::new(),
            });
            *worker.monthly_window.lock().unwrap() = Some(target.clone());
            let mut updates = worker.resource_entry.subscribe_capacity_updates();
            let cancelled = CancellationToken::new();
            let stop = cancelled.clone();
            let target_task = target.clone();
            let weak_worker = Arc::downgrade(worker);
            let actor = worker.state_actor.clone();
            let memory = window.monthly_memory_tracker();
            #[cfg(feature = "test-utils")]
            let clock = worker.monthly_clock.lock().unwrap().clone();
            let task = tokio::spawn(async move {
                #[cfg(feature = "test-utils")]
                let _exited = target_task.exited.clone().drop_guard();
                let result = std::panic::AssertUnwindSafe(async {
                    loop {
                        let Some(worker) = weak_worker.upgrade() else {
                            break;
                        };
                        worker.owner_runtime_resources.settle_resource_usage();
                        let duration = memory
                            .as_ref()
                            .and_then(|memory| {
                                worker.resource_entry.monthly_memory_check_after(memory.current_bytes())
                            })
                            .map_or(Duration::from_secs(30), |horizon| {
                                horizon.min(Duration::from_secs(30))
                            });
                        drop(worker);
                        #[cfg(feature = "test-utils")]
                        let tick = match &clock {
                            Some(clock) => futures::future::Either::Left(clock.sleep(duration)),
                            None => futures::future::Either::Right(tokio::time::sleep(duration)),
                        };
                        #[cfg(not(feature = "test-utils"))]
                        let tick = tokio::time::sleep(duration);
                        tokio::select! {
                            biased;
                            _ = stop.cancelled() => break,
                            _ = actor.monthly_delivery_closed() => {
                                return Err(WorkerExecutorError::runtime("Monthly monitor lost the lifecycle actor"));
                            },
                            _ = async {
                                #[cfg(feature = "test-utils")]
                                target_task.panic.cancelled().await;
                                #[cfg(not(feature = "test-utils"))]
                                std::future::pending::<()>().await;
                            } => panic!("injected monthly monitor panic"),
                            _ = tick => {},
                            update = updates.changed() => {
                                if update.is_err() {
                                    return Err(WorkerExecutorError::runtime("Monthly monitor lost capacity updates"));
                                }
                            },
                        }
                        let Some(worker) = weak_worker.upgrade() else {
                            break;
                        };
                        worker.owner_runtime_resources.settle_resource_usage();
                        let capacity = worker
                            .owner_runtime_resources
                            .with_monthly_capacity(worker.agent_mode(), |capacity| capacity);
                        if capacity.exhaustion.is_some() {
                            worker.state_actor.monthly_capacity_exhausted(
                                worker.clone(),
                                target_task.clone(),
                                capacity,
                            )?;
                        }
                    }
                    if let Some(worker) = weak_worker.upgrade() {
                        worker.owner_runtime_resources.settle_resource_usage();
                    }
                    Ok(())
                })
                .catch_unwind()
                .await
                .unwrap_or_else(|panic| {
                    let message = panic.downcast_ref::<&str>().copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    Err(WorkerExecutorError::runtime(format!("Monthly monitor panicked: {message}")))
                });
                if let Err(error) = &result
                    && let Some(worker) = weak_worker.upgrade()
                {
                    worker.fail_monthly_monitor(&target_task, error.clone()).await;
                }
                result
            });
            MonthlyMonitor {
                target,
                cancelled,
                task,
            }
        });
        Self {
            window: Some(window),
            monitor,
            stop_progress: worker.stop_progress.clone(),
        }
    }

    pub(super) async fn prepare_disposal(
        mut self,
    ) -> (
        ResourceUsageMeteringWindow,
        Option<WorkerExecutorError>,
        super::StopAdmissionSeal,
    ) {
        let mut seal = super::StopAdmissionSeal::new(&self.stop_progress);
        let monitor = self.monitor.take();
        stop_monitor(&monitor);
        let result = join_monitor(monitor).await;
        let stopped = seal.join().await;
        let mut window = self.window.take().unwrap();
        let frozen = window
            .freeze_allocation()
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()));
        let error = result.and(stopped).and(frozen).err();
        seal.health = error.clone().map_or(Ok(()), Err);
        (window, error, seal)
    }

    pub(super) fn close(
        mut self,
        deadline: Instant,
    ) -> impl Future<Output = Result<ResourceUsageSettlement, WorkerExecutorError>> + Send + 'static
    {
        #[cfg(feature = "test-utils")]
        if std::mem::take(&mut self.stop_progress.lock().unwrap().lose_settlement_observer) {
            self.window
                .as_mut()
                .unwrap()
                .lose_settlement_observer_for_test();
        }
        close_owned_window(
            self.window.take().unwrap(),
            self.monitor.take(),
            self.stop_progress.clone(),
            deadline,
        )
    }
}

impl Drop for ExecutionWindow {
    fn drop(&mut self) {
        if let Some(window) = self.window.take() {
            drop(close_owned_window(
                window,
                self.monitor.take(),
                self.stop_progress.clone(),
                Instant::now() + Duration::from_secs(30),
            ));
        }
    }
}

fn close_owned_window(
    window: ResourceUsageMeteringWindow,
    monitor: Option<MonthlyMonitor>,
    progress: super::StopProgress,
    deadline: Instant,
) -> impl Future<Output = Result<ResourceUsageSettlement, WorkerExecutorError>> + Send + 'static {
    let mut seal = super::StopAdmissionSeal::new(&progress);
    stop_monitor(&monitor);
    let unmonitored = monitor.is_none();
    let mut closing = Box::pin(async move {
        let monitor_result = join_monitor(monitor).await;
        let stopped = seal.join().await;
        let (settlement, permit) =
            resource_usage_metering::close_window_retaining_permit(window, deadline).await;
        let result = monitor_result
            .and(stopped)
            .and(settlement.map_err(|error| WorkerExecutorError::runtime(error.to_string())));
        drop(permit);
        seal.complete(result.as_ref().map(|_| ()).map_err(Clone::clone));
        result
    });
    // Poll by mutable pin so pending cleanup keeps its permit and seal when moved to the task.
    if unmonitored {
        let ready = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            closing.as_mut().now_or_never()
        }));
        match ready {
            Ok(Some(result)) => return futures::future::Either::Left(std::future::ready(result)),
            Ok(None) => {}
            Err(panic) => {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown panic");
                return futures::future::Either::Left(std::future::ready(Err(
                    WorkerExecutorError::runtime(format!(
                        "Execution window cleanup panicked: {message}"
                    )),
                )));
            }
        }
    }
    #[cfg(test)]
    {
        progress.lock().unwrap().window_close_tasks += 1;
    }
    let task = tokio::spawn(closing);
    futures::future::Either::Right(async move {
        task.await.unwrap_or_else(|error| {
            Err(WorkerExecutorError::runtime(format!(
                "Execution window cleanup failed: {error}"
            )))
        })
    })
}

fn stop_monitor(monitor: &Option<MonthlyMonitor>) {
    if let Some(monitor) = monitor {
        *monitor.target.active.lock().unwrap() = false;
        monitor.cancelled.cancel();
    }
}

async fn join_monitor(monitor: Option<MonthlyMonitor>) -> Result<(), WorkerExecutorError> {
    match monitor {
        Some(monitor) => monitor
            .task
            .await
            .map_err(|error| {
                WorkerExecutorError::runtime(format!("Monthly monitor failed: {error}"))
            })
            .and_then(|result| result),
        None => Ok(()),
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    async fn fail_monthly_monitor(
        &self,
        target: &Arc<MonthlyWindowTarget>,
        error: WorkerExecutorError,
    ) {
        loop {
            let lifecycle = self.instance.lock().await;
            let Some(mut interrupts) = self.interrupt_signal.try_lock() else {
                drop(lifecycle);
                tokio::task::yield_now().await;
                continue;
            };
            let active = target.active.lock().unwrap();
            if !*active
                || !self
                    .monthly_window
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, target))
            {
                return;
            }
            let mut admission = self.stop_progress.lock().unwrap();
            admission
                .tasks
                .push(std::future::ready(Err(error)).boxed().shared());
            let kind = InterruptKind::Suspend(Timestamp::now_utc());
            if !admission.retired
                && interrupts.queue(PendingWorkerInterrupt {
                    kind,
                    reacquire_permits: false,
                    unload_request: UnloadRequest::ordinary(UnloadReason::Failure),
                })
            {
                self.accept_interrupt(&lifecycle, kind, admission);
            }
            return;
        }
    }

    #[cfg(feature = "test-utils")]
    pub(crate) fn invocation_deadline_for_test(
        &self,
        duration: Duration,
    ) -> impl Future<Output = ()> + Send + 'static {
        let (advance, mut deadline) =
            tokio::sync::watch::channel(tokio::time::Instant::now() + duration);
        self.stop_progress.lock().unwrap().invocation_deadline = Some(advance);
        async move {
            loop {
                let expires = *deadline.borrow_and_update();
                tokio::select! {
                    _ = tokio::time::sleep_until(expires) => return,
                    changed = deadline.changed() => {
                        if changed.is_err() {
                            tokio::time::sleep_until(expires).await;
                            return;
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn expire_invocation_deadline_for_test(&self) {
        self.stop_progress
            .lock()
            .unwrap()
            .invocation_deadline
            .as_ref()
            .expect("invocation deadline armed")
            .send(tokio::time::Instant::now())
            .expect("invocation deadline still pending");
    }

    #[cfg(feature = "test-utils")]
    pub fn accepted_stop_receipt_for_test(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.stop_progress
            .lock()
            .unwrap()
            .publication
            .as_ref()
            .expect("accepted stop")
            .0
            .receipt
            .subscribe()
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_idle_close_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .idle_close_gate
                .replace((entered, wait))
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub(super) async fn wait_idle_close_for_test(&self) {
        let gate = self.stop_progress.lock().unwrap().idle_close_gate.take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            assert!(!release.await.unwrap_or(false), "injected idle panic");
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn unload_succeeded_for_test(&self) -> bool {
        self.stop_progress.lock().unwrap().unload_succeeded
    }

    #[cfg(feature = "test-utils")]
    pub async fn panic_monthly_monitor_for_test(&self) {
        let target = self
            .monthly_window
            .lock()
            .unwrap()
            .clone()
            .expect("active monthly window");
        target.panic.cancel();
        target.exited.cancelled().await;
    }

    #[cfg(feature = "test-utils")]
    pub fn lose_window_settlement_for_test(&self) {
        self.stop_progress.lock().unwrap().lose_settlement_observer = true;
    }

    #[cfg(feature = "test-utils")]
    pub async fn stop_lifecycle_actor_for_test(&self) {
        let target = self
            .monthly_window
            .lock()
            .unwrap()
            .clone()
            .expect("active monthly window");
        self.state_actor.stop_lifecycle_for_test().await;
        target.exited.cancelled().await;
    }

    #[cfg(feature = "test-utils")]
    pub fn permit_acquisitions_for_test(&self) -> usize {
        self.stop_progress.lock().unwrap().permit_acquisitions
    }

    #[cfg(feature = "test-utils")]
    pub fn observe_monthly_acceptance_for_test(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<MonthlyAcceptanceForTest> {
        let (notify, observe) = tokio::sync::mpsc::unbounded_channel();
        assert!(
            self.monthly_acceptance_observer
                .lock()
                .unwrap()
                .replace(notify)
                .is_none()
        );
        observe
    }

    #[cfg(feature = "test-utils")]
    pub fn current_monthly_proposal_for_test(&self) -> MonthlyProposalForTest {
        let target = self
            .monthly_window
            .lock()
            .unwrap()
            .clone()
            .expect("monthly window");
        self.owner_runtime_resources
            .with_monthly_capacity(self.agent_mode(), |capacity| {
                MonthlyProposalForTest::new(&target, capacity)
            })
    }

    #[cfg(feature = "test-utils")]
    pub fn submit_monthly_capacity_for_test(&self) -> Result<(), WorkerExecutorError> {
        let target = self
            .monthly_window
            .lock()
            .unwrap()
            .clone()
            .expect("monthly window");
        let capacity = self
            .owner_runtime_resources
            .with_monthly_capacity(self.agent_mode(), |capacity| capacity);
        self.state_actor.monthly_capacity_exhausted(
            self.self_ref.upgrade().unwrap(),
            target,
            capacity,
        )
    }

    #[cfg(feature = "test-utils")]
    pub async fn drain_lifecycle_for_test(&self) -> Result<(), WorkerExecutorError> {
        self.state_actor.drain_lifecycle().await
    }

    #[cfg(feature = "test-utils")]
    pub fn has_unload_cleanup_for_test(&self) -> bool {
        self.unload_cleanup.lock().unwrap().is_some()
    }

    #[cfg(feature = "test-utils")]
    pub async fn retained_cleanup_for_test(&self) -> Result<(), WorkerExecutorError> {
        self.wait_for_unload_cleanup().await
    }

    #[cfg(feature = "test-utils")]
    pub async fn cleanup_failure_for_test(&self) -> Option<WorkerExecutorError> {
        match self.instance.lock().await.deletion_runtime() {
            WorkerInstance::CleanupFailed(error) => Some(error.clone()),
            _ => None,
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_next_stop_close_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .close_gate
                .replace((entered, wait))
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_next_stop_driver_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
    ) {
        let (polled, observe_poll) = tokio::sync::oneshot::channel();
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        self.stop_progress
            .lock()
            .unwrap()
            .driver_gates
            .push_back((polled, (entered, wait)));
        (observe_poll, observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_next_stop_publication_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .publication_gate
                .replace((entered, wait))
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_next_stop_freeze_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .freeze_gate
                .replace((entered, wait))
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub(super) async fn wait_stop_freeze_for_test(&self) {
        let gate = self.stop_progress.lock().unwrap().freeze_gate.take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            assert!(
                !release.await.unwrap_or(false),
                "injected frozen stop panic"
            );
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_next_invocation_admission_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .invocation_admission_gate
                .replace((entered, wait))
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub(super) async fn wait_invocation_admission_for_test(&self) {
        let gate = self
            .stop_progress
            .lock()
            .unwrap()
            .invocation_admission_gate
            .take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            assert!(
                !release.await.unwrap_or(false),
                "injected invocation admission panic"
            );
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn observe_next_tail_drain_for_test(
        &self,
    ) -> tokio::sync::oneshot::Receiver<crate::durable_host::tail_work::TailWorkTracker> {
        let (notify, observe) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .tail_drain_observer
                .replace(notify)
                .is_none()
        );
        observe
    }

    #[cfg(feature = "test-utils")]
    pub(crate) fn notify_tail_drain_for_test(
        &self,
        tracker: crate::durable_host::tail_work::TailWorkTracker,
    ) {
        let observer = self
            .stop_progress
            .lock()
            .unwrap()
            .tail_drain_observer
            .take();
        if let Some(observer) = observer {
            let _ = observer.send(tracker);
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_completed_replay_for_test(
        &self,
        key: golem_common::model::IdempotencyKey,
    ) -> (
        tokio::sync::oneshot::Receiver<(
            golem_common::model::oplog::OplogIndex,
            golem_common::model::oplog::OplogIndex,
        )>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .completed_replay_gate
                .replace(super::CompletedReplayTestGate {
                    key,
                    entered,
                    release: wait,
                })
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub(crate) async fn wait_completed_replay_for_test(
        &self,
        key: &golem_common::model::IdempotencyKey,
        start: golem_common::model::oplog::OplogIndex,
        replay: &crate::durable_host::replay_state::ReplayState,
    ) {
        use crate::services::HasOplog;
        use golem_common::model::oplog::OplogEntry;

        let gate = {
            let mut progress = self.stop_progress.lock().unwrap();
            if progress
                .completed_replay_gate
                .as_ref()
                .is_some_and(|gate| &gate.key == key)
            {
                progress.completed_replay_gate.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            assert!(
                !replay.is_live(),
                "completed invocation must still be replaying"
            );
            assert!(!replay.is_in_skipped_region(start).await.unwrap());
            let entries = self
                .oplog()
                .read_exact(
                    start.next(),
                    replay.replay_target().as_u64() - start.as_u64(),
                )
                .await;
            let mut finished = None;
            for (index, entry) in entries {
                if replay.is_in_skipped_region(index).await.unwrap() {
                    continue;
                }
                match entry {
                    OplogEntry::AgentInvocationFinished { .. } => {
                        finished = Some(index);
                        break;
                    }
                    OplogEntry::AgentInvocationStarted { .. } => break,
                    _ => {}
                }
            }
            let finished =
                finished.expect("replay gate requires a visible Finished for this invocation");
            let _ = gate.entered.send((start, finished));
            let _ = gate.release.await;
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn observe_next_publication_wait_for_test(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (entered, observe) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .publication_wait_observer
                .replace(entered)
                .is_none()
        );
        observe
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_next_outcome_for_test(
        &self,
        after_selection: bool,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress.lock().unwrap().outcome_gates[usize::from(after_selection)]
                .replace((entered, wait))
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub(crate) async fn wait_outcome_gate_for_test(&self, after_selection: bool) {
        let gate =
            self.stop_progress.lock().unwrap().outcome_gates[usize::from(after_selection)].take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            assert!(
                !release.await.unwrap_or(false),
                "injected outcome writer panic"
            );
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_next_teardown_fence_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<super::OwnerFenceTestSnapshot>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.stop_progress
                .lock()
                .unwrap()
                .teardown_fence_gate
                .replace(super::OwnerFenceTestGate {
                    generation: self.resident_generation_for_test(),
                    entered,
                    release: wait,
                },)
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub(crate) async fn wait_teardown_fence_for_test(
        &self,
        proposed: Option<&crate::durable_host::tool::operation::OwnerFailureWinner>,
        final_interrupt: Option<InterruptKind>,
    ) {
        let gate = self
            .stop_progress
            .lock()
            .unwrap()
            .teardown_fence_gate
            .take();
        if let Some(gate) = gate {
            let generation = self.resident_generation_for_test();
            assert_eq!(generation, gate.generation);
            let selected = self.selected_owner_failure_for_test().await;
            let snapshot = super::OwnerFenceTestSnapshot {
                generation,
                proposed_owner_failure: format!("{proposed:?}"),
                final_interrupt,
                pending: self.pending_stop_for_test().await,
                frozen: self.frozen_stop_for_test(),
                selected_owner_failure: selected,
            };
            let _ = gate.entered.send(snapshot);
            let _ = gate.release.await;
        }
    }

    #[cfg(feature = "test-utils")]
    pub async fn pending_stop_for_test(&self) -> Option<InterruptKind> {
        match &*self.interrupt_signal.lock().await {
            super::WorkerInterruptState::Unpublished(pending)
            | super::WorkerInterruptState::Freezing(pending)
            | super::WorkerInterruptState::Pending(pending) => Some(pending.kind),
            _ => None,
        }
    }

    #[cfg(feature = "test-utils")]
    pub async fn terminal_stop_claimed_for_test(&self) -> bool {
        matches!(
            *self.interrupt_signal.lock().await,
            super::WorkerInterruptState::TerminalClaimed(_)
        )
    }

    #[cfg(feature = "test-utils")]
    pub fn monthly_window_active_for_test(&self) -> bool {
        let target = self.monthly_window.lock().unwrap().clone();
        target.is_some_and(|target| *target.active.lock().unwrap())
    }

    #[cfg(feature = "test-utils")]
    pub async fn selected_owner_failure_for_test(&self) -> Option<String> {
        let active = self
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await?;
        active
            .execution()
            .tool_operations()
            .selected_owner_failure()
            .map(|winner| format!("{winner:?}"))
    }

    #[cfg(feature = "test-utils")]
    pub fn frozen_stop_for_test(&self) -> Option<InterruptKind> {
        self.stop_progress
            .lock()
            .unwrap()
            .publication
            .as_ref()
            .and_then(|(publication, _)| publication.cause.get().map(|pending| pending.kind))
    }

    #[cfg(feature = "test-utils")]
    pub async fn join_accepted_stops_for_test(&self) -> Result<(), WorkerExecutorError> {
        self.join_stop_progress().await
    }

    #[cfg(feature = "test-utils")]
    pub fn await_interrupt_for_test(
        &self,
    ) -> std::pin::Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.execution_status
            .read()
            .unwrap()
            .create_await_interrupt_signal()
    }

    #[cfg(feature = "test-utils")]
    pub fn raw_interrupt_for_test(&self) -> tokio::sync::broadcast::Receiver<InterruptKind> {
        match &*self.execution_status.read().unwrap() {
            crate::model::ExecutionStatus::Loading {
                interrupt_signal, ..
            }
            | crate::model::ExecutionStatus::Running {
                interrupt_signal, ..
            } => interrupt_signal.subscribe(),
            _ => panic!("expected a Loading or Running interrupt sender"),
        }
    }

    #[cfg(feature = "test-utils")]
    pub async fn owner_stop_for_test(&self) -> Option<InterruptKind> {
        let active = self
            .active_agents()
            .try_get_active_agent(&self.owned_agent_id)
            .await?;
        match active
            .execution()
            .tool_operations()
            .selected_owner_failure()
        {
            Some(crate::durable_host::tool::operation::OwnerFailureWinner::Lifecycle(kind)) => {
                Some(kind)
            }
            _ => None,
        }
    }

    #[cfg(feature = "test-utils")]
    pub fn monthly_stop_for_test(&self) -> Option<InterruptKind> {
        self.monthly_stop
            .lock()
            .unwrap()
            .as_ref()
            .map(|(kind, _)| *kind)
    }

    #[cfg(feature = "test-utils")]
    pub fn monthly_stop_reason_for_test(&self) -> Option<&'static str> {
        self.monthly_stop
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, exhaustion)| exhaustion.reason())
    }

    #[cfg(feature = "test-utils")]
    pub fn startup_attempt_for_test(&self) -> Result<Option<Uuid>, WorkerExecutorError> {
        self.startup_attempt.current()
    }

    #[cfg(feature = "test-utils")]
    pub fn resident_generation_for_test(&self) -> u64 {
        self.resident_generation
            .load(std::sync::atomic::Ordering::Acquire)
    }

    #[cfg(feature = "test-utils")]
    pub fn set_monthly_clock_for_test(&self, clock: Arc<MonthlyClockForTest>) {
        *self.monthly_clock.lock().unwrap() = Some(clock);
    }

    #[cfg(feature = "test-utils")]
    pub async fn hold_interrupt_lock_for_test(&self) -> impl Send + '_ {
        self.interrupt_signal.lock().await
    }

    #[cfg(feature = "test-utils")]
    pub fn pause_next_monthly_acceptance_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        assert!(
            self.monthly_acceptance_gate
                .lock()
                .unwrap()
                .replace((entered, wait))
                .is_none()
        );
        (observe, release)
    }

    #[cfg(feature = "test-utils")]
    pub async fn concurrent_agent_permit_is_held(&self) -> bool {
        self.instance.lock().await.concurrent_agent_permit_is_held()
    }

    async fn lock_for_outcome(
        &self,
    ) -> Result<async_lock::MutexGuard<'_, super::WorkerInterruptState>, WorkerExecutorError> {
        loop {
            let interrupts = self.interrupt_signal.lock().await;
            if !matches!(&*interrupts,
                super::WorkerInterruptState::Unpublished(pending) | super::WorkerInterruptState::Freezing(pending)
                if pending.is_terminal())
            {
                return Ok(interrupts);
            }
            let (publication, completion) = self
                .stop_progress
                .lock()
                .unwrap()
                .publication
                .clone()
                .expect("accepted stop has retained publication");
            let notified = publication.published.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            drop(interrupts);
            #[cfg(feature = "test-utils")]
            if let Some(observer) = self
                .stop_progress
                .lock()
                .unwrap()
                .publication_wait_observer
                .take()
            {
                let _ = observer.send(());
            }
            tokio::select! {
                _ = notified => {}
                result = completion => { result?; }
            }
        }
    }

    pub(crate) async fn claim_invocation_success(
        &self,
        key: Option<golem_common::model::IdempotencyKey>,
    ) -> Result<super::InvocationOutcomeWrite, WorkerExecutorError> {
        let mut interrupts = self.lock_for_outcome().await?;
        let mut selected = self.invocation_outcome.lock().unwrap();
        let generation = self
            .resident_generation
            .load(std::sync::atomic::Ordering::Acquire);
        if selected
            .as_ref()
            .is_some_and(|selected| selected.generation == generation && selected.key == key)
        {
            return Err(WorkerExecutorError::runtime(
                "Invocation outcome was already selected",
            ));
        }
        let pending = interrupts.claim_pending_terminal();
        let error = pending.map(|pending| {
            self.monthly_interrupt_error(pending.kind)
                .unwrap_or(WorkerExecutorError::Interrupted { kind: pending.kind })
        });
        let failure = error.clone().map(|error| match error {
            WorkerExecutorError::Interrupted { kind } => crate::model::TrapType::Interrupt(kind),
            error => crate::model::TrapType::from_worker_executor_error::<Ctx>(
                error,
                golem_common::model::oplog::OplogIndex::INITIAL,
                false,
                false,
                self.agent_mode(),
            ),
        });
        let (writer, completion) = super::InvocationOutcomeWrite::new();
        *selected = Some(super::SelectedInvocationOutcome {
            generation,
            key,
            failure,
            writer_claimed: error.is_none(),
            writer_completion: error.is_none().then_some(completion),
        });
        error.map_or(Ok(writer), Err)
    }

    pub(crate) async fn claim_invocation_failure(
        &self,
        key: Option<golem_common::model::IdempotencyKey>,
        trap: &crate::model::TrapType,
        origin: super::InvocationFailureOrigin,
    ) -> Option<(crate::model::TrapType, super::InvocationOutcomeWrite)> {
        let (mut interrupts, publication_error) = match origin {
            super::InvocationFailureOrigin::Independent => {
                (self.interrupt_signal.lock().await, None)
            }
            super::InvocationFailureOrigin::InvocationDeadline
            | super::InvocationFailureOrigin::Lifecycle => match self.lock_for_outcome().await {
                Ok(interrupts) => (interrupts, None),
                Err(error) => (self.interrupt_signal.lock().await, Some(error)),
            },
        };
        let mut selected = self.invocation_outcome.lock().unwrap();
        let generation = self
            .resident_generation
            .load(std::sync::atomic::Ordering::Acquire);
        if let Some(selected) = selected
            .as_mut()
            .filter(|selected| selected.generation == generation && selected.key == key)
        {
            if selected.writer_claimed {
                return None;
            }
            let failure = selected.failure.clone()?;
            let (writer, completion) = super::InvocationOutcomeWrite::new();
            selected.writer_claimed = true;
            selected.writer_completion = Some(completion);
            return Some((failure, writer));
        }
        let pending = interrupts.claim_pending_terminal();
        let elected_error = publication_error.or_else(|| {
            matches!(
                origin,
                super::InvocationFailureOrigin::InvocationDeadline
                    | super::InvocationFailureOrigin::Lifecycle
            )
            .then_some(pending)
            .flatten()
            .map(|pending| {
                self.monthly_interrupt_error(pending.kind)
                    .unwrap_or(WorkerExecutorError::Interrupted { kind: pending.kind })
            })
        });
        let trap = match (elected_error, trap) {
            (Some(WorkerExecutorError::Interrupted { kind }), _) => {
                crate::model::TrapType::Interrupt(kind)
            }
            (Some(error), _) => crate::model::TrapType::from_worker_executor_error::<Ctx>(
                error,
                golem_common::model::oplog::OplogIndex::INITIAL,
                false,
                false,
                self.agent_mode(),
            ),
            (None, crate::model::TrapType::Interrupt(kind)) => self
                .monthly_interrupt_error(*kind)
                .map(|error| {
                    crate::model::TrapType::from_worker_executor_error::<Ctx>(
                        error,
                        golem_common::model::oplog::OplogIndex::INITIAL,
                        false,
                        false,
                        self.agent_mode(),
                    )
                })
                .unwrap_or_else(|| trap.clone()),
            _ => trap.clone(),
        };
        let (writer, completion) = super::InvocationOutcomeWrite::new();
        *selected = Some(super::SelectedInvocationOutcome {
            generation,
            key,
            failure: Some(trap.clone()),
            writer_claimed: true,
            writer_completion: Some(completion),
        });
        Some((trap, writer))
    }

    pub(super) fn monthly_interrupt_error(
        &self,
        kind: InterruptKind,
    ) -> Option<WorkerExecutorError> {
        if self.agent_mode() != golem_common::model::agent::AgentMode::Ephemeral {
            return None;
        }
        self.monthly_stop
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|(accepted, exhaustion)| {
                (*accepted == kind).then(|| {
                    super::invocation_loop::monthly_resource_exhausted_invocation_error(
                        &self.config(),
                        *exhaustion,
                    )
                })
            })
    }

    pub(super) async fn accept_monthly_capacity(
        &self,
        target: Arc<MonthlyWindowTarget>,
        proposed: MonthlyCapacity,
    ) {
        #[cfg(feature = "test-utils")]
        let completed = {
            let observer = self.monthly_acceptance_observer.lock().unwrap().clone();
            observer.map(|observer| {
                let (completed, receive) = tokio::sync::oneshot::channel();
                let _ = observer.send(MonthlyAcceptanceForTest {
                    proposal: MonthlyProposalForTest::new(&target, proposed),
                    completed: receive,
                });
                completed
            })
        };
        #[cfg(feature = "test-utils")]
        {
            let gate = self.monthly_acceptance_gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                let _ = entered.send(());
                let _ = release.await;
            }
        }
        let _accepted = self.try_accept_monthly_capacity(target, proposed).await;
        #[cfg(feature = "test-utils")]
        if let Some(completed) = completed {
            let _ = completed.send(_accepted);
        }
    }

    async fn try_accept_monthly_capacity(
        &self,
        target: Arc<MonthlyWindowTarget>,
        proposed: MonthlyCapacity,
    ) -> bool {
        if !self.is_current_cached_owner().await {
            return false;
        }
        let lifecycle = self.instance.lock().await;
        let WorkerInstance::Running(running) = &*lifecycle else {
            return false;
        };
        if target.fingerprint != self.initial_worker_metadata.fingerprint
            || target.start_attempt != running.start_attempt
            || target.resident_generation
                != self
                    .resident_generation
                    .load(std::sync::atomic::Ordering::Acquire)
            || !running
                .concurrent_agent_permit_held
                .load(std::sync::atomic::Ordering::Acquire)
            || !self
                .monthly_window
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &target))
        {
            return false;
        }
        let active = target.active.lock().unwrap();
        if !*active {
            return false;
        }
        let Some(mut interrupts) = self.interrupt_signal.try_lock() else {
            return false;
        };
        self.owner_runtime_resources
            .with_monthly_capacity(self.agent_mode(), |current| {
                if current != proposed || current.exhaustion.is_none() {
                    return false;
                }
                let kind = InterruptKind::Suspend(Timestamp::now_utc());
                let admission = self.stop_progress.lock().unwrap();
                // Monthly observations cannot reopen a terminal stop already claimed by this execution.
                if !admission.retired
                    && !matches!(*interrupts, super::WorkerInterruptState::TerminalClaimed(_))
                    && interrupts.queue(PendingWorkerInterrupt {
                        kind,
                        reacquire_permits: false,
                        unload_request: UnloadRequest::ordinary(UnloadReason::Suspend),
                    })
                {
                    *self.monthly_stop.lock().unwrap() = Some((kind, current.exhaustion.unwrap()));
                    self.accept_interrupt(&lifecycle, kind, admission);
                    true
                } else {
                    false
                }
            })
    }
}
