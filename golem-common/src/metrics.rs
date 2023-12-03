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
