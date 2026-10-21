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

impl AtomicResourceEntry {
    #[cfg(test)]
    pub(crate) fn fuel_delta(&self) -> i64 {
        self.delta.load(Ordering::Acquire)
    }

    pub fn borrow_fuel(&self, amount: u64) -> bool {
        matches!(
            self.borrow_fuel_with_revision(amount),
            FuelBorrow::Borrowed { .. }
        )
    }

    pub(crate) fn borrow_fuel_with_revision(&self, amount: u64) -> FuelBorrow {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        if self.metering.compute {
            self.update_usage_period_locked(&mut revision_state, AccountUsagePeriod::current());
        }
        let revision = revision_state.current_policy_revision;
        let period = revision_state.current_period;
        let generation = revision_state
            .monthly_policy
            .as_ref()
            .map_or(0, |gate| gate.settled_generation);
        if amount == 0 {
            return FuelBorrow::Borrowed {
                amount: 0,
                revision,
                generation,
                period,
            };
        }
        if !self.metering.compute {
            return FuelBorrow::Borrowed {
                amount,
                revision,
                generation,
                period,
            };
        }
        let Some(gate) = &revision_state.monthly_policy else {
            return FuelBorrow::Borrowed {
                amount,
                revision,
                generation,
                period,
            };
        };
        let borrowed = match gate.mode {
            MonthlyUsageMode::AllowOverage => amount,
            MonthlyUsageMode::HardLimit => {
                amount.min(self.effective_fuel_with_revision_state(&revision_state))
            }
        };

        if borrowed > 0 {
            let amt_i64 = borrowed.min(i64::MAX as u64) as i64;
            self.delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |d| {
                    Some(d.saturating_add(amt_i64))
                })
                .ok();
            record_fuel_borrow(borrowed);
            FuelBorrow::Borrowed {
                amount: borrowed,
                revision,
                generation,
                period,
            }
        } else {
            FuelBorrow::Exhausted {
                revision,
                generation,
                period,
            }
        }
    }

    pub fn return_fuel(&self, amount: u64) {
        let revision = self
            .usage_revision_state
            .lock()
            .unwrap()
            .current_policy_revision;
        self.return_fuel_for_revision(amount, revision);
    }

    pub(crate) fn return_fuel_for_revision(&self, amount: u64, revision: u64) {
        let period = self.usage_revision_state.lock().unwrap().current_period;
        self.return_fuel_for_revision_and_period(amount, revision, period);
    }

    pub(crate) fn return_fuel_for_revision_and_period(
        &self,
        amount: u64,
        revision: u64,
        period: AccountUsagePeriod,
    ) {
        if !self.metering.compute {
            return;
        }
        let amt_i64 = amount.min(i64::MAX as u64) as i64;
        self.record_fuel_delta_for_revision_and_period(revision, period, -amt_i64);
        record_fuel_return(amount);
    }

    pub fn record_overdraft_debt(&self, amount: u64) {
        let revision = self
            .usage_revision_state
            .lock()
            .unwrap()
            .current_policy_revision;
        self.record_overdraft_debt_for_revision(amount, revision);
    }

    pub(crate) fn record_overdraft_debt_for_revision(&self, amount: u64, revision: u64) {
        if !self.metering.compute || amount == 0 {
            return;
        }

        let amt_i64 = amount.min(i64::MAX as u64) as i64;
        self.record_fuel_delta_for_revision(revision, amt_i64);
        record_ephemeral_overdraft_fuel(amount);
    }

    pub(super) fn record_fuel_delta_for_revision(&self, revision: u64, fuel_delta: i64) {
        let period = self.usage_revision_state.lock().unwrap().current_period;
        self.record_fuel_delta_for_revision_and_period(revision, period, fuel_delta);
    }

    pub(super) fn record_fuel_delta_for_revision_and_period(
        &self,
        revision: u64,
        period: AccountUsagePeriod,
        fuel_delta: i64,
    ) {
        let mut revision_state = self.usage_revision_state.lock().unwrap();
        assert!(
            revision <= revision_state.current_policy_revision,
            "fuel usage references future revision {revision}; current revision is {}",
            revision_state.current_policy_revision
        );
        if revision == revision_state.current_policy_revision
            && period == revision_state.current_period
        {
            self.delta
                .try_update(Ordering::AcqRel, Ordering::Acquire, |delta| {
                    Some(delta.saturating_add(fuel_delta))
                })
                .ok();
        } else {
            let monthly_usage_mode_revision = revision_state.current_mode_revision;
            revision_state.pending.push_back(CapturedUsageUpdate {
                update: ResourceUsageUpdate {
                    blob_storage_bytes_delta: 0,
                    period,
                    monthly_usage_mode_revision,
                    monthly_policy_revision: revision,
                    memory_byte_nanoseconds_remainder: 0,
                    durable_storage_byte_nanoseconds_remainder: 0,
                    ephemeral_storage_byte_nanoseconds_remainder: 0,
                    fuel_delta,
                    http_call_count_delta: 0,
                    rpc_call_count_delta: 0,
                    durable_storage_byte_seconds_delta: 0,
                    ephemeral_storage_byte_seconds_delta: 0,
                    memory_gb_seconds_delta: 0,
                    metering: golem_service_base::clients::registry::ResourceUsageMetering {
                        compute: self.metering.compute,
                        memory: self.metering.memory,
                        filesystem: self.metering.filesystem,
                    },
                },
                durable_memory_gb_seconds_delta: 0,
                ephemeral_memory_gb_seconds_delta: 0,
            });
        }
    }
}
