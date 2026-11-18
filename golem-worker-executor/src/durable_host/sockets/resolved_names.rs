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
//! A live lookup links its addresses: a P2 lookup when its call returns, a P3 or
//! `golem:network/dns` lookup when its delivery observer settles its completion as delivered. A
//! completion that the guest discards links nothing.
//!
//! The primary Store of an agent does not link on replay. The status record folds the same links
//! from the oplog (`AgentStatusRecord::resolved_names`), and the Store takes them from the status
//! when it becomes live. A replayed connect returns the decision recorded in its `Start`, so it
//! never reads the index. A tool entity Store has its own index, which is not in the status: it
//! links on replay too, so a body that continues live has the links of its replayed lookups.

use crate::durable_host::replay_state::ReplayState;
use golem_common::model::agent::ResolvedNames;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The [`ResolvedNames`] of a Store, shared with the delivery observers that link P3 lookups
/// when the guest receives their result. An observer runs in Wasmtime's event loop without the
/// Store, so the index is behind a lock of its own; every holder keeps the lock for one index
/// operation only.
#[derive(Debug, Clone, Default)]
pub struct SharedResolvedNames {
    names: Arc<Mutex<ResolvedNames>>,
    /// The replay state of a primary Store, whose lookups link only once it is live.
    primary: Option<ReplayState>,
    /// Whether the links of a primary Store were taken from the status record.
    seeded: Arc<AtomicBool>,
}

impl SharedResolvedNames {
    /// The index of a primary Store with `replay_state`, or of an entity Store for `None`.
    pub fn new(primary: Option<ReplayState>) -> Self {
        Self {
            names: Arc::default(),
            primary,
            seeded: Arc::default(),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut ResolvedNames) -> R) -> R {
        f(&mut self
            .names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Links `addresses` to the hostname of a completed lookup of `name`, unless this is the
    /// index of a primary Store that is not live yet.
    pub fn link(&self, name: &str, addresses: impl IntoIterator<Item = IpAddr>) {
        if self
            .primary
            .as_ref()
            .is_none_or(|replay| replay.is_live_published())
        {
            self.with(|names| names.insert(name, addresses));
        }
    }

    /// Replaces the links of a primary Store with `links`, the links that the status record
    /// folded from the oplog, when the Store becomes live. Only the first call after the Store
    /// becomes live takes them: a later call would drop the links of live lookups that the
    /// status did not fold yet. Does nothing for an entity Store.
    pub fn seed(&self, links: &ResolvedNames) {
        if self.primary.is_some() && !self.seeded.swap(true, Ordering::AcqRel) {
            self.with(|names| *names = links.clone());
        }
    }
}
