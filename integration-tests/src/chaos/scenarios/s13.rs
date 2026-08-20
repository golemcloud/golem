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

//! S13 — rolling executor restarts under load (GOL-367).
//!
//! One executor is killed every 60 seconds for five minutes while the mixed
//! workload keeps running. Nothing is partitioned and nothing is updated. The
//! question is whether repeated rebalances compose.
//!
//! ## Why repetition is the point
//!
//! S8 kills an executor once and asks whether the platform recovers. It does.
//! S13 asks a different question: whether recovering five times in a row leaves
//! the cluster in the same state as recovering once.
//!
//! Each kill starts a rebalance. If a rebalance has not finished when the next
//! kill lands, the shard-manager is reasoning about a topology that is already
//! stale, and consumers are routing on caches that were invalidated mid-flight.
//! Faults that are individually survivable can still accumulate, and a single
//! kill cannot show that.
//!
//! The 60 second cadence is deliberately close to how long a rebalance takes.
//! Spacing the kills far enough apart to guarantee quiet between them would
//! turn S13 into S8 repeated five times, which nobody needs.
//!
//! ## What fails the run
//!
//! One thing: an idempotency key that executed twice.
//!
//! That is the same assertion S1 makes and for the same reason. Overlapping
//! shard ownership is not observable from outside the cluster, but the harm it
//! causes is: an agent whose state forked because two executors both served it.
//! See [`crate::chaos::ownership`] for why the routing table cannot answer this
//! directly.
//!
//! Everything else is reported. Acceptance degrades while executors restart, and
//! that is what a restart costs, not a defect. Shard movement, unassigned
//! shards and executor-set changes are recorded as context for reading a
//! violation, never as verdicts of their own.
//!
//! ## What the result carries
//!
//! Assignment is sampled every 20 seconds for the whole run, so a reader can
//! line each rebalance up against the restart that caused it. The restarts
//! themselves come from the workflow through `executor-restarts.json`, because
//! the driver knows nothing about Kubernetes and the single `fault-injected`
//! signal cannot describe five kills.

use crate::chaos::history::{OperationHistory, OperationRecord, Phase, Stream};
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::probe;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ReadKind, ScenarioOutcome, WARMUP_SETTLE, build_result, exactly_once_termination,
    read_back_agents, read_counters, sample_ownership, signal_termination, snapshot_routing,
    wait_for_settled_routing, warm_up, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals, RestartEvent};
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, ExactlyOnceReport, TerminationReason, stream_that_never_succeeded,
};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{info, warn};

/// How often the assignment is sampled for the whole run.
///
/// Phase-boundary samples are useless here: the interesting transitions happen
/// between them, five times over. A run whose only evidence was before and
/// after would show a cluster that looked fine at both ends.
const ASSIGNMENT_SAMPLE_INTERVAL: Duration = Duration::from_secs(20);

/// How long to wait after stopping the workload before reading durable state.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(30);

pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let history = OperationHistory::new(ScenarioCode::S13.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S13);

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
    let mut attention_extra: Vec<String> = Vec::new();

    // Sample the assignment continuously for the whole run. Five rebalances in
    // five minutes cannot be read from phase boundaries.
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
                if let Ok(mut t) = timeline.lock() {
                    t.push(sample);
                }
            }
        })
    };

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $exactly_once:expr) => {{
            sampler_stop.store(true, Ordering::Relaxed);
            sampler.abort();
            let mut samples = timeline.lock().map(|t| t.clone()).unwrap_or_default();
            samples.extend(ownership.clone());
            samples.sort_by_key(|s| s.taken_at);

            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            );
            summary.ownership = samples;
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
                    scheduled_selection: None,
                },
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Warm-up ─────────────────────────────────────────────────────────────
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);
    info!("S13: warming up agents before the baseline");
    let warmed = warm_up(&ctx, workload_config).await;
    info!("S13: warmed {warmed} agents, settling {:?}", WARMUP_SETTLE);
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S13: baseline phase, running mixed workload for {:?}",
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

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S13: baseline produced no confirmed operations, aborting before injection");
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

    info!("S13: baseline complete ({baseline_operations} confirmed ops), signalling readiness");
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S13.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        // Every kill picks its own pod. Which executor dies each time carries no
        // information the run depends on — the claim is about the sequence.
        fault_target: None,
    })?;

    // ── Rolling fault ───────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S13: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S13: first restart ({} on {}) reported at {}",
        injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S13: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S13: rolling restarts finished at {}",
        recovered.recovered_at
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    let restarts = signals.read_restart_events();
    attention_extra.push(describe_restarts(&restarts));
    for event in &restarts {
        info!(
            "S13: restart {} at {} ({})",
            event.sequence, event.killed_at, event.fault_id
        );
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S13: recovery phase, running for a further {:?}",
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
    ownership.push(sample_ownership(deps, "after-settle", ownership.last(), true).await);

    // ── Read-back and the exactly-once account ──────────────────────────────
    info!("S13: settling {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();
    let readback = read_back(&ctx, &records, workload_config).await;

    // Read-back before the probe: the probe can itself execute a key that never
    // ran, and comparing the two reads is what separates "replayed a stored
    // result" from "did the work".
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
        "S13: exactly-once account — {} keys checked, {} recovered by the probe, {} findings",
        exactly_once.keys_checked,
        exactly_once.keys_recovered_by_probe,
        exactly_once.findings.len()
    );

    let reason = exactly_once_termination(&exactly_once)
        .or_else(|| {
            stream_that_never_succeeded(&ChaosSummary::build(
                &records,
                readback.clone(),
                routing_snapshots.clone(),
                fault_injected_at,
            ))
            .map(|stream| TerminationReason::StreamNeverSucceeded {
                stream: stream.to_string(),
            })
        })
        .unwrap_or(TerminationReason::Completed);

    finish!(reason, &records, readback, Some(exactly_once));
}

/// One line saying what the rolling schedule actually did.
///
/// A run that intended five restarts and performed two is not a weaker version
/// of the same experiment, it is a different one. Saying so in the result means
/// a reader never has to reconstruct it from timestamps.
fn describe_restarts(restarts: &[RestartEvent]) -> String {
    if restarts.is_empty() {
        return "S13 recorded no executor restarts — the rolling schedule did not run, so \
                nothing about cumulative rebalance can be read from this run"
            .to_string();
    }
    let first = restarts.first().map(|e| e.killed_at);
    let last = restarts.last().map(|e| e.killed_at);
    let span = match (first, last) {
        (Some(a), Some(b)) => (b - a).num_seconds(),
        _ => 0,
    };
    format!(
        "S13 performed {} executor restarts over {span}s",
        restarts.len()
    )
}

/// Durable and scheduled state, compared against what the driver submitted.
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
