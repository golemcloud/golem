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

//! Shard assignment through a fault, as the shard-manager reports it (GOL-364).
//!
//! Everything here comes from `GetRoutingTable` over the port-forward the suite
//! already opens for every run. No new surface anywhere, and nothing added to
//! the worker-executor.
//!
//! ### What this can and cannot see
//!
//! The routing table is the shard-manager's *intent*: one pod per shard, by
//! construction. It can show a shard nobody owns, a redistribution, an executor
//! appearing or vanishing — and it can show all of that looking perfectly
//! healthy while a cut-off executor quietly disagrees, because such an executor
//! keeps serving what it was last told about and the table has no idea.
//!
//! So overlapping ownership is **not detectable here, and this module does not
//! pretend otherwise.** It is caught by its consequence instead: if two
//! executors both served an agent, one idempotency key executed twice, and the
//! exactly-once probe in [`crate::chaos::probe`] observes that exactly, per key,
//! from outside the cluster. That is the stronger claim anyway — evidence the
//! platform actually did the harmful thing, rather than evidence it was in a
//! position to.
//!
//! What this module contributes is the context that makes such a finding
//! readable: which shards moved, when, and whether the cluster ended up covering
//! all of them. A duplicate execution with no reassignment anywhere near it is a
//! different bug from one that happened during a handover.

use golem_common::model::RoutingTable;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Something about the shard-manager's assignment worth an operator's
/// attention.
///
/// None of these fail a run. A rebalance legitimately passes through every one
/// of them, and asserting on a state the cluster is *supposed* to move through
/// would make the run flaky about a property it cannot cleanly time. The one
/// assertion S1 makes lives in the exactly-once probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Finding {
    /// Shards the shard-manager has assigned to nobody. Agents on them are
    /// unroutable until something takes them over.
    UnassignedShards,
    /// The set of executors in the table changed between two samples.
    ExecutorSetChanged,
    /// Shards moved from one executor to another between two samples. Expected
    /// after a fault — this is recovery working — and recorded so a duplicate
    /// execution can be read against the handover that produced it.
    ShardsReassigned,
}

impl Finding {
    pub fn as_str(self) -> &'static str {
        match self {
            Finding::UnassignedShards => "unassigned-shards",
            Finding::ExecutorSetChanged => "executor-set-changed",
            Finding::ShardsReassigned => "shards-reassigned",
        }
    }
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One observation, with the evidence for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipFinding {
    pub finding: Finding,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executors: Vec<String>,
    pub detail: String,
}

/// The shard-manager's assignment at one instant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipSample {
    /// Which sample this is, e.g. `before-fault`, `during-fault`,
    /// `after-settle`.
    pub at: String,
    pub taken_at: chrono::DateTime<chrono::Utc>,
    /// `None` when the shard-manager could not be reached — expected during a
    /// fault aimed at it, and recorded rather than treated as an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_of_shards: Option<usize>,
    /// Which shards each executor is assigned.
    ///
    /// Kept in full rather than counted: the counts can be identical across a
    /// complete reshuffle, and *which* shards moved is exactly what a later
    /// finding has to be read against.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shard_ids_per_executor: BTreeMap<String, Vec<i64>>,
    /// Shards the table assigns to nobody.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unassigned_shards: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Whether this is the sample taken after the settling window — the one the
    /// cluster's final state is read from.
    pub settled: bool,
    pub findings: Vec<OwnershipFinding>,
}

impl OwnershipSample {
    /// Reads the assignment out of a routing table.
    ///
    /// `previous` supplies the findings that only exist relative to an earlier
    /// sample — what moved, and who joined or left.
    pub fn from_routing(
        at: &str,
        routing: Option<&RoutingTable>,
        previous: Option<&OwnershipSample>,
        settled: bool,
    ) -> Self {
        let Some(routing) = routing else {
            return Self {
                at: at.to_string(),
                taken_at: chrono::Utc::now(),
                number_of_shards: None,
                shard_ids_per_executor: BTreeMap::new(),
                unassigned_shards: Vec::new(),
                unavailable_reason: Some("shard-manager unreachable".to_string()),
                settled,
                findings: Vec::new(),
            };
        };

        let total = routing.number_of_shards.value;
        let shard_ids_per_executor: BTreeMap<String, Vec<i64>> = routing
            .shard_ids_per_pod()
            .into_iter()
            .map(|(pod, shards)| (pod.to_string(), shards.into_iter().collect()))
            .collect();

        let assigned: BTreeSet<i64> = shard_ids_per_executor
            .values()
            .flat_map(|shards| shards.iter().copied())
            .collect();
        let unassigned: Vec<i64> = (0..total as i64)
            .filter(|shard| !assigned.contains(shard))
            .collect();

        let mut findings = Vec::new();
        if !unassigned.is_empty() {
            findings.push(OwnershipFinding {
                finding: Finding::UnassignedShards,
                executors: Vec::new(),
                detail: format!(
                    "{} of {total} shards are assigned to no executor (first: {:?}) — agents on \
                     them are unroutable until something takes them over",
                    unassigned.len(),
                    unassigned.iter().take(8).collect::<Vec<_>>()
                ),
            });
        }

        let sample = Self {
            at: at.to_string(),
            taken_at: chrono::Utc::now(),
            number_of_shards: Some(total),
            shard_ids_per_executor,
            unassigned_shards: unassigned,
            unavailable_reason: None,
            settled,
            findings,
        };
        sample.with_movement_since(previous)
    }

    /// Adds the findings that only exist relative to an earlier sample.
    fn with_movement_since(mut self, previous: Option<&OwnershipSample>) -> Self {
        let Some(previous) = previous else {
            return self;
        };
        // Nothing to compare against a sample that could not be taken.
        if previous.unavailable_reason.is_some() || self.unavailable_reason.is_some() {
            return self;
        }

        let before: BTreeSet<&String> = previous.shard_ids_per_executor.keys().collect();
        let after: BTreeSet<&String> = self.shard_ids_per_executor.keys().collect();
        if before != after {
            let gone: Vec<String> = before.difference(&after).map(|p| (*p).clone()).collect();
            let joined: Vec<String> = after.difference(&before).map(|p| (*p).clone()).collect();
            let list = |pods: &[String]| {
                if pods.is_empty() {
                    "none".to_string()
                } else {
                    pods.join(", ")
                }
            };
            self.findings.push(OwnershipFinding {
                finding: Finding::ExecutorSetChanged,
                executors: gone.iter().chain(joined.iter()).cloned().collect(),
                detail: format!(
                    "executors in the routing table changed since {}: {} left, {} joined",
                    previous.at,
                    list(&gone),
                    list(&joined)
                ),
            });
        }

        let moved = self.shards_moved_since(previous);
        if moved > 0 {
            self.findings.push(OwnershipFinding {
                finding: Finding::ShardsReassigned,
                executors: Vec::new(),
                detail: format!(
                    "{moved} shards changed executor since {} — the handover any \
                     duplicate-execution finding has to be read against",
                    previous.at
                ),
            });
        }
        self
    }

    /// How many shards are held by a different executor than in `previous`.
    pub fn shards_moved_since(&self, previous: &OwnershipSample) -> usize {
        let owner_of = |sample: &OwnershipSample| -> BTreeMap<i64, String> {
            sample
                .shard_ids_per_executor
                .iter()
                .flat_map(|(pod, shards)| shards.iter().map(move |s| (*s, pod.clone())))
                .collect()
        };
        let before = owner_of(previous);
        let after = owner_of(self);
        after
            .iter()
            .filter(|(shard, pod)| before.get(shard).is_some_and(|was| was != *pod))
            .count()
    }

    /// How many shards each executor holds, for a quick read of the spread.
    pub fn shards_per_executor(&self) -> BTreeMap<String, usize> {
        self.shard_ids_per_executor
            .iter()
            .map(|(pod, shards)| (pod.clone(), shards.len()))
            .collect()
    }

    /// One-line summaries for the operator-facing attention list.
    ///
    /// Only from the settled sample, and never for reassignment. While the fault
    /// is active the table is *supposed* to be in flux, and hoisting that on
    /// every run is how a warning banner stops being read.
    pub fn attention_lines(&self) -> Vec<String> {
        if !self.settled {
            return Vec::new();
        }
        self.findings
            .iter()
            .filter(|f| f.finding != Finding::ShardsReassigned)
            .map(|f| format!("{} at {}: {}", f.finding, self.at, f.detail))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use test_r::test;

    fn sample(at: &str, settled: bool, pods: &[(&str, &[i64])], total: usize) -> OwnershipSample {
        let shard_ids_per_executor: BTreeMap<String, Vec<i64>> = pods
            .iter()
            .map(|(pod, shards)| ((*pod).to_string(), shards.to_vec()))
            .collect();
        let assigned: BTreeSet<i64> = shard_ids_per_executor
            .values()
            .flat_map(|s| s.iter().copied())
            .collect();
        let unassigned: Vec<i64> = (0..total as i64)
            .filter(|s| !assigned.contains(s))
            .collect();
        let mut findings = Vec::new();
        if !unassigned.is_empty() {
            findings.push(OwnershipFinding {
                finding: Finding::UnassignedShards,
                executors: Vec::new(),
                detail: format!(
                    "{} of {total} shards are assigned to no executor",
                    unassigned.len()
                ),
            });
        }
        OwnershipSample {
            at: at.to_string(),
            taken_at: Utc::now(),
            number_of_shards: Some(total),
            shard_ids_per_executor,
            unassigned_shards: unassigned,
            unavailable_reason: None,
            settled,
            findings,
        }
    }

    #[test]
    fn a_fully_covered_table_reports_nothing_to_attend_to() {
        let s = sample("after-settle", true, &[("a", &[0, 1]), ("b", &[2, 3])], 4);
        assert!(s.unassigned_shards.is_empty());
        assert!(s.attention_lines().is_empty());
    }

    /// A gap is real and worth surfacing — but only once the cluster has
    /// settled. While the fault is active it is the fault working.
    #[test]
    fn a_gap_reaches_the_operator_only_from_the_settled_sample() {
        let settled = sample("after-settle", true, &[("a", &[0, 1]), ("b", &[2])], 4);
        assert_eq!(settled.unassigned_shards, vec![3]);
        assert_eq!(settled.attention_lines().len(), 1);

        let mid_fault = OwnershipSample {
            settled: false,
            ..settled
        };
        assert!(mid_fault.attention_lines().is_empty());
    }

    /// Counts alone cannot see a reshuffle, which is exactly why the sample
    /// keeps the shard ids rather than a tally.
    #[test]
    fn movement_is_measured_by_shard_not_by_count() {
        let before = sample("before-fault", false, &[("a", &[0, 1]), ("b", &[2, 3])], 4);
        let after = sample("after-settle", true, &[("a", &[2, 3]), ("b", &[0, 1])], 4);

        assert_eq!(
            before.shards_per_executor(),
            after.shards_per_executor(),
            "the counts are identical across a total reshuffle"
        );
        assert_eq!(after.shards_moved_since(&before), 4);
    }

    /// Reassignment after a fault is recovery working. It is recorded, because
    /// a duplicate execution has to be read against the handover that produced
    /// it, but it is not something to wave at an operator.
    #[test]
    fn reassignment_is_recorded_without_reaching_the_attention_list() {
        let before = sample("before-fault", false, &[("a", &[0, 1, 2, 3])], 4);
        let after = sample("after-settle", true, &[("a", &[0, 1]), ("b", &[2, 3])], 4)
            .with_movement_since(Some(&before));

        let kinds: Vec<Finding> = after.findings.iter().map(|f| f.finding).collect();
        assert!(kinds.contains(&Finding::ShardsReassigned));
        assert!(kinds.contains(&Finding::ExecutorSetChanged));
        assert!(
            !after
                .attention_lines()
                .iter()
                .any(|l| l.contains("shards-reassigned")),
            "shards moving after a fault is recovery working, not an alarm: {:?}",
            after.attention_lines()
        );
        // The executor set changing is a different matter. S1 restarts nothing,
        // so an executor that left or joined across the fault is worth saying
        // out loud.
        assert!(
            after
                .attention_lines()
                .iter()
                .any(|l| l.contains("executor-set-changed")),
            "{:?}",
            after.attention_lines()
        );
    }

    /// An unreachable shard-manager is an observation, not an error — during a
    /// fault aimed at it, the expected one.
    #[test]
    fn an_unreachable_shard_manager_is_recorded_rather_than_failing() {
        let s = OwnershipSample::from_routing("during-fault", None, None, false);
        assert_eq!(
            s.unavailable_reason.as_deref(),
            Some("shard-manager unreachable")
        );
        assert!(s.number_of_shards.is_none());
        assert!(s.findings.is_empty());
    }

    /// A sample that could not be taken must not manufacture movement findings
    /// against the one before it.
    #[test]
    fn an_unavailable_sample_is_not_compared_against_its_predecessor() {
        let before = sample("before-fault", false, &[("a", &[0, 1, 2, 3])], 4);
        let during = OwnershipSample::from_routing("during-fault", None, Some(&before), false);
        assert!(during.findings.is_empty());
    }
}
