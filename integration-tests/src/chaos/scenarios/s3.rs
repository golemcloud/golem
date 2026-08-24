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

//! S3 — executor / worker-service network partition (GOL-370).
//!
//! S1 cuts the shard-manager off from an executor, and the cluster reacts: the
//! shard-manager stops hearing health checks and moves the shards. S3 cuts the
//! *other* link, and the cluster does not react at all — which is the point.
//!
//! The executor keeps talking to the shard-manager, so it keeps its shards and
//! stays in the routing table. worker-service therefore keeps being told, quite
//! correctly, that this executor owns those agents, and keeps trying to reach a
//! pod it cannot reach. There is no route around it, because as far as the
//! platform is concerned nothing is wrong. Every other scenario in the suite
//! ends with the platform recovering; this one asks what it does when there is
//! nothing to recover *from*, only something to wait out.
//!
//! ### What worker-service actually does
//!
//! Read out of `golem-worker-service/src/service/worker/routing_logic.rs`. A
//! call to an unreachable pod fails the 10s connect timeout, which is retriable,
//! so `call_worker_executor` invalidates the routing table and tries again —
//! forever. `get_delay` stops extending the backoff after five attempts and the
//! loop then settles at the 3s ceiling rather than giving up. The freshly
//! fetched table names the same unreachable pod every time, because the
//! shard-manager still believes in it.
//!
//! Two consequences, and S3 measures both:
//!
//! * **For the agents on that executor**, an invocation does not fail fast. It
//!   hangs, until the caller's own attempt timeout ends it. That is the
//!   "acceptance degradation with pending and timeout behaviour" the ticket
//!   asks to see, and it is visible in the history as attempts that timed out
//!   rather than as refusals.
//! * **For everyone else**, the routing table is one process-wide cache entry
//!   per worker-service replica. Every stalled caller invalidating it costs
//!   *every* caller a shard-manager round trip. `invalidation_min_delay` bounds
//!   that to twice a second, so the cost should be small — but "should be" is
//!   what the control group is there to check.
//!
//! ### The choreography
//!
//! 1. **Warm up** every agent, so the partition lands on a live population.
//! 2. **Select** the executor owning the largest share of them, exactly as S10
//!    and S11 pick their target, and name it for the workflow.
//! 3. **Baseline** — one emitter per agent, one operation each at a time.
//! 4. **Fault** — keep driving. Sample the routing table part-way in: S3's
//!    premise is that the assignment does *not* move, and a run where it did is
//!    a different experiment that has to be read differently.
//! 5. **Heal**, then keep driving so the isolated agents visibly come back.
//! 6. **Read back and probe** — the same completion and exactly-once oracles
//!    every other scenario ends with.

use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::probe;
use crate::chaos::reachability::ReachabilityReport;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ReadKind, ScenarioOutcome, WARMUP_SETTLE, build_result, exactly_once_termination,
    read_back_agents, read_counters, sample_ownership, signal_termination, snapshot_routing,
    wait_for_settled_routing, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals, FaultTarget};
use crate::chaos::split::{self, FaultWindow, PodSplit};
use crate::chaos::steady;
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, ExactlyOnceReport, Note, TerminationReason,
};
use crate::chaos::workload::{PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait after stopping the workload before reading durable state.
/// Same reasoning as every other scenario: an increment still in flight has to
/// land, and reading early reports a mismatch that says nothing.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(30);

/// How far into the fault window the assignment sample is taken, as a fraction
/// of it, and the ceiling on that.
///
/// Unlike S1 this sample is checking that nothing moved, so it is taken late
/// rather than early: a table that still looks untouched two minutes in is a
/// much stronger statement than one that looks untouched immediately.
const DURING_FAULT_SAMPLE_FRACTION: f64 = 0.75;
const DURING_FAULT_SAMPLE_CAP: Duration = Duration::from_secs(150);

/// Runs S3 end to end.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let isolation = config.require_isolation()?;
    let history = OperationHistory::new(ScenarioCode::S3.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S3);

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
        component_ids: vec![manifest.counters_component_id.0.to_string()],
        agent_id_prefix: key_prefix.clone(),
        idempotency_key_prefix: format!("{key_prefix}-"),
    };

    let agents = steady::agent_names(&ctx, isolation.agents);

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut ownership: Vec<OwnershipSample> = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut selection: Option<PodSplit> = None;
    let mut attention_extra: Vec<Note> = Vec::new();

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $exactly_once:expr, $reachability:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            )
            .with_ownership(ownership.clone());
            summary.absorb(attention_extra.clone());
            if let Some(report) = $exactly_once {
                summary = summary.with_exactly_once(report);
            }
            if let Some(report) = $reachability {
                summary = summary.with_reachability(report);
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
                    scheduled_selection: None,
                    promise_selection: None,
                    isolation_selection: selection.clone(),
                    revert_selection: None,
                },
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Warm-up ─────────────────────────────────────────────────────────────
    //
    // Construct every agent before measuring, for the reason S1 spells out: a
    // cold start looks exactly like a stall from outside, and this scenario's
    // entire signal is a comparison of throughput before and during the fault.
    // A baseline that was still cold-starting would understate itself and make
    // the fault look milder than it was.
    //
    // Reads, not increments. An increment here would be invisible to the
    // operation history and would leave every read-back off by one.
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);

    info!("S3: warming up {} counter agents", agents.len());
    let warm: Vec<(Stream, String, ReadKind)> = agents
        .iter()
        .map(|agent| (Stream::Durable, agent.clone(), ReadKind::Counter))
        .collect();
    let _ = read_back_agents(&ctx, &[], warm).await;
    info!(
        "S3: warmed {} agents, settling {WARMUP_SETTLE:?}",
        agents.len()
    );
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Aim ─────────────────────────────────────────────────────────────────
    //
    // Chaos Mesh's `mode: one` would pick an executor at random, and a
    // partition that cut off the executor owning six agents out of two hundred
    // would still produce a confident-looking report. The driver names the pod
    // instead; the workflow turns the IP into a pod name.
    let subject = split::counter_subject(&ctx);
    let split = match split::select(subject, deps, &agents).await {
        Ok(split) => split,
        Err(e) => {
            warn!("S3: cannot aim the partition: {e:#}");
            let records = history.snapshot();
            finish!(
                TerminationReason::FaultTargetUnverified {
                    detail: format!("{e:#}"),
                },
                &records,
                Vec::new(),
                None,
                None
            );
        }
    };
    selection = Some(split.clone());

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S3: baseline phase, running {} emitters for {:?}",
        agents.len(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = steady::start(ctx.clone(), isolation.agents, isolation.interval());
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    ownership.push(sample_ownership(deps, "before-fault", ownership.last(), false).await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S3: baseline produced no confirmed operations, aborting before injection");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new(),
            None,
            None
        );
    }

    // A rebalance between selection and injection would leave the run naming
    // the control group as the affected one and vice versa — a report that is
    // not merely wrong but confidently inverted.
    if let Err(e) = split::verify_ownership(subject, deps, &split).await {
        warn!("S3: ownership drifted between selection and injection: {e:#}");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!("{e:#}"),
            },
            &records,
            Vec::new(),
            None,
            None
        );
    }

    info!(
        "S3: baseline complete ({baseline_operations} confirmed ops), naming {} and signalling \
         readiness",
        split.pod_address
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S3.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        fault_target: Some(FaultTarget {
            pod_address: split.pod_address.clone(),
            pod_ip: split.pod_ip.clone(),
            owned_agents: split.on_pod.clone(),
        }),
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S3: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None, None);
        }
    };
    info!(
        "S3: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    // Evidence for the premise, not a verdict. The partitioned link is not the
    // one the shard-manager uses, so the assignment is expected to be identical
    // to the baseline. If it moved, the fault was wider than intended and every
    // reading below has a second explanation.
    let observe_after = config
        .phases
        .fault()
        .mul_f64(DURING_FAULT_SAMPLE_FRACTION)
        .min(DURING_FAULT_SAMPLE_CAP);
    info!("S3: sampling assignment {observe_after:?} into the fault window");
    tokio::time::sleep(observe_after).await;
    ownership.push(sample_ownership(deps, "during-fault", ownership.last(), false).await);

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S3: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None, None);
        }
    };
    info!(
        "S3: partition healed at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    info!(
        "S3: recovery phase, running for {:?}",
        config.phases.recovery()
    );
    tokio::time::sleep(config.phases.recovery()).await;

    handle.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);
    ownership.push(sample_ownership(deps, "after-recovery", ownership.last(), true).await);

    // ── Read-back ───────────────────────────────────────────────────────────
    info!("S3: letting the platform settle for {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();

    let readback = read_back(&ctx, &records, &agents).await;
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
        "S3: exactly-once account — {} keys checked, {} with a final result, {} recovered by the \
         probe, {} findings",
        exactly_once.keys_checked,
        exactly_once.keys_with_final_result,
        exactly_once.keys_recovered_by_probe,
        exactly_once.findings.len()
    );

    let reachability = ReachabilityReport::build(
        &records,
        &split,
        fault_injected_at.map(|injected_at| FaultWindow {
            injected_at,
            recovered_at: fault_recovered_at,
        }),
        isolation.isolated_ceiling_percent,
        isolation.control_floor_percent,
        isolation.recovery_budget(),
    );
    info!(
        "S3: reachability account — {} findings, {} isolated agents never recovered",
        reachability.findings.len(),
        reachability.agents_never_recovered.len()
    );

    // The assertion is the same one S1 makes, and for the same reason: a key
    // that executed twice is the only harm visible from outside the cluster.
    // Everything the reachability report says is reported for a human to judge,
    // because "worker-service waited rather than failing fast" is a design
    // question and not a defect the driver is entitled to rule on.
    let reason = exactly_once_termination(&exactly_once).unwrap_or_else(|| {
        if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
            TerminationReason::StreamNeverSucceeded {
                stream: Stream::Durable.to_string(),
            }
        } else {
            TerminationReason::Completed
        }
    });

    finish!(
        reason,
        &records,
        readback,
        Some(exactly_once),
        Some(reachability)
    );
}

async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    agents: &[String],
) -> Vec<AgentReadback> {
    let targets = agents
        .iter()
        .map(|agent| (Stream::Durable, agent.clone(), ReadKind::Counter))
        .collect();
    read_back_agents(ctx, records, targets).await
}
