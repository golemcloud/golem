use core::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;

use bincode::{Decode, Encode};
use golem_common::model::ShardId;
use golem_common::proto::golem;
use serde::{Deserialize, Serialize};

use crate::error::ShardManagerError;

#[derive(
    Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize, Encode, Decode,
)]
pub struct Pod {
    host: String,
    port: u16,
}

impl Pod {
    pub fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }

    pub fn address(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    pub fn from_register_request(
        request: tonic::Request<golem::shardmanager::RegisterRequest>,
    ) -> Result<Self, ShardManagerError> {
        let host = request
            .remote_addr()
            .ok_or(ShardManagerError::invalid_request("missing host"))?
            .ip()
            .to_string();
        let port = request.into_inner().port as u16;
        let pod = Pod::new(host, port);
        Ok(pod)
    }
}

impl From<Pod> for golem::Pod {
    fn from(value: Pod) -> golem::Pod {
        golem::Pod {
            host: value.host,
            port: value.port as u32,
        }
    }
}

impl From<golem::Pod> for Pod {
    fn from(proto: golem::Pod) -> Self {
        Self {
            host: proto.host,
            port: proto.port as u16,
        }
    }
}

impl Display for Pod {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RoutingTable {
    pub number_of_shards: usize,
    pub shard_assignments: HashMap<Pod, HashSet<ShardId>>,
}

impl RoutingTable {
    pub fn new(number_of_shards: usize) -> Self {
        Self {
            number_of_shards,
            shard_assignments: HashMap::new(),
        }
    }

    pub fn get_entries(&self) -> BTreeSet<RoutingTableEntry> {
        self.shard_assignments
            .clone()
            .into_iter()
            .map(|(pod, shard_ids)| RoutingTableEntry::new(pod, shard_ids))
            .collect()
    }

    pub fn get_pods(&self) -> HashSet<Pod> {
        self.shard_assignments.clone().into_keys().collect()
    }

    pub fn rebalance(&mut self, rebalance: &Rebalance) {
        for (pod, shard_ids) in rebalance.assignments.assignments.clone() {
            self.shard_assignments
                .get_mut(&pod)
                .unwrap()
                .extend(shard_ids);
        }
        for (pod, shard_ids) in rebalance.unassignments.unassignments.clone() {
            self.shard_assignments
                .get_mut(&pod)
                .unwrap()
                .retain(|shard_id| !shard_ids.contains(shard_id));
        }
    }

    pub fn get_unassigned_shards(&self) -> HashSet<ShardId> {
        let mut unassigned_shards: HashSet<ShardId> = (0..self.number_of_shards)
            .map(|shard_id| ShardId::new(shard_id as i64))
            .collect();
        for assigned_shards in self.shard_assignments.values() {
            unassigned_shards = unassigned_shards
                .difference(assigned_shards)
                .cloned()
                .collect();
        }
        unassigned_shards
    }

    pub fn get_shards(&self, pod: &Pod) -> Option<HashSet<ShardId>> {
        self.shard_assignments.get(pod).cloned()
    }

    pub fn get_pod_count(&self) -> usize {
        self.shard_assignments.len()
    }

    pub fn add_pod(&mut self, pod: &Pod) {
        self.shard_assignments.insert(pod.clone(), HashSet::new());
    }

    pub fn remove_pod(&mut self, pod: &Pod) {
        self.shard_assignments.remove(pod);
    }
}

impl From<RoutingTable> for golem::RoutingTable {
    fn from(routing_table: RoutingTable) -> golem::RoutingTable {
        golem::RoutingTable {
            number_of_shards: routing_table.number_of_shards as u32,
            shard_assignments: routing_table
                .shard_assignments
                .into_iter()
                .flat_map(|(pod, shard_ids)| {
                    shard_ids
                        .into_iter()
                        .map(move |shard_id| (pod.clone(), shard_id))
                })
                .map(|(pod, shard_id)| golem::RoutingTableEntry {
                    pod: Some(pod.into()),
                    shard_id: Some(shard_id.into()),
                })
                .collect(),
        }
    }
}

impl From<golem::RoutingTable> for RoutingTable {
    fn from(routing_table: golem::RoutingTable) -> Self {
        let mut shard_assignments: HashMap<Pod, HashSet<ShardId>> = HashMap::new();
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
    pub shard_ids: HashSet<ShardId>,
}

impl RoutingTableEntry {
    pub fn new(pod: Pod, shard_ids: HashSet<ShardId>) -> Self {
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
    pub assignments: HashMap<Pod, HashSet<ShardId>>,
}

impl Assignments {
    pub fn assign(&mut self, pod: Pod, shard_id: ShardId) {
        self.assignments.entry(pod).or_default().insert(shard_id);
    }

    pub fn new() -> Self {
        Self {
            assignments: HashMap::new(),
        }
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
    pub unassignments: HashMap<Pod, HashSet<ShardId>>,
}

impl Unassignments {
    pub fn unassign(&mut self, pod: Pod, shard_id: ShardId) {
        self.unassignments.entry(pod).or_default().insert(shard_id);
    }

    pub fn new() -> Self {
        Self {
            unassignments: HashMap::new(),
        }
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Rebalance {
    pub assignments: Assignments,
    pub unassignments: Unassignments,
}

impl Rebalance {
    pub fn from_routing_table(routing_table: &RoutingTable) -> Self {
        let mut assignments = Assignments::new();
        let mut unassignments = Unassignments::new();
        let pod_count = routing_table.get_pod_count();
        if pod_count == 0 {
            return Rebalance {
                assignments,
                unassignments,
            };
        }
        let mut routing_table_entries = routing_table.get_entries();
        let unassigned_shards = routing_table.get_unassigned_shards();
        for unassigned_shard in unassigned_shards {
            let mut routing_table_entry = routing_table_entries.pop_first().unwrap();
            assignments.assign(routing_table_entry.pod.clone(), unassigned_shard);
            routing_table_entry.shard_ids.insert(unassigned_shard);
            routing_table_entries.insert(routing_table_entry);
        }
        if pod_count == 1 {
            return Rebalance {
                assignments,
                unassignments,
            };
        };
        loop {
            let mut first = routing_table_entries.pop_first().unwrap();
            let mut last = routing_table_entries.pop_last().unwrap();
            if first.pod == last.pod || last.get_shard_count() - first.get_shard_count() <= 1 {
                break;
            }
            let shard_id = *last.shard_ids.iter().next().unwrap();
            first.shard_ids.insert(shard_id);
            last.shard_ids.remove(&shard_id);
            assignments.assign(first.pod.clone(), shard_id);
            unassignments.unassign(last.pod.clone(), shard_id);
            routing_table_entries.insert(first);
            routing_table_entries.insert(last);
        }
        Rebalance {
            assignments,
            unassignments,
        }
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

    pub fn new() -> Self {
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
        for (pod, shard_ids) in &rebalance.assignments.assignments {
            assignments.push((pod.clone(), shard_ids.iter().cloned().collect()));
        }
        let mut unassignments: Vec<(Pod, Vec<ShardId>)> = Vec::new();
        for (pod, shard_ids) in &rebalance.unassignments.unassignments {
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
        let mut shard_assignments: HashMap<Pod, HashSet<ShardId>> = HashMap::new();
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
        Rebalance {
            assignments,
            unassignments,
        }
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

fn shard_assignments_map_to_string(shard_assignments: &HashMap<Pod, HashSet<ShardId>>) -> String {
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
