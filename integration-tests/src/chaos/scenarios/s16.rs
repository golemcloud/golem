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

//! S16 — key-value PostgreSQL outage recovery (GOL-379).
//!
//! The first scenario in the suite that breaks something the platform depends
//! on rather than something the platform *is*. Every fault before this one
//! removed a golem process or a link between two of them, and in each case some
//! part of the cluster stayed healthy and could be compared against. Here the
//! executors keep running, keep their shards, keep answering the shard-manager,
//! and simply cannot reach the database underneath them.
//!
//! ## What the fault takes away
//!
//! More than the name suggests. Reading the golem-dev executor deployment, the
//! key-value Aurora cluster carries four things:
//!
//! * promises,
//! * the running-workers set,
//! * user-defined key-value data,
//! * and the scheduler, in its own schema on the same cluster.
//!
//! The oplog lives on a different Aurora cluster and the worker-status hot
//! cache lives in Redis, and the partition touches neither. That combination is
//! what makes the scenario worth running: the platform keeps the ability to
//! *record* what it did and loses the ability to know *what it is doing*.
//!
//! It also means no stream here is a control group. A durable increment needs
//! the running-workers set before it can start, so `durable` degrades along
//! with everything else and must not be read as untouched.
//!
//! ## The control is the baseline, not another pod
//!
//! S1 and S3 keep executors on the healthy side of the cut and read the verdict
//! off the disagreement between the two groups. There is no healthy side here:
//! all three executors share one cluster. So the comparison runs along time
//! instead, and every stream is measured against its own before-fault rate. See
//! [`crate::chaos::outage`] for what that costs and what it still answers.
//!
//! The first question it has to answer is whether the outage landed at all. A
//! partition that failed to take hold produces a report full of healthy numbers
//! and no error anywhere, which is the worst artifact this suite can produce,
//! so [`OutageViolation::OutageNotObserved`] is a named finding rather than
//! something a reader is left to infer from the cells.
//!
//! ## Why the scheduled stream is driven separately
//!
//! The mixed workload's scheduled stream registers through `schedule_poll_at`,
//! which increments a counter and records nothing else. That is enough to ask
//! "did every registration eventually fire" and useless for asking "how late".
//! Scheduler lag is one of the two things GOL-379 asks this run to record, and
//! the only place a due time survives is the target's own fire log, so S16
//! drives the token-carrying registration loop from [`crate::chaos::scheduled`]
//! instead and leaves `scheduledAgents` at zero in the mixed workload. Setting
//! both is refused at load time — see `ScenarioConfig::require_storage`.
//!
//! One consequence worth stating plainly for anyone reading the result: the
//! fire account's `group` axis is degenerate here. S16 kills no executor, so
//! every target is reported as `elsewhere` and only the `window` axis carries
//! information. The delays to read are the `during-fault` and `after-fault`
//! cells against `before-fault`.
//!
//! ## What fails the run
//!
//! The same narrow set S3 fails on, and for the same reason: this suite reports
//! rather than judges, and the bar for failing outright is "the run produced
//! nothing worth interpreting".
//!
//! * A token-level scheduled-fire violation. An accepted action that never ran,
//!   ran twice, or ran after being refused is a statement about one named
//!   registration with no band of doubt around it.
//! * A key that the exactly-once probe shows executed twice.
//! * A workload that never confirmed anything at all.
//!
//! Everything the storage account finds is loud without being fatal: it lands
//! in `attention`, which CI annotates. An outage that did not land and a stream
//! that never came back are both things a human has to look at, and neither is
//! improved by turning the job red.
//!
//! Run-by-run findings live in the S16 runbook in golem-cloud, not here.

use crate::chaos::fires::{FaultWindow, ScheduleFireReport};
use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use crate::chaos::outage::StorageOutageReport;
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::probe;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, WARMUP_SETTLE, build_result, exactly_once_termination,
    read_counters, readback_for, signal_termination, snapshot_routing, wait_for_settled_routing,
    write_outputs,
};
use crate::chaos::scheduled;
use crate::chaos::signal::{BaselineReady, FaultSignals};
use crate::chaos::summary::{
    AgentReadback, ChaosSummary, ExactlyOnceReport, Note, TerminationReason,
};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig, ScheduledConfig};
use chrono::Utc;
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeSet;
use std::time::Duration;
use tracing::{info, warn};

/// Extra quiet after the last scheduled action is due, before anything is read
/// back.
///
/// The rest of the settle is derived from the configuration, the same way S10
/// derives it: the final registration falls due one `lead` after the workload
/// stops, and an action the outage delayed can cost up to one lease budget on
/// top. Reading before that has elapsed would report actions as lost that were
/// merely late, which is the one mistake this scenario cannot afford — a
/// storage outage is precisely the thing that makes work late.
const SETTLE_MARGIN: Duration = Duration::from_secs(30);

/// How many targets to sample after the baseline to prove actions are firing at
/// all.
///
/// A smoke test rather than a measurement: if the scheduling path is broken
/// every target is equally broken, and the point is to fail before spending the
/// fault window on a run that would report a clean account of nothing.
const FIRE_PROOF_SAMPLE: usize = 5;

pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let workload_config = config.require_workload()?;
    let scheduled_config = config.require_scheduled()?;
    let storage_config = config.require_storage()?;
    let history = OperationHistory::new(ScenarioCode::S16.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S16);

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

    let targets: Vec<String> = (0..scheduled_config.targets)
        .map(|index| ctx.schedule_target_name(index))
        .collect();

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut attention_extra: Vec<Note> = Vec::new();

    // Every early return below goes through `finish`, so an abort produces the
    // same artifact shape as a completed run, with fewer phases filled in.
    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $fires:expr, $outage:expr, $exactly:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                $readback,
                routing_snapshots.clone(),
                fault_injected_at,
            );
            summary.absorb(attention_extra.clone());
            if let Some(report) = $fires {
                summary = summary.with_schedule_fires(report);
            }
            if let Some(report) = $outage {
                summary = summary.with_storage_outage(report);
            }
            if let Some(report) = $exactly {
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
    routing_snapshots.push(snapshot_routing(deps, "before-warmup").await);
    attention_extra.push(wait_for_settled_routing(deps, &mut routing_snapshots).await);
    info!(
        "S16: warming {} schedule emitters and targets before the baseline",
        targets.len()
    );
    let warmed = scheduled::warm(&ctx, &targets).await;
    info!("S16: warmed {warmed} agents, settling {WARMUP_SETTLE:?}");
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S16: baseline phase, mixed workload at {} ops/s plus {} schedule targets, for {:?}",
        workload_config.rate_per_sec,
        targets.len(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let mixed = workload::start(ctx.clone(), workload_config);
    let schedules = scheduled::start(ctx.clone(), &targets, scheduled_config);
    tokio::time::sleep(config.phases.baseline()).await;
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    // Both handles have to be stopped on every exit path, and forgetting one
    // leaves its emitters submitting into a history that has already been
    // snapshotted.
    macro_rules! stop_workloads {
        () => {{
            mixed.stop().await;
            schedules.stop().await;
        }};
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        // Taking a database away from a workload that never worked would
        // measure nothing. Stop before touching the cluster.
        warn!("S16: baseline produced no confirmed operations, aborting before injection");
        stop_workloads!();
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no operation succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new(),
            None,
            None,
            None
        );
    }

    // Registering is not firing. The scheduler is one of the three mechanisms
    // this scenario is about, and a platform that accepted every registration
    // and ran none of them would otherwise reach read-back and report a
    // flawless account of a mechanism that never worked.
    let sampled = sample_fire_count(&ctx, &targets).await;
    if sampled == 0 {
        warn!("S16: {baseline_operations} operations confirmed and no scheduled action has fired");
        stop_workloads!();
        let records = history.snapshot();
        finish!(
            TerminationReason::StreamNeverSucceeded {
                stream: Stream::Scheduled.to_string(),
            },
            &records,
            Vec::new(),
            None,
            None,
            None
        );
    }
    info!(
        "S16: baseline complete ({baseline_operations} confirmed ops, {sampled} fires across a \
         sample of {} targets), signalling readiness",
        FIRE_PROOF_SAMPLE.min(targets.len())
    );

    // ── Signal: ready for the fault ─────────────────────────────────────────
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S16.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        // Nothing to aim. The partition is between every executor and one
        // endpoint outside the cluster, so there is no pod for the driver to
        // choose and no ownership to verify.
        fault_target: None,
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S16: no fault-injected signal arrived: {e}");
            stop_workloads!();
            let records = history.snapshot();
            finish!(
                signal_termination(&e),
                &records,
                Vec::new(),
                None,
                None,
                None
            );
        }
    };
    info!(
        "S16: fault {} ({} on {}) reported active at {}, {} is now unreachable from the executors",
        injected.fault_id,
        injected.kind,
        injected.target,
        injected.injected_at,
        storage_config.endpoint
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S16: no fault-recovered signal arrived: {e}");
            stop_workloads!();
            let records = history.snapshot();
            finish!(
                signal_termination(&e),
                &records,
                Vec::new(),
                None,
                None,
                None
            );
        }
    };
    info!(
        "S16: fault cleared at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S16: recovery phase, running for a further {:?}",
        config.phases.recovery()
    );
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    tokio::time::sleep(config.phases.recovery()).await;

    let skipped = schedules.skipped();
    // Stopping waits for in-flight operations to record themselves rather than
    // cancelling them: an operation cancelled mid-flight is one the history
    // cannot classify, and during a storage outage those are exactly the
    // interesting ones.
    stop_workloads!();

    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    if skipped > 0 {
        attention_extra.push(Note::attention(format!(
            "S16 skipped {skipped} registration ticks because targets still had their budget of \
             {} in flight — the offered rate was clamped by the platform, so the phase counts \
             understate what the run intended to submit",
            scheduled::MAX_IN_FLIGHT_PER_TARGET
        )));
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);

    // ── Read-back ───────────────────────────────────────────────────────────
    let settle = settle_before_readback(scheduled_config);
    info!("S16: letting the last actions fall due and fire, {settle:?} before read-back");
    tokio::time::sleep(settle).await;

    let records = history.snapshot();
    let logs = scheduled::read_logs(&ctx, &targets).await;
    // Archived alongside the operations, not just reduced into the report: the
    // reduction is the part a later ticket is most likely to want to redo.
    history.record_fire_logs(logs.clone());

    let fault_window = fault_injected_at.map(|injected_at| FaultWindow {
        injected_at,
        recovered_at: fault_recovered_at,
    });

    let fires = ScheduleFireReport::build(
        &records,
        &logs,
        scheduled_config.lead(),
        fault_window,
        // Empty on purpose: nothing was killed, so every target belongs to the
        // report's `elsewhere` group and only its window axis carries meaning.
        &BTreeSet::new(),
        scheduled_config.lease_budget(),
    );
    info!(
        "S16: scheduled-fire account — {} registrations accepted, {} fired once, {} \
         inconclusive, {} unverifiable, {} findings",
        fires.registrations_confirmed,
        fires.fired_once,
        fires.inconclusive,
        fires.unverifiable,
        fires.findings.len()
    );

    let outage = StorageOutageReport::build(
        &records,
        fault_window,
        &storage_config.endpoint,
        storage_config.outage_quiet_floor_percent,
        storage_config.recovery_budget(),
    );
    info!(
        "S16: storage account — the least quiet stream answered nothing for {:?}% of the fault \
         window (floor {}%) while {} was unreachable, holding {:?}% of baseline throughput, {} \
         findings",
        outage.quietest_stream_percent,
        outage.outage_quiet_floor_percent,
        outage.endpoint,
        outage.share_of_baseline_percent,
        outage.findings.len()
    );
    for finding in &outage.findings {
        warn!("S16: {}: {}", finding.violation, finding.detail);
    }

    let readback = read_back(&ctx, &records, workload_config, &logs).await;

    // The idempotency half of the account. A key that timed out while the
    // database was gone may or may not have executed; re-invoking it under the
    // same key says which, because a platform that stored the result replays it
    // and one that did not runs the work again.
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
        "S16: exactly-once account — {} keys checked, {} with a final result, {} recovered by \
         the probe, {} findings",
        exactly_once.keys_checked,
        exactly_once.keys_with_final_result,
        exactly_once.keys_recovered_by_probe,
        exactly_once.findings.len()
    );

    let reason = termination(&fires, &exactly_once, &records);

    finish!(
        reason,
        &records,
        readback,
        Some(fires),
        Some(outage),
        Some(exactly_once)
    );
}

/// Why the run stopped.
///
/// Kept apart from [`run`] so the precedence is testable without a cluster. The
/// order matters: a token-level fire violation and a duplicated key are both
/// statements about one named thing, and either is worth more to a reader than
/// "the workload never worked", which is a statement about the run rather than
/// about the platform.
fn termination(
    fires: &ScheduleFireReport,
    exactly_once: &ExactlyOnceReport,
    records: &[OperationRecord],
) -> TerminationReason {
    if fires.has_violations() {
        return TerminationReason::ScheduledFireViolated {
            findings: fires.findings.len() as u64,
            first: fires
                .findings
                .first()
                .map(|f| format!("{} on token {}", f.violation, f.token))
                .unwrap_or_default(),
        };
    }
    if let Some(reason) = exactly_once_termination(exactly_once) {
        return reason;
    }
    if records.iter().all(|r| r.outcome != Outcome::Confirmed) {
        return TerminationReason::StreamNeverSucceeded {
            stream: Stream::Durable.to_string(),
        };
    }
    TerminationReason::Completed
}

/// How long to wait after the workload stops before reading anything back.
///
/// Same derivation as S10's, and load-bearing for the same reason: the last
/// registration falls due one `lead` after the loop stops, and a delayed action
/// can cost a lease budget on top of that.
fn settle_before_readback(config: &ScheduledConfig) -> Duration {
    config.lead() + config.lease_budget() + SETTLE_MARGIN
}

/// Reads durable state back for the streams that keep a count.
///
/// The durable counters come from the mixed workload's own agents; the
/// scheduled counts come from the fire logs that were already read, because
/// re-reading `polls` would be a second round trip for a number the log already
/// carries.
async fn read_back(
    ctx: &WorkloadContext,
    records: &[OperationRecord],
    config: &crate::chaos::WorkloadConfig,
    logs: &[crate::chaos::history::TargetFireLog],
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

    for log in logs {
        let scoped = records
            .iter()
            .filter(|r| r.stream == Stream::Scheduled && r.agent == log.agent);
        let observed = match (log.polls, &log.error) {
            (Some(polls), _) => Ok(polls),
            (None, Some(error)) => Err(error.clone()),
            (None, None) => Err(format!("target {} reported no poll count", log.agent)),
        };
        readback.extend(readback_for(
            Stream::Scheduled,
            &log.agent,
            scoped,
            observed,
        ));
    }

    readback
}

/// Reads the fire count of a few targets, to prove actions are firing at all.
async fn sample_fire_count(ctx: &WorkloadContext, targets: &[String]) -> u64 {
    let mut total = 0u64;
    for target in targets.iter().take(FIRE_PROOF_SAMPLE) {
        match workload::read_polls(ctx, target).await {
            Ok(polls) => total += polls,
            Err(e) => warn!("S16: could not sample fires on {target}: {e}"),
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::{AttemptRecord, FireRecord, TargetFireLog};
    use crate::chaos::outage::{OutageFinding, OutageViolation};
    use chrono::{DateTime, TimeDelta};
    use test_r::test;

    const TARGET: &str = "chaos-s16-schedule-target-0000";
    const LEAD: Duration = Duration::from_secs(10);
    const BUDGET: Duration = Duration::from_secs(240);

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn registration(token: &str, outcome: Outcome) -> OperationRecord {
        OperationRecord {
            op_id: 0,
            stream: Stream::Scheduled,
            phase: Phase::Baseline,
            agent: TARGET.to_string(),
            method: "schedule_fire_at".to_string(),
            idempotency_key: token.to_string(),
            submitted_at: t0(),
            completed_at: Some(t0() + TimeDelta::milliseconds(20)),
            attempts: 1,
            outcome,
            duration_ms: 20,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: vec![AttemptRecord {
                attempt: 1,
                started_at: t0(),
                duration_ms: 20,
                returned_value: None,
                succeeded: outcome == Outcome::Confirmed,
                error_class: None,
                error: None,
            }],
        }
    }

    fn log(fired: &[&str]) -> TargetFireLog {
        TargetFireLog {
            agent: TARGET.to_string(),
            polls: Some(fired.len() as u64),
            fires: fired
                .iter()
                .map(|token| FireRecord {
                    token: (*token).to_string(),
                    scheduled_at: t0() + TimeDelta::seconds(10),
                    observed_at: t0() + TimeDelta::seconds(11),
                })
                .collect(),
            error: None,
        }
    }

    fn fires(records: &[OperationRecord], logs: &[TargetFireLog]) -> ScheduleFireReport {
        ScheduleFireReport::build(records, logs, LEAD, None, &BTreeSet::new(), BUDGET)
    }

    /// The empty account: nothing registered, nothing fired, nothing to say.
    fn clean_exactly_once() -> ExactlyOnceReport {
        ExactlyOnceReport::build(
            &[],
            &[],
            Stream::Durable,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
        )
    }

    /// A registration the platform accepted whose action never ran is a
    /// statement about one named token, and it outranks every aggregate.
    #[test]
    fn a_token_that_never_fired_fails_the_run() {
        let records = vec![registration("token-a", Outcome::Confirmed)];
        let reason = termination(
            &fires(&records, &[log(&[])]),
            &clean_exactly_once(),
            &records,
        );

        assert!(
            matches!(reason, TerminationReason::ScheduledFireViolated { .. }),
            "expected a fire violation, got {reason:?}"
        );
    }

    /// A fire violation outranks "the workload never worked", because it says
    /// something about the platform and the other says something about the run.
    #[test]
    fn a_fire_violation_outranks_a_workload_that_never_confirmed_anything() {
        // Rejected registrations mean nothing confirmed anywhere, and a fire
        // that happened anyway is the sharpest thing this suite can find.
        let records = vec![registration("token-a", Outcome::Rejected)];
        let reason = termination(
            &fires(&records, &[log(&["token-a"])]),
            &clean_exactly_once(),
            &records,
        );

        assert!(
            matches!(reason, TerminationReason::ScheduledFireViolated { .. }),
            "expected a fire violation, got {reason:?}"
        );
    }

    /// A run where nothing confirmed at all says nothing about resilience, so
    /// it fails rather than reporting a clean account of a workload that never
    /// worked.
    #[test]
    fn a_workload_that_never_confirmed_anything_fails_the_run() {
        let records = vec![registration("token-a", Outcome::Indeterminate)];
        let reason = termination(
            &fires(&records, &[log(&["token-a"])]),
            &clean_exactly_once(),
            &records,
        );

        assert!(
            matches!(reason, TerminationReason::StreamNeverSucceeded { .. }),
            "expected a never-succeeded reason, got {reason:?}"
        );
    }

    /// The healthy shape completes, and the storage account's own findings do
    /// not change that: an outage that failed to land is loud in `attention`
    /// and deliberately not fatal.
    #[test]
    fn a_healthy_run_completes() {
        let records = vec![registration("token-a", Outcome::Confirmed)];
        let reason = termination(
            &fires(&records, &[log(&["token-a"])]),
            &clean_exactly_once(),
            &records,
        );

        assert_eq!(reason, TerminationReason::Completed);
    }

    /// The storage account's findings never reach the termination reason. An
    /// outage that failed to land is the loudest thing S16 can report and it is
    /// deliberately not fatal: turning the job red would say the platform did
    /// something wrong, and what actually went wrong is the experiment.
    #[test]
    fn a_storage_finding_does_not_change_the_termination_reason() {
        let mut outage =
            StorageOutageReport::build(&[], None, "db.example", 15.0, Duration::from_secs(120));
        outage.findings.push(OutageFinding {
            violation: OutageViolation::OutageNotObserved,
            stream: None,
            detail: "kept working".to_string(),
        });
        assert!(outage.has_findings());

        let records = vec![registration("token-a", Outcome::Confirmed)];
        let reason = termination(
            &fires(&records, &[log(&["token-a"])]),
            &clean_exactly_once(),
            &records,
        );
        assert_eq!(reason, TerminationReason::Completed);
    }
}
