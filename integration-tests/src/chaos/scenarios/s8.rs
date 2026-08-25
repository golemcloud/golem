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

//! S8 — HTTP in-flight executor crash recovery (GOL-366).
//!
//! S12 asks what a *stream* of work does when the platform is disturbed. S8 asks
//! what happens to *specific* operations that were provably running on the pod
//! that died, and answers it exactly rather than leaving it to an operator.
//!
//! The choreography:
//!
//! 1. **Pin** — pick an executor that owns enough candidate agents, and warm
//!    those agents so they are resident on it rather than merely routed to it.
//! 2. **Baseline** — start the pinned workload: one long `sleep_and_increment`
//!    in flight per agent, replaced as it completes, so the in-flight count is
//!    constant and known.
//! 3. **Verify and signal** — re-check ownership against a fresh routing table,
//!    then name the target pod in the readiness signal. The workflow aims at
//!    *that* pod; Chaos Mesh's `mode: one` would have picked at random.
//! 4. **Fault** — keep submitting while the executor is gone. Every operation
//!    in flight at the kill is one whose fate the run has to account for.
//! 5. **Recovery** — keep running after the workflow reports the pod healthy,
//!    long enough for shard reassignment and in-flight work to drain.
//! 6. **Account** — stop, settle, read the counters, probe every key under its
//!    own idempotency key, read the counters again.
//!
//! ### Why this one asserts and S12 does not
//!
//! S12 reports and lets an operator judge, because its findings come from
//! aggregate arithmetic over thousands of operations with a band of doubt around
//! them. S8's population is bounded and every key is checked individually
//! against the platform's own stored result, so its findings are facts about a
//! named key rather than a judgement about a distribution. Two of them fail the
//! run outright:
//!
//! - an accepted operation with no final result after recovery
//! - a key with more than one distinct successful completion
//!
//! Everything else — how long recovery took, how many keys the probe recovered
//! a result for, how the shards redistributed — is still reported for reading.

use crate::chaos::history::{OperationHistory, OperationRecord, Phase, Stream};
use crate::chaos::pinned::{self, PinnedSelection};
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::probe;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, build_result, readback_for, signal_termination, snapshot_routing,
    write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals, FaultTarget};
use crate::chaos::summary::{AgentReadback, ChaosSummary, ExactlyOnceReport, TerminationReason};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeMap;
use std::time::Duration;
use tracing::{info, warn};

/// How long to wait after stopping the workload before reading durable state.
///
/// Longer than S12's, on purpose: an operation here can be mid-`sleep` on an
/// executor that has just taken over its shard, so the tail is a whole operation
/// duration rather than a request round-trip.
const SETTLE_BEFORE_READBACK: Duration = Duration::from_secs(60);

/// Runs S8 end to end.
///
/// Returns the result even when the run failed — the artifact is the point, and
/// an aborted run's partial artifact is often the most interesting one there is.
pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let pinned_config = config.require_pinned()?;
    let history = OperationHistory::new(ScenarioCode::S8.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S8);

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

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut selection: Option<PinnedSelection> = None;

    // Every early return goes through `finish`, so an abort produces the same
    // artifact shape as a completed run — just with fewer phases filled in.
    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $exactly_once:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            );
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
                    pinned_selection: selection.clone(),
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

    // ── Pin the fault target ────────────────────────────────────────────────
    // Before anything else, because there is no point warming agents or running
    // a baseline for a fault that cannot be aimed.
    let chosen = match pinned::select(&ctx, deps, pinned_config).await {
        Ok(chosen) => chosen,
        Err(e) => {
            warn!("S8: could not pin a fault target: {e:#}");
            let records = history.snapshot();
            finish!(
                TerminationReason::FaultTargetUnverified {
                    detail: format!("{e:#}"),
                },
                &records,
                Vec::new(),
                None
            );
        }
    };
    selection = Some(chosen.clone());
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);

    // Warming does two things at once: it creates the agents, and it makes them
    // *resident* on the executor that owns them. Routing alone would put the
    // operations there; residency is what makes the kill destroy running work
    // rather than merely redirect a request.
    let baseline_counters = warm_and_read(&ctx, &chosen).await;
    info!(
        "S8: warmed {} pinned agents on {}",
        baseline_counters.len(),
        chosen.pod_address
    );

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S8: baseline phase, {} concurrent in-flight operations for {:?}",
        chosen.agents.len(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = pinned::start(ctx.clone(), &chosen, pinned_config);
    tokio::time::sleep(config.phases.baseline()).await;
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S8: baseline produced no confirmed operations, aborting before injection");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no pinned operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new(),
            None
        );
    }

    // ── Verify ownership, then signal ───────────────────────────────────────
    // The gap between selecting a target and the workflow killing it is where a
    // rebalance would silently redirect the experiment. Checking here, with the
    // workload already running, is as close to the injection as the driver can
    // get.
    if let Err(e) = pinned::verify_ownership(&ctx, deps, &chosen).await {
        warn!("S8: pinned ownership no longer holds, refusing to inject: {e:#}");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!("{e:#}"),
            },
            &records,
            Vec::new(),
            None
        );
    }

    info!(
        "S8: baseline complete ({baseline_operations} confirmed ops), signalling readiness \
         with fault target {}",
        chosen.pod_address
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S8.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        fault_target: Some(FaultTarget {
            pod_address: chosen.pod_address.clone(),
            pod_ip: chosen.pod_ip.clone(),
            owned_agents: chosen.agents.clone(),
        }),
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S8: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S8: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S8: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S8: fault cleared at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S8: recovery phase, running for a further {:?}",
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

    // ── Account ─────────────────────────────────────────────────────────────
    info!("S8: letting the platform settle for {SETTLE_BEFORE_READBACK:?} before read-back");
    tokio::time::sleep(SETTLE_BEFORE_READBACK).await;

    let records = history.snapshot();

    // Read-back is taken *before* the probe pass, because the probe can itself
    // execute a key that never ran. Comparing the two reads is what separates
    // "the probe replayed a stored result" from "the probe did the work".
    let before_probe = read_counters(&ctx, &chosen).await;
    let readback = readback_agents(&chosen, &records, &baseline_counters, &before_probe);

    let probes = probe::probe_keys(&ctx, &records, Stream::PinnedHttp).await;
    let after_probe = read_counters(&ctx, &chosen).await;

    let report = ExactlyOnceReport::build(
        &records,
        &probes,
        Stream::PinnedHttp,
        &before_probe,
        &after_probe,
    );
    info!(
        "S8: exactly-once account — {} keys checked, {} with a final result, \
         {} recovered by the probe, {} findings",
        report.keys_checked,
        report.keys_with_final_result,
        report.keys_recovered_by_probe,
        report.findings.len()
    );

    let reason = if report.has_violations() {
        TerminationReason::ExactlyOnceViolated {
            findings: report.findings.len() as u64,
            first: report
                .findings
                .first()
                .map(|f| format!("{} on key {}", f.violation, f.idempotency_key))
                .unwrap_or_default(),
        }
    } else if records
        .iter()
        .all(|r| r.outcome != crate::chaos::history::Outcome::Confirmed)
    {
        TerminationReason::StreamNeverSucceeded {
            stream: Stream::PinnedHttp.to_string(),
        }
    } else {
        TerminationReason::Completed
    };

    finish!(reason, &records, readback, Some(report));
}

/// Creates and warms every pinned agent, and reports the counter each one starts
/// from.
///
/// The starting values matter because a resumed run reuses the same agent names:
/// read-back compares *deltas*, so a rerun on an existing prep manifest stays
/// meaningful instead of reporting every agent as having executed far too much.
async fn warm_and_read(
    ctx: &WorkloadContext,
    selection: &PinnedSelection,
) -> BTreeMap<String, u64> {
    let mut baseline = BTreeMap::new();
    for agent in &selection.agents {
        // `count` reads without mutating, and creating the agent is exactly the
        // side effect wanted here.
        match workload::read_counter(ctx, agent).await {
            Ok(value) => {
                baseline.insert(agent.clone(), value);
            }
            Err(e) => {
                // Not fatal: an agent that cannot be read now is reported as
                // unavailable in its read-back rather than losing the run.
                warn!("S8: could not warm pinned agent {agent}: {e}");
            }
        }
    }
    baseline
}

/// Reads the current counter of every pinned agent.
async fn read_counters(
    ctx: &WorkloadContext,
    selection: &PinnedSelection,
) -> BTreeMap<String, u64> {
    let mut values = BTreeMap::new();
    for agent in &selection.agents {
        match workload::read_counter(ctx, agent).await {
            Ok(value) => {
                values.insert(agent.clone(), value);
            }
            Err(e) => warn!("S8: could not read pinned agent {agent}: {e}"),
        }
    }
    values
}

/// Builds the per-agent read-back from counter *deltas* since warm-up.
///
/// Deltas rather than absolutes because a resumed run reuses the same agent
/// names: comparing against zero would report every agent on a rerun as having
/// executed far more than the run submitted.
fn readback_agents(
    selection: &PinnedSelection,
    records: &[OperationRecord],
    baseline: &BTreeMap<String, u64>,
    observed: &BTreeMap<String, u64>,
) -> Vec<AgentReadback> {
    let mut readback = Vec::new();
    for agent in &selection.agents {
        let scoped = records
            .iter()
            .filter(|r| r.stream == Stream::PinnedHttp && &r.agent == agent);
        if scoped.clone().next().is_none() {
            continue;
        }
        let delta = match (observed.get(agent), baseline.get(agent)) {
            (Some(now), Some(start)) => Ok(now.saturating_sub(*start)),
            (Some(now), None) => Ok(*now),
            (None, _) => Err(format!("agent {agent} could not be read back")),
        };
        readback.extend(readback_for(Stream::PinnedHttp, agent, scoped, delta));
    }
    readback
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::errors::ErrorClass;
    use crate::chaos::history::Outcome;
    use crate::chaos::probe::KeyProbe;
    use crate::chaos::summary::ExactlyOnceViolation;
    use test_r::test;

    fn record(op_id: u64, agent: &str, outcome: Outcome, value: Option<u32>) -> OperationRecord {
        let key = format!("{agent}-{op_id:08}");
        OperationRecord {
            op_id,
            stream: Stream::PinnedHttp,
            phase: Phase::Fault,
            agent: agent.to_string(),
            method: "sleep_and_increment".to_string(),
            idempotency_key: key,
            submitted_at: Utc::now(),
            completed_at: Some(Utc::now()),
            attempts: 1,
            outcome,
            duration_ms: 20_000,
            returned_value: value,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: vec![crate::chaos::history::AttemptRecord {
                attempt: 1,
                started_at: Utc::now(),
                duration_ms: 20_000,
                returned_value: value,
                succeeded: outcome == Outcome::Confirmed,
                error_class: None,
                error: None,
            }],
        }
    }

    /// A probe that got a definite answer from the platform.
    fn probe(record: &OperationRecord, final_value: Option<u32>) -> KeyProbe {
        KeyProbe {
            idempotency_key: record.idempotency_key.clone(),
            agent: record.agent.clone(),
            final_value,
            error: final_value.is_none().then(|| "refused".to_string()),
            // `Response` is the class that means "the platform answered, and the
            // answer was no" — the only failure that is evidence about the key
            // rather than about the connection.
            error_class: final_value.is_none().then_some(ErrorClass::Response),
        }
    }

    /// A probe that never got to ask.
    fn probe_failed(record: &OperationRecord, class: ErrorClass) -> KeyProbe {
        KeyProbe {
            idempotency_key: record.idempotency_key.clone(),
            agent: record.agent.clone(),
            final_value: None,
            error: Some(format!("{class} failure")),
            error_class: Some(class),
        }
    }

    /// The healthy shape: the driver got a value, and asking again under the
    /// same key replays that same value.
    #[test]
    fn a_key_that_replays_its_stored_result_is_not_a_finding() {
        let r = record(0, "chaos-s8-pinned-http-0000", Outcome::Confirmed, Some(7));
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe(&r, Some(7))],
            Stream::PinnedHttp,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(!report.has_violations(), "{:?}", report.findings);
        assert_eq!(report.keys_with_final_result, 1);
        assert_eq!(report.keys_recovered_by_probe, 0);
    }

    /// The defect the scenario exists to catch: one key, two different
    /// post-increment counts, so the work ran twice.
    #[test]
    fn a_key_with_two_distinct_completions_fails_the_run() {
        let r = record(1, "chaos-s8-pinned-http-0000", Outcome::Confirmed, Some(7));
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe(&r, Some(9))],
            Stream::PinnedHttp,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(report.has_violations());
        assert_eq!(
            report.findings[0].violation,
            ExactlyOnceViolation::MultipleDistinctCompletions
        );
    }

    /// Accepted work has to end up with a result. An operation the driver could
    /// not classify is still accepted work: the platform may well have run it.
    #[test]
    fn an_accepted_key_with_no_final_result_fails_the_run() {
        let r = record(2, "chaos-s8-pinned-http-0001", Outcome::Indeterminate, None);
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe(&r, None)],
            Stream::PinnedHttp,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(report.has_violations());
        assert_eq!(
            report.findings[0].violation,
            ExactlyOnceViolation::MissingFinalResult
        );
    }

    /// The distinction the whole verdict rests on: a probe that *could not ask*
    /// says nothing about whether the platform has the result. Reporting it as
    /// lost work would turn a connection problem into a correctness defect,
    /// which is the exact mistake this suite is built to avoid.
    #[test]
    fn a_probe_that_could_not_complete_leaves_the_key_inconclusive_rather_than_failing() {
        for class in [
            ErrorClass::Transport,
            ErrorClass::Platform,
            ErrorClass::Application,
        ] {
            let r = record(9, "chaos-s8-pinned-http-0009", Outcome::Indeterminate, None);
            let report = ExactlyOnceReport::build(
                std::slice::from_ref(&r),
                &[probe_failed(&r, class)],
                Stream::PinnedHttp,
                &BTreeMap::new(),
                &BTreeMap::new(),
            );
            assert!(
                !report.has_violations(),
                "a {class} probe failure must not fail the run: {:?}",
                report.findings
            );
            assert_eq!(
                report.keys_inconclusive, 1,
                "but it must be counted, so a clean verdict over many of them reads as weaker"
            );
        }
    }

    /// The other half of that distinction: a probe the platform *answered*, by
    /// refusing, is real evidence and does fail the run.
    #[test]
    fn a_probe_the_platform_definitively_refused_is_a_missing_result() {
        let r = record(
            10,
            "chaos-s8-pinned-http-0010",
            Outcome::Indeterminate,
            None,
        );
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe_failed(&r, ErrorClass::Response)],
            Stream::PinnedHttp,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(report.has_violations());
        assert_eq!(
            report.findings[0].violation,
            ExactlyOnceViolation::MissingFinalResult
        );
        assert_eq!(report.keys_inconclusive, 0);
    }

    /// A definite refusal is not accepted work, so the platform owes it nothing
    /// — flagging it would turn every rejected request into a false alarm.
    #[test]
    fn a_rejected_key_is_excluded_from_the_missing_result_check() {
        let r = record(3, "chaos-s8-pinned-http-0002", Outcome::Rejected, None);
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe(&r, None)],
            Stream::PinnedHttp,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(!report.has_violations());
        assert_eq!(report.keys_rejected, 1);
    }

    /// Recovery working: the driver never heard back, but the platform had the
    /// result all along. Reported, not failed.
    #[test]
    fn a_result_the_probe_recovers_is_reported_rather_than_failed() {
        let r = record(4, "chaos-s8-pinned-http-0003", Outcome::Indeterminate, None);
        let report = ExactlyOnceReport::build(
            std::slice::from_ref(&r),
            &[probe(&r, Some(3))],
            Stream::PinnedHttp,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(!report.has_violations());
        assert_eq!(report.keys_recovered_by_probe, 1);
    }

    /// The probe's own footprint has to be visible: a counter that moved across
    /// the pass means some key had never run, and an operator reading a "no
    /// duplicates" verdict needs to know how much of it was replay.
    #[test]
    fn counter_movement_across_the_probe_pass_is_reported_per_agent() {
        let agent = "chaos-s8-pinned-http-0000".to_string();
        let before = BTreeMap::from([(agent.clone(), 40u64)]);
        let after = BTreeMap::from([(agent.clone(), 42u64)]);
        let report = ExactlyOnceReport::build(&[], &[], Stream::PinnedHttp, &before, &after);
        assert_eq!(report.probe_executed_total, 2);
        assert_eq!(report.probe_executed_per_agent.get(&agent), Some(&2));
    }

    /// Read-back compares deltas, not absolutes, so a rerun against agents that
    /// already carry state from an earlier attempt still reports honestly.
    #[test]
    fn readback_is_relative_to_the_counter_each_agent_started_from() {
        let agent = "chaos-s8-pinned-http-0000";
        let selection = PinnedSelection {
            pod_address: "10.0.0.1:9000".to_string(),
            pod_ip: "10.0.0.1".to_string(),
            agents: vec![agent.to_string()],
            number_of_shards: 1024,
            candidates_per_pod: BTreeMap::new(),
            candidates_scanned: 400,
        };
        let records = vec![
            record(0, agent, Outcome::Confirmed, Some(101)),
            record(1, agent, Outcome::Confirmed, Some(102)),
        ];
        // The agent was already at 100 when the run started and is at 102 now:
        // two executions, exactly what was submitted.
        let baseline = BTreeMap::from([(agent.to_string(), 100u64)]);
        let observed = BTreeMap::from([(agent.to_string(), 102u64)]);

        let readback = readback_agents(&selection, &records, &baseline, &observed);
        assert_eq!(readback.len(), 1);
        assert_eq!(readback[0].observed, Some(2));
        assert_eq!(
            readback[0].verdict,
            crate::chaos::summary::ReadbackVerdict::Consistent
        );
    }
}
