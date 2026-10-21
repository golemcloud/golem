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

use golem_common::model::AgentStatus;
use lazy_static::lazy_static;
use prometheus::*;
use std::time::Duration;

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
    pub static ref WORKER_ADMISSION_WAIT_SECONDS: HistogramVec = register_histogram_vec!(
        "worker_admission_wait_seconds",
        "Time a starting worker spent blocked in one phase of admission before it could become resident, labelled by phase (resolve_component_charge, concurrency_slot, memory). Observed once per phase per worker start. Sum across phases for the total wait; read per phase to see which resource is the constraint. worker_waiting_for_memory_count says how many workers are waiting, this says for how long",
        &["executor_id", "phase"],
        crate::metrics::ADMISSION_WAIT_BUCKETS.to_vec()
    )
    .unwrap();
    pub static ref WORKER_STORE_ALIVE_COUNT: GaugeVec = register_gauge_vec!(
        "golem_worker_store_alive_count",
        "Live wasmtime Store contexts on this executor, counted by the Store's own lifetime: incremented when a worker's Store is constructed and decremented when it is dropped. Diverging above the resident-worker count means Stores are retained after the owning worker was deleted",
        &["executor_id"]
    )
    .unwrap();
    static ref OWNER_GROUP_ALIVE_COUNT: GaugeVec = register_gauge_vec!(
        "golem_owner_group_alive_count",
        "Owner-routed active agent groups on this executor",
        &["executor_id"]
    )
    .unwrap();
    static ref PRIMARY_STORE_ALIVE_COUNT: GaugeVec = register_gauge_vec!(
        "golem_primary_store_alive_count",
        "Live primary agent Stores on this executor",
        &["executor_id"]
    )
    .unwrap();
    static ref ENTITY_STORE_ALIVE_COUNT: GaugeVec = register_gauge_vec!(
        "golem_entity_store_alive_count",
        "Live transient entity Stores on this executor",
        &["executor_id", "entity_kind"]
    )
    .unwrap();
    static ref ENTITY_INVOCATION_ACTIVE_COUNT: GaugeVec = register_gauge_vec!(
        "golem_entity_invocation_active_count",
        "Entity invocation bodies currently active on this executor",
        &["entity_kind", "execution_mode"]
    )
    .unwrap();
    static ref ENTITY_INVOCATION_TOTAL: CounterVec = register_counter_vec!(
        "golem_entity_invocation_total",
        "Entity invocation bodies by terminal outcome",
        &["entity_kind", "execution_mode", "outcome"]
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
    static ref WORKER_MEMORY_POOL_TOTAL_BYTES_GAUGE: Gauge = WORKER_MEMORY_POOL_TOTAL_BYTES
        .with_label_values(&[crate::metrics::storage::executor_id()]);
    static ref WORKER_MEMORY_POOL_USED_BYTES_GAUGE: Gauge = WORKER_MEMORY_POOL_USED_BYTES
        .with_label_values(&[crate::metrics::storage::executor_id()]);
    static ref WORKER_ADMISSION_RSS_BYTES_GAUGE: Gauge = WORKER_ADMISSION_RSS_BYTES
        .with_label_values(&[crate::metrics::storage::executor_id()]);
    pub static ref WORKER_MEMORY_GROW_REJECTED_TOTAL: CounterVec = register_counter_vec!(
        "golem_worker_memory_grow_rejected_total",
        "Invocations interrupted because a worker's linear-memory grow could not be admitted by the gate (out-of-memory trap, retried via reacquire)",
        &["executor_id"]
    )
    .unwrap();
    static ref AGENT_STATUS_FLUSH_TOTAL: CounterVec = register_counter_vec!(
        "agent_status_flush_total",
        "Number of cached agent status blob flushes by reason (background/forced)",
        &["reason"]
    )
    .unwrap();
    static ref AGENT_STATUS_FLUSH_FAILED_TOTAL: CounterVec = register_counter_vec!(
        "agent_status_flush_failed_total",
        "Number of cached agent status blob flushes that failed, by reason (background/forced)",
        &["reason"]
    )
    .unwrap();
    static ref REPLAY_METADATA_SCAN_ENTRIES_TOTAL: CounterVec = register_counter_vec!(
        "replay_metadata_scan_entries_total",
        "Number of oplog entries scanned for recovery metadata when a replay generation starts, by the start of the replayed history (snapshot/initial)",
        &["start"]
    )
    .unwrap();
    static ref AGENT_STATUS_RECOMPUTE_TOTAL: CounterVec = register_counter_vec!(
        "agent_status_recompute_total",
        "Number of agent status recomputes by the baseline used (cache/checkpoint/full)",
        &["source"]
    )
    .unwrap();
    static ref AGENT_STATUS_CHECKPOINT_WRITE_TOTAL: CounterVec = register_counter_vec!(
        "agent_status_checkpoint_write_total",
        "Number of clean agent status checkpoint writes by reason (snapshot/idle)",
        &["reason"]
    )
    .unwrap();
    static ref AGENT_STATUS_CHECKPOINT_WRITE_FAILED_TOTAL: CounterVec = register_counter_vec!(
        "agent_status_checkpoint_write_failed_total",
        "Number of clean agent status checkpoint writes that failed, by reason (snapshot/idle)",
        &["reason"]
    )
    .unwrap();
    static ref INVOCATION_RESULT_RESOLUTION_TOTAL: CounterVec = register_counter_vec!(
        "invocation_result_resolution_total",
        "Invocation-result resolutions by the lookup path and outcome",
        &["outcome"]
    )
    .unwrap();
    static ref INVOCATION_RESULT_INDEX_CATCH_UP_CHUNKS_TOTAL: Counter = register_counter!(
        "invocation_result_index_catch_up_chunks_total",
        "Physical invocation-result index catch-up chunks completed"
    )
    .unwrap();
    static ref INVOCATION_RESULT_INDEX_CATCH_UP_ENTRIES_TOTAL: Counter = register_counter!(
        "invocation_result_index_catch_up_entries_total",
        "Oplog entries processed while catching up the physical invocation-result index"
    )
    .unwrap();
    static ref AGENT_IDENTITY_RESOLUTION_TOTAL: CounterVec = register_counter_vec!(
        "agent_identity_resolution_total",
        "Mode-hint events observed during authoritative agent identity resolution; multiple events may occur per resolution",
        &["event"]
    )
    .unwrap();
    static ref STALE_RUNNING_WORKER_TOTAL: CounterVec = register_counter_vec!(
        "stale_running_worker_total",
        "Stale recovery-index members removed by reason",
        &["reason"]
    )
    .unwrap();
    static ref STATUS_CACHE_PUBLICATION_TOTAL: CounterVec = register_counter_vec!(
        "status_cache_publication_total",
        "Status cache publication events by cache and reconciliation path; fallback and eventual reconciliation are separate events",
        &["cache", "event"]
    )
    .unwrap();
    static ref DERIVED_CACHE_PUBLICATION_FAILED_TOTAL: CounterVec = register_counter_vec!(
        "derived_cache_publication_failed_total",
        "Derived-cache publications that failed while atomically applying expiry",
        &["cache"]
    )
    .unwrap();
    static ref FOREIGN_STREAM_FINGERPRINT_MISMATCH_TOTAL: Counter = register_counter!(
        "foreign_stream_fingerprint_mismatch_total",
        "Foreign durable-stream accesses rejected because the current fingerprint differs"
    )
    .unwrap();
}

pub fn record_worker_call(api_name: &'static str) {
    WORKER_EXECUTOR_CALL_TOTAL
        .with_label_values(&[api_name])
        .inc();
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
    WORKER_MEMORY_POOL_TOTAL_BYTES_GAUGE.set(bytes as f64);
}

pub fn increase_worker_memory_granted(bytes: u64) {
    WORKER_MEMORY_POOL_USED_BYTES_GAUGE.add(bytes as f64);
}

pub fn decrease_worker_memory_granted(bytes: u64) {
    WORKER_MEMORY_POOL_USED_BYTES_GAUGE.sub(bytes as f64);
}

/// Sets the gauge of measured resident memory last read by the gate.
pub fn record_worker_admission_rss(bytes: u64) {
    WORKER_ADMISSION_RSS_BYTES_GAUGE.set(bytes as f64);
}

pub fn record_agent_status_flush(reason: &'static str) {
    AGENT_STATUS_FLUSH_TOTAL.with_label_values(&[reason]).inc();
}

pub fn record_agent_status_flush_failed(reason: &'static str) {
    AGENT_STATUS_FLUSH_FAILED_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub fn record_replay_metadata_scan(start: &'static str, entries: u64) {
    REPLAY_METADATA_SCAN_ENTRIES_TOTAL
        .with_label_values(&[start])
        .inc_by(entries as f64);
}

pub fn record_agent_status_recompute(source: &'static str) {
    AGENT_STATUS_RECOMPUTE_TOTAL
        .with_label_values(&[source])
        .inc();
}

pub fn record_agent_status_checkpoint_write(reason: &'static str) {
    AGENT_STATUS_CHECKPOINT_WRITE_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub fn record_agent_status_checkpoint_write_failed(reason: &'static str) {
    AGENT_STATUS_CHECKPOINT_WRITE_FAILED_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub fn record_invocation_result_resolution(outcome: &'static str) {
    INVOCATION_RESULT_RESOLUTION_TOTAL
        .with_label_values(&[outcome])
        .inc();
}

pub fn record_invocation_result_index_catch_up(entries: usize) {
    INVOCATION_RESULT_INDEX_CATCH_UP_CHUNKS_TOTAL.inc();
    INVOCATION_RESULT_INDEX_CATCH_UP_ENTRIES_TOTAL.inc_by(entries as f64);
}

pub fn record_agent_identity_resolution(outcome: &'static str) {
    AGENT_IDENTITY_RESOLUTION_TOTAL
        .with_label_values(&[outcome])
        .inc();
}

pub fn record_stale_running_worker(reason: &'static str) {
    STALE_RUNNING_WORKER_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub fn record_status_cache_publication(cache: &'static str, outcome: &'static str) {
    STATUS_CACHE_PUBLICATION_TOTAL
        .with_label_values(&[cache, outcome])
        .inc();
}

pub fn record_derived_cache_publication_failed(cache: &'static str) {
    DERIVED_CACHE_PUBLICATION_FAILED_TOTAL
        .with_label_values(&[cache])
        .inc();
}

pub fn record_foreign_stream_fingerprint_mismatch() {
    FOREIGN_STREAM_FINGERPRINT_MISMATCH_TOTAL.inc();
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
    WORKER_STORE_ALIVE_COUNT.with_label_values(&[id]).set(0.0);
    OWNER_GROUP_ALIVE_COUNT.with_label_values(&[id]).set(0.0);
    PRIMARY_STORE_ALIVE_COUNT.with_label_values(&[id]).set(0.0);
    for entity_kind in ["tool", "tool_middleware"] {
        ENTITY_STORE_ALIVE_COUNT
            .with_label_values(&[id, entity_kind])
            .set(0.0);
        for execution_mode in ["live", "replaying_completed", "replaying_incomplete"] {
            ENTITY_INVOCATION_ACTIVE_COUNT
                .with_label_values(&[entity_kind, execution_mode])
                .set(0.0);
            for outcome in ["succeeded", "failed", "cancelled"] {
                ENTITY_INVOCATION_TOTAL
                    .with_label_values(&[entity_kind, execution_mode, outcome])
                    .inc_by(0.0);
            }
        }
    }
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

/// Incremented when a worker's wasmtime `Store` context is constructed.
/// Paired with [`dec_worker_store_alive`] from a guard dropped with the
/// `Store` itself, so the gauge tracks the `Store`'s true lifetime rather
/// than the owning worker's accounting.
pub fn inc_worker_store_alive() {
    WORKER_STORE_ALIVE_COUNT
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .inc();
}

/// Decremented when a worker's wasmtime `Store` context is dropped.
pub fn dec_worker_store_alive() {
    WORKER_STORE_ALIVE_COUNT
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .dec();
}

pub fn inc_owner_group_alive() {
    OWNER_GROUP_ALIVE_COUNT
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .inc();
}

pub fn dec_owner_group_alive() {
    OWNER_GROUP_ALIVE_COUNT
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .dec();
}

pub fn inc_primary_store_alive() {
    PRIMARY_STORE_ALIVE_COUNT
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .inc();
}

pub fn dec_primary_store_alive() {
    PRIMARY_STORE_ALIVE_COUNT
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .dec();
}

pub fn inc_entity_store_alive(entity_kind: &'static str) {
    ENTITY_STORE_ALIVE_COUNT
        .with_label_values(&[crate::metrics::storage::executor_id(), entity_kind])
        .inc();
}

pub fn dec_entity_store_alive(entity_kind: &'static str) {
    ENTITY_STORE_ALIVE_COUNT
        .with_label_values(&[crate::metrics::storage::executor_id(), entity_kind])
        .dec();
}

pub fn inc_entity_invocation_active(entity_kind: &'static str, execution_mode: &'static str) {
    ENTITY_INVOCATION_ACTIVE_COUNT
        .with_label_values(&[entity_kind, execution_mode])
        .inc();
}

pub fn dec_entity_invocation_active(entity_kind: &'static str, execution_mode: &'static str) {
    ENTITY_INVOCATION_ACTIVE_COUNT
        .with_label_values(&[entity_kind, execution_mode])
        .dec();
}

pub fn record_entity_invocation(
    entity_kind: &'static str,
    execution_mode: &'static str,
    outcome: &'static str,
) {
    ENTITY_INVOCATION_TOTAL
        .with_label_values(&[entity_kind, execution_mode, outcome])
        .inc();
}

/// Phases a starting worker waits through before it can become resident. Each is
/// a separate blocking acquisition, so recording them apart shows which resource
/// a stalled start is actually waiting on.
#[derive(Debug, Clone, Copy)]
pub enum AdmissionPhase {
    ResolveComponentCharge,
    ConcurrencySlot,
    Memory,
}

impl AdmissionPhase {
    fn as_str(self) -> &'static str {
        match self {
            AdmissionPhase::ResolveComponentCharge => "resolve_component_charge",
            AdmissionPhase::ConcurrencySlot => "concurrency_slot",
            AdmissionPhase::Memory => "memory",
        }
    }
}

pub fn record_worker_admission_wait(phase: AdmissionPhase, elapsed: Duration) {
    WORKER_ADMISSION_WAIT_SECONDS
        .with_label_values(&[crate::metrics::storage::executor_id(), phase.as_str()])
        .observe(elapsed.as_secs_f64());
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

pub fn record_worker_kv_cache_value_size(bytes: usize) {
    WORKER_KV_CACHE_VALUE_SIZE_BYTES
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .observe(bytes as f64);
}
