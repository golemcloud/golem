use super::*;
use crate::services::resource_limits::{FuelBorrow, MonthlyResourceExhaustion};
use golem_common::model::account_usage::{AccountUsagePeriod, MonthlyUsageMode};
use golem_common::model::agent::AgentMode;
use golem_service_base::model::MonthlyResourcePolicy;
use std::sync::{Barrier, RwLock};
use test_r::test;

fn policy(fuel: u64) -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel: fuel,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}

fn owner() -> Arc<OwnerRuntimeResources> {
    Arc::new(OwnerRuntimeResources::new(
        Arc::new(AtomicResourceEntry::new_with_monthly_policy(
            policy(100),
            0,
            0,
            0,
            1,
        )),
        Arc::new(RwLock::new(ExecutionStatus::loading(AgentMode::Durable))),
    ))
}

#[test]
fn monthly_owner_uses_oldest_live_store_generation_and_unreserved_store() {
    let owner = owner();
    let entry = owner.resource_limits();
    let older = owner.register_store_fuel_reservation();
    older.update(|| {
        let FuelBorrow::Borrowed {
            generation, amount, ..
        } = entry.borrow_fuel_with_revision(100)
        else {
            panic!("fuel available")
        };
        assert_eq!(amount, 100);
        ((), Some(generation))
    });
    let sample = || owner.with_monthly_capacity(AgentMode::Durable, |capacity| capacity);
    assert_eq!(sample().fuel_generation, Some(0));
    assert_eq!(sample().exhaustion, None);
    assert!(entry.apply_monthly_snapshot_for_test(1, policy(0), 1));
    assert_eq!(sample().policy_revision, 1);
    assert_eq!(
        sample().exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    let newer = owner.register_store_fuel_reservation();
    newer.update(|| ((), Some(1)));
    assert_eq!(sample().fuel_generation, Some(0));
    assert_eq!(
        sample().exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    drop(older);
    assert_eq!(sample().fuel_generation, Some(1));
    assert_eq!(sample().exhaustion, None);
    let unreserved = owner.register_store_fuel_reservation();
    assert_eq!(sample().fuel_generation, None);
    assert_eq!(
        sample().exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    drop(unreserved);
    assert_eq!(sample().fuel_generation, Some(1));
    assert_eq!(sample().exhaustion, None);
    drop(newer);
    assert_eq!(sample().fuel_generation, None);
    assert_eq!(
        sample().exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
}

#[test]
fn monthly_owner_borrow_and_generation_publication_are_one_sampling_transaction() {
    let owner = owner();
    let reservation = owner.register_store_fuel_reservation();
    let racing = Arc::new(Barrier::new(2));
    let observer = std::thread::spawn({
        let owner = owner.clone();
        let racing = racing.clone();
        move || {
            racing.wait();
            owner.with_monthly_capacity(AgentMode::Durable, |capacity| capacity)
        }
    });
    reservation.update(|| {
        let FuelBorrow::Borrowed { generation, .. } =
            owner.resource_limits().borrow_fuel_with_revision(100)
        else {
            panic!("fuel available")
        };
        assert!(
            owner.fuel_transaction.try_lock().is_err(),
            "borrowing and publication must exclude sampling"
        );
        racing.wait();
        ((), Some(generation))
    });
    let sample = observer.join().unwrap();
    assert_eq!(sample.fuel_generation, Some(0));
    assert_eq!(
        sample.exhaustion, None,
        "sampling must not see the debit without its prepaid generation"
    );
    owner
        .resource_limits()
        .apply_monthly_snapshot_for_test(1, policy(0), 1);
    let expired = owner.with_monthly_capacity(AgentMode::Durable, |capacity| capacity);
    assert_eq!(expired.fuel_generation, sample.fuel_generation);
    assert_eq!(expired.exhaustion, Some(MonthlyResourceExhaustion::Compute));
}
