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
use std::time::Duration;

lazy_static! {
    static ref COMPILATION_QUEUE_LENGTH: Gauge =
        catalog::COMPONENT_COMPILATION_QUEUE_LENGTH.register();
    static ref UPLOAD_QUEUE_LENGTH: Gauge =
        catalog::COMPONENT_COMPILATION_UPLOAD_QUEUE_LENGTH.register();
}

pub fn increment_queue_length() {
    COMPILATION_QUEUE_LENGTH.inc();
}

pub fn decrement_queue_length() {
    COMPILATION_QUEUE_LENGTH.dec();
}

pub fn increment_upload_queue_length() {
    UPLOAD_QUEUE_LENGTH.inc();
}

pub fn decrement_upload_queue_length() {
    UPLOAD_QUEUE_LENGTH.dec();
}

lazy_static! {
    pub static ref COMPILATION_TIME_SECONDS: Histogram =
        catalog::COMPONENT_COMPILATION_TIME_SECONDS.register();
}

pub fn record_compilation_time(duration: Duration) {
    COMPILATION_TIME_SECONDS.observe(duration.as_secs_f64());
}

pub fn register_all() -> Registry {
    golem_service_base::metrics::record_build_info();

    default_registry().clone()
}
