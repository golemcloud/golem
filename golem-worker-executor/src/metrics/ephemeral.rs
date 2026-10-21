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

use golem_common::model::agent::InvocationFreshnessDisposition;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref EPHEMERAL_PROMISE_WAITING: Gauge = register_gauge!(
        "ephemeral_promise_waiting",
        "Number of ephemeral agents currently waiting on promises"
    )
    .unwrap();
    static ref EPHEMERAL_NON_SUSPENDING_FAILURE_TOTAL: CounterVec = register_counter_vec!(
        "ephemeral_non_suspending_failure_total",
        "Number of ephemeral failures that replace suspension",
        &["reason"]
    )
    .unwrap();
    static ref EPHEMERAL_INVOCATION_ATTEMPT_TOTAL: CounterVec = register_counter_vec!(
        "ephemeral_invocation_attempt_total",
        "Number of ephemeral invocation attempts by freshness disposition",
        &["disposition"]
    )
    .unwrap();
    static ref EPHEMERAL_KNOWN_FRESH_VALIDATION_FAILURE_TOTAL: Counter = register_counter!(
        "ephemeral_known_fresh_validation_failure_total",
        "Number of rejected KnownFresh invocation requests"
    )
    .unwrap();
    static ref EPHEMERAL_LOWER_OPLOG_EXISTENCE_READ_TOTAL: Counter = register_counter!(
        "ephemeral_lower_oplog_existence_read_total",
        "Number of lower oplog existence reads for ephemeral agents"
    )
    .unwrap();
    static ref EPHEMERAL_INACTIVE_INVOCATION_FAILURE_TOTAL: Counter = register_counter!(
        "ephemeral_inactive_invocation_failure_total",
        "Number of attempts to invoke or resume inactive ephemeral agents"
    )
    .unwrap();
}

pub fn inc_promise_waiting() {
    EPHEMERAL_PROMISE_WAITING.inc();
}

pub fn dec_promise_waiting() {
    EPHEMERAL_PROMISE_WAITING.dec();
}

pub fn record_non_suspending_failure(reason: &'static str) {
    EPHEMERAL_NON_SUSPENDING_FAILURE_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub fn record_invocation_attempt(disposition: InvocationFreshnessDisposition) {
    let disposition = match disposition {
        InvocationFreshnessDisposition::MayExist => "may-exist",
        InvocationFreshnessDisposition::KnownFresh => "known-fresh",
    };
    EPHEMERAL_INVOCATION_ATTEMPT_TOTAL
        .with_label_values(&[disposition])
        .inc();
}

pub fn record_known_fresh_validation_failure() {
    EPHEMERAL_KNOWN_FRESH_VALIDATION_FAILURE_TOTAL.inc();
}

pub fn record_lower_oplog_existence_read() {
    EPHEMERAL_LOWER_OPLOG_EXISTENCE_READ_TOTAL.inc();
}

pub fn record_inactive_invocation_failure() {
    EPHEMERAL_INACTIVE_INVOCATION_FAILURE_TOTAL.inc();
}
