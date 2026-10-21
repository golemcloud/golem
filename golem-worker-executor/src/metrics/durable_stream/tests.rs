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

use super::*;
use test_r::test;

#[test]
fn durable_stream_metric_catalogue_is_registered() {
    record_lifecycle("registered");
    add_open_streams(1);
    remove_open_streams(1);
    record_producer_operation("write", false);
    record_catch_up(3);
    reader_attached();
    reader_detached();
    record_backpressure();
    record_attempt("resume", "accepted", Some(2));
    record_journal_lag(1);
    record_attachment_operation("activate", "committed");
    record_lease_remaining(20_000);
    record_reconciliation("active");
    record_cascade("complete");
    record_limit_violation("item_size");

    let names = prometheus::gather()
        .into_iter()
        .map(|family| family.name().to_string())
        .collect::<std::collections::HashSet<_>>();
    for expected in [
        "golem_durable_stream_lifecycle_total",
        "golem_durable_stream_open",
        "golem_durable_stream_producer_operation_total",
        "golem_durable_stream_catch_up_distance_events",
        "golem_durable_stream_live_join_total",
        "golem_durable_stream_active_readers",
        "golem_durable_stream_backpressure_total",
        "golem_durable_stream_attempt_total",
        "golem_durable_stream_attachment_epoch",
        "golem_durable_stream_journal_lag_events",
        "golem_durable_stream_attachment_operation_total",
        "golem_durable_stream_lease_remaining_seconds",
        "golem_durable_stream_reconciliation_total",
        "golem_durable_stream_cascade_total",
        "golem_durable_stream_limit_violation_total",
    ] {
        assert!(names.contains(expected), "missing metric {expected}");
    }
}
