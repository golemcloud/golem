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

//! S6 — executor pod kill during agent deletion (GOL-372).
//!
//! S7 asks the platform to forget some of an agent's work. S6 asks it to forget
//! the agent, and then kills the executor while it is doing so.
//!
//! ### Why this one can assert
//!
//! Invoking a deleted agent id creates a **new** agent, and a new counter starts
//! from nothing. So a round — increments to a known `V`, then a delete — has
//! exactly two legal answers when the slot is next used:
//!
//! * `1`, a fresh agent, meaning the deletion took
//! * `V + 1`, meaning the old agent is still there
//!
//! Neither is a defect on its own; a delete whose response was lost leaves the
//! question genuinely open. What makes one a defect is the platform's own answer
//! beside it. Confirmed, and the agent is still worth `V`, is the resurrection
//! this scenario is named for. See [`crate::chaos::resurrection`].
//!
//! ### What is actually being killed into
//!
//! Deleting is four steps in `delete_worker_internal`: interrupt the running
//! worker, `start_deleting`, remove it from the worker service, remove it from
//! the active set. Only the third is durable.
//!
//! The interesting part is that the happy path is **already defended**.
//! `Worker::start_deleting` stops the background status flush and the
//! checkpointer first, specifically so neither can — in the executor's own
//! comment — "resurrect the cached status" after the removal. So the question
//! S6 asks is not whether anyone thought about resurrection, but whether that
//! defence survives the pod dying between the mark and the removal, when
//! whoever picks up the shard next has to decide what a worker marked for
//! deletion but never removed means.
//!
//! ### A smoke round before anything else
//!
//! The whole account rests on "a deleted id comes back as a new agent". If that
//! is not true, every round in the run reports a resurrection and the report is
//! worthless. So one throwaway agent is built, deleted and re-invoked before the
//! baseline starts, and a run whose premise is wrong aborts in seconds instead
//! of spending the maintenance window discovering it. That lesson is S11's.
//!
//! ### The choreography
//!
//! Like S8, S10, S11, S3 and S7 the driver names the pod: it picks the executor
//! owning the largest share of its agent slots and keeps driving the rest as a
//! control group. As in S7 the last thing before read-back is one read per slot,
//! which answers the round no following increment ever probed.

use crate::chaos::deletions::{self, DeleteRound};
use crate::chaos::history::{OperationHistory, Outcome, Phase, Stream};
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::resurrection::ResurrectionReport;
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, WARMUP_SETTLE, build_result, sample_ownership,
    signal_termination, snapshot_routing, wait_for_settled_routing, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals, FaultTarget};
use crate::chaos::split::{self, FaultWindow, PodSplit};
use crate::chaos::summary::{ChaosSummary, Note, TerminationReason};
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
/// itself. This only covers a slot still coming back from the last delete.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(20);

/// How far into the fault window the assignment is sampled.
const DURING_FAULT_SAMPLE_FRACTION: f64 = 0.6;
const DURING_FAULT_SAMPLE_CAP: Duration = Duration::from_secs(120);

/// Runs S6 end to end.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let delete_config = config.require_delete()?;
    let history = OperationHistory::new(ScenarioCode::S6.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S6);

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

    let agents = deletions::agent_names(&ctx, delete_config.agents);

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
        ($reason:expr, $records:expr, $resurrection:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                Vec::new(),
                routing_snapshots.clone(),
                fault_injected_at,
            )
            .with_ownership(ownership.clone());
            summary.absorb(attention_extra.clone());
            if let Some(report) = $resurrection {
                summary = summary.with_resurrection(report);
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
                    revert_selection: None,
                    delete_selection: selection.clone(),
                },
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Warm-up ─────────────────────────────────────────────────────────────
    //
    // Constructing an agent is itself recorded in its oplog, so doing it inside
    // a measured round would leave a slot holding an agent the round did not
    // build. Reads here rather than increments, for the same reason every other
    // scenario warms with reads: an increment would be invisible to the round
    // arithmetic the whole oracle rests on.
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);

    info!("S6: warming up {} delete agents", agents.len());
    let mut warmed = 0usize;
    for agent in &agents {
        if workload::read_counter(&ctx, agent).await.is_ok() {
            warmed += 1;
        }
    }
    info!(
        "S6: warmed {warmed} of {} agents, settling {WARMUP_SETTLE:?}",
        agents.len()
    );
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Smoke round ─────────────────────────────────────────────────────────
    //
    // Everything below assumes a deleted id comes back as a new agent. If that
    // is not true, every round reports a resurrection and the artifact is
    // worthless — so one throwaway agent proves it before the baseline starts.
    // The first S11 run is why this is here: a wrong premise otherwise costs
    // the whole maintenance window before the numbers say so.
    if let Err(e) = deletions::smoke_round(&ctx, delete_config).await {
        warn!("S6: smoke round failed, aborting before the baseline: {e:#}");
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: format!("{e:#}"),
            },
            &records,
            None
        );
    }

    // ── Aim ─────────────────────────────────────────────────────────────────
    let subject = split::delete_subject(&ctx);
    let split = match split::select(subject, deps, &agents).await {
        Ok(split) => split,
        Err(e) => {
            warn!("S6: cannot aim the kill: {e:#}");
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
        "S6: baseline phase, running {} delete emitters for {:?}",
        agents.len(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = deletions::start(ctx.clone(), delete_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    ownership.push(sample_ownership(deps, "before-fault", ownership.last(), false).await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S6: baseline produced no confirmed operations, aborting before injection");
        let rounds = handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Some(build_resurrection(&rounds, &split, None, delete_config))
        );
    }

    if let Err(e) = split::verify_ownership(subject, deps, &split).await {
        warn!("S6: ownership drifted between selection and injection: {e:#}");
        let rounds = handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!("{e:#}"),
            },
            &records,
            Some(build_resurrection(&rounds, &split, None, delete_config))
        );
    }

    info!(
        "S6: baseline complete ({baseline_operations} confirmed ops, {} rounds), naming {} and \
         signalling readiness",
        handle.rounds().len(),
        split.pod_address
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S6.as_str().to_string(),
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
            warn!("S6: no fault-injected signal arrived: {e}");
            let rounds = handle.stop().await;
            let records = history.snapshot();
            finish!(
                signal_termination(&e),
                &records,
                Some(build_resurrection(&rounds, &split, None, delete_config))
            );
        }
    };
    info!(
        "S6: fault {} ({} on {}) reported active at {}",
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
            warn!("S6: no fault-recovered signal arrived: {e}");
            let rounds = handle.stop().await;
            let records = history.snapshot();
            finish!(
                signal_termination(&e),
                &records,
                Some(build_resurrection(
                    &rounds,
                    &split,
                    fault_window(fault_injected_at, None),
                    delete_config
                ))
            );
        }
    };
    info!(
        "S6: executor back at {} ({})",
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
        "S6: recovery phase, running for {:?}",
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
    info!("S6: settling {SETTLE_BEFORE_READBACK:?} before the final read");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    // The one read per agent, which answers the last round it ran. Every other
    // round was probed by the increment that followed it; this is the only one
    // that has nothing after it.
    close_last_rounds(&ctx, &agents, &mut rounds).await;

    let records = history.snapshot();
    let resurrection = build_resurrection(
        &rounds,
        &split,
        fault_window(fault_injected_at, fault_recovered_at),
        delete_config,
    );
    info!(
        "S6: resurrection account — {} rounds, {} deleted exactly, {} findings",
        resurrection.rounds_recorded,
        resurrection.deleted_exactly,
        resurrection.findings.len()
    );

    let reason = if resurrection.has_violations() {
        let first = resurrection
            .findings
            .first()
            .map(|f| format!("{} round {}: {}", f.agent, f.round, f.detail))
            .unwrap_or_default();
        TerminationReason::AgentResurrected {
            findings: resurrection.findings.len() as u64 + resurrection.findings_omitted,
            first,
        }
    } else if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
        TerminationReason::StreamNeverSucceeded {
            stream: Stream::Delete.to_string(),
        }
    } else {
        TerminationReason::Completed
    };

    finish!(reason, &records, Some(resurrection));
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

fn build_resurrection(
    rounds: &[DeleteRound],
    split: &PodSplit,
    fault: Option<FaultWindow>,
    config: &crate::chaos::DeleteConfig,
) -> ResurrectionReport {
    ResurrectionReport::build(rounds, split, fault, config.increments_per_round)
}

/// Reads each agent once and uses the value to judge the last round it ran.
///
/// A read is an invocation and would shift what "the last N invocations" means
/// for any delete after it — which is exactly why the workload never reads
/// mid-round. Here there is nothing after it, so it is safe, and it recovers a
/// round per agent that would otherwise be unjudgeable.
async fn close_last_rounds(ctx: &WorkloadContext, agents: &[String], rounds: &mut [DeleteRound]) {
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
        "S6: closing {} unprobed rounds with a final read",
        pending.len()
    );
    for (agent, index) in pending {
        if !agents.iter().any(|a| a == &agent) {
            continue;
        }
        match workload::read_counter(ctx, &agent).await {
            Ok(value) => rounds[index].observed_after = Some(value),
            Err(e) => warn!("S6: could not read {agent} to close its last round: {e}"),
        }
    }
}
