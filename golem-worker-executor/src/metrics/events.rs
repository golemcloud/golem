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
    static ref EVENT_TOTAL: CounterVec = register_counter_vec!(
        "event_total",
        "Number of events produced by the server",
        &["event"]
    )
    .unwrap();
    static ref EVENT_BROADCAST_TOTAL: CounterVec = register_counter_vec!(
        "event_broadcast_total",
        "Number of events broadcast by the server",
        &["event"]
    )
    .unwrap();
}

pub fn record_event(event: &'static str) {
    EVENT_TOTAL.with_label_values(&[event]).inc();
}

pub fn record_broadcast_event(event: &'static str) {
    EVENT_BROADCAST_TOTAL.with_label_values(&[event]).inc();
}
