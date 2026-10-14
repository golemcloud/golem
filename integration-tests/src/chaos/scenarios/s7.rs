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

//! S7 — executor pod kill during agent state revert (GOL-371).
//!
//! Every other scenario in this suite disturbs work that is trying to happen.
//! S7 disturbs work that is trying to be **undone**: each agent builds its
//! counter up with a run of increments and then asks the platform to take some
//! of them back, over and over, while an executor is killed underneath.
//!
//! ### Why this one can assert
//!
//! Read-backs elsewhere compare a counter against a range, and the width of the
//! range is the operations whose fate the driver could not determine. Here
//! there is no range. The last increment of a round *returns* the counter's
//! value, so the driver knows exactly what the agent was worth immediately
//! before the revert, and it asked for an exact number of invocations back. So
//! afterwards there are two legal values, `V` and `V - N`, and nothing between
//! them. See [`crate::chaos::truncation`] for what each other answer means.
//!
//! That is why S7 is one of the few scenarios whose read-back can fail the run
//! outright rather than being reported for a human to weigh.
//!
//! ### What is actually being killed into
//!
//! The truncation itself cannot tear: `RevertLastInvocations` commits a single
//! `OplogEntry::revert` marking a region deleted. The window worth aiming at is
//! the one around it — reverting takes `lock_stopped_worker`, so the worker is
//! stopped, the entry is committed, and only then is the worker status
//! reattached. An executor that dies between the commit and the reattach has
//! changed durable state and lost the thing that tells anyone about it.
//!
//! ### The choreography
//!
//! Like S8, S10, S11 and S3 the driver names the pod: it picks the executor
//! owning the largest share of its agents and keeps driving the rest as a
//! control group. Unlike them, the last thing it does before read-back is a
//! single read per agent, which answers the one round per agent that no
//! following increment ever probed.

use crate::chaos::history::{OperationHistory, Outcome, Phase, Stream};
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::reverts::{self, RevertRound};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, WARMUP_SETTLE, build_result, sample_ownership,
    signal_termination, snapshot_routing, wait_for_settled_routing, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals, FaultTarget};
use crate::chaos::split::{self, FaultWindow, PodSplit};
use crate::chaos::summary::{ChaosSummary, Note, TerminationReason};
use crate::chaos::truncation::TruncationReport;
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait after stopping the workload before the final read.
///
/// Shorter than the other scenarios' settle: nothing here is queued or
/// scheduled, and `stop` already waits for every operation in flight to record
/// itself. This only covers a worker still coming back from the last revert.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(20);

/// How far into the fault window the assignment is sampled.
const DURING_FAULT_SAMPLE_FRACTION: f64 = 0.6;
const DURING_FAULT_SAMPLE_CAP: Duration = Duration::from_secs(120);

/// Runs S7 end to end.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let revert_config = config.require_revert()?;
    let history = OperationHistory::new(ScenarioCode::S7.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S7);

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

    let agents = reverts::agent_names(&ctx, revert_config.agents);

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
        ($reason:expr, $records:expr, $truncation:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                Vec::new(),
                routing_snapshots.clone(),
                fault_injected_at,
            )
            .with_ownership(ownership.clone());
            summary.absorb(attention_extra.clone());
            if let Some(report) = $truncation {
                summary = summary.with_truncation(report);
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
                    isolation_selection: None,
                    revert_selection: selection.clone(),
                    delete_selection: None,
                },
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Warm-up ─────────────────────────────────────────────────────────────
    //
    // Constructing an agent is itself recorded in its oplog, so doing it inside
    // a measured round would put an entry between the increments and the revert
    // that counts them. Reads here rather than increments, for the same reason
    // every other scenario warms with reads: an increment would be invisible to
    // the round arithmetic the whole oracle rests on.
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);

    info!("S7: warming up {} revert agents", agents.len());
    let mut warmed = 0usize;
    for agent in &agents {
        if workload::read_counter(&ctx, agent).await.is_ok() {
            warmed += 1;
        }
    }
    info!(
        "S7: warmed {warmed} of {} agents, settling {WARMUP_SETTLE:?}",
        agents.len()
    );
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Aim ─────────────────────────────────────────────────────────────────
    let subject = split::revert_subject(&ctx);
    let split = match split::select(subject, deps, &agents).await {
        Ok(split) => split,
        Err(e) => {
            warn!("S7: cannot aim the kill: {e:#}");
            let records = history.snapshot();
            finish!(
                TerminationReason::FaultTargetUnverified {
                    detail: format!("{e:#}"),
                },
                &records,
                None
            );
        }
    };
    selection = Some(split.clone());

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S7: baseline phase, running {} revert emitters for {:?}",
        agents.len(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = reverts::start(ctx.clone(), revert_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    ownership.push(sample_ownership(deps, "before-fault", ownership.last(), false).await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S7: baseline produced no confirmed operations, aborting before injection");
        let rounds = handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Some(build_truncation(&rounds, &split, None, revert_config))
        );
    }

    if let Err(e) = split::verify_ownership(subject, deps, &split).await {
        warn!("S7: ownership drifted between selection and injection: {e:#}");
        let rounds = handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!("{e:#}"),
            },
            &records,
            Some(build_truncation(&rounds, &split, None, revert_config))
        );
    }

    info!(
        "S7: baseline complete ({baseline_operations} confirmed ops, {} rounds), naming {} and \
         signalling readiness",
        handle.rounds().len(),
        split.pod_address
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S7.as_str().to_string(),
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
            warn!("S7: no fault-injected signal arrived: {e}");
            let rounds = handle.stop().await;
            let records = history.snapshot();
            finish!(
                signal_termination(&e),
                &records,
                Some(build_truncation(&rounds, &split, None, revert_config))
            );
        }
    };
    info!(
        "S7: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let observe_after = config
        .phases
        .fault()
        .mul_f64(DURING_FAULT_SAMPLE_FRACTION)
        .min(DURING_FAULT_SAMPLE_CAP);
    tokio::time::sleep(observe_after).await;
    ownership.push(sample_ownership(deps, "during-fault", ownership.last(), false).await);

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S7: no fault-recovered signal arrived: {e}");
            let rounds = handle.stop().await;
            let records = history.snapshot();
            finish!(
                signal_termination(&e),
                &records,
                Some(build_truncation(
                    &rounds,
                    &split,
                    fault_window(fault_injected_at, None),
                    revert_config
                ))
            );
        }
    };
    info!(
        "S7: executor back at {} ({})",
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
        "S7: recovery phase, running for {:?}",
        config.phases.recovery()
    );
    tokio::time::sleep(config.phases.recovery()).await;

    let mut rounds = handle.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);
    ownership.push(sample_ownership(deps, "after-recovery", ownership.last(), true).await);

    // ── Read-back ───────────────────────────────────────────────────────────
    info!("S7: settling {SETTLE_BEFORE_READBACK:?} before the final read");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    // The one read per agent, which answers the last round it ran. Every other
    // round was probed by the increment that followed it; this is the only one
    // that has nothing after it.
    close_last_rounds(&ctx, &agents, &mut rounds).await;

    let records = history.snapshot();
    let truncation = build_truncation(
        &rounds,
        &split,
        fault_window(fault_injected_at, fault_recovered_at),
        revert_config,
    );
    info!(
        "S7: truncation account — {} rounds, {} applied exactly, {} findings",
        truncation.rounds_recorded,
        truncation.applied_exactly,
        truncation.findings.len()
    );

    let reason = if truncation.has_violations() {
        let first = truncation
            .findings
            .first()
            .map(|f| format!("{} round {}: {}", f.agent, f.round, f.detail))
            .unwrap_or_default();
        TerminationReason::RevertTruncationViolated {
            findings: truncation.findings.len() as u64 + truncation.findings_omitted,
            first,
        }
    } else if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
        TerminationReason::StreamNeverSucceeded {
            stream: Stream::Revert.to_string(),
        }
    } else {
        TerminationReason::Completed
    };

    finish!(reason, &records, Some(truncation));
}

fn fault_window(
    injected_at: Option<chrono::DateTime<Utc>>,
    recovered_at: Option<chrono::DateTime<Utc>>,
) -> Option<FaultWindow> {
    injected_at.map(|injected_at| FaultWindow {
        injected_at,
        recovered_at,
    })
}

fn build_truncation(
    rounds: &[RevertRound],
    split: &PodSplit,
    fault: Option<FaultWindow>,
    config: &crate::chaos::RevertConfig,
) -> TruncationReport {
    TruncationReport::build(
        rounds,
        split,
        fault,
        config.increments_per_round,
        config.revert_invocations,
    )
}

/// Reads each agent once and uses the value to judge the last round it ran.
///
/// A read is an invocation and would shift what "the last N invocations" means
/// for any revert after it — which is exactly why the workload never reads
/// mid-round. Here there is nothing after it, so it is safe, and it recovers a
/// round per agent that would otherwise be unjudgeable.
async fn close_last_rounds(ctx: &WorkloadContext, agents: &[String], rounds: &mut [RevertRound]) {
    let mut last_of: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for (index, round) in rounds.iter().enumerate() {
        if round.observed_after.is_none() {
            let slot = last_of.entry(round.agent.as_str()).or_insert(index);
            if rounds[*slot].round < round.round {
                *slot = index;
            }
        }
    }
    let pending: Vec<(String, usize)> = last_of
        .into_iter()
        .map(|(agent, index)| (agent.to_string(), index))
        .collect();

    info!(
        "S7: closing {} unprobed rounds with a final read",
        pending.len()
    );
    for (agent, index) in pending {
        if !agents.iter().any(|a| a == &agent) {
            continue;
        }
        match workload::read_counter(ctx, &agent).await {
            Ok(value) => rounds[index].observed_after = Some(value),
            Err(e) => warn!("S7: could not read {agent} to close its last round: {e}"),
        }
    }
}
