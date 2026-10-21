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

use lazy_static::lazy_static;
use prometheus::*;
use tracing::debug;

use golem_common::metrics::api::ApiErrorDetails;

use golem_service_base::error::worker_executor::WorkerExecutorError;

lazy_static! {
    static ref CREATE_WORKER_SECONDS: Histogram = register_histogram!(
        "create_worker_seconds",
        "Time taken to create a worker",
        golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
    static ref CREATE_WORKER_FAILURE_TOTAL: CounterVec = register_counter_vec!(
        "create_worker_failure_total",
        "Number of failed worker creations",
        &["error"]
    )
    .unwrap();
    static ref INVOCATION_TOTAL: CounterVec = register_counter_vec!(
        "invocation_total",
        "Number of invocations",
        &["mode", "outcome"]
    )
    .unwrap();
    static ref INVOCATION_CONSUMPTION_TOTAL: Histogram = register_histogram!(
        "invocation_consumption_total",
        "Amount of fuel consumed by an invocation",
        crate::metrics::FUEL_BUCKETS.to_vec()
    )
    .unwrap();
    static ref ALLOCATED_MEMORY_BYTES: Histogram = register_histogram!(
        "allocated_memory_bytes",
        "Worker's total linear memory size after a memory.grow, sampled at each grow",
        crate::metrics::MEMORY_SIZE_BUCKETS.to_vec()
    )
    .unwrap();
    static ref WORKER_ALLOCATED_LINEAR_MEMORY_BYTES: Histogram = register_histogram!(
        "worker_allocated_linear_memory_bytes",
        "Per-worker allocated linear-memory total sampled after instance reconciliation. This is the sum of current data_size values for unique linear-memory backings, not measured resident memory.",
        crate::metrics::MEMORY_SIZE_BUCKETS.to_vec()
    )
    .unwrap();
}

lazy_static! {
    static ref HOST_FUNCTION_CALL_TOTAL: CounterVec = register_counter_vec!(
        "host_function_call_total",
        "Number of calls to specific host functions",
        &["interface", "name"]
    )
    .unwrap();
    static ref RESUME_WORKER_SECONDS: Histogram = register_histogram!(
        "resume_worker_seconds",
        "Time taken to resume a worker",
        golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
    static ref REPLAYED_FUNCTIONS_COUNT: Histogram = register_histogram!(
        "replayed_functions_count",
        "Number of functions replayed per worker resumption",
        golem_common::metrics::DEFAULT_COUNT_BUCKETS.to_vec()
    )
    .unwrap();
}

lazy_static! {
    static ref IN_FUNCTION_RETRY_TOTAL: Counter = register_counter!(
        "in_function_retry_total",
        "Number of in-function retries (retries inside host function without oplog replay)"
    )
    .unwrap();
    static ref CUSTOM_INVOCATION_SCOPE_OPEN_TOTAL: Counter = register_counter!(
        "custom_invocation_scope_open_total",
        "Number of live custom-durability ownership scopes opened"
    )
    .unwrap();
    static ref AGENT_PERMISSION_AUTHORIZATION_TOTAL: CounterVec = register_counter_vec!(
        "agent_permission_authorization_total",
        "Number of live host-call permission authorization decisions",
        &["permission_class", "outcome"]
    )
    .unwrap();
    static ref AGENT_PERMISSION_AUTHORITY_SYNC_TOTAL: CounterVec = register_counter_vec!(
        "agent_permission_authority_sync_total",
        "Number of host-call authority boundary checks by path",
        &["path"]
    )
    .unwrap();
    static ref AGENT_PERMISSION_AUTHORITY_SYNC_SECONDS: Histogram = register_histogram!(
        "agent_permission_authority_sync_seconds",
        "Time spent refreshing host-call permission authority on the slow path",
        golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
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
