use super::MonthlyMemoryCheck;
use crate::services::active_agents::{
    ConcurrentAgentPermit, ConcurrentAgentsScheduler, MemoryGrant,
};
use crate::services::golem_config::ResourceUsageMeteringConfig;
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::resource_usage_metering::{
    ResourceUsageAccount, ResourceUsageMeter, ResourceUsageMeteringWindow, close_window,
    create_unbound_meter, open_window,
};
use futures::FutureExt;
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{AccountUsagePeriod, MonthlyUsageMode};
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_service_base::model::MonthlyResourcePolicy;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Wake, Waker};
use std::time::{Duration, Instant};
use test_r::test;
use uuid::Uuid;

const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Default)]
struct Woken(AtomicBool);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Fixture {
    entry: Arc<AtomicResourceEntry>,
    memory: LinearMemoryTracker,
    _meter: ResourceUsageMeter,
    window: ResourceUsageMeteringWindow,
    _scheduler: Arc<ConcurrentAgentsScheduler>,
}

impl Fixture {
    async fn new() -> Self {
        let entry = Arc::new(AtomicResourceEntry::new_with_monthly_policy(
            MonthlyResourcePolicy {
                period: AccountUsagePeriod::current(),
                mode: MonthlyUsageMode::HardLimit,
                available_fuel: u64::MAX,
                available_memory_gb_seconds: 1,
                available_memory_byte_nanoseconds_remainder: 0,
                available_durable_storage_byte_seconds: u64::MAX,
                available_durable_storage_byte_nanoseconds_remainder: 0,
                available_ephemeral_storage_byte_seconds: u64::MAX,
                available_ephemeral_storage_byte_nanoseconds_remainder: 0,
            },
            usize::MAX,
            usize::MAX,
            u64::MAX,
            1,
        ));
        let memory = LinearMemoryTracker::new_with_metering(
            GIB,
            GIB,
            AgentMode::Durable,
            true,
            entry.clone(),
            Arc::new(Mutex::new(MemoryGrant::inert(GIB))),
            true,
        );
        let meter = create_unbound_meter(
            ResourceUsageMeteringConfig {
                compute: false,
                memory: true,
                filesystem: false,
            },
            ResourceUsageAccount::new(AgentMode::Durable, memory.clone(), entry.clone()),
        );
        let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
        let account = AccountId(Uuid::new_v4());
        scheduler.register_account(account, entry.clone()).await;
        let permit: ConcurrentAgentPermit = scheduler
            .acquire(
                account,
                AgentId {
                    component_id: ComponentId(Uuid::new_v4()),
                    agent_id: "rate-wake".into(),
                },
            )
            .await;
        let window = open_window(&meter, permit).await.unwrap();
        memory.reconcile(GIB, Instant::now());
        Self {
            entry,
            memory,
            _meter: meter,
            window,
            _scheduler: scheduler,
        }
    }

    fn check(&self) -> MonthlyMemoryCheck {
        MonthlyMemoryCheck::new(self.entry.clone(), self.window.monthly_memory_tracker())
    }
}

#[test]
async fn live_activation_before_monitor_subscription_reads_positive_horizon() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.memory.billable_bytes(), 0);
    fixture.memory.switch_to_live();
    let _body = fixture.memory.begin_guest_body();
    fixture.memory.activate_live_usage_if_guest_body();
    let mut check = fixture.check();
    assert_eq!(check.duration(), Duration::from_secs(1));
    close_window(fixture.window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[test]
async fn live_activation_wakes_an_in_flight_zero_rate_monitor_wait() {
    let fixture = Fixture::new().await;
    let policy_updates = fixture.entry.subscribe_capacity_updates();
    let mut check = fixture.check();
    assert_eq!(check.duration(), Duration::from_secs(30));
    fixture.memory.switch_to_live();
    let _body = fixture.memory.begin_guest_body();
    let woken = Arc::new(Woken::default());
    let waker = Waker::from(woken.clone());
    {
        let mut context = Context::from_waker(&waker);
        let wait = check.wait(std::future::pending());
        tokio::pin!(wait);
        assert!(wait.as_mut().poll(&mut context).is_pending());
        assert!(!woken.0.load(Ordering::SeqCst));
        fixture.memory.activate_live_usage_if_guest_body();
        assert!(
            !policy_updates.has_changed().unwrap(),
            "activation is not a policy revision"
        );
        assert!(
            woken.0.load(Ordering::SeqCst),
            "activation must wake the monitor"
        );
        assert!(
            matches!(
                wait.as_mut().poll(&mut context),
                std::task::Poll::Ready(Ok(()))
            ),
            "a live rate must invalidate the armed zero-rate wait"
        );
    }
    assert_eq!(check.duration(), Duration::from_secs(1));
    close_window(fixture.window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[test]
async fn activation_between_horizon_read_and_wait_poll_is_not_lost() {
    let fixture = Fixture::new().await;
    let mut check = fixture.check();
    assert_eq!(check.duration(), Duration::from_secs(30));
    fixture.memory.switch_to_live();
    let _body = fixture.memory.begin_guest_body();
    fixture.memory.activate_live_usage_if_guest_body();
    assert!(
        matches!(
            check.wait(std::future::pending()).now_or_never(),
            Some(Ok(()))
        ),
        "a rate change in the read/wait gap must stay pending"
    );
    assert_eq!(check.duration(), Duration::from_secs(1));
    close_window(fixture.window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[test]
async fn live_memory_growth_wakes_the_existing_monitor_wait() {
    let fixture = Fixture::new().await;
    let policy_updates = fixture.entry.subscribe_capacity_updates();
    fixture.memory.switch_to_live();
    let _body = fixture.memory.begin_guest_body();
    fixture.memory.activate_live_usage_if_guest_body();
    let mut check = fixture.check();
    assert_eq!(check.duration(), Duration::from_secs(1));
    let woken = Arc::new(Woken::default());
    let waker = Waker::from(woken.clone());
    {
        let mut context = Context::from_waker(&waker);
        let wait = check.wait(std::future::pending());
        tokio::pin!(wait);
        assert!(wait.as_mut().poll(&mut context).is_pending());
        assert!(!woken.0.load(Ordering::SeqCst));
        assert_eq!(fixture.memory.grow(GIB, Instant::now()).0, 2 * GIB);
        assert!(
            !policy_updates.has_changed().unwrap(),
            "growth is not a policy revision"
        );
        assert!(
            woken.0.load(Ordering::SeqCst),
            "growth must wake the monitor"
        );
        assert!(
            matches!(
                wait.as_mut().poll(&mut context),
                std::task::Poll::Ready(Ok(()))
            ),
            "growth must invalidate the old rate horizon"
        );
    }
    assert_eq!(check.duration(), Duration::from_millis(500));
    close_window(fixture.window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[test]
async fn reconstruction_growth_and_unchanged_live_rate_do_not_wake_monitor() {
    let fixture = Fixture::new().await;
    let mut check = fixture.check();
    assert_eq!(check.duration(), Duration::from_secs(30));
    {
        let wait = check.wait(std::future::pending());
        tokio::pin!(wait);
        assert!(wait.as_mut().now_or_never().is_none());
        assert_eq!(fixture.memory.grow(GIB, Instant::now()).0, 2 * GIB);
        assert_eq!(fixture.memory.billable_bytes(), 0);
        assert!(wait.as_mut().now_or_never().is_none());
    }
    fixture.memory.switch_to_live();
    let _body = fixture.memory.begin_guest_body();
    fixture.memory.activate_live_usage_if_guest_body();
    assert_eq!(check.duration(), Duration::from_millis(500));
    {
        let wait = check.wait(std::future::pending());
        tokio::pin!(wait);
        assert!(wait.as_mut().now_or_never().is_none());
        assert_eq!(fixture.memory.grow(0, Instant::now()).0, 2 * GIB);
        fixture.memory.activate_live_usage_if_guest_body();
        assert!(wait.as_mut().now_or_never().is_none());
    }
    close_window(fixture.window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[test]
async fn late_live_activation_does_not_reopen_a_closed_monitor_window() {
    let fixture = Fixture::new().await;
    fixture.memory.switch_to_live();
    let _body = fixture.memory.begin_guest_body();
    fixture.memory.activate_live_usage_if_guest_body();
    let mut check = fixture.check();
    assert_eq!(check.duration(), Duration::from_secs(1));
    close_window(fixture.window, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(fixture.memory.billable_bytes(), 0);
    assert_eq!(check.duration(), Duration::from_secs(30));
    let wait = check.wait(std::future::pending());
    tokio::pin!(wait);
    assert!(wait.as_mut().now_or_never().is_none());
    fixture.memory.activate_live_usage_if_guest_body();
    assert_eq!(fixture.memory.billable_bytes(), 0);
    assert!(
        wait.as_mut().now_or_never().is_none(),
        "late activation cannot reopen rate or billing"
    );
}
