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

//! S10 — executor crash during scheduled-action fire (GOL-378).
//!
//! Every other scenario in this suite kills an executor while the driver is
//! holding a connection to it. S10 kills one while it is holding a *promise*:
//! work the platform accepted, acknowledged, and undertook to run later. Nobody
//! is waiting on the other end of a socket for it, which is exactly why it is
//! worth testing separately. A dropped invocation shows up as a failed request.
//! A dropped scheduled action shows up as nothing at all.
//!
//! ## The two windows, and which one a kill reliably lands in
//!
//! An executor's scheduler claims the actions that are due for the shards it
//! owns, leases each one, enqueues the invocation, and acknowledges. That
//! claim-to-acknowledge window is milliseconds wide: the action itself is a
//! near-no-op, and once the invocation is enqueued it is durable in the target
//! agent's oplog and covered by ordinary worker recovery rather than by the
//! lease. Killing an executor inside it is possible, not aimable, and its
//! signature is a fire delayed by roughly the lease TTL. The run reports when
//! that happens instead of claiming to have arranged it.
//!
//! The window a kill does land in, every time, is the wide one. At any instant
//! several hundred actions are registered and not yet due, and the shards they
//! belong to are owned by an executor that is about to stop existing. Nothing
//! claims them until the shards move, so the delay this scenario measures is
//! dominated by shard reassignment rather than by lease expiry. Both end in the
//! same question: does an accepted action still run, exactly once, once the
//! cluster has finished rearranging itself.
//!
//! ## Why the target agent records tokens rather than counting
//!
//! S12 already counts scheduled fires and compares the total against what the
//! driver registered. That is enough to notice that *something* went wrong
//! across a whole population, and useless for saying what. Here every
//! registration carries its own idempotency key into the scheduled action, and
//! the target agent records it when the action runs. Pairing tokens turns both
//! failures into statements about one named action: this registration, accepted
//! at this time, due at this time, never ran. See [`crate::chaos::fires`].
//!
//! ## Why the kill is aimed, and why every target is still driven
//!
//! The driver picks the executor owning the largest share of targets and names
//! it in the readiness signal, the same way S8 does. Without that, `mode: one`
//! would pick a pod at random and the run could not say which actions were
//! supposed to be disturbed.
//!
//! Unlike S8, though, the targets it does *not* own keep running too. They are
//! the control group: on a two-executor cluster roughly half the population is
//! never touched, and reporting one percentile over both would let a lease
//! recovery that took its full TTL hide behind the half that was never
//! disturbed.
//!
//! ## What the run measures the kill against
//!
//! The driver cannot see a claim, and does not pretend to. Every target
//! re-registers on a fixed cadence at a fixed lead, so actions arrive into
//! every part of the cycle continuously and there is always a population
//! registered but not yet due. What the run then records is how large that
//! population actually was at the instant the pod died, and how much of it was
//! on the pod — measured from the history, not assumed from the cadence,
//! because a platform that had slowed down would have registered fewer than the
//! arithmetic says. A kill that caught none of it is reported as a warning: a
//! clean account of a mechanism that was never disturbed is not evidence.
//!
//! ## What fails the run
//!
//! Only the three token-level violations in [`ScheduleFireReport`]. Fire delay
//! is reported against the configured lease budget as SLO evidence, not
//! asserted: how much a lease recovery may cost is a judgement, and the number
//! that matters is in the result either way.
//!
//! ## Reading a fire delay: registration first, scheduler second
//!
//! A large fire delay does not necessarily mean the scheduler was slow. The
//! registering invocation mints its action's due time *before* the call goes
//! out, so a registration that stalls in the client describes an action that was
//! already overdue when the platform first heard about it. Those fire instantly
//! and correctly, and would still arrive in the percentiles as minutes late.
//!
//! [`crate::chaos::fires`] holds them out of the scheduler-delay cells and
//! reports them as `overdue_on_arrival` instead. Two things follow for anyone
//! reading a result:
//!
//! * Check the registration latencies before concluding anything about the
//!   scheduler. A client-side stall and a lease recovery look identical in a
//!   delay percentile and have nothing to do with each other.
//! * Read the `overdue_on_arrival` count together with its worst case, never
//!   alone. The classification catches any registration slower than the
//!   configured lead, so a stall shrinking from minutes to seconds moves entries
//!   *into* the bucket rather than out of it: the count can rise while the
//!   platform gets strictly better.
//!
//! Run-by-run findings live in the S10 runbook in golem-cloud, not here.

use crate::chaos::fires::{FaultWindow, ScheduleFireReport};
use crate::chaos::history::{
    OperationHistory, OperationRecord, Outcome, Phase, Stream, TargetFireLog,
};
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, WARMUP_SETTLE, build_result, readback_for, signal_termination,
    snapshot_routing, wait_for_settled_routing, write_outputs,
};
use crate::chaos::scheduled::{self, ScheduledSelection};
use crate::chaos::signal::{BaselineReady, FaultSignals, FaultTarget};
use crate::chaos::summary::{AgentReadback, ChaosSummary, Note, TerminationReason};
use crate::chaos::workload::{self, PhaseMarker, WorkloadContext};
use crate::chaos::{ScenarioCode, ScenarioConfig, ScheduledConfig};
use chrono::{DateTime, TimeDelta, Utc};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeSet;
use std::time::Duration;
use tracing::{info, warn};

/// Extra quiet after the last registration's action is due, before the fire
/// logs are read.
///
/// The rest of the settle is derived from the configuration rather than fixed:
/// the final registration falls due one `lead` after the workload stops, and if
/// its executor died holding the claim, the recovery costs up to one lease
/// budget on top. Reading before that elapsed would report actions as lost that
/// were merely late, which is the one mistake this scenario cannot afford.
const SETTLE_MARGIN: Duration = Duration::from_secs(30);

/// How many targets to sample after the baseline to prove actions are firing at
/// all.
///
/// A handful, because this is a smoke test rather than a measurement: if the
/// scheduling path is broken, every target is equally broken, and the point is
/// to fail before spending the fault window on a run that would report a clean
/// account of nothing.
const FIRE_PROOF_SAMPLE: usize = 5;

pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let scheduled_config = config.require_scheduled()?;
    let history = OperationHistory::new(ScenarioCode::S10.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S10);

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

    let targets: Vec<String> = (0..scheduled_config.targets)
        .map(|index| ctx.schedule_target_name(index))
        .collect();

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut selection: Option<ScheduledSelection> = None;
    let mut attention_extra: Vec<Note> = Vec::new();

    macro_rules! finish {
        ($reason:expr, $records:expr, $readback:expr, $fires:expr) => {{
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
                    scheduled_selection: selection.clone(),
                    promise_selection: None,
                    isolation_selection: None,
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
        "S10: warming {} emitters and targets before the baseline",
        targets.len()
    );
    let warmed = scheduled::warm(&ctx, &targets).await;
    info!("S10: warmed {warmed} agents, settling {WARMUP_SETTLE:?}");
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Aim the fault ───────────────────────────────────────────────────────
    // Before the baseline, because a run that cannot be aimed should not spend
    // a maintenance window proving it.
    let chosen = match scheduled::select(&ctx, deps, &targets).await {
        Ok(chosen) => chosen,
        Err(e) => {
            warn!("S10: could not aim the fault at an executor: {e:#}");
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

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S10: baseline phase, {} targets registering every {:?} at a {:?} lead, for {:?}",
        targets.len(),
        scheduled_config.interval(),
        scheduled_config.lead(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = scheduled::start(ctx.clone(), &targets, scheduled_config);
    tokio::time::sleep(config.phases.baseline()).await;
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S10: baseline registered nothing, aborting before injection");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no scheduled registration succeeded during the baseline phase".to_string(),
            },
            &records,
            Vec::new(),
            None
        );
    }

    // Registering is not firing. A platform that accepted every registration and
    // scheduled none of them would otherwise reach read-back and report a
    // flawless account of a mechanism that never ran.
    let sampled = sample_fire_count(&ctx, &targets).await;
    if sampled == 0 {
        warn!("S10: {baseline_operations} registrations accepted and no action has fired");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::StreamNeverSucceeded {
                stream: Stream::Scheduled.to_string(),
            },
            &records,
            Vec::new(),
            None
        );
    }
    info!(
        "S10: baseline complete ({baseline_operations} registrations, {sampled} fires across a \
         sample of {} targets)",
        FIRE_PROOF_SAMPLE.min(targets.len())
    );

    // ── Verify ownership, then signal ───────────────────────────────────────
    if let Err(e) = scheduled::verify_ownership(&ctx, deps, &chosen).await {
        warn!("S10: target ownership no longer holds, refusing to inject: {e:#}");
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
        "S10: signalling readiness with fault target {} ({} of {} targets on it)",
        chosen.pod_address,
        chosen.on_pod.len(),
        targets.len()
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S10.as_str().to_string(),
        ready_at: Utc::now(),
        baseline_operations,
        fault_target: Some(FaultTarget {
            pod_address: chosen.pod_address.clone(),
            pod_ip: chosen.pod_ip.clone(),
            owned_agents: chosen.on_pod.clone(),
        }),
    })?;

    // ── Fault ───────────────────────────────────────────────────────────────
    let injected = match signals.await_fault_injected(config.signal_timeout()).await {
        Ok(injected) => injected,
        Err(e) => {
            warn!("S10: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S10: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    // How much work was actually in the window the scenario is about. Measured
    // from the history rather than assumed from the cadence, because a platform
    // that had slowed down would have registered fewer than the arithmetic says.
    let on_pod: BTreeSet<String> = chosen.on_pod.iter().cloned().collect();
    let pending = pending_at_injection(
        &history.snapshot(),
        injected.injected_at,
        scheduled_config.lead(),
        &on_pod,
    );
    attention_extra.push(pending.note());
    info!("S10: {}", pending.describe());

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S10: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, Vec::new(), None);
        }
    };
    info!(
        "S10: fault cleared at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S10: recovery phase, registering for a further {:?}",
        config.phases.recovery()
    );
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    tokio::time::sleep(config.phases.recovery()).await;
    let skipped = handle.skipped();
    handle.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    if skipped > 0 {
        attention_extra.push(Note::attention(format!(
            "S10 skipped {skipped} registration ticks because targets still had their budget \
             of {} in flight — the offered rate was clamped by the platform, so the phase \
             counts understate what the run intended to submit",
            scheduled::MAX_IN_FLIGHT_PER_TARGET
        )));
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);

    // ── Account ─────────────────────────────────────────────────────────────
    let settle = settle_before_readback(scheduled_config);
    info!("S10: letting the last actions fall due and fire, {settle:?} before read-back");
    tokio::time::sleep(settle).await;

    let records = history.snapshot();
    let logs = scheduled::read_logs(&ctx, &targets).await;
    // Archived alongside the operations, not just reduced into the report. The
    // first S10 run needed a correction to its delay percentiles that could not
    // be applied afterwards, because only the reduced numbers had been kept.
    history.record_fire_logs(logs.clone());

    let report = ScheduleFireReport::build(
        &records,
        &logs,
        scheduled_config.lead(),
        fault_injected_at.map(|injected_at| FaultWindow {
            injected_at,
            recovered_at: fault_recovered_at,
        }),
        &on_pod,
        scheduled_config.lease_budget(),
    );
    info!(
        "S10: scheduled-fire account — {} registrations accepted, {} fired once, {} never \
         fired, {} inconclusive, {} unverifiable, {} findings",
        report.registrations_confirmed,
        report.fired_once,
        report
            .findings
            .iter()
            .filter(|f| f.violation == crate::chaos::fires::FireViolation::NeverFired)
            .count(),
        report.inconclusive,
        report.unverifiable,
        report.findings.len()
    );
    if let Some(p99) = report.fault_window_p99_ms() {
        info!(
            "S10: fire delay p99 during the fault, on the killed executor's targets: {p99}ms \
             against a {}ms lease budget",
            report.lease_budget_ms
        );
    }

    // The count-based read-back as well, on the same read. It cannot localise
    // anything the token pairing does not, but it is the view every other
    // scenario reports and a disagreement between the two would itself be worth
    // knowing about.
    let readback = readback_from_polls(&records, &logs);

    let reason = if report.has_violations() {
        TerminationReason::ScheduledFireViolated {
            findings: report.findings.len() as u64,
            first: report
                .findings
                .first()
                .map(|f| format!("{} on token {}", f.violation, f.token))
                .unwrap_or_default(),
        }
    } else if report.fired_once == 0 {
        TerminationReason::StreamNeverSucceeded {
            stream: Stream::Scheduled.to_string(),
        }
    } else {
        TerminationReason::Completed
    };

    finish!(reason, &records, readback, Some(report));
}

/// How long to wait after the workload stops before reading the fire logs.
fn settle_before_readback(config: &ScheduledConfig) -> Duration {
    config.lead() + config.lease_budget() + SETTLE_MARGIN
}

/// Actions that were registered but not yet due when the executor died.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingAtInjection {
    pub total: u64,
    pub on_killed_executor: u64,
}

impl PendingAtInjection {
    /// Whether the kill landed anywhere near the mechanism under test.
    ///
    /// A run that caught nothing pending proves nothing however clean the rest
    /// of its numbers look, which is the one thing here a human has to act on.
    pub fn needs_attention(&self) -> bool {
        self.total == 0
    }

    /// The same line as [`Self::describe`], classified.
    pub fn note(&self) -> Note {
        Note::leveled(self.needs_attention(), self.describe())
    }

    /// The line an operator needs in order to know whether the kill landed
    /// anywhere near the mechanism under test.
    pub fn describe(&self) -> String {
        if self.total == 0 {
            return "WARNING: no scheduled action was between registration and its due time \
                    when the executor died, so this run says nothing about lease recovery"
                .to_string();
        }
        format!(
            "S10 killed the executor with {} scheduled actions registered and not yet due, {} \
             of them on targets it owned",
            self.total, self.on_killed_executor
        )
    }
}

/// Counts the actions in the claim window at the moment of injection.
///
/// Registered before the kill, due after it, and not definitively refused. A
/// refused registration is not work the platform owes anything, so counting it
/// here would overstate what the kill was aimed at.
pub fn pending_at_injection(
    records: &[OperationRecord],
    injected_at: DateTime<Utc>,
    lead: Duration,
    on_killed_executor: &BTreeSet<String>,
) -> PendingAtInjection {
    let lead = TimeDelta::from_std(lead).unwrap_or(TimeDelta::zero());
    let mut pending = PendingAtInjection {
        total: 0,
        on_killed_executor: 0,
    };

    for record in records
        .iter()
        .filter(|r| r.stream == Stream::Scheduled && r.outcome != Outcome::Rejected)
        .filter(|r| r.submitted_at <= injected_at && r.submitted_at + lead >= injected_at)
    {
        pending.total += 1;
        if on_killed_executor.contains(&record.agent) {
            pending.on_killed_executor += 1;
        }
    }
    pending
}

/// Per-target read-back from `polls`, the view every other scenario reports.
fn readback_from_polls(records: &[OperationRecord], logs: &[TargetFireLog]) -> Vec<AgentReadback> {
    logs.iter()
        .filter_map(|log| {
            let scoped = records
                .iter()
                .filter(|r| r.stream == Stream::Scheduled && r.agent == log.agent);
            let observed = match (log.polls, &log.error) {
                (Some(polls), _) => Ok(polls),
                (None, Some(error)) => Err(error.clone()),
                (None, None) => Err(format!("target {} reported no poll count", log.agent)),
            };
            readback_for(Stream::Scheduled, &log.agent, scoped, observed)
        })
        .collect()
}

/// Reads the fire count of a few targets, to prove actions are firing at all.
async fn sample_fire_count(ctx: &WorkloadContext, targets: &[String]) -> u64 {
    let mut total = 0u64;
    for target in targets.iter().take(FIRE_PROOF_SAMPLE) {
        match workload::read_polls(ctx, target).await {
            Ok(polls) => total += polls,
            Err(e) => warn!("S10: could not sample fires on {target}: {e}"),
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::AttemptRecord;
    use crate::chaos::summary::NoteLevel;
    use test_r::test;

    fn at(offset_secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + offset_secs, 0).unwrap()
    }

    fn registration(agent: &str, submitted_at: DateTime<Utc>, outcome: Outcome) -> OperationRecord {
        OperationRecord {
            op_id: 0,
            stream: Stream::Scheduled,
            phase: Phase::Baseline,
            agent: agent.to_string(),
            method: "schedule_fire_at".to_string(),
            idempotency_key: format!("{agent}-key"),
            submitted_at,
            completed_at: Some(submitted_at),
            attempts: 1,
            outcome,
            duration_ms: 10,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: vec![AttemptRecord {
                attempt: 1,
                started_at: submitted_at,
                duration_ms: 10,
                returned_value: None,
                succeeded: outcome == Outcome::Confirmed,
                error_class: None,
                error: None,
            }],
        }
    }

    const LEAD: Duration = Duration::from_secs(10);

    /// The population the scenario is about: registered before the kill, due
    /// after it.
    #[test]
    fn only_actions_still_inside_their_lead_count_as_pending() {
        let killed = BTreeSet::from(["target-0".to_string()]);
        let records = vec![
            // Due at 105, after the kill at 100.
            registration("target-0", at(95), Outcome::Confirmed),
            // Due at 99, so it had already fired.
            registration("target-0", at(89), Outcome::Confirmed),
            // Registered after the kill.
            registration("target-0", at(101), Outcome::Confirmed),
        ];
        let pending = pending_at_injection(&records, at(100), LEAD, &killed);
        assert_eq!(pending.total, 1);
        assert_eq!(pending.on_killed_executor, 1);
    }

    /// The split is what makes the percentile readable, so it has to be counted
    /// here too rather than inferred later.
    #[test]
    fn pending_actions_are_split_by_the_executor_that_owned_them() {
        let killed = BTreeSet::from(["target-0".to_string()]);
        let records = vec![
            registration("target-0", at(95), Outcome::Confirmed),
            registration("target-1", at(95), Outcome::Confirmed),
        ];
        let pending = pending_at_injection(&records, at(100), LEAD, &killed);
        assert_eq!(pending.total, 2);
        assert_eq!(pending.on_killed_executor, 1);
    }

    /// A refusal is not work the platform owes anything, so counting it would
    /// overstate what the kill was aimed at.
    #[test]
    fn a_refused_registration_is_not_pending_work() {
        let records = vec![registration("target-0", at(95), Outcome::Rejected)];
        let pending = pending_at_injection(&records, at(100), LEAD, &BTreeSet::new());
        assert_eq!(pending.total, 0);
    }

    /// The loudest thing this scenario can say: the kill missed the mechanism
    /// entirely, so nothing about lease recovery can be read from the run.
    #[test]
    fn a_kill_that_caught_no_pending_action_says_so_rather_than_reporting_a_clean_run() {
        let pending = PendingAtInjection {
            total: 0,
            on_killed_executor: 0,
        };
        assert!(pending.describe().starts_with("WARNING"));
        assert!(pending.needs_attention());
        assert_eq!(pending.note().level, NoteLevel::Attention);
    }

    /// The same sentence on a run that landed properly is context. It is the
    /// first thing a reader wants and it is true of every healthy run, so
    /// putting it in `attention` would fire CI's annotation every time.
    #[test]
    fn a_kill_that_landed_reports_its_count_as_context() {
        let pending = PendingAtInjection {
            total: 353,
            on_killed_executor: 226,
        };
        assert!(!pending.needs_attention());
        assert_eq!(pending.note().level, NoteLevel::Context);
        assert!(pending.note().message.contains("353"));
    }

    /// The last registration falls due one lead after the workload stops, and a
    /// recovery costs up to a lease on top. Reading before that would report
    /// late actions as lost.
    #[test]
    fn the_settle_covers_a_full_lead_plus_a_full_lease_recovery() {
        let config = ScheduledConfig {
            targets: 100,
            interval_millis: 2000,
            lead_secs: 10,
            lease_budget_secs: 45,
        };
        assert_eq!(
            settle_before_readback(&config),
            Duration::from_secs(10 + 45) + SETTLE_MARGIN
        );
    }
}
