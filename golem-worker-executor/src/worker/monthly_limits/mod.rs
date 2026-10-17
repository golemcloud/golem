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
#[cfg(test)]
mod usage_rate_tests;

/// The primary memory horizon and the timer, policy and usage-rate subscriptions of its monitor.
struct MonthlyMemoryCheck {
    entry: Arc<crate::services::resource_limits::AtomicResourceEntry>,
    memory: Option<crate::services::linear_memory::LinearMemoryTracker>,
    capacity_updates: tokio::sync::watch::Receiver<u64>,
    memory_updates: Option<tokio::sync::watch::Receiver<u64>>,
}

impl MonthlyMemoryCheck {
    fn new(
        entry: Arc<crate::services::resource_limits::AtomicResourceEntry>,
        memory: Option<crate::services::linear_memory::LinearMemoryTracker>,
    ) -> Self {
        let capacity_updates = entry.subscribe_capacity_updates();
        let memory_updates = memory
            .as_ref()
            .and_then(|memory| memory.subscribe_billable_bytes());
        Self {
            entry,
            memory,
            capacity_updates,
            memory_updates,
        }
    }

    fn duration(&mut self) -> Duration {
        // Mark the subscribed version seen before reading the meter. A change during or after
        // this read remains pending and invalidates the newly armed timer. Release the watch
        // borrow before taking the meter state lock used by rate publishers.
        if let Some(updates) = &mut self.memory_updates {
            drop(updates.borrow_and_update());
        }
        self.memory
            .as_ref()
            .and_then(|memory| {
                self.entry
                    .monthly_memory_check_after(memory.billable_bytes())
            })
            .map_or(Duration::from_secs(30), |horizon| {
                horizon.min(Duration::from_secs(30))
            })
    }

    async fn wait(
        &mut self,
        tick: impl Future<Output = ()>,
    ) -> Result<(), tokio::sync::watch::error::RecvError> {
        let memory_change = async {
            match &mut self.memory_updates {
                Some(updates) => updates.changed().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            _ = tick => Ok(()),
            update = self.capacity_updates.changed() => update,
            update = memory_change => update,
        }
    }
}

pub(super) struct MonthlyWindowTarget {
    diagnostic_window_id: u64,
    trace_origin: golem_common::tracing::TraceOrigin,
    fingerprint: AgentFingerprint,
    start_attempt: Uuid,
    resident_generation: u64,
    active: Mutex<bool>,
    failure: Mutex<Option<MonthlyMonitorFailure>>,
    #[cfg(feature = "test-utils")]
    exited: CancellationToken,
}

impl MonthlyWindowTarget {
    fn record_failure(&self, failure: MonthlyMonitorFailure) {
        let mut current = self.failure.lock().unwrap();
        if current.is_none()
            || matches!(
                current.as_ref(),
                Some(MonthlyMonitorFailure::PolicyUnavailable)
            ) && matches!(failure, MonthlyMonitorFailure::Cleanup(_))
        {
            *current = Some(failure);
        }
    }

    pub(super) fn cleanup_failure(&self) -> Option<WorkerExecutorError> {
        match self.failure.lock().unwrap().as_ref() {
            Some(MonthlyMonitorFailure::Cleanup(error)) => Some(error.clone()),
            _ => None,
        }
    }

    fn policy_is_unavailable(&self) -> bool {
        matches!(
            *self.failure.lock().unwrap(),
            Some(MonthlyMonitorFailure::PolicyUnavailable)
        )
    }

    fn take_policy_failure(&self) -> Option<WorkerExecutorError> {
        let mut failure = self.failure.lock().unwrap();
        if matches!(*failure, Some(MonthlyMonitorFailure::PolicyUnavailable)) {
            *failure = None;
            Some(
                crate::services::resource_limits::MonthlyResourceCapacityError::unavailable_error(),
            )
        } else {
            None
        }
    }

    fn trace_if_current(
        self: &Arc<Self>,
        current: &Mutex<Option<Arc<Self>>>,
    ) -> Option<crate::native_wait_diagnostics::WindowTrace> {
        let active = self.active.lock().unwrap();
        if !*active
            || !current
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|value| Arc::ptr_eq(value, self))
        {
            return None;
        }
        Some(crate::native_wait_diagnostics::WindowTrace {
            id: self.diagnostic_window_id,
            fingerprint: self.fingerprint,
            start_attempt: self.start_attempt,
            resident_generation: self.resident_generation,
            origin: self.trace_origin.clone(),
        })
    }

    fn matches_running_owner(
        self: &Arc<Self>,
        lifecycle: &WorkerInstance,
        fingerprint: AgentFingerprint,
        resident_generation: &std::sync::atomic::AtomicU64,
        current: &Mutex<Option<Arc<Self>>>,
    ) -> bool {
        let WorkerInstance::Running(running) = lifecycle else {
            return false;
        };
        self.matches_current(
            fingerprint,
            running.start_attempt,
            resident_generation.load(std::sync::atomic::Ordering::Acquire),
            running
                .concurrent_agent_permit_held
                .load(std::sync::atomic::Ordering::Acquire),
            current,
        )
    }

    fn matches_current(
        self: &Arc<Self>,
        fingerprint: AgentFingerprint,
        start_attempt: Uuid,
        resident_generation: u64,
        permit_held: bool,
        current: &Mutex<Option<Arc<Self>>>,
    ) -> bool {
        self.fingerprint == fingerprint
            && self.start_attempt == start_attempt
            && self.resident_generation == resident_generation
            && permit_held
            && current
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, self))
    }
}

fn queue_monthly_capacity<'a>(
    proposed: MonthlyCapacity,
    current: MonthlyCapacity,
    interrupts: &mut super::interrupt::InterruptGuard<'_>,
    progress: &'a super::StopProgress,
    monthly: &Mutex<
        Option<(
            InterruptKind,
            crate::services::resource_limits::MonthlyResourceExhaustion,
        )>,
    >,
) -> Option<(
    InterruptKind,
    std::sync::MutexGuard<'a, super::StopProgressState>,
)> {
    if current != proposed || current.exhaustion.is_none() {
        return None;
    }
    let kind = InterruptKind::Suspend(Timestamp::now_utc());
    let admission = progress.lock().unwrap();
    // Monthly observations cannot reopen a terminal stop already claimed by this execution.
    if !admission.retired
        && !matches!(
            **interrupts,
            super::WorkerInterruptState::TerminalClaimed(_)
        )
        && interrupts.queue(PendingWorkerInterrupt {
            kind,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Suspend),
        })
    {
        *monthly.lock().unwrap() = Some((kind, current.exhaustion.unwrap()));
        Some((kind, admission))
    } else {
        None
    }
}

#[cfg(feature = "test-utils")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonthlyProposalForTest {
    pub diagnostic_window_id: u64,
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
            diagnostic_window_id: target.diagnostic_window_id,
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
    pub(super) release_scope: crate::metrics::resource_release::ReleaseScope,
}

struct MonthlyMonitor {
    target: Arc<MonthlyWindowTarget>,
    cancelled: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), WorkerExecutorError>>,
}

#[derive(Clone)]
enum MonthlyMonitorFailure {
    PolicyUnavailable,
    Cleanup(WorkerExecutorError),
}

impl From<WorkerExecutorError> for MonthlyMonitorFailure {
    fn from(error: WorkerExecutorError) -> Self {
        Self::Cleanup(error)
    }
}

impl ExecutionWindow {
    #[cfg(test)]
    pub(super) fn unmonitored(window: ResourceUsageMeteringWindow) -> Self {
        Self::unmonitored_with_scope(window, Default::default())
    }

    #[cfg(test)]
    pub(super) fn unmonitored_with_scope(
        mut window: ResourceUsageMeteringWindow,
        release_scope: crate::metrics::resource_release::ReleaseScope,
    ) -> Self {
        window.observe_permit_release(&release_scope);
        Self {
            window: Some(window),
            monitor: None,
            stop_progress: Arc::default(),
            release_scope,
        }
    }

    pub(super) async fn new<Ctx: WorkerCtx>(
        worker: &Arc<Worker<Ctx>>,
        mut window: ResourceUsageMeteringWindow,
        start_attempt: uuid::Uuid,
    ) -> Self {
        let release_scope = worker.release_scope();
        window.observe_permit_release(&release_scope);
        let resident_generation = worker
            .resident_generation
            .load(std::sync::atomic::Ordering::Acquire);
        let (diagnostic, span) = crate::native_wait_diagnostics::WindowTrace::open(
            worker.initial_worker_metadata.fingerprint,
            start_attempt,
            resident_generation,
        );
        span.record(
            "agent_id",
            tracing::field::display(&worker.owned_agent_id.agent_id),
        );
        span.record(
            "environment_id",
            tracing::field::display(&worker.owned_agent_id.environment_id),
        );
        let diagnostic_window_id = diagnostic.id;
        window.observe_window(span);
        if !worker.resource_entry.monthly_metering_enabled() {
            return Self {
                window: Some(window),
                monitor: None,
                stop_progress: worker.stop_progress.clone(),
                release_scope,
            };
        }
        let monitor = worker.resource_entry.monthly_metering_enabled().then(|| {
            let monitor_release = release_scope.clone();
            if let Some(flusher) = window.usage_flusher() {
                worker
                    .owner_runtime_resources
                    .register_resource_usage_flusher(flusher);
            }
            let target = Arc::new(MonthlyWindowTarget {
                diagnostic_window_id,
                trace_origin: diagnostic.origin.clone(),
                fingerprint: worker.initial_worker_metadata.fingerprint,
                start_attempt,
                resident_generation,
                active: Mutex::new(true),
                failure: Mutex::new(None),
                #[cfg(feature = "test-utils")]
                exited: CancellationToken::new(),
            });
            *worker.monthly_window.lock().unwrap() = Some(target.clone());
            let cancelled = CancellationToken::new();
            let stop = cancelled.clone();
            let target_task = target.clone();
            let weak_worker = Arc::downgrade(worker);
            let actor = worker.state_actor.clone();
            let memory = window.monthly_memory_tracker();
            let mut memory_check = MonthlyMemoryCheck::new(worker.resource_entry.clone(), memory);
            let mut health = window.subscribe_enforcement_failure();
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
                        let duration = memory_check.duration();
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
                                monitor_release.fail(crate::metrics::resource_release::Stage::WindowPrepared, crate::metrics::resource_release::Failure::MonitorJoin);
                                return Err(WorkerExecutorError::runtime("Monthly monitor lost the lifecycle actor").into());
                            },
                            failure = crate::services::resource_usage_metering::wait_for_enforcement_failure(&mut health) => {
                                monitor_release.fail(crate::metrics::resource_release::Stage::WindowPrepared, crate::metrics::resource_release::Failure::MeterFault);
                                return Err(WorkerExecutorError::runtime(format!("Monthly enforcement unavailable: {failure}")).into());
                            },
                            update = memory_check.wait(tick) => {
                                if update.is_err() {
                                    monitor_release.fail(crate::metrics::resource_release::Stage::WindowPrepared, crate::metrics::resource_release::Failure::MonitorJoin);
                                    return Err(WorkerExecutorError::runtime("Monthly monitor lost capacity updates").into());
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
                        if capacity.policy_status == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable {
                            tracing::warn!(
                                agent_id = %worker.owned_agent_id.agent_id,
                                policy_revision = capacity.policy_revision,
                                "Monthly policy is unavailable; stopping execution"
                            );
                            return Err(MonthlyMonitorFailure::PolicyUnavailable);
                        }
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
                    monitor_release.fail(crate::metrics::resource_release::Stage::WindowPrepared, crate::metrics::resource_release::Failure::Panic);
                    let message = panic.downcast_ref::<&str>().copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    Err(WorkerExecutorError::runtime(format!("Monthly monitor panicked: {message}")).into())
                });
                if let Err(error) = &result
                    && let Some(worker) = weak_worker.upgrade()
                {
                    worker.fail_monthly_monitor(&target_task, error.clone()).await;
                }
                match result {
                    Ok(()) | Err(MonthlyMonitorFailure::PolicyUnavailable) => Ok(()),
                    Err(MonthlyMonitorFailure::Cleanup(error)) => Err(error),
                }
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
            release_scope,
        }
    }

    pub(super) async fn prepare_disposal(
        mut self,
    ) -> (
        ResourceUsageMeteringWindow,
        Option<WorkerExecutorError>,
        super::StopAdmissionSeal,
    ) {
        use crate::metrics::resource_release::{Failure, Stage};
        let prepared = self.release_scope.receipt(Stage::WindowPrepared);
        let mut seal = super::StopAdmissionSeal::new(&self.stop_progress);
        let monitor = self.monitor.take();
        stop_monitor(&monitor);
        let result = join_monitor(monitor).await;
        if result.is_err() {
            self.release_scope
                .fail(Stage::WindowPrepared, Failure::MonitorJoin);
        }
        let stopped = seal.join().await;
        if stopped.is_err() {
            self.release_scope
                .fail(Stage::WindowPrepared, Failure::StopDriver);
        }
        let window = self.window.take().unwrap();
        let error = result.and(stopped).err();
        seal.health = error.clone().map_or(Ok(()), Err);
        prepared.complete();
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
            self.release_scope.clone(),
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
                self.release_scope.clone(),
                Instant::now() + Duration::from_secs(30),
            ));
        }
    }
}

fn close_owned_window(
    window: ResourceUsageMeteringWindow,
    monitor: Option<MonthlyMonitor>,
    progress: super::StopProgress,
    release_scope: crate::metrics::resource_release::ReleaseScope,
    deadline: Instant,
) -> impl Future<Output = Result<ResourceUsageSettlement, WorkerExecutorError>> + Send + 'static {
    let mut seal = super::StopAdmissionSeal::new(&progress);
    stop_monitor(&monitor);
    let unmonitored = monitor.is_none();
    let closing_scope = release_scope.clone();
    let closed = release_scope.completion_barrier();
    let closed_in_task = closed.clone();
    let mut closing = Box::pin(async move {
        use crate::metrics::resource_release::{Failure, Stage};
        let mut observation = crate::metrics::resource_release::CleanupObservation::new(
            &closing_scope,
            Stage::EndToEnd,
        );
        let monitor_result = join_monitor(monitor).await;
        if monitor_result.is_err() {
            closing_scope.fail(Stage::EndToEnd, Failure::MonitorJoin);
        }
        let stopped = seal.join().await;
        if stopped.is_err() {
            closing_scope.fail(Stage::EndToEnd, Failure::StopDriver);
        }
        let (settlement, permit) =
            resource_usage_metering::close_window_retaining_permit(window, deadline).await;
        if let Err(error) = &settlement {
            closing_scope.fail(Stage::EndToEnd, error.release_failure());
        }
        let result = monitor_result
            .and(stopped)
            .and(settlement.map_err(|error| WorkerExecutorError::runtime(error.to_string())));
        drop(permit);
        seal.complete(result.as_ref().map(|_| ()).map_err(Clone::clone));
        observation.returned();
        closed_in_task.complete();
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
                release_scope.fail(
                    crate::metrics::resource_release::Stage::EndToEnd,
                    crate::metrics::resource_release::Failure::Panic,
                );
                closed.complete();
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
            release_scope.fail(
                crate::metrics::resource_release::Stage::EndToEnd,
                if error.is_panic() {
                    crate::metrics::resource_release::Failure::Panic
                } else {
                    crate::metrics::resource_release::Failure::ObserverLost
                },
            );
            closed.complete();
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
        Some(monitor) => {
            let result = monitor
                .task
                .await
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!("Monthly monitor failed: {error}"))
                })
                .and_then(|result| result);
            if let Err(error) = &result {
                monitor
                    .target
                    .record_failure(MonthlyMonitorFailure::Cleanup(error.clone()));
            }
            result
        }
        None => Ok(()),
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(super) fn monthly_cleanup_failure(&self) -> Option<WorkerExecutorError> {
        self.monthly_window
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|target| target.cleanup_failure())
    }

    pub(super) fn monthly_policy_is_unavailable(&self) -> bool {
        self.monthly_window
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|target| target.policy_is_unavailable())
    }

    pub(super) fn take_monthly_policy_failure(&self) -> Option<WorkerExecutorError> {
        self.monthly_window
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|target| target.take_policy_failure())
    }

    pub(crate) fn resident_window_trace(
        &self,
    ) -> Option<crate::native_wait_diagnostics::WindowTrace> {
        let target = self.monthly_window.lock().unwrap().clone()?;
        target.trace_if_current(&self.monthly_window)
    }

    async fn fail_monthly_monitor(
        &self,
        target: &Arc<MonthlyWindowTarget>,
        failure: MonthlyMonitorFailure,
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
            target.record_failure(failure);
            let admission = self.stop_progress.lock().unwrap();
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
    pub fn terminal_interrupt_pending_for_test(&self) -> bool {
        *self.terminal_interrupt().borrow()
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
    pub async fn concurrent_agent_permit_is_held(&self) -> bool {
        self.instance.lock().await.concurrent_agent_permit_is_held()
    }

    pub(crate) fn monthly_failure(
        &self,
        trap: &crate::model::TrapType,
    ) -> Option<crate::model::TrapType> {
        if self.agent_mode() != golem_common::model::agent::AgentMode::Ephemeral {
            return None;
        }
        super::outcome::monthly_failure::<Ctx>(
            trap,
            self.agent_mode(),
            &self.monthly_stop.lock().unwrap(),
            &self.config(),
        )
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
        let diagnostic = crate::native_wait_diagnostics::WindowTrace {
            id: target.diagnostic_window_id,
            fingerprint: target.fingerprint,
            start_attempt: target.start_attempt,
            resident_generation: target.resident_generation,
            origin: target.trace_origin.clone(),
        };
        let accepted = self.try_accept_monthly_capacity(target, proposed).await;
        diagnostic.event("monthly_acceptance", Some(proposed), Some(accepted));
        #[cfg(feature = "test-utils")]
        if let Some(completed) = completed {
            let _ = completed.send(accepted);
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
        if !target.matches_running_owner(
            &lifecycle,
            self.initial_worker_metadata.fingerprint,
            &self.resident_generation,
            &self.monthly_window,
        ) {
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
                let Some((kind, admission)) = queue_monthly_capacity(
                    proposed,
                    current,
                    &mut interrupts,
                    &self.stop_progress,
                    &self.monthly_stop,
                ) else {
                    return false;
                };
                self.accept_interrupt(&lifecycle, kind, admission);
                true
            })
    }
}
