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

//! Did every deleted agent stay deleted (GOL-372)?
//!
//! The same two-value oracle [`crate::chaos::truncation`] uses, one step
//! further. There a round asked the platform to forget some of an agent's work;
//! here it asks it to forget the agent. Invoking a deleted id creates a **new**
//! agent, so the counter a deletion leaves behind has exactly two legal values:
//!
//! * `0` — the deletion took, and the id is a fresh agent counting from nothing
//! * `V` — the deletion did not take, and the old agent is still there
//!
//! Anything else means an agent came back carrying part of a state it was
//! supposed to have lost.
//!
//! ### Which of the two is the finding
//!
//! Neither, on its own. Both are legitimate outcomes of a delete the driver
//! never heard back about — a lost response leaves the question genuinely open.
//! What makes one a defect is the platform's own answer next to it:
//!
//! * confirmed, and the agent is still worth `V` → **resurrection**. The
//!   platform said the agent was gone and it is not.
//! * refused, and the agent is gone → the opposite, and just as wrong.
//!
//! ### Why this is worth a scenario at all
//!
//! Because the happy path is already defended and the crash path is not
//! obviously so. `Worker::start_deleting` in the executor exists specifically to
//! stop a background status flush from "resurrecting the cached status" after
//! the durable removal — its own comment. Deleting is four steps (interrupt,
//! mark, remove from the worker service, remove from the active set) and only
//! the third is durable, so a pod that dies between the mark and the removal
//! leaves an agent marked for deletion that was never removed. Whoever picks up
//! its shard next decides what that means.

use crate::chaos::deletions::{COUNTER_OF_A_NEW_AGENT, DeleteRound};
use crate::chaos::history::Outcome;
use crate::chaos::split::{FaultWindow, Group, PodSplit, Window};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The most findings the report carries.
const MAX_FINDINGS: usize = 50;

/// What went wrong with one deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResurrectionViolation {
    /// The platform confirmed the deletion and the agent came back carrying the
    /// value it had before. The failure this scenario is named for.
    ResurrectedWithState,
    /// The agent came back worth neither nothing nor what it had been: part of
    /// a state it was supposed to have lost survived.
    PartialState,
    /// The platform refused the deletion and the agent is gone anyway, for a
    /// reason other than not finding it.
    ///
    /// A refusal that says the agent was not there is **not** this: see
    /// [`ResurrectionReport::deleted_despite_not_found`].
    RefusedButDeleted,
}

impl ResurrectionViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            ResurrectionViolation::ResurrectedWithState => "resurrected-with-state",
            ResurrectionViolation::PartialState => "partial-state",
            ResurrectionViolation::RefusedButDeleted => "refused-but-deleted",
        }
    }
}

/// One violation, against one named round.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResurrectionFinding {
    pub violation: ResurrectionViolation,
    pub agent: String,
    pub round: u32,
    pub window: Window,
    /// What the agent was worth before the delete, and what the slot reported
    /// afterwards. Carried so a finding reads without the history.
    pub before: u64,
    pub observed: u64,
    pub detail: String,
}

/// Rounds and their verdicts for one (group, window) cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResurrectionCell {
    pub group: Group,
    pub window: Window,
    pub rounds: u64,
    /// Deletions that took: the slot came back as a new agent.
    pub deleted: u64,
    /// Deletions that did not. Legitimate only when the platform never
    /// confirmed them.
    pub survived: u64,
    pub violations: u64,
    pub unjudgeable: u64,
    pub unprobed: u64,
}

/// Deletions the kill landed in the middle of.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletesCaught {
    pub group: Group,
    /// Deletes submitted before the kill that had not answered when it landed.
    /// The population the scenario is actually about.
    pub deletes: u64,
    pub agents: usize,
    pub confirmed: u64,
    pub indeterminate: u64,
    pub rejected: u64,
}

/// The resurrection account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResurrectionReport {
    /// What each round was configured to do, so an archived finding reads
    /// without the suite YAML.
    pub increments_per_round: u32,
    pub rounds_recorded: u64,
    pub deletes_confirmed: u64,
    pub deletes_indeterminate: u64,
    pub deletes_rejected: u64,
    /// Confirmed deletes whose slot came back as a new agent.
    pub deleted_exactly: u64,
    /// Deletes the driver never heard back about that took anyway.
    pub indeterminate_that_deleted: u64,
    /// Deletes the driver never heard back about that did not.
    pub indeterminate_that_did_not: u64,
    /// Deletions the platform reported as `AGENT_NOT_FOUND` whose agent was
    /// nonetheless gone afterwards.
    ///
    /// Not a violation, and the distinction matters. Deleting is not
    /// idempotent — `delete_worker_internal` opens with a metadata lookup and
    /// returns not-found when there is nothing there — and worker-service's
    /// routing layer retries a call whose executor became unreachable. So a
    /// delete that succeeded, on an executor that then died, is retried against
    /// the new owner and comes back as not-found. The work happened; the answer
    /// is misleading. That is worth an operator's attention and is not the
    /// platform resurrecting anything.
    pub deleted_despite_not_found: u64,
    pub unjudgeable: u64,
    pub unprobed: u64,
    pub cells: Vec<ResurrectionCell>,
    pub caught_by_the_kill: Vec<DeletesCaught>,
    pub findings: Vec<ResurrectionFinding>,
    pub findings_omitted: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Deleted,
    /// Gone, but reported to the caller as "no such agent".
    DeletedDespiteNotFound,
    Survived,
    Violation(ResurrectionViolation),
    Unjudgeable,
    Unprobed,
}

/// Judges one round against the two values it is allowed to have landed on.
fn judge(round: &DeleteRound) -> Verdict {
    let Some(before) = round.before_delete else {
        return Verdict::Unjudgeable;
    };
    let Some(observed) = round.observed_after else {
        return Verdict::Unprobed;
    };

    if observed == COUNTER_OF_A_NEW_AGENT {
        // Gone. Legitimate unless the platform said it refused — and even then,
        // one refusal means the opposite of what it looks like. See below.
        if round.outcome == Outcome::Rejected {
            if round.rejected_as_not_found {
                return Verdict::DeletedDespiteNotFound;
            }
            return Verdict::Violation(ResurrectionViolation::RefusedButDeleted);
        }
        return Verdict::Deleted;
    }
    if observed == before {
        // Still there. Legitimate unless the platform said it was gone.
        if round.outcome == Outcome::Confirmed {
            return Verdict::Violation(ResurrectionViolation::ResurrectedWithState);
        }
        return Verdict::Survived;
    }
    Verdict::Violation(ResurrectionViolation::PartialState)
}

impl ResurrectionReport {
    /// Builds the account from the rounds the workload recorded.
    pub fn build(
        rounds: &[DeleteRound],
        split: &PodSplit,
        fault: Option<FaultWindow>,
        increments_per_round: u32,
    ) -> Self {
        let mut cells: BTreeMap<(Group, Window), ResurrectionCell> = BTreeMap::new();
        let mut findings: Vec<ResurrectionFinding> = Vec::new();
        let mut report = ResurrectionReport {
            increments_per_round,
            rounds_recorded: rounds.len() as u64,
            deletes_confirmed: 0,
            deletes_indeterminate: 0,
            deletes_rejected: 0,
            deleted_exactly: 0,
            indeterminate_that_deleted: 0,
            indeterminate_that_did_not: 0,
            deleted_despite_not_found: 0,
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
                .or_insert_with(|| ResurrectionCell {
                    group,
                    window,
                    rounds: 0,
                    deleted: 0,
                    survived: 0,
                    violations: 0,
                    unjudgeable: 0,
                    unprobed: 0,
                });
            cell.rounds += 1;

            match round.outcome {
                Outcome::Confirmed => report.deletes_confirmed += 1,
                Outcome::Indeterminate => report.deletes_indeterminate += 1,
                Outcome::Rejected => report.deletes_rejected += 1,
            }

            match judge(round) {
                Verdict::Deleted => {
                    cell.deleted += 1;
                    if round.outcome == Outcome::Confirmed {
                        report.deleted_exactly += 1;
                    } else {
                        report.indeterminate_that_deleted += 1;
                    }
                }
                Verdict::DeletedDespiteNotFound => {
                    cell.deleted += 1;
                    report.deleted_despite_not_found += 1;
                }
                Verdict::Survived => {
                    cell.survived += 1;
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
                    let before = round.before_delete.unwrap_or_default();
                    let observed = round.observed_after.unwrap_or_default();
                    findings.push(ResurrectionFinding {
                        violation,
                        agent: round.agent.clone(),
                        round: round.round,
                        window,
                        before,
                        observed,
                        detail: detail_for(violation, before, observed),
                    });
                }
            }
        }

        if let Some(window) = fault {
            let mut by_group: BTreeMap<Group, Vec<&DeleteRound>> = BTreeMap::new();
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
                let agents: BTreeSet<&str> = caught.iter().map(|r| r.agent.as_str()).collect();
                report.caught_by_the_kill.push(DeletesCaught {
                    group,
                    deletes: caught.len() as u64,
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
                    "S6 {}: {} round {} — {}",
                    f.violation.as_str(),
                    f.agent,
                    f.round,
                    f.detail
                )
            })
            .collect();
        if self.findings_omitted > 0 {
            lines.push(format!(
                "S6: {} further resurrection finding(s) were dropped from the report",
                self.findings_omitted
            ));
        }

        if self.deleted_despite_not_found > 0 {
            lines.push(format!(
                "S6: {} deletion(s) came back as AGENT_NOT_FOUND and had in fact taken effect. \
                 Deleting is not idempotent and worker-service retries a call whose executor \
                 became unreachable, so a delete that succeeded on a dying pod is reported to \
                 the caller as though the agent had never existed. The work happened; the \
                 answer did not say so.",
                self.deleted_despite_not_found
            ));
        }

        let caught: u64 = self
            .caught_by_the_kill
            .iter()
            .filter(|c| c.group == Group::OnPod)
            .map(|c| c.deletes)
            .sum();
        if caught == 0 {
            lines.push(
                "S6: the kill caught no delete in flight on the targeted executor, so this run \
                 says nothing about crashing during a deletion. Every verdict below describes \
                 deletes that completed either side of it."
                    .to_string(),
            );
        }
        lines
    }

    /// Lines a reader needs in order to interpret the run.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "S6: {} rounds of {} increments then a delete; {} confirmed, {} in doubt, {} refused",
            self.rounds_recorded,
            self.increments_per_round,
            self.deletes_confirmed,
            self.deletes_indeterminate,
            self.deletes_rejected
        )];
        lines.push(format!(
            "S6: {} confirmed deletes left a slot that came back as a new agent; {} in doubt \
             deleted anyway, {} in doubt did not",
            self.deleted_exactly, self.indeterminate_that_deleted, self.indeterminate_that_did_not
        ));
        if self.unjudgeable > 0 || self.unprobed > 0 {
            lines.push(format!(
                "S6: {} rounds could not be judged (an increment never answered) and {} were \
                 never probed by a following increment",
                self.unjudgeable, self.unprobed
            ));
        }
        for caught in &self.caught_by_the_kill {
            lines.push(format!(
                "S6 {}: {} deletes across {} agents were unresolved when the kill landed — {} \
                 confirmed, {} in doubt, {} refused",
                caught.group.as_str(),
                caught.deletes,
                caught.agents,
                caught.confirmed,
                caught.indeterminate,
                caught.rejected
            ));
        }
        for cell in &self.cells {
            lines.push(format!(
                "S6 {} {}: {} rounds, {} deleted, {} survived, {} violations, {} unjudgeable, \
                 {} unprobed",
                cell.group.as_str(),
                cell.window.as_str(),
                cell.rounds,
                cell.deleted,
                cell.survived,
                cell.violations,
                cell.unjudgeable,
                cell.unprobed
            ));
        }
        lines
    }
}

fn count(rounds: &[&DeleteRound], outcome: Outcome) -> u64 {
    rounds.iter().filter(|r| r.outcome == outcome).count() as u64
}

fn detail_for(violation: ResurrectionViolation, before: u64, observed: u64) -> String {
    match violation {
        ResurrectionViolation::ResurrectedWithState => format!(
            "the platform confirmed the deletion of an agent worth {before}, and the slot came \
             back worth {observed} — the old agent is still there with its state intact"
        ),
        ResurrectionViolation::PartialState => format!(
            "the agent was worth {before} when it was deleted and the slot came back worth \
             {observed}, which is neither a new agent ({COUNTER_OF_A_NEW_AGENT}) nor the old \
             one ({before}): part of a state it was supposed to have lost survived"
        ),
        ResurrectionViolation::RefusedButDeleted => format!(
            "the platform refused to delete an agent worth {before} and it is gone anyway — the \
             slot came back worth {observed}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeDelta, Utc};
    use test_r::test;

    const ON_POD: &str = "chaos-s6-delete-0000";
    const CONTROL: &str = "chaos-s6-delete-0001";
    /// What a round builds an agent up to before deleting it.
    const BEFORE: u64 = 3;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-25T12:00:00Z")
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

    fn round(
        agent: &str,
        offset_secs: i64,
        before: Option<u64>,
        outcome: Outcome,
        observed: Option<u64>,
    ) -> DeleteRound {
        let submitted_at = t0() + TimeDelta::seconds(offset_secs);
        DeleteRound {
            agent: agent.to_string(),
            round: 0,
            before_delete: before,
            outcome,
            rejected_as_not_found: false,
            submitted_at,
            completed_at: Some(submitted_at + TimeDelta::milliseconds(90)),
            observed_after: observed,
        }
    }

    fn build(rounds: &[DeleteRound]) -> ResurrectionReport {
        ResurrectionReport::build(rounds, &split(), Some(fault()), 3)
    }

    fn violations(report: &ResurrectionReport) -> Vec<ResurrectionViolation> {
        report.findings.iter().map(|f| f.violation).collect()
    }

    /// The healthy shape: the platform said the agent was gone, and the slot
    /// came back counting from nothing.
    #[test]
    fn a_deletion_that_took_is_not_a_finding() {
        let report = build(&[round(
            ON_POD,
            -10,
            Some(BEFORE),
            Outcome::Confirmed,
            Some(0),
        )]);
        assert!(violations(&report).is_empty(), "{:?}", report.findings);
        assert_eq!(report.deleted_exactly, 1);
        assert!(!report.has_violations());
    }

    /// The failure this scenario is named for.
    #[test]
    fn a_confirmed_deletion_whose_agent_came_back_is_a_resurrection() {
        // Worth 3, deleted, and the slot reported 4 — the old agent, incremented.
        let report = build(&[round(
            ON_POD,
            -10,
            Some(BEFORE),
            Outcome::Confirmed,
            Some(BEFORE),
        )]);
        assert_eq!(
            violations(&report),
            vec![ResurrectionViolation::ResurrectedWithState]
        );
        assert!(report.has_violations(), "this must be able to fail the run");
        let finding = &report.findings[0];
        assert_eq!(finding.before, BEFORE);
        assert_eq!(finding.observed, BEFORE);
    }

    /// Its opposite: refused, and gone regardless.
    #[test]
    fn a_refused_deletion_that_happened_anyway_is_a_finding() {
        let report = build(&[round(ON_POD, -10, Some(BEFORE), Outcome::Rejected, Some(0))]);
        assert_eq!(
            violations(&report),
            vec![ResurrectionViolation::RefusedButDeleted]
        );
    }

    /// Neither a new agent nor the old one: some of a state that was supposed
    /// to be gone survived.
    #[test]
    fn a_slot_that_came_back_part_way_is_a_finding() {
        // Worth 3, so the delete was allowed to leave 0 or 3. It left 2.
        let report = build(&[round(
            ON_POD,
            -10,
            Some(BEFORE),
            Outcome::Confirmed,
            Some(2),
        )]);
        assert_eq!(
            violations(&report),
            vec![ResurrectionViolation::PartialState]
        );
        assert!(
            report.findings[0].detail.contains("neither a new agent"),
            "{}",
            report.findings[0].detail
        );
    }

    /// A delete the driver never heard back about may land either way, and
    /// neither answer is a defect.
    #[test]
    fn a_deletion_in_doubt_may_land_either_way_without_being_a_finding() {
        let report = build(&[
            round(ON_POD, -10, Some(BEFORE), Outcome::Indeterminate, Some(0)),
            round(
                CONTROL,
                -10,
                Some(BEFORE),
                Outcome::Indeterminate,
                Some(BEFORE),
            ),
        ]);
        assert!(violations(&report).is_empty(), "{:?}", report.findings);
        assert_eq!(report.indeterminate_that_deleted, 1);
        assert_eq!(report.indeterminate_that_did_not, 1);
    }

    /// Why `incrementsPerRound` must be at least two, stated exactly.
    ///
    /// The two legal answers never collide: a fresh agent reports 1 and a
    /// survivor reports `before + 1`. What one increment removes is the *gap*
    /// between them, and the gap is where a partial state would show up. At
    /// `before = 1` the answers are 1 and 2 with nothing in between, so
    /// `PartialState` cannot fire whatever the platform does.
    #[test]
    fn one_increment_leaves_no_room_for_a_partial_state_to_be_seen() {
        // At before = 3 there is room, and a slot landing in it is caught.
        let seen = build(&[round(ON_POD, -10, Some(3), Outcome::Confirmed, Some(2))]);
        assert_eq!(violations(&seen), vec![ResurrectionViolation::PartialState]);

        // At before = 1 every value is one of the two legal answers, so no
        // observation can ever produce this finding. `require_delete` refuses
        // the configuration rather than shipping a blind third of the oracle.
        for observed in [0, 1] {
            let report = build(&[round(
                ON_POD,
                -10,
                Some(1),
                Outcome::Indeterminate,
                Some(observed),
            )]);
            assert!(
                !violations(&report).contains(&ResurrectionViolation::PartialState),
                "observed {observed} should be legal at before=1"
            );
        }
    }

    #[test]
    fn a_round_whose_increments_never_answered_is_not_judged() {
        let report = build(&[round(ON_POD, -10, None, Outcome::Confirmed, Some(0))]);
        assert_eq!(report.unjudgeable, 1);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn a_round_no_increment_ever_probed_is_counted_as_unprobed() {
        let report = build(&[round(ON_POD, -10, Some(BEFORE), Outcome::Confirmed, None)]);
        assert_eq!(report.unprobed, 1);
        assert!(report.findings.is_empty());
    }

    /// The S10 lesson: a kill that caught nothing proves nothing, and the run
    /// has to say so rather than read as clean.
    #[test]
    fn a_kill_that_caught_no_delete_says_the_run_proved_nothing() {
        let report = build(&[round(
            ON_POD,
            -100,
            Some(BEFORE),
            Outcome::Confirmed,
            Some(0),
        )]);
        assert!(report.caught_by_the_kill.is_empty());
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|l| l.contains("caught no delete in flight")),
            "attention was {:?}",
            report.attention_lines()
        );
    }

    /// A delete still unresolved when the pod died is the population the whole
    /// scenario is about.
    #[test]
    fn deletes_unresolved_when_the_pod_died_are_reported_separately() {
        let mut caught = round(ON_POD, -1, Some(BEFORE), Outcome::Indeterminate, Some(0));
        caught.completed_at = Some(t0() + TimeDelta::seconds(30));
        let report = build(&[caught]);

        let entry = report
            .caught_by_the_kill
            .iter()
            .find(|c| c.group == Group::OnPod)
            .expect("the kill caught a delete");
        assert_eq!(entry.deletes, 1);
        assert_eq!(entry.indeterminate, 1);
        assert!(
            !report
                .attention_lines()
                .iter()
                .any(|l| l.contains("caught no delete in flight"))
        );
    }

    #[test]
    fn rounds_are_split_by_group_and_window() {
        let report = build(&[
            round(ON_POD, -10, Some(BEFORE), Outcome::Confirmed, Some(0)),
            round(ON_POD, 10, Some(BEFORE), Outcome::Confirmed, Some(0)),
            round(CONTROL, 10, Some(BEFORE), Outcome::Confirmed, Some(0)),
        ]);
        let cell = |g, w| {
            report
                .cells
                .iter()
                .find(|c| c.group == g && c.window == w)
                .cloned()
        };
        assert_eq!(cell(Group::OnPod, Window::BeforeFault).unwrap().rounds, 1);
        assert_eq!(cell(Group::OnPod, Window::DuringFault).unwrap().deleted, 1);
        assert_eq!(
            cell(Group::Elsewhere, Window::DuringFault).unwrap().rounds,
            1
        );
    }

    /// The bug the first S6 run reported 125 times, pinned.
    ///
    /// A slot's last round has no increment after it and is closed by a plain
    /// read instead. A read reports the counter directly; an increment reports
    /// the counter it just raised. `observed_after` means the counter the
    /// deletion left behind, so the *workload* subtracts one from the increment
    /// and the final read stores its value as-is. Get that backwards and every
    /// slot's last round reads as a partial state — 125 findings out of 138,275
    /// rounds, all in `after-fault`, all with the same shape.
    #[test]
    fn a_round_closed_by_the_final_read_is_judged_on_the_same_scale() {
        // A deleted agent reads 0. That is the fresh value, not a partial one.
        let closed_by_read = build(&[round(
            ON_POD,
            -10,
            Some(BEFORE),
            Outcome::Confirmed,
            Some(COUNTER_OF_A_NEW_AGENT),
        )]);
        assert!(
            violations(&closed_by_read).is_empty(),
            "a final read of a deleted agent must not read as a partial state: {:?}",
            closed_by_read.findings
        );
        assert_eq!(closed_by_read.deleted_exactly, 1);

        // And a survivor read reports what it was worth, not one more.
        let survivor = build(&[round(
            ON_POD,
            -10,
            Some(BEFORE),
            Outcome::Indeterminate,
            Some(BEFORE),
        )]);
        assert!(violations(&survivor).is_empty(), "{:?}", survivor.findings);
        assert_eq!(survivor.indeterminate_that_did_not, 1);
    }

    /// The other thing the first run found, and the reason it is not a finding.
    ///
    /// Deleting is not idempotent, and worker-service retries a call whose
    /// executor became unreachable. So a delete that succeeded on a pod that
    /// then died is retried against the new owner and comes back
    /// `AGENT_NOT_FOUND`. The agent really is gone; only the answer is wrong.
    #[test]
    fn a_not_found_refusal_whose_agent_is_gone_is_reported_not_failed() {
        let mut r = round(
            ON_POD,
            -10,
            Some(BEFORE),
            Outcome::Rejected,
            Some(COUNTER_OF_A_NEW_AGENT),
        );
        r.rejected_as_not_found = true;
        let report = build(&[r]);

        assert!(violations(&report).is_empty(), "{:?}", report.findings);
        assert_eq!(report.deleted_despite_not_found, 1);
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|l| l.contains("AGENT_NOT_FOUND") && l.contains("had in fact taken effect")),
            "the operator still has to be told: {:?}",
            report.attention_lines()
        );
    }

    /// Any *other* refusal with the agent gone is still a finding. The
    /// not-found case is an exception with a mechanism behind it, not a blanket
    /// excuse for refusals.
    #[test]
    fn a_refusal_that_is_not_about_finding_the_agent_still_fails_the_run() {
        let mut r = round(
            ON_POD,
            -10,
            Some(BEFORE),
            Outcome::Rejected,
            Some(COUNTER_OF_A_NEW_AGENT),
        );
        r.rejected_as_not_found = false;
        let report = build(&[r]);

        assert_eq!(
            violations(&report),
            vec![ResurrectionViolation::RefusedButDeleted]
        );
        assert!(report.has_violations());
    }

    #[test]
    fn findings_beyond_the_cap_are_counted_rather_than_carried() {
        let rounds: Vec<DeleteRound> = (0..MAX_FINDINGS + 4)
            .map(|_| round(ON_POD, -10, Some(BEFORE), Outcome::Confirmed, Some(BEFORE)))
            .collect();
        let report = build(&rounds);
        assert_eq!(report.findings.len(), MAX_FINDINGS);
        assert_eq!(report.findings_omitted, 4);
        assert!(report.has_violations());
    }
}
