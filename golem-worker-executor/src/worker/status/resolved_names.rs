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

//! The fold of the links from addresses to the hostnames that the address lookups and the
//! `golem:network/dns` A and AAAA lookups of the primary Store returned them for.

use crate::durable_host::sockets::dns::linked_addresses;
use golem_common::base_model::OplogIndex;
use golem_common::model::agent::{OpenLookup, ResolvedNameIndex};
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::types::SerializableDnsRecordType;
use golem_common::model::oplog::{
    HostRequest, HostRequestNetworkDnsResolve, HostRequestP3SocketsResolveName,
    HostRequestSocketsResolveName, HostResponse, HostResponseNetworkDnsResolve,
    HostResponseP3SocketsResolveName, HostResponseSocketsResolveName, OplogEntry, OplogPayload,
};
use golem_common::model::regions::DeletedRegions;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;

/// Whether `function_name` is a lookup whose result links, and whether it links at its
/// `CompletionDelivered` marker (`Some(true)`) or at its `End` (`Some(false)`).
fn links_at_delivery(function_name: &HostFunctionName) -> Option<bool> {
    match function_name {
        HostFunctionName::SocketsIpNameLookupResolveAddresses => Some(false),
        HostFunctionName::P3SocketsIpNameLookupResolveAddresses
        | HostFunctionName::GolemNetworkDnsResolve => Some(true),
        _ => None,
    }
}

/// Whether the fold reads the `Start` at `index`: a lookup outside the deleted regions. The
/// fold also reads the `End` of every lookup that it opened (see [`opens_lookup`]).
pub(super) fn reads_start(index: OplogIndex, entry: &OplogEntry, deleted: &DeletedRegions) -> bool {
    matches!(entry, OplogEntry::Start { function_name, .. } if links_at_delivery(function_name).is_some())
        && !deleted.is_in_deleted_region(index)
}

/// `resolved_names` after `entries`. A lookup links where live execution links it: a P2 lookup at
/// its `End`, a P3 or `golem:network/dns` lookup at its `CompletionDelivered` marker, and such a
/// lookup whose completion the guest discarded or did not receive yet not at all. A lookup that
/// no replay runs links nothing: one in `deleted`, or one whose `Start` is in `unreplayed`. The
/// lookups of entity bodies belong to their own Stores, and the physical calls of a custom
/// durable invocation do not replay, so neither links.
///
/// The payloads of the lookups must be loaded: the fold fails on a payload that is only a
/// reference to the payload store.
pub(super) fn calculate_resolved_names(
    mut names: ResolvedNameIndex,
    unreplayed: &DeletedRegions,
    deleted: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<ResolvedNameIndex, String> {
    for (&index, entry) in entries {
        if deleted.is_in_deleted_region(index) {
            continue;
        }
        match entry {
            OplogEntry::Start {
                parent_start_index,
                function_name,
                observational_owner,
                request,
                ..
            } => {
                let in_entity = parent_start_index
                    .is_some_and(|parent| names.following.entity_scopes.contains_key(&parent));
                if in_entity || *function_name == HostFunctionName::GolemEntityInvoke {
                    names.following.entity_scopes.insert(index, ());
                }
                let Some(at_delivery) = links_at_delivery(function_name) else {
                    continue;
                };
                if in_entity
                    || observational_owner.is_some()
                    || unreplayed.is_in_deleted_region(index)
                {
                    continue;
                }
                let Some(request) = request else {
                    continue;
                };
                let Some(name) = linking_name(loaded(request, index)?.as_ref()) else {
                    continue;
                };
                names
                    .following
                    .open
                    .insert(index, OpenLookup::Started { name, at_delivery });
            }
            OplogEntry::End {
                start_index,
                response,
                ..
            } => {
                names.following.entity_scopes.remove(start_index);
                let Some(OpenLookup::Started { name, at_delivery }) =
                    names.following.open.remove(start_index)
                else {
                    continue;
                };
                let Some(response) = response else {
                    continue;
                };
                let Some(addresses) = returned_addresses(loaded(response, index)?.as_ref()) else {
                    continue;
                };
                if at_delivery {
                    names.following.open.insert(
                        *start_index,
                        OpenLookup::Undelivered {
                            name,
                            addresses: addresses.into_iter().map(Into::into).collect(),
                        },
                    );
                } else {
                    names.links.insert(&name, addresses);
                }
            }
            OplogEntry::Cancelled { start_index, .. } => {
                names.following.entity_scopes.remove(start_index);
                names.following.open.remove(start_index);
            }
            OplogEntry::CompletionDelivered { start_index, .. } => {
                if let Some(OpenLookup::Undelivered { name, addresses }) =
                    names.following.open.remove(start_index)
                {
                    names
                        .links
                        .insert(&name, addresses.into_iter().map(IpAddr::from));
                }
            }
            OplogEntry::CompletionDiscarded { start_index, .. } => {
                names.following.open.remove(start_index);
            }
            _ => {}
        }
    }
    Ok(names)
}

/// The name of a lookup request whose result can link: an address lookup, or a
/// `golem:network/dns` query of A or AAAA records. An answer holds only records of the queried
/// type, so a query of another type links nothing, and the fold neither opens it nor reads its
/// response.
fn linking_name(request: &HostRequest) -> Option<String> {
    match request {
        HostRequest::SocketsResolveName(HostRequestSocketsResolveName { name })
        | HostRequest::P3SocketsResolveName(HostRequestP3SocketsResolveName { name })
        | HostRequest::NetworkDnsResolve(HostRequestNetworkDnsResolve {
            name,
            record_type: SerializableDnsRecordType::A | SerializableDnsRecordType::Aaaa,
        }) => Some(name.clone()),
        _ => None,
    }
}

/// Whether the fold opens the lookup whose loaded request is `request`, and so reads the
/// response of its `End`.
pub(super) fn opens_lookup(
    index: OplogIndex,
    request: &OplogPayload<HostRequest>,
) -> Result<bool, String> {
    Ok(linking_name(loaded(request, index)?.as_ref()).is_some())
}

/// The addresses that a successful lookup returned.
fn returned_addresses(response: &HostResponse) -> Option<Vec<IpAddr>> {
    match response {
        HostResponse::SocketsResolveName(HostResponseSocketsResolveName {
            result: Ok(addresses),
        })
        | HostResponse::P3SocketsResolveName(HostResponseP3SocketsResolveName {
            result: Ok(addresses),
        }) => Some(addresses.0.iter().cloned().map(IpAddr::from).collect()),
        HostResponse::NetworkDnsResolve(HostResponseNetworkDnsResolve {
            result: Ok(records),
        }) => Some(linked_addresses(records).collect()),
        _ => None,
    }
}

fn loaded<T>(payload: &OplogPayload<T>, index: OplogIndex) -> Result<Arc<T>, String>
where
    T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq,
{
    match payload {
        OplogPayload::Inline(value) => Ok(Arc::new(value.as_ref().clone())),
        OplogPayload::SerializedInline {
            cached: Some(value),
            ..
        }
        | OplogPayload::External {
            cached: Some(value),
            ..
        } => Ok(value.clone()),
        OplogPayload::SerializedInline { bytes, .. } => golem_common::serialization::decode(bytes)
            .map(Arc::new)
            .map_err(|error| {
                format!("the payload of the address lookup at oplog index {index} does not decode: {error}")
            }),
        OplogPayload::External { .. } => Err(format!(
            "the payload of the address lookup at oplog index {index} was not loaded before the status fold"
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::worker::status::update_status_with_new_entries;
    use golem_common::model::agent::AgentMode;
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::types::{
        SerializableDnsRecord, SerializableDnsRecordData, SerializableDnsRecordType,
        SerializableIpAddresses,
    };
    use golem_common::model::oplog::{
        DurableFunctionType, HostRequest, HostRequestNetworkDnsResolve,
        HostRequestP3SocketsResolveName, HostRequestSocketsResolveName, HostResponse,
        HostResponseNetworkDnsResolve, HostResponseP3SocketsResolveName,
        HostResponseSocketsResolveName, OplogEntry, OplogIndex, OplogPayload, UpdateDescription,
    };
    use golem_common::model::regions::OplogRegion;
    use golem_common::model::{AgentStatusRecord, RetryConfig, Timestamp};
    use std::collections::BTreeMap;
    use std::net::IpAddr;
    use test_r::test;

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    fn start(
        function_name: HostFunctionName,
        parent: Option<u64>,
        observational: bool,
        request: Option<HostRequest>,
    ) -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: parent.map(OplogIndex::from_u64),
            function_name,
            invocation_id: None,
            observational_owner: observational.then(|| OplogIndex::from_u64(1)),
            driver: None,
            request: request.map(|request| OplogPayload::Inline(Box::new(request))),
            durable_function_type: DurableFunctionType::ReadRemote,
            span_started: None,
            denial: None,
        }
    }

    fn end(start: u64, response: Option<HostResponse>) -> OplogEntry {
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start),
            response: response.map(|response| OplogPayload::Inline(Box::new(response))),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        }
    }

    fn addresses(address: &str) -> SerializableIpAddresses {
        SerializableIpAddresses(vec![ip(address).into()])
    }

    /// A P2 lookup, which links at its `End`.
    fn p2_start(parent: Option<u64>, observational: bool, name: &str) -> OplogEntry {
        start(
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

    fn p2_end(start: u64, address: &str) -> OplogEntry {
        end(
            start,
            Some(HostResponse::SocketsResolveName(
                HostResponseSocketsResolveName {
                    result: Ok(addresses(address)),
                },
            )),
        )
    }

    fn p3_start(name: &str) -> OplogEntry {
        start(
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

    fn p3_end(start: u64, address: &str) -> OplogEntry {
        end(
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
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start),
        }
    }

    fn discarded(start: u64) -> OplogEntry {
        OplogEntry::CompletionDiscarded {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start),
        }
    }

    fn numbered(first: u64, entries: Vec<OplogEntry>) -> BTreeMap<OplogIndex, OplogEntry> {
        entries
            .into_iter()
            .enumerate()
            .map(|(offset, entry)| (OplogIndex::from_u64(first + offset as u64), entry))
            .collect()
    }

    fn fold(
        status: AgentStatusRecord,
        entries: BTreeMap<OplogIndex, OplogEntry>,
    ) -> Option<AgentStatusRecord> {
        update_status_with_new_entries(AgentMode::Durable, status, entries, &RetryConfig::default())
            .unwrap()
    }

    /// The status after `entries`, which start at index 2, folded from an empty status.
    fn folded(entries: Vec<OplogEntry>) -> AgentStatusRecord {
        fold(AgentStatusRecord::default(), numbered(2, entries)).unwrap()
    }

    #[test]
    fn the_fold_links_only_the_lookups_that_replay_runs() {
        let status = folded(vec![
            // 2..=3: a primary lookup.
            p2_start(None, false, "primary.test"),
            p2_end(2, "10.0.0.1"),
            // 4..=5: a physical lookup of a custom durable invocation.
            p2_start(None, true, "observed.test"),
            p2_end(4, "10.0.0.2"),
            // 6..=8: an entity body and a lookup nested in it.
            start(HostFunctionName::GolemEntityInvoke, None, false, None),
            p2_start(Some(6), false, "entity.test"),
            p2_end(7, "10.0.0.3"),
            // 9..=10: a lookup that the jump at 11 skips.
            p2_start(None, false, "jumped.test"),
            p2_end(9, "10.0.0.4"),
            OplogEntry::jump(None, OplogRegion::from_range(9..=10)),
            // 12: a lookup without an `End`.
            p2_start(None, false, "incomplete.test"),
            // 13..=14: a lookup after the jump.
            p2_start(None, false, "later.test"),
            p2_end(13, "10.0.0.6"),
        ]);
        let names = &status.resolved_names;
        assert_eq!(names.links.hostnames(ip("10.0.0.1")), vec!["primary.test"]);
        assert_eq!(names.links.hostnames(ip("10.0.0.6")), vec!["later.test"]);
        for unlinked in ["10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5"] {
            assert!(names.links.hostnames(ip(unlinked)).is_empty(), "{unlinked}");
        }
        assert_eq!(names.links.len(), 2);
        // Only the lookup without an `End` stays open; the entity body is still open too.
        assert_eq!(
            names.following.open.keys().copied().collect::<Vec<_>>(),
            vec![OplogIndex::from_u64(12)]
        );
    }

    #[test]
    fn a_jump_over_folded_lookups_refolds_the_status() {
        let before = folded(vec![
            p2_start(None, false, "jumped.test"),
            p2_end(2, "10.0.0.4"),
        ]);
        assert_eq!(
            before.resolved_names.links.hostnames(ip("10.0.0.4")),
            vec!["jumped.test"]
        );
        // The folded links cannot be taken back, so the fold asks for a fold from the start,
        // which reads the jump before the lookups.
        assert!(
            fold(
                before,
                numbered(
                    4,
                    vec![OplogEntry::jump(None, OplogRegion::from_range(2..=3))]
                )
            )
            .is_none()
        );
    }

    #[test]
    fn p3_lookups_link_at_their_delivery_marker() {
        let status = folded(vec![
            // 2..=7: two lookups of one address whose completions reach the guest in the
            // reverse order of their `End` entries.
            p3_start("a.test"),
            p3_start("b.test"),
            p3_end(2, "10.0.0.5"),
            p3_end(3, "10.0.0.5"),
            delivered(3),
            delivered(2),
            // 8..=10: a lookup whose completion the guest discarded.
            p3_start("discarded.test"),
            p3_end(8, "10.0.0.6"),
            discarded(8),
            // 11..=12: a lookup whose completion the guest has not received.
            p3_start("undelivered.test"),
            p3_end(11, "10.0.0.7"),
        ]);
        let names = &status.resolved_names;
        // The lookup delivered last is the most recent link, as in the running Store.
        assert_eq!(
            names.links.hostnames(ip("10.0.0.5")),
            vec!["a.test", "b.test"]
        );
        assert!(names.links.hostnames(ip("10.0.0.6")).is_empty());
        assert!(names.links.hostnames(ip("10.0.0.7")).is_empty());

        // The undelivered lookup links when its marker comes in a later fold.
        let later = fold(status.clone(), numbered(13, vec![delivered(11)])).unwrap();
        assert_eq!(
            later.resolved_names.links.hostnames(ip("10.0.0.7")),
            vec!["undelivered.test"]
        );
        assert!(later.resolved_names.following.open.is_empty());
    }

    #[test]
    fn the_fold_links_the_address_records_of_delivered_dns_record_lookups() {
        let record_start = |name: &str| {
            start(
                HostFunctionName::GolemNetworkDnsResolve,
                None,
                false,
                Some(HostRequest::NetworkDnsResolve(
                    HostRequestNetworkDnsResolve {
                        name: name.to_string(),
                        record_type: SerializableDnsRecordType::A,
                    },
                )),
            )
        };
        let record_end = |start: u64, records: Vec<SerializableDnsRecordData>| {
            end(
                start,
                Some(HostResponse::NetworkDnsResolve(
                    HostResponseNetworkDnsResolve {
                        result: Ok(records
                            .into_iter()
                            .map(|data| SerializableDnsRecord { ttl: 60, data })
                            .collect()),
                    },
                )),
            )
        };
        let status = folded(vec![
            // 2..=4: a delivered A/AAAA answer links both addresses.
            record_start("db.test"),
            record_end(
                2,
                vec![
                    SerializableDnsRecordData::A(ip("10.0.0.1").into()),
                    SerializableDnsRecordData::Aaaa(ip("fd00::1").into()),
                ],
            ),
            delivered(2),
            // 5..=7: an MX answer links nothing, even when its exchange is an address.
            record_start("mail.test"),
            record_end(
                5,
                vec![SerializableDnsRecordData::Mx {
                    preference: 10,
                    exchange: "10.0.0.2".to_string(),
                }],
            ),
            delivered(5),
            // 8..=10: a discarded answer links nothing.
            record_start("discarded.test"),
            record_end(8, vec![SerializableDnsRecordData::A(ip("10.0.0.3").into())]),
            discarded(8),
        ]);
        let links = &status.resolved_names.links;
        assert_eq!(links.hostnames(ip("10.0.0.1")), vec!["db.test"]);
        assert_eq!(links.hostnames(ip("fd00::1")), vec!["db.test"]);
        assert!(links.hostnames(ip("10.0.0.2")).is_empty());
        assert!(links.hostnames(ip("10.0.0.3")).is_empty());
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn the_fold_keeps_only_open_lookups_and_scopes() {
        let mut status = AgentStatusRecord::default();
        let mut index = 2u64;
        for _ in 0..1_000 {
            let first = index;
            let entries = numbered(
                first,
                vec![
                    start(HostFunctionName::GolemEntityInvoke, None, false, None),
                    p2_start(Some(first), false, "entity.test"),
                    p2_end(first + 1, "10.0.0.9"),
                    end(first, None),
                    p2_start(None, false, "same.test"),
                    p2_end(first + 4, "10.0.0.5"),
                    p3_start("same.test"),
                    p3_end(first + 6, "10.0.0.5"),
                    delivered(first + 6),
                ],
            );
            index += entries.len() as u64;
            status = fold(status, entries).unwrap();
            assert!(status.resolved_names.following.entity_scopes.is_empty());
            assert!(status.resolved_names.following.open.is_empty());
        }
        assert_eq!(status.resolved_names.links.len(), 1);
        assert_eq!(
            status.resolved_names.links.hostnames(ip("10.0.0.5")),
            vec!["same.test"]
        );
    }

    /// A snapshot-based update whose `PendingUpdate` is at 4, after a lookup at 2..=3, with a
    /// lookup of its attempt at 5..=6 and `outcome` at 7.
    fn snapshot_based_update(outcome: OplogEntry) -> Vec<OplogEntry> {
        vec![
            p2_start(None, false, "before.test"),
            p2_end(2, "10.0.0.1"),
            OplogEntry::pending_update(
                UpdateDescription::SnapshotBased {
                    target_revision: ComponentRevision::new(2).unwrap(),
                    payload: OplogPayload::Inline(Box::new(vec![])),
                    mime_type: "application/octet-stream".to_string(),
                    filesystem_snapshot: None,
                },
                None,
            ),
            p2_start(None, false, "attempt.test"),
            p2_end(5, "10.0.0.1"),
            outcome,
        ]
    }

    #[test]
    fn the_fold_reads_the_answer_of_a_dns_query_only_when_its_record_type_links() {
        // An answer that the payload store holds, as an oplog read returns it without loading.
        let unloaded_end = OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(2),
            response: Some(OplogPayload::External {
                payload_id: golem_common::model::oplog::PayloadId::new(),
                md5_hash: vec![0; 16],
                cached: None,
            }),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        };
        let query = |record_type| {
            start(
                HostFunctionName::GolemNetworkDnsResolve,
                None,
                false,
                Some(HostRequest::NetworkDnsResolve(
                    HostRequestNetworkDnsResolve {
                        name: "big.test".to_string(),
                        record_type,
                    },
                )),
            )
        };
        let fold_query = |record_type| {
            update_status_with_new_entries(
                AgentMode::Durable,
                AgentStatusRecord::default(),
                numbered(
                    2,
                    vec![query(record_type), unloaded_end.clone(), delivered(2)],
                ),
                &RetryConfig::default(),
            )
        };
        for record_type in [
            SerializableDnsRecordType::Txt,
            SerializableDnsRecordType::Mx,
            SerializableDnsRecordType::Cname,
        ] {
            let status = fold_query(record_type).unwrap().unwrap();
            assert!(status.resolved_names.links.is_empty(), "{record_type:?}");
            assert!(
                status.resolved_names.following.open.is_empty(),
                "{record_type:?}"
            );
        }
        for record_type in [
            SerializableDnsRecordType::A,
            SerializableDnsRecordType::Aaaa,
        ] {
            assert_eq!(
                fold_query(record_type),
                Err(
                    "the payload of the address lookup at oplog index 3 was not loaded before the status fold"
                        .to_string()
                ),
                "{record_type:?}"
            );
        }
    }

    #[test]
    fn a_successful_snapshot_based_update_keeps_the_links_of_the_history_and_of_its_target() {
        let status = folded(snapshot_based_update(OplogEntry::successful_update(
            ComponentRevision::new(2).unwrap(),
            100,
            None,
            Default::default(),
            None,
            Some(OplogRegion::from_range(5..=6)),
        )));
        // The history before the update ran, and so did the target's initializer: both link,
        // the initializer most recently.
        assert_eq!(
            status.resolved_names.links.hostnames(ip("10.0.0.1")),
            vec!["attempt.test", "before.test"]
        );
    }

    #[test]
    fn a_failed_snapshot_based_update_drops_the_links_of_its_attempt() {
        let failed = OplogEntry::failed_update(
            ComponentRevision::new(2).unwrap(),
            Some("details".to_string()),
            None,
            None,
            None,
            golem_common::model::oplog::UpdateFailureClass::Other,
            None,
        );
        let status = folded(snapshot_based_update(failed.clone()));
        assert_eq!(
            status.resolved_names.links.hostnames(ip("10.0.0.1")),
            vec!["before.test"]
        );
        // A fold that already passed the attempt refolds from the start.
        let mut entries = snapshot_based_update(failed);
        let outcome = entries.pop().unwrap();
        let attempted = folded(entries);
        assert!(fold(attempted, numbered(7, vec![outcome])).is_none());
    }
}
