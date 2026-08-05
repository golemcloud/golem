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

use crate::services::agent_memory_meter::AgentMemoryMeter;
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::agent::AgentMode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct LinearMemoryTracker {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    bytes: AtomicU64,
    initially_reserved_bytes: u64,
    startup_bytes_remaining: AtomicU64,
    pending_growth_prepaid: AtomicU64,
    reconciling: AtomicBool,
    replaying: AtomicBool,
    transitions: Mutex<()>,
    meter: AgentMemoryMeter,
}

impl LinearMemoryTracker {
    pub fn new(
        bytes: u64,
        mode: AgentMode,
        replaying: bool,
        resource_entry: Arc<AtomicResourceEntry>,
        now: Instant,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                bytes: AtomicU64::new(bytes),
                initially_reserved_bytes: bytes,
                startup_bytes_remaining: AtomicU64::new(bytes),
                pending_growth_prepaid: AtomicU64::new(0),
                reconciling: AtomicBool::new(true),
                replaying: AtomicBool::new(replaying),
                transitions: Mutex::new(()),
                meter: AgentMemoryMeter::new(mode, bytes, true, resource_entry, now),
            }),
        }
    }

    pub fn current_bytes(&self) -> u64 {
        self.inner.bytes.load(Ordering::Acquire)
    }

    pub fn initially_reserved_bytes(&self) -> u64 {
        self.inner.initially_reserved_bytes
    }

    pub fn switch_to_live(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.replaying.store(false, Ordering::Release);
    }

    pub fn reconcile(&self, bytes: u64, now: Instant) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.meter.set_bytes(bytes, now);
        self.inner.bytes.store(bytes, Ordering::Release);
        self.inner
            .startup_bytes_remaining
            .store(0, Ordering::Release);
        self.inner
            .pending_growth_prepaid
            .store(0, Ordering::Release);
        self.inner.reconciling.store(false, Ordering::Release);
    }

    pub fn grow(&self, delta: u64, now: Instant) -> (u64, bool) {
        let _transition = self.inner.transitions.lock().unwrap();
        let prepaid = self
            .inner
            .pending_growth_prepaid
            .swap(0, Ordering::AcqRel)
            .min(delta);
        let bytes = self.current_bytes().saturating_add(delta - prepaid);
        self.inner.meter.set_bytes(bytes, now);
        self.inner.bytes.store(bytes, Ordering::Release);
        (bytes, self.inner.reconciling.load(Ordering::Acquire))
    }

    pub fn memory_grow_failed(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        let prepaid = self.inner.pending_growth_prepaid.swap(0, Ordering::AcqRel);
        self.inner
            .startup_bytes_remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                Some(remaining.saturating_add(prepaid))
            })
            .ok();
    }

    pub fn desired_total_after_unshared_growth(
        &self,
        current_memory: usize,
        desired_memory: usize,
    ) -> Option<(u64, u64)> {
        let _transition = self.inner.transitions.lock().unwrap();
        let delta = desired_memory.saturating_sub(current_memory) as u64;
        let reconciling = self.inner.reconciling.load(Ordering::Acquire);
        let prepaid = if reconciling
            && (current_memory == 0 || self.inner.replaying.load(Ordering::Acquire))
        {
            let remaining = self.inner.startup_bytes_remaining.load(Ordering::Acquire);
            let prepaid = delta.min(remaining);
            self.inner
                .startup_bytes_remaining
                .store(remaining - prepaid, Ordering::Release);
            prepaid
        } else {
            0
        };
        if current_memory != 0 {
            self.inner
                .pending_growth_prepaid
                .store(prepaid, Ordering::Release);
        }
        self.current_bytes()
            .checked_add(delta - prepaid)
            .map(|total| (delta, total))
    }

    pub fn resume(&self, now: Instant) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.meter.resume(self.current_bytes(), now);
    }

    pub fn pause(&self, now: Instant) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.meter.pause(now);
    }

    pub fn stop(&self, now: Instant) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.meter.stop(now);
    }

    pub fn meter(&self) -> &AgentMemoryMeter {
        &self.inner.meter
    }
}

pub fn desired_total_after_growth(
    current_total: u64,
    current_memory: usize,
    desired_memory: usize,
) -> Option<(u64, u64)> {
    let delta = desired_memory.saturating_sub(current_memory) as u64;
    current_total.checked_add(delta).map(|total| (delta, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::resource_limits::AtomicResourceEntry;
    use golem_common::model::agent::AgentMode;
    use std::time::Instant;
    use test_r::test;

    #[test]
    fn independent_memory_growth_uses_specific_memory_delta() {
        let (delta, total) = desired_total_after_growth(80, 30, 45).unwrap();
        assert_eq!(delta, 15);
        assert_eq!(total, 95);

        let (_, over_limit_total) = desired_total_after_growth(total, 20, 30).unwrap();
        assert_eq!(over_limit_total, 105);
    }

    #[test]
    fn growth_total_detects_overflow() {
        assert_eq!(desired_total_after_growth(u64::MAX, 0, 1), None);
    }

    #[test]
    fn tracker_preserves_initial_reservation_across_growth_and_reconciliation() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            40,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            now,
        );
        tracker.grow(20, now);
        tracker.reconcile(60, now);

        assert_eq!(tracker.initially_reserved_bytes(), 40);
        assert_eq!(tracker.current_bytes(), 60);
    }

    #[test]
    fn startup_minimums_consume_the_existing_reservation() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            40,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            now,
        );

        assert_eq!(
            tracker.desired_total_after_unshared_growth(0, 15),
            Some((15, 40))
        );
        assert_eq!(
            tracker.desired_total_after_unshared_growth(0, 25),
            Some((25, 40))
        );
        assert_eq!(
            tracker.desired_total_after_unshared_growth(25, 35),
            Some((10, 50))
        );
    }

    #[test]
    fn replay_growth_consumes_the_reconstructed_reservation() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            50,
            AgentMode::Durable,
            true,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            now,
        );

        assert_eq!(
            tracker.desired_total_after_unshared_growth(0, 40),
            Some((40, 50))
        );
        assert_eq!(
            tracker.desired_total_after_unshared_growth(40, 50),
            Some((10, 50))
        );
        tracker.grow(10, now);
        assert_eq!(tracker.current_bytes(), 50);
    }
}
