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

//! Cloud chaos scenarios (GOL-363).
//!
//! A chaos scenario runs a continuous mixed workload against a deployed
//! Cloud-mode Golem while a bounded fault is injected, then reports what
//! happened. The suite's shape follows density: the workflow drives one scenario
//! per invocation, each scenario is independently selectable through a YAML
//! `enabled` flag, and results are archived to the `golem-bench-results` bucket
//! per scenario so an interrupted run resumes rather than restarts.
//!
//! `S3` in this module always means the scenario code, never the bucket.
//!
//! Two boundaries define this module:
//!
//! 1. **The driver never touches infrastructure.** It does not know Kubernetes,
//!    Chaos Mesh, or Grafana exist. The workflow injects the fault and says so
//!    through [`signal`]; the workflow also turns the driver's plain report into
//!    operator-facing links. That is what lets a scenario be walked through by
//!    hand, against a local cluster, with `echo` and a text editor.
//! 2. **The driver reports; the operator judges.** There is no binary oracle
//!    engine here — see [`summary`] for what is measured and the narrow set of
//!    conditions that fail a run outright.

pub mod deletions;
pub mod errors;
pub mod fires;
pub mod history;
pub mod outage;
pub mod ownership;
pub mod pinned;
pub mod prep;
pub mod probe;
pub mod reachability;
pub mod relay;
pub mod result;
pub mod resurrection;
pub mod reverts;
pub mod rollback;
pub mod scenarios;
pub mod scheduled;
pub mod signal;
pub mod split;
pub mod steady;
pub mod summary;
pub mod truncation;
pub mod waiters;
pub mod wakeups;
pub mod workload;

use crate::chaos::history::Stream;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Stable identifier for a scenario, shared by the YAML switchboard, the CLI,
/// the result artifact and the tickets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScenarioCode {
    /// Shard-manager to executor network partition.
    S1,
    /// Executor pod kill with pinned HTTP invocations in flight.
    S8,
    /// Executor pod kill during an automatic component update.
    S5,
    /// Shard-manager pod restart under mixed workload.
    S12,
    /// Rolling executor restarts under load.
    S13,
    /// Executor pod kill while scheduled actions are between claim and fire.
    S10,
    /// Executor pod kill while agents are suspended on promises being completed.
    S11,
    /// Executor cut off from worker-service while it keeps its shards.
    S3,
    /// Two executors cut off from each other while agents on one invoke agents
    /// on the other.
    ///
    /// The control of the set, and the only scenario whose fault is expected to
    /// change nothing. Executors hold no connection to each other: an
    /// agent-to-agent call the local executor does not own is handed to
    /// `worker_proxy`, which is a client of *worker-service*, not of the
    /// executor that owns the callee. So the link this partition cuts carries
    /// no traffic, and a run that degrades is reporting a path the architecture
    /// says does not exist.
    S2,
    /// Executor pod kill while agents are having their state reverted.
    S7,
    /// Executor pod kill while agents are being deleted.
    S6,
    /// Executor pod kill while a component rollback is in flight.
    S9,
    /// Executors cut off from the key-value PostgreSQL cluster for about as
    /// long as an AWS storage failover takes.
    S16,
    /// The same cut, held for longer than the key-value retry budget, so the
    /// executors are expected to be replaced rather than to ride it out.
    S22,
    /// Executors cut off from the indexed-oplog PostgreSQL cluster, the other
    /// Aurora cluster underneath them, for the length of a writer failover.
    S14,
    /// Executors cut off from the Redis cache that fronts the key-value layer,
    /// for longer than a caller is willing to wait.
    S18,
    /// The same Redis cache still reachable but slowed, to ask whether the
    /// platform degrades or breaks when its worker-status store gets slower.
    S17,
    /// The key-value PostgreSQL cluster still reachable but slowed. S17's
    /// mirror on the other half of the split key-value layer.
    S15,
    /// S15 driving `ephemeral` alone. The control: that stream reaches none of
    /// the delayed cluster, so nothing should move.
    S15A,
    /// S15A plus `durable`, the heaviest user of the delayed cluster.
    S15B,
    /// S15B plus `promise`. The remaining step, `scheduled`, is S15 itself.
    S15C,
    /// The indexed-oplog PostgreSQL cluster still reachable but slowed. S14's
    /// mirror, and the last empty cell of the storage matrix.
    ///
    /// The one delay in the suite with no control stream. Every agent commits
    /// its oplog to this cluster, `ephemeral` included: it never opens the
    /// primary, but the first layer of the stack it does open is a compressed
    /// archive on the same indexed storage.
    S23,
}

impl ScenarioCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ScenarioCode::S1 => "S1",
            ScenarioCode::S8 => "S8",
            ScenarioCode::S5 => "S5",
            ScenarioCode::S12 => "S12",
            ScenarioCode::S13 => "S13",
            ScenarioCode::S10 => "S10",
            ScenarioCode::S11 => "S11",
            ScenarioCode::S3 => "S3",
            ScenarioCode::S2 => "S2",
            ScenarioCode::S7 => "S7",
            ScenarioCode::S6 => "S6",
            ScenarioCode::S9 => "S9",
            ScenarioCode::S16 => "S16",
            ScenarioCode::S22 => "S22",
            ScenarioCode::S14 => "S14",
            ScenarioCode::S18 => "S18",
            ScenarioCode::S17 => "S17",
            ScenarioCode::S15 => "S15",
            ScenarioCode::S15A => "S15A",
            ScenarioCode::S15B => "S15B",
            ScenarioCode::S15C => "S15C",
            ScenarioCode::S23 => "S23",
        }
    }

    /// Every scenario this driver implements. The suite YAML is checked against
    /// this list, so a scenario cannot be enabled in YAML without code behind
    /// it, nor implemented without an operational switch in front of it.
    pub const ALL: [ScenarioCode; 22] = [
        ScenarioCode::S1,
        ScenarioCode::S2,
        ScenarioCode::S3,
        ScenarioCode::S5,
        ScenarioCode::S6,
        ScenarioCode::S7,
        ScenarioCode::S8,
        ScenarioCode::S9,
        ScenarioCode::S10,
        ScenarioCode::S11,
        ScenarioCode::S12,
        ScenarioCode::S13,
        ScenarioCode::S14,
        ScenarioCode::S15,
        ScenarioCode::S15A,
        ScenarioCode::S15B,
        ScenarioCode::S15C,
        ScenarioCode::S16,
        ScenarioCode::S17,
        ScenarioCode::S18,
        ScenarioCode::S22,
        ScenarioCode::S23,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        ScenarioCode::ALL
            .into_iter()
            .find(|c| c.as_str().eq_ignore_ascii_case(s))
    }
}

impl std::fmt::Display for ScenarioCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Environment variable carrying a value unique to one *invocation* of a
/// scenario, set by the workflow.
pub const RUN_NONCE_ENV: &str = "GOLEM_CHAOS_RUN_NONCE";

/// The prefix every agent id and idempotency key of one scenario invocation
/// shares.
///
/// The scenario code alone is not enough, and getting this wrong is silent
/// rather than loud. Idempotency keys are deterministic *by design* — that is
/// what makes a retry the same operation to the platform, and it is the whole
/// basis of the duplicate-execution checks. But determinism across a *resumed
/// run* is a different thing entirely: the resume path reuses the prep
/// manifest, so the same account, components and agent names come back, and
/// without a per-invocation component every key would collide with the previous
/// invocation's.
///
/// The platform would then replay stored results instead of executing anything,
/// and the run would be worse than useless:
///
/// * S8 would find every key already complete and report `0 findings` with a
///   probe that executed nothing — a perfect-looking result that tested nothing.
/// * S12 and S1 would count those replays as confirmed while the durable
///   counters never moved, and read-back would report **lost work** that never
///   happened.
///
/// The nonce goes *after* the scenario code so that the documented trace
/// queries (`span.idempotency_key=~"chaos-s12-.*"`) still match every run.
pub fn scenario_key_prefix(code: ScenarioCode) -> String {
    key_prefix_with_nonce(code, std::env::var(RUN_NONCE_ENV).ok().as_deref())
}

/// The pure half of [`scenario_key_prefix`], so the rule can be tested without
/// mutating process-global state from a parallel test suite.
fn key_prefix_with_nonce(code: ScenarioCode, nonce: Option<&str>) -> String {
    let base = format!("chaos-{}", code.as_str().to_lowercase());
    // Kept to characters that are safe in an agent id and readable in a Grafana
    // query. An absent nonce is the normal local case: a hand-driven run against
    // a fresh cluster has nothing to collide with.
    let nonce: String = nonce
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(32)
        .collect();
    let nonce = nonce.trim_matches('-');
    if nonce.is_empty() {
        base
    } else {
        format!("{base}-{nonce}")
    }
}

/// What the workflow is expected to do to the cluster, mirrored here only so the
/// result can record what the run was configured to provoke. The driver does not
/// act on any of it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaultConfig {
    /// e.g. `pod-kill`.
    pub kind: String,
    /// e.g. `shard-manager`.
    pub target: String,
    /// Chaos Mesh selection mode, e.g. `one`.
    #[serde(default = "default_fault_mode")]
    pub mode: String,
    /// How many pods the fault applies to, for the modes that take a count.
    ///
    /// Recorded here rather than only in the manifest so the archived result
    /// says how wide the blast radius was *configured* to be — "we partitioned
    /// half the executors" is only meaningful next to how many that was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_count: Option<u32>,
    /// The Chaos Mesh manifest to apply, by file name, when it is not the one
    /// the workflow would find from the scenario code.
    ///
    /// Exists for the elimination variants. S15A, S15B and S15C inject exactly
    /// the fault S15 injects and differ only in which streams the workload
    /// drives, so copying `networkchaos-s15.yaml` three times would make four
    /// files that must be kept identical by hand — and a run whose fault
    /// silently diverged from the run it is being compared against is worse
    /// than no run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    pub duration_secs: u64,
}

fn default_fault_mode() -> String {
    "one".to_string()
}

/// Phase durations. The baseline exists so recovery is measured against a warm
/// steady state rather than against cold start.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseConfig {
    pub baseline_secs: u64,
    pub fault_secs: u64,
    pub recovery_secs: u64,
}

impl PhaseConfig {
    pub fn baseline(&self) -> Duration {
        Duration::from_secs(self.baseline_secs)
    }
    pub fn fault(&self) -> Duration {
        Duration::from_secs(self.fault_secs)
    }
    pub fn recovery(&self) -> Duration {
        Duration::from_secs(self.recovery_secs)
    }
}

/// Shape of the continuous mixed workload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadConfig {
    /// Durable counter agents. Read-back is per agent, so this is also how
    /// finely a duplicate can be localised.
    pub durable_agents: u32,
    pub ephemeral_agents: u32,
    pub scheduled_agents: u32,
    pub promise_agents: u32,
    /// Agents holding a quota lease. Zero for scenarios that do not need
    /// shard-manager↔executor traffic; see [`history::Stream::Quota`].
    #[serde(default)]
    pub quota_agents: u32,
    /// Caller agents that invoke another agent through `golem:rpc`. Zero for
    /// scenarios that do not need agent-to-agent traffic; see
    /// [`history::Stream::Rpc`].
    ///
    /// Each caller implies a second agent, its callee, so a run configured with
    /// `n` of these creates `2n` agents rather than `n`.
    #[serde(default)]
    pub rpc_agents: u32,
    /// Combined submission rate across all streams, in operations per second.
    /// The project caps this at 25% of measured per-pod capacity so the run
    /// measures fault recovery rather than saturation.
    pub rate_per_sec: u32,
}

/// Caller retry behaviour.
///
/// The defaults are the project's decision and they matter for correctness, not
/// just for load: retrying **only** transport errors, **once**, under the **same
/// idempotency key** is what lets a retry reveal duplicate execution. An
/// unbounded retry loop with a fresh key per attempt would make every run look
/// clean.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    #[serde(default = "default_true")]
    pub transport_only: bool,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_retry_delay_secs")]
    pub delay_secs: u64,
}

fn default_true() -> bool {
    true
}
fn default_max_retries() -> u32 {
    1
}
fn default_retry_delay_secs() -> u64 {
    5
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            transport_only: true,
            max_retries: default_max_retries(),
            delay_secs: default_retry_delay_secs(),
        }
    }
}

impl RetryPolicy {
    pub fn delay(&self) -> Duration {
        Duration::from_secs(self.delay_secs)
    }
}

/// Shape of a *pinned* workload: a fixed set of agents, all owned by one known
/// executor, each holding one long-running `invoke_and_await` in flight at a
/// time (GOL-366).
///
/// This is a different experiment from [`WorkloadConfig`], not a variation on
/// it. The mixed workload asks "what happens to a stream of short operations
/// when the platform is disturbed"; this one asks "what happens to *these
/// specific* operations, which were provably running on the pod that died".
/// Answering the second needs the agents chosen by shard ownership rather than
/// by index, and needs each operation to still be in flight when the fault
/// lands — hence a duration rather than a rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedConfig {
    /// How many agents to pin, which is also the concurrency: each agent holds
    /// exactly one operation in flight, so this is the number of in-flight
    /// operations at every instant of the run.
    pub agents: u32,
    /// Server-side duration of one operation. Long enough that the workflow can
    /// detect the readiness signal, apply the fault and see it become
    /// `AllInjected` while operations submitted beforehand are still running —
    /// otherwise the kill lands between operations and the scenario measures
    /// nothing.
    pub operation_millis: u32,
    /// How many candidate agent names to hash per pinned agent when looking for
    /// a single executor that owns enough of them. Ownership is a hash, so the
    /// candidates for any one pod are a fraction of the pool; the default
    /// leaves room for an uneven split without an unbounded search.
    #[serde(default = "default_candidate_pool_multiplier")]
    pub candidate_pool_multiplier: u32,
}

fn default_candidate_pool_multiplier() -> u32 {
    8
}

/// Shape of the scheduled-registration workload (GOL-378).
///
/// A third experiment shape next to [`WorkloadConfig`] and [`PinnedConfig`], not
/// a variation on either. The mixed workload asks what a stream of invocations
/// does when the platform is disturbed, and the pinned workload asks what
/// happens to specific invocations that were running on the pod that died. This
/// one asks about work the driver is not holding a connection to at all: an
/// action the platform promised to run later, whose executor died in between.
///
/// The two numbers that decide whether the run measures anything are `leadSecs`
/// and `intervalMillis`. See [`crate::chaos::scheduled`] for why.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledConfig {
    /// Target agents, each with its own emitter. Also the resolution of the
    /// report: a finding localises to one target out of this many.
    pub targets: u32,
    /// Milliseconds between registrations on one target. The offered rate is
    /// `targets / interval`.
    pub interval_millis: u64,
    /// How far ahead each action is registered. With the cadence above this
    /// sets how many actions stand accepted but not yet run at any instant,
    /// which is the population a kill has to land in the middle of.
    pub lead_secs: u64,
    /// What recovering a scheduled action is allowed to cost, which the
    /// fire-delay percentiles are reported against. An SLO the run records
    /// rather than a threshold it fails on: the floor is a shard reassignment,
    /// or the executor's `lease_ttl` for the rarer case of an action that was
    /// already claimed, and how much more than that is acceptable is a
    /// judgement.
    pub lease_budget_secs: u64,
}

impl ScheduledConfig {
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_millis)
    }
    pub fn lead(&self) -> Duration {
        Duration::from_secs(self.lead_secs)
    }
    pub fn lease_budget(&self) -> Duration {
        Duration::from_secs(self.lease_budget_secs)
    }
}

/// Shape of the suspended-waiter workload (GOL-377).
///
/// The fourth experiment shape, and the only one whose agents are *asleep* when
/// the fault lands. [`ScheduledConfig`] leaves work with the platform and walks
/// away; this one leaves an agent parked mid-invocation on a promise, so the
/// thing that has to survive the kill is not a queued action but a suspended
/// worker and the completion on its way to it.
///
/// The number that decides whether the run measures anything is `waiters`: each
/// one holds exactly one promise at a time, so the pool size *is* the population
/// standing parked at the instant the pod dies. `dwellMillis` decides how much
/// of that population is also mid-completion — see [`crate::chaos::waiters`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromiseConfig {
    /// Waiter agents, each parked on at most one promise at a time. Also the
    /// resolution of the report: a finding localises to one waiter out of this
    /// many.
    pub waiters: u32,
    /// How long a waiter stays parked before its promise is completed.
    ///
    /// Sets the completion rate — `waiters / dwell` — and, with it, how many
    /// completions are genuinely in flight when the pod dies. It has to
    /// comfortably exceed the workflow's inject-and-verify path (signal poll,
    /// `kubectl apply`, waiting for `AllInjected`) or every promise armed before
    /// the kill would already have been completed by the time it landed.
    pub dwell_millis: u64,
    /// What resuming a suspended waiter is allowed to cost, which the wakeup
    /// delay percentiles are reported against. An SLO the run records rather
    /// than a threshold it fails on: the floor is a shard reassignment plus the
    /// worker recovery that replays the waiter's oplog, and how much more than
    /// that is acceptable is a judgement.
    pub wakeup_budget_secs: u64,
}

impl PromiseConfig {
    pub fn dwell(&self) -> Duration {
        Duration::from_millis(self.dwell_millis)
    }
    pub fn wakeup_budget(&self) -> Duration {
        Duration::from_secs(self.wakeup_budget_secs)
    }
}

/// Shape of the reachability workload (GOL-370).
///
/// The fifth experiment shape, and the only one where nothing about the
/// platform is broken at all. [`PinnedConfig`] asks what happens to operations
/// running on a pod that dies; this one asks what happens to operations bound
/// for a pod that is perfectly healthy and simply cannot be reached from the
/// tier that routes to it. The executor keeps its shards for the whole fault,
/// because the link it needs in order to keep them — to the shard-manager — is
/// not the one that was cut.
///
/// Every agent gets its own emitter holding at most one operation, rather than
/// the shared per-stream budget [`WorkloadConfig`] drives. That is load-bearing
/// here and not a style choice: the stall this scenario induces is bounded by
/// *which executor owns the agent*, not by which stream it belongs to, so a
/// shared budget would be drained by the isolated half and would stop the
/// reachable half submitting too. The run would then report the control group
/// degrading, and the cause would be the driver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolationConfig {
    /// Durable counter agents, split by shard ownership into the ones the
    /// isolated executor holds and the ones it does not. Also the resolution of
    /// the report: a finding localises to one agent out of this many.
    pub agents: u32,
    /// Milliseconds between one agent's operations, measured from the end of
    /// the previous one. The offered rate is `agents / interval`, and an agent
    /// whose operation is stalled offers nothing at all — which is the signal,
    /// not a gap in it.
    pub interval_millis: u64,
    /// The most of its own baseline throughput the isolated group may keep
    /// during the fault, as a percentage, for the partition to count as
    /// observed.
    ///
    /// A run above this line did not cut the executor off, whatever the fault
    /// status says, and every other number in the report is then a measurement
    /// of an undisturbed cluster. That is reported as inconclusive rather than
    /// clean: a healthy-looking result from a fault that never landed is the
    /// worst artifact this suite can produce.
    pub isolated_ceiling_percent: f64,
    /// The least of its own baseline throughput the control group must keep
    /// during the fault, as a percentage.
    ///
    /// The sharpest thing S3 can find. The agents on the reachable executor
    /// have nothing to do with the partition, so a drop here is collateral
    /// damage from how worker-service handles an unreachable pod — its routing
    /// table is one process-wide entry, and every stalled caller invalidating it
    /// costs every other caller a shard-manager round trip.
    pub control_floor_percent: f64,
    /// What resuming an isolated agent may cost once the link is back, and the
    /// number the recovery gap is reported against. Recorded rather than
    /// asserted, like every other budget in the suite.
    pub recovery_budget_secs: u64,
}

/// Shape of the cross-pod RPC control (GOL-368).
///
/// S2 is the one scenario built to come back empty. It drives agent-to-agent
/// calls whose two halves are known to live on different executors, cuts those
/// executors off from each other, and asserts that nothing moves. The claim
/// under test is architectural: an executor reaches another executor's agents
/// only by asking worker-service, never by connecting to it directly.
///
/// A control is only worth running if it can fail, so the numbers below exist
/// to stop it passing for the wrong reason. A run where the pairs turned out to
/// be co-located, or where the partition never landed, would also report "no
/// degradation" — and that report would be worthless.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayConfig {
    /// The least share of configured callers that must have their callee on the
    /// *other* executor, as a percentage, for the run to mean anything.
    ///
    /// Pairing is not something the driver can arrange. Both halves are placed
    /// by hashing their agent ids onto shards, so the split is whatever the
    /// hash gives — around half on a two-executor cluster, but not guaranteed.
    /// Below this line the fault has almost nothing to cut and the run aborts
    /// rather than reporting a clean result it did not earn. Same instinct as
    /// S9's forward-leg gate.
    pub cross_pod_floor_percent: f64,
    /// The least of its own baseline throughput the cross-pod population must
    /// keep during the fault, as a percentage.
    ///
    /// This is the assertion, and it is deliberately high. Every other scenario
    /// sets a floor low enough to survive the disruption it injects; here there
    /// is nothing to survive, so a real drop means an executor was talking to
    /// an executor.
    pub cross_pod_floor_throughput_percent: f64,
    /// The same floor for the pairs that happened to land on one executor.
    ///
    /// They never leave the pod, so the partition cannot reach them even in
    /// principle. Kept as a separate number because if *both* populations drop
    /// the cause is not the link under test — it is the fault disturbing the
    /// cluster some other way, and the run should say so rather than blame the
    /// architecture.
    pub co_located_floor_throughput_percent: f64,
}

impl IsolationConfig {
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_millis)
    }
    pub fn recovery_budget(&self) -> Duration {
        Duration::from_secs(self.recovery_budget_secs)
    }
}

/// Shape of the revert workload (GOL-371).
///
/// The sixth experiment shape, and the only one that asks the platform to
/// *destroy* durable state on purpose. Every other scenario disturbs work that
/// is trying to happen; this one disturbs work that is trying to be undone.
///
/// Each agent repeats a round: increment `increments_per_round` times, then
/// revert the last `revert_invocations` of them. Both numbers are exact, and
/// that is the point — the driver knows the counter's value before the revert
/// from the last increment's own return value, so the value afterwards has
/// exactly two legitimate answers and no band of doubt between them.
///
/// Reverting needs the worker stopped (`lock_stopped_worker` in
/// `golem-worker-executor/src/worker/mod.rs`), so a revert is not one atomic
/// instant but a stop, a commit and a status reattach. The truncation itself is
/// a single oplog entry and cannot tear; the window worth killing into is the
/// one around it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertConfig {
    /// Counter agents running rounds, split by shard ownership around the
    /// executor the kill is aimed at. Also the resolution of the report: a
    /// torn revert localises to one agent out of this many.
    pub agents: u32,
    /// Increments before each revert. Must be at least `revert_invocations`,
    /// or the revert would reach back into an already-deleted oplog region and
    /// the platform would refuse it — see `find_nth_invocation_from_end`.
    pub increments_per_round: u32,
    /// How many of those increments each revert takes back.
    pub revert_invocations: u32,
    /// Milliseconds an agent waits between rounds. The share of the population
    /// standing mid-revert at any instant is roughly one round-step in
    /// `increments_per_round + 1`, so this and the round length together decide
    /// how much of the mechanism a kill can land in.
    pub interval_millis: u64,
    /// What recovering a reverted agent may cost, and the number the
    /// resume delay is reported against. Recorded rather than asserted, like
    /// every other budget in the suite.
    pub recovery_budget_secs: u64,
}

impl RevertConfig {
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_millis)
    }
    pub fn recovery_budget(&self) -> Duration {
        Duration::from_secs(self.recovery_budget_secs)
    }
    /// What one completed round adds to a counter.
    pub fn net_per_round(&self) -> u32 {
        self.increments_per_round
            .saturating_sub(self.revert_invocations)
    }
}

/// Shape of the deletion workload (GOL-372).
///
/// The seventh experiment shape, and one step past [`RevertConfig`]. A revert
/// asks the platform to forget some of an agent's work; this asks it to forget
/// the agent. Each slot builds a counter up, deletes it, and is used again —
/// invoking a deleted id creates a new agent, so the next round's first
/// increment says which of the two things happened.
///
/// The failure mode it is named for has a defence in the executor already:
/// `start_deleting` stops a background status flush from "resurrecting the
/// cached status" after the durable removal. So the question is not whether
/// anyone thought about it, but whether the defence survives the pod dying
/// between the mark and the removal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConfig {
    /// Agent slots running rounds, split by shard ownership around the executor
    /// the kill is aimed at. Also the resolution of the report: a resurrection
    /// localises to one slot out of this many.
    pub agents: u32,
    /// Increments before each delete.
    ///
    /// More than one, so that a *partial* survival is observable at all. The
    /// two legal answers are always distinguishable — a fresh agent reports 1
    /// and a survivor reports `before + 1`, which never collide — but at one
    /// increment there is no value *between* them, so a slot that came back
    /// carrying some of a state it should have lost has nowhere to land and
    /// [`crate::chaos::resurrection::ResurrectionViolation::PartialState`] can
    /// never fire. Three of them leaves room for it.
    pub increments_per_round: u32,
    /// Milliseconds a slot waits between rounds.
    pub interval_millis: u64,
    /// What recovering a deleted agent's slot may cost. Recorded rather than
    /// asserted, like every other budget in the suite.
    pub recovery_budget_secs: u64,
}

impl DeleteConfig {
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_millis)
    }
    pub fn recovery_budget(&self) -> Duration {
        Duration::from_secs(self.recovery_budget_secs)
    }
}

/// Shape of the component rollback (GOL-369).
///
/// S5 moves agents forward onto a new build and kills an executor while that is
/// happening. S9 moves them forward, waits for that to land, and then moves them
/// **back**, killing an executor during the return leg.
///
/// The rollback is a redeploy: the original artifact is uploaded again as a new
/// revision, and every agent is asked to move to it. That is what "rollback"
/// means operationally, and it is what makes the evidence unambiguous —
/// `Counter::component_version` is compiled into each build, so an agent that
/// has genuinely returned reports `1` from the code that is actually running,
/// not from metadata about what the platform believes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackConfig {
    /// How long to let the roll-forward settle before rolling back.
    ///
    /// Load-bearing. If the agents never reached the new build, the rollback
    /// returns them to a build they never left and the run proves nothing, so
    /// this has to be long enough for the forward leg to finish on a healthy
    /// cluster.
    pub settle_secs: u64,
    /// The share of agents that must actually report the new build before the
    /// rollback is worth attempting, as a percentage.
    ///
    /// Below it the driver stops rather than spending the maintenance window on
    /// a return journey nobody made. The same instinct as S6's smoke round.
    pub rolled_forward_floor_percent: f64,
    /// Retries for the rollback *control plane* — the per-agent update
    /// requests — recorded apart from the workload's own retries.
    ///
    /// Separate because they answer different questions. The workload's retry
    /// exists to expose duplicate execution and is deliberately one attempt.
    /// This one exists so that a refused rollback request is distinguishable
    /// from a rollback that was never asked for, which matters when the
    /// executor owning an agent is about to be killed.
    pub control_retries: u32,
    pub control_retry_delay_secs: u64,
    /// How far into the rollback to ask for the kill.
    pub kill_delay_secs: u64,
}

impl RollbackConfig {
    pub fn settle(&self) -> Duration {
        Duration::from_secs(self.settle_secs)
    }
    pub fn control_retry_delay(&self) -> Duration {
        Duration::from_secs(self.control_retry_delay_secs)
    }
    pub fn kill_delay(&self) -> Duration {
        Duration::from_secs(self.kill_delay_secs)
    }
}

/// Settings for a storage-outage scenario (GOL-379).
///
/// Not a workload shape, unlike the blocks above it: S16 drives the mixed
/// workload and the scheduled-registration workload that already exist, because
/// the fault is not about which agents are driven but about which *dependency*
/// is taken away from all of them. What this block carries is the thing being
/// taken away and the two numbers the account is judged by.
///
/// The endpoint is recorded rather than acted on, like every other entry in
/// [`FaultConfig`]. The driver never resolves it, connects to it or cuts
/// anything off from it; the workflow does that, and this exists so an archived
/// result says which storage the run was about rather than leaving a reader to
/// infer it from the scenario name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageConfig {
    /// The storage endpoint the workflow partitions the executors from, as a
    /// hostname. Chaos Mesh resolves it in the controller, so this is the same
    /// string that appears in the NetworkChaos manifest's `externalTargets`.
    pub endpoint: String,
    /// What this scenario expects its cut to do to the workload, and therefore
    /// what the run treats as evidence the cut landed.
    ///
    /// Deliberately per-scenario rather than one shared rule. The *account* the
    /// driver produces is shared because it is factual: throughput per stream
    /// per window, quiet time, latency, what was caught in flight. The
    /// *verdict* is not, because what a cut is supposed to do depends on what
    /// it cut, and a rule that reads correctly for one arrangement can be
    /// confidently wrong about another. See [`OutageExpectation`].
    pub expect: OutageExpectation,
    /// What serving again may cost once the storage is reachable, and the
    /// number each stream's recovery gap is reported against. Recorded rather
    /// than asserted, like every other budget in the suite: how long a
    /// connection pool may take to notice its database came back is a
    /// judgement, and the number is in the result either way.
    pub recovery_budget_secs: u64,
}

/// What a storage cut is expected to do to the workload.
///
/// The two variants exist because the storage scenarios cut two different
/// *kinds* of thing, not two instances of one thing. S16, S22 and S14 each take
/// away a database every stream depends on, so "did anything keep serving" is a
/// sound test of whether the fault landed. S18 takes away a cache only part of
/// the workload touches, and under that arrangement the same rule is not merely
/// too strict — it is backwards. It reported S18's first run as an outage that
/// never happened while the partition had plainly landed, because `durable`
/// held 100% of its baseline throughout, exactly as the design says it should.
///
/// Parameterising the old rule with a stream list would have fixed that one
/// message and left the deeper problem: for a partial cut, the streams that
/// *keep working* carry as much information as the ones that stop. If `durable`
/// had gone quiet under S18, the status blob would not be off the commit path
/// the way `AgentStatusFlusher` claims, and the shared rule would have called
/// that a clean pass. So the partial variant asserts both halves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum OutageExpectation {
    /// Everything is behind the cut, so every stream that was serving before it
    /// must fall silent. S16, S22 and S14.
    WholeWorkload {
        /// The least of the fault window each stream must answer nothing at all
        /// for, as a percentage of that window.
        ///
        /// This used to be a ceiling on during-fault throughput as a share of
        /// baseline, and that number is still recorded. It stopped being the
        /// verdict because it is a rate averaged over the whole window while
        /// all the serving in an absorbed outage happens in the seconds at its
        /// edges, so the same handful of edge confirmations reads as 8% of a
        /// 180s window and 26% of a 60s one. The threshold then tracks the
        /// window length rather than the platform, and shortening S16's window
        /// to 60s duly tripped it on a partition that had plainly landed.
        ///
        /// Quiet time has no such coupling: it is measured against the window's
        /// own edges, so it means the same thing whatever the window is.
        quiet_floor_percent: f64,
    },
    /// Only part of the workload is behind the cut. The named streams must fall
    /// silent and every other stream that was serving before it must keep
    /// serving. S18.
    PartialWorkload {
        /// The streams this cut is expected to stop. Their silence is the
        /// evidence the fault landed, so naming a stream the workload never
        /// drives would leave the run with no such evidence and no complaint —
        /// which `require_storage` refuses at load time rather than discovering
        /// during a maintenance window.
        silenced: Vec<Stream>,
        /// As [`Self::WholeWorkload::quiet_floor_percent`], but applied only to
        /// the streams named above.
        quiet_floor_percent: f64,
        /// The least of its own baseline rate an *unnamed* stream must still
        /// hold during the cut.
        ///
        /// This is the half a shared rule cannot express. A partial cut makes a
        /// claim about the streams it does not touch, and that claim is worth
        /// asserting: it is derived from where the code routes each namespace,
        /// so a stream stalling here says the routing is not what the source
        /// says it is.
        serving_floor_percent: f64,
    },
    /// The store is still reachable, only slow. Nothing is expected to fall
    /// silent at all, so the evidence the fault landed is latency rather than
    /// absence. S15 and S17.
    ///
    /// Worth being explicit that this is a different *question*, not a milder
    /// version of the other two. Those ask whether the platform survives losing
    /// a store. This asks whether it degrades or breaks when one gets slower,
    /// and "it went quiet" would be a failure here rather than the expected
    /// result.
    LatencyDegradation {
        /// The streams whose latency must rise for the injected delay to have
        /// landed. Their slowdown plays the part silence plays elsewhere: it is
        /// the only positive evidence the run has that anything happened.
        ///
        /// May be empty, and empty is a claim rather than an omission: a run
        /// that delays a store and names no stream to feel it is a control, so
        /// the absence of a slowdown is its result. S15A.
        slowed: Vec<Stream>,
        /// The least each named stream's during-fault median latency must be as
        /// a multiple of its own before-fault median.
        ///
        /// A multiple rather than a millisecond figure, so the threshold does
        /// not have to be re-derived per stream. Streams differ by an order of
        /// magnitude at rest — S18's baselines ran from 56ms on `durable` to
        /// 240ms on `ephemeral` — and a fixed ceiling would be slack for one
        /// and impossible for the other.
        slowdown_floor: f64,
        /// The streams the delay is expected *not* to reach, and whose
        /// steadiness is therefore the run's evidence that it reached only what
        /// it aimed at.
        ///
        /// The latency counterpart to what
        /// [`Self::PartialWorkload::serving_floor_percent`] does for a cut, and
        /// it earns its place for the same reason. Both halves of the key-value
        /// layer are delayed by a scenario in this suite, and each predicts a
        /// different split: S17 slows the Redis half, where only a lifecycle
        /// boundary crosses synchronously, so `ephemeral` moves and the other
        /// three do not; S15 slows the PostgreSQL half, where the running-workers
        /// recovery index, the promise keys and the scheduler's own schema all
        /// live, so those three move and `ephemeral` does not. Naming the far
        /// side turns each run into a test of that routing rather than a
        /// measurement taken on faith.
        ///
        /// Optional, and empty means the run asserts nothing about the streams
        /// it did not name.
        #[serde(default)]
        steady: Vec<Stream>,
        /// The most a steady stream's during-fault median may be as a multiple
        /// of its own before-fault median.
        ///
        /// Not 1.0. A stream on the far side of the delay still shares
        /// executors, connection pools and a tokio runtime with the streams on
        /// the near side, so some spill is expected and is not itself a finding.
        /// The number has to sit above that spill and below the multiple a real
        /// dependency would produce, and those are far apart: S17 measured 6.59x
        /// on the stream it delayed and no movement at all — 61ms, 100ms and
        /// 91ms, identical across all three windows — on the three it did not.
        #[serde(default = "default_steady_ceiling")]
        steady_ceiling: f64,
        /// The least of its own baseline rate *every* stream must still hold,
        /// including the slowed ones.
        ///
        /// The claim that separates degradation from breakage. A delay should
        /// make work take longer, not stop it, so a stream falling to nothing
        /// here means the added latency cost more than time — a timeout fired,
        /// a pool drained, a queue filled — and that is worth failing on.
        serving_floor_percent: f64,
    },
}

/// Applied when a `latency-degradation` expectation names steady streams
/// without saying how steady. Loose enough that ordinary run-to-run spread on
/// an undelayed stream does not trip it, which matters because the finding it
/// raises says the routing is wrong.
fn default_steady_ceiling() -> f64 {
    1.5
}

impl OutageExpectation {
    /// The quiet floor, which both variants carry and apply to a different set.
    pub fn quiet_floor_percent(&self) -> f64 {
        match self {
            OutageExpectation::WholeWorkload {
                quiet_floor_percent,
            }
            | OutageExpectation::PartialWorkload {
                quiet_floor_percent,
                ..
            } => *quiet_floor_percent,
            // Nothing is expected to go quiet, so there is no floor to apply
            // and reporting one would invite a reader to check it.
            OutageExpectation::LatencyDegradation { .. } => 0.0,
        }
    }

    /// Whether this stream is one whose slowdown would prove a delay landed.
    pub fn expects_slowdown(&self, stream: Stream) -> bool {
        match self {
            OutageExpectation::LatencyDegradation { slowed, .. } => slowed.contains(&stream),
            _ => false,
        }
    }

    /// Whether this is a delay that names no stream to feel it.
    ///
    /// True only of a control run. Worth its own accessor because
    /// [`Self::slowdown_floor`] is `Some` for every delay, empty `slowed` or
    /// not, so testing the floor cannot tell a control run apart from a run
    /// whose slowed streams produced no latency to compare.
    pub fn names_no_slowed_stream(&self) -> bool {
        matches!(
            self,
            OutageExpectation::LatencyDegradation { slowed, .. } if slowed.is_empty()
        )
    }

    /// The multiple of its own baseline median a slowed stream must reach, or
    /// `None` where the expectation makes no claim about latency.
    pub fn slowdown_floor(&self) -> Option<f64> {
        match self {
            OutageExpectation::LatencyDegradation { slowdown_floor, .. } => Some(*slowdown_floor),
            _ => None,
        }
    }

    /// Whether this stream is one the delay is expected not to reach.
    pub fn expects_steady(&self, stream: Stream) -> bool {
        match self {
            OutageExpectation::LatencyDegradation { steady, .. } => steady.contains(&stream),
            _ => false,
        }
    }

    /// The multiple of its own baseline median a steady stream may not exceed,
    /// or `None` where the expectation names no steady streams to hold to it.
    pub fn steady_ceiling(&self) -> Option<f64> {
        match self {
            OutageExpectation::LatencyDegradation {
                steady,
                steady_ceiling,
                ..
            } if !steady.is_empty() => Some(*steady_ceiling),
            _ => None,
        }
    }

    /// Whether this stream is one whose silence would prove the cut landed.
    ///
    /// `WholeWorkload` says yes to everything, which is what makes it the
    /// stricter rule rather than merely a different one.
    pub fn expects_silence(&self, stream: Stream) -> bool {
        match self {
            OutageExpectation::WholeWorkload { .. } => true,
            OutageExpectation::PartialWorkload { silenced, .. } => silenced.contains(&stream),
            OutageExpectation::LatencyDegradation { .. } => false,
        }
    }

    /// The floor an unnamed stream's during-fault throughput must clear, or
    /// `None` where the expectation makes no claim about streams still serving.
    pub fn serving_floor_percent(&self) -> Option<f64> {
        match self {
            OutageExpectation::WholeWorkload { .. } => None,
            OutageExpectation::PartialWorkload {
                serving_floor_percent,
                ..
            }
            | OutageExpectation::LatencyDegradation {
                serving_floor_percent,
                ..
            } => Some(*serving_floor_percent),
        }
    }
}

impl StorageConfig {
    pub fn recovery_budget(&self) -> Duration {
        Duration::from_secs(self.recovery_budget_secs)
    }
}

/// One step of the executor scale schedule the workflow runs during the fault.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScaleStep {
    /// How far into the fault window to run this step, as a fraction of it.
    pub after_fraction: f64,
    pub to_replicas: u32,
}

/// The executor scale schedule — S1's second traffic generator (GOL-364).
///
/// S12 drives four streams of invocations and S8 drives one pinned stream, but
/// none of that traffic goes anywhere near the shard-manager: invocations run
/// client → worker-service → executor. S1 needs traffic on the link it
/// partitions, and there are exactly two kinds.
///
/// The quota stream covers executor → shard-manager. This covers the other
/// direction: **removing an executor forces a revoke and reassign, adding one
/// back forces a register and rebalance**, and both are shard-manager →
/// executor calls that a partition can block.
///
/// Scaling *down* and back up rather than up and down is what makes this fit
/// the cluster: an executor requests 13Gi against 16 GiB nodes, so exactly one
/// fits per node, and the worker-exec nodegroup is pinned at two with no
/// autoscaler. A third replica would sit `Pending` for the whole run and
/// generate no traffic at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScaleDuringFaultConfig {
    pub steps: Vec<ScaleStep>,
}

/// Settings for the shard-ownership oracle (GOL-364).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipConfig {
    /// How long to wait after the fault is reported healed before taking the
    /// sample the run is *judged* on.
    ///
    /// This is the one number that decides what the scenario actually tests.
    /// Too short and it measures a rebalance in progress, where transient
    /// disagreement is normal and a verdict would be noise. Long enough and it
    /// measures the state the cluster settled into, which is the only state
    /// worth asserting about.
    pub settle_secs: u64,
}

impl OwnershipConfig {
    pub fn settle(&self) -> Duration {
        Duration::from_secs(self.settle_secs)
    }
}

/// One scenario's entry in the suite YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioConfig {
    pub code: String,
    pub name: String,
    /// The operational switch. `false` means the workflow skips this scenario
    /// entirely — the YAML, not the code, decides what a run does.
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub fault: FaultConfig,
    pub phases: PhaseConfig,
    /// The continuous mixed workload. Absent for scenarios that do not run one.
    #[serde(default)]
    pub workload: Option<WorkloadConfig>,
    /// The pinned in-flight workload. Absent for scenarios that do not run one.
    #[serde(default)]
    pub pinned: Option<PinnedConfig>,
    /// The scheduled-registration workload. Absent for scenarios that do not
    /// run one.
    #[serde(default)]
    pub scheduled: Option<ScheduledConfig>,
    /// The suspended-waiter workload. Absent for scenarios that do not run one.
    #[serde(default)]
    pub promise: Option<PromiseConfig>,
    /// The reachability workload. Absent for scenarios that do not run one.
    #[serde(default)]
    pub isolation: Option<IsolationConfig>,
    /// The revert workload. Absent for scenarios that do not run one.
    #[serde(default)]
    pub revert: Option<RevertConfig>,
    /// The deletion workload. Absent for scenarios that do not run one.
    #[serde(default)]
    pub delete: Option<DeleteConfig>,
    /// The component rollback. Absent for scenarios that do not run one.
    #[serde(default)]
    pub rollback: Option<RollbackConfig>,
    /// The cross-pod RPC control. Absent for scenarios that do not run one.
    #[serde(default)]
    pub relay: Option<RelayConfig>,
    /// Storage-outage settings. Absent for scenarios that do not take a
    /// storage dependency away.
    #[serde(default)]
    pub storage: Option<StorageConfig>,
    /// Shard-ownership oracle settings. Absent for scenarios that do not sample
    /// executor assignments.
    #[serde(default)]
    pub ownership: Option<OwnershipConfig>,
    /// Asks the workflow to scale executors mid-fault. Absent for scenarios
    /// that do not.
    #[serde(default)]
    pub scale_during_fault: Option<ScaleDuringFaultConfig>,
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// How long the driver waits for each workflow signal before aborting.
    /// Generous by default: it only has to be shorter than the maintenance
    /// window, and a premature abort wastes the whole window.
    #[serde(default = "default_signal_timeout_secs")]
    pub signal_timeout_secs: u64,
}

fn default_signal_timeout_secs() -> u64 {
    1800
}

impl ScenarioConfig {
    pub fn signal_timeout(&self) -> Duration {
        Duration::from_secs(self.signal_timeout_secs)
    }

    pub fn scenario_code(&self) -> anyhow::Result<ScenarioCode> {
        ScenarioCode::parse(&self.code)
            .ok_or_else(|| anyhow::anyhow!("unknown chaos scenario code {:?}", self.code))
    }

    /// The mixed workload block, which the scenarios that run one require.
    /// A missing block is a YAML mistake rather than an empty workload, so it
    /// fails loudly instead of running a scenario against nothing.
    pub fn require_workload(&self) -> anyhow::Result<&WorkloadConfig> {
        self.workload.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `workload` block in the suite YAML",
                self.code
            )
        })
    }

    /// The ownership-oracle block. See [`Self::require_workload`].
    pub fn require_ownership(&self) -> anyhow::Result<&OwnershipConfig> {
        self.ownership.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs an `ownership` block in the suite YAML",
                self.code
            )
        })
    }

    /// The scheduled-registration block. See [`Self::require_workload`].
    pub fn require_scheduled(&self) -> anyhow::Result<&ScheduledConfig> {
        self.scheduled.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `scheduled` block in the suite YAML",
                self.code
            )
        })
    }

    /// The suspended-waiter block. See [`Self::require_workload`].
    pub fn require_promise(&self) -> anyhow::Result<&PromiseConfig> {
        self.promise.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `promise` block in the suite YAML",
                self.code
            )
        })
    }

    /// The reachability workload block. See [`Self::require_workload`].
    pub fn require_isolation(&self) -> anyhow::Result<&IsolationConfig> {
        self.isolation.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs an `isolation` block in the suite YAML",
                self.code
            )
        })
    }

    /// The revert workload block. See [`Self::require_workload`].
    pub fn require_revert(&self) -> anyhow::Result<&RevertConfig> {
        let config = self.revert.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `revert` block in the suite YAML",
                self.code
            )
        })?;
        // Checked here rather than discovered mid-run: a revert reaching past
        // its own round lands in an already-deleted oplog region and the
        // platform refuses it, so every round would fail and the scenario would
        // measure nothing.
        if config.revert_invocations > config.increments_per_round {
            anyhow::bail!(
                "chaos scenario {}: revertInvocations ({}) exceeds incrementsPerRound ({}), \
                 so every revert would reach into an already-deleted oplog region",
                self.code,
                config.revert_invocations,
                config.increments_per_round
            );
        }
        if config.revert_invocations == 0 {
            anyhow::bail!(
                "chaos scenario {}: revertInvocations is 0, so the scenario would revert nothing",
                self.code
            );
        }
        Ok(config)
    }

    /// The deletion workload block. See [`Self::require_workload`].
    pub fn require_delete(&self) -> anyhow::Result<&DeleteConfig> {
        let config = self.delete.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `delete` block in the suite YAML",
                self.code
            )
        })?;
        // At one increment the two legal answers are adjacent — 1 and 2 — so
        // nothing can land between them and the partial-state violation is
        // structurally unobservable. A third of the oracle would be blind, and
        // every run would look clean on that axis by construction.
        if config.increments_per_round < 2 {
            anyhow::bail!(
                "chaos scenario {}: incrementsPerRound is {}, which leaves no value between \
                 a fresh agent and a survivor, so a partial state could never be observed",
                self.code,
                config.increments_per_round
            );
        }
        Ok(config)
    }

    /// The rollback block. See [`Self::require_workload`].
    pub fn require_rollback(&self) -> anyhow::Result<&RollbackConfig> {
        self.rollback.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `rollback` block in the suite YAML",
                self.code
            )
        })
    }

    /// The cross-pod RPC control block. See [`Self::require_workload`].
    pub fn require_relay(&self) -> anyhow::Result<&RelayConfig> {
        self.relay.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `relay` block in the suite YAML",
                self.code
            )
        })
    }

    /// Whether this scenario's configuration actually produces operations on a
    /// stream.
    ///
    /// Counts the blocks that drive traffic rather than the streams a run
    /// happens to record, so it can be answered from the YAML alone before any
    /// cluster time is spent. `Scheduled` has two possible sources and either
    /// one counts; the streams that belong to other scenario shapes are not
    /// reachable from a `storage` scenario's config at all.
    fn drives_stream(&self, stream: Stream) -> bool {
        let workload = self.workload.as_ref();
        match stream {
            Stream::Durable => workload.is_some_and(|w| w.durable_agents > 0),
            Stream::Ephemeral => workload.is_some_and(|w| w.ephemeral_agents > 0),
            Stream::Promise => workload.is_some_and(|w| w.promise_agents > 0),
            Stream::Quota => workload.is_some_and(|w| w.quota_agents > 0),
            Stream::Rpc => workload.is_some_and(|w| w.rpc_agents > 0),
            Stream::Scheduled => {
                workload.is_some_and(|w| w.scheduled_agents > 0)
                    || self.scheduled.as_ref().is_some_and(|s| s.targets > 0)
            }
            Stream::PinnedHttp | Stream::PromiseWait | Stream::Delete | Stream::Revert => false,
        }
    }

    /// A floor of 100% can never be met: a stream is only quiet between the
    /// answers it did give, and an operation submitted just before the heal
    /// confirms just after it, inside the window it was submitted in. Every run
    /// would then report the outage as not observed, and the one verdict that
    /// exists to catch a fault which never landed would be stuck on. A floor of
    /// zero is the opposite failure: every run passes, including the ones where
    /// nothing was ever cut off.
    fn check_quiet_floor(&self, floor: f64) -> anyhow::Result<()> {
        if !(0.0..100.0).contains(&floor) || floor <= 0.0 {
            anyhow::bail!(
                "chaos scenario {}: expect.quietFloorPercent is {floor}, which is not a share \
                 of the fault window a real outage could be judged by",
                self.code
            );
        }
        Ok(())
    }

    fn check_serving_floor(&self, floor: f64) -> anyhow::Result<()> {
        if !(0.0..=100.0).contains(&floor) || floor <= 0.0 {
            anyhow::bail!(
                "chaos scenario {}: expect.servingFloorPercent is {floor}, which is not a share \
                 of a stream's own baseline it could be held to",
                self.code
            );
        }
        Ok(())
    }

    /// The streams whose behaviour is the run's only positive evidence that the
    /// fault landed at all.
    ///
    /// Naming one the workload never drives leaves the run with no such
    /// evidence and nothing to complain about — the exact failure the floors
    /// exist to prevent, reintroduced one level up. Refused at load time so it
    /// costs a build rather than a maintenance window.
    fn check_evidence_streams(
        &self,
        field: &str,
        streams: &[Stream],
        evidence: &str,
    ) -> anyhow::Result<()> {
        if streams.is_empty() {
            anyhow::bail!(
                "chaos scenario {}: expect.{field} is empty, so nothing in the run would show \
                 whether the fault landed at all",
                self.code
            );
        }
        for stream in streams {
            if !self.drives_stream(*stream) {
                anyhow::bail!(
                    "chaos scenario {}: expect.{field} names `{stream}`, which this scenario's \
                     workload never drives, so its {evidence} during the fault would prove \
                     nothing",
                    self.code
                );
            }
        }
        Ok(())
    }

    /// The storage-outage block. See [`Self::require_workload`].
    pub fn require_storage(&self) -> anyhow::Result<&StorageConfig> {
        let config = self.storage.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `storage` block in the suite YAML",
                self.code
            )
        })?;
        // A floor of 100% can never be met: a stream is only quiet between the
        // answers it did give, and an operation submitted just before the heal
        // confirms just after it, inside the window it was submitted in. Every
        // run would then report the outage as not observed, and the one verdict
        // that exists to catch a fault which never landed would be stuck on. A
        // floor of zero is the opposite failure: every run passes, including
        // the ones where nothing was ever cut off.
        // Checked per variant rather than through the accessors, because the
        // variants do not share a set of knobs and a single pass over "whatever
        // is present" would silently skip whichever one this scenario actually
        // relies on.
        match &config.expect {
            OutageExpectation::WholeWorkload {
                quiet_floor_percent,
            } => {
                self.check_quiet_floor(*quiet_floor_percent)?;
            }
            OutageExpectation::PartialWorkload {
                silenced,
                quiet_floor_percent,
                serving_floor_percent,
            } => {
                self.check_quiet_floor(*quiet_floor_percent)?;
                self.check_serving_floor(*serving_floor_percent)?;
                self.check_evidence_streams("silenced", silenced, "silence")?;
            }
            OutageExpectation::LatencyDegradation {
                slowed,
                slowdown_floor,
                steady,
                steady_ceiling,
                serving_floor_percent,
            } => {
                // A floor of 1.0 or less asks a stream to be no slower than it
                // already was, which every run satisfies including one where
                // the delay never applied. That is the same hole a zero quiet
                // floor leaves, in the units this variant uses.
                if !slowdown_floor.is_finite() || *slowdown_floor <= 1.0 {
                    anyhow::bail!(
                        "chaos scenario {}: expect.slowdownFloor is {slowdown_floor}, and a \
                         factor at or below 1.0 is met by a run where the delay never applied",
                        self.code
                    );
                }
                self.check_serving_floor(*serving_floor_percent)?;
                // A run may name no slowed streams at all, but only if it names
                // steady ones — and then it is a control rather than an
                // experiment. S15A drives `ephemeral` alone, which reaches none
                // of the delayed store, so *nothing* in its workload should
                // move and there is no slowdown available as evidence that the
                // fault landed.
                //
                // The cost is real and worth stating: such a run cannot fail on
                // `slowdown-not-observed`, so it cannot tell a delay that
                // landed and did not matter from one that never applied. That
                // evidence has to come from outside the workload, and for S15A
                // it does — the scheduler polls its own schema on the delayed
                // cluster whether or not anything is scheduled, so
                // `db_success_seconds{svc="scheduler_storage"}` moves under the
                // fault regardless. The runbook says to check it.
                if slowed.is_empty() && steady.is_empty() {
                    anyhow::bail!(
                        "chaos scenario {}: expect.slowed and expect.steady are both empty, so \
                         nothing in the run would show whether the fault landed or whether it \
                         mattered",
                        self.code
                    );
                }
                if !slowed.is_empty() {
                    self.check_evidence_streams("slowed", slowed, "slowdown")?;
                }
                // The steady list is optional, so it is checked only when
                // present — but a present one carries the same two ways of
                // being useless as the slowed list, plus one of its own.
                for stream in steady {
                    if !self.drives_stream(*stream) {
                        anyhow::bail!(
                            "chaos scenario {}: expect.steady names `{stream}`, which this \
                             scenario's workload never drives, so its steadiness during the \
                             fault would prove nothing",
                            self.code
                        );
                    }
                    if slowed.contains(stream) {
                        anyhow::bail!(
                            "chaos scenario {}: expect names `{stream}` as both slowed and \
                             steady, so the run would demand the same stream both move and \
                             stay put",
                            self.code
                        );
                    }
                }
                // A ceiling at or below 1.0 asks an undelayed stream to run no
                // slower than its own baseline median, which run-to-run spread
                // alone breaks. The finding it would then raise says the
                // platform routes a namespace somewhere other than where the
                // source says, and that is far too strong a claim to make on
                // noise.
                if !steady.is_empty() && (!steady_ceiling.is_finite() || *steady_ceiling <= 1.0) {
                    anyhow::bail!(
                        "chaos scenario {}: expect.steadyCeiling is {steady_ceiling}, and a \
                         factor at or below 1.0 is broken by ordinary run-to-run spread on a \
                         stream the delay never reached",
                        self.code
                    );
                }
            }
        }
        if config.endpoint.trim().is_empty() {
            anyhow::bail!(
                "chaos scenario {}: storage.endpoint is empty, so the result could not say which \
                 storage the run took away",
                self.code
            );
        }
        // Checked here rather than in the driver so a bad YAML fails the build
        // instead of a maintenance window. Both blocks write to the scheduled
        // stream, but only the `scheduled` block's registrations carry a token
        // into the target's fire log — see `ScheduleEmitter::schedule_fire_at`
        // in the counters component. Driving both would leave the fire account
        // pairing the mixed workload's tokenless registrations against nothing
        // and reporting every one of them as an action that never ran.
        if let (Some(workload), Some(_)) = (&self.workload, &self.scheduled)
            && workload.scheduled_agents > 0
        {
            anyhow::bail!(
                "chaos scenario {}: the mixed workload drives {} scheduled agents while a \
                 `scheduled` block is also present, and only the latter's registrations carry a \
                 token into the fire log, so the fire account would report every mixed-workload \
                 registration as one that never fired",
                self.code,
                workload.scheduled_agents
            );
        }
        Ok(config)
    }

    /// The pinned workload block. See [`Self::require_workload`].
    pub fn require_pinned(&self) -> anyhow::Result<&PinnedConfig> {
        self.pinned.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "chaos scenario {} needs a `pinned` block in the suite YAML",
                self.code
            )
        })
    }
}

/// The suite YAML: the authoritative operational switchboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChaosSuite {
    pub name: String,
    pub scenarios: Vec<ScenarioConfig>,
}

impl ChaosSuite {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading chaos suite {:?}", path.as_ref()))?;
        let suite: ChaosSuite = serde_yaml::from_str(&raw)
            .with_context(|| format!("parsing chaos suite {:?}", path.as_ref()))?;
        Ok(suite)
    }

    /// The entry for `code`, which must exist and be enabled — the workflow is
    /// expected to have filtered disabled scenarios out already, so reaching one
    /// here means the two disagree and that is worth failing on.
    /// Looks a scenario up, refusing one the suite has switched off.
    ///
    /// `allow_disabled` is the caller saying an operator named this scenario
    /// deliberately. It exists for prototype scenarios, which are `enabled:
    /// false` so no ordinary run picks them up but must still be runnable on
    /// demand. Without it the two gates — this one and the workflow's — would
    /// disagree, and the workflow's would silently lose.
    pub fn scenario(
        &self,
        code: ScenarioCode,
        allow_disabled: bool,
    ) -> anyhow::Result<&ScenarioConfig> {
        let entry = self
            .scenarios
            .iter()
            .find(|s| s.code.eq_ignore_ascii_case(code.as_str()))
            .ok_or_else(|| anyhow::anyhow!("chaos suite has no entry for scenario {code}"))?;
        if !entry.enabled && !allow_disabled {
            anyhow::bail!(
                "chaos scenario {code} is disabled in the suite YAML \
                 (pass --allow-disabled to run it anyway)"
            );
        }
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// Path of the checked-in suite, resolved from the crate root so the test
    /// does not depend on the working directory.
    fn suite_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("chaos_suites/cloud-chaos.yaml")
    }

    /// The YAML is the operational switchboard and the registry is the code
    /// behind it. If they drift, an operator can enable something that does not
    /// exist, or a scenario can ship with no way to turn it off.
    #[test]
    fn suite_yaml_and_scenario_registry_are_in_sync() {
        let suite = ChaosSuite::load(suite_path()).expect("checked-in suite must parse");

        let mut yaml_codes: Vec<String> = suite
            .scenarios
            .iter()
            .map(|s| s.code.to_uppercase())
            .collect();
        yaml_codes.sort();
        let mut registry_codes: Vec<String> = ScenarioCode::ALL
            .into_iter()
            .map(|c| c.as_str().to_string())
            .collect();
        registry_codes.sort();

        assert_eq!(
            yaml_codes, registry_codes,
            "chaos_suites/cloud-chaos.yaml and ScenarioCode::ALL must list the same scenarios"
        );
    }

    /// A `pod-kill` is instantaneous and its `duration` only governs how long
    /// the object lingers, so those two numbers are free to differ. A
    /// `network-partition` is not: Chaos Mesh takes the iptables rules down on
    /// the object's own `duration`, while the workflow holds the phase for
    /// `faultSecs` and only then deletes it. A shorter `durationSecs` therefore
    /// heals the cluster part-way through the window the driver is still
    /// attributing to the fault, and every during-fault number is measured
    /// across a mix of the two states.
    #[test]
    fn a_partition_lasts_at_least_as_long_as_the_fault_phase_it_is_measured_over() {
        let suite = ChaosSuite::load(suite_path()).unwrap();
        for entry in &suite.scenarios {
            if entry.fault.kind != "network-partition" {
                continue;
            }
            assert!(
                entry.fault.duration_secs >= entry.phases.fault_secs,
                "{}: fault.durationSecs ({}) is shorter than phases.faultSecs ({}), \
                 so the partition lifts {}s before the fault phase ends and the rest \
                 of that phase measures a healed cluster",
                entry.code,
                entry.fault.duration_secs,
                entry.phases.fault_secs,
                entry.phases.fault_secs - entry.fault.duration_secs,
            );
        }
    }

    #[test]
    fn every_suite_entry_resolves_to_an_implemented_scenario() {
        let suite = ChaosSuite::load(suite_path()).unwrap();
        for entry in &suite.scenarios {
            entry
                .scenario_code()
                .unwrap_or_else(|e| panic!("suite entry {:?}: {e}", entry.name));
        }
    }

    #[test]
    fn looking_up_a_disabled_scenario_is_an_error_rather_than_a_silent_run() {
        let suite = ChaosSuite {
            name: "test".to_string(),
            scenarios: vec![ScenarioConfig {
                code: "S12".to_string(),
                name: "shard-manager-pod-restart".to_string(),
                enabled: false,
                fault: FaultConfig {
                    kind: "pod-kill".to_string(),
                    target: "shard-manager".to_string(),
                    mode: "one".to_string(),
                    target_count: None,
                    manifest: None,
                    duration_secs: 60,
                },
                phases: PhaseConfig {
                    baseline_secs: 1,
                    fault_secs: 1,
                    recovery_secs: 1,
                },
                workload: Some(WorkloadConfig {
                    durable_agents: 1,
                    ephemeral_agents: 1,
                    scheduled_agents: 1,
                    promise_agents: 1,
                    quota_agents: 1,
                    rpc_agents: 0,
                    rate_per_sec: 1,
                }),
                pinned: None,
                scheduled: None,
                promise: None,
                isolation: None,
                revert: None,
                delete: None,
                rollback: None,
                relay: None,
                storage: None,
                ownership: None,
                scale_during_fault: None,
                retry_policy: RetryPolicy::default(),
                signal_timeout_secs: 1,
            }],
        };
        assert!(suite.scenario(ScenarioCode::S12, false).is_err());
        // ...unless the caller says an operator asked for it by name, which is
        // how a prototype scenario stays off for ordinary runs.
        assert!(suite.scenario(ScenarioCode::S12, true).is_ok());
    }

    fn revert_config(increments: u32, revert: u32) -> ScenarioConfig {
        ScenarioConfig {
            code: "S7".to_string(),
            name: "executor-crash-during-revert".to_string(),
            enabled: true,
            fault: FaultConfig {
                kind: "pod-kill".to_string(),
                target: "worker-executor".to_string(),
                mode: "one".to_string(),
                target_count: None,
                manifest: None,
                duration_secs: 60,
            },
            phases: PhaseConfig {
                baseline_secs: 1,
                fault_secs: 1,
                recovery_secs: 1,
            },
            workload: None,
            pinned: None,
            scheduled: None,
            promise: None,
            isolation: None,
            delete: None,
            rollback: None,
            relay: None,
            storage: None,
            revert: Some(RevertConfig {
                agents: 10,
                increments_per_round: increments,
                revert_invocations: revert,
                interval_millis: 500,
                recovery_budget_secs: 60,
            }),
            ownership: None,
            scale_during_fault: None,
            retry_policy: RetryPolicy::default(),
            signal_timeout_secs: 1,
        }
    }

    /// A revert reaching further back than its own round lands in the region an
    /// earlier revert already deleted, and the platform refuses it outright.
    /// Every round would fail and the run would measure nothing, so this is
    /// caught before the maintenance window is spent rather than per round.
    #[test]
    fn a_revert_deeper_than_its_own_round_is_refused_before_the_run_starts() {
        let error = revert_config(2, 3)
            .require_revert()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("already-deleted oplog region"),
            "the message has to say why, got: {error}"
        );
    }

    /// Reverting nothing would leave a scenario that builds state up and takes
    /// none of it back, which is S8 with extra steps.
    #[test]
    fn a_revert_of_nothing_is_refused() {
        assert!(revert_config(4, 0).require_revert().is_err());
    }

    /// The boundary case is legal: a round may take back everything it added.
    #[test]
    fn a_revert_of_exactly_one_round_is_allowed() {
        let config = revert_config(3, 3);
        assert_eq!(config.require_revert().unwrap().net_per_round(), 0);
    }

    fn storage_config(
        endpoint: &str,
        quiet_floor: f64,
        scheduled_agents: u32,
        scheduled_block: bool,
    ) -> ScenarioConfig {
        ScenarioConfig {
            code: "S16".to_string(),
            name: "keyvalue-postgres-outage".to_string(),
            enabled: true,
            fault: FaultConfig {
                kind: "network-partition".to_string(),
                target: "worker-executor".to_string(),
                mode: "all".to_string(),
                target_count: None,
                manifest: None,
                duration_secs: 180,
            },
            phases: PhaseConfig {
                baseline_secs: 1,
                fault_secs: 1,
                recovery_secs: 1,
            },
            workload: Some(WorkloadConfig {
                durable_agents: 10,
                ephemeral_agents: 0,
                scheduled_agents,
                promise_agents: 0,
                quota_agents: 0,
                rpc_agents: 0,
                rate_per_sec: 10,
            }),
            pinned: None,
            scheduled: scheduled_block.then_some(ScheduledConfig {
                targets: 10,
                interval_millis: 2000,
                lead_secs: 10,
                lease_budget_secs: 240,
            }),
            promise: None,
            isolation: None,
            delete: None,
            rollback: None,
            relay: None,
            storage: Some(StorageConfig {
                endpoint: endpoint.to_string(),
                expect: OutageExpectation::WholeWorkload {
                    quiet_floor_percent: quiet_floor,
                },
                recovery_budget_secs: 120,
            }),
            revert: None,
            ownership: None,
            scale_during_fault: None,
            retry_policy: RetryPolicy::default(),
            signal_timeout_secs: 1,
        }
    }

    /// A floor of zero passes every run, including one where nothing was cut
    /// off, so the verdict that exists to catch a fault which never landed
    /// would never fire.
    #[test]
    fn a_storage_fault_quiet_floor_of_zero_is_refused() {
        let error = storage_config("db.example", 0.0, 0, true)
            .require_storage()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("expect.quietFloorPercent"),
            "the message has to name the knob, got: {error}"
        );
    }

    /// A stream is only quiet between the answers it did give, so no real
    /// outage can be quiet for the whole window and the verdict would be stuck
    /// on for every run.
    #[test]
    fn a_storage_fault_quiet_floor_of_a_whole_window_is_refused() {
        assert!(
            storage_config("db.example", 100.0, 0, true)
                .require_storage()
                .is_err(),
            "a floor of the whole window can never be met"
        );
    }

    /// A control run: no stream is expected to slow, and the one named steady
    /// is what the run is actually asking about. Allowed, because a series that
    /// isolates an interaction needs a run with nothing in it to interact.
    #[test]
    fn a_latency_run_that_names_only_steady_streams_is_allowed() {
        let mut config = storage_config("db.example", 50.0, 0, true);
        config.workload.as_mut().unwrap().ephemeral_agents = 10;
        config.storage.as_mut().unwrap().expect = OutageExpectation::LatencyDegradation {
            slowed: Vec::new(),
            slowdown_floor: 2.0,
            steady: vec![Stream::Ephemeral],
            steady_ceiling: 1.5,
            serving_floor_percent: 50.0,
        };
        assert!(
            config.require_storage().is_ok(),
            "a run may assert that nothing moved, as long as it names what should not have"
        );
    }

    /// Both lists empty is the one shape that says nothing at all: no stream
    /// would show the fault landing, and none would show it mattering either.
    #[test]
    fn a_latency_run_that_names_no_streams_at_all_is_refused() {
        let mut config = storage_config("db.example", 50.0, 0, true);
        config.storage.as_mut().unwrap().expect = OutageExpectation::LatencyDegradation {
            slowed: Vec::new(),
            slowdown_floor: 2.0,
            steady: Vec::new(),
            steady_ceiling: 1.5,
            serving_floor_percent: 50.0,
        };
        let error = config.require_storage().unwrap_err().to_string();
        assert!(
            error.contains("expect.slowed and expect.steady are both empty"),
            "the message has to say which pair is missing, got: {error}"
        );
    }

    /// Without an endpoint the archived result could not say which storage the
    /// run took away.
    #[test]
    fn a_storage_block_with_no_endpoint_is_refused() {
        assert!(
            storage_config("   ", 50.0, 0, true)
                .require_storage()
                .is_err()
        );
    }

    /// Both blocks write to the scheduled stream and only one of them carries a
    /// token into the fire log, so driving both would report every tokenless
    /// registration as an action that never ran. Caught at load time rather
    /// than discovered in the report.
    #[test]
    fn driving_the_scheduled_stream_from_both_blocks_is_refused() {
        let error = storage_config("db.example", 15.0, 50, true)
            .require_storage()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("token into the fire log"),
            "the message has to say why, got: {error}"
        );
    }

    /// The same mixed-workload setting is fine on its own: without a
    /// `scheduled` block there is only one writer to the stream.
    #[test]
    fn scheduled_agents_alone_are_allowed() {
        assert!(
            storage_config("db.example", 15.0, 50, false)
                .require_storage()
                .is_ok()
        );
    }

    /// The retry defaults are load-bearing for correctness, not just for load.
    #[test]
    fn retry_policy_defaults_to_one_same_key_transport_only_retry() {
        let policy = RetryPolicy::default();
        assert!(policy.transport_only);
        assert_eq!(policy.max_retries, 1);
        assert_eq!(policy.delay_secs, 5);
    }

    /// The property the whole resume path depends on: two invocations must not
    /// produce the same keys, or the second replays the first's results.
    #[test]
    fn a_run_nonce_makes_the_key_prefix_unique_per_invocation() {
        let first = key_prefix_with_nonce(ScenarioCode::S8, Some("32093963216-1"));
        let second = key_prefix_with_nonce(ScenarioCode::S8, Some("32093963216-2"));

        assert_eq!(first, "chaos-s8-32093963216-1");
        assert_ne!(first, second, "a second attempt must not reuse the keys");
    }

    /// The documented trace queries key on `chaos-<code>-`, so the nonce has to
    /// go after the code rather than in front of it.
    #[test]
    fn the_key_prefix_still_starts_with_the_scenario_code() {
        let prefix = key_prefix_with_nonce(ScenarioCode::S12, Some("run-7"));
        assert!(
            prefix.starts_with("chaos-s12-"),
            "{prefix} must still match the documented chaos-s12-.* query"
        );
    }

    /// A local run against a fresh cluster has nothing to collide with, and an
    /// unset or unusable nonce must not produce a trailing separator.
    #[test]
    fn an_absent_or_unusable_nonce_falls_back_to_the_bare_code() {
        for nonce in [None, Some(""), Some("   "), Some("///"), Some("-")] {
            assert_eq!(
                key_prefix_with_nonce(ScenarioCode::S1, nonce),
                "chaos-s1",
                "nonce {nonce:?} should have fallen back to the bare code"
            );
        }
    }

    /// Agent ids end up in URLs and Grafana queries, so a nonce carrying
    /// anything else must be stripped rather than passed through.
    #[test]
    fn a_nonce_is_reduced_to_characters_that_are_safe_in_an_agent_id() {
        assert_eq!(
            key_prefix_with_nonce(ScenarioCode::S8, Some("run/42 attempt#2")),
            "chaos-s8-run42attempt2"
        );
    }

    #[test]
    fn scenario_codes_parse_case_insensitively() {
        assert_eq!(ScenarioCode::parse("s12"), Some(ScenarioCode::S12));
        assert_eq!(ScenarioCode::parse("S12"), Some(ScenarioCode::S12));
        assert_eq!(ScenarioCode::parse("s8"), Some(ScenarioCode::S8));
        assert_eq!(ScenarioCode::parse("s1"), Some(ScenarioCode::S1));
        assert_eq!(ScenarioCode::parse("s3"), Some(ScenarioCode::S3));
        assert_eq!(ScenarioCode::parse("s7"), Some(ScenarioCode::S7));
        assert_eq!(ScenarioCode::parse("s6"), Some(ScenarioCode::S6));
        assert_eq!(ScenarioCode::parse("s9"), Some(ScenarioCode::S9));
        assert_eq!(ScenarioCode::parse("s16"), Some(ScenarioCode::S16));
        assert_eq!(ScenarioCode::parse("s14"), Some(ScenarioCode::S14));
        assert_eq!(ScenarioCode::parse("s18"), Some(ScenarioCode::S18));
        assert_eq!(ScenarioCode::parse("s17"), Some(ScenarioCode::S17));
        assert_eq!(ScenarioCode::parse("s15"), Some(ScenarioCode::S15));
        assert_eq!(ScenarioCode::parse("s15a"), Some(ScenarioCode::S15A));
        assert_eq!(ScenarioCode::parse("s15b"), Some(ScenarioCode::S15B));
        assert_eq!(ScenarioCode::parse("s15c"), Some(ScenarioCode::S15C));
        assert_eq!(ScenarioCode::parse("s23"), Some(ScenarioCode::S23));
        assert_eq!(ScenarioCode::parse("S99"), None);
    }

    /// S2 is the only scenario that drives agent-to-agent traffic, and it is
    /// the only one that must.
    ///
    /// Asserted from the checked-in suite rather than a fixture because the
    /// failure this guards against is a configuration one: an S2 entry that
    /// lost its `rpcAgents` would still run, still produce two throughput
    /// cells, and still come back clean — of a fault that had nothing to cut.
    /// The pairing gate catches that at run time; this catches it at build time.
    #[test]
    fn only_s2_drives_the_rpc_stream() {
        let suite = ChaosSuite::load(suite_path()).unwrap();
        for entry in &suite.scenarios {
            let code = entry.scenario_code().unwrap();
            assert_eq!(
                entry.drives_stream(Stream::Rpc),
                code == ScenarioCode::S2,
                "{code} disagrees with the suite about whether it drives cross-pod RPC"
            );
        }
    }

    /// The gate that fails a run when nothing fired has to tell "no fire was
    /// observed" apart from "no fire was asked for". S15A's first run was lost
    /// to that distinction: it registers no targets, so the gate aborted it
    /// during the baseline, before the fault it exists to measure was injected.
    #[test]
    fn a_run_that_registers_no_scheduled_targets_does_not_drive_that_stream() {
        let suite = ChaosSuite::load(suite_path()).unwrap();
        let drives = |code: ScenarioCode| {
            suite
                .scenarios
                .iter()
                .find(|entry| entry.scenario_code().ok() == Some(code))
                .unwrap_or_else(|| panic!("{code} is missing from the suite"))
                .drives_stream(Stream::Scheduled)
        };
        for code in [ScenarioCode::S15A, ScenarioCode::S15B, ScenarioCode::S15C] {
            assert!(
                !drives(code),
                "{code} registers no targets, so the fire gate has nothing to gate"
            );
        }
        for code in [ScenarioCode::S15, ScenarioCode::S23] {
            assert!(
                drives(code),
                "{code} drives the scheduled loop, so the fire gate still applies to it"
            );
        }
    }

    /// A scenario whose YAML entry is missing the workload block it needs must
    /// say so rather than quietly running against nothing.
    #[test]
    fn a_scenario_missing_its_workload_block_fails_loudly() {
        let suite = ChaosSuite::load(suite_path()).unwrap();
        for entry in &suite.scenarios {
            match entry.scenario_code().unwrap() {
                ScenarioCode::S12 => {
                    entry.require_workload().unwrap();
                }
                ScenarioCode::S8 => {
                    entry.require_pinned().unwrap();
                }
                ScenarioCode::S1 => {
                    entry.require_workload().unwrap();
                    entry.require_ownership().unwrap();
                }
                ScenarioCode::S5 => {
                    entry.require_workload().unwrap();
                }
                ScenarioCode::S13 => {
                    entry.require_workload().unwrap();
                }
                ScenarioCode::S10 => {
                    entry.require_scheduled().unwrap();
                }
                ScenarioCode::S11 => {
                    entry.require_promise().unwrap();
                }
                ScenarioCode::S3 => {
                    entry.require_isolation().unwrap();
                }
                ScenarioCode::S2 => {
                    entry.require_workload().unwrap();
                    entry.require_relay().unwrap();
                    entry.require_ownership().unwrap();
                }
                ScenarioCode::S7 => {
                    entry.require_revert().unwrap();
                }
                ScenarioCode::S6 => {
                    entry.require_delete().unwrap();
                }
                ScenarioCode::S9 => {
                    entry.require_workload().unwrap();
                    entry.require_rollback().unwrap();
                }
                ScenarioCode::S14
                | ScenarioCode::S15
                | ScenarioCode::S15A
                | ScenarioCode::S15B
                | ScenarioCode::S15C
                | ScenarioCode::S16
                | ScenarioCode::S17
                | ScenarioCode::S18
                | ScenarioCode::S22
                | ScenarioCode::S23 => {
                    entry.require_workload().unwrap();
                    entry.require_scheduled().unwrap();
                    entry.require_storage().unwrap();
                }
            }
        }
    }
}
