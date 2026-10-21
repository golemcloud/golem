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

pub(super) fn retain_unaccepted_delivery(
    gate: &mut MonthlyPolicyGate,
    delivered: Option<InFlightMonthlyUsage>,
    current_period: AccountUsagePeriod,
    usage_update_applied: bool,
) -> Option<CapturedUsageUpdate> {
    let delivered = delivered?;
    if !usage_update_applied {
        if delivered.captured.update.period == gate.period
            || delivered.captured.update.period == current_period
        {
            let failed_discrete_usage = LocalMonthlyUsage {
                fuel: delivered.usage.fuel,
                ..LocalMonthlyUsage::default()
            };
            if !failed_discrete_usage.is_zero() {
                gate.failed_delivery_usage
                    .entry(delivered.captured.update.period)
                    .or_default()
                    .saturating_add_nonnegative_assign(failed_discrete_usage);
            }
        }
        return delivered.captured.retryable_byte_time();
    }
    if delivered.captured.update.period == gate.period
        || delivered.captured.update.period == current_period
    {
        let usage = delivered.usage.positive();
        if !usage.is_zero() {
            gate.stale_delivered_usage.push(StaleDeliveredUsage {
                period: delivered.captured.update.period,
                usage,
                observed_refresh_generation: gate.refresh_generation,
            });
        }
    }
    None
}

impl AtomicResourceEntry {
    #[cfg(test)]
    pub(super) fn update_policy_revision(&self, new_revision: u64) {
        self.flush_active_resource_usage();
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        self.update_usage_revision_locked(&mut revision_state, new_revision);
    }

    pub(super) fn update_usage_revision_locked(
        &self,
        revision_state: &mut UsageRevisionState,
        new_revision: u64,
    ) {
        if revision_state.current_policy_revision >= new_revision {
            return;
        }

        loop {
            let active = (self.metering.compute && self.delta.load(Ordering::Acquire) != 0)
                || self
                    .account_usage_accumulator
                    .as_ref()
                    .is_some_and(|accumulator| accumulator.lock().unwrap().is_active())
                || self.unsynced_http_calls.load(Ordering::Acquire) > 0
                || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0
                || self.blob_storage_quota.lock().unwrap().unsynced_delta != 0;
            if !active {
                break;
            }
            let captured = self.capture_current_usage(
                revision_state.current_mode_revision,
                revision_state.current_policy_revision,
                revision_state.current_period,
                false,
            );
            revision_state.pending.push_back(captured);
        }
        let captured = self.capture_current_usage(
            revision_state.current_mode_revision,
            revision_state.current_policy_revision,
            revision_state.current_period,
            true,
        );
        if captured.update.memory_byte_nanoseconds_remainder != 0
            || captured.update.durable_storage_byte_nanoseconds_remainder != 0
            || captured.update.ephemeral_storage_byte_nanoseconds_remainder != 0
            || captured.update.blob_storage_bytes_delta != 0
        {
            revision_state.pending.push_back(captured);
        }
        revision_state.current_policy_revision = new_revision;
    }

    pub(super) fn update_usage_period_locked(
        &self,
        revision_state: &mut UsageRevisionState,
        new_period: AccountUsagePeriod,
    ) {
        if revision_state.current_period >= new_period {
            return;
        }

        loop {
            let active = (self.metering.compute && self.delta.load(Ordering::Acquire) != 0)
                || self
                    .account_usage_accumulator
                    .as_ref()
                    .is_some_and(|accumulator| accumulator.lock().unwrap().is_active())
                || self.unsynced_http_calls.load(Ordering::Acquire) > 0
                || self.unsynced_rpc_calls.load(Ordering::Acquire) > 0
                || self.blob_storage_quota.lock().unwrap().unsynced_delta != 0;
            if !active {
                break;
            }
            let captured = self.capture_current_usage(
                revision_state.current_mode_revision,
                revision_state.current_policy_revision,
                revision_state.current_period,
                false,
            );
            revision_state.pending.push_back(captured);
        }
        let captured = self.capture_current_usage(
            revision_state.current_mode_revision,
            revision_state.current_policy_revision,
            revision_state.current_period,
            true,
        );
        if captured.update.memory_byte_nanoseconds_remainder != 0
            || captured.update.durable_storage_byte_nanoseconds_remainder != 0
            || captured.update.ephemeral_storage_byte_nanoseconds_remainder != 0
            || captured.update.blob_storage_bytes_delta != 0
        {
            revision_state.pending.push_back(captured);
        }
        revision_state.current_period = new_period;
    }

    pub(super) fn begin_monthly_refresh(
        &self,
        generation: u64,
        update: &ResourceUsageUpdate,
        durable_memory_gb_seconds: i64,
        ephemeral_memory_gb_seconds: i64,
    ) {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        if let Some(gate) = &mut revision_state.monthly_policy {
            gate.refresh_generation = gate.refresh_generation.max(generation);
            let usage = LocalMonthlyUsage::from_update(update);
            if !usage.is_zero() {
                let remaining_unassigned = gate
                    .unassigned_in_flight_usage
                    .get(&update.period)
                    .copied()
                    .unwrap_or_default();
                let mut remaining_unassigned = remaining_unassigned;
                remaining_unassigned.saturating_sub_assign(usage);
                if remaining_unassigned.is_zero() {
                    gate.unassigned_in_flight_usage.remove(&update.period);
                } else {
                    gate.unassigned_in_flight_usage
                        .insert(update.period, remaining_unassigned);
                }
                let previous = gate.in_flight_usage.insert(
                    generation,
                    InFlightMonthlyUsage {
                        captured: CapturedUsageUpdate {
                            update: *update,
                            durable_memory_gb_seconds_delta: durable_memory_gb_seconds,
                            ephemeral_memory_gb_seconds_delta: ephemeral_memory_gb_seconds,
                        },
                        usage,
                    },
                );
                assert!(previous.is_none(), "refresh generation must be unique");
            }
        }
    }

    pub(super) fn finish_monthly_delivery(
        &self,
        generation: u64,
        gate: &mut MonthlyPolicyGate,
    ) -> Option<InFlightMonthlyUsage> {
        let delivered = gate.in_flight_usage.remove(&generation);
        if let Some(delivered) = delivered {
            self.in_flight_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(in_flight.saturating_sub(delivered.usage.fuel as i64))
                })
                .ok();
            self.in_flight_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(
                        in_flight.saturating_sub(delivered.captured.update.memory_gb_seconds_delta),
                    )
                })
                .ok();
            self.in_flight_durable_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(
                        in_flight
                            .saturating_sub(delivered.captured.durable_memory_gb_seconds_delta),
                    )
                })
                .ok();
            self.in_flight_ephemeral_memory_gb_seconds_delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |in_flight| {
                    Some(
                        in_flight
                            .saturating_sub(delivered.captured.ephemeral_memory_gb_seconds_delta),
                    )
                })
                .ok();
        }
        delivered
    }

    #[cfg(test)]
    pub(super) fn apply_monthly_snapshot(
        &self,
        generation: u64,
        monthly_policy: MonthlyResourcePolicy,
        monthly_usage_mode_revision: u64,
        usage_update_applied: bool,
    ) -> bool {
        self.apply_monthly_snapshot_with_policy_revision(
            generation,
            monthly_policy,
            monthly_usage_mode_revision,
            monthly_usage_mode_revision,
            golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            usage_update_applied,
        )
    }

    pub(super) fn apply_monthly_snapshot_with_policy_revision(
        &self,
        generation: u64,
        monthly_policy: MonthlyResourcePolicy,
        monthly_usage_mode_revision: u64,
        monthly_policy_revision: u64,
        policy_status: golem_service_base::model::MonthlyResourcePolicyStatus,
        usage_update_applied: bool,
    ) -> bool {
        self.flush_active_resource_usage();
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        let current_period = revision_state.current_period;
        let current_policy_revision = revision_state.current_policy_revision;
        if revision_state.monthly_policy.is_none() {
            self.update_usage_revision_locked(&mut revision_state, monthly_policy_revision);
            revision_state.current_mode_revision = monthly_usage_mode_revision;
            self.update_usage_period_locked(&mut revision_state, monthly_policy.period);
            return true;
        }
        let gate = revision_state
            .monthly_policy
            .as_mut()
            .expect("monthly policy gate was checked above");
        let delivered = self.finish_monthly_delivery(generation, gate);
        if generation != gate.refresh_generation || generation <= gate.settled_generation {
            if let Some(captured) =
                retain_unaccepted_delivery(gate, delivered, current_period, usage_update_applied)
            {
                revision_state.pending.push_front(captured);
            }
            return false;
        }
        if monthly_policy_revision < current_policy_revision
            || monthly_policy_revision < gate.minimum_policy_revision
        {
            let retry =
                retain_unaccepted_delivery(gate, delivered, current_period, usage_update_applied);
            gate.settled_generation = generation;
            if let Some(captured) = retry {
                revision_state.pending.push_front(captured);
            }
            return false;
        }

        if policy_status == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable {
            let retry =
                retain_unaccepted_delivery(gate, delivered, current_period, usage_update_applied);
            gate.policy_status = policy_status;
            gate.minimum_policy_revision = monthly_policy_revision;
            gate.settled_generation = generation;
            if let Some(captured) = retry {
                revision_state.pending.push_front(captured);
            }
            drop(revision_state);
            self.capacity_updates
                .send_modify(|version| *version = version.wrapping_add(1));
            return true;
        }

        self.update_usage_revision_locked(&mut revision_state, monthly_policy_revision);
        revision_state.current_mode_revision = monthly_usage_mode_revision;
        self.update_usage_period_locked(&mut revision_state, monthly_policy.period);
        let gate = revision_state
            .monthly_policy
            .as_mut()
            .expect("monthly policy gate was checked above");
        if usage_update_applied
            && let Some(delivered) = delivered
            && delivered.captured.update.period > monthly_policy.period
        {
            let usage = delivered.usage.positive();
            if !usage.is_zero() {
                gate.stale_delivered_usage.push(StaleDeliveredUsage {
                    period: delivered.captured.update.period,
                    usage,
                    observed_refresh_generation: generation,
                });
            }
        }
        gate.failed_delivery_usage
            .retain(|period, _| *period >= monthly_policy.period);
        gate.hard_limit_memory_overshoot_byte_nanoseconds
            .retain(|(period, _), _| *period >= monthly_policy.period);
        gate.hard_limit_durable_storage_overshoot_byte_nanoseconds
            .retain(|(period, _), _| *period >= monthly_policy.period);
        gate.hard_limit_ephemeral_storage_overshoot_byte_nanoseconds
            .retain(|(period, _), _| *period >= monthly_policy.period);
        gate.stale_delivered_usage.retain(|retained| {
            retained.period > monthly_policy.period
                || (retained.period == monthly_policy.period
                    && generation <= retained.observed_refresh_generation)
        });
        let retry = if usage_update_applied {
            None
        } else {
            retain_unaccepted_delivery(gate, delivered, current_period, false)
        };
        gate.period = monthly_policy.period;
        gate.policy_status = policy_status;
        gate.minimum_policy_revision = monthly_policy_revision;
        gate.mode = monthly_policy.mode;
        gate.available_fuel = self
            .metering
            .compute
            .then_some(monthly_policy.available_fuel);
        gate.available_memory_byte_nanoseconds = self
            .metering
            .memory
            .then(|| monthly_policy_available_memory_byte_nanoseconds(&monthly_policy));
        gate.available_durable_storage_byte_nanoseconds = self.metering.filesystem.then(|| {
            monthly_policy_available_storage_byte_nanoseconds(&monthly_policy, AgentMode::Durable)
        });
        gate.available_ephemeral_storage_byte_nanoseconds = self.metering.filesystem.then(|| {
            monthly_policy_available_storage_byte_nanoseconds(&monthly_policy, AgentMode::Ephemeral)
        });
        gate.settled_generation = generation;
        if let Some(captured) = retry {
            revision_state.pending.push_front(captured);
        }
        drop(revision_state);
        self.capacity_updates
            .send_modify(|version| *version = version.wrapping_add(1));
        true
    }

    pub(super) fn finish_ambiguous_transport_failure(&self, generation: u64) -> bool {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        let current_period = revision_state.current_period;
        let Some(gate) = &mut revision_state.monthly_policy else {
            return true;
        };
        let delivered = self.finish_monthly_delivery(generation, gate);
        if let Some(delivered) = delivered
            && (delivered.captured.update.period == gate.period
                || delivered.captured.update.period == current_period)
        {
            gate.failed_delivery_usage
                .entry(delivered.captured.update.period)
                .or_default()
                .saturating_add_nonnegative_assign(delivered.usage);
        }
        if generation != gate.refresh_generation || generation <= gate.settled_generation {
            return false;
        }
        gate.settled_generation = generation;
        true
    }

    pub(super) fn finish_confirmed_non_application(&self, generation: u64) -> bool {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        let current_period = revision_state.current_period;
        let Some(gate) = &mut revision_state.monthly_policy else {
            return true;
        };
        let delivered = self.finish_monthly_delivery(generation, gate);
        let retry = retain_unaccepted_delivery(gate, delivered, current_period, false);
        let latest = generation == gate.refresh_generation && generation > gate.settled_generation;
        if latest {
            gate.settled_generation = generation;
        }
        if let Some(captured) = retry {
            revision_state.pending.push_front(captured);
        }
        latest
    }

    #[cfg(test)]
    pub(crate) fn update_policy_revision_for_test(&self, new_revision: u64) {
        self.update_policy_revision(new_revision);
    }

    #[cfg(test)]
    pub(crate) fn apply_monthly_snapshot_for_test(
        &self,
        generation: u64,
        monthly_policy: MonthlyResourcePolicy,
        monthly_usage_mode_revision: u64,
    ) -> bool {
        self.begin_monthly_refresh_for_test(generation, 0);
        self.apply_monthly_snapshot(
            generation,
            monthly_policy,
            monthly_usage_mode_revision,
            true,
        )
    }

    #[cfg(test)]
    pub(super) fn begin_monthly_refresh_for_test(&self, generation: u64, fuel_delta: i64) {
        self.begin_monthly_refresh_with_memory_for_test(generation, fuel_delta, 0);
    }

    #[cfg(test)]
    pub(super) fn begin_monthly_refresh_with_memory_for_test(
        &self,
        generation: u64,
        fuel_delta: i64,
        memory_gb_seconds_delta: i64,
    ) {
        let period = self.usage_revision_state.lock().unwrap().current_period;
        let update = ResourceUsageUpdate {
            blob_storage_bytes_delta: 0,
            period,
            monthly_usage_mode_revision: 0,
            monthly_policy_revision: 0,
            memory_byte_nanoseconds_remainder: 0,
            durable_storage_byte_nanoseconds_remainder: 0,
            ephemeral_storage_byte_nanoseconds_remainder: 0,
            fuel_delta,
            http_call_count_delta: 0,
            rpc_call_count_delta: 0,
            durable_storage_byte_seconds_delta: 0,
            ephemeral_storage_byte_seconds_delta: 0,
            memory_gb_seconds_delta,
            metering: golem_service_base::clients::registry::ResourceUsageMetering {
                compute: self.metering.compute,
                memory: self.metering.memory,
                filesystem: self.metering.filesystem,
            },
        };
        self.begin_monthly_refresh(generation, &update, memory_gb_seconds_delta, 0);
    }

    pub(super) fn advance_to_current_usage_period(&self) -> AccountUsagePeriod {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        self.update_usage_period_locked(&mut revision_state, AccountUsagePeriod::current());
        revision_state.current_period
    }
}
