// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;
use std::net::ToSocketAddrs;

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use tonic::transport::Endpoint;

use golem_api_grpc::proto::golem;
use golem_common::model::ShardId;

use crate::error::ShardManagerError;
use crate::rebalancing::Rebalance;

#[derive(
    Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
)]
pub struct Pod {
    host: String,
    port: u16,
    pub pod_name: Option<String>,
}

impl Pod {
    #[cfg(test)]
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            pod_name: None,
        }
    }

    pub fn address(&self) -> Result<Endpoint, tonic::transport::Error> {
        Endpoint::try_from(format!("{}:{}", self.host, self.port))
    }

    pub fn from_register_request(
        request: tonic::Request<golem::shardmanager::RegisterRequest>,
    ) -> Result<Self, ShardManagerError> {
        let registration_addr = request
            .remote_addr()
            .ok_or(ShardManagerError::invalid_request("missing host"))?;
        let request = request.into_inner();
        let pod = Pod {
            host: request.host,
            port: request.port as u16,
            pod_name: request.pod_name,
        };

        let endpoint: Endpoint = pod.address()?;
        let resolved = endpoint
            .uri()
            .to_string()
            .to_socket_addrs()?
            .collect::<Vec<_>>();
        if !resolved.contains(&registration_addr) {
            Err(ShardManagerError::invalid_request(
                "Host mismatch between registration message and resolved message source",
            ))
        } else {
            Ok(pod)
        }
    }
}

impl From<Pod> for golem::shardmanager::Pod {
    fn from(value: Pod) -> golem::shardmanager::Pod {
        golem::shardmanager::Pod {
            host: value.host,
            port: value.port as u32,
            pod_name: value.pod_name,
        }
    }
}

impl From<golem::shardmanager::Pod> for Pod {
    fn from(proto: golem::shardmanager::Pod) -> Self {
        Self {
            host: proto.host,
            port: proto.port as u16,
            pod_name: proto.pod_name,
        }
    }
}

impl Display for Pod {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match &self.pod_name {
            Some(name) => write!(f, "{}", name),
            None => write!(f, "{}:{}", self.host, self.port),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RoutingTable {
    pub number_of_shards: usize,
    pub shard_assignments: BTreeMap<Pod, BTreeSet<ShardId>>,
}

impl RoutingTable {
    pub fn new(number_of_shards: usize) -> Self {
        Self {
            number_of_shards,
            shard_assignments: BTreeMap::new(),
        }
    }

    pub fn get_entries(&self) -> BTreeSet<RoutingTableEntry> {
        self.shard_assignments
            .clone()
            .into_iter()
            .map(|(pod, shard_ids)| RoutingTableEntry::new(pod, shard_ids))
            .collect()
    }

    pub fn get_entries_vec(&self) -> Vec<RoutingTableEntry> {
        self.shard_assignments
            .clone()
            .into_iter()
            .map(|(pod, shard_ids)| RoutingTableEntry::new(pod, shard_ids))
            .collect()
    }

    pub fn get_pods(&self) -> HashSet<Pod> {
        self.shard_assignments.clone().into_keys().collect()
    }

    pub fn rebalance(&mut self, rebalance: Rebalance) {
        for (pod, shard_ids) in &rebalance.get_assignments().assignments {
            self.shard_assignments
                .entry(pod.clone())
                .or_default()
                .extend(shard_ids);
        }
        for (pod, shard_ids) in &rebalance.get_unassignments().unassignments {
            self.shard_assignments
                .entry(pod.clone())
                .or_default()
                .retain(|shard_id| !shard_ids.contains(shard_id));
        }
    }

    pub fn get_unassigned_shards(&self) -> BTreeSet<ShardId> {
        let mut unassigned_shards: BTreeSet<ShardId> = (0..self.number_of_shards)
            .map(|shard_id| ShardId::new(shard_id as i64))
            .collect();
        for assigned_shards in self.shard_assignments.values() {
            unassigned_shards.retain(|shard_id| !assigned_shards.contains(shard_id));
        }
        unassigned_shards
    }

    pub fn get_shards(&self, pod: &Pod) -> Option<BTreeSet<ShardId>> {
        self.shard_assignments.get(pod).cloned()
    }

    pub fn get_pod_count(&self) -> usize {
        self.shard_assignments.len()
    }

    pub fn add_pod(&mut self, pod: &Pod) {
        self.shard_assignments.insert(pod.clone(), BTreeSet::new());
    }

    pub fn remove_pod(&mut self, pod: &Pod) {
        self.shard_assignments.remove(pod);
    }

    pub fn has_pod(&self, pod: &Pod) -> bool {
        self.shard_assignments.contains_key(pod)
    }
}

impl From<RoutingTable> for golem::shardmanager::RoutingTable {
    fn from(routing_table: RoutingTable) -> golem::shardmanager::RoutingTable {
        golem::shardmanager::RoutingTable {
            number_of_shards: routing_table.number_of_shards as u32,
            shard_assignments: routing_table
                .shard_assignments
                .into_iter()
                .flat_map(|(pod, shard_ids)| {
                    shard_ids
                        .into_iter()
                        .map(move |shard_id| (pod.clone(), shard_id))
                })
                .map(|(pod, shard_id)| golem::shardmanager::RoutingTableEntry {
                    pod: Some(pod.into()),
                    shard_id: Some(shard_id.into()),
                })
                .collect(),
        }
    }
}

impl From<golem::shardmanager::RoutingTable> for RoutingTable {
    fn from(routing_table: golem::shardmanager::RoutingTable) -> Self {
        let mut shard_assignments: BTreeMap<Pod, BTreeSet<ShardId>> = BTreeMap::new();
        for entry in routing_table.shard_assignments {
            let pod = entry.pod.unwrap().into();
            let shard_id = entry.shard_id.unwrap().into();
            shard_assignments.entry(pod).or_default().insert(shard_id);
        }
        Self {
            number_of_shards: routing_table.number_of_shards as usize,
            shard_assignments,
        }
    }
}

impl Display for RoutingTable {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{{ number_of_shards: {}, shard_assignments: [{}] }}",
            self.number_of_shards,
            shard_assignments_map_to_string(&self.shard_assignments)
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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
        write!(
            f,
            "[{}]",
            shard_assignments_map_to_string(&self.assignments)
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
        write!(
            f,
            "[{}]",
            shard_assignments_map_to_string(&self.unassignments)
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Encode, Decode)]
pub struct ShardManagerState {
    pub number_of_shards: usize,
    pub shard_assignments: Vec<(Pod, Vec<ShardId>)>,
    pub assignments: Vec<(Pod, Vec<ShardId>)>,
    pub unassignments: Vec<(Pod, Vec<ShardId>)>,
}

impl ShardManagerState {
    pub fn new(routing_table: &RoutingTable, rebalance: &Rebalance) -> Self {
        let mut shard_assignments: Vec<(Pod, Vec<ShardId>)> = Vec::new();
        for routing_table_entry in routing_table.get_entries() {
            shard_assignments.push((
                routing_table_entry.pod.clone(),
                routing_table_entry.shard_ids.iter().cloned().collect(),
            ));
        }
        let mut assignments: Vec<(Pod, Vec<ShardId>)> = Vec::new();
        for (pod, shard_ids) in &rebalance.get_assignments().assignments {
            assignments.push((pod.clone(), shard_ids.iter().cloned().collect()));
        }
        let mut unassignments: Vec<(Pod, Vec<ShardId>)> = Vec::new();
        for (pod, shard_ids) in &rebalance.get_unassignments().unassignments {
            unassignments.push((pod.clone(), shard_ids.iter().cloned().collect()));
        }
        ShardManagerState {
            number_of_shards: routing_table.number_of_shards,
            shard_assignments,
            assignments,
            unassignments,
        }
    }

    pub fn get_routing_table(&self) -> RoutingTable {
        let mut shard_assignments: BTreeMap<Pod, BTreeSet<ShardId>> = BTreeMap::new();
        for (pod, shard_ids) in &self.shard_assignments {
            shard_assignments.insert(pod.clone(), shard_ids.iter().cloned().collect());
        }
        RoutingTable {
            number_of_shards: self.number_of_shards,
            shard_assignments,
        }
    }

    pub fn get_rebalance(&self) -> Rebalance {
        let mut assignments = Assignments::new();
        for (pod, shard_ids) in &self.assignments {
            for shard_id in shard_ids {
                assignments.assign(pod.clone(), *shard_id);
            }
        }
        let mut unassignments = Unassignments::new();
        for (pod, shard_ids) in &self.unassignments {
            for shard_id in shard_ids {
                unassignments.unassign(pod.clone(), *shard_id);
            }
        }
        Rebalance::new(assignments, unassignments)
    }
}

impl Display for ShardManagerState {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{{ number_of_shards: {}, shard_assignments: [{}], assignments: [{}], unassignments: [{}] }}",
            self.number_of_shards,
            shard_assignments_to_string(&self.shard_assignments),
            shard_assignments_to_string(&self.assignments),
            shard_assignments_to_string(&self.unassignments)
        )
    }
}

fn shard_assignments_to_string(shard_assignments: &[(Pod, Vec<ShardId>)]) -> String {
    let elements: Vec<String> = shard_assignments
        .iter()
        .map(|(pod, shard_ids)| {
            let ranges: Vec<ShardIdRange> = shard_ids_to_ranges(shard_ids.iter());
            let strings: Vec<String> = ranges
                .iter()
                .map(|rng| format!("{rng}").to_string())
                .collect();
            format!("{}: [{}]", pod, strings.join(", "))
        })
        .collect();
    elements.join(", ")
}

fn shard_assignments_map_to_string(shard_assignments: &BTreeMap<Pod, BTreeSet<ShardId>>) -> String {
    let elements: Vec<String> = shard_assignments
        .iter()
        .map(|(pod, shard_ids)| {
            let ranges: Vec<ShardIdRange> = shard_ids_to_ranges(shard_ids.iter());
            let strings: Vec<String> = ranges
                .iter()
                .map(|rng| format!("{rng}").to_string())
                .collect();
            format!("{}: [{}]", pod, strings.join(", "))
        })
        .collect();
    elements.join(", ")
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
