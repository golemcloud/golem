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

//! Operation history: what the driver asked the platform to do, and what came
//! back (GOL-363).
//!
//! This is the raw material every other analysis reduces from, and the reason it
//! is persisted rather than summarised in place: a later ticket adding real
//! correctness oracles (shard overlap, exactly-once, recovered-state-matches)
//! needs exactly this and nothing more, so it can be written as pure analysis
//! over archived runs with no re-instrumentation and no second maintenance
//! window.
//!
//! The one thing the history must get right is honesty about *doubt*. When an
//! invocation fails at the transport level the platform may or may not have
//! executed it, and a shard-manager kill produces precisely those. Recording
//! them as "failed" would understate what the platform did; recording them as
//! "done" would overstate it. They get their own outcome, and the read-back
//! carries the doubt through as a range instead of a number.

use crate::chaos::errors::ErrorClass;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Bumped when the on-disk shape changes incompatibly, so an archived history
/// can be read years later by tooling that knows which shape it is looking at.
pub const HISTORY_SCHEMA_VERSION: u32 = 2;

/// Which workload stream an operation belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stream {
    /// Durable `Counter.increment` — the stream whose state survives a restart,
    /// and therefore the only one that can be read back exactly.
    Durable,
    /// Ephemeral `EphemeralCounter.increment` — no durable state, so no
    /// read-back is possible for it.
    Ephemeral,
    /// Scheduled `ScheduleEmitter.schedule_poll_at` → `ScheduleCounter.poll`.
    Scheduled,
    /// Promise create/complete/await.
    Promise,
    /// `QuotaCounter.reserve_and_increment` — agents holding a quota lease
    /// (GOL-364).
    ///
    /// The only stream whose traffic crosses the shard-manager↔executor link:
    /// holding a token keeps the executor renewing its lease against the
    /// shard-manager every few seconds. Every other stream goes client →
    /// worker-service → executor and never touches it.
    Quota,
    /// Pinned HTTP `invoke_and_await` operations held in flight across a fault
    /// (GOL-366). Distinct from `Durable` even though both land on `Counter`
    /// agents: these are deliberately long-running and deliberately aimed at
    /// one known executor, so mixing them into the durable population would
    /// blur two different experiments.
    PinnedHttp,
    /// `PromiseWaiter.arm` / `wait` / the external completion that resolves it
    /// (GOL-377). Distinct from `Promise` even though both land on the promise
    /// component: that stream creates and resolves a promise in one breath with
    /// nobody suspended on it, and this one exists precisely to leave an agent
    /// parked across a fault.
    PromiseWait,
}

impl Stream {
    pub fn as_str(self) -> &'static str {
        match self {
            Stream::Durable => "durable",
            Stream::Ephemeral => "ephemeral",
            Stream::Scheduled => "scheduled",
            Stream::Promise => "promise",
            Stream::Quota => "quota",
            Stream::PinnedHttp => "pinned-http",
            Stream::PromiseWait => "promise-wait",
        }
    }

    /// Whether the stream keeps a durable count the driver can read back and
    /// compare against what it submitted.
    ///
    /// Two streams do not, for different reasons, and both are named in the
    /// summary rather than quietly omitted:
    ///
    /// - `Ephemeral` agents keep no state across invocations at all.
    /// - `Promise` operations resolve a one-shot promise rather than advancing a
    ///   counter, so there is no accumulated number to read. They are reported
    ///   on created/completed counts and latency.
    /// - `PromiseWait` agents *do* keep a durable count, but comparing totals
    ///   would be strictly weaker than what S11 already does with them: every
    ///   completion carries a token into the waiter's wakeup log, so the report
    ///   pairs individual completions against individual wakeups instead of
    ///   arguing about sums. See [`crate::chaos::wakeups`].
    pub fn has_readback(self) -> bool {
        matches!(
            self,
            Stream::Durable | Stream::Scheduled | Stream::PinnedHttp | Stream::Quota
        )
    }

    pub const ALL: [Stream; 7] = [
        Stream::Durable,
        Stream::Ephemeral,
        Stream::Scheduled,
        Stream::Promise,
        Stream::Quota,
        Stream::PinnedHttp,
        Stream::PromiseWait,
    ];
}

impl std::fmt::Display for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which phase of the scenario an operation was submitted in. Phase is assigned
/// at submission, not completion: an operation submitted just before the kill
/// and completed after it belongs to the fault phase, which is exactly the
/// population an operator cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Baseline,
    Fault,
    Recovery,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Baseline => "baseline",
            Phase::Fault => "fault",
            Phase::Recovery => "recovery",
        }
    }

    pub const ALL: [Phase; 3] = [Phase::Baseline, Phase::Fault, Phase::Recovery];
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How an operation ended, from the driver's point of view.
///
/// The three-way split is the whole point: `Indeterminate` is not a failure, it
/// is an admission that the driver cannot tell, and the read-back turns that
/// admission into the width of a range rather than a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// The invocation returned success, possibly after a same-key retry. The
    /// platform definitely executed it — at least once.
    Confirmed,
    /// Every attempt failed at the transport level. The platform may or may not
    /// have executed it; nobody can say which from the client side.
    Indeterminate,
    /// A definite, non-transport error came back. The platform rejected it and
    /// did not execute it.
    Rejected,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Confirmed => "confirmed",
            Outcome::Indeterminate => "indeterminate",
            Outcome::Rejected => "rejected",
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One attempt at one operation.
///
/// The per-attempt log exists because the aggregate fields cannot answer the
/// exactly-once question on their own: "this key has more than one distinct
/// successful completion" is a statement about *attempts*, and an operation
/// that succeeded on its retry after the first attempt also succeeded is
/// exactly the shape a duplicate takes. Keeping every attempt also means an
/// archived history can be re-analysed later without re-running anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRecord {
    /// 1-based, so `attempt: 2` reads as "the retry".
    pub attempt: u32,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    /// Value the agent returned, when this attempt succeeded and the method
    /// returns one. `Some` here *is* the record of a successful completion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returned_value: Option<u32>,
    /// Whether this attempt succeeded. Distinct from `returned_value` being
    /// set: several workload methods succeed while returning nothing.
    pub succeeded: bool,
    /// How the failure was classified. Absent when the attempt succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<ErrorClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One submitted operation and everything the driver learned about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    /// Monotonic index within the run, so records can be ordered without
    /// depending on wall-clock resolution.
    pub op_id: u64,
    pub stream: Stream,
    pub phase: Phase,
    /// The agent the operation targeted. Read-back is per agent, so this is
    /// what localises a duplicate to a short list of suspect keys.
    pub agent: String,
    pub method: String,
    /// The deterministic idempotency key. Deterministic so that a retry is
    /// genuinely the *same* operation to the platform — a fresh key per attempt
    /// would silently turn a duplicate-execution bug into a clean run.
    pub idempotency_key: String,
    pub submitted_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Total attempts, including the first. `> 1` means the bounded same-key
    /// retry fired, which makes this operation worth a closer look.
    pub attempts: u32,
    pub outcome: Outcome,
    /// Wall-clock spent across all attempts, in milliseconds.
    pub duration_ms: u64,
    /// Value the agent returned, when it returned one. For `Counter.increment`
    /// this is the post-increment count, which makes a retry that comes back
    /// with a *higher* value direct proof of double execution for this one key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returned_value: Option<u32>,
    /// Value the first attempt returned, when it returned one and a later
    /// attempt also did. Only populated for retried operations, because that is
    /// the only case where comparing the two says anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_attempt_value: Option<u32>,
    /// Last error seen, for the operator. Not parsed by anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How the *final* attempt failed. Absent when the operation succeeded.
    /// This is what [`Outcome`] is derived from, kept alongside it so a reader
    /// can see why an operation landed in the band of doubt rather than in the
    /// rejected pile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<ErrorClass>,
    /// Every attempt, in order. See [`AttemptRecord`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempt_log: Vec<AttemptRecord>,
}

impl OperationRecord {
    /// Whether the bounded same-key retry fired for this operation.
    pub fn was_retried(&self) -> bool {
        self.attempts > 1
    }

    /// Whether this operation is worth pasting into a trace query: either the
    /// driver cannot tell what happened, or a retry means the platform saw the
    /// same key twice.
    pub fn is_suspect(&self) -> bool {
        self.outcome == Outcome::Indeterminate || self.was_retried()
    }

    /// Direct per-key evidence of double execution: the first attempt and a
    /// later same-key attempt both returned a counter value, and the later one
    /// was higher. Idempotent handling returns the same value; a second
    /// execution returns a larger one.
    ///
    /// This is the sharpest signal available, but it only exists when the first
    /// attempt got a response at all. A killed shard-manager usually means it
    /// did not, which is why the aggregate read-back still has to carry the
    /// load.
    pub fn shows_double_execution(&self) -> bool {
        match (self.first_attempt_value, self.returned_value) {
            (Some(first), Some(last)) => last > first,
            _ => false,
        }
    }

    /// The distinct values this key's successful attempts returned, sorted.
    ///
    /// A correctly deduplicated key has at most one: the platform stores the
    /// result of the single execution and replays it to every later attempt
    /// under the same key. Two entries mean the key executed twice.
    ///
    /// Attempts that succeeded without returning a value contribute nothing —
    /// they are indistinguishable from one another, so counting them would
    /// invent evidence that is not there.
    pub fn distinct_successful_values(&self) -> Vec<u32> {
        let mut values: Vec<u32> = self
            .attempt_log
            .iter()
            .filter(|a| a.succeeded)
            .filter_map(|a| a.returned_value)
            .collect();
        values.sort_unstable();
        values.dedup();
        values
    }

    /// Whether any attempt at this key succeeded at all.
    pub fn had_successful_attempt(&self) -> bool {
        self.outcome == Outcome::Confirmed || self.attempt_log.iter().any(|a| a.succeeded)
    }
}

/// What a scheduled action recorded when it ran, and the log it was recorded
/// in (GOL-378).
///
/// These live here rather than in [`crate::chaos::fires`] for the same reason
/// [`OperationRecord`] does: they are what was *observed*, and the analysis
/// that reduces them is a separate thing that a later ticket may want to redo
/// over an archived run. The first S10 run learned that the expensive way — its
/// delay percentiles turned out to need a correction that could not be applied
/// afterwards, because only the reduced numbers had been archived.
///
/// This one is a single fire, as the target agent recorded it.
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetFireLog {
    pub agent: String,
    /// `ScheduleCounter.polls`, which keeps counting past the log's cap and is
    /// therefore what says whether the log below is complete.
    pub polls: Option<u64>,
    pub fires: Vec<FireRecord>,
    /// Why the agent could not be read, when it could not be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// The persisted history document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDocument {
    pub schema_version: u32,
    pub scenario_code: String,
    /// True when the file was flushed by an abort rather than a completed run,
    /// so a reader never mistakes a partial history for a short one.
    pub partial: bool,
    pub operations: Vec<OperationRecord>,
    /// Per-target fire logs, for the scenarios that drive scheduled actions.
    /// Empty for every other scenario rather than absent, so a reader never
    /// wonders whether the section was dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scheduled_fires: Vec<TargetFireLog>,
    /// Per-waiter wakeup logs, for the scenarios that park agents on promises.
    /// Empty for every other scenario rather than absent, for the same reason as
    /// `scheduled_fires`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub promise_wakeups: Vec<WaiterWakeupLog>,
}

/// One wakeup, as the waiter agent recorded it (GOL-377).
///
/// The times are the *cluster's*, stamped inside the agent, which is what makes
/// this log the authority on whether a completion landed. The driver's own view
/// is in the operation record for the `wait` invocation, and during the fault
/// that view is frequently just a broken connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeupRecord {
    /// The round's idempotency key, carried in by `arm` and back out by the
    /// wakeup. This is what pairs a completion to the wakeup it caused.
    pub token: String,
    /// When the waiter armed the promise.
    pub armed_at: DateTime<Utc>,
    /// When the platform resumed the waiter.
    pub woken_at: DateTime<Utc>,
}

impl WakeupRecord {
    /// How long the waiter was parked, on one clock.
    ///
    /// Both ends are stamped by the executor, so this is free of the driver ↔
    /// cluster skew that the completion-to-wakeup delay carries. It is not the
    /// delay itself — it also contains the round's deliberate dwell — but it is
    /// what lets a reader tell a slow wakeup from a skewed clock.
    pub fn parked_ms(&self) -> i64 {
        (self.woken_at - self.armed_at).num_milliseconds()
    }
}

/// Everything read back from one waiter agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaiterWakeupLog {
    pub agent: String,
    /// `PromiseWaiter.wakes`, which keeps counting past the log's cap and is
    /// therefore what says whether the log below is complete.
    pub wakes: Option<u64>,
    pub wakeups: Vec<WakeupRecord>,
    /// Why the agent could not be read, when it could not be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WaiterWakeupLog {
    /// Whether this log can testify about its own completions.
    ///
    /// Same two ways it cannot as [`TargetFireLog::is_complete`], and the same
    /// consequence: without a whole log an absent wakeup is ambiguous between a
    /// lost completion and a dropped log entry, and S11 must not call the second
    /// one a finding.
    pub fn is_complete(&self) -> bool {
        match (self.error.is_some(), self.wakes) {
            (true, _) => false,
            (false, Some(wakes)) => self.wakeups.len() as u64 >= wakes,
            (false, None) => false,
        }
    }
}

/// Append-only operation log, shared across the concurrent workload streams.
///
/// Cloneable and cheap to clone: every stream holds one and appends to the same
/// underlying log.
#[derive(Debug, Clone)]
pub struct OperationHistory {
    scenario_code: String,
    inner: Arc<Mutex<Vec<OperationRecord>>>,
    next_id: Arc<std::sync::atomic::AtomicU64>,
    fire_logs: Arc<Mutex<Vec<TargetFireLog>>>,
    wakeup_logs: Arc<Mutex<Vec<WaiterWakeupLog>>>,
}

impl OperationHistory {
    pub fn new(scenario_code: impl Into<String>) -> Self {
        Self {
            scenario_code: scenario_code.into(),
            inner: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            fire_logs: Arc::new(Mutex::new(Vec::new())),
            wakeup_logs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Reserves the next operation id. Called at submission so ids reflect
    /// submission order even when completions interleave.
    pub fn next_op_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub fn record(&self, record: OperationRecord) {
        self.inner.lock().unwrap().push(record);
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A snapshot of every record so far, ordered by submission.
    pub fn snapshot(&self) -> Vec<OperationRecord> {
        let mut records = self.inner.lock().unwrap().clone();
        records.sort_by_key(|r| r.op_id);
        records
    }

    /// How many operations in `phase` ended `Confirmed`. Used to fill in the
    /// baseline-ready signal without holding the lock across a write.
    pub fn confirmed_in_phase(&self, phase: Phase) -> u64 {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.phase == phase && r.outcome == Outcome::Confirmed)
            .count() as u64
    }

    /// Archives the fire logs read back at the end of a scheduled scenario.
    ///
    /// Called once, after read-back, before the history is written. Kept here
    /// rather than in the result because the result is the reduced report and
    /// this is the raw material it was reduced from.
    pub fn record_fire_logs(&self, logs: Vec<TargetFireLog>) {
        *self.fire_logs.lock().unwrap() = logs;
    }

    pub fn record_wakeup_logs(&self, logs: Vec<WaiterWakeupLog>) {
        *self.wakeup_logs.lock().unwrap() = logs;
    }

    pub fn document(&self, partial: bool) -> HistoryDocument {
        HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            scenario_code: self.scenario_code.clone(),
            partial,
            operations: self.snapshot(),
            scheduled_fires: self.fire_logs.lock().unwrap().clone(),
            promise_wakeups: self.wakeup_logs.lock().unwrap().clone(),
        }
    }

    /// Writes the history to `path`. Callable mid-run: an aborted scenario
    /// flushes whatever it has, marked `partial`, so a cancelled maintenance
    /// window still yields something to read.
    pub fn save(&self, path: impl AsRef<Path>, partial: bool) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(&self.document(partial))?;
        std::fs::write(path.as_ref(), json).map_err(|e| {
            anyhow::anyhow!("writing operation history to {:?}: {e}", path.as_ref())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn record(op_id: u64, phase: Phase, outcome: Outcome) -> OperationRecord {
        OperationRecord {
            op_id,
            stream: Stream::Durable,
            phase,
            agent: "counter-0".to_string(),
            method: "increment".to_string(),
            idempotency_key: format!("chaos-s12-durable-counter-0-{op_id}"),
            submitted_at: Utc::now(),
            completed_at: Some(Utc::now()),
            attempts: 1,
            outcome,
            duration_ms: 5,
            returned_value: None,
            first_attempt_value: None,
            error_class: None,
            attempt_log: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn snapshot_is_ordered_by_submission_not_completion() {
        let history = OperationHistory::new("S12");
        history.record(record(2, Phase::Fault, Outcome::Confirmed));
        history.record(record(0, Phase::Baseline, Outcome::Confirmed));
        history.record(record(1, Phase::Baseline, Outcome::Indeterminate));

        let ids: Vec<_> = history.snapshot().iter().map(|r| r.op_id).collect();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn confirmed_in_phase_counts_only_confirmed_operations_of_that_phase() {
        let history = OperationHistory::new("S12");
        history.record(record(0, Phase::Baseline, Outcome::Confirmed));
        history.record(record(1, Phase::Baseline, Outcome::Indeterminate));
        history.record(record(2, Phase::Baseline, Outcome::Rejected));
        history.record(record(3, Phase::Fault, Outcome::Confirmed));

        assert_eq!(history.confirmed_in_phase(Phase::Baseline), 1);
        assert_eq!(history.confirmed_in_phase(Phase::Fault), 1);
        assert_eq!(history.confirmed_in_phase(Phase::Recovery), 0);
    }

    #[test]
    fn next_op_id_hands_out_distinct_increasing_ids() {
        let history = OperationHistory::new("S12");
        let ids: Vec<_> = (0..4).map(|_| history.next_op_id()).collect();
        assert_eq!(ids, vec![0, 1, 2, 3]);
    }

    /// A retry returning a *higher* counter value than the first attempt is the
    /// direct per-key proof of double execution.
    #[test]
    fn higher_value_on_retry_shows_double_execution() {
        let mut r = record(0, Phase::Fault, Outcome::Confirmed);
        r.attempts = 2;
        r.first_attempt_value = Some(7);
        r.returned_value = Some(8);
        assert!(r.shows_double_execution());
        assert!(r.is_suspect());
    }

    /// A retry returning the *same* value is proof idempotency deduplicated it.
    #[test]
    fn same_value_on_retry_does_not_show_double_execution() {
        let mut r = record(0, Phase::Fault, Outcome::Confirmed);
        r.attempts = 2;
        r.first_attempt_value = Some(7);
        r.returned_value = Some(7);
        assert!(!r.shows_double_execution());
        // Still worth a look: the platform did see the key twice.
        assert!(r.is_suspect());
    }

    /// The common shard-manager-kill shape: the first attempt returned nothing
    /// at all, so per-key evidence does not exist and the aggregate read-back
    /// has to carry it.
    #[test]
    fn no_first_attempt_value_yields_no_per_key_evidence() {
        let mut r = record(0, Phase::Fault, Outcome::Confirmed);
        r.attempts = 2;
        r.first_attempt_value = None;
        r.returned_value = Some(8);
        assert!(!r.shows_double_execution());
        assert!(r.is_suspect());
    }

    /// Only the two streams that accumulate a durable count can be read back.
    #[test]
    fn readback_is_available_exactly_for_durable_and_scheduled() {
        assert!(Stream::Durable.has_readback());
        assert!(Stream::Scheduled.has_readback());
        assert!(
            !Stream::Ephemeral.has_readback(),
            "ephemeral agents keep no state between invocations"
        );
        assert!(
            !Stream::Promise.has_readback(),
            "promises resolve once rather than accumulating a count"
        );
    }

    #[test]
    fn document_round_trips_through_json() {
        let history = OperationHistory::new("S12");
        history.record(record(0, Phase::Baseline, Outcome::Confirmed));
        history.record(record(1, Phase::Fault, Outcome::Indeterminate));

        let json = serde_json::to_string(&history.document(true)).unwrap();
        let parsed: HistoryDocument = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.schema_version, HISTORY_SCHEMA_VERSION);
        assert_eq!(parsed.scenario_code, "S12");
        assert!(parsed.partial, "partial flag must survive the round trip");
        assert_eq!(parsed.operations.len(), 2);
        assert_eq!(parsed.operations[1].outcome, Outcome::Indeterminate);
    }
}
