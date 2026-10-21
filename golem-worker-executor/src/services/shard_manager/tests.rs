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
use crate::services::shard::ShardServiceDefault;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::{ResourceDefinitionId, ResourceName};
use golem_common::model::{AgentId, RoutingTable};
use golem_service_base::clients::shard_manager::{
    BatchRenewalEntry, QuotaError, ShardLease, ShardManager, ShardRegistration,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::quota_lease::{PendingReservation, QuotaLease};
use std::collections::HashSet;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;
use test_r::test;

test_r::enable!();

const SHARDS: usize = 8;
const ENDPOINT: ExecutorEndpoint = ExecutorEndpoint {
    port: 9000,
    advertised_ip: None,
};

fn epochs(entries: impl IntoIterator<Item = (i64, u64)>) -> HashMap<ShardId, ShardEpoch> {
    entries
        .into_iter()
        .map(|(shard_id, epoch)| (ShardId::new(shard_id), ShardEpoch(epoch)))
        .collect()
}

fn epoch_map(entries: impl IntoIterator<Item = (i64, u64)>) -> BTreeMap<ShardId, ShardEpoch> {
    entries
        .into_iter()
        .map(|(shard_id, epoch)| (ShardId::new(shard_id), ShardEpoch(epoch)))
        .collect()
}

/// An agent id that routes to `shard`, found by search because
/// `ShardId::from_agent_id` is a hash.
fn agent_on_shard(shard: i64) -> AgentId {
    let component_id = ComponentId(Uuid::nil());
    for candidate in 0..10_000 {
        let agent_id = AgentId {
            component_id,
            agent_id: format!("agent-{candidate}"),
        };
        if ShardId::from_agent_id(&agent_id, SHARDS) == ShardId::new(shard) {
            return agent_id;
        }
    }
    panic!("no agent id in the search space routes to shard {shard}");
}

fn registration(
    expires_at: Instant,
    shard_epochs: impl IntoIterator<Item = (i64, u64)>,
) -> ShardRegistration {
    ShardRegistration {
        number_of_shards: SHARDS as u32,
        lease: ShardLease {
            shard_epochs: epoch_map(shard_epochs),
            expires_at,
            revision: revision_of(MANAGER, 1),
        },
    }
}

/// The manager process behind every delivery in the tests that do not care which one sent it.
const MANAGER: Uuid = Uuid::from_u128(0x5eed);

fn revision_of(manager: Uuid, number: u64) -> ShardLeaseRevision {
    ShardLeaseRevision {
        incarnation: manager,
        number,
    }
}

/// [`registration`], answered by the shard manager process `manager` at `revision`.
fn registration_from(
    manager: Uuid,
    revision: u64,
    expires_at: Instant,
    shard_epochs: impl IntoIterator<Item = (i64, u64)>,
) -> ShardRegistration {
    let mut registration = registration(expires_at, shard_epochs);
    registration.lease.revision = revision_of(manager, revision);
    registration
}

type RegisterFn = Box<dyn Fn(Uuid) -> Result<ShardRegistration, ShardManagerError> + Send + Sync>;
type RenewFn = Box<
    dyn Fn(Uuid, BTreeMap<ShardId, ShardEpoch>) -> Result<ShardLease, ShardLeaseError>
        + Send
        + Sync,
>;

/// The `ShardManager` client double, modelled on the `MockShardManager`
/// harness in `services/quota.rs`: every quota method is unreachable here,
/// and the three shard-lease methods are scripted and recorded.
struct MockShardManager {
    register_fn: StdMutex<Option<RegisterFn>>,
    renew_fn: StdMutex<Option<RenewFn>>,
    /// Holds a renewal inside the RPC, so a test can act while one is still in flight.
    renew_gate: StdMutex<Option<Arc<tokio::sync::Notify>>>,
    /// The same for a deregistration.
    deregister_gate: StdMutex<Option<Arc<tokio::sync::Notify>>>,
    /// Each registration's executor id and the previous epochs it carried.
    register_calls: StdMutex<Vec<Uuid>>,
    /// The endpoint each registration named, in the order of `register_calls`.
    register_endpoints: StdMutex<Vec<ExecutorEndpoint>>,
    renew_calls: StdMutex<Vec<(Uuid, BTreeMap<ShardId, ShardEpoch>)>>,
    /// The fenced epochs each renewal reported, in the order of `renew_calls`.
    deregister_calls: StdMutex<Vec<(Uuid, BTreeMap<ShardId, ShardEpoch>)>>,
    /// How many of the next registrations never answer.
    hanging_registrations: std::sync::atomic::AtomicUsize,
    /// Holds only the first renewal, until notified.
    first_renewal_gate: StdMutex<Option<Arc<tokio::sync::Notify>>>,
}

impl MockShardManager {
    fn new() -> Self {
        Self {
            register_fn: StdMutex::new(None),
            renew_fn: StdMutex::new(None),
            renew_gate: StdMutex::new(None),
            deregister_gate: StdMutex::new(None),
            register_calls: StdMutex::new(Vec::new()),
            register_endpoints: StdMutex::new(Vec::new()),
            renew_calls: StdMutex::new(Vec::new()),
            deregister_calls: StdMutex::new(Vec::new()),
            hanging_registrations: std::sync::atomic::AtomicUsize::new(0),
            first_renewal_gate: StdMutex::new(None),
        }
    }

    fn with_first_renewal_gate(self, gate: Arc<tokio::sync::Notify>) -> Self {
        *self.first_renewal_gate.lock().unwrap() = Some(gate);
        self
    }

    fn hang_next_registrations(&self, count: usize) {
        self.hanging_registrations
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    fn with_register(
        self,
        f: impl Fn(Uuid) -> Result<ShardRegistration, ShardManagerError> + Send + Sync + 'static,
    ) -> Self {
        *self.register_fn.lock().unwrap() = Some(Box::new(f));
        self
    }

    fn with_renew(
        self,
        f: impl Fn(Uuid, BTreeMap<ShardId, ShardEpoch>) -> Result<ShardLease, ShardLeaseError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        *self.renew_fn.lock().unwrap() = Some(Box::new(f));
        self
    }

    fn with_renew_gate(self, gate: Arc<tokio::sync::Notify>) -> Self {
        *self.renew_gate.lock().unwrap() = Some(gate);
        self
    }

    fn with_deregister_gate(self, gate: Arc<tokio::sync::Notify>) -> Self {
        *self.deregister_gate.lock().unwrap() = Some(gate);
        self
    }

    fn register_calls(&self) -> Vec<Uuid> {
        self.register_calls.lock().unwrap().clone()
    }

    fn register_endpoints(&self) -> Vec<ExecutorEndpoint> {
        self.register_endpoints.lock().unwrap().clone()
    }

    fn renew_calls(&self) -> Vec<(Uuid, BTreeMap<ShardId, ShardEpoch>)> {
        self.renew_calls.lock().unwrap().clone()
    }

    fn deregister_calls(&self) -> Vec<(Uuid, BTreeMap<ShardId, ShardEpoch>)> {
        self.deregister_calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ShardManager for MockShardManager {
    async fn get_routing_table(&self) -> Result<RoutingTable, ShardManagerError> {
        unimplemented!()
    }

    async fn register(
        &self,
        endpoint: ExecutorEndpoint,
        _pod_name: Option<String>,
        executor_id: Uuid,
    ) -> Result<ShardRegistration, ShardManagerError> {
        self.register_calls.lock().unwrap().push(executor_id);
        self.register_endpoints.lock().unwrap().push(endpoint);
        let hangs = self
            .hanging_registrations
            .try_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok();
        if hangs {
            std::future::pending::<()>().await;
        }
        let guard = self.register_fn.lock().unwrap();
        let f = guard.as_ref().expect("register_fn not configured");
        f(executor_id)
    }

    async fn renew_shard_lease(
        &self,
        executor_id: Uuid,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<ShardLease, ShardLeaseError> {
        self.renew_calls
            .lock()
            .unwrap()
            .push((executor_id, shard_epochs.clone()));
        // Cloned out before the await: the guard must not be held across it.
        let gate = self.renew_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let first = self.renew_calls.lock().unwrap().len() == 1;
        let first_gate = self.first_renewal_gate.lock().unwrap().clone();
        if let (true, Some(gate)) = (first, first_gate) {
            gate.notified().await;
        }
        let guard = self.renew_fn.lock().unwrap();
        let f = guard.as_ref().expect("renew_fn not configured");
        f(executor_id, shard_epochs)
    }

    async fn deregister(
        &self,
        executor_id: Uuid,
        shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    ) -> Result<(), ShardLeaseError> {
        self.deregister_calls
            .lock()
            .unwrap()
            .push((executor_id, shard_epochs));
        // Recorded before the gate, so a test can see the call was issued and then parked.
        // Cloned out before the await: the guard must not be held across it.
        let gate = self.deregister_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        Ok(())
    }

    async fn acquire_quota_lease(
        &self,
        _environment_id: EnvironmentId,
        _resource_name: ResourceName,
        _endpoint: ExecutorEndpoint,
    ) -> Result<QuotaLease, QuotaError> {
        unimplemented!()
    }

    async fn renew_quota_lease(
        &self,
        _resource_definition_id: ResourceDefinitionId,
        _endpoint: ExecutorEndpoint,
        _epoch: u64,
        _unused: u64,
        _pending_reservations: Vec<PendingReservation>,
    ) -> Result<QuotaLease, QuotaError> {
        unimplemented!()
    }

    async fn batch_renew_quota_leases(
        &self,
        _endpoint: ExecutorEndpoint,
        _renewals: Vec<BatchRenewalEntry>,
    ) -> Result<Vec<Result<QuotaLease, QuotaError>>, ShardManagerError> {
        unimplemented!()
    }

    async fn release_quota_lease(
        &self,
        _resource_definition_id: ResourceDefinitionId,
        _endpoint: ExecutorEndpoint,
        _epoch: u64,
        _unused: u64,
    ) -> Result<(), QuotaError> {
        unimplemented!()
    }
}

/// A service wired to the mock, with no renewal loop running: every test
/// drives `renew_shard_lease()` itself, exactly as the quota tests drive
/// `renew_all()`.
fn make_service(
    mock: Arc<MockShardManager>,
    shutdown: Shutdown,
) -> (Arc<GrpcShardManagerService>, Arc<ShardServiceDefault>) {
    let shard_service = Arc::new(ShardServiceDefault::new());
    let service = GrpcShardManagerService::new(mock, shard_service.clone(), shutdown);
    (service, shard_service)
}

/// [`make_service`] with the per-attempt deadline floor shrunk, so a test that has to watch a
/// renewal be given up on does not have to wait out the production floor.
fn make_service_with_rpc_deadline_floor(
    mock: Arc<MockShardManager>,
    shutdown: Shutdown,
    floor: Duration,
) -> (Arc<GrpcShardManagerService>, Arc<ShardServiceDefault>) {
    let shard_service = Arc::new(ShardServiceDefault::new());
    let service = GrpcShardManagerService::new_with_rpc_deadline_floor(
        mock,
        shard_service.clone(),
        shutdown,
        floor,
    );
    (service, shard_service)
}

#[test]
// The hook closes over the service graph that owns this service, so a strong reference here
// would be a cycle nothing could free. `WorkerExecutorImpl` owns the hook; this only borrows
// it.
async fn the_service_only_borrows_the_assignment_changed_hook() {
    let mock = Arc::new(MockShardManager::new());
    let (service, _shard_service) = make_service(mock, Shutdown::new());

    let owner = Arc::new(());
    let sentinel = Arc::downgrade(&owner);
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let _captured = owner.clone();
        Box::pin(async move { Ok(RecoveryOutcome::Recovered) })
    });
    service.set_assignment_changed_hook(&hook);
    assert!(
        sentinel.upgrade().is_some(),
        "the hook is alive while its owner holds it"
    );

    drop(hook);
    assert!(
        sentinel.upgrade().is_none(),
        "the shard manager service must not keep the assignment-changed hook alive - holding \
             it would keep the whole service graph alive with it"
    );

    // And an announcement with nothing to announce to is simply skipped, never a panic.
    service.announce_assignment_changed().await;
}

#[test]
// The loop watches the shutdown token while it sleeps *and* while a renewal is in flight. A
// renewal is a round trip that can be followed by a re-registration and a recovery of every
// running agent, so a token only looked at between renewals would let a termination signal
// wait out the grace `main` allows - and the lease would then be handed back by expiry, a
// whole lease period later, rather than by the deregister this loop exists to send.
async fn a_shutdown_during_an_in_flight_renewal_still_deregisters() {
    let expiry = Instant::now() + Duration::from_secs(3);
    // Never released: the renewal is still in the RPC when the token is cancelled.
    let gate = Arc::new(tokio::sync::Notify::new());
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration(expiry, [(0, 1)])))
            .with_renew(move |_, held| {
                Ok(ShardLease {
                    shard_epochs: held,
                    expires_at: expiry,
                    revision: revision_of(MANAGER, 1),
                })
            })
            .with_renew_gate(gate.clone()),
    );
    let shutdown = Shutdown::new();
    let (service, shard_service) = make_service(mock.clone(), shutdown.clone());

    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );

    // The renewal has been sent and is parked in the gate.
    for _ in 0..100 {
        if !mock.renew_calls().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        mock.renew_calls().len(),
        1,
        "the loop should have issued a renewal that is now in flight"
    );
    assert!(mock.deregister_calls().is_empty());

    shutdown.cancel();

    for _ in 0..100 {
        if !mock.deregister_calls().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        mock.deregister_calls().len(),
        1,
        "cancellation must be observed while the renewal is still in flight, not after it"
    );
}

#[test]
// A shard count of zero would reach `ShardId::from_agent_id`, which divides by it, on the
// first routing decision. The registration refuses it, so a misconfigured manager fails this
// executor at startup rather than aborting it later - and installs nothing.
async fn a_registration_granting_zero_shards_is_refused() {
    let expiry = Instant::now() + Duration::from_secs(60);
    let mock = Arc::new(MockShardManager::new().with_register(move |_| {
        Ok(ShardRegistration {
            number_of_shards: 0,
            lease: ShardLease {
                shard_epochs: BTreeMap::new(),
                expires_at: expiry,
                revision: revision_of(MANAGER, 1),
            },
        })
    }));
    let (service, shard_service) = make_service(mock, Shutdown::new());

    let result = service.register(ENDPOINT, None).await;

    assert!(
        matches!(&result, Err(ShardManagerError::ConversionError(message)) if message.contains("must not be 0")),
        "a zero shard count must be refused, got {result:?}"
    );
    assert!(
        shard_service.try_get_current_assignment().is_none(),
        "a refused registration must install nothing"
    );
}

#[test]
// A manager that accepts the renewal and never answers must not hold the loop: the RPC has a
// deadline derived from the lease, after which the pass counts as failed and backs off, so
// the loop keeps renewing (and can still re-register) instead of parking until the transport
// gives up while the lease lapses under it.
async fn a_renewal_that_never_answers_times_out_and_the_loop_keeps_going() {
    let expiry = Instant::now() + Duration::from_secs(3);
    // Never released: every renewal parks in the RPC until its deadline.
    let gate = Arc::new(tokio::sync::Notify::new());
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration(expiry, [(0, 1)])))
            .with_renew(move |_, held| {
                Ok(ShardLease {
                    shard_epochs: held,
                    expires_at: expiry,
                    revision: revision_of(MANAGER, 1),
                })
            })
            .with_renew_gate(gate.clone()),
    );
    // The production deadline floor is measured in seconds so that it always outlasts the
    // shard manager's write budget; a test that has to watch an attempt be abandoned injects a
    // small one instead of waiting it out.
    let (service, shard_service) = make_service_with_rpc_deadline_floor(
        mock.clone(),
        Shutdown::new(),
        Duration::from_millis(200),
    );

    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );

    // Cadence 1 s, deadline 200 ms, first backoff 1 s: a second renewal within a few seconds
    // is only possible if the first one was given up on.
    for _ in 0..160 {
        if mock.renew_calls().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        mock.renew_calls().len() >= 2,
        "a renewal that never answers must time out so the loop can try again"
    );
}

#[test]
// A reply from another shard-manager process is believed because it answers the request this
// executor has just made. That rests on the renewal loop having one request out at a time and
// giving up on it for good at its deadline: a reply from a manager the executor has since
// stopped following, delivered late, would otherwise be adopted and take it back to that
// manager's set.
async fn a_reply_that_arrives_after_its_renewal_was_given_up_on_is_never_applied() {
    let expiry = Instant::now() + Duration::from_secs(3);
    let (old_manager, new_manager) = (Uuid::new_v4(), Uuid::new_v4());
    let late_reply = Arc::new(tokio::sync::Notify::new());
    let renewals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration_from(old_manager, 10, expiry, [(0, 1)])))
            .with_renew({
                let renewals = renewals.clone();
                move |_, _| {
                    let lease = if renewals.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        // The old manager's answer to the first renewal, which the loop has
                        // given up on by the time it is released.
                        ShardLease {
                            shard_epochs: epoch_map([(1, 7)]),
                            expires_at: Instant::now() + Duration::from_secs(3),
                            revision: revision_of(old_manager, 11),
                        }
                    } else {
                        ShardLease {
                            shard_epochs: epoch_map([(0, 2)]),
                            expires_at: Instant::now() + Duration::from_secs(3),
                            revision: revision_of(new_manager, 1),
                        }
                    };
                    Ok(lease)
                }
            })
            .with_first_renewal_gate(late_reply.clone()),
    );
    let (service, shard_service) = make_service_with_rpc_deadline_floor(
        mock.clone(),
        Shutdown::new(),
        Duration::from_millis(200),
    );
    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );

    let follows = |manager: Uuid| {
        shard_service.current_assignment().is_ok_and(|assignment| {
            assignment
                .revision
                .is_some_and(|revision| revision.incarnation == manager)
        })
    };
    for _ in 0..160 {
        if follows(new_manager) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        follows(new_manager),
        "the renewal after the abandoned one must be answered by the new manager"
    );

    // Now the old manager's answer to the abandoned renewal comes back.
    late_reply.notify_waiters();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let assignment = shard_service.current_assignment().unwrap();
    assert_eq!(
        assignment.revision.map(|revision| revision.incarnation),
        Some(new_manager)
    );
    assert_eq!(assignment.shard_epochs, epochs([(0, 2)]));
}

#[test]
// Re-registering after a lost lease starts with the assignment already cleared, so a manager
// that accepts the call and never answers must not hold the executor there: the attempt is
// given up on at the per-attempt deadline and the loop tries again.
async fn a_re_registration_that_never_answers_times_out_and_is_tried_again() {
    let expiry = Instant::now() + Duration::from_secs(3);
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration(expiry, [(0, 1)])))
            .with_renew(|_, _| Err(ShardLeaseError::LeaseNotFound("unknown".to_string()))),
    );
    let (service, shard_service) = make_service_with_rpc_deadline_floor(
        mock.clone(),
        Shutdown::new(),
        Duration::from_millis(200),
    );

    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );
    mock.hang_next_registrations(1);

    // The first renewal is refused, the re-registration it triggers never answers, and only a
    // deadline on it lets the loop reach a second one.
    for _ in 0..160 {
        if mock.register_calls().len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        mock.register_calls().len() >= 3,
        "a re-registration that never answers must time out so the loop can try again; \
             registrations made: {}",
        mock.register_calls().len()
    );
}

#[test]
// Giving a still-loading agent up waits for its whole component load and replay, with no
// timeout. If the renewal loop awaited the assignment-changed hook inline, that sweep would
// hold every later renewal RPC back and the lease would lapse for the whole executor while a
// single agent is loading. The hook here blocks on a flag the test controls, standing in for
// that sweep, and the assertion is that the loop keeps renewing right through it.
async fn a_slow_assignment_changed_hook_does_not_stall_the_renewal_loop() {
    // Short enough that a stalled loop would be obvious within the test's own timeout, but
    // the cadence this derives floors at `MIN_RENEWAL_INTERVAL` (1 s) regardless - see
    // `renewal_interval` - so there is no point going below that.
    let ttl = Duration::from_secs(3);
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration(Instant::now() + ttl, [(0, 1)])))
            .with_renew(move |_, _held| {
                Ok(ShardLease {
                    // Always the widened set: the first grant is a real assignment change,
                    // and every later one only echoes it back (it now matches what was
                    // held), so only the first renewal ever owes an announcement.
                    shard_epochs: epoch_map([(0, 1), (1, 1)]),
                    expires_at: Instant::now() + ttl,
                    revision: revision_of(MANAGER, 1),
                })
            }),
    );
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());

    let hook_calls = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicBool::new(false));
    let calls = hook_calls.clone();
    let hook_released = released.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let calls = calls.clone();
        let released = hook_released.clone();
        Box::pin(async move {
            while !released.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(RecoveryOutcome::Recovered)
        })
    });
    service.set_assignment_changed_hook(&hook);

    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );

    // The first renewal widens the set, so its announcement enters the hook and blocks
    // there - on its own task, not on the loop.
    for _ in 0..200 {
        if !mock.renew_calls().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let renewals_before = mock.renew_calls().len();
    assert!(
        renewals_before >= 1,
        "the loop must have issued the first renewal"
    );

    // Several renewal periods pass (cadence is ~1s) while the hook stays blocked: the loop
    // must keep renewing rather than waiting on the sweep. Each of these renewals also finds
    // `recovery_outstanding` still set by the first
    // one - the sweep has not reported back yet - and asks for the announcement again; that
    // is correct (an unchanged grant re-runs a recovery still owed), and the single-flight
    // coalesces every one of these requests into exactly one rerun, asserted below.
    for _ in 0..100 {
        if mock.renew_calls().len() >= renewals_before + 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let renewals_after = mock.renew_calls().len();
    assert!(
        renewals_after >= renewals_before + 3,
        "further renewals must happen while the hook is blocked; before={renewals_before} \
             after={renewals_after}"
    );
    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        0,
        "the hook must still be blocked at this point"
    );

    // Releasing it lets the sweep complete - and, deterministically, exactly one coalesced
    // rerun right after: at least one of the renewals just observed above landed while the
    // single-flight was already running, which is what put it in its "one more owed" state.
    released.store(true, Ordering::SeqCst);
    for _ in 0..200 {
        if hook_calls.load(Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        2,
        "the blocked sweep and exactly one coalesced rerun, never one rerun per renewal"
    );
}

#[test]
// The single-flight coordinator behind the fix above must not let a burst of requests that
// arrive while one announcement is already running turn into one run per request: exactly one
// more run is owed, coalescing the rest. Drives `AnnouncementSingleFlight` directly, bypassing
// the renewal loop entirely, since that is the unit actually being tested here.
async fn concurrent_announcement_requests_coalesce_into_one_more_run() {
    let mock = Arc::new(MockShardManager::new());
    let (service, _shard_service) = make_service(mock, Shutdown::new());

    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicBool::new(false));
    let hook_calls = calls.clone();
    let hook_started = started.clone();
    let hook_released = released.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let calls = hook_calls.clone();
        let started = hook_started.clone();
        let released = hook_released.clone();
        Box::pin(async move {
            // Only the first call blocks; the coalesced rerun this test looks for must be
            // observable without releasing a second time.
            if started.fetch_add(1, Ordering::SeqCst) == 0 {
                while !released.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(RecoveryOutcome::Recovered)
        })
    });
    service.set_assignment_changed_hook(&hook);

    AnnouncementSingleFlight::request(&service);
    // Wait for the run to actually start (and block) before piling more requests onto it.
    for _ in 0..200 {
        if started.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(started.load(Ordering::SeqCst), 1);

    // A burst of requests while the first run is still blocked.
    for _ in 0..5 {
        AnnouncementSingleFlight::request(&service);
    }

    released.store(true, Ordering::SeqCst);

    for _ in 0..200 {
        if calls.load(Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a burst of requests while one run is in flight must coalesce into exactly one \
             more run"
    );
    // Give a wrongly-spawned extra run a chance to show up before declaring victory.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        2,
        "exactly two runs total: the first, and the one coalesced burst"
    );
}

#[test]
// The deregister has a deadline of its own, sized against the grace `main` waits rather than
// against the lease, so a shard manager that accepts the call and never answers cannot hold
// the process past that grace. The arm it makes reachable is the one that says the lease is
// being left to lapse - and it still clears the local assignment on the way out, so nothing
// is admitted against a lease this executor has stopped renewing.
async fn a_deregister_that_never_answers_gives_up_inside_the_shutdown_grace() {
    let expiry = Instant::now() + Duration::from_secs(300);
    // Never released: the deregister parks in the RPC until its deadline.
    let gate = Arc::new(tokio::sync::Notify::new());
    let mock = Arc::new(
        MockShardManager::new()
            .with_renew(move |_, held| {
                Ok(ShardLease {
                    shard_epochs: held,
                    expires_at: expiry,
                    revision: revision_of(MANAGER, 1),
                })
            })
            .with_deregister_gate(gate.clone()),
    );
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(expiry),
        revision_of(MANAGER, 1),
    );

    let started = std::time::Instant::now();
    service.deregister().await;
    let waited = started.elapsed();

    assert_eq!(
        mock.deregister_calls().len(),
        1,
        "the RPC must have been issued"
    );
    assert!(
        waited < shutdown::SHUTDOWN_GRACE,
        "deregister waited {waited:?}, which does not fit inside the {:?} grace `main` allows",
        shutdown::SHUTDOWN_GRACE
    );
    let assignment = shard_service
        .try_get_current_assignment()
        .expect("the assignment is installed, just cleared");
    assert!(
        assignment.shard_id_set().is_empty() && !shard_service.is_ready(),
        "a deregister that timed out must still clear the assignment locally"
    );
}

#[test]
// A recovery that failed on a renewal is recorded so the next grant repeats it - but an
// `AssignShards` push recovers the same agents on its own way in, so once one has, the record
// has to be cleared. Otherwise every later renewal sweeps again for a failure that has
// already been repaired, and says so in the log.
async fn a_push_that_recovers_agents_clears_a_failed_renewal_recovery() {
    let expiry = Instant::now() + Duration::from_secs(300);
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, _| {
        Ok(ShardLease {
            shard_epochs: epoch_map([(0, 1), (1, 1)]),
            expires_at: expiry,
            revision: revision_of(MANAGER, 2),
        })
    }));
    let (service, shard_service) = make_service(mock, Shutdown::new());
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(expiry),
        revision_of(MANAGER, 1),
    );

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook_calls = calls.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let hook_calls = hook_calls.clone();
        Box::pin(async move {
            hook_calls.fetch_add(1, Ordering::SeqCst);
            Err(anyhow::anyhow!("recovery failed"))
        })
    });
    service.set_assignment_changed_hook(&hook);

    // The corrected set: the hook runs, fails, and the failure is recorded.
    service.renew_shard_lease().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // A push then recovers the agents itself and reports it.
    let ticket = service.recovery_deferred();
    service.recovery_succeeded(ticket);

    // The next grant changes nothing and has nothing outstanding, so the hook stays quiet.
    service.renew_shard_lease().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a recovery already done by a push must not be repeated on the next renewal"
    );
}

#[test]
// A push whose recovery fails is retried by the manager; a corrected renewal has nobody to
// retry it, so the service remembers the failure and runs the recovery again on the next
// grant even though that grant changes nothing - and stops once it has succeeded.
async fn a_failed_recovery_is_retried_on_the_next_grant_even_if_the_set_is_unchanged() {
    let expiry = Instant::now() + Duration::from_secs(300);
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, _| {
        Ok(ShardLease {
            shard_epochs: epoch_map([(0, 1), (1, 1)]),
            expires_at: expiry,
            revision: revision_of(MANAGER, 2),
        })
    }));
    let (service, shard_service) = make_service(mock, Shutdown::new());
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(expiry),
        revision_of(MANAGER, 1),
    );

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook_calls = calls.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let hook_calls = hook_calls.clone();
        Box::pin(async move {
            let call = hook_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err(anyhow::anyhow!("recovery failed"))
            } else {
                Ok(RecoveryOutcome::Recovered)
            }
        })
    });
    service.set_assignment_changed_hook(&hook);

    // The corrected set: the hook runs and fails.
    service.renew_shard_lease().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Unchanged grant: the failed recovery is retried, and succeeds.
    service.renew_shard_lease().await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Unchanged grant, nothing pending: the hook stays quiet.
    service.renew_shard_lease().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a recovery that succeeded must not be run again for an unchanged set"
    );
}

#[test]
// A push that lands while the local lease has lapsed sweeps but does not recover: a fenced
// executor starts nothing. The push reports the recovery as deferred, and the grant that
// revives the lease is what runs it - through the same latch a failed recovery uses.
async fn a_deferred_recovery_runs_on_the_grant_that_revives_the_lease() {
    let expiry = Instant::now() + Duration::from_secs(300);
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, held| {
        Ok(ShardLease {
            shard_epochs: held,
            expires_at: expiry,
            revision: revision_of(MANAGER, 2),
        })
    }));
    let (service, shard_service) = make_service(mock, Shutdown::new());
    // Installed lapsed: the lease ran out while the manager was away.
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(Instant::now()),
        revision_of(MANAGER, 1),
    );
    assert!(!shard_service.is_ready());

    let calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = calls.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let hook_calls = hook_calls.clone();
        Box::pin(async move {
            hook_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RecoveryOutcome::Recovered)
        })
    });
    service.set_assignment_changed_hook(&hook);

    // The push's receipt path found the lease lapsed and deferred the recovery.
    service.recovery_deferred();

    // The grant revives the lease with an unchanged set, and runs the deferred recovery.
    service.renew_shard_lease().await;
    assert!(shard_service.is_ready());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the grant that revives the lease must run the recovery a fenced push deferred"
    );

    // Nothing outstanding: the next unchanged grant stays quiet.
    service.renew_shard_lease().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
// A registration can answer with a nonempty set whose lease, anchored where the request was
// sent, has already lapsed by the time it is installed. The receipt path then defers the
// recovery and latches it - stood in for here by `recovery_deferred()`, the call that path
// makes before anything runs - and the first renewal that revives the lease starts those
// agents even though it changes nothing. Without the latch an unchanged renewal restores
// admission and starts nothing, and the manager's repair push, arriving after that renewal
// advanced the revision, is dropped as stale: the agents are never started at all.
async fn a_registration_whose_lease_arrived_lapsed_is_recovered_by_the_renewal_that_revives_it() {
    let lapsed = Instant::now();
    let live = Instant::now() + Duration::from_secs(300);
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration(lapsed, [(0, 1), (1, 1)])))
            .with_renew(move |_, held| {
                Ok(ShardLease {
                    shard_epochs: held,
                    expires_at: live,
                    revision: revision_of(MANAGER, 2),
                })
            }),
    );
    let (service, shard_service) = make_service(mock, Shutdown::new());

    let calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = calls.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let hook_calls = hook_calls.clone();
        Box::pin(async move {
            hook_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RecoveryOutcome::Recovered)
        })
    });
    service.set_assignment_changed_hook(&hook);

    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );
    assert!(
        !shard_service.is_ready(),
        "the registration's lease had lapsed by the time it was installed"
    );
    service.recovery_deferred();

    service.renew_shard_lease().await;

    assert!(
        shard_service.is_ready(),
        "the unchanged renewal revived the lease"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "and it must start the agents the registration could not"
    );
}

#[test]
// Two recovery attempts can overlap: a renewal's recovery outlasts the lease, a push lands,
// finds the lease lapsed and defers its own. The renewal's attempt then completes - but it
// scanned the set before the push widened it, so it must not retire the push's deferral. A
// flag would; the ticket does not.
async fn a_recovery_that_finishes_after_a_newer_deferral_leaves_that_one_owed() {
    let expiry = Instant::now() + Duration::from_secs(300);
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, held| {
        Ok(ShardLease {
            shard_epochs: held,
            expires_at: expiry,
            revision: revision_of(MANAGER, 2),
        })
    }));
    let (service, shard_service) = make_service(mock, Shutdown::new());
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(expiry),
        revision_of(MANAGER, 1),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = calls.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let hook_calls = hook_calls.clone();
        Box::pin(async move {
            hook_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RecoveryOutcome::Recovered)
        })
    });
    service.set_assignment_changed_hook(&hook);

    let earlier = service.recovery_deferred();
    let _newer = service.recovery_deferred();

    // The earlier attempt completes: the newer one is still owed, so the grant runs it.
    service.recovery_succeeded(earlier);
    service.renew_shard_lease().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the deferral registered after the completed attempt must still be run"
    );

    // That run was the newest attempt, so nothing is owed any more.
    service.renew_shard_lease().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
// The hook can report a deferral too, and it is recorded the same way a failure is.
async fn a_recovery_the_hook_deferred_is_run_again_on_the_next_grant() {
    let expiry = Instant::now() + Duration::from_secs(300);
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, _| {
        Ok(ShardLease {
            shard_epochs: epoch_map([(0, 1), (1, 1)]),
            expires_at: expiry,
            revision: revision_of(MANAGER, 2),
        })
    }));
    let (service, shard_service) = make_service(mock, Shutdown::new());
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(expiry),
        revision_of(MANAGER, 1),
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let hook_calls = calls.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let hook_calls = hook_calls.clone();
        Box::pin(async move {
            let call = hook_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Ok(RecoveryOutcome::DeferredUntilLeaseIsLive)
            } else {
                Ok(RecoveryOutcome::Recovered)
            }
        })
    });
    service.set_assignment_changed_hook(&hook);

    // The corrected set: the hook runs and defers.
    service.renew_shard_lease().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Unchanged grant: the deferred recovery is run, and succeeds.
    service.renew_shard_lease().await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Unchanged grant, nothing pending.
    service.renew_shard_lease().await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// A never-expiring lease is the single-shard service's, and it has nothing
/// to renew: the cadence is `None`, which the loop's interval arm treats as
/// pending forever so that only shutdown can fire.
#[test]
async fn a_never_expiring_lease_has_no_cadence_and_parks_the_interval_arm() {
    assert_eq!(
        renewal_interval_for(None, Instant::now()),
        None,
        "a never-expiring lease must not be polled at all"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), sleep_or_park(None))
            .await
            .is_err(),
        "the parked interval arm must never fire"
    );
}

/// Cross-track contract: `RenewShardLeaseRequest.shard_epochs` is exactly
/// the set last received, and the granted expiry replaces the local one.
#[test]
async fn a_renewal_sends_the_last_received_set_and_adopts_the_granted_expiry() {
    let granted_expiry = Instant::now() + Duration::from_secs(300);
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, held| {
        Ok(ShardLease {
            shard_epochs: held,
            expires_at: granted_expiry,
            revision: revision_of(MANAGER, 1),
        })
    }));
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
    shard_service.register(
        SHARDS,
        &epochs([(0, 7), (3, 2)]),
        Some(Instant::now() + Duration::from_secs(10)),
        revision_of(MANAGER, 1),
    );

    let delay = service.renew_shard_lease().await;

    let calls = mock.renew_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].1,
        epoch_map([(0, 7), (3, 2)]),
        "the held epochs must be exactly the set last received, epochs included"
    );
    let assignment = shard_service.current_assignment().unwrap();
    assert_eq!(assignment.expires_at, Some(granted_expiry));
    assert_eq!(assignment.shard_epochs, epochs([(0, 7), (3, 2)]));
    let delay = delay.expect("a granted lease has a cadence");
    assert!(
        delay > Duration::from_secs(90) && delay <= Duration::from_secs(100),
        "the next pass is one third of the granted TTL, got {delay:?}"
    );
}

/// An unknown lease clears the assignment (leaving
/// it lapsed), re-registers under a fresh UUID, and announces the new
/// assignment so running agents are recovered.
#[test]
async fn a_lost_lease_re_registers_with_a_fresh_uuid_and_announces_the_new_assignment() {
    let fresh_expiry = Instant::now() + Duration::from_secs(120);
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration(fresh_expiry, [(2, 5)])))
            .with_renew(|_, _| Err(ShardLeaseError::LeaseNotFound("unknown".to_string()))),
    );
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());

    let announced = Arc::new(AtomicBool::new(false));
    let flag = announced.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let flag = flag.clone();
        Box::pin(async move {
            flag.store(true, Ordering::SeqCst);
            Ok(RecoveryOutcome::Recovered)
        })
    });
    service.set_assignment_changed_hook(&hook);

    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );
    let original_executor_id = mock.register_calls()[0];

    service.renew_shard_lease().await;

    let register_calls = mock.register_calls();
    assert_eq!(register_calls.len(), 2, "a lost lease must re-register");
    assert_ne!(
        register_calls[1], original_executor_id,
        "the re-registration must come back as a new instance, under a fresh UUID"
    );
    assert!(
        announced.load(Ordering::SeqCst),
        "the fresh grant must fire on_shard_assignment_changed"
    );
    let assignment = shard_service.current_assignment().unwrap();
    assert_eq!(assignment.shard_epochs, epochs([(2, 5)]));
    assert_eq!(assignment.expires_at, Some(fresh_expiry));
}

/// The re-registration after a lost lease names the executor at the same endpoint as the first
/// registration, so an advertised address survives the shard manager forgetting the executor.
#[test]
async fn a_re_registration_keeps_the_advertised_endpoint() {
    let expiry = Instant::now() + Duration::from_secs(120);
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration(expiry, [(2, 5)])))
            .with_renew(|_, _| Err(ShardLeaseError::LeaseNotFound("unknown".to_string()))),
    );
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
    let advertised = ExecutorEndpoint {
        port: 9000,
        advertised_ip: Some("fdaa:0:1:a7b::2".parse().unwrap()),
    };

    let assignment = service.register(advertised, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );
    service.renew_shard_lease().await;

    assert_eq!(mock.register_endpoints(), vec![advertised, advertised]);
}

/// A shard manager that came back on a wiped store no longer lists this executor, so the
/// renewal is refused and the re-registration carries the epochs for it to repair. Its answer
/// is read off a store that counts from the beginning again, far below the last revision this
/// executor applied, and must be adopted all the same.
#[test]
async fn a_re_registration_with_a_manager_that_lost_its_history_adopts_its_low_revision() {
    let expiry = Instant::now() + Duration::from_secs(120);
    let (old_manager, new_manager) = (Uuid::new_v4(), Uuid::new_v4());
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempt = attempts.clone();
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| match attempt.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(registration_from(old_manager, 10_000, expiry, [(2, 5)])),
                // A manager on a new store: its revisions start over, far below the last one.
                _ => Ok(registration_from(new_manager, 3, expiry, [(2, 6)])),
            })
            .with_renew(|_, _| Err(ShardLeaseError::LeaseNotFound("unknown".to_string()))),
    );
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );

    service.renew_shard_lease().await;

    let assignment = shard_service.current_assignment().unwrap();
    assert_eq!(
        assignment.shard_epochs,
        epochs([(2, 6)]),
        "the new manager's grant was dropped as older than the last delivery applied"
    );
    assert_eq!(assignment.revision, Some(revision_of(new_manager, 3)));
}

/// A shard manager restored from a backup still lists this executor, so it grants the renewal
/// - from a store whose revisions are far below the last one this executor applied. The
/// manager it replaced must not get a delivery in afterwards on the strength of its higher
/// revisions.
#[test]
async fn a_renewal_answered_by_a_manager_restored_from_a_backup_adopts_its_low_revision() {
    let expiry = Instant::now() + Duration::from_secs(120);
    let (old_manager, new_manager) = (Uuid::new_v4(), Uuid::new_v4());
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration_from(old_manager, 10_000, expiry, [(2, 5)])))
            .with_renew(move |_, _| {
                Ok(ShardLease {
                    shard_epochs: epoch_map([(2, 6)]),
                    expires_at: expiry,
                    revision: revision_of(new_manager, 3),
                })
            }),
    );
    let (service, shard_service) = make_service(mock, Shutdown::new());
    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );

    service.renew_shard_lease().await;

    let assignment = shard_service.current_assignment().unwrap();
    assert_eq!(
        assignment.shard_epochs,
        epochs([(2, 6)]),
        "the repaired grant was dropped as older than the last delivery applied"
    );
    assert_eq!(assignment.revision, Some(revision_of(new_manager, 3)));

    let delayed = shard_service
        .assign_shards(SHARDS, &epochs([(2, 5)]), revision_of(old_manager, 10_001))
        .unwrap();
    assert!(
        matches!(delayed, ShardDeliveryOutcome::FromAnotherManager { .. }),
        "got {delayed:?}"
    );
    assert_eq!(
        shard_service.current_assignment().unwrap().shard_epochs,
        epochs([(2, 6)])
    );
}

/// A push from a shard manager process this executor does not follow is ignored, and what it
/// owes instead is a renewal now rather than a third of a lease from now: the answer names
/// the process in charge and carries its set.
#[test]
async fn renew_now_runs_a_renewal_ahead_of_its_timer() {
    let expiry = Instant::now() + Duration::from_secs(120);
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(move |_| Ok(registration(expiry, [(0, 1)])))
            .with_renew(move |_, held| {
                Ok(ShardLease {
                    shard_epochs: held,
                    expires_at: expiry,
                    revision: revision_of(MANAGER, 1),
                })
            }),
    );
    let shutdown = Shutdown::new();
    let (service, shard_service) = make_service(mock.clone(), shutdown.clone());
    let assignment = service.register(ENDPOINT, None).await.unwrap();
    shard_service.register(
        assignment.number_of_shards,
        &assignment.shard_epochs,
        assignment.expires_at,
        assignment.revision.expect("a grant names its revision"),
    );
    assert!(
        mock.renew_calls().is_empty(),
        "the timer is a third of the lease away"
    );

    service.renew_now();

    for _ in 0..100 {
        if !mock.renew_calls().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(mock.renew_calls().len(), 1);
    shutdown.cancel();
}

/// A renewal that answers with shards this executor did not send is the
/// shard manager correcting a push that never arrived, so it has to recover
/// agents for them exactly as a push would. The common path — the same set
/// back, because a renewal never advances an epoch — must NOT fire the
/// hook, or every renewal would trigger a
/// recovery sweep.
#[test]
async fn a_renewal_that_changes_the_set_announces_it_and_an_unchanged_one_does_not() {
    let granted_expiry = Instant::now() + Duration::from_secs(300);
    // `None` echoes the held set; `Some` is the manager correcting it.
    let correction: Arc<StdMutex<Option<BTreeMap<ShardId, ShardEpoch>>>> =
        Arc::new(StdMutex::new(None));
    let correct = correction.clone();
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, held| {
        let shard_epochs = correct.lock().unwrap().clone().unwrap_or(held);
        Ok(ShardLease {
            shard_epochs,
            expires_at: granted_expiry,
            revision: revision_of(MANAGER, 1),
        })
    }));
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
    shard_service.register(
        SHARDS,
        &epochs([(0, 7), (3, 2)]),
        Some(Instant::now() + Duration::from_secs(30)),
        revision_of(MANAGER, 1),
    );

    let announced = Arc::new(AtomicUsize::new(0));
    let count = announced.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let count = count.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(RecoveryOutcome::Recovered)
        })
    });
    service.set_assignment_changed_hook(&hook);

    service.renew_shard_lease().await;
    assert_eq!(
        announced.load(Ordering::SeqCst),
        0,
        "an unchanged set is a lease-clock refresh, not an assignment change"
    );

    // wider: a shard this executor never knew it owned
    *correction.lock().unwrap() = Some(epoch_map([(0, 7), (3, 2), (4, 9)]));
    service.renew_shard_lease().await;
    assert_eq!(
        announced.load(Ordering::SeqCst),
        1,
        "a widened set must recover agents for the shards it just gained"
    );
    assert_eq!(
        shard_service.current_assignment().unwrap().shard_epochs,
        epochs([(0, 7), (3, 2), (4, 9)])
    );

    // narrower: a shard the manager has given to someone else
    *correction.lock().unwrap() = Some(epoch_map([(0, 7), (4, 9)]));
    service.renew_shard_lease().await;
    assert_eq!(
        announced.load(Ordering::SeqCst),
        2,
        "a narrowed set must sweep the agents on the shard it just lost"
    );
    assert_eq!(
        shard_service.current_assignment().unwrap().shard_epochs,
        epochs([(0, 7), (4, 9)]),
        "and the narrower set is what the executor now serves"
    );
}

/// Two deliveries can cross on the network. A renewal reply read from an
/// older state than a push the executor has already applied must not put
/// the older set back, nor announce that as a change. Its lease still
/// counts: it answers this executor's own request, and a push carries no
/// lease that could have made it redundant.
#[test]
async fn a_stale_renewal_keeps_the_pushed_set_and_moves_the_lease_clock() {
    let granted_expiry = Instant::now() + Duration::from_secs(300);
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, _| {
        Ok(ShardLease {
            shard_epochs: epoch_map([(0, 7)]),
            expires_at: granted_expiry,
            revision: revision_of(MANAGER, 4),
        })
    }));
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
    // the push at revision 5 landed first and widened the set
    shard_service.register(
        SHARDS,
        &epochs([(0, 7), (4, 9)]),
        Some(Instant::now() + Duration::from_secs(30)),
        revision_of(MANAGER, 5),
    );

    let announced = Arc::new(AtomicBool::new(false));
    let flag = announced.clone();
    let hook: ShardAssignmentChangedHook = Arc::new(move || {
        let flag = flag.clone();
        Box::pin(async move {
            flag.store(true, Ordering::SeqCst);
            Ok(RecoveryOutcome::Recovered)
        })
    });
    service.set_assignment_changed_hook(&hook);

    service.renew_shard_lease().await;

    let assignment = shard_service.current_assignment().unwrap();
    assert_eq!(
        assignment.shard_epochs,
        epochs([(0, 7), (4, 9)]),
        "the older renewal narrowed the set the newer push had just widened"
    );
    assert_eq!(
        assignment.expires_at,
        Some(granted_expiry),
        "the set is stale; the lease is not, and it answers this executor's own request"
    );
    assert_eq!(assignment.revision, Some(revision_of(MANAGER, 5)));
    assert!(
        !announced.load(Ordering::SeqCst),
        "an ignored delivery is not an assignment change"
    );
}

/// While the re-registration has not succeeded, the cleared
/// assignment must read as *lapsed*, not as never-expiring, so admission
/// keeps refusing.
#[test]
async fn a_lost_lease_leaves_the_assignment_lapsed_until_a_grant_replaces_it() {
    let mock = Arc::new(
        MockShardManager::new()
            .with_register(|_| {
                Err(ShardManagerError::InternalServerError(
                    "shard manager down".to_string(),
                ))
            })
            .with_renew(|_, _| Err(ShardLeaseError::LeaseNotFound("unknown".to_string()))),
    );
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
    // Registration arguments the re-register repeats; the client fails, so
    // nothing replaces the cleared assignment.
    assert!(service.register(ENDPOINT, None).await.is_err());
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(Instant::now() + Duration::from_secs(60)),
        revision_of(MANAGER, 1),
    );

    service.renew_shard_lease().await;

    let assignment = shard_service.current_assignment().unwrap();
    assert!(assignment.is_empty(), "every shard is dropped");
    assert!(
        assignment.expires_at.is_some(),
        "cleared means lapsed, never 'never expires'"
    );
    assert!(!shard_service.is_ready());
    assert!(matches!(
        shard_service.check_admission(&agent_on_shard(0)),
        Err(WorkerExecutorError::ShardingNotReady)
    ));
}

/// A transport failure changes nothing locally, so the executor keeps
/// serving until its own `expires_at` passes and then fences itself.
#[test]
async fn a_transport_failure_serves_until_the_local_expiry_and_then_fences() {
    let mock = Arc::new(MockShardManager::new().with_renew(|_, _| {
        Err(ShardLeaseError::InternalServerError(
            "unreachable".to_string(),
        ))
    }));
    let (service, shard_service) = make_service(mock.clone(), Shutdown::new());
    let agent = agent_on_shard(0);
    // The window has to outlast a descheduling of this thread under a loaded
    // test run: the assert below is only meaningful while the granted lease is
    // still live, so it gets a second of slack.
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(Instant::now() + Duration::from_secs(1)),
        revision_of(MANAGER, 1),
    );

    let first = service.renew_shard_lease().await;
    assert_eq!(first, Some(Duration::from_secs(1)));
    assert!(
        shard_service.check_admission(&agent).is_ok(),
        "still inside the lease the executor was granted"
    );

    tokio::time::sleep(Duration::from_millis(1300)).await;

    let second = service.renew_shard_lease().await;
    assert_eq!(second, Some(Duration::from_secs(2)), "and it keeps trying");
    assert_eq!(mock.renew_calls().len(), 2);
    assert!(
        matches!(
            shard_service.check_admission(&agent),
            Err(WorkerExecutorError::ShardingNotReady)
        ),
        "past the local expiry the self-fence refuses admission"
    );
    assert_eq!(
        shard_service.current_assignment().unwrap().shard_id_set(),
        HashSet::from([ShardId::new(0)]),
        "without dropping the shards: the fence refuses new work and leaves the set alone"
    );
}

/// Failed renewals back off exponentially from 1 s, capped by
/// `min(last granted TTL / 3, 30 s)`, and a grant resets the backoff.
#[test]
async fn failed_renewals_back_off_exponentially_and_reset_on_a_grant() {
    // Fails until told otherwise; the grant it then hands out has a 30 s
    // TTL, so the cap afterwards is that TTL / 3 rather than the ceiling.
    let grant_at_call = Arc::new(AtomicUsize::new(8));
    let calls = Arc::new(AtomicUsize::new(0));
    let grant_at = grant_at_call.clone();
    let seen = calls.clone();
    let mock = Arc::new(MockShardManager::new().with_renew(move |_, held| {
        let call = seen.fetch_add(1, Ordering::SeqCst) + 1;
        if call == grant_at.load(Ordering::SeqCst) {
            Ok(ShardLease {
                shard_epochs: held,
                expires_at: Instant::now() + Duration::from_secs(30),
                revision: revision_of(MANAGER, 1),
            })
        } else {
            Err(ShardLeaseError::InternalServerError("down".to_string()))
        }
    }));
    let (service, shard_service) = make_service(mock, Shutdown::new());
    shard_service.register(
        SHARDS,
        &epochs([(0, 1)]),
        Some(Instant::now() + Duration::from_secs(300)),
        revision_of(MANAGER, 1),
    );

    let mut delays = Vec::new();
    for _ in 0..7 {
        delays.push(service.renew_shard_lease().await);
    }
    assert_eq!(
        delays,
        vec![1u64, 2, 4, 8, 16, 30, 30]
            .into_iter()
            .map(|secs| Some(Duration::from_secs(secs)))
            .collect::<Vec<_>>(),
        "the backoff doubles from 1 s and stops at the 30 s ceiling"
    );

    // Call 8 is granted: a 30 s lease.
    let cadence = service
        .renew_shard_lease()
        .await
        .expect("a granted lease has a cadence");
    assert!(
        cadence > Duration::from_secs(9) && cadence <= Duration::from_secs(10),
        "the cadence is one third of the granted TTL, got {cadence:?}"
    );

    let mut after_grant = Vec::new();
    for _ in 0..6 {
        after_grant.push(service.renew_shard_lease().await.unwrap());
    }
    assert_eq!(
        after_grant[0],
        Duration::from_secs(1),
        "a grant resets the backoff to its first step"
    );
    assert_eq!(after_grant[1], Duration::from_secs(2));
    assert_eq!(after_grant[2], Duration::from_secs(4));
    assert_eq!(after_grant[3], Duration::from_secs(8));
    assert!(
        after_grant[4] > Duration::from_secs(9) && after_grant[4] <= Duration::from_secs(10),
        "and the ceiling now follows the granted TTL / 3, got {:?}",
        after_grant[4]
    );
    assert_eq!(after_grant[5], after_grant[4], "where it stays");
}
