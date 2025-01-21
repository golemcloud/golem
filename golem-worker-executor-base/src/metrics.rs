// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Module holding all the metrics used by the server.
// Collecting them in one place makes it easier to look them up and to share
// common metrics between different layers of the application.

use crate::VERSION;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref VERSION_INFO: IntCounterVec = register_int_counter_vec!(
        "executor_version_info",
        "Version info of the server",
        &["version", "wasmtime"]
    )
    .unwrap();
}

pub fn register_all() -> Registry {
    VERSION_INFO
        .with_label_values(&[VERSION, wasmtime::VERSION])
        .inc();

    default_registry().clone()
}

const FUEL_BUCKETS: &[f64; 11] = &[
    1000.0, 10000.0, 25000.0, 50000.0, 100000.0, 250000.0, 500000.0, 1000000.0, 2500000.0,
    5000000.0, 10000000.0,
];

const MEMORY_SIZE_BUCKETS: &[f64; 11] = &[
    1024.0,
    4096.0,
    16384.0,
    65536.0,
    262144.0,
    1048576.0,
    4194304.0,
    16777216.0,
    67108864.0,
    268435456.0,
    1073741824.0,
];

pub mod component {
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;

    use golem_common::metrics::DEFAULT_TIME_BUCKETS;

    lazy_static! {
        pub static ref COMPILATION_TIME_SECONDS: Histogram = register_histogram!(
            "compilation_time_seconds",
            "Time to compile a WASM component to native code",
            DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
    }

    pub fn record_compilation_time(duration: Duration) {
        COMPILATION_TIME_SECONDS.observe(duration.as_secs_f64());
    }
}

pub mod events {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref EVENT_TOTAL: CounterVec = register_counter_vec!(
            "event_total",
            "Number of events produced by the server",
            &["event"]
        )
        .unwrap();
        static ref EVENT_BROADCAST_TOTAL: CounterVec = register_counter_vec!(
            "event_broadcast_total",
            "Number of events broadcast by the server",
            &["event"]
        )
        .unwrap();
    }

    pub fn record_event(event: &'static str) {
        EVENT_TOTAL.with_label_values(&[event]).inc();
    }

    pub fn record_broadcast_event(event: &'static str) {
        EVENT_BROADCAST_TOTAL.with_label_values(&[event]).inc();
    }
}

pub mod workers {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref WORKER_EXECUTOR_CALL_TOTAL: CounterVec = register_counter_vec!(
            "worker_executor_call_total",
            "Number of calls to the worker layer",
            &["api"]
        )
        .unwrap();
    }

    pub fn record_worker_call(api_name: &'static str) {
        WORKER_EXECUTOR_CALL_TOTAL
            .with_label_values(&[api_name])
            .inc();
    }
}

pub mod promises {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref PROMISES_COUNT_TOTAL: Counter =
            register_counter!("promises_count_total", "Number of promises created").unwrap();
        static ref PROMISES_SCHEDULED_COMPLETE_TOTAL: Counter = register_counter!(
            "promises_scheduled_complete_total",
            "Number of scheduled promise completions"
        )
        .unwrap();
    }

    pub fn record_promise_created() {
        PROMISES_COUNT_TOTAL.inc();
    }

    pub fn record_scheduled_promise_completed() {
        PROMISES_SCHEDULED_COMPLETE_TOTAL.inc();
    }
}

pub mod sharding {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref ASSIGNED_SHARD_COUNT: Gauge =
            register_gauge!("assigned_shard_count", "Current number of assigned shards").unwrap();
    }

    pub fn record_assigned_shard_count(size: usize) {
        ASSIGNED_SHARD_COUNT.set(size as f64);
    }
}

pub mod wasm {
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;
    use tracing::debug;

    use golem_common::metrics::api::TraceErrorKind;

    use crate::error::GolemError;

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
            "Amount of memory allocated by a single memory.grow instruction",
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

    pub fn record_host_function_call(iface: &str, name: &str) {
        debug!("golem {iface}::{name} called");
        HOST_FUNCTION_CALL_TOTAL
            .with_label_values(&[iface, name])
            .inc();
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

    pub fn record_create_worker_failure(error: &GolemError) {
        CREATE_WORKER_FAILURE_TOTAL
            .with_label_values(&[error.trace_error_kind()])
            .inc();
    }

    pub fn record_invocation(is_live: bool, outcome: &'static str) {
        let mode: &'static str = if is_live { "live" } else { "replay" };
        INVOCATION_TOTAL.with_label_values(&[mode, outcome]).inc();
    }

    pub fn record_invocation_consumption(fuel: i64) {
        INVOCATION_CONSUMPTION_TOTAL.observe(fuel as f64);
    }

    pub fn record_allocated_memory(amount: usize) {
        ALLOCATED_MEMORY_BYTES.observe(amount as f64);
    }
}

pub mod oplog {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref OPLOG_SVC_CALL_TOTAL: CounterVec = register_counter_vec!(
            "oplog_svc_call_total",
            "Number of calls to the oplog service",
            &["api"]
        )
        .unwrap();
        static ref SCHEDULED_ARCHIVE_TIME: HistogramVec = register_histogram_vec!(
            "oplog_scheduled_archive",
            "Time taken to archive the oplog of a worker",
            &["type"],
            golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
    }

    pub fn record_oplog_call(api_name: &'static str) {
        OPLOG_SVC_CALL_TOTAL.with_label_values(&[api_name]).inc();
    }

    pub fn record_scheduled_archive(duration: std::time::Duration, has_more: bool) {
        SCHEDULED_ARCHIVE_TIME
            .with_label_values(if has_more {
                &["intermediate"]
            } else {
                &["final"]
            })
            .observe(duration.as_secs_f64());
    }
}
