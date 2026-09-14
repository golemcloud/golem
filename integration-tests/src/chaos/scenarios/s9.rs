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

//! S9 — executor pod kill during a component rollback (GOL-369).
//!
//! S5 moves agents forward onto a new build and kills an executor while that is
//! happening. S9 moves them forward, waits for that to land, and then moves them
//! **back**, killing an executor during the return leg.
//!
//! The return leg is the one that matters operationally. A rollback is what you
//! reach for when the new build is already going wrong, so a rollback happening
//! under a dying executor is not a contrived situation — it is the situation you
//! would actually be in.
//!
//! ## What a rollback is here
//!
//! A redeploy. The original artifact is uploaded again as a **new** revision and
//! every agent is asked to move to it. That is what rollback means in practice,
//! and it is what makes the evidence unambiguous: `Counter::component_version`
//! is compiled into each build — `1` in `agent-counters`, `2` in
//! `agent-counters-v2`, nothing else different — so an agent that has genuinely
//! returned reports `1` from the code that is running, not from metadata about
//! what the platform believes.
//!
//! ## The constraint that shapes this scenario
//!
//! An automatic update **replays the agent's oplog against the new build** and
//! aborts if any recorded invocation produces a different result. It can only
//! cross a build boundary that no recorded invocation can tell apart.
//!
//! That has a sharp operational consequence, and it is arguably S9's most
//! useful finding: **a behaviour-changing build cannot be rolled back
//! automatically.** If the new build ever returned a different answer for an
//! invocation still in an agent's oplog — which is usually *why* you are
//! rolling back — the automatic update is refused. A rollback in that situation
//! needs a snapshot-based update instead.
//!
//! It also dictates how this scenario may look at its own agents. The first S9
//! run verified the forward leg by invoking `component_version`, which exists
//! precisely to differ between builds, and thereby wrote an entry into all 200
//! oplogs that the rollback's replay could never reproduce. Every rollback was
//! refused with `Unexpected oplog entry: expected component_version => 1, got
//! 2`. The forward leg is now read from metadata, which leaves no trace; the
//! running code is asked only at the very end, where nothing depends on it.
//! See `an_automatic_update_rolls_an_agent_back_to_an_earlier_build` in
//! `golem-worker-executor/tests/hot_update.rs` for the minimal reproduction.
//!
//! ## Why the forward leg is verified first
//!
//! If the agents never reached the new build, rolling them back returns them to
//! a build they never left, every check passes, and the run proves nothing. So
//! the forward leg is measured and the rollback is refused outright if too few
//! agents made it. Same instinct as S6's smoke round: a clean report from a
//! scenario that never happened is the worst artifact this suite can produce.
//!
//! ## What fails the run
//!
//! The same two things S5 asserts, reused rather than re-invented because they
//! are the same facts:
//!
//! - an agent whose durable state fell below what the driver was told succeeded,
//!   or rose above what it could possibly have asked for;
//! - an agent still reporting the build it was rolled back *from*.
//!
//! An agent that cannot be read at all is reported rather than assumed either
//! way, and control-plane refusals are counted apart from workload retries —
//! see [`crate::chaos::rollback`] for why that separation is load-bearing.

use crate::chaos::history::{OperationHistory, OperationRecord, Phase, Stream};
use crate::chaos::prep::{COUNTERS_V2_WASM, COUNTERS_WASM, ChaosPrepManifest};
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::rollback::{ControlPlaneAttempts, RollbackReport, VersionCensus};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, WARMUP_SETTLE, build_result, readback_for, signal_termination,
    snapshot_routing, wait_for_settled_routing, warm_up, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals};
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, Note, ReadbackVerdict, TerminationReason,
    stream_that_never_succeeded,
};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait after stopping the workload before reading durable state.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(30);

/// How many agents to update concurrently.
///
/// Update requests are cheap to issue and the point is that many are in flight
/// when the executor dies, so this is wide rather than polite.
const UPDATE_CONCURRENCY: usize = 32;

/// What the running code reports on each of the two builds.
///
/// Compiled into the WASM rather than read from metadata, which is the whole
/// reason these numbers can be trusted: metadata says what the platform
/// believes, `component_version` says what is executing.
const VERSION_ON_THE_NEW_BUILD: u32 = 2;
const VERSION_AFTER_ROLLBACK: u32 = 1;

pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let rollback_config = config.require_rollback()?;
    let history = OperationHistory::new(ScenarioCode::S9.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S9);

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
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut attention_extra: Vec<Note> = Vec::new();
    let mut rollback_report: Option<RollbackReport> = None;

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            );
            summary.absorb(attention_extra.clone());
            if let Some(report) = rollback_report.clone() {
                summary = summary.with_rollback(report);
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
                    delete_selection: None,
                },
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Warm-up ─────────────────────────────────────────────────────────────
    //
    // Same reason as S1: an agent's first invocation costs far more than its
    // later ones, and a population still cold-starting is a different thing to
    // update than a population that is running.
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);

    info!("S9: warming up agents before the baseline");
    let warmed = warm_up(&ctx, workload_config).await;
    info!("S9: warmed {warmed} agents, settling {:?}", WARMUP_SETTLE);
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S9: baseline phase, running mixed workload for {:?}",
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = workload::start(ctx.clone(), workload_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S9: baseline produced no confirmed operations, aborting before injection");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new()
        );
    }

    // ── Roll forward ────────────────────────────────────────────────────────
    //
    // The workload keeps running throughout. This leg is not the experiment: it
    // exists to put the agents somewhere they can be brought back *from*.
    info!("S9: rolling forward — updating the counters component to {COUNTERS_V2_WASM}");
    let forward = match ctx
        .user
        .update_component(&manifest.counters_component_id, COUNTERS_V2_WASM)
        .await
    {
        Ok(updated) => updated,
        Err(e) => {
            warn!("S9: roll-forward component update failed: {e:#}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(
                TerminationReason::Aborted {
                    detail: format!("component update to {COUNTERS_V2_WASM} failed: {e:#}"),
                },
                &records,
                Vec::new()
            );
        }
    };
    let forward_revision = forward.revision;
    info!(
        "S9: component now at revision {forward_revision}, moving {} durable agents onto it",
        workload_config.durable_agents
    );
    let _ = request_updates(&ctx, workload_config, forward_revision, 0, Duration::ZERO).await;

    // Let the forward leg land before measuring it. Without this the census
    // below reads a population still in transit, and the gate would refuse a
    // rollback that would have been perfectly good.
    info!(
        "S9: letting the roll-forward settle for {:?}",
        rollback_config.settle()
    );
    tokio::time::sleep(rollback_config.settle()).await;

    // Read from metadata, NOT by invoking `component_version`.
    //
    // This is the correction the first S9 run forced, and it is not a
    // downgrade of the evidence — it is the only way to gather it without
    // destroying what comes next. An automatic update replays the agent's
    // oplog against the new build and aborts if any recorded invocation
    // produces a different result. `component_version` exists precisely to
    // differ between builds, so invoking it here writes an entry into every
    // agent's oplog that the rollback's replay can never reproduce. The first
    // run did exactly that and all 200 rollbacks were refused with
    // "Unexpected oplog entry: expected component_version => 1, got 2".
    //
    // The forward leg only has to establish that the agents moved, so that the
    // rollback has something to undo. Which revision the platform has them on
    // answers that, and leaves no trace. The end state is still judged on the
    // running code, at the very end, where nothing depends on it.
    let rolled_forward = VersionCensus::build(
        "before-rollback",
        forward_revision.get() as u32,
        &read_revisions(&ctx, workload_config).await,
    );
    info!(
        "S9: {} of {} agents are on revision {} ({} unreadable)",
        rolled_forward.on_expected,
        rolled_forward.agents,
        forward_revision,
        rolled_forward.unreadable
    );

    // ── Roll back ───────────────────────────────────────────────────────────
    //
    // A redeploy of the original artifact as a new revision. Uploading it again
    // rather than pointing agents back at the old revision is what "rollback"
    // means operationally, and it keeps the evidence in the running code.
    info!("S9: rolling back — re-uploading {COUNTERS_WASM} as a new revision");
    let back = match ctx
        .user
        .update_component(&manifest.counters_component_id, COUNTERS_WASM)
        .await
    {
        Ok(updated) => updated,
        Err(e) => {
            warn!("S9: rollback component update failed: {e:#}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(
                TerminationReason::Aborted {
                    detail: format!("rollback upload of {COUNTERS_WASM} failed: {e:#}"),
                },
                &records,
                Vec::new()
            );
        }
    };
    let rollback_revision = back.revision;

    let mut report = RollbackReport {
        forward_revision: forward_revision.get(),
        rollback_revision: rollback_revision.get(),
        forward_version: VERSION_ON_THE_NEW_BUILD,
        rollback_version: VERSION_AFTER_ROLLBACK,
        rolled_forward,
        rolled_back: None,
        control: ControlPlaneAttempts::default(),
        rolled_forward_floor_percent: rollback_config.rolled_forward_floor_percent,
    };

    // Refuse to spend the maintenance window rolling agents back to a build
    // they never left. Every check would pass and the run would prove nothing.
    if !report.forward_leg_landed() {
        warn!("S9: the roll-forward did not land, refusing to roll back");
        handle.stop().await;
        let records = history.snapshot();
        let detail = report
            .attention_lines()
            .first()
            .cloned()
            .unwrap_or_else(|| "the roll-forward did not land".to_string());
        rollback_report = Some(report);
        finish!(
            TerminationReason::FaultTargetUnverified { detail },
            &records,
            Vec::new()
        );
    }

    info!(
        "S9: rollback revision is {rollback_revision}, asking {} durable agents to return",
        workload_config.durable_agents
    );
    let rollback_started_at = Utc::now();
    report.control = request_updates(
        &ctx,
        workload_config,
        rollback_revision,
        rollback_config.control_retries,
        rollback_config.control_retry_delay(),
    )
    .await;
    attention_extra.push(Note::leveled(
        report.control.refused > 0,
        format!(
            "rollback to revision {rollback_revision} accepted for {} of {} durable agents at \
             {rollback_started_at}",
            report.control.accepted(),
            workload_config.durable_agents
        ),
    ));
    rollback_report = Some(report);

    // ── Signal: ready for the fault ─────────────────────────────────────────
    tokio::time::sleep(rollback_config.kill_delay()).await;
    info!(
        "S9: {:?} into the rollback, signalling readiness for the kill",
        rollback_config.kill_delay()
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S9.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        // Agents are mid-rollback across both executors, so killing either one
        // interrupts a return in flight. Which one carries no information.
        fault_target: None,
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S9: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new());
        }
    };
    let into_update = (injected.injected_at - rollback_started_at).num_milliseconds();
    info!(
        "S9: fault {} ({} on {}) reported active at {}, {into_update}ms into the rollback",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    attention_extra.push(Note::context(format!(
        "the executor kill landed {into_update}ms into the rollback"
    )));
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S9: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new());
        }
    };
    info!("S9: fault cleared at {}", recovered.recovered_at);
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S9: recovery phase, running for a further {:?}",
        config.phases.recovery()
    );
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    tokio::time::sleep(config.phases.recovery()).await;
    handle.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);

    // ── Read-back ───────────────────────────────────────────────────────────
    info!("S9: settling {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();
    let readback = read_back(&ctx, &records, workload_config).await;

    // One census, read once. Both the count and the stale list below describe
    // the same moment: reading twice let an agent be counted as rolled back in
    // the first pass and listed as stale in the second, so the report could
    // contradict itself over an agent that simply landed between the two.
    let versions = read_versions(&ctx, workload_config).await;
    let rolled_back = VersionCensus::build("after-recovery", VERSION_AFTER_ROLLBACK, &versions);
    let stale: Vec<String> = versions
        .into_iter()
        .filter(|(_, v)| *v == Some(VERSION_ON_THE_NEW_BUILD))
        .map(|(agent, _)| agent)
        .collect();
    attention_extra.push(Note::leveled(
        rolled_back.on_expected < rolled_back.agents,
        format!(
            "after recovery {} of {} durable agents report component version {}; {} could not \
             be read",
            rolled_back.on_expected,
            rolled_back.agents,
            VERSION_AFTER_ROLLBACK,
            rolled_back.unreadable
        ),
    ));
    if let Some(report) = rollback_report.as_mut() {
        report.rolled_back = Some(rolled_back);
    }

    // ── Verdict ─────────────────────────────────────────────────────────────
    let reason = if let Some(bad) = readback.iter().find(|r| {
        matches!(
            r.verdict,
            ReadbackVerdict::LostWork | ReadbackVerdict::DuplicateExecution
        )
    }) {
        TerminationReason::UpdateStateInconsistent {
            agent: bad.agent.clone(),
            detail: format!(
                "{:?}: observed {:?} against an expected range of {}..={}",
                bad.verdict, bad.observed, bad.expected_min, bad.expected_max
            ),
        }
    } else if let Some(agent) = stale.first() {
        // An agent still answering with the build it was rolled back *from* is
        // a real failure. One that could not be read at all is not: an
        // unreadable agent is reported above and says nothing either way.
        //
        // `UpdateNotApplied` rather than a parallel rollback variant, because
        // it is the same fact — an update that did not land — and inventing a
        // second name for it would only make the two harder to search for.
        TerminationReason::UpdateNotApplied {
            agent: agent.clone(),
            observed: Some(VERSION_ON_THE_NEW_BUILD),
            expected: VERSION_AFTER_ROLLBACK,
        }
    } else if let Some(stream) = stream_that_never_succeeded(&ChaosSummary::build(
        &records,
        readback.clone(),
        routing_snapshots.clone(),
        fault_injected_at,
    )) {
        TerminationReason::StreamNeverSucceeded {
            stream: stream.to_string(),
        }
    } else {
        TerminationReason::Completed
    };

    finish!(reason, &records, readback);
}

/// Asks every durable agent to move to `target_revision`, concurrently.
///
/// Returns how many requests the platform accepted. A refused request is
/// recorded and the run continues: the scenario is about what happens to the
/// updates that *did* start when the executor died.
/// Asks every durable agent to move to `target_revision`, retrying refusals.
///
/// The retries here are the **control plane's**, counted apart from the
/// workload's. They answer a different question: a request refused because its
/// agent's executor just died says nothing about the platform's correctness,
/// but an agent nobody successfully asked to come back explains a stale agent
/// later without excusing one. Passing `0` retries makes this the same
/// fire-once call S5 does.
async fn request_updates(
    ctx: &WorkloadContext,
    config: &crate::chaos::WorkloadConfig,
    target_revision: golem_common::model::component::ComponentRevision,
    retries: u32,
    delay: Duration,
) -> ControlPlaneAttempts {
    let mut pending: Vec<String> = (0..config.durable_agents)
        .map(|index| ctx.agent_name(Stream::Durable, index))
        .collect();

    let mut account = ControlPlaneAttempts {
        requested: pending.len() as u64,
        max_retries: retries,
        ..Default::default()
    };

    for attempt in 0..=retries {
        if pending.is_empty() {
            break;
        }
        if attempt > 0 && !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        let mut refused = Vec::new();
        for chunk in pending.chunks(UPDATE_CONCURRENCY) {
            let mut batch = tokio::task::JoinSet::new();
            for agent in chunk.iter().cloned() {
                let ctx = ctx.clone();
                batch.spawn(async move {
                    let id = workload::counter_agent_id(&ctx, &agent);
                    // `disable_wakeup: false` — the agent should be woken to
                    // process the update rather than waiting for its next
                    // invocation, because the kill is timed against the
                    // rollback starting, not against the next caller happening
                    // along.
                    ctx.user
                        .auto_update_worker(&id, target_revision, false)
                        .await
                        .map_err(|e| (agent, e))
                });
            }
            while let Some(joined) = batch.join_next().await {
                match joined {
                    Ok(Ok(())) => {
                        if attempt == 0 {
                            account.accepted_first_try += 1;
                        } else {
                            account.accepted_after_retry += 1;
                        }
                    }
                    Ok(Err((agent, e))) => {
                        warn!("S9: update request for {agent} refused: {e:#}");
                        refused.push(agent);
                    }
                    Err(e) => warn!("S9: an update request task panicked: {e}"),
                }
            }
        }
        pending = refused;
    }

    account.refused = pending.len() as u64;
    account
}

/// Asks the platform which component revision each durable agent is on.
///
/// Deliberately metadata rather than an invocation. See the comment at the
/// forward-leg census: invoking a method whose result differs between builds
/// writes an oplog entry that the next automatic update's replay cannot
/// reproduce, which aborts that update. Reading metadata leaves no trace.
async fn read_revisions(
    ctx: &WorkloadContext,
    config: &crate::chaos::WorkloadConfig,
) -> std::collections::BTreeMap<String, Option<u32>> {
    let names: Vec<String> = (0..config.durable_agents)
        .map(|index| ctx.agent_name(Stream::Durable, index))
        .collect();

    let mut out = std::collections::BTreeMap::new();
    for chunk in names.chunks(UPDATE_CONCURRENCY) {
        let mut batch = tokio::task::JoinSet::new();
        for agent in chunk.iter().cloned() {
            let ctx = ctx.clone();
            batch.spawn(async move {
                let id = workload::counter_agent_id(&ctx, &agent);
                let observed = ctx
                    .user
                    .get_worker_metadata(&id)
                    .await
                    .ok()
                    .map(|m| m.component_revision.get() as u32);
                (agent, observed)
            });
        }
        while let Some(joined) = batch.join_next().await {
            if let Ok((agent, observed)) = joined {
                out.insert(agent, observed);
            }
        }
    }
    out
}

/// Asks every durable agent which build it is running.
///
/// `None` means the agent could not be read, which is reported rather than
/// counted against the update.
async fn read_versions(
    ctx: &WorkloadContext,
    config: &crate::chaos::WorkloadConfig,
) -> std::collections::BTreeMap<String, Option<u32>> {
    let names: Vec<String> = (0..config.durable_agents)
        .map(|index| ctx.agent_name(Stream::Durable, index))
        .collect();

    let mut out = std::collections::BTreeMap::new();
    for chunk in names.chunks(UPDATE_CONCURRENCY) {
        let mut batch = tokio::task::JoinSet::new();
        for agent in chunk.iter().cloned() {
            let ctx = ctx.clone();
            batch.spawn(async move {
                let observed = workload::read_component_version(&ctx, &agent).await.ok();
                (agent, observed)
            });
        }
        while let Some(joined) = batch.join_next().await {
            if let Ok((agent, observed)) = joined {
                out.insert(agent, observed);
            }
        }
    }
    out
}

/// Durable and scheduled state, compared against what the driver submitted.
async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    config: &crate::chaos::WorkloadConfig,
) -> Vec<AgentReadback> {
    let mut readback = Vec::new();

    for index in 0..config.durable_agents {
        let agent = ctx.agent_name(Stream::Durable, index);
        let scoped = records
            .iter()
            .filter(|r| r.stream == Stream::Durable && r.agent == agent);
        if scoped.clone().next().is_none() {
            continue;
        }
        let observed = workload::read_counter(ctx, &agent).await;
        readback.extend(readback_for(Stream::Durable, &agent, scoped, observed));
    }

    for index in 0..config.scheduled_agents {
        let target = ctx.schedule_target_name(index);
        let scoped = records
            .iter()
            .filter(|r| r.stream == Stream::Scheduled && r.agent == target);
        if scoped.clone().next().is_none() {
            continue;
        }
        let observed = workload::read_polls(ctx, &target).await;
        readback.extend(readback_for(Stream::Scheduled, &target, scoped, observed));
    }

    readback
}
