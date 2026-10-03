use super::*;
use crate::model::ExecutionStatus;
use crate::services::resource_limits::MonthlyResourceExhaustion;
use crate::worker::instance::OwnerRuntimeResources;
use crate::worker::{MonthlyResourceAdmission, monthly_resource_admission};
use std::sync::RwLock;
use test_r::test;

fn storage_policy(period: AccountUsagePeriod) -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period,
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: 126,
        available_durable_storage_byte_nanoseconds_remainder: 250_000_000,
        available_ephemeral_storage_byte_seconds: 126,
        available_ephemeral_storage_byte_nanoseconds_remainder: 250_000_000,
    }
}

fn storage_entry(policy: MonthlyResourcePolicy) -> Arc<AtomicResourceEntry> {
    Arc::new(AtomicResourceEntry::new_with_monthly_policy(
        policy, 0, 0, 0, 2,
    ))
}

fn owner(entry: &Arc<AtomicResourceEntry>, mode: AgentMode) -> OwnerRuntimeResources {
    OwnerRuntimeResources::new(
        entry.clone(),
        Arc::new(RwLock::new(ExecutionStatus::loading(mode))),
    )
}

fn storage_meter(
    reader: Arc<ScriptedUsageReader>,
    clock: Arc<TestClock>,
    entry: &Arc<AtomicResourceEntry>,
    mode: AgentMode,
) -> ResourceUsageMeter {
    let (account, _) = configured_account_at_mode(entry, 0, false, mode, clock.time());
    create_configured_meter_with_clock(
        ResourceUsageMeteringConfig {
            compute: false,
            memory: false,
            filesystem: true,
        },
        || FilesystemUsageSource::scripted(reader),
        account,
        clock,
    )
}

fn storage_fields(
    update: &golem_service_base::clients::registry::ResourceUsageUpdate,
    mode: AgentMode,
) -> (i64, u64, i64, u64) {
    if mode == AgentMode::Durable {
        (
            update.durable_storage_byte_seconds_delta,
            update.durable_storage_byte_nanoseconds_remainder,
            update.ephemeral_storage_byte_seconds_delta,
            update.ephemeral_storage_byte_nanoseconds_remainder,
        )
    } else {
        (
            update.ephemeral_storage_byte_seconds_delta,
            update.ephemeral_storage_byte_nanoseconds_remainder,
            update.durable_storage_byte_seconds_delta,
            update.durable_storage_byte_nanoseconds_remainder,
        )
    }
}

#[test]
#[timeout("5s")]
async fn monthly_owner_scripted_storage_is_exact_mode_specific_and_recovers() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let clock = TestClock::new_at(Instant::now(), Utc::now());
        let mut policy = storage_policy(AccountUsagePeriod::current());
        policy.available_fuel = 100;
        // A prepaid Store must not shield storage exhaustion from the owner monitor.
        let entry = storage_entry(policy.clone());
        let owner = owner(&entry, mode);
        let prepaid = owner.register_store_fuel_reservation();
        prepaid.update(|| {
            let crate::services::resource_limits::FuelBorrow::Borrowed { generation, .. } =
                entry.borrow_fuel_with_revision(100)
            else {
                panic!("fuel available")
            };
            ((), Some(generation))
        });
        let reader = ScriptedUsageReader::new(
            (0..4)
                .map(|_| ObservationGate::ready(authoritative(101)))
                .collect(),
        );
        let meter = storage_meter(reader.clone(), clock.clone(), &entry, mode);
        let (_, _, target_permit) = permit(&entry).await;
        let mut window = open_window(&meter, target_permit).await.unwrap();
        let held = Arc::new(AtomicBool::new(false));
        window.track_permit_for_test(held.clone());
        assert!(held.load(Ordering::Acquire));
        owner.register_resource_usage_flusher(window.usage_flusher().unwrap());
        wait_for_calls(&reader, 1).await;
        wait_for_observation_state(&window).await;
        clock.wait_for_sleep_until(Duration::from_millis(10)).await;

        clock.set(Duration::from_millis(750)).await;
        wait_for_calls(&reader, 2).await;
        wait_for_observation_state(&window).await;
        clock.wait_for_sleep_until(Duration::from_millis(800)).await;
        clock.set(Duration::from_millis(1250)).await;
        wait_for_calls(&reader, 3).await;
        wait_for_observation_state(&window).await;
        owner.settle_resource_usage();
        let sample = || owner.with_monthly_capacity(mode, |capacity| capacity);
        let exhaustion = if mode == AgentMode::Durable {
            MonthlyResourceExhaustion::DurableStorage
        } else {
            MonthlyResourceExhaustion::EphemeralStorage
        };
        assert_eq!(sample().exhaustion, Some(exhaustion));
        assert_eq!(sample().fuel_generation, Some(0));
        let other = if mode == AgentMode::Durable {
            AgentMode::Ephemeral
        } else {
            AgentMode::Durable
        };
        assert_eq!(
            owner.with_monthly_capacity(other, |capacity| capacity.exhaustion),
            None
        );
        let captured = entry.capture_usage_update_for_test();
        assert_eq!(storage_fields(&captured, mode), (126, 0, 0, 0));
        assert_eq!(captured.monthly_policy_revision, 0);
        assert_eq!(
            sample().exhaustion,
            Some(exhaustion),
            "in-flight storage must remain visible"
        );
        // Capturing whole units retains the quarter byte-second until a revision boundary.
        policy.available_fuel = u64::MAX;
        policy.available_durable_storage_byte_seconds = 1000;
        policy.available_ephemeral_storage_byte_seconds = 1000;
        assert!(entry.apply_monthly_snapshot_for_test(1, policy.clone(), 1));
        assert_eq!(sample().policy_revision, 1);
        assert_eq!(sample().exhaustion, None);
        let remainder = entry.capture_usage_update_for_test();
        assert_eq!(storage_fields(&remainder, mode), (0, 250_000_000, 0, 0));
        assert_eq!(remainder.monthly_policy_revision, 0);
        assert!(!entry.apply_monthly_snapshot_for_test(2, storage_policy(policy.period), 0));
        assert_eq!(sample().policy_revision, 1);
        assert_eq!(sample().exhaustion, None);
        assert!(entry.apply_monthly_snapshot_for_test(3, storage_policy(policy.period), 2));
        assert_eq!(sample().exhaustion, Some(exhaustion));
        policy.mode = MonthlyUsageMode::AllowOverage;
        policy.available_fuel = 0;
        policy.available_durable_storage_byte_seconds = 0;
        policy.available_ephemeral_storage_byte_seconds = 0;
        policy.available_durable_storage_byte_nanoseconds_remainder = 0;
        policy.available_ephemeral_storage_byte_nanoseconds_remainder = 0;
        assert!(entry.apply_monthly_snapshot_for_test(4, policy, 3));
        assert_eq!(sample().policy_revision, 3);
        assert_eq!(sample().exhaustion, None);
        window.freeze_allocation().await.unwrap();
        clock.set(Duration::from_millis(2250)).await;
        owner.settle_resource_usage();
        let overage = entry.capture_usage_update_for_test();
        assert_eq!(storage_fields(&overage, mode), (101, 0, 0, 0));
        assert_eq!(overage.monthly_policy_revision, 3);
        assert_eq!(overage.monthly_usage_mode_revision, 3);
        assert!(entry.apply_monthly_snapshot_for_test(
            5,
            storage_policy(AccountUsagePeriod::current()),
            4
        ));
        assert_eq!(sample().exhaustion, Some(exhaustion));
        // Close without any later time advance, then prove the same owner can no longer accrue.
        close_window(window, clock.now() + Duration::from_secs(1))
            .await
            .unwrap();
        assert!(!held.load(Ordering::Acquire));
        assert!(!meter.is_active());
        clock.set(Duration::from_secs(5)).await;
        owner.settle_resource_usage();
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(entry.ephemeral_byte_seconds_delta(), 0);
    }
}

#[test]
#[timeout("5s")]
async fn monthly_owner_flush_does_not_settle_sibling_allocations_on_the_same_account() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let clock = TestClock::new_at(Instant::now(), Utc::now());
        let mut policy = storage_policy(AccountUsagePeriod::current());
        policy.available_durable_storage_byte_seconds = 12_626;
        policy.available_ephemeral_storage_byte_seconds = 12_626;
        policy.available_durable_storage_byte_nanoseconds_remainder = 0;
        policy.available_ephemeral_storage_byte_nanoseconds_remainder = 0;
        let entry = storage_entry(policy);
        let target_owner = owner(&entry, mode);
        let sibling_owner = owner(&entry, mode);
        let mut windows = Vec::new();
        let mut meters = Vec::new();
        for (owner, bytes) in [(&target_owner, 101), (&sibling_owner, 10_000)] {
            let reader = ScriptedUsageReader::new(vec![
                ObservationGate::ready(authoritative(bytes)),
                ObservationGate::ready(authoritative(bytes)),
            ]);
            let meter = storage_meter(reader.clone(), clock.clone(), &entry, mode);
            let (_, _, permit) = permit(&entry).await;
            let mut window = open_window(&meter, permit).await.unwrap();
            owner.register_resource_usage_flusher(window.usage_flusher().unwrap());
            wait_for_calls(&reader, 1).await;
            wait_for_observation_state(&window).await;
            window.freeze_allocation().await.unwrap();
            windows.push(window);
            meters.push(meter);
        }
        clock.set(Duration::from_millis(1250)).await;
        target_owner.settle_resource_usage();
        assert_eq!(
            target_owner.with_monthly_capacity(mode, |capacity| capacity.exhaustion),
            None
        );
        let deltas = || {
            (
                entry.durable_byte_seconds_delta(),
                entry.ephemeral_byte_seconds_delta(),
            )
        };
        assert_eq!(
            deltas(),
            if mode == AgentMode::Durable {
                (126, 0)
            } else {
                (0, 126)
            }
        );
        target_owner.settle_resource_usage();
        assert_eq!(
            deltas(),
            if mode == AgentMode::Durable {
                (126, 0)
            } else {
                (0, 126)
            }
        );
        sibling_owner.settle_resource_usage();
        assert_eq!(
            target_owner.with_monthly_capacity(mode, |capacity| capacity.exhaustion),
            Some(if mode == AgentMode::Durable {
                MonthlyResourceExhaustion::DurableStorage
            } else {
                MonthlyResourceExhaustion::EphemeralStorage
            })
        );
        assert_eq!(
            deltas(),
            if mode == AgentMode::Durable {
                (12_626, 0)
            } else {
                (0, 12_626)
            }
        );
        for window in windows {
            close_window(window, clock.now() + Duration::from_secs(1))
                .await
                .unwrap();
        }
        clock.set(Duration::from_secs(5)).await;
        target_owner.settle_resource_usage();
        sibling_owner.settle_resource_usage();
        assert_eq!(
            deltas(),
            if mode == AgentMode::Durable {
                (12_626, 0)
            } else {
                (0, 12_626)
            }
        );
        drop(meters);
    }
}

#[test]
#[timeout("5s")]
async fn monthly_scripted_storage_unavailable_observations_do_not_fabricate_exhaustion() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for unsupported in [true, false] {
            let clock = TestClock::new_at(Instant::now(), Utc::now());
            let mut policy = storage_policy(AccountUsagePeriod::current());
            policy.available_fuel = u64::MAX;
            let entry = storage_entry(policy);
            let reader = ScriptedUsageReader::new(vec![
                ObservationGate::ready(if unsupported {
                    Ok(FilesystemUsage::Unsupported)
                } else {
                    Err(observation_error())
                }),
                ObservationGate::ready(Ok(FilesystemUsage::Unsupported)),
            ]);
            let meter = storage_meter(reader.clone(), clock.clone(), &entry, mode);
            let (_, _, permit) = permit(&entry).await;
            let window = open_window(&meter, permit).await.unwrap();
            wait_for_calls(&reader, 1).await;
            wait_for_observation_state(&window).await;
            assert_eq!(
                window
                    .shared
                    .as_ref()
                    .unwrap()
                    .state
                    .lock()
                    .unwrap()
                    .storage
                    .is_none(),
                unsupported
            );
            stop_periodic_sampling(&window);
            clock.set(Duration::from_secs(1)).await;
            meter.flush(clock.now());
            assert_eq!(entry.monthly_resource_capacity(mode), Ok(()));
            assert_eq!(entry.durable_byte_seconds_delta(), 0);
            assert_eq!(entry.ephemeral_byte_seconds_delta(), 0);
            close_window(window, clock.now() + Duration::from_secs(1))
                .await
                .unwrap();
        }
    }
}

#[test]
#[timeout("5s")]
async fn monthly_scripted_storage_rollover_preserves_mode_and_recovers_capacity() {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        let january = AccountUsagePeriod {
            year: 2030,
            month: 1,
        };
        let february = AccountUsagePeriod {
            year: 2030,
            month: 2,
        };
        let clock = TestClock::new_at(
            Instant::now(),
            Utc.with_ymd_and_hms(2030, 1, 31, 23, 59, 59).unwrap()
                + chrono::Duration::milliseconds(500),
        );
        let mut policy = storage_policy(january);
        policy.available_fuel = u64::MAX;
        policy.available_durable_storage_byte_seconds = 50;
        policy.available_ephemeral_storage_byte_seconds = 50;
        policy.available_durable_storage_byte_nanoseconds_remainder = 0;
        policy.available_ephemeral_storage_byte_nanoseconds_remainder = 0;
        let entry = storage_entry(policy.clone());
        let reader = ScriptedUsageReader::new(vec![
            ObservationGate::ready(authoritative(100)),
            ObservationGate::ready(authoritative(100)),
        ]);
        let meter = storage_meter(reader.clone(), clock.clone(), &entry, mode);
        let (_, _, permit) = permit(&entry).await;
        let mut window = open_window(&meter, permit).await.unwrap();
        wait_for_calls(&reader, 1).await;
        wait_for_observation_state(&window).await;
        window.freeze_allocation().await.unwrap();
        clock.set(Duration::from_millis(500)).await;
        meter.flush(clock.now());
        let exhausted = if mode == AgentMode::Durable {
            MonthlyResourceExhaustion::DurableStorage
        } else {
            MonthlyResourceExhaustion::EphemeralStorage
        };
        assert_eq!(
            entry.monthly_memory_and_storage_capacity(mode),
            Err(exhausted)
        );
        let old_policy = policy.clone();
        policy.period = february;
        policy.available_durable_storage_byte_seconds = 100;
        policy.available_ephemeral_storage_byte_seconds = 100;
        assert!(entry.apply_monthly_snapshot_for_test(1, policy, 1));
        assert_eq!(entry.monthly_memory_and_storage_capacity(mode), Ok(()));
        assert!(!entry.apply_monthly_snapshot_for_test(2, old_policy, 0));
        clock.set(Duration::from_secs(1)).await;
        close_window(window, clock.now() + Duration::from_secs(1))
            .await
            .unwrap();
        let old = entry.capture_usage_update_at_for_test(february);
        assert_eq!(old.period, january);
        assert_eq!(old.monthly_policy_revision, 0);
        assert_eq!(storage_fields(&old, mode), (50, 0, 0, 0));
        let current = entry.capture_usage_update_at_for_test(february);
        assert_eq!(current.period, february);
        assert_eq!(current.monthly_policy_revision, 1);
        assert_eq!(storage_fields(&current, mode), (50, 0, 0, 0));
        assert_eq!(
            monthly_resource_admission(&entry, mode),
            MonthlyResourceAdmission::Admit
        );
    }
}
