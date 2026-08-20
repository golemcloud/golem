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
//! readable: how much moved, when, and whether the cluster ended up covering
//! every shard. A duplicate execution with no reassignment anywhere near it is a
//! different bug from one that happened during a handover.
//!
//! ### Counts, not shard ids
//!
//! [`RoutingTable::shards_per_pod`] is the whole input. The shard ids behind
//! those counts are not public, and asking for them would mean widening a shared
//! library for context that is not load-bearing — the verdict comes from the
//! probe either way.
//!
//! The cost is precise: counts cannot see a *symmetric* reshuffle, where two
//! executors swap equal numbers of shards. Movement is therefore reported as a
//! lower bound and labelled as one. For the fault this scenario injects that
//! costs nothing real — a partitioned executor's shards move to the executors
//! that stayed reachable, which always changes the counts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

fn is_zero(n: &usize) -> bool {
    *n == 0
}

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
    /// How many shards each executor is assigned.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shards_per_executor: BTreeMap<String, usize>,
    /// How many shards the table assigns to nobody.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unassigned_shards: usize,
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
        routing: Option<&golem_common::model::RoutingTable>,
        previous: Option<&OwnershipSample>,
        settled: bool,
    ) -> Self {
        let Some(routing) = routing else {
            return Self {
                at: at.to_string(),
                taken_at: chrono::Utc::now(),
                number_of_shards: None,
                shards_per_executor: BTreeMap::new(),
                unassigned_shards: 0,
                unavailable_reason: Some("shard-manager unreachable".to_string()),
                settled,
                findings: Vec::new(),
            };
        };

        let total = routing.number_of_shards.value;
        let shards_per_executor: BTreeMap<String, usize> = routing
            .shards_per_pod()
            .into_iter()
            .map(|(pod, count)| (pod.to_string(), count))
            .collect();

        let assigned: usize = shards_per_executor.values().sum();
        let unassigned = total.saturating_sub(assigned);

        let mut findings = Vec::new();
        if unassigned > 0 {
            findings.push(OwnershipFinding {
                finding: Finding::UnassignedShards,
                executors: Vec::new(),
                detail: format!(
                    "{unassigned} of {total} shards are assigned to no executor — agents on them \
                     are unroutable until something takes them over"
                ),
            });
        }

        let sample = Self {
            at: at.to_string(),
            taken_at: chrono::Utc::now(),
            number_of_shards: Some(total),
            shards_per_executor,
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

        let before: BTreeSet<&String> = previous.shards_per_executor.keys().collect();
        let after: BTreeSet<&String> = self.shards_per_executor.keys().collect();
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
                    "at least {moved} shards changed executor since {} — the handover any \
                     duplicate-execution finding has to be read against",
                    previous.at
                ),
            });
        }
        self
    }

    /// A lower bound on how many shards changed executor since `previous`.
    ///
    /// A lower bound, not a count, and the distinction is real: this is derived
    /// from per-executor totals, so two executors swapping equal numbers of
    /// shards is invisible. See the module docs for why that trade is worth
    /// making. Every caller and every rendered string says "at least".
    pub fn shards_moved_since(&self, previous: &OwnershipSample) -> usize {
        self.shards_per_executor
            .iter()
            .map(|(pod, now)| match previous.shards_per_executor.get(pod) {
                // An executor that gained shards gained them from somewhere.
                Some(was) => now.saturating_sub(*was),
                // An executor absent from the earlier sample holds nothing but
                // shards that moved to it — the shape a rejoining executor
                // takes, and the one a `get(pod)?` would silently score as
                // zero movement.
                None => *now,
            })
            .sum()
    }

    /// Executors the shard-manager is currently routing to.
    ///
    /// Only executors holding at least one shard appear: the routing table is a
    /// shard→pod map, so a registered executor that owns nothing has no entry.
    /// That absence is the point — an executor with no shards is one a partition
    /// can be aimed at without moving anything.
    pub fn executors_with_shards(&self) -> usize {
        self.shards_per_executor.len()
    }

    /// Whether this sample assigns shards exactly as `other` does.
    ///
    /// Used to notice that a fault changed nothing. Compares per-executor
    /// totals, so it inherits the same blind spot as
    /// [`Self::shards_moved_since`]: a symmetric swap reads as unchanged.
    pub fn assignment_matches(&self, other: &OwnershipSample) -> bool {
        self.unavailable_reason.is_none()
            && other.unavailable_reason.is_none()
            && self.shards_per_executor == other.shards_per_executor
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

    fn sample(at: &str, settled: bool, pods: &[(&str, usize)], total: usize) -> OwnershipSample {
        let shards_per_executor: BTreeMap<String, usize> = pods
            .iter()
            .map(|(pod, n)| ((*pod).to_string(), *n))
            .collect();
        let assigned: usize = shards_per_executor.values().sum();
        let unassigned = total.saturating_sub(assigned);
        let mut findings = Vec::new();
        if unassigned > 0 {
            findings.push(OwnershipFinding {
                finding: Finding::UnassignedShards,
                executors: Vec::new(),
                detail: format!("{unassigned} of {total} shards are assigned to no executor"),
            });
        }
        OwnershipSample {
            at: at.to_string(),
            taken_at: Utc::now(),
            number_of_shards: Some(total),
            shards_per_executor,
            unassigned_shards: unassigned,
            unavailable_reason: None,
            settled,
            findings,
        }
    }

    #[test]
    fn a_fully_covered_table_reports_nothing_to_attend_to() {
        let s = sample("after-settle", true, &[("a", 2), ("b", 2)], 4);
        assert_eq!(s.unassigned_shards, 0);
        assert!(s.attention_lines().is_empty());
    }

    /// A gap is real and worth surfacing — but only once the cluster has
    /// settled. While the fault is active it is the fault working.
    #[test]
    fn a_gap_reaches_the_operator_only_from_the_settled_sample() {
        let settled = sample("after-settle", true, &[("a", 2), ("b", 1)], 4);
        assert_eq!(settled.unassigned_shards, 1);
        assert_eq!(settled.attention_lines().len(), 1);

        let mid_fault = OwnershipSample {
            settled: false,
            ..settled
        };
        assert!(mid_fault.attention_lines().is_empty());
    }

    /// Movement is a lower bound, and the report says so. A symmetric swap is
    /// exactly the case counts cannot see — worth a test so nobody later reads
    /// a `0` here as "nothing moved".
    #[test]
    fn a_rejoining_executors_shards_all_count_as_movement() {
        let before = sample("before-fault", false, &[("a", 4)], 4);
        let after = sample("after-settle", true, &[("a", 2), ("b", 2)], 4);
        assert_eq!(
            after.shards_moved_since(&before),
            2,
            "an executor absent from the earlier sample holds only shards that moved to it"
        );
    }

    #[test]
    fn movement_is_a_lower_bound_and_a_symmetric_swap_is_invisible() {
        let before = sample("before-fault", false, &[("a", 2), ("b", 2)], 4);
        let swapped = sample("after-settle", true, &[("a", 2), ("b", 2)], 4);
        assert_eq!(
            swapped.shards_moved_since(&before),
            0,
            "equal counts cannot reveal a swap — the probe is what catches the harm"
        );

        let drained = sample("before-fault", false, &[("a", 4), ("b", 0)], 4);
        let rebalanced = sample("after-settle", true, &[("a", 1), ("b", 3)], 4);
        assert_eq!(
            rebalanced.shards_moved_since(&drained),
            3,
            "the shape a partition actually produces is visible in the counts"
        );
    }

    /// Reassignment after a fault is recovery working. It is recorded, because
    /// a duplicate execution has to be read against the handover that produced
    /// it, but it is not something to wave at an operator.
    #[test]
    fn reassignment_is_recorded_without_reaching_the_attention_list() {
        let before = sample("before-fault", false, &[("a", 4)], 4);
        let after = sample("after-settle", true, &[("a", 2), ("b", 2)], 4)
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
        let before = sample("before-fault", false, &[("a", 4)], 4);
        let during = OwnershipSample::from_routing("during-fault", None, Some(&before), false);
        assert!(during.findings.is_empty());
    }
}
