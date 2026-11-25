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
    static ref LIFECYCLE_TOTAL: CounterVec = catalog::DURABLE_STREAM_LIFECYCLE_TOTAL.register();
    static ref OPEN_STREAMS: IntGauge = catalog::DURABLE_STREAM_OPEN.register();
    static ref PRODUCER_OPERATION_TOTAL: CounterVec =
        catalog::DURABLE_STREAM_PRODUCER_OPERATION_TOTAL.register();
    static ref CATCH_UP_DISTANCE_EVENTS: Histogram =
        catalog::DURABLE_STREAM_CATCH_UP_DISTANCE_EVENTS.register();
    static ref LIVE_JOIN_TOTAL: CounterVec = catalog::DURABLE_STREAM_LIVE_JOIN_TOTAL.register();
    static ref ACTIVE_READERS: IntGauge = catalog::DURABLE_STREAM_ACTIVE_READERS.register();
    static ref BACKPRESSURE_TOTAL: IntCounter =
        catalog::DURABLE_STREAM_BACKPRESSURE_TOTAL.register();
    static ref ATTEMPT_TOTAL: CounterVec = catalog::DURABLE_STREAM_ATTEMPT_TOTAL.register();
    static ref ATTACHMENT_EPOCH: Histogram = catalog::DURABLE_STREAM_ATTACHMENT_EPOCH.register();
    static ref JOURNAL_LAG_EVENTS: Histogram =
        catalog::DURABLE_STREAM_JOURNAL_LAG_EVENTS.register();
    static ref ATTACHMENT_OPERATION_TOTAL: CounterVec =
        catalog::DURABLE_STREAM_ATTACHMENT_OPERATION_TOTAL.register();
    static ref LEASE_REMAINING_SECONDS: Histogram =
        catalog::DURABLE_STREAM_LEASE_REMAINING_SECONDS.register();
    static ref RECONCILIATION_TOTAL: CounterVec =
        catalog::DURABLE_STREAM_RECONCILIATION_TOTAL.register();
    static ref CASCADE_TOTAL: CounterVec = catalog::DURABLE_STREAM_CASCADE_TOTAL.register();
    static ref LIMIT_VIOLATION_TOTAL: CounterVec =
        catalog::DURABLE_STREAM_LIMIT_VIOLATION_TOTAL.register();
}

pub fn record_lifecycle(event: &'static str) {
    LIFECYCLE_TOTAL.with_label_values(&[event]).inc();
}

pub fn add_open_streams(count: usize) {
    OPEN_STREAMS.add(count as i64);
}

pub fn remove_open_streams(count: usize) {
    OPEN_STREAMS.sub(count as i64);
}

pub fn record_producer_operation(operation: &'static str, replayed: bool) {
    PRODUCER_OPERATION_TOTAL
        .with_label_values(&[operation, if replayed { "replayed" } else { "committed" }])
        .inc();
}

pub fn record_catch_up(distance: usize) {
    CATCH_UP_DISTANCE_EVENTS.observe(distance as f64);
    LIVE_JOIN_TOTAL.with_label_values(&["joined"]).inc();
}

pub fn record_live_join_rejected() {
    LIVE_JOIN_TOTAL.with_label_values(&["rejected"]).inc();
}

pub fn reader_attached() {
    ACTIVE_READERS.inc();
}

pub fn reader_detached() {
    ACTIVE_READERS.dec();
}

pub fn record_backpressure() {
    BACKPRESSURE_TOTAL.inc();
}

pub fn record_attempt(operation: &'static str, outcome: &'static str, epoch: Option<u64>) {
    ATTEMPT_TOTAL.with_label_values(&[operation, outcome]).inc();
    if let Some(epoch) = epoch {
        ATTACHMENT_EPOCH.observe(epoch as f64);
    }
}

pub fn record_journal_lag(events: usize) {
    JOURNAL_LAG_EVENTS.observe(events as f64);
}

pub fn record_attachment_operation(operation: &'static str, outcome: &'static str) {
    ATTACHMENT_OPERATION_TOTAL
        .with_label_values(&[operation, outcome])
        .inc();
}

pub fn record_lease_remaining(millis: u64) {
    LEASE_REMAINING_SECONDS.observe(millis as f64 / 1_000.0);
}

pub fn record_reconciliation(outcome: &'static str) {
    RECONCILIATION_TOTAL.with_label_values(&[outcome]).inc();
}

pub fn record_cascade(outcome: &'static str) {
    CASCADE_TOTAL.with_label_values(&[outcome]).inc();
}

pub fn record_limit_violation(limit: &'static str) {
    LIMIT_VIOLATION_TOTAL.with_label_values(&[limit]).inc();
}

#[cfg(test)]
mod tests;
