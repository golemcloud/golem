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

use golem_common::model::agent::AgentMode;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref FUEL_BORROW_TOTAL: Counter =
        register_counter!("fuel_borrow_total", "Total amount of fuel borrowed").unwrap();
    static ref FUEL_RETURN_TOTAL: Counter =
        register_counter!("fuel_return_total", "Total amount of fuel returned").unwrap();
    static ref EPHEMERAL_OVERDRAFT_FUEL_TOTAL: Counter = register_counter!(
        "ephemeral_overdraft_fuel_total",
        "Total amount of ephemeral overdraft fuel consumed"
    )
    .unwrap();
    static ref STORAGE_BYTE_SECONDS_TOTAL: CounterVec = register_counter_vec!(
        "storage_byte_seconds_total",
        "Filesystem storage byte-seconds billed, by account and agent mode",
        &["account_id", "agent_mode"]
    )
    .unwrap();
    static ref MEMORY_GB_SECONDS_TOTAL: CounterVec = register_counter_vec!(
        "memory_gb_seconds_total",
        "Allocated linear-memory GB-seconds delivered to the registry, by account and agent mode",
        &["account_id", "agent_mode"]
    )
    .unwrap();
    static ref RESOURCE_USAGE_BATCH_UPDATE_FAILURE_TOTAL: Counter = register_counter!(
        "resource_usage_batch_update_failure_total",
        "Number of resource usage batches dropped after registry update failures"
    )
    .unwrap();
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

pub fn record_storage_byte_seconds(account_id: &str, mode: AgentMode, amount: i64) {
    // Lower-cased here rather than via `Display`, which renders for humans and is
    // free to change; label values are a query interface and must stay stable.
    STORAGE_BYTE_SECONDS_TOTAL
        .with_label_values(&[account_id, agent_mode_label(mode)])
        .inc_by(amount as f64);
}

pub fn record_memory_gb_seconds(account_id: &str, mode: AgentMode, amount: i64) {
    MEMORY_GB_SECONDS_TOTAL
        .with_label_values(&[account_id, agent_mode_label(mode)])
        .inc_by(amount as f64);
}

#[cfg(test)]
pub(crate) fn memory_gb_seconds_total(account_id: &str, mode: AgentMode) -> f64 {
    MEMORY_GB_SECONDS_TOTAL
        .with_label_values(&[account_id, agent_mode_label(mode)])
        .get()
}

pub fn record_resource_usage_batch_update_failure() {
    RESOURCE_USAGE_BATCH_UPDATE_FAILURE_TOTAL.inc();
}
