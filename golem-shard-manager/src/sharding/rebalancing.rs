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

use super::model::{Assignments, ExecutorShards, ShardLeaseState, Unassignments};
use golem_common::model::ShardId;
use std::collections::HashSet;
use std::fmt;
use std::fmt::{Display, Formatter};
use tracing::trace;

#[derive(Clone, Debug)]
pub struct Rebalance {
    assignments: Assignments,
    unassignments: Unassignments,
}

impl Rebalance {
    pub fn new(assignments: Assignments, unassignments: Unassignments) -> Self {
        Rebalance {
            assignments,
            unassignments,
        }
    }

    /// Constructs a rebalance plan from the current shard lease state.
    ///
    /// The `threshold` parameter is used to reduce the number of shard reassignments by
    /// allowing a given number of shards to be over or under the optimal count per executor.
    ///
    /// The optimal count (balanced state) is number_of_shards/executor_count.
    /// Threshold is a percentage of the optimal count, so for 10 executors with 1000 shards,
    /// and a threshold of 10%, executors with shard count between 90 and 110 will be considered
    /// balanced.
    ///
    /// Executors are visited in `ExecutorId` order (see [`ShardLeaseState::executor_shard_sets`]).
    pub fn from_shard_state(shard_state: &ShardLeaseState, threshold: f64) -> Self {
        let mut assignments = Assignments::new();
        let mut unassignments = Unassignments::new();
        let executor_count = shard_state.executor_count();
        if executor_count == 0 {
            return Rebalance {
                assignments,
                unassignments,
            };
        }

        let mut executors: Vec<ExecutorShards> = shard_state.executor_shard_sets();
        let initial_target_executors: Vec<usize> = executors
            .iter()
            .enumerate()
            .filter(|&(_idx, entry)| entry.shard_ids.is_empty())
            .map(|(idx, _entry)| idx)
            .collect();
        let optimal_count = shard_state.number_of_shards / executor_count;
        let upper_threshold = (optimal_count as f64 * (1.0 + threshold)).ceil() as usize;
        let lower_threshold = (optimal_count as f64 * (1.0 - threshold)).floor() as usize;

        // Distributing unassigned shards evenly
        let unassigned_shards = shard_state.get_unassigned_shards();
        let mut unassigned_shards_iter = unassigned_shards.into_iter();

        // First assign to and distribute among empty executors, until all of them reach the optimal count
        if !initial_target_executors.is_empty() {
            let executor_count = initial_target_executors.len();
            let last_executor_idx = executor_count - 1;

            let mut idx = 0;
            for shard in unassigned_shards_iter.by_ref() {
                let target_idx = initial_target_executors[idx];
                let executor = &mut executors[target_idx];

                trace!(
                    "Assigning shard to originally empty executor: {} to {}",
                    shard, target_idx
                );
                assignments.assign(executor.executor_id, shard);
                executor.shard_ids.insert(shard);

                // If the last executor is at optimal count, then all executors are at optimal count
                if idx == last_executor_idx && executor.shard_ids.len() == optimal_count {
                    break;
                }

                idx = (idx + 1) % executor_count;
            }
        }

        // Now assign to and distribute among all executors
        {
            let mut idx = 0;
            for shard in unassigned_shards_iter {
                trace!("Assigning shard: {} to {}", shard, idx);
                let executor = &mut executors[idx];
                assignments.assign(executor.executor_id, shard);
                executor.shard_ids.insert(shard);
                idx = (idx + 1) % executor_count;
            }
        }

        if executor_count == 1 {
            return Rebalance {
                assignments,
                unassignments,
            };
        };

        // We redistribute shards from each entry having more than the optimal count
        // to the last one until it becomes balanced, and repeat if we have more than one unbalanced entry.
        // We also apply a threshold to the optimal count, to reduce the number of shard reassignments.
        for target_idx in 0..executors.len() {
            for (idx, entry) in executors.iter().enumerate() {
                trace!(
                    executor = idx,
                    shard_count = entry.shard_ids.len(),
                    shards = ?entry.shard_ids,
                    "Executor shard count before rebalancing step"
                );
            }

            if executors[target_idx].shard_ids.len() < lower_threshold {
                trace!("Found an executor with too few shards: {}", target_idx);

                loop {
                    trace!("Target count: {}..{}", lower_threshold, upper_threshold);
                    let current_target_len = executors[target_idx].shard_ids.len();
                    if current_target_len < lower_threshold {
                        // Finding a source executor which has more than enough shards
                        if let Some((source_idx, _)) = executors
                            .iter()
                            .enumerate()
                            .filter(|(idx, entry)| {
                                *idx != target_idx && // we need a different source
                                    entry.shard_ids.len() > lower_threshold
                            })
                            .max_by(|(_, a), (_, b)| a.shard_ids.len().cmp(&b.shard_ids.len()))
                        {
                            let shard_id = *executors[source_idx].shard_ids.iter().next().unwrap();
                            // this is guaranteed by check (**)
                            trace!(
                                "Moving first shard from {} to {}: {}",
                                source_idx, target_idx, shard_id
                            );
                            executors[source_idx].shard_ids.remove(&shard_id);

                            executors[target_idx].shard_ids.insert(shard_id);
                            assignments.assign(executors[target_idx].executor_id, shard_id);
                            unassignments.unassign(executors[source_idx].executor_id, shard_id);
                            assignments.unassign(executors[source_idx].executor_id, shard_id);
                        } else {
                            trace!("Target reached a balanced state");
                            // target reached a balanced state
                            break;
                        }
                    } else {
                        trace!("No more possible rebalance steps");
                        break;
                    }
                }
            }
        }

        Rebalance {
            assignments,
            unassignments,
        }
    }

    pub fn get_assignments(&self) -> &Assignments {
        &self.assignments
    }

    pub fn get_unassignments(&self) -> &Unassignments {
        &self.unassignments
    }

    pub fn is_empty(&self) -> bool {
        self.assignments.assignments.is_empty() && self.unassignments.unassignments.is_empty()
    }

    pub fn remove_shards(&mut self, shard_ids: &HashSet<ShardId>) {
        for assigned_shard_ids in self.assignments.assignments.values_mut() {
            assigned_shard_ids.retain(|shard_id| !shard_ids.contains(shard_id));
        }
        self.assignments
            .assignments
            .retain(|_, shards| !shards.is_empty());
        for unassigned_shard_ids in self.unassignments.unassignments.values_mut() {
            unassigned_shard_ids.retain(|shard_id| !shard_ids.contains(shard_id));
        }
        self.unassignments
            .unassignments
            .retain(|_, shards| !shards.is_empty());
    }

    pub fn remove_assignment_shards(&mut self, shard_ids: &HashSet<ShardId>) {
        for assigned_shard_ids in self.assignments.assignments.values_mut() {
            assigned_shard_ids.retain(|shard_id| !shard_ids.contains(shard_id));
        }
        self.assignments
            .assignments
            .retain(|_, shards| !shards.is_empty());
    }
}

impl Display for Rebalance {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{{ assignments: {}, unassignments: {} }}",
            self.assignments, self.unassignments
        )
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use tracing_test::traced_test;

    use golem_common::model::ShardId;

    use super::Rebalance;
    use crate::sharding::model::{ExecutorAddr, ExecutorId, ShardLeaseState};
    use chrono::{DateTime, Utc};
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;
    use uuid::Uuid;

    struct TestConfig {
        number_of_shards: usize,
        number_of_executors: usize,
        initial_assignments: Vec<(usize, Vec<i64>)>,
    }

    /// Executor ids are derived from the index so that `BTreeMap<ExecutorId, _>` iteration
    /// order matches the index order (`Uuid::from_u128` is big-endian) - the rebalancing
    /// algorithm is order sensitive and these expectations were written for that order.
    fn executor(idx: usize) -> ExecutorId {
        ExecutorId(Uuid::from_u128(idx as u128))
    }

    fn addr(idx: usize) -> ExecutorAddr {
        ExecutorAddr {
            ip: IpAddr::V4(Ipv4Addr::new(192, 168, 0, idx as u8)),
            port: (9000 + idx) as u16,
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn shard_ids(ids: Vec<i64>) -> Vec<ShardId> {
        ids.into_iter().map(ShardId::new).collect()
    }

    fn new_shard_state(config: TestConfig) -> ShardLeaseState {
        let mut shard_state = ShardLeaseState::new(config.number_of_shards);
        for i in 0..config.number_of_executors {
            shard_state.add_executor(executor(i), addr(i), None, now(), Duration::from_secs(60));
        }
        for (executor_idx, shards) in config.initial_assignments {
            assign_shards(&mut shard_state, executor(executor_idx), shards);
        }
        shard_state
    }

    fn assert_assignments_for_executor(
        rebalance: &Rebalance,
        executor: ExecutorId,
        shards: Vec<i64>,
    ) {
        assert_eq!(
            get_assigned_ids(rebalance, executor),
            shard_ids(shards),
            "assert_assignments_for_executor: {executor}\n{rebalance:#?}\n",
        );
    }

    fn assert_assignments(rebalance: &Rebalance, assignments: Vec<(usize, Vec<i64>)>) {
        for (executor_idx, shards) in assignments {
            assert_assignments_for_executor(rebalance, executor(executor_idx), shards)
        }
    }

    fn assert_unassignments_for_executor(
        rebalance: &Rebalance,
        executor: ExecutorId,
        shards: Vec<i64>,
    ) {
        assert_eq!(
            get_unassigned_ids(rebalance, executor),
            shard_ids(shards),
            "assert_unassignments_for_executor: {executor}\n{rebalance:#?}\n",
        );
    }

    fn assert_unassignments(rebalance: &Rebalance, unassignments: Vec<(usize, Vec<i64>)>) {
        for (executor_idx, shards) in unassignments {
            assert_unassignments_for_executor(rebalance, executor(executor_idx), shards)
        }
    }

    fn assign_shard(shard_state: &mut ShardLeaseState, executor: ExecutorId, shard_id: i64) {
        shard_state.assign_shard(executor, ShardId::new(shard_id));
    }

    fn assign_shards(shard_state: &mut ShardLeaseState, executor: ExecutorId, shard_ids: Vec<i64>) {
        for shar_id in shard_ids {
            assign_shard(shard_state, executor, shar_id)
        }
    }

    fn get_assigned_ids(rebalance: &Rebalance, executor: ExecutorId) -> Vec<ShardId> {
        let mut assigned_ids = rebalance
            .get_assignments()
            .assignments
            .get(&executor)
            .cloned()
            .unwrap_or_default()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        assigned_ids.sort();
        assigned_ids
    }

    fn get_unassigned_ids(rebalance: &Rebalance, executor: ExecutorId) -> Vec<ShardId> {
        let mut assigned_ids = rebalance
            .get_unassignments()
            .unassignments
            .get(&executor)
            .cloned()
            .unwrap_or_default()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        assigned_ids.sort();
        assigned_ids
    }

    #[test]
    #[traced_test]
    fn rebalance_empty_table() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 1000,
            number_of_executors: 0,
            initial_assignments: vec![],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_single_pod_no_unassigned() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 4,
            number_of_executors: 1,
            initial_assignments: vec![(0, vec![0, 1, 2, 3])],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_single_pod_unassigned() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 6,
            number_of_executors: 1,
            initial_assignments: vec![(0, vec![0, 3])],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert!(rebalance.get_unassignments().is_empty());
        assert_assignments(&rebalance, vec![(0, vec![1, 2, 4, 5])]);
    }

    #[test]
    #[traced_test]
    fn rebalance_three_balanced_pods_no_unassigned() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 9,
            number_of_executors: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2]),
                (1, vec![3, 4, 5]),
                (2, vec![6, 7, 8]),
            ],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_three_balanced_pods_unassigned() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 9,
            number_of_executors: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1]),
                (1, vec![4, 5]),
                (2, vec![6, 7]),
            ],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);
        assert!(rebalance.get_unassignments().is_empty());

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![2]),
                (1, vec![3]),
                (2, vec![8]),
            ],
        );
    }

    #[test]
    #[traced_test]
    fn rebalance_one_new_pod() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 9,
            number_of_executors: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2, 3]),
                (1, vec![4, 5, 6, 7, 8]),
            ],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![]),
                (1, vec![]),
                (2, vec![0, 4, 5]),
            ],
        );

        assert_unassignments(
            &rebalance,
            vec![
                //
                (0, vec![0]),
                (1, vec![4, 5]),
                (2, vec![]),
            ],
        );
    }

    #[test]
    #[traced_test]
    fn rebalance_one_new_pod_with_threshold() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 9,
            number_of_executors: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2, 3]),
                (1, vec![4, 5, 6, 7, 8]),
            ],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.33);

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![]),
                (1, vec![]),
                (2, vec![4, 5]),
            ],
        );

        assert_unassignments(
            &rebalance,
            vec![
                //
                (0, vec![]),
                (1, vec![4, 5]),
                (2, vec![]),
            ],
        );
    }

    #[test]
    #[traced_test]
    fn rebalance_one_new_pod_after_removing_two() {
        // 3,4,5 and 9,10,11 are unassigned
        // pod3 is empty
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 12,
            number_of_executors: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2]),
                (1, vec![6, 7, 8]),
            ],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![10]),
                (1, vec![11]),
                (2, vec![3, 4, 5, 9]),
            ],
        );

        assert_unassignments(
            &rebalance,
            vec![
                //
                (0, vec![]),
                (1, vec![]),
                (2, vec![]),
            ],
        );
    }

    #[test]
    #[traced_test]
    fn rebalance_two_new_pods() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 9,
            number_of_executors: 3,
            initial_assignments: vec![(0, vec![0, 1, 2, 3, 4, 5, 6, 7, 8])],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![]),
                (1, vec![0, 1, 2]),
                (2, vec![3, 4, 5]),
            ],
        );

        assert_unassignments(
            &rebalance,
            vec![
                //
                (0, vec![0, 1, 2, 3, 4, 5]),
                (1, vec![]),
                (2, vec![]),
            ],
        );
    }

    #[test]
    #[traced_test]
    fn rebalance_two_new_pods_after_removing_one() {
        // pod1 and pod2 has 4-4 shards because previously we had 3 pods for 12 shards
        // 4,5,6,11 are unassigned
        // pod3 and pod4 are empty
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 12,
            number_of_executors: 4,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2, 3]),
                (1, vec![7, 8, 9, 10]),
            ],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![]),
                (1, vec![]),
                (2, vec![4, 6, 7]),
                (3, vec![0, 5, 11]),
            ],
        );

        assert_unassignments(
            &rebalance,
            vec![
                //
                (0, vec![0]),
                (1, vec![7]),
                (2, vec![]),
                (3, vec![]),
            ],
        );
    }

    #[test]
    #[traced_test]
    fn two_empty_pods_one_filled() {
        // pod2 is empty
        // pod3 is new and empty
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 9,
            number_of_executors: 3,
            initial_assignments: vec![(0, vec![3, 4, 5])],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![]),
                (1, vec![0, 2, 7]),
                (2, vec![1, 6, 8]),
            ],
        );
    }

    #[test]
    #[traced_test]
    fn initial_assign_is_ordered_and_no_rebalance_needed() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 8,
            number_of_executors: 4,
            initial_assignments: vec![],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![0, 4]),
                (1, vec![1, 5]),
                (2, vec![2, 6]),
                (3, vec![3, 7]),
            ],
        );

        assert_eq!(rebalance.unassignments.unassignments.len(), 0);
    }

    #[test]
    #[traced_test]
    fn initial_assign_is_ordered_and_no_rebalance_needed_with_less_then_opt() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 14,
            number_of_executors: 4,
            initial_assignments: vec![],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert_assignments(
            &rebalance,
            vec![
                //
                (0, vec![0, 4, 8, 12]),
                (1, vec![1, 5, 9, 13]),
                (2, vec![2, 6, 10]),
                (3, vec![3, 7, 11]),
            ],
        );

        assert_eq!(rebalance.unassignments.unassignments.len(), 0);
    }

    #[test]
    #[traced_test]
    fn initial_assign_is_ordered_and_no_rebalance_with_some_saturated_pod() {
        let shard_state = new_shard_state(TestConfig {
            number_of_shards: 8,
            number_of_executors: 4,
            initial_assignments: vec![(0, vec![0, 1])],
        });

        let rebalance = Rebalance::from_shard_state(&shard_state, 0.0);

        assert_assignments(
            &rebalance,
            vec![
                //
                (1, vec![2, 5]),
                (2, vec![3, 6]),
                (3, vec![4, 7]),
            ],
        );

        assert_eq!(rebalance.unassignments.unassignments.len(), 0);
    }
}
