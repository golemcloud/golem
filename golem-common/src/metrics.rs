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

pub mod catalog;

pub const DEFAULT_TIME_BUCKETS: &[f64; 11] = &[
    0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0,
];

pub const DEFAULT_SIZE_BUCKETS: &[f64; 11] = &[
    1024.0,
    4096.0,
    16384.0,
    65536.0,
    262144.0,
    1048576.0,
    4194304.0,
    16777216.0,
    67108864.0,
    268435456.0,
    1073741824.0,
];

pub const DEFAULT_COUNT_BUCKETS: &[f64; 12] = &[
    1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 512.0, 1024.0, 4096.0,
];

pub mod external_calls {
    use crate::metrics::catalog;
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref EXTERNAL_CALL_SUCCESS_SECONDS: HistogramVec =
            catalog::EXTERNAL_CALL_SUCCESS_SECONDS.register();
        static ref EXTERNAL_CALL_RESPONSE_SIZE_BYTES: HistogramVec =
            catalog::EXTERNAL_CALL_RESPONSE_SIZE_BYTES.register();
        static ref EXTERNAL_CALL_RETRY_TOTAL: CounterVec =
            catalog::EXTERNAL_CALL_RETRY_TOTAL.register();
        static ref EXTERNAL_CALL_FAILURE_TOTAL: CounterVec =
            catalog::EXTERNAL_CALL_FAILURE_TOTAL.register();
    }

    pub fn record_external_call_success(
        target_name: &'static str,
        op_name: &'static str,
        duration: Duration,
    ) {
        EXTERNAL_CALL_SUCCESS_SECONDS
            .with_label_values(&[target_name, op_name])
            .observe(duration.as_secs_f64());
    }

    pub fn record_external_call_response_size_bytes(
        target_name: &'static str,
        op_name: &'static str,
        size: usize,
    ) {
        EXTERNAL_CALL_RESPONSE_SIZE_BYTES
            .with_label_values(&[target_name, op_name])
            .observe(size as f64);
    }

    pub fn record_external_call_retry(target_name: &'static str, op_name: &'static str) {
        EXTERNAL_CALL_RETRY_TOTAL
            .with_label_values(&[target_name, op_name])
            .inc();
    }

    pub fn record_external_call_failure(target_name: &'static str, op_name: &'static str) {
        EXTERNAL_CALL_FAILURE_TOTAL
            .with_label_values(&[target_name, op_name])
            .inc();
    }
}

pub mod redis {
    use crate::metrics::catalog;
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref REDIS_SUCCESS_SECONDS: HistogramVec = catalog::REDIS_SUCCESS_SECONDS.register();
        static ref REDIS_FAILURE_TOTAL: CounterVec = catalog::REDIS_FAILURE_TOTAL.register();
        static ref REDIS_SERIALIZED_SIZE_BYTES: HistogramVec =
            catalog::REDIS_SERIALIZED_SIZE_BYTES.register();
        static ref REDIS_DESERIALIZED_SIZE_BYTES: HistogramVec =
            catalog::REDIS_DESERIALIZED_SIZE_BYTES.register();
    }

    pub fn record_redis_success(
        svc_name: &'static str,
        api_name: &'static str,
        cmd_name: &'static str,
        duration: Duration,
    ) {
        REDIS_SUCCESS_SECONDS
            .with_label_values(&[svc_name, api_name, cmd_name])
            .observe(duration.as_secs_f64());
    }

    pub fn record_redis_failure(
        svc_name: &'static str,
        api_name: &'static str,
        cmd_name: &'static str,
    ) {
        REDIS_FAILURE_TOTAL
            .with_label_values(&[svc_name, api_name, cmd_name])
            .inc();
    }

    pub fn record_redis_serialized_size(
        svc_name: &'static str,
        entity_name: &'static str,
        size: usize,
    ) {
        REDIS_SERIALIZED_SIZE_BYTES
            .with_label_values(&[svc_name, entity_name])
            .observe(size as f64);
    }

    pub fn record_redis_deserialized_size(
        svc_name: &'static str,
        entity_name: &'static str,
        size: usize,
    ) {
        REDIS_DESERIALIZED_SIZE_BYTES
            .with_label_values(&[svc_name, entity_name])
            .observe(size as f64);
    }
}

pub mod caching {
    use crate::metrics::catalog;
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref CACHE_SIZE: GaugeVec = catalog::CACHE_SIZE.register();
        static ref CACHE_CAPACITY: GaugeVec = catalog::CACHE_CAPACITY.register();
        static ref CACHE_HIT_TOTAL: CounterVec = catalog::CACHE_HIT_TOTAL.register();
        static ref CACHE_MISS_TOTAL: CounterVec = catalog::CACHE_MISS_TOTAL.register();
        static ref CACHE_EVICTION_TOTAL: CounterVec = catalog::CACHE_EVICTION_TOTAL.register();
    }

    pub fn record_cache_size(cache: &'static str, size: usize) {
        CACHE_SIZE.with_label_values(&[cache]).set(size as f64);
    }

    pub fn record_cache_capacity(cache: &'static str, capacity: usize) {
        CACHE_CAPACITY
            .with_label_values(&[cache])
            .set(capacity as f64);
    }

    pub fn record_cache_hit(cache: &'static str) {
        CACHE_HIT_TOTAL.with_label_values(&[cache]).inc();
    }

    pub fn record_cache_miss(cache: &'static str) {
        CACHE_MISS_TOTAL.with_label_values(&[cache]).inc();
    }

    pub fn record_cache_eviction(cache: &'static str, trigger: &'static str) {
        CACHE_EVICTION_TOTAL
            .with_label_values(&[cache, trigger])
            .inc();
    }
}

pub mod api {
    use crate::metrics::catalog;
    use lazy_static::lazy_static;
    use prometheus::{Gauge, HistogramVec};
    use std::fmt::Debug;
    use tracing::{Span, debug, error};
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    lazy_static! {
        static ref API_SUCCESS_SECONDS: HistogramVec = catalog::API_SUCCESS_SECONDS.register();
        static ref API_FAILURE_SECONDS: HistogramVec = catalog::API_FAILURE_SECONDS.register();
        static ref GRPC_API_ACTIVE_STREAMS: Gauge = catalog::GRPC_API_ACTIVE_STREAMS.register();
    }

    pub fn record_api_success(
        api_name: &'static str,
        api_type: &'static str,
        duration: std::time::Duration,
    ) {
        API_SUCCESS_SECONDS
            .with_label_values(&[api_name, api_type])
            .observe(duration.as_secs_f64());
    }

    pub fn record_api_failure(
        api_name: &'static str,
        api_type: &'static str,
        error_kind: &'static str,
        duration: std::time::Duration,
    ) {
        API_FAILURE_SECONDS
            .with_label_values(&[api_name, api_type, error_kind])
            .observe(duration.as_secs_f64());
    }

    pub fn record_new_grpc_api_active_stream() {
        GRPC_API_ACTIVE_STREAMS.inc();
    }

    pub fn record_closed_grpc_api_active_stream() {
        GRPC_API_ACTIVE_STREAMS.dec();
    }

    pub struct RecordedApiRequest {
        api_name: &'static str,
        api_type: &'static str,
        start_time: Option<std::time::Instant>,
        pub span: Span,
    }

    pub trait ApiErrorDetails {
        fn trace_error_kind(&self) -> &'static str;

        fn is_expected(&self) -> bool;

        fn take_cause(&mut self) -> Option<anyhow::Error>;
    }

    impl RecordedApiRequest {
        pub fn new(api_name: &'static str, api_type: &'static str, span: Span) -> Self {
            Self {
                api_name,
                api_type,
                start_time: Some(std::time::Instant::now()),
                span,
            }
        }

        pub fn succeed<T>(self, result: T) -> T {
            self.handle_success();
            result
        }

        pub fn fail<T, E: Debug + ApiErrorDetails>(self, result: T, error: &mut E) -> T {
            self.handle_failure(error);
            result
        }

        pub fn result<T, E: ApiErrorDetails + Debug>(
            self,
            mut result: Result<T, E>,
        ) -> Result<T, E> {
            match &mut result {
                Ok(_) => self.handle_success(),
                Err(error) => self.handle_failure(error),
            }
            result
        }

        fn handle_success(mut self) {
            if let Some(start) = self.start_time.take() {
                self.span.in_scope(|| {
                    let elapsed = start.elapsed();
                    debug!(elapsed_ms = elapsed.as_millis(), "API request succeeded");

                    record_api_success(self.api_name, self.api_type, elapsed);
                })
            }
        }

        fn handle_failure<E: ApiErrorDetails + Debug>(mut self, error: &mut E) {
            if let Some(start) = self.start_time.take() {
                self.span.in_scope(|| {
                    let elapsed = start.elapsed();

                    if error.is_expected() {
                        // expected error, no need to display cause.
                        // TODO: make this nicer
                        error.take_cause();
                        debug!(
                            elapsed_ms = elapsed.as_millis(),
                            error = format!("{:?}", error),
                            "API request failed",
                        );
                    } else {
                        error!(
                            elapsed_ms = elapsed.as_millis(),
                            error = format!("{:?}", error),
                            "API request failed",
                        );
                    }

                    record_api_failure(
                        self.api_name,
                        self.api_type,
                        error.trace_error_kind(),
                        elapsed,
                    );
                })
            }
        }
    }

    impl Drop for RecordedApiRequest {
        fn drop(&mut self) {
            if let Some(start) = self.start_time.take() {
                // Neither `succeed` nor `fail` ran: the caller went away before the
                // request produced a result. Not an error status - the server did not
                // fail - but recorded, because work this request started can outlive
                // it.
                self.span.set_attribute("cancelled", true);
                record_api_failure(self.api_name, self.api_type, "Drop", start.elapsed());
            }
        }
    }

    /// Creates the entry-point span for an inbound gRPC request.
    ///
    /// `otel.kind = "server"` marks it as the service's inbound edge, which is what
    /// lets a trace backend derive the service graph. Without it every span Golem
    /// emits is exported as `Internal`.
    #[macro_export]
    macro_rules! recorded_grpc_api_request {
        ($api_name:expr,  $($fields:tt)*) => {
            {
                let span = tracing::span!(tracing::Level::INFO, concat!("gRPC ", $api_name), api = $api_name,  api_type = "grpc", otel.kind = "server", $($fields)*);
                $crate::metrics::api::RecordedApiRequest::new($api_name, "grpc", span)
            }
        };
    }

    /// Creates the entry-point span for an inbound HTTP request. See
    /// [`recorded_grpc_api_request!`] for why `otel.kind` is set.
    #[macro_export]
    macro_rules! recorded_http_api_request {
        ($api_name:expr,  $($fields:tt)*) => {
            {
                let span = tracing::span!(tracing::Level::INFO, concat!("HTTP ", $api_name), api = $api_name,  api_type = "http", otel.kind = "server", $($fields)*);
                $crate::metrics::api::RecordedApiRequest::new($api_name, "http", span)
            }
        };
    }
}

pub mod db {
    use crate::metrics::catalog;
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref DB_SUCCESS_SECONDS: HistogramVec = catalog::DB_SUCCESS_SECONDS.register();
        static ref DB_FAILURE_TOTAL: CounterVec = catalog::DB_FAILURE_TOTAL.register();
        static ref DB_ROLLBACK_TOTAL: CounterVec = catalog::DB_ROLLBACK_TOTAL.register();
        static ref DB_TRANSACTION_SECONDS: HistogramVec =
            catalog::DB_TRANSACTION_SECONDS.register();
        static ref DB_SERIALIZED_SIZE_BYTES: HistogramVec =
            catalog::DB_SERIALIZED_SIZE_BYTES.register();
        static ref DB_DESERIALIZED_SIZE_BYTES: HistogramVec =
            catalog::DB_DESERIALIZED_SIZE_BYTES.register();
        static ref DB_POOL_SIZE: GaugeVec = catalog::DB_POOL_SIZE.register();
        static ref DB_POOL_IDLE: GaugeVec = catalog::DB_POOL_IDLE.register();
        static ref DB_POOL_MAX: GaugeVec = catalog::DB_POOL_MAX.register();
    }

    pub fn record_db_success(
        db_type: &'static str,
        svc_name: &'static str,
        api_name: &'static str,
        duration: Duration,
    ) {
        DB_SUCCESS_SECONDS
            .with_label_values(&[db_type, svc_name, api_name])
            .observe(duration.as_secs_f64());
    }

    pub fn record_db_failure(
        db_type: &'static str,
        svc_name: &'static str,
        api_name: &'static str,
    ) {
        DB_FAILURE_TOTAL
            .with_label_values(&[db_type, svc_name, api_name])
            .inc();
    }

    pub fn record_db_rollback(
        db_type: &'static str,
        svc_name: &'static str,
        api_name: &'static str,
    ) {
        DB_ROLLBACK_TOTAL
            .with_label_values(&[db_type, svc_name, api_name])
            .inc();
    }

    pub fn record_db_transaction(
        db_type: &'static str,
        svc_name: &'static str,
        api_name: &'static str,
        duration: Duration,
    ) {
        DB_TRANSACTION_SECONDS
            .with_label_values(&[db_type, svc_name, api_name])
            .observe(duration.as_secs_f64());
    }

    pub fn set_db_pool_stats(
        db_type: &'static str,
        svc_name: &'static str,
        size: u32,
        idle: u32,
        max: u32,
    ) {
        DB_POOL_SIZE
            .with_label_values(&[db_type, svc_name])
            .set(size as f64);
        DB_POOL_IDLE
            .with_label_values(&[db_type, svc_name])
            .set(idle as f64);
        DB_POOL_MAX
            .with_label_values(&[db_type, svc_name])
            .set(max as f64);
    }

    pub fn record_db_serialized_size(
        db_type: &'static str,
        svc_name: &'static str,
        entity_name: &'static str,
        size: usize,
    ) {
        DB_SERIALIZED_SIZE_BYTES
            .with_label_values(&[db_type, svc_name, entity_name])
            .observe(size as f64);
    }

    pub fn record_db_deserialized_size(
        db_type: &'static str,
        svc_name: &'static str,
        entity_name: &'static str,
        size: usize,
    ) {
        DB_DESERIALIZED_SIZE_BYTES
            .with_label_values(&[db_type, svc_name, entity_name])
            .observe(size as f64);
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::{SpanKind, Status};
    use test_r::test;

    use crate::tracing::test::otel::{exported_spans, named};

    /// Golem's own entry-point spans have to declare `otel.kind`, otherwise every
    /// span the service emits is exported as `Internal` and a trace backend cannot
    /// tell which spans are the inbound edges of the service.
    #[test]
    fn recorded_grpc_api_request_is_a_server_span() {
        let spans = exported_spans(|| {
            let record = crate::recorded_grpc_api_request!("test_api",);
            drop(record);
        });

        assert_eq!(named(&spans, "gRPC test_api").span_kind, SpanKind::Server);
    }

    /// A request that is dropped without `succeed`/`fail` was abandoned by its
    /// caller. The span has to say so: work the request started can outlive it, and
    /// a reader seeing a child span run past its parent needs to be able to tell an
    /// abandoned request from a broken trace.
    #[test]
    fn an_abandoned_request_is_marked_cancelled() {
        let spans = exported_spans(|| {
            let record = crate::recorded_grpc_api_request!("test_api",);
            drop(record);
        });

        let span = named(&spans, "gRPC test_api");
        assert!(
            span.attributes
                .iter()
                .any(|kv| kv.key.as_str() == "cancelled" && kv.value.as_str() == "true"),
            "expected a cancelled attribute, got {:?}",
            span.attributes
        );
        assert!(
            !matches!(span.status, Status::Error { .. }),
            "an abandoned request is not a server error"
        );
    }

    #[test]
    fn a_completed_request_is_not_marked_cancelled() {
        let spans = exported_spans(|| {
            let record = crate::recorded_grpc_api_request!("test_api",);
            record.succeed(());
        });

        let span = named(&spans, "gRPC test_api");
        assert!(
            !span
                .attributes
                .iter()
                .any(|kv| kv.key.as_str() == "cancelled"),
            "got {:?}",
            span.attributes
        );
    }

    #[test]
    fn recorded_http_api_request_is_a_server_span() {
        let spans = exported_spans(|| {
            let record = crate::recorded_http_api_request!("test_api",);
            drop(record);
        });

        assert_eq!(named(&spans, "HTTP test_api").span_kind, SpanKind::Server);
    }
}
