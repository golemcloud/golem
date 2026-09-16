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

use crate::services::byte_time_accumulator::{
    ByteTimeAccumulator, MeteringTime, PeriodByteTimeSettlement,
};
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::account_usage::BYTE_NANOSECONDS_PER_GB_SECOND;
use golem_common::model::agent::AgentMode;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

#[derive(Clone, Debug)]
/// Leaf meter for linear-memory byte-time and memory-limit state.
///
/// `ResourceUsageMeter` owns permit-window lifecycle transitions and invokes this meter
/// under the transition lock shared with filesystem storage accounting.
pub struct AgentMemoryMeter {
    inner: Arc<Inner>,
}

struct Inner {
    mode: AgentMode,
    entry: Weak<AtomicResourceEntry>,
    state: Mutex<State>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("mode", &self.mode)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct State {
    bytes: u64,
    active: bool,
    stopped: bool,
    usage: ByteTimeAccumulator,
}

impl AgentMemoryMeter {
    pub fn new(
        mode: AgentMode,
        bytes: u64,
        active: bool,
        entry: Arc<AtomicResourceEntry>,
        now: Instant,
    ) -> Self {
        Self::new_at(
            mode,
            bytes,
            active,
            entry,
            MeteringTime::now().at_instant(now),
        )
    }

    pub(crate) fn new_at(
        mode: AgentMode,
        bytes: u64,
        active: bool,
        entry: Arc<AtomicResourceEntry>,
        now: MeteringTime,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                mode,
                entry: Arc::downgrade(&entry),
                state: Mutex::new(State {
                    bytes,
                    active,
                    stopped: false,
                    usage: ByteTimeAccumulator::new(BYTE_NANOSECONDS_PER_GB_SECOND, now),
                }),
            }),
        }
    }

    pub fn is_same_meter(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Changes only memory metering. Resource-window lifecycle transitions must use
    /// `ResourceUsageMeter` so filesystem storage changes at the same timestamp.
    pub fn resume(&self, bytes: u64, now: Instant) -> bool {
        self.resume_at(bytes, MeteringTime::now().at_instant(now))
    }

    pub(crate) fn resume_at(&self, bytes: u64, now: MeteringTime) -> bool {
        self.inner.transition(now, |state| {
            if state.stopped {
                false
            } else {
                state.bytes = bytes;
                state.active = true;
                true
            }
        })
    }

    /// Changes only memory metering. Resource-window lifecycle transitions must use
    /// `ResourceUsageMeter` so filesystem storage changes at the same timestamp.
    pub fn pause(&self, now: Instant) {
        self.pause_at(MeteringTime::now().at_instant(now));
    }

    pub(crate) fn pause_at(&self, now: MeteringTime) {
        self.inner.transition(now, |state| state.active = false);
    }

    /// Changes only memory metering. Resource-window lifecycle transitions must use
    /// `ResourceUsageMeter` so filesystem storage changes at the same timestamp.
    pub fn stop(&self, now: Instant) {
        self.stop_at(MeteringTime::now().at_instant(now));
    }

    pub(crate) fn stop_at(&self, now: MeteringTime) {
        let settlement = {
            let mut state = self.inner.state.lock().unwrap();
            state.accrue(now);
            if state.stopped {
                None
            } else {
                state.active = false;
                state.stopped = true;
                Some(state.take_settlement())
            }
        };
        if let Some(settlement) = settlement {
            self.inner.record_settlement(settlement);
        }
    }

    pub fn set_bytes(&self, bytes: u64, now: Instant) {
        self.set_bytes_at(bytes, MeteringTime::now().at_instant(now));
    }

    pub(crate) fn set_bytes_at(&self, bytes: u64, now: MeteringTime) {
        self.inner.transition(now, |state| state.bytes = bytes);
    }

    pub fn flush(&self, now: Instant) {
        self.flush_at(MeteringTime::now().at_instant(now));
    }

    pub(crate) fn flush_at(&self, now: MeteringTime) {
        let settlement = self.take_settlement_at(now);
        self.inner.record_settlement(settlement);
    }

    pub(crate) fn take_settlement(&self) -> Vec<PeriodByteTimeSettlement> {
        self.inner.state.lock().unwrap().take_settlement()
    }

    pub(crate) fn take_settlement_at(&self, now: MeteringTime) -> Vec<PeriodByteTimeSettlement> {
        let mut state = self.inner.state.lock().unwrap();
        state.accrue(now);
        state.take_settlement()
    }
}

impl Inner {
    fn transition<R>(&self, now: MeteringTime, update: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.state.lock().unwrap();
        state.accrue(now);
        update(&mut state)
    }

    fn record_settlement(&self, settlements: Vec<PeriodByteTimeSettlement>) {
        if let Some(entry) = self.entry.upgrade() {
            for settlement in settlements {
                entry.record_memory_settlement_for_period(
                    self.mode,
                    settlement.period,
                    settlement.usage,
                );
            }
        }
    }
}

impl State {
    fn accrue(&mut self, now: MeteringTime) {
        let bytes = (self.active && !self.stopped).then_some(self.bytes);
        self.usage.accrue(now, bytes);
    }

    fn take_settlement(&mut self) -> Vec<PeriodByteTimeSettlement> {
        self.usage.take_settlements()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let settlement = {
            let state = self.state.get_mut().unwrap();
            state.accrue(MeteringTime::now());
            state.take_settlement()
        };
        self.record_settlement(settlement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::time::Duration;
    use test_r::test;

    fn gib(bytes: u64) -> u64 {
        bytes * 1024 * 1024 * 1024
    }

    fn metering_time(instant: Instant) -> MeteringTime {
        MeteringTime {
            instant,
            utc: Utc
                .with_ymd_and_hms(2030, 6, 15, 12, 0, 0)
                .single()
                .unwrap(),
        }
    }

    fn later(time: MeteringTime, duration: Duration) -> MeteringTime {
        MeteringTime {
            instant: time.instant + duration,
            utc: time.utc + chrono::Duration::from_std(duration).unwrap(),
        }
    }

    #[test]
    fn pause_resume_and_stop_are_idempotent() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = metering_time(Instant::now());
        let meter = AgentMemoryMeter::new_at(AgentMode::Durable, gib(1), true, entry.clone(), now);

        meter.pause_at(later(now, Duration::from_secs(2)));
        meter.pause_at(later(now, Duration::from_secs(3)));
        assert!(meter.resume_at(gib(1), later(now, Duration::from_secs(4))));
        assert!(meter.resume_at(gib(1), later(now, Duration::from_secs(5))));
        meter.stop_at(later(now, Duration::from_secs(7)));
        meter.stop_at(later(now, Duration::from_secs(8)));
        assert!(!meter.resume_at(gib(1), later(now, Duration::from_secs(9))));
        meter.flush_at(later(now, Duration::from_secs(10)));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 5);
    }

    #[test]
    fn paused_warm_runnable_time_does_not_accrue() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = metering_time(Instant::now());
        let meter = AgentMemoryMeter::new_at(AgentMode::Durable, gib(1), true, entry.clone(), now);

        meter.pause_at(later(now, Duration::from_secs(1)));
        meter.flush_at(later(now, Duration::from_secs(11)));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 1);
    }

    #[test]
    fn growth_is_prospective() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = metering_time(Instant::now());
        let meter = AgentMemoryMeter::new_at(AgentMode::Durable, gib(1), true, entry.clone(), now);

        meter.set_bytes_at(gib(2), later(now, Duration::from_secs(2)));
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
        meter.flush_at(later(now, Duration::from_secs(5)));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 8);
    }

    #[test]
    fn fractional_remainder_crosses_short_lived_agents() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = metering_time(Instant::now());

        let first =
            AgentMemoryMeter::new_at(AgentMode::Ephemeral, gib(1), true, entry.clone(), now);
        first.stop_at(later(now, Duration::from_millis(400)));
        let second = AgentMemoryMeter::new_at(
            AgentMode::Ephemeral,
            gib(1),
            true,
            entry.clone(),
            later(now, Duration::from_millis(400)),
        );
        second.stop_at(later(now, Duration::from_secs(1)));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Ephemeral), 1);
    }

    #[test]
    fn account_remainder_crosses_agent_modes() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = metering_time(Instant::now());

        let durable =
            AgentMemoryMeter::new_at(AgentMode::Durable, gib(1), true, entry.clone(), now);
        durable.stop_at(later(now, Duration::from_millis(400)));
        let ephemeral = AgentMemoryMeter::new_at(
            AgentMode::Ephemeral,
            gib(1),
            true,
            entry.clone(),
            later(now, Duration::from_millis(400)),
        );
        ephemeral.stop_at(later(now, Duration::from_secs(1)));

        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Ephemeral), 1);
    }

    #[test]
    fn normal_transitions_publish_only_on_flush() {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let now = metering_time(Instant::now());
        let meter = AgentMemoryMeter::new_at(AgentMode::Durable, gib(1), true, entry.clone(), now);

        meter.pause_at(later(now, Duration::from_secs(2)));
        assert!(meter.resume_at(gib(2), later(now, Duration::from_secs(3))));
        meter.set_bytes_at(gib(3), later(now, Duration::from_secs(4)));
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);

        meter.flush_at(later(now, Duration::from_secs(5)));
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 7);
    }
}
