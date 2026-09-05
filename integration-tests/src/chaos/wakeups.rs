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

//! Pairing completions against wakeups (GOL-377).
//!
//! S11's question is not "how many promises resolved". It is, for each
//! completion the platform *accepted*, whether the agent suspended on that
//! promise was resumed — once, and only once. Counting cannot answer it: a lost
//! wakeup and a duplicate wakeup cancel out in a total, and neither can be
//! localised to an agent afterwards.
//!
//! So every round carries a token. The driver mints it, `arm` records it against
//! the promise it created, and the waiter writes it into its wakeup log when it
//! is resumed. Pairing the two turns each failure into a statement about one
//! named round: this completion, accepted at this time, against this waiter,
//! never woke it.
//!
//! ## What counts as a finding, and what only counts as doubt
//!
//! A completion the platform confirmed is a promise the platform made. If the
//! waiter never woke and its log is whole, that is a finding, full stop.
//!
//! A completion that failed in an *indeterminate* way is not. From the client
//! side a dropped connection is indistinguishable from a request that arrived
//! and executed, and a pod kill produces exactly these. If such a round woke,
//! the platform resolved the doubt in its own favour and the report says so; if
//! it did not, nothing is proven and the round is counted as inconclusive rather
//! than as a loss.
//!
//! ## The waiter that will not answer
//!
//! One case here is unlike anything the other scenarios see. A waiter is parked
//! *inside* an invocation, so a waiter that never wakes cannot answer a read
//! either — its `wakeups` read queues behind the `wait` that is still running.
//!
//! An unreadable agent is normally the weakest possible outcome: it means the
//! run cannot say. Here it is nearly the opposite. A waiter whose completion was
//! confirmed, which then stopped producing rounds *and* could not be read, is a
//! worker wedged on a promise that was resolved long ago. The report separates
//! that case ([`WakeupViolation::NeverWoke`], with the read failure as its
//! detail) from an agent that merely timed out while otherwise healthy.
//!
//! ## Two clocks, and which number to believe
//!
//! The waiter stamps `armedAt` and `wokenAt` from the executor's clock; the
//! driver stamps the completion from its own. So the headline delay —
//! completion accepted to waiter resumed — spans two clocks, exactly as the
//! scheduled-fire delay does in [`crate::chaos::fires`], and the same guard
//! applies: `minDelayMs` is reported per cell so skew shows up as a negative
//! number instead of quietly flattering a percentile.
//!
//! There is one cross-check S10 cannot make. `parkedMs` — armed to woken — is
//! stamped at both ends by the executor, so it carries no skew at all. It is not
//! the delay, because it also contains the round's deliberate dwell, but on a
//! healthy baseline `parked - dwell` and the cross-clock delay should agree. A
//! gap between them is skew, and the report carries both rather than picking.

use crate::chaos::history::{OperationRecord, Outcome, Stream, WaiterWakeupLog, WakeupRecord};
use crate::chaos::split::{FaultWindow, PodSplit, Window};
use crate::chaos::summary::LatencyStats;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Ceiling on how many findings the report carries.
///
/// A run that lost every wakeup would otherwise produce tens of thousands of
/// them and an artifact nobody can open. The count is reported separately, so
/// truncation is stated rather than inferred from a suspiciously round number.
const MAX_FINDINGS: usize = 200;

/// The method name the completion operations are recorded under.
const COMPLETE_METHOD: &str = "complete";

/// The method name the parking invocations are recorded under.
const WAIT_METHOD: &str = "wait";

/// What [`WAIT_METHOD`] operations append to their round's token to form their
/// own idempotency key.
const WAIT_KEY_SUFFIX: &str = "-wait";

/// Whether a waiter was on the executor the fault killed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaiterGroup {
    /// Owned by the killed executor when the driver signalled readiness.
    OnKilledExecutor,
    /// Owned by an executor the fault left alone: the run's own control group.
    Elsewhere,
}

impl WaiterGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            WaiterGroup::OnKilledExecutor => "on-killed-executor",
            WaiterGroup::Elsewhere => "elsewhere",
        }
    }
}

impl std::fmt::Display for WaiterGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What went wrong with one round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WakeupViolation {
    /// A completion the platform accepted whose waiter was never resumed.
    NeverWoke,
    /// One completion, two or more wakeups. A promise resolved twice, or a
    /// recovery that replayed the resume without deduplicating it.
    WokeMoreThanOnce,
    /// A wakeup for a round whose completion the platform definitively refused.
    WokeDespiteRejection,
}

impl WakeupViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            WakeupViolation::NeverWoke => "never-woke",
            WakeupViolation::WokeMoreThanOnce => "woke-more-than-once",
            WakeupViolation::WokeDespiteRejection => "woke-despite-rejection",
        }
    }
}

impl std::fmt::Display for WakeupViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One violation, against one round.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeupFinding {
    pub violation: WakeupViolation,
    pub token: String,
    pub agent: String,
    pub window: Window,
    pub detail: String,
}

/// Wakeup delay for one (group, window) cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeupDelayStats {
    pub group: WaiterGroup,
    pub window: Window,
    /// Percentiles over delays clamped at zero, so skew cannot flatter them.
    pub delay: LatencyStats,
    /// The most negative delay seen, which is the clock skew between the driver
    /// and the executor rather than a waiter woken before it was completed.
    pub min_delay_ms: i64,
    /// Wakeups whose delay exceeded the configured budget.
    pub over_budget: u64,
    /// Armed-to-woken, on the executor's clock alone. Carries the round's dwell
    /// as well as the delay, and carries no skew.
    pub parked: LatencyStats,
    /// How much longer the *caller* waited than the platform actually took.
    ///
    /// The `wait` invocation's own duration, less the round's dwell, less the
    /// delay the waiter recorded. On a healthy round this is a few milliseconds
    /// of round trip. A large value means the platform woke the agent on time
    /// and the answer did not come back — which is a different defect from a
    /// slow wakeup, and invisible in [`Self::delay`].
    pub client_excess: LatencyStats,
    /// Rounds whose caller waited longer than the whole wakeup budget *after*
    /// the waiter had already woken.
    pub client_stalled: u64,
}

/// The promise-wakeup account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeupReport {
    /// What resuming a suspended waiter is allowed to cost, from the suite YAML.
    /// Recorded so a percentile in an archived result can be read years later
    /// against the number it was judged by rather than against today's config.
    pub wakeup_budget_ms: u64,
    /// The dwell each round held before its completion, so `parked` can be read
    /// without the suite YAML to hand.
    pub dwell_ms: u64,
    pub completions_confirmed: u64,
    pub completions_indeterminate: u64,
    pub completions_rejected: u64,
    /// Wakeups the waiters recorded, including any whose token is unknown.
    pub wakeups_recorded: u64,
    /// Accepted completions paired with exactly one wakeup.
    pub woke_once: u64,
    /// Completions the driver was never sure of that woke anyway — doubt the
    /// platform resolved in its own favour.
    pub indeterminate_that_woke: u64,
    /// Completions the driver was never sure of that never woke. Not a finding:
    /// the completion may never have landed.
    pub inconclusive: u64,
    /// Completions whose waiter could not testify, because its log was
    /// unreadable or truncated *and* nothing else about it looked wrong.
    pub unverifiable: u64,
    /// Wakeups whose token no completion claims. Zero on a healthy run: agent
    /// names carry the run nonce, so nothing from an earlier run can appear.
    pub unknown_tokens: u64,
    /// Waiters the read-back could not reach at all.
    pub waiters_unreadable: Vec<String>,
    /// Waiters whose wakeup log hit the component's cap.
    pub waiters_truncated: Vec<String>,
    /// Waiters that stopped producing rounds during the run because a wakeup
    /// never arrived, as the workload itself observed. The live half of the
    /// oracle — see [`crate::chaos::waiters::WaiterHandle::stalled`].
    pub waiters_stood_down: u64,
    /// Waiters that stood down *and* could not then be read: parked inside an
    /// invocation, which is what a wedged worker looks like from outside.
    pub waiters_wedged: Vec<String>,
    pub delay: Vec<WakeupDelayStats>,
    /// Rounds across every cell whose caller waited past the budget after the
    /// wakeup had already happened. See [`WakeupDelayStats::client_excess`].
    pub client_stalled_total: u64,
    /// The worst such gap, in milliseconds.
    pub client_stall_worst_ms: u64,
    /// How many of those callers had to retry to get their answer at all. A
    /// stall that only ends on a retry is a request that was never coming back.
    pub client_stall_retried: u64,
    pub findings: Vec<WakeupFinding>,
    /// Findings past [`MAX_FINDINGS`], which the report drops rather than
    /// carries. Non-zero means `findings` is a sample.
    pub findings_omitted: u64,
}

/// One round, as the pairing sees it.
struct Round<'a> {
    token: &'a str,
    agent: &'a str,
    outcome: Outcome,
    submitted_at: DateTime<Utc>,
}

impl WakeupReport {
    /// Pairs completions against wakeups.
    ///
    /// `records` is the whole history; only the `complete` operations of the
    /// waiter stream are considered — `arm` and `wait` are recorded for the
    /// timeline, not for this account.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        records: &[OperationRecord],
        logs: &[WaiterWakeupLog],
        split: &PodSplit,
        fault: Option<FaultWindow>,
        dwell: Duration,
        wakeup_budget: Duration,
        stood_down: u64,
    ) -> Self {
        let budget_ms = wakeup_budget.as_millis().min(u64::MAX as u128) as u64;
        let dwell_ms = dwell.as_millis().min(u64::MAX as u128) as u64;

        let mut wakeups_by_token: BTreeMap<&str, Vec<&WakeupRecord>> = BTreeMap::new();
        let mut complete_log: BTreeMap<&str, bool> = BTreeMap::new();
        let mut waiters_unreadable = Vec::new();
        let mut waiters_truncated = Vec::new();
        let mut wakeups_recorded = 0u64;

        for log in logs {
            complete_log.insert(log.agent.as_str(), log.is_complete());
            if log.error.is_some() {
                waiters_unreadable.push(log.agent.clone());
            } else if !log.is_complete() {
                waiters_truncated.push(log.agent.clone());
            }
            for wakeup in &log.wakeups {
                wakeups_recorded += 1;
                wakeups_by_token
                    .entry(wakeup.token.as_str())
                    .or_default()
                    .push(wakeup);
            }
        }

        let on_killed: BTreeSet<&str> = split.on_pod.iter().map(|s| s.as_str()).collect();
        let mut report = Self::empty(budget_ms, dwell_ms);
        report.wakeups_recorded = wakeups_recorded;
        report.waiters_stood_down = stood_down;

        // A stalled waiter that then could not be read is the wedged case. It is
        // computed before the round loop because the loop uses it to decide
        // whether an unreadable waiter excuses a missing wakeup or convicts it.
        let unreadable: BTreeSet<&str> = waiters_unreadable.iter().map(|s| s.as_str()).collect();

        let mut cells: BTreeMap<(WaiterGroup, Window), DelayCell> = BTreeMap::new();
        let mut claimed: BTreeSet<&str> = BTreeSet::new();
        let waits = wait_observations(records);
        let mut stall_worst_ms = 0u64;
        let mut stall_retried = 0u64;

        for round in completions(records) {
            let group = if on_killed.contains(round.agent) {
                WaiterGroup::OnKilledExecutor
            } else {
                WaiterGroup::Elsewhere
            };
            let window = Window::of(round.submitted_at, fault);
            let wakeups = wakeups_by_token
                .get(round.token)
                .cloned()
                .unwrap_or_default();
            claimed.insert(round.token);

            match round.outcome {
                Outcome::Confirmed => report.completions_confirmed += 1,
                Outcome::Indeterminate => report.completions_indeterminate += 1,
                Outcome::Rejected => report.completions_rejected += 1,
            }

            if wakeups.len() > 1 {
                report.push_finding(WakeupFinding {
                    violation: WakeupViolation::WokeMoreThanOnce,
                    token: round.token.to_string(),
                    agent: round.agent.to_string(),
                    window,
                    detail: format!(
                        "{} wakeups recorded for one completion, at {}",
                        wakeups.len(),
                        wakeups
                            .iter()
                            .map(|w| w.woken_at.to_rfc3339())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }

            if let Some(wakeup) = wakeups.first() {
                if round.outcome == Outcome::Rejected {
                    report.push_finding(WakeupFinding {
                        violation: WakeupViolation::WokeDespiteRejection,
                        token: round.token.to_string(),
                        agent: round.agent.to_string(),
                        window,
                        detail: format!(
                            "the completion was definitively refused, and the waiter woke at {}",
                            wakeup.woken_at.to_rfc3339()
                        ),
                    });
                } else {
                    if round.outcome == Outcome::Confirmed {
                        report.woke_once += 1;
                    } else {
                        report.indeterminate_that_woke += 1;
                    }
                    let delay_ms = (wakeup.woken_at - round.submitted_at).num_milliseconds();
                    let cell = cells.entry((group, window)).or_default();
                    cell.push(delay_ms, wakeup.parked_ms(), budget_ms);

                    // The caller's own view of the same round. `wait` covers the
                    // dwell as well as the wakeup, so the dwell comes off before
                    // the two are compared.
                    if let Some(observed) = waits.get(round.token) {
                        let client_ms = observed.duration_ms.saturating_sub(dwell_ms);
                        let excess = client_ms.saturating_sub(delay_ms.max(0) as u64);
                        cell.push_client(excess, budget_ms);
                        if excess > budget_ms {
                            stall_worst_ms = stall_worst_ms.max(excess);
                            if observed.attempts > 1 {
                                stall_retried += 1;
                            }
                        }
                    }
                }
                continue;
            }

            // No wakeup. What that proves depends on the completion's outcome
            // and on whether the waiter could testify at all.
            match round.outcome {
                Outcome::Rejected => {}
                Outcome::Indeterminate => report.inconclusive += 1,
                Outcome::Confirmed => {
                    let log_whole = complete_log.get(round.agent).copied().unwrap_or(false);
                    if log_whole {
                        report.push_finding(WakeupFinding {
                            violation: WakeupViolation::NeverWoke,
                            token: round.token.to_string(),
                            agent: round.agent.to_string(),
                            window,
                            detail: format!(
                                "the completion was accepted at {} and the waiter's whole wakeup \
                                 log has no entry for it",
                                round.submitted_at.to_rfc3339()
                            ),
                        });
                    } else if unreadable.contains(round.agent) && stood_down > 0 {
                        // The wedged case: the workload watched this waiter stop
                        // producing, and the read-back then could not reach it.
                        // Both symptoms of one worker still parked on a promise
                        // that was resolved.
                        report.push_finding(WakeupFinding {
                            violation: WakeupViolation::NeverWoke,
                            token: round.token.to_string(),
                            agent: round.agent.to_string(),
                            window,
                            detail: format!(
                                "the completion was accepted at {} and the waiter has answered \
                                 nothing since, which is what a worker parked on a resolved \
                                 promise looks like from outside",
                                round.submitted_at.to_rfc3339()
                            ),
                        });
                        if !report.waiters_wedged.iter().any(|w| w == round.agent) {
                            report.waiters_wedged.push(round.agent.to_string());
                        }
                    } else {
                        report.unverifiable += 1;
                    }
                }
            }
        }

        report.unknown_tokens = wakeups_by_token
            .keys()
            .filter(|token| !claimed.contains(*token))
            .count() as u64;
        report.waiters_unreadable = waiters_unreadable;
        report.waiters_truncated = waiters_truncated;
        report.delay = cells
            .into_iter()
            .map(|((group, window), cell)| cell.into_stats(group, window))
            .collect();
        report.client_stalled_total = report.delay.iter().map(|c| c.client_stalled).sum();
        report.client_stall_worst_ms = stall_worst_ms;
        report.client_stall_retried = stall_retried;
        report
    }

    fn empty(wakeup_budget_ms: u64, dwell_ms: u64) -> Self {
        Self {
            wakeup_budget_ms,
            dwell_ms,
            completions_confirmed: 0,
            completions_indeterminate: 0,
            completions_rejected: 0,
            wakeups_recorded: 0,
            woke_once: 0,
            indeterminate_that_woke: 0,
            inconclusive: 0,
            unverifiable: 0,
            unknown_tokens: 0,
            waiters_unreadable: Vec::new(),
            waiters_truncated: Vec::new(),
            waiters_stood_down: 0,
            waiters_wedged: Vec::new(),
            delay: Vec::new(),
            client_stalled_total: 0,
            client_stall_worst_ms: 0,
            client_stall_retried: 0,
            findings: Vec::new(),
            findings_omitted: 0,
        }
    }

    fn push_finding(&mut self, finding: WakeupFinding) {
        if self.findings.len() < MAX_FINDINGS {
            self.findings.push(finding);
        } else {
            self.findings_omitted += 1;
        }
    }

    /// Whether the run found anything that fails it.
    pub fn has_violations(&self) -> bool {
        !self.findings.is_empty() || self.findings_omitted > 0
    }

    /// Total findings, including any the report dropped.
    pub fn violations(&self) -> u64 {
        self.findings.len() as u64 + self.findings_omitted
    }

    /// The p99 wakeup delay on the killed executor's waiters, during the fault:
    /// the one number this scenario exists to produce.
    pub fn fault_window_p99_ms(&self) -> Option<u64> {
        self.delay
            .iter()
            .find(|cell| {
                cell.group == WaiterGroup::OnKilledExecutor && cell.window == Window::DuringFault
            })
            .map(|cell| cell.delay.p99_ms)
    }

    /// Lines an operator has to act on.
    pub fn attention_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.has_violations() {
            lines.push(format!(
                "S11 found {} promise-wakeup violations: {}",
                self.violations(),
                self.findings
                    .iter()
                    .take(3)
                    .map(|f| format!("{} on {}", f.violation, f.token))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.waiters_wedged.is_empty() {
            lines.push(format!(
                "S11 left {} waiters wedged: {} — each stopped producing rounds and then \
                 answered no read, which is a worker still parked on a resolved promise",
                self.waiters_wedged.len(),
                self.waiters_wedged.join(", ")
            ));
        }
        if !self.waiters_unreadable.is_empty() || !self.waiters_truncated.is_empty() {
            lines.push(format!(
                "S11 could not take a whole account from {} waiters ({} unreadable, {} \
                 truncated), so {} accepted completions are unverified either way",
                self.waiters_unreadable.len() + self.waiters_truncated.len(),
                self.waiters_unreadable.len(),
                self.waiters_truncated.len(),
                self.unverifiable
            ));
        }
        if self.client_stalled_total > 0 {
            lines.push(format!(
                "S11: {} rounds woke on time and the caller was not told for up to {}ms — {} of \
                 them only got an answer by retrying. The waiters' own logs say the platform \
                 resumed them promptly, so this is the response path, not the wakeup",
                self.client_stalled_total, self.client_stall_worst_ms, self.client_stall_retried
            ));
        }
        if self.unknown_tokens > 0 {
            lines.push(format!(
                "S11 recorded {} wakeups whose token no completion claims — agent names carry \
                 the run nonce, so this should be impossible",
                self.unknown_tokens
            ));
        }
        lines
    }

    /// Lines that explain the account without claiming anything is wrong.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.waiters_stood_down > 0 && self.waiters_wedged.is_empty() {
            lines.push(format!(
                "S11 stood {} waiters down after a slow wakeup, and every one of them was \
                 readable afterwards — late rather than lost",
                self.waiters_stood_down
            ));
        }
        if self.indeterminate_that_woke > 0 {
            lines.push(format!(
                "S11 had {} completions fail in a way that proves nothing, whose waiters woke \
                 anyway",
                self.indeterminate_that_woke
            ));
        }
        if self.inconclusive > 0 {
            lines.push(format!(
                "S11 had {} completions that neither succeeded nor demonstrably landed, whose \
                 waiters did not wake — not losses, because the completion may never have \
                 arrived",
                self.inconclusive
            ));
        }
        lines
    }
}

/// Delays accumulated for one cell before they become percentiles.
#[derive(Default)]
struct DelayCell {
    delays: Vec<u64>,
    parked: Vec<u64>,
    client_excess: Vec<u64>,
    client_stalled: u64,
    min_delay_ms: i64,
    over_budget: u64,
    any: bool,
}

impl DelayCell {
    fn push(&mut self, delay_ms: i64, parked_ms: i64, budget_ms: u64) {
        if !self.any || delay_ms < self.min_delay_ms {
            self.min_delay_ms = delay_ms;
        }
        self.any = true;
        let clamped = delay_ms.max(0) as u64;
        if clamped > budget_ms {
            self.over_budget += 1;
        }
        self.delays.push(clamped);
        self.parked.push(parked_ms.max(0) as u64);
    }

    /// Records how much longer the caller waited than the platform took.
    fn push_client(&mut self, excess_ms: u64, budget_ms: u64) {
        if excess_ms > budget_ms {
            self.client_stalled += 1;
        }
        self.client_excess.push(excess_ms);
    }

    fn into_stats(self, group: WaiterGroup, window: Window) -> WakeupDelayStats {
        WakeupDelayStats {
            group,
            window,
            delay: LatencyStats::from_durations(self.delays),
            min_delay_ms: self.min_delay_ms,
            over_budget: self.over_budget,
            parked: LatencyStats::from_durations(self.parked),
            client_excess: LatencyStats::from_durations(self.client_excess),
            client_stalled: self.client_stalled,
        }
    }
}

/// What the driver's own `wait` invocation cost, per round.
///
/// Keyed by the round's token: the `wait` operation is recorded under
/// `{token}-wait`, which is what lets the caller's view be joined to the
/// waiter's own.
struct WaitObservation {
    duration_ms: u64,
    attempts: u32,
}

fn wait_observations(records: &[OperationRecord]) -> BTreeMap<&str, WaitObservation> {
    records
        .iter()
        .filter(|r| r.stream == Stream::PromiseWait && r.method == WAIT_METHOD)
        .filter_map(|r| {
            r.idempotency_key
                .strip_suffix(WAIT_KEY_SUFFIX)
                .map(|token| {
                    (
                        token,
                        WaitObservation {
                            duration_ms: r.duration_ms,
                            attempts: r.attempts,
                        },
                    )
                })
        })
        .collect()
}

/// The completion operations, which are the rounds this report is about.
fn completions(records: &[OperationRecord]) -> impl Iterator<Item = Round<'_>> {
    records
        .iter()
        .filter(|r| r.stream == Stream::PromiseWait && r.method == COMPLETE_METHOD)
        .map(|r| Round {
            token: r.idempotency_key.as_str(),
            agent: r.agent.as_str(),
            outcome: r.outcome,
            submitted_at: r.submitted_at,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::Phase;
    use test_r::test;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn at_ms(millis: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(millis).unwrap()
    }

    /// The fault: injected at 100s, cleared at 200s.
    fn fault() -> Option<FaultWindow> {
        Some(FaultWindow {
            injected_at: at(100),
            recovered_at: Some(at(200)),
        })
    }

    fn split_of(on_pod: &[&str], elsewhere: &[&str]) -> PodSplit {
        PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: on_pod.iter().map(|s| s.to_string()).collect(),
            elsewhere: elsewhere.iter().map(|s| s.to_string()).collect(),
            targets_per_pod: BTreeMap::new(),
            number_of_shards: 1024,
        }
    }

    fn completion(
        agent: &str,
        token: &str,
        submitted: DateTime<Utc>,
        outcome: Outcome,
    ) -> OperationRecord {
        OperationRecord {
            op_id: 1,
            stream: Stream::PromiseWait,
            phase: Phase::Fault,
            agent: agent.to_string(),
            method: COMPLETE_METHOD.to_string(),
            idempotency_key: token.to_string(),
            submitted_at: submitted,
            completed_at: Some(submitted),
            attempts: 1,
            outcome,
            duration_ms: 0,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: Vec::new(),
        }
    }

    /// A whole log: `wakes` matches the entries, so an absent wakeup is a
    /// statement rather than a gap.
    fn log(agent: &str, wakeups: Vec<WakeupRecord>) -> WaiterWakeupLog {
        WaiterWakeupLog {
            agent: agent.to_string(),
            wakes: Some(wakeups.len() as u64),
            wakeups,
            error: None,
        }
    }

    fn wakeup(token: &str, armed: DateTime<Utc>, woken: DateTime<Utc>) -> WakeupRecord {
        WakeupRecord {
            token: token.to_string(),
            armed_at: armed,
            woken_at: woken,
        }
    }

    fn build(
        records: &[OperationRecord],
        logs: &[WaiterWakeupLog],
        split: &PodSplit,
        stood_down: u64,
    ) -> WakeupReport {
        WakeupReport::build(
            records,
            logs,
            split,
            fault(),
            Duration::from_secs(5),
            Duration::from_secs(60),
            stood_down,
        )
    }

    #[test]
    fn a_completion_paired_with_one_wakeup_is_clean() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(111))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.completions_confirmed, 1);
        assert_eq!(report.woke_once, 1);
        assert!(!report.has_violations());
    }

    /// The headline failure. An accepted completion is a promise the platform
    /// made; a whole log with no entry for it is the platform not keeping it.
    #[test]
    fn an_accepted_completion_that_never_woke_its_waiter_is_a_finding() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![log("w-1", Vec::new())];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].violation, WakeupViolation::NeverWoke);
        assert_eq!(report.findings[0].token, "t-1");
        assert_eq!(report.findings[0].window, Window::DuringFault);
    }

    /// A truncated log cannot testify, so the same missing wakeup proves
    /// nothing. Calling it a loss would turn a component-side cap into a
    /// platform defect.
    #[test]
    fn a_missing_wakeup_on_a_truncated_log_is_unverifiable_rather_than_lost() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![WaiterWakeupLog {
            agent: "w-1".to_string(),
            wakes: Some(9000),
            wakeups: Vec::new(),
            error: None,
        }];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert!(!report.has_violations());
        assert_eq!(report.unverifiable, 1);
        assert_eq!(report.waiters_truncated, vec!["w-1".to_string()]);
    }

    /// The wedged case, and the one place where an unreadable agent convicts
    /// rather than excuses: the workload watched this waiter stop producing, and
    /// the read-back then could not reach it. Both are symptoms of one worker
    /// still parked on a promise that was resolved.
    #[test]
    fn a_waiter_that_stood_down_and_then_answered_nothing_is_a_finding() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![WaiterWakeupLog {
            agent: "w-1".to_string(),
            wakes: None,
            wakeups: Vec::new(),
            error: Some("wakeups timed out after 60s".to_string()),
        }];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 1);

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].violation, WakeupViolation::NeverWoke);
        assert_eq!(report.waiters_wedged, vec!["w-1".to_string()]);
        assert_eq!(report.unverifiable, 0);
    }

    /// The same unreadable waiter, with nothing having stood down, is only an
    /// unreadable waiter. A read that timed out on an otherwise healthy run says
    /// nothing about whether the completion landed.
    #[test]
    fn an_unreadable_waiter_that_never_stood_down_is_unverifiable() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![WaiterWakeupLog {
            agent: "w-1".to_string(),
            wakes: None,
            wakeups: Vec::new(),
            error: Some("wakeups timed out after 60s".to_string()),
        }];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert!(!report.has_violations());
        assert_eq!(report.unverifiable, 1);
        assert!(report.waiters_wedged.is_empty());
    }

    #[test]
    fn one_completion_and_two_wakeups_is_a_finding() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![log(
            "w-1",
            vec![
                wakeup("t-1", at(105), at(111)),
                wakeup("t-1", at(105), at(160)),
            ],
        )];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(
            report.findings[0].violation,
            WakeupViolation::WokeMoreThanOnce
        );
    }

    #[test]
    fn a_wakeup_for_a_refused_completion_is_a_finding() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Rejected)];
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(111))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(
            report.findings[0].violation,
            WakeupViolation::WokeDespiteRejection
        );
        assert_eq!(report.completions_rejected, 1);
    }

    /// A pod kill produces indeterminate completions by the dozen. One that woke
    /// anyway is the platform resolving the doubt in its own favour, and the
    /// report says so rather than counting it beside the confirmed ones.
    #[test]
    fn an_indeterminate_completion_that_woke_is_recorded_separately() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Indeterminate)];
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(111))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.indeterminate_that_woke, 1);
        assert_eq!(report.woke_once, 0);
        assert!(!report.has_violations());
    }

    /// And one that did not wake proves nothing at all: from the client side a
    /// dropped connection is indistinguishable from a request that arrived.
    #[test]
    fn an_indeterminate_completion_that_never_woke_is_inconclusive_rather_than_lost() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Indeterminate)];
        let logs = vec![log("w-1", Vec::new())];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.inconclusive, 1);
        assert!(!report.has_violations());
    }

    /// The control group is the whole reason the kill is aimed. Mixing the two
    /// would let a recovery that took its full budget hide behind the waiters
    /// that were never touched.
    #[test]
    fn delays_are_split_by_group_and_by_window() {
        let records = vec![
            completion("w-1", "t-1", at(110), Outcome::Confirmed),
            completion("w-2", "t-2", at(110), Outcome::Confirmed),
            completion("w-1", "t-3", at(50), Outcome::Confirmed),
        ];
        let logs = vec![
            log(
                "w-1",
                vec![
                    wakeup("t-1", at(105), at(150)),
                    wakeup("t-3", at(45), at(51)),
                ],
            ),
            log("w-2", vec![wakeup("t-2", at(105), at(111))]),
        ];
        let report = build(&records, &logs, &split_of(&["w-1"], &["w-2"]), 0);

        let killed_during = report
            .delay
            .iter()
            .find(|c| c.group == WaiterGroup::OnKilledExecutor && c.window == Window::DuringFault)
            .unwrap();
        assert_eq!(killed_during.delay.max_ms, 40_000);

        let control_during = report
            .delay
            .iter()
            .find(|c| c.group == WaiterGroup::Elsewhere && c.window == Window::DuringFault)
            .unwrap();
        assert_eq!(control_during.delay.max_ms, 1_000);

        assert!(report.delay.iter().any(|c| c.window == Window::BeforeFault));
        assert_eq!(report.fault_window_p99_ms(), Some(40_000));
    }

    /// The driver and the executor keep different clocks, so a wakeup can look
    /// as though it happened before the completion that caused it. That is skew,
    /// and it has to be visible rather than clamped silently into a percentile.
    #[test]
    fn clock_skew_shows_as_a_negative_minimum_rather_than_flattering_the_percentiles() {
        let records = vec![completion("w-1", "t-1", at_ms(110_000), Outcome::Confirmed)];
        let logs = vec![log(
            "w-1",
            vec![wakeup("t-1", at_ms(105_000), at_ms(109_700))],
        )];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        let cell = &report.delay[0];
        assert_eq!(cell.min_delay_ms, -300);
        assert_eq!(cell.delay.max_ms, 0);
    }

    /// `parked` is stamped at both ends by the executor, so it is the one number
    /// in the report free of that skew — which is what makes it worth carrying
    /// beside the delay rather than instead of it.
    #[test]
    fn the_parked_interval_is_reported_on_the_executors_own_clock() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(150))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.delay[0].parked.max_ms, 45_000);
        assert_eq!(report.dwell_ms, 5_000);
    }

    #[test]
    fn a_delay_past_the_budget_is_counted_without_failing_the_run() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(180))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.delay[0].over_budget, 1);
        assert!(!report.has_violations());
    }

    /// Agent names carry the run nonce, so a wakeup nobody asked for should be
    /// impossible. If it happens the report says so instead of dropping it.
    #[test]
    fn a_wakeup_no_completion_claims_is_counted() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![log(
            "w-1",
            vec![
                wakeup("t-1", at(105), at(111)),
                wakeup("t-stray", at(105), at(112)),
            ],
        )];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.unknown_tokens, 1);
        assert_eq!(report.wakeups_recorded, 2);
    }

    /// `arm` and `wait` are recorded for the timeline. Counting them as rounds
    /// would treble every total in the report.
    #[test]
    fn only_the_completion_operations_are_counted_as_rounds() {
        let mut armed = completion("w-1", "t-1-arm", at(109), Outcome::Confirmed);
        armed.method = "arm".to_string();
        let mut waited = completion("w-1", "t-1-wait", at(109), Outcome::Confirmed);
        waited.method = "wait".to_string();
        let records = vec![
            armed,
            waited,
            completion("w-1", "t-1", at(110), Outcome::Confirmed),
        ];
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(111))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.completions_confirmed, 1);
        assert_eq!(report.woke_once, 1);
    }

    /// A run that lost everything must still produce an artifact somebody can
    /// open, and must say that it truncated rather than leaving a suspiciously
    /// round number of findings.
    #[test]
    fn findings_beyond_the_cap_are_counted_rather_than_carried() {
        let records: Vec<OperationRecord> = (0..MAX_FINDINGS + 10)
            .map(|i| completion("w-1", &format!("t-{i}"), at(110), Outcome::Confirmed))
            .collect();
        let logs = vec![log("w-1", Vec::new())];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.findings.len(), MAX_FINDINGS);
        assert_eq!(report.findings_omitted, 10);
        assert_eq!(report.violations(), MAX_FINDINGS as u64 + 10);
    }

    /// The caller's view and the waiter's own can disagree, and when they do the
    /// waiter's is the one that describes the platform.
    ///
    /// This is not hypothetical: the first completed S11 run woke every one of
    /// 21,863 completions on time, and 89 of those callers were not told for 125
    /// seconds. A report with only [`WakeupReport::delay`] in it would have
    /// called that run flawless.
    #[test]
    fn a_wakeup_the_caller_was_not_told_about_is_counted_separately_from_the_delay() {
        let mut waited = completion("w-1", "t-1-wait", at(110), Outcome::Confirmed);
        waited.method = "wait".to_string();
        // Parked 5s, then 125s before the caller heard anything back.
        waited.duration_ms = 130_000;
        waited.attempts = 2;

        let records = vec![
            completion("w-1", "t-1", at(110), Outcome::Confirmed),
            waited,
        ];
        // The waiter itself woke one second after the completion.
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(111))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        // The platform did its job, and the delay table says so.
        assert_eq!(report.woke_once, 1);
        assert_eq!(report.delay[0].delay.max_ms, 1_000);
        assert_eq!(report.delay[0].over_budget, 0);
        assert!(!report.has_violations());

        // And the caller still waited two minutes past that.
        assert_eq!(report.client_stalled_total, 1);
        assert_eq!(report.client_stall_worst_ms, 124_000);
        assert_eq!(report.client_stall_retried, 1);
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|l| l.contains("response path, not the wakeup"))
        );
    }

    /// A healthy round's caller waits the dwell plus the wakeup and nothing
    /// more, so it must not be counted as stalled.
    #[test]
    fn a_prompt_round_records_no_client_stall() {
        let mut waited = completion("w-1", "t-1-wait", at(110), Outcome::Confirmed);
        waited.method = "wait".to_string();
        waited.duration_ms = 5_040;

        let records = vec![
            completion("w-1", "t-1", at(110), Outcome::Confirmed),
            waited,
        ];
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(110))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 0);

        assert_eq!(report.client_stalled_total, 0);
        assert!(report.attention_lines().is_empty());
    }

    /// Standing waiters down is normal on a run where recovery was slow. It only
    /// becomes an attention line when those waiters then could not be read.
    #[test]
    fn waiters_that_stood_down_but_answered_afterwards_are_context_not_a_finding() {
        let records = vec![completion("w-1", "t-1", at(110), Outcome::Confirmed)];
        let logs = vec![log("w-1", vec![wakeup("t-1", at(105), at(190))])];
        let report = build(&records, &logs, &split_of(&["w-1"], &[]), 3);

        assert!(report.attention_lines().is_empty());
        assert!(
            report
                .note_lines()
                .iter()
                .any(|line| line.contains("late rather than lost"))
        );
    }
}
