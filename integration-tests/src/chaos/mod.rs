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

pub mod history;
pub mod prep;
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
    /// Shard-manager pod restart under mixed workload.
    S12,
}

impl ScenarioCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ScenarioCode::S12 => "S12",
        }
    }

    /// Every scenario this driver implements. The suite YAML is checked against
    /// this list, so a scenario cannot be enabled in YAML without code behind
    /// it, nor implemented without an operational switch in front of it.
    pub const ALL: [ScenarioCode; 1] = [ScenarioCode::S12];

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
    pub workload: WorkloadConfig,
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
                    duration_secs: 60,
                },
                phases: PhaseConfig {
                    baseline_secs: 1,
                    fault_secs: 1,
                    recovery_secs: 1,
                },
                workload: WorkloadConfig {
                    durable_agents: 1,
                    ephemeral_agents: 1,
                    scheduled_agents: 1,
                    promise_agents: 1,
                    rate_per_sec: 1,
                },
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

    #[test]
    fn scenario_codes_parse_case_insensitively() {
        assert_eq!(ScenarioCode::parse("s12"), Some(ScenarioCode::S12));
        assert_eq!(ScenarioCode::parse("S12"), Some(ScenarioCode::S12));
        assert_eq!(ScenarioCode::parse("S99"), None);
    }
}
