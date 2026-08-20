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

//! Reduces an [operation history](super::history) into the numbers an operator
//! reads (GOL-363).
//!
//! This suite deliberately has no binary correctness oracle, following the
//! precedent density set: the driver reports, the operator decides. The driver
//! fails a scenario only on unambiguous breakage — a stream where nothing ever
//! succeeded, or a fault signal that never arrived. Everything a human should
//! weigh is reported as `observed` next to `expected`, so a mismatch is visible
//! without doing arithmetic on the way past.
//!
//! # Read-back: how double execution is detected
//!
//! Every durable operation is `Counter.increment` on a known agent under a
//! deterministic idempotency key. The counter is durable agent state, so once
//! the platform is quiescent its value counts how many increments actually took
//! effect. Against that, the driver knows what it submitted, split three ways
//! ([`Outcome`]): `Confirmed`, `Indeterminate`, `Rejected`.
//!
//! ```text
//! expectedMin = confirmed                  // every confirmed op took effect once
//! expectedMax = confirmed + indeterminate  // every in-doubt op landed too
//! observed    = Counter::count(agent)
//! ```
//!
//! - `observed > expectedMax` → **duplicate execution**: something ran twice
//!   under one key.
//! - `observed < expectedMin` → **lost accepted work**: an acknowledged
//!   operation never took effect.
//! - otherwise → healthy, with `indeterminate` reported as the width of the
//!   remaining doubt. A width of `0` makes the check exact.
//!
//! Both verdicts are *reported*, not asserted. What makes them actionable is
//! that read-back is **per agent**: a duplicate localises to one agent and the
//! handful of suspect keys on it, which the workflow turns into ready-made trace
//! queries. One global counter would have produced a haystack instead.

use crate::chaos::history::{Outcome, Phase, Stream};
use crate::chaos::ownership::OwnershipSample;
use crate::chaos::probe::KeyProbe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

use super::history::OperationRecord;

/// Verdict for one read-back comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadbackVerdict {
    /// `observed` sits within `[expectedMin, expectedMax]`.
    Consistent,
    /// `observed` exceeds `expectedMax`: more executions happened than the
    /// driver could possibly have asked for.
    DuplicateExecution,
    /// `observed` is below `expectedMin`: work the driver was told had
    /// succeeded left no trace.
    LostWork,
    /// The read-back itself could not be performed (the agent could not be
    /// read). Reported rather than assumed either way.
    Unavailable,
}

impl ReadbackVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            ReadbackVerdict::Consistent => "consistent",
            ReadbackVerdict::DuplicateExecution => "duplicate-execution",
            ReadbackVerdict::LostWork => "lost-work",
            ReadbackVerdict::Unavailable => "unavailable",
        }
    }

    /// Whether this verdict is the kind an operator should stop and look at.
    pub fn needs_attention(self) -> bool {
        matches!(
            self,
            ReadbackVerdict::DuplicateExecution | ReadbackVerdict::LostWork
        )
    }
}

impl std::fmt::Display for ReadbackVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One agent's read-back comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentReadback {
    pub stream: Stream,
    pub agent: String,
    /// Operations the driver knows took effect at least once.
    pub confirmed: u64,
    /// Operations the driver cannot tell either way — the width of the doubt.
    pub indeterminate: u64,
    /// Operations the platform definitely refused. Excluded from both bounds.
    pub rejected: u64,
    pub expected_min: u64,
    pub expected_max: u64,
    /// What the agent's durable state actually says, or `None` when it could
    /// not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<u64>,
    pub verdict: ReadbackVerdict,
    /// Why the read failed, when `observed` is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_error: Option<String>,
    /// Idempotency keys worth pasting into a trace query for this agent: the
    /// retried and the in-doubt ones. Short by construction — a shard-manager
    /// kill produces a handful, not thousands.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub suspect_keys: Vec<String>,
    /// Reservations the platform refused, for the quota stream only.
    ///
    /// Read from the agent rather than inferred: a refused reservation still
    /// returns successfully to the caller, so it is invisible in the operation
    /// outcomes. Non-zero during a partition is the cost of a lease the
    /// executor could no longer renew.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused_reservations: Option<u64>,
    /// Keys where a retry came back with a *higher* counter value than the
    /// first attempt. That is direct per-key proof of double execution and needs
    /// no read-back arithmetic at all.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub proven_double_execution_keys: Vec<String>,
}

impl AgentReadback {
    /// Builds the comparison for one agent from its records and the value read
    /// back from its durable state.
    pub fn evaluate(
        stream: Stream,
        agent: impl Into<String>,
        records: &[&OperationRecord],
        observed: Result<u64, String>,
    ) -> Self {
        let confirmed = count(records, Outcome::Confirmed);
        let indeterminate = count(records, Outcome::Indeterminate);
        let rejected = count(records, Outcome::Rejected);

        let expected_min = confirmed;
        let expected_max = confirmed + indeterminate;

        let (observed, read_error, verdict) = match observed {
            Ok(value) if value > expected_max => {
                (Some(value), None, ReadbackVerdict::DuplicateExecution)
            }
            Ok(value) if value < expected_min => (Some(value), None, ReadbackVerdict::LostWork),
            Ok(value) => (Some(value), None, ReadbackVerdict::Consistent),
            Err(e) => (None, Some(e), ReadbackVerdict::Unavailable),
        };

        let mut suspect_keys: Vec<String> = records
            .iter()
            .filter(|r| r.is_suspect())
            .map(|r| r.idempotency_key.clone())
            .collect();
        suspect_keys.sort();
        suspect_keys.dedup();

        let mut proven_double_execution_keys: Vec<String> = records
            .iter()
            .filter(|r| r.shows_double_execution())
            .map(|r| r.idempotency_key.clone())
            .collect();
        proven_double_execution_keys.sort();
        proven_double_execution_keys.dedup();

        Self {
            stream,
            agent: agent.into(),
            confirmed,
            indeterminate,
            rejected,
            expected_min,
            expected_max,
            observed,
            verdict,
            read_error,
            suspect_keys,
            proven_double_execution_keys,
            refused_reservations: None,
        }
    }
}

fn count(records: &[&OperationRecord], outcome: Outcome) -> u64 {
    records.iter().filter(|r| r.outcome == outcome).count() as u64
}

/// Latency distribution over a set of operations, in milliseconds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyStats {
    pub count: u64,
    pub p50_ms: u64,
    pub p90_ms: u64,
    pub p99_ms: u64,
    pub max_ms: u64,
}

impl LatencyStats {
    pub fn from_durations(mut samples: Vec<u64>) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        samples.sort_unstable();
        Self {
            count: samples.len() as u64,
            p50_ms: percentile(&samples, 50.0),
            p90_ms: percentile(&samples, 90.0),
            p99_ms: percentile(&samples, 99.0),
            max_ms: *samples.last().unwrap(),
        }
    }
}

/// Nearest-rank percentile over a sorted, non-empty slice.
fn percentile(sorted: &[u64], pct: f64) -> u64 {
    let rank = (pct / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

/// Per-stream, per-phase operation counts and latencies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseStats {
    pub stream: Stream,
    pub phase: Phase,
    pub submitted: u64,
    pub confirmed: u64,
    pub indeterminate: u64,
    pub rejected: u64,
    /// Submitted but never resolved either way — the driver stopped before it
    /// heard back. Non-zero at the end of recovery is worth investigating.
    pub outstanding: u64,
    /// Operations that needed the bounded same-key retry.
    pub retried: u64,
    pub latency: LatencyStats,
}

/// Where a stream stood after the fault: how long until it served a request
/// again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryStats {
    pub stream: Stream,
    /// Milliseconds from the fault-injection timestamp to the first confirmed
    /// operation after it. `None` when the stream never succeeded again, which
    /// is itself the finding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_first_success_ms: Option<u64>,
    /// Whether the stream ever confirmed an operation after the fault.
    pub recovered: bool,
}

/// A routing-table observation at a phase boundary.
///
/// Reported, not asserted: which executor owns which shards before and after a
/// shard-manager restart is what S12 exists to show an operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingSnapshot {
    /// Label for when it was taken, e.g. `before-fault`, `after-recovery`.
    pub at: String,
    pub taken_at: chrono::DateTime<chrono::Utc>,
    /// Executor endpoints and the shard count each held. `None` when the
    /// shard-manager could not be reached, which is expected mid-fault and is
    /// recorded rather than treated as an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shards_per_executor: Option<BTreeMap<String, usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// Which of the two exactly-once guarantees a key broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExactlyOnceViolation {
    /// The platform accepted the operation, but after recovery the key has no
    /// final result: asked again under its own idempotency key, the platform
    /// could not produce one.
    MissingFinalResult,
    /// The key has more than one distinct successful completion value. Since
    /// `Counter.sleep_and_increment` returns its post-increment count, two
    /// different values under one key is direct proof the work ran twice.
    MultipleDistinctCompletions,
}

impl ExactlyOnceViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            ExactlyOnceViolation::MissingFinalResult => "missing-final-result",
            ExactlyOnceViolation::MultipleDistinctCompletions => "multiple-distinct-completions",
        }
    }
}

impl std::fmt::Display for ExactlyOnceViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One key that broke a guarantee, with enough detail to go straight to a trace
/// query for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactlyOnceFinding {
    pub violation: ExactlyOnceViolation,
    pub idempotency_key: String,
    pub agent: String,
    /// Plain-language statement of what was observed, so a job-log reader does
    /// not have to reconstruct it from the numbers.
    pub detail: String,
}

/// The exactly-once account for a pinned run (GOL-366).
///
/// Unlike the read-back verdicts, the findings here are **assertions**, not
/// observations for an operator to weigh. That difference is earned: the pinned
/// population is bounded and every key was probed individually under its own
/// idempotency key, so there is no aggregate arithmetic and no band of doubt to
/// interpret. A finding is a fact about one key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExactlyOnceReport {
    /// Keys the probe pass covered.
    pub keys_checked: u64,
    /// Keys that had no probe result at all — a probe task that died rather
    /// than one that failed, which is the only way a key escapes the account.
    /// Counted rather than ignored: a verdict computed over a silently smaller
    /// population is a weaker claim than it appears to be.
    pub keys_unprobed: u64,
    /// Keys whose probe failed in a way that does not answer the question.
    ///
    /// A probe that dies at transport level says nothing about whether the
    /// platform has the result — only that the driver could not ask. Treating
    /// that as "accepted work has no final result" would report a connection
    /// problem as a correctness defect, which is the exact mistake the rest of
    /// this suite is built to avoid. They are counted here and named in the
    /// report so a clean verdict over a large number of them can be read for
    /// what it is: a weaker claim.
    pub keys_inconclusive: u64,
    /// Keys that had a final result after recovery.
    pub keys_with_final_result: u64,
    /// Keys the driver never got a result for, but which the platform produced
    /// one for when asked again. Not a defect — this is recovery working — but
    /// worth surfacing, because it is the population the fault actually
    /// disturbed.
    pub keys_recovered_by_probe: u64,
    /// Keys the driver was definitely refused, which are excluded from the
    /// missing-result check: nothing was accepted, so nothing is owed.
    pub keys_rejected: u64,
    /// Net counter movement across the probe pass, per agent. A probe of a key
    /// that already ran replays a stored result and moves nothing; this is
    /// therefore how many keys had never run at all, and it bounds how much of
    /// the pass was fresh execution rather than replay.
    pub probe_executed_per_agent: BTreeMap<String, i64>,
    /// Total of the above. Zero means every probed key already had a result
    /// stored, which is the strongest form of the guarantee holding.
    pub probe_executed_total: i64,
    pub findings: Vec<ExactlyOnceFinding>,
}

impl ExactlyOnceReport {
    /// Builds the account from the pinned records, the probe results, and the
    /// counter read-backs taken either side of the probe pass.
    /// `stream` is the population being accounted for, and it has to match the
    /// stream the probes were taken from. Passing it explicitly rather than
    /// inferring it keeps a mismatch loud: a report built over the wrong stream
    /// finds no records, checks nothing, and reports a flawless result.
    pub fn build(
        records: &[OperationRecord],
        probes: &[KeyProbe],
        stream: Stream,
        before_probe: &BTreeMap<String, u64>,
        after_probe: &BTreeMap<String, u64>,
    ) -> Self {
        let by_key: BTreeMap<&str, &KeyProbe> = probes
            .iter()
            .map(|p| (p.idempotency_key.as_str(), p))
            .collect();

        let mut report = ExactlyOnceReport::default();
        for record in records.iter().filter(|r| r.stream == stream) {
            let Some(probe) = by_key.get(record.idempotency_key.as_str()) else {
                report.keys_unprobed += 1;
                continue;
            };
            report.keys_checked += 1;

            if record.outcome == Outcome::Rejected {
                // Refused outright, so the platform owes nothing for this key.
                // Probing it anyway is still useful — it is how the refusal is
                // confirmed to have left no trace — but it cannot fail the run.
                report.keys_rejected += 1;
                continue;
            }

            match probe.final_value {
                None => {
                    // Only a *definite* answer is evidence. A probe that was
                    // refused outright asked the question and was told there is
                    // nothing; a probe that died at transport level never got
                    // to ask, and the platform may hold the result perfectly
                    // well. Failing a run on the second would be reporting a
                    // connection problem as a correctness defect.
                    let definitive = probe
                        .error_class
                        .is_some_and(|class| class.is_definite_rejection());
                    if !definitive {
                        report.keys_inconclusive += 1;
                        continue;
                    }
                    report.findings.push(ExactlyOnceFinding {
                        violation: ExactlyOnceViolation::MissingFinalResult,
                        idempotency_key: record.idempotency_key.clone(),
                        agent: record.agent.clone(),
                        detail: format!(
                            "accepted as {} but the platform refused to produce a final result                              for this key after recovery: {}",
                            record.outcome,
                            probe.error.as_deref().unwrap_or("probe returned no value")
                        ),
                    });
                }
                Some(final_value) => {
                    report.keys_with_final_result += 1;
                    if !record.had_successful_attempt() {
                        report.keys_recovered_by_probe += 1;
                    }

                    let mut values = record.distinct_successful_values();
                    if !values.contains(&final_value) {
                        values.push(final_value);
                    }
                    if values.len() > 1 {
                        values.sort_unstable();
                        report.findings.push(ExactlyOnceFinding {
                            violation: ExactlyOnceViolation::MultipleDistinctCompletions,
                            idempotency_key: record.idempotency_key.clone(),
                            agent: record.agent.clone(),
                            detail: format!(
                                "one key, {} distinct successful counter values {values:?} — the work ran more than once",
                                values.len()
                            ),
                        });
                    }
                }
            }
        }

        for (agent, after) in after_probe {
            let before = before_probe.get(agent).copied().unwrap_or(0);
            let delta = *after as i64 - before as i64;
            if delta != 0 {
                report.probe_executed_per_agent.insert(agent.clone(), delta);
            }
            report.probe_executed_total += delta;
        }

        report
    }

    /// The two conditions that fail a pinned scenario outright.
    pub fn has_violations(&self) -> bool {
        !self.findings.is_empty()
    }
}

/// Everything the driver reports for a scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChaosSummary {
    pub total_operations: u64,
    pub phases: Vec<PhaseStats>,
    pub recovery: Vec<RecoveryStats>,
    /// Read-back per agent, for the streams that keep durable state. Ephemeral
    /// agents are absent by design — see [`streams_without_readback`].
    pub readback: Vec<AgentReadback>,
    /// Named explicitly so a reader never wonders whether a stream was skipped
    /// or simply had nothing to report.
    pub streams_without_readback: Vec<Stream>,
    pub routing_snapshots: Vec<RoutingSnapshot>,
    /// The exactly-once account, for scenarios that run a pinned population.
    /// Absent for scenarios that do not, rather than an empty report that would
    /// read as "checked, nothing found".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exactly_once: Option<ExactlyOnceReport>,
    /// Shard-ownership samples, in the order they were taken. Empty for
    /// scenarios that do not sample executor assignments.
    ///
    /// Every sample is kept, not just the judged one: what an operator needs in
    /// order to read a violation is the *before* and *during* pictures next to
    /// it, so keeping only the verdict would throw away the context that makes
    /// it interpretable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ownership: Vec<OwnershipSample>,
    /// The read-back verdicts that need a human, hoisted for scanning.
    pub attention: Vec<String>,
}

impl ChaosSummary {
    /// Reduces a history plus the read-back results into the reported summary.
    ///
    /// `fault_injected_at` anchors the recovery measurement; when it is `None`
    /// (an abort before injection) recovery stats are omitted rather than
    /// invented.
    pub fn build(
        records: &[OperationRecord],
        readback: Vec<AgentReadback>,
        routing_snapshots: Vec<RoutingSnapshot>,
        fault_injected_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Self {
        let mut phases = Vec::new();
        for stream in Stream::ALL {
            for phase in Phase::ALL {
                let scoped: Vec<&OperationRecord> = records
                    .iter()
                    .filter(|r| r.stream == stream && r.phase == phase)
                    .collect();
                if scoped.is_empty() {
                    continue;
                }
                phases.push(PhaseStats {
                    stream,
                    phase,
                    submitted: scoped.len() as u64,
                    confirmed: count(&scoped, Outcome::Confirmed),
                    indeterminate: count(&scoped, Outcome::Indeterminate),
                    rejected: count(&scoped, Outcome::Rejected),
                    outstanding: scoped.iter().filter(|r| r.completed_at.is_none()).count() as u64,
                    retried: scoped.iter().filter(|r| r.was_retried()).count() as u64,
                    latency: LatencyStats::from_durations(
                        scoped
                            .iter()
                            .filter(|r| r.outcome == Outcome::Confirmed)
                            .map(|r| r.duration_ms)
                            .collect(),
                    ),
                });
            }
        }

        let recovery = fault_injected_at
            .map(|injected| {
                Stream::ALL
                    .into_iter()
                    .filter(|stream| records.iter().any(|r| r.stream == *stream))
                    .map(|stream| {
                        let first = records
                            .iter()
                            .filter(|r| {
                                r.stream == stream
                                    && r.outcome == Outcome::Confirmed
                                    && r.completed_at.is_some_and(|at| at >= injected)
                            })
                            .filter_map(|r| r.completed_at)
                            .min();
                        RecoveryStats {
                            stream,
                            time_to_first_success_ms: first
                                .map(|at| (at - injected).num_milliseconds().max(0) as u64),
                            recovered: first.is_some(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let attention = readback
            .iter()
            .filter(|r| r.verdict.needs_attention())
            .map(|r| {
                // Read by an operator in a job log, so an unread agent says
                // "unreadable" rather than leaking `Option` debug formatting.
                let observed = r
                    .observed
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unreadable".to_string());
                // `37..=37` is Rust's inclusive-range syntax, not something an
                // operator should have to decode. When nothing was in doubt the
                // bounds collapse, so say the single number instead of implying
                // a spread that is not there.
                let expected = if r.expected_min == r.expected_max {
                    format!("exactly {}", r.expected_min)
                } else {
                    format!("{} to {}", r.expected_min, r.expected_max)
                };
                format!(
                    "{} agent {}: {} (observed {observed}, expected {expected})",
                    r.stream, r.agent, r.verdict
                )
            })
            .collect();

        Self {
            total_operations: records.len() as u64,
            phases,
            recovery,
            readback,
            streams_without_readback: Stream::ALL
                .into_iter()
                .filter(|s| !s.has_readback())
                .collect(),
            routing_snapshots,
            exactly_once: None,
            ownership: Vec::new(),
            attention,
        }
    }

    /// Attaches the exactly-once account and hoists its findings into
    /// [`Self::attention`], so a reader scanning the top of a report sees them
    /// next to the read-back verdicts rather than further down.
    pub fn with_exactly_once(mut self, report: ExactlyOnceReport) -> Self {
        for finding in &report.findings {
            self.attention.push(format!(
                "{}: key {} on agent {} — {}",
                finding.violation, finding.idempotency_key, finding.agent, finding.detail
            ));
        }
        self.exactly_once = Some(report);
        self
    }

    /// Attaches the shard-ownership samples and hoists their findings into
    /// [`Self::attention`].
    ///
    /// Findings from *every* sample surface, not only the judged one: a
    /// mid-fault overlap that healed before the settle sample is not a run
    /// failure, but it is absolutely something an operator wants to know
    /// happened.
    pub fn with_ownership(mut self, reports: Vec<OwnershipSample>) -> Self {
        for report in &reports {
            self.attention.extend(report.attention_lines());
        }
        self.ownership = reports;
        self
    }
}

/// Why a scenario stopped.
///
/// Only the non-`Completed` variants fail the run. They are deliberately few:
/// this suite reports rather than judges, so the bar for failing outright is
/// "the run produced nothing worth interpreting".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminationReason {
    /// The scenario ran every phase and produced a full report.
    Completed,
    /// A workload stream never had a single confirmed operation. Nothing about
    /// resilience can be read from a run where the workload never worked.
    StreamNeverSucceeded { stream: String },
    /// A fault signal never arrived within its bound.
    SignalTimeout { file: String },
    /// The platform could not be reached at all.
    PlatformUnreachable { detail: String },
    /// The run was cancelled; artifacts are whatever had accumulated.
    Aborted { detail: String },
    /// A pinned scenario's exactly-once account found at least one key that
    /// broke a guarantee. Unlike the read-back verdicts this *is* asserted —
    /// see [`ExactlyOnceReport`] for why the pinned population earns that.
    ExactlyOnceViolated { findings: u64, first: String },
    /// The fault could not be aimed at a verified target. Better to spend the
    /// maintenance window fixing that than to kill an unrelated pod and report
    /// on it as though it were the right one.
    FaultTargetUnverified { detail: String },
    /// After the settling window, two or more executors still believed they
    /// owned the same shard. Asserted rather than reported: an agent with two
    /// owners is an agent whose state can fork, and there is no instant at
    /// which that is legitimate.
    ShardOwnershipViolated { findings: u64, first: String },
    /// An agent's durable state did not survive a component update. Asserted
    /// because an update is supposed to change what an agent runs and nothing
    /// about what it remembers — state that moved is the one outcome an update
    /// may never produce.
    UpdateStateInconsistent { agent: String, detail: String },
    /// An agent was still running the old build after recovery. Asserted on the
    /// agent's own answer rather than on component metadata: metadata says what
    /// the platform believes, and the question is what the code is.
    ///
    /// An agent that could not be read at all does not reach here. That is
    /// reported, because an unreadable agent says nothing either way.
    UpdateNotApplied {
        agent: String,
        observed: Option<u32>,
        expected: u32,
    },
}

impl TerminationReason {
    pub fn is_failure(&self) -> bool {
        !matches!(self, TerminationReason::Completed)
    }
}

/// The one hard-fail check over the reduced summary: a stream that never worked
/// at all. Returns the first offending stream, if any.
///
/// Deliberately narrow. A stream with *some* successes — even a mostly-failing
/// one — is reported and left to the operator, because "how much degradation is
/// acceptable during a shard-manager restart" is a judgement call, not a
/// constant.
pub fn stream_that_never_succeeded(summary: &ChaosSummary) -> Option<Stream> {
    Stream::ALL.into_iter().find(|stream| {
        let stats: Vec<&PhaseStats> = summary
            .phases
            .iter()
            .filter(|p| p.stream == *stream)
            .collect();
        !stats.is_empty() && stats.iter().all(|p| p.confirmed == 0)
    })
}

/// Convenience for callers that measure with [`Duration`].
pub fn duration_ms(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::OperationRecord;
    use chrono::{TimeZone, Utc};
    use test_r::test;

    fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
        Utc.timestamp_opt(1_800_000_000 + secs, 0).unwrap()
    }

    fn op(op_id: u64, stream: Stream, phase: Phase, outcome: Outcome) -> OperationRecord {
        OperationRecord {
            op_id,
            stream,
            phase,
            agent: "counter-0".to_string(),
            method: "increment".to_string(),
            idempotency_key: format!("chaos-s12-{stream}-counter-0-{op_id}"),
            submitted_at: at(op_id as i64),
            completed_at: Some(at(op_id as i64)),
            attempts: 1,
            outcome,
            duration_ms: 10,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: Vec::new(),
        }
    }

    fn readback_for(confirmed: u64, indeterminate: u64, observed: u64) -> AgentReadback {
        let mut records = Vec::new();
        for i in 0..confirmed {
            records.push(op(i, Stream::Durable, Phase::Fault, Outcome::Confirmed));
        }
        for i in 0..indeterminate {
            records.push(op(
                confirmed + i,
                Stream::Durable,
                Phase::Fault,
                Outcome::Indeterminate,
            ));
        }
        let refs: Vec<&OperationRecord> = records.iter().collect();
        AgentReadback::evaluate(Stream::Durable, "counter-0", &refs, Ok(observed))
    }

    // ── Read-back case table ────────────────────────────────────────────────
    // The whole detector lives in these bounds, so each edge gets its own test.

    #[test]
    fn readback_is_exact_when_nothing_is_in_doubt() {
        let r = readback_for(10, 0, 10);
        assert_eq!(r.expected_min, 10);
        assert_eq!(
            r.expected_max, 10,
            "zero doubt collapses the range to a point"
        );
        assert_eq!(r.verdict, ReadbackVerdict::Consistent);
    }

    #[test]
    fn readback_flags_duplicate_execution_above_the_upper_bound() {
        let r = readback_for(10, 2, 13);
        assert_eq!(r.expected_max, 12);
        assert_eq!(r.verdict, ReadbackVerdict::DuplicateExecution);
        assert!(r.verdict.needs_attention());
    }

    #[test]
    fn readback_flags_lost_work_below_the_lower_bound() {
        let r = readback_for(10, 2, 9);
        assert_eq!(r.expected_min, 10);
        assert_eq!(r.verdict, ReadbackVerdict::LostWork);
        assert!(r.verdict.needs_attention());
    }

    /// Inside the doubt band nothing can be concluded, and pretending otherwise
    /// is exactly the lie the three-way outcome split exists to prevent.
    #[test]
    fn readback_inside_the_indeterminate_band_is_consistent() {
        for observed in 10..=12 {
            let r = readback_for(10, 2, observed);
            assert_eq!(
                r.verdict,
                ReadbackVerdict::Consistent,
                "observed {observed} sits inside 10..=12"
            );
        }
    }

    /// When every operation is in doubt the band is at its widest and only a
    /// value outside `0..=n` can say anything.
    #[test]
    fn readback_with_everything_indeterminate_has_the_widest_band() {
        let r = readback_for(0, 5, 5);
        assert_eq!(r.expected_min, 0);
        assert_eq!(r.expected_max, 5);
        assert_eq!(r.verdict, ReadbackVerdict::Consistent);

        let r = readback_for(0, 5, 6);
        assert_eq!(r.verdict, ReadbackVerdict::DuplicateExecution);
    }

    #[test]
    fn rejected_operations_count_towards_neither_bound() {
        let records = [
            op(0, Stream::Durable, Phase::Fault, Outcome::Confirmed),
            op(1, Stream::Durable, Phase::Fault, Outcome::Rejected),
            op(2, Stream::Durable, Phase::Fault, Outcome::Rejected),
        ];
        let refs: Vec<&OperationRecord> = records.iter().collect();
        let r = AgentReadback::evaluate(Stream::Durable, "counter-0", &refs, Ok(1));
        assert_eq!(r.rejected, 2);
        assert_eq!(r.expected_min, 1);
        assert_eq!(r.expected_max, 1);
        assert_eq!(r.verdict, ReadbackVerdict::Consistent);
    }

    #[test]
    fn unreadable_agent_is_reported_as_unavailable_not_guessed() {
        let records = [op(0, Stream::Durable, Phase::Fault, Outcome::Confirmed)];
        let refs: Vec<&OperationRecord> = records.iter().collect();
        let r = AgentReadback::evaluate(
            Stream::Durable,
            "counter-0",
            &refs,
            Err("agent unreachable".to_string()),
        );
        assert_eq!(r.verdict, ReadbackVerdict::Unavailable);
        assert!(r.observed.is_none());
        assert_eq!(r.read_error.as_deref(), Some("agent unreachable"));
        assert!(!r.verdict.needs_attention(), "unreadable is not a finding");
    }

    #[test]
    fn suspect_keys_collect_retried_and_indeterminate_operations() {
        let mut retried = op(0, Stream::Durable, Phase::Fault, Outcome::Confirmed);
        retried.attempts = 2;
        retried.first_attempt_value = Some(3);
        retried.returned_value = Some(4);
        let records = [
            retried,
            op(1, Stream::Durable, Phase::Fault, Outcome::Indeterminate),
            op(2, Stream::Durable, Phase::Fault, Outcome::Confirmed),
        ];
        let refs: Vec<&OperationRecord> = records.iter().collect();
        let r = AgentReadback::evaluate(Stream::Durable, "counter-0", &refs, Ok(2));

        assert_eq!(
            r.suspect_keys.len(),
            2,
            "the retried and the in-doubt op are the ones worth tracing"
        );
        assert_eq!(
            r.proven_double_execution_keys.len(),
            1,
            "the retry that came back higher is proven, not merely suspect"
        );
    }

    // ── Summary reduction ───────────────────────────────────────────────────

    #[test]
    fn phase_stats_are_split_per_stream_and_phase() {
        let records = vec![
            op(0, Stream::Durable, Phase::Baseline, Outcome::Confirmed),
            op(1, Stream::Durable, Phase::Fault, Outcome::Indeterminate),
            op(2, Stream::Promise, Phase::Fault, Outcome::Confirmed),
        ];
        let summary = ChaosSummary::build(&records, Vec::new(), Vec::new(), None);

        assert_eq!(summary.total_operations, 3);
        assert_eq!(
            summary.phases.len(),
            3,
            "empty stream/phase cells are omitted"
        );
        let durable_fault = summary
            .phases
            .iter()
            .find(|p| p.stream == Stream::Durable && p.phase == Phase::Fault)
            .unwrap();
        assert_eq!(durable_fault.indeterminate, 1);
        assert_eq!(durable_fault.confirmed, 0);
    }

    #[test]
    fn recovery_measures_first_success_after_injection() {
        let mut before = op(0, Stream::Durable, Phase::Baseline, Outcome::Confirmed);
        before.completed_at = Some(at(10));
        let mut after = op(1, Stream::Durable, Phase::Recovery, Outcome::Confirmed);
        after.completed_at = Some(at(35));

        let summary = ChaosSummary::build(&[before, after], Vec::new(), Vec::new(), Some(at(30)));
        let durable = summary
            .recovery
            .iter()
            .find(|r| r.stream == Stream::Durable)
            .unwrap();
        assert!(durable.recovered);
        assert_eq!(durable.time_to_first_success_ms, Some(5_000));
    }

    #[test]
    fn a_stream_that_never_recovers_is_reported_as_such() {
        let mut before = op(0, Stream::Durable, Phase::Baseline, Outcome::Confirmed);
        before.completed_at = Some(at(10));
        let mut during = op(1, Stream::Durable, Phase::Fault, Outcome::Indeterminate);
        during.completed_at = Some(at(35));

        let summary = ChaosSummary::build(&[before, during], Vec::new(), Vec::new(), Some(at(30)));
        let durable = summary
            .recovery
            .iter()
            .find(|r| r.stream == Stream::Durable)
            .unwrap();
        assert!(!durable.recovered);
        assert!(durable.time_to_first_success_ms.is_none());
    }

    /// A reader must never have to wonder whether a stream was skipped or just
    /// had nothing to say.
    #[test]
    fn streams_without_readback_are_named_rather_than_omitted() {
        let summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), None);
        assert_eq!(
            summary.streams_without_readback,
            vec![Stream::Ephemeral, Stream::Promise]
        );
    }

    /// These strings are read by an operator in a job log, so they must not
    /// leak Rust `Option` formatting.
    #[test]
    fn attention_lines_render_observed_values_for_humans() {
        let summary = ChaosSummary::build(&[], vec![readback_for(10, 0, 12)], Vec::new(), None);
        let line = &summary.attention[0];
        assert!(line.contains("observed 12"), "got {line}");
        assert!(
            !line.contains("Some("),
            "Option debug formatting leaked: {line}"
        );
    }

    #[test]
    fn attention_hoists_only_verdicts_that_need_a_human() {
        let readback = vec![
            readback_for(10, 0, 10),
            readback_for(10, 0, 12),
            readback_for(10, 0, 8),
        ];
        let summary = ChaosSummary::build(&[], readback, Vec::new(), None);
        assert_eq!(
            summary.attention.len(),
            2,
            "the consistent one must not be hoisted"
        );
    }

    // ── Hard-fail boundary ──────────────────────────────────────────────────

    /// The bar for failing outright: a stream that never worked at all.
    #[test]
    fn a_stream_with_no_successes_at_all_is_a_hard_failure() {
        let records = vec![
            op(0, Stream::Durable, Phase::Baseline, Outcome::Confirmed),
            op(1, Stream::Promise, Phase::Baseline, Outcome::Rejected),
            op(2, Stream::Promise, Phase::Fault, Outcome::Indeterminate),
        ];
        let summary = ChaosSummary::build(&records, Vec::new(), Vec::new(), None);
        assert_eq!(stream_that_never_succeeded(&summary), Some(Stream::Promise));
    }

    /// Heavy but partial degradation is reported, never failed — how much
    /// degradation a shard-manager restart may cause is the operator's call.
    #[test]
    fn a_mostly_failing_stream_is_reported_not_failed() {
        let mut records = vec![op(0, Stream::Durable, Phase::Baseline, Outcome::Confirmed)];
        for i in 1..50 {
            records.push(op(i, Stream::Durable, Phase::Fault, Outcome::Indeterminate));
        }
        let summary = ChaosSummary::build(&records, Vec::new(), Vec::new(), None);
        assert_eq!(stream_that_never_succeeded(&summary), None);
    }

    /// A duplicate-execution verdict is a finding for the operator, not a
    /// driver-level failure.
    #[test]
    fn duplicate_execution_does_not_make_the_run_fail() {
        let records = [op(0, Stream::Durable, Phase::Fault, Outcome::Confirmed)];
        let summary =
            ChaosSummary::build(&records, vec![readback_for(10, 0, 12)], Vec::new(), None);
        assert_eq!(stream_that_never_succeeded(&summary), None);
        assert_eq!(summary.attention.len(), 1);
    }

    #[test]
    fn termination_reasons_other_than_completed_are_failures() {
        assert!(!TerminationReason::Completed.is_failure());
        assert!(
            TerminationReason::SignalTimeout {
                file: "fault-injected.json".to_string()
            }
            .is_failure()
        );
    }

    #[test]
    fn latency_percentiles_use_nearest_rank() {
        let stats = LatencyStats::from_durations((1..=100).collect());
        assert_eq!(stats.count, 100);
        assert_eq!(stats.p50_ms, 50);
        assert_eq!(stats.p90_ms, 90);
        assert_eq!(stats.p99_ms, 99);
        assert_eq!(stats.max_ms, 100);
    }

    #[test]
    fn latency_of_no_samples_is_zeroed_rather_than_panicking() {
        let stats = LatencyStats::from_durations(Vec::new());
        assert_eq!(stats.count, 0);
        assert_eq!(stats.max_ms, 0);
    }
}
