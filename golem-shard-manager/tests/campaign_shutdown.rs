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

//! Coverage for a replica that never reaches etcd at all.
//!
//! Outside `etcd_backed` because an unreachable endpoint is the case under test: no server to
//! share, no state key to serialise against. `Client::connect` is lazy, so connecting succeeds and
//! every failure lands on a later request instead.

use golem_shard_manager::config::{
    EtcdConfig, GrpcApiConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_shard_manager::{Deployment, LEADER_ELECTION_NAME, LeaderElection, ShardManagerError};
use std::time::Duration;
use test_r::test;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// Reserved by convention and never listened on, so requests fail immediately rather than after a
/// connect timeout.
const UNREACHABLE_ENDPOINT: &str = "http://127.0.0.1:1";

/// Short enough that several attempts fail before each test acts, so the campaign is reliably
/// deep in its retry loop.
const TEST_CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const TEST_REQUEST_TIMEOUT: Duration = Duration::from_millis(200);

const TEST_LEASE_TTL: Duration = Duration::from_secs(2);

/// Comfortably longer than one failed attempt plus its first backoff.
const RETRY_SETTLE: Duration = Duration::from_millis(300);

/// Longer than the retry loop's slowest iteration, so only a campaign that never observes
/// shutdown fails here.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// How long a replica is given to prove it is still starting: long enough that one giving up on
/// the first failure has already returned.
const STARTUP_SETTLE: Duration = Duration::from_secs(2);

const CAMPAIGN_FAILURES_METRIC: &str = "shard_manager_campaign_attempt_failures_total";

fn unreachable_config() -> EtcdConfig {
    EtcdConfig {
        endpoints: vec![UNREACHABLE_ENDPOINT.to_string()],
        connect_timeout: TEST_CONNECT_TIMEOUT,
        request_timeout: TEST_REQUEST_TIMEOUT,
        leader_lease_ttl: TEST_LEASE_TTL,
    }
}

async fn campaigner(shutdown: CancellationToken) -> LeaderElection {
    LeaderElection::connect(
        &unreachable_config(),
        format!("{LEADER_ELECTION_NAME}-test-unreachable"),
    )
    .await
    .expect("Connecting is lazy, so an unreachable endpoint should still connect")
    .with_shutdown(shutdown)
}

fn campaign_attempt_failures() -> Option<u64> {
    prometheus::default_registry()
        .gather()
        .into_iter()
        .find(|family| family.name() == CAMPAIGN_FAILURES_METRIC)
        .and_then(|family| {
            family
                .get_metric()
                .first()
                .and_then(|metric| metric.get_counter().as_ref())
                .map(|counter| counter.value() as u64)
        })
}

#[test]
#[tracing::instrument(skip_all)]
// A replica that cannot reach etcd spends its life between attempts: an election watching its
// token only from inside a campaign would ignore SIGTERM until the orchestrator killed it.
async fn a_campaign_that_cannot_reach_etcd_still_observes_shutdown() {
    let shutdown = CancellationToken::new();
    let election = campaigner(shutdown.clone()).await;
    let campaign = tokio::spawn(async move { election.campaign_until_elected().await });

    tokio::time::sleep(RETRY_SETTLE).await;
    shutdown.cancel();

    let result = timeout(SHUTDOWN_TIMEOUT, campaign)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "The campaign was still running {SHUTDOWN_TIMEOUT:?} after it was cancelled. A \
                 replica that checks the token only inside a campaign it can never start does not \
                 stop when it is asked to."
            )
        })
        .expect("The campaign task should not panic");

    match result {
        Err(ShardManagerError::ShutdownRequested) => {}
        // `run()` reports this variant as an orderly stop rather than a startup failure.
        Err(err) => panic!("A cancelled campaign must report the shutdown it was asked for: {err}"),
        Ok(elected) => panic!(
            "The campaign against `{UNREACHABLE_ENDPOINT}` reported winning on lease {:#x}.",
            elected.lease_id
        ),
    }
}

#[test]
#[tracing::instrument(skip_all)]
// The leadership gauges read 0 both for a standby queued behind a healthy leader and for one that
// cannot reach etcd; this counter is the only thing telling them apart, so it has to move while
// the campaign is still failing.
async fn a_campaign_that_keeps_failing_counts_its_attempts() {
    let before = campaign_attempt_failures().unwrap_or(0);

    let shutdown = CancellationToken::new();
    let election = campaigner(shutdown.clone()).await;
    let campaign = tokio::spawn(async move { election.campaign_until_elected().await });

    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown.cancel();
    let _ = timeout(SHUTDOWN_TIMEOUT, campaign).await;

    let after = campaign_attempt_failures().unwrap_or_else(|| {
        panic!(
            "`{CAMPAIGN_FAILURES_METRIC}` was never registered, so a replica wedged against an \
             unreachable etcd exports nothing to distinguish it from one that is patiently waiting."
        )
    });
    assert!(
        after > before,
        "The campaign failed repeatedly against `{UNREACHABLE_ENDPOINT}` but \
         `{CAMPAIGN_FAILURES_METRIC}` stayed at {before}."
    );
}

fn unreachable_shard_manager_config() -> ShardManagerConfig {
    ShardManagerConfig {
        // Ephemeral ports: this replica coexists with whatever else the test binary is running.
        http_port: 0,
        grpc: GrpcApiConfig {
            port: 0,
            ..GrpcApiConfig::default()
        },
        persistence: PersistenceConfig::Etcd(unreachable_config()),
        ..ShardManagerConfig::default()
    }
}

#[test]
#[tracing::instrument(skip_all)]
// The shard count read runs before the campaign, so it is the one place an unreachable etcd could
// fail a replica outright - no campaign, no standby, no retry. It has to wait instead, and the
// counter has to move from here too, or a replica wedged in that read is indistinguishable from a
// standby queued behind a healthy leader.
async fn a_pre_campaign_read_that_cannot_reach_etcd_does_not_fail_startup() {
    let before = campaign_attempt_failures().unwrap_or(0);
    let config = unreachable_shard_manager_config();
    let shutdown = CancellationToken::new();
    let mut join_set = tokio::task::JoinSet::new();
    // Scoped so that the borrow `run()` holds on the join set ends before it is drained.
    let err = {
        let mut run = std::pin::pin!(golem_shard_manager::run(
            &config,
            Deployment::Standalone {
                shutdown: shutdown.clone()
            },
            prometheus::Registry::new(),
            &mut join_set,
        ));

        let gave_up = timeout(STARTUP_SETTLE, &mut run).await;
        assert!(
            gave_up.is_err(),
            "`run()` returned inside {STARTUP_SETTLE:?} while etcd was unreachable, so the read \
             it makes before campaigning failed the replica outright instead of waiting for etcd \
             to come back. A restarting member would take every replica down with it."
        );

        shutdown.cancel();
        let stopped = timeout(SHUTDOWN_TIMEOUT, run).await.expect(
            "A retry loop that ignores the shutdown token leaves the process unable to stop at all",
        );
        match stopped {
            Err(err) => err,
            Ok(details) => panic!(
                "The replica reported gRPC port {} against an unreachable etcd",
                details.grpc_port
            ),
        }
    };
    join_set.abort_all();

    assert!(
        err.chain().any(|cause| matches!(
            cause.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::ShutdownRequested)
        )),
        "A cancelled startup must report the shutdown it was asked for, not the read that was in \
         flight: {err:#}"
    );

    let after = campaign_attempt_failures().unwrap_or_else(|| {
        panic!(
            "`{CAMPAIGN_FAILURES_METRIC}` was never registered, so a replica wedged in the \
             pre-campaign read exports nothing to distinguish it from one that is patiently waiting."
        )
    });
    assert!(
        after > before,
        "The pre-campaign read failed repeatedly against `{UNREACHABLE_ENDPOINT}` but \
         `{CAMPAIGN_FAILURES_METRIC}` stayed at {before}. Over its one remaining listener this \
         replica is now indistinguishable from a healthy standby, and it used to exit loudly."
    );
}
