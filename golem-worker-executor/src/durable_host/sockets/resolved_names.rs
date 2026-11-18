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
//! `golem:network/dns` A and AAAA lookups returned them for, so that a hostname grant matches a
//! raw-socket connect to the address.
//!
//! A lookup links its addresses: a P2 lookup when its call returns, a P3 or `golem:network/dns`
//! lookup when its delivery observer settles its completion as delivered. A completion that the
//! guest discards links nothing.
//!
//! The primary Store of an agent takes its links from the status record when it is created. The
//! status fold folds the same links from the oplog (`AgentStatusRecord::resolved_names`), so the
//! Store then links only the lookups that it records itself: a replayed lookup, also one that a
//! bootstrap cursor replays, is in the status already, and linking it again would refresh its
//! position in the LRU order. A connect therefore reads the links of its whole history on every
//! path that continues live. A tool entity Store has its own index, which is not in the status:
//! it links its replayed lookups too.

use golem_common::model::agent::ResolvedNames;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// The [`ResolvedNames`] of a Store, shared with the delivery observers that link P3 lookups
/// when the guest receives their result. An observer runs in Wasmtime's event loop without the
/// Store, so the index is behind a lock of its own; every holder keeps the lock for one index
/// operation only.
#[derive(Debug, Clone, Default)]
pub struct SharedResolvedNames {
    names: Arc<Mutex<ResolvedNames>>,
    /// Whether this is the index of a primary Store, which starts with the links of its status
    /// record.
    primary: bool,
}

impl SharedResolvedNames {
    /// The index of a primary Store, which starts with `links`, the links that its status record
    /// folded from the oplog.
    pub fn primary(links: ResolvedNames) -> Self {
        Self {
            names: Arc::new(Mutex::new(links)),
            primary: true,
        }
    }

    /// The index of an entity Store, which starts empty.
    pub fn entity() -> Self {
        Self::default()
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut ResolvedNames) -> R) -> R {
        f(&mut self
            .names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Links `addresses` to the hostname of a completed lookup of `name`. `recorded` tells
    /// whether this Store recorded the link point of the lookup now (its `End` or its
    /// `CompletionDelivered` marker) rather than replaying it. A primary Store links only a
    /// recorded lookup, because its status links hold every replayed one.
    pub fn link(&self, name: &str, addresses: impl IntoIterator<Item = IpAddr>, recorded: bool) {
        if recorded || !self.primary {
            self.with(|names| names.insert(name, addresses));
        }
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
    fn a_primary_store_links_only_recorded_lookups_after_its_status_links() {
        let mut status = ResolvedNames::default();
        status.insert("old.test", [ip("10.0.0.1")]);
        status.insert("new.test", [ip("10.0.0.1")]);
        let names = SharedResolvedNames::primary(status);

        // A replayed lookup, such as an initializer on a bootstrap cursor, keeps the order.
        names.link("old.test", [ip("10.0.0.1")], false);
        assert_eq!(
            names.with(|names| names.hostnames(ip("10.0.0.1"))),
            vec!["new.test", "old.test"]
        );
        names.link("live.test", [ip("10.0.0.1")], true);
        assert_eq!(
            names.with(|names| names.hostnames(ip("10.0.0.1"))),
            vec!["live.test", "new.test", "old.test"]
        );

        // At the bound, a replayed initializer link that is the oldest one stays the oldest, so
        // the next recorded lookup evicts it and keeps every newer link.
        let mut full = ResolvedNames::default();
        full.insert("initializer.test", [ip("10.0.1.1")]);
        for n in 1..golem_common::model::agent::MAX_RESOLVED_NAME_LINKS {
            full.insert(&format!("h{n}.test"), [ip("10.0.2.1")]);
        }
        let names = SharedResolvedNames::primary(full);
        names.link("initializer.test", [ip("10.0.1.1")], false);
        names.link("next.test", [ip("10.0.3.1")], true);
        assert!(
            names
                .with(|names| names.hostnames(ip("10.0.1.1")))
                .is_empty()
        );
        let newer = names.with(|names| names.hostnames(ip("10.0.2.1")));
        assert!(newer.contains(&"h1.test".to_string()));
        assert_eq!(
            newer.len(),
            golem_common::model::agent::MAX_RESOLVED_NAME_LINKS - 1
        );

        let entity = SharedResolvedNames::entity();
        entity.link("replayed.test", [ip("10.0.0.2")], false);
        assert_eq!(
            entity.with(|names| names.hostnames(ip("10.0.0.2"))),
            vec!["replayed.test"]
        );
    }
}
