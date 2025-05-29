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

use lazy_static::lazy_static;
use prometheus::*;

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

pub fn register_all() -> Registry {
    default_registry().clone()
}
