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

//! Did every revert land on a boundary, or did one tear (GOL-371)?
//!
//! The sharpest oracle in the suite, because it has no band of doubt in it.
//! Every other read-back compares a counter against a *range* — the width of
//! the range is the operations whose fate the driver could not determine. Here
//! the driver knows the counter's value immediately before the revert, because
//! the last increment of the round returned it, and it knows exactly how many
//! invocations the revert was asked to take back. So afterwards there are
//! exactly **two** legal values and nothing in between:
//!
//! * `V` — the revert never committed
//! * `V - N` — the revert committed
//!
//! Anything else is a defect, and which kind it is says what went wrong. A
//! value strictly between the two is a truncation that tore. A value below
//! `V - N` took back more than it was asked for. A value above `V` means state
//! grew across a revert.
//!
//! ### Why a partial truncation should be impossible, and why that is worth
//! testing anyway
//!
//! Reading `Worker::revert` in the executor: `RevertLastInvocations` walks back
//! to the nth `AgentInvocationStarted` entry and then commits **one**
//! `OplogEntry::revert` marking the region deleted. One entry cannot tear, so
//! the truncation itself is atomic by construction.
//!
//! The window worth killing into is the one *around* it. Reverting takes
//! `lock_stopped_worker`, so the worker is stopped first; then the entry is
//! committed; then `reattach_worker_status` runs, because — in the executor's
//! own words — "this commit will detach the worker status, immediately reattach
//! it so we see the up to date status". An executor that dies between the
//! commit and the reattach has left durable state changed and in-memory state
//! stale, which is the same shape as the S11 promise defect: the durable half
//! landed and the half that tells anyone about it did not.
//!
//! So the two findings this account expects to be able to make, if the platform
//! has a bug here, are [`TruncationViolation::AcknowledgedButNotApplied`] and
//! its opposite — not a torn counter.

use crate::chaos::history::Outcome;
use crate::chaos::reverts::RevertRound;
use crate::chaos::split::{FaultWindow, Group, PodSplit, Window};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most findings the report carries.
const MAX_FINDINGS: usize = 50;

/// What went wrong with one revert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TruncationViolation {
    /// The counter landed strictly between the pre-revert and post-revert
    /// values: some of the invocations were taken back and some were not.
    PartialTruncation,
    /// More was taken back than the revert was asked for.
    OverTruncation,
    /// The counter is higher after the revert than it was before it.
    Divergent,
    /// The platform confirmed the revert and the state never moved. The same
    /// shape as an accepted promise completion that never woke its waiter.
    AcknowledgedButNotApplied,
    /// The platform refused the revert and the state moved anyway.
    RefusedButApplied,
}

impl TruncationViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            TruncationViolation::PartialTruncation => "partial-truncation",
            TruncationViolation::OverTruncation => "over-truncation",
            TruncationViolation::Divergent => "divergent",
            TruncationViolation::AcknowledgedButNotApplied => "acknowledged-but-not-applied",
            TruncationViolation::RefusedButApplied => "refused-but-applied",
        }
    }
}

/// One violation, against one named round.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationFinding {
    pub violation: TruncationViolation,
    pub agent: String,
    pub round: u32,
    pub window: Window,
    /// The counter before the revert, what it should have become, and what it
    /// actually became. Carried so a finding can be read without the history.
    pub before: u64,
    pub expected_after_commit: u64,
    pub observed: u64,
    pub detail: String,
}

/// Rounds and their verdicts for one (group, window) cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationCell {
    pub group: Group,
    pub window: Window,
    pub rounds: u64,
    /// Reverts the platform confirmed that landed on the post-revert value.
    pub applied: u64,
    /// Reverts that left the agent exactly where it was. Legitimate only when
    /// the platform never confirmed them.
    pub not_applied: u64,
    pub violations: u64,
    /// Rounds the driver cannot judge: an increment that did not answer, so the
    /// pre-revert value is unknown.
    pub unjudgeable: u64,
    /// Rounds no following increment ever probed.
    pub unprobed: u64,
}

/// Reverts the kill landed in the middle of.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertsCaught {
    pub group: Group,
    /// Reverts submitted before the kill that had not answered when it landed.
    /// The population the scenario is actually about: a run that caught none of
    /// them proves nothing about crash-during-revert, however clean it looks.
    pub reverts: u64,
    pub agents: usize,
    pub confirmed: u64,
    pub indeterminate: u64,
    pub rejected: u64,
}

/// The truncation account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationReport {
    /// What each round was configured to do, so an archived finding can be read
    /// without the suite YAML to hand.
    pub increments_per_round: u32,
    pub revert_invocations: u32,
    pub rounds_recorded: u64,
    pub reverts_confirmed: u64,
    pub reverts_indeterminate: u64,
    pub reverts_rejected: u64,
    /// Confirmed reverts that landed exactly on the post-revert value.
    pub applied_exactly: u64,
    /// Reverts the driver never heard back about that applied anyway — doubt
    /// the platform resolved in its own favour, not a defect.
    pub indeterminate_that_applied: u64,
    /// Reverts the driver never heard back about that did not apply. Also not a
    /// defect: the call may never have landed.
    pub indeterminate_that_did_not: u64,
    pub unjudgeable: u64,
    pub unprobed: u64,
    pub cells: Vec<TruncationCell>,
    pub caught_by_the_kill: Vec<RevertsCaught>,
    pub findings: Vec<TruncationFinding>,
    pub findings_omitted: u64,
}

/// One round's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Applied,
    NotApplied,
    Violation(TruncationViolation),
    Unjudgeable,
    Unprobed,
}

/// Judges one round against the two values it is allowed to have landed on.
fn judge(round: &RevertRound) -> Verdict {
    let Some(before) = round.before_revert else {
        return Verdict::Unjudgeable;
    };
    let Some(observed) = round.observed_after else {
        return Verdict::Unprobed;
    };
    let expected = before.saturating_sub(round.asked_to_revert as u64);

    if observed == expected {
        // Applied. Legitimate unless the platform said it refused.
        if round.outcome == Outcome::Rejected {
            return Verdict::Violation(TruncationViolation::RefusedButApplied);
        }
        return Verdict::Applied;
    }
    if observed == before {
        // Not applied. Legitimate unless the platform said it had been.
        if round.outcome == Outcome::Confirmed {
            return Verdict::Violation(TruncationViolation::AcknowledgedButNotApplied);
        }
        return Verdict::NotApplied;
    }
    if observed > before {
        return Verdict::Violation(TruncationViolation::Divergent);
    }
    if observed < expected {
        return Verdict::Violation(TruncationViolation::OverTruncation);
    }
    Verdict::Violation(TruncationViolation::PartialTruncation)
}

impl TruncationReport {
    /// Builds the account from the rounds the workload recorded.
    pub fn build(
        rounds: &[RevertRound],
        split: &PodSplit,
        fault: Option<FaultWindow>,
        increments_per_round: u32,
        revert_invocations: u32,
    ) -> Self {
        let mut cells: BTreeMap<(Group, Window), TruncationCell> = BTreeMap::new();
        let mut findings: Vec<TruncationFinding> = Vec::new();
        let mut report = TruncationReport {
            increments_per_round,
            revert_invocations,
            rounds_recorded: rounds.len() as u64,
            reverts_confirmed: 0,
            reverts_indeterminate: 0,
            reverts_rejected: 0,
            applied_exactly: 0,
            indeterminate_that_applied: 0,
            indeterminate_that_did_not: 0,
            unjudgeable: 0,
            unprobed: 0,
            cells: Vec::new(),
            caught_by_the_kill: Vec::new(),
            findings: Vec::new(),
            findings_omitted: 0,
        };

        for round in rounds {
            let group = split.group_of(&round.agent).unwrap_or(Group::Elsewhere);
            let window = Window::of(round.submitted_at, fault);
            let cell = cells
                .entry((group, window))
                .or_insert_with(|| TruncationCell {
                    group,
                    window,
                    rounds: 0,
                    applied: 0,
                    not_applied: 0,
                    violations: 0,
                    unjudgeable: 0,
                    unprobed: 0,
                });
            cell.rounds += 1;

            match round.outcome {
                Outcome::Confirmed => report.reverts_confirmed += 1,
                Outcome::Indeterminate => report.reverts_indeterminate += 1,
                Outcome::Rejected => report.reverts_rejected += 1,
            }

            match judge(round) {
                Verdict::Applied => {
                    cell.applied += 1;
                    if round.outcome == Outcome::Confirmed {
                        report.applied_exactly += 1;
                    } else {
                        report.indeterminate_that_applied += 1;
                    }
                }
                Verdict::NotApplied => {
                    cell.not_applied += 1;
                    if round.outcome != Outcome::Confirmed {
                        report.indeterminate_that_did_not += 1;
                    }
                }
                Verdict::Unjudgeable => {
                    cell.unjudgeable += 1;
                    report.unjudgeable += 1;
                }
                Verdict::Unprobed => {
                    cell.unprobed += 1;
                    report.unprobed += 1;
                }
                Verdict::Violation(violation) => {
                    cell.violations += 1;
                    let before = round.before_revert.unwrap_or_default();
                    let observed = round.observed_after.unwrap_or_default();
                    let expected = before.saturating_sub(round.asked_to_revert as u64);
                    findings.push(TruncationFinding {
                        violation,
                        agent: round.agent.clone(),
                        round: round.round,
                        window,
                        before,
                        expected_after_commit: expected,
                        observed,
                        detail: detail_for(violation, round, before, expected, observed),
                    });
                }
            }
        }

        // ── What the kill landed in the middle of ───────────────────────────
        if let Some(window) = fault {
            let mut by_group: BTreeMap<Group, Vec<&RevertRound>> = BTreeMap::new();
            for round in rounds {
                let Some(group) = split.group_of(&round.agent) else {
                    continue;
                };
                let unresolved = round.completed_at.is_none_or(|at| at >= window.injected_at);
                if round.submitted_at < window.injected_at && unresolved {
                    by_group.entry(group).or_default().push(round);
                }
            }
            for (group, caught) in by_group {
                let agents: std::collections::BTreeSet<&str> =
                    caught.iter().map(|r| r.agent.as_str()).collect();
                report.caught_by_the_kill.push(RevertsCaught {
                    group,
                    reverts: caught.len() as u64,
                    agents: agents.len(),
                    confirmed: count(&caught, Outcome::Confirmed),
                    indeterminate: count(&caught, Outcome::Indeterminate),
                    rejected: count(&caught, Outcome::Rejected),
                });
            }
        }

        report.cells = cells.into_values().collect();
        report.findings_omitted = findings.len().saturating_sub(MAX_FINDINGS) as u64;
        findings.truncate(MAX_FINDINGS);
        report.findings = findings;
        report
    }

    pub fn has_violations(&self) -> bool {
        !self.findings.is_empty() || self.findings_omitted > 0
    }

    /// The lines that need a human.
    pub fn attention_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .findings
            .iter()
            .map(|f| {
                format!(
                    "S7 {}: {} round {} — {}",
                    f.violation.as_str(),
                    f.agent,
                    f.round,
                    f.detail
                )
            })
            .collect();
        if self.findings_omitted > 0 {
            lines.push(format!(
                "S7: {} further truncation finding(s) were dropped from the report",
                self.findings_omitted
            ));
        }

        // The S10 lesson: a run that caught none of the mechanism proves
        // nothing about it, however clean every other number looks.
        let caught: u64 = self
            .caught_by_the_kill
            .iter()
            .filter(|c| c.group == Group::OnPod)
            .map(|c| c.reverts)
            .sum();
        if caught == 0 {
            lines.push(
                "S7: the kill caught no revert in flight on the targeted executor, so this run \
                 says nothing about crashing during a revert. Every verdict below describes \
                 reverts that completed either side of it."
                    .to_string(),
            );
        }
        lines
    }

    /// Lines a reader needs in order to interpret the run.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "S7: {} rounds of {} increments then a revert of {}; {} confirmed, {} in doubt, {} \
             refused",
            self.rounds_recorded,
            self.increments_per_round,
            self.revert_invocations,
            self.reverts_confirmed,
            self.reverts_indeterminate,
            self.reverts_rejected
        )];
        lines.push(format!(
            "S7: {} confirmed reverts landed exactly on the post-revert value; {} in doubt \
             applied anyway, {} in doubt did not",
            self.applied_exactly, self.indeterminate_that_applied, self.indeterminate_that_did_not
        ));
        if self.unjudgeable > 0 || self.unprobed > 0 {
            lines.push(format!(
                "S7: {} rounds could not be judged (an increment never answered) and {} were \
                 never probed by a following increment",
                self.unjudgeable, self.unprobed
            ));
        }
        for caught in &self.caught_by_the_kill {
            lines.push(format!(
                "S7 {}: {} reverts across {} agents were unresolved when the kill landed — {} \
                 confirmed, {} in doubt, {} refused",
                caught.group.as_str(),
                caught.reverts,
                caught.agents,
                caught.confirmed,
                caught.indeterminate,
                caught.rejected
            ));
        }
        for cell in &self.cells {
            lines.push(format!(
                "S7 {} {}: {} rounds, {} applied, {} not applied, {} violations, {} unjudgeable, \
                 {} unprobed",
                cell.group.as_str(),
                cell.window.as_str(),
                cell.rounds,
                cell.applied,
                cell.not_applied,
                cell.violations,
                cell.unjudgeable,
                cell.unprobed
            ));
        }
        lines
    }
}

fn count(rounds: &[&RevertRound], outcome: Outcome) -> u64 {
    rounds.iter().filter(|r| r.outcome == outcome).count() as u64
}

fn detail_for(
    violation: TruncationViolation,
    round: &RevertRound,
    before: u64,
    expected: u64,
    observed: u64,
) -> String {
    let asked = round.asked_to_revert;
    match violation {
        TruncationViolation::PartialTruncation => format!(
            "the agent was worth {before}, a revert of {asked} invocations should have left it \
             at {expected}, and it came back at {observed} — between the two, so part of the \
             truncation landed and part did not"
        ),
        TruncationViolation::OverTruncation => format!(
            "the agent was worth {before} and a revert of {asked} invocations left it at \
             {observed}, below the {expected} it asked for: more was taken back than requested"
        ),
        TruncationViolation::Divergent => format!(
            "the agent was worth {before} before a revert of {asked} invocations and came back \
             at {observed}, higher than it started: state grew across a revert"
        ),
        TruncationViolation::AcknowledgedButNotApplied => format!(
            "the platform confirmed a revert of {asked} invocations and the agent is still \
             worth {before}, not the {expected} it acknowledged"
        ),
        TruncationViolation::RefusedButApplied => format!(
            "the platform refused a revert of {asked} invocations and the agent moved from \
             {before} to {observed} anyway"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeDelta, Utc};
    use std::collections::BTreeMap;
    use test_r::test;

    const ON_POD: &str = "chaos-s7-revert-0000";
    const CONTROL: &str = "chaos-s7-revert-0001";

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn split() -> PodSplit {
        PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: vec![ON_POD.to_string()],
            elsewhere: vec![CONTROL.to_string()],
            targets_per_pod: BTreeMap::new(),
            number_of_shards: 1024,
        }
    }

    fn fault() -> FaultWindow {
        FaultWindow {
            injected_at: t0(),
            recovered_at: Some(t0() + TimeDelta::seconds(120)),
        }
    }

    /// One round: the agent was worth `before`, asked for 2 invocations back,
    /// the platform answered `outcome`, and afterwards it read `observed`.
    fn round(
        agent: &str,
        offset_secs: i64,
        before: Option<u64>,
        outcome: Outcome,
        observed: Option<u64>,
    ) -> RevertRound {
        let submitted_at = t0() + TimeDelta::seconds(offset_secs);
        RevertRound {
            agent: agent.to_string(),
            round: 0,
            before_revert: before,
            asked_to_revert: 2,
            outcome,
            submitted_at,
            completed_at: Some(submitted_at + TimeDelta::milliseconds(80)),
            observed_after: observed,
        }
    }

    fn build(rounds: &[RevertRound]) -> TruncationReport {
        TruncationReport::build(rounds, &split(), Some(fault()), 4, 2)
    }

    fn violations(report: &TruncationReport) -> Vec<TruncationViolation> {
        report.findings.iter().map(|f| f.violation).collect()
    }

    /// The healthy shape: the platform said yes and the agent landed exactly on
    /// the post-revert value.
    #[test]
    fn a_revert_that_landed_where_it_said_it_would_is_not_a_finding() {
        let report = build(&[round(ON_POD, -10, Some(10), Outcome::Confirmed, Some(8))]);
        assert!(violations(&report).is_empty(), "{:?}", report.findings);
        assert_eq!(report.applied_exactly, 1);
        assert!(!report.has_violations());
    }

    /// The headline finding this scenario exists to make: the counter landed
    /// between the two values it was allowed to have.
    #[test]
    fn a_counter_between_the_two_legal_values_is_a_torn_truncation() {
        // Worth 10, asked for 4 back so 6 was legal, came back at 8.
        let mut r = round(ON_POD, -10, Some(10), Outcome::Confirmed, Some(8));
        r.asked_to_revert = 4;
        let report = build(&[r]);

        assert_eq!(
            violations(&report),
            vec![TruncationViolation::PartialTruncation]
        );
        let finding = &report.findings[0];
        assert_eq!(finding.before, 10);
        assert_eq!(finding.expected_after_commit, 6);
        assert_eq!(finding.observed, 8);
        assert!(report.has_violations(), "this must be able to fail the run");
    }

    /// The same shape as an accepted promise completion that never woke its
    /// waiter: the durable half was acknowledged and nothing moved.
    #[test]
    fn a_confirmed_revert_that_changed_nothing_is_a_finding() {
        let report = build(&[round(ON_POD, -10, Some(10), Outcome::Confirmed, Some(10))]);
        assert_eq!(
            violations(&report),
            vec![TruncationViolation::AcknowledgedButNotApplied]
        );
    }

    /// Its opposite, and just as serious: refused, and applied regardless.
    #[test]
    fn a_refused_revert_that_applied_anyway_is_a_finding() {
        let report = build(&[round(ON_POD, -10, Some(10), Outcome::Rejected, Some(8))]);
        assert_eq!(
            violations(&report),
            vec![TruncationViolation::RefusedButApplied]
        );
    }

    #[test]
    fn taking_back_more_than_was_asked_for_is_a_finding() {
        let report = build(&[round(ON_POD, -10, Some(10), Outcome::Confirmed, Some(5))]);
        assert_eq!(
            violations(&report),
            vec![TruncationViolation::OverTruncation]
        );
    }

    #[test]
    fn state_growing_across_a_revert_is_a_finding() {
        let report = build(&[round(ON_POD, -10, Some(10), Outcome::Confirmed, Some(11))]);
        assert_eq!(violations(&report), vec![TruncationViolation::Divergent]);
    }

    /// A revert the driver never heard back about is doubt, not damage. Both
    /// answers are legitimate and neither is a finding.
    #[test]
    fn a_revert_in_doubt_may_land_either_way_without_being_a_finding() {
        let report = build(&[
            round(ON_POD, -10, Some(10), Outcome::Indeterminate, Some(8)),
            round(CONTROL, -10, Some(10), Outcome::Indeterminate, Some(10)),
        ]);
        assert!(violations(&report).is_empty(), "{:?}", report.findings);
        assert_eq!(report.indeterminate_that_applied, 1);
        assert_eq!(report.indeterminate_that_did_not, 1);
    }

    /// A round whose increments never answered has no pre-revert value, so it
    /// cannot be judged. Counted, never guessed at.
    #[test]
    fn a_round_whose_increments_never_answered_is_not_judged() {
        let report = build(&[round(ON_POD, -10, None, Outcome::Confirmed, Some(8))]);
        assert_eq!(report.unjudgeable, 1);
        assert!(report.findings.is_empty());
    }

    /// The last round an agent ran has nothing after it to probe with. It is
    /// counted rather than assumed clean.
    #[test]
    fn a_round_no_increment_ever_probed_is_counted_as_unprobed() {
        let report = build(&[round(ON_POD, -10, Some(10), Outcome::Confirmed, None)]);
        assert_eq!(report.unprobed, 1);
        assert!(report.findings.is_empty());
    }

    /// The S10 lesson: a kill that caught none of the mechanism proves nothing
    /// about it, and the run has to say so rather than read as clean.
    #[test]
    fn a_kill_that_caught_no_revert_says_the_run_proved_nothing() {
        // Both rounds resolved well before the kill.
        let report = build(&[
            round(ON_POD, -100, Some(10), Outcome::Confirmed, Some(8)),
            round(CONTROL, -100, Some(10), Outcome::Confirmed, Some(8)),
        ]);
        assert!(report.caught_by_the_kill.is_empty());
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|l| l.contains("caught no revert in flight")),
            "attention was {:?}",
            report.attention_lines()
        );
    }

    /// A revert still unresolved when the pod died is the population the whole
    /// scenario is about, and it has to be counted apart from the rest.
    #[test]
    fn reverts_unresolved_when_the_pod_died_are_reported_separately() {
        let mut caught = round(ON_POD, -1, Some(10), Outcome::Indeterminate, Some(10));
        caught.completed_at = Some(t0() + TimeDelta::seconds(30));
        let report = build(&[caught]);

        let entry = report
            .caught_by_the_kill
            .iter()
            .find(|c| c.group == Group::OnPod)
            .expect("the kill caught a revert");
        assert_eq!(entry.reverts, 1);
        assert_eq!(entry.agents, 1);
        assert_eq!(entry.indeterminate, 1);
        assert!(
            !report
                .attention_lines()
                .iter()
                .any(|l| l.contains("caught no revert in flight"))
        );
    }

    /// Rounds are split by which executor owned the agent and which side of the
    /// kill they fell on, so a control group cannot hide a disturbed one.
    #[test]
    fn rounds_are_split_by_group_and_window() {
        let report = build(&[
            round(ON_POD, -10, Some(10), Outcome::Confirmed, Some(8)),
            round(ON_POD, 10, Some(12), Outcome::Confirmed, Some(10)),
            round(CONTROL, 10, Some(10), Outcome::Confirmed, Some(8)),
        ]);
        let cell = |g, w| {
            report
                .cells
                .iter()
                .find(|c| c.group == g && c.window == w)
                .cloned()
        };
        assert_eq!(cell(Group::OnPod, Window::BeforeFault).unwrap().rounds, 1);
        assert_eq!(cell(Group::OnPod, Window::DuringFault).unwrap().rounds, 1);
        assert_eq!(
            cell(Group::Elsewhere, Window::DuringFault).unwrap().rounds,
            1
        );
    }

    #[test]
    fn findings_beyond_the_cap_are_counted_rather_than_carried() {
        let rounds: Vec<RevertRound> = (0..MAX_FINDINGS + 7)
            .map(|_| round(ON_POD, -10, Some(10), Outcome::Confirmed, Some(10)))
            .collect();
        let report = build(&rounds);
        assert_eq!(report.findings.len(), MAX_FINDINGS);
        assert_eq!(report.findings_omitted, 7);
        assert!(report.has_violations());
    }

    /// A finding is read by an operator mid-window, so it has to state the
    /// three numbers that make it interpretable without the history to hand.
    #[test]
    fn a_finding_states_the_numbers_that_make_it_readable() {
        let report = build(&[round(ON_POD, -10, Some(10), Outcome::Confirmed, Some(10))]);
        let detail = &report.findings[0].detail;
        assert!(detail.contains("10"), "{detail}");
        assert!(detail.contains('8'), "{detail}");
        assert!(
            !detail.contains("Some("),
            "Option formatting leaked: {detail}"
        );
    }
}
