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

use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::agent::AgentMode;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

pub(crate) const BYTE_NANOSECONDS_PER_GB_SECOND: u128 = (1024_u128 * 1024 * 1024) * 1_000_000_000;

#[derive(Clone, Debug)]
pub struct AgentMemoryMeter {
    inner: Arc<Inner>,
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
    active: bool,
    stopped: bool,
    last_sample: Instant,
    pending_byte_nanoseconds: u128,
}

impl AgentMemoryMeter {
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
                state: Mutex::new(State {
                    bytes,
                    active,
                    stopped: false,
                    last_sample: now,
                    pending_byte_nanoseconds: 0,
                }),
            }),
        }
    }

    pub fn is_same_meter(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn resume(&self, bytes: u64, now: Instant) {
        self.inner.transition(now, |state| {
            if !state.stopped {
                state.bytes = bytes;
                state.active = true;
            }
        });
    }

    pub fn pause(&self, now: Instant) {
        self.inner.transition(now, |state| state.active = false);
    }

    pub fn stop(&self, now: Instant) {
        let remainder = self.inner.transition(now, |state| {
            state.active = false;
            state.stopped = true;
            std::mem::take(&mut state.pending_byte_nanoseconds)
        });
        self.inner.transfer_remainder(remainder);
    }

    pub fn set_bytes(&self, bytes: u64, now: Instant) {
        self.inner.transition(now, |state| state.bytes = bytes);
    }

    pub fn flush(&self, now: Instant) {
        self.inner.transition(now, |_| {});
    }
}

impl Inner {
    fn transition<R>(&self, now: Instant, update: impl FnOnce(&mut State) -> R) -> R {
        let (units, result) = {
            let mut state = self.state.lock().unwrap();
            let units = state.take_complete_units(now);
            let result = update(&mut state);
            (units, result)
        };
        self.record(units);
        result
    }

    fn record(&self, units: i64) {
        if units != 0
            && let Some(entry) = self.entry.upgrade()
        {
            entry.record_memory_gb_seconds(self.mode, units);
        }
    }

    fn transfer_remainder(&self, remainder: u128) {
        if remainder != 0
            && let Some(entry) = self.entry.upgrade()
        {
            entry.record_memory_remainder(self.mode, remainder);
        }
    }
}

impl State {
    fn take_complete_units(&mut self, now: Instant) -> i64 {
        if now <= self.last_sample {
            return 0;
        }

        let elapsed = now.saturating_duration_since(self.last_sample).as_nanos();
        self.last_sample = now;
        if self.active && !self.stopped {
            self.pending_byte_nanoseconds = self
                .pending_byte_nanoseconds
                .saturating_add((self.bytes as u128).saturating_mul(elapsed));
        }

        let units = self.pending_byte_nanoseconds / BYTE_NANOSECONDS_PER_GB_SECOND;
        self.pending_byte_nanoseconds %= BYTE_NANOSECONDS_PER_GB_SECOND;
        units.min(i64::MAX as u128) as i64
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let (units, remainder) = {
            let state = self.state.get_mut().unwrap();
            let units = state.take_complete_units(Instant::now());
            (units, std::mem::take(&mut state.pending_byte_nanoseconds))
        };
        self.record(units);
        self.transfer_remainder(remainder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use test_r::test;

    fn gib(bytes: u64) -> u64 {
        bytes * 1024 * 1024 * 1024
    }

    #[test]
    fn pause_resume_and_stop_are_idempotent() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentMemoryMeter::new(AgentMode::Durable, gib(1), true, entry.clone(), now);

        meter.pause(now + Duration::from_secs(2));
        meter.pause(now + Duration::from_secs(3));
        meter.resume(gib(1), now + Duration::from_secs(4));
        meter.resume(gib(1), now + Duration::from_secs(5));
        meter.stop(now + Duration::from_secs(7));
        meter.stop(now + Duration::from_secs(8));
        meter.resume(gib(1), now + Duration::from_secs(9));
        meter.flush(now + Duration::from_secs(10));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 5);
    }

    #[test]
    fn growth_is_prospective() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();
        let meter = AgentMemoryMeter::new(AgentMode::Durable, gib(1), true, entry.clone(), now);

        meter.set_bytes(gib(2), now + Duration::from_secs(2));
        meter.flush(now + Duration::from_secs(5));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 8);
    }

    #[test]
    fn fractional_remainder_crosses_short_lived_agents() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();

        let first = AgentMemoryMeter::new(AgentMode::Ephemeral, gib(1), true, entry.clone(), now);
        first.stop(now + Duration::from_millis(400));
        let second = AgentMemoryMeter::new(
            AgentMode::Ephemeral,
            gib(1),
            true,
            entry.clone(),
            now + Duration::from_millis(400),
        );
        second.stop(now + Duration::from_secs(1));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Ephemeral), 1);
    }

    #[test]
    fn account_remainder_keeps_agent_modes_separate() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = Instant::now();

        let durable = AgentMemoryMeter::new(AgentMode::Durable, gib(1), true, entry.clone(), now);
        durable.stop(now + Duration::from_millis(400));
        let ephemeral = AgentMemoryMeter::new(
            AgentMode::Ephemeral,
            gib(1),
            true,
            entry.clone(),
            now + Duration::from_millis(400),
        );
        ephemeral.stop(now + Duration::from_secs(1));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Ephemeral), 0);
    }
}
