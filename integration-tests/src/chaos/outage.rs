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

//! What a storage outage cost, and whether the platform came back from it
//! (GOL-379).
//!
//! ### The control is time, not a pod
//!
//! Every other partition scenario in this suite keeps a group of agents on the
//! healthy side of the cut and reads the verdict off the disagreement between
//! the two groups. A storage outage has no healthy side. All three executors
//! share one key-value cluster, so cutting them off from it cuts off everything
//! at once, and an agent that happened to live elsewhere would be no better
//! served.
//!
//! So the comparison runs along the other axis. Each stream is measured against
//! **its own before-fault rate**, and the question the report answers first is
//! whether that rate collapsed at all. It has to be asked explicitly, because a
//! storage partition that silently failed to take hold produces a report full
//! of healthy numbers, and that must read as "this run tested nothing" rather
//! than as a pass.
//!
//! ### Throughput, not success rate
//!
//! The same reason [`crate::chaos::reachability`] gives, arriving by a
//! different route. The mixed workload gives every stream its own in-flight
//! budget ([`crate::chaos::workload::start`]), so a stream whose operations
//! stop returning does not fail over and over: it fills its budget, stops
//! submitting, and offers nothing further. A success rate would read that as a
//! handful of failures out of a handful of attempts. Confirmed operations per
//! second is the number that collapses.
//!
//! The per-stream budget is also what makes a per-stream row worth printing.
//! With one shared pool a single stalled stream drains the budget for all of
//! them and every row degrades together, which is exactly the unattributable
//! result S1 produced before the budgets were split.
//!
//! ### What the streams are actually testing
//!
//! The key-value cluster holds more than its name suggests. Reading the
//! executor's deployment: promises, the running-workers set and user key-value
//! data go to its `KeyValue` namespaces, and the scheduler keeps its own schema
//! on the same cluster. The oplog is on a different Aurora cluster and the
//! worker-status hot cache is in Redis, and neither is touched here.
//!
//! That is why every stream degrades under that cut rather than only the
//! obviously storage-shaped ones. A durable increment needs the running-workers
//! set before it can run at all, so `durable` is not a control group there and
//! must not be read as one.
//!
//! S18 cuts the Redis half instead, and the picture inverts. Only `Worker`,
//! `AgentStatus` and `AgentStatusCheckpoint` live there, the status blob is
//! written off the commit path by a background sweeper, and what still crosses
//! the cache synchronously is a lifecycle boundary or a `get_agent_mode` miss.
//! So `ephemeral` stops and `durable`, `scheduled` and `promise` carry on, and
//! the streams still answering are evidence rather than noise.
//!
//! S17 and S15 delay the two halves rather than cutting them, and the same
//! split decides which streams move. Under S17 that is `ephemeral` alone, which
//! its run confirmed at 6.59x against three streams that did not shift by a
//! millisecond. Under S15 it is the other three: a durable invocation flips its
//! agent between tracked and untracked in the running-workers index and waits
//! for the write, a promise operation reads and writes promise keys, and
//! registering a schedule writes to the scheduler's schema — all on the
//! PostgreSQL half — while an ephemeral agent is excluded from that index by
//! `AgentStatusFlusher::on_status_changed` and never touches it.
//!
//! ### What fails the run
//!
//! Five things, and all are statements about the experiment rather than about
//! latency:
//!
//! * [`OutageViolation::OutageNotObserved`] — a stream the cut was supposed to
//!   stop kept working, so the fault did not land where the run says it did.
//! * [`OutageViolation::UnexpectedStall`] — a stream the cut was *not* supposed
//!   to touch stopped anyway.
//! * [`OutageViolation::SlowdownNotObserved`] — a stream a delay was aimed at
//!   ran no slower than at rest, so the netem rule proved nothing.
//! * [`OutageViolation::UnexpectedSlowdown`] — a stream a delay was *not* aimed
//!   at slowed with it, which says the routing is not what the source says.
//! * [`OutageViolation::StreamNeverRecovered`] — a stream that was working
//!   before the outage produced nothing at all after the heal.
//!
//! ### Why the verdict is per scenario and the account is not
//!
//! Everything above the verdict is factual and shared: throughput per stream
//! per window, quiet time, latency, what the fault caught in flight. The
//! verdict is supplied by the scenario, as an
//! [`OutageExpectation`](crate::chaos::OutageExpectation), because what a cut
//! is supposed to do depends on what it cut.
//!
//! This was one rule until S18. That rule — every stream must fall silent —
//! is sound for S16, S22 and S14, where a database every stream depends on goes
//! away. S18 cuts a cache only part of the workload touches, and there the same
//! rule is not merely too strict but backwards: it reported S18's first run as
//! an outage that never landed, on the grounds that `durable` held 100% of its
//! baseline, which is exactly what the routing in
//! `NamespaceRoutedKeyValueStorage` says should happen.
//!
//! Parameterising the old rule with a list of streams to exempt would have
//! silenced that message and left the deeper gap. Under a partial cut the
//! streams that keep working carry as much information as the ones that stop:
//! had `durable` gone quiet under S18, the status blob would not be off the
//! commit path the way `AgentStatusFlusher` describes, and a rule that only
//! looks for silence would have called that its cleanest possible pass. So the
//! partial expectation asserts both halves and the whole-workload one keeps
//! asserting the single half that is all it can know.
//!
//! Recovery time is recorded against the configured budget and never asserted
//! on, like every other budget in the suite. How long a connection pool may
//! take to notice its database is back is a judgement, and the number is in the
//! result either way.

use crate::chaos::OutageExpectation;
use crate::chaos::errors::ErrorClass;
use crate::chaos::history::{OperationRecord, Outcome, Stream};
use crate::chaos::split::{
    FaultWindow, Window, longest_silence_ms, round2, window_end, window_secs, window_start,
};
use crate::chaos::summary::LatencyStats;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// What an outage finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutageViolation {
    /// The workload kept most of its baseline throughput while the storage was
    /// supposed to be unreachable. Whatever the fault status said, the
    /// executors could still reach the database, and every other number in this
    /// report describes an undisturbed cluster.
    OutageNotObserved,
    /// A stream a partial cut was *not* supposed to touch stopped serving
    /// anyway.
    ///
    /// The counterpart to [`Self::OutageNotObserved`], and the reason the
    /// storage scenarios stopped sharing one rule. A partial cut is a claim
    /// about routing: these namespaces go to the store being cut and those go
    /// elsewhere. If a stream on the far side stalls, the routing is not what
    /// the source says it is, and the older rule — which asked only whether
    /// everything went quiet — would have called that its cleanest possible
    /// pass.
    UnexpectedStall,
    /// A stream a delay was aimed at ran no slower than it did at rest.
    ///
    /// The latency equivalent of [`Self::OutageNotObserved`]. Where a cut
    /// proves it landed by silence, a delay can only prove it by time, and a
    /// netem rule that failed to apply leaves a run full of healthy numbers and
    /// no error anywhere — the worst artifact this suite can produce.
    SlowdownNotObserved,
    /// A stream a delay was expected to leave alone slowed down with it.
    ///
    /// The counterpart to [`Self::SlowdownNotObserved`], and the same argument
    /// [`Self::UnexpectedStall`] makes for a cut. A delay aimed at one half of
    /// the key-value layer is a claim about routing: these namespaces go to the
    /// store being slowed and those go elsewhere. A stream on the far side
    /// moving with it says the claim is wrong — and a rule that only looked for
    /// slowdown would have read that as its cleanest possible pass, because
    /// everything the run aimed at did indeed get slower.
    UnexpectedSlowdown,
    /// A stream that was confirming operations before the outage confirmed
    /// nothing at all after the heal.
    StreamNeverRecovered,
}

impl OutageViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            OutageViolation::OutageNotObserved => "outage-not-observed",
            OutageViolation::UnexpectedStall => "unexpected-stall",
            OutageViolation::SlowdownNotObserved => "slowdown-not-observed",
            OutageViolation::UnexpectedSlowdown => "unexpected-slowdown",
            OutageViolation::StreamNeverRecovered => "stream-never-recovered",
        }
    }
}

impl std::fmt::Display for OutageViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One violation, against one stream. The aggregate verdict carries no stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutageFinding {
    pub violation: OutageViolation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<Stream>,
    pub detail: String,
}

/// What one stream managed in one window.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamThroughputCell {
    pub stream: Stream,
    pub window: Window,
    /// Agents of this stream that offered at least one operation in this
    /// window. Below the stream's pool size means emitters were stalled across
    /// the whole window rather than merely slowed.
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
    /// This is the one that says whether the platform was serving, and it is
    /// deliberately not `confirmed`: during an outage the two differ by exactly
    /// the work that was accepted while the storage was gone and answered only
    /// once it came back.
    pub served: u64,
    /// Attempts that hit the client's attempt timeout rather than answering.
    pub attempts_timed_out: u64,
    pub window_secs: f64,
    pub served_per_sec: f64,
    /// This cell's rate against the same stream's own before-fault rate. `None`
    /// for the before-fault cell itself, and for a stream that never had a
    /// baseline to compare against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_of_baseline_percent: Option<f64>,
    /// The longest the stream answered nothing at all, anywhere in this window.
    ///
    /// The number that stops a small non-zero during-fault rate being read as
    /// residual service. Measured against the window's own edges, so a stream
    /// that fell silent at the start or stayed silent to the end is caught by
    /// it too. A `quietMs` close to `windowSecs` is a total outage however the
    /// rate arithmetic came out, and it needs no threshold to say so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_ms: Option<u64>,
    pub latency: LatencyStats,
}

/// The operations the outage began underneath.
///
/// These were submitted before the storage went away and were still running
/// when it did, which makes them the population at risk: the platform may have
/// executed them, may have half-executed them, and cannot tell the client
/// which. They cannot be read off the cells, because every trace of them is
/// attributed to the `before-fault` row they were submitted in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCaughtInFlight {
    pub stream: Stream,
    pub operations: u64,
    /// Distinct agents they belonged to.
    pub agents: usize,
    pub confirmed: u64,
    pub rejected: u64,
    pub indeterminate: u64,
    /// Submission to final outcome, across every attempt.
    pub duration: LatencyStats,
    pub attempts_timed_out: u64,
    /// The most attempts any one of them needed. An operation that stalled and
    /// then answered on a later attempt was rescued by the caller's retry, not
    /// returned by the platform, and the outcome alone cannot say so.
    pub max_attempts: u32,
    /// How many were still unresolved when the storage was reported reachable
    /// again.
    pub outlived_the_fault: u64,
}

/// How long one stream took to serve anything again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamRecovery {
    pub stream: Stream,
    /// Milliseconds from the heal to this stream's first confirmed operation.
    /// `None` means it never confirmed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_confirmed_ms: Option<u64>,
    /// Whether that exceeded the configured budget. Recorded, not asserted.
    pub over_budget: bool,
}

/// How operations failed while the storage was unreachable.
///
/// The acceptance criteria ask for fault-window failures, and a count alone
/// does not answer the question an operator has: whether the platform refused
/// the work definitively, or accepted it and then lost the ability to say what
/// happened. That is exactly the [`ErrorClass`] split, so the histogram is
/// keyed on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultWindowErrors {
    pub stream: Stream,
    pub class: ErrorClass,
    pub operations: u64,
    /// One message, for the operator to paste into a log query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

/// The storage-outage account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageFaultReport {
    /// The endpoint the workflow was asked to cut the executors off from,
    /// recorded so an archived result says which storage the run was about
    /// rather than leaving it to the scenario name.
    pub endpoint: String,
    /// The rule from the suite YAML, recorded whole so an archived cell can be
    /// read years later against what it was judged by rather than against
    /// today's config — including *which* rule, since the storage scenarios no
    /// longer share one.
    pub expect: OutageExpectation,
    pub recovery_budget_ms: u64,
    /// The whole workload's during-fault rate as a share of its own baseline.
    /// `None` for a run that never learned when the fault was.
    ///
    /// Recorded, no longer the verdict. It is a rate averaged over the whole
    /// fault window, so it moves with the window length even when the platform
    /// behaves identically. See `outage_quiet_floor_percent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_of_baseline_percent: Option<f64>,
    /// The least any stream *expected to stop* stayed silent during the fault,
    /// as a share of that window. This is what the quiet half of the verdict is
    /// drawn from.
    ///
    /// Under `WholeWorkload` that is every stream, so this is the run-wide
    /// minimum and means what it always did. Under `PartialWorkload` it covers
    /// only the named streams, because the others are expected to keep
    /// answering and their quiet time says nothing about whether the cut
    /// landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quietest_stream_percent: Option<f64>,
    /// The least of its own baseline any stream *expected to keep serving* held
    /// during the fault. `None` under `WholeWorkload`, which expects none to.
    ///
    /// The other half of a partial cut's verdict, and the half no shared rule
    /// could state: a partial cut claims the streams it does not touch carry on,
    /// and that claim comes from where the code routes each namespace rather
    /// than from an assumption about faults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub least_serving_stream_percent: Option<f64>,
    /// The least any stream *expected to slow down* did, as a multiple of its
    /// own before-fault median latency. `None` unless the expectation is
    /// [`OutageExpectation::LatencyDegradation`].
    ///
    /// A multiple of the stream's own baseline rather than a millisecond
    /// figure, because the streams differ by an order of magnitude at rest and
    /// a single absolute threshold would be slack for one and unreachable for
    /// another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub least_slowdown_factor: Option<f64>,
    /// The most any stream the delay was expected *not* to reach moved, as a
    /// multiple of its own before-fault median. `None` unless the expectation
    /// names steady streams.
    ///
    /// Read together with `least_slowdown_factor`, the pair is the whole
    /// experiment in two numbers: how far the streams behind the delayed store
    /// moved, and how far the ones that should not be behind it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greatest_steady_factor: Option<f64>,
    pub cells: Vec<StreamThroughputCell>,
    /// What the outage began underneath, per stream. Empty for a run that never
    /// learned when the fault was.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caught_in_flight: Vec<StreamCaughtInFlight>,
    pub recovery: Vec<StreamRecovery>,
    pub fault_window_errors: Vec<FaultWindowErrors>,
    pub findings: Vec<OutageFinding>,
}

/// One stream's per-window accumulation, before it becomes a cell.
#[derive(Default)]
struct Tally {
    agents: BTreeSet<String>,
    submitted: u64,
    confirmed: u64,
    rejected: u64,
    indeterminate: u64,
    attempts_timed_out: u64,
    durations: Vec<u64>,
    /// When this stream actually answered inside this window, sorted later.
    ///
    /// Keyed on the window a confirmation *landed* in rather than the one its
    /// operation was offered in, which is the only way either of the numbers
    /// derived from it means what it says.
    served_at: Vec<DateTime<Utc>>,
}

impl StorageFaultReport {
    /// Builds the account from the operation history.
    ///
    /// `fault` is what the workflow reported. Without it every record lands in
    /// [`Window::Unknown`] and the report carries counts but no verdict, which
    /// is the honest outcome for a run that never learned when the fault was:
    /// both thresholds are defined relative to a before-and-during comparison
    /// that cannot be made.
    pub fn build(
        records: &[OperationRecord],
        fault: Option<FaultWindow>,
        endpoint: &str,
        expect: OutageExpectation,
        recovery_budget: Duration,
    ) -> Self {
        let mut tallies: BTreeMap<(Stream, Window), Tally> = BTreeMap::new();
        let mut first_submitted: Option<DateTime<Utc>> = None;
        let mut last_completed: Option<DateTime<Utc>> = None;

        for record in records {
            let window = Window::of(record.submitted_at, fault);
            let tally = tallies.entry((record.stream, window)).or_default();

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
            // offered during the outage and answered after the heal is service
            // the fault window did not get, and counting it there is how a total
            // outage reads as partial service.
            if record.outcome == Outcome::Confirmed
                && let Some(completed) = record.completed_at
            {
                tallies
                    .entry((record.stream, Window::of(completed, fault)))
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
        // stream's before-fault rate, so a stream the workload drives rarely
        // cannot make the picture look better or worse than it was.
        let mut baseline_rate: BTreeMap<Stream, f64> = BTreeMap::new();
        let mut cells: Vec<StreamThroughputCell> = Vec::new();
        for ((stream, window), tally) in &tallies {
            let secs = window_secs(*window, fault, first_submitted, last_completed);
            let served = tally.served_at.len() as u64;
            let rate = if secs > 0.0 {
                served as f64 / secs
            } else {
                0.0
            };
            if *window == Window::BeforeFault {
                baseline_rate.insert(*stream, rate);
            }
            let quiet_ms = longest_silence_ms(
                &tally.served_at,
                window_start(*window, fault, first_submitted),
                window_end(*window, fault, last_completed),
            );
            cells.push(StreamThroughputCell {
                stream: *stream,
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
            if let Some(base) = baseline_rate.get(&cell.stream).copied()
                && base > 0.0
            {
                cell.share_of_baseline_percent = Some(round2(cell.served_per_sec / base * 100.0));
            }
        }
        cells.sort_by_key(|c| (c.stream, c.window));

        let mut report = Self {
            endpoint: endpoint.to_string(),
            expect,
            recovery_budget_ms: recovery_budget.as_millis().min(u64::MAX as u128) as u64,
            share_of_baseline_percent: None,
            quietest_stream_percent: None,
            least_serving_stream_percent: None,
            least_slowdown_factor: None,
            greatest_steady_factor: None,
            cells,
            caught_in_flight: caught_in_flight(records, fault),
            recovery: Vec::new(),
            fault_window_errors: fault_window_errors(records, fault),
            findings: Vec::new(),
        };

        report.judge_outage(fault, first_submitted, last_completed);
        report.judge_recovery(records, fault, recovery_budget, &baseline_rate);
        report
    }

    /// Did the outage land?
    ///
    /// Judged on how long each stream answered *nothing*, as a share of the
    /// fault window, and on the stream that managed it least. A stream still
    /// serving is a fault that did not land on it, and averaging would let the
    /// quiet ones outvote it.
    ///
    /// Quiet time rather than throughput because throughput is a rate over the
    /// whole window while an absorbed outage does all its serving in the
    /// seconds at the window's edges: the same handful of confirmations reads
    /// as a small share of a long window and a large share of a short one, so a
    /// throughput threshold tracks the window length rather than the platform.
    /// Quiet time is measured against the window's own edges and does not move
    /// when the window does. The share is still computed, and still reported,
    /// as context.
    ///
    /// Streams with no before-fault serving are skipped. A stream the run never
    /// got going says nothing about whether the storage went away, and the
    /// driver's baseline gate already reports it.
    fn judge_outage(
        &mut self,
        fault: Option<FaultWindow>,
        first_submitted: Option<DateTime<Utc>>,
        last_completed: Option<DateTime<Utc>>,
    ) {
        if fault.is_none() {
            return;
        }
        let total = |window: Window| -> f64 {
            let served: u64 = self
                .cells
                .iter()
                .filter(|c| c.window == window)
                .map(|c| c.served)
                .sum();
            let secs = window_secs(window, fault, first_submitted, last_completed);
            if secs > 0.0 {
                served as f64 / secs
            } else {
                0.0
            }
        };

        let baseline = total(Window::BeforeFault);
        if baseline <= 0.0 {
            return;
        }
        self.share_of_baseline_percent =
            Some(round2(total(Window::DuringFault) / baseline * 100.0));

        let served_before: BTreeSet<Stream> = self
            .cells
            .iter()
            .filter(|c| c.window == Window::BeforeFault && c.served > 0)
            .map(|c| c.stream)
            .collect();

        // Split by what this scenario claims the cut does, not by what a cut
        // does in general. The two sets are judged by opposite tests, and a
        // stream in neither — one that never served before the fault — is
        // judged by neither, because it has no baseline to be read against.
        let during: Vec<&StreamThroughputCell> = self
            .cells
            .iter()
            .filter(|c| c.window == Window::DuringFault && c.window_secs > 0.0)
            .filter(|c| served_before.contains(&c.stream))
            .collect();

        // Driven from `served_before` for the same reason as the serving list
        // below: a stream that answered nothing at all leaves no during-fault
        // cell, and that is total silence rather than missing data. Scoring the
        // two sides of one fact differently — absence as 0% served but as
        // "unknown" quiet — would let the same run read as a stall on one check
        // and as no evidence on the other.
        let mut quiet: Vec<(Stream, f64)> = served_before
            .iter()
            .copied()
            .filter(|stream| self.expect.expects_silence(*stream))
            .filter_map(|stream| match during.iter().find(|c| c.stream == stream) {
                Some(cell) => cell
                    .quiet_ms
                    .map(|ms| (stream, round2(ms as f64 / (cell.window_secs * 10.0)))),
                None => Some((stream, 100.0)),
            })
            .collect();
        quiet.sort_by(|a, b| a.1.total_cmp(&b.1));

        if let Some(&(stream, quietest)) = quiet.first() {
            self.quietest_stream_percent = Some(quietest);
            let floor = self.expect.quiet_floor_percent();
            if quietest < floor {
                self.findings.push(OutageFinding {
                    violation: OutageViolation::OutageNotObserved,
                    stream: Some(stream),
                    detail: format!(
                        "{stream} was expected to stop while {} was unreachable and kept \
                         answering instead, silent for only {quietest}% of the fault window \
                         against a {floor}% floor — so this run has no evidence the cut landed",
                        self.endpoint
                    ),
                });
            }
        }

        // The latency half, before the serving one, because a delay that never
        // applied makes everything below it uninteresting: the streams would
        // all be serving normally and the run would read as a clean pass of an
        // experiment that never happened.
        if let Some(slowdown_floor) = self.expect.slowdown_floor() {
            let mut slowdowns: Vec<(Stream, f64)> = served_before
                .iter()
                .copied()
                .filter(|stream| self.expect.expects_slowdown(*stream))
                .filter_map(|stream| {
                    // Each stream against its own before-fault median. Medians
                    // rather than means because a handful of retried operations
                    // drag a mean far enough to hide whether the typical
                    // operation moved at all, which is the question.
                    let baseline = self.median_ms(stream, Window::BeforeFault)?;
                    let during = self.median_ms(stream, Window::DuringFault)?;
                    (baseline > 0.0).then_some((stream, round2(during / baseline)))
                })
                .collect();
            slowdowns.sort_by(|a, b| a.1.total_cmp(&b.1));

            if let Some(&(stream, least)) = slowdowns.first() {
                self.least_slowdown_factor = Some(least);
                if least < slowdown_floor {
                    self.findings.push(OutageFinding {
                        violation: OutageViolation::SlowdownNotObserved,
                        stream: Some(stream),
                        detail: format!(
                            "{stream} was expected to slow down while {} was delayed and ran at \
                             {least}x its own baseline median against a {slowdown_floor}x floor \
                             — so this run has no evidence the delay reached the executors",
                            self.endpoint
                        ),
                    });
                }
            }
        }

        // The far side of the same claim. A delay aimed at one store predicts
        // both which streams move and which do not, and only the second half
        // can catch a delay that landed somewhere wider than the run says.
        if let Some(steady_ceiling) = self.expect.steady_ceiling() {
            let mut steadies: Vec<(Stream, f64)> = served_before
                .iter()
                .copied()
                .filter(|stream| self.expect.expects_steady(*stream))
                .filter_map(|stream| {
                    let baseline = self.median_ms(stream, Window::BeforeFault)?;
                    let during = self.median_ms(stream, Window::DuringFault)?;
                    (baseline > 0.0).then_some((stream, round2(during / baseline)))
                })
                .collect();
            steadies.sort_by(|a, b| b.1.total_cmp(&a.1));

            if let Some(&(stream, most)) = steadies.first() {
                self.greatest_steady_factor = Some(most);
                if most > steady_ceiling {
                    self.findings.push(OutageFinding {
                        violation: OutageViolation::UnexpectedSlowdown,
                        stream: Some(stream),
                        detail: format!(
                            "{stream} does not depend on {} and was expected to run at its own \
                             pace through the delay, but ran at {most}x its own baseline median \
                             against a {steady_ceiling}x ceiling — the storage this stream \
                             actually reaches is not what the routing says it is",
                            self.endpoint
                        ),
                    });
                }
            }
        }

        let Some(serving_floor) = self.expect.serving_floor_percent() else {
            return;
        };
        // Driven from the streams that served *before* the cut rather than from
        // the during-fault cells, because the worst case leaves no such cell at
        // all: a stream whose operations all stall produces nothing to tally,
        // and reading the cells alone would let total silence escape the one
        // check that exists to catch it. Absence is the strongest evidence
        // here, so it is scored as zero rather than skipped.
        let mut serving: Vec<(Stream, f64)> = served_before
            .iter()
            .copied()
            .filter(|stream| !self.expect.expects_silence(*stream))
            .map(|stream| {
                let share = during
                    .iter()
                    .find(|c| c.stream == stream)
                    .and_then(|c| c.share_of_baseline_percent)
                    .unwrap_or(0.0);
                (stream, share)
            })
            .collect();
        serving.sort_by(|a, b| a.1.total_cmp(&b.1));

        if let Some(&(stream, least)) = serving.first() {
            self.least_serving_stream_percent = Some(least);
            if least < serving_floor {
                self.findings.push(OutageFinding {
                    violation: OutageViolation::UnexpectedStall,
                    stream: Some(stream),
                    detail: format!(
                        "{stream} does not depend on {} and was expected to carry on through \
                         the cut, but held only {least}% of its own baseline against a \
                         {serving_floor}% floor — the storage this stream actually reaches is \
                         not what the routing says it is",
                        self.endpoint
                    ),
                });
            }
        }
    }

    /// How long each stream took to serve again, and which never did.
    ///
    /// Only streams that were working *before* the outage are judged. A stream
    /// the run never got going is a different problem, already reported by the
    /// baseline gate in the driver, and calling it unrecovered here would put
    /// the same failure in two places under two names.
    fn judge_recovery(
        &mut self,
        records: &[OperationRecord],
        fault: Option<FaultWindow>,
        budget: Duration,
        baseline_rate: &BTreeMap<Stream, f64>,
    ) {
        let Some(FaultWindow {
            recovered_at: Some(recovered),
            ..
        }) = fault
        else {
            return;
        };
        let budget_ms = budget.as_millis().min(i64::MAX as u128) as i64;

        for (stream, base) in baseline_rate {
            if *base <= 0.0 {
                continue;
            }
            let first = records
                .iter()
                .filter(|r| r.stream == *stream && r.outcome == Outcome::Confirmed)
                .filter_map(|r| r.completed_at)
                .filter(|at| *at >= recovered)
                .min();

            let first_confirmed_ms =
                first.map(|at| (at - recovered).num_milliseconds().max(0) as u64);
            let over_budget = first
                .map(|at| (at - recovered).num_milliseconds() > budget_ms)
                .unwrap_or(false);

            if first.is_none() {
                self.findings.push(OutageFinding {
                    violation: OutageViolation::StreamNeverRecovered,
                    stream: Some(*stream),
                    detail: format!(
                        "the {stream} stream confirmed operations before the outage and none \
                         after {} became reachable again",
                        self.endpoint
                    ),
                });
            }
            self.recovery.push(StreamRecovery {
                stream: *stream,
                first_confirmed_ms,
                over_budget,
            });
        }
    }

    /// One stream's cell for one window, for the tests and for anything reading
    /// the report without wanting to scan the whole list.
    pub fn cell(&self, stream: Stream, window: Window) -> Option<&StreamThroughputCell> {
        self.cells
            .iter()
            .find(|c| c.stream == stream && c.window == window)
    }

    /// Whether the report found anything a human has to act on.
    pub fn has_findings(&self) -> bool {
        !self.findings.is_empty()
    }

    /// Lines that need a human. Empty on a run where the outage landed and
    /// everything came back.
    pub fn attention_lines(&self) -> Vec<String> {
        self.findings
            .iter()
            .map(|f| match f.stream {
                Some(stream) => format!("{}: {stream}: {}", f.violation, f.detail),
                None => format!("{}: {}", f.violation, f.detail),
            })
            .collect()
    }

    /// One stream's median latency in a window, if it has a cell with samples.
    fn median_ms(&self, stream: Stream, window: Window) -> Option<f64> {
        self.cells
            .iter()
            .find(|c| c.stream == stream && c.window == window)
            .filter(|c| c.latency.count > 0)
            .map(|c| c.latency.p50_ms as f64)
    }

    /// Context a reader needs in order to read the cells, which is not itself a
    /// problem.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(share) = self.share_of_baseline_percent {
            // Says which streams the numbers are about, because under a partial
            // cut they are about a subset and a reader who assumes otherwise
            // draws the opposite conclusion from the same figure.
            // Under a latency expectation nothing is meant to go quiet, so the
            // quiet figure is absent by design rather than missing, and the
            // slowdown takes its place as what says the fault landed.
            let quiet = match (self.quietest_stream_percent, self.least_slowdown_factor) {
                (_, Some(factor)) => format!(
                    "the streams expected to slow down ran at least {factor}x their own baseline \
                     median (floor {}x)",
                    self.expect.slowdown_floor().unwrap_or_default()
                ),
                (Some(q), None) => format!(
                    "the streams expected to stop were silent for at least {q}% of the fault \
                     window (floor {}%)",
                    self.expect.quiet_floor_percent()
                ),
                (None, None) => {
                    "no stream had a before-fault baseline to be judged against".to_string()
                }
            };
            // The far side of a delay, when the scenario named one. Printed
            // next to the slowdown rather than left in the cells, because the
            // two numbers only mean anything together: a run where everything
            // moved by the same multiple measured a slower cluster, not a
            // slower store.
            let steady = match (self.greatest_steady_factor, self.expect.steady_ceiling()) {
                (Some(most), Some(ceiling)) => format!(
                    ", the streams expected to be left alone ran at no more than {most}x theirs \
                     (ceiling {ceiling}x)"
                ),
                _ => String::new(),
            };
            let serving = match (
                self.least_serving_stream_percent,
                self.expect.serving_floor_percent(),
            ) {
                (Some(least), Some(floor)) => format!(
                    ", the streams expected to carry on held at least {least}% of their own \
                     baseline (floor {floor}%)"
                ),
                _ => String::new(),
            };
            lines.push(format!(
                "Storage outage: {quiet}{steady}{serving} while {} was unreachable, and the workload held \
                 {share}% of its baseline throughput across that window",
                self.endpoint
            ));
        } else {
            lines.push(format!(
                "Storage outage: no fault window was reported, so the {} cells carry counts but \
                 no verdict",
                self.endpoint
            ));
        }
        let over: Vec<String> = self
            .recovery
            .iter()
            .filter(|r| r.over_budget)
            .map(|r| {
                format!(
                    "{} ({}ms)",
                    r.stream,
                    r.first_confirmed_ms.unwrap_or_default()
                )
            })
            .collect();
        if !over.is_empty() {
            lines.push(format!(
                "Storage outage: {} took longer than the {}ms recovery budget to serve again: {}",
                over.len(),
                self.recovery_budget_ms,
                over.join(", ")
            ));
        }
        lines
    }
}

/// The operations that were submitted before the outage and still running when
/// it started.
fn caught_in_flight(
    records: &[OperationRecord],
    fault: Option<FaultWindow>,
) -> Vec<StreamCaughtInFlight> {
    let Some(window) = fault else {
        return Vec::new();
    };
    let mut by_stream: BTreeMap<Stream, Vec<&OperationRecord>> = BTreeMap::new();
    for record in records {
        // Submitted before the cut, and either still unfinished when it landed
        // or finished after it. An operation with no completion at all is
        // included: the driver never learned how it ended, which is the same
        // doubt in a starker form.
        if record.submitted_at >= window.injected_at {
            continue;
        }
        let still_running = record
            .completed_at
            .map(|at| at >= window.injected_at)
            .unwrap_or(true);
        if still_running {
            by_stream.entry(record.stream).or_default().push(record);
        }
    }

    by_stream
        .into_iter()
        .map(|(stream, caught)| StreamCaughtInFlight {
            stream,
            operations: caught.len() as u64,
            agents: caught
                .iter()
                .map(|r| r.agent.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            confirmed: count_of(&caught, Outcome::Confirmed),
            rejected: count_of(&caught, Outcome::Rejected),
            indeterminate: count_of(&caught, Outcome::Indeterminate),
            duration: LatencyStats::from_durations(
                caught.iter().map(|r| r.duration_ms).collect::<Vec<_>>(),
            ),
            attempts_timed_out: caught.iter().map(|r| r.attempts_timed_out()).sum(),
            max_attempts: caught.iter().map(|r| r.attempts).max().unwrap_or(0),
            outlived_the_fault: caught
                .iter()
                .filter(|r| match (r.completed_at, window.recovered_at) {
                    (Some(at), Some(recovered)) => at >= recovered,
                    (None, _) => true,
                    _ => false,
                })
                .count() as u64,
        })
        .collect()
}

/// How the operations submitted during the outage failed, by stream and class.
fn fault_window_errors(
    records: &[OperationRecord],
    fault: Option<FaultWindow>,
) -> Vec<FaultWindowErrors> {
    let mut tallies: BTreeMap<(Stream, ErrorClass), (u64, Option<String>)> = BTreeMap::new();
    for record in records {
        if Window::of(record.submitted_at, fault) != Window::DuringFault {
            continue;
        }
        let Some(class) = record.error_class else {
            continue;
        };
        let entry = tallies.entry((record.stream, class)).or_insert((0, None));
        entry.0 += 1;
        if entry.1.is_none() {
            entry.1.clone_from(&record.error);
        }
    }

    let mut rows: Vec<FaultWindowErrors> = tallies
        .into_iter()
        .map(
            |((stream, class), (operations, example))| FaultWindowErrors {
                stream,
                class,
                operations,
                example,
            },
        )
        .collect();
    // Commonest first: an operator reading this wants the dominant failure mode
    // before the long tail.
    rows.sort_by(|a, b| {
        b.operations
            .cmp(&a.operations)
            .then((a.stream, a.class).cmp(&(b.stream, b.class)))
    });
    rows
}

fn count_of(records: &[&OperationRecord], outcome: Outcome) -> u64 {
    records.iter().filter(|r| r.outcome == outcome).count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::{AttemptRecord, Phase};
    use chrono::TimeDelta;
    use test_r::test;

    const ENDPOINT: &str = "golem-postgres-dev-keyvalue.cluster-example.rds.amazonaws.com";
    const QUIET_FLOOR: f64 = 50.0;
    const AGENT: &str = "chaos-s16-durable-0000";

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn fault() -> FaultWindow {
        FaultWindow {
            injected_at: t0(),
            recovered_at: Some(t0() + TimeDelta::seconds(180)),
        }
    }

    /// One operation, submitted `offset` seconds from the moment the storage
    /// was taken away. Negative offsets are the baseline.
    fn op(stream: Stream, offset_secs: i64, outcome: Outcome) -> OperationRecord {
        let submitted_at = t0() + TimeDelta::seconds(offset_secs);
        OperationRecord {
            op_id: 0,
            stream,
            phase: Phase::Baseline,
            agent: AGENT.to_string(),
            method: "increment".to_string(),
            idempotency_key: format!("{stream}-{offset_secs}"),
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
    /// invocation takes while the database is unreachable.
    fn stalled(stream: Stream, offset_secs: i64) -> OperationRecord {
        let mut record = op(stream, offset_secs, Outcome::Indeterminate);
        record.duration_ms = 245_000;
        record.completed_at = Some(record.submitted_at + TimeDelta::seconds(245));
        record.returned_value = None;
        record.attempts = 2;
        record.error_class = Some(ErrorClass::Transport);
        record.error = Some("attempt timed out after 120s".to_string());
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

    /// 300s of baseline on both streams, then `during` confirmations while the
    /// storage is gone, then 240s of recovery.
    fn history(during: usize, recovery: bool) -> Vec<OperationRecord> {
        let mut records = Vec::new();
        for second in 1..=300 {
            records.push(op(Stream::Durable, -second, Outcome::Confirmed));
            records.push(op(Stream::Scheduled, -second, Outcome::Confirmed));
        }
        for i in 0..during {
            records.push(op(Stream::Durable, i as i64, Outcome::Confirmed));
            records.push(op(Stream::Scheduled, i as i64, Outcome::Confirmed));
        }
        if recovery {
            for second in 181..=420 {
                records.push(op(Stream::Durable, second, Outcome::Confirmed));
                records.push(op(Stream::Scheduled, second, Outcome::Confirmed));
            }
        }
        records
    }

    const SERVING_FLOOR: f64 = 50.0;

    /// A workload shaped like S18's: `durable` on the far side of the cut and
    /// `ephemeral` behind it. `durable_during` and `ephemeral_during` say how
    /// many confirmations each managed while the storage was gone, so one
    /// helper covers "behaved as designed", "the wrong stream stalled" and
    /// "nothing stopped at all".
    fn partial_history(durable_during: usize, ephemeral_during: usize) -> Vec<OperationRecord> {
        let mut records = Vec::new();
        for second in 1..=300 {
            records.push(op(Stream::Durable, -second, Outcome::Confirmed));
            records.push(op(Stream::Ephemeral, -second, Outcome::Confirmed));
        }
        for i in 0..durable_during {
            records.push(op(Stream::Durable, i as i64, Outcome::Confirmed));
        }
        for i in 0..ephemeral_during {
            records.push(op(Stream::Ephemeral, i as i64, Outcome::Confirmed));
        }
        for second in 181..=420 {
            records.push(op(Stream::Durable, second, Outcome::Confirmed));
            records.push(op(Stream::Ephemeral, second, Outcome::Confirmed));
        }
        records
    }

    /// The same shape as [`partial_history`], but with the operations that did
    /// not confirm still *recorded* as stalled attempts.
    ///
    /// This is what a real run looks like: the workload keeps submitting into a
    /// stall, so the stream has a during-fault cell showing what it offered and
    /// nothing served. S18's first run recorded 321 such ephemeral operations.
    /// [`partial_history`] covers the other shape, where a stream produced no
    /// during-fault record at all, and the two must reach the same verdict.
    fn partial_history_with_stalls(
        durable_confirmed: usize,
        ephemeral_confirmed: usize,
    ) -> Vec<OperationRecord> {
        let mut records = Vec::new();
        for second in 1..=300 {
            records.push(op(Stream::Durable, -second, Outcome::Confirmed));
            records.push(op(Stream::Ephemeral, -second, Outcome::Confirmed));
        }
        for (stream, confirmed) in [
            (Stream::Durable, durable_confirmed),
            (Stream::Ephemeral, ephemeral_confirmed),
        ] {
            for i in 0..180 {
                if i < confirmed {
                    records.push(op(stream, i as i64, Outcome::Confirmed));
                } else {
                    records.push(stalled(stream, i as i64));
                }
            }
        }
        for second in 181..=420 {
            records.push(op(Stream::Durable, second, Outcome::Confirmed));
            records.push(op(Stream::Ephemeral, second, Outcome::Confirmed));
        }
        records
    }

    const SLOWDOWN_FLOOR: f64 = 2.0;
    const STEADY_CEILING: f64 = 1.5;

    /// A workload where every stream keeps serving and `ephemeral` is the one
    /// the delay is aimed at. `ephemeral_ms` is what its operations take during
    /// the fault, against a 200ms baseline.
    fn latency_history(ephemeral_ms: u64, ephemeral_during: usize) -> Vec<OperationRecord> {
        latency_history_with(ephemeral_ms, ephemeral_during, 50)
    }

    /// As [`latency_history`], with the far-side stream's during-fault duration
    /// under the test's control too, so a run where the delay reached more than
    /// it was aimed at can be built.
    fn latency_history_with(
        ephemeral_ms: u64,
        ephemeral_during: usize,
        durable_during_ms: u64,
    ) -> Vec<OperationRecord> {
        let mut records = Vec::new();
        for second in 1..=300 {
            records.push(timed(Stream::Durable, -second, 50));
            records.push(timed(Stream::Ephemeral, -second, 200));
        }
        for i in 0..180 {
            records.push(timed(Stream::Durable, i as i64, durable_during_ms));
        }
        for i in 0..ephemeral_during {
            records.push(timed(Stream::Ephemeral, i as i64, ephemeral_ms));
        }
        for second in 181..=420 {
            records.push(timed(Stream::Durable, second, 50));
            records.push(timed(Stream::Ephemeral, second, 200));
        }
        records
    }

    /// A confirmed operation that took a stated time, so a window's median is
    /// something the test controls rather than something it inherits.
    fn timed(stream: Stream, offset_secs: i64, duration_ms: u64) -> OperationRecord {
        let mut record = op(stream, offset_secs, Outcome::Confirmed);
        record.duration_ms = duration_ms;
        record.completed_at =
            Some(record.submitted_at + TimeDelta::milliseconds(duration_ms as i64));
        record
    }

    fn build_latency(records: &[OperationRecord]) -> StorageFaultReport {
        StorageFaultReport::build(
            records,
            Some(fault()),
            ENDPOINT,
            OutageExpectation::LatencyDegradation {
                slowed: vec![Stream::Ephemeral],
                slowdown_floor: SLOWDOWN_FLOOR,
                steady: Vec::new(),
                steady_ceiling: STEADY_CEILING,
                serving_floor_percent: SERVING_FLOOR,
            },
            Duration::from_secs(120),
        )
    }

    /// The same delay, with the claim S15 and S17 both make about the streams
    /// on the far side of the store written down.
    fn build_latency_steady(records: &[OperationRecord]) -> StorageFaultReport {
        StorageFaultReport::build(
            records,
            Some(fault()),
            ENDPOINT,
            OutageExpectation::LatencyDegradation {
                slowed: vec![Stream::Ephemeral],
                slowdown_floor: SLOWDOWN_FLOOR,
                steady: vec![Stream::Durable],
                steady_ceiling: STEADY_CEILING,
                serving_floor_percent: SERVING_FLOOR,
            },
            Duration::from_secs(120),
        )
    }

    fn build_partial(records: &[OperationRecord]) -> StorageFaultReport {
        StorageFaultReport::build(
            records,
            Some(fault()),
            ENDPOINT,
            OutageExpectation::PartialWorkload {
                silenced: vec![Stream::Ephemeral],
                quiet_floor_percent: QUIET_FLOOR,
                serving_floor_percent: SERVING_FLOOR,
            },
            Duration::from_secs(120),
        )
    }

    fn build(records: &[OperationRecord]) -> StorageFaultReport {
        StorageFaultReport::build(
            records,
            Some(fault()),
            ENDPOINT,
            OutageExpectation::WholeWorkload {
                quiet_floor_percent: QUIET_FLOOR,
            },
            Duration::from_secs(120),
        )
    }

    /// The healthy shape: throughput collapses while the database is gone and
    /// comes back afterwards, and the report raises nothing.
    #[test]
    fn an_outage_that_lands_and_heals_has_no_findings() {
        let report = build(&history(2, true));

        assert!(
            report.findings.is_empty(),
            "expected no findings, got {:?}",
            report.findings
        );
        let during = report.cell(Stream::Durable, Window::DuringFault).unwrap();
        assert!(
            report.quietest_stream_percent.unwrap() > QUIET_FLOOR,
            "the streams should have gone silent, got {:?} from {during:?}",
            report.quietest_stream_percent
        );
    }

    /// The one thing this scenario cannot afford to be quiet about. A partition
    /// that never took hold leaves every other number describing a cluster
    /// nothing happened to, and a reader has to be told rather than left to
    /// infer it from cells that all look fine.
    #[test]
    fn a_workload_that_kept_working_reports_the_outage_as_not_observed() {
        // 180 confirmations per stream across the 180s fault window is the
        // baseline cadence carrying straight on through it.
        let report = build(&history(180, true));

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::OutageNotObserved),
            "expected an outage-not-observed finding, got {:?}",
            report.findings
        );
        assert!(
            report.quietest_stream_percent.unwrap() < QUIET_FLOOR,
            "a stream answering every second is never quiet for long, got {:?}",
            report.quietest_stream_percent
        );
    }

    /// The regression S18's first run produced, expressed as a test.
    ///
    /// Under a partial cut one stream stops and the others carry on by design.
    /// The whole-workload rule reads that as a fault that never landed, because
    /// the only question it can ask is whether *everything* went quiet. Run
    /// 33130077355 hit exactly this: `durable` held 100% of its baseline while
    /// the Redis partition had plainly landed on `ephemeral`.
    #[test]
    fn a_partial_cut_is_not_judged_by_the_streams_it_was_never_going_to_stop() {
        let records = partial_history(180, 0);

        let shared = build(&records);
        assert!(
            shared
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::OutageNotObserved),
            "the whole-workload rule is expected to misjudge this, or the test guards nothing"
        );

        let report = build_partial(&records);
        assert!(
            report.findings.is_empty(),
            "one stream stopping and the rest carrying on is the expected outcome here, got {:?}",
            report.findings
        );
    }

    /// The half no shared rule could state.
    ///
    /// A partial cut claims the streams it does not touch keep working, and
    /// that claim comes from where the code routes each namespace rather than
    /// from an assumption about faults. A stall on the far side means the
    /// routing is not what the source says, and the older rule would have
    /// called it the cleanest possible pass: everything went quiet.
    #[test]
    fn a_stream_the_cut_should_not_have_touched_stalling_is_a_finding() {
        let report = build_partial(&partial_history(0, 0));

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::UnexpectedStall),
            "durable does not depend on this endpoint and stopped anyway, got {:?}",
            report.findings
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::OutageNotObserved),
            "ephemeral did stop, so the cut was observed; got {:?}",
            report.findings
        );
    }

    /// The named stream refusing to stop still fails, which is the check the
    /// partial rule inherits rather than replaces.
    #[test]
    fn a_partial_cut_whose_named_stream_kept_serving_is_still_not_observed() {
        let report = build_partial(&partial_history(180, 180));

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::OutageNotObserved
                    && f.stream == Some(Stream::Ephemeral)),
            "ephemeral was the stream expected to stop, got {:?}",
            report.findings
        );
    }

    /// Both numbers are reported, and each covers only the streams its own rule
    /// judges. Reading the quiet figure as run-wide is how the first S18 report
    /// told its reader to draw the wrong conclusion.
    #[test]
    fn each_reported_share_covers_only_the_streams_its_rule_judges() {
        let report = build_partial(&partial_history(180, 0));

        assert!(
            report.quietest_stream_percent.unwrap() > QUIET_FLOOR,
            "the quiet figure must describe ephemeral, which stopped, not durable, got {:?}",
            report.quietest_stream_percent
        );
        assert!(
            report.least_serving_stream_percent.unwrap() > SERVING_FLOOR,
            "the serving figure must describe durable, which carried on, got {:?}",
            report.least_serving_stream_percent
        );
    }

    /// The same two verdicts against the shape a real run produces, where the
    /// stalled operations are recorded rather than absent. A stream that
    /// offered work and served none of it must read the same as one that
    /// offered nothing at all.
    #[test]
    fn a_recorded_stall_reads_the_same_as_a_stream_that_went_missing() {
        let as_designed = build_partial(&partial_history_with_stalls(180, 0));
        assert!(
            as_designed.findings.is_empty(),
            "ephemeral stalling and durable carrying on is the expected outcome, got {:?}",
            as_designed.findings
        );

        let wrong_stream = build_partial(&partial_history_with_stalls(0, 0));
        assert!(
            wrong_stream
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::UnexpectedStall
                    && f.stream == Some(Stream::Durable)),
            "durable served nothing while off the fault path, got {:?}",
            wrong_stream.findings
        );
    }

    /// The delay landed and the platform degraded rather than broke, which is
    /// the outcome S15 and S17 are written to confirm.
    #[test]
    fn a_slowed_stream_that_kept_serving_is_the_expected_outcome() {
        // 200ms at rest, 1s under the delay, and still completing throughout.
        let report = build_latency(&latency_history(1_000, 180));

        assert!(
            report.findings.is_empty(),
            "slower but still serving is what a delay is supposed to produce, got {:?}",
            report.findings
        );
        assert_eq!(
            report.least_slowdown_factor,
            Some(5.0),
            "1000ms against a 200ms baseline is 5x"
        );
        assert!(
            report.quietest_stream_percent.is_none(),
            "nothing is expected to go quiet under a delay, so there is no quiet verdict to draw"
        );
    }

    /// The netem rule that never applied. Without this the run is full of
    /// healthy numbers and no error anywhere, which is the worst artifact this
    /// suite can produce — the same failure `outage-not-observed` exists to
    /// catch, in the units a delay is measured in.
    #[test]
    fn a_delay_that_did_not_reach_the_executors_is_a_finding() {
        // 240ms against a 200ms baseline: 1.2x, well under the 2x floor.
        let report = build_latency(&latency_history(240, 180));

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::SlowdownNotObserved
                    && f.stream == Some(Stream::Ephemeral)),
            "a stream that barely moved cannot evidence a delay, got {:?}",
            report.findings
        );
    }

    /// A delay is supposed to cost time, not work. A slowed stream that stops
    /// entirely means the added latency broke something — a timeout fired, a
    /// pool drained — and that is a different and worse outcome than slowness.
    #[test]
    fn a_delay_that_stopped_a_stream_rather_than_slowing_it_is_a_finding() {
        // Slow enough to clear the slowdown floor, but only a trickle of
        // operations got through, so it degraded past degradation.
        let report = build_latency(&latency_history(1_000, 2));

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::UnexpectedStall),
            "a stream serving 2 operations where it served 180 has stopped, got {:?}",
            report.findings
        );
    }

    /// Both halves of a delay's claim, held at once: the stream behind the
    /// slowed store moved and the stream that is not behind it did not. This is
    /// the shape S17 measured and the one S15 predicts with the streams
    /// swapped, and it is what makes either run attributable to the store
    /// rather than to a slower cluster.
    #[test]
    fn a_steady_stream_that_stayed_steady_is_what_makes_the_slowdown_attributable() {
        let report = build_latency_steady(&latency_history_with(1_000, 180, 50));

        assert!(
            report.findings.is_empty(),
            "one stream slower and the other unchanged is the expected outcome, got {:?}",
            report.findings
        );
        assert_eq!(report.least_slowdown_factor, Some(5.0));
        assert_eq!(
            report.greatest_steady_factor,
            Some(1.0),
            "50ms against a 50ms baseline has not moved"
        );
    }

    /// The finding a rule that only looks for slowdown cannot raise. Everything
    /// the delay was aimed at did get slower, so the slowdown check passes and
    /// the run reads as clean — while a stream the routing says is nowhere near
    /// the delayed store moved with it, which means the routing is wrong or the
    /// rule landed wider than the manifest says.
    #[test]
    fn a_stream_the_delay_should_not_have_touched_slowing_with_it_is_a_finding() {
        // The delayed stream still clears its floor at 5x, so nothing else in
        // the report objects.
        let report = build_latency_steady(&latency_history_with(1_000, 180, 550));

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::UnexpectedSlowdown
                    && f.stream == Some(Stream::Durable)),
            "11x on a stream that does not depend on the delayed store is a finding, got {:?}",
            report.findings
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::SlowdownNotObserved),
            "the aimed-at stream did slow down, so that check should be quiet: {:?}",
            report.findings
        );
    }

    /// A stream the run barely drives cannot flip the verdict on its own. It is
    /// judged on how long it was silent, not on its rate against a baseline, so
    /// a trickle reads as the near-total silence it is rather than as a stream
    /// holding up.
    #[test]
    fn a_trickle_stream_does_not_make_the_outage_look_unobserved() {
        let mut records = history(0, true);
        // A trickle of a third stream that keeps working throughout: two
        // operations in the whole fault window against a baseline of hundreds.
        for i in 0..2 {
            records.push(op(Stream::Promise, i, Outcome::Confirmed));
        }
        for second in 1..=10 {
            records.push(op(Stream::Promise, -second, Outcome::Confirmed));
        }
        let report = build(&records);

        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.violation == OutageViolation::OutageNotObserved),
            "a trickle should not make the outage look unobserved, got {:?}",
            report.findings
        );
    }

    /// The inverse of the trickle case, and the one the old aggregate verdict
    /// could miss. A single stream still answering all the way through is a
    /// fault that did not land on it, and the silence of the others must not
    /// vote it down.
    #[test]
    fn a_stream_still_answering_is_a_finding_even_when_the_others_are_silent() {
        let mut records = history(0, true);
        for second in 1..=10 {
            records.push(op(Stream::Promise, -second, Outcome::Confirmed));
        }
        // Answering every second of the fault window, unlike durable and
        // scheduled which say nothing at all.
        for second in 0..180 {
            records.push(op(Stream::Promise, second, Outcome::Confirmed));
        }
        let report = build(&records);

        let finding = report
            .findings
            .iter()
            .find(|f| f.violation == OutageViolation::OutageNotObserved)
            .expect("a stream answering throughout is a fault that did not land on it");
        assert_eq!(finding.stream, Some(Stream::Promise));
    }

    /// An absorbed outage: silence across the window with the serving bunched
    /// into the seconds at its two edges, which is the shape the platform
    /// produces once storage failures are retried rather than fatal.
    fn absorbed(fault_secs: i64) -> StorageFaultReport {
        let mut records = Vec::new();
        for second in 1..=300 {
            for _ in 0..10 {
                records.push(op(Stream::Durable, -second, Outcome::Confirmed));
                records.push(op(Stream::Scheduled, -second, Outcome::Confirmed));
            }
        }
        for edge in [0, fault_secs - 1] {
            for _ in 0..60 {
                records.push(op(Stream::Durable, edge, Outcome::Confirmed));
                records.push(op(Stream::Scheduled, edge, Outcome::Confirmed));
            }
        }
        for second in fault_secs + 1..=fault_secs + 240 {
            records.push(op(Stream::Durable, second, Outcome::Confirmed));
            records.push(op(Stream::Scheduled, second, Outcome::Confirmed));
        }
        StorageFaultReport::build(
            &records,
            Some(FaultWindow {
                injected_at: t0(),
                recovered_at: Some(t0() + TimeDelta::seconds(fault_secs)),
            }),
            ENDPOINT,
            OutageExpectation::WholeWorkload {
                quiet_floor_percent: QUIET_FLOOR,
            },
            Duration::from_secs(120),
        )
    }

    /// The regression this floor exists for.
    ///
    /// An absorbed outage does all its serving in the seconds at the window's
    /// edges, so a during-fault *rate* divides one fixed burst by the window
    /// length: identical platform behaviour reads as a small share of a long
    /// window and a large share of a short one. Shortening S16's window from
    /// 180s to 60s duly tripped the old 15% ceiling on a partition that had
    /// plainly landed. Quiet time is measured against the window's own edges
    /// and does not move with it.
    #[test]
    fn the_verdict_does_not_move_when_the_fault_window_does() {
        let long = absorbed(180);
        let short = absorbed(60);

        for (secs, report) in [(180, &long), (60, &short)] {
            assert!(
                report
                    .findings
                    .iter()
                    .all(|f| f.violation != OutageViolation::OutageNotObserved),
                "a {secs}s absorbed outage did land, got {:?}",
                report.findings
            );
            assert!(
                report.quietest_stream_percent.unwrap() > QUIET_FLOOR,
                "a {secs}s absorbed outage is silent for most of its window, got {:?}",
                report.quietest_stream_percent
            );
        }

        // And the number that used to decide it, kept as the demonstration:
        // the same shape reads very differently at the two lengths, which is
        // exactly why it could not stay the verdict.
        let long_share = long.share_of_baseline_percent.unwrap();
        let short_share = short.share_of_baseline_percent.unwrap();
        // 15% was the old ceiling. The long window sits under it and the short
        // one over it, on the same behaviour, which is the whole defect.
        assert!(
            long_share < 15.0 && short_share > 15.0,
            "this test only means something if the old ceiling would have flipped between the \
             two windows, got {long_share}% over 180s against {short_share}% over 60s"
        );
    }

    /// A stream that was working before the outage and confirmed nothing after
    /// the heal is the platform losing a mechanism, not a slow recovery.
    #[test]
    fn a_stream_that_never_came_back_is_a_finding() {
        let mut records = history(0, true);
        records.retain(|r| !(r.stream == Stream::Scheduled && r.submitted_at > t0()));
        let report = build(&records);

        let finding = report
            .findings
            .iter()
            .find(|f| f.violation == OutageViolation::StreamNeverRecovered)
            .expect("expected a stream-never-recovered finding");
        assert_eq!(finding.stream, Some(Stream::Scheduled));
    }

    /// A stream the run never got going is the driver's problem, already
    /// reported by the baseline gate. Calling it unrecovered here would put one
    /// failure in two places under two names.
    #[test]
    fn a_stream_that_never_worked_at_all_is_not_reported_as_unrecovered() {
        let mut records = history(0, true);
        records.retain(|r| r.stream != Stream::Scheduled);
        // Present throughout, never confirming.
        for second in 1..=300 {
            records.push(op(Stream::Scheduled, -second, Outcome::Rejected));
        }
        let report = build(&records);

        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.stream == Some(Stream::Scheduled)),
            "a stream that never worked should not be reported as unrecovered, got {:?}",
            report.findings
        );
    }

    /// Recovery is measured and reported against the budget, never asserted on.
    #[test]
    fn a_slow_recovery_is_recorded_rather_than_a_finding() {
        let mut records = history(0, false);
        // First confirmation 200s after the heal, past the 120s budget.
        records.push(op(Stream::Durable, 380, Outcome::Confirmed));
        records.push(op(Stream::Scheduled, 380, Outcome::Confirmed));
        let report = build(&records);

        assert!(
            report.findings.is_empty(),
            "a slow recovery must not be a finding, got {:?}",
            report.findings
        );
        assert!(
            report.recovery.iter().all(|r| r.over_budget),
            "both streams should be recorded as over budget, got {:?}",
            report.recovery
        );
    }

    /// Without a fault window nothing can be placed either side of the outage,
    /// so the report carries counts and refuses to reach a verdict.
    #[test]
    fn without_a_fault_window_there_is_no_verdict() {
        let report = StorageFaultReport::build(
            &history(180, true),
            None,
            ENDPOINT,
            OutageExpectation::WholeWorkload {
                quiet_floor_percent: QUIET_FLOOR,
            },
            Duration::from_secs(120),
        );

        assert!(report.findings.is_empty());
        assert_eq!(report.share_of_baseline_percent, None);
        assert!(report.caught_in_flight.is_empty());
        assert!(
            report
                .cells
                .iter()
                .all(|c| c.window == Window::Unknown && c.share_of_baseline_percent.is_none()),
            "every cell should be unplaceable, got {:?}",
            report.cells
        );
    }

    /// The number that stops a small non-zero during-fault cell reading as
    /// residual service: it says the stream was silent for almost the whole
    /// window and answered only at the end.
    #[test]
    fn the_during_fault_cell_says_how_long_the_stream_stayed_quiet() {
        let mut records = history(0, true);
        // One confirmation, answered 170s into the 180s outage.
        records.push(op(Stream::Durable, 170, Outcome::Confirmed));
        let report = build(&records);

        let during = report.cell(Stream::Durable, Window::DuringFault).unwrap();
        assert_eq!(during.served, 1);
        // Silent from the cut until that one answer, which is nearly the whole
        // window — not the 10s that remained after it.
        assert_eq!(during.quiet_ms, Some(170_020));
    }

    /// A stream answering steadily right through the window is the shape the
    /// quiet number has to be able to tell apart from the one above. Both have
    /// a non-zero during-fault count; only the silence separates them.
    #[test]
    fn a_stream_answering_throughout_the_window_is_never_quiet_for_long() {
        let report = build(&history(180, true));

        let during = report.cell(Stream::Durable, Window::DuringFault).unwrap();
        assert_eq!(during.served, 180);
        assert!(
            during.quiet_ms.is_some_and(|ms| ms < 2_000),
            "a stream answering once a second should show no real silence, got {:?}",
            during.quiet_ms
        );
    }

    /// The regression the first S16 run turned up.
    ///
    /// The workload keeps offering work all through the outage, so anything
    /// derived from submission times is busy no matter what the platform is
    /// doing. Every one of these operations is offered during the fault and
    /// answered only after the heal: the fault window served nothing, and the
    /// report has to say so rather than crediting the window with work it did
    /// not do.
    #[test]
    fn work_offered_during_the_outage_and_answered_after_it_is_not_during_fault_service() {
        let mut records = history(0, true);
        for second in 0..120 {
            let mut record = op(Stream::Durable, second, Outcome::Confirmed);
            // Offered inside the outage, answered once the storage returned.
            record.completed_at = Some(t0() + TimeDelta::seconds(181));
            record.duration_ms = (181 - second) as u64 * 1_000;
            records.push(record);
        }
        let report = build(&records);

        let during = report.cell(Stream::Durable, Window::DuringFault).unwrap();
        assert_eq!(during.submitted, 120, "they were offered during the fault");
        assert_eq!(during.confirmed, 120, "and they did all eventually confirm");
        assert_eq!(
            during.served, 0,
            "but none of it was served during the fault"
        );
        assert_eq!(during.served_per_sec, 0.0);
        assert_eq!(
            during.quiet_ms,
            Some(180_000),
            "silent for the whole window"
        );
        assert!(
            report.findings.is_empty(),
            "a total outage must not be reported as one that never landed, got {:?}",
            report.findings
        );
    }

    /// The operations at risk. They were submitted before the cut and are
    /// attributed to the before-fault row, which is the last place anyone looks
    /// for the damage.
    #[test]
    fn operations_running_when_the_storage_went_away_are_reported_apart() {
        let mut records = history(0, true);
        // Submitted 30s before the cut, still running when it landed.
        for _ in 0..3 {
            records.push(stalled(Stream::Durable, -30));
        }
        let report = build(&records);

        let caught = report
            .caught_in_flight
            .iter()
            .find(|c| c.stream == Stream::Durable)
            .expect("expected a caught-in-flight row for the durable stream");
        assert_eq!(caught.operations, 3);
        assert_eq!(caught.indeterminate, 3);
        assert_eq!(caught.attempts_timed_out, 6);
        assert_eq!(caught.max_attempts, 2);
        // 245s from 30s before the cut lands 35s past the 180s heal.
        assert_eq!(caught.outlived_the_fault, 3);
    }

    /// An operator reading the fault window wants the dominant failure mode
    /// first, and wants to know whether the platform refused the work or lost
    /// track of it.
    #[test]
    fn fault_window_failures_are_grouped_by_class_commonest_first() {
        let mut records = history(0, true);
        for i in 0..5 {
            records.push(stalled(Stream::Durable, i));
        }
        let mut rejected = op(Stream::Durable, 10, Outcome::Rejected);
        rejected.error_class = Some(ErrorClass::Response);
        rejected.error = Some("agent not found".to_string());
        records.push(rejected);
        let report = build(&records);

        assert_eq!(report.fault_window_errors.len(), 2);
        assert_eq!(report.fault_window_errors[0].class, ErrorClass::Transport);
        assert_eq!(report.fault_window_errors[0].operations, 5);
        assert_eq!(
            report.fault_window_errors[0].example.as_deref(),
            Some("attempt timed out after 120s")
        );
        assert_eq!(report.fault_window_errors[1].class, ErrorClass::Response);
        assert_eq!(report.fault_window_errors[1].operations, 1);
    }

    /// Findings are hoisted for a human; the share line is context and belongs
    /// with the notes, not with the things to act on.
    #[test]
    fn the_share_line_is_a_note_and_the_findings_are_attention() {
        let report = build(&history(180, true));

        assert!(
            report
                .attention_lines()
                .iter()
                .any(|l| l.contains("outage-not-observed"))
        );
        assert!(
            report
                .note_lines()
                .iter()
                .any(|l| l.contains("of its baseline throughput"))
        );
    }
}
