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

//! S12 — shard-manager pod restart under mixed workload (GOL-363).
//!
//! The first complete chaos path, and the one that proves the harness. The
//! choreography:
//!
//! 1. **Baseline** — run the mixed workload until the platform is warm, so
//!    recovery is measured against steady state rather than against cold start.
//! 2. **Signal** — announce readiness, then wait for the workflow to report the
//!    fault active. The driver never kills anything itself.
//! 3. **Fault** — keep the workload running while the shard-manager is gone.
//!    This is the population that matters: operations submitted with no
//!    shard-manager to route them.
//! 4. **Recovery** — keep running after the workflow reports the pod healthy
//!    again, long enough for in-flight and scheduled work to drain.
//! 5. **Read-back** — stop the workload, let the platform quiesce, then read the
//!    durable counters and compare against what was submitted.
//!
//! Read-back must come after the workload stops and after a settle delay: it
//! compares durable state against submitted operations, so running it while
//! operations are still landing would report a mismatch that means nothing.

use crate::chaos::history::{OperationHistory, OperationRecord, Phase, Stream};
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RESULT_SCHEMA_VERSION, RunScope};
use crate::chaos::signal::{BaselineReady, FaultSignals, SignalError};
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, RoutingSnapshot, TerminationReason, stream_that_never_succeeded,
};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::benchmark::RunMetadata;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeMap;
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait after stopping the workload before reading durable state.
/// Scheduled polls registered just before the stop still have to fire, and an
/// in-flight increment still has to land; reading too early would report a
/// mismatch that says nothing about the platform.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(30);

/// Where the driver writes its artifacts.
pub struct OutputPaths {
    pub result: Option<std::path::PathBuf>,
    pub history: Option<std::path::PathBuf>,
}

/// Runs S12 end to end.
///
/// Returns the result. A failed run still returns one — the artifact is the
/// point, and an aborted run's partial artifact is often the most interesting
/// one there is.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let history = OperationHistory::new(ScenarioCode::S12.as_str());
    let key_prefix = format!("chaos-{}", ScenarioCode::S12.as_str().to_lowercase());

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

    // Every early return below goes through `finish`, so an abort produces the
    // same artifact shape as a completed run — just with fewer phases filled in.
    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr) => {{
            let result = build_result(
                config,
                started_at,
                phases.clone(),
                fault_injected_at,
                fault_recovered_at,
                fault_id.clone(),
                scope.clone(),
                $records,
                $readback,
                routing_snapshots.clone(),
                $reason,
            );
            write_outputs(&result, &history, outputs)?;
            return Ok(result);
        }};
    }

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S12: baseline phase, running mixed workload for {:?}",
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = workload::start(ctx.clone(), &config.workload);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    // ── Signal: ready for the fault ─────────────────────────────────────────
    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        // Injecting a fault into a workload that never worked would measure
        // nothing. Stop before touching the cluster.
        warn!("S12: baseline produced no confirmed operations, aborting before injection");
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

    info!("S12: baseline complete ({baseline_operations} confirmed ops), signalling readiness");
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S12.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S12: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new());
        }
    };
    info!(
        "S12: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S12: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new());
        }
    };
    info!(
        "S12: fault cleared at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S12: recovery phase, running for a further {:?}",
        config.phases.recovery()
    );
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    tokio::time::sleep(config.phases.recovery()).await;

    // Stopping waits for in-flight operations to record themselves rather than
    // cancelling them: an operation cancelled mid-flight is one the history
    // cannot classify, and during a fault those are exactly the interesting
    // ones.
    handle.stop().await;

    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);

    // ── Read-back ───────────────────────────────────────────────────────────
    info!("S12: letting the platform settle for {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();
    let readback = read_back(&ctx, &records, &config.workload).await;

    let reason = match stream_that_never_succeeded(&ChaosSummary::build(
        &records,
        readback.clone(),
        routing_snapshots.clone(),
        fault_injected_at,
    )) {
        Some(stream) => TerminationReason::StreamNeverSucceeded {
            stream: stream.to_string(),
        },
        None => TerminationReason::Completed,
    };

    finish!(reason, &records, readback);
}

/// Reads durable state back for every agent of the streams that keep a count,
/// and compares it against what the driver submitted.
async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    config: &crate::chaos::WorkloadConfig,
) -> Vec<AgentReadback> {
    let mut readback = Vec::new();

    for index in 0..config.durable_agents {
        let agent = ctx.agent_name(Stream::Durable, index);
        let scoped: Vec<&OperationRecord> = records
            .iter()
            .filter(|r| r.stream == Stream::Durable && r.agent == agent)
            .collect();
        if scoped.is_empty() {
            continue;
        }
        let observed = workload::read_counter(ctx, &agent).await;
        readback.push(AgentReadback::evaluate(
            Stream::Durable,
            &agent,
            &scoped,
            observed,
        ));
    }

    for index in 0..config.scheduled_agents {
        let target = ctx.schedule_target_name(index);
        let scoped: Vec<&OperationRecord> = records
            .iter()
            .filter(|r| r.stream == Stream::Scheduled && r.agent == target)
            .collect();
        if scoped.is_empty() {
            continue;
        }
        let observed = workload::read_polls(ctx, &target).await;
        readback.push(AgentReadback::evaluate(
            Stream::Scheduled,
            &target,
            &scoped,
            observed,
        ));
    }

    readback
}

/// Samples the routing table. Failure is recorded, not propagated: the
/// shard-manager being unreachable is an expected observation during a
/// shard-manager fault, and losing the whole run over it would be absurd.
async fn snapshot_routing(deps: &BenchmarkTestDependencies, at: &str) -> RoutingSnapshot {
    match deps.shard_manager().get_routing_table().await {
        Ok(table) => {
            let shards_per_executor: BTreeMap<String, usize> = table
                .shards_per_pod()
                .into_iter()
                .map(|(pod, count)| (pod.to_string(), count))
                .collect();
            RoutingSnapshot {
                at: at.to_string(),
                taken_at: Utc::now(),
                shards_per_executor: Some(shards_per_executor),
                unavailable_reason: None,
            }
        }
        Err(e) => {
            warn!("S12: routing table unavailable at {at}: {e:#}");
            RoutingSnapshot {
                at: at.to_string(),
                taken_at: Utc::now(),
                shards_per_executor: None,
                unavailable_reason: Some(format!("{e:#}")),
            }
        }
    }
}

fn signal_termination(error: &SignalError) -> TerminationReason {
    match error {
        SignalError::Timeout { file, .. } => TerminationReason::SignalTimeout {
            file: file.to_string(),
        },
        other => TerminationReason::Aborted {
            detail: other.to_string(),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn build_result(
    config: &ScenarioConfig,
    started_at: chrono::DateTime<Utc>,
    phases: Phases,
    fault_injected_at: Option<chrono::DateTime<Utc>>,
    fault_recovered_at: Option<chrono::DateTime<Utc>>,
    fault_id: Option<String>,
    scope: RunScope,
    records: &[OperationRecord],
    readback: Vec<AgentReadback>,
    routing_snapshots: Vec<RoutingSnapshot>,
    termination_reason: TerminationReason,
) -> ChaosResult {
    let summary = ChaosSummary::build(records, readback, routing_snapshots, fault_injected_at);
    let metadata = RunMetadata::from_env();

    ChaosResult {
        schema_version: RESULT_SCHEMA_VERSION,
        scenario_code: config.code.to_uppercase(),
        scenario_name: config.name.clone(),
        completed: !termination_reason.is_failure(),
        termination_reason,
        started_at,
        ended_at: Some(Utc::now()),
        phases,
        fault_injected_at,
        fault_recovered_at,
        fault_id,
        fault: config.fault.clone(),
        workload: config.workload.clone(),
        retry_policy: config.retry_policy.clone(),
        scope,
        summary,
        run_metadata: (!metadata.is_empty()).then_some(metadata),
    }
}

/// Writes result and history wherever the caller asked for them. Called on every
/// exit path, including aborts — a run that produced no readable artifact is a
/// wasted maintenance window.
fn write_outputs(
    result: &ChaosResult,
    history: &OperationHistory,
    outputs: &OutputPaths,
) -> anyhow::Result<()> {
    if let Some(path) = &outputs.result {
        result.save(path)?;
        info!("S12: result written to {path:?}");
    }
    if let Some(path) = &outputs.history {
        history.save(path, !result.completed)?;
        info!("S12: operation history written to {path:?}");
    }
    Ok(())
}

/// Writes whatever artifacts exist for a run that died before producing a
/// result — a cancelled workflow, or a panic. Best effort by definition.
pub fn flush_partial(history: &OperationHistory, outputs: &OutputPaths, detail: &str) {
    warn!("S12: flushing partial artifacts ({detail})");
    if let Some(path) = &outputs.history
        && let Err(e) = history.save(path, true)
    {
        warn!("S12: could not write partial history to {path:?}: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::signal::FAULT_INJECTED_FILE;
    use test_r::test;

    #[test]
    fn a_signal_timeout_names_the_file_that_never_arrived() {
        let error = SignalError::Timeout {
            file: FAULT_INJECTED_FILE,
            dir: "/tmp/signals".to_string(),
            waited: Duration::from_secs(1800),
        };
        assert_eq!(
            signal_termination(&error),
            TerminationReason::SignalTimeout {
                file: FAULT_INJECTED_FILE.to_string()
            }
        );
    }

    #[test]
    fn other_signal_errors_abort_with_the_underlying_detail() {
        let error = SignalError::Io(anyhow::anyhow!("permission denied"));
        match signal_termination(&error) {
            TerminationReason::Aborted { detail } => {
                assert!(detail.contains("permission denied"))
            }
            other => panic!("expected an abort, got {other:?}"),
        }
    }
}
