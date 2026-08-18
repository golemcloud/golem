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

//! The pinned in-flight workload (GOL-366).
//!
//! S12 disturbs a stream of short operations and asks what the platform did.
//! This asks a sharper question: take *these fifty* HTTP `invoke_and_await`
//! calls, prove they are running on *that* executor, kill it, and account for
//! every one of them afterwards. Three things follow from wanting an answer
//! that specific.
//!
//! **The driver has to choose the fault's target.** Chaos Mesh's `mode: one`
//! picks a pod at random, which cannot be right here — the whole claim is about
//! the pod that owned the work. Ownership is a pure function of the agent id and
//! the routing table ([`ShardId::from_agent_id`] into
//! [`RoutingTable::lookup`]), and the driver is the only side of the workflow
//! boundary that holds both. So it scans a candidate pool, finds an executor
//! that owns enough of them, and names it in the readiness signal. It still says
//! nothing about Kubernetes: it reports the `ip:port` the routing table uses and
//! lets the workflow resolve that to a pod.
//!
//! **Ownership is re-verified immediately before the signal.** A rebalance
//! between selection and injection would leave the run aimed at the wrong pod
//! while still reporting success, which is worse than not running at all.
//!
//! **Operations are long, and one per agent.** Each agent holds exactly one
//! `sleep_and_increment` in flight at a time, so the in-flight count is the
//! agent count — constant, and known without sampling. Sleeping rather than
//! spinning keeps this a crash-recovery experiment: fifty CPU-bound operations
//! on one executor would be measuring saturation instead.
//!
//! ### The exactly-once probe
//!
//! After the run settles, every key is re-invoked once under its *original*
//! idempotency key. The platform looks a key's result up before enqueuing
//! anything (`Worker::invoke_internal` → `LookupResult::Complete`), so a key
//! that already ran replays its stored result rather than running again. That
//! turns two otherwise-unanswerable questions into direct observations:
//!
//! - *Did accepted work end up with a final result?* The probe returns one, or
//!   it does not.
//! - *Did anything run twice?* A key whose probe returns a **different** value
//!   from the one the driver was given executed a second time.
//!
//! Reading the counters immediately before and immediately after the probe pass
//! bounds the probe's own footprint: the delta is exactly how many keys had
//! never run, which is what makes "the probe replayed a stored result" and "the
//! probe executed fresh work" distinguishable in aggregate.

use crate::chaos::PinnedConfig;
use crate::chaos::errors::ErrorClass;
use crate::chaos::history::{OperationRecord, Stream};
use crate::chaos::workload::{self, WorkloadContext};
use anyhow::Context;
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::model::{AgentId, RoutingTable};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Agent type the pinned stream drives. The same durable `Counter` the mixed
/// workload uses, so its state can be read back the same way.
const COUNTER_AGENT: &str = "Counter";

/// The method held in flight. See the counters component: it waits, *then*
/// increments, so the state change falls inside the fault window.
const PINNED_METHOD: &str = "sleep_and_increment";

/// How many probes run at once. The probe pass is thousands of replays of
/// stored results, so it wants some concurrency — but it runs against a cluster
/// that has just been through a pod kill, and hammering it would be a second
/// experiment nobody asked for.
const PROBE_CONCURRENCY: usize = 32;

/// Duration passed to a probe invocation, in milliseconds.
///
/// Zero, and deliberately not the workload's duration: a probe of a key that
/// already ran replays its stored result and ignores the arguments entirely,
/// while a probe of a key that never ran executes for real. Making that second
/// case instant keeps the probe pass from taking as long as the run it is
/// measuring — and the fresh execution still shows up in the counter delta,
/// which is what the pass is there to measure.
const PROBE_MILLIS: u32 = 0;

/// One executor, the agents it owns, and how the choice was made.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedSelection {
    /// The executor endpoint as the shard-manager names it, e.g.
    /// `10.0.14.207:9000`.
    pub pod_address: String,
    /// Host part of the address, which is what a Kubernetes `status.podIP`
    /// field selector matches.
    pub pod_ip: String,
    /// The agents this pod owns, in the order the scenario drives them.
    pub agents: Vec<String>,
    /// Shard count the routing table reported. Recorded because ownership is a
    /// hash modulo this number, so a selection cannot be re-derived later
    /// without it.
    pub number_of_shards: usize,
    /// How the scanned candidates spread across executors. An operator reading
    /// a run that failed to find a target needs to see whether the pool was too
    /// small or the cluster too lopsided.
    pub candidates_per_pod: BTreeMap<String, usize>,
    /// Candidates scanned to reach this selection.
    pub candidates_scanned: u32,
}

/// Names the `index`-th candidate agent of the pinned stream.
///
/// Candidates are drawn from a pool larger than the number of agents needed,
/// because which pod owns which name is a hash the driver cannot steer. The
/// names that end up selected are sparse in this space, which is why the
/// selection records them explicitly rather than a range.
pub fn candidate_agent_name(key_prefix: &str, index: u32) -> String {
    format!("{key_prefix}-{}-{index:04}", Stream::PinnedHttp)
}

/// The routing-table `AgentId` for one pinned agent.
///
/// This has to match how the worker-service builds the id it routes on —
/// the component id plus the *string form* of the parsed agent id — or the
/// ownership calculation would be answering a different question from the one
/// the platform answers.
fn routing_agent_id(ctx: &WorkloadContext, agent: &str) -> AgentId {
    let parsed: ParsedAgentId = agent_id!(COUNTER_AGENT, agent.to_string());
    AgentId {
        component_id: ctx.counters.id,
        agent_id: parsed.to_string(),
    }
}

/// Groups candidate agents by the executor that owns them.
fn owners(
    ctx: &WorkloadContext,
    table: &RoutingTable,
    candidates: &[String],
) -> BTreeMap<String, Vec<String>> {
    let mut by_pod: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for agent in candidates {
        if let Some(pod) = table.lookup(&routing_agent_id(ctx, agent)) {
            by_pod
                .entry(pod.to_string())
                .or_default()
                .push(agent.clone());
        }
    }
    by_pod
}

/// Chooses an executor and the agents it owns.
///
/// Fails rather than falling back to an unpinned run: a scenario that quietly
/// stopped verifying ownership would still produce a plausible-looking report,
/// and that report would be about a pod kill that hit an unrelated executor.
pub async fn select(
    ctx: &WorkloadContext,
    deps: &BenchmarkTestDependencies,
    config: &PinnedConfig,
) -> anyhow::Result<PinnedSelection> {
    let table = deps
        .shard_manager()
        .get_routing_table()
        .await
        .context("reading the routing table to pin the fault target")?;

    let pool = config
        .agents
        .saturating_mul(config.candidate_pool_multiplier.max(1));
    let candidates: Vec<String> = (0..pool)
        .map(|index| candidate_agent_name(&ctx.key_prefix, index))
        .collect();

    let by_pod = owners(ctx, &table, &candidates);
    let candidates_per_pod: BTreeMap<String, usize> = by_pod
        .iter()
        .map(|(pod, xs)| (pod.clone(), xs.len()))
        .collect();

    // Most-owned wins. Any pod owning enough would do, but taking the busiest
    // keeps the selection stable across reruns on an unchanged cluster, which
    // makes two runs comparable.
    let (pod_address, owned) = by_pod
        .iter()
        .max_by_key(|(_, agents)| agents.len())
        .map(|(pod, agents)| (pod.clone(), agents.clone()))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "routing table assigned none of the {pool} candidate agents to any executor"
            )
        })?;

    if owned.len() < config.agents as usize {
        anyhow::bail!(
            "no single executor owns {} of the {pool} candidate agents (best was {} with {}); \
             raise `candidatePoolMultiplier` or check the shard distribution: {candidates_per_pod:?}",
            config.agents,
            pod_address,
            owned.len()
        );
    }

    let agents: Vec<String> = owned.into_iter().take(config.agents as usize).collect();
    let pod_ip = pod_address
        .rsplit_once(':')
        .map(|(host, _)| host.to_string())
        .unwrap_or_else(|| pod_address.clone());

    info!(
        "S8: pinned {} agents to executor {pod_address} (scanned {pool} candidates across {} executors)",
        agents.len(),
        candidates_per_pod.len()
    );

    Ok(PinnedSelection {
        pod_address,
        pod_ip,
        agents,
        number_of_shards: table.number_of_shards.value,
        candidates_per_pod,
        candidates_scanned: pool,
    })
}

/// Re-checks, against a freshly read routing table, that every pinned agent is
/// still owned by the selected executor.
///
/// Called immediately before the readiness signal. A rebalance between selection
/// and injection is rare but not impossible, and a run that aimed at the wrong
/// pod while reporting success would be worse than no run at all.
pub async fn verify_ownership(
    ctx: &WorkloadContext,
    deps: &BenchmarkTestDependencies,
    selection: &PinnedSelection,
) -> anyhow::Result<()> {
    let table = deps
        .shard_manager()
        .get_routing_table()
        .await
        .context("re-reading the routing table to verify pinned ownership")?;

    let mut drifted: Vec<String> = Vec::new();
    for agent in &selection.agents {
        let owner = table
            .lookup(&routing_agent_id(ctx, agent))
            .map(|pod| pod.to_string());
        if owner.as_deref() != Some(selection.pod_address.as_str()) {
            drifted.push(format!(
                "{agent} now owned by {}",
                owner.unwrap_or_else(|| "nobody".to_string())
            ));
        }
    }

    if !drifted.is_empty() {
        anyhow::bail!(
            "{} of {} pinned agents are no longer owned by {}: {}",
            drifted.len(),
            selection.agents.len(),
            selection.pod_address,
            drifted.join(", ")
        );
    }

    info!(
        "S8: verified all {} pinned agents are still owned by {}",
        selection.agents.len(),
        selection.pod_address
    );
    Ok(())
}

/// A running pinned workload. As with the mixed workload, dropping the handle
/// does not stop it — call [`PinnedHandle::stop`] so in-flight operations record
/// themselves instead of being cancelled, which during a fault would lose
/// exactly the operations worth having.
pub struct PinnedHandle {
    stop: Arc<AtomicU8>,
    tasks: JoinSet<()>,
    submitted: Arc<AtomicU64>,
}

impl PinnedHandle {
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    pub async fn stop(mut self) {
        self.stop.store(1, Ordering::Relaxed);
        while self.tasks.join_next().await.is_some() {}
        info!(
            "Chaos pinned workload stopped after {} operations",
            self.submitted()
        );
    }
}

/// Starts one long-running operation per pinned agent and keeps replacing each
/// one as it finishes, so the in-flight count stays at the agent count for the
/// whole run.
pub fn start(
    ctx: WorkloadContext,
    selection: &PinnedSelection,
    config: &PinnedConfig,
) -> PinnedHandle {
    let stop = Arc::new(AtomicU8::new(0));
    let submitted = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();
    let millis = config.operation_millis;

    info!(
        "Chaos pinned workload starting: {} concurrent {PINNED_METHOD}({millis}ms) operations on {}",
        selection.agents.len(),
        selection.pod_address
    );

    for agent in &selection.agents {
        let ctx = ctx.clone();
        let stop = stop.clone();
        let submitted = submitted.clone();
        let agent = agent.clone();

        tasks.spawn(async move {
            let mut seq: u64 = 0;
            while stop.load(Ordering::Relaxed) == 0 {
                submitted.fetch_add(1, Ordering::Relaxed);
                submit_one(&ctx, &agent, seq, millis).await;
                seq += 1;
            }
        });
    }

    PinnedHandle {
        stop,
        tasks,
        submitted,
    }
}

/// Submits one pinned operation and waits for it to resolve.
async fn submit_one(ctx: &WorkloadContext, agent: &str, seq: u64, millis: u32) {
    let key = ctx.idempotency_key(agent, seq);
    let parsed: ParsedAgentId = agent_id!(COUNTER_AGENT, agent.to_string());
    let ctx2 = ctx.clone();

    workload::run_operation(
        ctx,
        Stream::PinnedHttp,
        agent.to_string(),
        PINNED_METHOD,
        key,
        |k| {
            let ctx = ctx2.clone();
            let parsed = parsed.clone();
            async move {
                let value = ctx
                    .user
                    .invoke_and_await_agent_with_key(
                        &ctx.counters,
                        &parsed,
                        &k,
                        PINNED_METHOD,
                        data_value!(millis),
                    )
                    .await?;
                Ok(workload::as_u32_value(value))
            }
        },
    )
    .await;
}

/// What the platform said when a key was asked about again after recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyProbe {
    pub idempotency_key: String,
    pub agent: String,
    /// The value the platform returned. `None` means the probe itself failed,
    /// which for an accepted operation is the scenario's hard failure: after
    /// recovery, work the platform took has to have a result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_value: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<ErrorClass>,
}

/// Re-invokes every recorded key under its original idempotency key.
///
/// Read the module documentation before changing anything here: the pass only
/// means what it means because the key is reused verbatim and because the
/// counters are read on either side of it.
pub async fn probe_keys(ctx: &WorkloadContext, records: &[OperationRecord]) -> Vec<KeyProbe> {
    let keys: Vec<(String, String)> = records
        .iter()
        .filter(|r| r.stream == Stream::PinnedHttp)
        .map(|r| (r.idempotency_key.clone(), r.agent.clone()))
        .collect();

    info!("S8: probing {} keys for their final results", keys.len());

    let mut probes = Vec::with_capacity(keys.len());
    for chunk in keys.chunks(PROBE_CONCURRENCY) {
        let mut batch = JoinSet::new();
        for (key, agent) in chunk {
            let ctx = ctx.clone();
            let key = key.clone();
            let agent = agent.clone();
            batch.spawn(async move { probe_one(&ctx, &agent, &key).await });
        }
        while let Some(joined) = batch.join_next().await {
            match joined {
                Ok(probe) => probes.push(probe),
                Err(e) => warn!("S8: a probe task panicked: {e}"),
            }
        }
    }

    probes.sort_by(|a, b| a.idempotency_key.cmp(&b.idempotency_key));
    probes
}

/// Probes one key, retrying once on a transport failure.
///
/// The retry follows the same rule as the workload itself — one attempt, same
/// idempotency key, transport failures only — and here it is purely about not
/// losing a key to a single dropped connection. A probe that cannot complete
/// leaves the key inconclusive, and an inconclusive key weakens the verdict; a
/// cheap same-key retry buys most of them back. It cannot mask a real problem,
/// because a definite refusal is not retried.
async fn probe_one(ctx: &WorkloadContext, agent: &str, key: &str) -> KeyProbe {
    let parsed: ParsedAgentId = agent_id!(COUNTER_AGENT, agent.to_string());
    let idempotency_key = golem_common::model::IdempotencyKey::new(key.to_string());

    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let outcome = ctx
            .user
            .invoke_and_await_agent_with_key(
                &ctx.counters,
                &parsed,
                &idempotency_key,
                PINNED_METHOD,
                data_value!(PROBE_MILLIS),
            )
            .await;

        match outcome {
            Ok(value) => {
                return KeyProbe {
                    idempotency_key: key.to_string(),
                    agent: agent.to_string(),
                    final_value: workload::as_u32_value(value),
                    error: None,
                    error_class: None,
                };
            }
            Err(e) => {
                let class = crate::chaos::errors::classify(&e);
                if attempts <= ctx.retry.max_retries && class.is_retryable_transport_failure() {
                    tokio::time::sleep(ctx.retry.delay()).await;
                    continue;
                }
                if !class.is_definite_rejection() {
                    warn!(
                        "S8: probe of {key} could not complete ({class}), leaving the key \
                         inconclusive rather than reporting a lost result: {e:#}"
                    );
                }
                return KeyProbe {
                    idempotency_key: key.to_string(),
                    agent: agent.to_string(),
                    final_value: None,
                    error: Some(format!("{e:#}")),
                    error_class: Some(class),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// The workflow's pod selector is built from `podIp`, so the split has to
    /// hold for the `ip:port` form the routing table actually produces.
    #[test]
    fn a_pod_address_splits_into_an_ip_the_workflow_can_select_on() {
        let split = |address: &str| {
            address
                .rsplit_once(':')
                .map(|(host, _)| host.to_string())
                .unwrap_or_else(|| address.to_string())
        };
        assert_eq!(split("10.0.14.207:9000"), "10.0.14.207");
        // No port at all is not a shape the shard-manager produces, but
        // degrading to the whole string beats panicking mid-run.
        assert_eq!(split("10.0.14.207"), "10.0.14.207");
    }

    /// Candidate names carry the run prefix and the stream, and are zero-padded
    /// so they sort — the same properties the mixed workload's names have, for
    /// the same reason: a trace search has to be narrowable to one run.
    #[test]
    fn candidate_names_are_prefixed_by_run_and_stream_and_sort() {
        assert_eq!(
            candidate_agent_name("chaos-s8", 7),
            "chaos-s8-pinned-http-0007"
        );
        assert!(
            candidate_agent_name("chaos-s8", 42) > candidate_agent_name("chaos-s8", 41),
            "zero-padded indices must order lexicographically"
        );
    }
}
