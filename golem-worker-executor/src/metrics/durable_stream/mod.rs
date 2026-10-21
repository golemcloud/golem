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

const EVENT_COUNT_BUCKETS: &[f64] = &[
    0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 512.0, 1_024.0, 4_096.0, 16_384.0,
];
const EPOCH_BUCKETS: &[f64] = &[
    0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 256.0, 1_024.0, 65_536.0,
];
const LEASE_REMAINING_BUCKETS: &[f64] =
    &[0.0, 1.0, 5.0, 10.0, 20.0, 30.0, 45.0, 60.0, 120.0, 300.0];

lazy_static! {
    static ref LIFECYCLE_TOTAL: CounterVec = register_counter_vec!(
        "golem_durable_stream_lifecycle_total",
        "Durable stream registrations and terminal transitions",
        &["event"]
    )
    .unwrap();
    static ref OPEN_STREAMS: IntGauge = register_int_gauge!(
        "golem_durable_stream_open",
        "Open durable streams owned by loaded producers on this executor"
    )
    .unwrap();
    static ref PRODUCER_OPERATION_TOTAL: CounterVec = register_counter_vec!(
        "golem_durable_stream_producer_operation_total",
        "Durable producer operations by bounded operation and commit outcome",
        &["operation", "outcome"]
    )
    .unwrap();
    static ref CATCH_UP_DISTANCE_EVENTS: Histogram = register_histogram!(
        "golem_durable_stream_catch_up_distance_events",
        "Committed events read before a durable reader joins the live tail",
        EVENT_COUNT_BUCKETS.to_vec()
    )
    .unwrap();
    static ref LIVE_JOIN_TOTAL: CounterVec = register_counter_vec!(
        "golem_durable_stream_live_join_total",
        "Durable live-tail join attempts by outcome",
        &["outcome"]
    )
    .unwrap();
    static ref ACTIVE_READERS: IntGauge = register_int_gauge!(
        "golem_durable_stream_active_readers",
        "Active durable live-tail readers on this executor"
    )
    .unwrap();
    static ref BACKPRESSURE_TOTAL: IntCounter = register_int_counter!(
        "golem_durable_stream_backpressure_total",
        "Durable event fan-outs that waited on a full live-reader queue"
    )
    .unwrap();
    static ref ATTEMPT_TOTAL: CounterVec = register_counter_vec!(
        "golem_durable_stream_attempt_total",
        "Durable Stream Session start, resume, and takeover attempts by outcome",
        &["operation", "outcome"]
    )
    .unwrap();
    static ref ATTACHMENT_EPOCH: Histogram = register_histogram!(
        "golem_durable_stream_attachment_epoch",
        "Accepted durable Stream Session attachment epochs",
        EPOCH_BUCKETS.to_vec()
    )
    .unwrap();
    static ref JOURNAL_LAG_EVENTS: Histogram = register_histogram!(
        "golem_durable_stream_journal_lag_events",
        "Committed source events not yet recorded in the value-only consumer journal, sampled at creation, terminal, and at most every 100 ms during consumption",
        EVENT_COUNT_BUCKETS.to_vec()
    )
    .unwrap();
    static ref ATTACHMENT_OPERATION_TOTAL: CounterVec = register_counter_vec!(
        "golem_durable_stream_attachment_operation_total",
        "Cross-oplog attachment operations by bounded operation and outcome",
        &["operation", "outcome"]
    )
    .unwrap();
    static ref LEASE_REMAINING_SECONDS: Histogram = register_histogram!(
        "golem_durable_stream_lease_remaining_seconds",
        "Remaining attachment lease time when inspected or reconciled",
        LEASE_REMAINING_BUCKETS.to_vec()
    )
    .unwrap();
    static ref RECONCILIATION_TOTAL: CounterVec = register_counter_vec!(
        "golem_durable_stream_reconciliation_total",
        "Attachment reconciliation candidates by outcome",
        &["outcome"]
    )
    .unwrap();
    static ref CASCADE_TOTAL: CounterVec = register_counter_vec!(
        "golem_durable_stream_cascade_total",
        "Producer deletion cascade dependents by durable outcome",
        &["outcome"]
    )
    .unwrap();
    static ref LIMIT_VIOLATION_TOTAL: CounterVec = register_counter_vec!(
        "golem_durable_stream_limit_violation_total",
        "Rejected durable Stream Session operations by frozen limit category",
        &["limit"]
    )
    .unwrap();
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
