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

//! S11 — executor crash during promise completion (GOL-377).
//!
//! S8 kills an executor with invocations running on it. S10 kills one holding
//! work it promised to do later. S11 kills one holding agents that are *asleep*:
//! each waiter is suspended inside an invocation, parked on a promise, occupying
//! no thread and running no code, and something outside is about to resolve that
//! promise and expect the agent to carry on.
//!
//! ## Why this is its own scenario
//!
//! A suspended waiter is the one durable-execution state with no in-memory
//! representation to lose and no queue entry to drain. Nothing about it looks
//! like work in flight. Resuming it depends on a chain that a pod kill can break
//! in the middle: the completion is written to storage under the promise's key,
//! then the worker that owns the promise is activated so it notices. Those are
//! two steps, and only the first of them is durable.
//!
//! So the failure this scenario exists to catch is specific and quiet. A
//! completion is accepted — the caller gets a success — the write lands, and the
//! activation goes to an executor that is already dying. The promise is resolved
//! forever after, and the agent waiting on it is never told. Nothing errors,
//! nothing retries, and no count anywhere goes down.
//!
//! ## What the run measures the kill against
//!
//! Each of `waiters` agents holds exactly one promise at a time, so the number of
//! agents standing suspended when the pod dies is a known constant rather than a
//! sample. The driver records how many of them were actually parked across the
//! injection, and on which executor, measured from the history rather than
//! assumed from the cadence — a platform that had slowed down would have armed
//! fewer than the arithmetic says. A kill that caught nothing parked is reported
//! as a warning: a clean account of a mechanism that was never disturbed is not
//! evidence.
//!
//! Like S8 and S10 the kill is aimed, and like S10 every waiter keeps running.
//! The ones on other executors are the control group, which is what stops a
//! recovery that took its whole budget from hiding behind the half of the
//! population that was never touched.
//!
//! ## Two independent answers, and why both are kept
//!
//! Every round is observed twice. The driver holds the `wait` invocation open, so
//! it sees the wakeup as a returning call; the waiter writes the wakeup into its
//! own durable log, which the run reads afterwards.
//!
//! The two disagree exactly when it matters. Killing the executor takes the
//! driver's connection with it, so for the rounds this scenario is about, the
//! client's view is a broken pipe and nothing more. The agent's log is what
//! answers, and [`crate::chaos::wakeups`] is built around it.
//!
//! ## What fails the run
//!
//! Only the three token-level violations in [`WakeupReport`]. Wakeup delay is
//! reported against the configured budget as SLO evidence, not asserted: how much
//! a shard reassignment plus a worker recovery may cost is a judgement, and the
//! number is in the result either way.
//!
//! ## The waiter that answers nothing
//!
//! An unreadable agent normally means the run cannot say. For a suspended waiter
//! it can mean the opposite, because the read queues behind the invocation the
//! waiter is parked in. A waiter that stopped producing rounds during the run and
//! then answered no read is a worker still parked on a promise resolved minutes
//! ago — the defect itself, observed from two directions. The report separates
//! that from an agent that merely timed out. See [`crate::chaos::wakeups`].

use crate::chaos::history::{OperationHistory, OperationRecord, Outcome, Phase, Stream};
use crate::chaos::prep::ChaosPrepManifest;
use crate::chaos::result::{ChaosResult, PhaseWindow, Phases, RunScope};
use crate::chaos::scenarios::{
    OutputPaths, ScenarioOutcome, WARMUP_SETTLE, build_result, signal_termination,
    snapshot_routing, wait_for_settled_routing, write_outputs,
};
use crate::chaos::signal::{BaselineReady, FaultSignals, FaultTarget};
use crate::chaos::split::{self, FaultWindow, PodSplit};
use crate::chaos::summary::{ChaosSummary, Note, TerminationReason};
use crate::chaos::waiters;
use crate::chaos::wakeups::WakeupReport;
use crate::chaos::workload::{PhaseMarker, WorkloadContext};
use crate::chaos::{PromiseConfig, ScenarioCode, ScenarioConfig};
use chrono::{DateTime, Utc};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::dsl::TestDsl;
use std::collections::BTreeSet;
use std::time::Duration;
use tracing::{info, warn};

/// Extra quiet after the workload stops, before the wakeup logs are read.
///
/// The rest of the settle is derived from the configuration: the last round's
/// completion goes out one dwell after the workload stops accepting new rounds,
/// and if its waiter's executor died holding it, the resume costs up to one
/// wakeup budget on top. Reading before that elapsed would report wakeups as
/// lost that were merely late, which is the one mistake this scenario cannot
/// afford.
const SETTLE_MARGIN: Duration = Duration::from_secs(30);

/// How many waiters to sample after the baseline to prove wakeups happen at all.
///
/// A handful, because this is a smoke test rather than a measurement: if the
/// completion path is broken, every waiter is equally broken, and the point is to
/// fail before spending the fault window on a run that would report a clean
/// account of nothing.
const WAKE_PROOF_SAMPLE: usize = 5;

pub async fn run(
    config: &ScenarioConfig,
    manifest: &ChaosPrepManifest,
    deps: &BenchmarkTestDependencies,
    signals: &FaultSignals,
    outputs: &OutputPaths,
) -> anyhow::Result<ChaosResult> {
    let started_at = Utc::now();
    let promise_config = config.require_promise()?;
    let history = OperationHistory::new(ScenarioCode::S11.as_str());
    let key_prefix = crate::chaos::scenario_key_prefix(ScenarioCode::S11);

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
        // The promise component, not the counters one: S11's agents live there,
        // and a reader narrowing traces by component needs the one that was
        // actually driven.
        component_ids: vec![manifest.promise_component_id.0.to_string()],
        agent_id_prefix: key_prefix.clone(),
        idempotency_key_prefix: format!("{key_prefix}-"),
    };

    let waiter_names: Vec<String> = (0..promise_config.waiters)
        .map(|index| ctx.waiter_name(index))
        .collect();

    let mut phases = Phases::default();
    let mut routing_snapshots = Vec::new();
    let mut fault_injected_at = None;
    let mut fault_recovered_at = None;
    let mut fault_id = None;
    let mut fault_target_observed = None;
    let mut selection: Option<PodSplit> = None;
    let mut attention_extra: Vec<Note> = Vec::new();

    macro_rules! finish {
        ($reason:expr, $records:expr, $wakeups:expr) => {{
            let mut summary = ChaosSummary::build(
                $records,
                Vec::new(),
                routing_snapshots.clone(),
                fault_injected_at,
            );
            summary.absorb(attention_extra.clone());
            if let Some(report) = $wakeups {
                summary = summary.with_promise_wakeups(report);
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
                    promise_selection: selection.clone(),
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
        "S11: warming {} waiters before the baseline",
        waiter_names.len()
    );
    let warmed = waiters::warm(&ctx, &waiter_names).await;
    info!("S11: warmed {warmed} waiters, settling {WARMUP_SETTLE:?}");
    tokio::time::sleep(WARMUP_SETTLE).await;

    // ── Prove one whole round works ─────────────────────────────────────────
    // Before aiming, before the baseline, before anything that costs the window.
    // Arming and completing can both succeed while the parking in between is
    // refused, and that combination looks entirely healthy in the operation
    // totals — so the totals are not what gets asked.
    if let Err(e) = waiters::smoke_test(&ctx, promise_config.dwell()).await {
        warn!("S11: a single promise round does not work against this cluster: {e}");
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: format!("promise round smoke test failed: {e}"),
            },
            &records,
            None
        );
    }
    info!("S11: smoke round armed, parked, completed and woke");

    // ── Aim the fault ───────────────────────────────────────────────────────
    // Before the baseline, because a run that cannot be aimed should not spend a
    // maintenance window proving it.
    let chosen = match split::select(split::waiter_subject(&ctx), deps, &waiter_names).await {
        Ok(chosen) => chosen,
        Err(e) => {
            warn!("S11: could not aim the fault at an executor: {e:#}");
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
    selection = Some(chosen.clone());
    routing_snapshots.push(snapshot_routing(deps, "before-fault").await);

    // ── Baseline ────────────────────────────────────────────────────────────
    info!(
        "S11: baseline phase, {} waiters parking for {:?} a round, for {:?}",
        waiter_names.len(),
        promise_config.dwell(),
        config.phases.baseline()
    );
    phases.baseline = Some(PhaseWindow::started(Utc::now()));
    let handle = waiters::start(ctx.clone(), &waiter_names, promise_config);
    tokio::time::sleep(config.phases.baseline()).await;
    if let Some(window) = phases.baseline.as_mut() {
        window.end(Utc::now());
    }

    let baseline_operations = history.confirmed_in_phase(Phase::Baseline);
    if baseline_operations == 0 {
        warn!("S11: baseline completed no promise round, aborting before injection");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::PlatformUnreachable {
                detail: "no promise round succeeded during the baseline phase".to_string(),
            },
            &records,
            None
        );
    }

    // Completing is not waking. A platform that accepted every completion and
    // resumed nobody would otherwise reach read-back and report a flawless
    // account of a mechanism that never ran.
    let sampled = sample_wakes(&ctx, &waiter_names).await;
    if sampled == 0 {
        warn!("S11: {baseline_operations} operations accepted and no waiter has woken");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::StreamNeverSucceeded {
                stream: Stream::PromiseWait.to_string(),
            },
            &records,
            None
        );
    }
    info!(
        "S11: baseline complete ({baseline_operations} operations, {sampled} wakeups across a \
         sample of {} waiters)",
        WAKE_PROOF_SAMPLE.min(waiter_names.len())
    );

    // ── Verify ownership, then signal ───────────────────────────────────────
    if let Err(e) = split::verify_ownership(split::waiter_subject(&ctx), deps, &chosen).await {
        warn!("S11: waiter ownership no longer holds, refusing to inject: {e:#}");
        handle.stop().await;
        let records = history.snapshot();
        finish!(
            TerminationReason::FaultTargetUnverified {
                detail: format!("{e:#}"),
            },
            &records,
            None
        );
    }

    info!(
        "S11: signalling readiness with fault target {} ({} of {} waiters on it)",
        chosen.pod_address,
        chosen.on_pod.len(),
        waiter_names.len()
    );
    signals.write_baseline_ready(&BaselineReady {
        scenario_code: ScenarioCode::S11.as_str().to_string(),
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
            warn!("S11: no fault-injected signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, None);
        }
    };
    info!(
        "S11: fault {} ({} on {}) reported active at {}",
        injected.fault_id, injected.kind, injected.target, injected.injected_at
    );
    fault_injected_at = Some(injected.injected_at);
    fault_id = Some(injected.fault_id.clone());
    fault_target_observed = Some(injected.target.clone());
    ctx.phase.set(Phase::Fault);
    phases.fault = Some(PhaseWindow::started(injected.injected_at));

    let on_pod: BTreeSet<String> = chosen.on_pod.iter().cloned().collect();

    let recovered = match signals.await_fault_recovered(config.signal_timeout()).await {
        Ok(recovered) => recovered,
        Err(e) => {
            warn!("S11: no fault-recovered signal arrived: {e}");
            handle.stop().await;
            let records = history.snapshot();
            finish!(signal_termination(&e), &records, None);
        }
    };
    info!(
        "S11: fault cleared at {} ({})",
        recovered.recovered_at, recovered.termination_reason
    );
    fault_recovered_at = Some(recovered.recovered_at);
    if let Some(window) = phases.fault.as_mut() {
        window.end(recovered.recovered_at);
    }

    // ── Recovery ────────────────────────────────────────────────────────────
    info!(
        "S11: recovery phase, running rounds for a further {:?}",
        config.phases.recovery()
    );
    ctx.phase.set(Phase::Recovery);
    phases.recovery = Some(PhaseWindow::started(Utc::now()));
    tokio::time::sleep(config.phases.recovery()).await;
    let stood_down = handle.stalled();
    let rounds = handle.rounds();
    handle.stop().await;
    if let Some(window) = phases.recovery.as_mut() {
        window.end(Utc::now());
    }
    routing_snapshots.push(snapshot_routing(deps, "after-recovery").await);

    // ── Account ─────────────────────────────────────────────────────────────
    let settle = settle_before_readback(promise_config);
    info!("S11: letting the last completions land, {settle:?} before read-back");
    tokio::time::sleep(settle).await;

    let records = history.snapshot();

    // Only now, with every `wait` record landed, can the parked population be
    // counted — see [`parked_at_injection`].
    if let Some(injected_at) = fault_injected_at {
        let parked = parked_at_injection(&records, injected_at, &on_pod);
        attention_extra.push(parked.note());
        info!("S11: {}", parked.describe());
    }

    let logs = waiters::read_logs(&ctx, &waiter_names).await;
    // Archived alongside the operations, not just reduced into the report: the
    // reduced numbers cannot be recomputed later, and a correction to how a
    // delay is classified has to be applicable to a run that has already
    // happened.
    history.record_wakeup_logs(logs.clone());

    let report = WakeupReport::build(
        &records,
        &logs,
        &chosen,
        fault_injected_at.map(|injected_at| FaultWindow {
            injected_at,
            recovered_at: fault_recovered_at,
        }),
        promise_config.dwell(),
        promise_config.wakeup_budget(),
        stood_down,
    );
    info!(
        "S11: promise-wakeup account — {rounds} rounds started, {} completions accepted, {} \
         woke once, {} never woke, {} inconclusive, {} unverifiable, {} findings",
        report.completions_confirmed,
        report.woke_once,
        report
            .findings
            .iter()
            .filter(|f| f.violation == crate::chaos::wakeups::WakeupViolation::NeverWoke)
            .count(),
        report.inconclusive,
        report.unverifiable,
        report.findings.len()
    );
    if let Some(p99) = report.fault_window_p99_ms() {
        info!(
            "S11: wakeup delay p99 during the fault, on the killed executor's waiters: {p99}ms \
             against a {}ms budget",
            report.wakeup_budget_ms
        );
    }

    let reason = if report.has_violations() {
        TerminationReason::PromiseWakeupViolated {
            findings: report.violations(),
            first: report
                .findings
                .first()
                .map(|f| format!("{} on token {}", f.violation, f.token))
                .unwrap_or_default(),
        }
    } else if report.woke_once == 0 {
        TerminationReason::StreamNeverSucceeded {
            stream: Stream::PromiseWait.to_string(),
        }
    } else {
        TerminationReason::Completed
    };

    finish!(reason, &records, Some(report));
}

/// How long to wait after the workload stops before reading the wakeup logs.
fn settle_before_readback(config: &PromiseConfig) -> Duration {
    config.dwell() + config.wakeup_budget() + SETTLE_MARGIN
}

/// Waiters that were suspended on a promise when the executor died.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParkedAtInjection {
    pub total: u64,
    pub on_killed_executor: u64,
}

impl ParkedAtInjection {
    /// Whether the kill landed anywhere near the mechanism under test.
    ///
    /// A run that caught nothing parked proves nothing however clean the rest of
    /// its numbers look, which is the one thing here a human has to act on. So
    /// does a run that caught waiters but none of them *on the pod it killed*:
    /// the affected group would be empty and every number in the report would
    /// describe an undisturbed cluster.
    pub fn needs_attention(&self) -> bool {
        self.total == 0 || self.on_killed_executor == 0
    }

    /// The same line as [`Self::describe`], classified.
    pub fn note(&self) -> Note {
        Note::leveled(self.needs_attention(), self.describe())
    }

    pub fn describe(&self) -> String {
        if self.total == 0 {
            return "WARNING: no waiter was suspended on a promise when the executor died, so \
                    this run says nothing about promise-completion recovery"
                .to_string();
        }
        if self.on_killed_executor == 0 {
            return format!(
                "WARNING: {} waiters were suspended when the executor died but none of them \
                 were on it, so the affected group is empty and this run says nothing about \
                 promise-completion recovery",
                self.total
            );
        }
        format!(
            "S11 killed the executor with {} waiters suspended on promises, {} of them on \
             waiters it owned",
            self.total, self.on_killed_executor
        )
    }
}

/// Counts the waiters parked across the moment of injection.
///
/// A `wait` invocation that had been submitted before the kill and had not
/// returned by then. Not derived from the round arithmetic, because a platform
/// that had slowed down would have armed fewer rounds than the cadence says, and
/// the whole point of this number is to say whether the kill landed in anything.
///
/// **This must be computed from the finished history, not from a snapshot taken
/// at injection time.** [`crate::chaos::workload::run_operation`] appends a
/// record only once the operation has completed, so a snapshot taken at the
/// moment of the kill contains none of the invocations that were open across it
/// — which is precisely the population being counted. The first S11 run reported
/// 5 waiters parked when 200 were, and read as a run that had tested nothing.
pub fn parked_at_injection(
    records: &[OperationRecord],
    injected_at: DateTime<Utc>,
    on_killed_executor: &BTreeSet<String>,
) -> ParkedAtInjection {
    let mut parked = ParkedAtInjection {
        total: 0,
        on_killed_executor: 0,
    };

    for record in records
        .iter()
        .filter(|r| r.stream == Stream::PromiseWait && r.method == "wait")
        .filter(|r| r.outcome != Outcome::Rejected)
        .filter(|r| r.submitted_at <= injected_at)
        .filter(|r| r.completed_at.is_none_or(|done| done > injected_at))
    {
        parked.total += 1;
        if on_killed_executor.contains(&record.agent) {
            parked.on_killed_executor += 1;
        }
    }

    parked
}

/// Total wakeups across a small sample of waiters.
async fn sample_wakes(ctx: &WorkloadContext, waiters_list: &[String]) -> u64 {
    let sample: Vec<String> = waiters_list
        .iter()
        .take(WAKE_PROOF_SAMPLE)
        .cloned()
        .collect();
    waiters::read_logs(ctx, &sample)
        .await
        .iter()
        .filter_map(|log| log.wakes)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::{Outcome, Phase, Stream};
    use test_r::test;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn wait_record(agent: &str, submitted: i64, completed: Option<i64>) -> OperationRecord {
        OperationRecord {
            op_id: 1,
            stream: Stream::PromiseWait,
            phase: Phase::Baseline,
            agent: agent.to_string(),
            method: "wait".to_string(),
            idempotency_key: format!("{agent}-wait"),
            submitted_at: at(submitted),
            completed_at: completed.map(at),
            attempts: 1,
            outcome: Outcome::Confirmed,
            duration_ms: 0,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: Vec::new(),
        }
    }

    #[test]
    fn a_waiter_still_parked_at_the_kill_is_counted() {
        let records = vec![wait_record("w-1", 90, Some(150))];
        let on_pod: BTreeSet<String> = ["w-1".to_string()].into_iter().collect();
        assert_eq!(
            parked_at_injection(&records, at(100), &on_pod),
            ParkedAtInjection {
                total: 1,
                on_killed_executor: 1,
            }
        );
    }

    /// A round that had already woken before the kill was not disturbed by it,
    /// and counting it would overstate what the fault landed in.
    #[test]
    fn a_waiter_that_woke_before_the_kill_is_not_counted() {
        let records = vec![wait_record("w-1", 80, Some(90))];
        let on_pod = BTreeSet::new();
        assert_eq!(parked_at_injection(&records, at(100), &on_pod).total, 0);
    }

    /// A `wait` that never returned at all is the most interesting case there
    /// is, so it must not fall out of the count for want of a completion time.
    #[test]
    fn a_wait_that_never_returned_is_still_counted_as_parked() {
        let records = vec![wait_record("w-1", 90, None)];
        let on_pod = BTreeSet::new();
        assert_eq!(parked_at_injection(&records, at(100), &on_pod).total, 1);
    }

    /// A kill that caught waiters but none of its *own* is exactly as
    /// uninformative as one that caught none at all — the affected group is
    /// empty either way — and the first completed S11 run reported this shape as
    /// ordinary context because the rule only looked at the total.
    #[test]
    fn a_kill_that_caught_none_of_its_own_waiters_needs_attention() {
        let parked = ParkedAtInjection {
            total: 200,
            on_killed_executor: 0,
        };
        assert!(parked.needs_attention());
        assert!(parked.describe().contains("none of them"));
    }

    #[test]
    fn a_kill_that_caught_nothing_parked_needs_attention() {
        let parked = ParkedAtInjection {
            total: 0,
            on_killed_executor: 0,
        };
        assert!(parked.needs_attention());
        assert!(parked.describe().contains("says nothing"));
    }
}
