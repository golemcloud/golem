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

//! The packs that one backend keeps in memory after a full read, up to a limit of bytes.
//!
//! rustic reads each tree blob with its own ranged read. A backend lives for one operation, so it
//! keeps each pack of tree blobs after its first read and gives the later ranges from memory.

use bytes::Bytes;
use rustic_core::{Id, RusticResult};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// The packs that one backend keeps, and the packs that a thread reads now.
pub(super) struct KeptPacks {
    limit: usize,
    state: Mutex<State>,
    /// Wakes the threads that wait for the read of a pack when that read ends.
    read_ended: Condvar,
}

#[derive(Default)]
struct State {
    packs: HashMap<Id, Bytes>,
    bytes: usize,
    reading: HashSet<Id>,
}

impl KeptPacks {
    /// Gives an empty set that keeps packs up to `limit` bytes in total.
    pub(super) fn new(limit: usize) -> Self {
        Self {
            limit,
            state: Mutex::default(),
            read_ended: Condvar::new(),
        }
    }

    /// Gives the kept pack, or reads it with `read`. While one thread reads a pack, the other
    /// threads that want it wait for that read, and then take the kept pack or read it again. A
    /// pack is kept only when its read succeeds and it fits in the limit.
    pub(super) fn get_or_read(
        &self,
        id: &Id,
        read: impl FnOnce() -> RusticResult<Bytes>,
    ) -> RusticResult<Bytes> {
        let mut state = self
            .read_ended
            .wait_while(self.state(), |state| state.reading.contains(id))
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(pack) = state.packs.get(id) {
            return Ok(pack.clone());
        }
        state.reading.insert(*id);
        drop(state);
        let reading = Reading {
            kept: self,
            id: *id,
        };
        let read = read();
        if let Ok(pack) = &read {
            reading.keep(pack);
        }
        read
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Debug for KeptPacks {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let state = self.state();
        formatter
            .debug_struct("KeptPacks")
            .field("limit", &self.limit)
            .field("packs", &state.packs.len())
            .field("bytes", &state.bytes)
            .finish()
    }
}

/// The read of one pack by one thread. Its drop ends the read and wakes the waiting threads, also
/// when the read fails or panics.
struct Reading<'a> {
    kept: &'a KeptPacks,
    id: Id,
}

impl Reading<'_> {
    /// Keeps the pack when it fits in the limit.
    fn keep(&self, pack: &Bytes) {
        let mut state = self.kept.state();
        let bytes = state.bytes.saturating_add(pack.len());
        if bytes <= self.kept.limit {
            state.bytes = bytes;
            state.packs.insert(self.id, pack.clone());
        }
    }
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        self.kept.state().reading.remove(&self.id);
        self.kept.read_ended.notify_all();
    }
}
