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
    shared_growth_reservation: AtomicU64,
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
                shared_growth_reservation: AtomicU64::new(0),
                replaying: AtomicBool::new(replaying),
                transitions: Mutex::new(()),
                meter: AgentMemoryMeter::new(mode, bytes, true, resource_entry, now),
            }),
        }
    }

    pub fn current_bytes(&self) -> u64 {
        self.inner.bytes.load(Ordering::Acquire)
    }

    pub fn is_replaying(&self) -> bool {
        self.inner.replaying.load(Ordering::Acquire)
    }

    pub fn switch_to_live(&self) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.replaying.store(false, Ordering::Release);
    }

    pub fn reconcile(&self, bytes: u64, shared_growth_reservation: u64, now: Instant) {
        let _transition = self.inner.transitions.lock().unwrap();
        self.inner.meter.set_bytes(bytes, now);
        self.inner.bytes.store(bytes, Ordering::Release);
        self.inner
            .shared_growth_reservation
            .store(shared_growth_reservation, Ordering::Release);
    }

    pub fn reconcile_preserving_shared_growth(
        &self,
        bytes: u64,
        shared_growth_reservation: u64,
        pre_subscription_bytes: u64,
        now: Instant,
    ) {
        let _transition = self.inner.transitions.lock().unwrap();
        let concurrent_growth = self.current_bytes().saturating_sub(pre_subscription_bytes);
        let bytes = bytes.saturating_add(concurrent_growth);
        self.inner.meter.set_bytes(bytes, now);
        self.inner.bytes.store(bytes, Ordering::Release);
        self.inner.shared_growth_reservation.store(
            shared_growth_reservation.saturating_sub(concurrent_growth),
            Ordering::Release,
        );
    }

    pub fn grow(&self, delta: u64, now: Instant) -> u64 {
        let _transition = self.inner.transitions.lock().unwrap();
        let bytes = self.current_bytes().saturating_add(delta);
        self.inner.meter.set_bytes(bytes, now);
        self.inner.bytes.store(bytes, Ordering::Release);
        bytes
    }

    pub fn grow_shared(&self, delta: u64, now: Instant) -> (u64, bool) {
        let _transition = self.inner.transitions.lock().unwrap();
        let bytes = self.current_bytes().saturating_add(delta);
        self.inner.meter.set_bytes(bytes, now);
        self.inner.bytes.store(bytes, Ordering::Release);
        self.inner
            .shared_growth_reservation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |reserved| {
                Some(reserved.saturating_sub(delta))
            })
            .ok();
        (bytes, self.inner.replaying.load(Ordering::Acquire))
    }

    pub fn desired_total_after_unshared_growth(
        &self,
        current_memory: usize,
        desired_memory: usize,
    ) -> Option<(u64, u64)> {
        let _transition = self.inner.transitions.lock().unwrap();
        self.current_bytes()
            .checked_add(self.inner.shared_growth_reservation.load(Ordering::Acquire))
            .and_then(|reserved_total| {
                desired_total_after_growth(reserved_total, current_memory, desired_memory)
            })
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
    fn unshared_growth_cannot_consume_shared_declared_maximum() {
        let now = Instant::now();
        let tracker = LinearMemoryTracker::new(
            40,
            AgentMode::Durable,
            false,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            now,
        );
        tracker.reconcile(40, 60, now);

        assert_eq!(
            tracker.desired_total_after_unshared_growth(20, 30),
            Some((10, 110))
        );

        tracker.grow_shared(20, now);
        assert_eq!(
            tracker.desired_total_after_unshared_growth(20, 30),
            Some((10, 110))
        );
    }
}
