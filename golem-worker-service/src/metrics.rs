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
use prometheus::*;
use std::sync::LazyLock;
use std::time::Duration;

static EPHEMERAL_PHANTOM_INVOCATION_REJECTION_TOTAL: LazyLock<IntCounterVec> =
    LazyLock::new(|| catalog::EPHEMERAL_PHANTOM_INVOCATION_REJECTION_TOTAL.register());

static DURABLE_STREAM_LOAD_REJECTION_TOTAL: LazyLock<IntCounterVec> =
    LazyLock::new(|| catalog::DURABLE_STREAM_LOAD_REJECTION_TOTAL.register());

static DURABLE_STREAM_APPEND_TOTAL: LazyLock<IntCounterVec> =
    LazyLock::new(|| catalog::DURABLE_STREAM_APPEND_TOTAL.register());

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

static HTTP_SESSION_REATTACH_ATTEMPTS_TOTAL: LazyLock<IntCounter> =
    LazyLock::new(|| catalog::HTTP_SESSION_REATTACH_ATTEMPTS_TOTAL.register());
static HTTP_SESSION_REATTACH_SECONDS: LazyLock<HistogramVec> =
    LazyLock::new(|| catalog::HTTP_SESSION_REATTACH_SECONDS.register());
static HTTP_SESSION_STALE_FENCING_TOTAL: LazyLock<IntCounter> =
    LazyLock::new(|| catalog::HTTP_SESSION_STALE_FENCING_TOTAL.register());
static HTTP_SESSION_EXPLICIT_CANCELLATIONS_TOTAL: LazyLock<IntCounter> =
    LazyLock::new(|| catalog::HTTP_SESSION_EXPLICIT_CANCELLATIONS_TOTAL.register());
static HTTP_SESSION_TERMINAL_TOTAL: LazyLock<IntCounterVec> =
    LazyLock::new(|| catalog::HTTP_SESSION_TERMINAL_TOTAL.register());
static HTTP_SESSION_CLEANUP_SECONDS: LazyLock<HistogramVec> =
    LazyLock::new(|| catalog::HTTP_SESSION_CLEANUP_SECONDS.register());

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

static OPENAPI_CACHE_TOTAL: LazyLock<IntCounterVec> =
    LazyLock::new(|| catalog::OPENAPI_CACHE_TOTAL.register());

static OPENAPI_GENERATION_SECONDS: LazyLock<HistogramVec> =
    LazyLock::new(|| catalog::OPENAPI_GENERATION_SECONDS.register());

pub fn record_openapi_cache(outcome: &'static str) {
    OPENAPI_CACHE_TOTAL.with_label_values(&[outcome]).inc();
}

pub fn record_openapi_generation(outcome: &'static str, duration: Duration) {
    OPENAPI_GENERATION_SECONDS
        .with_label_values(&[outcome])
        .observe(duration.as_secs_f64());
}

static HTTP_API_ACTIVE_STREAMS: LazyLock<IntGaugeVec> =
    LazyLock::new(|| catalog::HTTP_API_ACTIVE_STREAMS.register());

#[derive(Clone, Copy)]
pub enum HttpApiStreamKind {
    Connect,
    InvocationSession,
    DurableStreamSse,
}

impl HttpApiStreamKind {
    const ALL: [Self; 3] = [
        Self::Connect,
        Self::InvocationSession,
        Self::DurableStreamSse,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::InvocationSession => "invocation_session",
            Self::DurableStreamSse => "durable_stream_sse",
        }
    }
}

/// Counts one open HTTP API stream in `golem_http_api_active_streams` until it is dropped, so a
/// stream that ends by cancellation or failure leaves the gauge as well.
pub struct ActiveHttpApiStream(HttpApiStreamKind);

impl ActiveHttpApiStream {
    pub fn open(kind: HttpApiStreamKind) -> Self {
        HTTP_API_ACTIVE_STREAMS
            .with_label_values(&[kind.label()])
            .inc();
        Self(kind)
    }
}

impl Drop for ActiveHttpApiStream {
    fn drop(&mut self) {
        HTTP_API_ACTIVE_STREAMS
            .with_label_values(&[self.0.label()])
            .dec();
    }
}

pub fn register_all() -> Registry {
    golem_service_base::metrics::record_build_info();
    for kind in HttpApiStreamKind::ALL {
        HTTP_API_ACTIVE_STREAMS.with_label_values(&[kind.label()]);
    }

    default_registry().clone()
}
