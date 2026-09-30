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
use std::time::Duration;

static EPHEMERAL_PHANTOM_INVOCATION_REJECTION_TOTAL: LazyLock<IntCounterVec> =
    LazyLock::new(|| {
        register_int_counter_vec!(
            "ephemeral_phantom_invocation_rejection_total",
            "Number of ephemeral invocation requests rejected for an invalid phantom ID, by reason",
            &["reason"]
        )
        .unwrap()
    });

static DURABLE_STREAM_LOAD_REJECTION_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "durable_stream_load_rejection_total",
        "Number of durable stream requests rejected by load limiting, by reason",
        &["reason"]
    )
    .unwrap()
});

static DURABLE_STREAM_APPEND_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "durable_stream_append_total",
        "Number of durable stream append requests by bounded outcome",
        &["outcome"]
    )
    .unwrap()
});

pub fn record_durable_stream_load_rejection(reason: &str) {
    DURABLE_STREAM_LOAD_REJECTION_TOTAL
        .with_label_values(&[reason])
        .inc();
}

pub fn record_durable_stream_append(outcome: &'static str) {
    DURABLE_STREAM_APPEND_TOTAL
        .with_label_values(&[outcome])
        .inc();
}

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

static HTTP_SESSION_REATTACH_ATTEMPTS_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "http_session_reattach_attempts_total",
        "Number of HTTP session reattach attempts"
    )
    .unwrap()
});
static HTTP_SESSION_REATTACH_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "http_session_reattach_seconds",
        "HTTP session reattach duration by outcome",
        &["outcome"]
    )
    .unwrap()
});
static HTTP_SESSION_STALE_FENCING_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "http_session_stale_fencing_total",
        "Number of stale HTTP sessions fenced"
    )
    .unwrap()
});
static HTTP_SESSION_EXPLICIT_CANCELLATIONS_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "http_session_explicit_cancellations_total",
        "Number of explicitly cancelled HTTP sessions"
    )
    .unwrap()
});
static HTTP_SESSION_TERMINAL_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "http_session_terminal_total",
        "Number of terminal HTTP sessions by cause",
        &["cause"]
    )
    .unwrap()
});
static HTTP_SESSION_CLEANUP_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "http_session_cleanup_seconds",
        "HTTP session cleanup duration by outcome",
        &["outcome"]
    )
    .unwrap()
});

pub fn record_http_session_reattach_attempt() {
    HTTP_SESSION_REATTACH_ATTEMPTS_TOTAL.inc();
}

pub fn record_http_session_reattach_outcome(outcome: &'static str, duration: Duration) {
    HTTP_SESSION_REATTACH_SECONDS
        .with_label_values(&[outcome])
        .observe(duration.as_secs_f64());
}

pub fn record_http_session_stale_fencing() {
    HTTP_SESSION_STALE_FENCING_TOTAL.inc();
}

pub fn record_http_session_explicit_cancellation() {
    HTTP_SESSION_EXPLICIT_CANCELLATIONS_TOTAL.inc();
}

pub fn record_http_session_terminal_cause(cause: &'static str) {
    HTTP_SESSION_TERMINAL_TOTAL
        .with_label_values(&[cause])
        .inc();
}

pub fn record_http_session_cleanup_outcome(outcome: &'static str, duration: Duration) {
    HTTP_SESSION_CLEANUP_SECONDS
        .with_label_values(&[outcome])
        .observe(duration.as_secs_f64());
}

static OPENAPI_CACHE_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "openapi_cache_total",
        "OpenAPI cache lookups and stale fencing by bounded outcome",
        &["outcome"]
    )
    .unwrap()
});

static OPENAPI_GENERATION_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "openapi_generation_seconds",
        "OpenAPI generation duration by success or bounded failure category",
        &["outcome"]
    )
    .unwrap()
});

pub fn record_openapi_cache(outcome: &'static str) {
    OPENAPI_CACHE_TOTAL.with_label_values(&[outcome]).inc();
}

pub fn record_openapi_generation(outcome: &'static str, duration: Duration) {
    OPENAPI_GENERATION_SECONDS
        .with_label_values(&[outcome])
        .observe(duration.as_secs_f64());
}

pub fn register_all() -> Registry {
    VERSION_INFO.with_label_values(&[golem_version()]).inc();

    default_registry().clone()
}
