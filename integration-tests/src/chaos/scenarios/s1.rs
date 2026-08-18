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

//! S1 — shard-manager / executor network partition (GOL-364).
//!
//! S12 takes the shard-manager away entirely; S8 takes one executor away. S1
//! takes away neither — everything stays running, and only the *link* between
//! the shard-manager and some of the executors is cut. That is a strictly
//! nastier fault, because nothing crashes and nothing restarts: both sides keep
//! serving, each believing its own view is current, and the question is what
//! they agree on once they can talk again.
//!
//! The failure being ruled out is **overlapping ownership**. An agent lives on
//! exactly one executor; if two executors both believe they own its shard, both
//! will run its invocations and its state forks, with no reconciliation and no
//! way back. That is the one thing this scenario asserts. See
//! [`crate::chaos::ownership`] for why the other three ownership findings are
//! reported instead.
//!
//! The choreography:
//!
//! 1. **Baseline** — the mixed workload, as S12 runs it, so the partition lands
//!    on a warm cluster rather than a cold one.
//! 2. **Sample** — every executor's own view of what it owns, before anything
//!    is broken. This is the reference the later samples are read against.
//! 3. **Signal** — announce readiness. Unlike S8 the driver names no target:
//!    the partition is selected by label, and *which* executors get cut off
//!    carries no information the run depends on.
//! 4. **Fault** — keep the workload running through the partition, and sample
//!    ownership again while it is active. Divergence here is the fault working,
//!    not a defect, so this sample is evidence rather than verdict.
//! 5. **Settle** — after the workflow reports the partition healed, wait. This
//!    is the load-bearing wait of the whole scenario: sampling too early
//!    measures a rebalance in progress, where transient disagreement is normal.
//! 6. **Judge** — sample once more. *This* sample is the one the run is judged
//!    on, cross-checked against the shard-manager's routing table.
//!
//! ### How overlap is actually detected
//!
//! Not by asking the executors. The shard-manager's routing table is a map —
//! one pod per shard by construction — so it can never *show* two owners, and a
//! cut-off executor that still believes it owns a shard is invisible to it.
//!
//! It is caught by its consequence instead. If two executors both served an
//! agent, one idempotency key executed twice, and the exactly-once probe in
//! [`crate::chaos::probe`] observes that exactly, per key, from outside the
//! cluster: after the run settles every key is re-invoked under its original
//! key, and a key that comes back with a different counter value than the
//! driver was given ran more than once.
//!
//! That is a stronger claim than reading beliefs — it is evidence the platform
//! actually forked an agent's state, not evidence it was in a position to — and
//! it needs nothing from the worker-executor that is not already there. The
//! routing-table samples in [`crate::chaos::ownership`] supply the context that
//! makes such a finding readable: which shards moved, and when.

use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::probe;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ReadKind, ScenarioOutcome, build_result, read_back_agents, signal_termination,
    snapshot_routing, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals, ScaleEvent};
use crate::chaos::summary::{AgentReadback, ChaosSummary, ExactlyOnceReport, TerminationReason};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait after stopping the workload before reading durable state.
/// Same reasoning as S12: an in-flight increment still has to land, and reading
/// early would report a mismatch that says nothing about the platform.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(30);

/// How far into the fault window the `during-fault` assignment sample is taken,
/// as a fraction of that window, and the ceiling on it.
///
/// The shard-manager reassigns a cut-off executor's shards only after it has
/// missed enough health checks. Sampling at injection time would reliably show
/// an unchanged table and prove nothing about the fault.
const DURING_FAULT_SAMPLE_FRACTION: f64 = 0.6;
const DURING_FAULT_SAMPLE_CAP: Duration = Duration::from_secs(120);

/// How often the assignment is sampled in the background, for the whole run.
///
/// The three phase-boundary samples carry the narrative, but they cannot answer
/// the question the verdict rests on. A real run had a second executor hold 460
/// shards for its entire baseline and drop to zero nine seconds before the fault
/// landed; every labelled sample fell on one side of that transition or the
/// other, and the artifact showed a cluster that had never changed. A continuous
/// series makes a transition visible whichever side of it the phase boundaries
/// happen to land on.
const ASSIGNMENT_SAMPLE_INTERVAL: Duration = Duration::from_secs(20);

/// Runs S1 end to end.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let ownership_config = config.require_ownership()?;
    let history = OperationHistory::new(ScenarioCode::S1.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S1);

    let user = manifest.user_context(deps);
    let counters = user
        .get_latest_component_revision(&manifest.counters_component_id)
        .await?;
    let promise = user
        .get_latest_component_revision(&manifest.promise_component_id)
        .await?;

    let ctx = WorkloadContext {
        user,
        counters,
        promise,
        history: history.clone(),
        retry: config.retry_policy.clone(),
        phase: PhaseMarker::new(Phase::Baseline),
        key_prefix: key_prefix.clone(),
    };

    let scope = RunScope {
        environment_id: manifest.environment_id.0.to_string(),
        component_ids: vec![
            manifest.counters_component_id.0.to_string(),
            manifest.promise_component_id.0.to_string(),
        ],
        agent_id_prefix: key_prefix.clone(),
        idempotency_key_prefix: format!("{key_prefix}-"),
    };

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut ownership: Vec<OwnershipSample> = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut inconclusive: Option<String> = None;
    let mut attention_extra: Vec<String> = Vec::new();

    // Sample the assignment continuously for the whole run, alongside the
    // labelled phase-boundary samples. See ASSIGNMENT_SAMPLE_INTERVAL.
    let timeline = Arc::new(std::sync::Mutex::new(Vec::<OwnershipSample>::new()));
    let sampler_stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let timeline = timeline.clone();
        let stop = sampler_stop.clone();
        let deps = deps.clone();
        tokio::spawn(async move {
            let mut seq = 0u32;
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(ASSIGNMENT_SAMPLE_INTERVAL).await;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let routing = deps.shard_manager().get_routing_table().await.ok();
                let sample = OwnershipSample::from_routing(
                    &format!("t{seq:03}"),
                    routing.as_ref(),
                    None,
                    false,
                );
                seq += 1;
                if let Ok(mut timeline) = timeline.lock() {
                    timeline.push(sample);
                }
            }
        })
    };

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $exactly_once:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            )
            .with_ownership({
                // Labelled samples first, then the continuous series in time
                // order. The verdict reads the labelled ones by name, so the
                // extra rows cannot shift what it looks at.
                let mut all = ownership.clone();
                if let Ok(timeline) = timeline.lock() {
                    all.extend(timeline.iter().cloned());
                }
                all.sort_by_key(|s| s.taken_at);
                all
            });
            if let Some(detail) = inconclusive.clone() {
                summary.attention.push(detail);
            }
            summary.attention.extend(attention_extra.clone());
            if let Some(report) = $exactly_once {
                summary = summary.with_exactly_once(report);
            }
            let result = build_result(
                config,
                ScenarioOutcome {
                    started_at,
                    phases: phases.clone(),
                    fault_injected_at,
                    fault_recovered_at,
                    fault_id: fault_id.clone(),
                    fault_target_observed: fault_target_observed.clone(),
                    scope: scope.clone(),
                    summary,
                    termination_reason: $reason,
                    pinned_selection: None,
                },
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S1: baseline phase, running mixed workload for {:?}",
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = workload::start(ctx.clone(), workload_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    ownership.push(sample_ownership(deps, "before-fault", ownership.last(), false).await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    // A routing table the driver cannot read before the fault means the
    // shard-manager forward is broken, and every later sample would be empty
    // too. The workload can still run, so this is worth saying loudly rather
    // than failing — but a run whose context samples are all blank is much
    // harder to read.
    if ownership
        .last()
        .is_some_and(|s| s.unavailable_reason.is_some())
    {
        warn!(
            "S1: the shard-manager routing table could not be read before the fault; \
             the run will continue but its ownership context will be empty"
        );
    } else if let Some(sample) = ownership.last() {
        info!(
            "S1: baseline assignment covers {} executors, {} shards unassigned",
            sample.executors_with_shards(),
            sample.unassigned_shards
        );
    }

    // Refuse to inject a partition that cannot possibly be observed.
    //
    // The fault cuts the shard-manager off from `targetCount` executors chosen
    // by label. If the routing table lists no more executors than that, the one
    // picked either owns everything (and there is nowhere for its shards to go)
    // or owns nothing (and losing it changes nothing) — either way the run
    // produces a report that looks healthy and tested nothing.
    //
    // Executors owning zero shards are invisible here by construction: the
    // routing table is a shard→pod map, so an executor with no shards has no
    // entry. That is exactly the case worth catching, and it is what a real run
    // hit — two executor pods Ready, one holding all 1024 shards, the other
    // absent from the table and therefore a free target for the partition.
    let partitioned = config.fault.target_count.unwrap_or(1) as usize;
    if let Some(sample) = ownership.last()
        && sample.unavailable_reason.is_none()
        && sample.executors_with_shards() <= partitioned
    {
        warn!("S1: the configured partition cannot be observed, refusing to inject");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!(
                    "the routing table lists {} executor(s) holding shards, but the fault \
                     partitions {partitioned} of them — whichever is cut off, the assignment \
                     cannot change and the run would prove nothing. Executors owning zero \
                     shards do not appear in the routing table, so check that every \
                     worker-executor pod has actually registered and been assigned shards.",
                    sample.executors_with_shards()
                ),
            },
            &records,
            Vec::new(),
            None
        );
    }

    // ── Signal: ready for the fault ─────────────────────────────────────────
    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S1: baseline produced no confirmed operations, aborting before injection");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new(),
            None
        );
    }

    info!("S1: baseline complete ({baseline_operations} confirmed ops), signalling readiness");
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S1.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        // The partition is selected by label. Which executors end up on the far
        // side of it carries no information the run depends on — the claim is
        // about *any* partition healing cleanly.
        fault_target: None,
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S1: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S1: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    // Evidence, not verdict: while the link is cut the two sides are *supposed*
    // to disagree, and a sample that showed them agreeing would mean the
    // partition never took hold.
    //
    // Taken part-way into the fault window, not the instant it lands. The
    // shard-manager only reassigns once the cut-off executor has missed enough
    // health checks, so sampling immediately would show an unchanged table for
    // a reason that says nothing about the fault — and "unchanged" is exactly
    // the reading this sample exists to support.
    // S1's second traffic generator. The workflow runs an executor scale
    // schedule inside the fault window — down, then back up — which is what
    // puts shard-manager → executor calls (revoke, assign, register) on the
    // partitioned link. Picked up rather than awaited: it is traffic the
    // scenario induces, not a gate it waits on.
    let mut scale_events: Vec<ScaleEvent>;

    let observe_after = config
        .phases
        .fault()
        .mul_f64(DURING_FAULT_SAMPLE_FRACTION)
        .min(DURING_FAULT_SAMPLE_CAP);
    info!("S1: sampling assignment {observe_after:?} into the fault window");
    tokio::time::sleep(observe_after).await;
    // Read mid-window as well as after the heal: the steps land at different
    // points and logging them as they happen is what makes the assignment
    // samples readable afterwards.
    scale_events = signals.read_scale_events();
    for event in &scale_events {
        info!(
            "S1: workflow scaled executors {} -> {} at {}",
            event.from_replicas, event.to_replicas, event.scaled_at
        );
    }
    ownership.push(sample_ownership(deps, "during-fault", ownership.last(), false).await);

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S1: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S1: partition healed at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Settle, then judge ──────────────────────────────────────────────────
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));

    info!(
        "S1: letting shard assignment settle for {:?} before the judged sample",
        ownership_config.settle()
    );
    tokio::time::sleep(ownership_config.settle()).await;

    // Re-read: later steps in the schedule may have landed since.
    scale_events = signals.read_scale_events();
    let scale_events = scale_events;

    let settled = sample_ownership(deps, "after-settle", ownership.last(), true).await;
    info!(
        "S1: settled assignment covers {} executors, {} shards unassigned, {} findings",
        settled.executors_with_shards(),
        settled.unassigned_shards,
        settled.findings.len()
    );
    ownership.push(settled);

    // The workload keeps running through the rest of recovery whatever the
    // sampling showed: the verdict comes from the exactly-once probe at the
    // very end, and cutting the run short would throw away the operations it
    // needs to judge.
    let remaining = config
        .phases
        .recovery()
        .saturating_sub(ownership_config.settle());
    info!("S1: recovery phase, running for a further {remaining:?}");
    tokio::time::sleep(remaining).await;

    sampler_stop.store(true, Ordering::Relaxed);
    sampler.abort();
    handle.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);

    // ── Read-back ───────────────────────────────────────────────────────────
    info!("S1: letting the platform settle for {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();

    // Read-back before the probe pass: the probe can itself execute a key that
    // never ran, and comparing the two reads is what separates "replayed a
    // stored result" from "did the work".
    let readback = read_back(&ctx, &records, workload_config).await;
    let before_probe = read_counters(&ctx, &records).await;
    let probes = probe::probe_keys(&ctx, &records, Stream::Durable).await;
    let after_probe = read_counters(&ctx, &records).await;

    let exactly_once = ExactlyOnceReport::build(
        &records,
        &probes,
        Stream::Durable,
        &before_probe,
        &after_probe,
    );
    info!(
        "S1: exactly-once account — {} keys checked, {} with a final result, \
         {} recovered by the probe, {} findings",
        exactly_once.keys_checked,
        exactly_once.keys_with_final_result,
        exactly_once.keys_recovered_by_probe,
        exactly_once.findings.len()
    );

    // An assignment that never moved means the partition had no effect on
    // routing at all, and a clean verdict then says only that an undisturbed
    // cluster stayed consistent. Not a failure — the platform did nothing wrong
    // — but the operator has to be told the run is inconclusive, so it goes in
    // front of them rather than only into the artifact.
    let labelled = |at: &str| ownership.iter().find(|s| s.at == at).cloned();
    if let (Some(first), Some(during)) = (labelled("before-fault"), labelled("during-fault"))
        && during.assignment_matches(&first)
        // A transition anywhere in the continuous series means the cluster did
        // change, even if both labelled samples happened to fall on the same
        // side of it. Only claim the fault moved nothing when nothing moved at
        // any point.
        && timeline
            .lock()
            .map(|t| t.iter().all(|s| s.assignment_matches(&first)))
            .unwrap_or(false)
    {
        inconclusive = Some(format!(
            "the fault moved no shards: assignment was identical before and during it \
             ({} executor(s) holding shards). This run did not exercise recovery.",
            first.executors_with_shards()
        ));
    }

    // Did the cluster come back to the executor count it was scaled to, and
    // spread work across it? Reported, not asserted: "fair" is a judgement
    // about a rebalancer, and the driver is not in a position to make it.
    if let Some(final_step) = scale_events.last()
        && let Some(settled) = ownership.iter().find(|s| s.settled)
        && let Some(total) = settled.number_of_shards
    {
        let expected = final_step.to_replicas as usize;
        let executors = settled.executors_with_shards();
        let balanced = total / expected.max(1);
        let smallest = settled
            .shards_per_executor
            .values()
            .min()
            .copied()
            .unwrap_or(0);
        info!(
            "S1: after settling, {executors} of {expected} executors hold shards; smallest \
             holds {smallest} against a balanced {balanced}"
        );
        if executors < expected {
            attention_extra.push(format!(
                "executors were scaled back to {expected} during the fault, but only \
                 {executors} hold shards after settling — the cluster did not take the \
                 restored executor back"
            ));
        } else if smallest * 2 < balanced {
            attention_extra.push(format!(
                "after settling the least-loaded executor holds {smallest} shards against a \
                 balanced {balanced}: the cluster took the executor back but has not \
                 rebalanced onto it"
            ));
        }
    }

    // A key that executed twice is the observable consequence of two executors
    // owning one shard, and it is the only thing this scenario asserts.
    let reason = exactly_once_termination(&exactly_once).unwrap_or_else(|| {
        if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
            TerminationReason::StreamNeverSucceeded {
                stream: Stream::Durable.to_string(),
            }
        } else {
            TerminationReason::Completed
        }
    });

    finish!(reason, &records, readback, Some(exactly_once));
}

/// Reads the shard-manager's assignment, relative to the previous sample.
///
/// Failure is recorded, not propagated: an unreachable shard-manager is an
/// observation, and during a fault aimed at its links the expected one.
async fn sample_ownership(
    deps: &BenchmarkTestDependencies,
    at: &str,
    previous: Option<&OwnershipSample>,
    settled: bool,
) -> OwnershipSample {
    let routing = deps.shard_manager().get_routing_table().await.ok();
    OwnershipSample::from_routing(at, routing.as_ref(), previous, settled)
}

/// Turns the exactly-once account into a termination reason, if it found
/// something.
///
/// This is where S1's single assertion lives. Two executors owning one shard is
/// not observable from outside the cluster, but a key that executed twice is,
/// and it is the harm that ownership overlap would cause. See
/// [`crate::chaos::ownership`] for why the routing table cannot answer this.
fn exactly_once_termination(report: &ExactlyOnceReport) -> Option<TerminationReason> {
    if !report.has_violations() {
        return None;
    }
    Some(TerminationReason::ShardOwnershipViolated {
        findings: report.findings.len() as u64,
        first: report
            .findings
            .first()
            .map(|f| format!("{} on key {}", f.violation, f.idempotency_key))
            .unwrap_or_default(),
    })
}

/// Current counter of every durable agent the run touched.
///
/// Scoped to the agents in the history rather than the whole configured pool:
/// an agent the run never drove has nothing to say about it.
///
/// Concurrent and bounded, for the same reason [`super::read_back_agents`] is,
/// and this function learned it the expensive way: it walked 200 agents one at
/// a time behind a 30s ceiling, so a run where every agent had stopped
/// answering spent 100 minutes here producing nothing but timeouts. Reads do
/// not mutate, so batching them changes none of the numbers.
async fn read_counters(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
) -> std::collections::BTreeMap<String, u64> {
    let agents: Vec<String> = records
        .iter()
        .filter(|r| r.stream == Stream::Durable)
        .map(|r| r.agent.clone())
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect();

    let total = agents.len();
    let mut values = std::collections::BTreeMap::new();
    let mut done = 0usize;
    let mut next_report = super::READ_PROGRESS_EVERY;

    for chunk in agents.chunks(super::READ_CONCURRENCY) {
        let mut batch = tokio::task::JoinSet::new();
        for agent in chunk.iter().cloned() {
            let ctx = ctx.clone();
            batch.spawn(async move {
                let value = workload::read_counter(&ctx, &agent).await;
                (agent, value)
            });
        }

        while let Some(joined) = batch.join_next().await {
            match joined {
                Ok((agent, Ok(value))) => {
                    values.insert(agent, value);
                }
                Ok((agent, Err(e))) => {
                    warn!("S1: could not read durable agent {agent}: {e}")
                }
                Err(e) => warn!("S1: a durable read-back task panicked: {e}"),
            }
        }

        done += chunk.len();
        if done >= next_report {
            info!("S1: read {done} of {total} durable counters");
            next_report += super::READ_PROGRESS_EVERY;
        }
    }
    values
}

/// Read-back for every stream that keeps a durable count.
///
/// Concurrent and individually bounded: a fault can leave an agent that never
/// answers — a quota lease lost mid-run parks the agent's next reservation with
/// no timeout on the platform side — and walking 300 agents sequentially behind
/// a 30s ceiling would outlast the maintenance window several times over.
async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    config: &crate::chaos::WorkloadConfig,
) -> Vec<AgentReadback> {
    let mut agents = Vec::new();
    for index in 0..config.durable_agents {
        agents.push((
            Stream::Durable,
            ctx.agent_name(Stream::Durable, index),
            ReadKind::Counter,
        ));
    }
    // The quota stream is read on two numbers, not one: what committed and what
    // the platform refused. A refusal is the observable cost of a lease the
    // executor could no longer renew, and it is invisible in the counter alone.
    for index in 0..config.quota_agents {
        agents.push((
            Stream::Quota,
            ctx.agent_name(Stream::Quota, index),
            ReadKind::QuotaCounter,
        ));
    }
    for index in 0..config.scheduled_agents {
        agents.push((
            Stream::Scheduled,
            ctx.schedule_target_name(index),
            ReadKind::Polls,
        ));
    }

    read_back_agents(ctx, records, agents).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::errors::ErrorClass;
    use crate::chaos::history::{AttemptRecord, Outcome};
    use crate::chaos::probe::KeyProbe;
    use crate::chaos::summary::ExactlyOnceViolation;
    use std::collections::BTreeMap;
    use test_r::test;

    fn record(op_id: u64, agent: &str, outcome: Outcome, value: Option<u32>) -> OperationRecord {
        OperationRecord {
            op_id,
            stream: Stream::Durable,
            phase: Phase::Fault,
            agent: agent.to_string(),
            method: "increment".to_string(),
            idempotency_key: format!("{agent}-{op_id:08}"),
            submitted_at: Utc::now(),
            completed_at: Some(Utc::now()),
            attempts: 1,
            outcome,
            duration_ms: 12,
            returned_value: value,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: vec![AttemptRecord {
                attempt: 1,
                started_at: Utc::now(),
                duration_ms: 12,
                returned_value: value,
                succeeded: outcome == Outcome::Confirmed,
                error_class: None,
                error: None,
            }],
        }
    }

    fn probe_of(record: &OperationRecord, final_value: Option<u32>) -> KeyProbe {
        KeyProbe {
            idempotency_key: record.idempotency_key.clone(),
            agent: record.agent.clone(),
            final_value,
            error: final_value.is_none().then(|| "refused".to_string()),
            error_class: final_value.is_none().then_some(ErrorClass::Response),
        }
    }

    /// A clean heal: every key replays the value the driver was given.
    #[test]
    fn a_clean_recovery_produces_no_termination_reason() {
        let r = record(0, "chaos-s1-durable-0000", Outcome::Confirmed, Some(4));
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe_of(&r, Some(4))],
            Stream::Durable,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(exactly_once_termination(&report).is_none());
    }

    /// The one assertion S1 makes. Two executors owning one shard is not
    /// visible from outside the cluster; one key executing twice is, and it is
    /// the harm that overlap causes.
    #[test]
    fn a_key_that_executed_twice_terminates_the_run() {
        let r = record(1, "chaos-s1-durable-0001", Outcome::Confirmed, Some(4));
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe_of(&r, Some(9))],
            Stream::Durable,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        match exactly_once_termination(&report) {
            Some(TerminationReason::ShardOwnershipViolated { findings, first }) => {
                assert_eq!(findings, 1);
                assert!(
                    first.contains(&ExactlyOnceViolation::MultipleDistinctCompletions.to_string()),
                    "{first}"
                );
            }
            other => panic!("expected a shard-ownership violation, got {other:?}"),
        }
    }

    /// The precondition that would have caught the real inconclusive run: a
    /// partition of 1 of N executors proves nothing when the routing table
    /// lists only one executor holding shards.
    #[test]
    fn a_partition_that_cannot_move_anything_is_refused() {
        let one_executor = OwnershipSample {
            at: "before-fault".to_string(),
            taken_at: Utc::now(),
            number_of_shards: Some(1024),
            shards_per_executor: BTreeMap::from([("172.17.217.159:9093".to_string(), 1024usize)]),
            unassigned_shards: 0,
            unavailable_reason: None,
            settled: false,
            findings: Vec::new(),
        };
        assert_eq!(one_executor.executors_with_shards(), 1);
        assert!(
            one_executor.executors_with_shards() <= 1,
            "partitioning 1 of 1 executors cannot change the assignment"
        );

        let two = OwnershipSample {
            shards_per_executor: BTreeMap::from([
                ("a:9093".to_string(), 512usize),
                ("b:9093".to_string(), 512usize),
            ]),
            ..one_executor.clone()
        };
        assert!(two.executors_with_shards() > 1, "two is enough to observe");
    }

    /// An assignment identical before and during the fault means the partition
    /// landed somewhere it could not matter.
    #[test]
    fn an_unchanged_assignment_across_the_fault_is_detected() {
        let before = OwnershipSample {
            at: "before-fault".to_string(),
            taken_at: Utc::now(),
            number_of_shards: Some(1024),
            shards_per_executor: BTreeMap::from([("a:9093".to_string(), 1024usize)]),
            unassigned_shards: 0,
            unavailable_reason: None,
            settled: false,
            findings: Vec::new(),
        };
        let during = OwnershipSample {
            at: "during-fault".to_string(),
            ..before.clone()
        };
        assert!(during.assignment_matches(&before));

        let moved = OwnershipSample {
            shards_per_executor: BTreeMap::from([
                ("a:9093".to_string(), 512usize),
                ("b:9093".to_string(), 512usize),
            ]),
            ..during.clone()
        };
        assert!(!moved.assignment_matches(&before));
    }

    /// A sample that could not be taken must not read as "unchanged" — that
    /// would turn an unreachable shard-manager into a false all-clear.
    #[test]
    fn an_unavailable_sample_never_counts_as_unchanged() {
        let before = OwnershipSample::from_routing("before-fault", None, None, false);
        let during = OwnershipSample::from_routing("during-fault", None, None, false);
        assert!(!during.assignment_matches(&before));
    }

    /// A probe that timed out means the driver could not ask, which says
    /// nothing about whether the platform holds the result.
    ///
    /// This is the exact shape a wedged quota agent produces: the platform
    /// parks the reservation with no timeout, the driver's own ceiling fires,
    /// and the key must land as inconclusive. Reporting it as lost work would
    /// turn a platform hang into a false correctness finding — and it is the
    /// finding an operator is most likely to act on.
    #[test]
    fn a_probe_that_timed_out_is_inconclusive_not_lost_work() {
        let r = record(5, "chaos-s1-durable-0005", Outcome::Indeterminate, None);
        let timed_out = KeyProbe {
            idempotency_key: r.idempotency_key.clone(),
            agent: r.agent.clone(),
            final_value: None,
            error: Some("probe timed out after 30s".to_string()),
            // What `errors::classify` yields for a timeout: unreadable, so the
            // band of doubt widens rather than a refusal being invented.
            error_class: Some(ErrorClass::Transport),
        };

        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[timed_out],
            Stream::Durable,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(
            !report.has_violations(),
            "a driver-side timeout must not fail the run: {:?}",
            report.findings
        );
        assert_eq!(report.keys_inconclusive, 1);
    }

    /// Routing-table observations are context, never the verdict — the table
    /// cannot show overlap at all, so it must not be able to fail a run.
    #[test]
    fn routing_table_findings_do_not_terminate_the_run() {
        let before = OwnershipSample::from_routing("before-fault", None, None, false);
        let settled = OwnershipSample::from_routing("after-settle", None, Some(&before), true);
        let summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), None)
            .with_ownership(vec![before, settled]);

        assert_eq!(summary.ownership.len(), 2);
        let clean = ExactlyOnceReport::build(
            &[],
            &[],
            Stream::Durable,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(
            exactly_once_termination(&clean).is_none(),
            "only the exactly-once probe decides"
        );
    }

    /// A violation is a statement about one key; the findings still have to
    /// reach the operator alongside it.
    #[test]
    fn a_violation_reaches_the_attention_list() {
        let r = record(2, "chaos-s1-durable-0002", Outcome::Confirmed, Some(4));
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe_of(&r, Some(9))],
            Stream::Durable,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let summary =
            ChaosSummary::build(&[], Vec::new(), Vec::new(), None).with_exactly_once(report);
        assert!(
            summary
                .attention
                .iter()
                .any(|l| l.contains("multiple-distinct-completions")),
            "{:?}",
            summary.attention
        );
    }
}
