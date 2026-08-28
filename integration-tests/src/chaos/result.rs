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

use crate::chaos::pinned::PinnedSelection;
use crate::chaos::scheduled::ScheduledSelection;
use crate::chaos::split::PodSplit;
use crate::chaos::summary::{ChaosSummary, TerminationReason};
use crate::chaos::{
    DeleteConfig, FaultConfig, IsolationConfig, PinnedConfig, PromiseConfig, RetryPolicy,
    RevertConfig, RollbackConfig, ScheduledConfig, StorageConfig, WorkloadConfig,
};
use chrono::{DateTime, Utc};
use golem_test_framework::benchmark::RunMetadata;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Bumped when the on-disk shape changes incompatibly. Archived results outlive
/// the tooling that reads them, so the shape has to say which shape it is.
///
/// 3: the storage scenarios stopped sharing one verdict. `storage` carries an
/// `expect` block where it used to carry `outageQuietFloorPercent`, and the
/// storage-fault account carries the same block plus
/// `leastServingStreamPercent`. A version 2 result does not deserialise into
/// the version 3 types, which is what the bump is for; the report generator
/// reads both, because the runs already in the bucket are worth rendering.
pub const RESULT_SCHEMA_VERSION: u32 = 3;

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
    /// What the workflow reported it actually aimed at. For an unpinned
    /// scenario this is the deployment name; for a pinned one it is the pod the
    /// workflow resolved and killed — which is what an investigation needs to
    /// find that executor's own logs and traces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault_target_observed: Option<String>,
    /// What the run was configured to provoke.
    pub fault: FaultConfig,
    /// The mixed workload the run was configured with. Absent for scenarios
    /// that drive a pinned population instead — schema v2 made these two
    /// mutually exclusive rather than making one of them lie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload: Option<WorkloadConfig>,
    /// The pinned workload the run was configured with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<PinnedConfig>,
    /// Which executor the fault was aimed at and which agents it was verified
    /// to own. Present only for scenarios that pin the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_selection: Option<PinnedSelection>,
    /// The scheduled-registration workload the run was configured with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<ScheduledConfig>,
    /// How the schedule targets divided around the executor the fault was aimed
    /// at. Present only for S10, and load-bearing for reading its percentiles:
    /// without it there is no way to tell the affected population from the
    /// control group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_selection: Option<ScheduledSelection>,
    /// The suspended-waiter workload the run was configured with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promise: Option<PromiseConfig>,
    /// How the promise waiters divided around the executor the fault was aimed
    /// at. Present only for S11, and load-bearing for the same reason as
    /// `scheduledSelection`: without it there is no way to tell the affected
    /// population from the control group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promise_selection: Option<PodSplit>,
    /// The reachability workload the run was configured with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<IsolationConfig>,
    /// How the agents divided around the executor the partition cut off.
    /// Present only for S3. Load-bearing for the same reason as
    /// `promiseSelection`, and for one more: S3's whole verdict is a comparison
    /// between the two groups, so a report without this cannot be re-checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation_selection: Option<PodSplit>,
    /// The revert workload the run was configured with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revert: Option<RevertConfig>,
    /// How the revert agents divided around the executor the kill was aimed at.
    /// Present only for S7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revert_selection: Option<PodSplit>,
    /// The deletion workload the run was configured with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<DeleteConfig>,
    /// How the agent slots divided around the executor the kill was aimed at.
    /// Present only for S6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_selection: Option<PodSplit>,
    /// The component rollback the run was configured with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<RollbackConfig>,
    /// The storage the run was configured to take away, and the thresholds its
    /// account was judged by. Present only for S16.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageConfig>,
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
            fault_target_observed: None,
            fault: FaultConfig {
                kind: "pod-kill".to_string(),
                target: "shard-manager".to_string(),
                mode: "one".to_string(),
                target_count: None,
                manifest: None,
                duration_secs: 60,
            },
            workload: Some(WorkloadConfig {
                durable_agents: 50,
                ephemeral_agents: 20,
                scheduled_agents: 20,
                promise_agents: 20,
                quota_agents: 20,
                rate_per_sec: 10,
            }),
            pinned: None,
            pinned_selection: None,
            scheduled: None,
            scheduled_selection: None,
            promise: None,
            promise_selection: None,
            isolation: None,
            isolation_selection: None,
            revert: None,
            revert_selection: None,
            delete: None,
            delete_selection: None,
            rollback: None,
            storage: None,
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

    /// The S11 shape. Same contract as the S10 test below, for the same reason:
    /// `ci-scripts/chaos-investigation-report.py` in golem-cloud reads these by
    /// name, and the two repositories cannot be changed atomically.
    #[test]
    fn an_s11_result_carries_the_promise_wakeup_fields_the_investigation_report_reads() {
        use crate::chaos::history::{WaiterWakeupLog, WakeupRecord};
        use crate::chaos::split::{FaultWindow, PodSplit};
        use crate::chaos::wakeups::WakeupReport;

        let now = Utc::now();
        let waiter = "chaos-s11-promise-waiter-0000".to_string();
        let split = PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: vec![waiter.clone()],
            elsewhere: Vec::new(),
            targets_per_pod: std::collections::BTreeMap::new(),
            number_of_shards: 1024,
        };

        let mut result = sample_result(TerminationReason::Completed);
        result.scenario_code = "S11".to_string();
        result.promise = Some(crate::chaos::PromiseConfig {
            waiters: 200,
            dwell_millis: 5000,
            wakeup_budget_secs: 60,
        });
        result.promise_selection = Some(split.clone());
        result.summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), Some(now))
            .with_promise_wakeups(WakeupReport::build(
                &[],
                &[WaiterWakeupLog {
                    agent: waiter.clone(),
                    wakes: Some(1),
                    wakeups: vec![WakeupRecord {
                        token: format!("{waiter}-00000001"),
                        armed_at: now,
                        woken_at: now + chrono::Duration::seconds(6),
                    }],
                    error: None,
                }],
                &split,
                Some(FaultWindow {
                    injected_at: now,
                    recovered_at: None,
                }),
                std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(60),
                0,
            ));

        let json = serde_json::to_value(&result).unwrap();
        let wakeups = &json["summary"]["promiseWakeups"];
        for key in [
            "wakeupBudgetMs",
            "dwellMs",
            "completionsConfirmed",
            "completionsIndeterminate",
            "completionsRejected",
            "wakeupsRecorded",
            "wokeOnce",
            "indeterminateThatWoke",
            "inconclusive",
            "unverifiable",
            "unknownTokens",
            "waitersUnreadable",
            "waitersTruncated",
            "waitersStoodDown",
            "waitersWedged",
            "delay",
            "findings",
            "findingsOmitted",
        ] {
            assert!(
                !wakeups[key].is_null(),
                "summary.promiseWakeups.{key} is what the investigation report reads"
            );
        }
        assert_eq!(json["promise"]["wakeupBudgetSecs"], 60);
        assert_eq!(json["promiseSelection"]["podIp"], "10.0.1.1");

        // And it still round-trips, so an archived S11 result stays readable.
        let parsed: ChaosResult = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(parsed.scenario_code, "S11");
        assert!(parsed.summary.promise_wakeups.is_some());
        assert!(parsed.promise_selection.is_some());
    }

    /// The S9 shape. Same contract as the others: the investigation report in
    /// golem-cloud reads these fields by name.
    #[test]
    fn an_s9_result_carries_the_rollback_fields_the_investigation_report_reads() {
        use crate::chaos::rollback::{ControlPlaneAttempts, RollbackReport, VersionCensus};

        let now = Utc::now();
        let mut forward = std::collections::BTreeMap::new();
        forward.insert("chaos-s9-durable-0000".to_string(), Some(2u32));
        let mut back = std::collections::BTreeMap::new();
        back.insert("chaos-s9-durable-0000".to_string(), Some(1u32));

        let mut result = sample_result(TerminationReason::Completed);
        result.scenario_code = "S9".to_string();
        result.rollback = Some(crate::chaos::RollbackConfig {
            settle_secs: 90,
            rolled_forward_floor_percent: 90.0,
            control_retries: 2,
            control_retry_delay_secs: 5,
            kill_delay_secs: 2,
        });
        result.summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), Some(now)).with_rollback(
            RollbackReport {
                forward_revision: 2,
                rollback_revision: 3,
                forward_version: 2,
                rollback_version: 1,
                rolled_forward: VersionCensus::build("before-rollback", 2, &forward),
                rolled_back: Some(VersionCensus::build("after-recovery", 1, &back)),
                control: ControlPlaneAttempts {
                    requested: 200,
                    accepted_first_try: 198,
                    accepted_after_retry: 2,
                    refused: 0,
                    max_retries: 2,
                },
                rolled_forward_floor_percent: 90.0,
            },
        );

        let json = serde_json::to_value(&result).unwrap();
        let rollback = &json["summary"]["rollback"];
        for key in [
            "forwardRevision",
            "rollbackRevision",
            "forwardVersion",
            "rollbackVersion",
            "rolledForward",
            "rolledBack",
            "control",
            "rolledForwardFloorPercent",
        ] {
            assert!(
                !rollback[key].is_null(),
                "summary.rollback.{key} is what the investigation report reads"
            );
        }
        for key in [
            "requested",
            "acceptedFirstTry",
            "acceptedAfterRetry",
            "refused",
        ] {
            assert!(!rollback["control"][key].is_null(), "control.{key}");
        }
        assert_eq!(json["rollback"]["rolledForwardFloorPercent"], 90.0);

        let parsed: ChaosResult = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(parsed.scenario_code, "S9");
        assert!(parsed.summary.rollback.is_some());
    }

    /// The S6 shape. Same contract as the S3, S7, S10 and S11 tests: the
    /// investigation report in golem-cloud reads these fields by name.
    #[test]
    fn an_s6_result_carries_the_resurrection_fields_the_investigation_report_reads() {
        use crate::chaos::deletions::DeleteRound;
        use crate::chaos::resurrection::ResurrectionReport;
        use crate::chaos::split::{FaultWindow, PodSplit};

        let now = Utc::now();
        let agent = "chaos-s6-delete-0000".to_string();
        let split = PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: vec![agent.clone()],
            elsewhere: Vec::new(),
            targets_per_pod: std::collections::BTreeMap::new(),
            number_of_shards: 1024,
        };

        let mut result = sample_result(TerminationReason::Completed);
        result.scenario_code = "S6".to_string();
        result.delete = Some(crate::chaos::DeleteConfig {
            agents: 200,
            increments_per_round: 3,
            interval_millis: 500,
            recovery_budget_secs: 60,
        });
        result.delete_selection = Some(split.clone());
        result.summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), Some(now))
            .with_resurrection(ResurrectionReport::build(
                &[DeleteRound {
                    agent,
                    round: 0,
                    before_delete: Some(3),
                    outcome: crate::chaos::history::Outcome::Confirmed,
                    rejected_as_not_found: false,
                    submitted_at: now,
                    completed_at: Some(now),
                    observed_after: Some(0),
                }],
                &split,
                Some(FaultWindow {
                    injected_at: now,
                    recovered_at: None,
                }),
                3,
            ));

        let json = serde_json::to_value(&result).unwrap();
        let resurrection = &json["summary"]["resurrection"];
        for key in [
            "incrementsPerRound",
            "roundsRecorded",
            "deletesConfirmed",
            "deletesIndeterminate",
            "deletesRejected",
            "deletedExactly",
            "indeterminateThatDeleted",
            "indeterminateThatDidNot",
            "unjudgeable",
            "unprobed",
            "cells",
            "caughtByTheKill",
            "findings",
            "findingsOmitted",
        ] {
            assert!(
                !resurrection[key].is_null(),
                "summary.resurrection.{key} is what the investigation report reads"
            );
        }
        assert_eq!(json["delete"]["incrementsPerRound"], 3);
        assert_eq!(json["deleteSelection"]["podIp"], "10.0.1.1");

        let parsed: ChaosResult = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(parsed.scenario_code, "S6");
        assert!(parsed.summary.resurrection.is_some());
        assert!(parsed.delete_selection.is_some());
    }

    /// The S7 shape. Same contract as the S3, S10 and S11 tests: the
    /// investigation report in golem-cloud reads these fields by name.
    #[test]
    fn an_s7_result_carries_the_truncation_fields_the_investigation_report_reads() {
        use crate::chaos::reverts::RevertRound;
        use crate::chaos::split::{FaultWindow, PodSplit};
        use crate::chaos::truncation::TruncationReport;

        let now = Utc::now();
        let agent = "chaos-s7-revert-0000".to_string();
        let split = PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: vec![agent.clone()],
            elsewhere: Vec::new(),
            targets_per_pod: std::collections::BTreeMap::new(),
            number_of_shards: 1024,
        };

        let mut result = sample_result(TerminationReason::Completed);
        result.scenario_code = "S7".to_string();
        result.revert = Some(crate::chaos::RevertConfig {
            agents: 200,
            increments_per_round: 4,
            revert_invocations: 2,
            interval_millis: 500,
            recovery_budget_secs: 60,
        });
        result.revert_selection = Some(split.clone());
        result.summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), Some(now))
            .with_truncation(TruncationReport::build(
                &[RevertRound {
                    agent,
                    round: 0,
                    before_revert: Some(10),
                    asked_to_revert: 2,
                    outcome: crate::chaos::history::Outcome::Confirmed,
                    submitted_at: now,
                    completed_at: Some(now),
                    observed_after: Some(8),
                }],
                &split,
                Some(FaultWindow {
                    injected_at: now,
                    recovered_at: None,
                }),
                4,
                2,
            ));

        let json = serde_json::to_value(&result).unwrap();
        let truncation = &json["summary"]["truncation"];
        for key in [
            "incrementsPerRound",
            "revertInvocations",
            "roundsRecorded",
            "revertsConfirmed",
            "revertsIndeterminate",
            "revertsRejected",
            "appliedExactly",
            "indeterminateThatApplied",
            "indeterminateThatDidNot",
            "unjudgeable",
            "unprobed",
            "cells",
            "caughtByTheKill",
            "findings",
            "findingsOmitted",
        ] {
            assert!(
                !truncation[key].is_null(),
                "summary.truncation.{key} is what the investigation report reads"
            );
        }
        assert_eq!(json["revert"]["revertInvocations"], 2);
        assert_eq!(json["revertSelection"]["podIp"], "10.0.1.1");

        let parsed: ChaosResult = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(parsed.scenario_code, "S7");
        assert!(parsed.summary.truncation.is_some());
        assert!(parsed.revert_selection.is_some());
    }

    /// The S3 shape. Same contract as the S10 and S11 tests, for the same
    /// reason: `ci-scripts/chaos-investigation-report.py` in golem-cloud reads
    /// these by name, and the two repositories cannot be changed atomically.
    #[test]
    fn an_s3_result_carries_the_reachability_fields_the_investigation_report_reads() {
        use crate::chaos::reachability::ReachabilityReport;
        use crate::chaos::split::{FaultWindow, PodSplit};

        let now = Utc::now();
        let split = PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: vec!["chaos-s3-durable-0000".to_string()],
            elsewhere: vec!["chaos-s3-durable-0001".to_string()],
            targets_per_pod: std::collections::BTreeMap::new(),
            number_of_shards: 1024,
        };

        let mut result = sample_result(TerminationReason::Completed);
        result.scenario_code = "S3".to_string();
        result.isolation = Some(crate::chaos::IsolationConfig {
            agents: 200,
            interval_millis: 1000,
            isolated_ceiling_percent: 25.0,
            control_floor_percent: 75.0,
            recovery_budget_secs: 60,
        });
        result.isolation_selection = Some(split.clone());
        result.summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), Some(now))
            .with_reachability(ReachabilityReport::build(
                &[],
                &split,
                Some(FaultWindow {
                    injected_at: now,
                    recovered_at: Some(now + chrono::Duration::seconds(180)),
                }),
                25.0,
                75.0,
                std::time::Duration::from_secs(60),
            ));

        let json = serde_json::to_value(&result).unwrap();
        let reachability = &json["summary"]["reachability"];
        for key in [
            "isolatedPod",
            "isolatedAgents",
            "reachableAgents",
            "isolatedCeilingPercent",
            "controlFloorPercent",
            "recoveryBudgetMs",
            "cells",
            "recovery",
            "recoveryOverBudget",
            "agentsNeverRecovered",
            "recordsOutsideTheSplit",
            "findings",
            "findingsOmitted",
        ] {
            assert!(
                !reachability[key].is_null(),
                "summary.reachability.{key} is what the investigation report reads"
            );
        }
        assert_eq!(json["isolation"]["controlFloorPercent"], 75.0);
        assert_eq!(json["isolationSelection"]["podIp"], "10.0.1.1");

        // And it still round-trips, so an archived S3 result stays readable.
        let parsed: ChaosResult = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(parsed.scenario_code, "S3");
        assert!(parsed.summary.reachability.is_some());
        assert!(parsed.isolation_selection.is_some());
    }

    /// The S10 shape, whose report is read by a script in another repository.
    ///
    /// `ci-scripts/chaos-investigation-report.py` in golem-cloud renders these
    /// fields by name. The two repositories cannot be changed atomically, so a
    /// rename here would silently empty a section of the investigation report
    /// rather than fail anything. Naming the keys in a test is what makes that
    /// break loudly and locally.
    #[test]
    fn an_s10_result_carries_the_schedule_fire_fields_the_investigation_report_reads() {
        use crate::chaos::fires::{FaultWindow, ScheduleFireReport};
        use crate::chaos::history::{FireRecord, Stream, TargetFireLog};

        let now = Utc::now();
        let due = now + chrono::Duration::seconds(10);
        let mut result = sample_result(TerminationReason::Completed);
        result.scenario_code = "S10".to_string();
        result.scheduled = Some(crate::chaos::ScheduledConfig {
            targets: 100,
            interval_millis: 2000,
            lead_secs: 10,
            lease_budget_secs: 60,
        });
        result.summary = ChaosSummary::build(&[], Vec::new(), Vec::new(), Some(now))
            .with_schedule_fires(ScheduleFireReport::build(
                &[],
                &[TargetFireLog {
                    agent: "chaos-s10-scheduled-target-0000".to_string(),
                    polls: Some(1),
                    fires: vec![FireRecord {
                        token: "chaos-s10-scheduled-target-0000-00000001".to_string(),
                        scheduled_at: due,
                        observed_at: due + chrono::Duration::seconds(4),
                    }],
                    error: None,
                }],
                std::time::Duration::from_secs(10),
                Some(FaultWindow {
                    injected_at: now,
                    recovered_at: None,
                }),
                &std::collections::BTreeSet::from(["chaos-s10-scheduled-target-0000".to_string()]),
                std::time::Duration::from_secs(60),
            ));

        let json = serde_json::to_value(&result).unwrap();
        let fires = &json["summary"]["scheduleFires"];
        for key in [
            "leaseBudgetMs",
            "registrationsConfirmed",
            "registrationsIndeterminate",
            "firesRecorded",
            "firedOnce",
            "indeterminateThatFired",
            "inconclusive",
            "unverifiable",
            "unknownTokens",
            "targetsUnreadable",
            "targetsTruncated",
            "delay",
            "findings",
            "findingsOmitted",
            "overdueOnArrival",
            "overdueDelay",
        ] {
            assert!(
                !fires[key].is_null(),
                "summary.scheduleFires.{key} is what the investigation report reads"
            );
        }
        let cell = &fires["delay"][0];
        assert_eq!(cell["group"], "on-killed-executor");
        assert_eq!(cell["window"], "during-fault");
        assert_eq!(cell["delay"]["p99Ms"], 4000);
        assert_eq!(cell["overBudget"], 0);
        assert_eq!(cell["minDelayMs"], 4000);
        assert_eq!(json["scheduled"]["leaseBudgetSecs"], 60);

        // And it still round-trips, so an archived S10 result stays readable.
        let parsed: ChaosResult = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(parsed.scenario_code, "S10");
        assert!(parsed.summary.schedule_fires.is_some());
        assert!(!Stream::Scheduled.to_string().is_empty());
    }

    /// The CI annotation branches on `summary.attention` being non-empty, so
    /// the two lists have to stay two lists across the repo boundary. Folding
    /// context back into `attention` would make the annotation fire on every
    /// healthy run, which is how it came to mean nothing the first time.
    #[test]
    fn a_result_separates_findings_from_context_for_the_ci_annotation() {
        use crate::chaos::summary::Note;

        let mut result = sample_result(TerminationReason::Completed);
        result.summary.absorb([
            Note::context("routing at start: 1024/1024 shards (settled before measuring)"),
            Note::attention("4 scheduled targets filled their fire log and dropped entries"),
        ]);

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["summary"]["attention"].as_array().unwrap().len(),
            1,
            "summary.attention is what --attention-count counts"
        );
        assert_eq!(
            json["summary"]["notes"].as_array().unwrap().len(),
            1,
            "summary.notes is what the report renders as run context"
        );

        let parsed: ChaosResult = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(parsed.summary.attention.len(), 1);
        assert_eq!(parsed.summary.notes.len(), 1);
    }

    /// A tag can be moved; a digest identifies the build a run actually tested.
    /// The workflow emits both, and the runbook tells a reader to match them
    /// against the deployment manifests, so both key names are load-bearing.
    #[test]
    fn run_metadata_carries_the_image_digest_beside_the_tag() {
        use golem_test_framework::benchmark::RunMetadata;

        let mut result = sample_result(TerminationReason::Completed);
        result.run_metadata = Some(RunMetadata {
            worker_executor_image_tag: Some("v1.5.10-dev.2".to_string()),
            worker_executor_image_digest: Some("sha256:60eac87a".to_string()),
            ..Default::default()
        });

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json["runMetadata"]["workerExecutorImageTag"],
            "v1.5.10-dev.2"
        );
        assert_eq!(
            json["runMetadata"]["workerExecutorImageDigest"],
            "sha256:60eac87a"
        );
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
                error_class: None,
                attempt_log: Vec::new(),
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
            fault_target_observed: Some("shard-manager".to_string()),
            fault: FaultConfig {
                kind: "pod-kill".to_string(),
                target: "shard-manager".to_string(),
                mode: "one".to_string(),
                target_count: None,
                manifest: None,
                duration_secs: 60,
            },
            workload: Some(WorkloadConfig {
                durable_agents: 50,
                ephemeral_agents: 20,
                scheduled_agents: 20,
                promise_agents: 20,
                quota_agents: 20,
                rate_per_sec: 10,
            }),
            pinned: None,
            pinned_selection: None,
            scheduled: None,
            scheduled_selection: None,
            promise: None,
            promise_selection: None,
            isolation: None,
            isolation_selection: None,
            revert: None,
            revert_selection: None,
            delete: None,
            delete_selection: None,
            rollback: None,
            storage: None,
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

    /// The same, for S10, whose report has a section of its own.
    ///
    /// A separate artifact rather than a field added to the one above: the two
    /// scenarios do not share a workload shape, and a sample that carried both
    /// a mixed workload and a scheduled one would not resemble anything the
    /// driver ever writes.
    #[test]
    fn write_sample_s10_result_artifact() {
        use crate::chaos::fires::{FaultWindow, ScheduleFireReport};
        use crate::chaos::history::{FireRecord, TargetFireLog};
        use crate::chaos::scheduled::ScheduledSelection;

        let Ok(path) = std::env::var("CHAOS_SAMPLE_RESULT_S10") else {
            return;
        };
        let at = |s: i64| Utc.timestamp_opt(1_800_000_000 + s, 0).unwrap();
        let killed = "chaos-s10-scheduled-target-0000";
        let survivor = "chaos-s10-scheduled-target-0001";

        // Four registrations per target. On the killed executor one of them
        // never fires, which is the finding the section exists to render.
        let mut records = Vec::new();
        let mut killed_fires = Vec::new();
        let mut survivor_fires = Vec::new();
        for i in 0..8u64 {
            let target = if i % 2 == 0 { killed } else { survivor };
            // Spread so some actions fall due before the kill and some during it,
            // which is what gives the report both a control row and the row it is
            // actually about.
            let submitted = at(285 + i as i64 * 4);
            let token = format!("{target}-{:08}", i / 2);
            records.push(OperationRecord {
                op_id: i,
                stream: Stream::Scheduled,
                phase: if submitted < at(300) {
                    Phase::Baseline
                } else {
                    Phase::Fault
                },
                agent: target.to_string(),
                method: "schedule_fire_at".to_string(),
                idempotency_key: token.clone(),
                submitted_at: submitted,
                completed_at: Some(submitted),
                attempts: 1,
                outcome: Outcome::Confirmed,
                duration_ms: 8 + i,
                returned_value: None,
                first_attempt_value: None,
                error: None,
                error_class: None,
                attempt_log: Vec::new(),
            });

            let due = submitted + chrono::Duration::seconds(10);
            // One registration stalls on the client's attempt timeout and only
            // lands long after its action was due — the shape the first real run
            // produced 26 times over.
            if i == 1 {
                let stalled = records.last_mut().unwrap();
                stalled.completed_at = Some(submitted + chrono::Duration::seconds(125));
                stalled.duration_ms = 125_000;
                stalled.attempts = 2;
            }
            if target == killed {
                // The last one is the action the kill swallowed.
                if i < 6 {
                    killed_fires.push(FireRecord {
                        token,
                        scheduled_at: due,
                        // Late by a shard reassignment.
                        observed_at: due + chrono::Duration::milliseconds(41_500),
                    });
                }
            } else {
                // The stalled registration's action fires the moment it lands,
                // which the raw arithmetic calls 115s late.
                let late = if i == 1 { 115_200 } else { 120 };
                survivor_fires.push(FireRecord {
                    token,
                    scheduled_at: due,
                    observed_at: due + chrono::Duration::milliseconds(late),
                });
            }
        }

        let logs = vec![
            TargetFireLog {
                agent: killed.to_string(),
                polls: Some(killed_fires.len() as u64),
                fires: killed_fires,
                error: None,
            },
            TargetFireLog {
                agent: survivor.to_string(),
                polls: Some(survivor_fires.len() as u64),
                fires: survivor_fires,
                error: None,
            },
        ];

        let on_pod = std::collections::BTreeSet::from([killed.to_string()]);
        let report = ScheduleFireReport::build(
            &records,
            &logs,
            std::time::Duration::from_secs(10),
            Some(FaultWindow {
                injected_at: at(300),
                recovered_at: Some(at(420)),
            }),
            &on_pod,
            std::time::Duration::from_secs(60),
        );

        let readback: Vec<AgentReadback> = logs
            .iter()
            .map(|log| {
                let scoped: Vec<&OperationRecord> =
                    records.iter().filter(|r| r.agent == log.agent).collect();
                AgentReadback::evaluate(
                    Stream::Scheduled,
                    &log.agent,
                    &scoped,
                    Ok(log.polls.unwrap_or(0)),
                )
            })
            .collect();

        let result = ChaosResult {
            schema_version: RESULT_SCHEMA_VERSION,
            scenario_code: "S10".to_string(),
            scenario_name: "executor-crash-during-scheduled-fire".to_string(),
            completed: false,
            termination_reason: TerminationReason::ScheduledFireViolated {
                findings: report.findings.len() as u64,
                first: report
                    .findings
                    .first()
                    .map(|f| format!("{} on token {}", f.violation, f.token))
                    .unwrap_or_default(),
            },
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
            fault_id: Some("chaos-s10-12345".to_string()),
            fault_target_observed: Some("worker-executor-abc123".to_string()),
            fault: FaultConfig {
                kind: "pod-kill".to_string(),
                target: "worker-executor".to_string(),
                mode: "one".to_string(),
                target_count: None,
                manifest: None,
                duration_secs: 60,
            },
            workload: None,
            pinned: None,
            pinned_selection: None,
            scheduled: Some(crate::chaos::ScheduledConfig {
                targets: 2,
                interval_millis: 2000,
                lead_secs: 10,
                lease_budget_secs: 60,
            }),
            scheduled_selection: Some(ScheduledSelection {
                pod_address: "10.0.1.1:9000".to_string(),
                pod_ip: "10.0.1.1".to_string(),
                on_pod: vec![killed.to_string()],
                elsewhere: vec![survivor.to_string()],
                targets_per_pod: [
                    ("10.0.1.1:9000".to_string(), 1),
                    ("10.0.1.2:9000".to_string(), 1),
                ]
                .into_iter()
                .collect(),
                number_of_shards: 1024,
            }),
            promise: None,
            promise_selection: None,
            isolation: None,
            isolation_selection: None,
            revert: None,
            revert_selection: None,
            delete: None,
            delete_selection: None,
            rollback: None,
            storage: None,
            retry_policy: RetryPolicy::default(),
            scope: RunScope {
                environment_id: "0192f000-0000-7000-8000-000000000001".to_string(),
                component_ids: vec!["0192f000-0000-7000-8000-000000000002".to_string()],
                agent_id_prefix: "chaos-s10".to_string(),
                idempotency_key_prefix: "chaos-s10-".to_string(),
            },
            summary: ChaosSummary::build(&records, readback, Vec::new(), Some(at(300)))
                .with_schedule_fires(report),
            run_metadata: None,
        };
        result.save(&path).unwrap();
        println!("wrote sample S10 result to {path}");
    }
}
