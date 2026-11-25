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

use std::time::Duration;

use golem_common::metrics::catalog;
use lazy_static::lazy_static;
use prometheus::*;
use tracing::debug;

use golem_common::metrics::api::ApiErrorDetails;

use golem_service_base::error::worker_executor::WorkerExecutorError;

lazy_static! {
    static ref CREATE_WORKER_SECONDS: Histogram = catalog::CREATE_WORKER_SECONDS.register();
    static ref CREATE_WORKER_FAILURE_TOTAL: CounterVec =
        catalog::CREATE_WORKER_FAILURE_TOTAL.register();
    static ref INVOCATION_TOTAL: CounterVec = catalog::INVOCATION_TOTAL.register();
    static ref INVOCATION_CONSUMPTION_TOTAL: Histogram =
        catalog::INVOCATION_FUEL_CONSUMPTION.register();
    static ref ALLOCATED_MEMORY_BYTES: Histogram = catalog::ALLOCATED_MEMORY_BYTES.register();
    static ref WORKER_ALLOCATED_LINEAR_MEMORY_BYTES: Histogram =
        catalog::WORKER_ALLOCATED_LINEAR_MEMORY_BYTES.register();
}

lazy_static! {
    static ref HOST_FUNCTION_CALL_TOTAL: CounterVec = catalog::HOST_FUNCTION_CALL_TOTAL.register();
    static ref RESUME_WORKER_SECONDS: Histogram = catalog::RESUME_WORKER_SECONDS.register();
    static ref REPLAYED_FUNCTIONS_COUNT: Histogram = catalog::REPLAYED_FUNCTIONS.register();
}

lazy_static! {
    static ref IN_FUNCTION_RETRY_TOTAL: Counter = catalog::IN_FUNCTION_RETRY_TOTAL.register();
    static ref CUSTOM_INVOCATION_SCOPE_OPEN_TOTAL: Counter =
        catalog::CUSTOM_INVOCATION_SCOPE_OPEN_TOTAL.register();
    static ref AGENT_PERMISSION_AUTHORIZATION_TOTAL: CounterVec =
        catalog::AGENT_PERMISSION_AUTHORIZATION_TOTAL.register();
    static ref AGENT_PERMISSION_AUTHORITY_SYNC_TOTAL: CounterVec =
        catalog::AGENT_PERMISSION_AUTHORITY_SYNC_TOTAL.register();
    static ref AGENT_PERMISSION_AUTHORITY_SYNC_SECONDS: Histogram =
        catalog::AGENT_PERMISSION_AUTHORITY_SYNC_SECONDS.register();
}

pub fn record_host_function_call(iface: &str, name: &str) {
    debug!("golem {iface}::{name} called");
    HOST_FUNCTION_CALL_TOTAL
        .with_label_values(&[iface, name])
        .inc();
}

pub fn record_in_function_retry() {
    IN_FUNCTION_RETRY_TOTAL.inc();
}

pub fn record_custom_invocation_scope_open() {
    CUSTOM_INVOCATION_SCOPE_OPEN_TOTAL.inc();
}

pub fn record_agent_permission_authorization(permission_class: &str, allowed: bool) {
    AGENT_PERMISSION_AUTHORIZATION_TOTAL
        .with_label_values(&[permission_class, if allowed { "allowed" } else { "denied" }])
        .inc();
}

pub fn record_agent_permission_authority_fast_path() {
    AGENT_PERMISSION_AUTHORITY_SYNC_TOTAL
        .with_label_values(&["fast"])
        .inc();
}

pub fn record_agent_permission_authority_slow_path(duration: Duration) {
    AGENT_PERMISSION_AUTHORITY_SYNC_TOTAL
        .with_label_values(&["slow"])
        .inc();
    AGENT_PERMISSION_AUTHORITY_SYNC_SECONDS.observe(duration.as_secs_f64());
}

pub fn record_resume_worker(duration: Duration) {
    RESUME_WORKER_SECONDS.observe(duration.as_secs_f64());
}

pub fn record_number_of_replayed_functions(count: usize) {
    REPLAYED_FUNCTIONS_COUNT.observe(count as f64);
}

pub fn record_create_worker(duration: Duration) {
    CREATE_WORKER_SECONDS.observe(duration.as_secs_f64());
}

pub fn record_create_worker_failure(error: &WorkerExecutorError) {
    CREATE_WORKER_FAILURE_TOTAL
        .with_label_values(&[error.trace_error_kind()])
        .inc();
}

pub fn record_invocation(is_live: bool, outcome: &'static str) {
    let mode: &'static str = if is_live { "live" } else { "replay" };
    INVOCATION_TOTAL.with_label_values(&[mode, outcome]).inc();
}

pub fn record_invocation_consumption(fuel: u64) {
    INVOCATION_CONSUMPTION_TOTAL.observe(fuel as f64);
}

pub fn record_allocated_memory(amount: usize) {
    ALLOCATED_MEMORY_BYTES.observe(amount as f64);
}

pub fn record_worker_allocated_linear_memory(bytes: u64) {
    WORKER_ALLOCATED_LINEAR_MEMORY_BYTES.observe(bytes as f64);
}
