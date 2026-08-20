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

//! The scheduled-registration workload (GOL-378).
//!
//! The mixed workload's scheduled stream registers a poll and counts the fires.
//! This one registers a `fire` carrying the registration's own idempotency key,
//! which is what lets [`crate::chaos::fires`] pair an individual action against
//! the invocation that asked for it.
//!
//! ## Why the cadence, and why the lead
//!
//! `lead x targets / interval` is the population that matters: actions the
//! platform has accepted and not yet run. At 100 targets registering every two
//! seconds, ten seconds ahead, that is five hundred of them standing at any
//! instant, spread across both executors' shards. A kill lands in the middle of
//! that population by construction, which is what makes the cadence and the
//! lead the two numbers that decide whether the run measures anything.
//!
//! The lead also has to comfortably exceed the workflow's inject-and-verify
//! path — signal poll, `kubectl apply`, waiting for `AllInjected` — or every
//! action registered before the kill would already have fired by the time the
//! pod died. See [`crate::chaos::scenarios::s10`] for why the narrower
//! claim-to-acknowledge window is not something the driver tries to aim at.
//!
//! ## Why the target set is split rather than pinned
//!
//! [`crate::chaos::pinned`] drives *only* the agents its chosen executor owns,
//! because an S8 operation that was not on the dead pod says nothing. Here the
//! opposite is true: the actions on the surviving executor are the control
//! group. Every target is driven, the driver names the executor owning the
//! largest share, and the report splits the two so a lease recovery that took
//! its full TTL cannot hide behind the half of the population that was never
//! disturbed.

use crate::chaos::ScheduledConfig;
use crate::chaos::history::{FireRecord, Stream, TargetFireLog};
use crate::chaos::pinned::{owners_by_pod, pod_ip_of};
use crate::chaos::workload::{
    self, SCHEDULE_COUNTER_AGENT, SCHEDULE_EMITTER_AGENT, WorkloadContext,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Registrations one target may have in flight at once.
///
/// Small on purpose. The cadence is what sets the rate; this only keeps a
/// stalled platform from accumulating tasks, and a target that has spent its
/// budget skips its tick and says so rather than queueing behind itself.
pub const MAX_IN_FLIGHT_PER_TARGET: usize = 8;

/// How many targets are read back at once. Same reasoning as
/// [`crate::chaos::scenarios::read_back_agents`]: reads do not mutate, and
/// walking them one at a time behind a per-read ceiling outlasts the
/// maintenance window.
const READ_CONCURRENCY: usize = 16;

/// The smallest share of targets one executor must own for the run to mean
/// anything, as a divisor of the target count.
///
/// A two-executor cluster splits a hashed population roughly evenly, so a quarter
/// is a floor rather than an expectation. Below it the "affected" group is too
/// small for its percentile to say anything, and a run that reported one anyway
/// would be worse than one that refused.
const MIN_TARGET_SHARE_DIVISOR: usize = 4;

/// The executor the fault will be aimed at, and how the targets divide around
/// it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledSelection {
    /// The executor endpoint as the shard-manager names it, e.g.
    /// `10.0.14.207:9000`.
    pub pod_address: String,
    /// Host part of the address, which is what a Kubernetes `status.podIP`
    /// field selector matches.
    pub pod_ip: String,
    /// Targets this executor owns. The population whose actions have to survive
    /// a lease recovery.
    pub on_pod: Vec<String>,
    /// Targets owned by any other executor: the run's own control group.
    pub elsewhere: Vec<String>,
    /// How the targets spread across executors, so a run that refused to
    /// proceed says whether the cluster was lopsided or the pool too small.
    pub targets_per_pod: BTreeMap<String, usize>,
    /// Shard count the routing table reported. Ownership is a hash modulo this,
    /// so a selection cannot be re-derived later without it.
    pub number_of_shards: usize,
}

/// Chooses the executor to aim at: the one owning the largest share of targets.
///
/// Fails rather than proceeding unaimed. Chaos Mesh's `mode: one` would pick a
/// pod at random, and a run that killed an executor owning six targets out of a
/// hundred would still produce a confident-looking report about lease recovery.
pub async fn select(
    ctx: &WorkloadContext,
    deps: &BenchmarkTestDependencies,
    targets: &[String],
) -> anyhow::Result<ScheduledSelection> {
    let table = deps
        .shard_manager()
        .get_routing_table()
        .await
        .context("reading the routing table to aim the scheduled fault")?;

    let by_pod = owners_by_pod(ctx, &table, SCHEDULE_COUNTER_AGENT, targets);
    let targets_per_pod: BTreeMap<String, usize> = by_pod
        .iter()
        .map(|(pod, xs)| (pod.clone(), xs.len()))
        .collect();

    let (pod_address, on_pod) = by_pod
        .iter()
        .max_by_key(|(_, agents)| agents.len())
        .map(|(pod, agents)| (pod.clone(), agents.clone()))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "routing table assigned none of the {} schedule targets to any executor",
                targets.len()
            )
        })?;

    let floor = (targets.len() / MIN_TARGET_SHARE_DIVISOR).max(1);
    if on_pod.len() < floor {
        anyhow::bail!(
            "the most-loaded executor owns only {} of {} schedule targets, below the {floor} \
             needed for its share to be worth measuring: {targets_per_pod:?}",
            on_pod.len(),
            targets.len()
        );
    }

    let elsewhere: Vec<String> = targets
        .iter()
        .filter(|t| !on_pod.contains(t))
        .cloned()
        .collect();

    info!(
        "S10: aiming at executor {pod_address}, which owns {} of {} schedule targets ({} \
         elsewhere, across {} executors)",
        on_pod.len(),
        targets.len(),
        elsewhere.len(),
        targets_per_pod.len()
    );

    Ok(ScheduledSelection {
        pod_ip: pod_ip_of(&pod_address),
        pod_address,
        on_pod,
        elsewhere,
        targets_per_pod,
        number_of_shards: table.number_of_shards.value,
    })
}

/// Re-checks, against a freshly read routing table, that the targets are still
/// divided the way the selection says.
///
/// Called immediately before the readiness signal, for the same reason S8 does
/// it: a rebalance between selection and injection would leave the run reporting
/// a control group that was actually the affected one.
pub async fn verify_ownership(
    ctx: &WorkloadContext,
    deps: &BenchmarkTestDependencies,
    selection: &ScheduledSelection,
) -> anyhow::Result<()> {
    let table = deps
        .shard_manager()
        .get_routing_table()
        .await
        .context("re-reading the routing table to verify scheduled target ownership")?;

    let mut drifted = Vec::new();
    for agent in &selection.on_pod {
        let owner = table
            .lookup(&crate::chaos::pinned::routing_agent_id(
                ctx,
                SCHEDULE_COUNTER_AGENT,
                agent,
            ))
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
            "{} of {} schedule targets are no longer owned by {}: {}",
            drifted.len(),
            selection.on_pod.len(),
            selection.pod_address,
            drifted.join(", ")
        );
    }

    info!(
        "S10: verified all {} schedule targets are still owned by {}",
        selection.on_pod.len(),
        selection.pod_address
    );
    Ok(())
}

/// A running registration workload. As elsewhere, dropping the handle does not
/// stop it: call [`ScheduledHandle::stop`] so in-flight registrations record
/// themselves instead of being cancelled mid-flight.
pub struct ScheduledHandle {
    stop: Arc<AtomicU8>,
    tasks: JoinSet<()>,
    submitted: Arc<AtomicU64>,
    skipped: Arc<AtomicU64>,
}

impl ScheduledHandle {
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    /// Ticks a target dropped because it already had its budget of
    /// registrations in flight. Non-zero means the platform was slow enough to
    /// clamp the cadence, which is context for reading everything else.
    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }

    pub async fn stop(mut self) {
        self.stop.store(1, Ordering::Relaxed);
        while self.tasks.join_next().await.is_some() {}
        info!(
            "Chaos scheduled workload stopped after {} registrations ({} ticks skipped)",
            self.submitted(),
            self.skipped()
        );
    }
}

/// Starts one registration loop per target.
pub fn start(
    ctx: WorkloadContext,
    targets: &[String],
    config: &ScheduledConfig,
) -> ScheduledHandle {
    let stop = Arc::new(AtomicU8::new(0));
    let submitted = Arc::new(AtomicU64::new(0));
    let skipped = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();
    let interval = config.interval();
    let lead = config.lead();
    let count = targets.len().max(1);

    info!(
        "Chaos scheduled workload starting: {} targets, one registration every {:?} each \
         ({:.1}/s overall), {:?} ahead",
        targets.len(),
        interval,
        targets.len() as f64 / interval.as_secs_f64(),
        lead
    );

    for (index, target) in targets.iter().enumerate() {
        let ctx = ctx.clone();
        let stop = stop.clone();
        let submitted = submitted.clone();
        let skipped = skipped.clone();
        let target = target.clone();
        // Spread the loops across one interval so the whole population does not
        // register in the same instant, which would make the offered rate a
        // sawtooth instead of the constant the phase stats assume.
        let stagger = interval.mul_f64(index as f64 / count as f64);

        tasks.spawn(async move {
            tokio::time::sleep(stagger).await;
            let budget = Arc::new(Semaphore::new(MAX_IN_FLIGHT_PER_TARGET));
            let mut in_flight = JoinSet::new();
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut seq = 0u64;

            while stop.load(Ordering::Relaxed) == 0 {
                ticker.tick().await;
                if stop.load(Ordering::Relaxed) != 0 {
                    break;
                }
                let Ok(permit) = budget.clone().try_acquire_owned() else {
                    skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                submitted.fetch_add(1, Ordering::Relaxed);
                let ctx = ctx.clone();
                let target = target.clone();
                let this_seq = seq;
                seq += 1;
                in_flight.spawn(async move {
                    let _permit = permit;
                    register_one(&ctx, index as u32, &target, this_seq, lead).await;
                });
                while in_flight.try_join_next().is_some() {}
            }

            // Drain rather than cancel: a registration cancelled mid-flight is
            // one the history cannot classify, and during a fault those are the
            // interesting ones.
            while in_flight.join_next().await.is_some() {}
        });
    }

    ScheduledHandle {
        stop,
        tasks,
        submitted,
        skipped,
    }
}

/// Registers one action, `lead` from now, under a key the fire will carry back.
async fn register_one(ctx: &WorkloadContext, index: u32, target: &str, seq: u64, lead: Duration) {
    let emitter = ctx.agent_name(Stream::Scheduled, index);
    let key = ctx.idempotency_key(target, seq);
    let parsed: ParsedAgentId = agent_id!(SCHEDULE_EMITTER_AGENT, emitter);

    // Computed once, outside the retry, so a retried registration asks for the
    // same instant. A due time that moved with each attempt would make the fire
    // log's `scheduledMillis` disagree with what the driver believes it asked
    // for, and the delay measurement would be against the wrong baseline.
    let fire_at = SystemTime::now() + lead;
    let since_epoch = fire_at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (secs, nanos) = (since_epoch.as_secs(), since_epoch.subsec_nanos());

    let ctx2 = ctx.clone();
    let target2 = target.to_string();
    // Recorded against the target rather than the emitter, because the target is
    // where the fire lands and where the read-back looks.
    workload::run_operation(
        ctx,
        Stream::Scheduled,
        target.to_string(),
        "schedule_fire_at",
        key,
        |k| {
            let ctx = ctx2.clone();
            let parsed = parsed.clone();
            let target = target2.clone();
            async move {
                let token = k.value.clone();
                ctx.user
                    .invoke_and_await_agent_with_key(
                        &ctx.counters,
                        &parsed,
                        &k,
                        "schedule_fire_at",
                        data_value!(target, secs, nanos, token),
                    )
                    .await?;
                Ok(None)
            }
        },
    )
    .await;
}

/// Creates every emitter and target before the baseline starts.
///
/// Returns how many agents were touched. Residency matters here for the same
/// reason it does in S8: an agent that has to cold-start on first use would put
/// start-up cost inside the baseline the recovery is measured against.
pub async fn warm(ctx: &WorkloadContext, targets: &[String]) -> usize {
    let mut warmed = 0usize;
    for (offset, chunk) in targets.chunks(READ_CONCURRENCY).enumerate() {
        let mut batch = JoinSet::new();
        for (position, target) in chunk.iter().cloned().enumerate() {
            let ctx = ctx.clone();
            let index = (offset * READ_CONCURRENCY + position) as u32;
            batch.spawn(async move {
                let emitter = ctx.agent_name(Stream::Scheduled, index);
                let parsed: ParsedAgentId = agent_id!(SCHEDULE_EMITTER_AGENT, emitter.clone());
                if let Err(e) = ctx
                    .user
                    .invoke_and_await_agent(&ctx.counters, &parsed, "warm", data_value!())
                    .await
                {
                    warn!("S10: could not warm emitter {emitter}: {e:#}");
                }
                // Reading the target creates it without mutating it, which is
                // exactly the side effect wanted.
                if let Err(e) = workload::read_polls(&ctx, &target).await {
                    warn!("S10: could not warm target {target}: {e}");
                }
            });
        }
        while batch.join_next().await.is_some() {
            warmed += 1;
        }
    }
    warmed
}

/// Reads every target's fire log back.
///
/// A failed read is carried as [`TargetFireLog::error`] rather than dropped: an
/// agent that could not be read says nothing either way about its actions, and
/// the account has to be able to tell that apart from an agent that lost them.
pub async fn read_logs(ctx: &WorkloadContext, targets: &[String]) -> Vec<TargetFireLog> {
    let total = targets.len();
    let mut logs = Vec::with_capacity(total);

    for chunk in targets.chunks(READ_CONCURRENCY) {
        let mut batch = JoinSet::new();
        for target in chunk.iter().cloned() {
            let ctx = ctx.clone();
            batch.spawn(async move {
                let polls = workload::read_polls(&ctx, &target).await.ok();
                let fires = workload::read_fires(&ctx, &target).await;
                match fires {
                    Ok(raw) => TargetFireLog {
                        agent: target,
                        polls,
                        fires: to_fire_records(&raw),
                        error: None,
                    },
                    Err(e) => TargetFireLog {
                        agent: target,
                        polls,
                        fires: Vec::new(),
                        error: Some(e),
                    },
                }
            });
        }

        let mut batch_results = Vec::new();
        while let Some(joined) = batch.join_next().await {
            match joined {
                Ok(log) => batch_results.push(log),
                Err(e) => warn!("S10: a fire-log read task panicked: {e}"),
            }
        }
        batch_results.sort_by(|a, b| a.agent.cmp(&b.agent));
        logs.extend(batch_results);
        info!("S10: read fire logs for {} of {total} targets", logs.len());
    }

    logs
}

/// Turns the agent's raw triples into timestamps.
///
/// An entry whose millis cannot be a timestamp is dropped rather than clamped.
/// Dropping leaves the log shorter than the target's own `polls`, which is what
/// the account reads as "this target cannot testify" — a clamped nonsense
/// timestamp would instead have produced a confident wrong delay.
pub fn to_fire_records(raw: &[(String, u64, u64)]) -> Vec<FireRecord> {
    let mut out = Vec::with_capacity(raw.len());
    for (token, scheduled_millis, observed_millis) in raw {
        match (
            millis_to_time(*scheduled_millis),
            millis_to_time(*observed_millis),
        ) {
            (Some(scheduled_at), Some(observed_at)) => out.push(FireRecord {
                token: token.clone(),
                scheduled_at,
                observed_at,
            }),
            _ => warn!(
                "S10: dropping fire log entry {token} with unreadable timestamps \
                 ({scheduled_millis}, {observed_millis})"
            ),
        }
    }
    out
}

fn millis_to_time(millis: u64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(i64::try_from(millis).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// The happy path of the conversion the whole account is built on.
    #[test]
    fn raw_triples_become_timestamped_fire_records() {
        let raw = vec![("t-0".to_string(), 1_800_000_000_000, 1_800_000_000_450)];
        let records = to_fire_records(&raw);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, "t-0");
        assert_eq!(records[0].delay_ms(), 450);
    }

    /// An unreadable entry has to shorten the log rather than become a
    /// confident wrong delay: the shortfall against `polls` is what tells the
    /// account this target cannot testify.
    #[test]
    fn an_entry_with_an_impossible_timestamp_is_dropped_rather_than_clamped() {
        let raw = vec![
            ("good".to_string(), 1_800_000_000_000, 1_800_000_000_010),
            ("bad".to_string(), u64::MAX, 1_800_000_000_010),
        ];
        let records = to_fire_records(&raw);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, "good");

        let log = TargetFireLog {
            agent: "target-0".to_string(),
            polls: Some(2),
            fires: records,
            error: None,
        };
        assert!(
            !log.is_complete(),
            "a dropped entry must leave the log short of polls"
        );
    }
}
