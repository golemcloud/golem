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

//! The links from addresses to the hostnames that the Store's completed address lookups and
//! `golem:network/dns` A and AAAA lookups returned them for, so that a hostname grant matches a raw-socket connect to the address.
//!
//! Every completed lookup of the Store adds its links, live and on replay, in the order the
//! lookups complete; a tool entity Store has its own index. The links are derived from the
//! oplog: a full replay fills the index again, and a start from a snapshot rebuilds it from the
//! lookups that the snapshot skipped ([`ResolvedNames::rebuild`]). Only the lookups that replay
//! returns again link, so live execution, replay and the rebuild agree. The decision of a connect is
//! recorded in its `Start`, so the index never changes what a replayed connect returns.

use crate::services::oplog::{Oplog, OplogOps};
use crate::worker::status::regions::unreplayed_regions;
use golem_common::model::card::network_target::link_hostname;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    HostRequest, HostRequestNetworkDnsResolve, HostRequestP3SocketsResolveName,
    HostRequestSocketsResolveName, HostResponse, HostResponseNetworkDnsResolve,
    HostResponseP3SocketsResolveName, HostResponseSocketsResolveName, OplogEntry, OplogIndex,
    OplogPayload,
};
use golem_common::model::regions::DeletedRegions;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::IpAddr;

/// The most links one Store keeps. A lookup that adds a link past it evicts the link that was
/// added or refreshed longest ago. With hostnames of at most 253 bytes the index holds at most
/// about 270 KiB, and adding or reading links scans at most this many entries.
pub const MAX_RESOLVED_NAME_LINKS: usize = 1024;

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

    /// The links of the completed lookups of the primary Store in `INITIAL..=through`, in the
    /// order their `End` entries were recorded: the history that a start from a snapshot at
    /// `through` skips. A lookup that no replay runs links nothing: one in `deleted_regions`, in a
    /// region that a `Jump` skipped, or in a failed attempt of a snapshot-based update. The
    /// lookups of entity bodies belong to their own Stores, and the physical calls of a custom
    /// durable invocation do not replay, so neither links. The scan reads every entry of the range
    /// once and downloads only the payloads of lookups.
    pub async fn rebuild(
        oplog: &dyn Oplog,
        through: OplogIndex,
        deleted_regions: &DeletedRegions,
    ) -> Result<Self, WorkerExecutorError> {
        const CHUNK_SIZE: u64 = 1024;
        let mut scan = LookupScan::default();
        let mut next = OplogIndex::INITIAL;
        while next <= through {
            let available = u64::from(through) - u64::from(next) + 1;
            let entries = oplog.read_exact(next, CHUNK_SIZE.min(available)).await?;
            next = entries
                .last_key_value()
                .map_or(through, |(index, _)| *index)
                .next();
            for (index, entry) in entries {
                if !deleted_regions.is_in_deleted_region(index) {
                    scan.observe(index, entry);
                }
            }
        }
        let mut names = Self::default();
        for (request, response) in scan.replayed_lookups() {
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
        Ok(names)
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
            (
                HostRequest::NetworkDnsResolve(HostRequestNetworkDnsResolve { name, .. }),
                HostResponse::NetworkDnsResolve(HostResponseNetworkDnsResolve {
                    result: Ok(records),
                }),
            ) => {
                let addresses = super::dns::linked_addresses(&records).collect::<Vec<_>>();
                self.insert(&name, addresses);
                return;
            }
            _ => return,
        };
        self.insert(&name, addresses.0.into_iter().map(IpAddr::from));
    }
}

/// The completed lookups of the primary Store among the entries that [`ResolvedNames::rebuild`]
/// reads, with the entries that decide which history no replay runs.
#[derive(Default)]
struct LookupScan {
    entity_scopes: HashSet<OplogIndex>,
    started: HashMap<OplogIndex, OplogPayload<HostRequest>>,
    completed: Vec<(
        OplogIndex,
        OplogPayload<HostRequest>,
        OplogPayload<HostResponse>,
    )>,
    region_entries: BTreeMap<OplogIndex, OplogEntry>,
}

impl LookupScan {
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
                let lookup = matches!(
                    function_name,
                    HostFunctionName::SocketsIpNameLookupResolveAddresses
                        | HostFunctionName::P3SocketsIpNameLookupResolveAddresses
                        | HostFunctionName::GolemNetworkDnsResolve
                );
                if lookup
                    && !in_entity
                    && observational_owner.is_none()
                    && let Some(request) = request
                {
                    self.started.insert(index, request);
                }
            }
            OplogEntry::End {
                start_index,
                response: Some(response),
                ..
            } => {
                if let Some(request) = self.started.remove(&start_index) {
                    self.completed.push((start_index, request, response));
                }
            }
            entry @ (OplogEntry::Jump { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::PendingUpdate { .. }
            | OplogEntry::SuccessfulUpdate { .. }
            | OplogEntry::FailedUpdate { .. }) => {
                self.region_entries.insert(index, entry);
            }
            _ => {}
        }
    }

    /// The completed lookups that a replay runs, in the order of their `End` entries.
    fn replayed_lookups(
        self,
    ) -> impl Iterator<Item = (OplogPayload<HostRequest>, OplogPayload<HostResponse>)> {
        let unreplayed = unreplayed_regions(&self.region_entries);
        self.completed
            .into_iter()
            .filter(move |(start, _, _)| !unreplayed.is_in_deleted_region(*start))
            .map(|(_, request, response)| (request, response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn the_index_evicts_the_least_recently_linked_link() {
        let mut names = ResolvedNames::default();
        let address = |n: usize| IpAddr::from([10, 1, (n / 256) as u8, (n % 256) as u8]);
        names.insert("first.test", [address(0)]);
        names.insert("second.test", [address(1)]);
        for n in 2..MAX_RESOLVED_NAME_LINKS {
            names.insert("bulk.test", [address(n)]);
        }
        // A repeated lookup refreshes its link, so the next eviction takes the second one.
        names.insert("first.test", [address(0)]);
        names.insert("bulk.test", [address(MAX_RESOLVED_NAME_LINKS)]);
        assert_eq!(names.links.len(), MAX_RESOLVED_NAME_LINKS);
        assert_eq!(names.hostnames(address(0)), vec!["first.test"]);
        assert!(names.hostnames(address(1)).is_empty());
        assert_eq!(
            names.hostnames(address(MAX_RESOLVED_NAME_LINKS)),
            vec!["bulk.test"]
        );
    }

    fn lookup_start(parent: Option<u64>, observational: bool, name: &str) -> OplogEntry {
        OplogEntry::Start {
            timestamp: golem_common::model::Timestamp::now_utc(),
            parent_start_index: parent.map(OplogIndex::from_u64),
            function_name: HostFunctionName::P3SocketsIpNameLookupResolveAddresses,
            invocation_id: None,
            observational_owner: observational.then(|| OplogIndex::from_u64(1)),
            request: Some(OplogPayload::Inline(Box::new(
                HostRequest::P3SocketsResolveName(HostRequestP3SocketsResolveName {
                    name: name.to_string(),
                }),
            ))),
            durable_function_type: golem_common::model::oplog::DurableFunctionType::ReadRemote,
            span_started: None,
            denial: None,
        }
    }

    fn entity_start() -> OplogEntry {
        let mut entry = lookup_start(None, false, "");
        let OplogEntry::Start {
            function_name,
            request,
            ..
        } = &mut entry
        else {
            unreachable!()
        };
        *function_name = HostFunctionName::GolemEntityInvoke;
        *request = None;
        entry
    }

    fn lookup_end(start: u64, address: [u8; 4]) -> OplogEntry {
        OplogEntry::End {
            timestamp: golem_common::model::Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start),
            response: Some(OplogPayload::Inline(Box::new(
                HostResponse::P3SocketsResolveName(HostResponseP3SocketsResolveName {
                    result: Ok(golem_common::model::oplog::types::SerializableIpAddresses(
                        vec![IpAddr::from(address).into()],
                    )),
                }),
            ))),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        }
    }

    #[test]
    fn the_rebuild_links_only_the_lookups_that_replay_runs() {
        use golem_common::model::regions::OplogRegion;
        let entries = vec![
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
        ];
        let mut scan = LookupScan::default();
        for (offset, entry) in entries.into_iter().enumerate() {
            scan.observe(OplogIndex::from_u64(offset as u64 + 2), entry);
        }
        let mut names = ResolvedNames::default();
        for (request, response) in scan.replayed_lookups() {
            let (OplogPayload::Inline(request), OplogPayload::Inline(response)) =
                (request, response)
            else {
                unreachable!()
            };
            names.insert_recorded(*request, *response);
        }
        assert_eq!(names.hostnames(ip("10.0.0.1")), vec!["primary.test"]);
        assert_eq!(names.hostnames(ip("10.0.0.6")), vec!["later.test"]);
        for unlinked in ["10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5"] {
            assert!(names.hostnames(ip(unlinked)).is_empty(), "{unlinked}");
        }
        assert_eq!(names.links.len(), 2);
    }

    #[test]
    fn the_rebuild_links_the_address_records_of_dns_record_lookups() {
        use golem_common::model::oplog::types::{
            SerializableDnsRecord, SerializableDnsRecordData, SerializableDnsRecordType,
        };
        let record_lookup = |name: &str, records: Vec<SerializableDnsRecordData>| {
            let mut start = lookup_start(None, false, "");
            let OplogEntry::Start {
                function_name,
                request,
                ..
            } = &mut start
            else {
                unreachable!()
            };
            *function_name = HostFunctionName::GolemNetworkDnsResolve;
            *request = Some(OplogPayload::Inline(Box::new(
                HostRequest::NetworkDnsResolve(HostRequestNetworkDnsResolve {
                    name: name.to_string(),
                    record_type: SerializableDnsRecordType::A,
                }),
            )));
            let mut end = lookup_end(0, [0, 0, 0, 0]);
            let OplogEntry::End { response, .. } = &mut end else {
                unreachable!()
            };
            *response = Some(OplogPayload::Inline(Box::new(
                HostResponse::NetworkDnsResolve(HostResponseNetworkDnsResolve {
                    result: Ok(records
                        .into_iter()
                        .map(|data| SerializableDnsRecord { ttl: 60, data })
                        .collect()),
                }),
            )));
            (start, end)
        };
        let (a_start, mut a_end) = record_lookup(
            "db.test",
            vec![
                SerializableDnsRecordData::A(ip("10.0.0.1").into()),
                SerializableDnsRecordData::Aaaa(ip("fd00::1").into()),
            ],
        );
        let (mx_start, mut mx_end) = record_lookup(
            "mail.test",
            vec![SerializableDnsRecordData::Mx {
                preference: 10,
                exchange: "10.0.0.2".to_string(),
            }],
        );
        for (end, start) in [(&mut a_end, 2), (&mut mx_end, 4)] {
            let OplogEntry::End { start_index, .. } = end else {
                unreachable!()
            };
            *start_index = OplogIndex::from_u64(start);
        }
        let mut scan = LookupScan::default();
        for (offset, entry) in [a_start, a_end, mx_start, mx_end].into_iter().enumerate() {
            scan.observe(OplogIndex::from_u64(offset as u64 + 2), entry);
        }
        let mut names = ResolvedNames::default();
        for (request, response) in scan.replayed_lookups() {
            let (OplogPayload::Inline(request), OplogPayload::Inline(response)) =
                (request, response)
            else {
                unreachable!()
            };
            names.insert_recorded(*request, *response);
        }
        assert_eq!(names.hostnames(ip("10.0.0.1")), vec!["db.test"]);
        assert_eq!(names.hostnames(ip("fd00::1")), vec!["db.test"]);
        assert!(names.hostnames(ip("10.0.0.2")).is_empty());
        assert_eq!(names.links.len(), 2);
    }

    #[test]
    fn extending_keeps_the_later_links_most_recent() {
        let mut earlier = ResolvedNames::default();
        earlier.insert("a.test", [ip("10.0.0.1")]);
        let mut later = ResolvedNames::default();
        later.insert("b.test", [ip("10.0.0.1")]);
        later.insert("a.test", [ip("10.0.0.1")]);
        earlier.extend(later);
        assert_eq!(names_of(&earlier, "10.0.0.1"), vec!["a.test", "b.test"]);
    }

    fn names_of(names: &ResolvedNames, address: &str) -> Vec<String> {
        names.hostnames(ip(address))
    }
}
