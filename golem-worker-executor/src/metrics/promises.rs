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
