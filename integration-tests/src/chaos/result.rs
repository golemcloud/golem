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

//! The scenario result artifact (GOL-363).
//!
//! This is the driver's whole output, and it is deliberately plain: timestamps,
//! counts, identifiers. No Grafana URLs, no dashboard names, no cluster
//! vocabulary. The workflow reads this file and builds the operator-facing
//! artifact from it — links scoped to `phases`, trace queries scoped to `scope`.
//!
//! Keeping the URL-building one layer up is not tidiness for its own sake: the
//! Grafana host, datasource uids and log-label scheme are facts about the
//! deployment, and a driver that hard-coded them would be wrong the moment the
//! same scenario ran anywhere else.

use crate::chaos::summary::{ChaosSummary, TerminationReason};
use crate::chaos::{FaultConfig, RetryPolicy, WorkloadConfig};
use chrono::{DateTime, Utc};
use golem_test_framework::benchmark::RunMetadata;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Bumped when the on-disk shape changes incompatibly. Archived results outlive
/// the tooling that reads them, so the shape has to say which shape it is.
pub const RESULT_SCHEMA_VERSION: u32 = 1;

/// A phase's wall-clock extent. These are the numbers the workflow pins Grafana
/// time ranges to, so they are recorded in UTC with no ambiguity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseWindow {
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

impl PhaseWindow {
    pub fn started(at: DateTime<Utc>) -> Self {
        Self {
            started_at: at,
            ended_at: None,
        }
    }

    pub fn end(&mut self, at: DateTime<Utc>) {
        self.ended_at = Some(at);
    }
}

/// The three phase windows. Any of them can be absent on an aborted run — a
/// missing window says the run never got there, which is information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Phases {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<PhaseWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fault: Option<PhaseWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<PhaseWindow>,
}

/// What the run touched, so a query can be narrowed to exactly this run's data
/// rather than to everything the cluster did that afternoon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunScope {
    pub environment_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub component_ids: Vec<String>,
    /// Shared prefix of every agent the run created.
    pub agent_id_prefix: String,
    /// Shared prefix of every idempotency key the run used. This is what makes
    /// a trace search for "this run's invocations" possible at all.
    pub idempotency_key_prefix: String,
}

/// The scenario result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChaosResult {
    pub schema_version: u32,
    pub scenario_code: String,
    pub scenario_name: String,
    /// `true` only when every phase ran and the report is complete.
    pub completed: bool,
    pub termination_reason: TerminationReason,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    pub phases: Phases,
    /// When the workflow reported the fault active. Absent if the run aborted
    /// before injection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fault_injected_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fault_recovered_at: Option<DateTime<Utc>>,
    /// Echo of what the workflow said it injected, for the record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fault_id: Option<String>,
    /// What the run was configured to provoke.
    pub fault: FaultConfig,
    pub workload: WorkloadConfig,
    pub retry_policy: RetryPolicy,
    pub scope: RunScope,
    pub summary: ChaosSummary,
    /// `GOLEM_BENCH_*` environment captured by the workflow: image tags, replica
    /// counts, the run note, and whether the workflow had to switch OTLP tracing
    /// on — which is how a reader knows whether traces should exist at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_metadata: Option<RunMetadata>,
}

impl ChaosResult {
    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path.as_ref(), json)
            .map_err(|e| anyhow::anyhow!("writing chaos result to {:?}: {e}", path.as_ref()))?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref())
            .map_err(|e| anyhow::anyhow!("reading chaos result from {:?}: {e}", path.as_ref()))?;
        Ok(serde_json::from_str(&raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::PhaseConfig;
    use test_r::test;

    fn sample_result(termination: TerminationReason) -> ChaosResult {
        let now = Utc::now();
        ChaosResult {
            schema_version: RESULT_SCHEMA_VERSION,
            scenario_code: "S12".to_string(),
            scenario_name: "shard-manager-pod-restart".to_string(),
            completed: !termination.is_failure(),
            termination_reason: termination,
            started_at: now,
            ended_at: Some(now),
            phases: Phases {
                baseline: Some(PhaseWindow::started(now)),
                fault: None,
                recovery: None,
            },
            fault_injected_at: None,
            fault_recovered_at: None,
            fault_id: None,
            fault: FaultConfig {
                kind: "pod-kill".to_string(),
                target: "shard-manager".to_string(),
                mode: "one".to_string(),
                duration_secs: 60,
            },
            workload: WorkloadConfig {
                durable_agents: 50,
                ephemeral_agents: 20,
                scheduled_agents: 20,
                promise_agents: 20,
                rate_per_sec: 10,
            },
            retry_policy: RetryPolicy::default(),
            scope: RunScope {
                environment_id: "env-1".to_string(),
                component_ids: vec!["component-1".to_string()],
                agent_id_prefix: "chaos-s12".to_string(),
                idempotency_key_prefix: "chaos-s12-".to_string(),
            },
            summary: ChaosSummary::build(&[], Vec::new(), Vec::new(), None),
            run_metadata: None,
        }
    }

    #[test]
    fn result_round_trips_through_json() {
        let result = sample_result(TerminationReason::Completed);
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ChaosResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.schema_version, RESULT_SCHEMA_VERSION);
        assert_eq!(parsed.scenario_code, "S12");
        assert!(parsed.completed);
        assert_eq!(parsed.termination_reason, TerminationReason::Completed);
    }

    /// An aborted run still has to produce a readable artifact: that is the
    /// whole point of flushing partials.
    #[test]
    fn an_aborted_result_records_why_and_keeps_the_phases_it_reached() {
        let result = sample_result(TerminationReason::SignalTimeout {
            file: "fault-injected.json".to_string(),
        });
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ChaosResult = serde_json::from_str(&json).unwrap();

        assert!(!parsed.completed);
        assert!(parsed.termination_reason.is_failure());
        assert!(
            parsed.phases.baseline.is_some(),
            "the phase it did reach must survive"
        );
        assert!(
            parsed.phases.fault.is_none(),
            "a phase it never reached must stay absent rather than be invented"
        );
    }

    /// The workflow pins Grafana time ranges to these, so they have to be
    /// present and unambiguous whenever the phase ran.
    #[test]
    fn phase_windows_carry_utc_start_and_end() {
        let start = Utc::now();
        let mut window = PhaseWindow::started(start);
        assert!(window.ended_at.is_none());
        let end = start + chrono::Duration::seconds(300);
        window.end(end);
        assert_eq!(window.ended_at, Some(end));
    }

    #[test]
    fn phase_config_exposes_durations() {
        let phases = PhaseConfig {
            baseline_secs: 300,
            fault_secs: 120,
            recovery_secs: 300,
        };
        assert_eq!(phases.baseline().as_secs(), 300);
        assert_eq!(phases.fault().as_secs(), 120);
        assert_eq!(phases.recovery().as_secs(), 300);
    }
}

#[cfg(test)]
mod sample_artifact {
    use super::*;
    use crate::chaos::history::{OperationRecord, Outcome, Phase, Stream};
    use crate::chaos::summary::{AgentReadback, RoutingSnapshot};
    use crate::chaos::{FaultConfig, PhaseConfig, RetryPolicy, WorkloadConfig};
    use chrono::TimeZone;
    use test_r::test;

    /// Writes a representative result artifact to `CHAOS_SAMPLE_RESULT` when the
    /// variable is set. The golem-cloud investigation-report generator is
    /// developed against this, so its input is the real serialiser rather than a
    /// hand-written guess at the shape.
    #[test]
    fn write_sample_result_artifact() {
        let Ok(path) = std::env::var("CHAOS_SAMPLE_RESULT") else {
            return;
        };
        let at = |s: i64| Utc.timestamp_opt(1_800_000_000 + s, 0).unwrap();

        let mut records = Vec::new();
        for i in 0..20u64 {
            records.push(OperationRecord {
                op_id: i,
                stream: Stream::Durable,
                phase: if i < 12 {
                    Phase::Baseline
                } else {
                    Phase::Fault
                },
                agent: format!("chaos-s12-durable-{:04}", i % 2),
                method: "increment".to_string(),
                idempotency_key: format!("chaos-s12-durable-{:04}-{i:08}", i % 2),
                submitted_at: at(i as i64),
                completed_at: Some(at(i as i64 + 1)),
                attempts: if i == 15 { 2 } else { 1 },
                outcome: if i == 15 {
                    Outcome::Indeterminate
                } else {
                    Outcome::Confirmed
                },
                duration_ms: 10 + i,
                returned_value: None,
                first_attempt_value: None,
                error: None,
            });
        }

        let agent0: Vec<&OperationRecord> = records
            .iter()
            .filter(|r| r.agent.ends_with("0000"))
            .collect();
        let agent1: Vec<&OperationRecord> = records
            .iter()
            .filter(|r| r.agent.ends_with("0001"))
            .collect();
        let readback = vec![
            AgentReadback::evaluate(Stream::Durable, "chaos-s12-durable-0000", &agent0, Ok(10)),
            // Deliberately one above the upper bound: exercises the flagged path.
            AgentReadback::evaluate(Stream::Durable, "chaos-s12-durable-0001", &agent1, Ok(99)),
        ];

        let routing = vec![
            RoutingSnapshot {
                at: "before-fault".to_string(),
                taken_at: at(100),
                shards_per_executor: Some(
                    [
                        ("10.0.1.1:9000".to_string(), 512),
                        ("10.0.1.2:9000".to_string(), 512),
                    ]
                    .into_iter()
                    .collect(),
                ),
                unavailable_reason: None,
            },
            RoutingSnapshot {
                at: "after-recovery".to_string(),
                taken_at: at(600),
                shards_per_executor: None,
                unavailable_reason: Some("shard-manager unreachable".to_string()),
            },
        ];

        let result = ChaosResult {
            schema_version: RESULT_SCHEMA_VERSION,
            scenario_code: "S12".to_string(),
            scenario_name: "shard-manager-pod-restart".to_string(),
            completed: true,
            termination_reason: TerminationReason::Completed,
            started_at: at(0),
            ended_at: Some(at(900)),
            phases: Phases {
                baseline: Some({
                    let mut w = PhaseWindow::started(at(0));
                    w.end(at(300));
                    w
                }),
                fault: Some({
                    let mut w = PhaseWindow::started(at(300));
                    w.end(at(420));
                    w
                }),
                recovery: Some({
                    let mut w = PhaseWindow::started(at(420));
                    w.end(at(720));
                    w
                }),
            },
            fault_injected_at: Some(at(300)),
            fault_recovered_at: Some(at(420)),
            fault_id: Some("chaos-s12-12345".to_string()),
            fault: FaultConfig {
                kind: "pod-kill".to_string(),
                target: "shard-manager".to_string(),
                mode: "one".to_string(),
                duration_secs: 60,
            },
            workload: WorkloadConfig {
                durable_agents: 50,
                ephemeral_agents: 20,
                scheduled_agents: 20,
                promise_agents: 20,
                rate_per_sec: 10,
            },
            retry_policy: RetryPolicy::default(),
            scope: RunScope {
                environment_id: "0192f000-0000-7000-8000-000000000001".to_string(),
                component_ids: vec!["0192f000-0000-7000-8000-000000000002".to_string()],
                agent_id_prefix: "chaos-s12".to_string(),
                idempotency_key_prefix: "chaos-s12-".to_string(),
            },
            summary: ChaosSummary::build(&records, readback, routing, Some(at(300))),
            run_metadata: None,
        };
        let _ = PhaseConfig {
            baseline_secs: 300,
            fault_secs: 120,
            recovery_secs: 300,
        };
        result.save(&path).unwrap();
        println!("wrote sample result to {path}");
    }
}
