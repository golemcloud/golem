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

use crate::services::active_workers::{
    bytes_to_filesystem_storage_permits, filesystem_storage_permits_to_bytes,
};
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
    reservation_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug)]
struct State {
    bytes: u64,
    reserved_bytes: u64,
    active: bool,
    stopped: bool,
    last_sample: Instant,
    pending_byte_nanoseconds: u128,
}

impl AgentStorageMeter {
    pub fn new(
        mode: AgentMode,
        bytes: u64,
        active: bool,
        entry: Arc<AtomicResourceEntry>,
        now: Instant,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                mode,
                entry: Arc::downgrade(&entry),
                reservation_lock: Arc::new(tokio::sync::Mutex::new(())),
                state: Mutex::new(State {
                    bytes,
                    reserved_bytes: 0,
                    active,
                    stopped: false,
                    last_sample: now,
                    pending_byte_nanoseconds: 0,
                }),
            }),
        }
    }

    pub fn current_bytes(&self) -> u64 {
        self.inner.state.lock().unwrap().bytes
    }

    pub fn reserve(&self, bytes: u64, limit: u64) -> Option<u64> {
        let mut state = self.inner.state.lock().unwrap();
        let before = state.bytes.saturating_add(state.reserved_bytes);
        let after = before.saturating_add(bytes);
        if after > limit {
            return None;
        }
        let host_bytes = if state.reserved_bytes == 0 {
            host_capacity_delta(before, after)
        } else {
            host_capacity_delta(0, bytes)
        };
        state.reserved_bytes = state.reserved_bytes.saturating_add(bytes);
        Some(host_bytes)
    }

    pub fn extend_reservation(
        &self,
        reservation_bytes: u64,
        additional_bytes: u64,
        limit: u64,
    ) -> Option<u64> {
        let mut state = self.inner.state.lock().unwrap();
        let before = state.bytes.saturating_add(state.reserved_bytes);
        let after = before.saturating_add(additional_bytes);
        if after > limit {
            return None;
        }
        let other_reserved_bytes = state.reserved_bytes.saturating_sub(reservation_bytes);
        let host_bytes = if other_reserved_bytes == 0 {
            host_capacity_delta(before, after)
        } else {
            host_capacity_delta(
                reservation_bytes,
                reservation_bytes.saturating_add(additional_bytes),
            )
        };
        state.reserved_bytes = state.reserved_bytes.saturating_add(additional_bytes);
        Some(host_bytes)
    }

    pub fn commit_reservation(&self, reserved_bytes: u64, committed_bytes: u64, now: Instant) {
        self.inner.transition(now, |state| {
            state.reserved_bytes = state.reserved_bytes.saturating_sub(reserved_bytes);
            state.bytes = state
                .bytes
                .saturating_add(committed_bytes.min(reserved_bytes));
        });
    }

    pub fn rollback_reservation(&self, reserved_bytes: u64) {
        let mut state = self.inner.state.lock().unwrap();
        state.reserved_bytes = state.reserved_bytes.saturating_sub(reserved_bytes);
    }

    pub fn shrink_reservation(&self, reserved_bytes: u64) -> u64 {
        let mut state = self.inner.state.lock().unwrap();
        let reserved_bytes = reserved_bytes.min(state.reserved_bytes);
        let before = state.bytes.saturating_add(state.reserved_bytes);
        state.reserved_bytes = state.reserved_bytes.saturating_sub(reserved_bytes);
        let after = state.bytes.saturating_add(state.reserved_bytes);
        if state.reserved_bytes == 0 {
            host_capacity_delta(after, before)
        } else {
            0
        }
    }

    pub fn host_capacity_for_growth(&self, bytes: u64) -> u64 {
        let current = self.current_bytes();
        host_capacity_delta(current, current.saturating_add(bytes))
    }

    pub fn host_capacity_for_release(&self, bytes: u64) -> u64 {
        let state = self.inner.state.lock().unwrap();
        let before = state.bytes.saturating_add(state.reserved_bytes);
        let after = state
            .bytes
            .saturating_sub(bytes)
            .saturating_add(state.reserved_bytes);
        host_capacity_delta(after, before)
    }

    pub async fn lock_reservation(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.inner.reservation_lock.clone().lock_owned().await
    }

    pub fn on_acquire(&self, bytes: u64, now: Instant) {
        self.inner.update_bytes(bytes, true, now);
    }

    pub fn on_release(&self, bytes: u64, now: Instant) {
        self.inner.update_bytes(bytes, false, now);
    }

    pub fn resume(&self, now: Instant) {
        self.inner.transition(now, |state| {
            if !state.stopped {
                state.active = true;
            }
        });
    }

    pub fn pause(&self, now: Instant) {
        self.inner.transition(now, |state| state.active = false);
    }

    pub fn stop(&self, now: Instant) {
        let byte_seconds = {
            let mut state = self.inner.state.lock().unwrap();
            let byte_seconds = state.take_whole_byte_seconds(now);
            state.active = false;
            state.stopped = true;
            byte_seconds
        };
        self.inner.record(byte_seconds);
    }

    pub fn flush(&self, now: Instant) {
        self.inner.integrate(now);
    }
}

pub(crate) fn host_capacity_delta(smaller: u64, larger: u64) -> u64 {
    let smaller = bytes_to_filesystem_storage_permits(smaller);
    let larger = bytes_to_filesystem_storage_permits(larger);
    filesystem_storage_permits_to_bytes(larger.saturating_sub(smaller))
}

impl Inner {
    fn transition<R>(&self, now: Instant, update: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.state.lock().unwrap();
        let byte_seconds = state.take_whole_byte_seconds(now);
        let result = update(&mut state);
        drop(state);
        self.record(byte_seconds);
        result
    }

    fn update_bytes(&self, bytes: u64, acquire: bool, now: Instant) {
        self.transition(now, |state| {
            if state.stopped {
                return;
            }
            state.bytes = if acquire {
                state.bytes.saturating_add(bytes)
            } else {
                state.bytes.saturating_sub(bytes)
            };
        });
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
        if !self.active || self.stopped || self.bytes == 0 {
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
        let meter = AgentStorageMeter::new(AgentMode::Durable, 10, true, entry.clone(), now);

        meter.on_acquire(5, now + Duration::from_secs(2));
        meter.on_release(3, now + Duration::from_secs(4));
        meter.flush(now + Duration::from_secs(5));

        assert_eq!(entry.durable_byte_seconds_delta(), 62);
    }

    #[test]
    fn meters_ephemeral_storage_separately() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Ephemeral, 10, true, entry.clone(), now);

        meter.flush(now + Duration::from_secs(3));

        assert_eq!(entry.ephemeral_byte_seconds_delta(), 30);
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
    }

    #[test]
    fn ignores_a_stale_flush_timestamp() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 10, true, entry.clone(), now);

        meter.on_acquire(5, now + Duration::from_secs(2));
        meter.flush(now + Duration::from_secs(1));
        meter.flush(now + Duration::from_secs(4));

        assert_eq!(entry.durable_byte_seconds_delta(), 50);
    }

    #[test]
    fn retains_sub_byte_second_remainder_without_division() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 1024, true, entry.clone(), now);

        meter.flush(now + Duration::from_micros(1));
        assert_eq!(entry.durable_byte_seconds_delta(), 0);

        meter.flush(now + Duration::from_secs(1));
        assert_eq!(entry.durable_byte_seconds_delta(), 1024);
    }

    #[test]
    fn cloned_meter_flushes_shared_state() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 0, true, entry.clone(), now);
        let flusher = meter.clone();

        meter.on_acquire(10, now + Duration::from_secs(1));
        flusher.flush(now + Duration::from_secs(3));

        assert_eq!(entry.durable_byte_seconds_delta(), 20);
    }

    #[test]
    fn pause_resume_and_stop_are_idempotent() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 10, true, entry.clone(), now);

        meter.pause(now + Duration::from_secs(2));
        meter.pause(now + Duration::from_secs(3));
        meter.resume(now + Duration::from_secs(4));
        meter.resume(now + Duration::from_secs(5));
        meter.stop(now + Duration::from_secs(7));
        meter.stop(now + Duration::from_secs(8));
        meter.resume(now + Duration::from_secs(9));
        meter.flush(now + Duration::from_secs(10));

        assert_eq!(entry.durable_byte_seconds_delta(), 50);
    }

    #[test]
    fn paused_storage_changes_are_prospective() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 10, true, entry.clone(), now);

        meter.pause(now + Duration::from_secs(1));
        meter.on_acquire(10, now + Duration::from_secs(5));
        meter.resume(now + Duration::from_secs(10));
        meter.flush(now + Duration::from_secs(12));

        assert_eq!(meter.current_bytes(), 20);
        assert_eq!(entry.durable_byte_seconds_delta(), 50);
    }

    #[test]
    fn host_capacity_is_derived_from_canonical_total() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 500, true, entry, now);

        assert_eq!(meter.host_capacity_for_growth(500), 0);
        assert_eq!(meter.host_capacity_for_growth(600), 1024);

        meter.on_acquire(600, now);
        assert_eq!(meter.host_capacity_for_release(100), 1024);
        assert_eq!(meter.host_capacity_for_release(50), 0);
    }

    #[test]
    fn outstanding_reservations_count_toward_quota_without_being_billed() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 4, true, entry.clone(), now);

        assert_eq!(meter.reserve(4, 10), Some(0));
        assert_eq!(meter.reserve(3, 10), None);
        assert_eq!(meter.current_bytes(), 4);
        meter.flush(now + Duration::from_secs(2));
        assert_eq!(entry.durable_byte_seconds_delta(), 8);

        meter.commit_reservation(4, 3, now + Duration::from_secs(2));
        assert_eq!(meter.current_bytes(), 7);
        assert_eq!(meter.reserve(3, 10), Some(0));
        meter.flush(now + Duration::from_secs(4));
        assert_eq!(entry.durable_byte_seconds_delta(), 22);
        meter.rollback_reservation(3);
        meter.flush(now + Duration::from_secs(5));
        assert_eq!(meter.current_bytes(), 7);
        assert_eq!(entry.durable_byte_seconds_delta(), 29);
    }

    #[test]
    fn extending_one_reservation_only_acquires_new_rounding_boundaries() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 0, true, entry, now);

        assert_eq!(meter.reserve(4, 2048), Some(1024));
        assert_eq!(meter.extend_reservation(4, 4, 2048), Some(0));
        assert_eq!(meter.extend_reservation(8, 1020, 2048), Some(1024));
        assert_eq!(meter.extend_reservation(1028, 1021, 2048), None);
    }

    #[test]
    fn overlapping_reservations_own_independent_host_capacity() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 500, true, entry, now);

        assert_eq!(meter.reserve(600, 4096), Some(1024));
        assert_eq!(meter.reserve(600, 4096), Some(1024));
        meter.rollback_reservation(600);
        meter.commit_reservation(600, 600, now);
        assert_eq!(meter.current_bytes(), 1100);
    }

    #[test]
    fn release_preserves_capacity_used_by_an_outstanding_reservation() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 1025, true, entry, now);

        assert_eq!(meter.reserve(1023, 4096), Some(0));
        assert_eq!(meter.host_capacity_for_release(1025), 1024);
        meter.on_release(1025, now);
        meter.commit_reservation(1023, 1023, now);
        assert_eq!(meter.current_bytes(), 1023);
    }

    #[test]
    fn shrinking_a_reservation_releases_newly_excess_capacity() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 1025, true, entry, now);

        assert_eq!(meter.reserve(1023, 4096), Some(0));
        assert_eq!(meter.host_capacity_for_release(1025), 1024);
        meter.on_release(1025, now);
        assert_eq!(meter.shrink_reservation(1023), 1024);
    }

    #[test]
    fn shrinking_defers_capacity_release_while_other_reservations_exist() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 1025, true, entry, now);

        assert_eq!(meter.reserve(1023, 4096), Some(0));
        assert_eq!(meter.reserve(1, 4096), Some(1024));
        assert_eq!(meter.shrink_reservation(1023), 0);
    }

    proptest! {
        #[test]
        fn integrates_arbitrary_monotonic_storage_changes(
            operations in prop::collection::vec((0u8..3, 0u64..1024, 1u64..5), 1..100),
        ) {
            let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
            let mut now = Instant::now();
            let meter = AgentStorageMeter::new(AgentMode::Durable, 10, true, entry.clone(), now);
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
