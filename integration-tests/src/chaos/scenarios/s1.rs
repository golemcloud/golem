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
//! ### Why the driver can still reach the executors
//!
//! The partition is between the shard-manager and the executors. The driver
//! reaches executors over forwarded ports from outside the cluster, on the
//! health/metrics port, and that path is untouched by the fault. An
//! introspection endpoint that the fault could cut off would be useless for
//! observing that fault — see [`crate::chaos::executors`].

use crate::chaos::executors::ExecutorSample;
use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use crate::chaos::ownership::OwnershipReport;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, build_result, readback_for, signal_termination, snapshot_routing,
    write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals};
use crate::chaos::summary::{AgentReadback, ChaosSummary, TerminationReason};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait after stopping the workload before reading durable state.
/// Same reasoning as S12: an in-flight increment still has to land, and reading
/// early would report a mismatch that says nothing about the platform.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(30);

/// Runs S1 end to end.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    executor_endpoints: Option<&PathBuf>,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let ownership_config = config.require_ownership()?;
    let history = OperationHistory::new(ScenarioCode::S1.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S1);

    // Without executor endpoints the ownership oracle has nothing to read, and
    // an S1 run without it is just a slower S12 wearing its name. Refuse rather
    // than produce a report that looks complete and checks nothing.
    let endpoints_path = executor_endpoints.cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "chaos scenario S1 requires --executor-endpoints: its verdict is built from each \
             executor's own shard assignment, which it cannot read without them"
        )
    })?;

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
    let mut ownership: Vec<OwnershipReport> = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr) => {{
            let summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            )
            .with_ownership(ownership.clone());
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
    ownership.push(sample_ownership(deps, &endpoints_path, "before-fault", false).await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    // A baseline sample that could read nothing means the endpoints file is
    // wrong or the forwards never came up. Every later sample would be empty
    // too, so the verdict would be vacuous — stop before breaking anything.
    let baseline_ownership = ownership.last().expect("just pushed");
    if baseline_ownership.executors_analysed == 0 {
        warn!("S1: no executor could be read before the fault, aborting before injection");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: format!(
                    "no executor assignment could be read from {endpoints_path:?} before the \
                     fault, so the ownership oracle would have nothing to judge"
                ),
            },
            &records,
            Vec::new()
        );
    }
    info!(
        "S1: baseline ownership covers {} executors claiming {} shards",
        baseline_ownership.executors_analysed, baseline_ownership.shards_claimed
    );

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
            Vec::new()
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
            finish!(signal_termination(&e), &records, Vec::new());
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
    ownership.push(sample_ownership(deps, &endpoints_path, "during-fault", false).await);

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S1: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new());
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

    let judged = sample_ownership(deps, &endpoints_path, "after-settle", true).await;
    info!(
        "S1: judged ownership sample covers {} executors ({} excluded), {} findings",
        judged.executors_analysed,
        judged.executors_excluded.len(),
        judged.findings.len()
    );
    let ownership_verdict = ownership_termination(&judged);
    ownership.push(judged);

    // The workload keeps running through the rest of recovery whatever the
    // ownership verdict was: an overlap finding is about shards, and the
    // completion and read-back evidence is about operations. Cutting the run
    // short would throw away the second to report the first.
    let remaining = config
        .phases
        .recovery()
        .saturating_sub(ownership_config.settle());
    info!("S1: recovery phase, running for a further {remaining:?}");
    tokio::time::sleep(remaining).await;

    handle.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);

    // ── Read-back ───────────────────────────────────────────────────────────
    info!("S1: letting the platform settle for {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();
    let readback = read_back(&ctx, &records, workload_config).await;

    // The ownership verdict wins when there is one: a forked agent is a
    // stronger statement than any completion count, and it is the only thing
    // this scenario is in a position to assert.
    let reason = ownership_verdict.unwrap_or_else(|| {
        if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
            TerminationReason::StreamNeverSucceeded {
                stream: Stream::Durable.to_string(),
            }
        } else {
            TerminationReason::Completed
        }
    });

    finish!(reason, &records, readback);
}

/// Samples every executor's assignment and analyses it against the routing
/// table.
///
/// The routing table is best-effort: `snapshot_routing` already treats an
/// unreachable shard-manager as an observation, and the fatal ownership check
/// compares executors against each other, so it does not need the table at all.
async fn sample_ownership(
    deps: &BenchmarkTestDependencies,
    endpoints_path: &PathBuf,
    at: &str,
    judged: bool,
) -> OwnershipReport {
    let sample = ExecutorSample::take(endpoints_path, at).await;
    let routing = deps.shard_manager().get_routing_table().await.ok();
    OwnershipReport::build(sample, routing.as_ref(), judged)
}

/// Turns the judged sample into a termination reason, if it found something
/// fatal.
fn ownership_termination(judged: &OwnershipReport) -> Option<TerminationReason> {
    let fatal: Vec<&crate::chaos::ownership::OwnershipFinding> = judged.fatal_findings().collect();
    if fatal.is_empty() {
        return None;
    }
    Some(TerminationReason::ShardOwnershipViolated {
        findings: fatal.len() as u64,
        first: fatal[0].detail.clone(),
    })
}

/// Read-back for the streams that keep a durable count, exactly as S12 does it:
/// the completion and idempotency evidence has to keep running alongside the
/// ownership oracle, not instead of it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::executors::ExecutorAssignment;
    use crate::chaos::ownership::Violation;
    use test_r::test;

    fn executor(pod: &str, shards: &[i64]) -> ExecutorAssignment {
        ExecutorAssignment {
            pod_name: pod.to_string(),
            pod_ip: format!("10.0.0.{}", shards.len()),
            reported_executor_id: Some(pod.to_string()),
            identity_mismatch: None,
            assigned: true,
            number_of_shards: Some(4),
            shard_ids: shards.to_vec(),
            read_error: None,
        }
    }

    fn judged(executors: Vec<ExecutorAssignment>) -> OwnershipReport {
        OwnershipReport::build(
            ExecutorSample {
                at: "after-settle".to_string(),
                taken_at: Utc::now(),
                executors,
            },
            None,
            true,
        )
    }

    #[test]
    fn a_clean_settled_assignment_produces_no_termination_reason() {
        let report = judged(vec![
            executor("exec-a", &[0, 1]),
            executor("exec-b", &[2, 3]),
        ]);
        assert!(ownership_termination(&report).is_none());
    }

    /// The one assertion this scenario makes.
    #[test]
    fn overlap_after_settling_terminates_the_run() {
        let report = judged(vec![
            executor("exec-a", &[0, 1, 2]),
            executor("exec-b", &[2, 3]),
        ]);
        match ownership_termination(&report) {
            Some(TerminationReason::ShardOwnershipViolated { findings, first }) => {
                assert_eq!(findings, 1);
                assert!(first.contains("shard 2"), "{first}");
            }
            other => panic!("expected a shard-ownership violation, got {other:?}"),
        }
    }

    /// A gap is reported by the oracle but must not end the run — a rebalance
    /// legitimately passes through one.
    #[test]
    fn a_gap_after_settling_is_reported_without_terminating_the_run() {
        let report = judged(vec![executor("exec-a", &[0, 1]), executor("exec-b", &[2])]);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == Violation::UnownedShard)
        );
        assert!(ownership_termination(&report).is_none());
    }

    /// A violation is a statement about shards; the completion evidence is a
    /// statement about operations. Reporting the first must not cost the second.
    #[test]
    fn a_violation_is_recorded_as_the_reason_but_the_findings_stay_readable() {
        let report = judged(vec![
            executor("exec-a", &[0, 1, 2]),
            executor("exec-b", &[2, 3]),
        ]);
        let summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), None)
            .with_ownership(vec![report.clone()]);

        assert_eq!(summary.ownership.len(), 1);
        assert!(
            summary
                .attention
                .iter()
                .any(|line| line.contains("overlapping-ownership")),
            "the finding has to reach the attention list: {:?}",
            summary.attention
        );
    }
}
