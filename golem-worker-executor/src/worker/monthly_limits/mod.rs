use super::{PendingWorkerInterrupt, UnloadReason, UnloadRequest, Worker, WorkerInstance};
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
    active_span: Mutex<Option<tracing::Span>>,
    failure: Mutex<Option<MonthlyMonitorFailure>>,
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
        current: &Mutex<Option<Arc<Self>>>,
    ) -> bool {
        let WorkerInstance::Running(running) = lifecycle else {
            return false;
        };
        self.matches_current(
            running.start_attempt,
            running
                .concurrent_agent_permit_held
                .load(std::sync::atomic::Ordering::Acquire),
            current,
        )
    }

    fn matches_current(
        self: &Arc<Self>,
        start_attempt: Uuid,
        permit_held: bool,
        current: &Mutex<Option<Arc<Self>>>,
    ) -> bool {
        // The private Worker slot binds the target to its owner. Each new window replaces
        // the Arc, and closing a window deactivates it before its permit can be reused.
        self.start_attempt == start_attempt
            && permit_held
            && current
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, self))
    }
}

fn queue_monthly_capacity(
    proposed: MonthlyCapacity,
    current: MonthlyCapacity,
    interrupts: &mut super::interrupt::InterruptGuard<'_>,
    tail: &Mutex<Arc<super::InterruptEstablishment>>,
    monthly: &Mutex<
        Option<(
            InterruptKind,
            crate::services::resource_limits::MonthlyResourceExhaustion,
        )>,
    >,
) -> Option<PendingWorkerInterrupt> {
    if current != proposed || current.exhaustion.is_none() {
        return None;
    }
    let kind = InterruptKind::Suspend(Timestamp::now_utc());
    // Monthly observations cannot reopen a terminal stop already claimed by this execution.
    if matches!(**interrupts, super::WorkerInterruptState::TerminalClaimed) {
        return None;
    }
    let pending = super::register_interrupt_request(
        interrupts,
        tail,
        kind,
        false,
        UnloadRequest::ordinary(UnloadReason::Suspend),
    )?;
    *monthly.lock().unwrap() = Some((kind, current.exhaustion.unwrap()));
    Some(pending)
}

pub(super) struct ExecutionWindow {
    window: Option<ResourceUsageMeteringWindow>,
    monitor: Option<MonthlyMonitor>,
    #[cfg(test)]
    close_tasks: Arc<std::sync::atomic::AtomicUsize>,
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
            #[cfg(test)]
            close_tasks: Arc::default(),
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
        span.record("worker_shell", Arc::as_ptr(worker) as usize as u64);
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
                #[cfg(test)]
                close_tasks: Arc::default(),
                release_scope,
            };
        }
        let monitor = worker.resource_entry.monthly_metering_enabled().then(|| {
            let monitor_release = release_scope.clone();
            if let Some(flusher) = window.usage_flusher() {
                worker
                    .owner_runtime_resources
                    .register_resource_usage_flusher(flusher);
                worker.owner_runtime_resources.trace_usage_flushers(
                    &worker.owned_agent_id,
                    "primary_registered",
                );
            }
            let target = Arc::new(MonthlyWindowTarget {
                diagnostic_window_id,
                trace_origin: diagnostic.origin.clone(),
                fingerprint: worker.initial_worker_metadata.fingerprint,
                start_attempt,
                resident_generation,
                active: Mutex::new(true),
                active_span: Mutex::new(Some(tracing::debug_span!(
                    target: golem_common::tracing::target::NATIVE_WAIT, parent: None,
                    "monthly_horizon", window_id = diagnostic_window_id,
                    worker_shell = Arc::as_ptr(worker) as usize as u64,
                    fingerprint_high = (worker.initial_worker_metadata.fingerprint.0.as_u128() >> 64) as u64,
                    fingerprint_low = worker.initial_worker_metadata.fingerprint.0.as_u128() as u64,
                ))),
                failure: Mutex::new(None),
            });
            tracing::debug!(
                execution_event = "monthly_horizon_registered",
                fingerprint = %worker.initial_worker_metadata.fingerprint,
                worker_shell = Arc::as_ptr(worker) as usize as u64,
                trace_enabled = target.active_span.lock().unwrap().as_ref().is_some_and(|span| !span.is_disabled()),
                "Registered monthly horizon lifetime"
            );
            *worker.monthly_window.lock().unwrap() = Some(target.clone());
            let cancelled = CancellationToken::new();
            let stop = cancelled.clone();
            let target_task = target.clone();
            let diagnostic_task = diagnostic.clone();
            let weak_worker = Arc::downgrade(worker);
            let actor = worker.state_actor.clone();
            let memory = window.monthly_memory_tracker();
            let mut memory_check = MonthlyMemoryCheck::new(worker.resource_entry.clone(), memory);
            let mut health = window.subscribe_enforcement_failure();
            let task = tokio::spawn(async move {
                let result = std::panic::AssertUnwindSafe(async {
                    loop {
                        let Some(worker) = weak_worker.upgrade() else {
                            break;
                        };
                        worker.owner_runtime_resources.settle_resource_usage();
                        let duration = memory_check.duration();
                        drop(worker);
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
                        diagnostic_task.event("monthly_checked", Some(capacity), None);
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
            #[cfg(test)]
            close_tasks: Arc::default(),
            release_scope,
        }
    }

    pub(super) async fn prepare_disposal(
        mut self,
    ) -> (ResourceUsageMeteringWindow, Option<WorkerExecutorError>) {
        use crate::metrics::resource_release::{Failure, Stage};
        let prepared = self.release_scope.receipt(Stage::WindowPrepared);
        let monitor = self.monitor.take();
        stop_monitor(&monitor);
        let result = join_monitor(monitor).await;
        if result.is_err() {
            self.release_scope
                .fail(Stage::WindowPrepared, Failure::MonitorJoin);
        }
        let window = self.window.take().unwrap();
        let error = result.err();
        prepared.complete();
        (window, error)
    }

    pub(super) fn close(
        mut self,
        deadline: Instant,
    ) -> impl Future<Output = Result<ResourceUsageSettlement, WorkerExecutorError>> + Send + 'static
    {
        close_owned_window(
            self.window.take().unwrap(),
            self.monitor.take(),
            #[cfg(test)]
            self.close_tasks.clone(),
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
                #[cfg(test)]
                self.close_tasks.clone(),
                self.release_scope.clone(),
                Instant::now() + Duration::from_secs(30),
            ));
        }
    }
}

fn close_owned_window(
    window: ResourceUsageMeteringWindow,
    monitor: Option<MonthlyMonitor>,
    #[cfg(test)] close_tasks: Arc<std::sync::atomic::AtomicUsize>,
    release_scope: crate::metrics::resource_release::ReleaseScope,
    deadline: Instant,
) -> impl Future<Output = Result<ResourceUsageSettlement, WorkerExecutorError>> + Send + 'static {
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
        let (settlement, permit) =
            resource_usage_metering::close_window_retaining_permit(window, deadline).await;
        if let Err(error) = &settlement {
            closing_scope.fail(Stage::EndToEnd, error.release_failure());
        }
        let result = monitor_result
            .and(settlement.map_err(|error| WorkerExecutorError::runtime(error.to_string())));
        drop(permit);
        observation.returned();
        closed_in_task.complete();
        result
    });
    // Poll by mutable pin so pending cleanup keeps its permit when moved to the task.
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
    close_tasks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        let mut active = monitor.target.active.lock().unwrap();
        *active = false;
        drop(monitor.target.active_span.lock().unwrap().take());
        drop(active);
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
        self: &Arc<Self>,
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
            let kind = InterruptKind::Suspend(Timestamp::now_utc());
            if let Some(pending) = super::register_interrupt_request(
                &mut interrupts,
                &self.interrupt_establishment_tail,
                kind,
                false,
                UnloadRequest::ordinary(UnloadReason::Failure),
            ) {
                drop(self.start_interrupt_establishment(pending, true));
            }
            return;
        }
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
        self: &Arc<Self>,
        target: Arc<MonthlyWindowTarget>,
        proposed: MonthlyCapacity,
    ) {
        let diagnostic = crate::native_wait_diagnostics::WindowTrace {
            id: target.diagnostic_window_id,
            fingerprint: target.fingerprint,
            start_attempt: target.start_attempt,
            resident_generation: target.resident_generation,
            origin: target.trace_origin.clone(),
        };
        let accepted = self.try_accept_monthly_capacity(target, proposed).await;
        diagnostic.event("monthly_acceptance", Some(proposed), Some(accepted));
    }

    async fn try_accept_monthly_capacity(
        self: &Arc<Self>,
        target: Arc<MonthlyWindowTarget>,
        proposed: MonthlyCapacity,
    ) -> bool {
        if !self.is_current_cached_owner().await {
            return false;
        }
        let lifecycle = self.instance.lock().await;
        if !target.matches_running_owner(&lifecycle, &self.monthly_window) {
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
                let Some(pending) = queue_monthly_capacity(
                    proposed,
                    current,
                    &mut interrupts,
                    &self.interrupt_establishment_tail,
                    &self.monthly_stop,
                ) else {
                    return false;
                };
                drop(self.start_interrupt_establishment(pending, true));
                true
            })
    }
}
