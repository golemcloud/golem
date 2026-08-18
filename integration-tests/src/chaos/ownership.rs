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

//! The shard-ownership oracle (GOL-364).
//!
//! An agent lives on exactly one executor. If two executors both believe they
//! own its shard, both will happily run its invocations, and its state forks —
//! there is no reconciliation and no way back. That is the failure S1 exists to
//! rule out, and it is the one thing here that is *asserted* rather than
//! reported.
//!
//! ### What fails a run, and what does not
//!
//! | Finding | Fails? | Why |
//! | -- | -- | -- |
//! | [`Violation::OverlappingOwnership`] | **Yes** | Two owners for one shard is never correct, at any instant. There is no window in which it is a transient. |
//! | [`Violation::UnownedShard`] | No | A shard with no owner is unroutable, which is real — but a rebalance legitimately passes through gaps, and asserting on one would make the run flaky about a property it cannot cleanly time. |
//! | [`Violation::ShardCountDisagreement`] | No | Executors disagreeing about the cluster's shard count is serious, but it is a *cause* an operator investigates, not a verdict. |
//! | [`Violation::RoutingDivergence`] | No | The shard-manager's intent and an executor's belief differing is the definition of an unhealed partition. Expected mid-fault; worth reading afterwards. |
//!
//! The asymmetry is deliberate and it is the same judgement the rest of this
//! suite makes: assert only what can never legitimately be observed, report
//! everything an operator should weigh. Overlap is the only member of the first
//! category.
//!
//! ### Coverage is part of the verdict
//!
//! Overlap is detected by comparing executors against each other, so a verdict
//! computed while some executors were unreadable is a claim about a subset of
//! the cluster. That subset is carried on the report rather than left implicit:
//! "no overlap among the three executors we could reach" is a much weaker
//! statement than "no overlap", and a reader must be able to tell them apart.

use crate::chaos::executors::{ExecutorAssignment, ExecutorSample};
use golem_common::model::RoutingTable;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A way shard ownership can be wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Violation {
    /// Two or more executors believe they own the same shard. Agents on that
    /// shard can be run by either, so their state can fork.
    OverlappingOwnership,
    /// No executor believes it owns a shard. Agents on it are unroutable.
    UnownedShard,
    /// Executors disagree about how many shards the cluster has.
    ShardCountDisagreement,
    /// The shard-manager's routing table and an executor's own belief disagree
    /// about a shard.
    RoutingDivergence,
}

impl Violation {
    pub fn as_str(self) -> &'static str {
        match self {
            Violation::OverlappingOwnership => "overlapping-ownership",
            Violation::UnownedShard => "unowned-shard",
            Violation::ShardCountDisagreement => "shard-count-disagreement",
            Violation::RoutingDivergence => "routing-divergence",
        }
    }

    /// Whether observing this fails the run.
    ///
    /// Only overlapping ownership does. See the module documentation for why
    /// the other three are reported instead.
    pub fn is_fatal(self) -> bool {
        matches!(self, Violation::OverlappingOwnership)
    }
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One thing found wrong, with the evidence for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipFinding {
    pub violation: Violation,
    /// The shard concerned, when the finding is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shard_id: Option<i64>,
    /// The executors involved, so a finding points at pods rather than at the
    /// cluster.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executors: Vec<String>,
    pub detail: String,
}

/// The ownership analysis of one sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipReport {
    /// Which sample this is, e.g. `before-fault`, `during-fault`,
    /// `after-settle`.
    pub at: String,
    pub taken_at: chrono::DateTime<chrono::Utc>,
    /// Executors that answered and identified themselves correctly.
    pub executors_analysed: usize,
    /// Executors the sample covered but could not use, by pod name. Non-empty
    /// means every verdict below is about a subset of the cluster.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executors_excluded: Vec<String>,
    /// The cluster shard count, when every analysed executor agreed on one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_of_shards: Option<usize>,
    /// Distinct shards claimed by at least one executor.
    pub shards_claimed: usize,
    /// How many shards each executor claimed, for a quick read of the spread.
    pub shards_per_executor: BTreeMap<String, usize>,
    pub findings: Vec<OwnershipFinding>,
    /// Whether this is the sample the run is *judged* on.
    ///
    /// Only one sample is. The others are context: before the fault, so a
    /// reader knows what "normal" looked like, and during it, so a reader can
    /// see the partition actually took hold. Marking which is which in the
    /// artifact means a later re-analysis does not have to infer it from the
    /// label.
    pub judged: bool,
    /// The raw sample, kept so an archived run can be re-analysed later without
    /// re-running anything.
    pub sample: ExecutorSample,
}

impl OwnershipReport {
    /// Analyses one sample, optionally cross-checked against the
    /// shard-manager's routing table.
    ///
    /// The routing table is optional because during a shard-manager fault it is
    /// legitimately unreadable, and the executor-versus-executor checks — which
    /// include the only fatal one — do not need it.
    pub fn build(sample: ExecutorSample, routing: Option<&RoutingTable>, judged: bool) -> Self {
        let usable: Vec<&ExecutorAssignment> = sample.usable().collect();
        let excluded: Vec<String> = sample
            .executors
            .iter()
            .filter(|e| !e.is_usable())
            .map(|e| e.pod_name.clone())
            .collect();

        let mut findings = Vec::new();

        // ── Who claims what ─────────────────────────────────────────────────
        let mut claimants: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        let mut shards_per_executor = BTreeMap::new();
        for executor in &usable {
            shards_per_executor.insert(executor.pod_name.clone(), executor.shard_ids.len());
            for shard in &executor.shard_ids {
                claimants
                    .entry(*shard)
                    .or_default()
                    .push(executor.pod_name.clone());
            }
        }

        for (shard, owners) in &claimants {
            if owners.len() > 1 {
                findings.push(OwnershipFinding {
                    violation: Violation::OverlappingOwnership,
                    shard_id: Some(*shard),
                    executors: owners.clone(),
                    detail: format!(
                        "shard {shard} is claimed by {} executors ({}) — agents on it have more \
                         than one owner and their state can fork",
                        owners.len(),
                        owners.join(", ")
                    ),
                });
            }
        }

        // ── Do they agree on the size of the cluster? ───────────────────────
        let counts: BTreeSet<usize> = usable.iter().filter_map(|e| e.number_of_shards).collect();
        let number_of_shards = if counts.len() == 1 {
            counts.iter().next().copied()
        } else {
            if counts.len() > 1 {
                findings.push(OwnershipFinding {
                    violation: Violation::ShardCountDisagreement,
                    shard_id: None,
                    executors: usable.iter().map(|e| e.pod_name.clone()).collect(),
                    detail: format!(
                        "executors report different cluster shard counts ({counts:?}); every \
                         ownership comparison below is between executors that do not agree on \
                         what they are dividing up"
                    ),
                });
            }
            None
        };

        // ── Is anything unowned? ────────────────────────────────────────────
        // Only meaningful once the executors agree on a shard count and at
        // least one of them has been assigned: otherwise "every shard is
        // unowned" is a statement about the sample, not about the cluster.
        if let Some(total) = number_of_shards
            && usable.iter().any(|e| e.assigned)
        {
            let unowned: Vec<i64> = (0..total as i64)
                .filter(|shard| !claimants.contains_key(shard))
                .collect();
            if !unowned.is_empty() {
                findings.push(OwnershipFinding {
                    violation: Violation::UnownedShard,
                    // A gap is usually a range, so naming one shard would be
                    // misleading; the count and the first few go in the detail.
                    shard_id: None,
                    executors: Vec::new(),
                    detail: format!(
                        "{} of {total} shards are claimed by no executor (first: {:?}) — agents \
                         on them are unroutable until something takes them over",
                        unowned.len(),
                        unowned.iter().take(8).collect::<Vec<_>>()
                    ),
                });
            }
        }

        // ── Does the shard-manager agree with them? ─────────────────────────
        if let Some(routing) = routing {
            findings.extend(routing_divergence(&usable, routing));
        }

        Self {
            at: sample.at.clone(),
            taken_at: sample.taken_at,
            executors_analysed: usable.len(),
            executors_excluded: excluded,
            number_of_shards,
            shards_claimed: claimants.len(),
            shards_per_executor,
            findings,
            judged,
            sample,
        }
    }

    /// Findings that fail the run.
    pub fn fatal_findings(&self) -> impl Iterator<Item = &OwnershipFinding> {
        self.findings.iter().filter(|f| f.violation.is_fatal())
    }

    pub fn has_fatal_findings(&self) -> bool {
        self.fatal_findings().next().is_some()
    }

    /// Whether this report covers the whole cluster it sampled.
    ///
    /// A clean verdict over partial coverage is worth strictly less than a
    /// clean verdict over full coverage, and callers have to be able to say so.
    pub fn is_complete(&self) -> bool {
        self.executors_excluded.is_empty() && self.executors_analysed > 0
    }

    /// One-line summaries for the operator-facing attention list.
    ///
    /// Deliberately not every finding from every sample. While the fault is
    /// active the two sides are *supposed* to disagree — that is the fault
    /// working — so hoisting mid-fault divergence and gaps would put an
    /// expected observation under a warning banner on every single run, which
    /// is how a warning banner stops being read.
    ///
    /// What is hoisted: anything fatal, wherever it was seen, because two
    /// owners for one shard is never expected at any point; and everything from
    /// the judged sample, because that one is about the state the cluster
    /// settled into. The rest stays in the artifact.
    pub fn attention_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .findings
            .iter()
            .filter(|f| self.judged || f.violation.is_fatal())
            .map(|f| format!("{} at {}: {}", f.violation, self.at, f.detail))
            .collect();
        if self.judged && !self.executors_excluded.is_empty() {
            lines.push(format!(
                "ownership sample {} covered only {} executors; {} could not be used ({})",
                self.at,
                self.executors_analysed,
                self.executors_excluded.len(),
                self.executors_excluded.join(", ")
            ));
        }
        lines
    }
}

/// Compares the shard-manager's intent against each executor's belief.
///
/// Matched on pod IP, because the routing table names executors as `ip:port`
/// while the endpoints file names them as pods.
///
/// An executor the table lists but the sample did not cover is deliberately
/// *not* reported: that is a gap in the sample, already carried by
/// `executors_excluded`, and reporting it as divergence would turn every
/// partial sample into a pile of findings about the driver's own reach.
fn routing_divergence(
    usable: &[&ExecutorAssignment],
    routing: &RoutingTable,
) -> Vec<OwnershipFinding> {
    let intended: BTreeMap<String, BTreeSet<i64>> = routing
        .shard_ids_per_pod()
        .into_iter()
        .map(|(pod, shards)| (pod.ip.to_string(), shards))
        .collect();

    let mut findings = Vec::new();
    for executor in usable {
        let believes: BTreeSet<i64> = executor.shard_ids.iter().copied().collect();

        let Some(assigned) = intended.get(&executor.pod_ip) else {
            // The executor is serving shards nobody is routing to it. During a
            // partition this is the expected shape of the fault; afterwards it
            // means an executor never re-registered.
            if !believes.is_empty() {
                findings.push(OwnershipFinding {
                    violation: Violation::RoutingDivergence,
                    shard_id: None,
                    executors: vec![executor.pod_name.clone()],
                    detail: format!(
                        "executor {} ({}) believes it owns {} shards, but the shard-manager's \
                         routing table does not list it at all",
                        executor.pod_name,
                        executor.pod_ip,
                        believes.len()
                    ),
                });
            }
            continue;
        };

        let stale: Vec<i64> = believes.difference(assigned).copied().collect();
        let missed: Vec<i64> = assigned.difference(&believes).copied().collect();
        if stale.is_empty() && missed.is_empty() {
            continue;
        }

        findings.push(OwnershipFinding {
            violation: Violation::RoutingDivergence,
            shard_id: None,
            executors: vec![executor.pod_name.clone()],
            detail: format!(
                "executor {} disagrees with the routing table: it still believes it owns {} \
                 shards the shard-manager has moved away (first: {:?}), and has not picked up {} \
                 shards the shard-manager routes to it (first: {:?})",
                executor.pod_name,
                stale.len(),
                stale.iter().take(8).collect::<Vec<_>>(),
                missed.len(),
                missed.iter().take(8).collect::<Vec<_>>()
            ),
        });
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chaos::executors::ExecutorAssignment;
    use chrono::Utc;
    use test_r::test;

    fn executor(pod: &str, shards: &[i64], total: usize) -> ExecutorAssignment {
        ExecutorAssignment {
            pod_name: pod.to_string(),
            pod_ip: format!("10.0.0.{}", pod.len()),
            reported_executor_id: Some(pod.to_string()),
            identity_mismatch: None,
            assigned: true,
            number_of_shards: Some(total),
            shard_ids: shards.to_vec(),
            read_error: None,
        }
    }

    fn sample(executors: Vec<ExecutorAssignment>) -> ExecutorSample {
        ExecutorSample {
            at: "after-settle".to_string(),
            taken_at: Utc::now(),
            executors,
        }
    }

    /// A clean split: every shard owned exactly once.
    #[test]
    fn a_disjoint_covering_assignment_has_no_findings() {
        let report = OwnershipReport::build(
            sample(vec![
                executor("exec-a", &[0, 1], 4),
                executor("exec-b", &[2, 3], 4),
            ]),
            None,
            true,
        );
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(!report.has_fatal_findings());
        assert!(report.is_complete());
        assert_eq!(report.shards_claimed, 4);
        assert_eq!(report.number_of_shards, Some(4));
    }

    /// The defect the scenario exists to rule out, and the only fatal one.
    #[test]
    fn two_executors_claiming_one_shard_is_a_fatal_finding() {
        let report = OwnershipReport::build(
            sample(vec![
                executor("exec-a", &[0, 1, 2], 4),
                executor("exec-b", &[2, 3], 4),
            ]),
            None,
            true,
        );
        assert!(report.has_fatal_findings());
        let fatal: Vec<&OwnershipFinding> = report.fatal_findings().collect();
        assert_eq!(fatal.len(), 1);
        assert_eq!(fatal[0].violation, Violation::OverlappingOwnership);
        assert_eq!(fatal[0].shard_id, Some(2));
        assert_eq!(fatal[0].executors, vec!["exec-a", "exec-b"]);
    }

    /// A gap is real and is reported, but a rebalance legitimately passes
    /// through one, so it must not fail the run.
    #[test]
    fn an_unowned_shard_is_reported_but_does_not_fail_the_run() {
        let report = OwnershipReport::build(
            sample(vec![
                executor("exec-a", &[0, 1], 4),
                executor("exec-b", &[2], 4),
            ]),
            None,
            true,
        );
        assert!(!report.has_fatal_findings());
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].violation, Violation::UnownedShard);
        assert!(report.findings[0].detail.contains("[3]"));
    }

    #[test]
    fn executors_disagreeing_about_the_cluster_size_is_reported() {
        let report = OwnershipReport::build(
            sample(vec![
                executor("exec-a", &[0, 1], 4),
                executor("exec-b", &[2, 3], 8),
            ]),
            None,
            true,
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == Violation::ShardCountDisagreement)
        );
        assert_eq!(
            report.number_of_shards, None,
            "no agreed shard count means the unowned check must not run at all"
        );
        assert!(!report.has_fatal_findings());
    }

    /// A verdict computed while some executors were unreadable is a claim about
    /// a subset, and the report has to say so.
    #[test]
    fn an_excluded_executor_makes_the_report_incomplete() {
        let mut unreadable = executor("exec-b", &[], 4);
        unreadable.read_error = Some("connection refused".to_string());
        let report = OwnershipReport::build(
            sample(vec![executor("exec-a", &[0, 1], 4), unreadable]),
            None,
            true,
        );

        assert!(!report.is_complete());
        assert_eq!(report.executors_analysed, 1);
        assert_eq!(report.executors_excluded, vec!["exec-b"]);
        assert!(
            report
                .attention_lines()
                .iter()
                .any(|l| l.contains("could not be used")),
            "partial coverage has to reach the attention list, not just the JSON"
        );
    }

    /// Before any executor has registered, "every shard is unowned" is a fact
    /// about the sample rather than about the cluster.
    #[test]
    fn nothing_is_reported_unowned_when_no_executor_has_been_assigned_yet() {
        let mut fresh = executor("exec-a", &[], 4);
        fresh.assigned = false;
        let report = OwnershipReport::build(sample(vec![fresh]), None, true);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    /// While the fault is active the two sides are supposed to disagree, so a
    /// mid-fault gap must not raise a banner on every run.
    #[test]
    fn an_unjudged_sample_keeps_its_expected_findings_out_of_the_attention_list() {
        let mut sample = sample(vec![
            executor("exec-a", &[0, 1], 4),
            executor("exec-b", &[2], 4),
        ]);
        sample.at = "during-fault".to_string();
        let report = OwnershipReport::build(sample, None, false);

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.violation == Violation::UnownedShard),
            "the finding still has to be in the artifact"
        );
        assert!(
            report.attention_lines().is_empty(),
            "but not in front of the operator: {:?}",
            report.attention_lines()
        );
    }

    /// Overlap is the exception: there is no point in a run at which two
    /// executors owning one shard is an expected observation.
    #[test]
    fn an_unjudged_sample_still_hoists_overlap() {
        let mut sample = sample(vec![
            executor("exec-a", &[0, 1, 2], 4),
            executor("exec-b", &[2, 3], 4),
        ]);
        sample.at = "during-fault".to_string();
        let report = OwnershipReport::build(sample, None, false);

        assert!(
            report
                .attention_lines()
                .iter()
                .any(|l| l.contains("overlapping-ownership")),
            "{:?}",
            report.attention_lines()
        );
    }

    /// Overlap is fatal wherever it is seen — there is no window in which two
    /// owners for one shard is a legitimate transient.
    #[test]
    fn overlap_is_fatal_and_the_other_violations_are_not() {
        assert!(Violation::OverlappingOwnership.is_fatal());
        assert!(!Violation::UnownedShard.is_fatal());
        assert!(!Violation::ShardCountDisagreement.is_fatal());
        assert!(!Violation::RoutingDivergence.is_fatal());
    }
}
