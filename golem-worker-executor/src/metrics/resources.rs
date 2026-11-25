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

use golem_common::base_model::account_usage::BYTE_NANOSECONDS_PER_GB_SECOND;
use golem_common::metrics::catalog;
use golem_common::model::agent::AgentMode;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref FUEL_BORROW_TOTAL: Counter = catalog::FUEL_BORROW_TOTAL.register();
    static ref FUEL_RETURN_TOTAL: Counter = catalog::FUEL_RETURN_TOTAL.register();
    static ref EPHEMERAL_OVERDRAFT_FUEL_TOTAL: Counter =
        catalog::EPHEMERAL_OVERDRAFT_FUEL_TOTAL.register();
    static ref STORAGE_BYTE_SECONDS_TOTAL: CounterVec =
        catalog::STORAGE_BYTE_SECONDS_TOTAL.register();
    static ref MEMORY_BYTE_SECONDS_TOTAL: CounterVec =
        catalog::MEMORY_BYTE_SECONDS_TOTAL.register();
    static ref RESOURCE_USAGE_BATCH_UPDATE_FAILURE_TOTAL: Counter =
        catalog::RESOURCE_USAGE_BATCH_UPDATE_FAILURE_TOTAL.register();
}

pub fn record_fuel_borrow(amount: u64) {
    FUEL_BORROW_TOTAL.inc_by(amount as f64);
}

pub fn record_fuel_return(amount: u64) {
    FUEL_RETURN_TOTAL.inc_by(amount as f64);
}

pub fn record_ephemeral_overdraft_fuel(amount: u64) {
    EPHEMERAL_OVERDRAFT_FUEL_TOTAL.inc_by(amount as f64);
}

fn agent_mode_label(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Durable => "durable",
        AgentMode::Ephemeral => "ephemeral",
    }
}

pub fn record_storage_byte_seconds(mode: AgentMode, amount: i64) {
    // Lower-cased here rather than via `Display`, which renders for humans and is
    // free to change; label values are a query interface and must stay stable.
    STORAGE_BYTE_SECONDS_TOTAL
        .with_label_values(&[agent_mode_label(mode)])
        .inc_by(amount as f64);
}

const BYTES_PER_GB: f64 = (BYTE_NANOSECONDS_PER_GB_SECOND / 1_000_000_000) as f64;

pub fn record_memory_gb_seconds(mode: AgentMode, amount: i64) {
    MEMORY_BYTE_SECONDS_TOTAL
        .with_label_values(&[agent_mode_label(mode)])
        .inc_by(amount as f64 * BYTES_PER_GB);
}

#[cfg(test)]
pub(crate) fn memory_byte_seconds_total(mode: AgentMode) -> f64 {
    MEMORY_BYTE_SECONDS_TOTAL
        .with_label_values(&[agent_mode_label(mode)])
        .get()
}

pub fn record_resource_usage_batch_update_failure() {
    RESOURCE_USAGE_BATCH_UPDATE_FAILURE_TOTAL.inc();
}
