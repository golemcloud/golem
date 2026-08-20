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

use crate::services::byte_time_accumulator::{ByteTimeAccumulator, ByteTimeSettlement};
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::agent::AgentMode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

const BYTE_NANOSECONDS_PER_BYTE_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug)]
pub(crate) struct FilesystemUsageObservation {
    pub(crate) generation: u64,
    pub(crate) sequence: u64,
}

/// Leaf meter for authoritative filesystem-storage levels and byte-time integration.
///
/// `AgentResourceBilling` invokes lifecycle methods under the transition lock shared with
/// memory accounting. Observation generations and sequences reject stale asynchronous results.
#[derive(Debug)]
pub(crate) struct AgentStorageMeter {
    mode: AgentMode,
    entry: Weak<AtomicResourceEntry>,
    generation: AtomicU64,
    sequence: AtomicU64,
    state: Mutex<AgentStorageMeterState>,
}

#[derive(Debug)]
struct AgentStorageMeterState {
    active: bool,
    closing: bool,
    allocated_bytes: Option<u64>,
    generation: u64,
    applied_sequence: u64,
    usage: ByteTimeAccumulator,
}

impl AgentStorageMeter {
    pub(crate) fn new(mode: AgentMode, entry: Arc<AtomicResourceEntry>, now: Instant) -> Self {
        Self {
            mode,
            entry: Arc::downgrade(&entry),
            generation: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            state: Mutex::new(AgentStorageMeterState {
                active: false,
                closing: false,
                allocated_bytes: None,
                generation: 0,
                applied_sequence: 0,
                usage: ByteTimeAccumulator::new(BYTE_NANOSECONDS_PER_BYTE_SECOND, now),
            }),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.state.lock().unwrap().active
    }

    pub(crate) fn open(&self, allocated_bytes: Option<u64>, now: Instant) {
        let generation = self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .expect("resource-window generation overflowed")
            + 1;
        let sequence = self.next_sequence();
        let mut state = self.state.lock().unwrap();
        state.accrue(now);
        state.active = true;
        state.closing = false;
        state.allocated_bytes = allocated_bytes;
        state.generation = generation;
        state.applied_sequence = sequence;
    }

    pub(crate) fn begin_observation(&self) -> FilesystemUsageObservation {
        FilesystemUsageObservation {
            generation: self.generation.load(Ordering::Acquire),
            sequence: self.next_sequence(),
        }
    }

    pub(crate) fn begin_close(&self) -> Option<FilesystemUsageObservation> {
        let mut state = self.state.lock().unwrap();
        if !state.active || state.closing {
            return None;
        }
        state.closing = true;
        Some(FilesystemUsageObservation {
            generation: state.generation,
            sequence: self.next_sequence(),
        })
    }

    pub(crate) fn complete_observation(
        &self,
        observation: FilesystemUsageObservation,
        allocated_bytes: Option<u64>,
        now: Instant,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        let accepted = state.active
            && !state.closing
            && observation.generation == state.generation
            && observation.sequence > state.applied_sequence;
        if accepted {
            state.accrue(now);
            state.allocated_bytes = allocated_bytes;
            state.applied_sequence = observation.sequence;
        }
        accepted
    }

    pub(crate) fn close(
        &self,
        observation: FilesystemUsageObservation,
        allocated_bytes: Option<u64>,
        now: Instant,
    ) -> Option<ByteTimeSettlement> {
        let mut state = self.state.lock().unwrap();
        let accepted = state.active && state.closing && observation.generation == state.generation;
        if !accepted {
            return None;
        }
        state.accrue(now);
        state.allocated_bytes = allocated_bytes;
        state.applied_sequence = state.applied_sequence.max(observation.sequence);
        state.active = false;
        state.closing = false;
        Some(state.usage.take_settlement())
    }

    pub(crate) fn fail_observation(
        &self,
        observation: FilesystemUsageObservation,
    ) -> Option<ByteTimeSettlement> {
        let mut state = self.state.lock().unwrap();
        let accepted = state.active
            && !state.closing
            && observation.generation == state.generation
            && observation.sequence > state.applied_sequence;
        accepted.then(|| {
            state.active = false;
            state.usage.take_settlement()
        })
    }

    pub(crate) fn flush(&self, now: Instant) -> i64 {
        let mut state = self.state.lock().unwrap();
        state.accrue(now);
        state.usage.take_units()
    }

    pub(crate) fn abort(&self) -> Option<ByteTimeSettlement> {
        let mut state = self.state.lock().unwrap();
        state.active.then(|| {
            state.active = false;
            state.closing = false;
            state.usage.take_settlement()
        })
    }

    fn next_sequence(&self) -> u64 {
        self.sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .expect("filesystem usage observation sequence overflowed")
            + 1
    }

    fn record_settlement(&self, settlement: ByteTimeSettlement) {
        if let Some(entry) = self.entry.upgrade() {
            entry.record_storage_settlement(self.mode, settlement);
        }
    }
}

impl AgentStorageMeterState {
    fn accrue(&mut self, now: Instant) {
        let allocated_bytes = self.active.then_some(self.allocated_bytes).flatten();
        self.usage.accrue(now, allocated_bytes);
    }
}

impl Drop for AgentStorageMeter {
    fn drop(&mut self) {
        let settlement = self.state.get_mut().unwrap().usage.take_settlement();
        self.record_settlement(settlement);
    }
}
