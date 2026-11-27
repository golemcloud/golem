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

use golem_common::metrics::catalog;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref OPLOG_SVC_CALL_TOTAL: CounterVec = catalog::OPLOG_SVC_CALL_TOTAL.register();
    static ref SCHEDULED_ARCHIVE_TIME: HistogramVec =
        catalog::OPLOG_SCHEDULED_ARCHIVE_SECONDS.register();
    static ref OPLOG_RATE_LIMITED_TOTAL: Counter = catalog::OPLOG_RATE_LIMITED_TOTAL.register();
    static ref OPLOG_SWEEP_OUTCOME_TOTAL: CounterVec =
        catalog::OPLOG_SWEEP_OUTCOME_TOTAL.register();
    static ref OPLOG_SWEEP_TICK_TIME: HistogramVec = catalog::OPLOG_SWEEP_TICK_SECONDS.register();
    static ref OPLOG_SWEEP_TRUNCATED_TOTAL: CounterVec =
        catalog::OPLOG_SWEEP_TRUNCATED_TOTAL.register();
    static ref OPLOG_STORAGE_RETRY_TOTAL: CounterVec =
        catalog::OPLOG_STORAGE_RETRY_TOTAL.register();
    static ref OPLOG_ARCHIVE_MAINTENANCE_FAILURE_TOTAL: CounterVec =
        catalog::OPLOG_ARCHIVE_MAINTENANCE_FAILURE_TOTAL.register();
    static ref OPLOG_EPOCH_FENCE_TOTAL: CounterVec = catalog::OPLOG_EPOCH_FENCE_TOTAL.register();
}

pub fn record_oplog_call(api_name: &'static str) {
    OPLOG_SVC_CALL_TOTAL.with_label_values(&[api_name]).inc();
}

pub fn record_oplog_rate_limited() {
    OPLOG_RATE_LIMITED_TOTAL.inc();
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
