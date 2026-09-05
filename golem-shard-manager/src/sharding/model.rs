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

use super::error::ShardManagerError;
use super::rebalancing::Rebalance;
use chrono::{DateTime, Utc};
use desert_rust::BinaryCodec;
use golem_api_grpc::proto::golem;
use golem_common::model::{Pod, ShardId};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::net::IpAddr;
use std::time::Duration;
use tracing::warn;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BinaryCodec)]
#[desert(transparent)]
pub struct ShardEpoch(pub u64);

impl ShardEpoch {
    pub fn initial() -> Self {
        Self(0)
    }

    pub fn next(self) -> Self {
        Self(self.0.checked_add(1).expect("ShardEpoch overflow"))
    }
}

impl Display for ShardEpoch {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BinaryCodec)]
#[desert(transparent)]
pub struct ExecutorId(pub Uuid);

impl ExecutorId {
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for ExecutorId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BinaryCodec)]
#[desert(evolution())]
pub struct ExecutorAddr {
    pub ip: IpAddr,
    pub port: u16,
}

impl From<Pod> for ExecutorAddr {
    fn from(pod: Pod) -> Self {
        Self {
            ip: pod.ip,
            port: pod.port,
        }
    }
}

impl From<ExecutorAddr> for Pod {
    fn from(addr: ExecutorAddr) -> Self {
        Pod {
            ip: addr.ip,
            port: addr.port,
        }
    }
}

impl Display for ExecutorAddr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.ip, self.port)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, BinaryCodec)]
#[desert(transparent)]
pub struct ShardLeaseRevision(pub u64);

impl ShardLeaseRevision {
    pub const INITIAL: Self = Self(0);

    pub fn next(self) -> Result<Self, ShardManagerError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or_else(|| ShardManagerError::Internal("ShardLeaseRevision overflow".to_string()))
    }
}

impl Display for ShardLeaseRevision {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct ShardAssignmentEntry {
    pub executor_id: ExecutorId,
    pub epoch: ShardEpoch,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct ExecutorLease {
    pub addr: ExecutorAddr,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub pod_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct ShardLeaseState {
    pub number_of_shards: usize,
    pub revision: ShardLeaseRevision,
    pub shard_assignments: BTreeMap<ShardId, ShardAssignmentEntry>,
    pub shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    pub executor_leases: BTreeMap<ExecutorId, ExecutorLease>,
    pub pending_rebalance: BTreeSet<ShardId>,
}

#[derive(Clone, Debug)]
pub struct ExecutorShards {
    pub executor_id: ExecutorId,
    pub shard_ids: BTreeSet<ShardId>,
}

/// What the manager hands an executor when it grants or renews a shard lease: the complete set of
/// shards that executor owns with the ownership epoch of each, and the absolute time the lease
/// lapses if it is not renewed.
///
/// `BTreeMap` rather than `HashMap` so the encoded `repeated ShardEpochEntry` is deterministic and
/// a recorded push can be asserted on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardLeaseGrant {
    pub shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    pub expires_at: DateTime<Utc>,
}

/// The acknowledgement of a registration: the granted lease plus the cluster shard count, which
/// only `Register` carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterAck {
    pub number_of_shards: usize,
    pub grant: ShardLeaseGrant,
}

/// The complete shard set the manager wants one executor to hold, the ownership epoch of each
/// shard, the absolute time that executor's lease lapses, and the cluster shard count.
///
/// This is the whole of `AssignShardsRequest`: the push is a full replace, so the executor holds
/// exactly `shard_epochs` afterwards and drops everything else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardAssignmentPush {
    pub shard_epochs: BTreeMap<ShardId, ShardEpoch>,
    pub expires_at: DateTime<Utc>,
    pub number_of_shards: usize,
}

pub type ExecutorAddrs = BTreeMap<ExecutorId, ExecutorAddr>;

impl ShardLeaseState {
    pub fn new(number_of_shards: usize) -> Self {
        Self {
            number_of_shards,
            revision: ShardLeaseRevision::INITIAL,
            shard_assignments: BTreeMap::new(),
            shard_epochs: BTreeMap::new(),
            executor_leases: BTreeMap::new(),
            pending_rebalance: BTreeSet::new(),
        }
    }

    pub fn get_executors(&self) -> impl Iterator<Item = &ExecutorId> {
        self.executor_leases.keys()
    }

    pub fn has_executor(&self, executor_id: ExecutorId) -> bool {
        self.executor_leases.contains_key(&executor_id)
    }

    pub fn executor_count(&self) -> usize {
        self.executor_leases.len()
    }

    pub fn get_executors_with_addrs(&self) -> Vec<(ExecutorId, ExecutorAddr, Option<String>)> {
        self.executor_leases
            .iter()
            .map(|(id, lease)| (*id, lease.addr, lease.pod_name.clone()))
            .collect()
    }

    pub fn addr_for(&self, executor_id: ExecutorId) -> Option<ExecutorAddr> {
        self.executor_leases
            .get(&executor_id)
            .map(|lease| lease.addr)
    }

    pub fn executor_addrs(&self) -> ExecutorAddrs {
        self.executor_leases
            .iter()
            .map(|(id, lease)| (*id, lease.addr))
            .collect()
    }

    pub fn executor_for_addr(&self, addr: ExecutorAddr) -> Option<ExecutorId> {
        self.executor_leases
            .iter()
            .find(|(_, lease)| lease.addr == addr)
            .map(|(id, _)| *id)
    }

    pub fn shards_for_executor(&self, executor_id: ExecutorId) -> Option<BTreeSet<ShardId>> {
        if !self.has_executor(executor_id) {
            return None;
        }
        Some(
            self.shard_assignments
                .iter()
                .filter(|(_, entry)| entry.executor_id == executor_id)
                .map(|(shard_id, _)| *shard_id)
                .collect(),
        )
    }

    /// The lease `executor_id` currently holds, or `None` if it holds none.
    pub fn lease_grant_for(&self, executor_id: ExecutorId) -> Option<ShardLeaseGrant> {
        let lease = self.executor_leases.get(&executor_id)?;
        Some(ShardLeaseGrant {
            shard_epochs: self
                .shard_assignments
                .iter()
                .filter(|(_, entry)| entry.executor_id == executor_id)
                .map(|(shard_id, entry)| (*shard_id, entry.epoch))
                .collect(),
            expires_at: lease.expires_at,
        })
    }

    /// The full-replace payload for `executor_id`, or `None` if it holds no lease.
    ///
    /// An executor that holds a lease but no shards still gets a payload: an empty `shard_epochs`
    /// is how the manager tells it to drop everything it thinks it owns.
    pub fn assignment_push_for(&self, executor_id: ExecutorId) -> Option<ShardAssignmentPush> {
        self.lease_grant_for(executor_id)
            .map(|grant| ShardAssignmentPush {
                shard_epochs: grant.shard_epochs,
                expires_at: grant.expires_at,
                number_of_shards: self.number_of_shards,
            })
    }

    pub fn executor_shard_sets(&self) -> Vec<ExecutorShards> {
        let mut by_executor: BTreeMap<ExecutorId, BTreeSet<ShardId>> = self
            .executor_leases
            .keys()
            .map(|id| (*id, BTreeSet::new()))
            .collect();
        for (shard_id, entry) in &self.shard_assignments {
            if let Some(shard_ids) = by_executor.get_mut(&entry.executor_id) {
                shard_ids.insert(*shard_id);
            }
        }
        by_executor
            .into_iter()
            .map(|(executor_id, shard_ids)| ExecutorShards {
                executor_id,
                shard_ids,
            })
            .collect()
    }

    // If new added; returns None, if replaced; returns the replaced ExecutorId.
    pub fn add_executor(
        &mut self,
        executor_id: ExecutorId,
        addr: ExecutorAddr,
        pod_name: Option<String>,
        now: DateTime<Utc>,
        lease_ttl: Duration,
    ) -> Option<ExecutorId> {
        let replaced = match self.executor_for_addr(addr) {
            Some(previous) if previous != executor_id => {
                self.executor_leases.remove(&previous);
                Some(previous)
            }
            _ => None,
        };

        self.executor_leases.insert(
            executor_id,
            ExecutorLease {
                addr,
                granted_at: now,
                expires_at: now + lease_ttl,
                pod_name,
            },
        );

        if let Some(previous) = replaced {
            // The predecessor's shards change owner, so each of them advances its epoch.
            let transferred: Vec<ShardId> = self
                .shard_assignments
                .iter()
                .filter(|(_, entry)| entry.executor_id == previous)
                .map(|(shard_id, _)| *shard_id)
                .collect();
            for shard_id in transferred {
                self.assign_shard(executor_id, shard_id);
            }
        }

        debug_assert!(self.check_invariants().is_ok());
        replaced
    }

    pub fn remove_executor(&mut self, executor_id: ExecutorId) -> BTreeSet<ShardId> {
        if self.executor_leases.remove(&executor_id).is_none() {
            return BTreeSet::new();
        }
        let orphaned: BTreeSet<ShardId> = self
            .shard_assignments
            .iter()
            .filter(|(_, entry)| entry.executor_id == executor_id)
            .map(|(shard_id, _)| *shard_id)
            .collect();
        for shard_id in &orphaned {
            self.shard_assignments.remove(shard_id);
        }
        self.pending_rebalance.extend(orphaned.iter().copied());

        debug_assert!(self.check_invariants().is_ok());
        orphaned
    }

    /// Restarts the lease clock of `executor_id`. `false` if it holds no lease.
    ///
    /// Only the clock moves: a renewal never touches a shard assignment and never advances an
    /// epoch, which is what lets an executor assert the set it holds without the assertion racing
    /// the manager.
    pub fn renew_lease(
        &mut self,
        executor_id: ExecutorId,
        now: DateTime<Utc>,
        lease_ttl: Duration,
    ) -> bool {
        match self.executor_leases.get_mut(&executor_id) {
            Some(lease) => {
                lease.granted_at = now;
                lease.expires_at = now + lease_ttl;
                true
            }
            None => false,
        }
    }

    /// Restarts the lease clock of every listed executor that still holds one, and reports how
    /// many were re-granted.
    ///
    /// Persisted expiries are absolute, so after any outage longer than the lease every one of them
    /// is in the past and the first housekeeping would evict a cluster that is perfectly healthy.
    /// A shard manager coming up re-grants to exactly the executors its startup health check just
    /// found alive.
    pub fn regrant_leases(
        &mut self,
        executors: &HashSet<ExecutorId>,
        now: DateTime<Utc>,
        lease_ttl: Duration,
    ) -> usize {
        executors
            .iter()
            .filter(|executor_id| self.renew_lease(**executor_id, now, lease_ttl))
            .count()
    }

    pub fn contains_shard(&self, shard_id: ShardId) -> bool {
        shard_id.value() >= 0 && (shard_id.value() as usize) < self.number_of_shards
    }

    pub fn get_unassigned_shards(&self) -> BTreeSet<ShardId> {
        (0..self.number_of_shards)
            .map(|shard_id| ShardId::new(shard_id as i64))
            .filter(|shard_id| !self.shard_assignments.contains_key(shard_id))
            .collect()
    }

    /// The ownership epoch `shard_id` takes when it is assigned to `executor_id`: unchanged while
    /// the owner stays the same, one past the highest epoch ever recorded for that shard when the
    /// owner changes.
    ///
    /// Pure, and the single definition of the rule: [`Self::assign_shard`] mints with it, and
    /// [`Rebalance`] uses it to decide a plan's epochs at plan time so that the epoch pushed to an
    /// executor is the one that is later stored.
    pub fn next_epoch_for(&self, executor_id: ExecutorId, shard_id: ShardId) -> ShardEpoch {
        match self.shard_assignments.get(&shard_id) {
            Some(entry) if entry.executor_id == executor_id => entry.epoch,
            _ => match self.shard_epochs.get(&shard_id) {
                Some(last) => last.next(),
                None => ShardEpoch::initial(),
            },
        }
    }

    pub fn assign_shard(&mut self, executor_id: ExecutorId, shard_id: ShardId) -> ShardEpoch {
        let epoch = self.next_epoch_for(executor_id, shard_id);
        self.assign_shard_with_epoch(executor_id, shard_id, epoch);
        epoch
    }

    /// Records an assignment whose epoch was decided earlier, by [`Self::next_epoch_for`] against
    /// the state this assignment is applied to.
    fn assign_shard_with_epoch(
        &mut self,
        executor_id: ExecutorId,
        shard_id: ShardId,
        epoch: ShardEpoch,
    ) {
        debug_assert!(
            self.has_executor(executor_id),
            "assigning shard {shard_id} to executor {executor_id} without a lease"
        );
        debug_assert!(
            self.contains_shard(shard_id),
            "assigning shard {shard_id} outside 0..{}",
            self.number_of_shards
        );
        self.shard_assignments
            .insert(shard_id, ShardAssignmentEntry { executor_id, epoch });
        self.shard_epochs.insert(shard_id, epoch);
        self.pending_rebalance.remove(&shard_id);
    }

    pub fn unassign_shard(&mut self, owner: ExecutorId, shard_id: ShardId) -> bool {
        match self.shard_assignments.get(&shard_id) {
            Some(entry) if entry.executor_id == owner => {
                self.shard_assignments.remove(&shard_id);
                true
            }
            _ => false,
        }
    }

    pub fn epoch_for_shard(&self, shard_id: ShardId) -> Option<ShardEpoch> {
        self.shard_assignments
            .get(&shard_id)
            .map(|entry| entry.epoch)
    }

    /// Applies `rebalance`, and reports the executors whose push is now stale and has to be
    /// repeated.
    ///
    /// The epoch of every planned assignment was decided when the plan was computed and has
    /// already been pushed to its target, so it is applied rather than re-minted: minting again
    /// here would store an epoch the executor was never told. That holds only while the epoch is
    /// still *fresh* - strictly above the highest epoch recorded for that shard. `Register` writes
    /// outside the loop and transfers a restarted instance's shards inline, so between the push and
    /// this apply the recorded epoch can have caught up with the carried one. Storing it anyway
    /// would leave two live executors believing they hold the same shard at the same epoch, which
    /// is exactly the pair an oplog fence cannot tell apart. So a fresh epoch is minted instead and
    /// the target is reported back, for a full push that tells it the epoch it actually holds; the
    /// cost is one pass in which that executor is fenced out rather than admitted.
    pub fn apply_rebalance(&mut self, rebalance: &Rebalance) -> BTreeSet<ExecutorId> {
        let mut stale_pushes: BTreeSet<ExecutorId> = BTreeSet::new();
        for (executor_id, shard_ids) in &rebalance.get_assignments().assignments {
            if !self.has_executor(*executor_id) {
                warn!(
                    executor_id = %executor_id,
                    shards = shard_ids.len(),
                    "Skipping planned shard assignments: executor no longer holds a lease"
                );
                continue;
            }
            for shard_id in shard_ids {
                let epoch = match rebalance.epoch_for(*shard_id) {
                    Some(carried) if self.epoch_is_fresh(*shard_id, carried) => carried,
                    Some(carried) => {
                        let minted = self.next_epoch_for(*executor_id, *shard_id);
                        warn!(
                            executor_id = %executor_id,
                            shard_id = %shard_id,
                            planned_epoch = %carried,
                            minted_epoch = %minted,
                            "Planned shard epoch was overtaken before the plan was applied; \
                             minting a fresh one and repeating the push"
                        );
                        stale_pushes.insert(*executor_id);
                        minted
                    }
                    None => self.next_epoch_for(*executor_id, *shard_id),
                };
                self.assign_shard_with_epoch(*executor_id, *shard_id, epoch);
            }
        }
        for (executor_id, shard_ids) in &rebalance.get_unassignments().unassignments {
            for shard_id in shard_ids {
                self.unassign_shard(*executor_id, *shard_id);
            }
        }
        debug_assert!(self.check_invariants().is_ok());
        stale_pushes
    }

    /// Whether `epoch` is still above every epoch ever recorded for `shard_id`, and so may be
    /// stored without two holders ending up on the same `(shard, epoch)`.
    fn epoch_is_fresh(&self, shard_id: ShardId, epoch: ShardEpoch) -> bool {
        match self.shard_epochs.get(&shard_id) {
            Some(high_water) => epoch > *high_water,
            None => true,
        }
    }

    pub fn take_pending_rebalance(&mut self) -> BTreeSet<ShardId> {
        std::mem::take(&mut self.pending_rebalance)
    }

    pub fn housekeep(&mut self, now: DateTime<Utc>) -> Vec<(ExecutorId, BTreeSet<ShardId>)> {
        let expired: Vec<ExecutorId> = self
            .executor_leases
            .iter()
            .filter(|(_, lease)| now >= lease.expires_at)
            .map(|(id, _)| *id)
            .collect();
        expired
            .into_iter()
            .map(|executor_id| (executor_id, self.remove_executor(executor_id)))
            .collect()
    }

    pub fn bump_revision(&mut self) -> Result<ShardLeaseRevision, ShardManagerError> {
        self.revision = self.revision.next()?;
        Ok(self.revision)
    }

    pub fn check_invariants(&self) -> Result<(), String> {
        let mut addrs = BTreeSet::new();
        for (executor_id, lease) in &self.executor_leases {
            if !addrs.insert(lease.addr) {
                return Err(format!(
                    "address {} is leased by more than one executor (including {executor_id})",
                    lease.addr
                ));
            }
        }
        for (shard_id, entry) in &self.shard_assignments {
            if !self.has_executor(entry.executor_id) {
                return Err(format!(
                    "shard {shard_id} is assigned to executor {} which holds no lease",
                    entry.executor_id
                ));
            }
            if !self.contains_shard(*shard_id) {
                return Err(format!(
                    "shard {shard_id} is outside 0..{}",
                    self.number_of_shards
                ));
            }
            if self.pending_rebalance.contains(shard_id) {
                return Err(format!(
                    "shard {shard_id} is both assigned and pending rebalance"
                ));
            }
            match self.shard_epochs.get(shard_id) {
                Some(last) if *last == entry.epoch => {}
                Some(last) => {
                    return Err(format!(
                        "shard {shard_id} is assigned with epoch {} but its recorded highest epoch is {last}",
                        entry.epoch
                    ));
                }
                None => {
                    return Err(format!(
                        "shard {shard_id} is assigned with epoch {} but has no recorded epoch",
                        entry.epoch
                    ));
                }
            }
        }
        for shard_id in self.shard_epochs.keys() {
            if !self.contains_shard(*shard_id) {
                return Err(format!(
                    "shard {shard_id} with a recorded epoch is outside 0..{}",
                    self.number_of_shards
                ));
            }
        }
        for shard_id in &self.pending_rebalance {
            if !self.contains_shard(*shard_id) {
                return Err(format!(
                    "pending shard {shard_id} is outside 0..{}",
                    self.number_of_shards
                ));
            }
        }
        Ok(())
    }
}

impl From<ShardLeaseState> for golem::shardmanager::RoutingTable {
    fn from(shard_state: ShardLeaseState) -> golem::shardmanager::RoutingTable {
        golem::shardmanager::RoutingTable {
            number_of_shards: shard_state.number_of_shards as u32,
            shard_assignments: shard_state
                .shard_assignments
                .iter()
                .filter_map(|(shard_id, entry)| {
                    shard_state
                        .executor_leases
                        .get(&entry.executor_id)
                        .map(|lease| (*shard_id, lease.addr))
                })
                .map(|(shard_id, addr)| golem::shardmanager::RoutingTableEntry {
                    pod: Some(Pod::from(addr).into()),
                    shard_id: Some(shard_id.into()),
                })
                .collect(),
        }
    }
}

impl Display for ShardLeaseState {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let by_executor = self.executor_shard_sets();
        let executors: Vec<String> = by_executor
            .iter()
            .map(|entry| {
                let lease = &self.executor_leases[&entry.executor_id];
                shard_assignments_to_string(
                    &format!("{} {}", entry.executor_id, lease.addr),
                    lease.pod_name.as_deref(),
                    entry.shard_ids.iter(),
                )
            })
            .collect();
        let pending: Vec<String> = shard_ids_to_ranges(self.pending_rebalance.iter())
            .iter()
            .map(|rng| rng.to_string())
            .collect();
        write!(
            f,
            "{{ number_of_shards: {}, revision: {}, executors: [{}], pending_rebalance: [{}] }}",
            self.number_of_shards,
            self.revision,
            executors.join(", "),
            pending.join(", ")
        )
    }
}

#[derive(Clone, Debug)]
pub struct Assignments {
    pub assignments: BTreeMap<ExecutorId, BTreeSet<ShardId>>,
}

impl Assignments {
    pub fn assign(&mut self, executor_id: ExecutorId, shard_id: ShardId) {
        self.assignments
            .entry(executor_id)
            .or_default()
            .insert(shard_id);
    }

    pub fn unassign(&mut self, executor_id: ExecutorId, shard_id: ShardId) {
        self.assignments
            .entry(executor_id)
            .or_default()
            .remove(&shard_id);
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
            executor_shard_ids_map_to_string(&self.assignments)
        )
    }
}

#[derive(Clone, Debug)]
pub struct Unassignments {
    pub unassignments: BTreeMap<ExecutorId, BTreeSet<ShardId>>,
}

impl Unassignments {
    pub fn unassign(&mut self, executor_id: ExecutorId, shard_id: ShardId) {
        self.unassignments
            .entry(executor_id)
            .or_default()
            .insert(shard_id);
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
            executor_shard_ids_map_to_string(&self.unassignments)
        )
    }
}

fn executor_shard_ids_map_to_string(
    shards_by_executor: &BTreeMap<ExecutorId, BTreeSet<ShardId>>,
) -> String {
    let elements: Vec<String> = shards_by_executor
        .iter()
        .map(|(executor_id, shard_ids)| {
            shard_assignments_to_string(executor_id, None, shard_ids.iter())
        })
        .collect();
    elements.join(", ")
}

pub fn shard_assignments_to_string<'a, T: Iterator<Item = &'a ShardId>>(
    label: &dyn Display,
    pod_name: Option<&str>,
    shard_ids: T,
) -> String {
    let ranges: Vec<ShardIdRange> = shard_ids_to_ranges(shard_ids);
    let strings: Vec<String> = ranges
        .iter()
        .map(|rng| format!("{rng}").to_string())
        .collect();
    format!(
        "{label} {}: [{}]",
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

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use golem_common::model::ShardId;
    use std::net::Ipv4Addr;

    const TTL: Duration = Duration::from_secs(60);

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn executor(idx: u128) -> ExecutorId {
        ExecutorId(Uuid::from_u128(idx))
    }

    fn addr(idx: u8) -> ExecutorAddr {
        ExecutorAddr {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, idx)),
            port: 9000 + idx as u16,
        }
    }

    fn shard(id: i64) -> ShardId {
        ShardId::new(id)
    }

    fn shards(ids: &[i64]) -> BTreeSet<ShardId> {
        ids.iter().copied().map(ShardId::new).collect()
    }

    fn shard_state_with(
        number_of_shards: usize,
        executors: &[(u128, u8, &[i64])],
    ) -> ShardLeaseState {
        let mut shard_state = ShardLeaseState::new(number_of_shards);
        for (idx, addr_idx, shard_ids) in executors {
            shard_state.add_executor(executor(*idx), addr(*addr_idx), None, t0(), TTL);
            for shard_id in *shard_ids {
                shard_state.assign_shard(executor(*idx), shard(*shard_id));
            }
        }
        shard_state
    }

    #[test]
    fn new_state_is_empty_with_initial_revision() {
        let shard_state = ShardLeaseState::new(8);
        assert_eq!(shard_state.number_of_shards, 8);
        assert_eq!(shard_state.revision, ShardLeaseRevision::INITIAL);
        assert!(shard_state.shard_assignments.is_empty());
        assert!(shard_state.shard_epochs.is_empty());
        assert!(shard_state.executor_leases.is_empty());
        assert!(shard_state.pending_rebalance.is_empty());
        assert_eq!(shard_state.executor_count(), 0);
        assert_eq!(
            shard_state.get_unassigned_shards(),
            shards(&[0, 1, 2, 3, 4, 5, 6, 7])
        );
        assert!(shard_state.check_invariants().is_ok());
    }

    #[test]
    fn generated_executor_ids_are_unique_and_time_ordered() {
        let ids: Vec<ExecutorId> = (0..64).map(|_| ExecutorId::generate()).collect();
        let unique: BTreeSet<ExecutorId> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len());
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn add_executor_grants_lease_with_expected_window() {
        let mut shard_state = ShardLeaseState::new(4);
        let replaced =
            shard_state.add_executor(executor(1), addr(1), Some("pod-1".to_string()), t0(), TTL);
        assert_eq!(replaced, None);

        let lease = &shard_state.executor_leases[&executor(1)];
        assert_eq!(lease.addr, addr(1));
        assert_eq!(lease.granted_at, t0());
        assert_eq!(lease.expires_at, t0() + chrono::Duration::seconds(60));
        assert_eq!(lease.pod_name.as_deref(), Some("pod-1"));

        assert!(shard_state.has_executor(executor(1)));
        assert_eq!(shard_state.addr_for(executor(1)), Some(addr(1)));
        assert_eq!(shard_state.executor_for_addr(addr(1)), Some(executor(1)));
        assert_eq!(
            shard_state.shards_for_executor(executor(1)),
            Some(BTreeSet::new())
        );
        assert_eq!(shard_state.shards_for_executor(executor(2)), None);
        assert_eq!(
            shard_state.get_executors_with_addrs(),
            vec![(executor(1), addr(1), Some("pod-1".to_string()))]
        );
    }

    #[test]
    fn add_executor_at_reused_address_replaces_lease_and_transfers_shards() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[2])]);
        let mut later = t0() + chrono::Duration::seconds(10);

        // bump shard 1 once so we can tell "advanced" from "reset": moving it to executor 2 and
        // back advances its epoch to 1
        shard_state.assign_shard(executor(2), shard(1));
        shard_state.assign_shard(executor(1), shard(1));
        assert_eq!(shard_state.epoch_for_shard(shard(1)), Some(ShardEpoch(2)));

        let replaced = shard_state.add_executor(executor(3), addr(1), None, later, TTL);
        assert_eq!(replaced, Some(executor(1)));

        assert!(!shard_state.has_executor(executor(1)));
        assert!(shard_state.has_executor(executor(3)));
        assert_eq!(shard_state.executor_for_addr(addr(1)), Some(executor(3)));
        assert_eq!(shard_state.executor_count(), 2);

        assert_eq!(
            shard_state.shards_for_executor(executor(3)),
            Some(shards(&[0, 1]))
        );
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
        assert_eq!(shard_state.epoch_for_shard(shard(1)), Some(ShardEpoch(3)));
        assert_eq!(
            shard_state.shard_epochs.get(&shard(1)),
            Some(&ShardEpoch(3))
        );
        assert_eq!(
            shard_state.shards_for_executor(executor(2)),
            Some(shards(&[2]))
        );
        assert_eq!(shard_state.epoch_for_shard(shard(2)), Some(ShardEpoch(0)));
        assert!(shard_state.pending_rebalance.is_empty());
        assert_eq!(shard_state.executor_leases[&executor(3)].granted_at, later);

        // a second registration of the same id at the same address only refreshes the lease
        later += chrono::Duration::seconds(10);
        let replaced = shard_state.add_executor(executor(3), addr(1), None, later, TTL);
        assert_eq!(replaced, None);
        assert_eq!(
            shard_state.shards_for_executor(executor(3)),
            Some(shards(&[0, 1]))
        );
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
        assert_eq!(shard_state.executor_leases[&executor(3)].granted_at, later);
        assert!(shard_state.check_invariants().is_ok());
    }

    #[test]
    fn remove_executor_orphans_shards_into_pending_rebalance() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[2])]);

        let orphaned = shard_state.remove_executor(executor(1));
        assert_eq!(orphaned, shards(&[0, 1]));
        assert!(!shard_state.has_executor(executor(1)));
        assert_eq!(shard_state.pending_rebalance, shards(&[0, 1]));
        assert_eq!(shard_state.get_unassigned_shards(), shards(&[0, 1, 3]));
        assert_eq!(shard_state.shards_for_executor(executor(1)), None);
        assert_eq!(
            shard_state.shards_for_executor(executor(2)),
            Some(shards(&[2]))
        );
        assert!(shard_state.check_invariants().is_ok());

        // unknown ids are a no-op
        assert_eq!(shard_state.remove_executor(executor(42)), BTreeSet::new());
        assert_eq!(shard_state.executor_count(), 1);
    }

    #[test]
    fn assign_shard_epoch_semantics() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[]), (2, 2, &[])]);

        // first assignment starts at the initial epoch
        assert_eq!(
            shard_state.assign_shard(executor(1), shard(0)),
            ShardEpoch::initial()
        );
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(0)));

        // re-assigning to the same owner is idempotent
        assert_eq!(
            shard_state.assign_shard(executor(1), shard(0)),
            ShardEpoch(0)
        );

        // moving to another owner advances the epoch; stored == returned
        assert_eq!(
            shard_state.assign_shard(executor(2), shard(0)),
            ShardEpoch(1)
        );
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
        assert_eq!(
            shard_state.shards_for_executor(executor(1)),
            Some(BTreeSet::new())
        );
        assert_eq!(
            shard_state.shards_for_executor(executor(2)),
            Some(shards(&[0]))
        );

        // after a full unassignment the next assignment continues past the highest epoch ever
        // issued for the shard - an epoch is never reused in a shard's history
        assert!(shard_state.unassign_shard(executor(2), shard(0)));
        assert_eq!(shard_state.epoch_for_shard(shard(0)), None);
        assert_eq!(
            shard_state.shard_epochs.get(&shard(0)),
            Some(&ShardEpoch(1))
        );
        assert_eq!(
            shard_state.assign_shard(executor(1), shard(0)),
            ShardEpoch(2)
        );
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(2)));
    }

    #[test]
    fn epochs_stay_unique_across_eviction_and_reassignment() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[]), (3, 3, &[])]);
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(0)));

        // executor 1 is evicted (health check / lease expiry): its shards are orphaned ...
        shard_state.remove_executor(executor(1));
        assert_eq!(shard_state.epoch_for_shard(shard(0)), None);
        assert_eq!(shard_state.pending_rebalance, shards(&[0, 1]));

        // ... and later handed to other executors with epochs the evicted owner never held
        assert_eq!(
            shard_state.assign_shard(executor(2), shard(0)),
            ShardEpoch(1)
        );
        assert_eq!(
            shard_state.assign_shard(executor(3), shard(1)),
            ShardEpoch(1)
        );

        // a second eviction keeps advancing
        shard_state.remove_executor(executor(2));
        assert_eq!(
            shard_state.assign_shard(executor(3), shard(0)),
            ShardEpoch(2)
        );

        // housekeep-driven eviction behaves the same way
        let expired = t0() + chrono::Duration::seconds(3600);
        assert_eq!(
            shard_state.housekeep(expired),
            vec![(executor(3), shards(&[0, 1]))]
        );
        shard_state.add_executor(executor(4), addr(4), None, expired, TTL);
        assert_eq!(
            shard_state.assign_shard(executor(4), shard(0)),
            ShardEpoch(3)
        );
        assert_eq!(
            shard_state.assign_shard(executor(4), shard(1)),
            ShardEpoch(2)
        );
        assert!(shard_state.check_invariants().is_ok());
    }

    #[test]
    fn unassign_shard_is_guarded_by_owner() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0]), (2, 2, &[])]);

        assert!(!shard_state.unassign_shard(executor(2), shard(0)));
        assert_eq!(
            shard_state.shards_for_executor(executor(1)),
            Some(shards(&[0]))
        );

        assert!(shard_state.unassign_shard(executor(1), shard(0)));
        assert_eq!(
            shard_state.shards_for_executor(executor(1)),
            Some(BTreeSet::new())
        );
        assert!(!shard_state.unassign_shard(executor(1), shard(0)));
    }

    #[test]
    fn assign_shard_clears_pending_rebalance() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[])]);
        shard_state.remove_executor(executor(1));
        assert_eq!(shard_state.pending_rebalance, shards(&[0, 1]));

        shard_state.assign_shard(executor(2), shard(0));
        assert_eq!(shard_state.pending_rebalance, shards(&[1]));
        assert!(shard_state.check_invariants().is_ok());
    }

    #[test]
    fn apply_rebalance_moves_bump_epoch_once_and_unassignments_are_owner_guarded() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1, 2, 3]), (2, 2, &[])]);

        // move shards 0 and 1 from executor 1 to executor 2, as a Rebalance would express it
        let mut assignments = Assignments::new();
        assignments.assign(executor(2), shard(0));
        assignments.assign(executor(2), shard(1));
        let mut unassignments = Unassignments::new();
        unassignments.unassign(executor(1), shard(0));
        unassignments.unassign(executor(1), shard(1));
        let rebalance = Rebalance::new(assignments, unassignments, &shard_state);

        // The epochs are decided when the plan is built, because that is what the executors are
        // pushed before the plan is applied.
        assert_eq!(rebalance.epoch_for(shard(0)), Some(ShardEpoch(1)));
        assert_eq!(rebalance.epoch_for(shard(1)), Some(ShardEpoch(1)));
        assert_eq!(rebalance.epoch_for(shard(2)), None);

        let stale_pushes = shard_state.apply_rebalance(&rebalance);
        assert!(
            stale_pushes.is_empty(),
            "a plan applied to the state it was built from carries fresh epochs"
        );

        assert_eq!(
            shard_state.shards_for_executor(executor(1)),
            Some(shards(&[2, 3]))
        );
        assert_eq!(
            shard_state.shards_for_executor(executor(2)),
            Some(shards(&[0, 1]))
        );
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
        assert_eq!(shard_state.epoch_for_shard(shard(1)), Some(ShardEpoch(1)));
        assert_eq!(shard_state.epoch_for_shard(shard(2)), Some(ShardEpoch(0)));
        assert!(shard_state.check_invariants().is_ok());

        // Applying the same plan again is a no-op on the state (idempotent assignments, guarded
        // unassignments). The carried epoch is no longer above the recorded one - this very apply
        // put it there - so it is re-minted, which for an unchanged owner is the same value; the
        // target is reported for a repeat push, which is a redundant push, never a wrong one.
        let stale_pushes = shard_state.apply_rebalance(&rebalance);
        assert_eq!(stale_pushes, BTreeSet::from([executor(2)]));
        assert_eq!(
            shard_state.shards_for_executor(executor(2)),
            Some(shards(&[0, 1]))
        );
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));
    }

    #[test]
    // S11: `Register` writes outside the loop, so between the moment a plan's epochs were pushed to
    // their target and the moment the plan is applied, a restarted instance registering at a known
    // address can inherit the same shard and mint the same epoch inline. Storing the carried epoch
    // then would leave two live executors on the same `(shard, epoch)`, which is exactly the pair
    // an oplog fence cannot tell apart.
    fn a_planned_epoch_overtaken_before_the_apply_is_re_minted_and_reported() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1, 2, 3]), (2, 2, &[])]);

        // the plan: shard 0 moves from executor 1 to executor 2, at epoch 1
        let mut assignments = Assignments::new();
        assignments.assign(executor(2), shard(0));
        let mut unassignments = Unassignments::new();
        unassignments.unassign(executor(1), shard(0));
        let rebalance = Rebalance::new(assignments, unassignments, &shard_state);
        assert_eq!(rebalance.epoch_for(shard(0)), Some(ShardEpoch(1)));

        // ...and before it is applied, executor 1 is replaced at its own address by a restart,
        // which inherits shard 0 inline and mints epoch 1 for it too
        shard_state.add_executor(executor(3), addr(1), None, t0(), TTL);
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(1)));

        let stale_pushes = shard_state.apply_rebalance(&rebalance);

        assert_eq!(
            shard_state.epoch_for_shard(shard(0)),
            Some(ShardEpoch(2)),
            "the carried epoch was stored although the high-water mark had caught up with it"
        );
        assert_eq!(
            shard_state.shard_assignments[&shard(0)].executor_id,
            executor(2)
        );
        assert_eq!(
            stale_pushes,
            BTreeSet::from([executor(2)]),
            "the executor that was pushed the overtaken epoch must be queued for a full push"
        );
        assert!(shard_state.check_invariants().is_ok());
    }

    #[test]
    fn renew_lease_moves_only_the_clock() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1])]);
        let later = t0() + chrono::Duration::seconds(30);

        assert!(shard_state.renew_lease(executor(1), later, TTL));
        let lease = &shard_state.executor_leases[&executor(1)];
        assert_eq!(lease.granted_at, later);
        assert_eq!(lease.expires_at, later + chrono::Duration::seconds(60));
        assert_eq!(
            shard_state.shards_for_executor(executor(1)),
            Some(shards(&[0, 1]))
        );
        assert_eq!(shard_state.epoch_for_shard(shard(0)), Some(ShardEpoch(0)));

        assert!(!shard_state.renew_lease(executor(9), later, TTL));
    }

    #[test]
    fn regrant_leases_restarts_the_clock_of_the_listed_executors_only() {
        let mut shard_state = shard_state_with(4, &[(1, 1, &[0, 1]), (2, 2, &[2, 3])]);
        let later = t0() + chrono::Duration::seconds(3600);

        // executor 9 holds no lease and must not create one
        let regranted =
            shard_state.regrant_leases(&HashSet::from([executor(1), executor(9)]), later, TTL);

        assert_eq!(regranted, 1);
        assert_eq!(
            shard_state.executor_leases[&executor(1)].expires_at,
            later + chrono::Duration::seconds(60)
        );
        assert_eq!(
            shard_state.executor_leases[&executor(2)].expires_at,
            t0() + chrono::Duration::seconds(60)
        );
        assert!(!shard_state.has_executor(executor(9)));

        // the re-granted lease survives the housekeeping that evicts the one left behind
        let evicted = shard_state.housekeep(later);
        assert_eq!(evicted, vec![(executor(2), shards(&[2, 3]))]);
        assert!(shard_state.has_executor(executor(1)));
        assert_eq!(shard_state.pending_rebalance, shards(&[2, 3]));
    }

    #[test]
    fn executor_shard_sets_covers_every_lease_in_id_order() {
        let shard_state = shard_state_with(6, &[(3, 3, &[4]), (1, 1, &[0, 2]), (2, 2, &[])]);
        let sets = shard_state.executor_shard_sets();
        let ids: Vec<ExecutorId> = sets.iter().map(|s| s.executor_id).collect();
        assert_eq!(ids, vec![executor(1), executor(2), executor(3)]);
        assert_eq!(sets[0].shard_ids, shards(&[0, 2]));
        assert_eq!(sets[1].shard_ids, BTreeSet::new());
        assert_eq!(sets[2].shard_ids, shards(&[4]));
        assert_eq!(shard_state.get_unassigned_shards(), shards(&[1, 3, 5]));
    }

    #[test]
    fn housekeep_evicts_only_expired_leases() {
        let mut shard_state = ShardLeaseState::new(4);
        shard_state.add_executor(executor(1), addr(1), None, t0(), Duration::from_secs(10));
        shard_state.add_executor(executor(2), addr(2), None, t0(), Duration::from_secs(60));
        shard_state.assign_shard(executor(1), shard(0));
        shard_state.assign_shard(executor(2), shard(1));

        assert!(
            shard_state
                .housekeep(t0() + chrono::Duration::seconds(9))
                .is_empty()
        );
        assert_eq!(shard_state.executor_count(), 2);

        // expiry is inclusive
        let evicted = shard_state.housekeep(t0() + chrono::Duration::seconds(10));
        assert_eq!(evicted, vec![(executor(1), shards(&[0]))]);
        assert!(!shard_state.has_executor(executor(1)));
        assert!(shard_state.has_executor(executor(2)));
        assert_eq!(shard_state.pending_rebalance, shards(&[0]));
        assert_eq!(
            shard_state.shards_for_executor(executor(2)),
            Some(shards(&[1]))
        );

        assert_eq!(shard_state.take_pending_rebalance(), shards(&[0]));
        assert!(shard_state.pending_rebalance.is_empty());
        assert!(shard_state.check_invariants().is_ok());
    }

    #[test]
    fn bump_revision_increments_and_reports_overflow() {
        let mut shard_state = ShardLeaseState::new(1);
        assert_eq!(shard_state.bump_revision().unwrap(), ShardLeaseRevision(1));
        assert_eq!(shard_state.bump_revision().unwrap(), ShardLeaseRevision(2));
        assert_eq!(shard_state.revision, ShardLeaseRevision(2));

        shard_state.revision = ShardLeaseRevision(u64::MAX);
        match shard_state.bump_revision() {
            Err(ShardManagerError::Internal(msg)) => assert!(msg.contains("overflow")),
            other => panic!("expected Internal error, got {other:?}"),
        }
        assert_eq!(shard_state.revision, ShardLeaseRevision(u64::MAX));
    }

    #[test]
    fn binary_codec_roundtrip_is_exact() {
        let mut shard_state = shard_state_with(8, &[(1, 1, &[0, 1, 2]), (2, 2, &[5])]);
        shard_state
            .executor_leases
            .get_mut(&executor(2))
            .unwrap()
            .pod_name = Some("worker-executor-1".to_string());
        shard_state
            .executor_leases
            .get_mut(&executor(2))
            .unwrap()
            .granted_at = DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap();
        shard_state.assign_shard(executor(2), shard(0));
        shard_state.remove_executor(executor(1));
        shard_state.bump_revision().unwrap();

        let bytes = golem_common::serialization::serialize(&shard_state).unwrap();
        let decoded: ShardLeaseState = golem_common::serialization::deserialize(&bytes).unwrap();
        assert_eq!(decoded, shard_state);
    }

    #[test]
    fn proto_conversion_emits_one_entry_per_routable_shard() {
        let mut shard_state = shard_state_with(8, &[(1, 1, &[0, 1]), (2, 2, &[5])]);
        // violate invariant 1 on purpose: an assignment whose executor holds no lease
        shard_state.shard_assignments.insert(
            shard(7),
            ShardAssignmentEntry {
                executor_id: executor(99),
                epoch: ShardEpoch(3),
            },
        );
        assert!(shard_state.check_invariants().is_err());

        let proto: golem::shardmanager::RoutingTable = shard_state.into();
        assert_eq!(proto.number_of_shards, 8);
        assert_eq!(proto.shard_assignments.len(), 3);
        for entry in &proto.shard_assignments {
            assert!(entry.pod.is_some());
            assert!(entry.shard_id.is_some());
        }
        let mut routed: Vec<i64> = proto
            .shard_assignments
            .iter()
            .map(|entry| entry.shard_id.unwrap().value)
            .collect();
        routed.sort();
        assert_eq!(routed, vec![0, 1, 5]);
    }

    #[test]
    fn check_invariants_detects_each_violation() {
        let good = shard_state_with(4, &[(1, 1, &[0]), (2, 2, &[1])]);
        assert!(good.check_invariants().is_ok());

        let mut lease_less_owner = good.clone();
        lease_less_owner.shard_assignments.insert(
            shard(2),
            ShardAssignmentEntry {
                executor_id: executor(99),
                epoch: ShardEpoch(0),
            },
        );
        assert!(lease_less_owner.check_invariants().is_err());

        let mut duplicate_addr = good.clone();
        duplicate_addr
            .executor_leases
            .get_mut(&executor(2))
            .unwrap()
            .addr = addr(1);
        assert!(duplicate_addr.check_invariants().is_err());

        let mut out_of_range = good.clone();
        out_of_range.shard_assignments.insert(
            shard(4),
            ShardAssignmentEntry {
                executor_id: executor(1),
                epoch: ShardEpoch(0),
            },
        );
        assert!(out_of_range.check_invariants().is_err());

        let mut pending_and_assigned = good.clone();
        pending_and_assigned.pending_rebalance.insert(shard(0));
        assert!(pending_and_assigned.check_invariants().is_err());

        let mut pending_out_of_range = good.clone();
        pending_out_of_range.pending_rebalance.insert(shard(9));
        assert!(pending_out_of_range.check_invariants().is_err());

        let mut epoch_mismatch = good.clone();
        epoch_mismatch.shard_epochs.insert(shard(0), ShardEpoch(7));
        assert!(epoch_mismatch.check_invariants().is_err());

        let mut epoch_missing = good.clone();
        epoch_missing.shard_epochs.remove(&shard(0));
        assert!(epoch_missing.check_invariants().is_err());

        let mut epoch_out_of_range = good;
        epoch_out_of_range
            .shard_epochs
            .insert(shard(9), ShardEpoch(0));
        assert!(epoch_out_of_range.check_invariants().is_err());
    }

    #[test]
    fn display_is_readable() {
        let mut shard_state = shard_state_with(8, &[(1, 1, &[0, 1, 2, 5])]);
        shard_state
            .executor_leases
            .get_mut(&executor(1))
            .unwrap()
            .pod_name = Some("worker-executor-0".to_string());
        let rendered = shard_state.to_string();
        assert!(rendered.contains("number_of_shards: 8"));
        assert!(
            rendered.contains("10.0.0.1:9001 worker-executor-0: [<0>..<2>, <5>]"),
            "{rendered}"
        );
        assert!(rendered.contains("pending_rebalance: []"));
    }
}
