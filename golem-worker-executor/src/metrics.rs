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

// Module holding all the metrics used by the server.
// Collecting them in one place makes it easier to look them up and to share
// common metrics between different layers of the application.

use golem_common::golem_version;
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
        .with_label_values(&[golem_version(), wasmtime::VERSION])
        .inc();

    default_registry().clone()
}

const FUEL_BUCKETS: &[f64; 11] = &[
    1000.0, 10000.0, 25000.0, 50000.0, 100000.0, 250000.0, 500000.0, 1000000.0, 2500000.0,
    5000000.0, 10000000.0,
];

/// Byte-size buckets for scheduled-action payloads and promise completion data:
/// powers of 2 from 1 KB to 64 MB.
const BLOB_SIZE_BUCKETS: &[f64; 17] = &[
    1_024.0,
    2_048.0,
    4_096.0,
    8_192.0,
    16_384.0,
    32_768.0,
    65_536.0,
    131_072.0,
    262_144.0,
    524_288.0,
    1_048_576.0,
    2_097_152.0,
    4_194_304.0,
    8_388_608.0,
    16_777_216.0,
    33_554_432.0,
    67_108_864.0,
];

/// Lag buckets for the scheduler: sub-second to multi-minute range.
const SCHEDULER_LAG_BUCKETS: &[f64; 11] = &[
    0.001, 0.01, 0.1, 1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0,
];

/// Buckets for the size of a single `memory.grow` allocation. Deliberately
/// fine-grained in the 1-32 MiB band where typical guest grows cluster, so
/// that p90/p99 quantiles are not pinned to a coarse 4-16 MiB bucket edge.
const MEMORY_SIZE_BUCKETS: &[f64; 16] = &[
    65536.0,      // 64 KiB
    262144.0,     // 256 KiB
    1048576.0,    // 1 MiB
    2097152.0,    // 2 MiB
    4194304.0,    // 4 MiB
    6291456.0,    // 6 MiB
    8388608.0,    // 8 MiB
    12582912.0,   // 12 MiB
    16777216.0,   // 16 MiB
    25165824.0,   // 24 MiB
    33554432.0,   // 32 MiB
    67108864.0,   // 64 MiB
    134217728.0,  // 128 MiB
    268435456.0,  // 256 MiB
    536870912.0,  // 512 MiB
    1073741824.0, // 1 GiB
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

pub mod runtime {
    use std::sync::Arc;
    use std::time::Duration;

    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
    use tokio::runtime::Handle;
    use tokio::task::JoinSet;
    use tokio_metrics::RuntimeMetricsReporterBuilder;

    /// How often the recorder's upkeep runs to keep its internal storage
    /// bounded (e.g. pruning idle metrics once an idle timeout is configured).
    const UPKEEP_INTERVAL: Duration = Duration::from_secs(30);

    /// Installs a dedicated `metrics`-crate Prometheus recorder for tokio
    /// runtime metrics, spawns the tokio-metrics reporter on `join_set`, and
    /// returns a renderer that emits the collected metrics in Prometheus text
    /// format.
    ///
    /// `sampling_interval` controls how often metrics are sampled from the
    /// runtime into the recorder; Prometheus scrapes the rendered values
    /// independently.
    ///
    /// The returned closure is appended to the `prometheus`-crate scrape output
    /// on the shared `/metrics` endpoint, so all `tokio_*` series appear on the
    /// same endpoint as the rest of the executor's metrics, carrying the same
    /// `executor_id` label.
    ///
    /// Returns `None` if a global `metrics` recorder is already installed (which
    /// should not happen in the executor), in which case runtime metrics are
    /// simply not exported.
    pub fn install_runtime_metrics(
        runtime: Handle,
        sampling_interval: Duration,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Option<Arc<dyn Fn() -> String + Send + Sync>> {
        let executor_id = crate::identity::executor_id();

        let handle: PrometheusHandle = match PrometheusBuilder::new()
            .add_global_label("executor_id", executor_id)
            .install_recorder()
        {
            Ok(handle) => handle,
            Err(err) => {
                tracing::warn!(
                    "Failed to install tokio runtime metrics recorder, runtime metrics will not be exported: {err}"
                );
                return None;
            }
        };

        let reporter = RuntimeMetricsReporterBuilder::default().with_interval(sampling_interval);
        join_set.spawn_on(
            async move {
                reporter.describe_and_run().await;
                Ok(())
            },
            &runtime,
        );

        // Run periodic upkeep so the recorder's internal storage stays bounded.
        let upkeep_handle = handle.clone();
        join_set.spawn_on(
            async move {
                let mut interval = tokio::time::interval(UPKEEP_INTERVAL);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    upkeep_handle.run_upkeep();
                }
            },
            &runtime,
        );

        Some(Arc::new(move || handle.render()))
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
    use golem_common::model::AgentStatus;
    use lazy_static::lazy_static;
    use prometheus::core::Number;
    use prometheus::*;

    lazy_static! {
        static ref WORKER_EXECUTOR_CALL_TOTAL: CounterVec = register_counter_vec!(
            "worker_executor_call_total",
            "Number of calls to the worker layer",
            &["api"]
        )
        .unwrap();
        static ref WORKER_COUNT_BY_STATUS: GaugeVec = register_gauge_vec!(
            "worker_count_by_status",
            "Number of in-memory workers per status",
            &["status"]
        )
        .unwrap();
        static ref WORKER_EVICTION_TOTAL: CounterVec = register_counter_vec!(
            "worker_eviction_total",
            "Number of workers evicted from memory",
            &["class"]
        )
        .unwrap();
        static ref WORKER_FILESYSTEM_SEMAPHORE_AVAILABLE: Gauge = register_gauge!(
            "worker_filesystem_semaphore_available",
            "Available filesystem semaphore permits (bytes)"
        )
        .unwrap();
        pub static ref WORKER_MEMORY_RESIDENT_COUNT: GaugeVec = register_gauge_vec!(
            "worker_memory_resident_count",
            "Workers currently holding a memory permit and running an invocation loop on this executor",
            &["executor_id"]
        )
        .unwrap();
        pub static ref WORKER_WAITING_FOR_MEMORY_COUNT: GaugeVec = register_gauge_vec!(
            "worker_waiting_for_memory_count",
            "Workers blocked waiting to acquire a memory permit on this executor",
            &["executor_id"]
        )
        .unwrap();
        pub static ref WORKER_KV_CACHE_VALUE_SIZE_BYTES: HistogramVec = register_histogram_vec!(
            "worker_kv_cache_value_size_bytes",
            "Bytes of a value written to the Worker-namespace KV cache (worker status blob size)",
            &["executor_id"],
            crate::metrics::BLOB_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
        pub static ref WORKER_MEMORY_POOL_TOTAL_BYTES: GaugeVec = register_gauge_vec!(
            "golem_worker_memory_pool_total_bytes",
            "Usable memory ceiling (usable_ratio * measured limit) the admission gate admits against on this executor",
            &["executor_id"]
        )
        .unwrap();
        pub static ref WORKER_MEMORY_POOL_USED_BYTES: GaugeVec = register_gauge_vec!(
            "golem_worker_memory_pool_used_bytes",
            "Total linear memory granted to live workers and reserved by the admission gate on this executor",
            &["executor_id"]
        )
        .unwrap();
        pub static ref WORKER_ADMISSION_RSS_BYTES: GaugeVec = register_gauge_vec!(
            "golem_worker_admission_rss_bytes",
            "Measured resident memory (probe snapshot) the admission gate last read on this executor",
            &["executor_id"]
        )
        .unwrap();
        pub static ref WORKER_MEMORY_GROW_REJECTED_TOTAL: CounterVec = register_counter_vec!(
            "golem_worker_memory_grow_rejected_total",
            "Invocations interrupted because a worker's linear-memory grow could not be admitted by the gate (out-of-memory trap, retried via reacquire)",
            &["executor_id"]
        )
        .unwrap();
    }

    /// Counts one invocation interrupted because a linear-memory grow was
    /// refused by the admission gate (the worker traps out-of-memory and is
    /// restarted to reacquire memory).
    pub fn record_worker_memory_grow_rejected() {
        WORKER_MEMORY_GROW_REJECTED_TOTAL
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .inc();
    }

    /// Sets the gate's usable memory ceiling gauge.
    pub fn record_worker_memory_ceiling(bytes: u64) {
        WORKER_MEMORY_POOL_TOTAL_BYTES
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .set(bytes as f64);
    }

    /// Sets the gauge of total memory granted to live workers (the gate's
    /// reservation).
    pub fn record_worker_memory_granted(bytes: u64) {
        WORKER_MEMORY_POOL_USED_BYTES
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .set(bytes as f64);
    }

    /// Sets the gauge of measured resident memory last read by the gate.
    pub fn record_worker_admission_rss(bytes: u64) {
        WORKER_ADMISSION_RSS_BYTES
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .set(bytes as f64);
    }

    pub fn record_worker_call(api_name: &'static str) {
        WORKER_EXECUTOR_CALL_TOTAL
            .with_label_values(&[api_name])
            .inc();
    }

    pub fn set_worker_count_by_status(status: &'static str, count: f64) {
        WORKER_COUNT_BY_STATUS
            .with_label_values(&[status])
            .set(count);
    }

    pub fn initialize_worker_count_by_status() {
        for status in [
            AgentStatus::Running,
            AgentStatus::Idle,
            AgentStatus::Suspended,
            AgentStatus::Interrupted,
            AgentStatus::Retrying,
            AgentStatus::Failed,
            AgentStatus::Exited,
        ] {
            set_worker_count_by_status(worker_status_label(status), 0.0);
        }
    }

    /// Initialises all worker-related gauges to zero so every label combination
    /// appears in the first Prometheus scrape even before any workers are created.
    pub fn initialize_worker_metrics() {
        initialize_worker_count_by_status();
        let id = crate::metrics::storage::executor_id();
        WORKER_MEMORY_RESIDENT_COUNT
            .with_label_values(&[id])
            .set(0.0);
        WORKER_WAITING_FOR_MEMORY_COUNT
            .with_label_values(&[id])
            .set(0.0);
        WORKER_MEMORY_GROW_REJECTED_TOTAL
            .with_label_values(&[id])
            .inc_by(0.0);
    }

    pub fn inc_worker_memory_resident() {
        WORKER_MEMORY_RESIDENT_COUNT
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .inc();
    }

    pub fn dec_worker_memory_resident() {
        WORKER_MEMORY_RESIDENT_COUNT
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .dec();
    }

    pub fn inc_worker_waiting_for_memory() {
        WORKER_WAITING_FOR_MEMORY_COUNT
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .inc();
    }

    pub fn dec_worker_waiting_for_memory() {
        WORKER_WAITING_FOR_MEMORY_COUNT
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .dec();
    }

    pub fn inc_worker_count_by_status(status: AgentStatus) {
        WORKER_COUNT_BY_STATUS
            .with_label_values(&[worker_status_label(status)])
            .inc();
    }

    pub fn dec_worker_count_by_status(status: AgentStatus) {
        WORKER_COUNT_BY_STATUS
            .with_label_values(&[worker_status_label(status)])
            .dec();
    }

    pub fn record_worker_status_transition(old_status: AgentStatus, new_status: AgentStatus) {
        if old_status != new_status {
            dec_worker_count_by_status(old_status);
            inc_worker_count_by_status(new_status);
        }
    }

    fn worker_status_label(status: AgentStatus) -> &'static str {
        match status {
            AgentStatus::Running => "Running",
            AgentStatus::Idle => "Idle",
            AgentStatus::Suspended => "Suspended",
            AgentStatus::Interrupted => "Interrupted",
            AgentStatus::Retrying => "Retrying",
            AgentStatus::Failed => "Failed",
            AgentStatus::Exited => "Exited",
        }
    }

    pub fn record_worker_eviction(class: &'static str) {
        WORKER_EVICTION_TOTAL.with_label_values(&[class]).inc();
    }

    pub fn set_filesystem_semaphore_available(permits: u64) {
        WORKER_FILESYSTEM_SEMAPHORE_AVAILABLE.set(permits.into_f64());
    }

    pub fn dec_filesystem_semaphore_available(permits: u64) {
        WORKER_FILESYSTEM_SEMAPHORE_AVAILABLE.sub(permits.into_f64());
    }

    pub fn inc_filesystem_semaphore_available(permits: u64) {
        WORKER_FILESYSTEM_SEMAPHORE_AVAILABLE.add(permits.into_f64());
    }

    pub fn record_worker_kv_cache_value_size(bytes: usize) {
        WORKER_KV_CACHE_VALUE_SIZE_BYTES
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .observe(bytes as f64);
    }
}

pub mod promises {
    use lazy_static::lazy_static;
    use prometheus::*;
    use std::time::Duration;

    lazy_static! {
        static ref PROMISES_COUNT_TOTAL: Counter =
            register_counter!("promises_count_total", "Number of promises created").unwrap();
        static ref PROMISES_SCHEDULED_COMPLETE_TOTAL: Counter = register_counter!(
            "promises_scheduled_complete_total",
            "Number of scheduled promise completions"
        )
        .unwrap();
        pub static ref PROMISE_COMPLETION_SECONDS: HistogramVec = register_histogram_vec!(
            "promise_completion_seconds",
            "Wall time of complete_promise from call entry to return, labelled by outcome",
            &["executor_id", "outcome"],
            golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        pub static ref PROMISE_PENDING_COUNT: GaugeVec = register_gauge_vec!(
            "promise_pending_count",
            "Number of distinct PromiseIds in Pending state in this executor's PromiseRegistry",
            &["executor_id"]
        )
        .unwrap();
        pub static ref PROMISE_DATA_SIZE_BYTES: HistogramVec = register_histogram_vec!(
            "promise_data_size_bytes",
            "Bytes of the data payload submitted to complete_promise at call time",
            &["executor_id"],
            crate::metrics::BLOB_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
    }

    pub fn record_promise_created() {
        PROMISES_COUNT_TOTAL.inc();
    }

    pub fn record_scheduled_promise_completed() {
        PROMISES_SCHEDULED_COMPLETE_TOTAL.inc();
    }

    pub fn record_promise_completion(duration: Duration, outcome: &'static str) {
        PROMISE_COMPLETION_SECONDS
            .with_label_values(&[crate::metrics::storage::executor_id(), outcome])
            .observe(duration.as_secs_f64());
    }

    pub fn inc_promise_pending_count() {
        PROMISE_PENDING_COUNT
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .inc();
    }

    pub fn dec_promise_pending_count() {
        PROMISE_PENDING_COUNT
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .dec();
    }

    pub fn record_promise_data_size(bytes: usize) {
        PROMISE_DATA_SIZE_BYTES
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .observe(bytes as f64);
    }
}

pub mod scheduler {
    use lazy_static::lazy_static;
    use prometheus::*;
    use std::time::Duration;

    lazy_static! {
        pub static ref SCHEDULED_ACTION_LAG_SECONDS: HistogramVec = register_histogram_vec!(
            "scheduled_action_lag_seconds",
            "Wall-clock delay in seconds between scheduled_at and the time the action fires",
            &["executor_id"],
            crate::metrics::SCHEDULER_LAG_BUCKETS.to_vec()
        )
        .unwrap();
        pub static ref SCHEDULER_QUEUE_DEPTH: GaugeVec = register_gauge_vec!(
            "scheduler_queue_depth",
            "Count of matching actions to process at the start of each scheduler process() iteration",
            &["executor_id"]
        )
        .unwrap();
        pub static ref SCHEDULER_TICK_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
            "scheduler_tick_duration_seconds",
            "Wall time of a single scheduler process() iteration",
            &["executor_id"],
            golem_common::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        pub static ref SCHEDULED_ACTION_SIZE_BYTES: HistogramVec = register_histogram_vec!(
            "scheduled_action_size_bytes",
            "Serialized blob size in bytes of a ScheduledAction at insert time",
            &["executor_id", "action_kind"],
            crate::metrics::BLOB_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
    }

    pub fn record_scheduled_action_lag(lag: Duration) {
        SCHEDULED_ACTION_LAG_SECONDS
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .observe(lag.as_secs_f64());
    }

    pub fn set_scheduler_queue_depth(depth: usize) {
        SCHEDULER_QUEUE_DEPTH
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .set(depth as f64);
    }

    pub fn record_scheduler_tick_duration(duration: Duration) {
        SCHEDULER_TICK_DURATION_SECONDS
            .with_label_values(&[crate::metrics::storage::executor_id()])
            .observe(duration.as_secs_f64());
    }

    pub fn record_scheduled_action_size(action_kind: &'static str, bytes: usize) {
        SCHEDULED_ACTION_SIZE_BYTES
            .with_label_values(&[crate::metrics::storage::executor_id(), action_kind])
            .observe(bytes as f64);
    }

    /// Maps a `ScheduledAction` to its metric `action_kind` label value.
    pub fn action_kind_label(action: &golem_common::model::ScheduledAction) -> &'static str {
        use golem_common::model::ScheduledAction;
        match action {
            ScheduledAction::CompletePromise { .. } => "complete_promise",
            ScheduledAction::ArchiveOplog { .. } => "archive_oplog",
            ScheduledAction::Invoke { .. } => "invoke",
            ScheduledAction::Resume { .. } => "resume",
        }
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
        static ref WORKER_RESIDENT_LINEAR_MEMORY_BYTES: Histogram = register_histogram!(
            "worker_resident_linear_memory_bytes",
            "Per-worker cumulative linear-memory grant (total_linear_memory_size = sum of memory.grow deltas) sampled when the worker is admitted. This is the linear memory the admission gate reserves for the worker; it is an upper bound on resident RSS, not measured resident memory, since grown pages are largely demand-paged. Compare to container_memory_working_set_bytes for the gap.",
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

    pub fn record_worker_resident_linear_memory(bytes: u64) {
        WORKER_RESIDENT_LINEAR_MEMORY_BYTES.observe(bytes as f64);
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
        static ref OPLOG_RATE_LIMITED_TOTAL: CounterVec = register_counter_vec!(
            "oplog_rate_limited_total",
            "Number of oplog add calls that were delayed by the rate limiter",
            &["account_id", "environment_id"]
        )
        .unwrap();
        static ref OPLOG_STORAGE_RETRY_TOTAL: CounterVec = register_counter_vec!(
            "oplog_storage_retry_total",
            "Number of oplog storage operation retries due to transient errors",
            &["op"]
        )
        .unwrap();
    }

    pub fn record_oplog_call(api_name: &'static str) {
        OPLOG_SVC_CALL_TOTAL.with_label_values(&[api_name]).inc();
    }

    pub fn record_oplog_rate_limited(account_id: &str, environment_id: &str) {
        OPLOG_RATE_LIMITED_TOTAL
            .with_label_values(&[account_id, environment_id])
            .inc();
    }

    pub fn record_oplog_storage_retry(op_name: &str) {
        OPLOG_STORAGE_RETRY_TOTAL
            .with_label_values(&[op_name])
            .inc();
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

pub mod resources {
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
}

pub mod ephemeral {
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref EPHEMERAL_PROMISE_WAITING: Gauge = register_gauge!(
            "ephemeral_promise_waiting",
            "Number of ephemeral agents currently waiting on promises"
        )
        .unwrap();
        static ref EPHEMERAL_NON_SUSPENDING_FAILURE_TOTAL: CounterVec = register_counter_vec!(
            "ephemeral_non_suspending_failure_total",
            "Number of ephemeral failures that replace suspension",
            &["reason"]
        )
        .unwrap();
    }

    pub fn inc_promise_waiting() {
        EPHEMERAL_PROMISE_WAITING.inc();
    }

    pub fn dec_promise_waiting() {
        EPHEMERAL_PROMISE_WAITING.dec();
    }

    pub fn record_non_suspending_failure(reason: &'static str) {
        EPHEMERAL_NON_SUSPENDING_FAILURE_TOTAL
            .with_label_values(&[reason])
            .inc();
    }
}

pub mod storage {
    // Re-export shared storage metrics from golem-service-base so all services
    // can use the same metric definitions (same Prometheus global registry).
    pub use golem_service_base::metrics::storage::*;

    pub const STORAGE_TYPE_BLOB_STORE: &str = "blob_store";
    pub const STORAGE_TYPE_KV: &str = "kv";
    pub const STORAGE_TYPE_OPLOG: &str = "oplog";
    pub const STORAGE_TYPE_OPLOG_ARCHIVE: &str = "oplog_archive";
    pub const STORAGE_TYPE_FILESYSTEM: &str = "filesystem";

    use lazy_static::lazy_static;
    use prometheus::*;

    /// Re-exported from [`crate::identity`], which owns the process identity.
    /// Kept here so existing metric-recording call sites can keep using
    /// `crate::metrics::storage::executor_id()`.
    pub use crate::identity::executor_id;

    lazy_static! {
        pub static ref STORAGE_FILESYSTEM_POOL_TOTAL_BYTES: GaugeVec = register_gauge_vec!(
            "golem_storage_filesystem_pool_total_bytes",
            "Total filesystem storage pool capacity for this executor",
            &["executor_id"]
        )
        .unwrap();
        pub static ref STORAGE_FILESYSTEM_POOL_USED_BYTES: GaugeVec = register_gauge_vec!(
            "golem_storage_filesystem_pool_used_bytes",
            "Currently acquired filesystem storage bytes across all workers on this executor",
            &["executor_id"]
        )
        .unwrap();
    }

    pub fn record_filesystem_pool_total(bytes: u64) {
        STORAGE_FILESYSTEM_POOL_TOTAL_BYTES
            .with_label_values(&[executor_id()])
            .set(bytes as f64);
    }

    pub fn record_filesystem_pool_acquired(bytes: u64) {
        STORAGE_FILESYSTEM_POOL_USED_BYTES
            .with_label_values(&[executor_id()])
            .add(bytes as f64);
    }

    pub fn record_filesystem_pool_released(bytes: u64) {
        STORAGE_FILESYSTEM_POOL_USED_BYTES
            .with_label_values(&[executor_id()])
            .sub(bytes as f64);
    }
}
