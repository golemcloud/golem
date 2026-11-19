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

use crate::base_model::OplogIndex;
use crate::base_model::card::network_target::link_hostname;
use crate::model::oplog::types::SerializableIpAddress;
use desert_rust::BinaryCodec;
use im::{OrdMap, Vector};
use std::net::IpAddr;

/// The most links one index keeps. A lookup that adds a link past it evicts the link that was
/// added or refreshed longest ago. With hostnames of at most 253 bytes the index holds at most
/// about 270 KiB, and adding or reading links scans at most this many entries.
pub const MAX_RESOLVED_NAME_LINKS: usize = 1024;

/// The links from addresses to the normalized hostnames that completed address lookups returned
/// them for, the least recently added first. The links share their structure between clones, so
/// a status fold that clones the status record does not copy them.
#[derive(Clone, Debug, Default, PartialEq, Eq, BinaryCodec)]
#[desert(transparent)]
pub struct ResolvedNames(Vector<(SerializableIpAddress, String)>);

impl ResolvedNames {
    /// Links `addresses` to the hostname of the lookup of `name`. A link that exists already
    /// becomes the most recent one.
    pub fn insert(&mut self, name: &str, addresses: impl IntoIterator<Item = IpAddr>) {
        let Some(hostname) = link_hostname(name) else {
            return;
        };
        for address in addresses {
            let link = (
                SerializableIpAddress::from(address.to_canonical()),
                hostname.clone(),
            );
            if let Some(position) = self.0.iter().position(|existing| *existing == link) {
                self.0.remove(position);
            }
            self.0.push_back(link);
            if self.0.len() > MAX_RESOLVED_NAME_LINKS {
                self.0.pop_front();
            }
        }
    }

    /// The hostnames linked to `address`, the most recently linked first.
    pub fn hostnames(&self, address: IpAddr) -> Vec<String> {
        let address = SerializableIpAddress::from(address.to_canonical());
        self.0
            .iter()
            .rev()
            .filter(|(linked, _)| *linked == address)
            .map(|(_, hostname)| hostname.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The links of the address lookups of an agent's primary Store, folded from its oplog, and the
/// lookups and entity scopes that the fold still follows.
#[derive(Clone, Debug, Default, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct ResolvedNameIndex {
    pub links: ResolvedNames,
    pub following: FollowedLookups,
}

/// The lookups and entity scopes that the fold of the links still follows. They change with
/// calls that link nothing, so the status cache keeps them apart from the links.
#[derive(Clone, Debug, Default, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct FollowedLookups {
    /// The lookups that can still link, by their `Start` index.
    pub open: OrdMap<OplogIndex, OpenLookup>,
    /// The `Start` indexes of the open entity invocations and of the open calls inside them.
    /// A lookup inside an entity body belongs to the entity's own Store and links nothing here.
    pub entity_scopes: OrdMap<OplogIndex, ()>,
}

/// A lookup whose result did not link yet.
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum OpenLookup {
    /// A lookup without its `End`. `at_delivery` is set for a lookup whose result links at its
    /// `CompletionDelivered` marker, and unset for one that links at its `End`.
    Started { name: String, at_delivery: bool },
    /// A lookup whose `End` returned `addresses` and that links at its `CompletionDelivered`
    /// marker.
    Undelivered {
        name: String,
        addresses: Vec<SerializableIpAddress>,
    },
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
        assert_eq!(names.len(), MAX_RESOLVED_NAME_LINKS);
        assert_eq!(names.hostnames(bulk_address(0)), vec!["first.test"]);
        assert!(names.hostnames(bulk_address(1)).is_empty());
        assert_eq!(
            names.hostnames(bulk_address(MAX_RESOLVED_NAME_LINKS)),
            vec!["bulk.test"]
        );
    }

    #[test]
    fn the_index_roundtrips_with_its_link_order() {
        let mut index = ResolvedNameIndex::default();
        index.links.insert("a.test", [ip("10.0.0.1"), ip("::1")]);
        index.links.insert("b.test", [ip("10.0.0.1")]);
        index.following.open.insert(
            OplogIndex::from_u64(7),
            OpenLookup::Undelivered {
                name: "c.test".to_string(),
                addresses: vec![ip("10.0.0.3").into()],
            },
        );
        index
            .following
            .entity_scopes
            .insert(OplogIndex::from_u64(9), ());
        let bytes = crate::serialization::serialize(&index).unwrap();
        let decoded: ResolvedNameIndex = crate::serialization::deserialize(&bytes).unwrap();
        assert_eq!(decoded, index);
        assert_eq!(
            decoded.links.hostnames(ip("10.0.0.1")),
            vec!["b.test", "a.test"]
        );
    }
}
