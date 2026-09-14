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

use super::proxy::BreakableProxy;
use chrono::DateTime;
use etcd_client::{Client, ConnectOptions, EventType, GetOptions, WatchOptions};
use golem_shard_manager::LEADER_ELECTION_NAME;
use golem_shard_manager::config::EtcdConfig;
use golem_shard_manager::{
    Elected, EtcdRoutingTablePersistence, ExecutorAddr, ExecutorId, LeaderElection, LeaderFence,
    LeaseKeepAlive, LeaseLossReason, NO_REVISION, RoutingTablePersistence, STATE_KEY,
    ShardLeaseState, ShardManagerError,
};
use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tokio::sync::{Notify, oneshot};
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

inherit_test_dep!(Arc<DockerEtcd>);

/// etcd clamps anything below its 2s `MinLeaseTTL` up, so 2s is the shortest TTL a test can
/// request and still reason about.
const TEST_LEASE_TTL: Duration = Duration::from_secs(2);

/// Far shorter than any campaign this file waits through, so a request timeout leaking onto the
/// election channel fails instantly rather than flakily.
const TEST_REQUEST_TIMEOUT: Duration = Duration::from_millis(500);

const TEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Hard stop for a campaign that should win promptly: a failed test rather than a hung suite.
const CAMPAIGN_TIMEOUT: Duration = Duration::from_secs(15);

fn election_config(etcd: &DockerEtcd) -> EtcdConfig {
    // Exhaustive on purpose: `..Default::default()` would let a future field reintroduce a timeout
    // this file depends on controlling.
    EtcdConfig {
        endpoints: vec![etcd.client_url()],
        connect_timeout: TEST_CONNECT_TIMEOUT,
        request_timeout: TEST_REQUEST_TIMEOUT,
        leader_lease_ttl: TEST_LEASE_TTL,
    }
}

/// A per-test election name, so tests sharing one etcd server never contend.
///
/// A sibling of the production name, never a child: etcd scans the prefix `<name>/`, so a nested
/// name would put this campaigner inside the production election's key space.
fn unique_election_name() -> String {
    format!("{LEADER_ELECTION_NAME}-test-{}", Uuid::new_v4())
}

/// A client for looking at what is actually in etcd, separate from anything under test.
///
/// Bounded, unlike the election channel: a hung etcd should fail the test rather than the suite.
async fn inspection_client(etcd: &DockerEtcd) -> Client {
    let options = ConnectOptions::new()
        .with_connect_timeout(TEST_CONNECT_TIMEOUT)
        .with_timeout(TEST_REQUEST_TIMEOUT);

    Client::connect([etcd.client_url()], Some(options))
        .await
        .expect("Cannot connect an inspection client to etcd")
}

/// Waits for a second campaigner to appear under the election prefix, and returns the lease its key
/// is attached to.
///
/// etcd creates one key per campaigner, so the key that is not the leader's is the standby's.
async fn await_standby_lease(etcd: &DockerEtcd, election_name: &str, leader_key: &[u8]) -> i64 {
    let deadline = Instant::now() + CAMPAIGN_TIMEOUT;
    let mut kv = inspection_client(etcd).await.kv_client();

    while Instant::now() < deadline {
        let response = kv
            .get(
                format!("{election_name}/"),
                Some(GetOptions::new().with_prefix()),
            )
            .await
            .expect("Reading the election prefix should succeed");

        if let Some(standby) = response.kvs().iter().find(|kv| kv.key() != leader_key) {
            return standby.lease();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!(
        "No campaigner other than the leader appeared under `{election_name}/` within \
         {CAMPAIGN_TIMEOUT:?}"
    )
}

/// Asserts that the leadership a campaign reported winning actually exists in etcd, at the exact
/// creation revision the fence will compare against.
///
/// etcd's `Campaign` never re-checks the campaigner's own key, so a replica whose lease expired
/// mid-campaign is still told it won and gets a fence over a key that does not exist.
async fn assert_fence_is_live(etcd: &DockerEtcd, fence: &LeaderFence, whose: &str) {
    let response = inspection_client(etcd)
        .await
        .kv_client()
        .get(fence.key(), None)
        .await
        .expect("Reading the leader key back should succeed");

    let kv = response.kvs().first().unwrap_or_else(|| {
        panic!(
            "{whose} was told it won the election, but its leader key `{}` does not exist in etcd. \
             The lease backing it expired before the campaign returned - most likely because the \
             keepalive was not running *during* the campaign. Every fenced write from this replica \
             would fail, and only in distributed mode.",
            fence.key_str()
        )
    });

    assert_eq!(
        kv.create_revision(),
        fence.create_revision(),
        "{whose}'s leader key `{}` exists but was created at revision {}, while its fence compares \
         against {}. The fence would reject every write.",
        fence.key_str(),
        kv.create_revision(),
        fence.create_revision()
    );
}

#[test]
#[tracing::instrument(skip_all)]
// Smoke test: v3election is reachable on the pinned image and a win has the shape the rest of the
// design assumes - a real lease, and a fence over this campaigner's own key at its creation
// revision.
async fn an_uncontested_campaign_is_won_immediately(etcd: &Arc<DockerEtcd>) {
    let election_name = unique_election_name();
    let election = LeaderElection::connect(&election_config(etcd), election_name.clone())
        .await
        .expect("Connecting the election client to etcd should succeed");

    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "An uncontested campaign on the fresh name `{election_name}` did not finish within \
                 {CAMPAIGN_TIMEOUT:?}. Nothing else holds this name, so either v3election is not \
                 answering on this image, or the campaign is being aborted and retried forever."
            )
        })
        .expect("Winning an uncontested campaign should succeed");

    assert_ne!(
        elected.lease_id, 0,
        "The campaign returned lease id 0, etcd's no-lease sentinel: the leadership key would \
         never expire and no standby could ever take over."
    );

    let create_revision = elected.fence.create_revision();
    assert!(
        create_revision > 0,
        "The fence's create_revision is {create_revision}. Revision 0 is reserved for absent keys, \
         so the fence would invert into 'this key must NOT exist' and every state write would pass \
         exactly when leadership had been lost."
    );
    // Revisions are a small counter on a fresh server and lease ids large generated values, so
    // equality means the fence was built from the wrong field.
    assert_ne!(
        create_revision, elected.lease_id,
        "The fence's create_revision equals the lease id, so it is carrying the lease rather than \
         the leader key's creation revision."
    );

    // The election name is only a prefix, and is never itself a key.
    let fence_key = elected.fence.key_str();
    let prefix = format!("{election_name}/");
    assert!(
        fence_key.starts_with(&prefix),
        "The fence key is `{fence_key}`, but it must be this campaigner's own key under \
         `{prefix}`. A fence over the bare election name compares a nonexistent key, so every \
         fenced write would fail - silently, and only in distributed mode."
    );

    assert!(
        elected.granted_ttl >= TEST_LEASE_TTL,
        "etcd granted a {:?} lease for a {TEST_LEASE_TTL:?} request. etcd only ever clamps a TTL \
         upwards, and the keepalive watchdog arms its deadline from the granted value - a shorter \
         grant would let the lease expire before the watchdog declared it lost.",
        elected.granted_ttl
    );

    info!(
        fence_key,
        create_revision,
        lease_id = elected.lease_id,
        granted_ttl = ?elected.granted_ttl,
        "Uncontested campaign won"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// Requesting below etcd's 2s floor is the only way to observe the granted-TTL plumbing: at exactly
// 2s, reporting back the requested value is indistinguishable from reporting etcd's answer.
async fn a_below_floor_lease_ttl_is_reported_as_the_clamped_value(etcd: &Arc<DockerEtcd>) {
    let below_floor = Duration::from_secs(1);
    let config = EtcdConfig {
        leader_lease_ttl: below_floor,
        ..election_config(etcd)
    };

    let election = LeaderElection::connect(&config, unique_election_name())
        .await
        .expect("Connecting the election client to etcd should succeed");
    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");

    assert!(
        elected.granted_ttl > below_floor,
        "Requested a {below_floor:?} lease and `granted_ttl` came back as {:?}. etcd clamps below \
         its 2s MinLeaseTTL, so reporting the requested value back means the grant response is \
         being ignored - and the keepalive watchdog would then arm a deadline shorter than the \
         lease etcd actually issued.",
        elected.granted_ttl
    );
    info!(requested = ?below_floor, granted = ?elected.granted_ttl, "etcd clamped the lease TTL up");
}

#[test]
#[tracing::instrument(skip_all)]
// Proves two things at once. A request timeout on the election channel would ERROR B's campaign
// long before the head start elapses; a keepalive started only after winning would let B's own
// lease expire mid-campaign, so the win it is handed is over a key etcd deleted - visible here as
// an election on a lease other than the one it campaigned on.
async fn campaign_survives_much_longer_than_the_lease_ttl(etcd: &Arc<DockerEtcd>) {
    // Comfortably more than one lease TTL, so B must be renewing from *inside* its campaign.
    let head_start = TEST_LEASE_TTL * 3;
    assert!(
        head_start > TEST_REQUEST_TIMEOUT * 2,
        "the head start must be long enough that a request timeout on the election channel would \
         abort B's campaign well before it elapses, or this test proves nothing about finding #1"
    );

    let election_name = unique_election_name();
    let config = election_config(etcd);

    let leader_a = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica A should succeed");
    let elected_a = timeout(CAMPAIGN_TIMEOUT, leader_a.campaign_until_elected())
        .await
        .expect("A's campaign should not hang")
        .expect("A should win the uncontested campaign");
    let Elected {
        keepalive: keepalive_a,
        fence: fence_a,
        ..
    } = elected_a;

    // A only keeps leadership while something is renewing its lease.
    let keepalive_a = tokio::spawn(keepalive_a.run());

    let replica_b = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica B should succeed");
    let mut campaign_b = tokio::spawn(async move { replica_b.campaign_until_elected().await });

    // The lease B is campaigning on right now, so that the win below can be tied back to it.
    let b_campaign_lease_id = await_standby_lease(etcd, &election_name, fence_a.key()).await;

    // Polling `&mut handle` observes without consuming, so B's campaign continues afterwards.
    match timeout(head_start, &mut campaign_b).await {
        Err(_) => {} // still campaigning after 3x the lease TTL - correct
        Ok(Ok(Ok(_))) => panic!(
            "Standby B was elected while A still held the leadership and was renewing its lease. \
             Two replicas would both believe they are the leader."
        ),
        Ok(Ok(Err(err))) => panic!(
            "Standby B's campaign FAILED after less than {head_start:?} instead of waiting: {err}. \
             This is the signature of a request timeout on the election channel - \
             `Election.Campaign` is a unary RPC etcd does not answer until the caller wins, so an \
             endpoint timeout aborts it. Check that `connect_for_election` still applies no \
             request timeout and that nothing hands it a clone of the request client."
        ),
        Ok(Err(join_err)) => panic!("Standby B's campaign task panicked: {join_err}"),
    }

    // Release A by dropping its keepalive: the lease then expires on its own TTL.
    keepalive_a.abort();

    let elected_b = timeout(CAMPAIGN_TIMEOUT, campaign_b)
        .await
        .expect("B should be elected once A's lease expires")
        .expect("B's campaign task should not panic")
        .expect("B's campaign should succeed once the leadership is free");
    let Elected {
        keepalive: keepalive_b,
        fence: fence_b,
        lease_id: b_elected_lease_id,
        ..
    } = elected_b;

    // Before the assertions, not after: B's 2s lease could otherwise expire underneath
    // `assert_fence_is_live` and fail the test for a reason that is not the one under test.
    let _keepalive_b = tokio::spawn(keepalive_b.run());

    assert_eq!(
        b_elected_lease_id, b_campaign_lease_id,
        "B was elected on lease {b_elected_lease_id:#x} but campaigned on \
         {b_campaign_lease_id:#x}, so its first lease did not survive the wait. etcd's election \
         server orphans the session it builds from a campaigner's lease, so nothing renews it \
         except this replica's own keepalive - which therefore has to run alongside the campaign, \
         not after it."
    );

    assert!(
        fence_b.create_revision() > fence_a.create_revision(),
        "B's leader key was created at revision {} but A's was {}. A later election must produce a \
         strictly later creation revision, which is what makes the fence a fence.",
        fence_b.create_revision(),
        fence_a.create_revision()
    );

    assert_fence_is_live(etcd, &fence_b, "Standby B").await;
}

#[test]
#[tracing::instrument(skip_all)]
// The kill must let A's lease EXPIRE, never revoke or resign it, or this measures a graceful
// handover and stays green with expiry-driven failover broken. Aborting works because
// `LeaseKeeper` has no `Drop`: dropping its sender ends the stream and etcd sees EOF. A `Drop`
// added to `LeaseKeepAlive`, or a graceful resign, is what the lower-bound assertion catches.
async fn a_standby_is_elected_after_the_leader_lease_expires_without_a_revoke(
    etcd: &Arc<DockerEtcd>,
) {
    let election_name = unique_election_name();
    let config = election_config(etcd);

    let leader_a = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica A should succeed");
    let elected_a = timeout(CAMPAIGN_TIMEOUT, leader_a.campaign_until_elected())
        .await
        .expect("A's campaign should not hang")
        .expect("A should win the uncontested campaign");
    let granted_ttl = elected_a.granted_ttl;
    let fence_a_revision = elected_a.fence.create_revision();
    let keepalive_a = tokio::spawn(elected_a.keepalive.run());

    let replica_b = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica B should succeed");
    let campaign_b = tokio::spawn(async move { replica_b.campaign_until_elected().await });

    // Let B block on a genuinely held leadership before the kill.
    tokio::time::sleep(granted_ttl / 2).await;
    assert!(
        !campaign_b.is_finished(),
        "B finished while A was still renewing - it should have been blocked behind A."
    );

    // The kill. No revoke, no resign: just stop renewing.
    let killed_at = Instant::now();
    keepalive_a.abort();

    let elected_b = timeout(CAMPAIGN_TIMEOUT, campaign_b)
        .await
        .expect("B should be elected within the campaign timeout after A stops renewing")
        .expect("B's campaign task should not panic")
        .expect("B's campaign should succeed once A's lease expires");
    let elapsed = killed_at.elapsed();

    // The bound is TTL minus one renewal interval, not the full TTL: etcd measures expiry from the
    // last renewal it received, and the abort can land a whole interval after that one, so an
    // honest failover is as short as `ttl - interval`. A revoke frees the leadership immediately.
    let renewal_interval = LeaseKeepAlive::renewal_interval(granted_ttl);
    let earliest_honest_failover = granted_ttl - renewal_interval;
    assert!(
        elapsed >= earliest_honest_failover,
        "B was elected {elapsed:?} after A stopped renewing, sooner than the \
         {earliest_honest_failover:?} floor for a lease that has to expire on its own \
         ({granted_ttl:?} TTL less one {renewal_interval:?} renewal interval). Something revoked \
         or resigned the lease - most likely a `Drop` impl added to `LeaseKeepAlive`, or a \
         graceful resign on shutdown. This test then measures a graceful handover and would pass \
         even if expiry-driven failover were broken."
    );

    assert!(
        elected_b.fence.create_revision() > fence_a_revision,
        "B's leader key creation revision ({}) must be strictly later than A's ({}).",
        elected_b.fence.create_revision(),
        fence_a_revision
    );

    info!(failover = ?elapsed, lease_ttl = ?granted_ttl, "Standby elected after lease expiry");
}

#[test]
#[tracing::instrument(skip_all)]
// A leader asked to stop must *release* its lease rather than let it lapse, or every rolling
// restart costs the deployment one lease TTL with no routing table.
async fn a_leader_that_steps_down_hands_over_before_its_lease_could_expire(etcd: &Arc<DockerEtcd>) {
    let election_name = unique_election_name();
    let config = election_config(etcd);

    let leader_a = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica A should succeed");
    let elected_a = timeout(CAMPAIGN_TIMEOUT, leader_a.campaign_until_elected())
        .await
        .expect("A's campaign should not hang")
        .expect("A should win the uncontested campaign");
    let granted_ttl = elected_a.granted_ttl;
    let a_lease_id = elected_a.lease_id;
    let fence_a = elected_a.fence.clone();
    let leadership_a = elected_a.leadership.clone();
    let keepalive_a = tokio::spawn(elected_a.keepalive.run());

    let replica_b = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica B should succeed");
    let campaign_b = tokio::spawn(async move { replica_b.campaign_until_elected().await });

    // B has to be queued behind a leadership A genuinely holds, or the handover below is measuring
    // an uncontested campaign.
    await_standby_lease(etcd, &election_name, fence_a.key()).await;

    let handed_over_at = Instant::now();
    leadership_a
        .step_down()
        .await
        .expect("Stepping down from a held leadership should succeed");

    let elected_b = timeout(CAMPAIGN_TIMEOUT, campaign_b)
        .await
        .expect(
            "B was never elected after A stepped down. A step-down that releases nothing leaves \
             B queued behind a lease that is still being renewed.",
        )
        .expect("B's campaign task should not panic")
        .expect("B's campaign should succeed once the leadership is released");
    let elapsed = handed_over_at.elapsed();
    let fence_b = elected_b.fence.clone();
    // Before the assertions below, which are not instant and would otherwise race B's own 2s lease.
    let _keepalive_b = tokio::spawn(elected_b.keepalive.run());

    // Anything at or past this bound is indistinguishable from A's lease simply lapsing, so a
    // `step_down` that released nothing would still look like a pass.
    let renewal_interval = LeaseKeepAlive::renewal_interval(granted_ttl);
    let earliest_expiry_failover = granted_ttl - renewal_interval;
    assert!(
        elapsed < earliest_expiry_failover,
        "B was elected {elapsed:?} after A stepped down, no sooner than the \
         {earliest_expiry_failover:?} floor for A's lease expiring on its own ({granted_ttl:?} TTL \
         less one {renewal_interval:?} renewal interval). The step-down released nothing, so a \
         rolling restart still costs a full lease TTL of routing-table unavailability."
    );

    assert!(
        fence_b.create_revision() > fence_a.create_revision(),
        "B's leader key was created at revision {} but A's was {}. A later election must produce a \
         strictly later creation revision, which is what makes the fence a fence.",
        fence_b.create_revision(),
        fence_a.create_revision()
    );
    assert_fence_is_live(etcd, &fence_b, "Standby B").await;

    // A `step_down` that resigned the election but kept the lease would pass everything above,
    // leaving A's lease parked in etcd for the rest of its TTL.
    let lost = timeout(granted_ttl * 2, keepalive_a)
        .await
        .expect(
            "A's keepalive was still renewing after A stepped down, so the lease behind its \
             leadership was never revoked.",
        )
        .expect("A's keepalive task should not panic");
    assert_eq!(
        lost.lease_id, a_lease_id,
        "A's keepalive reported a loss for lease {:#x}, but it was renewing {a_lease_id:#x}.",
        lost.lease_id
    );

    info!(handover = ?elapsed, lease_ttl = ?granted_ttl, "Leadership handed over on step-down");
}

#[test]
#[tracing::instrument(skip_all)]
// `run` returns `LeaseLost` with no success case on purpose: `serve_until_stopped` keeps serving a
// task that returns `Ok(())`, so a keepalive that could stop cleanly leaves a zombie leader. An
// explicit revoke is fine here - this is about detection, not about expiry fidelity.
async fn the_keepalive_task_reports_lease_loss(etcd: &Arc<DockerEtcd>) {
    let config = election_config(etcd);
    let election = LeaderElection::connect(&config, unique_election_name())
        .await
        .expect("Connecting the election client should succeed");
    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");

    let lease_id = elected.lease_id;
    let granted_ttl = elected.granted_ttl;
    let keepalive = tokio::spawn(elected.keepalive.run());

    // Longer than one full TTL, the watchdog's own budget: a keepalive that never renewed, or that
    // never reset its deadline on a renewal, resolves here before anything revokes the lease.
    let healthy_for = granted_ttl + Duration::from_millis(500);
    tokio::time::sleep(healthy_for).await;
    assert!(
        !keepalive.is_finished(),
        "The keepalive reported the lease lost after {healthy_for:?} while nothing had touched it. \
         It is longer than the {granted_ttl:?} watchdog budget, so either renewals are not being \
         sent or a successful renewal is not resetting the watchdog deadline."
    );

    inspection_client(etcd)
        .await
        .lease_client()
        .revoke(lease_id)
        .await
        .expect("Revoking the lease out of band should succeed");

    let lost = timeout(granted_ttl * 2, keepalive)
        .await
        .expect("The keepalive should report the loss promptly after an external revoke")
        .expect("The keepalive task should not panic");

    assert_eq!(
        lost.lease_id, lease_id,
        "The keepalive reported a loss for lease {:#x}, but it was renewing {lease_id:#x}.",
        lost.lease_id
    );

    // etcd may answer the in-flight keepalive with ttl 0 or just close the stream, so the reason
    // is not deterministic - record it rather than assert one.
    info!(reason = %lost.reason, lease_id = format!("{lease_id:#x}"), "Lease loss detected");
}

#[test]
#[tracing::instrument(skip_all)]
// A broken keepalive stream is not a lost lease: an etcd member restart ends every stream on that
// connection while the server goes on holding the leases they renewed, so reporting the loss would
// cost one failover per member restart, each for nothing.
async fn the_keepalive_survives_a_dropped_connection(etcd: &Arc<DockerEtcd>) {
    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let config = EtcdConfig {
        endpoints: vec![proxy.url()],
        ..election_config(etcd)
    };

    let election = LeaderElection::connect(&config, unique_election_name())
        .await
        .expect("Connecting the election client through the proxy should succeed");
    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");

    let lease_id = elected.lease_id;
    let granted_ttl = elected.granted_ttl;
    let renewal_interval = LeaseKeepAlive::renewal_interval(granted_ttl);
    let keepalive = tokio::spawn(elected.keepalive.run());

    // Past a renewal first, so what breaks is a connection that was working.
    tokio::time::sleep(renewal_interval + Duration::from_millis(200)).await;
    proxy.drop_all_connections().await;

    // A full watchdog budget past the break: long enough that a keepalive which gave up has
    // resolved, and that a lease nobody renewed any more has expired.
    let recovery_window = granted_ttl + Duration::from_millis(500);
    tokio::time::sleep(recovery_window).await;

    assert!(
        !keepalive.is_finished(),
        "The keepalive reported the lease lost {recovery_window:?} after its connection was \
         dropped. etcd was never touched and still holds the lease - a broken stream has to be \
         reconnected, not reported, or every etcd member restart forces a failover."
    );

    let ttl = inspection_client(etcd)
        .await
        .lease_client()
        .time_to_live(lease_id, None)
        .await
        .expect("Reading the lease TTL back should succeed")
        .ttl();
    assert!(
        ttl > 0,
        "etcd reports {ttl}s left on lease {lease_id:#x} - it expired. The keepalive is still \
         running, so it is holding a stream that no longer reaches etcd instead of re-issuing one."
    );

    // Nothing else ends this task, and the proxy it renews through is about to be dropped.
    keepalive.abort();
    info!(
        lease_id = format!("{lease_id:#x}"),
        ttl, "Lease survived the dropped connection"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// The watchdog's budget belongs to the lease, not to the task watching it: etcd counts down from
// the renewal it last acknowledged, so re-arming the deadline any later - on a second entry to
// `drive`, say - claims a belief window longer than the lease. Only the anchoring rule is checked
// deterministically, by the `RenewalWatchdog` unit tests; this covers it end to end over a lease.
async fn the_watchdog_measures_from_the_last_acknowledged_renewal(etcd: &Arc<DockerEtcd>) {
    // Twice the floor: both halves below measure a fraction of the TTL, which at 2s would be
    // inside the scheduling noise of a loaded machine.
    let lease_ttl = TEST_LEASE_TTL * 2;
    let tolerance = Duration::from_millis(500);

    // The renewing half: the deadline is measured from the last renewal etcd acknowledged, so a
    // black hole is detected within one TTL of it - not of when the traffic stopped.
    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let config = EtcdConfig {
        endpoints: vec![proxy.url()],
        leader_lease_ttl: lease_ttl,
        ..election_config(etcd)
    };
    let election = LeaderElection::connect(&config, unique_election_name())
        .await
        .expect("Connecting the election client through the proxy should succeed");
    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");

    let granted_ttl = elected.granted_ttl;
    let renewal_interval = LeaseKeepAlive::renewal_interval(granted_ttl);
    let keepalive = tokio::spawn(elected.keepalive.run());

    // One whole renewal interval, so at least one renewal has been acknowledged since the
    // handshake and the last acknowledgement is at most that far in the past.
    tokio::time::sleep(renewal_interval).await;
    let silent_since = Instant::now();
    proxy.black_hole();

    let lost = timeout(granted_ttl * 3, keepalive)
        .await
        .expect("The watchdog should report the loss within three TTLs of the black hole")
        .expect("The keepalive task should not panic");
    let elapsed = silent_since.elapsed();

    assert_eq!(
        lost.reason,
        LeaseLossReason::RenewalDeadlineExceeded,
        "A black-holed connection produces neither an error nor a close, so the watchdog is the \
         only thing that can end this lease - anything else reporting it means something read a \
         fault that was not there."
    );
    assert!(
        elapsed >= granted_ttl.saturating_sub(renewal_interval + tolerance),
        "The lease was declared lost {elapsed:?} after the traffic stopped, with a {granted_ttl:?} \
         TTL whose last renewal was at most {renewal_interval:?} earlier. That is too early: the \
         deadline is being armed from something other than an acknowledged renewal."
    );
    assert!(
        elapsed <= granted_ttl + tolerance,
        "The lease was declared lost {elapsed:?} after the traffic stopped, which is more than the \
         {granted_ttl:?} etcd gives it. etcd expired this lease before the watchdog did, so the \
         replica went on believing it was the leader after a new one could already be elected."
    );

    // The anchored half: nothing renews between handshake and spawn, so a deadline armed on entry
    // to `drive` would hand this keepalive a fresh TTL the lease does not have.
    let idle_proxy = BreakableProxy::start(&etcd.client_url()).await;
    let idle_config = EtcdConfig {
        endpoints: vec![idle_proxy.url()],
        leader_lease_ttl: lease_ttl,
        ..election_config(etcd)
    };
    let idle_election = LeaderElection::connect(&idle_config, unique_election_name())
        .await
        .expect("Connecting the second election client through its proxy should succeed");
    let idle_elected = timeout(CAMPAIGN_TIMEOUT, idle_election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");

    let idle_granted_ttl = idle_elected.granted_ttl;
    let unrenewed_for = idle_granted_ttl / 2;
    tokio::time::sleep(unrenewed_for).await;

    // Set before the spawn, so not a single renewal can be acknowledged after it: the only anchor
    // left is the handshake the campaign made.
    idle_proxy.black_hole();
    let spawned_at = Instant::now();
    let idle_keepalive = tokio::spawn(idle_elected.keepalive.run());

    let idle_lost = timeout(idle_granted_ttl * 3, idle_keepalive)
        .await
        .expect("The watchdog should report the loss without waiting out a second full TTL")
        .expect("The keepalive task should not panic");
    let idle_elapsed = spawned_at.elapsed();

    assert_eq!(
        idle_lost.reason,
        LeaseLossReason::RenewalDeadlineExceeded,
        "Nothing was closed and nothing errored, so the watchdog is the only thing that can end \
         this lease."
    );
    assert!(
        idle_elapsed <= idle_granted_ttl - unrenewed_for + tolerance,
        "The lease was declared lost {idle_elapsed:?} after the keepalive was spawned, though \
         {unrenewed_for:?} of its {idle_granted_ttl:?} had already been spent unrenewed. The \
         deadline is being armed when the keepalive starts running rather than at the last \
         acknowledged renewal, so the replica believes it holds a lease etcd has already expired."
    );
    assert!(
        idle_elapsed + tolerance >= idle_granted_ttl - unrenewed_for,
        "The lease was declared lost {idle_elapsed:?} after the keepalive was spawned, earlier \
         than the {:?} it still had. The deadline is being armed from before the handshake.",
        idle_granted_ttl - unrenewed_for
    );
}

#[test]
#[tracing::instrument(skip_all)]
// The watchdog deadline is the reconnect loop's only bound. A fault that breaks every connection
// and answers none of them must still end the lease inside the TTL etcd is counting down, or the
// replica believes it leads something a successor can already hold.
async fn the_watchdog_still_bounds_a_reconnect_that_cannot_succeed(etcd: &Arc<DockerEtcd>) {
    // Twice the floor, so the window measured here sits outside scheduling noise.
    let lease_ttl = TEST_LEASE_TTL * 2;
    let tolerance = Duration::from_millis(500);

    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let config = EtcdConfig {
        endpoints: vec![proxy.url()],
        leader_lease_ttl: lease_ttl,
        ..election_config(etcd)
    };
    let election = LeaderElection::connect(&config, unique_election_name())
        .await
        .expect("Connecting the election client through the proxy should succeed");
    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");

    let granted_ttl = elected.granted_ttl;
    let renewal_interval = LeaseKeepAlive::renewal_interval(granted_ttl);
    let keepalive = tokio::spawn(elected.keepalive.run());

    // A whole renewal interval first, so what breaks is a keepalive that was working and the last
    // acknowledged renewal is at most that far in the past.
    tokio::time::sleep(renewal_interval).await;

    // This order, not the reverse: the flag is set before any socket is torn down, so every
    // reconnect the drop provokes lands on a pump that is already parked. Together they are the
    // fault a reconnect loop cannot escape - connections open, nothing ever answered.
    proxy.black_hole();
    proxy.drop_all_connections().await;
    let broken_at = Instant::now();

    let lost = timeout(granted_ttl + tolerance, keepalive)
        .await
        .expect(
            "The keepalive never reported the loss. Its reconnect loop is not bounded by the \
             watchdog deadline, so the replica believes it holds a lease etcd has already expired.",
        )
        .expect("The keepalive task should not panic");
    let elapsed = broken_at.elapsed();

    assert_eq!(
        lost.reason,
        LeaseLossReason::RenewalDeadlineExceeded,
        "Every reconnect attempt was met with silence rather than with an error or a close, so the \
         deadline is the only thing that can have ended this lease."
    );
    assert!(
        elapsed + tolerance >= granted_ttl.saturating_sub(renewal_interval),
        "The lease was declared lost {elapsed:?} after the last connection that worked, with a \
         {granted_ttl:?} TTL whose last renewal was at most {renewal_interval:?} earlier. That is \
         too early: the reconnect loop is giving up on its own instead of running out the budget \
         the lease still had."
    );
}

#[test]
#[tracing::instrument(skip_all)]
// A standby holds nothing, so a lease that lapses while it waits is recoverable - grant a new one
// and campaign again. Treated as fatal, one etcd blip takes down every replica that was merely
// waiting its turn. Only an elected leader's loss has to end the process.
async fn a_standby_whose_lease_lapses_mid_campaign_re_campaigns_instead_of_exiting(
    etcd: &Arc<DockerEtcd>,
) {
    let election_name = unique_election_name();
    let config = election_config(etcd);

    let leader_a = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica A should succeed");
    let elected_a = timeout(CAMPAIGN_TIMEOUT, leader_a.campaign_until_elected())
        .await
        .expect("A's campaign should not hang")
        .expect("A should win the uncontested campaign");
    let fence_a = elected_a.fence.clone();
    let keepalive_a = tokio::spawn(elected_a.keepalive.run());

    let replica_b = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica B should succeed");
    let mut campaign_b = tokio::spawn(async move { replica_b.campaign_until_elected().await });

    let b_first_lease_id = await_standby_lease(etcd, &election_name, fence_a.key()).await;

    // The blip. B is still queued behind A and holds nothing of its own.
    inspection_client(etcd)
        .await
        .lease_client()
        .revoke(b_first_lease_id)
        .await
        .expect("Revoking B's campaign lease out of band should succeed");

    // Longer than one TTL: a standby that reported the loss as fatal has finished by now.
    match timeout(TEST_LEASE_TTL * 2, &mut campaign_b).await {
        Err(_) => {} // still campaigning, on a lease it granted itself - correct
        Ok(Ok(Ok(_))) => panic!(
            "Standby B was elected while A still held the leadership and was renewing its lease."
        ),
        Ok(Ok(Err(err))) => panic!(
            "Standby B's campaign FAILED after its own lease was revoked: {err}. A campaigning \
             replica holds nothing, so a lapsed lease is recovered by granting a new one and \
             campaigning again. Reported as non-retriable, this ends `run()` and the process - so \
             one etcd blip would exit every standby in the deployment."
        ),
        Ok(Err(join_err)) => panic!("Standby B's campaign task panicked: {join_err}"),
    }

    // Release A the way a crash would: stop renewing and let the lease expire.
    keepalive_a.abort();

    let elected_b = timeout(CAMPAIGN_TIMEOUT, campaign_b)
        .await
        .expect("B should be elected once A's lease expires")
        .expect("B's campaign task should not panic")
        .expect("B's campaign should succeed on the lease it granted itself after the revoke");

    assert_ne!(
        elected_b.lease_id, b_first_lease_id,
        "B was elected on lease {b_first_lease_id:#x}, the one that was revoked mid-campaign. It \
         must campaign on a fresh grant, or its fence key is backed by a lease etcd has already \
         forgotten."
    );
    assert!(
        elected_b.fence.create_revision() > fence_a.create_revision(),
        "B's leader key was created at revision {} but A's was {}. A later election must produce a \
         strictly later creation revision.",
        elected_b.fence.create_revision(),
        fence_a.create_revision()
    );
    assert_fence_is_live(etcd, &elected_b.fence, "Standby B").await;
}

#[test]
#[tracing::instrument(skip_all)]
// etcd's campaign resolves when the key ahead of it is deleted and never re-checks the
// campaigner's own key: revoking B's lease and then A's - a revoke deletes the attached keys
// synchronously - has B told it won on a lease that is already gone. Only a read-back can tell.
async fn a_campaign_won_on_an_already_revoked_lease_is_not_accepted(etcd: &Arc<DockerEtcd>) {
    let election_name = unique_election_name();
    let config = election_config(etcd);

    let leader_a = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica A should succeed");
    let elected_a = timeout(CAMPAIGN_TIMEOUT, leader_a.campaign_until_elected())
        .await
        .expect("A's campaign should not hang")
        .expect("A should win the uncontested campaign");
    let a_lease_id = elected_a.lease_id;
    let fence_a = elected_a.fence.clone();
    let _keepalive_a = tokio::spawn(elected_a.keepalive.run());

    let replica_b = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica B should succeed");
    let campaign_b = tokio::spawn(async move { replica_b.campaign_until_elected().await });

    let b_first_lease_id = await_standby_lease(etcd, &election_name, fence_a.key()).await;

    let mut lease_client = inspection_client(etcd).await.lease_client();
    lease_client
        .revoke(b_first_lease_id)
        .await
        .expect("Revoking B's campaign lease out of band should succeed");
    // No wait in between: A's key goes away while B's keepalive has not yet noticed that B's own
    // key went with it. That window is the whole point.
    lease_client
        .revoke(a_lease_id)
        .await
        .expect("Revoking A's leadership lease out of band should succeed");

    let elected_b = timeout(CAMPAIGN_TIMEOUT, campaign_b)
        .await
        .expect("B should be elected once A's lease is gone")
        .expect("B's campaign task should not panic")
        .expect("B's campaign should succeed on a lease that is still alive");

    assert_ne!(
        elected_b.lease_id, b_first_lease_id,
        "B accepted the campaign it won on lease {b_first_lease_id:#x}, which had already been \
         revoked. etcd answers the campaign with the leader key it recorded when the campaign \
         started, so accepting it hands the persistence layer a fence over a key that no longer \
         exists - and every state write fails for the life of the process, in distributed mode \
         only."
    );
    assert_fence_is_live(etcd, &elected_b.fence, "Standby B").await;
}

/// etcd's current revision, taken from the header of an ordinary read.
async fn current_revision(etcd: &DockerEtcd) -> i64 {
    inspection_client(etcd)
        .await
        .kv_client()
        .get(STATE_KEY, None)
        .await
        .expect("Reading any key should succeed")
        .header()
        .expect("Every etcd response carries a header")
        .revision()
}

/// How many campaigner keys were created under `election_name` after `start_revision`, counting up
/// to and including `final_key`.
///
/// From the event history rather than by polling: a discarded campaign deletes its first key within
/// milliseconds, leaving a prefix indistinguishable from a campaign that was won once.
async fn campaigner_keys_created(
    etcd: &DockerEtcd,
    election_name: &str,
    start_revision: i64,
    final_key: &[u8],
) -> usize {
    // Unbounded on purpose, unlike `inspection_client`: `request_timeout` becomes a channel-wide
    // tonic timeout, and a watch is a long-lived stream rather than a request.
    let mut client = Client::connect(
        [etcd.client_url()],
        Some(ConnectOptions::new().with_connect_timeout(TEST_CONNECT_TIMEOUT)),
    )
    .await
    .expect("Cannot connect a watch client to etcd");

    let mut stream = client
        .watch(
            format!("{election_name}/"),
            Some(
                WatchOptions::new()
                    .with_prefix()
                    .with_start_revision(start_revision + 1),
            ),
        )
        .await
        .expect("Watching the election prefix should succeed");

    timeout(CAMPAIGN_TIMEOUT, async {
        let mut created = 0;
        loop {
            let response = stream
                .message()
                .await
                .expect("The election prefix watch should not fail")
                .expect("The election prefix watch should stay open");

            for event in response.events() {
                if event.event_type() != EventType::Put {
                    continue;
                }
                created += 1;
                if event.kv().is_some_and(|kv| kv.key() == final_key) {
                    return created;
                }
            }
        }
    })
    .await
    .expect(
        "etcd replays a watch from a past revision immediately, so the key the campaign was \
         won on must arrive",
    )
}

#[test]
#[tracing::instrument(skip_all)]
// A confirming read that fails in transit says nothing about whether the key is there. Discarding
// the win over it revokes a lease this replica holds and queues it up again - and if the revoke
// fails too, the key left behind blocks every other campaigner for a full TTL.
async fn a_transient_confirm_failure_does_not_discard_the_win(etcd: &Arc<DockerEtcd>) {
    let election_name = unique_election_name();
    let start_revision = current_revision(etcd).await;

    let election = LeaderElection::connect(&election_config(etcd), election_name.clone())
        .await
        .expect("Connecting the election client to etcd should succeed")
        // The first read only: the second is a real one, and must find the key exactly as the
        // campaign left it.
        .with_confirm_hook(|attempt| (attempt == 1).then_some(ShardManagerError::Timeout));

    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("A campaign whose confirming read fails once should still finish promptly")
        .expect("A transient confirming read failure is not a lost campaign");

    assert_fence_is_live(etcd, &elected.fence, "The leader").await;

    let response = inspection_client(etcd)
        .await
        .kv_client()
        .get(elected.fence.key(), None)
        .await
        .expect("Reading the leader key back should succeed");
    let leader_key = response
        .kvs()
        .first()
        .expect("The leader key must exist; `assert_fence_is_live` has just read it");
    assert_eq!(
        leader_key.lease(),
        elected.lease_id,
        "The leadership was reported on lease {:#x}, but the key holding it is attached to \
         lease {:#x}. The win was thrown away over a transient read and re-taken on a fresh \
         lease.",
        elected.lease_id,
        leader_key.lease()
    );

    let created =
        campaigner_keys_created(etcd, &election_name, start_revision, elected.fence.key()).await;
    assert_eq!(
        created, 1,
        "{created} campaigner keys were created under `{election_name}/` for one campaign. A \
         transient confirming read discarded the first win, so the replica revoked a lease \
         it still held and queued again behind everyone else."
    );
}

/// How often the standby under test announces itself. Short enough that a 1.5s wait produces
/// several announcements, and far longer than the round trip each one makes.
const STANDBY_ANNOUNCE_INTERVAL: Duration = Duration::from_millis(300);

/// Collects formatted log output, because the standby announcement has no effect other than the
/// line it writes.
///
/// Shared behind an `Arc`: the `MakeWriter` below is a closure handing out clones of this one.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .expect("The captured log buffer should not be poisoned"),
        )
        .into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("The captured log buffer should not be poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
#[tracing::instrument(skip_all)]
// A standby that waits correctly never returns from its campaign, so an announcement on the retry
// path only ever fires for one that is failing. This is what says the healthy case speaks too.
async fn a_standby_logs_who_holds_the_leadership_while_it_waits(etcd: &Arc<DockerEtcd>) {
    let election_name = unique_election_name();
    let config = election_config(etcd);

    let leader_a = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica A should succeed");
    let elected_a = timeout(CAMPAIGN_TIMEOUT, leader_a.campaign_until_elected())
        .await
        .expect("A's campaign should not hang")
        .expect("A should win the uncontested campaign");
    let _keepalive_a = tokio::spawn(elected_a.keepalive.run());

    let recorded_leader = leader_a
        .current_leader()
        .await
        .expect("Reading the current leader should succeed")
        .expect("A holds the leadership, so etcd has a leader recorded");

    // A scoped subscriber only applies to the thread that sets it, and the test binary has already
    // installed a global one - so B campaigns on a thread of its own, under a single-threaded
    // runtime whose tasks therefore all run there and are all captured.
    let captured = CapturedLogs::default();
    let writer = captured.clone();
    let standby_config = config.clone();
    let standby_election = election_name.clone();
    let (finished_tx, finished_rx) = oneshot::channel();
    let standby = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Building the standby's runtime should succeed");

        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();

        let still_waiting = tracing::subscriber::with_default(subscriber, || {
            runtime.block_on(async {
                let replica_b = LeaderElection::connect(&standby_config, standby_election)
                    .await
                    .expect("Connecting replica B should succeed")
                    .with_standby_log_interval(STANDBY_ANNOUNCE_INTERVAL);

                timeout(
                    STANDBY_ANNOUNCE_INTERVAL * 5,
                    replica_b.campaign_until_elected(),
                )
                .await
                .is_err()
            })
        });

        let _ = finished_tx.send(());
        still_waiting
    });

    timeout(CAMPAIGN_TIMEOUT, finished_rx)
        .await
        .expect("The standby thread should finish well within the campaign timeout")
        .expect("The standby thread should report that it finished");
    let still_waiting = standby.join().expect("The standby thread should not panic");

    assert!(
        still_waiting,
        "B stopped campaigning while A was still renewing its lease, so whatever it logged was not \
         logged as a standby."
    );

    let logged = captured.text();
    assert!(
        logged.contains("Still standing by"),
        "B waited behind A for several {STANDBY_ANNOUNCE_INTERVAL:?} announcement intervals and \
         never said so. A replica blocked in its campaign is indistinguishable from a hung one in \
         the log, which is the only thing an operator has to go on. Captured instead:\n{logged}"
    );
    assert!(
        logged.contains(&recorded_leader),
        "B announced that it was standing by, but not who holds the leadership: etcd records \
         `{recorded_leader}` and the announcement must carry it, or the log says nothing about \
         which replica to go and look at. Captured:\n{logged}"
    );
}

#[test]
#[tracing::instrument(skip_all)]
// etcd answers a leaderless election with an error, not an empty response. Left unmapped, the one
// moment worth looking at - a handover in progress, nobody holding it yet - reads as a failure.
async fn a_leaderless_election_reads_back_as_no_leader(etcd: &Arc<DockerEtcd>) {
    let election_name = unique_election_name();
    let config = election_config(etcd);

    let leader_a = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica A should succeed");
    let elected_a = timeout(CAMPAIGN_TIMEOUT, leader_a.campaign_until_elected())
        .await
        .expect("A's campaign should not hang")
        .expect("A should win the uncontested campaign");
    let fence_a = elected_a.fence.clone();
    let leadership_a = elected_a.leadership.clone();
    let keepalive_a = tokio::spawn(elected_a.keepalive.run());

    // Establishes that this election name reads back at all, so the `None` below is the absence of
    // a leader rather than the absence of anything ever having happened here.
    leader_a
        .current_leader()
        .await
        .expect("Reading the leader of a held election should succeed")
        .expect("A holds the leadership, so etcd has a leader recorded");

    leadership_a
        .step_down()
        .await
        .expect("Stepping down from a held leadership should succeed");
    keepalive_a.abort();
    await_leader_key_deleted(etcd, fence_a.key()).await;

    let leader = timeout(CAMPAIGN_TIMEOUT, leader_a.current_leader())
        .await
        .expect("Reading the leader of a leaderless election should not hang")
        .expect("An election nobody holds is not a failed read");
    assert_eq!(
        leader, None,
        "`{election_name}` has no campaigner left, so the leader lookup must report that rather \
         than an error a caller would log as a fault."
    );
}

const NUMBER_OF_SHARDS: usize = 8;

/// A state that differs from a freshly created one, so that "the rejected write stored nothing" is
/// a comparison of two different states rather than of a state with itself.
fn state_with_one_executor(number_of_shards: usize) -> ShardLeaseState {
    let mut shard_state = ShardLeaseState::new(number_of_shards);
    shard_state.add_executor(
        ExecutorId(Uuid::from_u128(1)),
        ExecutorAddr {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            port: 9010,
        },
        None,
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
        Duration::from_secs(60),
    );
    shard_state
}

/// Waits until the leader key itself is gone from etcd.
///
/// Not `time_to_live`: etcd reports a lease expired before deleting its keys, and the fence
/// compares the key - so waiting on the lease would leave a window the tests below would race.
async fn await_leader_key_deleted(etcd: &DockerEtcd, key: &[u8]) {
    let deadline = Instant::now() + CAMPAIGN_TIMEOUT;
    let mut kv = inspection_client(etcd).await.kv_client();

    while Instant::now() < deadline {
        let response = kv
            .get(key.to_vec(), None)
            .await
            .expect("Reading the leader key should succeed");
        if response.kvs().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "The leader key `{}` was still present after {CAMPAIGN_TIMEOUT:?}",
        String::from_utf8_lossy(key)
    );
}

#[test]
#[tracing::instrument(skip_all)]
// A revision compare-and-swap alone is not enough: two replicas that both read revision R both
// pass it. A demoted leader is put in exactly that position - its cached revision still current -
// and the write must be refused anyway, by the second compare on the election key.
async fn a_stale_leaders_write_is_rejected_even_at_the_current_revision(etcd: &Arc<DockerEtcd>) {
    let config = election_config(etcd);
    let election_name = unique_election_name();

    // Start from an empty state key so the first write is a create.
    inspection_client(etcd)
        .await
        .kv_client()
        .delete(STATE_KEY, None)
        .await
        .expect("Wiping the state key should succeed");

    let leader_a = LeaderElection::connect(&config, election_name.clone())
        .await
        .expect("Connecting replica A should succeed");
    let elected_a = timeout(CAMPAIGN_TIMEOUT, leader_a.campaign_until_elected())
        .await
        .expect("A's campaign should not hang")
        .expect("A should win the uncontested campaign");
    let fence_a_key = elected_a.fence.key().to_vec();
    let keepalive_a = tokio::spawn(elected_a.keepalive.run());

    let persistence_a =
        EtcdRoutingTablePersistence::new(&config, NUMBER_OF_SHARDS, elected_a.fence)
            .await
            .expect("Building A's persistence should succeed");

    let state = ShardLeaseState::new(NUMBER_OF_SHARDS);
    let revision = persistence_a
        .write(&state, NO_REVISION)
        .await
        .expect("A holds the leadership, so its first write should succeed");

    // Differs from what A stored, so a write that was refused but still landed shows up in the
    // read-back rather than hiding behind identical bytes.
    let later_state = state_with_one_executor(NUMBER_OF_SHARDS);

    // Demote A without touching the state key: stop renewing and let the lease expire.
    keepalive_a.abort();
    await_leader_key_deleted(etcd, &fence_a_key).await;

    // Nothing has written since A did, so A's cached revision is still current and the revision
    // compare-and-swap alone would accept the write below. Only the leadership compare rejects it.
    let stored = inspection_client(etcd)
        .await
        .kv_client()
        .get(STATE_KEY, None)
        .await
        .expect("Reading the state key should succeed");
    let stored_revision = stored
        .kvs()
        .first()
        .expect("The state written by A should still be there")
        .mod_revision();
    assert_eq!(
        stored_revision, revision,
        "The state key moved to revision {stored_revision} while the test was waiting for A's \
         lease to expire. A's cached revision {revision} is then stale, so the rejection below \
         would prove nothing about the leadership fence."
    );

    let rejected = persistence_a.write(&later_state, revision).await;
    assert!(
        matches!(rejected, Err(ShardManagerError::LeadershipLost { .. })),
        "A wrote with a revision that is still current, but its leadership had expired. Expected \
         LeadershipLost; got {rejected:?}. Without the election-key compare in the write \
         transaction, a demoted leader silently overwrites the routing table."
    );

    let (read_back, read_revision) = persistence_a
        .read()
        .await
        .expect("Reading the state back should succeed");
    assert_eq!(
        read_revision, revision,
        "The rejected write must store nothing"
    );
    assert_eq!(read_back, state, "The rejected write must store nothing");

    // ... and the fence is specific to A, not a blanket refusal: B can write at the same revision.
    let replica_b = LeaderElection::connect(&config, election_name)
        .await
        .expect("Connecting replica B should succeed");
    let elected_b = timeout(CAMPAIGN_TIMEOUT, replica_b.campaign_until_elected())
        .await
        .expect("B's campaign should not hang")
        .expect("B should win once A's lease has expired");
    let _keepalive_b = tokio::spawn(elected_b.keepalive.run());

    let persistence_b =
        EtcdRoutingTablePersistence::new(&config, NUMBER_OF_SHARDS, elected_b.fence)
            .await
            .expect("Building B's persistence should succeed");
    persistence_b
        .write(&later_state, revision)
        .await
        .expect("B holds the leadership, so the same write it refused from A must succeed");

    info!("Stale leader refused at a current revision; the new leader accepted at the same one");
}

#[test]
#[tracing::instrument(skip_all)]
// The lease starts counting at the handshake, not when the keepalive is finally spawned - and
// `start_distributed_mode` does real work in between. A watchdog armed at the spawn grants itself
// that gap twice. The gap is made large and the connection then silenced, so the deadline is the
// only thing that can end the lease and the only question is what it was armed from.
async fn the_watchdog_is_armed_from_the_handshake_not_from_the_spawn(etcd: &Arc<DockerEtcd>) {
    let lease_ttl = TEST_LEASE_TTL * 2;
    let tolerance = Duration::from_millis(500);

    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let config = EtcdConfig {
        endpoints: vec![proxy.url()],
        leader_lease_ttl: lease_ttl,
        ..election_config(etcd)
    };
    let election = LeaderElection::connect(&config, unique_election_name())
        .await
        .expect("Connecting the election client through the proxy should succeed");
    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");
    let handshake = Instant::now();
    let granted_ttl = elected.granted_ttl;

    // Half the lease spent between winning it and starting to renew it.
    let gap = granted_ttl / 2;
    tokio::time::sleep(gap).await;

    // Silenced before the spawn, so not one renewal is acknowledged and what the deadline reports
    // is purely where it was armed. No connection is dropped: a reconnect would be a second thing
    // under test.
    proxy.black_hole();

    let spawned = Instant::now();
    let budget = granted_ttl - gap + tolerance;
    let lost = timeout(budget, elected.keepalive.run())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "The keepalive still held the lease {budget:?} after it started renewing, having \
                 been handed a {granted_ttl:?} lease {gap:?} earlier. Its deadline was armed from \
                 the spawn rather than from the handshake, so this replica goes on leading for \
                 {gap:?} past the point where etcd has expired the lease and elected a successor."
            )
        });
    let since_handshake = handshake.elapsed();
    let since_spawn = spawned.elapsed();

    assert_eq!(
        lost.reason,
        LeaseLossReason::RenewalDeadlineExceeded,
        "Nothing was closed and nothing errored, so the deadline is the only thing that can have \
         ended this lease; `{}` means something else did.",
        lost.reason
    );
    assert!(
        since_handshake >= granted_ttl.saturating_sub(tolerance),
        "The lease was given up {since_handshake:?} after it was granted, with a {granted_ttl:?} \
         TTL and no renewal ever acknowledged. Giving it up early costs a failover that the lease \
         did not require."
    );
    info!(?since_handshake, ?since_spawn, "Watchdog fired");
}

#[test]
#[tracing::instrument(skip_all)]
async fn a_reconnect_that_succeeds_re_arms_the_watchdog(etcd: &Arc<DockerEtcd>) {
    let lease_ttl = TEST_LEASE_TTL * 2;
    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let config = EtcdConfig {
        endpoints: vec![proxy.url()],
        leader_lease_ttl: lease_ttl,
        ..election_config(etcd)
    };
    let election = LeaderElection::connect(&config, unique_election_name())
        .await
        .expect("Connecting the election client through the proxy should succeed");
    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");

    let granted_ttl = elected.granted_ttl;
    let renewal_interval = LeaseKeepAlive::renewal_interval(granted_ttl);
    let keepalive = tokio::spawn(elected.keepalive.run());

    // Let one renewal be acknowledged, then silence the connection and let the deadline run most
    // of the way down, so an un-re-armed watchdog has only a sliver of it left.
    tokio::time::sleep(renewal_interval).await;
    let silent_since = Instant::now();
    proxy.black_hole();
    tokio::time::sleep(granted_ttl / 2).await;

    // Restored first, so the reconnect that the closed socket triggers can actually succeed.
    proxy.restore();
    proxy.drop_all_connections().await;

    let gave_up = timeout(granted_ttl / 2, keepalive).await;
    assert!(
        gave_up.is_err(),
        "The keepalive reported the lease lost {:?} after the connection went silent, having \
         reconnected successfully in between. etcd renewed the lease for a full {granted_ttl:?} \
         when it answered that reconnect, so the watchdog is still counting from a renewal that \
         predates the break and a working connection is being read as a lost lease.",
        silent_since.elapsed()
    );
}

/// Every key currently queued under one election name.
async fn keys_under(etcd: &DockerEtcd, election_name: &str) -> Vec<Vec<u8>> {
    inspection_client(etcd)
        .await
        .kv_client()
        .get(
            format!("{election_name}/"),
            Some(GetOptions::new().with_prefix()),
        )
        .await
        .expect("Reading the election prefix should succeed")
        .kvs()
        .iter()
        .map(|kv| kv.key().to_vec())
        .collect()
}

#[test]
#[tracing::instrument(skip_all)]

async fn a_dropped_campaign_that_had_already_won_leaves_no_key_behind(etcd: &Arc<DockerEtcd>) {
    let election_name = unique_election_name();
    let election = LeaderElection::connect(&election_config(etcd), election_name.clone())
        .await
        .expect("Connecting the election client should succeed")
        // Holds the attempt inside its confirming read: the campaign is won, so the key exists on
        // the lease, and the attempt is still in flight and therefore droppable.
        .with_confirm_hook(|_| Some(ShardManagerError::Timeout));

    {
        let mut campaign = std::pin::pin!(election.campaign_until_elected());
        let still_confirming = timeout(TEST_LEASE_TTL / 2, &mut campaign).await;
        assert!(
            still_confirming.is_err(),
            "The campaign finished instead of retrying its confirming read, so this test never \
             reaches the window it is about: a won campaign that is then abandoned."
        );
        assert!(
            !keys_under(etcd, &election_name).await.is_empty(),
            "The campaign has not created its election key yet, so there is nothing for the \
             cleanup under test to leave behind."
        );
    }

    election.revoke_pending_lease().await;

    assert!(
        keys_under(etcd, &election_name).await.is_empty(),
        "The abandoned campaign's key is still queued on a lease nothing renews. It had already \
         won, so etcd has no cancelled RPC to resign, and every other replica now waits out that \
         lease behind a campaign that no longer exists."
    );
}

#[test]
#[tracing::instrument(skip_all)]
// A shutdown that lands while the won campaign is still confirming its key has to be seen there:
// the alternative is to wait the confirm's whole budget out. Either way it leaves through the
// revoke, so the key the win is holding is released.
async fn a_shutdown_during_the_confirming_read_releases_the_lease_at_once(etcd: &Arc<DockerEtcd>) {
    let election_name = unique_election_name();
    let shutdown = CancellationToken::new();
    // The hook both holds the attempt inside its confirming read and reports when it got there,
    // so the shutdown is sent inside that window and nowhere else.
    let confirming = Arc::new(Notify::new());
    let election = LeaderElection::connect(&election_config(etcd), election_name.clone())
        .await
        .expect("Connecting the election client should succeed")
        .with_shutdown(shutdown.clone())
        .with_confirm_hook({
            let confirming = confirming.clone();
            move |_| {
                confirming.notify_one();
                Some(ShardManagerError::Timeout)
            }
        });
    let campaign = tokio::spawn(async move { election.campaign_until_elected().await });

    timeout(CAMPAIGN_TIMEOUT, confirming.notified())
        .await
        .expect("The campaign should be won and reach its confirming read");
    shutdown.cancel();

    let outcome = timeout(Duration::from_millis(500), campaign)
        .await
        .expect(
            "The campaign ignored the shutdown for the whole of its confirming budget. A pod \
             being rolled sits there for most of a lease TTL before it can begin to stop.",
        )
        .expect("The campaign task should not panic");
    let err = outcome
        .err()
        .expect("A campaign asked to stop must report that, not finish starting");
    assert!(
        matches!(err, ShardManagerError::ShutdownRequested),
        "A campaign asked to stop must report that, but ended with: {err}"
    );
    assert!(
        keys_under(etcd, &election_name).await.is_empty(),
        "The stopped campaign's key is still queued: the shutdown left without the revoke, so \
         every other replica waits out this lease behind a campaign that has already exited."
    );
}

#[test]
#[tracing::instrument(skip_all)]
// A reconnect whose own handshake is answered with a zero TTL has learned the lease is gone; that
// is a loss to report now, not a fault to retry until the watchdog gives up on its own.
async fn a_reconnect_that_finds_the_lease_gone_reports_it_as_expired(etcd: &Arc<DockerEtcd>) {
    let proxy = BreakableProxy::start(&etcd.client_url()).await;
    let config = EtcdConfig {
        endpoints: vec![proxy.url()],
        ..election_config(etcd)
    };
    let election = LeaderElection::connect(&config, unique_election_name())
        .await
        .expect("Connecting the election client through the proxy should succeed");
    let elected = timeout(CAMPAIGN_TIMEOUT, election.campaign_until_elected())
        .await
        .expect("The campaign should not hang")
        .expect("Winning an uncontested campaign should succeed");
    let lease_id = elected.lease_id;
    let granted_ttl = elected.granted_ttl;
    let keepalive = tokio::spawn(elected.keepalive.run());

    // Silence the connection, take the lease away behind its back, then break the connection so
    // the keepalive reconnects - and is told, on that handshake, that its lease no longer exists.
    tokio::time::sleep(LeaseKeepAlive::renewal_interval(granted_ttl)).await;
    proxy.black_hole();
    inspection_client(etcd)
        .await
        .lease_client()
        .revoke(lease_id)
        .await
        .expect("Revoking the lease out of band should succeed");
    proxy.restore();
    let broken_at = Instant::now();
    proxy.drop_all_connections().await;

    let lost = timeout(granted_ttl, keepalive)
        .await
        .expect("The keepalive should report the loss well inside one TTL of the reconnect")
        .expect("The keepalive task should not panic");
    assert_eq!(
        lost.reason,
        LeaseLossReason::LeaseExpired,
        "The reconnect was told the lease is gone and reported {} instead, {:?} after the break: \
         a lease etcd has already expired was kept alive in this replica's belief until the \
         watchdog ran out on its own.",
        lost.reason,
        broken_at.elapsed()
    );
}
