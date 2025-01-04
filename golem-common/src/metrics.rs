// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;

    use crate::metrics::DEFAULT_TIME_BUCKETS;

    lazy_static! {
        static ref EXTERNAL_CALL_SUCCESS_SECONDS: HistogramVec = register_histogram_vec!(
            "external_call_success_seconds",
            "Duration of successful external calls",
            &["target", "op"],
            DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref EXTERNAL_CALL_RESPONSE_SIZE_BYTES: HistogramVec = register_histogram_vec!(
            "external_call_response_size_bytes",
            "Size of response of external calls",
            &["target", "op"],
            crate::metrics::DEFAULT_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
        static ref EXTERNAL_CALL_RETRY_TOTAL: CounterVec = register_counter_vec!(
            "external_call_retry_total",
            "Number of failed external calls that get retried",
            &["target", "op"]
        )
        .unwrap();
        static ref EXTERNAL_CALL_FAILURE_TOTAL: CounterVec = register_counter_vec!(
            "external_call_failure_total",
            "Number of failed external calls that not to be retried",
            &["target", "op"]
        )
        .unwrap();
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
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref REDIS_SUCCESS_SECONDS: HistogramVec = register_histogram_vec!(
            "redis_success_seconds",
            "Duration of successful Redis calls",
            &["svc", "api", "cmd"],
            crate::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref REDIS_FAILURE_TOTAL: CounterVec = register_counter_vec!(
            "redis_failure_total",
            "Number of failed Redis calls",
            &["svc", "api", "cmd"]
        )
        .unwrap();
        static ref REDIS_SERIALIZED_SIZE_BYTES: HistogramVec = register_histogram_vec!(
            "redis_serialized_size_bytes",
            "Size of serialized Redis entities",
            &["svc", "entity"],
            crate::metrics::DEFAULT_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
        static ref REDIS_DESERIALIZED_SIZE_BYTES: HistogramVec = register_histogram_vec!(
            "redis_deserialized_size_bytes",
            "Size of deserialized Redis entities",
            &["svc", "entity"],
            crate::metrics::DEFAULT_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
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
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref CACHE_SIZE: GaugeVec =
            register_gauge_vec!("cache_size", "Current size of the cache", &["cache"]).unwrap();
        static ref CACHE_CAPACITY: GaugeVec = register_gauge_vec!(
            "cache_capacity",
            "Current maximal capacity of the cache",
            &["cache"]
        )
        .unwrap();
        static ref CACHE_HIT_TOTAL: CounterVec =
            register_counter_vec!("cache_hit_total", "Number of cache hits", &["cache"]).unwrap();
        static ref CACHE_MISS_TOTAL: CounterVec =
            register_counter_vec!("cache_miss_total", "Number of cache misses", &["cache"])
                .unwrap();
        static ref CACHE_EVICTION_TOTAL: CounterVec = register_counter_vec!(
            "cache_eviction_total",
            "Number of cache evictions",
            &["cache", "trigger"]
        )
        .unwrap();
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
    use lazy_static::lazy_static;
    use prometheus::{register_gauge, register_histogram_vec, Gauge, HistogramVec};
    use std::fmt::Debug;
    use tracing::{error, info, Span};

    lazy_static! {
        static ref API_SUCCESS_SECONDS: HistogramVec = register_histogram_vec!(
            "api_success_seconds",
            "Time taken for successfully serving API requests",
            &["api", "api_type"],
            crate::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref API_FAILURE_SECONDS: HistogramVec = register_histogram_vec!(
            "api_failure_seconds",
            "Time taken for serving failed API requests",
            &["api", "api_type", "error"],
            crate::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref GRPC_API_ACTIVE_STREAMS: Gauge = register_gauge!(
            "grpc_api_active_streams",
            "Number of active gRPC API streams"
        )
        .unwrap();
        static ref HTTP_API_ACTIVE_STREAMS: Gauge = register_gauge!(
            "http_api_active_streams",
            "Number of active HTTP API streams"
        )
        .unwrap();
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

    pub fn record_new_http_api_active_stream() {
        HTTP_API_ACTIVE_STREAMS.inc();
    }

    pub fn record_closed_http_api_active_stream() {
        HTTP_API_ACTIVE_STREAMS.dec();
    }

    pub struct RecordedApiRequest {
        api_name: &'static str,
        api_type: &'static str,
        start_time: Option<std::time::Instant>,
        pub span: Span,
    }

    pub trait TraceErrorKind {
        fn trace_error_kind(&self) -> &'static str;
    }

    impl TraceErrorKind for &'static str {
        fn trace_error_kind(&self) -> &'static str {
            self
        }
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

        pub fn succeed<T>(mut self, result: T) -> T {
            match self.start_time.take() {
                Some(start) => self.span.in_scope(|| {
                    let elapsed = start.elapsed();
                    info!(elapsed_ms = elapsed.as_millis(), "API request succeeded");

                    record_api_success(self.api_name, self.api_type, elapsed);
                    result
                }),
                None => result,
            }
        }

        pub fn fail<T, E: Debug + TraceErrorKind>(mut self, result: T, error: &E) -> T {
            match self.start_time.take() {
                Some(start) => self.span.in_scope(|| {
                    let elapsed = start.elapsed();
                    error!(
                        elapsed_ms = elapsed.as_millis(),
                        error = format!("{:?}", error),
                        "API request failed",
                    );

                    record_api_failure(
                        self.api_name,
                        self.api_type,
                        error.trace_error_kind(),
                        elapsed,
                    );
                    result
                }),

                None => result,
            }
        }

        pub fn result<T, E: Clone + TraceErrorKind + Debug>(
            self,
            result: Result<T, E>,
        ) -> Result<T, E> {
            match result {
                ok @ Ok(_) => self.succeed(ok),
                Err(error) => self.fail(Err(error.clone()), &error),
            }
        }
    }

    impl Drop for RecordedApiRequest {
        fn drop(&mut self) {
            if let Some(start) = self.start_time.take() {
                record_api_failure(self.api_name, self.api_type, "Drop", start.elapsed());
            }
        }
    }

    #[macro_export]
    macro_rules! recorded_grpc_api_request {
        ($api_name:expr,  $($fields:tt)*) => {
            {
                let span = tracing::span!(tracing::Level::INFO, "api_request", api = $api_name,  api_type = "grpc", $($fields)*);
                $crate::metrics::api::RecordedApiRequest::new($api_name, "grpc", span)
            }
        };
    }

    #[macro_export]
    macro_rules! recorded_http_api_request {
        ($api_name:expr,  $($fields:tt)*) => {
            {
                let span = tracing::span!(tracing::Level::INFO, "api_request", api = $api_name,  api_type = "http", $($fields)*);
                $crate::metrics::api::RecordedApiRequest::new($api_name, "http", span)
            }
        };
    }
}

pub mod db {
    use std::time::Duration;

    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref DB_SUCCESS_SECONDS: HistogramVec = register_histogram_vec!(
            "db_success_seconds",
            "Duration of successful db calls",
            &["db_type", "svc", "api"],
            crate::metrics::DEFAULT_TIME_BUCKETS.to_vec()
        )
        .unwrap();
        static ref DB_FAILURE_TOTAL: CounterVec = register_counter_vec!(
            "db_failure_total",
            "Number of failed db calls",
            &["db_type", "svc", "api"]
        )
        .unwrap();
        static ref DB_SERIALIZED_SIZE_BYTES: HistogramVec = register_histogram_vec!(
            "db_serialized_size_bytes",
            "Size of serialized db entities",
            &["db_type", "svc", "entity"],
            crate::metrics::DEFAULT_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
        static ref DB_DESERIALIZED_SIZE_BYTES: HistogramVec = register_histogram_vec!(
            "db_deserialized_size_bytes",
            "Size of deserialized db entities",
            &["db_type", "svc", "entity"],
            crate::metrics::DEFAULT_SIZE_BUCKETS.to_vec()
        )
        .unwrap();
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
