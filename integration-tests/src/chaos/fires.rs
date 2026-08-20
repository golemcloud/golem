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

//! The scheduled-fire account (GOL-378).
//!
//! Every other oracle in this suite reduces to arithmetic over counts: the
//! driver submitted N, the durable state says M, and the gap between them is
//! the finding. That works because an increment is interchangeable with every
//! other increment. A scheduled action is not. The question S10 asks is whether
//! *this* action, claimed by an executor that then died, fired — once — and how
//! long the lease took to hand it to somebody else.
//!
//! So the target agent records a token per fire rather than a tally, and this
//! module pairs those tokens against the registrations the driver made. Pairing
//! is what makes the verdicts facts about a named action rather than a
//! judgement about a distribution, which is the same reason S8 probes keys
//! individually.
//!
//! ## What fails a run, and what only gets reported
//!
//! Three things fail it, and all three are statements about one token:
//!
//! - a **confirmed** registration whose action never fired
//! - a token that fired **more than once**
//! - a registration the platform **refused** that fired anyway
//!
//! Everything else is reported. In particular an *indeterminate* registration
//! that never fired is not a finding: the driver never learned whether the
//! registration landed, so an action that never fired is one of the two
//! legitimate answers. Those are counted, so a clean verdict over many of them
//! reads as weaker than a clean verdict with none.
//!
//! The same care applies to the read itself. A target whose fire log could not
//! be read, or whose log hit the component's cap, cannot testify about its own
//! registrations — those become unverifiable rather than lost. Reporting an
//! unreadable agent as lost work would turn a failed read into a correctness
//! defect, which is the exact mistake this suite exists to avoid.
//!
//! ## Delay, and why it is grouped the way it is
//!
//! Fire delay is `observed - scheduled`: how far past its due time the platform
//! actually ran the action. It is grouped two ways at once, because either
//! alone is misleading.
//!
//! By **window**, because an action due while the executor was gone is the only
//! one whose delay says anything about recovery. By **group**, because on a
//! two-executor cluster roughly half the targets were never on the pod that
//! died: mixing them in drags the percentile down until a lease recovery that
//! took its full TTL looks like a healthy p99.
//!
//! Delays are measured across two clocks — the driver mints the due time, the
//! executor stamps the fire — so a small negative delay is skew rather than an
//! action that fired early. `minDelayMs` is reported per group so that skew is
//! visible instead of silently folded into the percentiles.

use crate::chaos::history::{OperationRecord, Outcome, Stream};
use crate::chaos::summary::LatencyStats;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Ceiling on how many findings the report carries.
///
/// A scenario that lost every action would otherwise produce tens of thousands
/// of them and an artifact nobody can open. The count is reported separately,
/// so truncation is stated rather than inferred from a suspiciously round
/// number of findings.
const MAX_FINDINGS: usize = 200;

/// One fire, as the target agent recorded it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FireRecord {
    /// The registering invocation's idempotency key.
    pub token: String,
    /// When the action was due, as the driver asked for it.
    pub scheduled_at: DateTime<Utc>,
    /// When the platform ran it, as the executor's clock saw it.
    pub observed_at: DateTime<Utc>,
}

impl FireRecord {
    /// How far past its due time the action ran. Negative means clock skew
    /// between the driver and the executor, not an action that fired early.
    pub fn delay_ms(&self) -> i64 {
        (self.observed_at - self.scheduled_at).num_milliseconds()
    }
}

/// Everything read back from one target agent.
#[derive(Debug, Clone)]
pub struct TargetFireLog {
    pub agent: String,
    /// `ScheduleCounter.polls`, which keeps counting past the log's cap and is
    /// therefore what says whether the log below is complete.
    pub polls: Option<u64>,
    pub fires: Vec<FireRecord>,
    /// Why the agent could not be read, when it could not be.
    pub error: Option<String>,
}

impl TargetFireLog {
    /// Whether this log can testify about its own registrations.
    ///
    /// Two ways it cannot: the read failed outright, or the component's fire log
    /// hit its cap and dropped entries. Both leave an absent fire ambiguous.
    pub fn is_complete(&self) -> bool {
        match (self.error.is_some(), self.polls) {
            (true, _) => false,
            (false, Some(polls)) => self.fires.len() as u64 >= polls,
            // No `polls` read means no way to tell whether the log is whole.
            (false, None) => false,
        }
    }
}

/// The fault window, as the workflow reported it.
#[derive(Debug, Clone, Copy)]
pub struct FaultWindow {
    pub injected_at: DateTime<Utc>,
    /// Absent for a run that never saw the fault clear.
    pub recovered_at: Option<DateTime<Utc>>,
}

/// Which side of the fault an action was due on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FireWindow {
    BeforeFault,
    DuringFault,
    AfterFault,
    /// The run never learned when the fault was injected, so no action can be
    /// placed relative to it.
    Unknown,
}

impl FireWindow {
    pub fn as_str(self) -> &'static str {
        match self {
            FireWindow::BeforeFault => "before-fault",
            FireWindow::DuringFault => "during-fault",
            FireWindow::AfterFault => "after-fault",
            FireWindow::Unknown => "unknown",
        }
    }

    fn of(due: DateTime<Utc>, fault: Option<FaultWindow>) -> Self {
        match fault {
            None => FireWindow::Unknown,
            Some(window) if due < window.injected_at => FireWindow::BeforeFault,
            Some(FaultWindow {
                recovered_at: Some(recovered),
                ..
            }) if due >= recovered => FireWindow::AfterFault,
            Some(_) => FireWindow::DuringFault,
        }
    }
}

impl std::fmt::Display for FireWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether an action's target was on the executor the fault killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetGroup {
    /// Owned by the killed executor when the driver signalled readiness.
    OnKilledExecutor,
    /// Owned by an executor the fault left alone: the run's own control group.
    Elsewhere,
}

impl TargetGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetGroup::OnKilledExecutor => "on-killed-executor",
            TargetGroup::Elsewhere => "elsewhere",
        }
    }
}

impl std::fmt::Display for TargetGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a token did that it should not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FireViolation {
    /// A registration the platform accepted whose action never ran.
    NeverFired,
    /// One registration, two or more fires.
    FiredMoreThanOnce,
    /// A registration the platform definitively refused, whose action ran
    /// anyway.
    FiredDespiteRejection,
}

impl FireViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            FireViolation::NeverFired => "never-fired",
            FireViolation::FiredMoreThanOnce => "fired-more-than-once",
            FireViolation::FiredDespiteRejection => "fired-despite-rejection",
        }
    }
}

impl std::fmt::Display for FireViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One violation, against one token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FireFinding {
    pub violation: FireViolation,
    pub token: String,
    pub agent: String,
    pub window: FireWindow,
    pub detail: String,
}

/// Fire delay for one (group, window) cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FireDelayStats {
    pub group: TargetGroup,
    pub window: FireWindow,
    /// Percentiles over delays clamped at zero, so skew cannot flatter them.
    pub delay: LatencyStats,
    /// The most negative delay seen, which is the clock skew between the driver
    /// and the executor rather than an action that fired early.
    pub min_delay_ms: i64,
    /// Fires whose delay exceeded the configured lease budget.
    pub over_budget: u64,
}

/// The scheduled-fire account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleFireReport {
    /// What a lease recovery is allowed to cost, from the suite YAML. Recorded
    /// so a percentile in an archived result can be read years later against
    /// the number it was judged by rather than against today's config.
    pub lease_budget_ms: u64,
    pub registrations_confirmed: u64,
    pub registrations_indeterminate: u64,
    pub registrations_rejected: u64,
    /// Fires the targets recorded, including any whose token is unknown.
    pub fires_recorded: u64,
    /// Accepted registrations paired with exactly one fire.
    pub fired_once: u64,
    /// Registrations the driver was never sure of that fired anyway — doubt the
    /// platform resolved in its own favour.
    pub indeterminate_that_fired: u64,
    /// Registrations the driver was never sure of that never fired. Not a
    /// finding: the registration may never have landed.
    pub inconclusive: u64,
    /// Registrations whose target could not testify, because its log was
    /// unreadable or truncated.
    pub unverifiable: u64,
    /// Fires whose token no registration claims. Zero on a healthy run: agent
    /// names carry the run nonce, so nothing from an earlier run can appear.
    pub unknown_tokens: u64,
    /// Targets the read-back could not reach at all.
    pub targets_unreadable: Vec<String>,
    /// Targets whose fire log hit the component's cap.
    pub targets_truncated: Vec<String>,
    pub delay: Vec<FireDelayStats>,
    pub findings: Vec<FireFinding>,
    /// Findings past [`MAX_FINDINGS`], which the report drops rather than
    /// carries. Non-zero means `findings` is a sample.
    pub findings_omitted: u64,
}

impl ScheduleFireReport {
    /// Pairs registrations against fires.
    ///
    /// `records` is the whole history; only the scheduled stream is considered.
    /// `lead` is how far ahead registrations were made, used to say when an
    /// action that never fired was due — the fire log is the only place the
    /// exact due time survives, and an action that never fired left no entry.
    pub fn build(
        records: &[OperationRecord],
        logs: &[TargetFireLog],
        lead: Duration,
        fault: Option<FaultWindow>,
        killed_targets: &BTreeSet<String>,
        lease_budget: Duration,
    ) -> Self {
        let lead = TimeDelta::from_std(lead).unwrap_or(TimeDelta::zero());
        let budget_ms = lease_budget.as_millis().min(u64::MAX as u128) as u64;

        let mut fires_by_token: BTreeMap<&str, Vec<&FireRecord>> = BTreeMap::new();
        let mut complete: BTreeMap<&str, bool> = BTreeMap::new();
        let mut targets_unreadable = Vec::new();
        let mut targets_truncated = Vec::new();
        let mut fires_recorded = 0u64;

        for log in logs {
            complete.insert(log.agent.as_str(), log.is_complete());
            if log.error.is_some() {
                targets_unreadable.push(log.agent.clone());
            } else if !log.is_complete() {
                targets_truncated.push(log.agent.clone());
            }
            for fire in &log.fires {
                fires_recorded += 1;
                fires_by_token
                    .entry(fire.token.as_str())
                    .or_default()
                    .push(fire);
            }
        }

        let mut report = Self {
            lease_budget_ms: budget_ms,
            registrations_confirmed: 0,
            registrations_indeterminate: 0,
            registrations_rejected: 0,
            fires_recorded,
            fired_once: 0,
            indeterminate_that_fired: 0,
            inconclusive: 0,
            unverifiable: 0,
            unknown_tokens: 0,
            targets_unreadable,
            targets_truncated,
            delay: Vec::new(),
            findings: Vec::new(),
            findings_omitted: 0,
        };

        let mut claimed: BTreeSet<&str> = BTreeSet::new();
        let mut findings: Vec<FireFinding> = Vec::new();

        for record in records.iter().filter(|r| r.stream == Stream::Scheduled) {
            let token = record.idempotency_key.as_str();
            claimed.insert(token);
            let fires = fires_by_token.get(token).map(Vec::as_slice).unwrap_or(&[]);
            // The due time comes from the fire itself when there is one, because
            // that is what the platform was actually told. Without a fire the
            // driver only knows what it asked for.
            let due = fires
                .first()
                .map(|f| f.scheduled_at)
                .unwrap_or(record.submitted_at + lead);
            let window = FireWindow::of(due, fault);
            let can_testify = complete
                .get(record.agent.as_str())
                .copied()
                .unwrap_or(false);

            match record.outcome {
                Outcome::Confirmed => report.registrations_confirmed += 1,
                Outcome::Indeterminate => report.registrations_indeterminate += 1,
                Outcome::Rejected => report.registrations_rejected += 1,
            }

            match (record.outcome, fires.len()) {
                (_, count) if count > 1 => findings.push(FireFinding {
                    violation: FireViolation::FiredMoreThanOnce,
                    token: token.to_string(),
                    agent: record.agent.clone(),
                    window,
                    detail: format!(
                        "one registration, {count} fires at {} — the action ran more than once",
                        fires
                            .iter()
                            .map(|f| f.observed_at.to_rfc3339())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }),
                (Outcome::Rejected, 1) => findings.push(FireFinding {
                    violation: FireViolation::FiredDespiteRejection,
                    token: token.to_string(),
                    agent: record.agent.clone(),
                    window,
                    detail: "the platform refused the registration, then ran the action"
                        .to_string(),
                }),
                (Outcome::Confirmed, 1) => report.fired_once += 1,
                (Outcome::Indeterminate, 1) => {
                    report.fired_once += 1;
                    report.indeterminate_that_fired += 1;
                }
                (Outcome::Confirmed, 0) if !can_testify => report.unverifiable += 1,
                (Outcome::Confirmed, 0) => findings.push(FireFinding {
                    violation: FireViolation::NeverFired,
                    token: token.to_string(),
                    agent: record.agent.clone(),
                    window,
                    detail: format!(
                        "accepted registration due at {} never fired",
                        due.to_rfc3339()
                    ),
                }),
                (Outcome::Indeterminate, 0) if !can_testify => report.unverifiable += 1,
                (Outcome::Indeterminate, 0) => report.inconclusive += 1,
                (Outcome::Rejected, 0) => {}
                (_, _) => {}
            }
        }

        report.unknown_tokens = fires_by_token
            .iter()
            .filter(|(token, _)| !claimed.contains(*token))
            .map(|(_, fires)| fires.len() as u64)
            .sum();

        report.findings_omitted = findings.len().saturating_sub(MAX_FINDINGS) as u64;
        findings.truncate(MAX_FINDINGS);
        report.findings = findings;
        report.delay = delay_stats(logs, fault, killed_targets, budget_ms);
        report
    }

    /// The three conditions that fail the scenario.
    pub fn has_violations(&self) -> bool {
        !self.findings.is_empty()
    }

    /// The p99 of the cell that the SLO is about: actions due while the
    /// executor was gone, on targets it owned.
    pub fn fault_window_p99_ms(&self) -> Option<u64> {
        self.delay
            .iter()
            .find(|d| {
                d.group == TargetGroup::OnKilledExecutor && d.window == FireWindow::DuringFault
            })
            .map(|d| d.delay.p99_ms)
    }

    /// Lines an operator should see next to the read-back verdicts.
    pub fn attention_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for finding in &self.findings {
            lines.push(format!(
                "{}: token {} on target {} ({}) — {}",
                finding.violation, finding.token, finding.agent, finding.window, finding.detail
            ));
        }
        if self.findings_omitted > 0 {
            lines.push(format!(
                "scheduled-fire findings are a sample: {} more were dropped",
                self.findings_omitted
            ));
        }
        if !self.targets_unreadable.is_empty() {
            lines.push(format!(
                "{} scheduled targets could not be read back, so {} registrations are \
                 unverifiable rather than accounted for",
                self.targets_unreadable.len(),
                self.unverifiable
            ));
        }
        if !self.targets_truncated.is_empty() {
            lines.push(format!(
                "{} scheduled targets filled their fire log and dropped entries — raise the \
                 cadence or shorten the run before reading this as exactly-once evidence",
                self.targets_truncated.len()
            ));
        }
        if self.unknown_tokens > 0 {
            lines.push(format!(
                "{} fires carried a token no registration claims, which should be impossible \
                 within one run nonce",
                self.unknown_tokens
            ));
        }
        if let Some(p99) = self.fault_window_p99_ms()
            && p99 > self.lease_budget_ms
        {
            lines.push(format!(
                "scheduled-fire p99 during the fault was {p99}ms against a {}ms lease budget",
                self.lease_budget_ms
            ));
        }
        lines
    }
}

/// Delay percentiles per (group, window) cell.
fn delay_stats(
    logs: &[TargetFireLog],
    fault: Option<FaultWindow>,
    killed_targets: &BTreeSet<String>,
    budget_ms: u64,
) -> Vec<FireDelayStats> {
    let mut cells: BTreeMap<(TargetGroup, FireWindow), Vec<i64>> = BTreeMap::new();

    for log in logs {
        let group = if killed_targets.contains(&log.agent) {
            TargetGroup::OnKilledExecutor
        } else {
            TargetGroup::Elsewhere
        };
        for fire in &log.fires {
            cells
                .entry((group, FireWindow::of(fire.scheduled_at, fault)))
                .or_default()
                .push(fire.delay_ms());
        }
    }

    cells
        .into_iter()
        .map(|((group, window), delays)| {
            let min = delays.iter().copied().min().unwrap_or(0);
            let over_budget = delays.iter().filter(|d| **d > budget_ms as i64).count() as u64;
            FireDelayStats {
                group,
                window,
                delay: LatencyStats::from_durations(
                    delays.iter().map(|d| (*d).max(0) as u64).collect(),
                ),
                min_delay_ms: min,
                over_budget,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::{AttemptRecord, Phase};
    use test_r::test;

    fn at(offset_secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + offset_secs, 0).unwrap()
    }

    const LEAD: Duration = Duration::from_secs(10);
    const BUDGET: Duration = Duration::from_secs(30);

    fn record(
        token: &str,
        agent: &str,
        outcome: Outcome,
        submitted_at: DateTime<Utc>,
    ) -> OperationRecord {
        OperationRecord {
            op_id: 0,
            stream: Stream::Scheduled,
            phase: Phase::Fault,
            agent: agent.to_string(),
            method: "schedule_fire_at".to_string(),
            idempotency_key: token.to_string(),
            submitted_at,
            completed_at: Some(submitted_at),
            attempts: 1,
            outcome,
            duration_ms: 12,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: vec![AttemptRecord {
                attempt: 1,
                started_at: submitted_at,
                duration_ms: 12,
                returned_value: None,
                succeeded: outcome == Outcome::Confirmed,
                error_class: None,
                error: None,
            }],
        }
    }

    fn fire(token: &str, scheduled: DateTime<Utc>, delay_ms: i64) -> FireRecord {
        FireRecord {
            token: token.to_string(),
            scheduled_at: scheduled,
            observed_at: scheduled + TimeDelta::milliseconds(delay_ms),
        }
    }

    /// A log the agent answered in full.
    fn log(agent: &str, fires: Vec<FireRecord>) -> TargetFireLog {
        TargetFireLog {
            agent: agent.to_string(),
            polls: Some(fires.len() as u64),
            fires,
            error: None,
        }
    }

    fn build(records: &[OperationRecord], logs: &[TargetFireLog]) -> ScheduleFireReport {
        ScheduleFireReport::build(records, logs, LEAD, None, &BTreeSet::new(), BUDGET)
    }

    /// The healthy shape: one registration, one fire.
    #[test]
    fn a_registration_paired_with_one_fire_is_not_a_finding() {
        let r = record("t-0", "target-0", Outcome::Confirmed, at(0));
        let report = build(
            std::slice::from_ref(&r),
            &[log("target-0", vec![fire("t-0", at(10), 40)])],
        );
        assert!(!report.has_violations(), "{:?}", report.findings);
        assert_eq!(report.fired_once, 1);
        assert_eq!(report.registrations_confirmed, 1);
    }

    /// Accepted work has to happen. This is the loss half of the guarantee.
    #[test]
    fn a_confirmed_registration_that_never_fired_fails_the_run() {
        let r = record("t-1", "target-0", Outcome::Confirmed, at(0));
        let report = build(std::slice::from_ref(&r), &[log("target-0", vec![])]);
        assert!(report.has_violations());
        assert_eq!(report.findings[0].violation, FireViolation::NeverFired);
        assert_eq!(report.findings[0].token, "t-1");
        assert!(
            report.findings[0].detail.contains(&at(10).to_rfc3339()),
            "the finding must say when the action was due: {}",
            report.findings[0].detail
        );
    }

    /// The duplicate half, and the reason the target records tokens rather than
    /// a tally: a lease recovered while the original claim was still running
    /// runs the action twice, and no count over a busy target would show it.
    #[test]
    fn a_token_that_fired_twice_fails_the_run() {
        let r = record("t-2", "target-0", Outcome::Confirmed, at(0));
        let report = build(
            std::slice::from_ref(&r),
            &[log(
                "target-0",
                vec![fire("t-2", at(10), 40), fire("t-2", at(10), 31_000)],
            )],
        );
        assert!(report.has_violations());
        assert_eq!(
            report.findings[0].violation,
            FireViolation::FiredMoreThanOnce
        );
    }

    /// A definite refusal means nothing was accepted, so an action that runs
    /// anyway is the platform contradicting its own answer.
    #[test]
    fn a_refused_registration_that_fired_anyway_fails_the_run() {
        let r = record("t-3", "target-0", Outcome::Rejected, at(0));
        let report = build(
            std::slice::from_ref(&r),
            &[log("target-0", vec![fire("t-3", at(10), 5)])],
        );
        assert!(report.has_violations());
        assert_eq!(
            report.findings[0].violation,
            FireViolation::FiredDespiteRejection
        );
    }

    /// The distinction the whole verdict rests on: the driver never learned
    /// whether this registration landed, so an action that never fired is one
    /// of two legitimate answers.
    #[test]
    fn an_indeterminate_registration_that_never_fired_is_counted_rather_than_failed() {
        let r = record("t-4", "target-0", Outcome::Indeterminate, at(0));
        let report = build(std::slice::from_ref(&r), &[log("target-0", vec![])]);
        assert!(!report.has_violations(), "{:?}", report.findings);
        assert_eq!(
            report.inconclusive, 1,
            "but it must be counted, so a clean verdict over many of them reads as weaker"
        );
    }

    /// ...and when it did fire, the doubt resolved in the platform's favour.
    #[test]
    fn an_indeterminate_registration_that_fired_resolves_the_doubt() {
        let r = record("t-5", "target-0", Outcome::Indeterminate, at(0));
        let report = build(
            std::slice::from_ref(&r),
            &[log("target-0", vec![fire("t-5", at(10), 20)])],
        );
        assert!(!report.has_violations());
        assert_eq!(report.fired_once, 1);
        assert_eq!(report.indeterminate_that_fired, 1);
    }

    /// A read that failed is not evidence about the platform. Turning it into
    /// one would report a network problem as lost durable work.
    #[test]
    fn an_unreadable_target_leaves_its_registrations_unverifiable_rather_than_lost() {
        let r = record("t-6", "target-0", Outcome::Confirmed, at(0));
        let unreadable = TargetFireLog {
            agent: "target-0".to_string(),
            polls: None,
            fires: Vec::new(),
            error: Some("timed out after 30s".to_string()),
        };
        let report = build(std::slice::from_ref(&r), &[unreadable]);
        assert!(!report.has_violations(), "{:?}", report.findings);
        assert_eq!(report.unverifiable, 1);
        assert_eq!(report.targets_unreadable, vec!["target-0".to_string()]);
    }

    /// Same reasoning for a log that filled up: the fire may have happened and
    /// been dropped. `polls` is what makes that detectable at all.
    #[test]
    fn a_truncated_fire_log_leaves_its_registrations_unverifiable_rather_than_lost() {
        let r = record("t-7", "target-0", Outcome::Confirmed, at(0));
        let truncated = TargetFireLog {
            agent: "target-0".to_string(),
            // The agent fired 500 actions and kept 2 of them.
            polls: Some(500),
            fires: vec![fire("t-other", at(10), 5), fire("t-more", at(11), 5)],
            error: None,
        };
        let report = build(std::slice::from_ref(&r), &[truncated]);
        assert!(!report.has_violations(), "{:?}", report.findings);
        assert_eq!(report.unverifiable, 1);
        assert_eq!(report.targets_truncated, vec!["target-0".to_string()]);
    }

    /// The grouping that makes the percentile mean anything. Both cells are
    /// reported; folding them together would let the untouched half hide what
    /// the recovery cost.
    #[test]
    fn delay_is_split_by_whether_the_target_was_on_the_killed_executor() {
        let fault = FaultWindow {
            injected_at: at(100),
            recovered_at: Some(at(200)),
        };
        let killed = BTreeSet::from(["target-killed".to_string()]);
        let logs = vec![
            log("target-killed", vec![fire("a", at(150), 28_000)]),
            log("target-survivor", vec![fire("b", at(150), 40)]),
        ];
        let report = ScheduleFireReport::build(&[], &logs, LEAD, Some(fault), &killed, BUDGET);

        let killed_cell = report
            .delay
            .iter()
            .find(|d| d.group == TargetGroup::OnKilledExecutor)
            .expect("the killed executor's targets need their own cell");
        assert_eq!(killed_cell.window, FireWindow::DuringFault);
        assert_eq!(killed_cell.delay.p99_ms, 28_000);

        let survivor_cell = report
            .delay
            .iter()
            .find(|d| d.group == TargetGroup::Elsewhere)
            .expect("the control group needs its own cell");
        assert_eq!(survivor_cell.delay.p99_ms, 40);

        assert_eq!(report.fault_window_p99_ms(), Some(28_000));
    }

    /// Actions are placed by when they were *due*, not when they fired: an
    /// action due during the outage that landed afterwards is exactly the
    /// population the scenario is about.
    #[test]
    fn an_action_is_placed_in_the_window_its_due_time_fell_in() {
        let fault = Some(FaultWindow {
            injected_at: at(100),
            recovered_at: Some(at(200)),
        });
        assert_eq!(FireWindow::of(at(99), fault), FireWindow::BeforeFault);
        assert_eq!(FireWindow::of(at(100), fault), FireWindow::DuringFault);
        // Due mid-fault, fired long after it healed: still a fault-window action.
        assert_eq!(FireWindow::of(at(199), fault), FireWindow::DuringFault);
        assert_eq!(FireWindow::of(at(200), fault), FireWindow::AfterFault);
        assert_eq!(FireWindow::of(at(150), None), FireWindow::Unknown);
    }

    /// A fault that never cleared leaves everything after injection inside it,
    /// rather than inventing an end.
    #[test]
    fn a_fault_that_never_cleared_has_no_after_window() {
        let fault = Some(FaultWindow {
            injected_at: at(100),
            recovered_at: None,
        });
        assert_eq!(FireWindow::of(at(10_000), fault), FireWindow::DuringFault);
    }

    /// Two clocks measure this, so a small negative delay is skew. It stays
    /// visible as a minimum instead of being folded into the percentiles.
    #[test]
    fn clock_skew_shows_as_a_negative_minimum_rather_than_flattering_the_percentiles() {
        let logs = vec![log(
            "target-0",
            vec![fire("a", at(10), -35), fire("b", at(11), 60)],
        )];
        let report = ScheduleFireReport::build(&[], &logs, LEAD, None, &BTreeSet::new(), BUDGET);
        let cell = &report.delay[0];
        assert_eq!(cell.min_delay_ms, -35);
        assert_eq!(cell.delay.p50_ms, 0, "the negative sample clamps to zero");
        assert_eq!(cell.delay.max_ms, 60);
    }

    /// Fires over the lease budget are counted per cell, which is what turns a
    /// percentile into SLO evidence rather than a number.
    #[test]
    fn fires_past_the_lease_budget_are_counted() {
        let logs = vec![log(
            "target-0",
            vec![
                fire("a", at(10), 29_000),
                fire("b", at(11), 31_000),
                fire("c", at(12), 45_000),
            ],
        )];
        let report = ScheduleFireReport::build(&[], &logs, LEAD, None, &BTreeSet::new(), BUDGET);
        assert_eq!(report.delay[0].over_budget, 2);
    }

    /// An artifact nobody can open is not evidence. Truncation is stated.
    #[test]
    fn findings_beyond_the_cap_are_counted_rather_than_carried() {
        let records: Vec<OperationRecord> = (0..MAX_FINDINGS + 25)
            .map(|i| record(&format!("t-{i}"), "target-0", Outcome::Confirmed, at(0)))
            .collect();
        let report = build(&records, &[log("target-0", vec![])]);
        assert_eq!(report.findings.len(), MAX_FINDINGS);
        assert_eq!(report.findings_omitted, 25);
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|line| line.contains("are a sample")),
            "an operator must be told the list is partial"
        );
    }

    /// Agent names carry the run nonce, so this cannot happen within a run. If
    /// it ever does, the pairing is answering a different question than it
    /// thinks it is, and the report says so.
    #[test]
    fn a_fire_whose_token_no_registration_claims_is_counted() {
        let r = record("t-8", "target-0", Outcome::Confirmed, at(0));
        let report = build(
            std::slice::from_ref(&r),
            &[log(
                "target-0",
                vec![fire("t-8", at(10), 5), fire("stranger", at(10), 5)],
            )],
        );
        assert_eq!(report.unknown_tokens, 1);
        assert!(!report.has_violations());
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|line| line.contains("no registration claims"))
        );
    }

    /// Only the scheduled stream is paired: a durable operation carries an
    /// idempotency key too, and pairing it against a fire log would invent
    /// findings out of an unrelated stream.
    #[test]
    fn operations_from_other_streams_are_not_paired() {
        let mut durable = record("t-9", "target-0", Outcome::Confirmed, at(0));
        durable.stream = Stream::Durable;
        let report = build(std::slice::from_ref(&durable), &[log("target-0", vec![])]);
        assert_eq!(report.registrations_confirmed, 0);
        assert!(!report.has_violations());
    }

    /// The p99 an operator is judged on is the fault-window cell for the
    /// targets that were on the dead pod. With no such cell there is nothing to
    /// judge, and the report says nothing rather than substituting another.
    #[test]
    fn the_reported_p99_is_absent_when_the_fault_window_produced_no_fires() {
        let report = ScheduleFireReport::build(&[], &[], LEAD, None, &BTreeSet::new(), BUDGET);
        assert_eq!(report.fault_window_p99_ms(), None);
    }
}
