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

//! The links from addresses to the hostnames that the Store's completed address lookups
//! returned them for, so that a hostname grant matches a raw-socket connect to the address.
//!
//! Every completed lookup of the Store adds its links, live and on replay: a P2 lookup when its
//! call returns, a P3 lookup when its delivery observer settles its completion as delivered, which
//! is where the `CompletionDelivered` marker is recorded or, on replay, reached. A P3 completion
//! that the guest discards, also after the host call returned it, adds no link. A tool entity Store has its
//! own index. The links are derived from the
//! oplog: a full replay fills the index again, and a start from a snapshot rebuilds it from the
//! lookups that the snapshot skipped ([`ResolvedNames::rebuild`]). Only the lookups that replay
//! returns again link, so live execution, replay and the rebuild agree. The decision of a connect is
//! recorded in its `Start`, so the index never changes what a replayed connect returns.

use crate::services::oplog::{Oplog, OplogOps};
use crate::worker::status::regions::unreplayed_regions;
use golem_common::model::card::network_target::link_hostname;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    HostRequest, HostRequestP3SocketsResolveName, HostRequestSocketsResolveName, HostResponse,
    HostResponseP3SocketsResolveName, HostResponseSocketsResolveName, OplogEntry, OplogIndex,
    OplogPayload,
};
use golem_common::model::regions::{DeletedRegions, OplogRegion};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// The most links one Store keeps. A lookup that adds a link past it evicts the link that was
/// added or refreshed longest ago. With hostnames of at most 253 bytes the index holds at most
/// about 270 KiB, and adding or reading links scans at most this many entries.
pub const MAX_RESOLVED_NAME_LINKS: usize = 1024;

/// The [`ResolvedNames`] of a Store, shared with the delivery observers that link P3 lookups
/// when the guest receives their result. An observer runs in Wasmtime's event loop without the
/// Store, so the index is behind a lock of its own; every holder keeps the lock for one index
/// operation only.
#[derive(Debug, Clone, Default)]
pub struct SharedResolvedNames(Arc<Mutex<ResolvedNames>>);

impl SharedResolvedNames {
    pub fn with<R>(&self, f: impl FnOnce(&mut ResolvedNames) -> R) -> R {
        f(&mut self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// A link that a P3 lookup sets once its result is known, and the hook that adds it to the
    /// index when the guest receives that result. A hook that never runs, because the guest
    /// discarded the result, adds nothing. Every clone of the hook adds the link at most once.
    pub fn deferred_link(&self) -> (DeferredLink, impl FnOnce() + Clone + Send + use<>) {
        let slot: LinkSlot = Arc::new(Mutex::new(None));
        let link = DeferredLink(slot.clone());
        let names = self.clone();
        let on_delivered = move || {
            let taken = slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some((name, addresses)) = taken {
                names.with(|names| names.insert(&name, addresses));
            }
        };
        (link, on_delivered)
    }
}

/// The link of one P3 lookup, set before the guest can receive its result; see
/// [`SharedResolvedNames::deferred_link`].
pub struct DeferredLink(LinkSlot);

/// The hostname and addresses of a deferred link, once its lookup has completed.
type LinkSlot = Arc<Mutex<Option<(String, Vec<IpAddr>)>>>;

impl DeferredLink {
    pub fn set(&self, name: &str, addresses: impl IntoIterator<Item = IpAddr>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some((name.to_string(), addresses.into_iter().collect()));
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedNames {
    /// The links, the least recently added first.
    links: VecDeque<(IpAddr, String)>,
}

impl ResolvedNames {
    /// Links `addresses` to the hostname of the lookup of `name`. A link that exists already
    /// becomes the most recent one.
    pub fn insert(&mut self, name: &str, addresses: impl IntoIterator<Item = IpAddr>) {
        let Some(hostname) = link_hostname(name) else {
            return;
        };
        for address in addresses {
            let link = (address.to_canonical(), hostname.clone());
            if let Some(position) = self.links.iter().position(|existing| *existing == link) {
                self.links.remove(position);
            }
            self.links.push_back(link);
            if self.links.len() > MAX_RESOLVED_NAME_LINKS {
                self.links.pop_front();
            }
        }
    }

    /// The hostnames linked to `address`, the most recently linked first.
    pub fn hostnames(&self, address: IpAddr) -> Vec<String> {
        let address = address.to_canonical();
        self.links
            .iter()
            .rev()
            .filter(|(linked, _)| *linked == address)
            .map(|(_, hostname)| hostname.clone())
            .collect()
    }

    /// Adds the links of `later` after the links of this index, in their order.
    pub fn extend(&mut self, later: ResolvedNames) {
        for (address, hostname) in later.links {
            self.insert(&hostname, [address]);
        }
    }

    /// The links of the completed lookups of the primary Store in `INITIAL..=through`: the
    /// history that a start from a snapshot at `through` skips. A lookup links where live
    /// execution and replay link it: a P2 lookup at its `End`, a P3 lookup at its
    /// `CompletionDelivered` marker, and a P3 lookup whose completion the guest discarded or did
    /// not receive yet not at all. A lookup that no replay runs links nothing: one in
    /// `deleted_regions`, in a region that a `Jump` skipped, or in a failed attempt of a
    /// snapshot-based update. The lookups of entity bodies belong to their own Stores, and the
    /// physical calls of a custom durable invocation do not replay, so neither links.
    ///
    /// The range is read twice in chunks: once for the entries that decide which history no
    /// replay runs, then for the lookups, which go into the bounded index as they link. So the
    /// scan keeps only the lookups and entity scopes that are open, and downloads only the
    /// payloads of lookups.
    pub async fn rebuild(
        oplog: &dyn Oplog,
        through: OplogIndex,
        deleted_regions: &DeletedRegions,
    ) -> Result<Self, WorkerExecutorError> {
        let mut region_entries = BTreeMap::new();
        let mut next = OplogIndex::INITIAL;
        while next <= through {
            let entries = read_chunk(oplog, next, through).await?;
            next = chunk_end(&entries, through).next();
            region_entries.extend(entries.into_iter().filter(|(index, entry)| {
                !deleted_regions.is_in_deleted_region(*index) && is_region_entry(entry)
            }));
        }
        let mut scan = LookupScan::new(unreplayed_regions(&region_entries));
        drop(region_entries);

        let mut names = Self::default();
        let mut next = OplogIndex::INITIAL;
        while next <= through {
            let entries = read_chunk(oplog, next, through).await?;
            next = chunk_end(&entries, through).next();
            for (index, entry) in entries {
                if !deleted_regions.is_in_deleted_region(index) {
                    scan.observe(index, entry);
                }
            }
            for (request, response) in scan.take_links() {
                let request = oplog
                    .download_payload(request)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
                let response = oplog
                    .download_payload(response)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
                names.insert_recorded(request, response);
            }
        }
        Ok(names)
    }

    /// The index of a Store after a start from a snapshot at `through`: the `rebuilt` links of
    /// the skipped history, and the links that the core initializer made before the rebuild. An
    /// initializer whose bootstrap range ends at or before `through` replayed lookups of the
    /// skipped history, which `rebuilt` already holds at their own position. Only an
    /// initializer after `through`, such as that of the target revision of a snapshot-based
    /// update, adds new links, and they are the most recent ones.
    pub fn after_rebuild(
        mut rebuilt: ResolvedNames,
        initializer: ResolvedNames,
        initializer_range: Option<&OplogRegion>,
        through: OplogIndex,
    ) -> Self {
        if !initializer_range.is_some_and(|range| range.end <= through) {
            rebuilt.extend(initializer);
        }
        rebuilt
    }

    fn insert_recorded(&mut self, request: HostRequest, response: HostResponse) {
        let (name, addresses) = match (request, response) {
            (
                HostRequest::SocketsResolveName(HostRequestSocketsResolveName { name }),
                HostResponse::SocketsResolveName(HostResponseSocketsResolveName {
                    result: Ok(addresses),
                }),
            )
            | (
                HostRequest::P3SocketsResolveName(HostRequestP3SocketsResolveName { name }),
                HostResponse::P3SocketsResolveName(HostResponseP3SocketsResolveName {
                    result: Ok(addresses),
                }),
            ) => (name, addresses),
            _ => return,
        };
        self.insert(&name, addresses.0.into_iter().map(IpAddr::from));
    }
}

async fn read_chunk(
    oplog: &dyn Oplog,
    next: OplogIndex,
    through: OplogIndex,
) -> Result<BTreeMap<OplogIndex, OplogEntry>, WorkerExecutorError> {
    const CHUNK_SIZE: u64 = 1024;
    let available = u64::from(through) - u64::from(next) + 1;
    Ok(oplog.read_exact(next, CHUNK_SIZE.min(available)).await?)
}

fn chunk_end(entries: &BTreeMap<OplogIndex, OplogEntry>, through: OplogIndex) -> OplogIndex {
    entries
        .last_key_value()
        .map_or(through, |(index, _)| *index)
}

/// The entries that decide which history no replay runs.
fn is_region_entry(entry: &OplogEntry) -> bool {
    matches!(
        entry,
        OplogEntry::Jump { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::PendingUpdate { .. }
            | OplogEntry::SuccessfulUpdate { .. }
            | OplogEntry::FailedUpdate { .. }
    )
}

type RecordedLookup = (OplogPayload<HostRequest>, OplogPayload<HostResponse>);

/// The lookups of the primary Store among the entries that [`ResolvedNames::rebuild`] reads, in
/// the order they link. It keeps only what is open: lookups without a terminal, P3 lookups
/// without a delivery marker, entity scopes without a terminal, and the links of the current
/// chunk.
struct LookupScan {
    unreplayed: DeletedRegions,
    entity_scopes: HashSet<OplogIndex>,
    started: HashMap<OplogIndex, (bool, OplogPayload<HostRequest>)>,
    undelivered: HashMap<OplogIndex, RecordedLookup>,
    links: Vec<RecordedLookup>,
}

impl LookupScan {
    fn new(unreplayed: DeletedRegions) -> Self {
        Self {
            unreplayed,
            entity_scopes: HashSet::new(),
            started: HashMap::new(),
            undelivered: HashMap::new(),
            links: Vec::new(),
        }
    }

    fn observe(&mut self, index: OplogIndex, entry: OplogEntry) {
        match entry {
            OplogEntry::Start {
                parent_start_index,
                function_name,
                observational_owner,
                request,
                ..
            } => {
                let in_entity =
                    parent_start_index.is_some_and(|parent| self.entity_scopes.contains(&parent));
                if in_entity || function_name == HostFunctionName::GolemEntityInvoke {
                    self.entity_scopes.insert(index);
                }
                let p3 = match function_name {
                    HostFunctionName::SocketsIpNameLookupResolveAddresses => false,
                    HostFunctionName::P3SocketsIpNameLookupResolveAddresses => true,
                    _ => return,
                };
                if !in_entity
                    && observational_owner.is_none()
                    && !self.unreplayed.is_in_deleted_region(index)
                    && let Some(request) = request
                {
                    self.started.insert(index, (p3, request));
                }
            }
            OplogEntry::End {
                start_index,
                response,
                ..
            } => {
                self.entity_scopes.remove(&start_index);
                if let Some((p3, request)) = self.started.remove(&start_index)
                    && let Some(response) = response
                {
                    if p3 {
                        self.undelivered.insert(start_index, (request, response));
                    } else {
                        self.links.push((request, response));
                    }
                }
            }
            OplogEntry::Cancelled { start_index, .. } => {
                self.entity_scopes.remove(&start_index);
                self.started.remove(&start_index);
            }
            OplogEntry::CompletionDelivered { start_index, .. } => {
                if let Some(lookup) = self.undelivered.remove(&start_index) {
                    self.links.push(lookup);
                }
            }
            OplogEntry::CompletionDiscarded { start_index, .. } => {
                self.undelivered.remove(&start_index);
            }
            _ => {}
        }
    }

    /// The lookups that linked since the last call, in the order they linked.
    fn take_links(&mut self) -> Vec<RecordedLookup> {
        std::mem::take(&mut self.links)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::types::SerializableIpAddresses;
    use test_r::test;

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn lookups_link_addresses_to_normalized_hostnames() {
        let mut names = ResolvedNames::default();
        names.insert("DB.Test.", [ip("10.0.0.5"), ip("::ffff:10.0.0.6")]);
        names.insert("cache.test", [ip("10.0.0.5")]);
        names.insert("10.0.0.7", [ip("10.0.0.7")]);
        assert_eq!(
            names.hostnames(ip("10.0.0.5")),
            vec!["cache.test", "db.test"]
        );
        assert_eq!(names.hostnames(ip("10.0.0.6")), vec!["db.test"]);
        assert_eq!(
            names.hostnames(ip("::ffff:10.0.0.5")),
            vec!["cache.test", "db.test"]
        );
        assert!(names.hostnames(ip("10.0.0.7")).is_empty());
        assert!(names.hostnames(ip("10.0.0.8")).is_empty());
    }

    fn bulk_address(n: usize) -> IpAddr {
        IpAddr::from([10, 1, (n / 256) as u8, (n % 256) as u8])
    }

    #[test]
    fn the_index_evicts_the_least_recently_linked_link() {
        let mut names = ResolvedNames::default();
        names.insert("first.test", [bulk_address(0)]);
        names.insert("second.test", [bulk_address(1)]);
        for n in 2..MAX_RESOLVED_NAME_LINKS {
            names.insert("bulk.test", [bulk_address(n)]);
        }
        // A repeated lookup refreshes its link, so the next eviction takes the second one.
        names.insert("first.test", [bulk_address(0)]);
        names.insert("bulk.test", [bulk_address(MAX_RESOLVED_NAME_LINKS)]);
        assert_eq!(names.links.len(), MAX_RESOLVED_NAME_LINKS);
        assert_eq!(names.hostnames(bulk_address(0)), vec!["first.test"]);
        assert!(names.hostnames(bulk_address(1)).is_empty());
        assert_eq!(
            names.hostnames(bulk_address(MAX_RESOLVED_NAME_LINKS)),
            vec!["bulk.test"]
        );
    }

    fn start_entry(
        function_name: HostFunctionName,
        parent: Option<u64>,
        observational: bool,
        request: Option<HostRequest>,
    ) -> OplogEntry {
        OplogEntry::Start {
            timestamp: golem_common::model::Timestamp::now_utc(),
            parent_start_index: parent.map(OplogIndex::from_u64),
            function_name,
            invocation_id: None,
            observational_owner: observational.then(|| OplogIndex::from_u64(1)),
            request: request.map(|request| OplogPayload::Inline(Box::new(request))),
            durable_function_type: golem_common::model::oplog::DurableFunctionType::ReadRemote,
            span_started: None,
            denial: None,
        }
    }

    fn end_entry(start: u64, response: Option<HostResponse>) -> OplogEntry {
        OplogEntry::End {
            timestamp: golem_common::model::Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start),
            response: response.map(|response| OplogPayload::Inline(Box::new(response))),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        }
    }

    fn addresses(address: [u8; 4]) -> SerializableIpAddresses {
        SerializableIpAddresses(vec![IpAddr::from(address).into()])
    }

    /// A P2 lookup, which records no delivery marker.
    fn lookup_start(parent: Option<u64>, observational: bool, name: &str) -> OplogEntry {
        start_entry(
            HostFunctionName::SocketsIpNameLookupResolveAddresses,
            parent,
            observational,
            Some(HostRequest::SocketsResolveName(
                HostRequestSocketsResolveName {
                    name: name.to_string(),
                },
            )),
        )
    }

    fn lookup_end(start: u64, address: [u8; 4]) -> OplogEntry {
        end_entry(
            start,
            Some(HostResponse::SocketsResolveName(
                HostResponseSocketsResolveName {
                    result: Ok(addresses(address)),
                },
            )),
        )
    }

    fn p3_lookup_start(name: &str) -> OplogEntry {
        start_entry(
            HostFunctionName::P3SocketsIpNameLookupResolveAddresses,
            None,
            false,
            Some(HostRequest::P3SocketsResolveName(
                HostRequestP3SocketsResolveName {
                    name: name.to_string(),
                },
            )),
        )
    }

    fn p3_lookup_end(start: u64, address: [u8; 4]) -> OplogEntry {
        end_entry(
            start,
            Some(HostResponse::P3SocketsResolveName(
                HostResponseP3SocketsResolveName {
                    result: Ok(addresses(address)),
                },
            )),
        )
    }

    fn delivered(start: u64) -> OplogEntry {
        OplogEntry::CompletionDelivered {
            timestamp: golem_common::model::Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start),
        }
    }

    fn discarded(start: u64) -> OplogEntry {
        OplogEntry::CompletionDiscarded {
            timestamp: golem_common::model::Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start),
        }
    }

    fn entity_start() -> OplogEntry {
        start_entry(HostFunctionName::GolemEntityInvoke, None, false, None)
    }

    fn link(names: &mut ResolvedNames, scan: &mut LookupScan) {
        for (request, response) in scan.take_links() {
            let (OplogPayload::Inline(request), OplogPayload::Inline(response)) =
                (request, response)
            else {
                unreachable!()
            };
            names.insert_recorded(*request, *response);
        }
    }

    /// The two passes of [`ResolvedNames::rebuild`] over `entries`, which start at index 2.
    fn rebuild_from(entries: Vec<OplogEntry>) -> ResolvedNames {
        let entries = entries
            .into_iter()
            .enumerate()
            .map(|(offset, entry)| (OplogIndex::from_u64(offset as u64 + 2), entry))
            .collect::<Vec<_>>();
        let region_entries = entries
            .iter()
            .filter(|(_, entry)| is_region_entry(entry))
            .cloned()
            .collect();
        let mut scan = LookupScan::new(unreplayed_regions(&region_entries));
        let mut names = ResolvedNames::default();
        for (index, entry) in entries {
            scan.observe(index, entry);
            link(&mut names, &mut scan);
        }
        names
    }

    #[test]
    fn the_rebuild_links_only_the_lookups_that_replay_runs() {
        use golem_common::model::regions::OplogRegion;
        let names = rebuild_from(vec![
            // 2..=3: a primary lookup.
            lookup_start(None, false, "primary.test"),
            lookup_end(2, [10, 0, 0, 1]),
            // 4..=5: a physical lookup of a custom durable invocation.
            lookup_start(None, true, "observed.test"),
            lookup_end(4, [10, 0, 0, 2]),
            // 6..=8: an entity body and a lookup nested in it.
            entity_start(),
            lookup_start(Some(6), false, "entity.test"),
            lookup_end(7, [10, 0, 0, 3]),
            // 9..=10: a lookup that the jump at 11 skips.
            lookup_start(None, false, "jumped.test"),
            lookup_end(9, [10, 0, 0, 4]),
            OplogEntry::Jump {
                timestamp: golem_common::model::Timestamp::now_utc(),
                entity_parent_start_index: None,
                jump: OplogRegion::from_range(9..=10),
            },
            // 12: a lookup without an `End`.
            lookup_start(None, false, "incomplete.test"),
            // 13..=14: a lookup after the jump.
            lookup_start(None, false, "later.test"),
            lookup_end(13, [10, 0, 0, 6]),
        ]);
        assert_eq!(names.hostnames(ip("10.0.0.1")), vec!["primary.test"]);
        assert_eq!(names.hostnames(ip("10.0.0.6")), vec!["later.test"]);
        for unlinked in ["10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5"] {
            assert!(names.hostnames(ip(unlinked)).is_empty(), "{unlinked}");
        }
        assert_eq!(names.links.len(), 2);
    }

    #[test]
    fn p3_lookups_link_at_their_delivery_marker() {
        let names = rebuild_from(vec![
            // 2..=7: two lookups of one address whose completions reach the guest in the
            // reverse order of their `End` entries.
            p3_lookup_start("a.test"),
            p3_lookup_start("b.test"),
            p3_lookup_end(2, [10, 0, 0, 5]),
            p3_lookup_end(3, [10, 0, 0, 5]),
            delivered(3),
            delivered(2),
            // 8..=10: a lookup whose completion the guest discarded.
            p3_lookup_start("discarded.test"),
            p3_lookup_end(8, [10, 0, 0, 6]),
            discarded(8),
            // 11..=12: a lookup whose completion the guest has not received.
            p3_lookup_start("undelivered.test"),
            p3_lookup_end(11, [10, 0, 0, 7]),
        ]);
        // The lookup delivered last is the most recent link, as in the running Store.
        assert_eq!(names.hostnames(ip("10.0.0.5")), vec!["a.test", "b.test"]);
        assert!(names.hostnames(ip("10.0.0.6")).is_empty());
        assert!(names.hostnames(ip("10.0.0.7")).is_empty());
    }

    #[test]
    fn the_rebuild_keeps_only_open_lookups_and_scopes() {
        let mut scan = LookupScan::new(DeletedRegions::new());
        let mut names = ResolvedNames::default();
        let mut index = 2u64;
        let mut observe = |scan: &mut LookupScan, entry: OplogEntry| {
            scan.observe(OplogIndex::from_u64(index), entry);
            index += 1;
            index - 1
        };
        for _ in 0..10_000 {
            let entity = observe(&mut scan, entity_start());
            let nested = observe(&mut scan, lookup_start(Some(entity), false, "entity.test"));
            observe(&mut scan, lookup_end(nested, [10, 0, 0, 9]));
            observe(&mut scan, end_entry(entity, None));
            let p2 = observe(&mut scan, lookup_start(None, false, "same.test"));
            observe(&mut scan, lookup_end(p2, [10, 0, 0, 5]));
            let p3 = observe(&mut scan, p3_lookup_start("same.test"));
            observe(&mut scan, p3_lookup_end(p3, [10, 0, 0, 5]));
            observe(&mut scan, delivered(p3));
            link(&mut names, &mut scan);
            assert!(scan.entity_scopes.is_empty());
            assert!(scan.started.is_empty());
            assert!(scan.undelivered.is_empty());
        }
        assert_eq!(names.links.len(), 1);
        assert_eq!(names.hostnames(ip("10.0.0.5")), vec!["same.test"]);
    }

    #[test]
    fn a_replayed_initializer_keeps_the_position_of_its_links() {
        let mut rebuilt = ResolvedNames::default();
        rebuilt.insert("initializer.test", [bulk_address(0)]);
        for n in 1..=MAX_RESOLVED_NAME_LINKS {
            rebuilt.insert("method.test", [bulk_address(n)]);
        }
        let mut initializer = ResolvedNames::default();
        initializer.insert("initializer.test", [bulk_address(0)]);
        let range = OplogRegion::from_range(2..=3);

        let restored = ResolvedNames::after_rebuild(
            rebuilt.clone(),
            initializer.clone(),
            Some(&range),
            OplogIndex::from_u64(100),
        );
        assert!(restored.hostnames(bulk_address(0)).is_empty());
        assert_eq!(restored.hostnames(bulk_address(1)), vec!["method.test"]);

        // The initializer of an update target runs after the snapshot, so its links are new.
        let updated = ResolvedNames::after_rebuild(
            rebuilt,
            initializer,
            Some(&range),
            OplogIndex::from_u64(1),
        );
        assert_eq!(updated.hostnames(bulk_address(0)), vec!["initializer.test"]);
        assert!(updated.hostnames(bulk_address(1)).is_empty());
    }

    #[test]
    fn a_deferred_link_is_added_only_when_its_hook_runs() {
        let shared = SharedResolvedNames::default();
        // The guest discards the returned result: the observer drops the hook unrun.
        let (discarded, hook) = shared.deferred_link();
        discarded.set("discarded.test", [ip("10.0.0.5")]);
        drop(hook);
        // The guest receives the result: the hook runs, and a clone of it adds nothing more.
        let (delivered, hook) = shared.deferred_link();
        delivered.set("delivered.test", [ip("10.0.0.6")]);
        let replay_clone = hook.clone();
        hook();
        shared.with(|names| names.insert("later.test", [ip("10.0.0.6")]));
        replay_clone();
        shared.with(|names| {
            assert!(names.hostnames(ip("10.0.0.5")).is_empty());
            assert_eq!(
                names.hostnames(ip("10.0.0.6")),
                vec!["later.test", "delivered.test"]
            );
        });
    }

    #[test]
    fn extending_keeps_the_later_links_most_recent() {
        let mut earlier = ResolvedNames::default();
        earlier.insert("a.test", [ip("10.0.0.1")]);
        let mut later = ResolvedNames::default();
        later.insert("b.test", [ip("10.0.0.1")]);
        later.insert("a.test", [ip("10.0.0.1")]);
        earlier.extend(later);
        assert_eq!(earlier.hostnames(ip("10.0.0.1")), vec!["a.test", "b.test"]);
    }
}
