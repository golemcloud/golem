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

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use desert_rust::BinaryCodec;
use http::Uri;
use rand::prelude::IteratorRandom;
use tonic::transport::Endpoint;
use uuid::Uuid;

use crate::grpc_uri;
use crate::model::{AgentId, ShardId};

#[derive(Debug, Clone)]
pub struct NumberOfShards {
    pub value: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BinaryCodec)]
#[desert(evolution())]
pub struct Pod {
    pub ip: IpAddr,
    pub port: u16,
}

impl Pod {
    pub fn uri(&self, use_tls: bool) -> Uri {
        grpc_uri(&self.ip.to_string(), self.port, use_tls)
    }

    pub fn address(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }

    pub fn endpoint(&self, use_tls: bool) -> Endpoint {
        Endpoint::from(self.uri(use_tls))
    }
}

impl Display for Pod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.address())
    }
}

#[derive(Debug, Clone)]
pub struct RoutingTable {
    pub number_of_shards: NumberOfShards,
    pub(super) shard_assignments: HashMap<ShardId, Pod>,
}

impl RoutingTable {
    pub fn lookup(&self, agent_id: &AgentId) -> Option<&Pod> {
        self.shard_assignments.get(&ShardId::from_agent_id(
            &agent_id.clone(),
            self.number_of_shards.value,
        ))
    }

    pub fn random(&self) -> Option<&Pod> {
        self.shard_assignments.values().choose(&mut rand::rng())
    }

    pub fn first(&self) -> Option<&Pod> {
        self.shard_assignments.values().next()
    }

    pub fn all(&self) -> HashSet<&Pod> {
        self.shard_assignments.values().collect()
    }

    /// How many shards each pod currently owns.
    ///
    /// [`Self::all`] answers "which pods are in play", which is enough to route
    /// but not enough to see a rebalance: after a shard-manager restart the pod
    /// set can be unchanged while the distribution across it has moved, and that
    /// movement is what chaos scenarios report to an operator.
    pub fn shards_per_pod(&self) -> HashMap<&Pod, usize> {
        let mut counts: HashMap<&Pod, usize> = HashMap::new();
        for pod in self.shard_assignments.values() {
            *counts.entry(pod).or_default() += 1;
        }
        counts
    }
}

impl Display for RoutingTable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Number of shards: {}", self.number_of_shards.value)?;
        writeln!(f, "Pods used: {:?}", self.all())?;
        Ok(())
    }
}

#[allow(dead_code)]
pub struct RoutingTableEntry {
    pub(super) shard_id: ShardId,
    pub(super) pod: Pod,
}

/// Ownership generation of a single shard, granted by the shard manager.
///
/// The epoch advances only when a shard changes owner, never on a lease
/// renewal, so an executor can assert the set it believes it holds without the
/// assertion racing the manager. A newtype of this crate's own: the shard
/// manager keeps a twin in `sharding::model`, and the wire's `u64` is the only
/// bridge between them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShardEpoch(pub u64);

impl Display for ShardEpoch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where a delivered shard set sits in the shard manager's order: the manager
/// process that sent it, and the revision of the persisted state the set was
/// read from. Every delivery carries one - a registration, a push, a renewal -
/// and an executor applies a delivery only if its number is at least the last
/// one it applied from that process, so two deliveries that cross on the
/// network cannot leave the older set in place.
///
/// The numbers of two manager processes are not comparable: one that failed
/// over, or came back on a wiped or restored store, counts from a state this
/// executor's last applied number says nothing about. So this has no `Ord`, and
/// [`ShardAssignment`] is the one place the two halves are read together. The
/// executor's own type; it never imports the shard manager's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShardLeaseRevision {
    pub incarnation: Uuid,
    pub number: u64,
}

impl ShardLeaseRevision {
    /// Off the wire. Every delivery names the process that sent it, so an id
    /// that is empty, or not a UUID, makes the delivery malformed.
    pub fn from_wire(incarnation_id: &str, number: u64) -> Result<Self, String> {
        let incarnation = Uuid::parse_str(incarnation_id)
            .map_err(|error| format!("incarnation_id {incarnation_id:?} is not a UUID: {error}"))?;
        Ok(Self {
            incarnation,
            number,
        })
    }
}

impl Display for ShardLeaseRevision {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.number, self.incarnation)
    }
}

/// Which way a delivery reached this executor, which decides what a change of
/// manager process means - see [`transition_shard_assignment`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShardDeliveryPath {
    /// The answer to a registration or a renewal this executor sent.
    Reply,
    /// An `AssignShards` or `RevokeShards` the manager sent on its own.
    Push,
}

/// What applying a delivered shard set did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardDeliveryOutcome {
    /// Applied. `set_changed` is whether the owned set moved, which decides
    /// whether agents on dropped shards are swept and agents on gained shards
    /// recovered.
    Applied { set_changed: bool },
    /// Older than a delivery already applied, so ignored whole.
    Stale {
        delivered: ShardLeaseRevision,
        applied: ShardLeaseRevision,
    },
    /// A push from a manager process other than the one this executor follows,
    /// so ignored whole. The executor owes a renewal: its answer names the
    /// process in charge and carries that process's set.
    FromAnotherManager {
        delivered: ShardLeaseRevision,
        applied: ShardLeaseRevision,
    },
}

/// The shards this executor currently holds, with the epoch each was granted
/// at, and when the lease over them lapses on this executor's own clock, if
/// it lapses at all.
#[derive(Clone, Debug, Default)]
pub struct ShardAssignment {
    pub number_of_shards: usize,
    /// The executor lease count from the last accepted grant or assignment.
    pub executor_count: usize,
    /// Exactly the shards this executor holds. The shard manager pushes the
    /// complete set; anything absent from it has been dropped.
    pub shard_epochs: HashMap<ShardId, ShardEpoch>,
    /// When the shard lease lapses, on this executor's own monotonic clock.
    /// Only a grant moves it - the answer to this executor's own registration
    /// or renewal - and the grant is anchored where that request was sent, so
    /// the shard manager's copy of the lease is never earlier than this one.
    /// A push carries no lease. `None` means the lease never expires
    /// (single-shard mode and the pre-registration placeholder).
    pub expires_at: Option<Instant>,
    /// The revision of the delivery this set came from, and the shard manager
    /// process this executor follows. A delivery older than this, or pushed
    /// by another process, is ignored; see [`ShardLeaseRevision`]. `None`
    /// until a delivery has been applied, and for good on the single-shard
    /// assignment, which no shard manager delivers.
    pub revision: Option<ShardLeaseRevision>,
}

impl ShardAssignment {
    /// An assignment that never expires, with every shard at epoch 0. Used by
    /// the single-shard implementations and by tests, which have no shard
    /// manager to grant epochs.
    pub fn unexpiring(
        number_of_shards: usize,
        shard_ids: impl IntoIterator<Item = ShardId>,
    ) -> Self {
        Self {
            number_of_shards,
            executor_count: 1,
            shard_epochs: shard_ids
                .into_iter()
                .map(|shard_id| (shard_id, ShardEpoch::default()))
                .collect(),
            expires_at: None,
            revision: None,
        }
    }

    pub fn contains(&self, shard_id: &ShardId) -> bool {
        self.shard_epochs.contains_key(shard_id)
    }

    pub fn is_empty(&self) -> bool {
        self.shard_epochs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.shard_epochs.len()
    }

    pub fn shard_ids(&self) -> impl Iterator<Item = ShardId> + '_ {
        self.shard_epochs.keys().copied()
    }

    pub fn shard_id_set(&self) -> HashSet<ShardId> {
        self.shard_epochs.keys().copied().collect()
    }

    pub fn epoch_of(&self, shard_id: &ShardId) -> Option<ShardEpoch> {
        self.shard_epochs.get(shard_id).copied()
    }

    /// The held epochs sent on a lease renewal: exactly the set last received, in a
    /// deterministic order.
    pub fn held_epochs(&self) -> BTreeMap<ShardId, ShardEpoch> {
        self.shard_epochs
            .iter()
            .map(|(shard_id, epoch)| (*shard_id, *epoch))
            .collect()
    }

    /// The single place the never-expires rule is spelled: a `None` expiry is
    /// always live.
    pub fn lease_is_live(&self, now: Instant) -> bool {
        match self.expires_at {
            None => true,
            Some(expires_at) => now < expires_at,
        }
    }
}

/// An input to the executor's assignment transition. Maps and sets are borrowed for one
/// operation; the transition owns the current assignment and never clones it to change it.
#[derive(Clone, Copy, Debug)]
pub enum ShardAssignmentEvent<'a> {
    Register {
        number_of_shards: usize,
        shard_epochs: &'a HashMap<ShardId, ShardEpoch>,
        expires_at: Option<Instant>,
        revision: ShardLeaseRevision,
        executor_count: usize,
    },
    Assign {
        number_of_shards: usize,
        shard_epochs: &'a HashMap<ShardId, ShardEpoch>,
        revision: ShardLeaseRevision,
        executor_count: usize,
    },
    Renew {
        shard_epochs: &'a HashMap<ShardId, ShardEpoch>,
        expires_at: Instant,
        revision: ShardLeaseRevision,
        executor_count: usize,
    },
    Revoke {
        shard_ids: &'a HashSet<ShardId>,
        revision: ShardLeaseRevision,
    },
    InstallUnexpiring {
        number_of_shards: usize,
        shard_epochs: &'a HashMap<ShardId, ShardEpoch>,
    },
    Clear {
        now: Instant,
    },
}

/// Returns the assignment after one event and the delivery outcome, if the event delivered
/// anything. `None` is the outcome of a clear, or of a delivery refused before registration;
/// the latter also leaves the assignment absent. Registration and standalone installation
/// always return an assignment and an outcome.
///
/// Revisions order sets within one manager incarnation. Equal revisions apply, including the
/// full assignment sent after a revoke. A reply adopts another manager's incarnation, while a
/// push from another manager is refused. An internal registration without an expiry uses the
/// push gate. Every registration adopts its expiry even when that gate refuses the set.
///
/// A renewal adopts its exact expiry even when its set is stale: the request anchors the lease
/// independently of the set's revision. A push or revoke never changes expiry. Clear removes
/// the set and lapses the lease without discarding the last revision or executor count.
pub fn transition_shard_assignment(
    state: Option<ShardAssignment>,
    event: ShardAssignmentEvent<'_>,
) -> (Option<ShardAssignment>, Option<ShardDeliveryOutcome>) {
    use ShardAssignmentEvent as E;

    match event {
        E::InstallUnexpiring {
            number_of_shards,
            shard_epochs,
        } => {
            let set_changed = state
                .as_ref()
                .is_none_or(|current| current.shard_epochs != *shard_epochs);
            return (
                Some(ShardAssignment {
                    number_of_shards,
                    executor_count: 1,
                    shard_epochs: shard_epochs.clone(),
                    expires_at: None,
                    revision: None,
                }),
                Some(ShardDeliveryOutcome::Applied { set_changed }),
            );
        }
        E::Clear { now } => {
            return (
                state.map(|mut current| {
                    current.shard_epochs.clear();
                    current.expires_at = Some(now);
                    current
                }),
                None,
            );
        }
        _ => {}
    }

    let mut current = match state {
        Some(current) => current,
        None if matches!(event, E::Register { .. }) => ShardAssignment::default(),
        None => return (None, None),
    };
    let (number_of_shards, shard_epochs, revision, path, executor_count) = match event {
        E::Register {
            number_of_shards,
            shard_epochs,
            expires_at,
            revision,
            executor_count,
        } => {
            current.expires_at = expires_at;
            let path = if expires_at.is_some() {
                ShardDeliveryPath::Reply
            } else {
                ShardDeliveryPath::Push
            };
            (
                Some(number_of_shards),
                Some(shard_epochs),
                revision,
                path,
                Some(executor_count),
            )
        }
        E::Renew {
            shard_epochs,
            expires_at,
            revision,
            executor_count,
        } => {
            current.expires_at = Some(expires_at);
            (
                None,
                Some(shard_epochs),
                revision,
                ShardDeliveryPath::Reply,
                Some(executor_count),
            )
        }
        E::Assign {
            number_of_shards,
            shard_epochs,
            revision,
            executor_count,
        } => (
            Some(number_of_shards),
            Some(shard_epochs),
            revision,
            ShardDeliveryPath::Push,
            Some(executor_count),
        ),
        E::Revoke { revision, .. } => (None, None, revision, ShardDeliveryPath::Push, None),
        E::Clear { .. } | E::InstallUnexpiring { .. } => unreachable!(),
    };
    if let Some(applied) = current.revision {
        let refused = if revision.incarnation != applied.incarnation {
            (path == ShardDeliveryPath::Push).then_some(ShardDeliveryOutcome::FromAnotherManager {
                delivered: revision,
                applied,
            })
        } else {
            (revision.number < applied.number).then_some(ShardDeliveryOutcome::Stale {
                delivered: revision,
                applied,
            })
        };
        if let Some(outcome) = refused {
            return (Some(current), Some(outcome));
        }
    }
    let set_changed = if let Some(shard_epochs) = shard_epochs {
        let changed = current.shard_epochs != *shard_epochs;
        if changed {
            current.shard_epochs = shard_epochs.clone();
        }
        changed
    } else if let E::Revoke { shard_ids, .. } = event {
        let before = current.len();
        current
            .shard_epochs
            .retain(|shard_id, _| !shard_ids.contains(shard_id));
        current.len() != before
    } else {
        unreachable!()
    };
    if let Some(number_of_shards) = number_of_shards {
        current.number_of_shards = number_of_shards;
    }
    if let Some(executor_count) = executor_count {
        current.executor_count = executor_count;
    }
    current.revision = Some(revision);
    (
        Some(current),
        Some(ShardDeliveryOutcome::Applied { set_changed }),
    )
}

impl Display for ShardAssignment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut entries = self.shard_epochs.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(shard_id, _)| shard_id.value);
        let shard_epochs = entries
            .into_iter()
            .map(|(shard_id, epoch)| format!("{shard_id}@{epoch}"))
            .collect::<Vec<_>>()
            .join(",");
        write!(
            f,
            "{{ number_of_shards: {}, shard_epochs: {}, lease: {} }}",
            self.number_of_shards,
            shard_epochs,
            match self.expires_at {
                Some(expires_at) => match expires_at.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => format!("{}ms left", left.as_millis()),
                    _ => "lapsed".to_string(),
                },
                None => "never expires".to_string(),
            }
        )
    }
}
