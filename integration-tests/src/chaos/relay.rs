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

//! What a fault cost the agents calling across executors (GOL-368, GOL-382).
//!
//! Two scenarios share this report and disagree about which way its numbers
//! should point, which is what [`RelayExpectation`] is for.
//!
//! **S2 injects a fault it expects to be inert**, and measures that nothing
//! happened. The claim under test is architectural: an executor never opens a
//! connection to another executor. When an agent invokes an agent its own
//! executor does not own, `DirectWorkerInvocationRpc` hands the call to
//! `worker_proxy`, which is a client of *worker-service*. The reply comes back
//! the same way. So executor A reaches executor B's agents by asking a third
//! party, and cutting the A-to-B link cuts nothing.
//!
//! **S21 injects a fault aimed at that third party**, and measures what it
//! cost. Same populations, same cells, opposite verdict.
//!
//! ### The premium is the instrument, and it took a real run to find it
//!
//! Throughput cannot separate the two populations. The driver sets the cadence,
//! so both run at the rate they were asked to whether or not a call leaves the
//! pod: S2's first run measured 9.51/s cross-pod and 10.49/s co-located before,
//! during and after a partition, and those cells look identical on a healthy run
//! *and* on a run whose pairing was broken.
//!
//! Latency does separate them, and by an exactly interpretable amount. Every
//! call in the workload crosses worker-service once; a cross-pod call crosses it
//! twice. So the gap between the two populations is one worker-service hop and
//! nothing else — 38ms and 50ms on S2's two runs. That makes it usable in a
//! window where *everything* is slower, which is precisely the window S21 runs
//! in: a fault that widened the premium reached worker-service, and one that
//! moved both populations together did not.
//!
//! ### Why either scenario needs more oracles than a normal one
//!
//! A scenario that expects damage fails safe: if the fault misses, the damage is
//! absent and the report says the fault was not observed. Neither of these does.
//! S2 fails the other way — a run where the pairs were accidentally co-located,
//! where the workload never started, or where the partition was never injected
//! all produce the same clean report as a run that proved the point. S21 fails
//! the same way one step along: a stress that missed worker-service produces the
//! same undisturbed numbers as a platform that shrugged the load off.
//!
//! So the numbers here exist mostly to stop a run passing for the wrong reason:
//!
//! 1. **Were the pairs actually split?** [`Placement::CrossPod`] must hold at
//!    least `cross_pod_floor_percent` of the callers. See
//!    [`RelayViolation::PairingTooThin`].
//! 2. **Are the two populations really two?** A cross-pod call has to cost more
//!    than a co-located one on the undisturbed baseline. See
//!    [`RelayViolation::CrossPodNotRelayed`].
//! 3. **Did the fault do what the scenario said it would?** Under `Inert`, a
//!    cross-pod drop is the finding. Under `RelayDegraded`, a premium that never
//!    widened is. See [`RelayViolation::CrossPodDegraded`] and
//!    [`RelayViolation::RelayNotDegraded`].
//! 4. **Did it stop when the fault stopped?** Shared, because a bounded fault
//!    has to have a bounded effect. See
//!    [`RelayViolation::CrossPodDidNotReturn`].
//!
//! ### The one thing S2 cannot check
//!
//! Whether its partition took hold. Every other partition scenario confirms the
//! fault landed by watching something stop; there nothing is supposed to stop,
//! so there is no in-cluster evidence to read and the run relies entirely on
//! Chaos Mesh reporting `AllInjected`. S21 is not in that position — the premium
//! is its own evidence — and [`RelayReport::partition_evidence`] says which of
//! the two a reader is holding.
//!
//! ### Throughput, not success rate
//!
//! For the same reason as [`crate::chaos::reachability`]: a stalled agent fails
//! slowly and once, which a success rate reads as one failure out of one
//! attempt. Confirmed operations per second is what a success rate cannot say.

use crate::chaos::history::{OperationRecord, Outcome, Stream};
use crate::chaos::pinned::routing_agent_id_in;
use crate::chaos::split::{
    FaultWindow, Window, longest_silence_ms, round2, window_end, window_secs, window_start,
};
use crate::chaos::summary::LatencyStats;
use crate::chaos::workload::{COUNTER_AGENT, WorkloadContext, rpc_callee_name};
use crate::chaos::{RelayConfig, ScenarioCode};
use chrono::{DateTime, Utc};
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tracing::info;

/// The most findings the report carries.
const MAX_FINDINGS: usize = 50;

/// How far apart the two populations' shares of their own baselines have to be
/// before the difference is read as one of them being hurt more.
///
/// The driver sets the cadence for both, so on an undisturbed run they land
/// within a point of each other: S2's two green runs measured cross-pod at
/// 100.11% against co-located at 99.9%, then 100.23% against 99.91%. Without a
/// margin, ordinary jitter of that size would file
/// [`RelayViolation::CoLocatedDegradedMore`] on any run where the coin landed
/// the other way up.
const PLACEMENT_SHARE_MARGIN_PERCENT: f64 = 5.0;

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

/// What the run's fault is supposed to do to the relay.
///
/// The two scenarios built on this pairing measure the same things and disagree
/// only about which way the numbers should point, so the split lives here
/// rather than in two copies of the report.
///
/// It is not a cosmetic label. Under [`Self::Inert`] a cross-pod population that
/// fell behind is the finding; under [`Self::RelayDegraded`] it is the expected
/// result and the finding is the opposite one, a fault that changed nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelayExpectation {
    /// The fault cannot reach the relay path, and the run exists to show that.
    ///
    /// S2: two executors cut off from each other, when no executor ever opens a
    /// connection to another executor. The default, because it is what this
    /// module was built for.
    #[default]
    Inert,
    /// The fault is aimed at the relay itself, and the run exists to measure
    /// what that costs without losing work.
    ///
    /// S21: worker-service starved of CPU while both populations depend on it.
    RelayDegraded,
}

impl RelayExpectation {
    pub fn as_str(self) -> &'static str {
        match self {
            RelayExpectation::Inert => "inert",
            RelayExpectation::RelayDegraded => "relay-degraded",
        }
    }
}

impl std::fmt::Display for RelayExpectation {
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
    /// The fault was supposed to reach the relay and the premium did not widen,
    /// so the run has no evidence it reached anything.
    ///
    /// [`RelayExpectation::RelayDegraded`] only. The mirror of
    /// [`Self::CrossPodNotRelayed`]: that one says the two populations were
    /// never split, this one says they were split and the fault missed the
    /// thing that separates them.
    RelayNotDegraded,
    /// Co-located calls lost more of their throughput than cross-pod ones did.
    ///
    /// [`RelayExpectation::RelayDegraded`] only, and backwards for a fault on
    /// the shared relay. A cross-pod call crosses worker-service twice and a
    /// co-located one crosses it once, so a fault on worker-service cannot hurt
    /// the shorter path more.
    ///
    /// Which makes this the same kind of statement as [`Self::BothDegraded`] is
    /// for the control: something other than the fault disturbed the run. It is
    /// not "the load hit the executors" — on golem-dev it cannot, because the
    /// executors hold a dedicated node pool that worker-service's own node
    /// selector excludes. The candidates are an executor that restarted, and a
    /// shard reassignment that left the pairing describing agents which have
    /// since moved.
    CoLocatedDegradedMore,
}

impl RelayViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            RelayViolation::PairingTooThin => "pairing-too-thin",
            RelayViolation::CrossPodDegraded => "cross-pod-degraded",
            RelayViolation::BothDegraded => "both-degraded",
            RelayViolation::CrossPodDidNotReturn => "cross-pod-did-not-return",
            RelayViolation::CrossPodNotRelayed => "cross-pod-not-relayed",
            RelayViolation::RelayNotDegraded => "relay-not-degraded",
            RelayViolation::CoLocatedDegradedMore => "co-located-degraded-more",
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

/// The whole relay verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayReport {
    /// Which scenario produced this. Carried so the report's own prose names it
    /// rather than every line hard-coding one of the two scenarios that build
    /// this report.
    pub scenario: ScenarioCode,
    /// Which way the numbers were supposed to point.
    pub expectation: RelayExpectation,
    /// How the pairs were placed, and against which executors.
    pub pairing: RelayPairing,
    pub cross_pod_percent: Option<f64>,
    /// The thresholds from the suite YAML, recorded so an archived result can
    /// be read against the numbers it was judged by.
    pub cross_pod_floor_percent: f64,
    pub cross_pod_floor_throughput_percent: f64,
    pub co_located_floor_throughput_percent: f64,
    pub cross_pod_premium_floor_ms: u64,
    /// The inflation floor, when the run carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_pod_premium_inflation_floor_percent: Option<f64>,
    /// How much more a cross-pod call cost than a co-located one on the
    /// undisturbed baseline, in milliseconds of p50. `None` when either
    /// population had no baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_pod_premium_ms: Option<i64>,
    /// The same premium measured inside the fault window.
    ///
    /// The difference between the two populations is one worker-service hop, so
    /// this number is that hop under whatever the fault did to it. Recorded for
    /// both expectations: S21 judges it, and S2 gains a second way of saying its
    /// partition changed nothing, since a partition between executors has no
    /// business moving the cost of a relay through a third party.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_pod_premium_during_fault_ms: Option<i64>,
    /// The fault-window premium as a percentage of the baseline premium, so
    /// **100 means it did not move** and 250 means it is two and a half times
    /// as wide. Same convention as
    /// [`RelayCell::share_of_baseline_percent`].
    ///
    /// `None` when either premium is missing, or when the baseline premium is
    /// zero and a percentage would divide by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_pod_premium_inflation_percent: Option<f64>,
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
        let code = self.scenario;
        let mut lines: Vec<String> = self
            .findings
            .iter()
            .map(|f| format!("{code} {}: {}", f.violation.as_str(), f.detail))
            .collect();

        if self.findings_omitted > 0 {
            lines.push(format!(
                "{code}: {} further relay finding(s) were dropped from the report",
                self.findings_omitted
            ));
        }
        if self.records_outside_the_pairing > 0 {
            lines.push(format!(
                "{code}: {} operation(s) ran against callers the pairing never placed, so they \
                 are in no population and in no cell — the pairing and the workload disagree \
                 about who was driven",
                self.records_outside_the_pairing
            ));
        }
        lines
    }

    /// Context a reader needs to judge the numbers, findings or not.
    ///
    /// The evidence line goes here rather than into [`Self::attention_lines`]
    /// deliberately. It is a standing property of the scenario, true of every
    /// run including the good ones, and an attention item that fires every
    /// single time teaches a reader to skip attention items.
    ///
    /// Under [`RelayExpectation::RelayDegraded`] the degradation itself is a
    /// note for the same reason. The run was asked to hurt these calls, so
    /// reporting that it did as an attention item would bury the one line that
    /// says whether the platform stayed correct while it happened.
    pub fn note_lines(&self) -> Vec<String> {
        let code = self.scenario;
        let mut lines = vec![format!("{code}: {}", self.partition_evidence)];

        if let Some(percent) = self.cross_pod_percent {
            lines.push(format!(
                "{code}: {percent}% of placed callers had their callee on the other executor \
                 ({} cross-pod, {} co-located, {} unplaced)",
                self.pairing.cross_pod.len(),
                self.pairing.co_located.len(),
                self.pairing.unresolved.len()
            ));
        }

        if let Some(premium) = self.cross_pod_premium_ms {
            lines.push(format!(
                "{code}: a cross-pod call cost {premium}ms more than a co-located one at p50 on \
                 the undisturbed baseline. That premium is the relay hop through worker-service, \
                 and it is the evidence that the two populations really are split"
            ));
        }

        if let Some(during) = self.cross_pod_premium_during_fault_ms {
            let movement = match self.cross_pod_premium_inflation_percent {
                Some(percent) => format!("{percent}% of its baseline width"),
                None => "an unmeasured share of its baseline width".to_string(),
            };
            // The same measurement means opposite things to the two scenarios,
            // so the sentence that interprets it has to differ. Reporting one
            // reading under both would leave half the runs carrying a line that
            // argues against what they set out to show.
            let reading = match self.expectation {
                RelayExpectation::Inert => {
                    "A partition between two executors has no business moving the cost of a relay \
                     through a third party, so this is a second way of saying the fault was inert"
                }
                RelayExpectation::RelayDegraded => {
                    "That premium is one worker-service hop and nothing else, so this is where \
                     this run's degradation lives"
                }
            };
            lines.push(format!(
                "{code}: during the fault that same premium was {during}ms, {movement}. {reading}"
            ));
        }

        if let (Some(cross), Some(co)) = (
            self.cell(Placement::CrossPod, Window::DuringFault),
            self.cell(Placement::CoLocated, Window::DuringFault),
        ) {
            let share = |cell: &RelayCell| {
                cell.share_of_baseline_percent
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "an unmeasured".to_string())
            };
            // Throughput is the blunt instrument here and the line says so
            // under the expectation that would otherwise be read wrong. A
            // degradation run whose cells both sit near 100% has not failed to
            // degrade anything; the driver sets the cadence, so these hold until
            // the platform cannot keep up at all, and the premium line above is
            // where the degradation actually shows.
            let caveat = match self.expectation {
                RelayExpectation::Inert => "",
                RelayExpectation::RelayDegraded => {
                    ". The driver sets the cadence, so both hold up until the platform cannot \
                     keep up at all — these are the SLO breach when it comes, not the measurement"
                }
            };
            lines.push(format!(
                "{code}: during the fault cross-pod ran at {}% of its own baseline and \
                 co-located at {}%{caveat}",
                share(cross),
                share(co),
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
    scenario: ScenarioCode,
    config: &RelayConfig,
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

    // The gap between the two populations in one window, in milliseconds of
    // p50. Both populations cross worker-service; only the cross-pod one crosses
    // it twice, so whatever is left after the subtraction is one relay hop and
    // nothing else. That is what makes this a usable instrument in a window
    // where *everything* is slower.
    let premium_in = |window: Window| {
        let p50 = |placement: Placement| {
            cells
                .iter()
                .find(|c| c.placement == placement && c.window == window)
                .map(|c| c.latency.p50_ms as i64)
        };
        match (p50(Placement::CrossPod), p50(Placement::CoLocated)) {
            (Some(cross), Some(co)) => Some(cross - co),
            _ => None,
        }
    };

    // The baseline premium is a statement about the workload rather than about
    // the fault: it says the two populations really are split. Measuring it
    // during the fault instead would confuse "the pairing is wrong" with "the
    // fault did something".
    let cross_pod_premium_ms = premium_in(Window::BeforeFault);
    let cross_pod_premium_during_fault_ms = premium_in(Window::DuringFault);
    let cross_pod_premium_inflation_percent =
        match (cross_pod_premium_ms, cross_pod_premium_during_fault_ms) {
            // A zero baseline has no width to grow by, and a percentage of it would
            // be a division by zero dressed up as a measurement. The run is already
            // reporting `cross-pod-not-relayed` in that case, which is the more
            // useful thing to say.
            (Some(baseline), Some(during)) if baseline > 0 => {
                Some(round2(100.0 * during as f64 / baseline as f64))
            }
            _ => None,
        };

    let cross_pod_percent = pairing.cross_pod_percent();
    let mut report = RelayReport {
        scenario,
        expectation: config.expectation,
        pairing,
        cross_pod_percent,
        cross_pod_floor_percent: config.cross_pod_floor_percent,
        cross_pod_floor_throughput_percent: config.cross_pod_floor_throughput_percent,
        co_located_floor_throughput_percent: config.co_located_floor_throughput_percent,
        cross_pod_premium_floor_ms: config.cross_pod_premium_floor_ms,
        cross_pod_premium_inflation_floor_percent: config.cross_pod_premium_inflation_floor_percent,
        cross_pod_premium_ms,
        cross_pod_premium_during_fault_ms,
        cross_pod_premium_inflation_percent,
        cells,
        partition_evidence: fault_evidence(fault, config.expectation),
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
///
/// The two expectations are in genuinely different positions here, and the line
/// says which one the reader is holding. An inert fault leaves nothing in the
/// cluster to observe, so the run has only Chaos Mesh's word for it. A fault
/// aimed at the relay leaves a mark in the run's own numbers, and the report
/// points at it rather than asking to be trusted.
fn fault_evidence(fault: Option<FaultWindow>, expectation: RelayExpectation) -> String {
    match fault {
        None => "the run never learned when the fault was injected, so every cell here is filed \
                 under an unknown window and none of them can be read against it"
            .to_string(),
        Some(window) if window.recovered_at.is_none() => {
            "the run saw the fault injected but never saw it healed, so the during-fault window \
             runs to the last operation rather than to the heal"
                .to_string()
        }
        Some(_) => match expectation {
            RelayExpectation::Inert => {
                "Chaos Mesh reporting AllInjected is the only evidence that the partition took \
                 hold. Unlike every other partition scenario there is nothing in the cluster \
                 that is supposed to stop, so a clean result here cannot by itself distinguish \
                 an inert fault from an absent one"
                    .to_string()
            }
            RelayExpectation::RelayDegraded => {
                "the fault is aimed at the relay itself, so unlike the control that shares these \
                 populations this run carries its own evidence: the cross-pod premium is one \
                 worker-service hop, and a premium that widened during the window says the \
                 fault reached it"
                    .to_string()
            }
        },
    }
}

/// Turns the cells into findings.
///
/// Split by expectation after the checks the two share. Those are the ones about
/// the *measurement* — whether the populations exist and whether they are really
/// two — and they hold whichever way the fault is supposed to point.
fn judge(report: &RelayReport) -> Vec<RelayFinding> {
    let mut findings = pairing_findings(report);

    let cross = report
        .cell(Placement::CrossPod, Window::DuringFault)
        .and_then(|c| c.share_of_baseline_percent);
    let co = report
        .cell(Placement::CoLocated, Window::DuringFault)
        .and_then(|c| c.share_of_baseline_percent);

    match report.expectation {
        RelayExpectation::Inert => findings.extend(inert_findings(report, cross, co)),
        RelayExpectation::RelayDegraded => {
            findings.extend(degraded_findings(report, cross, co));
        }
    }

    // Shared, and last. Both scenarios require the relay to come back: the
    // control because nothing should have moved it, and the degradation
    // scenario because a fault that is bounded in time has to be bounded in
    // effect too. Only worth saying when the fault window itself was clean
    // under `Inert`, since a population that dropped and stayed down is already
    // reported there and repeating it would double-count one problem.
    let already_reported = report.expectation == RelayExpectation::Inert
        && cross.is_some_and(|s| s < report.cross_pod_floor_throughput_percent);
    let after = report
        .cell(Placement::CrossPod, Window::AfterFault)
        .and_then(|c| c.share_of_baseline_percent);
    if !already_reported
        && after.is_some_and(|after| after < report.cross_pod_floor_throughput_percent)
    {
        findings.push(RelayFinding {
            violation: RelayViolation::CrossPodDidNotReturn,
            detail: format!(
                "cross-pod was only at {}% of its baseline after the heal, below the {}% floor. \
                 The fault was bounded in time, so its effect has to be bounded too",
                after.unwrap_or_default(),
                report.cross_pod_floor_throughput_percent
            ),
        });
    }

    findings
}

/// The checks that are about the measurement rather than about the fault.
///
/// Run first under both expectations, because everything after them compares
/// two populations and all of it passes for free if there are not two.
fn pairing_findings(report: &RelayReport) -> Vec<RelayFinding> {
    let mut findings = Vec::new();

    match report.cross_pod_percent {
        None => findings.push(RelayFinding {
            violation: RelayViolation::PairingTooThin,
            detail: "no RPC caller was placed on either executor, so the run drove no pairs the \
                     fault could reach"
                .to_string(),
        }),
        Some(percent) if percent < report.cross_pod_floor_percent => {
            findings.push(RelayFinding {
                violation: RelayViolation::PairingTooThin,
                detail: format!(
                    "only {percent}% of placed callers had their callee on the other executor, \
                     below the {}% floor — the fault had almost nothing to reach, so a clean \
                     result here would not have been earned",
                    report.cross_pod_floor_percent
                ),
            });
        }
        Some(_) => {}
    }

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

    findings
}

/// S2's verdict: the fault should have changed nothing, so any drop is a
/// finding.
fn inert_findings(report: &RelayReport, cross: Option<f64>, co: Option<f64>) -> Vec<RelayFinding> {
    let mut findings = Vec::new();
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

    findings
}

/// S21's verdict: the fault was aimed at the relay, so the finding is a fault
/// that missed it.
///
/// Neither population dropping is *not* a finding on its own here. The driver
/// sets the cadence, so throughput holds until the platform is too slow to keep
/// up at all, and a scenario that waited for that would only ever fire on a
/// worker-service that had already fallen over.
fn degraded_findings(
    report: &RelayReport,
    cross: Option<f64>,
    co: Option<f64>,
) -> Vec<RelayFinding> {
    let mut findings = Vec::new();

    let floor = report
        .cross_pod_premium_inflation_floor_percent
        .unwrap_or(f64::INFINITY);
    match report.cross_pod_premium_inflation_percent {
        Some(inflation) if inflation < floor => findings.push(RelayFinding {
            violation: RelayViolation::RelayNotDegraded,
            detail: format!(
                "the cross-pod premium was {inflation}% of its baseline width during the fault, \
                 under the {floor}% floor, where 100% is a premium that did not move. That \
                 premium is one worker-service hop and nothing else, so a fault aimed at \
                 worker-service that left it alone did not reach worker-service. Read this run \
                 as inconclusive rather than as the platform absorbing the fault"
            ),
        }),
        None => findings.push(RelayFinding {
            violation: RelayViolation::RelayNotDegraded,
            detail: "the premium could not be compared across the fault window, so the run has \
                     no evidence the fault reached the relay at all"
                .to_string(),
        }),
        Some(_) => {}
    }

    // Reported as a share of a share: co-located keeping less of its own
    // baseline than cross-pod kept of its own. Comparing the two rates directly
    // would compare two populations that never ran at the same rate to begin
    // with.
    if let (Some(cross), Some(co)) = (cross, co)
        && cross - co > PLACEMENT_SHARE_MARGIN_PERCENT
    {
        findings.push(RelayFinding {
            violation: RelayViolation::CoLocatedDegradedMore,
            detail: format!(
                "co-located held {co}% of its own baseline and cross-pod held {cross}%, so the \
                 shorter path lost more, by more than the {PLACEMENT_SHARE_MARGIN_PERCENT} \
                 points these two normally sit apart. A cross-pod call crosses worker-service \
                 twice and a co-located one crosses it once, so a fault on worker-service cannot \
                 hurt the shorter path more — something other than the fault disturbed this run. \
                 Check the executor restarts and the ownership samples: a shard that moved leaves \
                 the pairing naming agents that are no longer where it put them"
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
        record_costing(agent, submitted, completed, duration_ms)
    }

    /// The same record with the call's cost named rather than derived.
    ///
    /// The derived version fixes a population's latency for the whole run, which
    /// is right for the control and useless for the scenario that measures the
    /// premium *changing*. Kept as two functions so the existing fixtures keep
    /// saying what they said.
    fn record_costing(
        agent: &str,
        submitted: i64,
        completed: Option<i64>,
        duration_ms: u64,
    ) -> OperationRecord {
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

    /// The control's thresholds, matching S2's suite entry closely enough that
    /// a test reads against the numbers a real run is judged by.
    fn inert_config() -> RelayConfig {
        RelayConfig {
            cross_pod_floor_percent: 25.0,
            cross_pod_floor_throughput_percent: 70.0,
            co_located_floor_throughput_percent: 70.0,
            cross_pod_premium_floor_ms: 5,
            expectation: RelayExpectation::Inert,
            cross_pod_premium_inflation_floor_percent: None,
        }
    }

    /// S21's, with an inflation floor the fixtures can straddle: the premium
    /// has to be at least half again as wide during the fault.
    fn degraded_config() -> RelayConfig {
        RelayConfig {
            expectation: RelayExpectation::RelayDegraded,
            cross_pod_premium_inflation_floor_percent: Some(150.0),
            ..inert_config()
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
            ScenarioCode::S2,
            &inert_config(),
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
            ScenarioCode::S2,
            &inert_config(),
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
            ScenarioCode::S2,
            &inert_config(),
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
            ScenarioCode::S2,
            &inert_config(),
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
            ScenarioCode::S2,
            &inert_config(),
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
            ScenarioCode::S2,
            &inert_config(),
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
            ScenarioCode::S2,
            &inert_config(),
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
            ScenarioCode::S2,
            &inert_config(),
        );
        assert!(report.partition_evidence.contains("unknown window"));
    }

    /// A run that spans the fault with a named cost on each side.
    ///
    /// `during_*` applies to operations that complete inside the window, which
    /// is how the cells file them, so a fixture whose fault-window calls cost
    /// more is a fixture of a relay under load.
    fn records_across_the_fault(
        during_cross_ms: u64,
        during_co_ms: u64,
        during_step: usize,
    ) -> Vec<OperationRecord> {
        let mut records = Vec::new();
        for second in (0..100).step_by(2) {
            records.push(record_costing(
                "cross-0",
                second,
                Some(second),
                CROSS_POD_MS,
            ));
            records.push(record_costing("co-0", second, Some(second), CO_LOCATED_MS));
        }
        for second in (100..200).step_by(during_step) {
            records.push(record_costing(
                "cross-0",
                second,
                Some(second),
                during_cross_ms,
            ));
            records.push(record_costing("co-0", second, Some(second), during_co_ms));
        }
        for second in (200..300).step_by(2) {
            records.push(record_costing(
                "cross-0",
                second,
                Some(second),
                CROSS_POD_MS,
            ));
            records.push(record_costing("co-0", second, Some(second), CO_LOCATED_MS));
        }
        records
    }

    /// S21's clean run. Both populations got slower and the cross-pod one got
    /// slower *twice over*, which is what one starved worker-service hop looks
    /// like from the outside.
    #[test]
    fn a_relay_fault_that_widened_the_premium_reports_nothing() {
        // Baseline premium 50ms; during the fault 190 - 90 = 100ms, so 200%.
        let records = records_across_the_fault(190, 90, 2);
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            ScenarioCode::S21,
            &degraded_config(),
        );
        assert!(
            !report.has_findings(),
            "expected a clean degradation run, got {:?}",
            report.findings
        );
        assert_eq!(report.cross_pod_premium_ms, Some(50));
        assert_eq!(report.cross_pod_premium_during_fault_ms, Some(100));
        assert_eq!(report.cross_pod_premium_inflation_percent, Some(200.0));
    }

    /// The vacuity guard. Both populations slowed by the same amount, so the
    /// gap between them — one worker-service hop — never moved, and the load
    /// landed somewhere else.
    #[test]
    fn a_fault_that_left_the_premium_alone_did_not_reach_the_relay() {
        // Both populations pay a flat 100ms more, so the premium stays at 50.
        let records = records_across_the_fault(250, 200, 2);
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            ScenarioCode::S21,
            &degraded_config(),
        );
        assert_eq!(report.cross_pod_premium_inflation_percent, Some(100.0));
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == RelayViolation::RelayNotDegraded),
            "a premium that did not move should read as inconclusive, got {:?}",
            report.findings
        );
    }

    /// The same numbers under the control's expectation raise nothing, because
    /// a premium that held steady is exactly what S2 wants to see.
    #[test]
    fn the_control_does_not_ask_its_fault_to_widen_the_premium() {
        let records = records_across_the_fault(150, 100, 2);
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            ScenarioCode::S2,
            &inert_config(),
        );
        assert!(
            !report.has_findings(),
            "the control should not require its fault to change anything, got {:?}",
            report.findings
        );
        assert_eq!(report.cross_pod_premium_inflation_percent, Some(100.0));
    }

    /// Cross-pod collapsing is the control's headline finding and the
    /// degradation scenario's expected result. It must not be reported as a
    /// defect by the one that asked for it.
    #[test]
    fn a_degradation_run_does_not_report_its_own_fault_as_a_defect() {
        // Cross-pod is served a tenth as often inside the window, and pays a
        // widened premium while it happens.
        let records = records_across_the_fault(190, 90, 20);
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            ScenarioCode::S21,
            &degraded_config(),
        );
        let violations: Vec<_> = report.findings.iter().map(|f| f.violation).collect();
        assert!(
            !violations.contains(&RelayViolation::CrossPodDegraded)
                && !violations.contains(&RelayViolation::BothDegraded),
            "the scenario asked for this degradation, so it is context and not a finding: {:?}",
            report.findings
        );
        assert!(
            report
                .note_lines()
                .iter()
                .any(|line| line.contains("cross-pod ran at")),
            "the degradation still has to be reported somewhere: {:?}",
            report.note_lines()
        );
    }

    /// The load hit the executors instead of the relay. A cross-pod call crosses
    /// worker-service twice and a co-located one crosses it once, so the shorter
    /// path cannot be the one that suffers more.
    #[test]
    fn the_shorter_path_losing_more_says_the_load_missed_the_relay() {
        let mut records = Vec::new();
        for second in (0..100).step_by(2) {
            records.push(record_costing(
                "cross-0",
                second,
                Some(second),
                CROSS_POD_MS,
            ));
            records.push(record_costing("co-0", second, Some(second), CO_LOCATED_MS));
        }
        // The premium still widens, so this is not caught by the vacuity guard.
        // Co-located is served far less often than cross-pod all the same.
        for second in (100..200).step_by(4) {
            records.push(record_costing("cross-0", second, Some(second), 190));
        }
        for second in (100..200).step_by(50) {
            records.push(record_costing("co-0", second, Some(second), 90));
        }
        for second in (200..300).step_by(2) {
            records.push(record_costing(
                "cross-0",
                second,
                Some(second),
                CROSS_POD_MS,
            ));
            records.push(record_costing("co-0", second, Some(second), CO_LOCATED_MS));
        }
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            ScenarioCode::S21,
            &degraded_config(),
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == RelayViolation::CoLocatedDegradedMore),
            "the shorter path losing more should be a finding, got {:?}",
            report.findings
        );
    }

    /// The bound that survives the expectation split: a fault that is over has
    /// to stop costing anything, whichever way it was supposed to point.
    #[test]
    fn a_relay_that_never_came_back_is_a_finding_under_either_expectation() {
        let mut records = Vec::new();
        for second in (0..100).step_by(2) {
            records.push(record("cross-0", second, Some(second)));
            records.push(record("co-0", second, Some(second)));
        }
        for second in (100..200).step_by(2) {
            records.push(record_costing("cross-0", second, Some(second), 190));
            records.push(record_costing("co-0", second, Some(second), 90));
        }
        // Cross-pod stays down long after the heal.
        for second in (200..300).step_by(50) {
            records.push(record("cross-0", second, Some(second)));
        }
        for second in (200..300).step_by(2) {
            records.push(record("co-0", second, Some(second)));
        }
        for (code, config) in [
            (ScenarioCode::S2, inert_config()),
            (ScenarioCode::S21, degraded_config()),
        ] {
            let report = build(
                &records,
                pairing(&["cross-0"], &["co-0"]),
                fault(),
                code,
                &config,
            );
            assert!(
                report
                    .findings
                    .iter()
                    .any(|f| f.violation == RelayViolation::CrossPodDidNotReturn),
                "{code} should require the relay to recover, got {:?}",
                report.findings
            );
        }
    }

    /// The two populations sitting a fraction of a point apart is what an
    /// undisturbed run looks like, and it must not be read as one of them being
    /// hurt.
    #[test]
    fn ordinary_jitter_between_the_populations_is_not_an_inversion() {
        let mut records = Vec::new();
        for second in (0..100).step_by(2) {
            records.push(record_costing(
                "cross-0",
                second,
                Some(second),
                CROSS_POD_MS,
            ));
            records.push(record_costing("co-0", second, Some(second), CO_LOCATED_MS));
        }
        // Cross-pod is served on every even second of the window; co-located
        // misses one, which is the sub-point difference S2's green runs showed.
        for second in (100..200).step_by(2) {
            records.push(record_costing("cross-0", second, Some(second), 190));
            if second != 150 {
                records.push(record_costing("co-0", second, Some(second), 90));
            }
        }
        for second in (200..300).step_by(2) {
            records.push(record_costing(
                "cross-0",
                second,
                Some(second),
                CROSS_POD_MS,
            ));
            records.push(record_costing("co-0", second, Some(second), CO_LOCATED_MS));
        }
        let report = build(
            &records,
            pairing(&["cross-0"], &["co-0"]),
            fault(),
            ScenarioCode::S21,
            &degraded_config(),
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.violation == RelayViolation::CoLocatedDegradedMore),
            "a fraction of a point apart is not the shorter path being hurt: {:?}",
            report.findings
        );
    }
}
