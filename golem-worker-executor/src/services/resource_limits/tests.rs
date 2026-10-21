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

use super::*;

pub(crate) fn assert_capacity_observers(entry: &AtomicResourceEntry, expected: usize) {
    assert_eq!(entry.capacity_updates.receiver_count(), expected);
}

#[test]
fn unavailable_policy_acknowledges_usage_without_installing_fake_capacity() {
    use golem_service_base::model::MonthlyResourcePolicyStatus;
    let period = AccountUsagePeriod::current();
    for mode in [MonthlyUsageMode::HardLimit, MonthlyUsageMode::AllowOverage] {
        for metering in [
            ResourceUsageMeteringConfig {
                compute: true,
                memory: false,
                filesystem: false,
            },
            ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: false,
            },
            ResourceUsageMeteringConfig {
                compute: false,
                memory: false,
                filesystem: true,
            },
        ] {
            let mut policy = memory_policy(period, mode, 50);
            policy.available_fuel = 50;
            policy.available_durable_storage_byte_seconds = 50;
            policy.available_ephemeral_storage_byte_seconds = 50;
            let entry = AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
                policy.clone(),
                4096,
                8192,
                16384,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                32,
                u64::MAX,
                metering,
                7,
                7,
                0,
            );
            if metering.compute {
                assert!(entry.borrow_fuel(1));
            }
            if metering.memory {
                entry.record_memory_gb_seconds(AgentMode::Durable, 1);
            }
            if metering.filesystem {
                entry.record_storage_byte_seconds(AgentMode::Durable, 1);
                entry.record_storage_byte_seconds(AgentMode::Ephemeral, 1);
            }
            let captured = entry.capture_usage_update(0).unwrap();
            entry.begin_monthly_refresh(
                1,
                &captured.update,
                captured.durable_memory_gb_seconds_delta,
                captured.ephemeral_memory_gb_seconds_delta,
            );
            let mut fake_policy = policy.clone();
            fake_policy.mode = MonthlyUsageMode::HardLimit;
            fake_policy.available_fuel = 0;
            fake_policy.available_memory_gb_seconds = 0;
            assert!(entry.apply_monthly_snapshot_with_policy_revision(
                1,
                fake_policy,
                9,
                9,
                MonthlyResourcePolicyStatus::Unavailable,
                true,
            ));
            for agent_mode in [AgentMode::Durable, AgentMode::Ephemeral] {
                assert_eq!(
                    entry.monthly_resource_capacity(agent_mode),
                    Err(MonthlyResourceCapacityError::Unavailable)
                );
                entry.with_monthly_capacity(agent_mode, None, |capacity| {
                    assert_eq!(
                        capacity.policy_status,
                        MonthlyResourcePolicyStatus::Unavailable
                    );
                    assert_eq!(capacity.exhaustion, None);
                    assert_eq!(capacity.policy_revision, 7);
                });
            }
            let retry = entry
                .capture_usage_update(i64::MAX)
                .expect("unavailable policy uses the next existing batch tick");
            assert_eq!(retry.update.fuel_delta, 0);
            assert_eq!(retry.update.memory_gb_seconds_delta, 0);
            assert_eq!(retry.update.durable_storage_byte_seconds_delta, 0);
            assert_eq!(retry.update.ephemeral_storage_byte_seconds_delta, 0);
            assert_eq!(entry.max_memory_limit(), 4096);
            assert_eq!(entry.max_disk_space_limit(), 16384);
            entry.begin_monthly_refresh(2, &retry.update, 0, 0);
            policy.available_fuel = 49;
            policy.available_memory_gb_seconds = 49;
            policy.available_durable_storage_byte_seconds = 49;
            policy.available_ephemeral_storage_byte_seconds = 49;
            assert!(entry.apply_monthly_snapshot_with_policy_revision(
                2,
                policy,
                9,
                9,
                MonthlyResourcePolicyStatus::Available,
                true,
            ));
            for agent_mode in [AgentMode::Durable, AgentMode::Ephemeral] {
                assert!(entry.monthly_resource_capacity(agent_mode).is_ok());
            }
            assert_eq!(
                entry.effective_memory_gb_seconds(),
                if metering.memory { 49 } else { u64::MAX }
            );
        }
    }
}

#[test]
fn policy_availability_rejects_stale_generations_and_is_inactive_when_all_meters_are_off() {
    use golem_service_base::model::MonthlyResourcePolicyStatus;
    let period = AccountUsagePeriod::current();
    let entry = memory_entry(period, MonthlyUsageMode::AllowOverage, 50);
    let policy = memory_policy(period, MonthlyUsageMode::AllowOverage, 50);
    entry.begin_monthly_refresh_with_memory_for_test(1, 0, 0);
    entry.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
    assert!(!entry.apply_monthly_snapshot_with_policy_revision(
        1,
        policy.clone(),
        7,
        7,
        MonthlyResourcePolicyStatus::Unavailable,
        true
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    assert!(entry.apply_monthly_snapshot_with_policy_revision(
        2,
        policy.clone(),
        7,
        7,
        MonthlyResourcePolicyStatus::Unavailable,
        true
    ));
    assert!(!entry.apply_monthly_snapshot_with_policy_revision(
        1,
        policy.clone(),
        7,
        7,
        MonthlyResourcePolicyStatus::Available,
        true
    ));
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceCapacityError::Unavailable)
    );
    entry.begin_monthly_refresh_with_memory_for_test(3, 0, 0);
    assert!(entry.apply_monthly_snapshot_with_policy_revision(
        3,
        policy.clone(),
        9,
        9,
        MonthlyResourcePolicyStatus::Unavailable,
        true
    ));
    entry.begin_monthly_refresh_with_memory_for_test(4, 0, 0);
    assert!(!entry.apply_monthly_snapshot_with_policy_revision(
        4,
        policy.clone(),
        8,
        8,
        MonthlyResourcePolicyStatus::Available,
        true
    ));
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceCapacityError::Unavailable)
    );
    let off = AtomicResourceEntry::new_with_all_limits_and_metering(
        0,
        4096,
        8192,
        16384,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        32,
        u64::MAX,
        ResourceUsageMeteringConfig::default(),
    );
    assert!(off.apply_monthly_snapshot_with_policy_revision(
        1,
        policy,
        7,
        7,
        MonthlyResourcePolicyStatus::Unavailable,
        true
    ));
    assert_eq!(off.monthly_resource_capacity(AgentMode::Durable), Ok(()));
    assert!(off.capture_usage_update(i64::MAX).is_none());
}
use crate::services::golem_config::GolemConfig;
use crate::span_test_support::{Tracing, get_tracing_dependency as test_r_get_dep_tracing};
use crate::worker::{MonthlyResourceAdmission, monthly_resource_admission};
use golem_common::model::AgentId;
use golem_common::model::agent::{AgentTypeName, RegisteredAgentType, ResolvedAgentType};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::custom_api::CompiledRoutes;
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use golem_service_base::model::environment::EnvironmentState;
use golem_service_base::model::{AccountResourceLimits, ResourceLimits as ServiceResourceLimits};
use std::collections::VecDeque;
use std::sync::Mutex;
use test_r::{test, timeout};
use tokio::sync::Semaphore;
use uuid::Uuid;

test_r::enable!();

#[test]
fn monthly_capacity_rechecks_same_revision_restoration_and_period() {
    let period = AccountUsagePeriod::current();
    let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 0);
    let read = || entry.with_monthly_capacity(AgentMode::Durable, None, |capacity| capacity);
    let exhausted = read();
    assert_eq!(
        exhausted.exhaustion,
        Some(MonthlyResourceExhaustion::Memory)
    );
    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        memory_policy(period, MonthlyUsageMode::HardLimit, 10),
        7
    ));
    let restored = read();
    assert_eq!(restored.policy_revision, exhausted.policy_revision);
    assert_ne!(restored, exhausted);
    assert_eq!(restored.exhaustion, None);

    let old = memory_entry(usage_period(2000, 1), MonthlyUsageMode::HardLimit, 0);
    let rolled = old.with_monthly_capacity(AgentMode::Durable, None, |capacity| capacity);
    assert_eq!(rolled.period, period);
    assert_eq!(rolled.exhaustion, None);
}

#[test]
fn monthly_snapshot_revision_predicate_control() {
    let period = AccountUsagePeriod::current();
    let policy = memory_policy(period, MonthlyUsageMode::HardLimit, 0);
    let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 0);
    let read = || entry.with_monthly_capacity(AgentMode::Durable, None, |capacity| capacity);
    let original = read();
    assert_eq!(original.exhaustion, Some(MonthlyResourceExhaustion::Memory));
    assert!(entry.apply_monthly_snapshot_for_test(1, policy.clone(), 7));
    assert_eq!(
        read(),
        original,
        "refresh generation alone is not proposal identity"
    );

    entry.begin_monthly_refresh_for_test(2, 0);
    assert!(entry.apply_monthly_snapshot_with_policy_revision(
        2,
        policy.clone(),
        7,
        8,
        golem_service_base::model::MonthlyResourcePolicyStatus::Available,
        true
    ));
    let revised = read();
    assert_eq!(revised.policy_revision, 8);
    assert_eq!(
        revised,
        MonthlyCapacity {
            policy_revision: 8,
            ..original
        }
    );
    assert_ne!(
        revised, original,
        "equal exhaustion does not erase revision identity"
    );

    entry.begin_monthly_refresh_for_test(3, 0);
    assert!(!entry.apply_monthly_snapshot_with_policy_revision(
        3,
        policy,
        7,
        7,
        golem_service_base::model::MonthlyResourcePolicyStatus::Available,
        true
    ));
    assert_eq!(
        read(),
        revised,
        "stale Registry revision cannot restore the proposal"
    );
}

#[test]
fn monthly_snapshot_period_predicate_control() {
    let period = AccountUsagePeriod::current();
    let next_period = if period.month == 12 {
        usage_period(period.year + 1, 1)
    } else {
        usage_period(period.year, period.month + 1)
    };
    let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 0);
    let read = || entry.with_monthly_capacity(AgentMode::Durable, None, |capacity| capacity);
    let original = read();
    assert_eq!(original.exhaustion, Some(MonthlyResourceExhaustion::Memory));
    // Inject a later account period rather than waiting for the wall clock to cross a month.
    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        memory_policy(next_period, MonthlyUsageMode::HardLimit, 0),
        7
    ));
    let rolled = read();
    assert_eq!(
        rolled,
        MonthlyCapacity {
            period: next_period,
            ..original
        }
    );
    assert_ne!(
        rolled, original,
        "equal exhaustion does not erase period identity"
    );
    assert!(entry.apply_monthly_snapshot_for_test(
        2,
        memory_policy(period, MonthlyUsageMode::HardLimit, 0),
        7
    ));
    let stale_period = read();
    assert_eq!(
        stale_period.period, next_period,
        "account period never moves backward"
    );
    assert_eq!(
        stale_period.exhaustion, None,
        "a mismatched policy period cannot stop a worker"
    );
}

#[test]
fn monthly_snapshot_fuel_generation_predicate_control() {
    let entry = compute_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        0,
    );
    let read = |generation| {
        entry.with_monthly_capacity(AgentMode::Durable, generation, |capacity| capacity)
    };
    let unreserved = read(None);
    assert_eq!(
        unreserved.exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    assert_eq!(read(Some(0)).exhaustion, None);
    assert!(entry.apply_monthly_snapshot_for_test(1, monthly_policy(0), 7));
    assert_eq!(read(None), unreserved);
    let expired = read(Some(0));
    assert_eq!(
        expired,
        MonthlyCapacity {
            fuel_generation: Some(0),
            ..unreserved
        }
    );
    assert_ne!(
        expired, unreserved,
        "even exhausted snapshots require exact reservation identity"
    );
    assert_eq!(
        read(Some(1)).exhaustion,
        None,
        "equal settled generation remains prepaid"
    );
    assert_eq!(
        read(Some(2)).exhaustion,
        None,
        "newer reservation remains prepaid"
    );
    assert!(entry.apply_monthly_snapshot_for_test(2, monthly_policy(0), 7));
    assert_eq!(
        read(Some(1)).exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    assert_eq!(read(Some(2)).exhaustion, None);
}

#[test]
fn monthly_capacity_updates_signal_only_applied_snapshots() {
    let period = AccountUsagePeriod::current();
    let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 0);
    let mut updates = entry.subscribe_capacity_updates();
    assert!(!updates.has_changed().unwrap());
    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        memory_policy(period, MonthlyUsageMode::HardLimit, 10),
        7
    ));
    assert!(updates.has_changed().unwrap());
    updates.borrow_and_update();
    assert!(!entry.apply_monthly_snapshot_for_test(
        2,
        memory_policy(period, MonthlyUsageMode::HardLimit, 0),
        6
    ));
    assert!(!updates.has_changed().unwrap());
    assert!(entry.apply_monthly_snapshot_for_test(
        3,
        memory_policy(period, MonthlyUsageMode::HardLimit, 0),
        7
    ));
    assert!(updates.has_changed().unwrap());
}

#[test]
fn monthly_capacity_uses_prepaid_generation_without_account_scan() {
    #[derive(Debug)]
    struct ForbiddenFlusher;
    impl ResourceUsageFlusher for ForbiddenFlusher {
        fn flush_usage(&self) {
            panic!("monthly monitor must not scan account flushers");
        }
    }
    let period = AccountUsagePeriod::current();
    let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 0);
    let flusher: Arc<dyn ResourceUsageFlusher> = Arc::new(ForbiddenFlusher);
    entry.register_resource_usage_flusher(Arc::downgrade(&flusher));
    let prepaid = entry.with_monthly_capacity(AgentMode::Durable, Some(0), |capacity| capacity);
    assert_eq!(prepaid.exhaustion, None);
    let unreserved = entry.with_monthly_capacity(AgentMode::Durable, None, |capacity| capacity);
    assert_eq!(
        unreserved.exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    drop(flusher);
    assert!(entry.apply_monthly_snapshot_for_test(1, monthly_policy(0), 7));
    let invalidated = entry.with_monthly_capacity(AgentMode::Durable, Some(0), |capacity| capacity);
    assert_eq!(
        invalidated.exhaustion,
        Some(MonthlyResourceExhaustion::Compute)
    );
    assert_ne!(prepaid, invalidated);
}

fn metered_entry_with_revision(revision: u64) -> AtomicResourceEntry {
    AtomicResourceEntry::new_with_all_limits_metering_and_revision(
        10_000,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig {
            compute: true,
            memory: true,
            filesystem: true,
        },
        revision,
    )
}

fn monthly_policy(available_fuel: u64) -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period: AccountUsagePeriod::current(),
        mode: MonthlyUsageMode::HardLimit,
        available_fuel,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}

fn usage_period(year: i32, month: u32) -> AccountUsagePeriod {
    AccountUsagePeriod { year, month }
}

fn memory_policy(
    period: AccountUsagePeriod,
    mode: MonthlyUsageMode,
    available_memory_gb_seconds: u64,
) -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period,
        mode,
        available_fuel: u64::MAX,
        available_memory_gb_seconds,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds: u64::MAX,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds: u64::MAX,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}

fn storage_policy(
    period: AccountUsagePeriod,
    mode: MonthlyUsageMode,
    available_durable_storage_byte_seconds: u64,
    available_ephemeral_storage_byte_seconds: u64,
) -> MonthlyResourcePolicy {
    MonthlyResourcePolicy {
        period,
        mode,
        available_fuel: u64::MAX,
        available_memory_gb_seconds: u64::MAX,
        available_memory_byte_nanoseconds_remainder: 0,
        available_durable_storage_byte_seconds,
        available_durable_storage_byte_nanoseconds_remainder: 0,
        available_ephemeral_storage_byte_seconds,
        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
    }
}

fn compute_entry(
    period: AccountUsagePeriod,
    mode: MonthlyUsageMode,
    available_fuel: u64,
) -> AtomicResourceEntry {
    AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
        MonthlyResourcePolicy {
            period,
            mode,
            available_fuel,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig {
            compute: true,
            memory: false,
            filesystem: false,
        },
        7,
        7,
        0,
    )
}

fn memory_entry(
    period: AccountUsagePeriod,
    mode: MonthlyUsageMode,
    available_memory_gb_seconds: u64,
) -> AtomicResourceEntry {
    AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
        memory_policy(period, mode, available_memory_gb_seconds),
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        },
        7,
        7,
        0,
    )
}

#[test]
fn monthly_memory_check_after_uses_exact_remaining_budget_and_ceiling() {
    let entry = memory_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        1,
    );
    entry.record_resource_settlement_for_period(
        AgentMode::Durable,
        AccountUsagePeriod::current(),
        ByteTimeSettlement {
            units: 0,
            remainder: BYTE_NANOSECONDS_PER_GB_SECOND - 10,
        },
        ByteTimeSettlement::default(),
    );
    assert_eq!(
        entry.monthly_memory_check_after(2),
        Some(Duration::from_nanos(5))
    );
    assert_eq!(
        entry.monthly_memory_check_after(3),
        Some(Duration::from_nanos(4))
    );
    assert_eq!(
        entry.monthly_memory_check_after(11),
        Some(Duration::from_nanos(1))
    );
    let huge = memory_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        u64::MAX,
    );
    assert_eq!(
        huge.monthly_memory_check_after(1),
        Some(Duration::from_nanos(u64::MAX))
    );
}

#[test]
fn monthly_memory_check_after_has_no_zero_or_inapplicable_horizon() {
    let period = AccountUsagePeriod::current();
    let hard = memory_entry(period, MonthlyUsageMode::HardLimit, 1);
    assert_eq!(hard.monthly_memory_check_after(0), None);
    let exhausted = memory_entry(period, MonthlyUsageMode::HardLimit, 0);
    assert_eq!(exhausted.monthly_memory_check_after(1), None);
    let overage = memory_entry(period, MonthlyUsageMode::AllowOverage, 1);
    assert_eq!(overage.monthly_memory_check_after(1), None);
    let disabled = compute_entry(period, MonthlyUsageMode::HardLimit, 1);
    assert_eq!(disabled.monthly_memory_check_after(1), None);
    hard.usage_revision_state
        .lock()
        .unwrap()
        .monthly_policy
        .as_mut()
        .unwrap()
        .period = AccountUsagePeriod {
        year: 2000,
        month: 1,
    };
    assert_eq!(hard.monthly_memory_check_after(1), None);
}

fn storage_entry(
    period: AccountUsagePeriod,
    mode: MonthlyUsageMode,
    available_durable_storage_byte_seconds: u64,
    available_ephemeral_storage_byte_seconds: u64,
) -> AtomicResourceEntry {
    AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
        storage_policy(
            period,
            mode,
            available_durable_storage_byte_seconds,
            available_ephemeral_storage_byte_seconds,
        ),
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: false,
            filesystem: true,
        },
        7,
        7,
        0,
    )
}

#[derive(Debug)]
struct MemoryUsageFlusher {
    entry: Weak<AtomicResourceEntry>,
    amount: Mutex<Option<i64>>,
}

impl ResourceUsageFlusher for MemoryUsageFlusher {
    fn flush_usage(&self) {
        let Some(amount) = self.amount.lock().unwrap().take() else {
            return;
        };
        if let Some(entry) = self.entry.upgrade() {
            entry.record_memory_gb_seconds(AgentMode::Durable, amount);
        }
    }
}

#[derive(Debug)]
struct RecordingMemoryLimitTarget {
    enforced: AtomicU64,
}

impl AgentMemoryLimitTarget for RecordingMemoryLimitTarget {
    fn enforce_limit(&self, limit: u64) {
        self.enforced.store(limit, Ordering::Release);
    }
}

#[test]
async fn filesystem_limit_update_is_delivered_to_registered_target() {
    let entry = Arc::new(AtomicResourceEntry::new(
        0,
        0,
        0,
        AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE,
        1,
    ));
    let owned_agent_id = OwnedAgentId::new(
        EnvironmentId(Uuid::new_v4()),
        &AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "live-filesystem-limit".to_string(),
        },
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let _registration = entry.register_agent_filesystem_limit_target(owned_agent_id, {
        let observed = Arc::clone(&observed);
        move |allocated_bytes| {
            let observed = Arc::clone(&observed);
            Box::pin(async move {
                observed.lock().unwrap().push(allocated_bytes);
                Ok(())
            })
        }
    });

    entry.apply_agent_filesystem_limit(4096).await.unwrap();

    assert_eq!(entry.max_disk_space_limit(), 4096);
    assert_eq!(*observed.lock().unwrap(), vec![4096]);
}

#[test]
async fn dropped_filesystem_limit_registration_detaches_target() {
    let entry = Arc::new(AtomicResourceEntry::new(
        0,
        0,
        0,
        AtomicResourceEntry::EFFECTIVELY_UNLIMITED_DISK_SPACE,
        1,
    ));
    let owned_agent_id = OwnedAgentId::new(
        EnvironmentId(Uuid::new_v4()),
        &AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: "detached-filesystem-limit".to_string(),
        },
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let registration = entry.register_agent_filesystem_limit_target(owned_agent_id, {
        let observed = Arc::clone(&observed);
        move |allocated_bytes| {
            let observed = Arc::clone(&observed);
            Box::pin(async move {
                observed.lock().unwrap().push(allocated_bytes);
                Ok(())
            })
        }
    });
    drop(registration);

    entry.apply_agent_filesystem_limit(2048).await.unwrap();

    assert_eq!(entry.max_disk_space_limit(), 2048);
    assert!(observed.lock().unwrap().is_empty());
}

#[test]
fn account_usage_accumulator_emits_oversized_settlements_in_bounded_batches() {
    let mut accumulator = AccountUsageAccumulator::default();
    let oversized = i64::MAX as u128 + 7;
    accumulator.add_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: oversized,
            remainder: 0,
        },
        None,
    );
    accumulator.add_storage_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: oversized,
            remainder: 0,
        },
        None,
    );

    assert_eq!(
        accumulator.capture(false),
        CapturedAccountUsage {
            memory_gb_seconds: i64::MAX,
            durable_memory_gb_seconds: i64::MAX,
            ephemeral_memory_gb_seconds: 0,
            durable_storage_byte_seconds: i64::MAX,
            ephemeral_storage_byte_seconds: 0,
            ..Default::default()
        }
    );
    assert_eq!(
        accumulator.capture(false),
        CapturedAccountUsage {
            memory_gb_seconds: 7,
            durable_memory_gb_seconds: 7,
            ephemeral_memory_gb_seconds: 0,
            durable_storage_byte_seconds: 7,
            ephemeral_storage_byte_seconds: 0,
            ..Default::default()
        }
    );
    assert!(!accumulator.is_active());
}

// -------------------------------------------------------------------------
// AtomicResourceEntry
// -------------------------------------------------------------------------

#[test]
fn delayed_pre_opt_in_usage_keeps_its_accrual_revision() {
    let entry = metered_entry_with_revision(3);
    assert!(entry.borrow_fuel(100));
    entry.record_resource_usage(AgentMode::Durable, 10, 30);
    entry.record_resource_usage(AgentMode::Ephemeral, 5, 7);
    assert!(entry.record_http_call());
    assert!(entry.record_rpc_call());

    entry.update_policy_revision(4);
    assert_eq!(entry.effective_fuel(), 9_900);
    assert!(entry.borrow_fuel(200));
    entry.record_resource_usage(AgentMode::Durable, 20, 40);

    let before_opt_in = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(before_opt_in.update.monthly_policy_revision, 3);
    assert_eq!(before_opt_in.update.fuel_delta, 100);
    assert_eq!(before_opt_in.update.memory_gb_seconds_delta, 15);
    assert_eq!(before_opt_in.update.http_call_count_delta, 1);
    assert_eq!(before_opt_in.update.rpc_call_count_delta, 1);
    assert_eq!(before_opt_in.update.durable_storage_byte_seconds_delta, 30);
    assert_eq!(before_opt_in.update.ephemeral_storage_byte_seconds_delta, 7);
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 100);
    assert_eq!(entry.syncing_http_calls.load(Ordering::Acquire), 1);
    assert_eq!(entry.syncing_rpc_calls.load(Ordering::Acquire), 1);
    assert_eq!(
        entry
            .in_flight_memory_gb_seconds_delta
            .load(Ordering::Acquire),
        15
    );
    assert_eq!(
        entry
            .in_flight_durable_memory_gb_seconds_delta
            .load(Ordering::Acquire),
        10
    );
    assert_eq!(
        entry
            .in_flight_ephemeral_memory_gb_seconds_delta
            .load(Ordering::Acquire),
        5
    );
    let after_opt_in = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(after_opt_in.update.monthly_policy_revision, 4);
    assert_eq!(after_opt_in.update.fuel_delta, 200);
    assert_eq!(after_opt_in.update.memory_gb_seconds_delta, 20);
    assert_eq!(after_opt_in.update.durable_storage_byte_seconds_delta, 40);
}

#[test]
fn older_policy_revision_cannot_roll_back_usage_attribution() {
    let entry = metered_entry_with_revision(7);

    entry.update_policy_revision(3);

    assert_eq!(
        entry
            .usage_revision_state
            .lock()
            .unwrap()
            .current_policy_revision,
        7
    );
}

#[test]
fn unaccepted_deliveries_are_retained_only_for_relevant_periods() {
    let gate_period = usage_period(2026, 4);
    let current_period = usage_period(2026, 5);
    let unrelated_period = usage_period(2026, 3);
    for (period, retained) in [
        (gate_period, true),
        (current_period, true),
        (unrelated_period, false),
    ] {
        let entry = storage_entry(gate_period, MonthlyUsageMode::HardLimit, 10, 10);
        let mut revision_state = entry.usage_revision_state.lock().unwrap();
        revision_state.current_period = current_period;
        let gate = revision_state.monthly_policy.as_mut().unwrap();
        let captured = entry.captured_byte_time_update_with_policy_revision(
            7,
            7,
            period,
            CapturedAccountUsage {
                durable_storage_byte_seconds: 1,
                ..CapturedAccountUsage::default()
            },
        );
        assert!(
            retain_unaccepted_delivery(
                gate,
                Some(InFlightMonthlyUsage {
                    captured,
                    usage: LocalMonthlyUsage::from_update(&captured.update),
                }),
                current_period,
                true,
            )
            .is_none()
        );
        assert_eq!(gate.stale_delivered_usage.len(), usize::from(retained));
    }

    for period in [gate_period, current_period, unrelated_period] {
        let entry = storage_entry(gate_period, MonthlyUsageMode::HardLimit, 10, 10);
        let mut revision_state = entry.usage_revision_state.lock().unwrap();
        revision_state.current_period = current_period;
        let gate = revision_state.monthly_policy.as_mut().unwrap();
        let mut captured = entry.captured_byte_time_update_with_policy_revision(
            7,
            7,
            period,
            CapturedAccountUsage {
                durable_storage_byte_seconds: 1,
                durable_storage_byte_nanoseconds_remainder: 3,
                ..CapturedAccountUsage::default()
            },
        );
        captured.update.fuel_delta = 5;
        let retry = retain_unaccepted_delivery(
            gate,
            Some(InFlightMonthlyUsage {
                captured,
                usage: LocalMonthlyUsage::from_update(&captured.update),
            }),
            current_period,
            false,
        );
        let retry = retry.unwrap();
        assert_eq!(retry.update.period, captured.update.period);
        assert_eq!(retry.update.durable_storage_byte_seconds_delta, 1);
        assert_eq!(retry.update.durable_storage_byte_nanoseconds_remainder, 3);
        assert_eq!(retry.update.fuel_delta, 0);
        assert_eq!(
            gate.failed_delivery_usage
                .get(&period)
                .map(|usage| usage.fuel),
            (period == gate_period || period == current_period).then_some(5)
        );
    }
}

#[test]
fn period_tagged_settlements_preserve_old_usage_and_storage_classes() {
    let january = usage_period(2030, 1);
    let february = usage_period(2030, 2);
    let entry = storage_entry(january, MonthlyUsageMode::AllowOverage, 100, 100);

    entry.record_resource_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 2,
            remainder: 7,
        },
    );
    entry.record_resource_settlement_for_period(
        AgentMode::Ephemeral,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 3,
            remainder: 11,
        },
    );
    entry.record_resource_settlement_for_period(
        AgentMode::Durable,
        february,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 5,
            remainder: 17,
        },
    );

    let old = entry.capture_usage_update_at_for_test(february);
    assert_eq!(old.period, january);
    assert_eq!(old.durable_storage_byte_seconds_delta, 2);
    assert_eq!(old.ephemeral_storage_byte_seconds_delta, 3);
    assert_eq!(old.durable_storage_byte_nanoseconds_remainder, 0);
    assert_eq!(old.ephemeral_storage_byte_nanoseconds_remainder, 0);

    let old_remainders = entry.capture_usage_update_at_for_test(february);
    assert_eq!(old_remainders.period, january);
    assert_eq!(old_remainders.durable_storage_byte_seconds_delta, 0);
    assert_eq!(old_remainders.durable_storage_byte_nanoseconds_remainder, 7);
    assert_eq!(
        old_remainders.ephemeral_storage_byte_nanoseconds_remainder,
        11
    );

    let current = entry.capture_usage_update_at_for_test(february);
    assert_eq!(current.period, february);
    assert_eq!(current.durable_storage_byte_seconds_delta, 5);
    assert_eq!(current.durable_storage_byte_nanoseconds_remainder, 0);
    assert_eq!(current.ephemeral_storage_byte_seconds_delta, 0);

    let march = usage_period(2030, 3);
    let current_remainder = entry.capture_usage_update_at_for_test(march);
    assert_eq!(current_remainder.period, february);
    assert_eq!(current_remainder.durable_storage_byte_seconds_delta, 0);
    assert_eq!(
        current_remainder.durable_storage_byte_nanoseconds_remainder,
        17
    );

    let memory = memory_entry(january, MonthlyUsageMode::AllowOverage, 100);
    memory.record_memory_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement {
            units: 1,
            remainder: 5,
        },
    );
    memory.record_memory_settlement_for_period(
        AgentMode::Ephemeral,
        february,
        ByteTimeSettlement {
            units: 4,
            remainder: 13,
        },
    );
    let old_memory = memory.capture_usage_update_at_for_test(february);
    assert_eq!(old_memory.period, january);
    assert_eq!(old_memory.memory_gb_seconds_delta, 1);
    assert_eq!(old_memory.memory_byte_nanoseconds_remainder, 0);
    let old_memory_remainder = memory.capture_usage_update_at_for_test(february);
    assert_eq!(old_memory_remainder.period, january);
    assert_eq!(old_memory_remainder.memory_gb_seconds_delta, 0);
    assert_eq!(old_memory_remainder.memory_byte_nanoseconds_remainder, 5);
    let current_memory = memory.capture_usage_update_at_for_test(march);
    assert_eq!(current_memory.period, february);
    assert_eq!(current_memory.memory_gb_seconds_delta, 4);
    assert_eq!(current_memory.memory_byte_nanoseconds_remainder, 0);
    let current_memory_remainder = memory.capture_usage_update_at_for_test(march);
    assert_eq!(current_memory_remainder.period, february);
    assert_eq!(current_memory_remainder.memory_gb_seconds_delta, 0);
    assert_eq!(
        current_memory_remainder.memory_byte_nanoseconds_remainder,
        13
    );
}

#[test]
fn late_prior_period_settlements_keep_their_period_and_bypass_current_limits() {
    let january = usage_period(2030, 1);
    let february = usage_period(2030, 2);
    let storage = storage_entry(february, MonthlyUsageMode::HardLimit, 0, 0);

    storage.record_resource_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 2,
            remainder: 7,
        },
    );
    storage.record_resource_settlement_for_period(
        AgentMode::Ephemeral,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 3,
            remainder: 11,
        },
    );

    let durable = storage.capture_usage_update_at_for_test(february);
    assert_eq!(durable.period, january);
    assert_eq!(durable.durable_storage_byte_seconds_delta, 2);
    assert_eq!(durable.durable_storage_byte_nanoseconds_remainder, 0);
    let durable_remainder = storage.capture_usage_update_at_for_test(february);
    assert_eq!(durable_remainder.period, january);
    assert_eq!(durable_remainder.durable_storage_byte_seconds_delta, 0);
    assert_eq!(
        durable_remainder.durable_storage_byte_nanoseconds_remainder,
        7
    );
    let ephemeral = storage.capture_usage_update_at_for_test(february);
    assert_eq!(ephemeral.period, january);
    assert_eq!(ephemeral.ephemeral_storage_byte_seconds_delta, 3);
    assert_eq!(ephemeral.ephemeral_storage_byte_nanoseconds_remainder, 0);
    let ephemeral_remainder = storage.capture_usage_update_at_for_test(february);
    assert_eq!(ephemeral_remainder.period, january);
    assert_eq!(ephemeral_remainder.ephemeral_storage_byte_seconds_delta, 0);
    assert_eq!(
        ephemeral_remainder.ephemeral_storage_byte_nanoseconds_remainder,
        11
    );

    let memory = memory_entry(february, MonthlyUsageMode::HardLimit, 0);
    memory.record_memory_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement {
            units: 5,
            remainder: 13,
        },
    );

    let memory_update = memory.capture_usage_update_at_for_test(february);
    assert_eq!(memory_update.period, january);
    assert_eq!(memory_update.memory_gb_seconds_delta, 5);
    assert_eq!(memory_update.memory_byte_nanoseconds_remainder, 0);
    let memory_remainder = memory.capture_usage_update_at_for_test(february);
    assert_eq!(memory_remainder.period, january);
    assert_eq!(memory_remainder.memory_gb_seconds_delta, 0);
    assert_eq!(memory_remainder.memory_byte_nanoseconds_remainder, 13);
}

#[test]
fn oversized_late_settlements_are_fully_drained_with_one_remainder() {
    let january = usage_period(2030, 1);
    let february = usage_period(2030, 2);
    let oversized = i64::MAX as u128 + 5;

    let storage = storage_entry(february, MonthlyUsageMode::AllowOverage, 0, 0);
    storage.record_resource_settlement_for_period(
        AgentMode::Ephemeral,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: oversized,
            remainder: 17,
        },
    );
    let first = storage.capture_usage_update_at(february, i64::MAX).unwrap();
    let second = storage.capture_usage_update_at(february, i64::MAX).unwrap();
    let remainder = storage.capture_usage_update_at(february, i64::MAX).unwrap();
    assert_eq!(first.update.period, january);
    assert_eq!(first.update.ephemeral_storage_byte_seconds_delta, i64::MAX);
    assert_eq!(second.update.ephemeral_storage_byte_seconds_delta, 5);
    assert_eq!(remainder.update.ephemeral_storage_byte_seconds_delta, 0);
    assert_eq!(first.update.ephemeral_storage_byte_nanoseconds_remainder, 0);
    assert_eq!(
        second.update.ephemeral_storage_byte_nanoseconds_remainder,
        0
    );
    assert_eq!(
        remainder
            .update
            .ephemeral_storage_byte_nanoseconds_remainder,
        17
    );

    storage.record_resource_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: oversized,
            remainder: 23,
        },
    );
    let first = storage.capture_usage_update_at(february, i64::MAX).unwrap();
    let second = storage.capture_usage_update_at(february, i64::MAX).unwrap();
    let remainder = storage.capture_usage_update_at(february, i64::MAX).unwrap();
    assert_eq!(first.update.durable_storage_byte_seconds_delta, i64::MAX);
    assert_eq!(first.update.ephemeral_storage_byte_seconds_delta, 0);
    assert_eq!(second.update.durable_storage_byte_seconds_delta, 5);
    assert_eq!(remainder.update.durable_storage_byte_seconds_delta, 0);
    assert_eq!(
        remainder.update.durable_storage_byte_nanoseconds_remainder,
        23
    );
    assert!(
        storage
            .capture_usage_update_at(february, i64::MAX)
            .is_none()
    );

    let memory = memory_entry(february, MonthlyUsageMode::AllowOverage, 0);
    memory.record_memory_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement {
            units: oversized,
            remainder: 19,
        },
    );
    let first = memory.capture_usage_update_at(february, i64::MAX).unwrap();
    let second = memory.capture_usage_update_at(february, i64::MAX).unwrap();
    let remainder = memory.capture_usage_update_at(february, i64::MAX).unwrap();
    assert_eq!(first.update.memory_gb_seconds_delta, i64::MAX);
    assert_eq!(first.durable_memory_gb_seconds_delta, i64::MAX);
    assert_eq!(second.update.memory_gb_seconds_delta, 5);
    assert_eq!(second.durable_memory_gb_seconds_delta, 5);
    assert_eq!(remainder.update.memory_gb_seconds_delta, 0);
    assert_eq!(remainder.update.memory_byte_nanoseconds_remainder, 19);
    assert!(memory.capture_usage_update_at(february, i64::MAX).is_none());
}

#[test]
fn explicitly_rejected_prior_period_update_is_recaptured_exactly_once() {
    let january = usage_period(2030, 1);
    let february = usage_period(2030, 2);
    let entry = storage_entry(february, MonthlyUsageMode::HardLimit, 10, 10);
    entry.record_resource_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 0,
            remainder: 7,
        },
    );

    let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    entry.begin_monthly_refresh(
        1,
        &captured.update,
        captured.durable_memory_gb_seconds_delta,
        captured.ephemeral_memory_gb_seconds_delta,
    );
    assert!(entry.apply_monthly_snapshot(
        1,
        storage_policy(february, MonthlyUsageMode::HardLimit, 10, 10),
        7,
        false,
    ));

    let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    assert_eq!(retried.update.period, captured.update.period);
    assert_eq!(retried.update.monthly_usage_mode_revision, 7);
    assert_eq!(retried.update.durable_storage_byte_seconds_delta, 0);
    assert_eq!(retried.update.durable_storage_byte_nanoseconds_remainder, 7);
    let revision_state = entry.usage_revision_state.lock().unwrap();
    assert!(
        revision_state
            .monthly_policy
            .as_ref()
            .unwrap()
            .failed_delivery_usage
            .is_empty()
    );
    assert_eq!(
        entry.effective_storage_byte_nanoseconds_with_revision_state(
            &revision_state,
            AgentMode::Durable,
        ),
        10_000_000_000
    );
    drop(revision_state);

    entry.begin_monthly_refresh(
        2,
        &retried.update,
        retried.durable_memory_gb_seconds_delta,
        retried.ephemeral_memory_gb_seconds_delta,
    );
    assert!(entry.apply_monthly_snapshot(
        2,
        storage_policy(february, MonthlyUsageMode::HardLimit, 10, 10),
        7,
        true,
    ));
    assert!(entry.capture_usage_update_at(february, i64::MAX).is_none());
}

#[test]
fn out_of_order_explicit_rejection_requeues_the_original_update() {
    let january = usage_period(2030, 1);
    let february = usage_period(2030, 2);
    let entry = memory_entry(february, MonthlyUsageMode::AllowOverage, 10);
    entry.record_memory_settlement_for_period(
        AgentMode::Ephemeral,
        january,
        ByteTimeSettlement {
            units: 0,
            remainder: 11,
        },
    );
    let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    entry.begin_monthly_refresh(
        1,
        &captured.update,
        captured.durable_memory_gb_seconds_delta,
        captured.ephemeral_memory_gb_seconds_delta,
    );
    entry.begin_monthly_refresh_for_test(2, 0);

    assert!(!entry.apply_monthly_snapshot(
        1,
        memory_policy(february, MonthlyUsageMode::AllowOverage, 10),
        7,
        false,
    ));
    let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    assert_eq!(retried.update.period, january);
    assert_eq!(retried.update.monthly_usage_mode_revision, 7);
    assert_eq!(retried.update.memory_gb_seconds_delta, 0);
    assert_eq!(retried.update.memory_byte_nanoseconds_remainder, 11);
    assert_eq!(retried.ephemeral_memory_gb_seconds_delta, 0);
}

#[test]
fn confirmed_server_failure_requeues_prior_period_byte_time() {
    let january = usage_period(2030, 1);
    let february = usage_period(2030, 2);
    let entry = storage_entry(february, MonthlyUsageMode::HardLimit, 10, 10);
    entry.record_resource_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 0,
            remainder: 29,
        },
    );
    let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    entry.begin_monthly_refresh(
        1,
        &captured.update,
        captured.durable_memory_gb_seconds_delta,
        captured.ephemeral_memory_gb_seconds_delta,
    );

    assert!(entry.finish_confirmed_non_application(1));
    let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    assert_eq!(retried.update.period, january);
    assert_eq!(retried.update.monthly_usage_mode_revision, 7);
    assert_eq!(
        retried.update.durable_storage_byte_nanoseconds_remainder,
        29
    );
}

#[test]
fn duplicate_confirmed_non_application_is_not_latest_but_requeues_byte_time() {
    let january = usage_period(2030, 1);
    let february = usage_period(2030, 2);
    let entry = storage_entry(february, MonthlyUsageMode::HardLimit, 10, 10);
    entry.record_resource_settlement_for_period(
        AgentMode::Durable,
        january,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 0,
            remainder: 31,
        },
    );
    let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    entry.begin_monthly_refresh(1, &captured.update, 0, 0);
    entry
        .usage_revision_state
        .lock()
        .unwrap()
        .monthly_policy
        .as_mut()
        .unwrap()
        .settled_generation = 1;

    assert!(!entry.finish_confirmed_non_application(1));
    let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    assert_eq!(retried.update.period, january);
    assert_eq!(
        retried.update.durable_storage_byte_nanoseconds_remainder,
        31
    );
    assert!(entry.capture_usage_update_at(february, i64::MAX).is_none());
}

#[test]
fn out_of_order_confirmed_non_application_is_not_latest_but_requeues_byte_time() {
    let january = usage_period(2030, 1);
    let february = usage_period(2030, 2);
    let entry = memory_entry(february, MonthlyUsageMode::AllowOverage, 10);
    entry.record_memory_settlement_for_period(
        AgentMode::Ephemeral,
        january,
        ByteTimeSettlement {
            units: 0,
            remainder: 37,
        },
    );
    let captured = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    entry.begin_monthly_refresh(1, &captured.update, 0, 0);
    entry.begin_monthly_refresh_for_test(2, 0);

    assert!(!entry.finish_confirmed_non_application(1));
    let retried = entry.capture_usage_update_at(february, i64::MAX).unwrap();
    assert_eq!(retried.update.period, january);
    assert_eq!(retried.update.memory_byte_nanoseconds_remainder, 37);
    assert_eq!(retried.ephemeral_memory_gb_seconds_delta, 0);
}

fn blob_entry_without_monthly_metering() -> AtomicResourceEntry {
    let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig::default(),
    );
    entry.set_available_blob_storage_bytes(100);
    entry
}

#[test]
fn blob_only_usage_survives_month_rollover_without_monthly_metering() {
    let entry = blob_entry_without_monthly_metering();
    let old_period = usage_period(2026, 1);
    let new_period = usage_period(2026, 2);
    entry.usage_revision_state.lock().unwrap().current_period = old_period;
    assert!(!entry.monthly_metering_enabled());
    assert!(entry.try_record_blob_storage_delta(40));

    let captured = entry.capture_usage_update_at(new_period, i64::MAX).unwrap();
    assert_eq!(captured.update.period, old_period);
    assert_eq!(captured.update.blob_storage_bytes_delta, 40);
    assert!(
        entry
            .capture_usage_update_at(new_period, i64::MAX)
            .is_none()
    );
    let quota = entry.blob_storage_quota.lock().unwrap();
    assert_eq!(quota.unsynced_delta, 0);
    assert_eq!(quota.syncing_delta, 40);
}

#[test]
fn blob_only_usage_survives_policy_revision_rotation() {
    let entry = blob_entry_without_monthly_metering();
    assert!(entry.try_record_blob_storage_delta(40));
    entry.update_policy_revision(1);

    let captured = entry.capture_usage_update(i64::MAX).unwrap();
    assert_eq!(captured.update.monthly_policy_revision, 0);
    assert_eq!(captured.update.blob_storage_bytes_delta, 40);
    assert!(entry.capture_usage_update(i64::MAX).is_none());
    let quota = entry.blob_storage_quota.lock().unwrap();
    assert_eq!(quota.unsynced_delta, 0);
    assert_eq!(quota.syncing_delta, 40);
}

#[test]
fn revision_rotation_captures_each_usage_dimension_independently() {
    let compute = metered_entry_with_revision(1);
    assert!(compute.borrow_fuel(1));
    compute.update_policy_revision(2);
    let compute_update = compute
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(compute_update.update.monthly_policy_revision, 1);
    assert_eq!(compute_update.update.fuel_delta, 1);

    let memory = metered_entry_with_revision(1);
    memory.record_memory_gb_seconds(AgentMode::Durable, 2);
    memory.update_policy_revision(2);
    let memory_update = memory
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(memory_update.update.monthly_policy_revision, 1);
    assert_eq!(memory_update.update.memory_gb_seconds_delta, 2);

    let http = metered_entry_with_revision(1);
    assert!(http.record_http_call());
    http.update_policy_revision(2);
    let http_update = http
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(http_update.update.monthly_policy_revision, 1);
    assert_eq!(http_update.update.http_call_count_delta, 1);

    let rpc = metered_entry_with_revision(1);
    assert!(rpc.record_rpc_call());
    rpc.update_policy_revision(2);
    let rpc_update = rpc
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(rpc_update.update.monthly_policy_revision, 1);
    assert_eq!(rpc_update.update.rpc_call_count_delta, 1);
}

#[test]
fn repeated_revision_rotations_preserve_fractional_settlement_remainders() {
    let entry = metered_entry_with_revision(3);
    entry.record_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 0,
            remainder: BYTE_NANOSECONDS_PER_GB_SECOND - 1,
        },
    );
    entry.update_policy_revision(4);

    entry.record_resource_settlement(
        AgentMode::Durable,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 0,
            remainder: 1_000_000_000 - 1,
        },
    );
    entry.update_policy_revision(5);

    entry.record_resource_settlement(
        AgentMode::Ephemeral,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 0,
            remainder: 1_000_000_000 - 1,
        },
    );
    entry.update_policy_revision(6);

    entry.record_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 0,
            remainder: 1,
        },
    );
    entry.record_resource_settlement(
        AgentMode::Durable,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 0,
            remainder: 1,
        },
    );
    entry.record_resource_settlement(
        AgentMode::Ephemeral,
        ByteTimeSettlement::default(),
        ByteTimeSettlement {
            units: 0,
            remainder: 1,
        },
    );
    entry.update_policy_revision(7);

    let revision_3 = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(revision_3.update.monthly_policy_revision, 3);
    assert_eq!(
        revision_3.update.memory_byte_nanoseconds_remainder,
        BYTE_NANOSECONDS_PER_GB_SECOND as u64 - 1
    );
    assert_eq!(
        revision_3.update.durable_storage_byte_nanoseconds_remainder,
        0
    );
    assert_eq!(
        revision_3
            .update
            .ephemeral_storage_byte_nanoseconds_remainder,
        0
    );

    let revision_4 = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(revision_4.update.monthly_policy_revision, 4);
    assert_eq!(revision_4.update.memory_byte_nanoseconds_remainder, 0);
    assert_eq!(
        revision_4.update.durable_storage_byte_nanoseconds_remainder,
        1_000_000_000 - 1
    );
    assert_eq!(
        revision_4
            .update
            .ephemeral_storage_byte_nanoseconds_remainder,
        0
    );

    let revision_5 = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(revision_5.update.monthly_policy_revision, 5);
    assert_eq!(revision_5.update.memory_byte_nanoseconds_remainder, 0);
    assert_eq!(
        revision_5.update.durable_storage_byte_nanoseconds_remainder,
        0
    );
    assert_eq!(
        revision_5
            .update
            .ephemeral_storage_byte_nanoseconds_remainder,
        1_000_000_000 - 1
    );

    let revision_6 = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(revision_6.update.monthly_policy_revision, 6);
    assert_eq!(revision_6.update.memory_byte_nanoseconds_remainder, 1);
    assert_eq!(
        revision_6.update.durable_storage_byte_nanoseconds_remainder,
        1
    );
    assert_eq!(
        revision_6
            .update
            .ephemeral_storage_byte_nanoseconds_remainder,
        1
    );

    assert_eq!(
        revision_3.update.memory_byte_nanoseconds_remainder
            + revision_6.update.memory_byte_nanoseconds_remainder,
        BYTE_NANOSECONDS_PER_GB_SECOND as u64
    );
    assert_eq!(
        revision_4.update.durable_storage_byte_nanoseconds_remainder
            + revision_6.update.durable_storage_byte_nanoseconds_remainder,
        1_000_000_000
    );
    assert_eq!(
        revision_5
            .update
            .ephemeral_storage_byte_nanoseconds_remainder
            + revision_6
                .update
                .ephemeral_storage_byte_nanoseconds_remainder,
        1_000_000_000
    );
}

#[test]
fn delayed_pre_disable_usage_keeps_its_accrual_revision() {
    let entry = metered_entry_with_revision(8);
    assert!(entry.borrow_fuel(100));
    entry.record_storage_byte_seconds(AgentMode::Durable, 10);

    entry.update_policy_revision(9);
    assert!(entry.borrow_fuel(200));
    entry.record_storage_byte_seconds(AgentMode::Durable, 20);

    let before_disable = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(before_disable.update.monthly_policy_revision, 8);
    assert_eq!(before_disable.update.fuel_delta, 100);
    assert_eq!(before_disable.update.durable_storage_byte_seconds_delta, 10);
    let after_disable = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(after_disable.update.monthly_policy_revision, 9);
    assert_eq!(after_disable.update.fuel_delta, 200);
    assert_eq!(after_disable.update.durable_storage_byte_seconds_delta, 20);
}

#[test]
fn revision_refresh_does_not_relabel_in_flight_usage() {
    let entry = metered_entry_with_revision(12);
    assert!(entry.borrow_fuel(100));
    let in_flight = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();

    entry.update_policy_revision(13);
    assert!(entry.borrow_fuel(200));

    assert_eq!(in_flight.update.monthly_policy_revision, 12);
    let next = entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(next.update.monthly_policy_revision, 13);
    assert_eq!(next.update.fuel_delta, 200);
}

#[test]
fn disabled_usage_dimensions_accumulate_and_export_zero() {
    let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
        10,
        20,
        30,
        40,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig::default(),
    );
    assert!(entry.account_usage_accumulator.is_none());
    entry.delta.store(17, Ordering::Release);
    entry.in_flight_delta.store(19, Ordering::Release);
    entry
        .in_flight_memory_gb_seconds_delta
        .store(23, Ordering::Release);
    assert!(entry.borrow_fuel(7));
    entry.return_fuel(3);
    entry.record_overdraft_debt(5);
    entry.record_memory_gb_seconds(AgentMode::Durable, 11);
    entry.record_storage_byte_seconds(AgentMode::Durable, 13);

    let captured = entry
        .capture_usage_update(0)
        .expect("stale limit refresh still produces an update");

    assert_eq!(entry.effective_fuel(), u64::MAX);
    assert_eq!(entry.delta.load(Ordering::Acquire), 17);
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 19);
    assert_eq!(
        entry
            .in_flight_memory_gb_seconds_delta
            .load(Ordering::Acquire),
        23
    );
    assert_eq!(captured.update.fuel_delta, 0);
    assert_eq!(captured.update.memory_gb_seconds_delta, 0);
    assert_eq!(captured.update.durable_storage_byte_seconds_delta, 0);
    assert_eq!(captured.update.ephemeral_storage_byte_seconds_delta, 0);
    assert!(!captured.update.metering.compute);
    assert!(!captured.update.metering.memory);
    assert!(!captured.update.metering.filesystem);
}

#[test]
async fn unmanaged_filesystem_config_constructs_no_monthly_storage_path_but_keeps_disk_limit() {
    let mut config = GolemConfig::default();
    config.resource_usage_metering.filesystem = true;
    config.filesystem_storage.mode =
        crate::services::golem_config::FilesystemStorageMode::Temporary;
    let metering = config.effective_resource_usage_metering();
    let entry = AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
        MonthlyResourcePolicy {
            period: AccountUsagePeriod::current(),
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 0,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 0,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        usize::MAX,
        usize::MAX,
        4_096,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        metering,
        0,
        0,
        0,
    );

    assert!(!metering.filesystem);
    assert!(
        entry
            .usage_revision_state
            .lock()
            .unwrap()
            .monthly_policy
            .is_none()
    );
    assert!(entry.account_usage_accumulator.is_none());
    entry.record_storage_byte_seconds(AgentMode::Durable, 10);
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

    assert_eq!(entry.max_disk_space_limit(), 4_096);
    entry.apply_agent_filesystem_limit(8_192).await.unwrap();
    assert_eq!(entry.max_disk_space_limit(), 8_192);
}

#[test]
fn usage_update_reports_configured_metering_state() {
    let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig {
            compute: true,
            memory: false,
            filesystem: true,
        },
    );

    let captured = entry
        .capture_usage_update(0)
        .expect("stale limit refresh still produces an update");

    assert!(captured.update.metering.compute);
    assert!(!captured.update.metering.memory);
    assert!(captured.update.metering.filesystem);
}

#[test]
fn usage_accumulation_respects_each_enabled_dimension() {
    for memory in [false, true] {
        for filesystem in [false, true] {
            let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
                u64::MAX,
                usize::MAX,
                usize::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
                AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                ResourceUsageMeteringConfig {
                    compute: false,
                    memory,
                    filesystem,
                },
            );
            entry.record_memory_gb_seconds(AgentMode::Durable, 11);
            entry.record_storage_byte_seconds(AgentMode::Durable, 13);

            let (has_accumulator, has_memory, has_storage) = entry
                .account_usage_accumulator
                .as_ref()
                .map_or((false, false, false), |accumulator| {
                    let usage = accumulator.lock().unwrap();
                    (true, usage.memory.is_some(), usage.storage.is_some())
                });
            assert_eq!(has_accumulator, memory || filesystem);
            assert_eq!(has_memory, memory);
            assert_eq!(has_storage, filesystem);

            assert_eq!(
                entry.memory_gb_seconds_delta(AgentMode::Durable),
                if memory { 11 } else { 0 }
            );
            assert_eq!(
                entry.durable_byte_seconds_delta(),
                if filesystem { 13 } else { 0 }
            );
        }
    }
}

#[test]
fn every_enabled_monthly_dimension_independently_controls_admission_in_all_meter_combinations() {
    let period = AccountUsagePeriod::current();
    for compute in [false, true] {
        for memory in [false, true] {
            for filesystem in [false, true] {
                let metering = ResourceUsageMeteringConfig {
                    compute,
                    memory,
                    filesystem,
                };
                let cases = [
                    (
                        MonthlyResourceExhaustion::Compute,
                        compute,
                        AgentMode::Durable,
                    ),
                    (
                        MonthlyResourceExhaustion::Memory,
                        memory,
                        AgentMode::Durable,
                    ),
                    (
                        MonthlyResourceExhaustion::DurableStorage,
                        filesystem,
                        AgentMode::Durable,
                    ),
                    (
                        MonthlyResourceExhaustion::EphemeralStorage,
                        filesystem,
                        AgentMode::Ephemeral,
                    ),
                ];

                for (exhausted, enabled, agent_mode) in cases {
                    let mut policy = MonthlyResourcePolicy {
                        period,
                        mode: MonthlyUsageMode::HardLimit,
                        available_fuel: 1,
                        available_memory_gb_seconds: 1,
                        available_memory_byte_nanoseconds_remainder: 0,
                        available_durable_storage_byte_seconds: 1,
                        available_durable_storage_byte_nanoseconds_remainder: 0,
                        available_ephemeral_storage_byte_seconds: 1,
                        available_ephemeral_storage_byte_nanoseconds_remainder: 0,
                    };
                    match exhausted {
                        MonthlyResourceExhaustion::Compute => policy.available_fuel = 0,
                        MonthlyResourceExhaustion::Memory => policy.available_memory_gb_seconds = 0,
                        MonthlyResourceExhaustion::DurableStorage => {
                            policy.available_durable_storage_byte_seconds = 0
                        }
                        MonthlyResourceExhaustion::EphemeralStorage => {
                            policy.available_ephemeral_storage_byte_seconds = 0
                        }
                    }
                    let entry =
                        AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
                            policy,
                            4_096,
                            usize::MAX,
                            8_192,
                            u64::MAX,
                            u64::MAX,
                            u64::MAX,
                            u64::MAX,
                            AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
                            AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                            metering,
                            7,
                            7,
                            0,
                        );

                    let expected_capacity = enabled.then_some(exhausted);
                    assert_eq!(
                        entry.monthly_resource_capacity(agent_mode),
                        expected_capacity.map_or(Ok(()), |exhaustion| Err(exhaustion.into())),
                        "metering={metering:?}, exhausted={exhausted:?}, mode={agent_mode:?}"
                    );
                    let expected_admission = match (expected_capacity, agent_mode) {
                        (None, _) => MonthlyResourceAdmission::Admit,
                        (Some(_), AgentMode::Durable) => MonthlyResourceAdmission::Suspend,
                        (Some(exhaustion), AgentMode::Ephemeral) => {
                            MonthlyResourceAdmission::FailInvocation(exhaustion)
                        }
                    };
                    assert_eq!(
                        monthly_resource_admission(&entry, agent_mode),
                        expected_admission,
                        "metering={metering:?}, exhausted={exhausted:?}, mode={agent_mode:?}"
                    );

                    assert_eq!(entry.max_memory_limit(), 4_096);
                    assert_eq!(entry.max_disk_space_limit(), 8_192);
                }

                let accounting_entry =
                    AtomicResourceEntry::new_with_all_limits_metering_policy_and_revisions(
                        MonthlyResourcePolicy {
                            period,
                            mode: MonthlyUsageMode::HardLimit,
                            available_fuel: 1,
                            available_memory_gb_seconds: 1,
                            available_memory_byte_nanoseconds_remainder: 0,
                            available_durable_storage_byte_seconds: 1,
                            available_durable_storage_byte_nanoseconds_remainder: 0,
                            available_ephemeral_storage_byte_seconds: 1,
                            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
                        },
                        4_096,
                        usize::MAX,
                        8_192,
                        u64::MAX,
                        u64::MAX,
                        u64::MAX,
                        u64::MAX,
                        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
                        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
                        metering,
                        7,
                        7,
                        0,
                    );
                assert!(accounting_entry.borrow_fuel(1));
                accounting_entry.record_memory_gb_seconds(AgentMode::Durable, 1);
                accounting_entry.record_storage_byte_seconds(AgentMode::Durable, 1);
                accounting_entry.record_storage_byte_seconds(AgentMode::Ephemeral, 1);

                let captured = accounting_entry
                    .capture_usage_update(0)
                    .expect("the zero refresh threshold includes every meter combination")
                    .update;
                assert_eq!(captured.fuel_delta, i64::from(compute));
                assert_eq!(captured.memory_gb_seconds_delta, i64::from(memory));
                assert_eq!(
                    captured.durable_storage_byte_seconds_delta,
                    i64::from(filesystem)
                );
                assert_eq!(
                    captured.ephemeral_storage_byte_seconds_delta,
                    i64::from(filesystem)
                );
                assert_eq!(captured.metering.compute, compute);
                assert_eq!(captured.metering.memory, memory);
                assert_eq!(captured.metering.filesystem, filesystem);
            }
        }
    }
}

#[test]
fn effective_fuel_with_zero_delta() {
    let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
    assert_eq!(entry.effective_fuel(), 1000);
}

#[test]
fn effective_fuel_subtracts_unsent_and_in_flight_usage() {
    let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
    assert!(entry.borrow_fuel(50));
    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    entry.begin_monthly_refresh_for_test(1, 50);
    assert!(entry.borrow_fuel(200));
    assert_eq!(entry.effective_fuel(), 750);
}

#[test]
fn effective_fuel_applies_signed_refunds_with_saturation() {
    let entry = AtomicResourceEntry::new(100, 0, usize::MAX, u64::MAX, u64::MAX);
    entry.delta.store(-200, Ordering::Release);
    assert_eq!(entry.effective_fuel(), 300);
}

#[test]
fn effective_fuel_clamps_refund_overflow_to_u64_max() {
    let entry = AtomicResourceEntry::new(u64::MAX, 0, usize::MAX, u64::MAX, u64::MAX);
    entry.delta.store(-1, Ordering::Release);
    assert_eq!(entry.effective_fuel(), u64::MAX);
}

#[test]
fn borrow_fuel_succeeds_and_increases_delta() {
    let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
    assert!(entry.borrow_fuel(300));
    assert_eq!(entry.delta.load(Ordering::Acquire), 300);
    assert_eq!(entry.effective_fuel(), 700);
}

#[test]
fn borrow_fuel_fails_when_effective_fuel_is_zero() {
    // fuel=0, delta=0 → effective=0; any non-zero borrow fails
    let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, u64::MAX);
    assert!(!entry.borrow_fuel(1));
    assert_eq!(entry.delta.load(Ordering::Acquire), 0);
}

#[test]
fn borrow_fuel_uses_partial_final_capacity() {
    let entry = AtomicResourceEntry::new(100, 0, usize::MAX, u64::MAX, u64::MAX);
    assert!(entry.borrow_fuel(101));
    assert_eq!(entry.delta.load(Ordering::Acquire), 100);
    assert!(!entry.borrow_fuel(1));
}

#[test]
fn borrow_fuel_zero_amount_always_succeeds_without_touching_delta() {
    let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, u64::MAX);
    assert!(entry.borrow_fuel(0));
    assert_eq!(entry.delta.load(Ordering::Acquire), 0);
}

#[test]
fn borrow_fuel_exactly_at_effective_fuel_succeeds() {
    // Borrowing exactly effective_fuel must succeed
    let entry = AtomicResourceEntry::new(500, 0, usize::MAX, u64::MAX, u64::MAX);
    assert!(entry.borrow_fuel(500));
    assert_eq!(entry.delta.load(Ordering::Acquire), 500);
}

#[test]
fn borrow_fuel_one_over_effective_fuel_takes_the_exact_remainder() {
    let entry = AtomicResourceEntry::new(500, 0, usize::MAX, u64::MAX, u64::MAX);
    assert!(entry.borrow_fuel(501));
    assert_eq!(entry.delta.load(Ordering::Acquire), 500);
}

#[test]
fn return_fuel_decreases_delta() {
    // borrow 400 → delta = +400; return 100 unused → delta = 300
    let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
    entry.borrow_fuel(400);
    entry.return_fuel(100);
    assert_eq!(entry.delta.load(Ordering::Acquire), 300);
}

#[test]
fn borrow_then_full_return_nets_delta_to_zero() {
    // borrow 500, return 500 (nothing consumed) → delta = 0
    let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
    entry.borrow_fuel(500);
    entry.return_fuel(500);
    assert_eq!(entry.delta.load(Ordering::Acquire), 0);
}

#[test]
fn return_fuel_does_not_panic_on_large_amount() {
    // delta at i64::MIN, return u64::MAX → saturates at i64::MIN, no panic
    let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, u64::MAX);
    entry.delta.store(i64::MIN, Ordering::Release);
    entry.return_fuel(u64::MAX);
    let _ = entry.delta.load(Ordering::Acquire);
}

#[test]
fn record_overdraft_debt_increases_delta_by_actual_consumed_amount() {
    let entry = AtomicResourceEntry::new(1000, 0, usize::MAX, u64::MAX, u64::MAX);
    entry.record_overdraft_debt(2000);

    assert_eq!(entry.delta.load(Ordering::Acquire), 2000);
    assert_eq!(entry.effective_fuel(), 0);
}

#[test]
fn hard_limit_zero_blocks_while_allow_overage_zero_borrows() {
    let period = AccountUsagePeriod::current();
    let hard = compute_entry(period, MonthlyUsageMode::HardLimit, 0);
    assert!(hard.monthly_resource_capacity(AgentMode::Durable).is_err());
    assert!(matches!(
        hard.borrow_fuel_with_revision(1),
        FuelBorrow::Exhausted { .. }
    ));

    let overage = compute_entry(period, MonthlyUsageMode::AllowOverage, 0);
    assert!(
        overage
            .monthly_resource_capacity(AgentMode::Durable)
            .is_ok()
    );
    assert_eq!(
        overage.borrow_fuel_with_revision(10),
        FuelBorrow::Borrowed {
            amount: 10,
            revision: 7,
            generation: 0,
            period,
        }
    );
    assert_eq!(overage.fuel_delta(), 10);
}

#[test]
fn allow_overage_bypasses_direct_memory_and_storage_capacity_checks() {
    let period = AccountUsagePeriod::current();
    let memory = memory_entry(period, MonthlyUsageMode::AllowOverage, 0);
    assert!(
        memory
            .monthly_memory_and_storage_capacity(AgentMode::Durable)
            .is_ok()
    );

    let storage = storage_entry(period, MonthlyUsageMode::AllowOverage, 0, 0);
    assert!(
        storage
            .monthly_memory_and_storage_capacity(AgentMode::Durable)
            .is_ok()
    );
    assert!(
        storage
            .monthly_memory_and_storage_capacity(AgentMode::Ephemeral)
            .is_ok()
    );
}

#[test]
fn exact_capacity_and_partial_final_borrow_are_atomic() {
    let entry = compute_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        15,
    );
    assert_eq!(
        entry.borrow_fuel_with_revision(10),
        FuelBorrow::Borrowed {
            amount: 10,
            revision: 7,
            generation: 0,
            period: AccountUsagePeriod::current(),
        }
    );
    assert_eq!(
        entry.borrow_fuel_with_revision(10),
        FuelBorrow::Borrowed {
            amount: 5,
            revision: 7,
            generation: 0,
            period: AccountUsagePeriod::current(),
        }
    );
    assert!(matches!(
        entry.borrow_fuel_with_revision(1),
        FuelBorrow::Exhausted { .. }
    ));
    assert_eq!(entry.fuel_delta(), 15);
}

#[test]
fn every_local_delivery_state_reduces_hard_limit_capacity() {
    let unsent = compute_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        100,
    );
    assert!(unsent.borrow_fuel(10));
    assert_eq!(unsent.effective_fuel(), 90);

    let in_flight = compute_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        100,
    );
    assert!(in_flight.borrow_fuel(10));
    in_flight
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(in_flight.effective_fuel(), 90);

    let pending = compute_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        100,
    );
    assert!(pending.borrow_fuel(10));
    pending.update_policy_revision(8);
    assert_eq!(pending.effective_fuel(), 90);

    let failed = compute_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        100,
    );
    assert!(failed.borrow_fuel(10));
    failed
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    failed.begin_monthly_refresh_for_test(1, 10);
    assert!(failed.finish_ambiguous_transport_failure(1));
    assert_eq!(failed.effective_fuel(), 90);
    assert_eq!(failed.in_flight_delta.load(Ordering::Acquire), 0);
    assert!(
        failed
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .is_none(),
        "failed compute usage is retained only for enforcement"
    );
}

#[test]
fn memory_hard_limit_exports_full_usage_while_preserving_exhaustion() {
    let period = AccountUsagePeriod::current();
    let hard = memory_entry(period, MonthlyUsageMode::HardLimit, 10);
    hard.record_memory_gb_seconds(AgentMode::Durable, 14);

    assert_eq!(hard.effective_memory_gb_seconds(), 0);
    assert_eq!(
        hard.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
    assert_eq!(
        hard.capture_usage_update_for_test().memory_gb_seconds_delta,
        14
    );
    assert_eq!(
        hard.usage_revision_state
            .lock()
            .unwrap()
            .monthly_policy
            .as_ref()
            .unwrap()
            .hard_limit_memory_overshoot_byte_nanoseconds
            .get(&(period, 7)),
        Some(&(4 * BYTE_NANOSECONDS_PER_GB_SECOND))
    );
    hard.begin_monthly_refresh_with_memory_for_test(1, 0, 14);
    assert!(hard.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 0,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(
        hard.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
    hard.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
    assert!(hard.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 1,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(hard.effective_memory_gb_seconds(), 1);

    let overage = memory_entry(period, MonthlyUsageMode::AllowOverage, 10);
    overage.record_memory_gb_seconds(AgentMode::Ephemeral, 14);

    assert!(overage.has_monthly_memory_capacity());
    assert!(
        overage
            .monthly_resource_capacity(AgentMode::Durable)
            .is_ok()
    );
    assert_eq!(
        overage
            .capture_usage_update_for_test()
            .memory_gb_seconds_delta,
        14
    );
}

#[test]
fn storage_hard_limits_are_isolated_by_agent_mode() {
    let period = AccountUsagePeriod::current();
    let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 10, 20);

    entry.record_storage_byte_seconds(AgentMode::Durable, 10);
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::DurableStorage.into())
    );
    assert!(
        entry
            .monthly_resource_capacity(AgentMode::Ephemeral)
            .is_ok()
    );

    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 20);
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Ephemeral),
        Err(MonthlyResourceExhaustion::EphemeralStorage.into())
    );
    let captured = entry.capture_usage_update_for_test();
    assert_eq!(captured.durable_storage_byte_seconds_delta, 10);
    assert_eq!(captured.ephemeral_storage_byte_seconds_delta, 20);
}

#[test]
fn hard_limit_memory_exports_full_measurement_and_matches_fresh_entry_capacity() {
    let period = AccountUsagePeriod::current();
    let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 1);
    entry.record_memory_gb_seconds(AgentMode::Durable, 2);
    let captured = entry.capture_usage_update(i64::MAX).unwrap();
    assert_eq!(captured.update.memory_gb_seconds_delta, 2);
    entry.begin_monthly_refresh(1, &captured.update, 2, 0);
    assert!(entry.apply_monthly_snapshot(
        1,
        memory_policy(period, MonthlyUsageMode::HardLimit, 0),
        7,
        true,
    ));
    let reconstructed = memory_entry(period, MonthlyUsageMode::HardLimit, 0);
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        reconstructed.monthly_resource_capacity(AgentMode::Durable)
    );
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
    assert!(entry.apply_monthly_snapshot_for_test(
        2,
        memory_policy(period, MonthlyUsageMode::HardLimit, 1),
        8,
    ));
    assert_eq!(entry.effective_memory_gb_seconds(), 1);
}

#[test]
fn hard_limit_storage_exports_full_measurement_for_both_classes() {
    let period = AccountUsagePeriod::current();
    let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 1, 1);
    entry.record_storage_byte_seconds(AgentMode::Durable, 2);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 3);
    let captured = entry.capture_usage_update(i64::MAX).unwrap();
    assert_eq!(captured.update.durable_storage_byte_seconds_delta, 2);
    assert_eq!(captured.update.ephemeral_storage_byte_seconds_delta, 3);
    entry.begin_monthly_refresh(1, &captured.update, 0, 0);
    assert!(entry.apply_monthly_snapshot(
        1,
        storage_policy(period, MonthlyUsageMode::HardLimit, 0, 0),
        7,
        true,
    ));
    let reconstructed = storage_entry(period, MonthlyUsageMode::HardLimit, 0, 0);
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        assert_eq!(
            entry.monthly_resource_capacity(mode),
            reconstructed.monthly_resource_capacity(mode)
        );
        assert!(entry.monthly_resource_capacity(mode).is_err());
    }
    assert!(entry.apply_monthly_snapshot_for_test(
        2,
        storage_policy(period, MonthlyUsageMode::HardLimit, 1, 1),
        8,
    ));
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        assert!(entry.monthly_resource_capacity(mode).is_ok());
    }
}

#[test]
fn idle_refresh_accounts_for_adversarial_batch_phase_with_sub_unit_memory() {
    let ResourceLimitsConfig::Grpc(config) = ResourceLimitsConfig::default() else {
        panic!("default resource limits must use Registry refresh");
    };
    let threshold =
        idle_refresh_threshold_secs(config.batch_update_interval, config.limit_refresh_interval);
    let entry = memory_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::AllowOverage,
        1,
    );
    entry.record_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 0,
            remainder: 1,
        },
    );
    assert!(
        !entry
            .account_usage_accumulator
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .is_active()
    );
    // The account initializes one second after the batch timer's zero tick.
    // At the 300-second tick its cached policy is only 299 seconds old.
    entry
        .last_refresh_secs
        .store(Utc::now().timestamp() - 299, Ordering::Release);
    assert!(
        entry.capture_usage_update(threshold).is_some(),
        "the 300-second tick must refresh rather than waiting until 360 seconds"
    );
}

#[test]
fn storage_hard_limit_exports_full_usage_and_preserves_exhaustion() {
    let period = AccountUsagePeriod::current();
    let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 1, u64::MAX);

    entry.record_storage_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 2,
            remainder: 500_000_000,
        },
    );

    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::DurableStorage.into())
    );
    let captured = entry.capture_usage_update_for_test();
    assert_eq!(captured.durable_storage_byte_seconds_delta, 2);
    assert_eq!(captured.durable_storage_byte_nanoseconds_remainder, 0);
    assert_eq!(
        entry
            .usage_revision_state
            .lock()
            .unwrap()
            .monthly_policy
            .as_ref()
            .unwrap()
            .hard_limit_durable_storage_overshoot_byte_nanoseconds
            .get(&(period, 7)),
        Some(&1_500_000_000)
    );
}

#[test]
fn same_period_refresh_retains_storage_overshoot_for_admission() {
    let period = AccountUsagePeriod::current();
    let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 1, 1);
    entry.record_storage_byte_seconds(AgentMode::Durable, 2);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 2);

    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 2,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 2,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
    ));
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::DurableStorage.into())
    );
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Ephemeral),
        Err(MonthlyResourceExhaustion::EphemeralStorage.into())
    );
}

#[test]
fn storage_allow_overage_records_full_usage() {
    let entry = storage_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::AllowOverage,
        1,
        1,
    );

    entry.record_storage_byte_seconds(AgentMode::Durable, 3);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 4);

    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    assert!(
        entry
            .monthly_resource_capacity(AgentMode::Ephemeral)
            .is_ok()
    );
    let captured = entry.capture_usage_update_for_test();
    assert_eq!(captured.durable_storage_byte_seconds_delta, 3);
    assert_eq!(captured.ephemeral_storage_byte_seconds_delta, 4);
}

#[test]
fn combined_resource_settlement_records_memory_and_storage_by_mode() {
    let entry = metered_entry_with_revision(7);

    entry.record_resource_settlement(
        AgentMode::Ephemeral,
        ByteTimeSettlement {
            units: 3,
            remainder: 250_000_000,
        },
        ByteTimeSettlement {
            units: 5,
            remainder: 750_000_000,
        },
    );

    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Ephemeral), 3);
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    assert_eq!(entry.ephemeral_byte_seconds_delta(), 5);
}

#[test]
fn in_flight_storage_usage_remains_visible_to_hard_limit_enforcement() {
    let entry = storage_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        10,
        u64::MAX,
    );
    entry.record_storage_byte_seconds(AgentMode::Durable, 10);

    let captured = entry.capture_usage_update_for_test();

    assert_eq!(captured.durable_storage_byte_seconds_delta, 10);
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::DurableStorage.into())
    );
}

#[test]
fn concurrent_storage_settlements_preserve_all_measured_usage() {
    let entry = Arc::new(storage_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        100,
        u64::MAX,
    ));
    let recorders = (0..16)
        .map(|_| {
            let entry = Arc::clone(&entry);
            std::thread::spawn(move || entry.record_storage_byte_seconds(AgentMode::Durable, 10))
        })
        .collect::<Vec<_>>();
    for recorder in recorders {
        recorder.join().unwrap();
    }

    assert_eq!(entry.durable_byte_seconds_delta(), 160);
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::DurableStorage.into())
    );
    assert_eq!(
        entry
            .capture_usage_update_for_test()
            .durable_storage_byte_seconds_delta,
        160
    );
}

#[test]
fn larger_storage_amount_recovers_admission_and_stale_revision_cannot_undo_it() {
    let period = AccountUsagePeriod::current();
    let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 10, 20);
    entry.record_storage_byte_seconds(AgentMode::Durable, 10);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 20);
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());
    assert!(
        entry
            .monthly_resource_capacity(AgentMode::Ephemeral)
            .is_err()
    );

    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 20,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 40,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        8,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    assert!(
        entry
            .monthly_resource_capacity(AgentMode::Ephemeral)
            .is_ok()
    );

    assert!(!entry.apply_monthly_snapshot_for_test(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 0,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 0,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    assert!(
        entry
            .monthly_resource_capacity(AgentMode::Ephemeral)
            .is_ok()
    );
}

#[test]
fn new_month_recovers_storage_admission() {
    let period = AccountUsagePeriod::current();
    let next_period = if period.month == 12 {
        AccountUsagePeriod {
            year: period.year + 1,
            month: 1,
        }
    } else {
        AccountUsagePeriod {
            year: period.year,
            month: period.month + 1,
        }
    };
    let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 10, 20);
    entry.record_storage_byte_seconds(AgentMode::Durable, 10);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 20);

    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        MonthlyResourcePolicy {
            period: next_period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 10,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 20,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    assert!(
        entry
            .monthly_resource_capacity(AgentMode::Ephemeral)
            .is_ok()
    );
}

#[test]
fn allow_overage_recovers_storage_admission_and_stale_revision_cannot_undo_it() {
    let period = AccountUsagePeriod::current();
    let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 0, 0);

    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 0,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 0,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        8,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    assert!(
        entry
            .monthly_resource_capacity(AgentMode::Ephemeral)
            .is_ok()
    );

    assert!(!entry.apply_monthly_snapshot_for_test(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: 0,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: 0,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    assert!(
        entry
            .monthly_resource_capacity(AgentMode::Ephemeral)
            .is_ok()
    );
}

#[test]
fn hard_limit_preserves_memory_settlement_units_and_remainder() {
    let entry = memory_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        1,
    );
    entry.record_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 2,
            remainder: BYTE_NANOSECONDS_PER_GB_SECOND / 2,
        },
    );

    let captured = entry.capture_usage_update_for_test();
    assert_eq!(captured.memory_gb_seconds_delta, 2);
    assert_eq!(captured.memory_byte_nanoseconds_remainder, 0);
    entry.update_policy_revision(8);
    let remainder = entry.capture_usage_update_for_test();
    assert_eq!(remainder.memory_gb_seconds_delta, 0);
    assert_eq!(
        remainder.memory_byte_nanoseconds_remainder,
        (BYTE_NANOSECONDS_PER_GB_SECOND / 2) as u64
    );
}

#[test]
fn fractional_memory_settlements_combine_before_hard_limit_enforcement() {
    let entry = memory_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        1,
    );
    let three_fifths = BYTE_NANOSECONDS_PER_GB_SECOND * 3 / 5;

    entry.record_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 0,
            remainder: three_fifths,
        },
    );
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
    entry.record_memory_settlement(
        AgentMode::Ephemeral,
        ByteTimeSettlement {
            units: 0,
            remainder: three_fifths,
        },
    );

    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
    let captured = entry.capture_usage_update_for_test();
    assert_eq!(captured.memory_gb_seconds_delta, 1);
    assert_eq!(captured.memory_byte_nanoseconds_remainder, 0);
}

#[test]
fn revisioned_fractional_memory_remains_visible_to_hard_limit_enforcement() {
    let entry = memory_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        1,
    );
    let three_quarters = BYTE_NANOSECONDS_PER_GB_SECOND * 3 / 4;

    entry.record_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 0,
            remainder: three_quarters,
        },
    );
    entry.update_policy_revision(8);
    entry.record_memory_settlement(
        AgentMode::Ephemeral,
        ByteTimeSettlement {
            units: 0,
            remainder: three_quarters,
        },
    );

    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
    entry.update_policy_revision(9);
    let revision_7 = entry.capture_usage_update_for_test();
    let revision_8 = entry.capture_usage_update_for_test();
    assert_eq!(revision_7.monthly_policy_revision, 7);
    assert_eq!(
        revision_7.memory_byte_nanoseconds_remainder,
        three_quarters as u64
    );
    assert_eq!(revision_8.monthly_policy_revision, 8);
    assert_eq!(
        revision_8.memory_byte_nanoseconds_remainder,
        three_quarters as u64
    );
}

#[test]
fn confirmed_fractional_memory_preserves_exact_remaining_capacity() {
    let period = AccountUsagePeriod::current();
    let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 1);
    let three_quarters = BYTE_NANOSECONDS_PER_GB_SECOND * 3 / 4;
    let one_quarter = BYTE_NANOSECONDS_PER_GB_SECOND / 4;

    entry.record_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 0,
            remainder: three_quarters,
        },
    );
    entry.update_policy_revision(8);
    let captured = entry.capture_usage_update_for_test();
    entry.begin_monthly_refresh(1, &captured, 0, 0);

    assert!(entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 0,
            available_memory_byte_nanoseconds_remainder: one_quarter as u64,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        8,
        true,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

    entry.record_memory_settlement(
        AgentMode::Ephemeral,
        ByteTimeSettlement {
            units: 0,
            remainder: one_quarter,
        },
    );
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
}

#[test]
fn concurrent_memory_settlements_preserve_all_measured_usage() {
    let entry = Arc::new(memory_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        100,
    ));
    let recorders = (0..16)
        .map(|_| {
            let entry = entry.clone();
            std::thread::spawn(move || entry.record_memory_gb_seconds(AgentMode::Durable, 10))
        })
        .collect::<Vec<_>>();
    for recorder in recorders {
        recorder.join().unwrap();
    }

    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 160);
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
    assert_eq!(
        entry
            .capture_usage_update_for_test()
            .memory_gb_seconds_delta,
        160
    );
}

#[test]
fn every_local_delivery_state_reduces_memory_hard_limit_capacity() {
    let period = AccountUsagePeriod::current();
    let unsent = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
    unsent.record_memory_gb_seconds(AgentMode::Durable, 10);
    assert_eq!(unsent.effective_memory_gb_seconds(), 90);

    let in_flight = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
    in_flight.record_memory_gb_seconds(AgentMode::Durable, 10);
    in_flight.capture_usage_update_for_test();
    assert_eq!(in_flight.effective_memory_gb_seconds(), 90);

    let pending = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
    pending.record_memory_gb_seconds(AgentMode::Durable, 10);
    pending.update_policy_revision(8);
    assert_eq!(pending.effective_memory_gb_seconds(), 90);

    let failed = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
    failed.record_memory_gb_seconds(AgentMode::Durable, 10);
    failed.capture_usage_update_for_test();
    failed.begin_monthly_refresh_with_memory_for_test(1, 0, 10);
    assert!(failed.finish_ambiguous_transport_failure(1));
    assert_eq!(failed.effective_memory_gb_seconds(), 90);

    let stale = memory_entry(period, MonthlyUsageMode::HardLimit, 100);
    stale.record_memory_gb_seconds(AgentMode::Durable, 10);
    stale.capture_usage_update_for_test();
    stale.begin_monthly_refresh_with_memory_for_test(1, 0, 10);
    stale.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
    assert!(!stale.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 100,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(stale.effective_memory_gb_seconds(), 90);
}

#[test]
fn memory_refresh_acknowledges_only_its_own_in_flight_usage() {
    let period = AccountUsagePeriod::current();
    let entry = memory_entry(period, MonthlyUsageMode::AllowOverage, 100);

    entry.record_memory_gb_seconds(AgentMode::Durable, 10);
    entry.capture_usage_update_for_test();
    entry.begin_monthly_refresh_with_memory_for_test(1, 0, 10);

    entry.record_memory_gb_seconds(AgentMode::Durable, 20);
    entry.capture_usage_update_for_test();
    entry.begin_monthly_refresh_with_memory_for_test(2, 0, 20);

    assert!(entry.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 70,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(
        entry
            .in_flight_memory_gb_seconds_delta
            .load(Ordering::Acquire),
        10
    );
    assert_eq!(entry.effective_memory_gb_seconds(), 60);

    assert!(!entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(
        entry
            .in_flight_memory_gb_seconds_delta
            .load(Ordering::Acquire),
        0
    );
    assert_eq!(entry.effective_memory_gb_seconds(), 60);

    entry.begin_monthly_refresh_with_memory_for_test(3, 0, 0);
    assert!(entry.apply_monthly_snapshot(
        3,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 70,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(entry.effective_memory_gb_seconds(), 70);
}

#[test]
fn monthly_admission_flushes_resident_memory_before_deciding() {
    let entry = Arc::new(memory_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        10,
    ));
    let flusher: Arc<dyn ResourceUsageFlusher> = Arc::new(MemoryUsageFlusher {
        entry: Arc::downgrade(&entry),
        amount: Mutex::new(Some(10)),
    });
    entry.register_resource_usage_flusher(Arc::downgrade(&flusher));

    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 10);
}

#[test]
fn amount_mode_and_period_changes_restore_memory_admission() {
    let period = AccountUsagePeriod::current();
    let entry = memory_entry(period, MonthlyUsageMode::HardLimit, 10);
    entry.record_memory_gb_seconds(AgentMode::Durable, 10);
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );

    entry.begin_monthly_refresh_with_memory_for_test(1, 0, 0);
    assert!(entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 20,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

    entry.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
    assert!(entry.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 0,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        8,
        true,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

    let failed = memory_entry(period, MonthlyUsageMode::HardLimit, 10);
    failed.record_memory_gb_seconds(AgentMode::Durable, 10);
    failed.capture_usage_update_for_test();
    failed.begin_monthly_refresh_with_memory_for_test(1, 0, 10);
    assert!(failed.finish_ambiguous_transport_failure(1));
    assert_eq!(
        failed.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::Memory.into())
    );
    let next_period = if period.month == 12 {
        AccountUsagePeriod {
            year: period.year + 1,
            month: 1,
        }
    } else {
        AccountUsagePeriod {
            year: period.year,
            month: period.month + 1,
        }
    };
    failed.begin_monthly_refresh_with_memory_for_test(2, 0, 0);
    assert!(failed.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period: next_period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: 10,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        8,
        true,
    ));
    assert!(failed.monthly_resource_capacity(AgentMode::Durable).is_ok());
}

#[test]
fn memory_usage_advances_period_before_registry_refresh() {
    let current_period = AccountUsagePeriod::current();
    let previous_period = if current_period.month == 1 {
        AccountUsagePeriod {
            year: current_period.year - 1,
            month: 12,
        }
    } else {
        AccountUsagePeriod {
            year: current_period.year,
            month: current_period.month - 1,
        }
    };
    let entry = memory_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

    entry.record_memory_gb_seconds(AgentMode::Durable, 10);
    let captured = entry.capture_usage_update_for_test();

    assert_eq!(captured.period, current_period);
    assert_eq!(captured.memory_gb_seconds_delta, 10);
}

#[test]
fn memory_disabled_entry_has_no_memory_gate_or_accounting() {
    let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig {
            compute: true,
            memory: false,
            filesystem: false,
        },
    );

    assert!(
        entry
            .usage_revision_state
            .lock()
            .unwrap()
            .monthly_policy
            .as_ref()
            .unwrap()
            .available_memory_byte_nanoseconds
            .is_none()
    );
    assert!(entry.account_usage_accumulator.is_none());
    entry.record_memory_gb_seconds(AgentMode::Durable, 10);
    assert_eq!(entry.effective_memory_gb_seconds(), u64::MAX);
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());
}

#[test]
fn signed_refunds_reduce_unsent_and_failed_delivery_enforcement_usage() {
    let entry = compute_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        100,
    );
    assert!(entry.borrow_fuel(60));
    entry.return_fuel(20);
    assert_eq!(entry.effective_fuel(), 60);

    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    entry.begin_monthly_refresh_for_test(1, 40);
    assert!(entry.finish_ambiguous_transport_failure(1));
    assert_eq!(entry.effective_fuel(), 60);

    entry.return_fuel(20);
    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    entry.begin_monthly_refresh_for_test(2, -20);
    assert!(entry.finish_ambiguous_transport_failure(2));
    assert_eq!(entry.effective_fuel(), 80);
}

#[test]
fn concurrent_hard_limit_borrowers_cannot_exceed_capacity() {
    let entry = Arc::new(compute_entry(
        AccountUsagePeriod::current(),
        MonthlyUsageMode::HardLimit,
        100,
    ));
    let borrowers = (0..16)
        .map(|_| {
            let entry = entry.clone();
            std::thread::spawn(move || match entry.borrow_fuel_with_revision(10) {
                FuelBorrow::Borrowed { amount, .. } => amount,
                FuelBorrow::Exhausted { .. } => 0,
            })
        })
        .collect::<Vec<_>>();
    let borrowed = borrowers
        .into_iter()
        .map(|borrower| borrower.join().unwrap())
        .sum::<u64>();

    assert_eq!(borrowed, 100);
    assert_eq!(entry.fuel_delta(), 100);
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());
}

#[test]
fn stale_refresh_cannot_change_policy_or_revision() {
    let period = AccountUsagePeriod::current();
    let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 100);
    entry.begin_monthly_refresh_for_test(1, 0);
    assert!(entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: 0,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        8,
        true,
    ));
    assert!(!entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 1_000,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert!(entry.borrow_fuel(20));
    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    entry.begin_monthly_refresh_for_test(2, 20);

    assert!(!entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 1_000,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        1,
        true,
    ));
    let revision_state = entry.usage_revision_state.lock().unwrap();
    assert_eq!(revision_state.current_policy_revision, 8);
    assert_eq!(
        revision_state.monthly_policy.as_ref().unwrap().mode,
        MonthlyUsageMode::AllowOverage
    );
    drop(revision_state);
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 20);

    assert!(entry.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 0,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        9,
        true,
    ));
    entry.begin_monthly_refresh_for_test(3, 0);
    assert!(!entry.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        10,
        true,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());
}

#[test]
fn newer_generation_with_older_revision_cannot_restore_stale_storage_policy() {
    let period = AccountUsagePeriod::current();
    let entry = storage_entry(period, MonthlyUsageMode::HardLimit, 10, 20);
    entry.update_policy_revision(8);
    entry.record_storage_byte_seconds(AgentMode::Durable, 1);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 2);
    let captured = entry.capture_usage_update_for_test();
    entry.begin_monthly_refresh(1, &captured, 0, 0);

    assert!(!entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));

    let revision_state = entry.usage_revision_state.lock().unwrap();
    let gate = revision_state.monthly_policy.as_ref().unwrap();
    assert_eq!(revision_state.current_policy_revision, 8);
    assert_eq!(gate.mode, MonthlyUsageMode::HardLimit);
    assert_eq!(
        gate.available_durable_storage_byte_nanoseconds,
        Some(10_000_000_000)
    );
    assert_eq!(
        gate.available_ephemeral_storage_byte_nanoseconds,
        Some(20_000_000_000)
    );
    assert!(gate.in_flight_usage.is_empty());
    drop(revision_state);

    entry.record_storage_byte_seconds(AgentMode::Durable, 9);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 18);
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Durable),
        Err(MonthlyResourceExhaustion::DurableStorage.into())
    );
    assert_eq!(
        entry.monthly_resource_capacity(AgentMode::Ephemeral),
        Err(MonthlyResourceExhaustion::EphemeralStorage.into())
    );
}

#[test]
fn refresh_acknowledges_only_its_own_in_flight_fuel() {
    let period = AccountUsagePeriod::current();
    let entry = compute_entry(period, MonthlyUsageMode::AllowOverage, 100);

    assert!(entry.borrow_fuel(10));
    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    entry.begin_monthly_refresh_for_test(1, 10);

    assert!(entry.borrow_fuel(20));
    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    entry.begin_monthly_refresh_for_test(2, 20);

    assert!(entry.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 70,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 10);
    assert_eq!(entry.effective_fuel(), 60);

    assert!(!entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: u64::MAX,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
    assert_eq!(entry.effective_fuel(), 60);

    entry.begin_monthly_refresh_for_test(3, 0);
    assert!(entry.apply_monthly_snapshot(
        3,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 70,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(entry.effective_fuel(), 70);
}

#[test]
fn unapplied_registry_update_remains_charged_for_enforcement() {
    let period = AccountUsagePeriod::current();
    let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 100);
    assert!(entry.borrow_fuel(10));
    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    entry.begin_monthly_refresh_for_test(1, 10);

    assert!(entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 100,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        false,
    ));
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
    assert_eq!(entry.effective_fuel(), 90);
    assert!(
        entry
            .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
            .is_none(),
        "unapplied compute usage is retained only for enforcement"
    );
}

#[test]
fn amount_mode_and_period_changes_restore_compute_admission() {
    let period = AccountUsagePeriod::current();
    let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 10);
    assert!(entry.borrow_fuel(10));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());

    entry.begin_monthly_refresh_for_test(1, 0);
    assert!(entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 20,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

    entry.begin_monthly_refresh_for_test(2, 0);
    assert!(entry.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: 0,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        8,
        true,
    ));
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_ok());

    let failed = compute_entry(period, MonthlyUsageMode::HardLimit, 10);
    assert!(failed.borrow_fuel(10));
    failed
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    failed.begin_monthly_refresh_for_test(1, 10);
    assert!(failed.finish_ambiguous_transport_failure(1));
    assert!(
        failed
            .monthly_resource_capacity(AgentMode::Durable)
            .is_err()
    );
    let next_period = if period.month == 12 {
        AccountUsagePeriod {
            year: period.year + 1,
            month: 1,
        }
    } else {
        AccountUsagePeriod {
            year: period.year,
            month: period.month + 1,
        }
    };
    failed.begin_monthly_refresh_for_test(2, 0);
    assert!(failed.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period: next_period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 10,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert!(failed.monthly_resource_capacity(AgentMode::Durable).is_ok());
}

#[test]
fn period_rollover_keeps_pre_refresh_usage_in_its_accrual_period() {
    let period = AccountUsagePeriod::current();
    let next_period = if period.month == 12 {
        AccountUsagePeriod {
            year: period.year + 1,
            month: 1,
        }
    } else {
        AccountUsagePeriod {
            year: period.year,
            month: period.month + 1,
        }
    };
    let entry = compute_entry(period, MonthlyUsageMode::HardLimit, 100);
    assert!(entry.borrow_fuel(10));

    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        MonthlyResourcePolicy {
            period: next_period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 100,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
    ));
    assert_eq!(entry.effective_fuel(), 100);

    let previous_period_usage = entry.capture_usage_update_for_test();
    assert_eq!(previous_period_usage.period, period);
    assert_eq!(previous_period_usage.fuel_delta, 10);

    assert!(entry.borrow_fuel(5));
    let current_period_usage = entry.capture_usage_update_for_test();
    assert_eq!(current_period_usage.period, next_period);
    assert_eq!(current_period_usage.fuel_delta, 5);
}

#[test]
fn compute_usage_advances_period_before_registry_refresh() {
    let current_period = AccountUsagePeriod::current();
    let previous_period = if current_period.month == 1 {
        AccountUsagePeriod {
            year: current_period.year - 1,
            month: 12,
        }
    } else {
        AccountUsagePeriod {
            year: current_period.year,
            month: current_period.month - 1,
        }
    };
    let entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

    assert_eq!(
        entry.borrow_fuel_with_revision(10),
        FuelBorrow::Borrowed {
            amount: 10,
            revision: 7,
            generation: 0,
            period: current_period,
        }
    );
    let captured = entry.capture_usage_update_for_test();
    assert_eq!(captured.period, current_period);
    assert_eq!(captured.fuel_delta, 10);
}

#[test]
fn pre_refresh_rollover_borrows_share_the_remaining_capacity() {
    let current_period = AccountUsagePeriod::current();
    let previous_period = if current_period.month == 1 {
        AccountUsagePeriod {
            year: current_period.year - 1,
            month: 12,
        }
    } else {
        AccountUsagePeriod {
            year: current_period.year,
            month: current_period.month - 1,
        }
    };
    let entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

    assert!(entry.borrow_fuel(60));
    assert_eq!(entry.effective_fuel(), 40);
    assert_eq!(
        entry.borrow_fuel_with_revision(60),
        FuelBorrow::Borrowed {
            amount: 40,
            revision: 7,
            generation: 0,
            period: current_period,
        }
    );
    assert!(entry.monthly_resource_capacity(AgentMode::Durable).is_err());
}

#[test]
fn pre_refresh_rollover_in_flight_and_failed_usage_stays_charged() {
    let current_period = AccountUsagePeriod::current();
    let previous_period = if current_period.month == 1 {
        AccountUsagePeriod {
            year: current_period.year - 1,
            month: 12,
        }
    } else {
        AccountUsagePeriod {
            year: current_period.year,
            month: current_period.month - 1,
        }
    };
    let entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

    assert!(entry.borrow_fuel(60));
    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    assert_eq!(entry.effective_fuel(), 40);
    entry.begin_monthly_refresh_for_test(1, 60);
    assert_eq!(entry.effective_fuel(), 40);

    assert!(entry.finish_ambiguous_transport_failure(1));
    assert_eq!(entry.effective_fuel(), 40);
    assert_eq!(
        entry.borrow_fuel_with_revision(60),
        FuelBorrow::Borrowed {
            amount: 40,
            revision: 7,
            generation: 1,
            period: current_period,
        }
    );

    let pending_entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);
    assert!(pending_entry.borrow_fuel(60));
    pending_entry.update_policy_revision(8);
    assert_eq!(pending_entry.effective_fuel(), 40);
}

#[test]
fn newer_period_delivery_stays_charged_until_registry_reaches_that_period() {
    let current_period = AccountUsagePeriod::current();
    let previous_period = if current_period.month == 1 {
        AccountUsagePeriod {
            year: current_period.year - 1,
            month: 12,
        }
    } else {
        AccountUsagePeriod {
            year: current_period.year,
            month: current_period.month - 1,
        }
    };
    let entry = compute_entry(previous_period, MonthlyUsageMode::HardLimit, 100);

    assert!(entry.borrow_fuel(60));
    entry
        .capture_usage_update(NO_IDLE_REFRESH_THRESHOLD_SECS)
        .unwrap();
    entry.begin_monthly_refresh_for_test(1, 60);
    assert!(entry.apply_monthly_snapshot(
        1,
        MonthlyResourcePolicy {
            period: previous_period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 100,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(entry.effective_fuel(), 40);

    entry.begin_monthly_refresh_for_test(2, 0);
    assert!(entry.apply_monthly_snapshot(
        2,
        MonthlyResourcePolicy {
            period: current_period,
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 40,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
        true,
    ));
    assert_eq!(entry.effective_fuel(), 40);
}

#[test]
fn compute_disabled_entry_has_no_compute_gate_or_accounting() {
    let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
        0,
        20,
        30,
        40,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: true,
        },
    );
    assert!(
        entry
            .usage_revision_state
            .lock()
            .unwrap()
            .monthly_policy
            .as_ref()
            .unwrap()
            .available_fuel
            .is_none()
    );
    assert_eq!(
        entry.borrow_fuel_with_revision(123),
        FuelBorrow::Borrowed {
            amount: 123,
            revision: 0,
            generation: 0,
            period: AccountUsagePeriod::current(),
        }
    );
    assert_eq!(entry.delta.load(Ordering::Acquire), 0);
    let captured = entry.capture_usage_update(0).unwrap();
    assert_eq!(captured.update.fuel_delta, 0);
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
}

#[test]
fn compute_disabled_entry_advances_shared_usage_attribution() {
    let period = AccountUsagePeriod::current();
    let next_period = AccountUsagePeriod {
        year: period.year + 1,
        month: 1,
    };
    let entry = AtomicResourceEntry::new_with_all_limits_and_metering(
        0,
        20,
        30,
        40,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: true,
        },
    );

    assert!(entry.apply_monthly_snapshot_for_test(
        1,
        MonthlyResourcePolicy {
            period: next_period,
            mode: MonthlyUsageMode::AllowOverage,
            available_fuel: 0,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        },
        7,
    ));
    entry.record_resource_usage(AgentMode::Durable, 3, 5);
    let captured = entry.capture_usage_update_for_test();

    assert_eq!(captured.period, next_period);
    assert_eq!(captured.monthly_usage_mode_revision, 7);
    assert_eq!(captured.fuel_delta, 0);
    assert_eq!(captured.memory_gb_seconds_delta, 3);
    assert_eq!(captured.durable_storage_byte_seconds_delta, 5);
}

#[test]
fn max_memory_limit_returns_stored_value() {
    let entry = AtomicResourceEntry::new(0, 65536, usize::MAX, u64::MAX, u64::MAX);
    assert_eq!(entry.max_memory_limit(), 65536);
}

#[test]
fn last_refresh_secs_is_set_on_initialize() {
    let before = Utc::now().timestamp();
    let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, u64::MAX);
    let after = Utc::now().timestamp();
    let stored = entry.last_refresh_secs.load(Ordering::Acquire);
    assert!(stored >= before, "last_refresh_secs should be >= before");
    assert!(stored <= after, "last_refresh_secs should be <= after");
}

// -------------------------------------------------------------------------
// AtomicResourceEntry — table element limit
// -------------------------------------------------------------------------

#[test]
fn atomic_resource_entry_returns_table_elements_limit() {
    let entry = AtomicResourceEntry::new(1000, 65536, 500, u64::MAX, u64::MAX);
    assert_eq!(entry.max_table_elements_limit(), 500);
}

#[test]
fn atomic_resource_entry_table_elements_independent_of_memory() {
    let entry = AtomicResourceEntry::new(0, 1024, 256, u64::MAX, u64::MAX);
    assert_eq!(entry.max_memory_limit(), 1024);
    assert_eq!(entry.max_table_elements_limit(), 256);
}

#[test]
fn atomic_resource_entry_table_elements_usize_max_for_disabled() {
    let entry = AtomicResourceEntry::new(u64::MAX, usize::MAX, usize::MAX, u64::MAX, u64::MAX);
    assert_eq!(entry.max_table_elements_limit(), usize::MAX);
}

#[test]
fn atomic_resource_entry_table_elements_zero() {
    let entry = AtomicResourceEntry::new(100, 4096, 0, u64::MAX, u64::MAX);
    assert_eq!(entry.max_table_elements_limit(), 0);
}

// -------------------------------------------------------------------------
// AtomicResourceEntry — per-invocation limits
// -------------------------------------------------------------------------

#[test]
fn new_with_invocation_limits_stores_http_limit() {
    let entry = AtomicResourceEntry::new_with_invocation_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        42,
        u64::MAX,
    );
    assert_eq!(entry.per_invocation_http_call_limit(), 42);
}

#[test]
fn new_with_invocation_limits_stores_rpc_limit() {
    let entry = AtomicResourceEntry::new_with_invocation_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        99,
    );
    assert_eq!(entry.per_invocation_rpc_call_limit(), 99);
}

#[test]
fn new_defaults_invocation_limits_to_max() {
    // AtomicResourceEntry::new (without invocation limits) must default to u64::MAX
    // so that workers using the old constructor are unaffected.
    let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, u64::MAX);
    assert_eq!(entry.per_invocation_http_call_limit(), u64::MAX);
    assert_eq!(entry.per_invocation_rpc_call_limit(), u64::MAX);
}

#[test]
fn invocation_limits_can_be_updated_via_store() {
    let entry =
        AtomicResourceEntry::new_with_invocation_limits(500, 256, usize::MAX, u64::MAX, 10, 20);
    // Simulate a plan change: update limits via the atomic store
    entry
        .per_invocation_http_call_limit
        .store(50, Ordering::Release);
    entry
        .per_invocation_rpc_call_limit
        .store(100, Ordering::Release);
    assert_eq!(entry.per_invocation_http_call_limit(), 50);
    assert_eq!(entry.per_invocation_rpc_call_limit(), 100);
}

// -------------------------------------------------------------------------
// AtomicResourceEntry — monthly HTTP/RPC call tracking
// -------------------------------------------------------------------------

#[test]
fn update_last_known_limits_resets_syncing_and_refreshes_available() {
    // Simulate a batch response: syncing is cleared, available_from_server refreshed.
    let entry = AtomicResourceEntry::new_with_all_limits(
        0,
        0,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        5,
        5,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );
    // Simulate what send_batch does: move unsynced → syncing
    entry.syncing_http_calls.store(3, Ordering::Release);
    entry.syncing_rpc_calls.store(2, Ordering::Release);

    // Manually apply what update_last_known_limits does
    entry.syncing_http_calls.store(0, Ordering::Release);
    entry
        .available_http_calls_from_server
        .store(50, Ordering::Release);
    entry.syncing_rpc_calls.store(0, Ordering::Release);
    entry
        .available_rpc_calls_from_server
        .store(40, Ordering::Release);

    assert_eq!(entry.remaining_http_calls(), 50);
    assert_eq!(entry.remaining_rpc_calls(), 40);
}

#[test]
fn record_http_call_returns_false_when_budget_exhausted() {
    // 0 available; any call should fail immediately.
    let entry = AtomicResourceEntry::new_with_all_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        0,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );
    assert!(
        !entry.record_http_call(),
        "call with 0 available should return false"
    );
}

#[test]
fn record_http_call_exhausts_exactly_at_limit() {
    // 2 available; two calls succeed, third fails.
    let entry = AtomicResourceEntry::new_with_all_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        2,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );
    assert!(entry.record_http_call(), "first call should succeed");
    assert!(entry.record_http_call(), "second call should succeed");
    assert!(
        !entry.record_http_call(),
        "third call should fail — budget exhausted"
    );
}

#[test]
fn record_rpc_call_decrements_remaining_rpc_calls() {
    let entry = AtomicResourceEntry::new_with_all_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        3,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );
    assert!(entry.record_rpc_call());
    assert_eq!(entry.remaining_rpc_calls(), 2);
}

#[test]
fn record_rpc_call_returns_false_when_budget_exhausted() {
    let entry = AtomicResourceEntry::new_with_all_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        0,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );
    assert!(!entry.record_rpc_call());
}

#[test]
fn blob_storage_admission_is_atomic_and_deletions_release_capacity() {
    let entry = Arc::new(AtomicResourceEntry::new(
        u64::MAX,
        usize::MAX,
        usize::MAX,
        u64::MAX,
        u64::MAX,
    ));
    entry
        .blob_storage_quota
        .lock()
        .unwrap()
        .available_from_server = 10;

    let admitted = std::thread::scope(|scope| {
        let handles = (0..4)
            .map(|_| {
                let entry = entry.clone();
                scope.spawn(move || entry.try_record_blob_storage_delta(6))
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|admitted| *admitted)
            .count()
    });

    assert_eq!(admitted, 1);
    assert!(entry.try_record_blob_storage_delta(-4));
    assert!(entry.try_record_blob_storage_delta(8));
    assert!(!entry.try_record_blob_storage_delta(1));

    entry.rollback_blob_storage_delta(8);
    assert!(entry.try_record_blob_storage_delta(8));

    let full = AtomicResourceEntry::new(u64::MAX, usize::MAX, usize::MAX, u64::MAX, u64::MAX);
    full.set_available_blob_storage_bytes(10);
    assert!(full.try_record_blob_storage_delta(10));
    let captured = full.capture_usage_update(i64::MAX).unwrap();
    assert_eq!(captured.update.blob_storage_bytes_delta, 10);
    assert!(!full.try_record_blob_storage_delta(1));

    let deletion_credit =
        AtomicResourceEntry::new(u64::MAX, usize::MAX, usize::MAX, u64::MAX, u64::MAX);
    deletion_credit.set_available_blob_storage_bytes(0);
    assert!(deletion_credit.try_record_blob_storage_delta(-4));
    assert!(deletion_credit.try_record_blob_storage_delta(4));
}

#[test]
fn http_and_rpc_budgets_are_independent() {
    // HTTP exhausted, RPC still available.
    let entry = AtomicResourceEntry::new_with_all_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        0,
        5,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );
    assert!(!entry.record_http_call(), "HTTP should be exhausted");
    assert!(entry.record_rpc_call(), "RPC should still be available");
}

#[test]
fn unsynced_http_calls_accumulates_across_calls() {
    // Each record_http_call increments unsynced_http_calls by 1.
    let entry = AtomicResourceEntry::new_with_all_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        10,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );
    entry.record_http_call();
    entry.record_http_call();
    entry.record_http_call();
    // 3 calls made locally, not yet synced
    assert_eq!(entry.unsynced_http_calls.load(Ordering::Acquire), 3);
    // remaining = 10 - 3 - 0 = 7
    assert_eq!(entry.remaining_http_calls(), 7);
}

#[test]
fn moving_unsynced_to_syncing_preserves_remaining_http_calls() {
    // Start with 10 available and 3 unsynced local calls.
    let entry = AtomicResourceEntry::new_with_all_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        10,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );
    entry.unsynced_http_calls.store(3, Ordering::Release);
    assert_eq!(entry.remaining_http_calls(), 7);

    // Simulate send_batch's transfer: unsynced -> syncing.
    let moved = entry.unsynced_http_calls.swap(0, Ordering::AcqRel);
    entry
        .syncing_http_calls
        .try_update(Ordering::AcqRel, Ordering::Acquire, |c| {
            Some(c.saturating_add(moved))
        })
        .ok();

    // Remaining must stay unchanged while the batch is in flight.
    assert_eq!(entry.remaining_http_calls(), 7);
}

#[test]
fn clearing_syncing_does_not_clear_new_unsynced_calls() {
    let entry = AtomicResourceEntry::new_with_all_limits(
        1000,
        512,
        usize::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        10,
        u64::MAX,
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS,
        AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
    );

    // One call is included in the in-flight batch.
    entry.unsynced_http_calls.store(1, Ordering::Release);
    let moved = entry.unsynced_http_calls.swap(0, Ordering::AcqRel);
    entry
        .syncing_http_calls
        .try_update(Ordering::AcqRel, Ordering::Acquire, |c| {
            Some(c.saturating_add(moved))
        })
        .ok();

    // While request is in-flight, two new local calls are recorded.
    entry.unsynced_http_calls.fetch_add(2, Ordering::AcqRel);

    // Simulate successful response handling: clear syncing and refresh available.
    entry.syncing_http_calls.store(0, Ordering::Release);
    entry
        .available_http_calls_from_server
        .store(100, Ordering::Release);

    // New unsynced calls made during in-flight period must be preserved.
    assert_eq!(entry.unsynced_http_calls.load(Ordering::Acquire), 2);
    assert_eq!(entry.remaining_http_calls(), 98);
}

#[test]
async fn batch_success_refreshes_http_rpc_available_counts() {
    // After a successful send_batch, http_calls and rpc_calls should be
    // updated from the server response and in_flight cleared.
    let id = AccountId::SYSTEM;
    let mock = Arc::new(MockRegistryService::new(1000, 512));

    // Prime the entry with 5 available HTTP and 3 available RPC.
    mock.set_get_limits_response(ServiceResourceLimits {
        monthly_policy: monthly_policy(1000),
        max_memory_per_worker: 512,
        max_table_elements_per_worker: u64::MAX,
        max_disk_space_per_worker: u64::MAX,
        per_invocation_http_call_limit: u64::MAX,
        per_invocation_rpc_call_limit: u64::MAX,
        available_http_calls: 5,
        available_rpc_calls: 3,
        available_blob_storage_bytes: u64::MAX,
        max_concurrent_agents_per_executor: u64::MAX,
        oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        monthly_policy_status: golem_service_base::model::MonthlyResourcePolicyStatus::Available,
        usage_update_applied: true,
        monthly_usage_mode_revision: 0,
        monthly_policy_revision: 0,
    });

    let svc = make_grpc(mock.clone());
    let entry: Arc<AtomicResourceEntry> = svc.initialize_account(id).await.unwrap();

    // Record some calls to build up deltas.
    entry.record_http_call();
    entry.record_http_call();
    entry.record_rpc_call();

    // Server will respond with fresh counts.
    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(1000),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: 50,
            available_rpc_calls: 40,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    svc.send_batch(0).await;

    // After batch success remaining must reflect the server's fresh available count.
    assert_eq!(entry.remaining_http_calls(), 50);
    assert_eq!(entry.remaining_rpc_calls(), 40);
    // syncing buckets cleared, unsynced also zero (were swapped to syncing)
    assert_eq!(entry.syncing_http_calls.load(Ordering::Acquire), 0);
    assert_eq!(entry.syncing_rpc_calls.load(Ordering::Acquire), 0);
    assert_eq!(entry.unsynced_http_calls.load(Ordering::Acquire), 0);
    assert_eq!(entry.unsynced_rpc_calls.load(Ordering::Acquire), 0);
}

#[test]
async fn batch_failure_clears_http_rpc_in_flight_without_double_counting() {
    // On batch failure the in-flight deltas must be cleared so the next
    // tick doesn't double-count them.
    let id = account_id();
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_get_limits_response(ServiceResourceLimits {
        monthly_policy: monthly_policy(1000),
        max_memory_per_worker: 512,
        max_table_elements_per_worker: u64::MAX,
        max_disk_space_per_worker: u64::MAX,
        per_invocation_http_call_limit: u64::MAX,
        per_invocation_rpc_call_limit: u64::MAX,
        available_http_calls: 10,
        available_rpc_calls: 10,
        available_blob_storage_bytes: u64::MAX,
        max_concurrent_agents_per_executor: u64::MAX,
        oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        monthly_policy_status: golem_service_base::model::MonthlyResourcePolicyStatus::Available,
        usage_update_applied: true,
        monthly_usage_mode_revision: 0,
        monthly_policy_revision: 0,
    });
    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(1000),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: 10,
            available_rpc_calls: 10,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));
    mock.set_batch_update_error();

    let svc = make_grpc(mock.clone());
    let entry: Arc<AtomicResourceEntry> = svc.initialize_account(id).await.unwrap();
    entry.record_http_call();
    entry.record_rpc_call();

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    // After the batch error the syncing buckets must be zeroed.
    assert_eq!(
        entry.syncing_http_calls.load(Ordering::Acquire),
        0,
        "syncing_http_calls should be cleared on error"
    );
    assert_eq!(
        entry.syncing_rpc_calls.load(Ordering::Acquire),
        0,
        "syncing_rpc_calls should be cleared on error"
    );
}

#[test]
async fn resource_limits_disabled_returns_max_table_elements() {
    let disabled = ResourceLimitsDisabled;
    let entry = disabled
        .initialize_account(AccountId::SYSTEM)
        .await
        .expect("initialize_account should succeed");
    assert_eq!(entry.max_table_elements_limit(), usize::MAX);
}

// -------------------------------------------------------------------------
// AtomicResourceEntry — concurrent agent limit
// -------------------------------------------------------------------------

#[test]
fn concurrent_agent_limit_defaults_to_max_when_passing_u64_max() {
    let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, u64::MAX);
    assert_eq!(entry.max_concurrent_agents_per_executor(), u64::MAX);
}

#[test]
fn concurrent_agent_limit_is_stored_from_new() {
    let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, 5);
    assert_eq!(entry.max_concurrent_agents_per_executor(), 5);
}

#[test]
fn concurrent_agent_limit_zero_is_stored_correctly() {
    let entry = AtomicResourceEntry::new(0, 0, usize::MAX, u64::MAX, 0);
    assert_eq!(entry.max_concurrent_agents_per_executor(), 0);
}

#[test]
fn concurrent_agent_limit_can_be_updated_atomically() {
    let entry = AtomicResourceEntry::new(1000, 512, usize::MAX, u64::MAX, 5);
    entry.set_max_concurrent_agents_per_executor(10);
    assert_eq!(entry.max_concurrent_agents_per_executor(), 10);
}

#[test]
fn concurrent_agent_limit_is_independent_of_other_fields() {
    let entry = AtomicResourceEntry::new(500, 1024, 256, 4096, 7);
    assert_eq!(entry.max_concurrent_agents_per_executor(), 7);
    assert_eq!(entry.effective_fuel(), 500);
    assert_eq!(entry.max_memory_limit(), 1024);
    assert_eq!(entry.max_table_elements_limit(), 256);
    assert_eq!(entry.max_disk_space_limit(), 4096);
}

// -------------------------------------------------------------------------
// ResourceLimitsGrpc
// -------------------------------------------------------------------------

struct MockRegistryService {
    get_limits_result: Mutex<Result<ServiceResourceLimits, RegistryServiceError>>,
    batch_update_result: Mutex<Result<AccountResourceLimits, RegistryServiceError>>,
    delayed_batch_updates: Mutex<VecDeque<DelayedBatchUpdate>>,
    last_batch_updates: Mutex<HashMap<AccountId, ResourceUsageUpdate>>,
    batch_update_calls: AtomicUsize,
}

struct DelayedBatchUpdate {
    result: AccountResourceLimits,
    started: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

impl MockRegistryService {
    fn new(available_fuel: u64, max_memory: u64) -> Self {
        Self {
            get_limits_result: Mutex::new(Ok(ServiceResourceLimits {
                monthly_policy: monthly_policy(available_fuel),
                max_memory_per_worker: max_memory,
                max_table_elements_per_worker: u64::MAX,
                max_disk_space_per_worker: u64::MAX,
                per_invocation_http_call_limit: u64::MAX,
                per_invocation_rpc_call_limit: u64::MAX,
                available_http_calls: u64::MAX,
                available_rpc_calls: u64::MAX,
                available_blob_storage_bytes: u64::MAX,
                max_concurrent_agents_per_executor: u64::MAX,
                oplog_writes_per_second: u64::MAX,
                monthly_policy_status:
                    golem_service_base::model::MonthlyResourcePolicyStatus::Available,
                usage_update_applied: true,
                monthly_usage_mode_revision: 0,
                monthly_policy_revision: 0,
            })),
            batch_update_result: Mutex::new(Ok(AccountResourceLimits(HashMap::new()))),
            delayed_batch_updates: Mutex::new(VecDeque::new()),
            last_batch_updates: Mutex::new(HashMap::new()),
            batch_update_calls: AtomicUsize::new(0),
        }
    }

    fn set_get_limits_response(&self, limits: ServiceResourceLimits) {
        *self.get_limits_result.lock().unwrap() = Ok(limits);
    }

    fn set_get_limits_error(&self) {
        *self.get_limits_result.lock().unwrap() = Err(RegistryServiceError::InternalServerError(
            "mock error".into(),
        ));
    }

    fn set_batch_update_response(&self, limits: AccountResourceLimits) {
        *self.batch_update_result.lock().unwrap() = Ok(limits);
    }

    fn set_batch_update_error(&self) {
        *self.batch_update_result.lock().unwrap() = Err(RegistryServiceError::InternalServerError(
            "mock batch error".into(),
        ));
    }

    fn delay_next_batch_update(
        &self,
        result: AccountResourceLimits,
    ) -> (Arc<Semaphore>, Arc<Semaphore>) {
        let started = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        self.delayed_batch_updates
            .lock()
            .unwrap()
            .push_back(DelayedBatchUpdate {
                result,
                started: Arc::clone(&started),
                release: Arc::clone(&release),
            });
        (started, release)
    }

    fn last_batch_update(&self, account_id: AccountId) -> ResourceUsageUpdate {
        *self
            .last_batch_updates
            .lock()
            .unwrap()
            .get(&account_id)
            .unwrap()
    }
}

#[async_trait]
impl RegistryService for MockRegistryService {
    async fn resolve_mcp_import(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
        _: bool,
    ) -> Result<golem_service_base::model::mcp_import::McpImportObservation, RegistryServiceError>
    {
        panic!("unexpected MCP discovery")
    }
    async fn get_mcp_runtime_credential(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
    ) -> Result<golem_service_base::clients::registry::McpRuntimeCredential, RegistryServiceError>
    {
        panic!("unexpected MCP credential request")
    }
    async fn report_mcp_resource_unauthorized(
        &self,
        _: &golem_common::model::mcp_import::McpImportSource,
        _: &AuthCtx,
        _: Option<uuid::Uuid>,
    ) -> Result<(), RegistryServiceError> {
        panic!("unexpected MCP feedback")
    }
    async fn authenticate_token(
        &self,
        _token: &TokenSecret,
    ) -> Result<AuthCtx, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_resource_limits(
        &self,
        _account_id: AccountId,
        _metering: golem_service_base::clients::registry::ResourceUsageMetering,
    ) -> Result<ServiceResourceLimits, RegistryServiceError> {
        self.get_limits_result
            .lock()
            .unwrap()
            .clone()
            .map_err(|e| RegistryServiceError::InternalServerError(e.to_string()))
    }

    async fn update_worker_connection_limit(
        &self,
        _account_id: AccountId,
        _agent_id: &AgentId,
        _added: bool,
    ) -> Result<(), RegistryServiceError> {
        unimplemented!()
    }

    async fn batch_update_resource_usage(
        &self,
        updates: HashMap<AccountId, ResourceUsageUpdate>,
    ) -> Result<AccountResourceLimits, RegistryServiceError> {
        self.batch_update_calls.fetch_add(1, Ordering::AcqRel);
        *self.last_batch_updates.lock().unwrap() = updates;
        let delayed = self.delayed_batch_updates.lock().unwrap().pop_front();
        if let Some(delayed) = delayed {
            delayed.started.add_permits(1);
            delayed.release.acquire().await.unwrap().forget();
            return Ok(delayed.result);
        }
        self.batch_update_result
            .lock()
            .unwrap()
            .clone()
            .map_err(|e| RegistryServiceError::InternalServerError(e.to_string()))
    }

    async fn download_component(
        &self,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_component_metadata(
        &self,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_deployed_component_metadata(
        &self,
        _component_id: ComponentId,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_all_deployed_component_revisions(
        &self,
        _component_id: ComponentId,
    ) -> Result<Vec<Component>, RegistryServiceError> {
        unimplemented!()
    }

    async fn resolve_component(
        &self,
        _resolving_account_id: AccountId,
        _resolving_application_id: ApplicationId,
        _resolving_environment_id: EnvironmentId,
        _component_slug: &str,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_all_agent_types(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_agent_type(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _name: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }

    async fn resolve_agent_type_by_names(
        &self,
        _app_name: &ApplicationName,
        _environment_name: &EnvironmentName,
        _agent_type_name: &AgentTypeName,
        _deployment_revision: Option<DeploymentRevision>,
        _owner_account_email: Option<&str>,
        _auth_ctx: &AuthCtx,
    ) -> Result<ResolvedAgentType, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_active_routes_for_domain(
        &self,
        _domain: &Domain,
    ) -> Result<CompiledRoutes, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_active_compiled_mcps_for_domain(
        &self,
        _domain: &Domain,
    ) -> Result<CompiledMcp, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_current_environment_state(
        &self,
        _environment_id: EnvironmentId,
    ) -> Result<EnvironmentState, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_agent_secret_revision(
        &self,
        _environment_id: EnvironmentId,
        _agent_secret_id: golem_common::model::agent_secret::AgentSecretId,
        _path: golem_common::model::agent_secret::CanonicalAgentSecretPath,
        _revision: golem_common::model::agent_secret::AgentSecretRevision,
    ) -> Result<Option<golem_service_base::model::agent_secret::AgentSecret>, RegistryServiceError>
    {
        unimplemented!()
    }

    async fn get_resource_definition_by_id(
        &self,
        _resource_definition_id: ResourceDefinitionId,
    ) -> Result<ResourceDefinition, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_resource_definition_by_name(
        &self,
        _environment_id: EnvironmentId,
        _resource_name: ResourceName,
    ) -> Result<ResourceDefinition, RegistryServiceError> {
        unimplemented!()
    }

    async fn subscribe_registry_invalidations(
        &self,
        _last_seen_event_id: Option<u64>,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<
                            golem_common::model::agent::RegistryInvalidationEvent,
                            RegistryServiceError,
                        >,
                    > + Send,
            >,
        >,
        RegistryServiceError,
    > {
        unimplemented!()
    }

    async fn run_registry_invalidation_event_subscriber(
        &self,
        _service_name: &'static str,
        _shutdown_token: Option<tokio_util::sync::CancellationToken>,
        _handler: std::sync::Arc<
            dyn golem_service_base::clients::registry::RegistryInvalidationHandler,
        >,
    ) {
        unimplemented!()
    }
}

fn account_id() -> AccountId {
    AccountId(Uuid::new_v4())
}

// Threshold used in tests that want stale idle accounts to be picked up.
const STALE_THRESHOLD_SECS: i64 = 300;
// Threshold used in tests that want idle accounts to never be picked up.
const NO_IDLE_REFRESH_THRESHOLD_SECS: i64 = i64::MAX;

fn make_grpc(mock: Arc<MockRegistryService>) -> Arc<ResourceLimitsGrpc> {
    // Tests drive send_batch manually, without a competing background updater.
    Arc::new(ResourceLimitsGrpc {
        client: mock,
        entries: scc::HashMap::new(),
        metering: ResourceUsageMeteringConfig::all_enabled(),
        refresh_generation: AtomicU64::new(1),
    })
}

fn service_limits(
    monthly_policy: MonthlyResourcePolicy,
    monthly_usage_mode_revision: u64,
    max_memory_per_worker: u64,
    max_disk_space_per_worker: u64,
) -> ServiceResourceLimits {
    ServiceResourceLimits {
        available_blob_storage_bytes: u64::MAX,
        monthly_policy,
        max_memory_per_worker,
        max_table_elements_per_worker: u64::MAX,
        max_disk_space_per_worker,
        per_invocation_http_call_limit: u64::MAX,
        per_invocation_rpc_call_limit: u64::MAX,
        available_http_calls: u64::MAX,
        available_rpc_calls: u64::MAX,
        max_concurrent_agents_per_executor: u64::MAX,
        oplog_writes_per_second: u64::MAX,
        monthly_policy_status: golem_service_base::model::MonthlyResourcePolicyStatus::Available,
        usage_update_applied: true,
        monthly_usage_mode_revision,
        monthly_policy_revision: monthly_usage_mode_revision,
    }
}

fn account_limits(account_id: AccountId, limits: ServiceResourceLimits) -> AccountResourceLimits {
    AccountResourceLimits(HashMap::from([(account_id, limits)]))
}

fn assert_monthly_resource_admission(
    entry: &AtomicResourceEntry,
    durable: MonthlyResourceAdmission,
    ephemeral: MonthlyResourceAdmission,
) {
    assert_eq!(
        monthly_resource_admission(entry, AgentMode::Durable),
        durable
    );
    assert_eq!(
        monthly_resource_admission(entry, AgentMode::Ephemeral),
        ephemeral
    );
}

#[test]
async fn initialize_account_fetches_limits_from_registry() {
    let mock = Arc::new(MockRegistryService::new(5000, 1024));
    let svc = make_grpc(mock);
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();

    assert_eq!(entry.effective_fuel(), 5000);
    assert_eq!(entry.max_memory_limit(), 1024);
}

#[test]
async fn all_disabled_production_service_skips_monthly_work_and_keeps_per_agent_limits() {
    let account_id = account_id();
    let mock = Arc::new(MockRegistryService::new(0, 4_096));
    mock.set_get_limits_response(service_limits(
        MonthlyResourcePolicy {
            available_memory_gb_seconds: 0,
            available_durable_storage_byte_seconds: 0,
            available_ephemeral_storage_byte_seconds: 0,
            ..monthly_policy(0)
        },
        0,
        4_096,
        8_192,
    ));
    let token = CancellationToken::new();
    token.cancel();
    let client: Arc<dyn RegistryService> = mock.clone();
    let svc = ResourceLimitsGrpc::new(
        client,
        Duration::from_secs(3600),
        Duration::from_secs(300),
        ResourceUsageMeteringConfig::default(),
        token,
    );

    let entry = svc.initialize_account(account_id).await.unwrap();
    assert!(
        entry
            .usage_revision_state
            .lock()
            .unwrap()
            .monthly_policy
            .is_none()
    );
    assert!(entry.account_usage_accumulator.is_none());
    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Admit,
        MonthlyResourceAdmission::Admit,
    );
    assert!(entry.borrow_fuel(u64::MAX));
    entry.record_memory_gb_seconds(AgentMode::Durable, i64::MAX);
    entry.record_storage_byte_seconds(AgentMode::Durable, i64::MAX);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, i64::MAX);

    let memory_target = Arc::new(RecordingMemoryLimitTarget {
        enforced: AtomicU64::new(0),
    });
    let memory_target_dyn: Arc<dyn AgentMemoryLimitTarget> = memory_target.clone();
    entry.register_agent_memory_limit_target(Arc::downgrade(&memory_target_dyn));
    let observed_disk_limits = Arc::new(Mutex::new(Vec::new()));
    let _filesystem_registration = entry.register_agent_filesystem_limit_target(
        OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "all-disabled-managed-xfs".to_string(),
            },
        ),
        {
            let observed_disk_limits = Arc::clone(&observed_disk_limits);
            move |limit| {
                let observed_disk_limits = Arc::clone(&observed_disk_limits);
                Box::pin(async move {
                    observed_disk_limits.lock().unwrap().push(limit);
                    Ok(())
                })
            }
        },
    );

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert_eq!(mock.batch_update_calls.load(Ordering::Acquire), 0);
    assert_eq!(entry.max_memory_limit(), 4_096);
    assert_eq!(entry.max_disk_space_limit(), 8_192);
    entry.update_memory_limit(2_048);
    entry.apply_agent_filesystem_limit(4_096).await.unwrap();
    assert_eq!(entry.max_memory_limit(), 2_048);
    assert_eq!(entry.max_disk_space_limit(), 4_096);
    assert_eq!(memory_target.enforced.load(Ordering::Acquire), 2_048);
    assert_eq!(*observed_disk_limits.lock().unwrap(), vec![4_096]);
}

async fn periodic_refresh_with_pending_filesystem_reply(changed_limit: bool) {
    let id = account_id();
    let other_id = account_id();
    let mock = Arc::new(MockRegistryService::new(0, 8_192));
    let overage = MonthlyResourcePolicy {
        mode: MonthlyUsageMode::AllowOverage,
        available_memory_gb_seconds: 0,
        ..monthly_policy(0)
    };
    mock.set_get_limits_response(service_limits(overage.clone(), 0, 8_192, 16_384));
    let first_disk_limit = if changed_limit { 8_192 } else { 16_384 };
    mock.set_batch_update_response(AccountResourceLimits(HashMap::from([
        (
            id,
            service_limits(overage.clone(), 1, 8_192, first_disk_limit),
        ),
        (other_id, service_limits(overage.clone(), 1, 8_192, 16_384)),
    ])));
    let shutdown = CancellationToken::new();
    let svc = ResourceLimitsGrpc::new(
        mock.clone(),
        Duration::from_millis(20),
        Duration::ZERO,
        ResourceUsageMeteringConfig {
            compute: false,
            memory: true,
            filesystem: false,
        },
        shutdown.clone(),
    );
    let entry = svc.initialize_account(id).await.unwrap();
    let other = svc.initialize_account(other_id).await.unwrap();
    let mut updates = entry.subscribe_capacity_updates();
    let mut other_updates = other.subscribe_capacity_updates();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (completed_tx, mut completed_rx) = tokio::sync::mpsc::unbounded_channel();
    let registration = entry.register_agent_filesystem_limit_target(
        OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "periodic-refresh-pending-filesystem-reply".to_string(),
            },
        ),
        {
            let entered = entered.clone();
            let release = release.clone();
            move |limit| {
                let entered = entered.clone();
                let release = release.clone();
                let completed_tx = completed_tx.clone();
                Box::pin(async move {
                    entered.add_permits(1);
                    release.acquire().await.unwrap().forget();
                    let _ = completed_tx.send(limit);
                    Ok(())
                })
            }
        },
    );
    tokio::time::timeout(Duration::from_secs(2), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Admit,
        MonthlyResourceAdmission::Admit,
    );
    let final_disk_limit = if changed_limit { 4_096 } else { 16_384 };
    let hard_limit = MonthlyResourcePolicy {
        mode: MonthlyUsageMode::HardLimit,
        ..overage
    };
    mock.set_batch_update_response(AccountResourceLimits(HashMap::from([
        (
            id,
            service_limits(hard_limit.clone(), 2, 4_096, final_disk_limit),
        ),
        (other_id, service_limits(hard_limit, 2, 4_096, 16_384)),
    ])));
    let refreshed = tokio::time::timeout(Duration::from_secs(2), async {
        while entry
            .usage_revision_state
            .lock()
            .unwrap()
            .current_policy_revision
            != 2
        {
            updates.changed().await.unwrap();
        }
        while other
            .usage_revision_state
            .lock()
            .unwrap()
            .current_policy_revision
            != 2
        {
            other_updates.changed().await.unwrap();
        }
        assert_monthly_resource_admission(
            &entry,
            MonthlyResourceAdmission::Suspend,
            MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Memory),
        );
        assert_monthly_resource_admission(
            &other,
            MonthlyResourceAdmission::Suspend,
            MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Memory),
        );
        assert_eq!(entry.max_memory_limit(), 4_096);
        assert_eq!(entry.max_disk_space_limit(), final_disk_limit);
        assert!(matches!(
            completed_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    })
    .await;
    shutdown.cancel();
    release.add_permits(8);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), completed_rx.recv())
            .await
            .unwrap(),
        Some(first_disk_limit)
    );
    drop(registration);
    assert!(
        refreshed.is_ok(),
        "the production periodic refresher must apply the next policy to both accounts before the filesystem reply completes"
    );
}

#[test]
#[timeout("10s")]
async fn periodic_policy_refresh_continues_while_unchanged_filesystem_reply_is_pending() {
    periodic_refresh_with_pending_filesystem_reply(false).await;
}

#[test]
#[timeout("10s")]
async fn periodic_policy_refresh_continues_while_changed_filesystem_reply_is_pending() {
    periodic_refresh_with_pending_filesystem_reply(true).await;
}

#[test]
async fn production_refresh_updates_shared_entry_in_both_directions() {
    let account_id = account_id();
    let mock = Arc::new(MockRegistryService::new(0, usize::MAX as u64));
    let svc = make_grpc(Arc::clone(&mock));
    let entry = svc.initialize_account(account_id).await.unwrap();
    let shared_entry = svc.initialize_account(account_id).await.unwrap();
    assert!(Arc::ptr_eq(&entry, &shared_entry));
    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Suspend,
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
    );

    mock.set_batch_update_response(account_limits(
        account_id,
        service_limits(monthly_policy(1), 0, usize::MAX as u64, u64::MAX),
    ));
    svc.send_batch(0).await;
    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Admit,
        MonthlyResourceAdmission::Admit,
    );

    mock.set_batch_update_response(account_limits(
        account_id,
        service_limits(monthly_policy(0), 0, usize::MAX as u64, u64::MAX),
    ));
    svc.send_batch(0).await;
    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Suspend,
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
    );
}

#[test]
async fn production_refresh_updates_existing_per_agent_limit_targets() {
    let account_id = account_id();
    let mock = Arc::new(MockRegistryService::new(1, 8_192));
    mock.set_get_limits_response(service_limits(monthly_policy(1), 0, 8_192, 16_384));
    let svc = make_grpc(Arc::clone(&mock));
    let entry = svc.initialize_account(account_id).await.unwrap();
    let memory_target = Arc::new(RecordingMemoryLimitTarget {
        enforced: AtomicU64::new(0),
    });
    let memory_target_dyn: Arc<dyn AgentMemoryLimitTarget> = memory_target.clone();
    entry.register_agent_memory_limit_target(Arc::downgrade(&memory_target_dyn));
    let observed_disk_limits = Arc::new(Mutex::new(Vec::new()));
    let (applied, mut applied_limits) = tokio::sync::mpsc::unbounded_channel();
    let _filesystem_registration = entry.register_agent_filesystem_limit_target(
        OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "refresh-existing-limit-targets".to_string(),
            },
        ),
        {
            let observed_disk_limits = Arc::clone(&observed_disk_limits);
            move |limit| {
                let observed_disk_limits = Arc::clone(&observed_disk_limits);
                let applied = applied.clone();
                Box::pin(async move {
                    observed_disk_limits.lock().unwrap().push(limit);
                    let _ = applied.send(limit);
                    Ok(())
                })
            }
        },
    );
    mock.set_batch_update_response(account_limits(
        account_id,
        service_limits(monthly_policy(1), 0, 4_096, 8_192),
    ));

    svc.send_batch(0).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), applied_limits.recv())
            .await
            .unwrap(),
        Some(8_192)
    );

    assert_eq!(entry.max_memory_limit(), 4_096);
    assert_eq!(entry.max_disk_space_limit(), 8_192);
    assert_eq!(memory_target.enforced.load(Ordering::Acquire), 4_096);
    assert_eq!(*observed_disk_limits.lock().unwrap(), vec![8_192]);
}

#[test]
async fn monthly_refresh_finishes_before_pending_filesystem_limit_target() {
    let account_id = account_id();
    let mock = Arc::new(MockRegistryService::new(1, 8_192));
    mock.set_get_limits_response(service_limits(monthly_policy(1), 0, 8_192, 16_384));
    let svc = make_grpc(Arc::clone(&mock));
    let entry = svc.initialize_account(account_id).await.unwrap();
    let mut updates = entry.subscribe_capacity_updates();
    assert_eq!(
        monthly_resource_admission(&entry, AgentMode::Durable),
        MonthlyResourceAdmission::Admit
    );
    assert_eq!(entry.capacity_updates.receiver_count(), 1);
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let completed = Arc::new(tokio::sync::Semaphore::new(0));
    let _registration = entry.register_agent_filesystem_limit_target(
        OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "unchanged-filesystem-limit-capacity-watch".to_string(),
            },
        ),
        {
            let entered = entered.clone();
            let release = release.clone();
            let completed = completed.clone();
            move |limit| {
                let entered = entered.clone();
                let release = release.clone();
                let completed = completed.clone();
                Box::pin(async move {
                    assert_eq!(limit, 16_384);
                    entered.add_permits(1);
                    release.acquire().await.unwrap().forget();
                    completed.add_permits(1);
                    Ok(())
                })
            }
        },
    );
    mock.set_batch_update_response(account_limits(
        account_id,
        service_limits(monthly_policy(0), 0, 8_192, 16_384),
    ));
    let refresh = tokio::spawn({
        let svc = svc.clone();
        async move { svc.send_batch(0).await }
    });
    entered.acquire().await.unwrap().forget();
    updates.changed().await.unwrap();
    assert_eq!(
        monthly_resource_admission(&entry, AgentMode::Durable),
        MonthlyResourceAdmission::Suspend
    );
    tokio::time::timeout(Duration::from_secs(2), refresh)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        completed.available_permits(),
        0,
        "batch completion must not claim filesystem completion"
    );
    drop(updates);
    assert_eq!(entry.capacity_updates.receiver_count(), 0);
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), completed.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

#[test]
#[timeout("10s")]
async fn queued_filesystem_limits_coalesce_without_overwriting_latest_cached_limit() {
    let entry = Arc::new(AtomicResourceEntry::new(1, 8_192, usize::MAX, 16_384, 1));
    let release = Arc::new(Semaphore::new(0));
    let (entered, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let (completed, mut completions) = tokio::sync::mpsc::unbounded_channel();
    let registration = entry.register_agent_filesystem_limit_target(
        OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "coalesced-filesystem-limit".to_string(),
            },
        ),
        {
            let release = release.clone();
            move |limit| {
                let release = release.clone();
                let entered = entered.clone();
                let completed = completed.clone();
                Box::pin(async move {
                    let _ = entered.send(limit);
                    release.acquire().await.unwrap().forget();
                    let _ = completed.send(limit);
                    Ok(())
                })
            }
        },
    );
    let id = account_id();
    entry.queue_agent_filesystem_limit(id, 8_192);
    assert_eq!(requests.recv().await, Some(8_192));
    entry.queue_agent_filesystem_limit(id, 4_096);
    entry.queue_agent_filesystem_limit(id, 2_048);
    assert_eq!(entry.max_disk_space_limit(), 2_048);
    assert!(matches!(
        requests.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    release.add_permits(1);
    assert_eq!(completions.recv().await, Some(8_192));
    assert_eq!(requests.recv().await, Some(2_048));
    assert_eq!(
        entry.max_disk_space_limit(),
        2_048,
        "an older physical update cannot overwrite the desired cached limit"
    );
    release.add_permits(1);
    assert_eq!(completions.recv().await, Some(2_048));
    let weak = Arc::downgrade(&entry);
    drop(registration);
    drop(entry);
    tokio::time::timeout(Duration::from_secs(2), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[test]
#[timeout("10s")]
async fn next_filesystem_refresh_retries_same_limit_after_target_failure() {
    let entry = Arc::new(AtomicResourceEntry::new(1, 8_192, usize::MAX, 16_384, 1));
    let attempts = Arc::new(AtomicUsize::new(0));
    let (completed, mut completions) = tokio::sync::mpsc::unbounded_channel();
    let _registration = entry.register_agent_filesystem_limit_target(
        OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "retry-filesystem-limit-on-next-refresh".to_string(),
            },
        ),
        move |limit| {
            let completed = completed.clone();
            let attempts = attempts.clone();
            Box::pin(async move {
                let attempt = attempts.fetch_add(1, Ordering::AcqRel);
                let _ = completed.send((attempt, limit));
                if attempt == 0 {
                    Err(WorkerExecutorError::runtime(
                        "filesystem limit update failed",
                    ))
                } else {
                    Ok(())
                }
            })
        },
    );
    let id = account_id();
    entry.queue_agent_filesystem_limit(id, 8_192);
    assert_eq!(completions.recv().await, Some((0, 8_192)));
    entry.queue_agent_filesystem_limit(id, 8_192);
    assert_eq!(completions.recv().await, Some((1, 8_192)));
    assert_eq!(entry.max_disk_space_limit(), 8_192);
}

#[test]
async fn delayed_older_refresh_cannot_restore_admission() {
    let account_id = account_id();
    let mock = Arc::new(MockRegistryService::new(1, usize::MAX as u64));
    let svc = make_grpc(Arc::clone(&mock));
    let entry = svc.initialize_account(account_id).await.unwrap();

    let (older_started, release_older) = mock.delay_next_batch_update(account_limits(
        account_id,
        service_limits(monthly_policy(1), 0, usize::MAX as u64, u64::MAX),
    ));
    let older_refresh = {
        let svc = Arc::clone(&svc);
        tokio::spawn(async move { svc.send_batch(0).await })
    };
    older_started.acquire().await.unwrap().forget();

    mock.set_batch_update_response(account_limits(
        account_id,
        service_limits(monthly_policy(0), 0, usize::MAX as u64, u64::MAX),
    ));
    svc.send_batch(0).await;
    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Suspend,
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
    );

    release_older.add_permits(1);
    older_refresh.await.unwrap();
    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Suspend,
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
    );
}

#[test]
async fn owner_overage_refresh_recovers_durable_and_ephemeral_admission() {
    let account_id = account_id();
    let mock = Arc::new(MockRegistryService::new(0, usize::MAX as u64));
    let svc = make_grpc(Arc::clone(&mock));
    let entry = svc.initialize_account(account_id).await.unwrap();
    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Suspend,
        MonthlyResourceAdmission::FailInvocation(MonthlyResourceExhaustion::Compute),
    );

    let mut owner_overage = monthly_policy(0);
    owner_overage.mode = MonthlyUsageMode::AllowOverage;
    mock.set_batch_update_response(account_limits(
        account_id,
        service_limits(owner_overage, 1, usize::MAX as u64, u64::MAX),
    ));

    svc.send_batch(0).await;

    assert_monthly_resource_admission(
        &entry,
        MonthlyResourceAdmission::Admit,
        MonthlyResourceAdmission::Admit,
    );
}

#[test]
async fn initialize_account_same_account_returns_shared_entry() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock);
    let id = account_id();

    let entry1 = svc.initialize_account(id).await.unwrap();
    let entry2 = svc.initialize_account(id).await.unwrap();

    // Both arcs must point to the exact same allocation
    assert!(Arc::ptr_eq(&entry1, &entry2));
}

#[test]
async fn initialize_account_different_accounts_return_different_entries() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock);

    let entry1 = svc.initialize_account(account_id()).await.unwrap();
    let entry2 = svc.initialize_account(account_id()).await.unwrap();

    assert!(!Arc::ptr_eq(&entry1, &entry2));
}

#[test]
async fn initialize_account_propagates_registry_error() {
    let mock = Arc::new(MockRegistryService::new(0, 0));
    mock.set_get_limits_error();
    let svc = make_grpc(mock);

    let result = svc.initialize_account(account_id()).await;
    assert!(result.is_err());
}

/// One span per tick, not one for the lifetime of the batch loop.
#[test]
async fn send_batch_records_one_closed_span_when_it_sends_a_batch(tracing: &Tracing) {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock);
    let entry = svc.initialize_account(account_id()).await.unwrap();
    entry.borrow_fuel(300);

    let recorder = crate::span_test_support::record_spans(tracing);
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    recorder.assert_closed_span("resource_limits_batch_update");
    recorder.assert_all_closed();
}

/// An idle tick is still spanned: discovering that there is nothing to send is
/// itself work that can fail, and events recorded outside a span never reach
/// the trace.
#[test]
async fn send_batch_records_one_closed_span_when_there_is_nothing_to_send(tracing: &Tracing) {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock);
    let _ = svc.initialize_account(account_id()).await.unwrap();

    let recorder = crate::span_test_support::record_spans(tracing);
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    recorder.assert_closed_span("resource_limits_batch_update");
}

#[test]
async fn send_batch_does_nothing_when_no_consumption_and_no_stale_accounts() {
    // No borrows, entry is freshly initialised (last_refresh_secs = now).
    // send_batch with a large threshold should produce no server call.
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock);
    let id = account_id();

    let _ = svc.initialize_account(id).await.unwrap();
    // Large threshold → not stale; no delta → not active.
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    // Nothing changed — no panic, no server call expected.
}

#[test]
async fn send_batch_treats_storage_only_delta_as_activity() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock.clone());
    let id = account_id();
    let entry = svc.initialize_account(id).await.unwrap();
    entry.record_storage_byte_seconds(AgentMode::Durable, 100);

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    assert_eq!(
        mock.last_batch_update(id)
            .durable_storage_byte_seconds_delta,
        100
    );
}

#[test]
fn storage_remainders_do_not_cross_agent_modes() {
    let entry = AtomicResourceEntry::new(0, 0, 0, 0, 0);

    entry.record_storage_remainder(AgentMode::Durable, 600_000_000);
    entry.record_storage_remainder(AgentMode::Ephemeral, 600_000_000);

    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    assert_eq!(entry.ephemeral_byte_seconds_delta(), 0);

    entry.record_storage_remainder(AgentMode::Durable, 400_000_000);
    entry.record_storage_remainder(AgentMode::Ephemeral, 400_000_000);

    assert_eq!(entry.durable_byte_seconds_delta(), 1);
    assert_eq!(entry.ephemeral_byte_seconds_delta(), 1);
}

#[test]
async fn send_batch_sends_storage_only_delta_when_limits_are_stale() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();
    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(1000),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));
    let svc = make_grpc(mock.clone());
    let entry = svc.initialize_account(id).await.unwrap();
    entry.last_refresh_secs.store(0, Ordering::Release);
    entry.record_storage_byte_seconds(AgentMode::Durable, 100);

    svc.send_batch(STALE_THRESHOLD_SECS).await;

    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    assert_eq!(
        mock.last_batch_update(id)
            .durable_storage_byte_seconds_delta,
        100
    );
}

#[test]
async fn send_batch_captures_active_delta_and_zeroes_it() {
    // After borrow_fuel(300): delta = +300.
    // send_batch must swap delta to 0 and include the 300 in the batch.
    // We verify that delta is zeroed; in_flight is cleared only if the server
    // returns a limit update for the account.
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();

    // Server returns updated limits for the account so in_flight is also cleared.
    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(700),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock.clone());
    let entry = svc.initialize_account(id).await.unwrap();
    entry.borrow_fuel(300);
    entry.record_storage_byte_seconds(AgentMode::Durable, 100);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 200);
    entry.record_memory_gb_seconds(AgentMode::Durable, 3);
    entry.record_memory_gb_seconds(AgentMode::Ephemeral, 4);

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert_eq!(entry.delta.load(Ordering::Acquire), 0);
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    assert_eq!(entry.ephemeral_byte_seconds_delta(), 0);
    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Ephemeral), 0);
    assert_eq!(
        entry
            .in_flight_memory_gb_seconds_delta
            .load(Ordering::Acquire),
        0
    );
    let update = mock.last_batch_update(id);
    assert_eq!(update.memory_gb_seconds_delta, 7);
    assert_eq!(update.durable_storage_byte_seconds_delta, 100);
    assert_eq!(update.ephemeral_storage_byte_seconds_delta, 200);
    assert_eq!(
        crate::metrics::resources::memory_gb_seconds_total(&id.to_string(), AgentMode::Durable,),
        3.0
    );
    assert_eq!(
        crate::metrics::resources::memory_gb_seconds_total(&id.to_string(), AgentMode::Ephemeral,),
        4.0
    );
}

#[test]
async fn registry_policy_revision_is_used_by_the_next_batch_without_changing_mode_revision() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();
    let mut current_limits = mock.get_limits_result.lock().unwrap().clone().unwrap();
    current_limits.monthly_policy_revision = 1;
    let mut updated = HashMap::new();
    updated.insert(id, current_limits);
    mock.set_batch_update_response(AccountResourceLimits(updated));
    let svc = make_grpc(mock.clone());
    let entry = svc.initialize_account(id).await.unwrap();

    assert!(entry.borrow_fuel(100));
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
    assert_eq!(mock.last_batch_update(id).monthly_usage_mode_revision, 0);
    assert_eq!(mock.last_batch_update(id).monthly_policy_revision, 0);

    assert!(entry.borrow_fuel(100));
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
    assert_eq!(mock.last_batch_update(id).monthly_usage_mode_revision, 0);
    assert_eq!(mock.last_batch_update(id).monthly_policy_revision, 1);
}

#[test]
async fn send_batch_success_refreshes_fuel_and_clears_in_flight() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();

    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(600),
            max_memory_per_worker: 1024,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();
    entry.borrow_fuel(400);

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert_eq!(entry.effective_fuel(), 600);
    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
    assert_eq!(entry.max_memory.load(Ordering::Acquire), 1024);
}

#[test]
async fn send_batch_success_effective_fuel_reflects_server_value() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();

    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(700),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();
    entry.borrow_fuel(200);

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert_eq!(entry.effective_fuel(), 700);
}

#[test]
async fn send_batch_failure_clears_in_flight_without_updating_fuel() {
    // On failure: in_flight_delta is zeroed; fuel stays at the old value.
    // The consumed fuel for this interval is lost (not retried).
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_batch_update_error();
    let svc = make_grpc(mock);
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    entry.borrow_fuel(300);
    entry.record_storage_byte_seconds(AgentMode::Durable, 100);
    entry.record_storage_byte_seconds(AgentMode::Ephemeral, 200);
    entry.record_memory_gb_seconds(AgentMode::Durable, 5);

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert_eq!(entry.in_flight_delta.load(Ordering::Acquire), 0);
    assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
    assert_eq!(entry.durable_byte_seconds_delta(), 0);
    assert_eq!(entry.ephemeral_byte_seconds_delta(), 0);
    assert_eq!(
        entry
            .in_flight_memory_gb_seconds_delta
            .load(Ordering::Acquire),
        0
    );
    assert_eq!(entry.effective_fuel(), 700);
    assert_eq!(
        crate::metrics::resources::memory_gb_seconds_total(&id.to_string(), AgentMode::Durable,),
        0.0
    );
}

#[test]
async fn send_batch_failure_retires_remainder_only_memory_delivery() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_batch_update_error();
    let svc = make_grpc(mock);
    let id = account_id();
    let entry = svc.initialize_account(id).await.unwrap();
    let remainder = BYTE_NANOSECONDS_PER_GB_SECOND / 2;

    entry.record_memory_settlement(
        AgentMode::Durable,
        ByteTimeSettlement {
            units: 0,
            remainder,
        },
    );
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    let revision_state = entry.usage_revision_state.lock().unwrap();
    let gate = revision_state.monthly_policy.as_ref().unwrap();
    assert!(gate.in_flight_usage.is_empty());
}

#[test]
async fn send_batch_failure_does_not_double_count_on_next_cycle() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_batch_update_error();
    let svc = make_grpc(mock.clone());
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    entry.borrow_fuel(300);
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await; // fails; 300 is lost

    // New borrows in the second interval
    entry.borrow_fuel(200);
    // delta must only contain the 200, not 300 + 200
    assert_eq!(entry.delta.load(Ordering::Acquire), 200);
}

#[test]
async fn stale_monthly_response_settles_blob_without_installing_stale_headroom() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock);
    let id = account_id();
    let entry = svc.initialize_account(id).await.unwrap();
    entry.update_policy_revision(2);
    {
        let mut quota = entry.blob_storage_quota.lock().unwrap();
        quota.available_from_server = 10;
        quota.syncing_delta = 4;
        quota.unsynced_delta = 3;
    }
    entry.begin_monthly_refresh_for_test(2, 0);
    let mut stale = service_limits(monthly_policy(1000), 1, 512, u64::MAX);
    stale.available_blob_storage_bytes = 100;
    svc.update_last_known_limits(id, stale, 2, 4).await;
    let quota = entry.blob_storage_quota.lock().unwrap();
    assert_eq!(quota.syncing_delta, 0);
    assert_eq!(quota.unsynced_delta, 3);
    assert_eq!(quota.available_from_server, 6);
}

#[test]
async fn rejected_monthly_retry_does_not_resend_blob_stock() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock.clone());
    let id = account_id();
    let entry = svc.initialize_account(id).await.unwrap();
    entry.set_available_blob_storage_bytes(10);
    assert!(entry.try_record_blob_storage_delta(4));
    entry.record_memory_gb_seconds(AgentMode::Durable, 1);
    let mut rejected = service_limits(monthly_policy(1000), 0, 512, u64::MAX);
    rejected.usage_update_applied = false;
    rejected.available_blob_storage_bytes = 6;
    mock.set_batch_update_response(account_limits(id, rejected));
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
    assert_eq!(mock.last_batch_update(id).blob_storage_bytes_delta, 4);
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
    let retry = mock.last_batch_update(id);
    assert_eq!(retry.memory_gb_seconds_delta, 1);
    assert_eq!(retry.blob_storage_bytes_delta, 0);
    assert_eq!(entry.blob_storage_quota.lock().unwrap().syncing_delta, 0);
}

#[test]
async fn blob_only_capture_remains_active_when_monthly_meters_are_disabled() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let mut svc = make_grpc(mock.clone());
    Arc::get_mut(&mut svc).unwrap().metering = ResourceUsageMeteringConfig::default();
    let id = account_id();
    let entry = svc.initialize_account(id).await.unwrap();
    entry.set_available_blob_storage_bytes(10);
    assert!(entry.try_record_blob_storage_delta(4));
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
    assert_eq!(mock.last_batch_update(id).blob_storage_bytes_delta, 4);
}

#[test]
async fn failed_blob_usage_batch_is_settled_and_not_resent() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_batch_update_error();
    let svc = make_grpc(mock.clone());
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    entry.set_available_blob_storage_bytes(10);
    assert!(entry.try_record_blob_storage_delta(10));
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    {
        let quota = entry.blob_storage_quota.lock().unwrap();
        assert_eq!(quota.unsynced_delta, 0);
        assert_eq!(quota.syncing_delta, 0);
    }

    mock.set_batch_update_response(AccountResourceLimits(HashMap::from([(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(1000),
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: 0,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: u64::MAX,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
        },
    )])));
    svc.send_batch(0).await;

    assert_eq!(mock.last_batch_update(id).blob_storage_bytes_delta, 0);
    assert!(!entry.try_record_blob_storage_delta(1));
}

#[test]
async fn failed_blob_usage_batch_does_not_restore_spent_local_headroom() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_batch_update_error();
    let svc = make_grpc(mock);
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    entry.set_available_blob_storage_bytes(10);
    assert!(entry.try_record_blob_storage_delta(10));

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert!(
        !entry.try_record_blob_storage_delta(1),
        "an ambiguous report must not make already spent local quota available again"
    );
}

#[test]
async fn omitted_blob_usage_batch_result_is_settled_and_not_resent() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_batch_update_response(AccountResourceLimits(HashMap::new()));
    let svc = make_grpc(mock.clone());
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    entry.set_available_blob_storage_bytes(10);
    assert!(entry.try_record_blob_storage_delta(10));
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    {
        let quota = entry.blob_storage_quota.lock().unwrap();
        assert_eq!(quota.unsynced_delta, 0);
        assert_eq!(quota.syncing_delta, 0);
    }

    svc.send_batch(0).await;
    assert_eq!(mock.last_batch_update(id).blob_storage_bytes_delta, 0);
    assert!(!entry.try_record_blob_storage_delta(1));
}

#[test]
async fn rejected_blob_usage_batch_is_settled_and_refreshes_headroom() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock.clone());
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    entry.set_available_blob_storage_bytes(10);
    assert!(entry.try_record_blob_storage_delta(10));

    mock.set_batch_update_response(AccountResourceLimits(HashMap::from([(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(1000),
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: 10,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: u64::MAX,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: false,
        },
    )])));
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    let quota = entry.blob_storage_quota.lock().unwrap();
    assert_eq!(quota.unsynced_delta, 0);
    assert_eq!(quota.syncing_delta, 0);
    assert_eq!(quota.available_from_server, 10);
}

#[test]
async fn settling_an_in_flight_blob_delta_preserves_new_unsynced_delta() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock);
    let id = account_id();
    let entry = svc.initialize_account(id).await.unwrap();
    entry.set_available_blob_storage_bytes(10);
    {
        let mut quota = entry.blob_storage_quota.lock().unwrap();
        quota.syncing_delta = 10;
        quota.unsynced_delta = 3;
    }

    svc.settle_blob_storage_delta(id, 10).await;

    let quota = entry.blob_storage_quota.lock().unwrap();
    assert_eq!(quota.syncing_delta, 0);
    assert_eq!(quota.unsynced_delta, 3);
    assert_eq!(quota.available_from_server, 0);
}

#[test]
async fn connectivity_outage_keeps_fuel_non_zero_and_allows_borrowing() {
    let mock = Arc::new(MockRegistryService::new(500, 512));
    mock.set_batch_update_error();
    let svc = make_grpc(mock);
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();

    for _ in 0..3 {
        entry.borrow_fuel(100);
        svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
    }

    assert_eq!(entry.effective_fuel(), 200);
    assert!(entry.borrow_fuel(1));
}

#[test]
async fn in_flight_not_double_counted_after_successful_cycle() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();

    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(700),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();
    entry.borrow_fuel(300);

    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert!(entry.borrow_fuel(700));
}

#[test]
async fn last_refresh_secs_is_updated_on_successful_batch() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();

    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(800),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();
    entry.last_refresh_secs.store(0, Ordering::Release);

    let before = Utc::now().timestamp();
    entry.borrow_fuel(200);
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;
    let after = Utc::now().timestamp();

    let stored = entry.last_refresh_secs.load(Ordering::Acquire);
    assert!(
        stored >= before,
        "last_refresh_secs should be updated on success"
    );
    assert!(stored <= after);
}

#[test]
async fn last_refresh_secs_is_not_updated_on_failed_batch() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_batch_update_error();
    let svc = make_grpc(mock);
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    let old_ts = 0i64;
    entry.last_refresh_secs.store(old_ts, Ordering::Release);

    entry.borrow_fuel(200);
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert_eq!(entry.last_refresh_secs.load(Ordering::Acquire), old_ts);
}

#[test]
async fn send_batch_active_account_not_included_in_idle_refresh() {
    // An account with non-zero delta is active — even if stale, it is sent
    // with its real delta (not zero) and must not be double-counted.
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();

    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(900),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();
    entry.last_refresh_secs.store(0, Ordering::Release); // stale
    entry.borrow_fuel(100); // also active

    // With threshold=0 every account is stale, but active accounts take
    // precedence with their real delta.
    svc.send_batch(0).await;

    // Server returned 900 — entry must reflect that, not be zeroed.
    assert_eq!(entry.effective_fuel(), 900);
    assert_eq!(entry.delta.load(Ordering::Acquire), 0);
}

#[test]
async fn send_batch_idle_stale_account_is_refreshed() {
    // An idle account (delta=0) that is stale must have its limits refreshed
    // via a zero-delta update in the same batch.
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();

    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(5000),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();
    entry.last_refresh_secs.store(0, Ordering::Release); // stale, no borrows

    let before = Utc::now().timestamp();
    svc.send_batch(STALE_THRESHOLD_SECS).await;
    let after = Utc::now().timestamp();

    assert_eq!(entry.effective_fuel(), 5000);
    let stored = entry.last_refresh_secs.load(Ordering::Acquire);
    assert!(stored >= before);
    assert!(stored <= after);
}

#[test]
async fn send_batch_recently_refreshed_idle_account_is_skipped() {
    // An idle account whose last_refresh_secs is recent must not be included.
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let svc = make_grpc(mock);
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    // last_refresh_secs is already set to now by new()

    // Large threshold → not stale, no delta → send_batch does nothing.
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    // fuel unchanged (no server call)
    assert_eq!(entry.effective_fuel(), 1000);
}

#[test]
async fn send_batch_idle_failure_does_not_update_last_refresh() {
    // On batch failure, stale idle accounts must retain old last_refresh_secs
    // so they are retried on the next tick.
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    mock.set_batch_update_error();
    let svc = make_grpc(mock);
    let id = account_id();

    let entry = svc.initialize_account(id).await.unwrap();
    let old_ts = 0i64;
    entry.last_refresh_secs.store(old_ts, Ordering::Release);

    svc.send_batch(STALE_THRESHOLD_SECS).await;

    assert_eq!(entry.last_refresh_secs.load(Ordering::Acquire), old_ts);
    assert_eq!(entry.effective_fuel(), 1000);
}

#[test]
async fn idle_account_is_refreshed_when_stale() {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    let id = account_id();

    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(5000),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: u64::MAX,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();

    entry.last_refresh_secs.store(0, Ordering::Release);

    svc.send_batch(STALE_THRESHOLD_SECS).await;

    // Fuel should now reflect the server-returned value
    assert_eq!(entry.effective_fuel(), 5000);
}

// -------------------------------------------------------------------------
// ResourceLimitsGrpc — concurrent agent limit propagation
// -------------------------------------------------------------------------

fn mock_with_concurrent_agent_limit(limit: u64) -> Arc<MockRegistryService> {
    let mock = Arc::new(MockRegistryService::new(1000, 512));
    *mock.get_limits_result.lock().unwrap() = Ok(ServiceResourceLimits {
        monthly_policy: monthly_policy(1000),
        max_memory_per_worker: 512,
        max_table_elements_per_worker: u64::MAX,
        max_disk_space_per_worker: u64::MAX,
        per_invocation_http_call_limit: u64::MAX,
        per_invocation_rpc_call_limit: u64::MAX,
        available_http_calls: u64::MAX,
        available_rpc_calls: u64::MAX,
        available_blob_storage_bytes: u64::MAX,
        max_concurrent_agents_per_executor: limit,
        oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
        monthly_policy_status: golem_service_base::model::MonthlyResourcePolicyStatus::Available,
        usage_update_applied: true,
        monthly_usage_mode_revision: 0,
        monthly_policy_revision: 0,
    });
    mock
}

#[test]
async fn initialize_account_propagates_concurrent_agent_limit() {
    let mock = mock_with_concurrent_agent_limit(5);
    let svc = make_grpc(mock);

    let entry = svc.initialize_account(account_id()).await.unwrap();

    assert_eq!(entry.max_concurrent_agents_per_executor(), 5);
}

#[test]
async fn initialize_account_propagates_unlimited_sentinel() {
    // The DB/registry stores 10^18 as "unlimited". The executor stores it
    // as-is in AtomicResourceEntry. The semaphore detects it via >= threshold.
    let mock = mock_with_concurrent_agent_limit(AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS);
    let svc = make_grpc(mock);

    let entry = svc.initialize_account(account_id()).await.unwrap();

    assert_eq!(
        entry.max_concurrent_agents_per_executor(),
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS
    );
}

#[test]
async fn update_last_known_limits_refreshes_concurrent_agent_limit() {
    let mock = mock_with_concurrent_agent_limit(5);
    let id = account_id();

    // Batch response returns a raised limit of 10.
    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(900),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: 10,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();
    assert_eq!(entry.max_concurrent_agents_per_executor(), 5);

    entry.borrow_fuel(100); // trigger active batch
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    // After the batch sync the limit should be updated to 10.
    assert_eq!(entry.max_concurrent_agents_per_executor(), 10);
}

#[test]
async fn update_last_known_limits_reflects_lowered_concurrent_agent_limit() {
    let mock = mock_with_concurrent_agent_limit(10);
    let id = account_id();

    let mut updated = HashMap::new();
    updated.insert(
        id,
        ServiceResourceLimits {
            monthly_policy: monthly_policy(900),
            max_memory_per_worker: 512,
            max_table_elements_per_worker: u64::MAX,
            max_disk_space_per_worker: u64::MAX,
            per_invocation_http_call_limit: u64::MAX,
            per_invocation_rpc_call_limit: u64::MAX,
            available_http_calls: u64::MAX,
            available_rpc_calls: u64::MAX,
            available_blob_storage_bytes: u64::MAX,
            max_concurrent_agents_per_executor: 3,
            oplog_writes_per_second: AtomicResourceEntry::UNLIMITED_OPLOG_WRITES_PER_SECOND,
            monthly_policy_status:
                golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied: true,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
        },
    );
    mock.set_batch_update_response(AccountResourceLimits(updated));

    let svc = make_grpc(mock);
    let entry = svc.initialize_account(id).await.unwrap();
    assert_eq!(entry.max_concurrent_agents_per_executor(), 10);

    entry.borrow_fuel(100);
    svc.send_batch(NO_IDLE_REFRESH_THRESHOLD_SECS).await;

    assert_eq!(entry.max_concurrent_agents_per_executor(), 3);
}

#[test]
async fn disabled_returns_unlimited_concurrent_agent_sentinel() {
    // ResourceLimitsDisabled returns the sentinel value (not u64::MAX directly)
    // matching the convention used throughout the registry service.
    let svc = ResourceLimitsDisabled;
    let entry = svc.initialize_account(account_id()).await.unwrap();
    assert_eq!(
        entry.max_concurrent_agents_per_executor(),
        AtomicResourceEntry::UNLIMITED_CONCURRENT_AGENTS
    );
}

// -------------------------------------------------------------------------
// ResourceLimitsDisabled
// -------------------------------------------------------------------------

#[test]
async fn disabled_returns_max_fuel() {
    let svc = ResourceLimitsDisabled;
    let entry = svc.initialize_account(account_id()).await.unwrap();
    assert_eq!(entry.effective_fuel(), u64::MAX);
}

#[test]
async fn disabled_returns_max_memory() {
    let svc = ResourceLimitsDisabled;
    let entry = svc.initialize_account(account_id()).await.unwrap();
    assert_eq!(entry.max_memory_limit(), usize::MAX);
}

#[test]
async fn disabled_borrow_always_succeeds() {
    let svc = ResourceLimitsDisabled;
    let entry = svc.initialize_account(account_id()).await.unwrap();
    assert!(entry.borrow_fuel(u64::MAX / 2));
    // Can borrow again — no real limit
    assert!(entry.borrow_fuel(u64::MAX / 2));
}
