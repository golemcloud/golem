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

//! What a partition between two executors cost the agents calling across it
//! (GOL-368).
//!
//! Every other scenario in the suite injects a fault it expects to hurt, and
//! measures the damage. This one injects a fault it expects to be inert, and
//! measures that nothing happened. The claim under test is architectural: an
//! executor never opens a connection to another executor. When an agent invokes
//! an agent its own executor does not own, `DirectWorkerInvocationRpc` hands the
//! call to `worker_proxy`, which is a client of *worker-service*. The reply
//! comes back the same way. So executor A reaches executor B's agents by asking
//! a third party, and cutting the A-to-B link cuts nothing.
//!
//! ### Why a control needs more oracles than a normal scenario, not fewer
//!
//! A scenario that expects damage fails safe: if the fault misses, the damage
//! is absent and the report says the fault was not observed. A scenario that
//! expects *no* damage fails the other way. A run where the pairs were
//! accidentally co-located, where the workload never started, or where the
//! partition was never injected all produce the same clean-looking report as a
//! run that genuinely proved the point.
//!
//! So the numbers here exist mostly to stop this passing for the wrong reason:
//!
//! 1. **Were the pairs actually split?** [`Placement::CrossPod`] must hold at
//!    least `cross_pod_floor_percent` of the callers. Below that the partition
//!    had almost nothing to cut. See [`RelayViolation::PairingTooThin`].
//! 2. **Did the cross-pod calls keep being served?** This is the assertion.
//!    A drop here means an executor was talking to an executor.
//! 3. **Did the co-located calls keep being served?** They never leave the pod,
//!    so the partition cannot reach them even in principle. If *both* groups
//!    drop, the cause is not the link under test and the report says so rather
//!    than blaming the architecture.
//!
//! ### The one thing this cannot check
//!
//! Whether the partition took hold. Every other partition scenario confirms the
//! fault landed by watching something stop; here nothing is supposed to stop,
//! so there is no in-cluster evidence to read. The run relies entirely on Chaos
//! Mesh reporting `AllInjected`, which the workflow waits for before the
//! measured window opens. That is a weaker guarantee than the rest of the suite
//! works to, and it is stated in the report rather than left for a reader to
//! infer — see [`RelayReport::partition_evidence`].
//!
//! ### Throughput, not success rate
//!
//! For the same reason as [`crate::chaos::reachability`]: a stalled agent fails
//! slowly and once, which a success rate reads as one failure out of one
//! attempt. Confirmed operations per second is the number that would collapse
//! if the architecture claim were false, so it is the number the report is
//! built on.

use crate::chaos::history::{OperationRecord, Outcome, Stream};
use crate::chaos::pinned::routing_agent_id_in;
use crate::chaos::split::{
    FaultWindow, Window, longest_silence_ms, round2, window_end, window_secs, window_start,
};
use crate::chaos::summary::LatencyStats;
use crate::chaos::workload::{COUNTER_AGENT, WorkloadContext, rpc_callee_name};
use chrono::{DateTime, Utc};
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tracing::info;

/// The most findings the report carries.
const MAX_FINDINGS: usize = 50;

/// Where the two halves of an RPC pair ended up.
///
/// Not a property of the fault, unlike [`crate::chaos::split::Group`]. Both
/// halves are placed by hashing their agent ids onto shards, so this records
/// what the hash gave rather than what the driver chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    /// Caller and callee are owned by different executors, so the call crosses
    /// the partitioned link. The population the scenario is about.
    CrossPod,
    /// Caller and callee are owned by the same executor, so the call never
    /// leaves the pod. The run's own control.
    CoLocated,
}

impl Placement {
    pub fn as_str(self) -> &'static str {
        match self {
            Placement::CrossPod => "cross-pod",
            Placement::CoLocated => "co-located",
        }
    }
}

impl std::fmt::Display for Placement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Something about the run worth an operator's attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelayViolation {
    /// Too few pairs straddled the two executors for the partition to have had
    /// anything to cut. The run is inconclusive, not clean.
    PairingTooThin,
    /// Cross-pod calls were served materially less during the partition while
    /// co-located calls were not. **This is the finding the scenario exists to
    /// make**: it says the two executors were reaching each other directly.
    CrossPodDegraded,
    /// Both populations dropped together. Something disturbed the cluster, but
    /// a fault that hurts calls which never leave a pod is not evidence about
    /// the link between pods.
    BothDegraded,
    /// Cross-pod calls never recovered their baseline rate after the heal, even
    /// though they held up during the fault. Late damage, and still worth a
    /// look.
    CrossPodDidNotReturn,
    /// Cross-pod calls cost no more than co-located ones on the undisturbed
    /// baseline, so the population labelled cross-pod was not paying for a
    /// network hop and this run measured nothing.
    ///
    /// The guard against the whole scenario being quietly vacuous. Every other
    /// check here compares the two populations *through* the fault, and all of
    /// them pass trivially if the two populations are really the same thing.
    CrossPodNotRelayed,
}

impl RelayViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            RelayViolation::PairingTooThin => "pairing-too-thin",
            RelayViolation::CrossPodDegraded => "cross-pod-degraded",
            RelayViolation::BothDegraded => "both-degraded",
            RelayViolation::CrossPodDidNotReturn => "cross-pod-did-not-return",
            RelayViolation::CrossPodNotRelayed => "cross-pod-not-relayed",
        }
    }
}

impl std::fmt::Display for RelayViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One finding, with the evidence behind it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayFinding {
    pub violation: RelayViolation,
    pub detail: String,
}

/// How each caller's pair was placed, decided before the fault.
///
/// Built once and carried, rather than recomputed while reading the history.
/// Ownership can move during a run, and a report that classified the same
/// operation differently depending on when it was read would be worse than one
/// that states the placement it measured against.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayPairing {
    /// The two executors, as the shard-manager names them.
    pub pods: Vec<String>,
    /// Caller agents whose callee is on the other executor.
    pub cross_pod: Vec<String>,
    /// Caller agents whose callee is on the same executor.
    pub co_located: Vec<String>,
    /// Callers whose owner or callee's owner the routing table did not resolve.
    /// Counted rather than assigned: a guess here would put an operation in the
    /// wrong population and the report is a comparison between populations.
    pub unresolved: Vec<String>,
}

impl RelayPairing {
    /// Which population a caller belongs to, or `None` if it was never placed.
    pub fn placement_of(&self, caller: &str) -> Option<Placement> {
        if self.cross_pod.iter().any(|a| a == caller) {
            Some(Placement::CrossPod)
        } else if self.co_located.iter().any(|a| a == caller) {
            Some(Placement::CoLocated)
        } else {
            None
        }
    }

    /// The share of placed callers that straddle the two executors.
    ///
    /// Denominator is the callers that were *placed*, not every caller
    /// configured. An unresolved caller says nothing either way, and counting
    /// it against the share would let a flaky routing-table read block a run
    /// whose pairs were split perfectly well. Same reasoning as S9's exclusion
    /// of unreadable agents from its forward-leg share.
    pub fn cross_pod_percent(&self) -> Option<f64> {
        let placed = self.cross_pod.len() + self.co_located.len();
        if placed == 0 {
            return None;
        }
        Some(round2(100.0 * self.cross_pod.len() as f64 / placed as f64))
    }
}

/// One population's throughput in one window.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayCell {
    pub placement: Placement,
    pub window: Window,
    /// Callers of this population that offered at least one operation here.
    pub agents_active: usize,
    /// Operations *offered* in this window, attributed by submission time, and
    /// how they eventually ended up.
    pub submitted: u64,
    pub confirmed: u64,
    pub rejected: u64,
    pub indeterminate: u64,
    /// Operations *answered* in this window, whenever they were offered. This
    /// is the one that says whether the population was being served.
    pub served: u64,
    pub window_secs: f64,
    pub served_per_sec: f64,
    /// This cell's rate against the same population's own before-fault rate.
    /// `None` for the before-fault cell itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_of_baseline_percent: Option<f64>,
    /// The longest stretch of this window in which nothing was answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longest_silence_ms: Option<u64>,
    /// Round-trip latency of the operations answered in this window.
    ///
    /// The most sensitive instrument this report has, and on the first run the
    /// only one that could tell the two populations apart at all. Throughput is
    /// set by the driver's own cadence, so both populations sit at the rate they
    /// were asked to run at whether or not a call leaves the pod. Latency is
    /// not: a cross-pod call pays executor -> worker-service -> executor, and
    /// that hop shows up as a flat premium per call.
    ///
    /// So this is where the architecture is actually visible. A *change* in the
    /// premium across the fault window would say the relay path changed under a
    /// partition it is supposed to be indifferent to.
    pub latency: LatencyStats,
}

/// The whole S2 verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayReport {
    /// How the pairs were placed, and against which executors.
    pub pairing: RelayPairing,
    pub cross_pod_percent: Option<f64>,
    /// The thresholds from the suite YAML, recorded so an archived result can
    /// be read against the numbers it was judged by.
    pub cross_pod_floor_percent: f64,
    pub cross_pod_floor_throughput_percent: f64,
    pub co_located_floor_throughput_percent: f64,
    pub cross_pod_premium_floor_ms: u64,
    /// How much more a cross-pod call cost than a co-located one on the
    /// undisturbed baseline, in milliseconds of p50. `None` when either
    /// population had no baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_pod_premium_ms: Option<i64>,
    pub cells: Vec<RelayCell>,
    /// What is, and is not, known about the fault having taken hold. Spelled
    /// out because this scenario has no in-cluster evidence of its own — see
    /// the module docs.
    pub partition_evidence: String,
    /// Records whose caller the pairing never placed. Non-zero means the
    /// pairing and the workload disagree about who was driven.
    pub records_outside_the_pairing: u64,
    pub findings: Vec<RelayFinding>,
    /// Findings past [`MAX_FINDINGS`], dropped rather than carried.
    pub findings_omitted: u64,
}

impl RelayReport {
    /// Whether the run produced a verdict an operator has to act on.
    pub fn has_findings(&self) -> bool {
        !self.findings.is_empty()
    }

    /// One cell, if the run produced it.
    pub fn cell(&self, placement: Placement, window: Window) -> Option<&RelayCell> {
        self.cells
            .iter()
            .find(|c| c.placement == placement && c.window == window)
    }

    /// Lines an operator has to read.
    pub fn attention_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .findings
            .iter()
            .map(|f| format!("S2 {}: {}", f.violation.as_str(), f.detail))
            .collect();

        if self.findings_omitted > 0 {
            lines.push(format!(
                "S2: {} further relay finding(s) were dropped from the report",
                self.findings_omitted
            ));
        }
        if self.records_outside_the_pairing > 0 {
            lines.push(format!(
                "S2: {} operation(s) ran against callers the pairing never placed, so they are \
                 in no population and in no cell — the pairing and the workload disagree about \
                 who was driven",
                self.records_outside_the_pairing
            ));
        }
        lines
    }

    /// Context a reader needs to judge the numbers, findings or not.
    ///
    /// The evidence line goes here rather than into [`Self::attention_lines`]
    /// deliberately. It is a standing property of the scenario, true of every
    /// S2 run including the good ones, and an attention item that fires every
    /// single time teaches a reader to skip attention items.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("S2: {}", self.partition_evidence)];

        if let Some(percent) = self.cross_pod_percent {
            lines.push(format!(
                "S2: {percent}% of placed callers had their callee on the other executor \
                 ({} cross-pod, {} co-located, {} unplaced)",
                self.pairing.cross_pod.len(),
                self.pairing.co_located.len(),
                self.pairing.unresolved.len()
            ));
        }

        if let Some(premium) = self.cross_pod_premium_ms {
            lines.push(format!(
                "S2: a cross-pod call cost {premium}ms more than a co-located one at p50 on the \
                 undisturbed baseline. That premium is the relay hop through worker-service, and \
                 it is the evidence that the two populations really are split"
            ));
        }

        if let (Some(cross), Some(co)) = (
            self.cell(Placement::CrossPod, Window::DuringFault),
            self.cell(Placement::CoLocated, Window::DuringFault),
        ) {
            lines.push(format!(
                "S2: during the partition cross-pod ran at {}% of its own baseline and \
                 co-located at {}%",
                cross
                    .share_of_baseline_percent
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "an unmeasured".to_string()),
                co.share_of_baseline_percent
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "an unmeasured".to_string()),
            ));
        }

        lines
    }
}

/// Places every caller's pair by asking the shard-manager who owns each half.
///
/// Runs before the fault, and its answer is then fixed for the whole run. Shard
/// ownership can move, and a report that re-derived placement while reading the
/// history could file the same caller under both populations.
///
/// Fails rather than proceeding unplaced. A run whose pairs were never resolved
/// would still produce two throughput cells and a clean-looking verdict, and
/// that verdict would be about nothing.
pub async fn select_pairing(
    ctx: &WorkloadContext,
    deps: &BenchmarkTestDependencies,
    callers: &[String],
) -> anyhow::Result<RelayPairing> {
    let table = deps
        .shard_manager()
        .get_routing_table()
        .await
        .map_err(|e| anyhow::anyhow!("S2: could not read the routing table: {e:?}"))?;

    let mut cross_pod = Vec::new();
    let mut co_located = Vec::new();
    let mut unresolved = Vec::new();
    let mut pods: BTreeSet<String> = BTreeSet::new();

    for caller in callers {
        let callee = rpc_callee_name(caller);
        let caller_pod = table.lookup(&routing_agent_id_in(&ctx.counters, COUNTER_AGENT, caller));
        let callee_pod = table.lookup(&routing_agent_id_in(&ctx.counters, COUNTER_AGENT, &callee));

        match (caller_pod, callee_pod) {
            (Some(from), Some(to)) => {
                pods.insert(from.to_string());
                pods.insert(to.to_string());
                if from == to {
                    co_located.push(caller.clone());
                } else {
                    cross_pod.push(caller.clone());
                }
            }
            // One half unresolved says nothing about the pair. Recorded, not
            // guessed: see `RelayPairing::cross_pod_percent`.
            _ => unresolved.push(caller.clone()),
        }
    }

    if cross_pod.is_empty() && co_located.is_empty() {
        anyhow::bail!(
            "S2: the routing table placed none of the {} RPC callers, so the run has no \
             populations to compare",
            callers.len()
        );
    }

    let pairing = RelayPairing {
        pods: pods.into_iter().collect(),
        cross_pod,
        co_located,
        unresolved,
    };
    info!(
        "S2: {} of {} callers call across executors ({}%), {} stay on one, {} unplaced; \
         executors seen: {}",
        pairing.cross_pod.len(),
        callers.len(),
        pairing
            .cross_pod_percent()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "no".to_string()),
        pairing.co_located.len(),
        pairing.unresolved.len(),
        pairing.pods.join(", ")
    );
    Ok(pairing)
}

/// One population's per-window accumulation, before it becomes a cell.
#[derive(Default)]
struct Tally {
    agents: BTreeSet<String>,
    submitted: u64,
    confirmed: u64,
    rejected: u64,
    indeterminate: u64,
    served_at: Vec<DateTime<Utc>>,
    /// Durations of the operations *answered* in this window, so the latency
    /// stats and the served count describe the same set of operations.
    served_ms: Vec<u64>,
}

/// Which window an instant fell in.
fn window_of(at: DateTime<Utc>, fault: Option<FaultWindow>) -> Window {
    let Some(fault) = fault else {
        return Window::Unknown;
    };
    if at < fault.injected_at {
        Window::BeforeFault
    } else if fault.recovered_at.is_some_and(|healed| at >= healed) {
        Window::AfterFault
    } else {
        Window::DuringFault
    }
}

/// Builds the report from the run's history.
///
/// `records` is the whole history; only [`Stream::Rpc`] entries are read, so a
/// caller does not have to pre-filter and cannot filter differently from the
/// way the cells are built.
pub fn build(
    records: &[OperationRecord],
    pairing: RelayPairing,
    fault: Option<FaultWindow>,
    cross_pod_floor_percent: f64,
    cross_pod_floor_throughput_percent: f64,
    co_located_floor_throughput_percent: f64,
    cross_pod_premium_floor_ms: u64,
) -> RelayReport {
    let mut tallies: BTreeMap<(Placement, Window), Tally> = BTreeMap::new();
    let mut first_submitted: BTreeMap<Placement, DateTime<Utc>> = BTreeMap::new();
    let mut last_completed: BTreeMap<Placement, DateTime<Utc>> = BTreeMap::new();
    let mut records_outside_the_pairing = 0u64;

    for record in records.iter().filter(|r| r.stream == Stream::Rpc) {
        let Some(placement) = pairing.placement_of(&record.agent) else {
            records_outside_the_pairing += 1;
            continue;
        };

        first_submitted
            .entry(placement)
            .and_modify(|at| *at = (*at).min(record.submitted_at))
            .or_insert(record.submitted_at);

        // Offered work is filed by submission time: that is when the platform
        // was asked, which is the question a throughput cell answers.
        let offered = tallies
            .entry((placement, window_of(record.submitted_at, fault)))
            .or_default();
        offered.agents.insert(record.agent.clone());
        offered.submitted += 1;
        match record.outcome {
            Outcome::Confirmed => offered.confirmed += 1,
            Outcome::Rejected => offered.rejected += 1,
            Outcome::Indeterminate => offered.indeterminate += 1,
        }

        // Answered work is filed by completion time, which is a different
        // window whenever an operation was held across an edge.
        if let Some(completed_at) = record.completed_at {
            last_completed
                .entry(placement)
                .and_modify(|at| *at = (*at).max(completed_at))
                .or_insert(completed_at);
            if record.outcome == Outcome::Confirmed {
                let answered = tallies
                    .entry((placement, window_of(completed_at, fault)))
                    .or_default();
                answered.served_at.push(completed_at);
                answered.served_ms.push(record.duration_ms);
            }
        }
    }

    let mut cells: Vec<RelayCell> = Vec::new();
    let mut baseline_rate: BTreeMap<Placement, f64> = BTreeMap::new();

    // Before-fault first, so every later cell has a baseline to divide by.
    for window in [
        Window::BeforeFault,
        Window::DuringFault,
        Window::AfterFault,
        Window::Unknown,
    ] {
        for placement in [Placement::CrossPod, Placement::CoLocated] {
            let Some(tally) = tallies.get(&(placement, window)) else {
                continue;
            };
            let first = first_submitted.get(&placement).copied();
            let last = last_completed.get(&placement).copied();
            let secs = window_secs(window, fault, first, last);
            let served = tally.served_at.len() as u64;
            let served_per_sec = if secs > 0.0 {
                round2(served as f64 / secs)
            } else {
                0.0
            };

            if window == Window::BeforeFault && served_per_sec > 0.0 {
                baseline_rate.insert(placement, served_per_sec);
            }
            let share = if window == Window::BeforeFault {
                None
            } else {
                baseline_rate
                    .get(&placement)
                    .map(|base| round2(100.0 * served_per_sec / base))
            };

            cells.push(RelayCell {
                placement,
                window,
                agents_active: tally.agents.len(),
                submitted: tally.submitted,
                confirmed: tally.confirmed,
                rejected: tally.rejected,
                indeterminate: tally.indeterminate,
                served,
                window_secs: round2(secs),
                served_per_sec,
                share_of_baseline_percent: share,
                latency: LatencyStats::from_durations(tally.served_ms.clone()),
                longest_silence_ms: longest_silence_ms(
                    &tally.served_at,
                    window_start(window, fault, first),
                    window_end(window, fault, last),
                ),
            });
        }
    }

    // Measured on the undisturbed baseline, because this is a statement about
    // the workload rather than about the fault. Judging it during the partition
    // would confuse "the pairing is wrong" with "the partition did something".
    let baseline_p50 = |placement: Placement| {
        cells
            .iter()
            .find(|c| c.placement == placement && c.window == Window::BeforeFault)
            .map(|c| c.latency.p50_ms as i64)
    };
    let cross_pod_premium_ms = match (
        baseline_p50(Placement::CrossPod),
        baseline_p50(Placement::CoLocated),
    ) {
        (Some(cross), Some(co)) => Some(cross - co),
        _ => None,
    };

    let cross_pod_percent = pairing.cross_pod_percent();
    let mut report = RelayReport {
        pairing,
        cross_pod_percent,
        cross_pod_floor_percent,
        cross_pod_floor_throughput_percent,
        co_located_floor_throughput_percent,
        cross_pod_premium_floor_ms,
        cross_pod_premium_ms,
        cells,
        partition_evidence: partition_evidence(fault),
        records_outside_the_pairing,
        findings: Vec::new(),
        findings_omitted: 0,
    };
    report.findings = judge(&report);
    if report.findings.len() > MAX_FINDINGS {
        report.findings_omitted = (report.findings.len() - MAX_FINDINGS) as u64;
        report.findings.truncate(MAX_FINDINGS);
    }
    report
}

/// What can honestly be said about the fault having landed.
fn partition_evidence(fault: Option<FaultWindow>) -> String {
    match fault {
        None => "the run never learned when the partition was injected, so every cell here is \
                 filed under an unknown window and none of them can be read against the fault"
            .to_string(),
        Some(window) if window.recovered_at.is_none() => {
            "the run saw the partition injected but never saw it healed, so the during-fault \
             window runs to the last operation rather than to the heal"
                .to_string()
        }
        Some(_) => "Chaos Mesh reporting AllInjected is the only evidence that the partition \
                    took hold. Unlike every other partition scenario there is nothing in the \
                    cluster that is supposed to stop, so a clean result here cannot by itself \
                    distinguish an inert fault from an absent one"
            .to_string(),
    }
}

/// Turns the cells into findings.
fn judge(report: &RelayReport) -> Vec<RelayFinding> {
    let mut findings = Vec::new();

    match report.cross_pod_percent {
        None => findings.push(RelayFinding {
            violation: RelayViolation::PairingTooThin,
            detail: "no RPC caller was placed on either executor, so the run drove no pairs the \
                     partition could reach"
                .to_string(),
        }),
        Some(percent) if percent < report.cross_pod_floor_percent => {
            findings.push(RelayFinding {
                violation: RelayViolation::PairingTooThin,
                detail: format!(
                    "only {percent}% of placed callers had their callee on the other executor, \
                     below the {}% floor — the partition had almost nothing to cut, so a clean \
                     result here would not have been earned",
                    report.cross_pod_floor_percent
                ),
            });
        }
        Some(_) => {}
    }

    // Checked before anything that compares the two populations through the
    // fault, because if this fires those comparisons are between two samples of
    // the same thing and every one of them passes for free.
    match report.cross_pod_premium_ms {
        Some(premium) if premium < report.cross_pod_premium_floor_ms as i64 => {
            findings.push(RelayFinding {
                violation: RelayViolation::CrossPodNotRelayed,
                detail: format!(
                    "on the undisturbed baseline a cross-pod call cost {premium}ms more than a \
                     co-located one at p50, under the {}ms floor. A call to an agent this \
                     executor does not own goes out through worker-service and back, which is a \
                     real network round trip, so the two populations costing the same says they \
                     are not actually split — either the pairing is wrong or both halves were \
                     served locally. Every comparison below is then between two samples of the \
                     same thing",
                    report.cross_pod_premium_floor_ms
                ),
            });
        }
        None => findings.push(RelayFinding {
            violation: RelayViolation::CrossPodNotRelayed,
            detail: "one of the two populations produced no baseline latency, so the run cannot \
                     show that its cross-pod calls were paying for a network hop"
                .to_string(),
        }),
        Some(_) => {}
    }

    let cross = report
        .cell(Placement::CrossPod, Window::DuringFault)
        .and_then(|c| c.share_of_baseline_percent);
    let co = report
        .cell(Placement::CoLocated, Window::DuringFault)
        .and_then(|c| c.share_of_baseline_percent);

    let cross_dropped = cross.is_some_and(|s| s < report.cross_pod_floor_throughput_percent);
    let co_dropped = co.is_some_and(|s| s < report.co_located_floor_throughput_percent);

    // Order matters. Both populations dropping is not evidence about the link
    // between pods, so it must not be reported as though it were.
    if cross_dropped && co_dropped {
        findings.push(RelayFinding {
            violation: RelayViolation::BothDegraded,
            detail: format!(
                "cross-pod fell to {}% of its own baseline and co-located to {}%, below their \
                 {}% and {}% floors. Calls that never leave a pod cannot be hurt by a partition \
                 between pods, so this says the run was disturbed by something other than the \
                 link under test",
                cross.unwrap_or_default(),
                co.unwrap_or_default(),
                report.cross_pod_floor_throughput_percent,
                report.co_located_floor_throughput_percent
            ),
        });
    } else if cross_dropped {
        findings.push(RelayFinding {
            violation: RelayViolation::CrossPodDegraded,
            detail: format!(
                "cross-pod fell to {}% of its own baseline against a {}% floor while co-located \
                 held at {}%. The only difference between the two populations is whether the \
                 call crosses the partitioned link, so this says the executors were reaching \
                 each other directly rather than through worker-service",
                cross.unwrap_or_default(),
                report.cross_pod_floor_throughput_percent,
                co.map(|s| s.to_string())
                    .unwrap_or_else(|| "no measured".to_string())
            ),
        });
    }

    // Only worth saying when the fault window itself was clean: a population
    // that dropped during the fault and stayed down is already reported above,
    // and repeating it as a recovery failure would double-count one problem.
    let after = report
        .cell(Placement::CrossPod, Window::AfterFault)
        .and_then(|c| c.share_of_baseline_percent);
    if !cross_dropped
        && after.is_some_and(|after| after < report.cross_pod_floor_throughput_percent)
    {
        findings.push(RelayFinding {
            violation: RelayViolation::CrossPodDidNotReturn,
            detail: format!(
                "cross-pod held up during the partition but was only at {}% of its baseline \
                 after the heal, below the {}% floor",
                after.unwrap_or_default(),
                report.cross_pod_floor_throughput_percent
            ),
        });
    }

    findings
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::chaos::history::Phase;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
    }

    fn fault() -> Option<FaultWindow> {
        Some(FaultWindow {
            injected_at: at(100),
            recovered_at: Some(at(200)),
        })
    }

    /// How long a call takes in the fixtures, by population.
    ///
    /// Not decoration. The relay premium is what
    /// [`RelayViolation::CrossPodNotRelayed`] is built on, so a fixture where
    /// both populations cost the same is a fixture of a broken run — which is
    /// exactly what one test below wants and what the others must avoid.
    const CO_LOCATED_MS: u64 = 100;
    const CROSS_POD_MS: u64 = 150;

    fn record(agent: &str, submitted: i64, completed: Option<i64>) -> OperationRecord {
        let duration_ms = if agent.starts_with("cross") {
            CROSS_POD_MS
        } else {
            CO_LOCATED_MS
        };
        OperationRecord {
            op_id: 0,
            stream: Stream::Rpc,
            phase: Phase::Fault,
            agent: agent.to_string(),
            method: "increment_through_rpc".to_string(),
            idempotency_key: format!("{agent}-{submitted}"),
            submitted_at: at(submitted),
            completed_at: completed.map(at),
            attempts: 1,
            outcome: if completed.is_some() {
                Outcome::Confirmed
            } else {
                Outcome::Indeterminate
            },
            duration_ms,
            returned_value: None,
            first_attempt_value: None,
            error: None,
            error_class: None,
            attempt_log: Vec::new(),
        }
    }

    fn pairing(cross: &[&str], co: &[&str]) -> RelayPairing {
        RelayPairing {
            pods: vec!["exec-a".to_string(), "exec-b".to_string()],
            cross_pod: cross.iter().map(|s| s.to_string()).collect(),
            co_located: co.iter().map(|s| s.to_string()).collect(),
            unresolved: Vec::new(),
        }
    }

    /// A steady run where neither population moved is the expected outcome, and
    /// it must produce no findings at all.
    #[test]
    fn a_partition_that_changed_nothing_reports_nothing() {
        let mut records = Vec::new();
        for second in (0..300).step_by(2) {
            records.push(record("cross-0", second, Some(second)));
            records.push(record("co-0", second, Some(second)));
        }
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            25.0,
            70.0,
            70.0,
            5,
        );
        assert!(
            !report.has_findings(),
            "expected a clean control run, got {:?}",
            report.findings
        );
        assert_eq!(report.cross_pod_percent, Some(50.0));
    }

    /// The finding the scenario exists to make: the cross-pod half collapsed
    /// and the co-located half did not.
    #[test]
    fn cross_pod_collapsing_alone_says_the_executors_talked_directly() {
        let mut records = Vec::new();
        for second in (0..300).step_by(2) {
            records.push(record("co-0", second, Some(second)));
            // Cross-pod is served before and after, but not during.
            if !(100..200).contains(&second) {
                records.push(record("cross-0", second, Some(second)));
            } else {
                records.push(record("cross-0", second, None));
            }
        }
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            25.0,
            70.0,
            70.0,
            5,
        );
        let violations: Vec<_> = report.findings.iter().map(|f| f.violation).collect();
        assert_eq!(violations, vec![RelayViolation::CrossPodDegraded]);
    }

    /// Both halves dropping is not evidence about the link, and must not be
    /// reported as though it were.
    #[test]
    fn both_populations_dropping_is_not_blamed_on_the_link() {
        let mut records = Vec::new();
        for second in (0..300).step_by(2) {
            let served = !(100..200).contains(&second);
            records.push(record("cross-0", second, served.then_some(second)));
            records.push(record("co-0", second, served.then_some(second)));
        }
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            25.0,
            70.0,
            70.0,
            5,
        );
        let violations: Vec<_> = report.findings.iter().map(|f| f.violation).collect();
        assert_eq!(violations, vec![RelayViolation::BothDegraded]);
        assert!(
            !violations.contains(&RelayViolation::CrossPodDegraded),
            "a fault that also hurt calls which never left a pod must not be read as \
             executor-to-executor traffic"
        );
    }

    /// A run whose pairs nearly all landed on one executor proved nothing, and
    /// has to say so rather than reporting the clean numbers it produced.
    #[test]
    fn too_few_cross_pod_pairs_makes_the_run_inconclusive() {
        let mut records = Vec::new();
        for second in (0..300).step_by(2) {
            records.push(record("cross-0", second, Some(second)));
            for co in [
                "co-0", "co-1", "co-2", "co-3", "co-4", "co-5", "co-6", "co-7", "co-8",
            ] {
                records.push(record(co, second, Some(second)));
            }
        }
        let report = build(
            &records,
            pairing(
                &["cross-0"],
                &[
                    "co-0", "co-1", "co-2", "co-3", "co-4", "co-5", "co-6", "co-7", "co-8",
                ],
            ),
            fault(),
            25.0,
            70.0,
            70.0,
            5,
        );
        assert_eq!(report.cross_pod_percent, Some(10.0));
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == RelayViolation::PairingTooThin),
            "a 10% split is below the 25% floor and must be called out"
        );
    }

    /// The guard the first cluster run showed was needed.
    ///
    /// Run 33789281692 came back perfectly clean on every throughput cell, and
    /// throughput could not have said otherwise: the driver sets the cadence,
    /// so both populations run at the rate they were asked to whether or not a
    /// call leaves the pod. Only latency separated them, at 151ms against
    /// 101ms. A run where that premium is absent is a run whose two populations
    /// are one population, and every comparison it makes passes for free.
    #[test]
    fn two_populations_that_cost_the_same_did_not_cross_a_pod_boundary() {
        let mut records = Vec::new();
        for second in (0..300).step_by(2) {
            // Both at the co-located cost: nothing here paid a relay hop.
            records.push(record("co-0", second, Some(second)));
            records.push(record("co-1", second, Some(second)));
        }
        // `co-1` is *labelled* cross-pod, and the latency says it is not.
        let report = build(
            &records,
            pairing(&["co-1"], &["co-0"]),
            fault(),
            25.0,
            70.0,
            70.0,
            5,
        );
        assert_eq!(report.cross_pod_premium_ms, Some(0));
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == RelayViolation::CrossPodNotRelayed),
            "a pairing whose halves cost the same has to be called out, got {:?}",
            report.findings
        );
    }

    /// The premium is the evidence the split is real, so it belongs in the
    /// notes on every run rather than only when something is wrong.
    #[test]
    fn the_relay_premium_is_reported_on_a_clean_run() {
        let mut records = Vec::new();
        for second in (0..300).step_by(2) {
            records.push(record("cross-0", second, Some(second)));
            records.push(record("co-0", second, Some(second)));
        }
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            25.0,
            70.0,
            70.0,
            5,
        );
        assert!(
            !report.has_findings(),
            "expected clean, got {:?}",
            report.findings
        );
        assert_eq!(
            report.cross_pod_premium_ms,
            Some((CROSS_POD_MS - CO_LOCATED_MS) as i64)
        );
        assert!(
            report.note_lines().iter().any(|l| l.contains("relay hop")),
            "the premium is the evidence the populations are split; it must reach the reader"
        );
    }

    /// An unresolved caller says nothing either way, so it must not drag the
    /// cross-pod share down and fail an otherwise well-split run.
    #[test]
    fn unresolved_callers_do_not_count_against_the_split() {
        let pairing = RelayPairing {
            pods: vec!["exec-a".to_string(), "exec-b".to_string()],
            cross_pod: vec!["cross-0".to_string()],
            co_located: vec!["co-0".to_string()],
            unresolved: (0..50).map(|i| format!("lost-{i}")).collect(),
        };
        assert_eq!(pairing.cross_pod_percent(), Some(50.0));
    }

    /// A record for a caller nobody placed is counted, not silently dropped:
    /// it means the pairing and the workload disagree.
    #[test]
    fn records_for_unplaced_callers_are_counted() {
        let records = vec![record("nobody", 10, Some(10))];
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            25.0,
            70.0,
            70.0,
            5,
        );
        assert_eq!(report.records_outside_the_pairing, 1);
    }

    /// A run with no fault window cannot be read against the fault, and the
    /// report has to say that rather than presenting cells as if it could.
    #[test]
    fn a_run_with_no_fault_window_says_so() {
        let records = vec![record("cross-0", 10, Some(10))];
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            None,
            25.0,
            70.0,
            70.0,
            5,
        );
        assert!(report.partition_evidence.contains("unknown window"));
    }
}
