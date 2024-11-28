// Copyright 2024 Golem Cloud
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

use std::time::Duration;

use golem_common::metrics::DEFAULT_TIME_BUCKETS;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref RDBMS_SUCCESS_SECONDS: HistogramVec = register_histogram_vec!(
        "rdbms_success_seconds",
        "Duration of successful rdbms calls",
        &["rdbms_type", "api"],
        DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
    static ref RDBMS_FAILURE_TOTAL: CounterVec = register_counter_vec!(
        "rdbms_failure_total",
        "Number of failed rdbms calls",
        &["rdbms_type", "api"]
    )
    .unwrap();
}

pub fn record_rdbms_success(rdbms_type: &str, api_name: &str, duration: Duration) {
    RDBMS_SUCCESS_SECONDS
        .with_label_values(&[rdbms_type, api_name])
        .observe(duration.as_secs_f64());
}

pub fn record_rdbms_failure(rdbms_type: &str, api_name: &str) {
    RDBMS_FAILURE_TOTAL
        .with_label_values(&[rdbms_type, api_name])
        .inc();
}
