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

use crate::services::agent_filesystem::{
    AgentFilesystemRuntime, AgentFilesystemUsage, FilesystemStorageError,
};
use crate::services::agent_storage_meter::{AgentStorageMeter, FilesystemUsageObservation};
use crate::services::byte_time_accumulator::ByteTimeSettlement;
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::resource_limits::AtomicResourceEntry;
use golem_common::model::agent::AgentMode;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

pub(crate) trait FilesystemUsageObserver: Send + Sync {
    fn is_active(&self) -> bool {
        true
    }

    fn begin_observation(&self) -> FilesystemUsageObservation;
    fn complete_observation(
        &self,
        observation: FilesystemUsageObservation,
        usage: Option<AgentFilesystemUsage>,
        now: Instant,
    );
    fn fail_observation(&self, observation: FilesystemUsageObservation) -> bool;
}

#[derive(Clone, Debug)]
/// Long-lived owner of the memory and storage meters for one resident agent.
///
/// Each `open`/`close` pair starts and settles one permit-owned billing interval. The shared
/// transition lock keeps both leaf meters on the same monotonic timeline.
pub(crate) struct AgentResourceBilling {
    state: Arc<AgentResourceBillingState>,
}

#[derive(Debug)]
struct AgentResourceBillingState {
    mode: AgentMode,
    entry: Weak<AtomicResourceEntry>,
    linear_memory: LinearMemoryTracker,
    transition: Arc<Mutex<()>>,
    storage: AgentStorageMeter,
}

impl AgentResourceBilling {
    pub(crate) fn new(
        mode: AgentMode,
        linear_memory: LinearMemoryTracker,
        entry: Arc<AtomicResourceEntry>,
        now: Instant,
    ) -> Self {
        let transition = linear_memory.resource_transition();
        Self {
            state: Arc::new(AgentResourceBillingState {
                mode,
                entry: Arc::downgrade(&entry),
                linear_memory,
                transition,
                storage: AgentStorageMeter::new(mode, entry, now),
            }),
        }
    }

    pub(crate) fn is_same_billing(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.state.storage.is_active()
    }

    pub(crate) async fn open(
        &self,
        filesystem: &AgentFilesystemRuntime,
    ) -> Result<(), FilesystemStorageError> {
        let _admission_pause = filesystem.pause_effect_admission();
        filesystem.drain().await;
        let usage = filesystem.usage().await?;
        let now = Instant::now();
        if self.open_at(usage.map(|usage| usage.allocated_bytes), now) {
            Ok(())
        } else {
            Err(FilesystemStorageError::resource_billing_transition(
                "open resource window with a stopped memory meter",
            ))
        }
    }

    pub(crate) async fn close(
        &self,
        filesystem: &AgentFilesystemRuntime,
    ) -> Result<(), FilesystemStorageError> {
        let _admission_pause = filesystem.pause_effect_admission();
        filesystem.drain().await;
        filesystem.wait_for_usage_completion_debounce().await;
        let observation = self.begin_close_observation().ok_or_else(|| {
            FilesystemStorageError::resource_billing_transition(
                "begin terminal resource-window close",
            )
        })?;
        let usage = match filesystem.usage().await {
            Ok(usage) => usage,
            Err(error) => {
                self.abort();
                return Err(error);
            }
        };
        let now = Instant::now();
        if self.close_at(observation, usage.map(|usage| usage.allocated_bytes), now) {
            Ok(())
        } else {
            Err(FilesystemStorageError::resource_billing_transition(
                "complete terminal resource-window close",
            ))
        }
    }

    pub(crate) fn flush(&self, now: Instant) {
        let _transition = self.state.transition.lock().unwrap();
        let memory_units = self.state.linear_memory.meter().take_units(now);
        let storage_units = self.state.storage.flush(now);
        self.state.record_usage(memory_units, storage_units);
    }

    pub(crate) fn abort(&self) {
        let _transition = self.state.transition.lock().unwrap();
        let storage_settlement = self.state.storage.abort();
        if let Some(storage_settlement) = storage_settlement {
            let memory_settlement = self
                .state
                .linear_memory
                .meter()
                .take_abort_settlement()
                .unwrap_or_default();
            self.state
                .record_settlement(memory_settlement, storage_settlement);
        }
    }

    pub(crate) fn enforce_memory_limit(&self, limit: u64) {
        self.state
            .linear_memory
            .enforce_resource_memory_limit(limit);
    }

    fn open_at(&self, allocated_bytes: Option<u64>, now: Instant) -> bool {
        let _transition = self.state.transition.lock().unwrap();
        // Component instantiation reconciles every Wasmtime memory into this canonical tracker
        // before the first window opens. Memory cannot shrink while the resident worker is idle,
        // so this tracker read is the authoritative opening observation.
        if !self
            .state
            .linear_memory
            .meter()
            .resume(self.state.linear_memory.current_bytes(), now)
        {
            return false;
        }
        self.state.storage.open(allocated_bytes, now);
        true
    }

    fn close_at(
        &self,
        observation: FilesystemUsageObservation,
        allocated_bytes: Option<u64>,
        now: Instant,
    ) -> bool {
        let _transition = self.state.transition.lock().unwrap();
        let storage_settlement = self.state.storage.close(observation, allocated_bytes, now);
        if let Some(storage_settlement) = storage_settlement {
            self.state.linear_memory.meter().pause(now);
            let memory_settlement = self.state.linear_memory.meter().take_settlement();
            self.state
                .record_settlement(memory_settlement, storage_settlement);
            true
        } else {
            false
        }
    }

    fn begin_close_observation(&self) -> Option<FilesystemUsageObservation> {
        let _transition = self.state.transition.lock().unwrap();
        self.state.storage.begin_close()
    }

    #[cfg(test)]
    pub(crate) fn open_for_test(&self, allocated_bytes: Option<u64>, now: Instant) -> bool {
        self.open_at(allocated_bytes, now)
    }

    #[cfg(test)]
    pub(crate) fn begin_close_for_test(&self) -> Option<FilesystemUsageObservation> {
        self.begin_close_observation()
    }

    #[cfg(test)]
    pub(crate) fn close_for_test(
        &self,
        observation: FilesystemUsageObservation,
        allocated_bytes: Option<u64>,
        now: Instant,
    ) -> bool {
        self.close_at(observation, allocated_bytes, now)
    }
}

impl FilesystemUsageObserver for AgentResourceBilling {
    fn is_active(&self) -> bool {
        AgentResourceBilling::is_active(self)
    }

    fn begin_observation(&self) -> FilesystemUsageObservation {
        self.state.storage.begin_observation()
    }

    fn complete_observation(
        &self,
        observation: FilesystemUsageObservation,
        usage: Option<AgentFilesystemUsage>,
        now: Instant,
    ) {
        let _transition = self.state.transition.lock().unwrap();
        let accepted = self.state.storage.complete_observation(
            observation,
            usage.map(|usage| usage.allocated_bytes),
            now,
        );
        if accepted {
            self.state.linear_memory.meter().sample(now);
        }
    }

    fn fail_observation(&self, observation: FilesystemUsageObservation) -> bool {
        let _transition = self.state.transition.lock().unwrap();
        let settlement = self.state.storage.fail_observation(observation);
        if let Some(storage_settlement) = settlement {
            let memory_settlement = self
                .state
                .linear_memory
                .meter()
                .take_abort_settlement()
                .unwrap_or_default();
            self.state
                .record_settlement(memory_settlement, storage_settlement);
            true
        } else {
            false
        }
    }
}

impl AgentResourceBillingState {
    fn record_usage(&self, memory_units: i64, storage_units: i64) {
        if let Some(entry) = self.entry.upgrade() {
            entry.record_resource_usage(self.mode, memory_units, storage_units);
        }
    }

    fn record_settlement(&self, memory: ByteTimeSettlement, storage: ByteTimeSettlement) {
        if let Some(entry) = self.entry.upgrade() {
            entry.record_resource_settlement(self.mode, memory, storage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::active_workers::MemoryGrant;
    use std::time::Duration;
    use test_r::test;

    fn meter(now: Instant) -> (Arc<AtomicResourceEntry>, AgentResourceBilling) {
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        let memory = LinearMemoryTracker::new(
            1024 * 1024 * 1024,
            1024 * 1024 * 1024,
            AgentMode::Durable,
            false,
            entry.clone(),
            Arc::new(Mutex::new(MemoryGrant::inert(0))),
            now,
        );
        let meter = AgentResourceBilling::new(AgentMode::Durable, memory, entry.clone(), now);
        (entry, meter)
    }

    #[test]
    fn storage_is_prospective_and_final_level_has_zero_duration() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let increase = meter.begin_observation();
        meter.complete_observation(
            increase,
            Some(AgentFilesystemUsage {
                allocated_bytes: 300,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(2),
        );
        let close = meter.begin_close_for_test().unwrap();
        meter.close_for_test(close, Some(900), t0 + Duration::from_secs(5));
        meter.flush(t0 + Duration::from_secs(20));

        assert_eq!(entry.durable_byte_seconds_delta(), 100 * 2 + 300 * 3);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 5);
    }

    #[test]
    fn permit_to_start_gap_and_paused_changes_are_unbilled() {
        let acquired = Instant::now();
        let (entry, meter) = meter(acquired);
        meter.open_for_test(Some(100), acquired + Duration::from_secs(3));
        let close = meter.begin_close_for_test().unwrap();
        meter.close_for_test(close, Some(200), acquired + Duration::from_secs(4));
        let paused = meter.begin_observation();
        meter.complete_observation(
            paused,
            Some(AgentFilesystemUsage {
                allocated_bytes: 900,
                filesystem_objects: 0,
            }),
            acquired + Duration::from_secs(8),
        );
        meter.flush(acquired + Duration::from_secs(10));

        assert_eq!(entry.durable_byte_seconds_delta(), 100);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 1);
    }

    #[test]
    fn stale_observation_cannot_replace_a_newer_level() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let stale = meter.begin_observation();
        let current = meter.begin_observation();
        meter.complete_observation(
            current,
            Some(AgentFilesystemUsage {
                allocated_bytes: 300,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(1),
        );
        meter.complete_observation(
            stale,
            Some(AgentFilesystemUsage {
                allocated_bytes: 10,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(2),
        );
        let close = meter.begin_close_for_test().unwrap();
        meter.close_for_test(close, Some(300), t0 + Duration::from_secs(3));
        meter.flush(t0 + Duration::from_secs(3));

        assert_eq!(entry.durable_byte_seconds_delta(), 100 + 300 * 2);
    }

    #[test]
    fn terminal_close_rejects_a_newer_sampler_completion() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let close = meter.begin_close_for_test().unwrap();
        let newer_sample = meter.begin_observation();
        meter.complete_observation(
            newer_sample,
            Some(AgentFilesystemUsage {
                allocated_bytes: 300,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(2),
        );

        meter.close_for_test(close, Some(900), t0 + Duration::from_secs(5));
        let idle_sample = meter.begin_observation();
        meter.complete_observation(
            idle_sample,
            Some(AgentFilesystemUsage {
                allocated_bytes: 1200,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(10),
        );
        meter.flush(t0 + Duration::from_secs(20));

        assert!(!meter.is_active());
        assert_eq!(entry.durable_byte_seconds_delta(), 100 * 5);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 5);
    }

    #[test]
    fn failed_observation_aborts_both_meters_without_estimating_failed_interval() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let completed = meter.begin_observation();
        meter.complete_observation(
            completed,
            Some(AgentFilesystemUsage {
                allocated_bytes: 300,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(2),
        );

        let failed = meter.begin_observation();
        assert!(meter.fail_observation(failed));
        meter.flush(t0 + Duration::from_secs(20));

        assert!(!meter.is_active());
        assert_eq!(entry.durable_byte_seconds_delta(), 200);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 2);
    }

    #[test]
    fn failed_observation_prevents_a_partial_resource_window_reopen() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        assert!(meter.open_for_test(Some(100), t0));
        let failed = meter.begin_observation();
        assert!(meter.fail_observation(failed));

        assert!(!meter.open_for_test(Some(300), t0 + Duration::from_secs(1)));
        meter.flush(t0 + Duration::from_secs(10));

        assert!(!meter.is_active());
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
    }

    #[test]
    fn failed_observation_after_close_is_rejected() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let failed = meter.begin_observation();
        let close = meter.begin_close_for_test().unwrap();

        meter.close_for_test(close, Some(300), t0 + Duration::from_secs(2));
        assert!(!meter.fail_observation(failed));
        meter.flush(t0 + Duration::from_secs(10));

        assert!(!meter.is_active());
        assert_eq!(entry.durable_byte_seconds_delta(), 200);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 2);
    }

    #[test]
    fn abort_after_close_cannot_discard_the_closed_window() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let close = meter.begin_close_for_test().unwrap();

        assert!(meter.close_for_test(close, Some(100), t0 + Duration::from_secs(2)));
        meter.abort();

        assert_eq!(entry.durable_byte_seconds_delta(), 200);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 2);
    }

    #[test]
    fn older_failed_observation_after_newer_sample_is_rejected() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let failed = meter.begin_observation();
        let completed = meter.begin_observation();
        meter.complete_observation(
            completed,
            Some(AgentFilesystemUsage {
                allocated_bytes: 300,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(2),
        );

        assert!(!meter.fail_observation(failed));
        let close = meter.begin_close_for_test().unwrap();
        meter.close_for_test(close, Some(300), t0 + Duration::from_secs(5));
        meter.flush(t0 + Duration::from_secs(10));

        assert_eq!(entry.durable_byte_seconds_delta(), 100 * 2 + 300 * 3);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 5);
    }

    #[test]
    fn unsupported_storage_keeps_memory_billing() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(None, t0);
        let close = meter.begin_close_for_test().unwrap();
        meter.close_for_test(close, None, t0 + Duration::from_secs(2));
        meter.flush(t0 + Duration::from_secs(2));

        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 2);
    }

    #[test]
    fn storage_decrease_is_prospective() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(300), t0);
        let decrease = meter.begin_observation();
        meter.complete_observation(
            decrease,
            Some(AgentFilesystemUsage {
                allocated_bytes: 100,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(2),
        );
        let close = meter.begin_close_for_test().unwrap();
        meter.close_for_test(close, Some(100), t0 + Duration::from_secs(5));
        meter.flush(t0 + Duration::from_secs(5));

        assert_eq!(entry.durable_byte_seconds_delta(), 300 * 2 + 100 * 3);
    }

    #[test]
    async fn opening_observation_failure_starts_neither_meter() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        let runtime = AgentFilesystemRuntime::new_for_test_with_failed_observations();

        assert!(meter.open(&runtime).await.is_err());
        meter.flush(t0 + Duration::from_secs(10));

        assert!(!meter.is_active());
        assert_eq!(entry.durable_byte_seconds_delta(), 0);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 0);
    }

    #[test]
    async fn close_owns_effect_admission_until_the_final_observation() {
        let t0 = Instant::now();
        let (_, meter) = meter(t0);
        let runtime = AgentFilesystemRuntime::new_for_test();
        meter.open(&runtime).await.unwrap();
        let active_effect = runtime.begin_effect().await.unwrap();
        let close_meter = meter.clone();
        let close_runtime = runtime.clone();
        let close = tokio::spawn(async move { close_meter.close(&close_runtime).await });

        while !runtime.effect_admission_is_paused() {
            tokio::task::yield_now().await;
        }
        assert!(runtime.begin_effect().await.is_err());
        drop(active_effect);
        close.await.unwrap().unwrap();
        assert!(runtime.begin_effect().await.is_ok());
    }

    #[test]
    fn failed_close_discards_the_unsampled_tail() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let completed = meter.begin_observation();
        meter.complete_observation(
            completed,
            Some(AgentFilesystemUsage {
                allocated_bytes: 300,
                filesystem_objects: 0,
            }),
            t0 + Duration::from_secs(1),
        );
        meter.abort();
        meter.flush(t0 + Duration::from_secs(10));

        assert_eq!(entry.durable_byte_seconds_delta(), 100);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 1);
    }

    #[test]
    fn sampler_failure_cannot_abort_a_window_owned_by_terminal_close() {
        let t0 = Instant::now();
        let (entry, meter) = meter(t0);
        meter.open_for_test(Some(100), t0);
        let close = meter.begin_close_for_test().unwrap();
        let sampler = meter.begin_observation();

        assert!(!meter.fail_observation(sampler));
        meter.close_for_test(close, Some(300), t0 + Duration::from_secs(5));
        meter.flush(t0 + Duration::from_secs(5));

        assert_eq!(entry.durable_byte_seconds_delta(), 500);
        assert_eq!(entry.memory_gb_seconds_delta(AgentMode::Durable), 5);
    }

    #[test]
    fn account_storage_remainder_crosses_short_windows() {
        let t0 = Instant::now();
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
        for offset in [0, 400] {
            let memory = LinearMemoryTracker::new(
                0,
                0,
                AgentMode::Durable,
                false,
                entry.clone(),
                Arc::new(Mutex::new(MemoryGrant::inert(0))),
                t0,
            );
            let meter = AgentResourceBilling::new(
                AgentMode::Durable,
                memory,
                entry.clone(),
                t0 + Duration::from_millis(offset),
            );
            meter.open_for_test(Some(1), t0 + Duration::from_millis(offset));
            let close = meter.begin_close_for_test().unwrap();
            meter.close_for_test(close, Some(1), t0 + Duration::from_millis(offset + 600));
        }

        assert_eq!(entry.durable_byte_seconds_delta(), 1);
    }
}
