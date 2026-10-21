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

impl LocalMonthlyUsage {
    pub(super) fn from_update(update: &ResourceUsageUpdate) -> Self {
        Self {
            fuel: update.fuel_delta as i128,
            memory_byte_nanoseconds: (update.memory_gb_seconds_delta.max(0) as u128)
                .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
                .saturating_add(update.memory_byte_nanoseconds_remainder as u128),
            durable_storage_byte_nanoseconds: (update.durable_storage_byte_seconds_delta.max(0)
                as u128)
                .saturating_mul(1_000_000_000)
                .saturating_add(update.durable_storage_byte_nanoseconds_remainder as u128),
            ephemeral_storage_byte_nanoseconds: (update.ephemeral_storage_byte_seconds_delta.max(0)
                as u128)
                .saturating_mul(1_000_000_000)
                .saturating_add(update.ephemeral_storage_byte_nanoseconds_remainder as u128),
        }
    }

    pub(super) fn is_zero(self) -> bool {
        self.fuel == 0
            && self.memory_byte_nanoseconds == 0
            && self.durable_storage_byte_nanoseconds == 0
            && self.ephemeral_storage_byte_nanoseconds == 0
    }

    pub(super) fn positive(self) -> Self {
        Self {
            fuel: self.fuel.max(0),
            memory_byte_nanoseconds: self.memory_byte_nanoseconds,
            durable_storage_byte_nanoseconds: self.durable_storage_byte_nanoseconds,
            ephemeral_storage_byte_nanoseconds: self.ephemeral_storage_byte_nanoseconds,
        }
    }

    pub(super) fn saturating_add_assign(&mut self, other: Self) {
        self.fuel = self.fuel.saturating_add(other.fuel);
        self.memory_byte_nanoseconds = self
            .memory_byte_nanoseconds
            .saturating_add(other.memory_byte_nanoseconds);
        self.durable_storage_byte_nanoseconds = self
            .durable_storage_byte_nanoseconds
            .saturating_add(other.durable_storage_byte_nanoseconds);
        self.ephemeral_storage_byte_nanoseconds = self
            .ephemeral_storage_byte_nanoseconds
            .saturating_add(other.ephemeral_storage_byte_nanoseconds);
    }

    pub(super) fn saturating_sub_assign(&mut self, other: Self) {
        self.fuel = self.fuel.saturating_sub(other.fuel);
        self.memory_byte_nanoseconds = self
            .memory_byte_nanoseconds
            .saturating_sub(other.memory_byte_nanoseconds);
        self.durable_storage_byte_nanoseconds = self
            .durable_storage_byte_nanoseconds
            .saturating_sub(other.durable_storage_byte_nanoseconds);
        self.ephemeral_storage_byte_nanoseconds = self
            .ephemeral_storage_byte_nanoseconds
            .saturating_sub(other.ephemeral_storage_byte_nanoseconds);
    }

    pub(super) fn saturating_add_nonnegative_assign(&mut self, other: Self) {
        self.saturating_add_assign(other);
        self.fuel = self.fuel.max(0);
    }
}

pub(super) fn monthly_policy_available_memory_byte_nanoseconds(
    policy: &MonthlyResourcePolicy,
) -> u128 {
    (policy.available_memory_gb_seconds as u128)
        .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
        .saturating_add(policy.available_memory_byte_nanoseconds_remainder as u128)
}

pub(super) fn monthly_policy_available_storage_byte_nanoseconds(
    policy: &MonthlyResourcePolicy,
    mode: AgentMode,
) -> u128 {
    let (seconds, remainder) = match mode {
        AgentMode::Durable => (
            policy.available_durable_storage_byte_seconds,
            policy.available_durable_storage_byte_nanoseconds_remainder,
        ),
        AgentMode::Ephemeral => (
            policy.available_ephemeral_storage_byte_seconds,
            policy.available_ephemeral_storage_byte_nanoseconds_remainder,
        ),
    };
    (seconds as u128)
        .saturating_mul(1_000_000_000)
        .saturating_add(remainder as u128)
}

impl AtomicResourceEntry {
    #[cfg(test)]
    pub(super) fn effective_fuel(&self) -> u64 {
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.effective_fuel_with_revision_state(&revision_state)
    }

    pub(super) fn effective_fuel_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
    ) -> u64 {
        let Some(gate) = &revision_state.monthly_policy else {
            return u64::MAX;
        };
        let Some(available_fuel) = gate.available_fuel else {
            return u64::MAX;
        };
        (available_fuel as i128)
            .saturating_sub(self.local_monthly_usage(revision_state).fuel)
            .clamp(0, u64::MAX as i128) as u64
    }

    #[cfg(test)]
    pub(super) fn effective_memory_gb_seconds(&self) -> u64 {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.effective_memory_with_revision_state(&revision_state)
    }

    #[cfg(test)]
    pub(super) fn effective_memory_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
    ) -> u64 {
        (self.effective_memory_byte_nanoseconds_with_revision_state(revision_state)
            / BYTE_NANOSECONDS_PER_GB_SECOND)
            .min(u64::MAX as u128) as u64
    }

    pub(super) fn effective_memory_byte_nanoseconds_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
    ) -> u128 {
        let Some(gate) = &revision_state.monthly_policy else {
            return (u64::MAX as u128).saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND);
        };
        let Some(available_memory) = gate.available_memory_byte_nanoseconds else {
            return (u64::MAX as u128).saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND);
        };
        available_memory.saturating_sub(
            self.local_monthly_usage(revision_state)
                .memory_byte_nanoseconds,
        )
    }

    pub(crate) fn monthly_memory_check_after(&self, allocated_bytes: u64) -> Option<Duration> {
        if allocated_bytes == 0 || !self.metering.memory {
            return None;
        }
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        self.update_usage_period_locked(&mut revision_state, AccountUsagePeriod::current());
        let remaining = self.memory_settlement_limit(&revision_state)?;
        if remaining == 0 {
            return None;
        }
        let nanoseconds = remaining.div_ceil(allocated_bytes as u128);
        Some(Duration::from_nanos(
            nanoseconds.min(u64::MAX as u128) as u64
        ))
    }

    pub(super) fn memory_settlement_limit(
        &self,
        revision_state: &UsageRevisionState,
    ) -> Option<u128> {
        let gate = revision_state.monthly_policy.as_ref()?;
        (gate.period == revision_state.current_period
            && gate.mode == MonthlyUsageMode::HardLimit
            && gate.available_memory_byte_nanoseconds.is_some())
        .then(|| self.effective_memory_byte_nanoseconds_with_revision_state(revision_state))
    }

    pub(super) fn effective_storage_byte_nanoseconds_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
        mode: AgentMode,
    ) -> u128 {
        let Some(gate) = &revision_state.monthly_policy else {
            return (u64::MAX as u128).saturating_mul(1_000_000_000);
        };
        let available = match mode {
            AgentMode::Durable => gate.available_durable_storage_byte_nanoseconds,
            AgentMode::Ephemeral => gate.available_ephemeral_storage_byte_nanoseconds,
        };
        let Some(available) = available else {
            return (u64::MAX as u128).saturating_mul(1_000_000_000);
        };
        let usage = self.local_monthly_usage(revision_state);
        available.saturating_sub(match mode {
            AgentMode::Durable => usage.durable_storage_byte_nanoseconds,
            AgentMode::Ephemeral => usage.ephemeral_storage_byte_nanoseconds,
        })
    }

    pub(super) fn storage_settlement_limit(
        &self,
        revision_state: &UsageRevisionState,
        mode: AgentMode,
    ) -> Option<u128> {
        let gate = revision_state.monthly_policy.as_ref()?;
        let available = match mode {
            AgentMode::Durable => gate.available_durable_storage_byte_nanoseconds,
            AgentMode::Ephemeral => gate.available_ephemeral_storage_byte_nanoseconds,
        };
        (gate.period == revision_state.current_period
            && gate.mode == MonthlyUsageMode::HardLimit
            && available.is_some())
        .then(|| self.effective_storage_byte_nanoseconds_with_revision_state(revision_state, mode))
    }

    pub(super) fn record_hard_limit_memory_overshoot(
        revision_state: &mut UsageRevisionState,
        amount: u128,
    ) {
        if amount == 0 {
            return;
        }
        let key = (
            revision_state.current_period,
            revision_state.current_policy_revision,
        );
        let gate = revision_state
            .monthly_policy
            .as_mut()
            .expect("memory overshoot was recorded without an active monthly policy");
        let overshoot = gate
            .hard_limit_memory_overshoot_byte_nanoseconds
            .entry(key)
            .or_default();
        *overshoot = overshoot.saturating_add(amount);
    }

    pub(super) fn record_hard_limit_storage_overshoot(
        revision_state: &mut UsageRevisionState,
        mode: AgentMode,
        amount: u128,
    ) {
        if amount == 0 {
            return;
        }
        let key = (
            revision_state.current_period,
            revision_state.current_policy_revision,
        );
        let gate = revision_state
            .monthly_policy
            .as_mut()
            .expect("storage overshoot was recorded without an active monthly policy");
        let overshoots = match mode {
            AgentMode::Durable => &mut gate.hard_limit_durable_storage_overshoot_byte_nanoseconds,
            AgentMode::Ephemeral => {
                &mut gate.hard_limit_ephemeral_storage_overshoot_byte_nanoseconds
            }
        };
        let overshoot = overshoots.entry(key).or_default();
        *overshoot = overshoot.saturating_add(amount);
    }

    pub(super) fn local_monthly_usage(
        &self,
        revision_state: &UsageRevisionState,
    ) -> LocalMonthlyUsage {
        let gate = revision_state
            .monthly_policy
            .as_ref()
            .expect("monthly policy gate is active");
        let period_matches = |period: AccountUsagePeriod| {
            period == gate.period || period == revision_state.current_period
        };
        let mut usage = LocalMonthlyUsage {
            fuel: self.delta.load(Ordering::Acquire) as i128,
            memory_byte_nanoseconds: self.account_usage_accumulator.as_ref().map_or(
                0,
                |accumulator| {
                    let accumulator = accumulator.lock().unwrap();
                    accumulator
                        .total_memory()
                        .saturating_mul(BYTE_NANOSECONDS_PER_GB_SECOND)
                        .saturating_add(accumulator.memory_remainder())
                },
            ),
            durable_storage_byte_nanoseconds: self.account_usage_accumulator.as_ref().map_or(
                0,
                |accumulator| {
                    let accumulator = accumulator.lock().unwrap();
                    accumulator
                        .storage(AgentMode::Durable)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(
                            accumulator
                                .storage
                                .as_ref()
                                .map_or(0, |storage| storage.durable_storage_remainder),
                        )
                },
            ),
            ephemeral_storage_byte_nanoseconds: self.account_usage_accumulator.as_ref().map_or(
                0,
                |accumulator| {
                    let accumulator = accumulator.lock().unwrap();
                    accumulator
                        .storage(AgentMode::Ephemeral)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(
                            accumulator
                                .storage
                                .as_ref()
                                .map_or(0, |storage| storage.ephemeral_storage_remainder),
                        )
                },
            ),
        };
        for in_flight in gate
            .in_flight_usage
            .values()
            .filter(|in_flight| period_matches(in_flight.captured.update.period))
        {
            usage.saturating_add_assign(in_flight.usage);
        }
        for local in gate
            .unassigned_in_flight_usage
            .iter()
            .filter(|(period, _)| period_matches(**period))
            .map(|(_, usage)| *usage)
        {
            usage.saturating_add_assign(local);
        }
        for local in revision_state
            .pending
            .iter()
            .filter(|captured| period_matches(captured.update.period))
            .map(|captured| LocalMonthlyUsage::from_update(&captured.update))
        {
            usage.saturating_add_assign(local);
        }
        for local in gate
            .stale_delivered_usage
            .iter()
            .filter(|delivered| period_matches(delivered.period))
            .map(|delivered| delivered.usage.positive())
        {
            usage.saturating_add_assign(local);
        }
        for local in gate
            .failed_delivery_usage
            .iter()
            .filter(|(period, _)| period_matches(**period))
            .map(|(_, usage)| *usage)
        {
            usage.saturating_add_assign(local);
        }
        usage
    }

    pub(crate) fn monthly_metering_enabled(&self) -> bool {
        self.metering.compute || self.metering.memory || self.metering.filesystem
    }

    pub fn subscribe_capacity_updates(&self) -> tokio::sync::watch::Receiver<u64> {
        self.capacity_updates.subscribe()
    }

    pub fn with_monthly_capacity<R>(
        &self,
        agent_mode: AgentMode,
        fuel_generation: Option<u64>,
        read: impl FnOnce(MonthlyCapacity) -> R,
    ) -> R {
        let mut state = self.usage_revision_state.lock().unwrap();
        self.update_usage_period_locked(&mut state, AccountUsagePeriod::current());
        read(self.monthly_capacity_snapshot(&state, agent_mode, fuel_generation))
    }

    pub(super) fn monthly_capacity_snapshot(
        &self,
        state: &UsageRevisionState,
        agent_mode: AgentMode,
        fuel_generation: Option<u64>,
    ) -> MonthlyCapacity {
        let policy_status = state.monthly_policy.as_ref().map_or(
            golem_service_base::model::MonthlyResourcePolicyStatus::Available,
            |gate| gate.policy_status,
        );
        let exhaustion = state.monthly_policy.as_ref().and_then(|gate| {
            if gate.policy_status
                != golem_service_base::model::MonthlyResourcePolicyStatus::Available
                || gate.mode != MonthlyUsageMode::HardLimit
                || gate.period != state.current_period
            {
                return None;
            }
            if gate.available_fuel.is_some()
                && self.effective_fuel_with_revision_state(state) == 0
                && fuel_generation.is_none_or(|generation| gate.settled_generation > generation)
            {
                Some(MonthlyResourceExhaustion::Compute)
            } else {
                self.monthly_memory_and_storage_capacity_with_revision_state(state, agent_mode)
                    .err()
            }
        });
        MonthlyCapacity {
            policy_status,
            exhaustion,
            policy_revision: state.current_policy_revision,
            period: state.current_period,
            fuel_generation,
        }
    }

    pub(crate) fn ensure_monthly_policy_available(
        &self,
    ) -> Result<(), MonthlyResourceCapacityError> {
        let state = self.usage_revision_state.lock().unwrap();
        if state.monthly_policy.as_ref().is_some_and(|gate| {
            gate.policy_status
                == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
        }) {
            Err(MonthlyResourceCapacityError::Unavailable)
        } else {
            Ok(())
        }
    }

    pub(crate) fn monthly_resource_capacity(
        &self,
        agent_mode: AgentMode,
    ) -> Result<(), MonthlyResourceCapacityError> {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        let Some(gate) = revision_state.monthly_policy.as_ref() else {
            return Ok(());
        };
        if gate.policy_status == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
        {
            return Err(MonthlyResourceCapacityError::Unavailable);
        }
        if gate.mode == MonthlyUsageMode::AllowOverage {
            return Ok(());
        }
        if gate.available_fuel.is_some()
            && self.effective_fuel_with_revision_state(&revision_state) == 0
        {
            return Err(MonthlyResourceExhaustion::Compute.into());
        }
        self.monthly_memory_and_storage_capacity_with_revision_state(&revision_state, agent_mode)
            .map_err(Into::into)
    }

    pub(crate) fn monthly_memory_and_storage_capacity(
        &self,
        agent_mode: AgentMode,
    ) -> Result<(), MonthlyResourceCapacityError> {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        let Some(gate) = revision_state.monthly_policy.as_ref() else {
            return Ok(());
        };
        if gate.policy_status == golem_service_base::model::MonthlyResourcePolicyStatus::Unavailable
        {
            return Err(MonthlyResourceCapacityError::Unavailable);
        }
        if gate.mode == MonthlyUsageMode::AllowOverage {
            return Ok(());
        }
        self.monthly_memory_and_storage_capacity_with_revision_state(&revision_state, agent_mode)
            .map_err(Into::into)
    }

    pub(super) fn monthly_memory_and_storage_capacity_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
        agent_mode: AgentMode,
    ) -> Result<(), MonthlyResourceExhaustion> {
        let gate = revision_state
            .monthly_policy
            .as_ref()
            .expect("monthly policy was checked before resource capacity");
        if !self.has_monthly_memory_capacity_with_revision_state(revision_state) {
            return Err(MonthlyResourceExhaustion::Memory);
        }
        let storage_enabled = match agent_mode {
            AgentMode::Durable => gate.available_durable_storage_byte_nanoseconds.is_some(),
            AgentMode::Ephemeral => gate.available_ephemeral_storage_byte_nanoseconds.is_some(),
        };
        if storage_enabled
            && self
                .effective_storage_byte_nanoseconds_with_revision_state(revision_state, agent_mode)
                == 0
        {
            return Err(match agent_mode {
                AgentMode::Durable => MonthlyResourceExhaustion::DurableStorage,
                AgentMode::Ephemeral => MonthlyResourceExhaustion::EphemeralStorage,
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn has_monthly_memory_capacity(&self) -> bool {
        self.flush_active_resource_usage();
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.has_monthly_memory_capacity_with_revision_state(&revision_state)
    }

    pub(super) fn has_monthly_memory_capacity_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
    ) -> bool {
        revision_state.monthly_policy.as_ref().is_none_or(|gate| {
            gate.mode == MonthlyUsageMode::AllowOverage
                || gate.available_memory_byte_nanoseconds.is_none()
                || self.effective_memory_byte_nanoseconds_with_revision_state(revision_state) > 0
        })
    }

    pub(crate) fn hard_limit_attribution_after(
        &self,
        generation: Option<u64>,
    ) -> Option<(u64, AccountUsagePeriod)> {
        let revision_state = self.usage_revision_state.lock().unwrap();
        revision_state.monthly_policy.as_ref().and_then(|gate| {
            (gate.mode == MonthlyUsageMode::HardLimit
                && gate.available_fuel.is_some()
                && self.effective_fuel_with_revision_state(&revision_state) == 0
                && generation.is_none_or(|generation| gate.settled_generation > generation))
            .then_some((revision_state.current_policy_revision, gate.period))
        })
    }

    pub(crate) fn record_hard_limit_overshoot(
        &self,
        amount: u64,
        revision: u64,
        period: AccountUsagePeriod,
    ) {
        if amount > 0 {
            self.record_fuel_delta_for_revision_and_period(
                revision,
                period,
                amount.min(i64::MAX as u64) as i64,
            );
        }
    }
}
