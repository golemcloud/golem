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

//! The clock-skew account (GOL-383).
//!
//! S19 moves one executor's wall clock back by half a minute and asks what
//! breaks. Most of the answer is "nothing you can see", and the interesting
//! work in this module is establishing *why* that is the honest answer rather
//! than the answer of an experiment that was never wired up.
//!
//! ## Skew is invisible to anything that judges its own timestamps
//!
//! A clock that is uniformly wrong is undetectable from inside. Every
//! comparison a skewed pod makes between two of its own readings gives the
//! right answer, because the error cancels. Only a comparison between *its*
//! clock and *someone else's* can go wrong, so those comparisons are the entire
//! surface a skew can attack, and there are exactly two of them on this
//! platform.
//!
//! **The scheduler is not one of them.** An executor claims scheduled actions
//! `WHERE shard_id = ANY(...) AND available_at_ms <= now`, and shard sets are
//! disjoint, so two executors can never contend for one action however far
//! apart their clocks are. There is no lease to lose and no double fire to
//! produce: the owner simply fires late, by exactly the skew. That is why S19
//! is aimed at the quota lease and not at the recurring schedules the ticket
//! originally named.
//!
//! **The quota lease is.** The shard-manager mints `expires_at` on its own
//! clock and the executor judges it on the executor's, in two places
//! (`quota.rs`): whether to renew, and whether the lease is already dead. That
//! asymmetry is the fault surface, and the platform already ships a fence for
//! it — `LeaseEpoch` exists so "an executor must reject operations from a stale
//! epoch". S19 asks whether the fence holds when the two clocks disagree.
//!
//! ## Why thirty seconds, and why backwards
//!
//! Not a round number picked for the ticket. On golem-dev the lease runs for
//! `60s` and the executor renews once fewer than `20s` remain, so a renewal
//! normally lands 40 seconds into a 60-second lease with 20 seconds of headroom.
//! A skew smaller than that headroom keeps every renewal inside the valid window
//! and the fault is inert. Thirty seconds overshoots it by ten, so the skewed
//! executor renews **ten seconds after its own lease has, by the granting
//! authority's clock, already expired** — and does so on every cycle, for the
//! whole window.
//!
//! Backwards, because forwards does nothing. A clock that runs fast makes the
//! executor renew *early*: more RPCs, no disagreement.
//!
//! ## Why the control group also needs quota agents
//!
//! The shard-manager's expiry is lazy. `reclaim_expired` runs only inside
//! `acquire_lease` and `renew_lease`, and `renew_lease` refreshes the caller's
//! own `expires_at` *before* it housekeeps. So a skewed executor renewing late
//! rescues its own lease and nothing ever notices. The stale lease is only
//! reclaimed if some *other* pod touches the same resource inside that
//! ten-second window.
//!
//! This is why S19 departs from the ticket's "pin recurring agents to the
//! skewed executor": the quota population has to span both executors. The
//! skewed one holds the stale lease; the healthy one is what makes the
//! disagreement real.
//!
//! ## What can actually be measured, and by whom
//!
//! The scheduled stream's fire log is worth being careful about, because it
//! looks like it measures the skew and does not.
//!
//! Fire delay is `observed - scheduled`: the driver mints the due time, the
//! target agent stamps the fire. On a skewed pod *both* the decision to fire and
//! the stamp come from the same wrong clock. An action due at `D` fires at true
//! `D + 30` and is stamped `D`, so the delay reads **zero**. The two errors
//! cancel exactly. A run that trusted this number would report a perfectly
//! punctual scheduler while every fire was half a minute late.
//!
//! Two things do see it:
//!
//! * **The oplog probe.** The driver knows when it made a call on its own
//!   clock; the executor stamps the oplog entry that call produced. The
//!   difference is the skew, straight, with no cancellation because the two
//!   clocks belong to different machines. This is [`ClockProbe`], and it is
//!   what proves the fault landed.
//! * **The recovery edge.** When the skew lifts the clock jumps forward and the
//!   backlog fires with a *corrected* stamp, so those fires show a real delay
//!   of about the skew. The fire report is blind during the fault and sighted
//!   the moment it ends, which is why its `after-fault` cells are the ones to
//!   read.
//!
//! ## What the two findings mean, and which of them fails a run
//!
//! * [`SkewViolation::ClockNeverMoved`] — the probe could not confirm the
//!   offset. This one **fails the run**, as
//!   `TerminationReason::FaultTargetUnverified`, because it is that reason
//!   exactly: the window was spent and nothing shows the fault reached its
//!   target. A run whose fault was inert has measured nothing, and recording it
//!   as a pass is the failure S2 was built to avoid.
//! * [`SkewViolation::QuotaDidNotRecover`] — the quota stream did not return to
//!   its own baseline once the clock was fixed. Reported, not failed, the same
//!   way `relay::RelayViolation::RelayDidNotRecover` is. Losing a lease under
//!   skew is a legitimate response, and how long getting it back may take is a
//!   judgement rather than a constant.
//!
//! Duplicate execution and lost accepted work — the ticket's headline
//! guarantees — are not checked here. They are checked by the exactly-once,
//! scheduled-fire and read-back oracles the scenario ends with, which is the
//! right place for them: a duplicate is a duplicate whatever caused it.

use crate::chaos::history::{OperationRecord, Outcome, Stream};
use crate::chaos::pinned::routing_agent_id;
use crate::chaos::split::{FaultWindow, Group, PodSplit, Window, round2};
use crate::chaos::summary::LatencyStats;
use crate::chaos::workload::{QUOTA_COUNTER_AGENT, WorkloadContext};
use chrono::{DateTime, Utc};
use golem_common::base_model::OplogIndex;
use golem_common::model::oplog::PublicOplogEntry;
use golem_test_framework::dsl::TestDsl;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use tracing::{info, warn};

/// Scenario code, for the lines a reader eventually sees. Only one scenario
/// moves a clock, so this is a constant rather than a field.
const SCENARIO: &str = "S19";

/// Ceiling on one probe's invocation and on each of the two oplog reads around
/// it.
///
/// A probe that hangs must cost one reading, not the round. Generous, because a
/// slow answer is still an answer and the offset it carries is tens of seconds
/// wide.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// One driver-versus-executor clock reading.
///
/// The driver notes its own clock, invokes an agent, then reads back the
/// timestamp the executor wrote on the oplog entry that invocation produced.
/// `offsetMs` is `stamped - asked`, so a pod running half a minute behind
/// reports about `-30000`.
///
/// The reading is not exact and does not need to be: it carries the invocation's
/// own round trip, which is milliseconds against an offset of tens of seconds.
/// What it has to distinguish is "the clock moved by roughly what we asked for"
/// from "the clock never moved", and it separates those by three orders of
/// magnitude.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockProbe {
    pub agent: String,
    /// Which executor owned the agent when the split was taken.
    pub group: Group,
    /// Which side of the fault the reading was taken on, filled in by
    /// [`build`] from `askedAt`.
    ///
    /// The verdict reads only the probes taken *during* the fault. Probes from
    /// the baseline are archived rather than judged, and they are what
    /// distinguishes a broken probe from a clock that never moved: a baseline
    /// round that read cleanly and a fault round that read nothing are two very
    /// different reports.
    pub window: Window,
    /// The driver's clock, immediately before the invocation.
    pub asked_at: DateTime<Utc>,
    /// The executor's stamp on the oplog entry, when it could be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stamped_at: Option<DateTime<Utc>>,
    /// `stamped_at - asked_at`, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<i64>,
    /// Why the probe produced nothing. An unreadable probe is not an offset of
    /// zero, and conflating the two would let a broken read pass as proof that
    /// the fault was inert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a skewed run did that it should not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkewViolation {
    /// The probes could not show the executor's clock had moved, so the run
    /// exercised nothing.
    ClockNeverMoved,
    /// The quota stream never came back to its baseline after the clock was
    /// corrected.
    QuotaDidNotRecover,
}

impl SkewViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            SkewViolation::ClockNeverMoved => "clock-never-moved",
            SkewViolation::QuotaDidNotRecover => "quota-did-not-recover",
        }
    }
}

impl std::fmt::Display for SkewViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One violation, with the arithmetic that produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkewFinding {
    pub violation: SkewViolation,
    pub detail: String,
}

/// One (stream, group, window) cell of the quota account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkewCell {
    pub stream: Stream,
    pub group: Group,
    pub window: Window,
    pub submitted: u64,
    pub confirmed: u64,
    pub rejected: u64,
    pub indeterminate: u64,
    /// Latency over confirmed operations only, filed by completion time.
    pub latency: LatencyStats,
}

/// The clock-skew account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkewReport {
    /// What the suite asked Chaos Mesh for, in milliseconds. Negative for a
    /// clock set behind.
    pub injected_offset_ms: i64,
    /// How far the measured offset may sit from the injected one before the run
    /// is treated as having failed to inject anything.
    pub tolerance_ms: i64,
    /// Percentage of its own baseline the quota stream's post-fault p50 may
    /// reach before the run is called unrecovered.
    pub recovery_floor_percent: f64,
    /// Every reading taken, including the ones that failed. Archived in full
    /// because the whole verdict rests on them.
    pub probes: Vec<ClockProbe>,
    /// Median offset over the probes on the skewed executor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_offset_ms: Option<i64>,
    /// Median offset over the probes on every other executor: the reading that
    /// says how much of the number above is ordinary driver-to-cluster skew
    /// rather than the fault.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_offset_ms: Option<i64>,
    pub cells: Vec<SkewCell>,
    /// Post-fault p50 as a percentage of the baseline p50, on the skewed
    /// executor's quota agents. Reported whether or not it breaches the floor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_recovery_percent: Option<f64>,
    pub findings: Vec<SkewFinding>,
}

impl SkewReport {
    pub fn has_violations(&self) -> bool {
        !self.findings.is_empty()
    }

    /// One cell, or `None` when the run produced no operations for it.
    pub fn cell(&self, group: Group, window: Window) -> Option<&SkewCell> {
        self.cells
            .iter()
            .find(|c| c.group == group && c.window == window)
    }

    /// What a reader has to act on.
    pub fn attention_lines(&self) -> Vec<String> {
        self.findings
            .iter()
            .map(|f| format!("{SCENARIO} {}: {}", f.violation.as_str(), f.detail))
            .collect()
    }

    /// Context a reader needs to judge the numbers, findings or not.
    ///
    /// The measured offset goes here rather than into
    /// [`Self::attention_lines`], on every run including the good ones. It is
    /// the only line that says the experiment happened at all, and this
    /// scenario is the one where a clean report and a report of nothing look
    /// identical.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        match (self.observed_offset_ms, self.control_offset_ms) {
            (Some(observed), control) => {
                let control = control.unwrap_or(0);
                lines.push(format!(
                    "{SCENARIO}: the skewed executor stamped its oplog {observed}ms from the \
                     driver's clock and the healthy one {control}ms, so the fault moved the \
                     clock by {}ms against the {}ms asked for",
                    observed - control,
                    self.injected_offset_ms
                ));
            }
            (None, _) => lines.push(format!(
                "{SCENARIO}: no readable clock probe inside the fault window, so nothing in this \
                 run says the offset was applied"
            )),
        }

        // The fire log is worth a standing line because its during-fault
        // numbers look wrong until you know why. Both the decision to fire and
        // the stamp come from the same wrong clock, so the two errors cancel
        // and the delay reads zero however late the action really was.
        lines.push(format!(
            "{SCENARIO}: fire delay is blind during the fault — a skewed pod stamps its own late \
             fire with its own late clock — so the scheduled cost of the skew is in the \
             after-fault cells, where the backlog drains under a corrected clock"
        ));

        if let Some(percent) = self.quota_recovery_percent {
            lines.push(format!(
                "{SCENARIO}: quota latency on the skewed executor's agents ended at {percent}% of \
                 its own baseline, against a {}% floor",
                self.recovery_floor_percent
            ));
        }

        for group in [Group::OnPod, Group::Elsewhere] {
            if let (Some(before), Some(during)) = (
                self.cell(group, Window::BeforeFault),
                self.cell(group, Window::DuringFault),
            ) {
                lines.push(format!(
                    "{SCENARIO}: {} quota p50 went {}ms -> {}ms across the injection ({} \
                     confirmed before, {} during, {} rejected during)",
                    group.as_str(),
                    before.latency.p50_ms,
                    during.latency.p50_ms,
                    before.confirmed,
                    during.confirmed,
                    during.rejected
                ));
            }
        }

        lines
    }
}

/// Everything the caller has to decide, kept out of the suite YAML's way.
#[derive(Debug, Clone)]
pub struct SkewInputs<'a> {
    pub split: &'a PodSplit,
    pub fault: Option<FaultWindow>,
    pub injected_offset_ms: i64,
    pub tolerance_ms: i64,
    pub recovery_floor_percent: f64,
    pub probes: Vec<ClockProbe>,
}

/// Builds the account.
pub fn build(records: &[OperationRecord], inputs: SkewInputs<'_>) -> SkewReport {
    let mut tallies: BTreeMap<(Group, Window), Tally> = BTreeMap::new();

    for record in records.iter().filter(|r| r.stream == Stream::Quota) {
        let Some(group) = inputs.split.group_of(&record.agent) else {
            // An agent the selection never saw cannot be attributed to either
            // side, and guessing would put the fault's own damage in the
            // control group.
            continue;
        };

        let offered = tallies
            .entry((group, Window::of(record.submitted_at, inputs.fault)))
            .or_default();
        offered.submitted += 1;
        match record.outcome {
            Outcome::Confirmed => offered.confirmed += 1,
            Outcome::Rejected => offered.rejected += 1,
            Outcome::Indeterminate => offered.indeterminate += 1,
        }

        // Latency is filed by *completion*, not submission: an operation held
        // across the recovery edge was paid for on the far side of it.
        if record.outcome == Outcome::Confirmed
            && let Some(completed_at) = record.completed_at
        {
            tallies
                .entry((group, Window::of(completed_at, inputs.fault)))
                .or_default()
                .latencies
                .push(record.duration_ms);
        }
    }

    let cells: Vec<SkewCell> = tallies
        .into_iter()
        .map(|((group, window), tally)| SkewCell {
            stream: Stream::Quota,
            group,
            window,
            submitted: tally.submitted,
            confirmed: tally.confirmed,
            rejected: tally.rejected,
            indeterminate: tally.indeterminate,
            latency: LatencyStats::from_durations(tally.latencies),
        })
        .collect();

    // Classified here rather than by the caller, so the probes and the cells
    // answer "which side of the fault" the same way and cannot drift.
    let mut probes = inputs.probes;
    for probe in &mut probes {
        probe.window = Window::of(probe.asked_at, inputs.fault);
    }

    let observed_offset_ms = median_offset(&probes, Group::OnPod);
    let control_offset_ms = median_offset(&probes, Group::Elsewhere);

    let mut report = SkewReport {
        injected_offset_ms: inputs.injected_offset_ms,
        tolerance_ms: inputs.tolerance_ms,
        recovery_floor_percent: inputs.recovery_floor_percent,
        probes,
        observed_offset_ms,
        control_offset_ms,
        cells,
        quota_recovery_percent: None,
        findings: Vec::new(),
    };

    report.quota_recovery_percent = recovery_percent(&report);
    report.findings = findings(&report);
    report
}

#[derive(Default)]
struct Tally {
    submitted: u64,
    confirmed: u64,
    rejected: u64,
    indeterminate: u64,
    latencies: Vec<u64>,
}

/// Median offset over the readable probes taken inside the fault window, in one
/// group.
///
/// A median rather than a mean because a single probe that caught a slow
/// invocation would drag an average by seconds, and rather than the extreme
/// because one unreadable-but-parsed timestamp should not decide a run.
///
/// Restricted to the fault window because the baseline probes read ~0 by
/// construction: averaging the two rounds together would halve the measured
/// offset and put a correctly injected skew outside its own tolerance.
fn median_offset(probes: &[ClockProbe], group: Group) -> Option<i64> {
    let mut offsets: Vec<i64> = probes
        .iter()
        .filter(|p| p.group == group && p.window == Window::DuringFault)
        .filter_map(|p| p.offset_ms)
        .collect();
    if offsets.is_empty() {
        return None;
    }
    offsets.sort_unstable();
    Some(offsets[offsets.len() / 2])
}

/// Post-fault p50 as a percentage of the baseline p50, on the skewed executor.
fn recovery_percent(report: &SkewReport) -> Option<f64> {
    let baseline = report
        .cell(Group::OnPod, Window::BeforeFault)?
        .latency
        .p50_ms as f64;
    let after = report
        .cell(Group::OnPod, Window::AfterFault)?
        .latency
        .p50_ms as f64;
    (baseline > 0.0).then(|| round2(100.0 * after / baseline))
}

fn findings(report: &SkewReport) -> Vec<SkewFinding> {
    let mut findings = Vec::new();

    // ── Did the clock actually move? ────────────────────────────────────────
    //
    // The control offset is subtracted rather than ignored. The driver runs on
    // a GitHub runner and the cluster on EC2; neither is guaranteed to agree
    // with the other to the millisecond, and the fault is the *difference*
    // between the two executors, not the absolute reading of either.
    let readable = report
        .probes
        .iter()
        .filter(|p| p.window == Window::DuringFault && p.offset_ms.is_some())
        .count();
    let taken = report
        .probes
        .iter()
        .filter(|p| p.window == Window::DuringFault)
        .count();
    match report.observed_offset_ms {
        None => findings.push(SkewFinding {
            violation: SkewViolation::ClockNeverMoved,
            detail: format!(
                "no probe on the skewed executor produced a readable timestamp inside the fault \
                 window ({readable} of {taken} readable there, {} taken over the whole run), so \
                 the run cannot show the {}ms offset was ever applied",
                report.probes.len(),
                report.injected_offset_ms
            ),
        }),
        Some(observed) => {
            let control = report.control_offset_ms.unwrap_or(0);
            let measured = observed - control;
            let drift = (measured - report.injected_offset_ms).abs();
            if drift > report.tolerance_ms {
                findings.push(SkewFinding {
                    violation: SkewViolation::ClockNeverMoved,
                    detail: format!(
                        "asked for {}ms of skew and measured {}ms (skewed executor {}ms, control \
                         {}ms), which is {}ms out against a {}ms tolerance",
                        report.injected_offset_ms,
                        measured,
                        observed,
                        control,
                        drift,
                        report.tolerance_ms
                    ),
                });
            }
        }
    }

    // ── Did the quota stream come back? ─────────────────────────────────────
    if let Some(percent) = report.quota_recovery_percent
        && percent >= report.recovery_floor_percent
    {
        findings.push(SkewFinding {
            violation: SkewViolation::QuotaDidNotRecover,
            detail: format!(
                "quota latency on the skewed executor's agents stood at {percent}% of its own \
                 baseline after the clock was corrected, at or above the {}% floor",
                report.recovery_floor_percent
            ),
        });
    }

    findings
}

/// Takes one clock reading against a quota agent.
///
/// Reads the agent's last oplog index, notes the driver's clock, invokes the
/// agent's `count` method, then reads the entries that invocation produced and
/// takes the timestamp off the first of them. That timestamp was written by the
/// executor that owns the agent, so the difference between it and the driver's
/// reading is the difference between two machines' clocks and nothing else.
///
/// `count` rather than `reserve_and_increment` on purpose. The probe must not
/// appear in the operation history, and a reservation that did would leave the
/// quota read-back short by however many probes the run took — a lost-work
/// finding manufactured by the instrument.
pub async fn probe_clock(ctx: &WorkloadContext, agent: &str, group: Group) -> ClockProbe {
    let agent_id = routing_agent_id(ctx, QUOTA_COUNTER_AGENT, agent);

    let failed = |asked_at: DateTime<Utc>, error: String| ClockProbe {
        agent: agent.to_string(),
        group,
        window: Window::Unknown,
        asked_at,
        stamped_at: None,
        offset_ms: None,
        error: Some(error),
    };

    let before =
        match tokio::time::timeout(PROBE_TIMEOUT, ctx.user.get_oplog_last_index(&agent_id)).await {
            Ok(Ok(index)) => index,
            Ok(Err(e)) => return failed(Utc::now(), format!("reading the oplog length: {e:#}")),
            Err(_) => {
                return failed(
                    Utc::now(),
                    format!("reading the oplog length timed out after {PROBE_TIMEOUT:?}"),
                );
            }
        };

    // Taken as late as possible, so the interval this reading has to absorb is
    // the invocation alone rather than the invocation plus the read before it.
    let asked_at = Utc::now();
    let parsed = golem_common::agent_id!(QUOTA_COUNTER_AGENT, agent.to_string());
    if let Err(e) = tokio::time::timeout(
        PROBE_TIMEOUT,
        ctx.user.invoke_and_await_agent(
            &ctx.counters,
            &parsed,
            "count",
            golem_common::data_value!(),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out after {PROBE_TIMEOUT:?}"))
    .and_then(|inner| inner)
    {
        return failed(asked_at, format!("invoking the agent: {e:#}"));
    }

    let entries = match tokio::time::timeout(
        PROBE_TIMEOUT,
        ctx.user
            .get_oplog(&agent_id, OplogIndex::from_u64(before + 1)),
    )
    .await
    {
        Ok(Ok(entries)) => entries,
        Ok(Err(e)) => return failed(asked_at, format!("reading back the oplog: {e:#}")),
        Err(_) => {
            return failed(
                asked_at,
                format!("reading back the oplog timed out after {PROBE_TIMEOUT:?}"),
            );
        }
    };

    let Some(stamped_at) = entries.first().and_then(|e| entry_timestamp(&e.entry)) else {
        return failed(
            asked_at,
            format!(
                "the invocation added {} oplog entries and none of them carried a readable \
                 timestamp",
                entries.len()
            ),
        );
    };

    ClockProbe {
        agent: agent.to_string(),
        group,
        window: Window::Unknown,
        asked_at,
        stamped_at: Some(stamped_at),
        offset_ms: Some((stamped_at - asked_at).num_milliseconds()),
        error: None,
    }
}

/// The executor's stamp on one oplog entry.
///
/// Every variant of `PublicOplogEntry` carries a `timestamp` in its parameters
/// and the enum is tagged rather than nested, so the field sits at the top level
/// of the serialised form whatever the entry turned out to be. Going through
/// JSON rather than matching thirty variants keeps this from needing an arm per
/// entry kind, which is a list that grows.
fn entry_timestamp(entry: &PublicOplogEntry) -> Option<DateTime<Utc>> {
    let value = serde_json::to_value(entry).ok()?;
    let raw = value.get("timestamp")?.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// Takes a round of readings, `per_group` from each side of the split.
///
/// Both sides, always. The skewed executor's reading on its own cannot say how
/// much of itself is the fault: the driver runs on a GitHub runner and the
/// cluster on EC2, and nothing makes those two agree. The healthy executor's
/// reading is what subtracts that out.
pub async fn probe_round(
    ctx: &WorkloadContext,
    split: &PodSplit,
    per_group: u32,
) -> Vec<ClockProbe> {
    let mut probes = Vec::new();
    for (group, agents) in [
        (Group::OnPod, &split.on_pod),
        (Group::Elsewhere, &split.elsewhere),
    ] {
        for agent in agents.iter().take(per_group as usize) {
            let probe = probe_clock(ctx, agent, group).await;
            match (&probe.offset_ms, &probe.error) {
                (Some(offset), _) => info!(
                    "{SCENARIO}: clock probe on {} ({}) read {offset}ms",
                    probe.agent,
                    group.as_str()
                ),
                (None, Some(error)) => warn!(
                    "{SCENARIO}: clock probe on {} ({}) failed: {error}",
                    probe.agent,
                    group.as_str()
                ),
                (None, None) => {}
            }
            probes.push(probe);
        }
    }
    probes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::Phase;
    use chrono::TimeDelta;
    use test_r::test;

    const INJECTED_MS: i64 = -30_000;
    const TOLERANCE_MS: i64 = 5_000;
    const RECOVERY_FLOOR: f64 = 150.0;

    fn at(offset_secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + offset_secs, 0).unwrap()
    }

    /// Injected at +100s, healed at +200s, so `at(50)` is baseline, `at(150)`
    /// is inside the fault and `at(250)` is after it.
    fn fault() -> FaultWindow {
        FaultWindow {
            injected_at: at(100),
            recovered_at: Some(at(200)),
        }
    }

    fn split() -> PodSplit {
        PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: vec!["quota-skewed".to_string()],
            elsewhere: vec!["quota-healthy".to_string()],
            targets_per_pod: BTreeMap::new(),
            number_of_shards: 1024,
        }
    }

    fn probe(group: Group, asked_secs: i64, offset_ms: Option<i64>) -> ClockProbe {
        ClockProbe {
            agent: match group {
                Group::OnPod => "quota-skewed".to_string(),
                Group::Elsewhere => "quota-healthy".to_string(),
            },
            group,
            window: Window::Unknown,
            asked_at: at(asked_secs),
            stamped_at: offset_ms.map(|ms| at(asked_secs) + TimeDelta::milliseconds(ms)),
            offset_ms,
            error: offset_ms.is_none().then(|| "unreadable".to_string()),
        }
    }

    fn record(agent: &str, submitted_secs: i64, duration_ms: u64) -> OperationRecord {
        OperationRecord {
            op_id: 0,
            stream: Stream::Quota,
            phase: Phase::Fault,
            agent: agent.to_string(),
            method: "reserve_and_increment".to_string(),
            idempotency_key: format!("{agent}-{submitted_secs}"),
            submitted_at: at(submitted_secs),
            completed_at: Some(at(submitted_secs)),
            attempts: 1,
            outcome: Outcome::Confirmed,
            duration_ms,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: Vec::new(),
        }
    }

    fn build_with(records: &[OperationRecord], probes: Vec<ClockProbe>) -> SkewReport {
        let split = split();
        build(
            records,
            SkewInputs {
                split: &split,
                fault: Some(fault()),
                injected_offset_ms: INJECTED_MS,
                tolerance_ms: TOLERANCE_MS,
                recovery_floor_percent: RECOVERY_FLOOR,
                probes,
            },
        )
    }

    /// The baseline round reads about zero by construction, so averaging it in
    /// with the fault round would halve the measured offset and put a correctly
    /// injected skew outside its own tolerance.
    ///
    /// This is the test that stops the scenario failing every good run.
    #[test]
    fn baseline_probes_are_archived_without_diluting_the_measurement() {
        let report = build_with(
            &[],
            vec![
                probe(Group::OnPod, 50, Some(0)),
                probe(Group::OnPod, 50, Some(0)),
                probe(Group::OnPod, 150, Some(-30_000)),
                probe(Group::OnPod, 150, Some(-30_000)),
                probe(Group::Elsewhere, 150, Some(0)),
            ],
        );
        assert_eq!(report.observed_offset_ms, Some(-30_000));
        assert_eq!(report.probes.len(), 5, "every reading is archived");
        assert!(
            report.findings.is_empty(),
            "a correctly injected skew must not be reported as one that never landed, got {:?}",
            report.findings
        );
    }

    /// The driver runs on a GitHub runner and the cluster on EC2, and nothing
    /// makes those two agree. The fault is the *difference* between the two
    /// executors, so a base offset shared by both must not count as skew.
    #[test]
    fn a_clock_offset_shared_by_both_executors_is_not_the_fault() {
        // Both pods read 4s behind the driver; only one of them is also skewed.
        let report = build_with(
            &[],
            vec![
                probe(Group::OnPod, 150, Some(-34_000)),
                probe(Group::Elsewhere, 150, Some(-4_000)),
            ],
        );
        assert_eq!(report.observed_offset_ms, Some(-34_000));
        assert_eq!(report.control_offset_ms, Some(-4_000));
        assert!(
            report.findings.is_empty(),
            "the shared -4000ms belongs to the driver, not to the fault, got {:?}",
            report.findings
        );
    }

    /// A run whose fault never landed looks exactly like a run whose fault
    /// landed and did no harm. This is the only thing that tells them apart.
    #[test]
    fn a_clock_that_never_moved_fails_the_run() {
        let report = build_with(
            &[],
            vec![
                probe(Group::OnPod, 150, Some(-40)),
                probe(Group::Elsewhere, 150, Some(-35)),
            ],
        );
        assert_eq!(
            report.findings.first().map(|f| f.violation),
            Some(SkewViolation::ClockNeverMoved)
        );
    }

    /// An unreadable probe is not a reading of zero. Conflating the two would
    /// let a broken oplog read pass as proof that the fault was inert.
    #[test]
    fn an_unreadable_probe_is_not_an_offset_of_zero() {
        let report = build_with(
            &[],
            vec![
                probe(Group::OnPod, 150, None),
                probe(Group::Elsewhere, 150, Some(0)),
            ],
        );
        assert_eq!(report.observed_offset_ms, None);
        let finding = report
            .findings
            .first()
            .expect("an unreadable probe has to be a finding");
        assert_eq!(finding.violation, SkewViolation::ClockNeverMoved);
        assert!(
            finding.detail.contains("readable"),
            "the detail has to say the probe could not be read, got {:?}",
            finding.detail
        );
    }

    /// Losing a lease under skew is legitimate; never getting it back is not.
    #[test]
    fn quota_latency_that_stays_high_after_the_heal_is_a_finding() {
        let records = vec![
            record("quota-skewed", 50, 10),
            record("quota-skewed", 150, 400),
            record("quota-skewed", 250, 30),
        ];
        let report = build_with(
            &records,
            vec![
                probe(Group::OnPod, 150, Some(-30_000)),
                probe(Group::Elsewhere, 150, Some(0)),
            ],
        );
        assert_eq!(report.quota_recovery_percent, Some(300.0));
        assert_eq!(
            report.findings.first().map(|f| f.violation),
            Some(SkewViolation::QuotaDidNotRecover)
        );
    }

    /// A quota stream that came back is not a finding, however much it cost
    /// while the clock was wrong.
    #[test]
    fn quota_latency_that_returns_to_its_baseline_is_not_a_finding() {
        let records = vec![
            record("quota-skewed", 50, 20),
            record("quota-skewed", 150, 5_000),
            record("quota-skewed", 250, 22),
        ];
        let report = build_with(
            &records,
            vec![
                probe(Group::OnPod, 150, Some(-30_000)),
                probe(Group::Elsewhere, 150, Some(0)),
            ],
        );
        assert_eq!(report.quota_recovery_percent, Some(110.0));
        assert!(report.findings.is_empty(), "got {:?}", report.findings);
    }

    /// Agents the split never placed cannot be attributed to either side, and
    /// guessing would put the fault's own damage in the control group.
    #[test]
    fn operations_on_agents_the_split_never_saw_land_in_no_cell() {
        let report = build_with(&[record("quota-unplaced", 150, 10)], Vec::new());
        assert!(report.cells.is_empty(), "got {:?}", report.cells);
    }

    /// Latency is filed by completion, not submission: an operation held across
    /// the heal was paid for on the far side of it.
    #[test]
    fn an_operation_held_across_the_heal_costs_the_window_it_finished_in() {
        let mut held = record("quota-skewed", 150, 60_000);
        held.completed_at = Some(at(250));
        let report = build_with(&[held], Vec::new());

        let during = report
            .cell(Group::OnPod, Window::DuringFault)
            .expect("submitted during the fault, so it is offered there");
        assert_eq!(during.submitted, 1);
        assert_eq!(
            during.latency.p50_ms, 0,
            "the cost belongs to the window it was paid in, not the one it was offered in"
        );

        let after = report
            .cell(Group::OnPod, Window::AfterFault)
            .expect("completed after the heal, so its cost is filed there");
        assert_eq!(after.submitted, 0);
        assert_eq!(after.latency.p50_ms, 60_000);
    }

    /// The verdict is only ever taken over the fault window, so a run that
    /// probed nothing there has measured nothing however many baseline
    /// readings it archived.
    #[test]
    fn baseline_probes_alone_cannot_carry_a_run() {
        let report = build_with(
            &[],
            vec![
                probe(Group::OnPod, 50, Some(-30_000)),
                probe(Group::Elsewhere, 50, Some(0)),
            ],
        );
        assert_eq!(report.observed_offset_ms, None);
        assert_eq!(
            report.findings.first().map(|f| f.violation),
            Some(SkewViolation::ClockNeverMoved)
        );
    }
}
