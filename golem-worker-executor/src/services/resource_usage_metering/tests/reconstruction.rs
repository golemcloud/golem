use super::*;
use test_r::test;

fn current_clock() -> Arc<TestClock> {
    let period = AccountUsagePeriod::current();
    TestClock::new_at(
        Instant::now(),
        Utc.with_ymd_and_hms(period.year, period.month, 15, 12, 0, 0)
            .single()
            .unwrap(),
    )
}

fn unlimited_policy() -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}

fn replay_memory(
    entry: &Arc<AtomicResourceEntry>,
    bytes: u64,
    clock: &TestClock,
) -> LinearMemoryTracker {
    LinearMemoryTracker::new_at(
        bytes,
        bytes,
        AgentMode::Durable,
        true,
        entry.clone(),
        Arc::new(Mutex::new(MemoryGrant::inert(bytes))),
        clock.time(),
    )
}

async fn accept_allocation(window: &ResourceUsageMeteringWindow) {
    let shared = window.shared.as_ref().unwrap();
    let observation = shared.start_observation(WindowStatus::Closing).unwrap();
    let result = observation.receiver.await.unwrap();
    assert!(shared.finish_observation(observation.active, result, shared.clock.time(), true));
}

#[test]
#[timeout("5s")]
async fn identical_live_byte_time_excludes_reconstruction_and_preserves_blob_stock() {
    let mut charges = Vec::new();
    for reconstruct in [false, true] {
        let clock = current_clock();
        let entry = Arc::new(AtomicResourceEntry::new_with_blob_storage_limit(100));
        assert!(entry.try_record_blob_storage_delta(40));
        let memory = replay_memory(&entry, GIB, &clock);
        let reader = ScriptedUsageReader::new(vec![
            ObservationGate::ready(authoritative(100)),
            ObservationGate::ready(authoritative(100)),
            ObservationGate::ready(authoritative(100)),
        ]);
        let meter = create_configured_meter_with_clock(
            ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: true,
            },
            || FilesystemUsageSource::scripted(reader.clone()),
            ResourceUsageAccount::new(AgentMode::Durable, memory.clone(), entry.clone()),
            clock.clone(),
        );
        let (_, _, permit) = permit(&entry).await;
        let mut window = open_window(&meter, permit).await.unwrap();
        wait_for_calls(&reader, 1).await;
        wait_for_observations_to_finish(&reader).await;
        wait_for_observation_state(&window).await;
        stop_periodic_sampling(&window);
        let offset = if reconstruct {
            Duration::from_secs(3)
        } else {
            Duration::ZERO
        };
        clock.set(offset).await;
        accept_allocation(&window).await;
        let _body = memory.begin_guest_body();
        memory.activate_live_usage_if_guest_body();
        // Freeze the known live stock exactly as ordinary cleanup does. The live allocation
        // must remain charged through the remaining permit-held cleanup interval.
        window.freeze_allocation().await.unwrap();
        clock.set(offset + Duration::from_millis(1250)).await;
        close_window(window, clock.now() + Duration::from_secs(1))
            .await
            .unwrap();
        let update = entry.capture_usage_update_for_test();
        assert_eq!(update.blob_storage_bytes_delta, 40);
        assert!(entry.apply_monthly_snapshot_for_test(1, unlimited_policy(), 1));
        let fraction = entry.capture_usage_update_for_test();
        assert_eq!(fraction.monthly_policy_revision, 0);
        assert_eq!(fraction.memory_gb_seconds_delta, 0);
        assert_eq!(fraction.blob_storage_bytes_delta, 0);
        charges.push((
            update.memory_gb_seconds_delta,
            update.memory_byte_nanoseconds_remainder + fraction.memory_byte_nanoseconds_remainder,
            update.durable_storage_byte_seconds_delta,
            update.durable_storage_byte_nanoseconds_remainder,
        ));
    }
    let expected = (1, (BYTE_NANOSECONDS_PER_GB_SECOND / 4) as u64, 125, 0);
    assert_eq!(charges, vec![expected, expected]);
}

#[test]
#[timeout("5s")]
async fn local_live_entity_memory_does_not_bill_primary_replay_or_incapable_filesystem() {
    for filesystem_capable in [false, true] {
        let clock = current_clock();
        let entry = Arc::new(AtomicResourceEntry::new(0, usize::MAX, 0, 0, 1));
        let primary = replay_memory(&entry, GIB, &clock);
        let entity = replay_memory(&entry, GIB / 4, &clock);
        let reader = ScriptedUsageReader::new(vec![
            ObservationGate::ready(authoritative(100)),
            ObservationGate::ready(authoritative(100)),
        ]);
        let meter = create_configured_meter_with_clock(
            ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: true,
            },
            || FilesystemUsageSource::scripted(reader.clone()),
            ResourceUsageAccount::new(AgentMode::Durable, primary.clone(), entry.clone()),
            clock.clone(),
        );
        entity.set_live_activation(
            meter
                .live_activation()
                .unwrap()
                .with_filesystem(filesystem_capable),
        );
        let (_, _, permit) = permit(&entry).await;
        let window = open_window(&meter, permit).await.unwrap();
        wait_for_calls(&reader, 1).await;
        wait_for_observations_to_finish(&reader).await;
        wait_for_observation_state(&window).await;
        stop_periodic_sampling(&window);
        let body = entity.begin_guest_body();
        entity.switch_to_live();
        assert!(
            !entity.usage_is_live(),
            "physical admission alone cannot start billing"
        );
        entity.activate_live_usage_if_guest_body();
        assert!(!primary.usage_is_live());
        clock.set(Duration::from_secs(1)).await;
        entity.meter().flush_at(clock.time());
        drop(body);
        drop(entity);
        close_window(window, clock.now() + Duration::from_secs(1))
            .await
            .unwrap();
        let whole = entry.capture_pending_usage_for_test();
        assert_eq!(
            whole.map(|update| update.durable_storage_byte_seconds_delta),
            filesystem_capable.then_some(100)
        );
        assert!(entry.apply_monthly_snapshot_for_test(1, unlimited_policy(), 1));
        let update = entry.capture_usage_update_for_test();
        assert_eq!(update.monthly_policy_revision, 0);
        assert_eq!(update.memory_gb_seconds_delta, 0);
        assert_eq!(
            update.memory_byte_nanoseconds_remainder,
            (BYTE_NANOSECONDS_PER_GB_SECOND / 4) as u64
        );
        assert_eq!(update.durable_storage_byte_seconds_delta, 0);
        assert_eq!(update.durable_storage_byte_nanoseconds_remainder, 0);
        // A late activation must not reopen this owner's closed accounting window.
        let _late_body = primary.begin_guest_body();
        primary.activate_live_usage_if_guest_body();
        clock.set(Duration::from_secs(2)).await;
        primary.meter().flush_at(clock.time());
        assert!(entry.capture_pending_usage_for_test().is_none());
        assert!(entry.apply_monthly_snapshot_for_test(2, unlimited_policy(), 2));
        assert!(entry.capture_pending_usage_for_test().is_none());
    }
}

#[test]
#[timeout("5s")]
async fn reconstruction_window_keeps_permit_without_memory_or_filesystem_charge() {
    use futures::FutureExt;

    let mut charges = Vec::new();
    for (memory_enabled, filesystem_enabled) in [(true, false), (false, true)] {
        let period = AccountUsagePeriod::current();
        let clock = TestClock::new_at(
            Instant::now(),
            Utc.with_ymd_and_hms(period.year, period.month, 15, 12, 0, 0)
                .single()
                .unwrap(),
        );
        let entry = Arc::new(AtomicResourceEntry::new(0, usize::MAX, 0, 0, 1));
        let memory = LinearMemoryTracker::new_at(
            GIB,
            GIB,
            AgentMode::Durable,
            true,
            entry.clone(),
            Arc::new(Mutex::new(MemoryGrant::inert(GIB))),
            clock.time(),
        );
        let first = ObservationGate::ready(authoritative(100));
        let final_read = ObservationGate::ready(authoritative(100));
        let reader = ScriptedUsageReader::new(vec![first, final_read]);
        let meter = create_configured_meter_with_clock(
            ResourceUsageMeteringConfig {
                compute: false,
                memory: memory_enabled,
                filesystem: filesystem_enabled,
            },
            || FilesystemUsageSource::scripted(reader.clone()),
            ResourceUsageAccount::new(AgentMode::Durable, memory, entry.clone()),
            clock.clone(),
        );
        let (scheduler, account, permit) = permit(&entry).await;
        let window = open_window(&meter, permit).await.unwrap();
        if filesystem_enabled {
            wait_for_calls(&reader, 1).await;
            wait_for_observations_to_finish(&reader).await;
            wait_for_observation_state(&window).await;
        }
        // Close the sampler, not physical ownership, before advancing its injected clock.
        stop_periodic_sampling(&window);
        let blocked = scheduler.acquire(
            account,
            AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "blocked-during-reconstruction".into(),
            },
        );
        tokio::pin!(blocked);
        assert!(blocked.as_mut().now_or_never().is_none());
        clock.set(Duration::from_secs(1)).await;
        meter.flush(clock.now());
        close_window(window, clock.now() + Duration::from_secs(1))
            .await
            .unwrap();
        let update = entry.capture_pending_usage_for_test();
        charges.push(update.map(|update| {
            (
                update.memory_gb_seconds_delta,
                update.memory_byte_nanoseconds_remainder,
                update.durable_storage_byte_seconds_delta,
                update.durable_storage_byte_nanoseconds_remainder,
            )
        }));
        assert!(entry.apply_monthly_snapshot_for_test(1, unlimited_policy(), 1));
        assert!(
            entry.capture_pending_usage_for_test().is_none(),
            "reconstruction must not retain a fractional charge"
        );
        drop(blocked.await);
    }
    assert_eq!(
        charges,
        vec![None, None],
        "physical reconstruction allocations must add no memory or filesystem charge"
    );
}

#[test]
fn replay_memory_tracking_still_enforces_the_per_agent_limit() {
    let now = MeteringTime::now();
    let entry = Arc::new(AtomicResourceEntry::new(0, GIB as usize, 0, 0, 1));
    let memory = LinearMemoryTracker::new_at(
        GIB,
        GIB,
        AgentMode::Durable,
        true,
        entry,
        Arc::new(Mutex::new(MemoryGrant::inert(GIB))),
        now,
    );
    let stopped = Arc::new(AtomicBool::new(false));
    let observed = stopped.clone();
    memory.set_limit_exceeded_callback(Arc::new(move || observed.store(true, Ordering::Release)));
    assert!(!stopped.load(Ordering::Acquire));
    memory.grow(1, now.instant);
    assert_eq!(memory.current_bytes(), GIB + 1);
    assert!(memory.exceeds_current_limit());
    assert!(stopped.load(Ordering::Acquire));
}
