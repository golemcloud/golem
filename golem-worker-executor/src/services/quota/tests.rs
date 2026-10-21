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
use crate::span_test_support::{Tracing, get_tracing_dependency as test_r_get_dep_tracing};
use chrono::Duration as ChronoDuration;
use golem_common::model::quota::{ResourceLimit, ResourceRateLimit, TimePeriod};
use golem_common::model::{Pod, RoutingTable, ShardEpoch, ShardId};
use golem_service_base::clients::shard_manager::{
    ShardLease, ShardLeaseError, ShardManagerError, ShardRegistration,
};
use pretty_assertions::assert_eq;
use pretty_assertions::assert_matches;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex as StdMutex;
use test_r::test;
use uuid::Uuid;

test_r::enable!();

fn test_env_id() -> EnvironmentId {
    EnvironmentId::new()
}

fn test_resource_name() -> ResourceName {
    ResourceName("test-resource".to_string())
}

fn test_pod() -> Pod {
    Pod {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 9093,
    }
}

fn test_resource_definition_id() -> ResourceDefinitionId {
    ResourceDefinitionId::new()
}

fn bounded_lease(
    resource_definition_id: ResourceDefinitionId,
    epoch: u64,
    amount: u64,
) -> QuotaLease {
    bounded_lease_with_total(resource_definition_id, epoch, amount, amount)
}

fn bounded_lease_with_total(
    resource_definition_id: ResourceDefinitionId,
    epoch: u64,
    amount: u64,
    total_available_amount: u64,
) -> QuotaLease {
    QuotaLease::Bounded {
        resource_definition_id,
        pod: test_pod(),
        epoch: LeaseEpoch(epoch),
        allocation: amount,
        // Use an expiry just within the 2×renewal_interval threshold
        // (test services use 100ms interval → 200ms threshold) so that
        // tests calling renew_all() always trigger a renewal.
        expires_at: Utc::now() + ChronoDuration::milliseconds(150),
        resource_limit: ResourceLimit::Rate(ResourceRateLimit {
            value: 1000,
            period: TimePeriod::Second,
            max: 1000,
        }),
        enforcement_action: EnforcementAction::Throttle,
        total_available_amount,
    }
}

/// Bounded lease expiring far beyond the test renewal threshold, so it is
/// only renewed when the renewal explicitly carries demand.
fn bounded_lease_with_distant_expiry(
    resource_definition_id: ResourceDefinitionId,
    epoch: u64,
    amount: u64,
    total_available_amount: u64,
) -> QuotaLease {
    QuotaLease::Bounded {
        resource_definition_id,
        pod: test_pod(),
        epoch: LeaseEpoch(epoch),
        allocation: amount,
        expires_at: Utc::now() + ChronoDuration::seconds(60),
        resource_limit: ResourceLimit::Rate(ResourceRateLimit {
            value: 1000,
            period: TimePeriod::Second,
            max: 1000,
        }),
        enforcement_action: EnforcementAction::Throttle,
        total_available_amount,
    }
}

/// Lease with a very slow refill rate (1 token/hour).
/// Any deficit ≥ 1 → estimated_wait ≥ 3600s, well above a 60s threshold.
fn slow_rate_lease_with_total(
    resource_definition_id: ResourceDefinitionId,
    epoch: u64,
    amount: u64,
    total_available_amount: u64,
) -> QuotaLease {
    QuotaLease::Bounded {
        resource_definition_id,
        pod: test_pod(),
        epoch: LeaseEpoch(epoch),
        allocation: amount,
        expires_at: Utc::now() + ChronoDuration::milliseconds(150),
        resource_limit: ResourceLimit::Rate(ResourceRateLimit {
            value: 1,
            period: TimePeriod::Hour,
            max: 1,
        }),
        enforcement_action: EnforcementAction::Throttle,
        total_available_amount,
    }
}

/// Capacity lease (no refill — deficit can only be resolved by release).
fn capacity_lease_with_total(
    resource_definition_id: ResourceDefinitionId,
    epoch: u64,
    amount: u64,
    total_available_amount: u64,
) -> QuotaLease {
    use golem_common::model::quota::ResourceCapacityLimit;
    QuotaLease::Bounded {
        resource_definition_id,
        pod: test_pod(),
        epoch: LeaseEpoch(epoch),
        allocation: amount,
        expires_at: Utc::now() + ChronoDuration::milliseconds(150),
        resource_limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Throttle,
        total_available_amount,
    }
}

fn unlimited_lease() -> QuotaLease {
    QuotaLease::Unlimited {
        pod: test_pod(),
        expires_at: Utc::now() + ChronoDuration::minutes(5),
    }
}

type RenewFn =
    Box<dyn Fn(ResourceDefinitionId, u64, u64) -> Result<QuotaLease, QuotaError> + Send + Sync>;

#[derive(Debug, Clone)]
struct RenewCall {
    resource_definition_id: ResourceDefinitionId,
    epoch: u64,
    unused: u64,
    pending_reservations: Vec<PendingReservation>,
}

struct MockShardManager {
    acquire_response: StdMutex<Option<Result<QuotaLease, QuotaError>>>,
    acquire_calls: std::sync::atomic::AtomicUsize,
    acquire_gate: Option<Arc<tokio::sync::Semaphore>>,
    renew_fn: StdMutex<Option<RenewFn>>,
    renewed: StdMutex<Vec<RenewCall>>,
    released: StdMutex<Vec<(ResourceDefinitionId, u64, u64)>>,
    /// The RPC and the endpoint of every quota call, in call order.
    endpoints: StdMutex<Vec<(&'static str, ExecutorEndpoint)>>,
}

impl MockShardManager {
    fn new() -> Self {
        Self {
            acquire_response: StdMutex::new(None),
            acquire_calls: std::sync::atomic::AtomicUsize::new(0),
            acquire_gate: None,
            renew_fn: StdMutex::new(None),
            renewed: StdMutex::new(Vec::new()),
            released: StdMutex::new(Vec::new()),
            endpoints: StdMutex::new(Vec::new()),
        }
    }

    fn with_acquire(self, response: Result<QuotaLease, QuotaError>) -> Self {
        *self.acquire_response.lock().unwrap() = Some(response);
        self
    }

    fn with_renew_fn(
        self,
        f: impl Fn(ResourceDefinitionId, u64, u64) -> Result<QuotaLease, QuotaError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        *self.renew_fn.lock().unwrap() = Some(Box::new(f));
        self
    }

    fn releases(&self) -> Vec<(ResourceDefinitionId, u64, u64)> {
        self.released.lock().unwrap().clone()
    }

    fn renewed(&self) -> Vec<RenewCall> {
        self.renewed.lock().unwrap().clone()
    }

    fn endpoints(&self) -> Vec<(&'static str, ExecutorEndpoint)> {
        self.endpoints.lock().unwrap().clone()
    }
}

#[async_trait]
impl ShardManager for MockShardManager {
    async fn get_routing_table(&self) -> Result<RoutingTable, ShardManagerError> {
        unimplemented!()
    }

    async fn register(
        &self,
        _endpoint: ExecutorEndpoint,
        _pod_name: Option<String>,
        _executor_id: Uuid,
    ) -> Result<ShardRegistration, ShardManagerError> {
        unimplemented!()
    }

    async fn renew_shard_lease(
        &self,
        _executor_id: Uuid,
        _shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<ShardLease, ShardLeaseError> {
        unimplemented!()
    }

    async fn deregister(
        &self,
        _executor_id: Uuid,
        _shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<(), ShardLeaseError> {
        unimplemented!()
    }

    async fn acquire_quota_lease(
        &self,
        _environment_id: EnvironmentId,
        _resource_name: ResourceName,
        endpoint: ExecutorEndpoint,
    ) -> Result<QuotaLease, QuotaError> {
        self.endpoints.lock().unwrap().push(("acquire", endpoint));
        self.acquire_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(gate) = &self.acquire_gate {
            gate.acquire().await.unwrap().forget();
        }
        self.acquire_response
            .lock()
            .unwrap()
            .clone()
            .expect("acquire_response not configured")
    }

    async fn renew_quota_lease(
        &self,
        resource_definition_id: ResourceDefinitionId,
        endpoint: ExecutorEndpoint,
        epoch: u64,
        unused: u64,
        pending_reservations: Vec<PendingReservation>,
    ) -> Result<QuotaLease, QuotaError> {
        self.endpoints.lock().unwrap().push(("renew", endpoint));
        self.renewed.lock().unwrap().push(RenewCall {
            resource_definition_id,
            epoch,
            unused,
            pending_reservations,
        });
        let guard = self.renew_fn.lock().unwrap();
        let f = guard.as_ref().expect("renew_fn not configured");
        f(resource_definition_id, epoch, unused)
    }

    async fn batch_renew_quota_leases(
        &self,
        endpoint: ExecutorEndpoint,
        renewals: Vec<BatchRenewalEntry>,
    ) -> Result<Vec<Result<QuotaLease, QuotaError>>, ShardManagerError> {
        let mut results = Vec::with_capacity(renewals.len());
        for entry in renewals {
            results.push(
                self.renew_quota_lease(
                    entry.resource_definition_id,
                    endpoint,
                    entry.epoch,
                    entry.unused,
                    entry.pending_reservations,
                )
                .await,
            );
        }
        Ok(results)
    }

    async fn release_quota_lease(
        &self,
        resource_definition_id: ResourceDefinitionId,
        endpoint: ExecutorEndpoint,
        epoch: u64,
        unused: u64,
    ) -> Result<(), QuotaError> {
        self.endpoints.lock().unwrap().push(("release", endpoint));
        self.released
            .lock()
            .unwrap()
            .push((resource_definition_id, epoch, unused));
        Ok(())
    }
}

/// Service without a running renewal loop. Tests drive `renew_all()` manually.
fn make_service(mock: MockShardManager) -> Arc<GrpcQuotaService> {
    GrpcQuotaService::new_inner(
        Arc::new(mock),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    )
}

/// One span per renewal pass, not one for the lifetime of the renewal loop.
#[test]
async fn renew_all_records_one_closed_span_per_pass(tracing: &Tracing) {
    let svc = make_service(MockShardManager::new());

    let recorder = crate::span_test_support::record_spans(tracing);
    svc.renew_all().await;

    recorder.assert_closed_span("quota_renewal");
    recorder.assert_all_closed();
}

#[test]
#[test_r::timeout("10s")]
async fn reservation_joins_in_flight_background_acquire() {
    let rid = test_resource_definition_id();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let mut mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 4)));
    mock.acquire_gate = Some(gate.clone());
    let mock = Arc::new(mock);
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );
    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 4, 0, None)
        .await;

    let mut renewal = Box::pin(svc.renew_all());
    assert!(futures::poll!(&mut renewal).is_pending());
    let mut reservation = Box::pin(svc.try_reserve(&mut interest, 2));
    assert!(futures::poll!(&mut reservation).is_pending());
    assert_eq!(
        mock.acquire_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a reservation must not replace an in-flight quota lease"
    );

    gate.add_permits(1);
    let ((), first) = tokio::join!(renewal, reservation);
    assert_matches!(
        first,
        ReserveResult::Ok(Reservation::Bounded { reserved: 2, .. })
    );
    assert_matches!(
        svc.try_reserve(&mut interest, 2).await,
        ReserveResult::Ok(Reservation::Bounded { reserved: 2, .. })
    );
    assert_eq!(
        mock.acquire_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[test]
#[test_r::timeout("10s")]
async fn first_reservation_exceeding_baseline_share_is_served_by_demand_renewal() {
    let rid = test_resource_definition_id();
    // The acquire grants only the baseline share of the pool (50 of 100);
    // the demand-carrying renewal re-grants 90, enough for the queued 60.
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease_with_distant_expiry(rid, 1, 50, 100)))
            .with_renew_fn(move |_, _, _| Ok(bounded_lease_with_distant_expiry(rid, 2, 90, 100))),
    );
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // 60 is globally satisfiable (total_available 100) but above the baseline
    // share, so the demand-blind acquire cannot serve it inline.
    let svc2 = svc.clone();
    let mut interest2 = interest.clone();
    let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 60).await });

    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect(
            "first reservation must be served by a demand-carrying renewal \
                 following the acquire, not parked until the near-expiry renewal",
        )
        .unwrap();
    assert_matches!(
        result,
        ReserveResult::Ok(Reservation::Bounded { reserved: 60, .. })
    );

    // The demand-carrying renewal renewed the just-acquired lease and
    // reported the queued reservation to the shard manager.
    let renewed = mock.renewed();
    assert_eq!(renewed.len(), 1);
    assert_eq!(renewed[0].resource_definition_id, rid);
    assert_eq!(renewed[0].epoch, 1);
    assert_eq!(renewed[0].unused, 50);
    assert_eq!(
        renewed[0]
            .pending_reservations
            .iter()
            .map(|p| p.amount)
            .collect::<Vec<_>>(),
        vec![60]
    );
}

#[test]
#[test_r::timeout("10s")]
async fn reacquired_lost_lease_is_followed_by_demand_renewal() {
    let rid = test_resource_definition_id();
    // The demand-blind re-acquire grants only the baseline share of the
    // pool (50 of 100); the demand-carrying follow-up renewal re-grants 90,
    // enough for the queued 60.
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease_with_distant_expiry(rid, 1, 50, 100)))
            .with_renew_fn(move |_, _, _| Ok(bounded_lease_with_distant_expiry(rid, 2, 90, 100))),
    );
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // A waiter queued on the lost lease: the next renewal pass re-acquires it.
    let (tx, rx) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        slot.waiters.push(Waiter::with_credit(60, 0, tx));
    }

    // Phase 4 of the renewal pass re-acquires the lost lease. Without a
    // demand-carrying follow-up renewal the waiter would stay parked until
    // the lease is close to expiry, which is 60s - threshold away.
    svc.renew_all().await;

    let result = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect(
            "a waiter queued on a reacquired lease must be served by a \
                 demand-carrying renewal following the re-acquire",
        )
        .unwrap();
    assert_matches!(
        result,
        WaiterOutcome::Granted(Reservation::Bounded { reserved: 60, .. })
    );

    let renewed = mock.renewed();
    assert_eq!(renewed.len(), 1);
    assert_eq!(
        renewed[0]
            .pending_reservations
            .iter()
            .map(|p| p.amount)
            .collect::<Vec<_>>(),
        vec![60]
    );
}

#[test]
async fn acquire_bounded_lease() {
    let rid = test_resource_definition_id();
    let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
    let svc = make_service(mock);

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;
    assert_eq!(interest.resource_name, test_resource_name());
}

#[test]
async fn acquire_is_idempotent() {
    let rid = test_resource_definition_id();
    let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
    let svc = make_service(mock);

    let env_id = test_env_id();
    let name = test_resource_name();

    let mut interest1 = svc.acquire(env_id, name.clone(), 100, 0, None).await;
    let mut interest2 = svc.acquire(env_id, name.clone(), 100, 0, None).await;

    let r1 = svc.try_reserve(&mut interest1, 50).await;
    assert_matches!(
        r1,
        ReserveResult::Ok(Reservation::Bounded { reserved: 50, .. })
    );

    let r2 = svc.try_reserve(&mut interest2, 50).await;
    assert_matches!(
        r2,
        ReserveResult::Ok(Reservation::Bounded { reserved: 50, .. })
    );
    // 100 units fully consumed — exhaustion behaviour is tested in reserve_drains_allocation.
}

#[test]
async fn reserve_bounded_success() {
    let rid = test_resource_definition_id();
    let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
    let svc = make_service(mock);

    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;
    let result = svc.try_reserve(&mut interest, 30).await;
    match result {
        ReserveResult::Ok(Reservation::Bounded {
            resource_definition_id,
            epoch,
            reserved,
            ..
        }) => {
            assert_eq!(resource_definition_id, rid);
            assert_eq!(epoch, LeaseEpoch(1));
            assert_eq!(reserved, 30);
        }
        other => panic!("Expected Bounded reservation, got {:?}", other),
    }
}

#[test]
async fn reserve_unlimited() {
    let mock = MockShardManager::new().with_acquire(Ok(unlimited_lease()));
    let svc = make_service(mock);
    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;
    assert_matches!(
        svc.try_reserve(&mut interest, 999).await,
        ReserveResult::Ok(Reservation::Unlimited)
    );
}

#[test]
async fn reserve_drains_allocation() {
    let rid = test_resource_definition_id();
    // Capacity resource. Renewal returns total_available=0 so the
    // waiter is rejected (no refill, estimated_wait=None → always reject).
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(capacity_lease_with_total(rid, 1, 100, 100)))
            .with_renew_fn(move |_, _, _| Ok(capacity_lease_with_total(rid, 2, 0, 0))),
    );
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_millis(60),
    );
    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    assert_matches!(
        svc.try_reserve(&mut interest, 40).await,
        ReserveResult::Ok(_)
    );
    assert_matches!(
        svc.try_reserve(&mut interest, 40).await,
        ReserveResult::Ok(_)
    );
    assert_matches!(
        svc.try_reserve(&mut interest, 20).await,
        ReserveResult::Ok(_)
    );

    // Allocation exhausted. Spawn a waiter for 1 unit — it enters the queue.
    let svc2 = svc.clone();
    let mut interest2 = interest.clone();
    let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 1).await });

    // notify_demand may resolve the waiter inline; only call renew_all if
    // the task is still waiting after notify_demand completes.
    let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
    let needs_renew = loop {
        tokio::task::yield_now().await;
        if task.is_finished() {
            break false;
        }
        if let Some(slot_mutex) = svc.get_slot(&key).await
            && !slot_mutex.inner.lock().await.waiters.is_empty()
        {
            break true;
        }
    };

    if needs_renew {
        svc.renew_all().await;
    }

    let result = task.await.unwrap();
    assert!(
        matches!(result, ReserveResult::InsufficientAllocation { .. }),
        "got {:?}",
        result
    );
}

#[test]
async fn reserve_globally_impossible_rejected_after_renewal() {
    let rid = test_resource_definition_id();
    // Slow rate (1/hour), total_available=10 < amount=11.
    // deficit=1 → estimated_wait=3600s >> 60s threshold → rejected by process_waiters.
    let svc = GrpcQuotaService::new_inner(
        Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(slow_rate_lease_with_total(rid, 1, 0, 10)))
                .with_renew_fn(move |_, _, _| Ok(slow_rate_lease_with_total(rid, 2, 0, 10))),
        ),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );
    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    let svc2 = svc.clone();
    let mut interest2 = interest.clone();
    let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 11).await });

    // notify_demand (called inside try_reserve) may resolve the waiter
    // immediately via the inline renewal. if it does, the task completes
    // without ever needing renew_all.
    let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
    let needs_renew = loop {
        tokio::task::yield_now().await;
        if task.is_finished() {
            break false;
        }
        if let Some(slot) = svc.get_slot(&key).await
            && !slot.inner.lock().await.waiters.is_empty()
        {
            break true;
        }
    };

    if needs_renew {
        svc.renew_all().await;
    }

    let result = task.await.unwrap();
    assert_matches!(
        result,
        ReserveResult::InsufficientAllocation {
            estimated_wait_nanos: Some(_),
            ..
        }
    );
}

#[test]
async fn waiter_rejected_when_globally_impossible_after_renewal() {
    let rid = test_resource_definition_id();
    // Acquire: total_available = 100 → waiter can enter queue.
    // Renewal: total_available drops to 5, slow rate (1/hour) →
    //          estimated wait = 5 × 3600s = 18000s >> 60s threshold → reject.
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
            .with_renew_fn(move |_, _, _| Ok(slow_rate_lease_with_total(rid, 2, 0, 5))),
    );

    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // Prime the slot: the new acquire design inserts a Lost placeholder;
    // phase 4 of the first renew_all does the initial acquire_quota_lease.
    svc.renew_all().await;

    // Add a waiter for 10 units — possible at acquire time, rejected after renewal.
    let (tx, rx) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        slot.waiters.push(Waiter::with_credit(10, 0, tx));
    }

    svc.renew_all().await;

    let result = rx.await.unwrap();
    assert_matches!(
        result,
        WaiterOutcome::Insufficient {
            estimated_wait_time: Some(_),
            ..
        }
    );
}

#[test]
async fn waiters_served_in_priority_order() {
    let rid = test_resource_definition_id();
    // Initial: 0 allocation, total = 100.
    // Renewal gives 30 — enough for one 20-unit waiter.
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
            .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 30))),
    );

    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // Prime the slot: acquire inserts a Lost placeholder; phase 4 of the
    // first renew_all does the initial acquire_quota_lease.
    svc.renew_all().await;

    // Enqueue two waiters directly to avoid ordering races.
    let (tx_a, mut rx_a) = oneshot::channel::<WaiterOutcome>();
    let (tx_b, rx_b) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        // Low priority: credit=0, amount=20 → priority 0.0
        slot.waiters.push(Waiter::with_credit(20, 0, tx_a));
        // High priority: credit=1000, amount=20 → priority 50.0
        slot.waiters.push(Waiter::with_credit(20, 1000, tx_b));
    }

    svc.renew_all().await;

    // B (high priority) should be granted.
    let result_b = rx_b.await.unwrap();
    assert_matches!(
        result_b,
        WaiterOutcome::Granted(_),
        "high-priority waiter should be granted"
    );

    // A is still waiting (only 10 units left after B took 20 of 30).
    // It stays in the queue — sender not yet dropped.
    assert!(rx_a.try_recv().is_err());
}

#[test]
async fn spawned_waiter_granted_by_renew_all() {
    let rid = test_resource_definition_id();
    // Acquire gives 0 allocation, 100 available globally — so the
    // waiter enters the queue. Renewal brings 50 allocation.
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
            .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 50))),
    );
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // Spawn try_reserve — it will block because allocation = 0.
    let svc2 = svc.clone();
    let mut interest2 = interest.clone();
    let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 30).await });

    // Wait until either the task completes (notify_demand served it inline)
    // or the waiter is in the queue (needs renew_all to serve it).
    let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
    let needs_renew = loop {
        tokio::task::yield_now().await;
        if task.is_finished() {
            break false;
        }
        if let Some(slot_mutex) = svc.get_slot(&key).await
            && !slot_mutex.inner.lock().await.waiters.is_empty()
        {
            break true;
        }
    };

    if needs_renew {
        svc.renew_all().await;
    }

    let result = task.await.unwrap();
    assert_matches!(
        result,
        ReserveResult::Ok(Reservation::Bounded { reserved: 30, .. })
    );
}

#[test]
async fn spawned_waiter_rejected_by_renew_all() {
    let rid = test_resource_definition_id();
    // Acquire: total_available = 100 → waiter can enter queue.
    // Renewal: total_available = 5, slow rate (1/hour) →
    //          estimated wait = 5 × 3600s >> 60s threshold → reject.
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
            .with_renew_fn(move |_, _, _| Ok(slow_rate_lease_with_total(rid, 2, 0, 5))),
    );
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // Spawn try_reserve — enters the queue because allocation = 0.
    let svc2 = svc.clone();
    let mut interest2 = interest.clone();
    let task = tokio::spawn(async move { svc2.try_reserve(&mut interest2, 10).await });

    let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
    let needs_renew = loop {
        tokio::task::yield_now().await;
        if task.is_finished() {
            break false;
        }
        if let Some(slot_mutex) = svc.get_slot(&key).await
            && !slot_mutex.inner.lock().await.waiters.is_empty()
        {
            break true;
        }
    };

    if needs_renew {
        svc.renew_all().await;
    }

    let result = task.await.unwrap();
    assert_matches!(
        result,
        ReserveResult::InsufficientAllocation {
            estimated_wait_nanos: Some(_),
            ..
        }
    );
}

#[test]
async fn waiter_served_on_commit_without_renewal() {
    let rid = test_resource_definition_id();
    // Allocation = 50, all consumed up front.
    let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 50)));
    let svc = make_service(mock);
    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // Drain the allocation.
    let big_reservation = match svc.try_reserve(&mut interest, 50).await {
        ReserveResult::Ok(r) => r,
        other => panic!("Expected Ok, got {:?}", other),
    };

    // Enqueue a waiter for 20 units.
    let (tx, mut rx) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        slot.waiters.push(Waiter::with_credit(20, 0, tx));
    }

    // Commit with underuse — 30 units credited back. No renewal needed.
    svc.commit(&mut interest, big_reservation, 20).await;

    // Waiter should be resolved immediately.
    let result = rx
        .try_recv()
        .expect("waiter should have been resolved by commit");
    assert!(
        matches!(
            result,
            WaiterOutcome::Granted(Reservation::Bounded { reserved: 20, .. })
        ),
        "got {:?}",
        result
    );
}

#[test]
async fn credit_debited_on_immediate_grant() {
    for (initial_credit, expected_credit) in [(10_000, 9_970), (500, 470)] {
        let rid = test_resource_definition_id();
        let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
        let svc = make_service(mock);

        let mut interest = svc
            .acquire(
                test_env_id(),
                test_resource_name(),
                100,
                initial_credit,
                None,
            )
            .await;
        if initial_credit < interest.max_credit {
            // At the cap accrual cannot change credit; below it, disable accrual.
            interest.credit_rate = 0.0;
        }

        let result = svc.try_reserve(&mut interest, 30).await;
        assert_matches!(
            result,
            ReserveResult::Ok(Reservation::Bounded { reserved: 30, .. })
        );

        assert_eq!(interest.last_credit_value, expected_credit);
    }
}

#[test]
async fn credit_back_on_underuse_commit() {
    let rid = test_resource_definition_id();
    let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
    let svc = make_service(mock);

    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    let reservation = match svc.try_reserve(&mut interest, 50).await {
        ReserveResult::Ok(r) => r,
        other => panic!("{:?}", other),
    };

    let credit_after_reserve = interest.current_credit();

    // Used only 10 of 50 reserved — 40 should be credited back.
    svc.commit(&mut interest, reservation, 10).await;

    // Credit after commit should be higher than after reserve.
    assert!(
        interest.last_credit_value > credit_after_reserve,
        "expected credit to increase: before={credit_after_reserve}, after={}",
        interest.last_credit_value
    );
    // Specifically: credited back 40 (on top of whatever time accrued).
    assert!(interest.last_credit_value >= credit_after_reserve + 40);
}

#[test]
async fn credit_not_back_on_epoch_mismatch_commit() {
    let rid = test_resource_definition_id();
    let mock = MockShardManager::new()
        .with_acquire(Ok(bounded_lease(rid, 1, 100)))
        .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 200)));
    let svc = make_service(mock);

    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    let reservation = match svc.try_reserve(&mut interest, 50).await {
        ReserveResult::Ok(r) => r,
        other => panic!("{:?}", other),
    };
    let credit_after_reserve = interest.last_credit_value;

    // Renewal bumps the epoch.
    svc.renew_all().await;

    // Commit with underuse but stale epoch — no credit back.
    svc.commit(&mut interest, reservation, 10).await;

    // Credit should be unchanged (modulo time accrual, which we ignore
    // by comparing the snapshot value directly).
    assert_eq!(
        interest.last_credit_value, credit_after_reserve,
        "credit should not change on epoch-mismatch commit"
    );
}

#[test]
async fn priority_reflects_time_in_queue() {
    let rid = test_resource_definition_id();
    // Renewal gives 20 units — enough for one 20-unit waiter.
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 100)))
            .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 20))),
    );
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // Prime the slot: acquire inserts a Lost placeholder; phase 4 of the
    // first renew_all does the initial acquire_quota_lease.
    svc.renew_all().await;

    // Waiter A: high initial credit (1000), just arrived.
    let (tx_a, mut rx_a) = oneshot::channel::<WaiterOutcome>();
    // Waiter B: low initial credit (-500) but been waiting 200ms
    // (credit_rate = 100 * 0.1 = 10/ms → accrues 2000 credits in 200ms).
    let (tx_b, rx_b) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        slot.waiters.push(Waiter::with_credit(20, 1000, tx_a));
        // Simulate 200ms wait by backdating last_credit_value_at.
        slot.waiters.push(Waiter {
            amount: 20,
            last_credit_value: -500,
            last_credit_value_at: Utc::now() - ChronoDuration::milliseconds(200),
            credit_rate: 10.0, // 100 * 0.1
            max_credit: 10_000,
            tx: tx_b,
        });
    }

    svc.renew_all().await;

    // B should win because its current_credit ≈ -500 + 200*10 = 1500 > A's 1000.
    let result_b = rx_b.await.unwrap();
    assert_matches!(
        result_b,
        WaiterOutcome::Granted(_),
        "long-waiting waiter B should be granted over A, got {:?}",
        result_b
    );
    // A stays in the queue.
    assert!(rx_a.try_recv().is_err(), "A should still be waiting");
}

#[test]
async fn waiter_stays_queued_when_refill_is_imminent() {
    let rid = test_resource_definition_id();
    // Use a call counter to return different responses on successive renewals.
    let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_count2 = call_count.clone();
    let svc = GrpcQuotaService::new_inner(
        Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(bounded_lease_with_total(rid, 1, 0, 5)))
                .with_renew_fn(move |_, _, _| {
                    let n = call_count2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        // First renewal: still insufficient, refill imminent.
                        Ok(bounded_lease_with_total(rid, 2, 0, 5))
                    } else {
                        // Second renewal: capacity available.
                        Ok(bounded_lease(rid, 3, 20))
                    }
                }),
        ),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // Prime the slot: acquire inserts a Lost placeholder; phase 4 of the
    // first renew_all does the initial acquire_quota_lease.
    svc.renew_all().await;

    let (tx, mut rx) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        slot.waiters.push(Waiter::with_credit(10, 0, tx));
    }

    // Second renewal (first with the waiter): total_available=5 < 10, refill imminent → stay queued.
    svc.renew_all().await;
    assert!(
        rx.try_recv().is_err(),
        "waiter should still be queued after first renewal"
    );

    // Second renewal brings 20 allocation — waiter is now served.
    svc.renew_all().await;
    let result = rx.try_recv().expect("waiter should have been resolved");
    assert_matches!(
        result,
        WaiterOutcome::Granted(Reservation::Bounded { reserved: 10, .. })
    );
}

#[test]
async fn waiter_rejected_when_estimated_wait_exceeds_threshold() {
    let rid = test_resource_definition_id();
    // Slow rate: 1 token/hour. deficit = 10 - 5 = 5 → 5 refills × 3600s = 18000s >> 60s.
    let svc = GrpcQuotaService::new_inner(
        Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(slow_rate_lease_with_total(rid, 1, 0, 5)))
                .with_renew_fn(move |_, _, _| Ok(slow_rate_lease_with_total(rid, 2, 0, 5))),
        ),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    let (tx, rx) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        slot.waiters.push(Waiter::with_credit(10, 0, tx));
    }

    // Estimated wait >> 60s threshold → reject.
    svc.renew_all().await;
    let result = rx.await.unwrap();
    assert_matches!(result, WaiterOutcome::Insufficient { .. });
}

#[test]
async fn waiter_rejected_immediately_when_no_refill() {
    let rid = test_resource_definition_id();
    // Capacity resource — no refill, so None → always reject.
    let svc = GrpcQuotaService::new_inner(
        Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(capacity_lease_with_total(rid, 1, 0, 5)))
                .with_renew_fn(move |_, _, _| Ok(capacity_lease_with_total(rid, 2, 0, 5))),
        ),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    let (tx, rx) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        slot.waiters.push(Waiter::with_credit(10, 0, tx));
    }

    // No refill → always reject regardless of threshold.
    svc.renew_all().await;
    let result = rx.await.unwrap();
    assert_matches!(
        result,
        WaiterOutcome::Insufficient {
            estimated_wait_time: None,
            ..
        }
    );
}

#[test]
fn estimated_wait_already_satisfied() {
    use golem_common::model::quota::ResourceRateLimit;
    let limit = ResourceLimit::Rate(ResourceRateLimit {
        value: 100,
        period: TimePeriod::Second,
        max: 100,
    });
    assert_eq!(
        GrpcQuotaService::estimated_wait(50, 100, &limit),
        Some(Duration::ZERO)
    );
}

#[test]
fn estimated_wait_one_refill() {
    use golem_common::model::quota::ResourceRateLimit;
    // deficit = 10, tokens/refill = 100 → 1 refill → 1 second
    let limit = ResourceLimit::Rate(ResourceRateLimit {
        value: 100,
        period: TimePeriod::Second,
        max: 100,
    });
    assert_eq!(
        GrpcQuotaService::estimated_wait(110, 100, &limit),
        Some(Duration::from_secs(1))
    );
}

#[test]
fn estimated_wait_multiple_refills() {
    use golem_common::model::quota::ResourceRateLimit;
    // deficit = 250, tokens/refill = 100 → 3 refills → 3 minutes
    let limit = ResourceLimit::Rate(ResourceRateLimit {
        value: 100,
        period: TimePeriod::Minute,
        max: 1000,
    });
    assert_eq!(
        GrpcQuotaService::estimated_wait(350, 100, &limit),
        Some(Duration::from_mins(3))
    );
}

#[test]
fn estimated_wait_capacity_resource_returns_none() {
    use golem_common::model::quota::ResourceCapacityLimit;
    let limit = ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 });
    assert_eq!(GrpcQuotaService::estimated_wait(200, 50, &limit), None);
}

#[test]
fn estimated_wait_concurrency_resource_returns_none() {
    use golem_common::model::quota::ResourceConcurrencyLimit;
    let limit = ResourceLimit::Concurrency(ResourceConcurrencyLimit { value: 10 });
    assert_eq!(GrpcQuotaService::estimated_wait(20, 5, &limit), None);
}

#[test]
async fn dead_bounded_lease_is_released() {
    let rid = test_resource_definition_id();
    let mock = Arc::new(MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100))));
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;
    // Consume some allocation so unused = 70 on release.
    let _ = svc.try_reserve(&mut interest, 30).await;
    drop(interest);

    svc.renew_all().await;

    let releases = mock.releases();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].0, rid);
    assert_eq!(releases[0].1, 1); // epoch
    assert_eq!(releases[0].2, 70); // unused
}

/// Every quota call names the executor at the endpoint the service was initialized with, so
/// the shard manager resolves the same address as for the shard lease.
#[test]
async fn every_quota_call_carries_the_advertised_endpoint() {
    let rid = test_resource_definition_id();
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease(rid, 1, 100)))
            .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 200))),
    );
    let advertised = ExecutorEndpoint {
        port: 9093,
        advertised_ip: Some("fdaa:0:1:a7b::2".parse().unwrap()),
    };
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        advertised,
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let mut interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;
    // More than the acquired allocation, so the reservation renews the lease.
    assert_matches!(
        svc.try_reserve(&mut interest, 200).await,
        ReserveResult::Ok(Reservation::Bounded { epoch, .. }) if epoch == LeaseEpoch(2)
    );
    drop(interest);
    svc.renew_all().await;

    let calls = mock.endpoints();
    for rpc in ["acquire", "renew", "release"] {
        assert!(
            calls.iter().any(|(called, _)| *called == rpc),
            "no {rpc} call in {calls:?}"
        );
    }
    assert!(
        calls.iter().all(|(_, endpoint)| *endpoint == advertised),
        "{calls:?}"
    );
}

#[test]
async fn live_lease_is_not_released() {
    let rid = test_resource_definition_id();
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease(rid, 1, 100)))
            .with_renew_fn(move |_, _, _| Ok(bounded_lease(rid, 2, 200))),
    );
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;
    svc.renew_all().await;

    assert!(mock.releases().is_empty());
    // Lease was renewed — new epoch 2, allocation 200.
    let mut interest = interest;
    assert_matches!(
        svc.try_reserve(&mut interest, 200).await,
        ReserveResult::Ok(Reservation::Bounded { epoch, .. }) if epoch == LeaseEpoch(2)
    );
}

#[test]
async fn acquire_with_previous_credit_at_sets_baseline() {
    let rid = test_resource_definition_id();
    let mock = MockShardManager::new().with_acquire(Ok(bounded_lease(rid, 1, 100)));
    let svc = make_service(mock);

    // Set credit baseline 1000ms in the past with credit_rate=10/ms.
    // Expected current_credit = min(max, -100 + 1000 * 10) = min(10000, 9900) = 9900.
    let past = Utc::now() - ChronoDuration::milliseconds(1000);
    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, -100, Some(past))
        .await;

    // Credit should reflect ~1000ms of accrual from the past baseline.
    let credit = interest.current_credit();
    assert!(credit >= 9800, "expected credit ~9900, got {credit}");
}

#[test]
async fn lost_lease_is_reacquired_and_waiters_served() {
    let rid = test_resource_definition_id();
    let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_count2 = call_count.clone();
    let svc = GrpcQuotaService::new_inner(
        Arc::new(
            MockShardManager::new()
                .with_acquire(Ok(bounded_lease(rid, 1, 50)))
                .with_renew_fn(move |_, _, _| {
                    // First renew: simulate lease-not-found → triggers Lost.
                    let n = call_count2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        Err(QuotaError::LeaseNotFound("gone".to_string()))
                    } else {
                        // Re-acquire (phase 4) returns a fresh lease.
                        Ok(bounded_lease(rid, 2, 30))
                    }
                }),
        ),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;

    // Enqueue a waiter while the lease is still valid.
    let (tx, rx) = oneshot::channel::<WaiterOutcome>();
    {
        let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
        let slot_mutex = svc.get_slot(&key).await.unwrap();
        let mut slot = slot_mutex.inner.lock().await;
        slot.waiters.push(Waiter::with_credit(20, 0, tx));
    }

    // renew_all: phase 3 marks lease as Lost; phase 4 re-acquires.
    svc.renew_all().await;

    let result = rx.await.unwrap();
    assert_matches!(
        result,
        WaiterOutcome::Granted(Reservation::Bounded { reserved: 20, .. })
    );
}

#[test]
// A shard manager that lost its leadership refuses the renewal, but the lease itself is safe:
// the elected leader restored it from the store. Treating the refusal like `LeaseNotFound`
// would drop the lease and re-acquire it, handing the allocation back and forth on every
// failover.
async fn renewal_refused_by_a_deposed_shard_manager_keeps_the_lease() {
    let rid = test_resource_definition_id();
    let renewals = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let renewals_seen = renewals.clone();
    let mock = Arc::new(
        MockShardManager::new()
            .with_acquire(Ok(bounded_lease(rid, 1, 50)))
            .with_renew_fn(move |_, _, _| {
                renewals_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(QuotaError::LeadershipLost(
                    "no longer the leader".to_string(),
                ))
            }),
    );
    let svc = GrpcQuotaService::new_inner(
        mock.clone(),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    let interest = svc
        .acquire(test_env_id(), test_resource_name(), 100, 0, None)
        .await;
    // The first pass acquires the lease; the second renews it, and is refused.
    svc.renew_all().await;
    svc.renew_all().await;
    assert_eq!(
        renewals.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the lease was never renewed, so the refusal was never seen"
    );

    let key: ResourceKey = (interest.environment_id, interest.resource_name.clone());
    let slot_mutex = svc.get_slot(&key).await.unwrap();
    let slot = slot_mutex.inner.lock().await;
    assert_matches!(
        &slot.lease,
        TrackedLease::Bounded(BoundedLease {
            epoch: LeaseEpoch(1),
            ..
        })
    );
    assert_eq!(
        mock.acquire_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the lease was re-acquired instead of kept"
    );
}

#[test]
async fn commit_on_lost_lease_is_noop() {
    // Configure so that renewal fails (LeaseNotFound) but re-acquire also
    // fails, keeping the lease in Lost state through the commit.
    let svc = GrpcQuotaService::new_inner(
        Arc::new(
            MockShardManager::new()
                .with_acquire(Err(QuotaError::LeaseNotFound("gone".to_string())))
                .with_renew_fn(move |_, _, _| Err(QuotaError::LeaseNotFound("gone".to_string()))),
        ),
        ExecutorEndpoint::without_advertised_ip(9093),
        Duration::from_millis(200),
        Duration::from_secs(60),
    );

    // Manually insert a bounded lease so try_reserve can succeed.
    let key: ResourceKey = (test_env_id(), test_resource_name());
    let arc = Arc::new(());
    let max_credit = 100i64 * CREDIT_MAX_FACTOR;
    let interest_arc = Arc::new(());
    svc.state
        .upsert_async(
            key.clone(),
            Arc::new(LeaseEntry {
                interest: StdMutex::new(Arc::downgrade(&interest_arc)),
                inner: Mutex::new(LeaseInner {
                    waiters: Vec::new(),
                    lease: TrackedLease::Bounded(BoundedLease {
                        resource_definition_id: test_resource_definition_id(),
                        epoch: LeaseEpoch(1),
                        remaining: 100,
                        expires_at: Utc::now() + ChronoDuration::milliseconds(150),
                        resource_limit: ResourceLimit::Rate(ResourceRateLimit {
                            value: 1000,
                            period: TimePeriod::Minute,
                            max: 1000,
                        }),
                        enforcement_action: EnforcementAction::Throttle,
                        total_available_amount: 100,
                    }),
                }),
            }),
        )
        .await;
    let _ = arc; // keep interest alive

    let mut interest = LeaseInterest {
        environment_id: key.0,
        resource_name: key.1.clone(),
        expected_use: 100,
        last_credit_value: 0,
        last_credit_value_at: Utc::now(),
        credit_rate: 100.0 * CREDIT_RATE_FACTOR,
        max_credit,
        _token: interest_arc,
    };

    let reservation = match svc.try_reserve(&mut interest, 30).await {
        ReserveResult::Ok(r) => r,
        other => panic!("{:?}", other),
    };
    let credit_after_reserve = interest.last_credit_value;

    // Renew fails → lease becomes Lost. Re-acquire also fails → stays Lost.
    svc.renew_all().await;

    // Commit on a Lost lease: should not panic, should not credit back.
    svc.commit(&mut interest, reservation, 10).await;

    assert_eq!(
        interest.last_credit_value, credit_after_reserve,
        "commit on Lost lease should not modify credit"
    );
}
