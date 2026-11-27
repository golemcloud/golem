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
use golem_common::model::agent::InvocationFreshnessDisposition;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref EPHEMERAL_PROMISE_WAITING: Gauge = catalog::EPHEMERAL_PROMISE_WAITING.register();
    static ref EPHEMERAL_NON_SUSPENDING_FAILURE_TOTAL: CounterVec =
        catalog::EPHEMERAL_NON_SUSPENDING_FAILURE_TOTAL.register();
    static ref EPHEMERAL_INVOCATION_ATTEMPT_TOTAL: CounterVec =
        catalog::EPHEMERAL_INVOCATION_ATTEMPT_TOTAL.register();
    static ref EPHEMERAL_KNOWN_FRESH_VALIDATION_FAILURE_TOTAL: Counter =
        catalog::EPHEMERAL_KNOWN_FRESH_VALIDATION_FAILURE_TOTAL.register();
    static ref EPHEMERAL_LOWER_OPLOG_EXISTENCE_READ_TOTAL: Counter =
        catalog::EPHEMERAL_LOWER_OPLOG_EXISTENCE_READ_TOTAL.register();
    static ref EPHEMERAL_INACTIVE_INVOCATION_FAILURE_TOTAL: Counter =
        catalog::EPHEMERAL_INACTIVE_INVOCATION_FAILURE_TOTAL.register();
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
