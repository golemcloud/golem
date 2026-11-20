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

//! A bounded, revision-aware cache of account tree nodes.
//!
//! The cache holds individual nodes by account ID and a sibling index from
//! `(parent, segment)` to the child's ID. A path or an ancestry is composed from cached nodes,
//! so a warm read makes no database call, and a node changes in the cache only through its own
//! entry. A derived path or effective lifecycle state therefore follows a change to any
//! ancestor, not only to the leaf.
//!
//! Dependencies and freshness:
//!
//! - Every committed tree change appends an `AccountChanged` journal event, and the registry
//!   change notifier delivers it after the commit. [`AccountTreeCache::handle_event`] evicts the
//!   node and its sibling-index entry.
//! - A miss loads the whole chain in one database statement. The cache has a generation that
//!   every installation and every invalidation advances. A load notes the generation before it
//!   reads and installs its result only if the generation has not moved; otherwise it returns
//!   the result to its caller and installs nothing. A delayed load therefore cannot replace a
//!   value that an event removed, nor nodes that a later-starting load installed, and the
//!   sibling index always comes from the snapshot of one load. Concurrent cold loads of
//!   overlapping chains may therefore skip installation and read again on the next miss.
//! - A lagged subscription, a closed notifier channel or a notifier resync clears the cache,
//!   because events may have been missed. The PostgreSQL notifier replays the journal from its
//!   cursor after its listener reconnects and signals a resync when the journal no longer
//!   holds every event after the cursor.
//! - Capacity eviction removes the least recently used nodes; a later read reloads them.
//! - As a backup to the journal, every node has a maximum age counted from the database load
//!   that installed it. A hit does not renew it. A read that meets an expired node reloads the
//!   chain instead of serving it. The maximum age only bounds how long a missed notification can
//!   keep a stale node; it does not replace the journal invalidation.
//!
//! On a database error the cache returns the error and installs nothing; it never falls back to
//! an expired node.
//!
//! Costs and limits:
//!
//! - The capacity counts cached nodes, ancestors included, not accounts of the installation or
//!   bytes of memory. The constructor raises it to at least nine, one full ancestry.
//! - A warm read makes no database call; a miss loads its chain in one statement. Eviction and
//!   expiry cost more database reads; they never refuse an account.
//! - An installation over capacity sorts every cached node under the one mutex, so heavy churn
//!   can delay concurrent reads.
//! - The cache makes no promise about account count, memory use or throughput. HA-15 chooses the
//!   production capacity and maximum age, and measures memory, mutex contention and the database
//!   load of churn and expiry before activation.

use crate::repo::account_tree::AccountTreeRepo;
use crate::repo::model::account_tree::AccountNode;
use crate::repo::registry_change::RegistryChangeEvent;
use golem_common::model::account_path::AccountPath;
use golem_service_base::repo::RepoError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

type SiblingKey = (Option<Uuid>, String);

struct Entry {
    node: AccountNode,
    last_used: u64,
    loaded_at: Instant,
}

#[derive(Default)]
struct State {
    nodes: HashMap<Uuid, Entry>,
    children: HashMap<SiblingKey, Uuid>,
    generation: u64,
    clock: u64,
}

impl State {
    fn sibling_key(node: &AccountNode) -> SiblingKey {
        (
            node.parent_id,
            node.segment
                .as_ref()
                .map(|segment| segment.as_str().to_string())
                .unwrap_or_default(),
        )
    }

    fn touch(&mut self, account_id: Uuid, max_age: Duration) -> Option<AccountNode> {
        if self
            .nodes
            .get(&account_id)
            .is_some_and(|entry| entry.loaded_at.elapsed() >= max_age)
        {
            self.remove(account_id);
            return None;
        }
        self.clock += 1;
        let clock = self.clock;
        self.nodes.get_mut(&account_id).map(|entry| {
            entry.last_used = clock;
            entry.node.clone()
        })
    }

    fn remove(&mut self, account_id: Uuid) {
        if let Some(entry) = self.nodes.remove(&account_id) {
            let key = Self::sibling_key(&entry.node);
            if self.children.get(&key) == Some(&account_id) {
                self.children.remove(&key);
            }
        }
    }

    fn install(&mut self, chain: &[AccountNode], capacity: usize, loaded_at: Instant) {
        self.generation += 1;
        for node in chain {
            self.remove(node.account_id);
            self.clock += 1;
            self.children
                .insert(Self::sibling_key(node), node.account_id);
            self.nodes.insert(
                node.account_id,
                Entry {
                    node: node.clone(),
                    last_used: self.clock,
                    loaded_at,
                },
            );
        }
        if self.nodes.len() > capacity {
            let mut by_age: Vec<(u64, Uuid)> = self
                .nodes
                .iter()
                .map(|(id, entry)| (entry.last_used, *id))
                .collect();
            by_age.sort_unstable();
            let excess = self.nodes.len() - capacity;
            for (_, id) in by_age.into_iter().take(excess) {
                self.remove(id);
            }
        }
    }

    fn ancestry(&mut self, account_id: Uuid, max_age: Duration) -> Option<Vec<AccountNode>> {
        let mut chain = Vec::new();
        let mut next = Some(account_id);
        while let Some(id) = next {
            let node = self.touch(id, max_age)?;
            next = node.parent_id;
            chain.push(node);
            if chain.len() > AccountPath::MAX_DEPTH + 1 {
                return None;
            }
        }
        chain.reverse();
        Some(chain)
    }

    fn resolve(&mut self, path: &AccountPath, max_age: Duration) -> Option<Vec<AccountNode>> {
        let mut parent = None;
        let mut chain = Vec::with_capacity(path.depth() + 1);
        let segments = std::iter::once(String::new()).chain(
            path.segments()
                .iter()
                .map(|segment| segment.as_str().to_string()),
        );
        for segment in segments {
            let id = *self.children.get(&(parent, segment))?;
            let node = self.touch(id, max_age)?;
            parent = Some(node.account_id);
            chain.push(node);
        }
        Some(chain)
    }
}

pub struct AccountTreeCache {
    repo: Arc<dyn AccountTreeRepo>,
    capacity: usize,
    max_age: Duration,
    state: Mutex<State>,
}

impl AccountTreeCache {
    /// `max_age` bounds how long a node stays cached after the load that installed it.
    pub fn new(repo: Arc<dyn AccountTreeRepo>, capacity: usize, max_age: Duration) -> Self {
        Self {
            repo,
            capacity: capacity.max(AccountPath::MAX_DEPTH + 1),
            max_age,
            state: Mutex::new(State::default()),
        }
    }

    /// The node and its ancestors, root first, or `None` when the account does not exist.
    pub async fn ancestry(&self, account_id: Uuid) -> Result<Option<Vec<AccountNode>>, RepoError> {
        let generation = {
            let mut state = self.state.lock().unwrap();
            if let Some(chain) = state.ancestry(account_id, self.max_age) {
                return Ok(Some(chain));
            }
            state.generation
        };
        let loaded_at = Instant::now();
        let records = self.repo.get_ancestry(account_id).await?;
        self.finish_load(generation, loaded_at, records)
    }

    /// The nodes along a path, root first, or `None` when no current account has the path.
    pub async fn resolve(&self, path: &AccountPath) -> Result<Option<Vec<AccountNode>>, RepoError> {
        let generation = {
            let mut state = self.state.lock().unwrap();
            if let Some(chain) = state.resolve(path, self.max_age) {
                return Ok(Some(chain));
            }
            state.generation
        };
        let loaded_at = Instant::now();
        let records = self.repo.resolve_path(path).await?;
        self.finish_load(generation, loaded_at, records)
    }

    fn finish_load(
        &self,
        generation: u64,
        loaded_at: Instant,
        records: Vec<crate::repo::model::account_tree::AccountNodeRecord>,
    ) -> Result<Option<Vec<AccountNode>>, RepoError> {
        if records.is_empty() {
            return Ok(None);
        }
        let chain = records
            .into_iter()
            .map(AccountNode::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let mut state = self.state.lock().unwrap();
        if state.generation == generation {
            state.install(&chain, self.capacity, loaded_at);
        }
        Ok(Some(chain))
    }

    pub fn invalidate(&self, account_id: Uuid) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.remove(account_id);
    }

    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.nodes.clear();
        state.children.clear();
    }

    pub fn handle_event(&self, event: &RegistryChangeEvent) {
        if let RegistryChangeEvent::AccountChanged { account_id, .. } = event {
            self.invalidate(*account_id);
        }
    }

    /// Applies journal events until the event channel closes. Clears the cache whenever events
    /// may have been missed: when the subscription lags, when it closes, and whenever the
    /// notifier signals a resync.
    pub async fn run_invalidation(
        &self,
        mut events: broadcast::Receiver<RegistryChangeEvent>,
        resyncs: watch::Receiver<u64>,
    ) {
        let mut resyncs = Some(resyncs);
        loop {
            let resync = async {
                match resyncs.as_mut() {
                    Some(resyncs) => resyncs.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                event = events.recv() => match event {
                    Ok(event) => self.handle_event(&event),
                    Err(broadcast::error::RecvError::Lagged(_)) => self.clear(),
                    Err(broadcast::error::RecvError::Closed) => {
                        self.clear();
                        return;
                    }
                },
                signalled = resync => {
                    if signalled {
                        self.clear();
                    } else {
                        // The notifier has no resync source.
                        resyncs = None;
                    }
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.state.lock().unwrap().nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
