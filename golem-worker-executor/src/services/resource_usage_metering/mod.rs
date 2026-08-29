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

use crate::sandbox_filesystem::FilesystemStorageError;
use crate::services::active_agents::ConcurrentAgentPermit;
use crate::services::agent_memory_meter::AgentMemoryMeter;
use crate::services::byte_time_accumulator::{ByteTimeAccumulator, ByteTimeSettlement};
use crate::services::golem_config::ResourceUsageMeteringConfig;
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::resource_limits::{AtomicResourceEntry, ResourceUsageFlusher};
use golem_common::model::agent::AgentMode;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

const BYTE_NANOSECONDS_PER_BYTE_SECOND: u128 = 1_000_000_000;
const FILESYSTEM_FIRST_SAMPLE_DELAY: Duration = Duration::from_millis(10);
const FILESYSTEM_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
const FILESYSTEM_OBSERVATION_TIMEOUT: Duration = Duration::from_millis(250);
const FILESYSTEM_STALE_AFTER: Duration = Duration::from_secs(1);
const FILESYSTEM_CLOSE_BUDGET: Duration = Duration::from_secs(1);
const FILESYSTEM_RETRY_DELAYS: [Duration; 5] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
    Duration::from_secs(1),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesystemUsage {
    Unsupported,
    Authoritative {
        allocated_bytes: u64,
        filesystem_objects: u64,
    },
}

pub(crate) trait FilesystemUsageReader: Send + Sync {
    fn observe(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<FilesystemUsage, FilesystemStorageError>> + Send + '_>>;
}

#[derive(Clone)]
pub(crate) struct FilesystemUsageSource {
    reader: Arc<dyn FilesystemUsageReader>,
}

impl Debug for FilesystemUsageSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemUsageSource")
            .finish_non_exhaustive()
    }
}

impl FilesystemUsageSource {
    pub(crate) fn new(reader: Arc<dyn FilesystemUsageReader>) -> Self {
        Self { reader }
    }

    async fn observe(&self) -> Result<FilesystemUsage, FilesystemStorageError> {
        self.reader.observe().await
    }

    #[cfg(test)]
    fn scripted(reader: Arc<dyn FilesystemUsageReader>) -> Self {
        Self { reader }
    }
}

trait MeteringClock: Send + Sync {
    fn now(&self) -> Instant;

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

struct SystemMeteringClock;

impl MeteringClock for SystemMeteringClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
            deadline,
        )))
    }
}

#[derive(Clone)]
pub(crate) struct ResourceUsageAccount {
    mode: AgentMode,
    entry: Weak<AtomicResourceEntry>,
    linear_memory: LinearMemoryTracker,
    transition: Arc<Mutex<()>>,
}

impl ResourceUsageAccount {
    pub(crate) fn new(
        mode: AgentMode,
        linear_memory: LinearMemoryTracker,
        entry: Arc<AtomicResourceEntry>,
    ) -> Self {
        Self {
            mode,
            entry: Arc::downgrade(&entry),
            transition: linear_memory.resource_transition(),
            linear_memory,
        }
    }

    fn memory_bytes(&self) -> u64 {
        self.linear_memory.current_bytes()
    }

    fn record_usage(&self, memory_units: i64, storage_units: i64) {
        if let Some(entry) = self.entry.upgrade() {
            entry.record_resource_usage(self.mode, memory_units, storage_units);
        }
    }

    fn record_settlement(&self, settlement: ResourceUsageSettlement) {
        if let Some(entry) = self.entry.upgrade() {
            entry.record_resource_settlement(self.mode, settlement.memory, settlement.storage);
        }
    }
}

pub(crate) struct ResourceUsageMeter {
    shared: Option<Arc<MeterShared>>,
}

impl Debug for ResourceUsageMeter {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResourceUsageMeter")
            .finish_non_exhaustive()
    }
}

struct MeterShared {
    usage: OnceLock<FilesystemUsageSource>,
    account: ResourceUsageAccount,
    clock: Arc<dyn MeteringClock>,
    memory_enabled: bool,
    filesystem_enabled: bool,
    observation_lane: Option<Arc<tokio::sync::Semaphore>>,
    next_generation: AtomicU64,
    lifecycle: Mutex<MeterLifecycle>,
}

impl Debug for MeterShared {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MeterShared")
            .finish_non_exhaustive()
    }
}

enum MeterLifecycle {
    Dormant,
    Window {
        generation: u64,
        shared: Arc<WindowShared>,
    },
}

#[must_use]
pub struct ResourceUsageMeteringWindow {
    shared: Option<Arc<WindowShared>>,
    permit: Option<ConcurrentAgentPermit>,
}

impl Debug for ResourceUsageMeteringWindow {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResourceUsageMeteringWindow")
            .finish_non_exhaustive()
    }
}

struct WindowShared {
    meter: Weak<MeterShared>,
    generation: u64,
    opened_at: Instant,
    usage: Option<FilesystemUsageSource>,
    account: ResourceUsageAccount,
    clock: Arc<dyn MeteringClock>,
    memory_enabled: bool,
    observation_lane: Option<Arc<tokio::sync::Semaphore>>,
    observation_changed: tokio::sync::Notify,
    sampler_changed: tokio::sync::Notify,
    state: Mutex<WindowState>,
}

struct WindowState {
    status: WindowStatus,
    sampling: bool,
    next_observation: u64,
    active_observation: Option<ActiveObservation>,
    storage: Option<StorageState>,
    settlement: Option<ResourceUsageSettlement>,
}

#[derive(Clone, Copy)]
struct ActiveObservation {
    sequence: u64,
    started_at: Instant,
}

struct StorageState {
    accumulator: ByteTimeAccumulator,
    level: Option<u64>,
    last_accepted_at: Option<Instant>,
}

impl StorageState {
    fn new(opened_at: Instant) -> Self {
        Self {
            accumulator: ByteTimeAccumulator::new(BYTE_NANOSECONDS_PER_BYTE_SECOND, opened_at),
            level: None,
            last_accepted_at: None,
        }
    }

    fn accrue_until(&mut self, at: Instant, pending_attempt: Option<Instant>) {
        let mut charge_until = at;
        if let Some(started_at) = pending_attempt {
            charge_until = charge_until.min(started_at);
        }
        if let Some(last_accepted_at) = self.last_accepted_at {
            charge_until = charge_until.min(last_accepted_at + FILESYSTEM_STALE_AFTER);
        }
        self.accumulator.advance(charge_until, self.level);
        if charge_until < at && pending_attempt.is_none_or(|started_at| started_at < at) {
            self.accumulator.advance(at, None);
            if self
                .last_accepted_at
                .is_some_and(|accepted| accepted + FILESYSTEM_STALE_AFTER <= at)
            {
                self.level = None;
                self.last_accepted_at = None;
            }
        }
    }

    fn accept(&mut self, allocated_bytes: u64, at: Instant) {
        self.accrue_until(at, None);
        self.level = Some(allocated_bytes);
        self.last_accepted_at = Some(at);
    }

    fn suspend_from(&mut self, attempt_started_at: Instant) {
        self.accrue_until(attempt_started_at, None);
        self.level = None;
        self.last_accepted_at = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowStatus {
    Active,
    Closing,
    Closed,
}

#[derive(Debug)]
pub enum MeteringOpenError {
    AlreadyOpen,
    FilesystemObservation(FilesystemStorageError),
    MemoryMeterStopped,
    OpeningCancelled,
}

impl Display for MeteringOpenError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyOpen => formatter.write_str("a resource usage window is already open"),
            Self::FilesystemObservation(error) => {
                write!(
                    formatter,
                    "failed to observe opening filesystem usage: {error}"
                )
            }
            Self::MemoryMeterStopped => formatter.write_str("the memory meter is stopped"),
            Self::OpeningCancelled => {
                formatter.write_str("resource usage window opening was cancelled")
            }
        }
    }
}

impl std::error::Error for MeteringOpenError {}

#[derive(Debug)]
pub enum MeteringCloseError {
    Deadline,
    FilesystemObservation(FilesystemStorageError),
    Faulted(String),
    ObserverLost,
}

impl Display for MeteringCloseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deadline => {
                formatter.write_str("resource usage window close reached its deadline")
            }
            Self::FilesystemObservation(error) => {
                write!(
                    formatter,
                    "failed to observe closing filesystem usage: {error}"
                )
            }
            Self::Faulted(error) => write!(formatter, "resource usage window faulted: {error}"),
            Self::ObserverLost => formatter.write_str("resource usage close task was lost"),
        }
    }
}

impl std::error::Error for MeteringCloseError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceUsageSettlement {
    pub(crate) memory: ByteTimeSettlement,
    pub(crate) storage: ByteTimeSettlement,
}

#[cfg(test)]
pub(crate) fn create_configured_meter(
    config: ResourceUsageMeteringConfig,
    usage: impl FnOnce() -> FilesystemUsageSource,
    account: ResourceUsageAccount,
) -> ResourceUsageMeter {
    let clock: Arc<dyn MeteringClock> = Arc::new(SystemMeteringClock);
    create_configured_meter_with_clock(config, usage, account, clock)
}

pub(crate) fn create_unbound_meter(
    config: ResourceUsageMeteringConfig,
    account: ResourceUsageAccount,
) -> ResourceUsageMeter {
    create_unbound_meter_with_clock(config, account, Arc::new(SystemMeteringClock))
}

pub(crate) fn install_filesystem_usage(meter: &ResourceUsageMeter, usage: FilesystemUsageSource) {
    let shared = meter
        .shared
        .as_ref()
        .expect("filesystem usage cannot be installed on a disabled meter");
    assert!(shared.filesystem_enabled, "filesystem metering is disabled");
    shared
        .usage
        .set(usage)
        .expect("filesystem usage was already installed");
}

#[cfg(test)]
fn create_configured_meter_with_clock(
    config: ResourceUsageMeteringConfig,
    usage: impl FnOnce() -> FilesystemUsageSource,
    account: ResourceUsageAccount,
    clock: Arc<dyn MeteringClock>,
) -> ResourceUsageMeter {
    let meter = create_unbound_meter_with_clock(config, account, clock);
    if config.filesystem {
        install_filesystem_usage(&meter, usage());
    }
    meter
}

fn create_unbound_meter_with_clock(
    config: ResourceUsageMeteringConfig,
    account: ResourceUsageAccount,
    clock: Arc<dyn MeteringClock>,
) -> ResourceUsageMeter {
    if !config.any_byte_time_enabled() {
        return ResourceUsageMeter { shared: None };
    }
    let entry = account.entry.upgrade();
    let shared = Arc::new(MeterShared {
        usage: OnceLock::new(),
        account,
        clock,
        memory_enabled: config.memory,
        filesystem_enabled: config.filesystem,
        observation_lane: config
            .filesystem
            .then(|| Arc::new(tokio::sync::Semaphore::new(1))),
        next_generation: AtomicU64::new(0),
        lifecycle: Mutex::new(MeterLifecycle::Dormant),
    });
    if let Some(entry) = entry {
        let flusher: Arc<dyn ResourceUsageFlusher> = shared.clone();
        entry.register_resource_usage_flusher(Arc::downgrade(&flusher));
    }
    ResourceUsageMeter {
        shared: Some(shared),
    }
}

pub(crate) fn open_window(
    meter: &ResourceUsageMeter,
    permit: ConcurrentAgentPermit,
) -> Pin<
    Box<
        dyn Future<Output = Result<ResourceUsageMeteringWindow, MeteringOpenError>>
            + Send
            + 'static,
    >,
> {
    let Some(meter) = meter.shared.as_ref().cloned() else {
        return Box::pin(async move {
            Ok(ResourceUsageMeteringWindow {
                shared: None,
                permit: Some(permit),
            })
        });
    };
    Box::pin(async move {
        let generation = meter
            .next_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .expect("resource usage window generation overflowed")
            + 1;
        let mut lifecycle = meter.lifecycle.lock().unwrap();
        if !matches!(*lifecycle, MeterLifecycle::Dormant) {
            return Err(MeteringOpenError::AlreadyOpen);
        }

        let _transition = meter
            .memory_enabled
            .then(|| meter.account.transition.lock().unwrap());
        let opened_at = meter.clock.now();
        if meter.memory_enabled {
            let memory_bytes = meter.account.memory_bytes();
            if !meter
                .account
                .linear_memory
                .meter_if_enabled()
                .expect("memory metering is enabled")
                .resume(memory_bytes, opened_at)
            {
                return Err(MeteringOpenError::MemoryMeterStopped);
            }
        }
        let usage = meter.filesystem_enabled.then(|| {
            meter
                .usage
                .get()
                .expect("filesystem metering is enabled without a usage observer")
                .clone()
        });
        let shared = Arc::new(WindowShared {
            meter: Arc::downgrade(&meter),
            generation,
            opened_at,
            usage,
            account: meter.account.clone(),
            clock: Arc::clone(&meter.clock),
            memory_enabled: meter.memory_enabled,
            observation_lane: meter.observation_lane.clone(),
            observation_changed: tokio::sync::Notify::new(),
            sampler_changed: tokio::sync::Notify::new(),
            state: Mutex::new(WindowState {
                status: WindowStatus::Active,
                sampling: false,
                next_observation: 0,
                active_observation: None,
                storage: meter
                    .filesystem_enabled
                    .then(|| StorageState::new(opened_at)),
                settlement: None,
            }),
        });
        *lifecycle = MeterLifecycle::Window {
            generation,
            shared: Arc::clone(&shared),
        };
        drop(lifecycle);
        shared.start_sampler();
        Ok(ResourceUsageMeteringWindow {
            shared: Some(shared),
            permit: Some(permit),
        })
    })
}

pub fn close_window(
    mut window: ResourceUsageMeteringWindow,
    deadline: Instant,
) -> Pin<
    Box<dyn Future<Output = Result<ResourceUsageSettlement, MeteringCloseError>> + Send + 'static>,
> {
    let Some(shared) = window.shared.take() else {
        let permit = window
            .permit
            .take()
            .expect("unmetered resource window lost its permit");
        return Box::pin(async move {
            drop(permit);
            Ok(ResourceUsageSettlement::default())
        });
    };
    let permit = window
        .permit
        .take()
        .expect("metering window lost its permit");
    shared.begin_close();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_metering_task(async move {
        let result = Arc::clone(&shared).complete_close(deadline).await;
        drop(permit);
        let _ = sender.send(result);
    });
    Box::pin(async move {
        receiver
            .await
            .unwrap_or(Err(MeteringCloseError::ObserverLost))
    })
}

#[cfg(test)]
fn stop_periodic_sampling(window: &ResourceUsageMeteringWindow) {
    if let Some(shared) = &window.shared {
        shared.begin_close();
    }
}

pub(crate) fn stop_metering(meter: &ResourceUsageMeter) {
    let Some(meter) = &meter.shared else {
        return;
    };
    let window = {
        let lifecycle = meter.lifecycle.lock().unwrap();
        match &*lifecycle {
            MeterLifecycle::Window { shared, .. } => Some(Arc::clone(shared)),
            MeterLifecycle::Dormant => None,
        }
    };
    if let Some(window) = window {
        window.begin_close();
    }
}

impl Drop for ResourceUsageMeteringWindow {
    fn drop(&mut self) {
        let (Some(shared), Some(permit)) = (self.shared.take(), self.permit.take()) else {
            return;
        };
        shared.begin_close();
        shared.detach_active_observation();
        let settlement = shared.settle_close(shared.clock.now());
        shared.account.record_settlement(settlement);
        shared.clear_meter();
        drop(permit);
    }
}

impl ResourceUsageMeter {
    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        let Some(shared) = &self.shared else {
            return false;
        };
        let lifecycle = shared.lifecycle.lock().unwrap();
        match &*lifecycle {
            MeterLifecycle::Window { shared, .. } => {
                shared.state.lock().unwrap().status == WindowStatus::Active
            }
            MeterLifecycle::Dormant => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn flush(&self, now: Instant) {
        let Some(shared) = &self.shared else {
            return;
        };
        shared.flush_at(now);
    }
}

impl ResourceUsageFlusher for MeterShared {
    fn flush_usage(&self) {
        self.flush_at(self.clock.now());
    }
}

impl MeterShared {
    fn flush_at(&self, now: Instant) {
        let window = {
            let lifecycle = self.lifecycle.lock().unwrap();
            match &*lifecycle {
                MeterLifecycle::Window { shared, .. } => Some(Arc::clone(shared)),
                MeterLifecycle::Dormant => None,
            }
        };
        let _transition = self
            .memory_enabled
            .then(|| self.account.transition.lock().unwrap());
        let (memory_units, storage_units) = window.map_or((0, 0), |window| {
            let mut state = window.state.lock().unwrap();
            if state.status == WindowStatus::Closed {
                return (0, 0);
            }
            let memory_units = if self.memory_enabled {
                self.account
                    .linear_memory
                    .meter_if_enabled()
                    .expect("memory metering is enabled")
                    .take_units(now)
            } else {
                0
            };
            let pending_attempt = state
                .active_observation
                .map(|observation| observation.started_at);
            let storage_units = state.storage.as_mut().map_or(0, |storage| {
                storage.accrue_until(now, pending_attempt);
                storage.accumulator.take_units()
            });
            (memory_units, storage_units)
        });
        self.account.record_usage(memory_units, storage_units);
    }
}

impl WindowShared {
    fn begin_close(&self) {
        let mut state = self.state.lock().unwrap();
        if state.status == WindowStatus::Active {
            state.status = WindowStatus::Closing;
        }
        drop(state);
        self.sampler_changed.notify_waiters();
    }

    fn start_sampler(self: &Arc<Self>) {
        let should_start = {
            let mut state = self.state.lock().unwrap();
            if state.status != WindowStatus::Active || state.storage.is_none() || state.sampling {
                false
            } else {
                state.sampling = true;
                true
            }
        };
        if !should_start {
            return;
        }
        let window = Arc::clone(self);
        spawn_metering_task(async move {
            Arc::clone(&window).run_sampler().await;
            let mut state = window.state.lock().unwrap();
            state.sampling = false;
            drop(state);
            window.sampler_changed.notify_waiters();
        });
    }

    async fn run_sampler(self: Arc<Self>) {
        let mut deadline = self.opened_at;
        let mut consecutive_failures = 0usize;
        loop {
            if !self.wait_for_active_deadline(deadline).await {
                return;
            }
            let Some(mut observation) = self.start_observation(WindowStatus::Active) else {
                return;
            };
            let timed_out = self
                .wait_for_periodic_result(&mut observation, FILESYSTEM_OBSERVATION_TIMEOUT)
                .await;
            let completed_at = self.clock.now();
            let succeeded = match timed_out {
                PeriodicAttempt::Completed(result) => {
                    self.finish_observation(observation.active, result, completed_at, false)
                }
                PeriodicAttempt::TimedOut => {
                    self.suspend_observation(observation.active);
                    let result = observation.receiver.await.ok();
                    let at = self.clock.now();
                    result.is_some_and(|result| {
                        self.finish_observation(observation.active, result, at, false)
                    })
                }
            };
            if self.state.lock().unwrap().status != WindowStatus::Active {
                return;
            }
            if succeeded {
                consecutive_failures = 0;
                deadline = next_periodic_deadline(self.opened_at, self.clock.now());
            } else {
                let delay = FILESYSTEM_RETRY_DELAYS
                    [consecutive_failures.min(FILESYSTEM_RETRY_DELAYS.len() - 1)];
                consecutive_failures = consecutive_failures.saturating_add(1);
                deadline = completed_at + delay;
            }
        }
    }

    async fn wait_for_active_deadline(&self, deadline: Instant) -> bool {
        loop {
            let changed = self.sampler_changed.notified();
            if self.state.lock().unwrap().status != WindowStatus::Active {
                return false;
            }
            if self.clock.now() >= deadline {
                return true;
            }
            tokio::select! {
                () = self.clock.sleep_until(deadline) => return self.state.lock().unwrap().status == WindowStatus::Active,
                () = changed => {}
            }
        }
    }

    fn start_observation(self: &Arc<Self>, required_status: WindowStatus) -> Option<Observation> {
        let active = {
            let mut state = self.state.lock().unwrap();
            if state.status != required_status
                || state.storage.is_none()
                || state.active_observation.is_some()
            {
                return None;
            }
            state.next_observation = state
                .next_observation
                .checked_add(1)
                .expect("filesystem usage observation sequence overflowed");
            let active = ActiveObservation {
                sequence: state.next_observation,
                started_at: self.clock.now(),
            };
            state.active_observation = Some(active);
            active
        };
        let usage = self
            .usage
            .as_ref()
            .expect("filesystem metering is enabled")
            .clone();
        let observation_lane = Arc::clone(
            self.observation_lane
                .as_ref()
                .expect("filesystem metering is enabled without an observation lane"),
        );
        let window = Arc::clone(self);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        spawn_metering_task(async move {
            let lane = observation_lane
                .acquire_owned()
                .await
                .expect("filesystem observation lane closed");
            if !window.observation_is_current(active, required_status) {
                window.clear_observation(active);
                return;
            }
            let result = usage.observe().await;
            drop(lane);
            let _ = sender.send(result);
        });
        Some(Observation { active, receiver })
    }

    fn observation_is_current(
        &self,
        observation: ActiveObservation,
        required_status: WindowStatus,
    ) -> bool {
        let Some(meter) = self.meter.upgrade() else {
            return false;
        };
        let lifecycle = meter.lifecycle.lock().unwrap();
        if !matches!(
            &*lifecycle,
            MeterLifecycle::Window { generation, shared }
                if *generation == self.generation && std::ptr::eq(shared.as_ref(), self)
        ) {
            return false;
        }
        let state = self.state.lock().unwrap();
        state.status == required_status
            && state.active_observation.map(|active| active.sequence) == Some(observation.sequence)
    }

    async fn wait_for_periodic_result(
        &self,
        observation: &mut Observation,
        timeout: Duration,
    ) -> PeriodicAttempt {
        let timeout_at = observation.active.started_at + timeout;
        tokio::select! {
            result = &mut observation.receiver => PeriodicAttempt::Completed(result.unwrap_or_else(|_| {
                Err(FilesystemStorageError::verification(
                    "observe filesystem usage task completion",
                    std::path::Path::new("<resource-usage-meter>"),
                ))
            })),
            () = self.clock.sleep_until(timeout_at) => PeriodicAttempt::TimedOut,
        }
    }

    fn finish_observation(
        &self,
        observation: ActiveObservation,
        result: Result<FilesystemUsage, FilesystemStorageError>,
        accepted_at: Instant,
        accept_while_closing: bool,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.active_observation.map(|active| active.sequence) != Some(observation.sequence) {
            return false;
        }
        let may_accept = state.status == WindowStatus::Active
            || (accept_while_closing && state.status == WindowStatus::Closing);
        let succeeded = match result {
            Ok(FilesystemUsage::Authoritative {
                allocated_bytes, ..
            }) if may_accept => {
                state
                    .storage
                    .as_mut()
                    .expect("filesystem storage state is missing")
                    .accept(allocated_bytes, accepted_at);
                true
            }
            Ok(FilesystemUsage::Unsupported) | Err(_)
                if may_accept || state.status == WindowStatus::Closing =>
            {
                state
                    .storage
                    .as_mut()
                    .expect("filesystem storage state is missing")
                    .suspend_from(observation.started_at);
                false
            }
            _ => false,
        };
        state.active_observation = None;
        drop(state);
        self.observation_changed.notify_waiters();
        succeeded
    }

    fn suspend_observation(&self, observation: ActiveObservation) {
        let mut state = self.state.lock().unwrap();
        if state.active_observation.map(|active| active.sequence) == Some(observation.sequence)
            && state.status != WindowStatus::Closed
        {
            state
                .storage
                .as_mut()
                .expect("filesystem storage state is missing")
                .suspend_from(observation.started_at);
        }
    }

    async fn complete_close(
        self: Arc<Self>,
        deadline: Instant,
    ) -> Result<ResourceUsageSettlement, MeteringCloseError> {
        if self.state.lock().unwrap().storage.is_some() {
            let final_deadline = deadline.min(self.clock.now() + FILESYSTEM_CLOSE_BUDGET);
            self.run_final_observation_sequence(final_deadline).await;
        }
        let settlement = self.settle_close(self.clock.now());
        self.account.record_settlement(settlement);
        self.clear_meter();
        Ok(settlement)
    }

    async fn run_final_observation_sequence(self: &Arc<Self>, deadline: Instant) {
        if !self.wait_for_observation(deadline).await {
            self.detach_active_observation();
            return;
        }
        loop {
            if self.clock.now() >= deadline {
                return;
            }
            let Some(mut observation) = self.start_observation(WindowStatus::Closing) else {
                return;
            };
            let timeout_at =
                (observation.active.started_at + FILESYSTEM_OBSERVATION_TIMEOUT).min(deadline);
            let result = tokio::select! {
                result = &mut observation.receiver => Some(result.ok()),
                () = self.clock.sleep_until(timeout_at) => None,
            };
            match result.flatten() {
                Some(result) => {
                    let accepted =
                        self.finish_observation(observation.active, result, self.clock.now(), true);
                    if accepted {
                        return;
                    }
                }
                None => {
                    self.suspend_observation(observation.active);
                    if timeout_at >= deadline {
                        self.detach_late_observation(observation);
                        return;
                    }
                    let late = tokio::select! {
                        result = &mut observation.receiver => Some(result.ok()),
                        () = self.clock.sleep_until(deadline) => None,
                    };
                    match late.flatten() {
                        Some(result) => {
                            let accepted = self.finish_observation(
                                observation.active,
                                result,
                                self.clock.now(),
                                true,
                            );
                            if accepted {
                                return;
                            }
                        }
                        None => {
                            self.detach_late_observation(observation);
                            return;
                        }
                    }
                }
            }
        }
    }

    fn detach_late_observation(&self, observation: Observation) {
        self.clear_observation(observation.active);
    }

    async fn wait_for_observation(&self, deadline: Instant) -> bool {
        loop {
            let changed = self.observation_changed.notified();
            if self.state.lock().unwrap().active_observation.is_none() {
                return true;
            }
            if self.clock.now() >= deadline {
                return false;
            }
            tokio::select! {
                () = changed => {}
                () = self.clock.sleep_until(deadline) => return false,
            }
        }
    }

    fn clear_observation(&self, observation: ActiveObservation) {
        let mut state = self.state.lock().unwrap();
        if state.active_observation.map(|active| active.sequence) == Some(observation.sequence) {
            state.active_observation = None;
        }
        drop(state);
        self.observation_changed.notify_waiters();
    }

    fn detach_active_observation(&self) {
        let mut state = self.state.lock().unwrap();
        let Some(observation) = state.active_observation.take() else {
            return;
        };
        state
            .storage
            .as_mut()
            .expect("filesystem storage state is missing")
            .suspend_from(observation.started_at);
        drop(state);
        self.observation_changed.notify_waiters();
    }

    fn settle_close(&self, closed_at: Instant) -> ResourceUsageSettlement {
        let _transition = self
            .memory_enabled
            .then(|| self.account.transition.lock().unwrap());
        let mut state = self.state.lock().unwrap();
        if let Some(settlement) = state.settlement {
            return settlement;
        }
        let pending_attempt = state
            .active_observation
            .map(|observation| observation.started_at);
        if let Some(storage) = state.storage.as_mut() {
            storage.accrue_until(closed_at, pending_attempt);
        }
        if self.memory_enabled {
            let memory_bytes = self.account.memory_bytes();
            let meter = self
                .account
                .linear_memory
                .meter_if_enabled()
                .expect("memory metering is enabled");
            meter.set_bytes(memory_bytes, closed_at);
            meter.pause(closed_at);
        }
        let settlement = ResourceUsageSettlement {
            memory: self.account.linear_memory.meter_if_enabled().map_or_else(
                ByteTimeSettlement::default,
                AgentMemoryMeter::take_settlement,
            ),
            storage: state
                .storage
                .as_mut()
                .map_or_else(ByteTimeSettlement::default, |storage| {
                    storage.accumulator.take_settlement()
                }),
        };
        state.status = WindowStatus::Closed;
        state.settlement = Some(settlement);
        settlement
    }

    fn clear_meter(&self) {
        let Some(meter) = self.meter.upgrade() else {
            return;
        };
        let mut lifecycle = meter.lifecycle.lock().unwrap();
        if matches!(&*lifecycle, MeterLifecycle::Window { generation, .. } if *generation == self.generation)
        {
            *lifecycle = MeterLifecycle::Dormant;
        }
    }
}

struct Observation {
    active: ActiveObservation,
    receiver: tokio::sync::oneshot::Receiver<Result<FilesystemUsage, FilesystemStorageError>>,
}

enum PeriodicAttempt {
    Completed(Result<FilesystemUsage, FilesystemStorageError>),
    TimedOut,
}

fn next_periodic_deadline(opened_at: Instant, now: Instant) -> Instant {
    let first = opened_at + FILESYSTEM_FIRST_SAMPLE_DELAY;
    if now < first {
        return first;
    }
    let elapsed = now.saturating_duration_since(opened_at).as_nanos();
    let interval = FILESYSTEM_SAMPLE_INTERVAL.as_nanos();
    let periods = elapsed / interval + 1;
    opened_at
        + FILESYSTEM_SAMPLE_INTERVAL
            .checked_mul(u32::try_from(periods).unwrap_or(u32::MAX))
            .unwrap_or(Duration::MAX)
}

fn spawn_metering_task(task: impl Future<Output = ()> + Send + 'static) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(task);
    } else {
        std::thread::Builder::new()
            .name("resource-usage-metering".to_string())
            .spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build resource usage metering runtime")
                    .block_on(task);
            })
            .expect("failed to start resource usage metering thread");
    }
}

#[cfg(test)]
mod tests;
