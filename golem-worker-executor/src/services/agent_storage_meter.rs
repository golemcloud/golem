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

//! Canonical per-agent filesystem storage accounting. Clones share committed and reserved byte
//! counts, executor-capacity rounding, mutation serialization, and byte-second metering.

use crate::services::active_workers::{
    FilesystemStoragePermit, bytes_to_filesystem_storage_permits,
    filesystem_storage_permits_to_bytes,
};
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::agent::AgentMode;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct AgentStorageMeter {
    inner: Arc<Inner>,
}

#[derive(Debug)]
pub struct StorageAccountingGuard {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

#[derive(Debug, Default)]
pub struct FilesystemStoragePermitBank {
    state: Mutex<FilesystemStoragePermitBankState>,
}

#[derive(Debug, Default)]
struct FilesystemStoragePermitBankState {
    permit: Option<FilesystemStoragePermit>,
    generation: Option<Arc<()>>,
}

pub struct FilesystemStoragePermitRegistration {
    bank: Arc<FilesystemStoragePermitBank>,
    generation: Arc<()>,
}

impl Drop for FilesystemStoragePermitRegistration {
    fn drop(&mut self) {
        let mut state = self.bank.state.lock().unwrap();
        if state
            .generation
            .as_ref()
            .is_some_and(|generation| Arc::ptr_eq(generation, &self.generation))
        {
            state.generation.take();
            state.permit.take();
        }
    }
}

impl FilesystemStoragePermitBank {
    pub fn install(
        self: &Arc<Self>,
        permit: Option<FilesystemStoragePermit>,
    ) -> FilesystemStoragePermitRegistration {
        let generation = Arc::new(());
        let mut state = self.state.lock().unwrap();
        assert!(
            state.generation.is_none(),
            "filesystem storage permit bank is already installed"
        );
        state.permit = permit;
        state.generation = Some(generation.clone());
        FilesystemStoragePermitRegistration {
            bank: self.clone(),
            generation,
        }
    }

    fn generation(&self) -> Option<Weak<()>> {
        self.state
            .lock()
            .unwrap()
            .generation
            .as_ref()
            .map(Arc::downgrade)
    }

    fn is_current_generation(
        state: &FilesystemStoragePermitBankState,
        generation: &Option<Weak<()>>,
    ) -> bool {
        match (&state.generation, generation) {
            (None, None) => true,
            (Some(current), Some(expected)) => expected
                .upgrade()
                .is_some_and(|expected| Arc::ptr_eq(current, &expected)),
            _ => false,
        }
    }

    fn merge(&self, generation: &Option<Weak<()>>, permit: Option<FilesystemStoragePermit>) {
        let Some(permit) = permit else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if !Self::is_current_generation(&state, generation) {
            return;
        }
        match &mut state.permit {
            Some(existing) => existing.merge(permit),
            None => state.permit = Some(permit),
        }
    }

    fn reconcile(&self, generation: &Option<Weak<()>>, bytes: u64) {
        let target = bytes_to_filesystem_storage_permits(bytes) as usize;
        let mut state = self.state.lock().unwrap();
        if !Self::is_current_generation(&state, generation) {
            return;
        }
        let held = state
            .permit
            .as_ref()
            .map_or(0, FilesystemStoragePermit::num_permits);
        let excess = held.saturating_sub(target);
        if excess > 0
            && let Some(permit) = state.permit.as_mut()
        {
            drop(permit.split(excess));
        }
        if state
            .permit
            .as_ref()
            .is_some_and(|permit| permit.num_permits() == 0)
        {
            state.permit.take();
        }
    }

    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.generation.take();
        state.permit.take();
    }

    fn held_bytes_for(&self, generation: &Option<Weak<()>>) -> Option<u64> {
        let state = self.state.lock().unwrap();
        Self::is_current_generation(&state, generation).then(|| {
            filesystem_storage_permits_to_bytes(
                state
                    .permit
                    .as_ref()
                    .map_or(0, FilesystemStoragePermit::num_permits) as u32,
            )
        })
    }

    pub fn held_bytes(&self) -> u64 {
        filesystem_storage_permits_to_bytes(
            self.state
                .lock()
                .unwrap()
                .permit
                .as_ref()
                .map_or(0, FilesystemStoragePermit::num_permits) as u32,
        )
    }
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
    capacity: Arc<FilesystemStoragePermitBank>,
    capacity_generation: Option<Weak<()>>,
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
    pub fn new(mode: AgentMode, bytes: u64, entry: Arc<AtomicResourceEntry>, now: Instant) -> Self {
        Self::new_with_capacity_bank(
            mode,
            bytes,
            entry,
            now,
            Arc::new(FilesystemStoragePermitBank::default()),
        )
    }

    pub fn new_with_capacity_bank(
        mode: AgentMode,
        bytes: u64,
        entry: Arc<AtomicResourceEntry>,
        now: Instant,
        capacity: Arc<FilesystemStoragePermitBank>,
    ) -> Self {
        let capacity_generation = capacity.generation();
        Self {
            inner: Arc::new(Inner {
                mode,
                entry: Arc::downgrade(&entry),
                reservation_lock: Arc::new(tokio::sync::Mutex::new(())),
                capacity,
                capacity_generation,
                state: Mutex::new(State {
                    bytes,
                    reserved_bytes: 0,
                    active: true,
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
        let host_bytes = host_capacity_delta(before, after);
        state.reserved_bytes = state.reserved_bytes.saturating_add(bytes);
        Some(host_bytes)
    }

    pub fn extend_reservation(
        &self,
        _reservation_bytes: u64,
        additional_bytes: u64,
        limit: u64,
    ) -> Option<u64> {
        let mut state = self.inner.state.lock().unwrap();
        let before = state.bytes.saturating_add(state.reserved_bytes);
        let after = before.saturating_add(additional_bytes);
        if after > limit {
            return None;
        }
        let host_bytes = host_capacity_delta(before, after);
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
        self.reconcile_capacity();
    }

    pub fn rollback_reservation(&self, reserved_bytes: u64) {
        let mut state = self.inner.state.lock().unwrap();
        state.reserved_bytes = state.reserved_bytes.saturating_sub(reserved_bytes);
        let total = state.bytes.saturating_add(state.reserved_bytes);
        drop(state);
        self.inner
            .capacity
            .reconcile(&self.inner.capacity_generation, total);
    }

    pub fn shrink_reservation(&self, reserved_bytes: u64) {
        let mut state = self.inner.state.lock().unwrap();
        let reserved_bytes = reserved_bytes.min(state.reserved_bytes);
        state.reserved_bytes = state.reserved_bytes.saturating_sub(reserved_bytes);
        let total = state.bytes.saturating_add(state.reserved_bytes);
        drop(state);
        self.inner
            .capacity
            .reconcile(&self.inner.capacity_generation, total);
    }

    pub async fn lock_reservation(&self) -> StorageAccountingGuard {
        StorageAccountingGuard {
            _guard: self.inner.reservation_lock.clone().lock_owned().await,
        }
    }

    pub fn on_acquire(&self, bytes: u64, now: Instant) {
        self.inner.update_bytes(bytes, true, now);
    }

    pub fn on_release(&self, bytes: u64, now: Instant) {
        self.inner.update_bytes(bytes, false, now);
        self.reconcile_capacity();
    }

    pub fn merge_capacity(&self, permit: Option<FilesystemStoragePermit>) {
        self.inner
            .capacity
            .merge(&self.inner.capacity_generation, permit);
        self.reconcile_capacity();
    }

    pub fn capacity_shortfall(&self) -> u64 {
        let state = self.inner.state.lock().unwrap();
        let target = filesystem_storage_permits_to_bytes(bytes_to_filesystem_storage_permits(
            state.bytes.saturating_add(state.reserved_bytes),
        ));
        drop(state);
        self.inner
            .capacity
            .held_bytes_for(&self.inner.capacity_generation)
            .map_or(0, |held| target.saturating_sub(held))
    }

    fn reconcile_capacity(&self) {
        let state = self.inner.state.lock().unwrap();
        let total = state.bytes.saturating_add(state.reserved_bytes);
        drop(state);
        self.inner
            .capacity
            .reconcile(&self.inner.capacity_generation, total);
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
    use crate::services::active_workers::FilesystemStorageSemaphore;
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

    #[test]
    fn pause_resume_and_stop_are_idempotent() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 10, entry.clone(), now);

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
        let meter = AgentStorageMeter::new(AgentMode::Durable, 10, entry.clone(), now);

        meter.pause(now + Duration::from_secs(1));
        meter.on_acquire(10, now + Duration::from_secs(5));
        meter.resume(now + Duration::from_secs(10));
        meter.flush(now + Duration::from_secs(12));

        assert_eq!(meter.current_bytes(), 20);
        assert_eq!(entry.durable_byte_seconds_delta(), 50);
    }

    #[test]
    fn outstanding_reservations_count_toward_quota_without_being_billed() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentStorageMeter::new(AgentMode::Durable, 4, entry.clone(), now);

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
        let meter = AgentStorageMeter::new(AgentMode::Durable, 0, entry, now);

        assert_eq!(meter.reserve(4, 2048), Some(1024));
        assert_eq!(meter.extend_reservation(4, 4, 2048), Some(0));
        assert_eq!(meter.extend_reservation(8, 1020, 2048), Some(1024));
        assert_eq!(meter.extend_reservation(1028, 1021, 2048), None);
    }

    async fn acquire_capacity(
        meter: &AgentStorageMeter,
        semaphore: &FilesystemStorageSemaphore,
        bytes: u64,
    ) {
        if bytes > 0 {
            meter.merge_capacity(semaphore.try_acquire(bytes).await);
        }
    }

    fn merge_current_capacity(
        capacity: &FilesystemStoragePermitBank,
        permit: Option<FilesystemStoragePermit>,
    ) {
        capacity.merge(&capacity.generation(), permit);
    }

    fn current_capacity_bytes(capacity: &FilesystemStoragePermitBank) -> u64 {
        capacity
            .held_bytes_for(&capacity.generation())
            .expect("capacity generation must remain current")
    }

    #[test]
    async fn stale_generation_drop_does_not_clear_replacement_capacity() {
        let semaphore = FilesystemStorageSemaphore::new(1024, Duration::from_millis(1));
        let capacity = Arc::new(FilesystemStoragePermitBank::default());
        let first_generation = capacity.install(None);
        capacity.clear();
        let second_generation = capacity.install(semaphore.try_acquire(1).await);

        drop(first_generation);
        assert_eq!(current_capacity_bytes(&capacity), 1024);
        assert_eq!(semaphore.available_bytes(), 0);

        drop(second_generation);
        assert_eq!(semaphore.available_bytes(), 1024);
    }

    #[test]
    async fn stale_meter_does_not_reconcile_replacement_capacity() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let semaphore = FilesystemStorageSemaphore::new(2 * 1024, Duration::from_millis(1));
        let capacity = Arc::new(FilesystemStoragePermitBank::default());
        let first_generation = capacity.install(semaphore.try_acquire(1024).await);
        let stale_meter = AgentStorageMeter::new_with_capacity_bank(
            AgentMode::Durable,
            1024,
            entry,
            Instant::now(),
            capacity.clone(),
        );
        assert_eq!(stale_meter.reserve(1024, 4096), Some(1024));

        capacity.clear();
        let replacement_generation = capacity.install(semaphore.try_acquire(1024).await);
        stale_meter.rollback_reservation(1024);
        stale_meter.merge_capacity(semaphore.try_acquire(1024).await);

        assert_eq!(current_capacity_bytes(&capacity), 1024);
        assert_eq!(semaphore.available_bytes(), 1024);
        drop(first_generation);
        drop(replacement_generation);
    }

    #[test]
    async fn rollback_recomputes_an_in_flight_reservations_capacity_shortfall() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let semaphore = FilesystemStorageSemaphore::new(2 * 1024, Duration::from_millis(1));
        let capacity = Arc::new(FilesystemStoragePermitBank::default());
        merge_current_capacity(&capacity, semaphore.try_acquire(500).await);
        let meter = AgentStorageMeter::new_with_capacity_bank(
            AgentMode::Durable,
            500,
            entry,
            Instant::now(),
            capacity,
        );
        let first = meter.reserve(600, 4096).unwrap();
        acquire_capacity(&meter, &semaphore, first).await;
        assert_eq!(meter.reserve(1000, 4096), Some(1024));
        assert_eq!(meter.capacity_shortfall(), 1024);

        meter.rollback_reservation(600);
        assert_eq!(meter.capacity_shortfall(), 0);
    }

    #[test]
    async fn overlapping_reservations_use_aggregate_host_capacity() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let semaphore = FilesystemStorageSemaphore::new(3 * 1024, Duration::from_millis(1));
        let capacity = Arc::new(FilesystemStoragePermitBank::default());
        merge_current_capacity(&capacity, semaphore.try_acquire(500).await);
        let meter = AgentStorageMeter::new_with_capacity_bank(
            AgentMode::Durable,
            500,
            entry,
            now,
            capacity.clone(),
        );

        let first = meter.reserve(600, 4096).unwrap();
        acquire_capacity(&meter, &semaphore, first).await;
        let second = meter.reserve(600, 4096).unwrap();
        acquire_capacity(&meter, &semaphore, second).await;

        assert_eq!(first, 1024);
        assert_eq!(second, 0);
        assert_eq!(current_capacity_bytes(&capacity), 2048);
        meter.rollback_reservation(600);
        assert_eq!(current_capacity_bytes(&capacity), 2048);
        meter.commit_reservation(600, 600, now);
        assert_eq!(meter.current_bytes(), 1100);
        assert_eq!(current_capacity_bytes(&capacity), 2048);
        assert_eq!(semaphore.available_bytes(), 1024);
    }

    #[test]
    async fn release_preserves_capacity_used_by_an_outstanding_reservation() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let semaphore = FilesystemStorageSemaphore::new(3 * 1024, Duration::from_millis(1));
        let capacity = Arc::new(FilesystemStoragePermitBank::default());
        merge_current_capacity(&capacity, semaphore.try_acquire(1025).await);
        let meter = AgentStorageMeter::new_with_capacity_bank(
            AgentMode::Durable,
            1025,
            entry,
            now,
            capacity.clone(),
        );

        assert_eq!(meter.reserve(1023, 4096), Some(0));
        meter.on_release(1025, now);
        assert_eq!(current_capacity_bytes(&capacity), 1024);
        meter.commit_reservation(1023, 1023, now);
        assert_eq!(meter.current_bytes(), 1023);
        assert_eq!(current_capacity_bytes(&capacity), 1024);
    }

    #[test]
    async fn shrinking_a_reservation_releases_newly_excess_capacity() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let semaphore = FilesystemStorageSemaphore::new(2 * 1024, Duration::from_millis(1));
        let capacity = Arc::new(FilesystemStoragePermitBank::default());
        merge_current_capacity(&capacity, semaphore.try_acquire(1025).await);
        let meter = AgentStorageMeter::new_with_capacity_bank(
            AgentMode::Durable,
            1025,
            entry,
            now,
            capacity.clone(),
        );

        assert_eq!(meter.reserve(1023, 4096), Some(0));
        meter.on_release(1025, now);
        assert_eq!(current_capacity_bytes(&capacity), 1024);
        meter.shrink_reservation(1023);
        assert_eq!(current_capacity_bytes(&capacity), 0);
        assert_eq!(semaphore.available_bytes(), 2048);
    }

    #[test]
    async fn shrinking_reconciles_capacity_while_other_reservations_exist() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let semaphore = FilesystemStorageSemaphore::new(3 * 1024, Duration::from_millis(1));
        let capacity = Arc::new(FilesystemStoragePermitBank::default());
        merge_current_capacity(&capacity, semaphore.try_acquire(1025).await);
        let meter = AgentStorageMeter::new_with_capacity_bank(
            AgentMode::Durable,
            1025,
            entry,
            now,
            capacity.clone(),
        );

        assert_eq!(meter.reserve(1023, 4096), Some(0));
        let additional = meter.reserve(1, 4096).unwrap();
        acquire_capacity(&meter, &semaphore, additional).await;
        assert_eq!(current_capacity_bytes(&capacity), 3072);
        meter.shrink_reservation(1023);
        assert_eq!(current_capacity_bytes(&capacity), 2048);
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
