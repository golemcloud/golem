// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_common::metrics::DEFAULT_TIME_BUCKETS;
use lazy_static::lazy_static;
use prometheus::*;
use std::time::Duration;

lazy_static! {
    static ref COMPILATION_QUEUE_LENGTH: Gauge = register_gauge!(
        "component_compilation_queue_length",
        "Number of outstanding compilation requests"
    )
    .unwrap();
}

pub fn increment_queue_length() {
    COMPILATION_QUEUE_LENGTH.inc();
}

pub fn decrement_queue_length() {
    COMPILATION_QUEUE_LENGTH.dec();
}

lazy_static! {
    pub static ref COMPILATION_TIME_SECONDS: Histogram = register_histogram!(
        "component_compilation_time_seconds",
        "Time to compile a WASM component to native code",
        DEFAULT_TIME_BUCKETS.to_vec()
    )
    .unwrap();
}

pub fn record_compilation_time(duration: Duration) {
    COMPILATION_TIME_SECONDS.observe(duration.as_secs_f64());
}

pub fn register_all() -> Registry {
    default_registry().clone()
}
