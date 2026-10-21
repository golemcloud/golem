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
    static ref OPLOG_SWEEP_OUTCOME_TOTAL: CounterVec = register_counter_vec!(
        "oplog_sweep_outcome_total",
        "Keys the oplog sweep examined, by what it decided about each. One outcome per key, \
         so the outcomes sum to what a tick decided",
        &["route", "outcome"]
    )
    .unwrap();
    static ref OPLOG_SWEEP_TICK_TIME: HistogramVec = register_histogram_vec!(
        "oplog_sweep_tick_seconds",
        "Time taken by one oplog sweep tick",
        &["route"],
        crate::metrics::OPLOG_SWEEP_TICK_BUCKETS.to_vec()
    )
    .unwrap();
    static ref OPLOG_SWEEP_TRUNCATED_TOTAL: CounterVec = register_counter_vec!(
        "oplog_sweep_truncated_total",
        "Oplog sweep ticks that hit a budget or their deadline before reaching the end of the namespace",
        &["route"]
    )
    .unwrap();
    static ref OPLOG_STORAGE_RETRY_TOTAL: CounterVec = register_counter_vec!(
        "oplog_storage_retry_total",
        "Number of oplog storage operation retries due to transient errors or indeterminate writes",
        &["op"]
    )
    .unwrap();
    static ref OPLOG_ARCHIVE_MAINTENANCE_FAILURE_TOTAL: CounterVec = register_counter_vec!(
        "oplog_archive_maintenance_failure_total",
        "Number of failed background oplog archive maintenance operations",
        &["operation"]
    )
    .unwrap();
    static ref OPLOG_EPOCH_FENCE_TOTAL: CounterVec = register_counter_vec!(
        "oplog_epoch_fence_total",
        "Oplog operations checked against the shard epoch: `op` is `record` for an open \
         recording its epoch, `append` for a write and `drop_prefix` for a trim after \
         archiving, and `archive_record`, `archive_append`, `archive_drop_prefix` and \
         `archive_delete_empty` for the same on a compressed archive level or a blob level's \
         manifest; `outcome` is `accepted` or `refused`",
        &["op", "outcome"]
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

pub fn record_archive_maintenance_failure(operation: &'static str) {
    OPLOG_ARCHIVE_MAINTENANCE_FAILURE_TOTAL
        .with_label_values(&[operation])
        .inc();
}

pub fn record_oplog_epoch_fence(op: &'static str, refused: bool) {
    OPLOG_EPOCH_FENCE_TOTAL
        .with_label_values(&[op, if refused { "refused" } else { "accepted" }])
        .inc();
}

pub fn record_oplog_sweep_outcome(route: &str, outcome: &'static str, count: u64) {
    if count > 0 {
        OPLOG_SWEEP_OUTCOME_TOTAL
            .with_label_values(&[route, outcome])
            .inc_by(count as f64);
    }
}

pub fn record_oplog_sweep_tick(route: &str, duration: std::time::Duration, truncated: bool) {
    OPLOG_SWEEP_TICK_TIME
        .with_label_values(&[route])
        .observe(duration.as_secs_f64());
    if truncated {
        OPLOG_SWEEP_TRUNCATED_TOTAL
            .with_label_values(&[route])
            .inc();
    }
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
