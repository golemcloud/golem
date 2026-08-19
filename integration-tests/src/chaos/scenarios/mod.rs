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

//! Chaos scenario implementations.
//!
//! One module per scenario code. Each one owns its phase choreography — which
//! is the part that differs, and the part worth reading — while everything
//! around it lives here: where artifacts go, how a signal failure becomes a
//! termination reason, how a routing table is sampled, and how a result is
//! assembled.
//!
//! The split matters because these shared pieces are where a scenario is easy
//! to get quietly wrong. A scenario that forgot to write its artifacts on an
//! abort path, or that invented a phase window it never reached, would still
//! produce a plausible-looking report from a wasted maintenance window.

pub mod s1;
pub mod s12;
pub mod s8;

use crate::chaos::ScenarioConfig;
use crate::chaos::history::{OperationHistory, OperationRecord};
use crate::chaos::pinned::PinnedSelection;
use crate::chaos::result::{ChaosResult, Phases, RESULT_SCHEMA_VERSION, RunScope};
use crate::chaos::signal::SignalError;
use crate::chaos::summary::{AgentReadback, ChaosSummary, RoutingSnapshot, TerminationReason};
use chrono::{DateTime, Utc};
use golem_test_framework::benchmark::RunMetadata;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use std::collections::BTreeMap;
use tracing::{info, warn};

/// Where the driver writes its artifacts. Both are optional so a scenario can
/// be run by hand with no archiving at all.
pub struct OutputPaths {
    pub result: Option<std::path::PathBuf>,
    pub history: Option<std::path::PathBuf>,
}

/// Everything a scenario accumulates as it runs, handed over once to become a
/// result.
///
/// A struct rather than a dozen positional arguments because every field here
/// is optional-shaped for the same reason — an aborted run fills in fewer of
/// them — and a positional call site makes it far too easy to swap two
/// `Option<DateTime>`s and never notice.
pub struct ScenarioOutcome {
    pub started_at: DateTime<Utc>,
    pub phases: Phases,
    pub fault_injected_at: Option<DateTime<Utc>>,
    pub fault_recovered_at: Option<DateTime<Utc>>,
    pub fault_id: Option<String>,
    /// What the workflow reported it aimed at — a pod name for a pinned
    /// scenario, a deployment name otherwise.
    pub fault_target_observed: Option<String>,
    pub scope: RunScope,
    pub summary: ChaosSummary,
    pub termination_reason: TerminationReason,
    /// Present only for scenarios that pin the fault to one executor.
    pub pinned_selection: Option<PinnedSelection>,
}

/// Assembles the archived result.
pub fn build_result(config: &ScenarioConfig, outcome: ScenarioOutcome) -> ChaosResult {
    let metadata = RunMetadata::from_env();

    ChaosResult {
        schema_version: RESULT_SCHEMA_VERSION,
        scenario_code: config.code.to_uppercase(),
        scenario_name: config.name.clone(),
        completed: !outcome.termination_reason.is_failure(),
        termination_reason: outcome.termination_reason,
        started_at: outcome.started_at,
        ended_at: Some(Utc::now()),
        phases: outcome.phases,
        fault_injected_at: outcome.fault_injected_at,
        fault_recovered_at: outcome.fault_recovered_at,
        fault_id: outcome.fault_id,
        fault_target_observed: outcome.fault_target_observed,
        fault: config.fault.clone(),
        workload: config.workload.clone(),
        pinned: config.pinned.clone(),
        pinned_selection: outcome.pinned_selection,
        retry_policy: config.retry_policy.clone(),
        scope: outcome.scope,
        summary: outcome.summary,
        run_metadata: (!metadata.is_empty()).then_some(metadata),
    }
}

/// Writes result and history wherever the caller asked for them.
///
/// Called on every exit path, including aborts: a run that produced no readable
/// artifact is a wasted maintenance window, and an aborted run's partial
/// artifact is often the most interesting one there is.
pub fn write_outputs(
    result: &ChaosResult,
    history: &OperationHistory,
    outputs: &OutputPaths,
) -> anyhow::Result<()> {
    if let Some(path) = &outputs.result {
        result.save(path)?;
        info!("{}: result written to {path:?}", result.scenario_code);
    }
    if let Some(path) = &outputs.history {
        history.save(path, !result.completed)?;
        info!(
            "{}: operation history written to {path:?}",
            result.scenario_code
        );
    }
    Ok(())
}

/// Writes whatever artifacts exist for a run that died before producing a
/// result — a cancelled workflow, or a panic. Best effort by definition.
pub fn flush_partial(history: &OperationHistory, outputs: &OutputPaths, detail: &str) {
    warn!("Chaos: flushing partial artifacts ({detail})");
    if let Some(path) = &outputs.history
        && let Err(e) = history.save(path, true)
    {
        warn!("Chaos: could not write partial history to {path:?}: {e:#}");
    }
}

/// Turns a failed signal wait into the reason the run stopped.
pub fn signal_termination(error: &SignalError) -> TerminationReason {
    match error {
        SignalError::Timeout { file, .. } => TerminationReason::SignalTimeout {
            file: file.to_string(),
        },
        other => TerminationReason::Aborted {
            detail: other.to_string(),
        },
    }
}

/// Samples the routing table.
///
/// Failure is recorded, not propagated: the shard-manager being unreachable is
/// an expected *observation* during a shard-manager fault, and losing the whole
/// run over it would be absurd.
pub async fn snapshot_routing(deps: &BenchmarkTestDependencies, at: &str) -> RoutingSnapshot {
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
            warn!("Chaos: routing table unavailable at {at}: {e:#}");
            RoutingSnapshot {
                at: at.to_string(),
                taken_at: Utc::now(),
                shards_per_executor: None,
                unavailable_reason: Some(format!("{e:#}")),
            }
        }
    }
}

/// How long to wait for the routing table to cover every shard before a
/// scenario starts measuring.
///
/// A cluster that has just scaled up is still converging, and a measurement
/// taken against it cannot distinguish "the platform was slow" from "the shard
/// was not routable yet". Waiting removes that doubt rather than leaving it to
/// be argued about afterwards. Not a correctness gate: on expiry the scenario
/// runs anyway and the snapshot says what the table looked like.
const ROUTING_SETTLE_TIMEOUT_SECS: u64 = 180;

/// How often to re-read the routing table while waiting.
const ROUTING_POLL_SECS: u64 = 3;

/// Blocks until the routing table covers every shard, or the timeout lapses.
///
/// Returns the line to record, so the result says which of the two happened
/// rather than leaving a reader to infer it from timings.
pub async fn wait_for_settled_routing(
    deps: &BenchmarkTestDependencies,
    snapshots: &mut Vec<RoutingSnapshot>,
) -> String {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(ROUTING_SETTLE_TIMEOUT_SECS);
    // Assigned on every path through the loop below before it is read.
    let mut last: String;

    loop {
        match deps.shard_manager().get_routing_table().await {
            Ok(table) => {
                let total = table.number_of_shards.value;
                let per_pod = table.shards_per_pod();
                let assigned: usize = per_pod.values().sum();
                let executors = per_pod.len();
                last = format!(
                    "routing at start: {assigned}/{total} shards across {executors} executor(s)"
                );
                if assigned == total && executors > 0 {
                    snapshots.push(snapshot_routing(deps, "settled-before-start").await);
                    info!("Chaos: {last} — settled");
                    return format!("{last} (settled before measuring)");
                }
                info!("Chaos: {last} — waiting for the table to cover every shard");
            }
            Err(e) => {
                last = format!("routing table unavailable: {e:#}");
                warn!("Chaos: {last}");
            }
        }

        if std::time::Instant::now() >= deadline {
            snapshots.push(snapshot_routing(deps, "unsettled-before-start").await);
            warn!("Chaos: {last} — proceeding anyway after {ROUTING_SETTLE_TIMEOUT_SECS}s");
            return format!(
                "WARNING: measured against an unsettled cluster — {last}. \
                 Baseline numbers may reflect routing convergence rather than the \
                 platform."
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(ROUTING_POLL_SECS)).await;
    }
}

/// Which durable value an agent is read back on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadKind {
    /// `Counter.count`.
    Counter,
    /// `ScheduleCounter.polls`.
    Polls,
    /// `QuotaCounter.count`, paired with `QuotaCounter.refused`.
    QuotaCounter,
}

/// How many agents are read back at once.
///
/// Sequential read-back does not survive contact with a fault. Every read is
/// bounded by `READ_TIMEOUT`, so a scenario with 300 agents and a handful of
/// unresponsive ones spends hours walking them one at a time — long after the
/// maintenance window it was given. Reads do not mutate, so running them
/// concurrently changes nothing about the numbers.
const READ_CONCURRENCY: usize = 16;

/// How often to report progress through a read-back pass.
const READ_PROGRESS_EVERY: usize = 100;

/// Reads a set of agents back concurrently, in bounded batches.
///
/// Returns one entry per agent that had records, in the order given. An agent
/// that could not be read carries its reason rather than being dropped — see
/// [`AgentReadback`].
pub async fn read_back_agents(
    ctx: &crate::chaos::workload::WorkloadContext,
    records: &[OperationRecord],
    agents: Vec<(crate::chaos::history::Stream, String, ReadKind)>,
) -> Vec<AgentReadback> {
    use crate::chaos::workload;

    let total = agents.len();
    let mut out = Vec::with_capacity(total);
    let mut next_report = READ_PROGRESS_EVERY;

    for chunk in agents.chunks(READ_CONCURRENCY) {
        let mut batch = tokio::task::JoinSet::new();
        for (stream, agent, kind) in chunk.iter().cloned() {
            let ctx = ctx.clone();
            batch.spawn(async move {
                let observed = match kind {
                    ReadKind::Counter => workload::read_counter(&ctx, &agent).await,
                    ReadKind::Polls => workload::read_polls(&ctx, &agent).await,
                    ReadKind::QuotaCounter => workload::read_quota_counter(&ctx, &agent).await,
                };
                // Only the quota stream has a second number, and it is the one
                // that says what losing a lease actually cost.
                let refused = match kind {
                    ReadKind::QuotaCounter => workload::read_refused(&ctx, &agent).await.ok(),
                    _ => None,
                };
                (stream, agent, observed, refused)
            });
        }

        let mut batch_results = Vec::new();
        while let Some(joined) = batch.join_next().await {
            match joined {
                Ok(result) => batch_results.push(result),
                Err(e) => warn!("Chaos: a read-back task panicked: {e}"),
            }
        }
        // Restore the caller's order, which the JoinSet does not preserve.
        batch_results.sort_by(|a, b| a.1.cmp(&b.1));

        for (stream, agent, observed, refused) in batch_results {
            let scoped = records
                .iter()
                .filter(|r| r.stream == stream && r.agent == agent);
            if let Some(mut entry) = readback_for(stream, &agent, scoped, observed) {
                entry.refused_reservations = refused;
                out.push(entry);
            }
        }

        if out.len() >= next_report {
            info!("Chaos: read back {} of {total} agents", out.len());
            next_report += READ_PROGRESS_EVERY;
        }
    }

    out
}

/// Read-back for one agent, given the records aimed at it and the value its
/// durable state reported.
pub fn readback_for<'a>(
    stream: crate::chaos::history::Stream,
    agent: &str,
    records: impl Iterator<Item = &'a OperationRecord>,
    observed: Result<u64, String>,
) -> Option<AgentReadback> {
    let scoped: Vec<&OperationRecord> = records.collect();
    if scoped.is_empty() {
        return None;
    }
    Some(AgentReadback::evaluate(stream, agent, &scoped, observed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::signal::FAULT_INJECTED_FILE;
    use std::time::Duration;
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
