use super::*;
use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentId;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use test_r::{test, timeout};
use tokio::sync::Semaphore;
use uuid::Uuid;

const GIB: u64 = 1024 * 1024 * 1024;

struct TestClock {
    base: Instant,
    offset_nanos: AtomicU64,
    sleep_deadline_nanos: AtomicU64,
    changed: tokio::sync::Notify,
}

impl TestClock {
    fn new(base: Instant) -> Arc<Self> {
        Arc::new(Self {
            base,
            offset_nanos: AtomicU64::new(0),
            sleep_deadline_nanos: AtomicU64::new(u64::MAX),
            changed: tokio::sync::Notify::new(),
        })
    }

    async fn set(&self, duration: Duration) {
        self.offset_nanos.store(
            u64::try_from(duration.as_nanos()).unwrap(),
            Ordering::Release,
        );
        self.changed.notify_waiters();
        tokio::task::yield_now().await;
    }

    async fn wait_for_sleep_until(&self, deadline: Duration) {
        let deadline_nanos = u64::try_from(deadline.as_nanos()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while self.sleep_deadline_nanos.load(Ordering::Acquire) != deadline_nanos {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

impl MeteringClock for TestClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_nanos(self.offset_nanos.load(Ordering::Acquire))
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.sleep_deadline_nanos.store(
            u64::try_from(deadline.duration_since(self.base).as_nanos()).unwrap(),
            Ordering::Release,
        );
        Box::pin(async move {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.now() >= deadline {
                    return;
                }
                changed.await;
            }
        })
    }
}

struct ObservationGate {
    result: Mutex<Option<Result<FilesystemUsage, FilesystemStorageError>>>,
    started: Semaphore,
    released: Semaphore,
}

impl ObservationGate {
    fn pending(result: Result<FilesystemUsage, FilesystemStorageError>) -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(Some(result)),
            started: Semaphore::new(0),
            released: Semaphore::new(0),
        })
    }

    fn ready(result: Result<FilesystemUsage, FilesystemStorageError>) -> Arc<Self> {
        let gate = Self::pending(result);
        gate.released.add_permits(1);
        gate
    }

    async fn wait_started(&self) {
        tokio::time::timeout(Duration::from_secs(1), self.started.acquire())
            .await
            .expect("filesystem usage observation did not start")
            .unwrap()
            .forget();
    }

    fn release(&self) {
        self.released.add_permits(1);
    }
}

struct ScriptedUsageReader {
    observations: Mutex<VecDeque<Arc<ObservationGate>>>,
    calls: AtomicUsize,
    active: AtomicUsize,
    maximum_active: AtomicUsize,
}

impl ScriptedUsageReader {
    fn new(observations: Vec<Arc<ObservationGate>>) -> Arc<Self> {
        Arc::new(Self {
            observations: Mutex::new(observations.into()),
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            maximum_active: AtomicUsize::new(0),
        })
    }
}

impl FilesystemUsageReader for ScriptedUsageReader {
    fn observe(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<FilesystemUsage, FilesystemStorageError>> + Send + '_>>
    {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.maximum_active.fetch_max(active, Ordering::AcqRel);
        let gate = self
            .observations
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted filesystem usage observation exhausted");
        Box::pin(async move {
            gate.started.add_permits(1);
            gate.released.acquire().await.unwrap().forget();
            self.active.fetch_sub(1, Ordering::AcqRel);
            gate.result.lock().unwrap().take().unwrap()
        })
    }
}

fn authoritative(bytes: u64) -> Result<FilesystemUsage, FilesystemStorageError> {
    Ok(FilesystemUsage::Authoritative {
        allocated_bytes: bytes,
        filesystem_objects: bytes / 10,
    })
}

fn observation_error() -> FilesystemStorageError {
    FilesystemStorageError::verification("observe scripted filesystem usage", Path::new("<test>"))
}

fn configured_account(
    entry: &Arc<AtomicResourceEntry>,
    memory_bytes: u64,
    memory_metering: bool,
    now: Instant,
) -> (ResourceUsageAccount, LinearMemoryTracker) {
    let memory = if memory_metering {
        LinearMemoryTracker::new(
            memory_bytes,
            memory_bytes,
            AgentMode::Durable,
            false,
            entry.clone(),
            Arc::new(Mutex::new(MemoryGrant::inert(0))),
            now,
        )
    } else {
        LinearMemoryTracker::new_with_metering(
            memory_bytes,
            memory_bytes,
            AgentMode::Durable,
            false,
            entry.clone(),
            Arc::new(Mutex::new(MemoryGrant::inert(0))),
            false,
        )
    };
    (
        ResourceUsageAccount::new(AgentMode::Durable, memory.clone(), entry.clone()),
        memory,
    )
}

fn meter(
    reader: Arc<ScriptedUsageReader>,
    clock: Arc<TestClock>,
    entry: &Arc<AtomicResourceEntry>,
    memory_bytes: u64,
) -> (ResourceUsageMeter, LinearMemoryTracker) {
    let (account, memory) = configured_account(entry, memory_bytes, true, clock.base);
    let clock_trait: Arc<dyn MeteringClock> = clock;
    let meter = create_configured_meter_with_clock(
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: true,
        },
        || FilesystemUsageSource::scripted(reader),
        account,
        clock_trait,
    );
    (meter, memory)
}

async fn permit(
    entry: &Arc<AtomicResourceEntry>,
) -> (
    Arc<ConcurrentAgentsScheduler>,
    AccountId,
    ConcurrentAgentPermit,
) {
    let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
    let account_id = AccountId(Uuid::new_v4());
    scheduler.register_account(account_id, entry.clone()).await;
    let permit = scheduler
        .acquire(
            account_id,
            AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "metered-agent".to_string(),
            },
        )
        .await;
    (scheduler, account_id, permit)
}

async fn wait_for_calls(reader: &ScriptedUsageReader, expected: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while reader.calls.load(Ordering::Acquire) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_observations_to_finish(reader: &ScriptedUsageReader) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while reader.active.load(Ordering::Acquire) != 0 {
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;
    })
    .await
    .unwrap();
}

async fn wait_for_observation_state(window: &ResourceUsageMeteringWindow) {
    let shared = window.shared.as_ref().unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while shared.state.lock().unwrap().active_observation.is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_active_observation(window: &ResourceUsageMeteringWindow) {
    let shared = window.shared.as_ref().unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while shared.state.lock().unwrap().active_observation.is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn deployment_switches_construct_only_enabled_storage_state() {
    for memory in [false, true] {
        for filesystem in [false, true] {
            let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
            let (account, tracker) = configured_account(&entry, GIB, memory, Instant::now());
            let constructions = Arc::new(AtomicUsize::new(0));
            let factory_constructions = Arc::clone(&constructions);
            let reader = ScriptedUsageReader::new(Vec::new());
            let meter = create_configured_meter(
                ResourceUsageMeteringConfig {
                    compute: false,
                    memory,
                    filesystem,
                },
                move || {
                    factory_constructions.fetch_add(1, Ordering::AcqRel);
                    FilesystemUsageSource::scripted(reader)
                },
                account,
            );

            assert_eq!(meter.shared.is_some(), memory || filesystem);
            assert_eq!(tracker.meter_if_enabled().is_some(), memory);
            assert_eq!(
                meter
                    .shared
                    .as_ref()
                    .is_some_and(|shared| shared.observation_lane.is_some()),
                filesystem
            );
            assert_eq!(
                constructions.load(Ordering::Acquire),
                usize::from(filesystem)
            );
        }
    }
}

#[test]
async fn filesystem_observation_lane_is_constructed_only_when_filesystem_metering_is_enabled() {
    for (memory, filesystem) in [(false, false), (true, false), (false, true), (true, true)] {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
        let (account, _) = configured_account(&entry, 0, memory, Instant::now());
        let meter = create_configured_meter(
            ResourceUsageMeteringConfig {
                compute: false,
                memory,
                filesystem,
            },
            || FilesystemUsageSource::scripted(ScriptedUsageReader::new(Vec::new())),
            account,
        );

        assert_eq!(
            meter
                .shared
                .as_ref()
                .is_some_and(|shared| shared.observation_lane.is_some()),
            filesystem
        );
        if !filesystem {
            let (_, _, permit) = permit(&entry).await;
            let window = open_window(&meter, permit).await.unwrap();
            assert!(
                window
                    .shared
                    .as_ref()
                    .and_then(|shared| shared.observation_lane.as_ref())
                    .is_none()
            );
            close_window(window, Instant::now() + Duration::from_secs(1))
                .await
                .unwrap();
        }
    }
}

#[test]
async fn disabled_storage_opens_without_observer_task_or_effect_tokens() {
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (account, _) = configured_account(&entry, GIB, true, Instant::now());
    let constructions = Arc::new(AtomicUsize::new(0));
    let factory_constructions = Arc::clone(&constructions);
    let meter = create_configured_meter(
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        },
        move || {
            factory_constructions.fetch_add(1, Ordering::AcqRel);
            panic!("disabled storage constructed an observer")
        },
        account,
    );
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();

    assert_eq!(constructions.load(Ordering::Acquire), 0);
    close_window(window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[test]
#[timeout("5s")]
async fn sampler_uses_immediate_ten_millisecond_and_anchored_hundred_millisecond_deadlines() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let baseline = ObservationGate::pending(authoritative(100));
    let first = ObservationGate::pending(authoritative(200));
    let sustained = ObservationGate::pending(authoritative(300));
    let final_observation = ObservationGate::ready(authoritative(300));
    let reader = ScriptedUsageReader::new(vec![
        Arc::clone(&baseline),
        Arc::clone(&first),
        Arc::clone(&sustained),
        final_observation,
    ]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 0);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();

    baseline.wait_started().await;
    baseline.release();
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    clock.set(Duration::from_millis(9)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(reader.calls.load(Ordering::Acquire), 1);
    clock.set(Duration::from_millis(10)).await;
    first.wait_started().await;
    first.release();
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    clock.set(Duration::from_millis(99)).await;
    assert_eq!(reader.calls.load(Ordering::Acquire), 2);
    clock.set(Duration::from_millis(100)).await;
    sustained.wait_started().await;
    sustained.release();
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;

    close_window(window, now + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(reader.calls.load(Ordering::Acquire), 4);
}

#[test]
#[timeout("5s")]
async fn timeout_keeps_single_flight_skips_ticks_and_accepts_late_success() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let baseline = ObservationGate::ready(authoritative(100));
    let blocked = ObservationGate::pending(authoritative(200));
    let next = ObservationGate::pending(authoritative(300));
    let final_observation = ObservationGate::ready(authoritative(300));
    let reader = ScriptedUsageReader::new(vec![
        baseline,
        Arc::clone(&blocked),
        Arc::clone(&next),
        final_observation,
    ]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 0);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    wait_for_calls(&reader, 1).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    clock.set(Duration::from_millis(10)).await;
    blocked.wait_started().await;

    clock.set(Duration::from_millis(259)).await;
    assert_eq!(
        window
            .shared
            .as_ref()
            .unwrap()
            .state
            .lock()
            .unwrap()
            .storage
            .as_ref()
            .unwrap()
            .level,
        Some(100)
    );
    clock.set(Duration::from_millis(260)).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let suspended = window
                .shared
                .as_ref()
                .unwrap()
                .state
                .lock()
                .unwrap()
                .storage
                .as_ref()
                .unwrap()
                .level
                .is_none();
            if suspended {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    clock.set(Duration::from_millis(500)).await;
    assert_eq!(reader.calls.load(Ordering::Acquire), 2);
    assert_eq!(reader.maximum_active.load(Ordering::Acquire), 1);
    blocked.release();
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    clock.set(Duration::from_millis(599)).await;
    assert_eq!(reader.calls.load(Ordering::Acquire), 2);
    clock.set(Duration::from_millis(600)).await;
    next.wait_started().await;
    next.release();
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;

    close_window(window, now + Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(reader.maximum_active.load(Ordering::Acquire), 1);
}

#[test]
#[timeout("5s")]
async fn failure_backoff_suspends_at_attempt_start_and_recovery_is_prospective() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let reader = ScriptedUsageReader::new(vec![
        ObservationGate::ready(authoritative(1_000)),
        ObservationGate::ready(Err(observation_error())),
        ObservationGate::ready(authoritative(3_000)),
        ObservationGate::ready(authoritative(3_000)),
        ObservationGate::ready(authoritative(3_000)),
    ]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 0);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    wait_for_calls(&reader, 1).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    clock.wait_for_sleep_until(Duration::from_millis(10)).await;
    clock.set(Duration::from_millis(10)).await;
    wait_for_calls(&reader, 2).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    clock.wait_for_sleep_until(Duration::from_millis(110)).await;
    clock.set(Duration::from_millis(109)).await;
    assert_eq!(reader.calls.load(Ordering::Acquire), 2);
    clock.set(Duration::from_millis(110)).await;
    wait_for_calls(&reader, 3).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    clock.wait_for_sleep_until(Duration::from_millis(200)).await;
    clock.set(Duration::from_millis(210)).await;
    wait_for_calls(&reader, 4).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    meter.flush(clock.now());

    assert_eq!(entry.durable_byte_seconds_delta(), 310);
    close_window(window, now + Duration::from_secs(1))
        .await
        .unwrap();
}

#[test]
#[timeout("5s")]
async fn retry_backoff_uses_the_full_capped_sequence() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let mut observations = vec![ObservationGate::ready(authoritative(1_000))];
    observations.extend((0..6).map(|_| ObservationGate::ready(Err(observation_error()))));
    observations.push(ObservationGate::ready(authoritative(2_000)));
    observations.push(ObservationGate::ready(authoritative(2_000)));
    let reader = ScriptedUsageReader::new(observations);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 0);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    wait_for_calls(&reader, 1).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;

    for (index, deadline_millis) in [10, 110, 310, 710, 1_510, 2_510, 3_510]
        .into_iter()
        .enumerate()
    {
        clock.set(Duration::from_millis(deadline_millis - 1)).await;
        assert_eq!(reader.calls.load(Ordering::Acquire), index + 1);
        clock.set(Duration::from_millis(deadline_millis)).await;
        wait_for_calls(&reader, index + 2).await;
        wait_for_observations_to_finish(&reader).await;
        wait_for_observation_state(&window).await;
    }

    close_window(window, now + Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(reader.calls.load(Ordering::Acquire), 9);
}

#[test]
fn scheduler_staleness_caps_accrual_at_one_second() {
    let now = Instant::now();
    let mut storage = StorageState::new(now);
    storage.accept(100, now);

    storage.accrue_until(now + Duration::from_secs(3), None);

    assert_eq!(storage.accumulator.take_settlement().units, 100);
    assert_eq!(storage.level, None);
}

#[test]
#[timeout("5s")]
async fn storage_timeout_does_not_pause_memory_or_fault_the_window() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let blocked = ObservationGate::pending(authoritative(100));
    let reader = ScriptedUsageReader::new(vec![Arc::clone(&blocked)]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader, clock.clone(), &entry, GIB);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    blocked.wait_started().await;

    clock.set(Duration::from_secs(2)).await;
    meter.flush(clock.now());

    assert!(meter.is_active());
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 2);
    drop(window);
    blocked.release();
}

#[test]
#[timeout("5s")]
async fn unsupported_filesystem_disables_only_storage_metering() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let reader = ScriptedUsageReader::new(vec![ObservationGate::ready(Ok(
        FilesystemUsage::Unsupported,
    ))]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, GIB);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    wait_for_calls(&reader, 1).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;

    assert!(
        window
            .shared
            .as_ref()
            .unwrap()
            .state
            .lock()
            .unwrap()
            .storage
            .is_none()
    );
    clock.set(Duration::from_secs(2)).await;
    meter.flush(clock.now());

    assert_eq!(reader.calls.load(Ordering::Acquire), 1);
    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 2);
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    close_window(window, now + Duration::from_secs(3))
        .await
        .unwrap();
}

#[test]
#[timeout("5s")]
async fn account_batch_flushes_active_memory_and_storage_without_close_duplication() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let reader = ScriptedUsageReader::new(vec![
        ObservationGate::ready(authoritative(100)),
        ObservationGate::ready(authoritative(100)),
        ObservationGate::ready(authoritative(100)),
        ObservationGate::ready(authoritative(100)),
    ]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 2 * GIB);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    wait_for_calls(&reader, 1).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;

    clock.offset_nanos.store(
        u64::try_from(Duration::from_millis(500).as_nanos()).unwrap(),
        Ordering::Release,
    );
    assert_eq!(entry.capture_byte_time_usage_for_test(), (1, 50));
    close_window(window, now + Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
}

#[test]
#[timeout("5s")]
async fn active_error_completed_during_close_suspends_from_attempt_start() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let crossing_error = ObservationGate::pending(Err(observation_error()));
    let reader = ScriptedUsageReader::new(vec![
        ObservationGate::ready(authoritative(100)),
        Arc::clone(&crossing_error),
        ObservationGate::ready(authoritative(200)),
    ]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 0);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    wait_for_calls(&reader, 1).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    clock.set(Duration::from_millis(10)).await;
    crossing_error.wait_started().await;

    stop_periodic_sampling(&window);
    let close = tokio::spawn(close_window(window, now + Duration::from_secs(1)));
    clock.set(Duration::from_millis(50)).await;
    crossing_error.release();
    close.await.unwrap().unwrap();

    assert_eq!(entry.durable_byte_seconds_delta(), 1);
}

#[test]
#[timeout("5s")]
async fn close_retries_fast_errors_without_backoff() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let final_failure = ObservationGate::ready(Err(observation_error()));
    let final_success = ObservationGate::ready(authoritative(200));
    let reader = ScriptedUsageReader::new(vec![
        ObservationGate::ready(authoritative(100)),
        final_failure,
        final_success,
    ]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 0);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    wait_for_calls(&reader, 1).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&window).await;
    close_window(window, now + Duration::from_secs(1))
        .await
        .unwrap();

    assert_eq!(reader.calls.load(Ordering::Acquire), 3);
}

#[test]
#[timeout("5s")]
async fn close_timeout_exhausts_in_customers_favor_and_rejects_late_generation_result() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let final_blocked = ObservationGate::pending(authoritative(9_000));
    let next_baseline = ObservationGate::ready(authoritative(50));
    let next_final = ObservationGate::ready(authoritative(50));
    let reader = ScriptedUsageReader::new(vec![
        ObservationGate::ready(authoritative(100)),
        Arc::clone(&final_blocked),
        next_baseline,
        next_final,
    ]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 0);
    let (scheduler, account_id, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    wait_for_calls(&reader, 1).await;
    let close = tokio::spawn(close_window(window, now + Duration::from_secs(10)));
    final_blocked.wait_started().await;
    clock.set(Duration::from_secs(1)).await;

    close.await.unwrap().unwrap();
    assert_eq!(scheduler.running_count(&account_id).await, Some(0));
    let second_permit = scheduler
        .acquire(
            account_id,
            AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "next-generation".to_string(),
            },
        )
        .await;
    let second = open_window(&meter, second_permit).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), async {
            while reader.calls.load(Ordering::Acquire) < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err()
    );
    assert_eq!(reader.calls.load(Ordering::Acquire), 2);
    assert_eq!(reader.maximum_active.load(Ordering::Acquire), 1);

    final_blocked.release();
    wait_for_calls(&reader, 3).await;
    close_window(second, now + Duration::from_secs(2))
        .await
        .unwrap();

    assert_eq!(entry.durable_byte_seconds_delta(), 0);
}

#[test]
#[timeout("5s")]
async fn closed_window_queued_baseline_is_discarded_before_observer_call() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let first_baseline = ObservationGate::pending(authoritative(100));
    let current_baseline = ObservationGate::ready(authoritative(200));
    let current_final = ObservationGate::ready(authoritative(200));
    let reader = ScriptedUsageReader::new(vec![
        Arc::clone(&first_baseline),
        current_baseline,
        current_final,
    ]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock.clone(), &entry, 0);
    let (scheduler, account_id, first_permit) = permit(&entry).await;
    let first = open_window(&meter, first_permit).await.unwrap();
    first_baseline.wait_started().await;

    let first_close = close_window(first, now + Duration::from_secs(1));
    clock.set(Duration::from_secs(1)).await;
    first_close.await.unwrap();

    let second_permit = scheduler
        .acquire(
            account_id,
            AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "queued-window".to_string(),
            },
        )
        .await;
    let second = open_window(&meter, second_permit).await.unwrap();
    wait_for_active_observation(&second).await;

    let second_close = close_window(second, now + Duration::from_secs(2));
    clock.set(Duration::from_secs(2)).await;
    second_close.await.unwrap();

    first_baseline.release();
    wait_for_observations_to_finish(&reader).await;

    let third_permit = scheduler
        .acquire(
            account_id,
            AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "current-window".to_string(),
            },
        )
        .await;
    let third = open_window(&meter, third_permit).await.unwrap();
    wait_for_active_observation(&third).await;
    wait_for_calls(&reader, 2).await;
    wait_for_observations_to_finish(&reader).await;
    wait_for_observation_state(&third).await;
    assert_eq!(reader.calls.load(Ordering::Acquire), 2);
    close_window(third, now + Duration::from_secs(3))
        .await
        .unwrap();

    assert_eq!(reader.calls.load(Ordering::Acquire), 3);
}

#[test]
#[timeout("5s")]
async fn close_settles_if_the_final_observer_loses_its_meter() {
    let now = Instant::now();
    let clock = TestClock::new(now);
    let baseline = ObservationGate::pending(authoritative(100));
    let reader = ScriptedUsageReader::new(vec![Arc::clone(&baseline)]);
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (meter, _) = meter(reader.clone(), clock, &entry, 0);
    let (_, _, permit) = permit(&entry).await;
    let window = open_window(&meter, permit).await.unwrap();
    baseline.wait_started().await;

    let close = tokio::spawn(close_window(window, now + Duration::from_secs(1)));
    drop(meter);
    baseline.release();

    close.await.unwrap().unwrap();
    assert_eq!(reader.calls.load(Ordering::Acquire), 1);
}

#[test]
async fn dropped_window_releases_permit() {
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let (account, _) = configured_account(&entry, 0, false, Instant::now());
    let reader = ScriptedUsageReader::new(vec![ObservationGate::ready(authoritative(1))]);
    let meter = create_configured_meter(
        ResourceUsageMeteringConfig {
            compute: false,
            memory: false,
            filesystem: true,
        },
        || FilesystemUsageSource::scripted(reader),
        account,
    );
    let (scheduler, account_id, permit) = permit(&entry).await;
    let held = Arc::new(AtomicBool::new(false));
    let permit = permit.track_held(Arc::clone(&held));
    let window = open_window(&meter, permit).await.unwrap();

    drop(window);
    tokio::time::timeout(Duration::from_secs(1), async {
        while scheduler.running_count(&account_id).await != Some(0) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!held.load(Ordering::Acquire));
}
