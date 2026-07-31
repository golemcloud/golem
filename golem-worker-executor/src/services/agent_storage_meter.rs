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

//! Per-agent byte-second metering. Clones share one meter; dropping the last clone records any
//! whole byte-seconds accumulated since the previous flush.

use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::agent::AgentMode;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct AgentStorageMeter {
    inner: Arc<Inner>,
}

impl AgentStorageMeter {
    /// Whether both handles refer to the same underlying meter.
    ///
    /// Deliberately not `PartialEq`: this is identity, not value equality — two meters
    /// holding identical byte counts are still different meters, and unregistration must
    /// only remove the exact handle it was given.
    pub fn is_same_meter(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

#[derive(Debug)]
struct Inner {
    mode: AgentMode,
    entry: Weak<AtomicResourceEntry>,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    bytes: u64,
    last_sample: Instant,
    pending_byte_nanoseconds: u128,
}

impl AgentStorageMeter {
    pub fn new(mode: AgentMode, bytes: u64, entry: Arc<AtomicResourceEntry>, now: Instant) -> Self {
        Self {
            inner: Arc::new(Inner {
                mode,
                entry: Arc::downgrade(&entry),
                state: Mutex::new(State {
                    bytes,
                    last_sample: now,
                    pending_byte_nanoseconds: 0,
                }),
            }),
        }
    }

    pub fn on_acquire(&self, bytes: u64, now: Instant) {
        self.inner.update_bytes(bytes, true, now);
    }

    pub fn on_release(&self, bytes: u64, now: Instant) {
        self.inner.update_bytes(bytes, false, now);
    }

    pub fn flush(&self, now: Instant) {
        self.inner.integrate(now);
    }
}

impl Inner {
    fn update_bytes(&self, bytes: u64, acquire: bool, now: Instant) {
        let byte_seconds = {
            let mut state = self.state.lock().unwrap();
            let byte_seconds = state.take_whole_byte_seconds(now);
            state.bytes = if acquire {
                state.bytes.saturating_add(bytes)
            } else {
                state.bytes.saturating_sub(bytes)
            };
            byte_seconds
        };
        self.record(byte_seconds);
    }

    fn integrate(&self, now: Instant) {
        let byte_seconds = self.state.lock().unwrap().take_whole_byte_seconds(now);
        self.record(byte_seconds);
    }

    fn record(&self, byte_seconds: i64) {
        if byte_seconds == 0 {
            return;
        }
        if let Some(entry) = self.entry.upgrade() {
            entry.record_storage_byte_seconds(self.mode, byte_seconds);
        }
    }
}

impl State {
    fn take_whole_byte_seconds(&mut self, now: Instant) -> i64 {
        if now <= self.last_sample {
            return 0;
        }

        let elapsed_nanoseconds = now.saturating_duration_since(self.last_sample).as_nanos();
        self.last_sample = now;
        if self.bytes == 0 {
            return 0;
        }
        self.pending_byte_nanoseconds = self
            .pending_byte_nanoseconds
            .saturating_add((self.bytes as u128).saturating_mul(elapsed_nanoseconds));
        if self.pending_byte_nanoseconds < 1_000_000_000 {
            return 0;
        }

        let byte_seconds = self.pending_byte_nanoseconds / 1_000_000_000;
        self.pending_byte_nanoseconds %= 1_000_000_000;
        byte_seconds.min(i64::MAX as u128) as i64
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.integrate(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::time::Duration;
    use test_r::test;

    #[test]
    fn integrates_acquire_release_and_flush() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 10, entry.clone(), now);

        meter.on_acquire(5, now + Duration::from_secs(2));
        meter.on_release(3, now + Duration::from_secs(4));
        meter.flush(now + Duration::from_secs(5));

        assert_eq!(entry.durable_byte_seconds_delta(), 62);
    }

    #[test]
    fn meters_ephemeral_storage_separately() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Ephemeral, 10, entry.clone(), now);

        meter.flush(now + Duration::from_secs(3));

        assert_eq!(entry.ephemeral_byte_seconds_delta(), 30);
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
    }

    #[test]
    fn ignores_a_stale_flush_timestamp() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 10, entry.clone(), now);

        meter.on_acquire(5, now + Duration::from_secs(2));
        meter.flush(now + Duration::from_secs(1));
        meter.flush(now + Duration::from_secs(4));

        assert_eq!(entry.durable_byte_seconds_delta(), 50);
    }

    #[test]
    fn retains_sub_byte_second_remainder_without_division() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 1024, entry.clone(), now);

        meter.flush(now + Duration::from_micros(1));
        assert_eq!(entry.durable_byte_seconds_delta(), 0);

        meter.flush(now + Duration::from_secs(1));
        assert_eq!(entry.durable_byte_seconds_delta(), 1024);
    }

    #[test]
    fn cloned_meter_flushes_shared_state() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 0, entry.clone(), now);
        let flusher = meter.clone();

        meter.on_acquire(10, now + Duration::from_secs(1));
        flusher.flush(now + Duration::from_secs(3));

        assert_eq!(entry.durable_byte_seconds_delta(), 20);
    }

    proptest! {
        #[test]
        fn integrates_arbitrary_monotonic_storage_changes(
            operations in prop::collection::vec((0u8..3, 0u64..1024, 1u64..5), 1..100),
        ) {
            let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
            let mut now = Instant::now();
            let meter = AgentStorageMeter::new(AgentMode::Durable, 10, entry.clone(), now);
            let mut bytes = 10u64;
            let mut expected = 0u64;

            for (operation, amount, elapsed_seconds) in operations {
                now += Duration::from_secs(elapsed_seconds);
                expected += bytes * elapsed_seconds;

                match operation {
                    0 => {
                        meter.on_acquire(amount, now);
                        bytes = bytes.saturating_add(amount);
                    }
                    1 => {
                        meter.on_release(amount, now);
                        bytes = bytes.saturating_sub(amount);
                    }
                    _ => meter.flush(now),
                }
            }

            prop_assert_eq!(entry.durable_byte_seconds_delta(), expected as i64);
        }
    }
}
