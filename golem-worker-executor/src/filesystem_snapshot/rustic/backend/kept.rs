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
//! keeps each pack of tree blobs after its first read and gives the later ranges from memory. Once
//! the kept packs fill the limit, or a pack that was read whole did not fit, the set is closed: a
//! pack that is not kept is not read whole, because it cannot be kept, and the caller reads only
//! its range.

use bytes::Bytes;
use rustic_core::{Id, RusticResult};
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// The packs that one backend keeps, and the packs that a thread reads now. When the kept bytes
/// reach the limit, or a pack that was read whole does not fit, the set is closed. A closed set
/// keeps no more packs, and a pack that it does not keep is not read whole.
pub(super) struct KeptPacks {
    limit: usize,
    state: Mutex<State>,
    /// Wakes the threads that wait for the read of a pack when that read ends.
    read_ended: Condvar,
}

#[derive(Debug, Default)]
struct State {
    packs: HashMap<Id, Bytes>,
    bytes: usize,
    /// A pack that was read whole did not fit in the limit.
    closed: bool,
    reading: HashSet<Id>,
}

/// What a thread that wants a pack does.
#[derive(Debug, PartialEq, Eq)]
enum Want<'a> {
    /// Another thread reads the pack, so this thread waits for that read.
    Wait,
    /// The set keeps the pack.
    Kept(&'a Bytes),
    /// The set is closed or full and does not keep the pack, so the pack is not read whole.
    Skip,
    /// This thread reads the pack whole.
    Read,
}

/// Gives what a thread that wants the pack does in the state, with the limit of the kept bytes.
fn want<'a>(state: &'a State, id: &Id, limit: usize) -> Want<'a> {
    if state.reading.contains(id) {
        Want::Wait
    } else if let Some(pack) = state.packs.get(id) {
        Want::Kept(pack)
    } else if state.closed || state.bytes >= limit {
        Want::Skip
    } else {
        Want::Read
    }
}

/// What the set does with a pack that a thread read whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admit {
    /// Keeps the pack, and then keeps `bytes` in total.
    Keep { bytes: usize },
    /// Closes the set, because the pack does not fit.
    Close,
}

/// Gives what the set that keeps `kept_bytes` does with a pack of `len` bytes, with the limit of
/// the kept bytes.
fn admit(kept_bytes: usize, len: usize, limit: usize) -> Admit {
    let bytes = kept_bytes.saturating_add(len);
    if bytes <= limit {
        Admit::Keep { bytes }
    } else {
        Admit::Close
    }
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
    /// pack is kept only when its read succeeds and it fits in the limit. When the set is closed and
    /// the pack is not kept, it gives `None` and does not read, so the caller reads only its range.
    /// A failed read keeps nothing and does not close the set.
    pub(super) fn get_or_read(
        &self,
        id: &Id,
        read: impl FnOnce() -> RusticResult<Bytes>,
    ) -> Option<RusticResult<Bytes>> {
        let mut state = self
            .read_ended
            .wait_while(self.state(), |state| {
                want(state, id, self.limit) == Want::Wait
            })
            .unwrap_or_else(PoisonError::into_inner);
        match want(&state, id, self.limit) {
            Want::Kept(pack) => return Some(Ok(pack.clone())),
            Want::Skip => return None,
            Want::Wait | Want::Read => {}
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
        Some(read)
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
    /// Keeps the pack when it fits in the limit, and closes the set when it does not.
    fn keep(&self, pack: &Bytes) {
        let mut state = self.kept.state();
        match admit(state.bytes, pack.len(), self.kept.limit) {
            Admit::Keep { bytes } => {
                state.bytes = bytes;
                state.packs.insert(self.id, pack.clone());
            }
            Admit::Close => state.closed = true,
        }
    }
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        self.kept.state().reading.remove(&self.id);
        self.kept.read_ended.notify_all();
    }
}

#[cfg(test)]
mod tests;
