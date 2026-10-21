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
/// manager process means - see [`ShardAssignment::apply`].
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

    /// Full replace from a push: hold exactly these shards, drop everything
    /// else. One of the three doors a delivery comes through, with
    /// [`Self::adopt_grant`] and [`Self::revoke_shards`]; all three gate the
    /// set on the revision the same way in [`Self::apply`]. The lease is
    /// untouched: a push has no request of this executor's to anchor a lease
    /// to, so it carries none.
    pub fn set_shards(
        &mut self,
        number_of_shards: usize,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        self.apply(
            Some(number_of_shards),
            shard_epochs,
            revision,
            ShardDeliveryPath::Push,
        )
    }

    /// A grant: the answer to this executor's own registration
    /// (`number_of_shards` is `Some`) or renewal, carrying the shard manager's
    /// set for it and a lease anchored where the request was sent.
    ///
    /// The lease clock always moves, whatever becomes of the set. A grant
    /// answers a request this executor made, anchored before the request went
    /// out, so its expiry is never later than the manager's; and since a push
    /// carries no lease, refusing a grant's would cost this executor a renewal
    /// every time a rebalance push crossed a renewal reply - which is how an
    /// executor the manager keeps renewing would end up fencing itself. The
    /// set goes through [`Self::apply`]'s revision gate like every other
    /// delivery: the revision orders sets and the request time orders leases,
    /// and the two are independent. Normally the set is exactly what was
    /// held, because a renewal never advances an epoch; when it is not, the
    /// manager is correcting a push this executor never received, and the
    /// caller sweeps and recovers agents exactly as it would for a push.
    ///
    /// The expiry is written as is, not maxed against the current one: each
    /// grant is the manager's current word, and the manager's own deadline for
    /// this executor never moves earlier, so a later grant is never the shorter
    /// one. Grants arrive in the order they were asked for (the renewal loop
    /// awaits each one), so a later grant is always anchored later.
    pub fn adopt_grant(
        &mut self,
        number_of_shards: Option<usize>,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        expires_at: Instant,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        self.expires_at = Some(expires_at);
        self.apply(
            number_of_shards,
            shard_epochs,
            revision,
            ShardDeliveryPath::Reply,
        )
    }

    /// The one place any delivery's set is applied, including
    /// [`Self::revoke_shards`], which turns its delta into the set it leaves
    /// behind and comes through here.
    ///
    /// Two deliveries can cross on the network - a renewal reply computed
    /// before a push, arriving after it - and both say "hold exactly this", so
    /// without an order the older set would win. The revision is that order: a
    /// set older than the last one applied is ignored. Equal revisions come
    /// from the same persisted state and carry the same set, so they apply
    /// harmlessly. The lease is not this function's business: only
    /// [`Self::adopt_grant`] moves it, and it does so before coming here.
    ///
    /// That order holds within one manager process. A delivery from another
    /// one is told apart by how it came. A reply answers a request this
    /// executor has just made, so its sender is the manager in charge: the
    /// executor follows it from here on and its numbers start over, which is
    /// what lets a manager that lost its history deliver a set at all. A push
    /// proves nothing of the kind - a deposed manager can still be sending -
    /// so it is ignored, and the caller renews to hear from the one in charge.
    /// Starting the numbers over cannot let an old manager's delayed push
    /// back in, because that push no longer names the process followed. The
    /// first delivery applied names the process followed, whichever way it
    /// came; on an executor that is a registration's reply, because the shard
    /// service refuses pushes until a registration has installed an
    /// assignment.
    fn apply(
        &mut self,
        number_of_shards: Option<usize>,
        shard_epochs: &HashMap<ShardId, ShardEpoch>,
        revision: ShardLeaseRevision,
        path: ShardDeliveryPath,
    ) -> ShardDeliveryOutcome {
        if let Some(applied) = self.revision {
            if revision.incarnation != applied.incarnation {
                match path {
                    // The process in charge answered: its numbers start over.
                    ShardDeliveryPath::Reply => {}
                    ShardDeliveryPath::Push => {
                        return ShardDeliveryOutcome::FromAnotherManager {
                            delivered: revision,
                            applied,
                        };
                    }
                }
            } else if revision.number < applied.number {
                return ShardDeliveryOutcome::Stale {
                    delivered: revision,
                    applied,
                };
            }
        }
        let set_changed = self.shard_epochs != *shard_epochs;
        if let Some(number_of_shards) = number_of_shards {
            self.number_of_shards = number_of_shards;
        }
        self.shard_epochs = shard_epochs.clone();
        self.revision = Some(revision);
        ShardDeliveryOutcome::Applied { set_changed }
    }

    /// A revoke: `shard_ids` are dropped and everything else is kept.
    ///
    /// The one delta among the deliveries, but applied as the set it leaves behind, so it goes
    /// through the same gate as the rest: a grant read before the shards moved and arriving after
    /// this is older, and cannot put them back. Like any push it carries no lease and touches none.
    ///
    /// Adopting the revoke's revision is safe even though this is a delta, because the shard
    /// manager sends the full set at that same revision in the same pass: nothing older than it is
    /// still needed.
    pub fn revoke_shards(
        &mut self,
        shard_ids: &HashSet<ShardId>,
        revision: ShardLeaseRevision,
    ) -> ShardDeliveryOutcome {
        let mut remaining = self.shard_epochs.clone();
        remaining.retain(|shard_id, _| !shard_ids.contains(shard_id));
        self.apply(None, &remaining, revision, ShardDeliveryPath::Push)
    }

    /// Drops every shard, keeping `number_of_shards`, and leaves the lease
    /// **lapsed** as of `now`. Used when the shard manager no longer knows this
    /// executor's lease.
    ///
    /// The expiry is not reset to `None`, because `None` means
    /// "never expires". A cleared assignment must read as not ready, so
    /// admission keeps refusing until a re-registration installs a fresh grant.
    pub fn clear(&mut self, now: Instant) {
        self.shard_epochs.clear();
        self.expires_at = Some(now);
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
