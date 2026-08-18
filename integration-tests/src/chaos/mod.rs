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
//! `enabled` flag, and results are archived to S3 per scenario so an interrupted
//! run resumes rather than restarts.
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

pub mod errors;
pub mod history;
pub mod ownership;
pub mod pinned;
pub mod prep;
pub mod probe;
pub mod result;
pub mod scenarios;
pub mod signal;
pub mod summary;
pub mod workload;

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
    /// Shard-manager pod restart under mixed workload.
    S12,
}

impl ScenarioCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ScenarioCode::S1 => "S1",
            ScenarioCode::S8 => "S8",
            ScenarioCode::S12 => "S12",
        }
    }

    /// Every scenario this driver implements. The suite YAML is checked against
    /// this list, so a scenario cannot be enabled in YAML without code behind
    /// it, nor implemented without an operational switch in front of it.
    pub const ALL: [ScenarioCode; 3] = [ScenarioCode::S1, ScenarioCode::S8, ScenarioCode::S12];

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
    pub fn scenario(&self, code: ScenarioCode) -> anyhow::Result<&ScenarioConfig> {
        let entry = self
            .scenarios
            .iter()
            .find(|s| s.code.eq_ignore_ascii_case(code.as_str()))
            .ok_or_else(|| anyhow::anyhow!("chaos suite has no entry for scenario {code}"))?;
        if !entry.enabled {
            anyhow::bail!("chaos scenario {code} is disabled in the suite YAML");
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
                    rate_per_sec: 1,
                }),
                pinned: None,
                ownership: None,
                scale_during_fault: None,
                retry_policy: RetryPolicy::default(),
                signal_timeout_secs: 1,
            }],
        };
        assert!(suite.scenario(ScenarioCode::S12).is_err());
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
        assert_eq!(ScenarioCode::parse("S99"), None);
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
            }
        }
    }
}
