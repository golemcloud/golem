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

use super::rebalancing::Rebalance;
use core::cmp::Ordering;
use desert_rust::BinaryCodec;
use golem_api_grpc::proto::golem;
use golem_common::model::{Pod, ShardId};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fmt::{Debug, Display, Formatter};

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct PodState {
    pub pod_name: Option<String>,
    pub assigned_shards: BTreeSet<ShardId>,
}

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct RoutingTable {
    pub number_of_shards: usize,
    pub pod_states: BTreeMap<Pod, PodState>,
}

impl RoutingTable {
    pub fn new(number_of_shards: usize) -> Self {
        Self {
            number_of_shards,
            pod_states: BTreeMap::new(),
        }
    }

    pub fn get_entries(&self) -> BTreeSet<RoutingTableEntry> {
        self.pod_states
            .clone()
            .into_iter()
            .map(|(pod, state)| RoutingTableEntry::new(pod, state.assigned_shards))
            .collect()
    }

    pub fn get_entries_vec(&self) -> Vec<RoutingTableEntry> {
        self.pod_states
            .clone()
            .into_iter()
            .map(|(pod, state)| RoutingTableEntry::new(pod, state.assigned_shards))
            .collect()
    }

    pub fn get_pods(&self) -> HashSet<Pod> {
        self.pod_states.clone().into_keys().collect()
    }

    pub fn get_pods_with_names(&self) -> Vec<(Pod, Option<String>)> {
        self.pod_states
            .iter()
            .map(|(pod, pod_state)| (*pod, pod_state.pod_name.clone()))
            .collect()
    }

    pub fn rebalance(&mut self, rebalance: Rebalance) {
        for (pod, shard_ids) in &rebalance.get_assignments().assignments {
            if let Some(pod_state) = self.pod_states.get_mut(pod) {
                pod_state.assigned_shards.extend(shard_ids);
            }
        }
        for (pod, shard_ids) in &rebalance.get_unassignments().unassignments {
            if let Some(pod_state) = self.pod_states.get_mut(pod) {
                pod_state
                    .assigned_shards
                    .retain(|shard_id| !shard_ids.contains(shard_id));
            }
        }
    }

    pub fn get_unassigned_shards(&self) -> BTreeSet<ShardId> {
        let mut unassigned_shards: BTreeSet<ShardId> = (0..self.number_of_shards)
            .map(|shard_id| ShardId::new(shard_id as i64))
            .collect();

        for pod_state in self.pod_states.values() {
            unassigned_shards.retain(|shard_id| !pod_state.assigned_shards.contains(shard_id));
        }

        unassigned_shards
    }

    pub fn get_shards(&self, pod: Pod) -> Option<BTreeSet<ShardId>> {
        self.pod_states
            .get(&pod)
            .map(|ps| ps.assigned_shards.clone())
    }

    pub fn get_pod_count(&self) -> usize {
        self.pod_states.len()
    }

    pub fn add_pod(&mut self, pod: Pod, pod_name: Option<String>) {
        self.pod_states.insert(
            pod,
            PodState {
                pod_name,
                assigned_shards: BTreeSet::new(),
            },
        );
    }

    pub fn remove_pod(&mut self, pod: Pod) {
        self.pod_states.remove(&pod);
    }

    pub fn has_pod(&self, pod: Pod) -> bool {
        self.pod_states.contains_key(&pod)
    }
}

impl From<RoutingTable> for golem::shardmanager::RoutingTable {
    fn from(routing_table: RoutingTable) -> golem::shardmanager::RoutingTable {
        golem::shardmanager::RoutingTable {
            number_of_shards: routing_table.number_of_shards as u32,
            shard_assignments: routing_table
                .pod_states
                .into_iter()
                .flat_map(|(pod, pod_state)| {
                    pod_state
                        .assigned_shards
                        .into_iter()
                        .map(move |shard_id| (pod, shard_id))
                })
                .map(|(pod, shard_id)| golem::shardmanager::RoutingTableEntry {
                    pod: Some(pod.into()),
                    shard_id: Some(shard_id.into()),
                })
                .collect(),
        }
    }
}

impl Display for RoutingTable {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{{ number_of_shards: {}, shard_assignments: [{}] }}",
            self.number_of_shards,
            pod_states_map_to_string(&self.pod_states)
        )
    }
}

pub struct RoutingTableEntry {
    pub pod: Pod,
    pub shard_ids: BTreeSet<ShardId>,
}

impl RoutingTableEntry {
    pub fn new(pod: Pod, shard_ids: BTreeSet<ShardId>) -> Self {
        Self { pod, shard_ids }
    }
    pub fn get_shard_count(&self) -> usize {
        self.shard_ids.len()
    }
}

impl PartialEq for RoutingTableEntry {
    fn eq(&self, other: &Self) -> bool {
        self.shard_ids.len() == other.shard_ids.len() && self.pod == other.pod
    }
}

impl Eq for RoutingTableEntry {}

impl PartialOrd for RoutingTableEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RoutingTableEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.shard_ids.len().cmp(&other.shard_ids.len()) {
            Ordering::Equal => self.pod.cmp(&other.pod),
            other => other,
        }
    }
}

impl Display for RoutingTableEntry {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let shard_ids: Vec<String> = self.shard_ids.iter().map(|elem| elem.to_string()).collect();
        write!(
            f,
            "{{ pod: {}, shard_ids: [{}] }}",
            self.pod,
            shard_ids.join(", ")
        )
    }
}

#[derive(Clone, Debug)]
pub struct Assignments {
    pub assignments: BTreeMap<Pod, BTreeSet<ShardId>>,
}

impl Assignments {
    pub fn assign(&mut self, pod: Pod, shard_id: ShardId) {
        self.assignments.entry(pod).or_default().insert(shard_id);
    }

    pub fn unassign(&mut self, pod: Pod, shard_id: ShardId) {
        self.assignments.entry(pod).or_default().remove(&shard_id);
    }

    pub fn new() -> Self {
        Self {
            assignments: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }
}

impl Default for Assignments {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for Assignments {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "[{}]", pod_shard_ids_map_to_string(&self.assignments))
    }
}

#[derive(Clone, Debug)]
pub struct Unassignments {
    pub unassignments: BTreeMap<Pod, BTreeSet<ShardId>>,
}

impl Unassignments {
    pub fn unassign(&mut self, pod: Pod, shard_id: ShardId) {
        self.unassignments.entry(pod).or_default().insert(shard_id);
    }

    pub fn new() -> Self {
        Self {
            unassignments: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.unassignments.is_empty()
    }
}

impl Default for Unassignments {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for Unassignments {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "[{}]", pod_shard_ids_map_to_string(&self.unassignments))
    }
}

fn pod_states_map_to_string(pod_states: &BTreeMap<Pod, PodState>) -> String {
    let elements: Vec<String> = pod_states
        .iter()
        .map(|(pod, pod_state)| {
            pod_shard_assignments_to_string(
                pod,
                pod_state.pod_name.clone(),
                pod_state.assigned_shards.iter(),
            )
        })
        .collect();
    elements.join(", ")
}

fn pod_shard_ids_map_to_string(pod_states: &BTreeMap<Pod, BTreeSet<ShardId>>) -> String {
    let elements: Vec<String> = pod_states
        .iter()
        .map(|(pod, shard_ids)| pod_shard_assignments_to_string(pod, None, shard_ids.iter()))
        .collect();
    elements.join(", ")
}

pub fn pod_shard_assignments_to_string<'a, T: Iterator<Item = &'a ShardId>>(
    pod: &Pod,
    pod_name: Option<String>,
    shard_ids: T,
) -> String {
    let ranges: Vec<ShardIdRange> = shard_ids_to_ranges(shard_ids);
    let strings: Vec<String> = ranges
        .iter()
        .map(|rng| format!("{rng}").to_string())
        .collect();
    format!(
        "{pod} {}: [{}]",
        pod_name.unwrap_or_default(),
        strings.join(", ")
    )
}

enum ShardIdRange {
    Range { min: ShardId, max: ShardId },
    Single(ShardId),
}

impl Display for ShardIdRange {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ShardIdRange::Range { min, max } => write!(f, "{min}..{max}"),
            ShardIdRange::Single(shard_id) => Display::fmt(shard_id, f),
        }
    }
}

fn shard_ids_to_ranges<'a, T: Iterator<Item = &'a ShardId>>(ids: T) -> Vec<ShardIdRange> {
    let mut sorted: Vec<&ShardId> = ids.collect();
    sorted.sort();

    let mut result: Vec<ShardIdRange> = vec![];
    let mut current: Option<ShardIdRange> = None;

    for shard_id in sorted {
        match current {
            Some(ShardIdRange::Single(prev)) if prev.is_left_neighbor(shard_id) => {
                current = Some(ShardIdRange::Range {
                    min: prev,
                    max: *shard_id,
                })
            }
            Some(rng @ ShardIdRange::Single(_)) => {
                result.push(rng);
                current = Some(ShardIdRange::Single(*shard_id));
            }
            Some(ShardIdRange::Range { min, max }) if max.is_left_neighbor(shard_id) => {
                current = Some(ShardIdRange::Range {
                    min,
                    max: *shard_id,
                })
            }
            Some(rng @ ShardIdRange::Range { .. }) => {
                result.push(rng);
                current = Some(ShardIdRange::Single(*shard_id));
            }
            None => {
                current = Some(ShardIdRange::Single(*shard_id));
            }
        }
    }

    if let Some(last) = current {
        result.push(last);
    }

    result
}
