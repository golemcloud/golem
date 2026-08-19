// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! S5 — executor pod kill during an automatic component update (GOL-365).
//!
//! Agents are running on the counters component. The scenario updates that
//! component to a second build, asks every durable agent to move to it, and
//! then kills an executor while those updates are still in flight. Every
//! affected agent has to come back on the new build with its state intact.
//!
//! ## What makes an update the interesting moment to kill an executor
//!
//! An update rewrites what an agent runs while its state stays put. An executor
//! dying part-way through leaves agents in three different situations at once:
//! updated, not yet updated, and updating. All three have to converge, and the
//! state each one carries has to survive the transition. That is a different
//! question from S8's crash under load, where nothing about the agent is
//! changing.
//!
//! ## Why v2 is one character different from v1
//!
//! `agent-counters-v2` is `agent-counters` with `Counter::component_version`
//! returning 2 instead of 1. Nothing else differs.
//!
//! That is deliberate. If the two builds behaved differently, a state mismatch
//! after the kill would be ambiguous between "the restart lost it" and "the two
//! builds disagree about what the state means". Keeping them identical makes
//! every mismatch attributable to the restart, which is the only thing S5 is
//! entitled to claim.
//!
//! `component_version` is also how the run proves the update landed. Component
//! metadata reports the revision the platform *believes* an agent is on;
//! invoking `component_version` reports what the code actually executing says.
//! Only the second is evidence.
//!
//! ## What fails the run
//!
//! Two things, both narrow:
//!
//! - an agent whose durable state fell below what the driver was told
//!   succeeded, or rose above what it could possibly have asked for
//!   ([`ReadbackVerdict::LostWork`] and [`ReadbackVerdict::DuplicateExecution`]);
//! - an agent still reporting the old build after recovery.
//!
//! Acceptance degradation during the kill is expected and is recorded without
//! changing the verdict. An agent that cannot be read at all is reported rather
//! than assumed either way.
//!
//! ## Timing, and what the driver actually controls
//!
//! The driver cannot kill anything. It starts the update, waits
//! [`KILL_DELAY_INTO_UPDATE`], then writes `baseline-ready.json`, which is what
//! asks the workflow to inject. Applying the PodChaos and confirming it takes a
//! few seconds more, so the kill lands within roughly the first ten seconds of
//! the update rather than at exactly two. The result records when the update
//! started and when the fault was confirmed active, so how far in it landed is
//! always readable rather than assumed.

use crate::chaos::history::{OperationHistory, OperationRecord, Phase, Stream};
use crate::chaos::prep::{COUNTERS_V2_WASM, ChaosPrepManifest};
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, WARMUP_SETTLE, build_result, readback_for, signal_termination,
    snapshot_routing, wait_for_settled_routing, warm_up, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals};
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, ReadbackVerdict, TerminationReason, stream_that_never_succeeded,
};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::time::Duration;
use tracing::{info, warn};

/// How long after asking agents to update before the workflow is told to kill.
///
/// The spec says two seconds. See the module docs for why that is the moment
/// the driver *asks*, not the moment the executor dies.
const KILL_DELAY_INTO_UPDATE: Duration = Duration::from_secs(2);

/// How long to wait after stopping the workload before reading durable state.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(30);

/// How many agents to update concurrently.
///
/// Update requests are cheap to issue and the point is that many are in flight
/// when the executor dies, so this is wide rather than polite.
const UPDATE_CONCURRENCY: usize = 32;

/// The build every agent must be running once the dust settles.
const EXPECTED_VERSION_AFTER_UPDATE: u32 = 2;

pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let history = OperationHistory::new(ScenarioCode::S5.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S5);

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
    let mut attention_extra: Vec<String> = Vec::new();

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            );
            summary.attention.extend(attention_extra.clone());
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

    // ── Warm-up ─────────────────────────────────────────────────────────────
    //
    // Same reason as S1: an agent's first invocation costs far more than its
    // later ones, and a population still cold-starting is a different thing to
    // update than a population that is running.
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);

    info!("S5: warming up agents before the baseline");
    let warmed = warm_up(&ctx, workload_config).await;
    info!("S5: warmed {warmed} agents, settling {:?}", WARMUP_SETTLE);
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S5: baseline phase, running mixed workload for {:?}",
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
        warn!("S5: baseline produced no confirmed operations, aborting before injection");
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

    // ── Update ──────────────────────────────────────────────────────────────
    //
    // The workload keeps running throughout. Agents are being invoked while
    // they are being moved to the new build, which is the state the kill has to
    // interrupt.
    info!("S5: updating the counters component to {COUNTERS_V2_WASM}");
    let updated = match ctx
        .user
        .update_component(&manifest.counters_component_id, COUNTERS_V2_WASM)
        .await
    {
        Ok(updated) => updated,
        Err(e) => {
            warn!("S5: component update failed: {e:#}");
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
    let target_revision = updated.revision;
    let update_started_at = Utc::now();
    info!(
        "S5: component now at revision {target_revision}, asking {} durable agents to update",
        workload_config.durable_agents
    );

    let requested = request_updates(&ctx, workload_config, target_revision).await;
    attention_extra.push(format!(
        "update to revision {target_revision} requested for {requested} of {} durable agents \
         at {update_started_at}",
        workload_config.durable_agents
    ));

    // ── Signal: ready for the fault ─────────────────────────────────────────
    tokio::time::sleep(KILL_DELAY_INTO_UPDATE).await;
    info!(
        "S5: {:?} into the update, signalling readiness for the kill",
        KILL_DELAY_INTO_UPDATE
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S5.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        // Agents are mid-update across both executors, so killing either one
        // interrupts updates in flight. Which one carries no information.
        fault_target: None,
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S5: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new());
        }
    };
    let into_update = (injected.injected_at - update_started_at).num_milliseconds();
    info!(
        "S5: fault {} ({} on {}) reported active at {}, {into_update}ms into the update",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    attention_extra.push(format!(
        "the executor kill landed {into_update}ms into the update"
    ));
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S5: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new());
        }
    };
    info!("S5: fault cleared at {}", recovered.recovered_at);
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S5: recovery phase, running for a further {:?}",
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
    info!("S5: settling {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();
    let readback = read_back(&ctx, &records, workload_config).await;

    let versions = read_versions(&ctx, workload_config).await;
    let stale: Vec<&String> = versions
        .iter()
        .filter(|(_, v)| **v != Some(EXPECTED_VERSION_AFTER_UPDATE))
        .map(|(agent, _)| agent)
        .collect();
    let unreadable = versions.values().filter(|v| v.is_none()).count();
    attention_extra.push(format!(
        "after recovery {} of {} durable agents report component version {}; {} could not be read",
        versions.len() - stale.len(),
        versions.len(),
        EXPECTED_VERSION_AFTER_UPDATE,
        unreadable
    ));

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
    } else if let Some(agent) = stale
        .iter()
        .find(|agent| versions.get(**agent).copied().flatten().is_some())
    {
        // An agent that answered with the old version is a real failure. One
        // that could not be read at all is not: an unreadable agent is reported
        // above and says nothing either way about the update.
        TerminationReason::UpdateNotApplied {
            agent: (*agent).clone(),
            observed: versions.get(*agent).copied().flatten(),
            expected: EXPECTED_VERSION_AFTER_UPDATE,
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
async fn request_updates(
    ctx: &WorkloadContext,
    config: &crate::chaos::WorkloadConfig,
    target_revision: golem_common::model::component::ComponentRevision,
) -> usize {
    let names: Vec<String> = (0..config.durable_agents)
        .map(|index| ctx.agent_name(Stream::Durable, index))
        .collect();

    let mut accepted = 0usize;
    for chunk in names.chunks(UPDATE_CONCURRENCY) {
        let mut batch = tokio::task::JoinSet::new();
        for agent in chunk.iter().cloned() {
            let ctx = ctx.clone();
            batch.spawn(async move {
                let id = workload::counter_agent_id(&ctx, &agent);
                // `disable_wakeup: false` — the agent should be woken to
                // process the update rather than waiting for its next
                // invocation, because the kill is timed against the update
                // starting, not against the next caller happening along.
                ctx.user
                    .auto_update_worker(&id, target_revision, false)
                    .await
                    .map_err(|e| (agent, e))
            });
        }
        while let Some(joined) = batch.join_next().await {
            match joined {
                Ok(Ok(())) => accepted += 1,
                Ok(Err((agent, e))) => warn!("S5: update request for {agent} refused: {e:#}"),
                Err(e) => warn!("S5: an update request task panicked: {e}"),
            }
        }
    }
    accepted
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
