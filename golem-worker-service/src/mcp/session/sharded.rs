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

//! A map split into shards, each behind its own lock, so that requests of different sessions
//! and events of different streams rarely wait for each other.
//!
//! Lock rule: while a shard lock is held, nothing awaits, no other shard lock is taken, and no
//! value whose `Drop` can take a lock is dropped. The only locks taken under a shard lock are
//! leaf locks that never lead back to a shard: the node lock of a cancellation token (taken by
//! `CancellationToken::clone`) and the tail lock of a broadcast channel (taken by
//! `broadcast::Sender::subscribe`). A step therefore returns every value it takes out of the map,
//! and the caller drops it after the lock is released.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, RandomState};
use std::sync::{Mutex, PoisonError};

/// The number of shards of every sharded map.
pub(super) const SHARDS: usize = 64;

pub(super) struct Sharded<K, V> {
    shards: Box<[Mutex<HashMap<K, V>>]>,
    /// Keyed per map, so that a client cannot choose keys that all land in one shard.
    hasher: RandomState,
}

impl<K: Hash + Eq, V> Sharded<K, V> {
    pub(super) fn new() -> Self {
        Self {
            shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
            hasher: RandomState::new(),
        }
    }

    /// Runs `step` on the shard of `key` under its lock and returns its result. `step` must
    /// return the values it removes, so that they are dropped after the lock is released. The
    /// key is borrowed, so a lookup builds no owned key.
    #[track_caller]
    pub(super) fn with<Q, R>(&self, key: &Q, step: impl FnOnce(&mut HashMap<K, V>) -> R) -> R
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let index = (self.hasher.hash_one(key) % SHARDS as u64) as usize;
        self.with_shard(index, step)
    }

    /// Runs `step` on the shard at `index` (below [`SHARDS`]) under its lock.
    #[track_caller]
    pub(super) fn with_shard<R>(
        &self,
        index: usize,
        step: impl FnOnce(&mut HashMap<K, V>) -> R,
    ) -> R {
        #[cfg(test)]
        let _held = lock_check::enter();
        // A step that panicked leaves its shard as it was at the panic; the map stays usable.
        let mut shard = self.shards[index]
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        step(&mut shard)
    }
}

/// Test builds only: taking a shard lock while this thread already holds one panics, and the
/// first such violation is recorded for the whole process. Tokio catches panics in spawned tasks,
/// so the tests check the record ([`assert_no_violation`]) in addition to the panic.
#[cfg(test)]
pub(super) mod lock_check {
    use std::cell::Cell;
    use std::panic::Location;
    use std::sync::OnceLock;

    thread_local! {
        static HELD: Cell<bool> = const { Cell::new(false) };
    }

    /// Where the first nested shard lock of this process was taken. Never reset.
    static FIRST_VIOLATION: OnceLock<Box<str>> = OnceLock::new();

    /// Marks this thread as holding a shard lock until it is dropped, also on a panic.
    pub(in crate::mcp::session) struct Held(());

    impl Drop for Held {
        fn drop(&mut self) {
            HELD.with(|held| held.set(false));
        }
    }

    #[track_caller]
    pub(in crate::mcp::session) fn enter() -> Held {
        if HELD.with(Cell::get) {
            let location = Location::caller();
            FIRST_VIOLATION.get_or_init(|| location.to_string().into_boxed_str());
            panic!("a shard lock was taken at {location} while another shard lock is held");
        }
        HELD.with(|held| held.set(true));
        Held(())
    }

    /// Fails when any thread of this process has taken a shard lock while holding another one.
    pub(in crate::mcp::session) fn assert_no_violation() {
        if let Some(location) = FIRST_VIOLATION.get() {
            panic!("a shard lock was taken while another shard lock was held, first at {location}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Sharded;
    use std::sync::Arc;
    use test_r::test;

    #[test]
    fn a_panicking_step_does_not_break_its_shard() {
        let map = Arc::new(Sharded::<u32, u32>::new());
        map.with(&1, |shard| shard.insert(1, 10));
        let panicking = map.clone();
        let result = std::thread::spawn(move || {
            panicking.with(&1, |shard| {
                shard.insert(2, 20);
                panic!("step failed")
            })
        })
        .join();
        assert!(result.is_err());
        assert_eq!(map.with(&1, |shard| shard.get(&1).copied()), Some(10));
        super::lock_check::assert_no_violation();
    }
}
