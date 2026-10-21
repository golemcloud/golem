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

use super::*;

impl CapturedAccountUsage {
    pub(super) fn is_zero(&self) -> bool {
        self.memory_gb_seconds == 0
            && self.durable_storage_byte_seconds == 0
            && self.ephemeral_storage_byte_seconds == 0
            && self.memory_byte_nanoseconds_remainder == 0
            && self.durable_storage_byte_nanoseconds_remainder == 0
            && self.ephemeral_storage_byte_nanoseconds_remainder == 0
    }
}

impl AccountUsageAccumulator {
    pub(super) fn new(config: ResourceUsageMeteringConfig) -> Self {
        Self {
            memory: config.memory.then(MemoryUsageAccumulator::default),
            storage: config.filesystem.then(StorageUsageAccumulator::default),
        }
    }

    pub(super) fn add_storage(&mut self, mode: AgentMode, units: u128) {
        let Some(storage) = &mut self.storage else {
            return;
        };
        let pending = match mode {
            AgentMode::Durable => &mut storage.durable_storage_byte_seconds,
            AgentMode::Ephemeral => &mut storage.ephemeral_storage_byte_seconds,
        };
        *pending = pending.saturating_add(units);
    }

    pub(super) fn add_memory_settlement(
        &mut self,
        mode: AgentMode,
        settlement: ByteTimeSettlement,
        maximum_byte_nanoseconds: Option<u128>,
    ) -> u128 {
        let Some(memory) = &mut self.memory else {
            return 0;
        };
        let requested_byte_nanoseconds = settlement
            .units
            .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
            .saturating_add(settlement.remainder);
        let within_capacity_byte_nanoseconds = maximum_byte_nanoseconds
            .map_or(requested_byte_nanoseconds, |maximum| {
                requested_byte_nanoseconds.min(maximum)
            });
        let measured_units = requested_byte_nanoseconds / BYTE_NANOSECONDS_PER_GB_SECOND;
        let measured_remainder = requested_byte_nanoseconds % BYTE_NANOSECONDS_PER_GB_SECOND;
        memory.remainder = memory.remainder.saturating_add(measured_remainder);
        let remainder_units = memory.remainder / BYTE_NANOSECONDS_PER_GB_SECOND;
        memory.remainder %= BYTE_NANOSECONDS_PER_GB_SECOND;
        let pending = match mode {
            AgentMode::Durable => &mut memory.durable_memory_gb_seconds,
            AgentMode::Ephemeral => &mut memory.ephemeral_memory_gb_seconds,
        };
        *pending = pending.saturating_add(measured_units.saturating_add(remainder_units));
        requested_byte_nanoseconds.saturating_sub(within_capacity_byte_nanoseconds)
    }

    pub(super) fn add_storage_settlement(
        &mut self,
        mode: AgentMode,
        settlement: ByteTimeSettlement,
        maximum_byte_nanoseconds: Option<u128>,
    ) -> u128 {
        let Some(storage) = &mut self.storage else {
            return 0;
        };
        let requested_byte_nanoseconds = settlement
            .units
            .saturating_mul(1_000_000_000)
            .saturating_add(settlement.remainder);
        let within_capacity_byte_nanoseconds = maximum_byte_nanoseconds
            .map_or(requested_byte_nanoseconds, |maximum| {
                requested_byte_nanoseconds.min(maximum)
            });
        let measured_units = requested_byte_nanoseconds / 1_000_000_000;
        let measured_remainder = requested_byte_nanoseconds % 1_000_000_000;
        let remainder = match mode {
            AgentMode::Durable => &mut storage.durable_storage_remainder,
            AgentMode::Ephemeral => &mut storage.ephemeral_storage_remainder,
        };
        *remainder = remainder.saturating_add(measured_remainder);
        let remainder_units = *remainder / 1_000_000_000;
        *remainder %= 1_000_000_000;
        self.add_storage(mode, measured_units.saturating_add(remainder_units));
        requested_byte_nanoseconds.saturating_sub(within_capacity_byte_nanoseconds)
    }

    pub(super) fn is_active(&self) -> bool {
        self.memory.as_ref().is_some_and(|memory| {
            memory.durable_memory_gb_seconds != 0 || memory.ephemeral_memory_gb_seconds != 0
        }) || self.storage.as_ref().is_some_and(|storage| {
            storage.durable_storage_byte_seconds != 0 || storage.ephemeral_storage_byte_seconds != 0
        })
    }

    #[cfg(test)]
    pub(super) fn memory(&self, mode: AgentMode) -> u128 {
        self.memory.as_ref().map_or(0, |memory| match mode {
            AgentMode::Durable => memory.durable_memory_gb_seconds,
            AgentMode::Ephemeral => memory.ephemeral_memory_gb_seconds,
        })
    }

    pub(super) fn total_memory(&self) -> u128 {
        self.memory.as_ref().map_or(0, |memory| {
            memory
                .durable_memory_gb_seconds
                .saturating_add(memory.ephemeral_memory_gb_seconds)
        })
    }

    pub(super) fn memory_remainder(&self) -> u128 {
        self.memory.as_ref().map_or(0, |memory| memory.remainder)
    }

    pub(super) fn storage(&self, mode: AgentMode) -> u128 {
        self.storage.as_ref().map_or(0, |storage| match mode {
            AgentMode::Durable => storage.durable_storage_byte_seconds,
            AgentMode::Ephemeral => storage.ephemeral_storage_byte_seconds,
        })
    }

    pub(super) fn capture(&mut self, include_remainders: bool) -> CapturedAccountUsage {
        let (durable_memory, ephemeral_memory) = self.memory.as_mut().map_or((0, 0), |memory| {
            let durable = take_bounded(&mut memory.durable_memory_gb_seconds, i64::MAX as u128);
            let ephemeral = take_bounded(
                &mut memory.ephemeral_memory_gb_seconds,
                i64::MAX as u128 - durable,
            );
            (durable, ephemeral)
        });
        let (durable_storage, ephemeral_storage) =
            self.storage.as_mut().map_or((0, 0), |storage| {
                (
                    take_bounded(&mut storage.durable_storage_byte_seconds, i64::MAX as u128),
                    take_bounded(
                        &mut storage.ephemeral_storage_byte_seconds,
                        i64::MAX as u128,
                    ),
                )
            });
        CapturedAccountUsage {
            memory_gb_seconds: (durable_memory + ephemeral_memory) as i64,
            durable_memory_gb_seconds: durable_memory as i64,
            ephemeral_memory_gb_seconds: ephemeral_memory as i64,
            durable_storage_byte_seconds: durable_storage as i64,
            ephemeral_storage_byte_seconds: ephemeral_storage as i64,
            memory_byte_nanoseconds_remainder: self.memory.as_mut().map_or(0, |memory| {
                capture_remainder(&mut memory.remainder, include_remainders)
            }),
            durable_storage_byte_nanoseconds_remainder: self.storage.as_mut().map_or(
                0,
                |storage| {
                    capture_remainder(&mut storage.durable_storage_remainder, include_remainders)
                },
            ),
            ephemeral_storage_byte_nanoseconds_remainder: self.storage.as_mut().map_or(
                0,
                |storage| {
                    capture_remainder(&mut storage.ephemeral_storage_remainder, include_remainders)
                },
            ),
        }
    }
}

impl Default for AccountUsageAccumulator {
    fn default() -> Self {
        Self::new(ResourceUsageMeteringConfig::all_enabled())
    }
}

fn take_bounded(pending: &mut u128, maximum: u128) -> u128 {
    let captured = (*pending).min(maximum);
    *pending -= captured;
    captured
}

fn capture_remainder(remainder: &mut u128, capture: bool) -> u64 {
    if capture {
        std::mem::take(remainder) as u64
    } else {
        0
    }
}

impl AtomicResourceEntry {
    pub(super) fn secs_since_last_refresh(&self) -> i64 {
        Utc::now()
            .timestamp()
            .saturating_sub(self.last_refresh_secs.load(Ordering::Acquire))
    }

    pub fn record_storage_byte_seconds(&self, mode: AgentMode, amount: i64) {
        if amount > 0 {
            self.record_storage_settlement(
                mode,
                ByteTimeSettlement {
                    units: amount as u128,
                    remainder: 0,
                },
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn record_resource_settlement(
        &self,
        mode: AgentMode,
        memory: ByteTimeSettlement,
        storage: ByteTimeSettlement,
    ) {
        let period = self.advance_to_current_usage_period();
        self.record_resource_settlement_for_period(mode, period, memory, storage);
    }

    pub(crate) fn record_resource_settlement_for_period(
        &self,
        mode: AgentMode,
        period: AccountUsagePeriod,
        memory: ByteTimeSettlement,
        storage: ByteTimeSettlement,
    ) {
        let Some(accumulator) = &self.account_usage_accumulator else {
            return;
        };
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        if period < revision_state.current_period {
            let captured = self.capture_historical_byte_time_settlements(
                revision_state.current_mode_revision,
                revision_state.current_policy_revision,
                period,
                mode,
                memory,
                storage,
            );
            revision_state.pending.extend(captured);
            return;
        }
        self.update_usage_period_locked(&mut revision_state, period);
        let memory_limit = self.memory_settlement_limit(&revision_state);
        let storage_limit = self.storage_settlement_limit(&revision_state, mode);
        let mut accumulator = accumulator.lock().unwrap();
        if self.metering.memory {
            let overshoot = accumulator.add_memory_settlement(mode, memory, memory_limit);
            Self::record_hard_limit_memory_overshoot(&mut revision_state, overshoot);
        }
        if self.metering.filesystem {
            Self::record_storage_settlement_locked(
                &mut revision_state,
                &mut accumulator,
                mode,
                storage,
                storage_limit,
            );
        }
    }

    pub(super) fn capture_historical_byte_time_settlements(
        &self,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        period: AccountUsagePeriod,
        mode: AgentMode,
        memory: ByteTimeSettlement,
        storage: ByteTimeSettlement,
    ) -> Vec<CapturedUsageUpdate> {
        let mut accumulator = AccountUsageAccumulator::new(self.metering);
        accumulator.add_memory_settlement(mode, memory, None);
        accumulator.add_storage_settlement(mode, storage, None);
        let mut batches = Vec::new();
        while accumulator.is_active() {
            let captured = accumulator.capture(false);
            batches.push(self.captured_byte_time_update_with_policy_revision(
                monthly_usage_mode_revision,
                monthly_policy_revision,
                period,
                captured,
            ));
        }
        let remainder = accumulator.capture(true);
        if !remainder.is_zero() {
            batches.push(self.captured_byte_time_update_with_policy_revision(
                monthly_usage_mode_revision,
                monthly_policy_revision,
                period,
                remainder,
            ));
        }
        batches
    }

    pub(super) fn captured_byte_time_update_with_policy_revision(
        &self,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        period: AccountUsagePeriod,
        captured: CapturedAccountUsage,
    ) -> CapturedUsageUpdate {
        CapturedUsageUpdate {
            update: ResourceUsageUpdate {
                blob_storage_bytes_delta: 0,
                period,
                monthly_usage_mode_revision,
                monthly_policy_revision,
                memory_byte_nanoseconds_remainder: captured.memory_byte_nanoseconds_remainder,
                durable_storage_byte_nanoseconds_remainder: captured
                    .durable_storage_byte_nanoseconds_remainder,
                ephemeral_storage_byte_nanoseconds_remainder: captured
                    .ephemeral_storage_byte_nanoseconds_remainder,
                fuel_delta: 0,
                memory_gb_seconds_delta: captured.memory_gb_seconds,
                http_call_count_delta: 0,
                rpc_call_count_delta: 0,
                durable_storage_byte_seconds_delta: captured.durable_storage_byte_seconds,
                ephemeral_storage_byte_seconds_delta: captured.ephemeral_storage_byte_seconds,
                metering: golem_service_base::clients::registry::ResourceUsageMetering {
                    compute: self.metering.compute,
                    memory: self.metering.memory,
                    filesystem: self.metering.filesystem,
                },
            },
            durable_memory_gb_seconds_delta: captured.durable_memory_gb_seconds,
            ephemeral_memory_gb_seconds_delta: captured.ephemeral_memory_gb_seconds,
        }
    }

    #[cfg(test)]
    pub(super) fn record_resource_usage(
        &self,
        mode: AgentMode,
        memory_gb_seconds: i64,
        storage_byte_seconds: i64,
    ) {
        self.record_resource_settlement(
            mode,
            ByteTimeSettlement {
                units: memory_gb_seconds.max(0) as u128,
                remainder: 0,
            },
            ByteTimeSettlement {
                units: storage_byte_seconds.max(0) as u128,
                remainder: 0,
            },
        );
    }

    pub(crate) fn register_resource_usage_flusher(&self, flusher: Weak<dyn ResourceUsageFlusher>) {
        self.resource_usage_flushers.lock().unwrap().push(flusher);
    }

    pub(super) fn flush_active_resource_usage(&self) {
        let flushers = {
            let mut registered = self.resource_usage_flushers.lock().unwrap();
            let mut flushers = Vec::with_capacity(registered.len());
            registered.retain(|flusher| match flusher.upgrade() {
                Some(flusher) => {
                    flushers.push(flusher);
                    true
                }
                None => false,
            });
            flushers
        };
        for flusher in flushers {
            flusher.flush_usage();
        }
    }

    #[cfg(test)]
    pub(crate) fn record_storage_remainder(&self, mode: AgentMode, remainder: u128) {
        if !self.metering.filesystem || remainder == 0 {
            return;
        }
        let _revision_state = self.usage_revision_state.lock().unwrap();
        self.account_usage_accumulator
            .as_ref()
            .expect("filesystem metering is enabled")
            .lock()
            .unwrap()
            .add_storage_settlement(
                mode,
                ByteTimeSettlement {
                    units: 0,
                    remainder,
                },
                None,
            );
    }

    /// Returns the local durable storage delta for production-context executor tests.
    #[doc(hidden)]
    pub fn flush_durable_storage_byte_seconds_for_test(&self) -> i64 {
        self.account_usage_accumulator
            .as_ref()
            .map_or(0, |accumulator| {
                accumulator
                    .lock()
                    .unwrap()
                    .storage(AgentMode::Durable)
                    .min(i64::MAX as u128) as i64
            })
    }

    pub(super) fn capture_usage_update(
        &self,
        refresh_threshold_secs: i64,
    ) -> Option<CapturedUsageUpdate> {
        self.capture_usage_update_at(AccountUsagePeriod::current(), refresh_threshold_secs)
    }

    pub(super) fn capture_usage_update_at(
        &self,
        period: AccountUsagePeriod,
        refresh_threshold_secs: i64,
    ) -> Option<CapturedUsageUpdate> {
        self.flush_active_resource_usage();
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        self.update_usage_period_locked(&mut revision_state, period);
        if let Some(captured) = revision_state.pending.pop_front() {
            self.mark_in_flight(&captured, &mut revision_state);
            return Some(captured);
        }

        let active = (self.metering.compute && self.delta.load(Ordering::Acquire) != 0)
            || self
                .account_usage_accumulator
                .as_ref()
                .is_some_and(|accumulator| accumulator.lock().unwrap().is_active())
            || self.unsynced_http_calls.load(Ordering::Acquire) > 0
            || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0
            || self.blob_storage_quota.lock().unwrap().unsynced_delta != 0;
        let stale = self.secs_since_last_refresh() >= refresh_threshold_secs;

        let unavailable = revision_state.monthly_policy.as_ref().is_some_and(|gate| {
            gate.policy_status
                == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
        });
        if !active && !stale && !unavailable {
            return None;
        }

        let captured = self.capture_current_usage(
            revision_state.current_mode_revision,
            revision_state.current_policy_revision,
            revision_state.current_period,
            false,
        );
        self.mark_in_flight(&captured, &mut revision_state);
        Some(captured)
    }

    pub(super) fn capture_current_usage(
        &self,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        period: AccountUsagePeriod,
        include_remainders: bool,
    ) -> CapturedUsageUpdate {
        let fuel_delta = if self.metering.compute {
            self.delta.swap(0, Ordering::AcqRel)
        } else {
            0
        };
        let captured_usage = self
            .account_usage_accumulator
            .as_ref()
            .map_or_else(CapturedAccountUsage::default, |accumulator| {
                accumulator.lock().unwrap().capture(include_remainders)
            });
        let mut captured = self.captured_byte_time_update_with_policy_revision(
            monthly_usage_mode_revision,
            monthly_policy_revision,
            period,
            captured_usage,
        );
        captured.update.fuel_delta = fuel_delta;
        captured.update.http_call_count_delta = self.unsynced_http_calls.swap(0, Ordering::AcqRel);
        captured.update.rpc_call_count_delta = self.unsynced_rpc_calls.swap(0, Ordering::AcqRel);
        captured.update.blob_storage_bytes_delta = {
            let mut quota = self.blob_storage_quota.lock().unwrap();
            let delta = quota.unsynced_delta;
            quota.unsynced_delta = 0;
            quota.syncing_delta = quota.syncing_delta.saturating_add(delta);
            delta
        };
        captured
    }

    pub(super) fn mark_in_flight(
        &self,
        captured: &CapturedUsageUpdate,
        revision_state: &mut UsageRevisionState,
    ) {
        if captured.update.http_call_count_delta > 0 {
            self.syncing_http_calls
                .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(captured.update.http_call_count_delta))
                })
                .ok();
        }
        if captured.update.rpc_call_count_delta > 0 {
            self.syncing_rpc_calls
                .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    Some(count.saturating_add(captured.update.rpc_call_count_delta))
                })
                .ok();
        }
        if captured.update.fuel_delta != 0 {
            self.in_flight_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.update.fuel_delta))
                })
                .ok();
        }
        if captured.update.memory_gb_seconds_delta != 0 {
            self.in_flight_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.update.memory_gb_seconds_delta))
                })
                .ok();
        }
        if captured.durable_memory_gb_seconds_delta != 0 {
            self.in_flight_durable_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.durable_memory_gb_seconds_delta))
                })
                .ok();
        }
        if captured.ephemeral_memory_gb_seconds_delta != 0 {
            self.in_flight_ephemeral_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(captured.ephemeral_memory_gb_seconds_delta))
                })
                .ok();
        }
        let usage = LocalMonthlyUsage::from_update(&captured.update);
        if !usage.is_zero()
            && let Some(gate) = &mut revision_state.monthly_policy
        {
            gate.unassigned_in_flight_usage
                .entry(captured.update.period)
                .or_default()
                .saturating_add_assign(usage);
        }
    }

    #[cfg(test)]
    pub(crate) fn capture_usage_update_for_test(&self) -> ResourceUsageUpdate {
        self.capture_usage_update(i64::MAX)
            .expect("expected a pending resource usage update")
            .update
    }

    #[cfg(test)]
    pub(crate) fn capture_pending_usage_for_test(&self) -> Option<ResourceUsageUpdate> {
        self.capture_usage_update(i64::MAX)
            .map(|captured| captured.update)
    }

    #[cfg(test)]
    pub(crate) fn capture_usage_update_at_for_test(
        &self,
        period: AccountUsagePeriod,
    ) -> ResourceUsageUpdate {
        self.capture_usage_update_at(period, i64::MAX)
            .expect("expected a pending resource usage update")
            .update
    }

    #[cfg(test)]
    pub(crate) fn capture_byte_time_usage_for_test(&self) -> (i64, i64) {
        let captured = self
            .capture_usage_update(i64::MAX)
            .expect("active byte-time usage was not captured");
        (
            captured.update.memory_gb_seconds_delta,
            captured.update.durable_storage_byte_seconds_delta,
        )
    }

    pub fn record_memory_gb_seconds(&self, mode: AgentMode, amount: i64) {
        if amount > 0 {
            self.record_memory_settlement(
                mode,
                ByteTimeSettlement {
                    units: amount as u128,
                    remainder: 0,
                },
            );
        }
    }

    pub(crate) fn record_memory_settlement(&self, mode: AgentMode, settlement: ByteTimeSettlement) {
        let period = self.advance_to_current_usage_period();
        self.record_memory_settlement_for_period(mode, period, settlement);
    }

    pub(crate) fn record_memory_settlement_for_period(
        &self,
        mode: AgentMode,
        period: AccountUsagePeriod,
        settlement: ByteTimeSettlement,
    ) {
        if self.metering.memory {
            self.record_resource_settlement_for_period(
                mode,
                period,
                settlement,
                ByteTimeSettlement::default(),
            );
        }
    }

    pub(super) fn record_storage_settlement(
        &self,
        mode: AgentMode,
        settlement: ByteTimeSettlement,
    ) {
        let period = self.advance_to_current_usage_period();
        self.record_storage_settlement_for_period(mode, period, settlement);
    }

    pub(super) fn record_storage_settlement_for_period(
        &self,
        mode: AgentMode,
        period: AccountUsagePeriod,
        settlement: ByteTimeSettlement,
    ) {
        if self.metering.filesystem {
            self.record_resource_settlement_for_period(
                mode,
                period,
                ByteTimeSettlement::default(),
                settlement,
            );
        }
    }

    pub(super) fn record_storage_settlement_locked(
        revision_state: &mut UsageRevisionState,
        accumulator: &mut AccountUsageAccumulator,
        mode: AgentMode,
        settlement: ByteTimeSettlement,
        maximum_byte_nanoseconds: Option<u128>,
    ) {
        let overshoot =
            accumulator.add_storage_settlement(mode, settlement, maximum_byte_nanoseconds);
        Self::record_hard_limit_storage_overshoot(revision_state, mode, overshoot);
    }

    #[cfg(test)]
    pub(crate) fn memory_gb_seconds_delta(&self, mode: AgentMode) -> i64 {
        self.account_usage_accumulator
            .as_ref()
            .map_or(0, |accumulator| {
                accumulator
                    .lock()
                    .unwrap()
                    .memory(mode)
                    .min(i64::MAX as u128) as i64
            })
    }

    #[cfg(test)]
    pub(crate) fn durable_byte_seconds_delta(&self) -> i64 {
        self.account_usage_accumulator
            .as_ref()
            .map_or(0, |accumulator| {
                accumulator
                    .lock()
                    .unwrap()
                    .storage(AgentMode::Durable)
                    .min(i64::MAX as u128) as i64
            })
    }

    #[cfg(test)]
    pub(crate) fn ephemeral_byte_seconds_delta(&self) -> i64 {
        self.account_usage_accumulator
            .as_ref()
            .map_or(0, |accumulator| {
                accumulator
                    .lock()
                    .unwrap()
                    .storage(AgentMode::Ephemeral)
                    .min(i64::MAX as u128) as i64
            })
    }
}
