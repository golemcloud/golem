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

use golem_common::golem_version;
use golem_service_base::metrics::VERSION_INFO;
use prometheus::*;
use std::sync::LazyLock;

static EPHEMERAL_PHANTOM_INVOCATION_REJECTION_TOTAL: LazyLock<IntCounterVec> =
    LazyLock::new(|| {
        register_int_counter_vec!(
            "ephemeral_phantom_invocation_rejection_total",
            "Number of ephemeral invocation requests rejected for an invalid phantom ID, by reason",
            &["reason"]
        )
        .unwrap()
    });

pub fn record_ephemeral_explicit_phantom_invocation_rejection() {
    EPHEMERAL_PHANTOM_INVOCATION_REJECTION_TOTAL
        .with_label_values(&["explicit-phantom"])
        .inc();
}

pub fn record_ephemeral_derived_phantom_mismatch_rejection() {
    EPHEMERAL_PHANTOM_INVOCATION_REJECTION_TOTAL
        .with_label_values(&["derived-phantom-mismatch"])
        .inc();
}

pub fn register_all() -> Registry {
    VERSION_INFO.with_label_values(&[golem_version()]).inc();

    default_registry().clone()
}
