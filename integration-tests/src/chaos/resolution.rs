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

//! The name-resolution account (GOL-373).
//!
//! S4 poisons `shard-manager.golem-release.svc.cluster.local` on one executor
//! and drives quota work on both. This module turns the operation history into
//! the comparison that answers the ticket.
//!
//! ### Why the expected answer is "nothing", and what that costs to report
//!
//! The executor holds one connection to the shard manager, built by
//! `GrpcClient::new`, whose idle TTL is `Duration::MAX`
//! (`golem-service-base/src/grpc/client.rs`). DNS is consulted when that
//! connection is established and never again while it lives. A name that stops
//! resolving therefore reaches nothing: no re-resolution is attempted, so the
//! SERVFAIL is never asked for.
//!
//! That makes S4 the second scenario in the suite — after S19 — where a clean
//! report and a report of nothing are the same document. The suite answers that
//! in two places and neither is here: the workflow proves the mechanism on a
//! throwaway pod before the run, and [`crate::chaos::split`] refuses a run whose
//! quota population landed entirely on one executor. What this module owes the
//! reader is the third part: it states the comparison it made on every run,
//! including the ones with no findings, so a result that says nothing happened
//! also says what it looked at.
//!
//! ### Why the comparison is across executors, not across time
//!
//! S19 compares the faulted pod's post-fault latency against its own baseline,
//! because a clock skew's cost arrives and leaves with the fault. A DNS failure
//! that reaches anything would not: the executor would be trying and failing to
//! rebuild a connection, so the cost would be concurrent with the fault and
//! nothing else in the run would move.
//!
//! So the headline is the target group against the **control group in the same
//! window**. Both halves are running the same workload against the same shard
//! manager at the same instant, so a difference between them is the fault and
//! not the hour. The against-its-own-baseline reading is kept as well, because
//! a fault that cost something and then failed to give it back is a different
//! result from one that cost nothing.
//!
//! Both are recorded rather than failed. What fails an S4 run lives elsewhere:
//! the exactly-once oracle, and a shard assignment that moved.

use crate::chaos::history::{OperationRecord, Stream};
use crate::chaos::split::{
    self, FaultWindow, Group, PodSplit, StreamCell, Window, recovery_percent, round2,
};
use serde::{Deserialize, Serialize};

const SCENARIO: &str = "S4";

/// What the account can say went wrong, both recorded rather than failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionViolation {
    /// The target executor's quota stream was slower than the control
    /// executor's while the name was poisoned.
    QuotaDegraded,
    /// It was still slower than its own baseline after the name came back.
    QuotaDidNotRecover,
}

impl ResolutionViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolutionViolation::QuotaDegraded => "quota-degraded",
            ResolutionViolation::QuotaDidNotRecover => "quota-did-not-recover",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionFinding {
    pub violation: ResolutionViolation,
    pub detail: String,
}

/// The name-resolution account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionReport {
    /// The name the fault was pointed at, mirrored from the suite so an
    /// archived result says what was poisoned without needing the manifest.
    pub poisoned_name: String,
    /// How far above the control group the target group's during-fault p50 may
    /// sit before [`ResolutionViolation::QuotaDegraded`] is recorded.
    pub degradation_ceiling_percent: f64,
    /// How far above its own baseline the target group's post-fault p50 may sit
    /// before [`ResolutionViolation::QuotaDidNotRecover`] is recorded.
    pub recovery_floor_percent: f64,
    pub cells: Vec<StreamCell>,
    /// Target group's during-fault p50 as a percentage of the control group's,
    /// in the same window. The headline. Reported whether or not it breaches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub during_fault_percent: Option<f64>,
    /// Target group's post-fault p50 as a percentage of its own baseline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_recovery_percent: Option<f64>,
    pub findings: Vec<ResolutionFinding>,
}

impl ResolutionReport {
    pub fn has_violations(&self) -> bool {
        !self.findings.is_empty()
    }

    pub fn cell(&self, group: Group, window: Window) -> Option<&StreamCell> {
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
    /// The comparison goes here on every run, including the clean ones. It is
    /// the only line that says what the run looked at, and this is a scenario
    /// where a clean report and a report of nothing read identically.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        match self.during_fault_percent {
            Some(percent) => lines.push(format!(
                "{SCENARIO}: with {} unresolvable on the target executor, its quota p50 ran at \
                 {percent}% of the control executor's over the same window (ceiling {}%). The \
                 executor's shard-manager channel never expires, so the expected reading is \
                 about 100",
                self.poisoned_name, self.degradation_ceiling_percent
            )),
            None => lines.push(format!(
                "{SCENARIO}: no during-fault quota comparison could be made — one of the two \
                 executor groups confirmed nothing in that window, so the run says nothing about \
                 whether {} mattered",
                self.poisoned_name
            )),
        }

        if let Some(percent) = self.quota_recovery_percent {
            lines.push(format!(
                "{SCENARIO}: the target executor's quota p50 settled at {percent}% of its own \
                 baseline after the name came back (floor {}%)",
                self.recovery_floor_percent
            ));
        }

        lines
    }
}

/// Everything the caller has to decide, kept out of the suite YAML's way.
#[derive(Debug, Clone)]
pub struct ResolutionInputs<'a> {
    pub split: &'a PodSplit,
    pub fault: Option<FaultWindow>,
    pub poisoned_name: String,
    pub degradation_ceiling_percent: f64,
    pub recovery_floor_percent: f64,
}

/// Builds the account.
pub fn build(records: &[OperationRecord], inputs: ResolutionInputs<'_>) -> ResolutionReport {
    let cells = split::stream_cells(records, Stream::Quota, inputs.split, inputs.fault);

    let mut report = ResolutionReport {
        poisoned_name: inputs.poisoned_name,
        degradation_ceiling_percent: inputs.degradation_ceiling_percent,
        recovery_floor_percent: inputs.recovery_floor_percent,
        during_fault_percent: None,
        quota_recovery_percent: recovery_percent(&cells),
        cells,
        findings: Vec::new(),
    };

    report.during_fault_percent = during_fault_percent(&report);
    report.findings = findings(&report);
    report
}

/// Target group's during-fault p50 against the control group's, same window.
///
/// `None` when either side confirmed nothing in the window, or when the control
/// side's p50 was zero. A percentage of nothing is not a comparison, and one
/// reported anyway would be read as evidence.
fn during_fault_percent(report: &ResolutionReport) -> Option<f64> {
    let control = report
        .cell(Group::Elsewhere, Window::DuringFault)?
        .latency
        .p50_ms as f64;
    let target = report
        .cell(Group::OnPod, Window::DuringFault)?
        .latency
        .p50_ms as f64;
    (control > 0.0).then(|| round2(100.0 * target / control))
}

fn findings(report: &ResolutionReport) -> Vec<ResolutionFinding> {
    let mut findings = Vec::new();

    if let Some(percent) = report.during_fault_percent
        && percent > report.degradation_ceiling_percent
    {
        let target = report.cell(Group::OnPod, Window::DuringFault);
        let control = report.cell(Group::Elsewhere, Window::DuringFault);
        findings.push(ResolutionFinding {
            violation: ResolutionViolation::QuotaDegraded,
            detail: format!(
                "the executor that could not resolve {} ran its quota work at {percent}% of the \
                 executor that could ({}ms against {}ms at p50), over a ceiling of {}%. The \
                 shard-manager channel is built once with an infinite idle TTL, so a cost here \
                 means something re-resolved the name — which is what MF2 asks about on purpose",
                report.poisoned_name,
                target.map(|c| c.latency.p50_ms).unwrap_or_default(),
                control.map(|c| c.latency.p50_ms).unwrap_or_default(),
                report.degradation_ceiling_percent,
            ),
        });
    }

    if let Some(percent) = report.quota_recovery_percent
        && percent > report.recovery_floor_percent
    {
        findings.push(ResolutionFinding {
            violation: ResolutionViolation::QuotaDidNotRecover,
            detail: format!(
                "the target executor's quota p50 is still at {percent}% of its own baseline after \
                 {} resolved again, against a floor of {}%",
                report.poisoned_name, report.recovery_floor_percent
            ),
        });
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::history::{Outcome, Phase};
    use chrono::{DateTime, Utc};
    use std::collections::BTreeMap;
    use test_r::test;

    const CEILING: f64 = 130.0;
    const FLOOR: f64 = 150.0;
    const NAME: &str = "shard-manager.golem-release.svc.cluster.local";

    fn at(offset_secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + offset_secs, 0).unwrap()
    }

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
            on_pod: vec!["target-a".to_string()],
            elsewhere: vec!["control-a".to_string()],
            targets_per_pod: BTreeMap::new(),
            number_of_shards: 1024,
        }
    }

    fn op(agent: &str, submitted_secs: i64, duration_ms: u64) -> OperationRecord {
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

    fn build_with(records: &[OperationRecord]) -> ResolutionReport {
        build(
            records,
            ResolutionInputs {
                split: &split(),
                fault: Some(fault()),
                poisoned_name: NAME.to_string(),
                degradation_ceiling_percent: CEILING,
                recovery_floor_percent: FLOOR,
            },
        )
    }

    /// The expected result, and the one that has to stay quiet.
    #[test]
    fn two_executors_that_perform_alike_under_the_fault_produce_no_finding() {
        let records: Vec<OperationRecord> = (0..10)
            .flat_map(|i| {
                [
                    op("target-a", 120 + i, 20),
                    op("control-a", 120 + i, 20),
                    op("target-a", 10 + i, 20),
                    op("target-a", 220 + i, 20),
                ]
            })
            .collect();
        let report = build_with(&records);
        assert_eq!(report.during_fault_percent, Some(100.0));
        assert!(!report.has_violations(), "{:?}", report.findings);
    }

    /// The result that would make S4 interesting, and the one MF2 is built to
    /// produce on purpose.
    #[test]
    fn a_target_executor_slower_than_the_control_is_a_finding() {
        let records: Vec<OperationRecord> = (0..10)
            .flat_map(|i| [op("target-a", 120 + i, 200), op("control-a", 120 + i, 20)])
            .collect();
        let report = build_with(&records);
        assert_eq!(report.during_fault_percent, Some(1000.0));
        assert_eq!(
            report.findings.first().map(|f| f.violation),
            Some(ResolutionViolation::QuotaDegraded)
        );
    }

    /// A comparison that cannot be made must not be reported as one that was.
    #[test]
    fn a_control_group_that_did_nothing_yields_no_comparison_and_no_finding() {
        let records: Vec<OperationRecord> = (0..10).map(|i| op("target-a", 120 + i, 200)).collect();
        let report = build_with(&records);
        assert_eq!(report.during_fault_percent, None);
        assert!(!report.has_violations(), "{:?}", report.findings);
        assert!(
            report
                .note_lines()
                .iter()
                .any(|line| line.contains("no during-fault quota comparison")),
            "the run must say it could not compare, rather than say nothing"
        );
    }

    /// Latency that stays high after the name comes back is a second finding,
    /// separate from the during-fault one.
    #[test]
    fn latency_that_never_returns_to_baseline_is_recorded_on_its_own() {
        let mut records: Vec<OperationRecord> =
            (0..10).map(|i| op("target-a", 10 + i, 20)).collect();
        records.extend((0..10).map(|i| op("target-a", 220 + i, 400)));
        let report = build_with(&records);
        assert_eq!(report.quota_recovery_percent, Some(2000.0));
        assert_eq!(
            report.findings.first().map(|f| f.violation),
            Some(ResolutionViolation::QuotaDidNotRecover)
        );
    }

    /// The comparison is stated on a clean run too, because a clean report and
    /// a report of nothing are otherwise the same document.
    #[test]
    fn a_clean_run_still_says_what_it_compared() {
        let records: Vec<OperationRecord> = (0..4)
            .flat_map(|i| [op("target-a", 120 + i, 20), op("control-a", 120 + i, 20)])
            .collect();
        let report = build_with(&records);
        assert!(report.attention_lines().is_empty());
        assert!(
            report
                .note_lines()
                .iter()
                .any(|line| line.contains(NAME) && line.contains("100%")),
            "notes were {:?}",
            report.note_lines()
        );
    }
}
