use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use tracing::trace;

use golem_common::model::ShardId;

use crate::model::{Assignments, Pod, RoutingTable, Unassignments};

#[derive(Clone, Debug, Deserialize, Serialize)]
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

    /// Constructs a rebalance plan from the current state of the routing table.
    ///
    /// The `threshold` parameter is used to reduce the number of shard reassignments by
    /// allowing a given number of shards to be over or under the optimal count per pod.
    ///
    /// The optimal count (balanced state) is number_of_shards/pod_count.
    /// Threshold is a percentage of the optimal count, so for 10 pods with 1000 shards,
    /// and a threshold of 10%, pods with shard count between 90 and 110 will be considered
    /// balanced.
    pub fn from_routing_table(routing_table: &RoutingTable, threshold: f64) -> Self {
        let mut assignments = Assignments::new();
        let mut unassignments = Unassignments::new();
        let pod_count = routing_table.get_pod_count();
        if pod_count == 0 {
            return Rebalance {
                assignments,
                unassignments,
            };
        }

        let mut routing_table_entries = routing_table.get_entries_vec();
        let initial_target_pods: Vec<usize> = routing_table_entries
            .iter()
            .enumerate()
            .filter(|&(_idx, entry)| entry.shard_ids.is_empty())
            .map(|(idx, _entry)| idx)
            .collect();
        let optimal_count = routing_table.number_of_shards / pod_count;
        let upper_threshold = (optimal_count as f64 * (1.0 + threshold)).ceil() as usize;
        let lower_threshold = (optimal_count as f64 * (1.0 - threshold)).floor() as usize;

        // Distributing unassigned shards evenly
        let unassigned_shards = routing_table.get_unassigned_shards();
        let mut unassigned_shards_iter = unassigned_shards.into_iter();

        // First assign to and distribute among empty pods, until all of them reach the optimal count
        if !initial_target_pods.is_empty() {
            let pod_count = initial_target_pods.len();
            let last_pod_idx = pod_count - 1;

            let mut idx = 0;
            for shard in unassigned_shards_iter.by_ref() {
                let target_idx = initial_target_pods[idx];
                let routing_table_entry = &mut routing_table_entries[target_idx];

                trace!(
                    "Assigning shard to originally empty pod: {} to {}",
                    shard,
                    target_idx
                );
                assignments.assign(routing_table_entry.pod.clone(), shard);
                routing_table_entry.shard_ids.insert(shard);

                // If the last pod is at optimal count, then all pods are at optimal count
                if idx == last_pod_idx && routing_table_entry.shard_ids.len() == optimal_count {
                    break;
                }

                idx = (idx + 1) % pod_count;
            }
        }

        // Now assign to and distribute among all pods
        {
            let mut idx = 0;
            for shard in unassigned_shards_iter {
                trace!("Assigning shard: {} to {}", shard, idx);
                let routing_table_entry = &mut routing_table_entries[idx];
                assignments.assign(routing_table_entry.pod.clone(), shard);
                routing_table_entry.shard_ids.insert(shard);
                idx = (idx + 1) % pod_count;
            }
        }

        if pod_count == 1 {
            return Rebalance {
                assignments,
                unassignments,
            };
        };

        // We redistribute shards from each entry having more than the optimal count
        // to the last one until it becomes balanced, and repeat if we have more than one unbalanced entry.
        // We also apply a threshold to the optimal count, to reduce the number of shard reassignments.
        for target_idx in 0..routing_table_entries.len() {
            for (idx, entry) in routing_table_entries.iter().enumerate() {
                trace!(
                    "Pod {} has {} shards: {:?}",
                    idx,
                    entry.shard_ids.len(),
                    entry.shard_ids
                );
            }

            if routing_table_entries[target_idx].shard_ids.len() < lower_threshold {
                trace!("Found a pod with too few shards: {}", target_idx);

                loop {
                    trace!("Target count: {}..{}", lower_threshold, upper_threshold);
                    let current_target_len = routing_table_entries[target_idx].shard_ids.len();
                    if current_target_len < lower_threshold {
                        // Finding a source pod which has more than enough shards
                        if let Some((source_idx, _)) = routing_table_entries
                            .iter()
                            .enumerate()
                            .filter(|(idx, entry)| {
                                *idx != target_idx && // we need a different source
                                    entry.shard_ids.len() > lower_threshold
                            })
                            .max_by(|(_, a), (_, b)| a.shard_ids.len().cmp(&b.shard_ids.len()))
                        {
                            let shard_id = *routing_table_entries[source_idx]
                                .shard_ids
                                .iter()
                                .next()
                                .unwrap();
                            // this is guaranteed by check (**)
                            trace!(
                                "Moving first shard from {} to {}: {}",
                                source_idx,
                                target_idx,
                                shard_id
                            );
                            routing_table_entries[source_idx]
                                .shard_ids
                                .remove(&shard_id);

                            routing_table_entries[target_idx].shard_ids.insert(shard_id);
                            assignments
                                .assign(routing_table_entries[target_idx].pod.clone(), shard_id);
                            unassignments
                                .unassign(routing_table_entries[source_idx].pod.clone(), shard_id);
                            assignments
                                .unassign(routing_table_entries[source_idx].pod.clone(), shard_id);
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

    pub fn get_pods(&self) -> HashSet<Pod> {
        let mut pods = HashSet::new();
        for pod in self.assignments.assignments.keys() {
            pods.insert(pod.clone());
        }
        for pod in self.unassignments.unassignments.keys() {
            pods.insert(pod.clone());
        }
        pods
    }

    pub fn is_empty(&self) -> bool {
        self.assignments.assignments.is_empty() && self.unassignments.unassignments.is_empty()
    }

    pub fn empty() -> Self {
        Rebalance {
            assignments: Assignments::new(),
            unassignments: Unassignments::new(),
        }
    }

    pub fn remove_pods(&mut self, pods: &HashSet<Pod>) {
        self.assignments
            .assignments
            .retain(|pod, _| !pods.contains(pod));
        self.unassignments
            .unassignments
            .retain(|pod, _| !pods.contains(pod));
    }

    pub fn remove_shards(&mut self, shard_ids: &HashSet<ShardId>) {
        for assigned_shard_ids in &mut self.assignments.assignments.values_mut() {
            assigned_shard_ids.retain(|shard_id| !shard_ids.contains(shard_id));
        }
        for unassigned_shard_ids in &mut self.unassignments.unassignments.values_mut() {
            unassigned_shard_ids.retain(|shard_id| !shard_ids.contains(shard_id));
        }
    }

    pub fn add_assignments(&mut self, pod: &Pod, mut shard_ids: BTreeSet<ShardId>) {
        let empty = BTreeSet::new();
        let unassignments = self.unassignments.unassignments.get(pod).unwrap_or(&empty);
        shard_ids.retain(|shard_id| !unassignments.contains(shard_id));
        self.assignments
            .assignments
            .entry(pod.clone())
            .or_default()
            .append(&mut shard_ids);
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
    use tracing_test::traced_test;

    use golem_common::model::ShardId;

    use crate::model::{Pod, RoutingTable};
    use crate::rebalancing::Rebalance;

    struct TestConfig {
        number_of_shards: usize,
        number_of_pods: usize,
        initial_assignments: Vec<(usize, Vec<i64>)>,
    }

    fn pod(idx: usize) -> Pod {
        Pod::new(format!("pod{}", idx), (9000 + idx) as u16)
    }

    fn shard_ids(ids: Vec<i64>) -> Vec<ShardId> {
        ids.into_iter().map(ShardId::new).collect()
    }

    fn new_routing_table(config: TestConfig) -> RoutingTable {
        let mut routing_table = RoutingTable::new(config.number_of_shards);
        for i in 0..config.number_of_pods {
            routing_table.add_pod(&pod(i));
        }
        for (pod_idx, shards) in config.initial_assignments {
            assign_shards(&mut routing_table, &pod(pod_idx), shards);
        }
        routing_table
    }

    fn assert_assignments_for_pod(rebalance: &Rebalance, pod: &Pod, shards: Vec<i64>) {
        assert_eq!(
            get_assigned_ids(rebalance, pod),
            shard_ids(shards),
            "assert_assignments_for_pod: {}\n{:#?}\n",
            pod,
            rebalance,
        );
    }

    fn assert_assignments(rebalance: &Rebalance, assignments: Vec<(usize, Vec<i64>)>) {
        for (pod_idx, shards) in assignments {
            assert_assignments_for_pod(rebalance, &pod(pod_idx), shards)
        }
    }

    fn assert_unassignments_for_pod(rebalance: &Rebalance, pod: &Pod, shards: Vec<i64>) {
        assert_eq!(
            get_unassigned_ids(rebalance, pod),
            shard_ids(shards),
            "assert_unassignments_for_pod: {}\n{:#?}\n",
            pod,
            rebalance,
        );
    }

    fn assert_unassignments(rebalance: &Rebalance, unassignments: Vec<(usize, Vec<i64>)>) {
        for (pod_idx, shards) in unassignments {
            assert_unassignments_for_pod(rebalance, &pod(pod_idx), shards)
        }
    }

    fn assign_shard(routing_table: &mut RoutingTable, pod: &Pod, shard_id: i64) {
        routing_table
            .shard_assignments
            .entry(pod.clone())
            .or_default()
            .insert(ShardId::new(shard_id));
    }

    fn assign_shards(routing_table: &mut RoutingTable, pod: &Pod, shard_ids: Vec<i64>) {
        for shar_id in shard_ids {
            assign_shard(routing_table, pod, shar_id)
        }
    }

    fn get_assigned_ids(rebalance: &Rebalance, pod: &Pod) -> Vec<ShardId> {
        let mut assigned_ids = rebalance
            .get_assignments()
            .assignments
            .get(pod)
            .cloned()
            .unwrap_or_default()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        assigned_ids.sort();
        assigned_ids
    }

    fn get_unassigned_ids(rebalance: &Rebalance, pod: &Pod) -> Vec<ShardId> {
        let mut assigned_ids = rebalance
            .get_unassignments()
            .unassignments
            .get(pod)
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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 1000,
            number_of_pods: 0,
            initial_assignments: vec![],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_single_pod_no_unassigned() {
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 4,
            number_of_pods: 1,
            initial_assignments: vec![(0, vec![0, 1, 2, 3])],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_single_pod_unassigned() {
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 6,
            number_of_pods: 1,
            initial_assignments: vec![(0, vec![0, 3])],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

        assert!(rebalance.get_unassignments().is_empty());
        assert_assignments(&rebalance, vec![(0, vec![1, 2, 4, 5])]);
    }

    #[test]
    #[traced_test]
    fn rebalance_three_balanced_pods_no_unassigned() {
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 9,
            number_of_pods: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2]),
                (1, vec![3, 4, 5]),
                (2, vec![6, 7, 8]),
            ],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_three_balanced_pods_unassigned() {
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 9,
            number_of_pods: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1]),
                (1, vec![4, 5]),
                (2, vec![6, 7]),
            ],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 9,
            number_of_pods: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2, 3]),
                (1, vec![4, 5, 6, 7, 8]),
            ],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 9,
            number_of_pods: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2, 3]),
                (1, vec![4, 5, 6, 7, 8]),
            ],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.33);

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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 12,
            number_of_pods: 3,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2]),
                (1, vec![6, 7, 8]),
            ],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 9,
            number_of_pods: 3,
            initial_assignments: vec![(0, vec![0, 1, 2, 3, 4, 5, 6, 7, 8])],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 12,
            number_of_pods: 4,
            initial_assignments: vec![
                //
                (0, vec![0, 1, 2, 3]),
                (1, vec![7, 8, 9, 10]),
            ],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 9,
            number_of_pods: 3,
            initial_assignments: vec![(0, vec![3, 4, 5])],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 8,
            number_of_pods: 4,
            initial_assignments: vec![],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 14,
            number_of_pods: 4,
            initial_assignments: vec![],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

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
        let routing_table = new_routing_table(TestConfig {
            number_of_shards: 8,
            number_of_pods: 4,
            initial_assignments: vec![(0, vec![0, 1])],
        });

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

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
