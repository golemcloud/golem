use crate::model::{Assignments, Pod, RoutingTable, Unassignments};
use golem_common::model::ShardId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::fmt::{Display, Formatter};
use tracing::trace;

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

        let mut empty_pods = BTreeSet::new();
        for (idx, entry) in routing_table_entries.iter().enumerate() {
            if entry.shard_ids.is_empty() {
                empty_pods.insert(idx);
            }
        }
        let mut initial_target_pods = empty_pods.iter().copied().collect::<Vec<_>>(); // this will be updated during the unassignment step, while empty_pods is used later in the assignment step
        let optimal_count = routing_table.number_of_shards / pod_count;
        let upper_threshold = (optimal_count as f64 * (1.0 + threshold)).ceil() as usize;
        let lower_threshold = (optimal_count as f64 * (1.0 - threshold)).floor() as usize;

        // Distributing unassigned shards evenly first among the new pods, once they reached
        // the optimal per-pod shard count, distribute the rest among all pods
        let unassigned_shards = routing_table.get_unassigned_shards();
        let mut idx = 0;
        for unassigned_shard in unassigned_shards {
            if initial_target_pods.is_empty() {
                // No more initial empty pods, distribute shards among all pods
                trace!("Assigning shard: {} to {}", unassigned_shard, idx);
                let routing_table_entry = &mut routing_table_entries[idx];
                assignments.assign(routing_table_entry.pod.clone(), unassigned_shard);
                routing_table_entry.shard_ids.insert(unassigned_shard);
                idx = (idx + 1) % pod_count;
            } else {
                let target_idx = initial_target_pods[idx];
                let routing_table_entry = &mut routing_table_entries[target_idx];

                trace!(
                    "Assigning shard to originally empty pod: {} to {}",
                    unassigned_shard,
                    target_idx
                );
                assignments.assign(routing_table_entry.pod.clone(), unassigned_shard);
                routing_table_entry.shard_ids.insert(unassigned_shard);
                idx = (idx + 1) % initial_target_pods.len();

                if routing_table_entry.shard_ids.len() == optimal_count {
                    // This pod reached the optimal count, removing it from the list targets
                    // TODO: is this the right idx? we just incremented it
                    initial_target_pods.remove(idx);
                    if initial_target_pods.is_empty() {
                        idx = 0;
                    } else {
                        // TODO: is this the right idx? we increased two times now, and also removed the entry, so we might
                        //       jumped over the actual next one
                        idx = (idx + 1) % initial_target_pods.len();
                    }
                }
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
    use crate::model::{Pod, RoutingTable};
    use crate::rebalancing::Rebalance;
    use golem_common::model::ShardId;
    use tracing::trace;
    use tracing_test::traced_test;

    struct TestRoutingConf {
        number_of_shards: usize,
        number_of_pods: usize,
    }

    fn pod_id(idx: usize) -> Pod {
        Pod::new(format!("pod{}", idx), (9000 + idx) as u16)
    }

    fn shard_ids(ids: Vec<i64>) -> Vec<ShardId> {
        ids.into_iter().map(|idx| ShardId::new(idx)).collect()
    }

    fn new_test_routing_table(config: &TestRoutingConf) -> RoutingTable {
        let mut routing_table = RoutingTable::new(config.number_of_shards);
        for i in 0..config.number_of_pods {
            routing_table.add_pod(&pod_id(i));
        }
        routing_table
    }

    fn assert_rebalance_assignments_for_pod(rebalance: &Rebalance, pod: usize, shards: Vec<i64>) {
        let pod = pod_id(pod);
        assert_eq!(
            get_assigned_ids(rebalance, &pod),
            shard_ids(shards),
            "assert_rebalance_assignments_for_pod: {}",
            &pod
        );
    }

    fn assign_shard(routing_table: &mut RoutingTable, pod: &Pod, shard_id: i64) {
        routing_table
            .shard_assignments
            .entry(pod.clone())
            .or_default()
            .insert(ShardId::new(shard_id));
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
        let routing_table = RoutingTable::new(1000);
        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_single_pod_no_unassigned() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let mut routing_table = RoutingTable::new(4);
        routing_table.add_pod(&pod1);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 1);
        assign_shard(&mut routing_table, &pod1, 2);
        assign_shard(&mut routing_table, &pod1, 3);

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_single_pod_unassigned() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let mut routing_table = RoutingTable::new(6);
        routing_table.add_pod(&pod1);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 3);

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

        assert!(rebalance.get_unassignments().is_empty());

        let assigned_ids = get_assigned_ids(&rebalance, &pod1);
        assert_eq!(
            assigned_ids,
            vec![
                ShardId::new(1),
                ShardId::new(2),
                ShardId::new(4),
                ShardId::new(5),
            ]
        );
    }

    #[test]
    #[traced_test]
    fn rebalance_three_balanced_pods_no_unassigned() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let pod2 = Pod::new("pod2".to_string(), 9001);
        let pod3 = Pod::new("pod3".to_string(), 9002);

        let mut routing_table = RoutingTable::new(9);
        routing_table.add_pod(&pod1);
        routing_table.add_pod(&pod2);
        routing_table.add_pod(&pod3);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 1);
        assign_shard(&mut routing_table, &pod1, 2);
        assign_shard(&mut routing_table, &pod2, 3);
        assign_shard(&mut routing_table, &pod2, 4);
        assign_shard(&mut routing_table, &pod2, 5);
        assign_shard(&mut routing_table, &pod3, 6);
        assign_shard(&mut routing_table, &pod3, 7);
        assign_shard(&mut routing_table, &pod3, 8);

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
        assert!(rebalance.is_empty());
    }

    #[test]
    #[traced_test]
    fn rebalance_three_balanced_pods_unassigned() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let pod2 = Pod::new("pod2".to_string(), 9001);
        let pod3 = Pod::new("pod3".to_string(), 9002);

        let mut routing_table = RoutingTable::new(9);
        routing_table.add_pod(&pod1);
        routing_table.add_pod(&pod2);
        routing_table.add_pod(&pod3);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 1);
        assign_shard(&mut routing_table, &pod2, 4);
        assign_shard(&mut routing_table, &pod2, 5);
        assign_shard(&mut routing_table, &pod3, 6);
        assign_shard(&mut routing_table, &pod3, 7);

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
        assert!(rebalance.get_unassignments().is_empty());

        let assigned_ids_1 = get_assigned_ids(&rebalance, &pod1);
        let assigned_ids_2 = get_assigned_ids(&rebalance, &pod2);
        let assigned_ids_3 = get_assigned_ids(&rebalance, &pod3);
        assert_eq!(assigned_ids_1, vec![ShardId::new(2)]);
        assert_eq!(assigned_ids_2, vec![ShardId::new(3)]);
        assert_eq!(assigned_ids_3, vec![ShardId::new(8)]);
    }

    #[test]
    #[traced_test]
    fn rebalance_one_new_pod() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let pod2 = Pod::new("pod2".to_string(), 9001);
        let pod3 = Pod::new("pod3".to_string(), 9002);

        let mut routing_table = RoutingTable::new(9);
        routing_table.add_pod(&pod1);
        routing_table.add_pod(&pod2);
        routing_table.add_pod(&pod3);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 1);
        assign_shard(&mut routing_table, &pod1, 2);
        assign_shard(&mut routing_table, &pod1, 3);
        assign_shard(&mut routing_table, &pod2, 4);
        assign_shard(&mut routing_table, &pod2, 5);
        assign_shard(&mut routing_table, &pod2, 6);
        assign_shard(&mut routing_table, &pod2, 7);
        assign_shard(&mut routing_table, &pod2, 8);

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

        let assigned_ids_1 = get_assigned_ids(&rebalance, &pod1);
        let assigned_ids_2 = get_assigned_ids(&rebalance, &pod2);
        let assigned_ids_3 = get_assigned_ids(&rebalance, &pod3);

        let unassigned_ids_1 = get_unassigned_ids(&rebalance, &pod1);
        let unassigned_ids_2 = get_unassigned_ids(&rebalance, &pod2);
        let unassigned_ids_3 = get_unassigned_ids(&rebalance, &pod3);

        assert_eq!(assigned_ids_1, vec![]);
        assert_eq!(assigned_ids_2, vec![]);
        assert_eq!(
            assigned_ids_3,
            vec![ShardId::new(0), ShardId::new(4), ShardId::new(5)]
        );

        assert_eq!(unassigned_ids_1, vec![ShardId::new(0)]);
        assert_eq!(unassigned_ids_2, vec![ShardId::new(4), ShardId::new(5)]);
        assert_eq!(unassigned_ids_3, vec![]);
    }

    #[test]
    #[traced_test]
    fn rebalance_one_new_pod_with_threshold() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let pod2 = Pod::new("pod2".to_string(), 9001);
        let pod3 = Pod::new("pod3".to_string(), 9002);

        let mut routing_table = RoutingTable::new(9);
        routing_table.add_pod(&pod1);
        routing_table.add_pod(&pod2);
        routing_table.add_pod(&pod3);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 1);
        assign_shard(&mut routing_table, &pod1, 2);
        assign_shard(&mut routing_table, &pod1, 3);
        assign_shard(&mut routing_table, &pod2, 4);
        assign_shard(&mut routing_table, &pod2, 5);
        assign_shard(&mut routing_table, &pod2, 6);
        assign_shard(&mut routing_table, &pod2, 7);
        assign_shard(&mut routing_table, &pod2, 8);

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.33);

        let assigned_ids_1 = get_assigned_ids(&rebalance, &pod1);
        let assigned_ids_2 = get_assigned_ids(&rebalance, &pod2);
        let assigned_ids_3 = get_assigned_ids(&rebalance, &pod3);

        let unassigned_ids_1 = get_unassigned_ids(&rebalance, &pod1);
        let unassigned_ids_2 = get_unassigned_ids(&rebalance, &pod2);
        let unassigned_ids_3 = get_unassigned_ids(&rebalance, &pod3);

        assert_eq!(assigned_ids_1, vec![]);
        assert_eq!(assigned_ids_2, vec![]);
        assert_eq!(assigned_ids_3, vec![ShardId::new(4), ShardId::new(5)]);

        assert_eq!(unassigned_ids_1, vec![]);
        assert_eq!(unassigned_ids_2, vec![ShardId::new(4), ShardId::new(5)]);
        assert_eq!(unassigned_ids_3, vec![]);
    }

    #[test]
    #[traced_test]
    fn rebalance_one_new_pod_after_removing_two() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let pod2 = Pod::new("pod2".to_string(), 9001);
        let pod3 = Pod::new("pod3".to_string(), 9002);

        let mut routing_table = RoutingTable::new(12);
        routing_table.add_pod(&pod1);
        routing_table.add_pod(&pod2);
        routing_table.add_pod(&pod3);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 1);
        assign_shard(&mut routing_table, &pod1, 2);
        assign_shard(&mut routing_table, &pod2, 6);
        assign_shard(&mut routing_table, &pod2, 7);
        assign_shard(&mut routing_table, &pod2, 8);
        // 3,4,5 and 9,10,11 are unassigned
        // pod3 is empty

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

        let assigned_ids_1 = get_assigned_ids(&rebalance, &pod1);
        let assigned_ids_2 = get_assigned_ids(&rebalance, &pod2);
        let assigned_ids_3 = get_assigned_ids(&rebalance, &pod3);

        let unassigned_ids_1 = get_unassigned_ids(&rebalance, &pod1);
        let unassigned_ids_2 = get_unassigned_ids(&rebalance, &pod2);
        let unassigned_ids_3 = get_unassigned_ids(&rebalance, &pod3);

        assert_eq!(assigned_ids_1, vec![ShardId::new(10)]);
        assert_eq!(assigned_ids_2, vec![ShardId::new(11)]);
        assert_eq!(
            assigned_ids_3,
            vec![
                ShardId::new(3),
                ShardId::new(4),
                ShardId::new(5),
                ShardId::new(9)
            ]
        );

        assert_eq!(unassigned_ids_1, vec![]);
        assert_eq!(unassigned_ids_2, vec![]);
        assert_eq!(unassigned_ids_3, vec![]);
    }

    #[test]
    #[traced_test]
    fn rebalance_two_new_pods() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let pod2 = Pod::new("pod2".to_string(), 9001);
        let pod3 = Pod::new("pod3".to_string(), 9002);

        let mut routing_table = RoutingTable::new(9);
        routing_table.add_pod(&pod1);
        routing_table.add_pod(&pod2);
        routing_table.add_pod(&pod3);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 1);
        assign_shard(&mut routing_table, &pod1, 2);
        assign_shard(&mut routing_table, &pod1, 3);
        assign_shard(&mut routing_table, &pod1, 4);
        assign_shard(&mut routing_table, &pod1, 5);
        assign_shard(&mut routing_table, &pod1, 6);
        assign_shard(&mut routing_table, &pod1, 7);
        assign_shard(&mut routing_table, &pod1, 8);

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

        let assigned_ids_1 = get_assigned_ids(&rebalance, &pod1);
        let assigned_ids_2 = get_assigned_ids(&rebalance, &pod2);
        let assigned_ids_3 = get_assigned_ids(&rebalance, &pod3);

        let unassigned_ids_1 = get_unassigned_ids(&rebalance, &pod1);
        let unassigned_ids_2 = get_unassigned_ids(&rebalance, &pod2);
        let unassigned_ids_3 = get_unassigned_ids(&rebalance, &pod3);

        assert_eq!(assigned_ids_1, vec![]);
        assert_eq!(
            assigned_ids_2,
            vec![ShardId::new(0), ShardId::new(1), ShardId::new(2)]
        );
        assert_eq!(
            assigned_ids_3,
            vec![ShardId::new(3), ShardId::new(4), ShardId::new(5)]
        );

        assert_eq!(
            unassigned_ids_1,
            vec![
                ShardId::new(0),
                ShardId::new(1),
                ShardId::new(2),
                ShardId::new(3),
                ShardId::new(4),
                ShardId::new(5)
            ]
        );
        assert_eq!(unassigned_ids_2, vec![]);
        assert_eq!(unassigned_ids_3, vec![]);
    }

    #[test]
    #[traced_test]
    fn rebalance_two_new_pods_after_removing_one() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let pod2 = Pod::new("pod2".to_string(), 9001);
        let pod3 = Pod::new("pod3".to_string(), 9002);
        let pod4 = Pod::new("pod4".to_string(), 9003);

        let mut routing_table = RoutingTable::new(12);
        routing_table.add_pod(&pod1);
        routing_table.add_pod(&pod2);
        routing_table.add_pod(&pod3);
        routing_table.add_pod(&pod4);

        assign_shard(&mut routing_table, &pod1, 0);
        assign_shard(&mut routing_table, &pod1, 1);
        assign_shard(&mut routing_table, &pod1, 2);
        assign_shard(&mut routing_table, &pod1, 3);
        assign_shard(&mut routing_table, &pod2, 7);
        assign_shard(&mut routing_table, &pod2, 8);
        assign_shard(&mut routing_table, &pod2, 9);
        assign_shard(&mut routing_table, &pod2, 10);
        // pod1 and pod2 has 4-4 shards because previously we had 3 pods for 12 shards
        // 4,5,6,11 are unassigned
        // pod3 and pod4 are empty

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

        let assigned_ids_1 = get_assigned_ids(&rebalance, &pod1);
        let assigned_ids_2 = get_assigned_ids(&rebalance, &pod2);
        let assigned_ids_3 = get_assigned_ids(&rebalance, &pod3);
        let assigned_ids_4 = get_assigned_ids(&rebalance, &pod4);

        let unassigned_ids_1 = get_unassigned_ids(&rebalance, &pod1);
        let unassigned_ids_2 = get_unassigned_ids(&rebalance, &pod2);
        let unassigned_ids_3 = get_unassigned_ids(&rebalance, &pod3);
        let unassigned_ids_4 = get_unassigned_ids(&rebalance, &pod4);

        assert_eq!(assigned_ids_1, vec![]);
        assert_eq!(assigned_ids_2, vec![]);
        assert_eq!(
            assigned_ids_3,
            vec![ShardId::new(4), ShardId::new(6), ShardId::new(7)]
        );
        assert_eq!(
            assigned_ids_4,
            vec![ShardId::new(0), ShardId::new(5), ShardId::new(11)]
        );

        assert_eq!(unassigned_ids_1, vec![ShardId::new(0)]);
        assert_eq!(unassigned_ids_2, vec![ShardId::new(7)]);
        assert_eq!(unassigned_ids_3, vec![]);
        assert_eq!(unassigned_ids_4, vec![]);
    }

    #[test]
    #[traced_test]
    fn two_empty_pods_one_filled() {
        let pod1 = Pod::new("pod1".to_string(), 9000);
        let pod2 = Pod::new("pod2".to_string(), 9001);
        let pod3 = Pod::new("pod3".to_string(), 9002);

        let mut routing_table = RoutingTable::new(9);
        routing_table.add_pod(&pod1);
        routing_table.add_pod(&pod2);
        routing_table.add_pod(&pod3);

        assign_shard(&mut routing_table, &pod1, 3);
        assign_shard(&mut routing_table, &pod1, 4);
        assign_shard(&mut routing_table, &pod1, 5);

        // pod2 is empty
        // pod3 is new and empty

        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);

        println!("{rebalance:?}");

        let assigned_ids_1 = get_assigned_ids(&rebalance, &pod1);
        let assigned_ids_2 = get_assigned_ids(&rebalance, &pod2);
        let assigned_ids_3 = get_assigned_ids(&rebalance, &pod3);
        assert_eq!(assigned_ids_1, vec![]);
        assert_eq!(
            assigned_ids_2,
            vec![ShardId::new(2), ShardId::new(7), ShardId::new(8)]
        );
        assert_eq!(
            assigned_ids_3,
            vec![ShardId::new(0), ShardId::new(1), ShardId::new(6)]
        );
    }

    #[test]
    #[traced_test]
    fn initial_assign_is_ordered_and_no_rebalance_needed() {
        let routing_table = new_test_routing_table(&TestRoutingConf {
            number_of_shards: 8,
            number_of_pods: 4,
        });

        trace!("routing_table: {}", routing_table);
        let rebalance = Rebalance::from_routing_table(&routing_table, 0.0);
        trace!("rebalance: {}", rebalance);
        assert_eq!(rebalance.assignments.assignments.len(), 4);
        assert_rebalance_assignments_for_pod(&rebalance, 0, vec![0, 4]);
        assert_rebalance_assignments_for_pod(&rebalance, 0, vec![1, 5]);
        assert_rebalance_assignments_for_pod(&rebalance, 0, vec![2, 6]);
        assert_rebalance_assignments_for_pod(&rebalance, 0, vec![4, 7]);
        assert_eq!(rebalance.unassignments.unassignments.len(), 0);
    }
}
