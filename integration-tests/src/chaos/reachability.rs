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

//! What a partition between worker-service and one executor cost (GOL-370).
//!
//! Three questions, in the order they have to be answered:
//!
//! 1. **Did the fault land?** The agents on the isolated executor must stop
//!    being served. If they did not, nothing else in the report means anything,
//!    and it says so — see [`ReachabilityViolation::PartitionNotObserved`].
//! 2. **What did it cost the agents it was not aimed at?** The other executor
//!    is reachable throughout and its agents should be untouched. They share a
//!    worker-service with the stalled half, and worker-service keeps one
//!    process-wide routing table that every stalled caller invalidates, so
//!    "untouched" is a claim worth measuring rather than assuming.
//! 3. **Did the isolated agents come back?** Every one of them, and how long
//!    after the link was restored.
//!
//! Throughput, not success rate, is what the first two are measured on, and the
//! difference matters. An emitter holds one operation at a time
//! ([`crate::chaos::steady`]), so an agent whose executor is unreachable does
//! not fail repeatedly — it fails *slowly*, once, and offers nothing else for
//! two minutes. A success rate would read that as one failure out of one
//! attempt and call the group 0% degraded. Confirmed operations per second is
//! the number that collapses, so it is the number the report is built on.
//!
//! ### Reading a non-zero isolated cell
//!
//! Operations are placed in the window they were *submitted* in, which is when
//! the platform was asked to do the work. An isolated operation submitted late
//! in the fault window can still be waiting when the link comes back, and then
//! confirms — so the isolated group's during-fault throughput is small rather
//! than exactly zero. At one operation per agent per second against a
//! two-minute client timeout, that is a handful of confirmations against a
//! baseline of hundreds. A cell anywhere near the ceiling means something else.

use crate::chaos::history::{OperationRecord, Outcome, Stream};
use crate::chaos::split::{
    FaultWindow, Group, PodSplit, Window, longest_silence_ms, round2, window_end, window_secs,
    window_start,
};
use crate::chaos::summary::LatencyStats;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The most findings the report carries. Past this it says how many it dropped
/// rather than growing without bound: 200 agents that all failed to recover is
/// one fact, not 200.
const MAX_FINDINGS: usize = 50;

/// What a reachability finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReachabilityViolation {
    /// The isolated executor kept serving its agents through the fault. The
    /// partition did not take hold where the run says it did, and every other
    /// number here describes an undisturbed cluster.
    PartitionNotObserved,
    /// The agents on the *reachable* executor lost throughput while the other
    /// executor was cut off. They were never partitioned from anything.
    ControlDegraded,
    /// An isolated agent produced no confirmed operation at all once the link
    /// was restored.
    NeverRecovered,
}

impl ReachabilityViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            ReachabilityViolation::PartitionNotObserved => "partition-not-observed",
            ReachabilityViolation::ControlDegraded => "control-degraded",
            ReachabilityViolation::NeverRecovered => "never-recovered",
        }
    }
}

/// One finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReachabilityFinding {
    pub violation: ReachabilityViolation,
    /// The agent it localises to, for the findings that localise to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub detail: String,
}

/// What one group of agents managed in one window.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThroughputCell {
    /// `on-pod` is the group the isolated executor owns; `elsewhere` is the
    /// control group. The names are the suite's, shared with every other
    /// scenario that divides its agents around one pod — see
    /// [`crate::chaos::split`].
    pub group: Group,
    pub window: Window,
    /// Agents of this group that offered at least one operation in this window.
    /// Below the group's size means emitters were stalled across the whole
    /// window rather than merely slowed.
    pub agents_active: usize,
    /// Operations *offered* in this window, and how they eventually ended up.
    /// Attributed by submission time, so an operation counted here may not have
    /// been answered until a later window.
    pub submitted: u64,
    pub confirmed: u64,
    pub rejected: u64,
    pub indeterminate: u64,
    /// Operations *answered* in this window, whenever they were offered.
    ///
    /// This is the one that says whether the group was being served, and it is
    /// deliberately not `confirmed`: while an executor is unreachable the two
    /// differ by exactly the work that was accepted and answered only once the
    /// partition came down.
    pub served: u64,
    /// Attempts that hit the client's attempt timeout rather than answering.
    /// The pending-then-timeout behaviour the scenario exists to make visible.
    pub attempts_timed_out: u64,
    pub window_secs: f64,
    pub served_per_sec: f64,
    /// This cell's rate against the same group's own before-fault rate. `None`
    /// for the before-fault cell itself, and for a group that never had a
    /// baseline to compare against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_of_baseline_percent: Option<f64>,
    /// The longest the group was answered nothing at all, anywhere in this
    /// window.
    ///
    /// The number that stops a small non-zero rate being read as residual
    /// service. A during-fault cell can show a handful of confirmations that
    /// all arrived in the last seconds of the window, once the fault was
    /// already coming down. Measured against the window's own edges, so a group
    /// that fell silent at the start or stayed silent to the end is caught by
    /// it too, and a `quietMs` close to `windowSecs` is a total outage however
    /// the rate arithmetic came out.
    ///
    /// `None` when the window has no fixed bounds to measure against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_ms: Option<u64>,
    pub latency: LatencyStats,
}

/// The operations the fault landed in the middle of.
///
/// The population S3 actually disturbed, and the one number a reader wants
/// first. It cannot be read off the cells: these operations were *submitted*
/// before the cut, so every trace of them — their timeouts, their duration —
/// is attributed to the `before-fault` row, which is the last place anyone
/// looks for the damage.
///
/// The control group's entry is the comparison that makes it mean something,
/// but read it on **duration**, not on count. How many operations a healthy
/// group has in flight at any instant is its duty cycle — operation time over
/// interval — so a group answering in 45ms on a one-second cadence has about
/// one agent in twenty busy. A stalled group accumulates instead: every emitter
/// ends up holding an operation that will not return, so its count climbs to
/// the size of the group. A real run showed 113 of 113 against 3 of 87, and the
/// gap between 47ms and 182 seconds is the finding, not the gap between 3 and
/// 113.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaughtInFlight {
    pub group: Group,
    /// Operations submitted before the fault that were still running when it
    /// landed.
    pub operations: u64,
    /// Distinct agents they belonged to. Equal to the group size when every
    /// emitter was mid-operation, which is what one-in-flight-per-agent makes
    /// the normal case.
    pub agents: usize,
    pub confirmed: u64,
    pub rejected: u64,
    pub indeterminate: u64,
    /// Submission to final outcome, across every attempt.
    pub duration: LatencyStats,
    /// Attempts that hit the client's attempt timeout rather than answering.
    pub attempts_timed_out: u64,
    /// The most attempts any one of them needed.
    ///
    /// Load-bearing rather than trivia. An operation that stalled and then
    /// answered on a later attempt was rescued by the caller's retry, not
    /// returned by the platform: with retries off it would have ended
    /// indeterminate. That distinction is invisible in the outcome alone.
    pub max_attempts: u32,
    /// How many were still unresolved when the fault was reported healed.
    pub outlived_the_fault: u64,
}

/// The reachability account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReachabilityReport {
    /// The executor the partition was aimed at, as the shard-manager names it.
    pub isolated_pod: String,
    pub isolated_agents: usize,
    pub reachable_agents: usize,
    /// The thresholds from the suite YAML, recorded so an archived cell can be
    /// read years later against the numbers it was judged by rather than
    /// against today's config.
    pub isolated_ceiling_percent: f64,
    pub control_floor_percent: f64,
    pub recovery_budget_ms: u64,
    pub cells: Vec<ThroughputCell>,
    /// What the fault landed in the middle of, per group. Empty for a run that
    /// never learned when the fault was.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caught_in_flight: Vec<CaughtInFlight>,
    /// Per isolated agent, how long after the link was restored its first
    /// confirmed operation landed.
    pub recovery: LatencyStats,
    pub recovery_over_budget: u64,
    /// Isolated agents that never confirmed anything after the heal.
    pub agents_never_recovered: Vec<String>,
    /// Records whose agent the selection never saw. Zero on a healthy run;
    /// non-zero means the split and the workload disagree about who was driven.
    pub records_outside_the_split: u64,
    pub findings: Vec<ReachabilityFinding>,
    /// Findings past [`MAX_FINDINGS`], dropped rather than carried. Non-zero
    /// means `findings` is a sample.
    pub findings_omitted: u64,
}

/// One group's per-window accumulation, before it becomes a cell.
#[derive(Default)]
struct Tally {
    agents: BTreeSet<String>,
    submitted: u64,
    confirmed: u64,
    rejected: u64,
    indeterminate: u64,
    attempts_timed_out: u64,
    durations: Vec<u64>,
    /// When this group actually answered inside this window, sorted later.
    ///
    /// Keyed on the window a confirmation *landed* in rather than the one its
    /// operation was offered in, which is the only way either of the numbers
    /// derived from it means what it says.
    served_at: Vec<DateTime<Utc>>,
}

impl ReachabilityReport {
    /// Builds the account from the operation history.
    ///
    /// `fault` is what the workflow reported. Without it every record lands in
    /// [`Window::Unknown`] and the report carries counts but no verdict, which
    /// is the honest outcome for a run that never learned when the fault was:
    /// the thresholds are all defined relative to a before-and-during
    /// comparison that cannot be made.
    pub fn build(
        records: &[OperationRecord],
        split: &PodSplit,
        fault: Option<FaultWindow>,
        isolated_ceiling_percent: f64,
        control_floor_percent: f64,
        recovery_budget: std::time::Duration,
    ) -> Self {
        let mut tallies: BTreeMap<(Group, Window), Tally> = BTreeMap::new();
        let mut records_outside_the_split = 0u64;
        let mut first_submitted: Option<DateTime<Utc>> = None;
        let mut last_completed: Option<DateTime<Utc>> = None;

        for record in records.iter().filter(|r| r.stream == Stream::Durable) {
            let Some(group) = split.group_of(&record.agent) else {
                records_outside_the_split += 1;
                continue;
            };
            let window = Window::of(record.submitted_at, fault);
            let tally = tallies.entry((group, window)).or_default();

            tally.agents.insert(record.agent.clone());
            tally.submitted += 1;
            match record.outcome {
                Outcome::Confirmed => {
                    tally.confirmed += 1;
                    tally.durations.push(record.duration_ms);
                }
                Outcome::Rejected => tally.rejected += 1,
                Outcome::Indeterminate => tally.indeterminate += 1,
            }
            tally.attempts_timed_out += record.attempts_timed_out();

            // Answered work is filed under the window it was answered in, which
            // is usually but not always the one it was offered in. An operation
            // offered while the executor was unreachable and answered once the
            // partition came down is not service the fault window delivered,
            // and counting it there is how a total outage reads as partial.
            if record.outcome == Outcome::Confirmed
                && let Some(completed) = record.completed_at
            {
                tallies
                    .entry((group, Window::of(completed, fault)))
                    .or_default()
                    .served_at
                    .push(completed);
            }

            first_submitted = Some(match first_submitted {
                Some(at) if at <= record.submitted_at => at,
                _ => record.submitted_at,
            });
            if let Some(completed) = record.completed_at {
                last_completed = Some(match last_completed {
                    Some(at) if at >= completed => at,
                    _ => completed,
                });
            }
        }

        // Baselines first: every other cell is expressed as a share of its own
        // group's before-fault rate, so a lopsided split cannot make one group
        // look better than the other.
        let mut baseline_rate: BTreeMap<Group, f64> = BTreeMap::new();
        let mut cells: Vec<ThroughputCell> = Vec::new();
        for ((group, window), tally) in &tallies {
            let secs = window_secs(*window, fault, first_submitted, last_completed);
            let served = tally.served_at.len() as u64;
            let rate = if secs > 0.0 {
                served as f64 / secs
            } else {
                0.0
            };
            if *window == Window::BeforeFault {
                baseline_rate.insert(*group, rate);
            }
            let quiet_ms = longest_silence_ms(
                &tally.served_at,
                window_start(*window, fault, first_submitted),
                window_end(*window, fault, last_completed),
            );
            cells.push(ThroughputCell {
                group: *group,
                window: *window,
                agents_active: tally.agents.len(),
                submitted: tally.submitted,
                confirmed: tally.confirmed,
                served,
                rejected: tally.rejected,
                indeterminate: tally.indeterminate,
                attempts_timed_out: tally.attempts_timed_out,
                window_secs: round2(secs),
                served_per_sec: round2(rate),
                share_of_baseline_percent: None,
                quiet_ms,
                latency: LatencyStats::from_durations(tally.durations.clone()),
            });
        }

        for cell in &mut cells {
            if cell.window == Window::BeforeFault {
                continue;
            }
            if let Some(baseline) = baseline_rate.get(&cell.group).filter(|r| **r > 0.0) {
                cell.share_of_baseline_percent =
                    Some(round2(cell.served_per_sec / baseline * 100.0));
            }
        }
        cells.sort_by_key(|c| (c.group, c.window));

        // ── Recovery, per isolated agent ────────────────────────────────────
        let recovered_at = fault.and_then(|w| w.recovered_at);
        let mut gaps: Vec<u64> = Vec::new();
        let mut over_budget = 0u64;
        let mut agents_never_recovered: Vec<String> = Vec::new();
        if let Some(healed) = recovered_at {
            for agent in &split.on_pod {
                let first = records
                    .iter()
                    .filter(|r| {
                        r.stream == Stream::Durable
                            && &r.agent == agent
                            && r.outcome == Outcome::Confirmed
                    })
                    .filter_map(|r| r.completed_at)
                    .filter(|at| *at >= healed)
                    .min();
                match first {
                    Some(at) => {
                        let gap = (at - healed).num_milliseconds().max(0) as u64;
                        if gap > recovery_budget.as_millis() as u64 {
                            over_budget += 1;
                        }
                        gaps.push(gap);
                    }
                    None => agents_never_recovered.push(agent.clone()),
                }
            }
        }

        // ── What the fault landed in the middle of ──────────────────────────
        //
        // Computed from the whole history rather than from the cells, because
        // the cells cannot answer it: these operations were submitted before
        // the cut, so every trace of them sits in the `before-fault` row.
        let mut caught_in_flight: Vec<CaughtInFlight> = Vec::new();
        if let Some(window) = fault {
            let mut by_group: BTreeMap<Group, Vec<&OperationRecord>> = BTreeMap::new();
            for record in records.iter().filter(|r| r.stream == Stream::Durable) {
                let Some(group) = split.group_of(&record.agent) else {
                    continue;
                };
                let still_running = record
                    .completed_at
                    .is_none_or(|at| at >= window.injected_at);
                if record.submitted_at < window.injected_at && still_running {
                    by_group.entry(group).or_default().push(record);
                }
            }
            for (group, caught) in by_group {
                let agents: BTreeSet<&str> = caught.iter().map(|r| r.agent.as_str()).collect();
                caught_in_flight.push(CaughtInFlight {
                    group,
                    operations: caught.len() as u64,
                    agents: agents.len(),
                    confirmed: count_of(&caught, Outcome::Confirmed),
                    rejected: count_of(&caught, Outcome::Rejected),
                    indeterminate: count_of(&caught, Outcome::Indeterminate),
                    duration: LatencyStats::from_durations(
                        caught.iter().map(|r| r.duration_ms).collect(),
                    ),
                    attempts_timed_out: caught.iter().map(|r| r.attempts_timed_out()).sum(),
                    max_attempts: caught.iter().map(|r| r.attempts).max().unwrap_or(0),
                    outlived_the_fault: caught
                        .iter()
                        .filter(|r| match (r.completed_at, window.recovered_at) {
                            // Never finished at all, so it certainly outlived it.
                            (None, _) => true,
                            (Some(done), Some(healed)) => done >= healed,
                            // No heal was ever reported; nothing can be said.
                            (Some(_), None) => false,
                        })
                        .count() as u64,
                });
            }
        }

        let mut report = ReachabilityReport {
            isolated_pod: split.pod_address.clone(),
            isolated_agents: split.on_pod.len(),
            reachable_agents: split.elsewhere.len(),
            isolated_ceiling_percent,
            control_floor_percent,
            recovery_budget_ms: recovery_budget.as_millis() as u64,
            cells,
            caught_in_flight,
            recovery: LatencyStats::from_durations(gaps),
            recovery_over_budget: over_budget,
            agents_never_recovered,
            records_outside_the_split,
            findings: Vec::new(),
            findings_omitted: 0,
        };
        report.judge();
        report
    }

    /// A cell by group and window, if the run produced one.
    pub fn cell(&self, group: Group, window: Window) -> Option<&ThroughputCell> {
        self.cells
            .iter()
            .find(|c| c.group == group && c.window == window)
    }

    fn judge(&mut self) {
        let mut findings: Vec<ReachabilityFinding> = Vec::new();

        // Did the fault land? Asked first, because a "no" makes the rest of the
        // report a description of a cluster nothing happened to.
        if let Some(share) = self
            .cell(Group::OnPod, Window::DuringFault)
            .and_then(|c| c.share_of_baseline_percent)
            && share > self.isolated_ceiling_percent
        {
            findings.push(ReachabilityFinding {
                violation: ReachabilityViolation::PartitionNotObserved,
                agent: None,
                detail: format!(
                    "the {} agents on the isolated executor kept {share:.1}% of their baseline \
                     throughput during the fault, above the {:.0}% ceiling: the partition did \
                     not cut worker-service off from {}. Nothing else in this report describes \
                     a disturbed cluster.",
                    self.isolated_agents, self.isolated_ceiling_percent, self.isolated_pod
                ),
            });
        }

        // What did it cost the half it was not aimed at?
        if let Some(share) = self
            .cell(Group::Elsewhere, Window::DuringFault)
            .and_then(|c| c.share_of_baseline_percent)
            && share < self.control_floor_percent
        {
            findings.push(ReachabilityFinding {
                violation: ReachabilityViolation::ControlDegraded,
                agent: None,
                detail: format!(
                    "the {} agents on the reachable executor kept only {share:.1}% of their \
                     baseline throughput while {} was cut off, below the {:.0}% floor. They \
                     were never partitioned from anything, so this is what serving an \
                     unreachable executor cost the rest of the cluster.",
                    self.reachable_agents, self.isolated_pod, self.control_floor_percent
                ),
            });
        }

        for agent in &self.agents_never_recovered {
            findings.push(ReachabilityFinding {
                violation: ReachabilityViolation::NeverRecovered,
                agent: Some(agent.clone()),
                detail: format!(
                    "{agent} confirmed no operation at all after the link to {} was restored",
                    self.isolated_pod
                ),
            });
        }

        self.findings_omitted = findings.len().saturating_sub(MAX_FINDINGS) as u64;
        findings.truncate(MAX_FINDINGS);
        self.findings = findings;
    }

    /// The lines that need a human.
    pub fn attention_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .findings
            .iter()
            .map(|f| format!("S3 {}: {}", f.violation.as_str(), f.detail))
            .collect();

        if self.findings_omitted > 0 {
            lines.push(format!(
                "S3: {} further reachability finding(s) were dropped from the report",
                self.findings_omitted
            ));
        }
        if self.records_outside_the_split > 0 {
            lines.push(format!(
                "S3: {} operation(s) ran against agents the ownership split never saw, so they \
                 are in no group and in no cell — the split and the workload disagree about \
                 who was driven",
                self.records_outside_the_split
            ));
        }
        // Work the fault caught and the run cannot account for. Not a finding —
        // an indeterminate operation is doubt, not damage, and the read-back
        // and exactly-once accounts are what resolve it — but the operator has
        // to see it next to the clean cells rather than infer it from them.
        for caught in &self.caught_in_flight {
            let unresolved = caught.indeterminate + caught.rejected;
            if unresolved > 0 {
                lines.push(format!(
                    "S3: {unresolved} of the {} operations the cut caught in flight on {} did \
                     not confirm ({} indeterminate, {} rejected)",
                    caught.operations,
                    caught.group.as_str(),
                    caught.indeterminate,
                    caught.rejected
                ));
            }
        }

        // Over budget is not a finding. How long a partition heal may take
        // before an agent is served again is a judgement, and the driver is not
        // the one to make it.
        if self.recovery_over_budget > 0 {
            lines.push(format!(
                "S3: {} of {} isolated agents took longer than the {}ms recovery budget to \
                 confirm anything after the heal (p99 {}ms, worst {}ms)",
                self.recovery_over_budget,
                self.isolated_agents,
                self.recovery_budget_ms,
                self.recovery.p99_ms,
                self.recovery.max_ms
            ));
        }
        lines
    }

    /// The lines a reader needs in order to interpret the run, which are not
    /// themselves problems.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "S3: {} agents on the isolated executor {}, {} elsewhere",
            self.isolated_agents, self.isolated_pod, self.reachable_agents
        )];

        // The caught population first: it is what the fault actually disturbed,
        // and nothing in the cells below points at it.
        for caught in &self.caught_in_flight {
            lines.push(format!(
                "S3 {}: {} operations across {} agents were in flight when the cut landed — \
                 p50 {}ms / p99 {}ms / worst {}ms, {} attempt(s) timed out, up to {} attempts \
                 each, {} still unresolved at the heal",
                caught.group.as_str(),
                caught.operations,
                caught.agents,
                caught.duration.p50_ms,
                caught.duration.p99_ms,
                caught.duration.max_ms,
                caught.attempts_timed_out,
                caught.max_attempts,
                caught.outlived_the_fault,
            ));
        }

        for group in [Group::OnPod, Group::Elsewhere] {
            for window in [Window::BeforeFault, Window::DuringFault, Window::AfterFault] {
                if let Some(cell) = self.cell(group, window) {
                    lines.push(format!(
                        "S3 {} {}: {:.2} served/s{}, {} offered{}, {} indeterminate, {} \
                         attempt(s) timed out, {} of {} agents active",
                        group.as_str(),
                        window.as_str(),
                        cell.served_per_sec,
                        cell.share_of_baseline_percent
                            .map(|s| format!(" ({s:.1}% of baseline)"))
                            .unwrap_or_default(),
                        cell.submitted,
                        // Silence is the reading that stops a small rate being
                        // mistaken for residual service.
                        cell.quiet_ms
                            .filter(|_| cell.window_secs > 0.0)
                            .map(|q| {
                                format!(
                                    ", answered nothing for {:.1}s of a {:.1}s window",
                                    q as f64 / 1000.0,
                                    cell.window_secs
                                )
                            })
                            .unwrap_or_default(),
                        cell.indeterminate,
                        cell.attempts_timed_out,
                        cell.agents_active,
                        if group == Group::OnPod {
                            self.isolated_agents
                        } else {
                            self.reachable_agents
                        },
                    ));
                }
            }
        }

        if self.recovery.count > 0 {
            lines.push(format!(
                "S3: isolated agents were served again p50 {}ms / p99 {}ms / worst {}ms after \
                 the heal",
                self.recovery.p50_ms, self.recovery.p99_ms, self.recovery.max_ms
            ));
        }
        lines
    }
}

/// Operations of one outcome.
fn count_of(records: &[&OperationRecord], outcome: Outcome) -> u64 {
    records.iter().filter(|r| r.outcome == outcome).count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::errors::ErrorClass;
    use crate::chaos::history::{AttemptRecord, Phase};
    use chrono::TimeDelta;
    use std::time::Duration;
    use test_r::test;

    const ISOLATED: &str = "chaos-s3-durable-0000";
    const CONTROL: &str = "chaos-s3-durable-0001";

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn split() -> PodSplit {
        PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: vec![ISOLATED.to_string()],
            elsewhere: vec![CONTROL.to_string()],
            targets_per_pod: BTreeMap::new(),
            number_of_shards: 1024,
        }
    }

    fn fault() -> FaultWindow {
        FaultWindow {
            injected_at: t0(),
            recovered_at: Some(t0() + TimeDelta::seconds(180)),
        }
    }

    /// One operation, submitted `offset` seconds from the moment the partition
    /// was injected. Negative offsets are the baseline.
    fn op(agent: &str, offset_secs: i64, outcome: Outcome) -> OperationRecord {
        let submitted_at = t0() + TimeDelta::seconds(offset_secs);
        OperationRecord {
            op_id: 0,
            stream: Stream::Durable,
            phase: Phase::Baseline,
            agent: agent.to_string(),
            method: "increment".to_string(),
            idempotency_key: format!("{agent}-{offset_secs}"),
            submitted_at,
            completed_at: Some(submitted_at + TimeDelta::milliseconds(20)),
            attempts: 1,
            outcome,
            duration_ms: 20,
            returned_value: Some(1),
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: vec![AttemptRecord {
                attempt: 1,
                started_at: submitted_at,
                duration_ms: 20,
                returned_value: Some(1),
                succeeded: outcome == Outcome::Confirmed,
                error_class: None,
                error: None,
            }],
        }
    }

    /// An operation that hung until the client gave up, twice: the shape every
    /// isolated invocation takes while the link is cut.
    fn stalled(agent: &str, offset_secs: i64) -> OperationRecord {
        let mut record = op(agent, offset_secs, Outcome::Indeterminate);
        record.duration_ms = 245_000;
        record.completed_at = Some(record.submitted_at + TimeDelta::seconds(245));
        record.returned_value = None;
        record.attempts = 2;
        record.error_class = Some(ErrorClass::Transport);
        record.attempt_log = (1..=2)
            .map(|attempt| AttemptRecord {
                attempt,
                started_at: record.submitted_at,
                duration_ms: 120_000,
                returned_value: None,
                succeeded: false,
                error_class: Some(ErrorClass::Transport),
                error: Some("attempt timed out after 120s".to_string()),
            })
            .collect();
        record
    }

    /// A baseline both groups share, then a fault the isolated group is cut off
    /// by and the control group sails through. `during_isolated` is how many
    /// operations the isolated group still managed.
    fn history(during_isolated: usize, during_control: usize) -> Vec<OperationRecord> {
        let mut records = Vec::new();
        // 300s of baseline, one operation per second per agent.
        for second in 1..=300 {
            for agent in [ISOLATED, CONTROL] {
                records.push(op(agent, -second, Outcome::Confirmed));
            }
        }
        // 180s of fault.
        for i in 0..during_isolated {
            records.push(op(ISOLATED, i as i64, Outcome::Confirmed));
        }
        for i in 0..during_control {
            records.push(op(CONTROL, i as i64, Outcome::Confirmed));
        }
        // 240s of recovery, both groups back to cadence.
        for second in 181..=420 {
            for agent in [ISOLATED, CONTROL] {
                records.push(op(agent, second, Outcome::Confirmed));
            }
        }
        records
    }

    fn build(records: &[OperationRecord]) -> ReachabilityReport {
        ReachabilityReport::build(
            records,
            &split(),
            Some(fault()),
            25.0,
            75.0,
            Duration::from_secs(60),
        )
    }

    /// The healthy shape: the cut half stops, the other half does not, and the
    /// report says so without raising anything.
    #[test]
    fn a_partition_that_lands_and_costs_the_control_group_nothing_has_no_findings() {
        let report = build(&history(2, 180));

        assert!(
            report.findings.is_empty(),
            "expected no findings, got {:?}",
            report.findings
        );
        let isolated = report.cell(Group::OnPod, Window::DuringFault).unwrap();
        assert!(
            isolated.share_of_baseline_percent.unwrap() < 25.0,
            "the isolated group should have collapsed, got {isolated:?}"
        );
        let control = report.cell(Group::Elsewhere, Window::DuringFault).unwrap();
        assert!(
            control.share_of_baseline_percent.unwrap() >= 75.0,
            "the control group should have held, got {control:?}"
        );
    }

    /// The inconclusive case, and the most important one in this module: a
    /// partition that never took hold produces a report full of healthy numbers,
    /// and it has to read as "this run tested nothing" rather than as a pass.
    #[test]
    fn an_isolated_group_that_kept_working_says_the_partition_never_landed() {
        let report = build(&history(180, 180));

        assert_eq!(
            report
                .findings
                .iter()
                .map(|f| f.violation)
                .collect::<Vec<_>>(),
            vec![ReachabilityViolation::PartitionNotObserved]
        );
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|line| line.contains("Nothing else in this report")),
            "the operator has to be told the rest of the report is meaningless"
        );
    }

    /// The finding S3 exists to hunt: agents that were never partitioned from
    /// anything losing throughput because their worker-service was busy waiting
    /// on a pod they do not use.
    #[test]
    fn a_control_group_that_degraded_alongside_the_isolated_one_is_a_finding() {
        // Half the control group's baseline rate, well under the 75% floor.
        let report = build(&history(2, 90));

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == ReachabilityViolation::ControlDegraded),
            "expected collateral damage to be reported, got {:?}",
            report.findings
        );
    }

    /// Throughput, not raw counts. The baseline is 300s long and the fault
    /// window 180s, so comparing totals would call an untouched group degraded
    /// by 40% on window length alone.
    #[test]
    fn a_group_that_held_its_rate_is_not_penalised_for_a_shorter_fault_window() {
        // 180 operations across a 180s window is exactly the baseline rate.
        let report = build(&history(2, 180));
        let control = report.cell(Group::Elsewhere, Window::DuringFault).unwrap();

        assert_eq!(control.submitted, 180);
        assert!(
            (control.share_of_baseline_percent.unwrap() - 100.0).abs() < 1.0,
            "an unchanged rate should read as ~100% of baseline, got {:?}",
            control.share_of_baseline_percent
        );
    }

    /// The pending-then-timeout behaviour the ticket asks to see in the history.
    #[test]
    fn attempts_that_hit_the_client_timeout_are_counted_per_cell() {
        let mut records = history(0, 180);
        records.push(stalled(ISOLATED, 10));
        let report = build(&records);

        let isolated = report.cell(Group::OnPod, Window::DuringFault).unwrap();
        assert_eq!(isolated.attempts_timed_out, 2);
        assert_eq!(isolated.indeterminate, 1);
        assert_eq!(isolated.confirmed, 0);
    }

    /// Recovery is measured from the heal, not from when the operation was
    /// submitted: an agent whose call was already in flight when the link came
    /// back was served promptly, and the number has to say so.
    #[test]
    fn recovery_is_measured_from_the_heal() {
        let mut records = history(0, 180);
        // Submitted mid-fault, answered five seconds after the heal.
        let mut late = op(ISOLATED, 100, Outcome::Confirmed);
        late.completed_at = Some(t0() + TimeDelta::seconds(185));
        records.retain(|r| !(r.agent == ISOLATED && r.submitted_at > t0()));
        records.push(late);

        let report = build(&records);
        assert_eq!(report.recovery.count, 1);
        assert_eq!(report.recovery.max_ms, 5_000);
        assert!(report.agents_never_recovered.is_empty());
    }

    /// An isolated agent that never answered again is the strongest finding this
    /// report can make: the link is back and nothing else is wrong.
    #[test]
    fn an_isolated_agent_that_never_came_back_is_a_finding() {
        let records: Vec<OperationRecord> = history(0, 180)
            .into_iter()
            .filter(|r| !(r.agent == ISOLATED && r.submitted_at >= t0()))
            .collect();

        let report = build(&records);
        assert_eq!(report.agents_never_recovered, vec![ISOLATED.to_string()]);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == ReachabilityViolation::NeverRecovered
                    && f.agent.as_deref() == Some(ISOLATED))
        );
    }

    /// Without the fault window there is no before-and-during to compare, so
    /// every threshold in this report is undefined. It must count what it saw
    /// and claim nothing — a verdict here would be invented.
    #[test]
    fn a_run_that_never_learned_when_the_fault_was_reports_counts_but_no_verdict() {
        let report = ReachabilityReport::build(
            &history(180, 180),
            &split(),
            None,
            25.0,
            75.0,
            Duration::from_secs(60),
        );

        assert!(report.findings.is_empty());
        assert!(report.cells.iter().all(|c| c.window == Window::Unknown));
        assert!(report.cells.iter().all(|c| c.served_per_sec == 0.0));
        assert_eq!(report.recovery.count, 0);
        // And nothing is silently blamed on the agents themselves.
        assert!(report.agents_never_recovered.is_empty());
    }

    /// An agent the selection never saw is reported, not folded into a group.
    /// It means the split and the workload disagree about who was driven, and
    /// silently counting it as a control would corrupt the one comparison the
    /// whole scenario rests on.
    #[test]
    fn operations_against_an_unknown_agent_are_reported_rather_than_grouped() {
        let mut records = history(2, 180);
        records.push(op("chaos-s3-durable-9999", -10, Outcome::Confirmed));

        let report = build(&records);
        assert_eq!(report.records_outside_the_split, 1);
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|line| line.contains("the ownership split never saw"))
        );
    }

    /// Two hundred agents that all failed to recover is one fact, not two
    /// hundred, and an artifact carrying all of them is unreadable.
    #[test]
    fn findings_beyond_the_cap_are_counted_rather_than_carried() {
        let many: Vec<String> = (0..MAX_FINDINGS + 10)
            .map(|i| format!("chaos-s3-durable-{i:04}"))
            .collect();
        let mut split = split();
        split.on_pod = many.clone();

        let report = ReachabilityReport::build(
            &[],
            &split,
            Some(fault()),
            25.0,
            75.0,
            Duration::from_secs(60),
        );

        assert_eq!(report.agents_never_recovered.len(), MAX_FINDINGS + 10);
        assert_eq!(report.findings.len(), MAX_FINDINGS);
        assert_eq!(report.findings_omitted, 10);
    }

    /// Over budget is context, not a finding: how long worker-service's retry
    /// loop may take to notice a healed link is a judgement, and the driver is
    /// not the one to make it.
    #[test]
    fn a_slow_recovery_is_an_attention_line_rather_than_a_finding() {
        let mut records = history(0, 180);
        let mut slow = op(ISOLATED, 100, Outcome::Confirmed);
        slow.completed_at = Some(t0() + TimeDelta::seconds(300));
        records.retain(|r| !(r.agent == ISOLATED && r.submitted_at > t0()));
        records.push(slow);

        let report = build(&records);
        assert_eq!(report.recovery_over_budget, 1);
        assert!(report.findings.is_empty());
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|line| line.contains("recovery budget"))
        );
    }

    /// An operation the cut landed in the middle of: submitted just before it,
    /// still running when it landed, answered only once the link returned.
    fn caught(agent: &str, timed_out_attempts: u32) -> OperationRecord {
        let mut record = op(agent, -1, Outcome::Confirmed);
        record.duration_ms = 178_000;
        record.completed_at = Some(record.submitted_at + TimeDelta::milliseconds(178_000));
        record.attempts = timed_out_attempts + 1;
        record.attempt_log = (1..=timed_out_attempts)
            .map(|attempt| AttemptRecord {
                attempt,
                started_at: record.submitted_at,
                duration_ms: 120_000,
                returned_value: None,
                succeeded: false,
                error_class: Some(ErrorClass::Transport),
                error: Some("attempt timed out after 120s".to_string()),
            })
            .chain(std::iter::once(AttemptRecord {
                attempt: timed_out_attempts + 1,
                started_at: record.submitted_at,
                duration_ms: 56_000,
                returned_value: Some(1),
                succeeded: true,
                error_class: None,
                error: None,
            }))
            .collect();
        record
    }

    /// The population the fault actually disturbed, which no cell can show:
    /// these were submitted before the cut, so their timeouts and their
    /// duration are attributed to the `before-fault` row.
    #[test]
    fn the_operations_the_cut_caught_are_reported_apart_from_the_cells() {
        let mut records = history(0, 180);
        records.retain(|r| {
            !(r.agent == ISOLATED
                && r.submitted_at >= t0()
                && r.submitted_at < t0() + TimeDelta::seconds(180))
        });
        records.push(caught(ISOLATED, 1));

        let report = build(&records);
        let caught = report
            .caught_in_flight
            .iter()
            .find(|c| c.group == Group::OnPod)
            .expect("the isolated group had an operation in flight");

        assert_eq!(caught.operations, 1);
        assert_eq!(caught.agents, 1);
        assert_eq!(caught.confirmed, 1);
        assert_eq!(caught.duration.max_ms, 178_000);
        assert_eq!(caught.attempts_timed_out, 1);
        // The retry is what landed it. With retries off it would have been
        // indeterminate, and the outcome alone cannot say so.
        assert_eq!(caught.max_attempts, 2);
        // It answered before the heal was stamped, so it did not outlive it.
        assert_eq!(caught.outlived_the_fault, 0);

        assert!(
            report
                .note_lines()
                .iter()
                .any(|l| l.contains("were in flight when the cut landed")),
            "the caught population has to be in front of the reader"
        );
    }

    /// An operation still unanswered when the link came back is a different
    /// statement from one that resolved inside the window, and the report has
    /// to keep them apart.
    #[test]
    fn an_operation_still_running_at_the_heal_is_counted_as_outliving_it() {
        let mut records = history(0, 180);
        records.retain(|r| {
            !(r.agent == ISOLATED
                && r.submitted_at >= t0()
                && r.submitted_at < t0() + TimeDelta::seconds(180))
        });
        let mut late = caught(ISOLATED, 1);
        late.completed_at = Some(t0() + TimeDelta::seconds(200));
        late.duration_ms = 201_000;
        records.push(late);

        let report = build(&records);
        let caught = report
            .caught_in_flight
            .iter()
            .find(|c| c.group == Group::OnPod)
            .unwrap();
        assert_eq!(caught.outlived_the_fault, 1);
    }

    /// The number that stops a small during-fault rate reading as residual
    /// service. A real run showed 1.1% of baseline from operations that all
    /// arrived in the last four seconds of a 182-second window, once the fault
    /// was already coming down.
    #[test]
    fn a_group_silent_until_the_heal_reports_how_long_it_answered_nothing() {
        let mut records = history(0, 180);
        records.retain(|r| {
            !(r.agent == ISOLATED
                && r.submitted_at >= t0()
                && r.submitted_at < t0() + TimeDelta::seconds(180))
        });
        // A late tail, exactly as the heal window produces.
        for offset in 176..180 {
            records.push(op(ISOLATED, offset, Outcome::Confirmed));
        }

        let report = build(&records);
        let cell = report.cell(Group::OnPod, Window::DuringFault).unwrap();

        assert_eq!(cell.submitted, 4);
        assert_eq!(cell.quiet_ms, Some(176_020));
        assert!(
            cell.quiet_ms.unwrap() as f64 / 1000.0 > cell.window_secs * 0.9,
            "silence has to dominate the window, not trail it"
        );
        // The same group's baseline answers once a second, so its longest
        // silence is one interval. That contrast is the whole reading: 176s of
        // silence is not a slower version of this, it is a different state.
        let baseline = report.cell(Group::OnPod, Window::BeforeFault).unwrap();
        assert_eq!(baseline.quiet_ms, Some(1_000));

        assert!(
            report
                .note_lines()
                .iter()
                .any(|l| l.contains("answered nothing for 176.0s of a 180.0s window")),
            "notes were {:?}",
            report.note_lines()
        );
    }

    /// The regression the first S16 run turned up, in the module that shares
    /// the construction.
    ///
    /// The workload keeps offering work to an unreachable executor all through
    /// the partition, so anything derived from submission times looks busy no
    /// matter what the platform is doing. Every one of these is offered during
    /// the fault and answered only after the heal: the fault window served the
    /// isolated group nothing, and the cell has to say so.
    #[test]
    fn work_offered_to_an_isolated_group_and_answered_after_the_heal_is_not_service() {
        let mut records = history(0, 180);
        records.retain(|r| {
            !(r.agent == ISOLATED
                && r.submitted_at >= t0()
                && r.submitted_at < t0() + TimeDelta::seconds(180))
        });
        for offset in 0..120 {
            let mut record = op(ISOLATED, offset, Outcome::Confirmed);
            record.completed_at = Some(t0() + TimeDelta::seconds(181));
            record.duration_ms = (181 - offset) as u64 * 1_000;
            records.push(record);
        }

        let report = build(&records);
        let cell = report.cell(Group::OnPod, Window::DuringFault).unwrap();

        assert_eq!(cell.submitted, 120, "they were offered during the fault");
        assert_eq!(cell.confirmed, 120, "and they did all eventually confirm");
        assert_eq!(cell.served, 0, "but none of it was served during the fault");
        assert_eq!(cell.served_per_sec, 0.0);
        assert_eq!(cell.quiet_ms, Some(180_000), "silent for the whole window");
    }

    /// Work the cut caught and the run cannot account for. Not a finding, but
    /// it must sit next to the clean cells rather than be inferred from them.
    #[test]
    fn caught_work_that_never_confirmed_is_raised_to_the_operator() {
        let mut records = history(0, 180);
        records.retain(|r| {
            !(r.agent == ISOLATED
                && r.submitted_at >= t0()
                && r.submitted_at < t0() + TimeDelta::seconds(180))
        });
        let mut lost = caught(ISOLATED, 2);
        lost.outcome = Outcome::Indeterminate;
        records.push(lost);

        let report = build(&records);
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|l| l.contains("caught in flight") && l.contains("did not confirm")),
            "attention was {:?}",
            report.attention_lines()
        );
        // Still not a finding: doubt is not damage, and the exactly-once and
        // read-back accounts are what resolve it.
        assert!(report.findings.is_empty());
    }

    /// A run with no fault window cannot say what was in flight when the cut
    /// landed, because it does not know when that was.
    #[test]
    fn a_run_without_a_fault_window_claims_nothing_was_caught() {
        let report = ReachabilityReport::build(
            &history(2, 180),
            &split(),
            None,
            25.0,
            75.0,
            Duration::from_secs(60),
        );
        assert!(report.caught_in_flight.is_empty());
        assert!(report.cells.iter().all(|c| c.quiet_ms.is_none()));
    }
}
