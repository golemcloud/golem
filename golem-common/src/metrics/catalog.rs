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

//! The catalog of every Prometheus metric that a Golem service exports.
//!
//! Each entry declares the name, type, unit, help text and labels of one metric family. Services
//! register their metrics only through [`Metric::register`], or through [`register_collector`]
//! for a custom collector built from [`Metric::build`]; the workspace `clippy.toml` disallows
//! every other way to register a metric. Metric families that a library generates are declared as
//! a [`MetricGroup`] by their name prefix.
//!
//! The `/metrics` documentation page is generated from this catalog by [`render_docs_page`].

use std::marker::PhantomData;

use prometheus::core::Collector;
use prometheus::{
    Counter, CounterVec, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec, IntCounter,
    IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
};

use crate::metrics::{DEFAULT_COUNT_BUCKETS, DEFAULT_SIZE_BUCKETS, DEFAULT_TIME_BUCKETS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
}

impl MetricType {
    pub fn as_str(self) -> &'static str {
        match self {
            MetricType::Counter => "counter",
            MetricType::Gauge => "gauge",
            MetricType::Histogram => "histogram",
        }
    }
}

/// The unit of the values of a metric. A metric with a unit ends in the matching suffix,
/// followed by `_total` for a counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Seconds,
    Bytes,
    /// Bytes held for a number of seconds, the product of the two base units.
    ByteSeconds,
    /// A count, or a quantity that has no Prometheus base unit.
    None,
}

impl Unit {
    pub const ALL: &[Unit] = &[Unit::Seconds, Unit::Bytes, Unit::ByteSeconds, Unit::None];

    pub fn suffix(self) -> Option<&'static str> {
        match self {
            Unit::Seconds => Some("_seconds"),
            Unit::Bytes => Some("_bytes"),
            Unit::ByteSeconds => Some("_byte_seconds"),
            Unit::None => None,
        }
    }
}

/// The declaration of one metric family.
#[derive(Debug)]
pub struct MetricDef {
    pub name: &'static str,
    pub help: &'static str,
    pub metric_type: MetricType,
    pub unit: Unit,
    pub labels: &'static [&'static str],
    /// The bucket bounds of a histogram; empty for the other types.
    pub buckets: &'static [f64],
}

/// A Prometheus metric type that a catalog entry can build.
pub trait MetricKind: Collector + Clone + Sized + 'static {
    const TYPE: MetricType;
    /// Whether the metric has labels (the `*Vec` types).
    const LABELLED: bool;

    fn build(def: &MetricDef) -> prometheus::Result<Self>;
}

macro_rules! metric_kind {
    ($ty:ty, $metric_type:ident, $labelled:literal, |$def:ident| $build:expr) => {
        impl MetricKind for $ty {
            const TYPE: MetricType = MetricType::$metric_type;
            const LABELLED: bool = $labelled;

            fn build($def: &MetricDef) -> prometheus::Result<Self> {
                $build
            }
        }
    };
}

fn opts(def: &MetricDef) -> Opts {
    Opts::new(def.name, def.help)
}

fn histogram_opts(def: &MetricDef) -> HistogramOpts {
    HistogramOpts::new(def.name, def.help).buckets(def.buckets.to_vec())
}

metric_kind!(Counter, Counter, false, |def| Counter::with_opts(opts(def)));
metric_kind!(IntCounter, Counter, false, |def| IntCounter::with_opts(
    opts(def)
));
metric_kind!(CounterVec, Counter, true, |def| CounterVec::new(
    opts(def),
    def.labels
));
metric_kind!(IntCounterVec, Counter, true, |def| IntCounterVec::new(
    opts(def),
    def.labels
));
metric_kind!(Gauge, Gauge, false, |def| Gauge::with_opts(opts(def)));
metric_kind!(IntGauge, Gauge, false, |def| IntGauge::with_opts(opts(def)));
metric_kind!(GaugeVec, Gauge, true, |def| GaugeVec::new(
    opts(def),
    def.labels
));
metric_kind!(IntGaugeVec, Gauge, true, |def| IntGaugeVec::new(
    opts(def),
    def.labels
));
metric_kind!(Histogram, Histogram, false, |def| Histogram::with_opts(
    histogram_opts(def)
));
metric_kind!(HistogramVec, Histogram, true, |def| HistogramVec::new(
    histogram_opts(def),
    def.labels
));

/// A catalog entry whose Prometheus type is `M`.
pub struct Metric<M> {
    pub def: MetricDef,
    kind: PhantomData<fn() -> M>,
}

impl<M: MetricKind> Metric<M> {
    const fn new(
        name: &'static str,
        unit: Unit,
        labels: &'static [&'static str],
        help: &'static str,
    ) -> Self {
        assert!(
            M::LABELLED == !labels.is_empty(),
            "a labelled metric type needs labels, an unlabelled one has none"
        );
        assert!(
            !matches!(M::TYPE, MetricType::Histogram),
            "a histogram is declared with Metric::histogram"
        );
        Self {
            def: MetricDef {
                name,
                help,
                metric_type: M::TYPE,
                unit,
                labels,
                buckets: &[],
            },
            kind: PhantomData,
        }
    }

    const fn histogram(
        name: &'static str,
        unit: Unit,
        labels: &'static [&'static str],
        buckets: &'static [f64],
        help: &'static str,
    ) -> Self {
        assert!(
            M::LABELLED == !labels.is_empty(),
            "a labelled metric type needs labels, an unlabelled one has none"
        );
        assert!(
            matches!(M::TYPE, MetricType::Histogram),
            "Metric::histogram declares only histograms"
        );
        assert!(!buckets.is_empty(), "a histogram needs buckets");
        Self {
            def: MetricDef {
                name,
                help,
                metric_type: M::TYPE,
                unit,
                labels,
                buckets,
            },
            kind: PhantomData,
        }
    }

    /// Builds the metric without registering it, for a custom collector that
    /// [`register_collector`] registers.
    pub fn build(&'static self) -> M {
        M::build(&self.def).unwrap_or_else(|err| panic!("cannot build {}: {err}", self.def.name))
    }

    /// Registers the metric in the default Prometheus registry. Panics when a metric of the same
    /// name is already registered, so call it once, from a static.
    pub fn register(&'static self) -> M {
        let metric = self.build();
        register_collector(prometheus::default_registry(), Box::new(metric.clone()))
            .unwrap_or_else(|err| panic!("cannot register {}: {err}", self.def.name));
        metric
    }
}

/// Registers a collector in `registry`. Panics when the collector exports a metric family that
/// the catalog does not declare, or one whose labels or type differ from its catalog entry, so a
/// custom collector cannot export an undeclared family.
#[allow(clippy::disallowed_methods)]
pub fn register_collector(
    registry: &Registry,
    collector: Box<dyn Collector>,
) -> prometheus::Result<()> {
    if let Err(err) = check_collector(collector.as_ref()) {
        panic!("{err}");
    }
    registry.register(collector)
}

/// Checks every family of `collector` against its catalog entry: the name, the label names and
/// the type. A generated group is not accepted; only the catalog registers those collectors.
fn check_collector(collector: &dyn Collector) -> Result<(), String> {
    for desc in collector.desc() {
        let name = &desc.fq_name;
        let def = find(name)
            .ok_or_else(|| format!("{name} is not declared in golem_common::metrics::catalog"))?;
        if !desc.variable_labels.iter().eq(def.labels.iter()) {
            return Err(format!(
                "{name} has the labels {:?}, the catalog declares {:?}",
                desc.variable_labels, def.labels
            ));
        }
    }
    for family in collector.collect() {
        let name = family.name();
        let def = find(name)
            .ok_or_else(|| format!("{name} is not declared in golem_common::metrics::catalog"))?;
        let metric_type = match family.get_field_type() {
            prometheus::proto::MetricType::COUNTER => MetricType::Counter,
            prometheus::proto::MetricType::GAUGE => MetricType::Gauge,
            prometheus::proto::MetricType::HISTOGRAM => MetricType::Histogram,
            other => {
                return Err(format!(
                    "{name} is a {other:?}, which the catalog cannot declare"
                ));
            }
        };
        if metric_type != def.metric_type {
            return Err(format!(
                "{name} is a {}, the catalog declares a {}",
                metric_type.as_str(),
                def.metric_type.as_str()
            ));
        }
    }
    Ok(())
}

/// Replaces the process metrics that the Prometheus client registers in its default registry,
/// named `process_*`, with the same metrics under the `golem_process_` prefix of the catalog.
/// Call it once per process, from the owner of the process.
#[cfg(target_os = "linux")]
#[allow(clippy::disallowed_methods)]
pub fn register_process_metrics(registry: &Registry) {
    use prometheus::process_collector::ProcessCollector;
    let _ = registry.unregister(Box::new(ProcessCollector::for_self()));
    let namespace = PROCESS.prefix.trim_end_matches("_process_");
    let collector = ProcessCollector::new(std::process::id() as i32, namespace);
    debug_assert!(
        collector
            .desc()
            .iter()
            .all(|desc| desc.fq_name.starts_with(PROCESS.prefix))
    );
    if let Err(err) = registry.register(Box::new(collector)) {
        tracing::warn!("Failed to register the process metrics: {err}");
    }
}

/// The process metrics of the Prometheus client exist only on Linux.
#[cfg(not(target_os = "linux"))]
pub fn register_process_metrics(_registry: &Registry) {}

/// Metric families that a library generates, declared by the prefix that Golem gives their names.
#[derive(Debug)]
pub struct MetricGroup {
    pub prefix: &'static str,
    pub help: &'static str,
}

/// One heading of the catalog and of the documentation page.
pub struct Section {
    pub title: &'static str,
    pub intro: &'static str,
    pub metrics: &'static [&'static MetricDef],
    pub groups: &'static [&'static MetricGroup],
}

macro_rules! catalog {
    (
        $(
            section $title:literal $(intro $intro:literal)? {
                $( metric $id:ident: $ty:ty = $value:expr; )*
                $( group $group:ident = $group_value:expr; )*
            }
        )*
    ) => {
        $(
            $( pub static $id: Metric<$ty> = $value; )*
            $( pub static $group: MetricGroup = $group_value; )*
        )*

        pub static SECTIONS: &[Section] = &[
            $(
                Section {
                    title: $title,
                    intro: concat!("" $(, $intro)?),
                    metrics: &[$(&$id.def),*],
                    groups: &[$(&$group),*],
                },
            )*
        ];
    };
}

/// Every metric family in the catalog.
pub fn metrics() -> impl Iterator<Item = &'static MetricDef> {
    SECTIONS
        .iter()
        .flat_map(|section| section.metrics.iter().copied())
}

/// Every generated group in the catalog.
pub fn groups() -> impl Iterator<Item = &'static MetricGroup> {
    SECTIONS
        .iter()
        .flat_map(|section| section.groups.iter().copied())
}

/// The catalog entry of a metric family name.
pub fn find(name: &str) -> Option<&'static MetricDef> {
    metrics().find(|def| def.name == name)
}

/// The generated group whose prefix a metric family name starts with.
pub fn find_group(name: &str) -> Option<&'static MetricGroup> {
    groups().find(|group| name.starts_with(group.prefix))
}

/// Fuel buckets for the fuel an invocation consumes.
pub const FUEL_BUCKETS: &[f64] = &[
    1000.0, 10000.0, 25000.0, 50000.0, 100000.0, 250000.0, 500000.0, 1000000.0, 2500000.0,
    5000000.0, 10000000.0,
];

/// Byte-size buckets for scheduled-action payloads and promise completion data: powers of 2 from
/// 1 KiB to 64 MiB.
pub const BLOB_SIZE_BUCKETS: &[f64] = &[
    1_024.0,
    2_048.0,
    4_096.0,
    8_192.0,
    16_384.0,
    32_768.0,
    65_536.0,
    131_072.0,
    262_144.0,
    524_288.0,
    1_048_576.0,
    2_097_152.0,
    4_194_304.0,
    8_388_608.0,
    16_777_216.0,
    33_554_432.0,
    67_108_864.0,
];

/// Lag buckets for the scheduler. Values are dense at low latencies so Grafana quantiles
/// distinguish normal scheduling delay from missed ticks.
pub const SCHEDULER_LAG_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
    10.0, 30.0, 60.0, 300.0, 600.0,
];

/// Admission-wait buckets. A worker normally clears admission in milliseconds, but under memory
/// pressure a single phase can block for minutes, so the tail reaches well past the scheduler
/// buckets.
pub const ADMISSION_WAIT_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0,
    1800.0, 3600.0,
];

/// Tick-duration buckets for the scheduler. Values are dense at low latencies because slow ticks
/// directly add scheduling delay.
pub const SCHEDULER_TICK_DURATION_BUCKETS: &[f64] = &[
    0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.3, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0,
    2.5, 3.0, 5.0, 10.0, 15.0, 20.0, 30.0, 60.0,
];

/// Tick-duration buckets for the oplog sweep. They reach past the shared set's one minute, so
/// ticks against a slow store stay measurable.
pub const OPLOG_SWEEP_TICK_BUCKETS: &[f64] =
    &[0.01, 0.1, 1.0, 5.0, 15.0, 60.0, 300.0, 900.0, 3600.0];

/// Buckets for the deletes and the prunes of filesystem snapshots. A prune that repacks a large
/// repository takes minutes, past the shared set's one minute.
pub const SNAPSHOT_STORE_BUCKETS: &[f64] = &[
    0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
];

/// Buckets for the age of a restored filesystem snapshot: from one minute to 30 days.
pub const SNAPSHOT_AGE_BUCKETS: &[f64] = &[
    60.0, 300.0, 900.0, 1800.0, 3600.0, 7200.0, 21600.0, 43200.0, 86400.0, 259200.0, 604800.0,
    1209600.0, 2592000.0,
];

/// Buckets for the size of a single `memory.grow` allocation. Deliberately fine-grained in the
/// 1-32 MiB band where typical guest grows cluster, so that p90/p99 quantiles are not pinned to a
/// coarse 4-16 MiB bucket edge.
pub const MEMORY_SIZE_BUCKETS: &[f64] = &[
    65536.0,      // 64 KiB
    262144.0,     // 256 KiB
    1048576.0,    // 1 MiB
    2097152.0,    // 2 MiB
    4194304.0,    // 4 MiB
    6291456.0,    // 6 MiB
    8388608.0,    // 8 MiB
    12582912.0,   // 12 MiB
    16777216.0,   // 16 MiB
    25165824.0,   // 24 MiB
    33554432.0,   // 32 MiB
    67108864.0,   // 64 MiB
    134217728.0,  // 128 MiB
    268435456.0,  // 256 MiB
    536870912.0,  // 512 MiB
    1073741824.0, // 1 GiB
];

/// Event-count buckets of the durable stream histograms.
pub const EVENT_COUNT_BUCKETS: &[f64] = &[
    0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 512.0, 1_024.0, 4_096.0, 16_384.0,
];

/// Buckets of the accepted durable stream attachment epochs.
pub const EPOCH_BUCKETS: &[f64] = &[
    0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 256.0, 1_024.0, 65_536.0,
];

/// Buckets of the remaining durable stream attachment lease time.
pub const LEASE_REMAINING_BUCKETS: &[f64] =
    &[0.0, 1.0, 5.0, 10.0, 20.0, 30.0, 45.0, 60.0, 120.0, 300.0];

/// Buckets of the agent resource release times.
pub const RESOURCE_RELEASE_BUCKETS: &[f64] = &[
    0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
];

/// The Prometheus client's default buckets.
pub const PROMETHEUS_DEFAULT_BUCKETS: &[f64] = prometheus::DEFAULT_BUCKETS;

use Unit::{ByteSeconds, Bytes, None as NoUnit, Seconds};

catalog! {
    section "Build" intro "Every service exports its version, so a query can join any metric with \
        the version of the process that exported it." {
        metric BUILD_INFO: IntGaugeVec = Metric::new(
            "golem_build_info", NoUnit, &["version"],
            "Always 1; the version label holds the version of the service");
        metric EXECUTOR_BUILD_INFO: IntGaugeVec = Metric::new(
            "golem_executor_build_info", NoUnit, &["version", "wasmtime"],
            "Always 1; the labels hold the version of the worker executor and of its Wasmtime");
    }

    section "HTTP and gRPC APIs" {
        metric HTTP_REQUESTS_TOTAL: IntCounterVec = Metric::new(
            "golem_http_requests_total", NoUnit, &["method", "route", "status"],
            "Requests served by the worker service gateway and MCP server, and by the HTTP \
             interface of the single executable. The handlers of the management APIs record \
             golem_api_success_seconds and golem_api_failure_seconds instead. `route` is the \
             matched route pattern of a Golem API, or `other` for any other request, such as a \
             request of a custom HTTP API; `method` is `other` for a method that HTTP does not \
             define");
        metric HTTP_REQUEST_DURATION_SECONDS: HistogramVec = Metric::histogram(
            "golem_http_request_duration_seconds", Seconds, &["method", "route", "status"],
            DEFAULT_TIME_BUCKETS,
            "Time of serving a request on an HTTP interface, with the labels of \
             golem_http_requests_total");
        metric API_SUCCESS_SECONDS: HistogramVec = Metric::histogram(
            "golem_api_success_seconds", Seconds, &["api", "api_type"], DEFAULT_TIME_BUCKETS,
            "Time of successfully serving an API request (HTTP and gRPC)");
        metric API_FAILURE_SECONDS: HistogramVec = Metric::histogram(
            "golem_api_failure_seconds", Seconds, &["api", "api_type", "error"],
            DEFAULT_TIME_BUCKETS,
            "Time of serving a failed API request (HTTP and gRPC). `error` is `Drop` when the \
             caller went away before the request produced a result");
        metric GRPC_API_ACTIVE_STREAMS: Gauge = Metric::new(
            "golem_grpc_api_active_streams", NoUnit, &[],
            "Open incoming gRPC streams");
        metric HTTP_API_ACTIVE_STREAMS: IntGaugeVec = Metric::new(
            "golem_http_api_active_streams", NoUnit, &["kind"],
            "Open long-lived HTTP streams of the worker service. `kind` is `connect` (agent \
             connect WebSockets), `invocation_session` (invocation session WebSockets) or \
             `durable_stream_sse` (durable stream reads with `live=sse`)");
        metric INTERNAL_GRPC_SUCCESS_SECONDS: HistogramVec = Metric::histogram(
            "golem_internal_grpc_success_seconds", Seconds, &["target", "op"],
            DEFAULT_TIME_BUCKETS,
            "Time of a successful internal gRPC call between Golem services");
        metric INTERNAL_GRPC_FAILURE_TOTAL: CounterVec = Metric::new(
            "golem_internal_grpc_failure_total", NoUnit, &["target", "op", "status_code"],
            "Internal gRPC calls that failed without a retry or after the retries ran out");
        metric INTERNAL_GRPC_RETRY_TOTAL: CounterVec = Metric::new(
            "golem_internal_grpc_retry_total", NoUnit, &["target", "op"],
            "Internal gRPC call attempts retried after a reconnectable error");
    }

    section "External calls" {
        metric EXTERNAL_CALL_SUCCESS_SECONDS: HistogramVec = Metric::histogram(
            "golem_external_call_success_seconds", Seconds, &["target", "op"],
            DEFAULT_TIME_BUCKETS,
            "Time of a successful outgoing call");
        metric EXTERNAL_CALL_RESPONSE_SIZE_BYTES: HistogramVec = Metric::histogram(
            "golem_external_call_response_size_bytes", Bytes, &["target", "op"],
            DEFAULT_SIZE_BUCKETS,
            "Size of the response of an outgoing call");
        metric EXTERNAL_CALL_RETRY_TOTAL: CounterVec = Metric::new(
            "golem_external_call_retry_total", NoUnit, &["target", "op"],
            "Failed outgoing calls that got retried");
        metric EXTERNAL_CALL_FAILURE_TOTAL: CounterVec = Metric::new(
            "golem_external_call_failure_total", NoUnit, &["target", "op"],
            "Failed outgoing calls that are not retried");
    }

    section "Redis" {
        metric REDIS_SUCCESS_SECONDS: HistogramVec = Metric::histogram(
            "golem_redis_success_seconds", Seconds, &["svc", "api", "cmd"], DEFAULT_TIME_BUCKETS,
            "Time of a successful Redis call");
        metric REDIS_FAILURE_TOTAL: CounterVec = Metric::new(
            "golem_redis_failure_total", NoUnit, &["svc", "api", "cmd"],
            "Failed Redis calls");
        metric REDIS_SERIALIZED_SIZE_BYTES: HistogramVec = Metric::histogram(
            "golem_redis_serialized_size_bytes", Bytes, &["svc", "entity"], DEFAULT_SIZE_BUCKETS,
            "Size of a serialized Redis entity");
        metric REDIS_DESERIALIZED_SIZE_BYTES: HistogramVec = Metric::histogram(
            "golem_redis_deserialized_size_bytes", Bytes, &["svc", "entity"],
            DEFAULT_SIZE_BUCKETS,
            "Size of a deserialized Redis entity");
    }

    section "Databases" {
        metric DB_SUCCESS_SECONDS: HistogramVec = Metric::histogram(
            "golem_db_success_seconds", Seconds, &["db_type", "svc", "api"], DEFAULT_TIME_BUCKETS,
            "Time of a successful database call");
        metric DB_FAILURE_TOTAL: CounterVec = Metric::new(
            "golem_db_failure_total", NoUnit, &["db_type", "svc", "api"],
            "Failed database calls");
        metric DB_ROLLBACK_TOTAL: CounterVec = Metric::new(
            "golem_db_rollback_total", NoUnit, &["db_type", "svc", "api"],
            "Explicit rollbacks of database transactions");
        metric DB_TRANSACTION_SECONDS: HistogramVec = Metric::histogram(
            "golem_db_transaction_seconds", Seconds, &["db_type", "svc", "api"],
            DEFAULT_TIME_BUCKETS,
            "Time of a database transaction");
        metric DB_SERIALIZED_SIZE_BYTES: HistogramVec = Metric::histogram(
            "golem_db_serialized_size_bytes", Bytes, &["db_type", "svc", "entity"],
            DEFAULT_SIZE_BUCKETS,
            "Size of a serialized database entity");
        metric DB_DESERIALIZED_SIZE_BYTES: HistogramVec = Metric::histogram(
            "golem_db_deserialized_size_bytes", Bytes, &["db_type", "svc", "entity"],
            DEFAULT_SIZE_BUCKETS,
            "Size of a deserialized database entity");
        metric DB_POOL_SIZE: GaugeVec = Metric::new(
            "golem_db_pool_size", NoUnit, &["db_type", "svc"],
            "Connections in the database connection pool");
        metric DB_POOL_IDLE: GaugeVec = Metric::new(
            "golem_db_pool_idle", NoUnit, &["db_type", "svc"],
            "Idle connections in the database connection pool");
        metric DB_POOL_MAX: GaugeVec = Metric::new(
            "golem_db_pool_max", NoUnit, &["db_type", "svc"],
            "Maximum number of connections in the database connection pool");
    }

    section "Caches" {
        metric CACHE_SIZE: GaugeVec = Metric::new(
            "golem_cache_size", NoUnit, &["cache"],
            "Current number of entries in the cache");
        metric CACHE_CAPACITY: GaugeVec = Metric::new(
            "golem_cache_capacity", NoUnit, &["cache"],
            "Current maximal capacity of the cache");
        metric CACHE_HIT_TOTAL: CounterVec = Metric::new(
            "golem_cache_hit_total", NoUnit, &["cache"],
            "Cache hits");
        metric CACHE_MISS_TOTAL: CounterVec = Metric::new(
            "golem_cache_miss_total", NoUnit, &["cache"],
            "Cache misses");
        metric CACHE_EVICTION_TOTAL: CounterVec = Metric::new(
            "golem_cache_eviction_total", NoUnit, &["cache", "trigger"],
            "Cache evictions");
    }

    section "Storage" {
        metric STORAGE_LOGICAL_OPERATIONS_TOTAL: IntCounterVec = Metric::new(
            "golem_storage_logical_operations_total", NoUnit,
            &["kind", "operation", "service", "api", "entity"],
            "Storage API calls, including failed calls, before backend retries");
        metric STORAGE_BYTES_WRITTEN_TOTAL: CounterVec = Metric::new(
            "golem_storage_written_bytes_total", Bytes, &["storage_type"],
            "Bytes written to storage, by storage type");
        metric STORAGE_OBJECTS_WRITTEN_TOTAL: CounterVec = Metric::new(
            "golem_storage_objects_written_total", NoUnit, &["storage_type"],
            "Objects written to storage, by storage type");
        metric STORAGE_OBJECTS_DELETED_TOTAL: CounterVec = Metric::new(
            "golem_storage_objects_deleted_total", NoUnit, &["storage_type"],
            "Objects deleted from storage, by storage type");
        metric COMPILATION_CACHE_BYTES_WRITTEN_TOTAL: Counter = Metric::new(
            "golem_compilation_cache_written_bytes_total", Bytes, &[],
            "Bytes written to the compiled component cache");
        metric COMPILATION_CACHE_OBJECTS_WRITTEN_TOTAL: Counter = Metric::new(
            "golem_compilation_cache_objects_written_total", NoUnit, &[],
            "Objects written to the compiled component cache");
    }

    section "TLS certificates" intro "Every service that serves or calls gRPC with TLS, and the \
        shard manager's etcd client, exports these metrics for the certificate in use. See \
        [TLS between the services](/next/deploy/cluster#tls-between-the-services) and \
        [Renew certificates](/next/operate/shard-manager#renew-certificates)." {
        metric TLS_CERTIFICATE_NOT_AFTER_SECONDS: GaugeVec = Metric::new(
            "golem_tls_certificate_not_after_seconds", Seconds, &["use"],
            "Unix time at which the TLS certificate in use expires. `use` is `grpc server`, \
             `<target> client` or `etcd client`");
        metric TLS_CERTIFICATE_RELOAD_FAILURES_TOTAL: IntCounterVec = Metric::new(
            "golem_tls_certificate_reload_failures_total", NoUnit, &["use"],
            "Changed TLS files that could not be used, counted once for each distinct set of \
             files");
    }

    section "Tokio runtime" intro "Every service exports the metrics of its Tokio runtime. \
        The `tokio-metrics` crate generates them; Golem replaces their `tokio_` prefix with \
        `golem_tokio_` and keeps the rest of the name. These names do not follow the unit and \
        `_total` rules of this page: for example `golem_tokio_total_busy_duration` is a counter \
        of microseconds." {
        group TOKIO_RUNTIME = MetricGroup {
            prefix: "golem_tokio_",
            help: "Tokio runtime metrics, such as `golem_tokio_workers_count`, \
                   `golem_tokio_global_queue_depth` and `golem_tokio_total_busy_duration`; see \
                   the `RuntimeMetrics` documentation of the `tokio-metrics` crate",
        };
    }

    section "Process" intro "Every service on Linux exports the process metrics of the \
        Prometheus client, with the `golem_` prefix added to their standard `process_` names." {
        group PROCESS = MetricGroup {
            prefix: "golem_process_",
            help: "CPU time, resident and virtual memory, open and maximum file descriptors, \
                   threads and start time of the process, such as \
                   `golem_process_cpu_seconds_total` and `golem_process_resident_memory_bytes`",
        };
    }

    section "Component compilation" {
        metric COMPONENT_COMPILATION_QUEUE_LENGTH: Gauge = Metric::new(
            "golem_component_compilation_queue_length", NoUnit, &[],
            "Pending compilation requests of the compilation service, excluding the active \
             request");
        metric COMPONENT_COMPILATION_UPLOAD_QUEUE_LENGTH: Gauge = Metric::new(
            "golem_component_compilation_upload_queue_length", NoUnit, &[],
            "Compiled components waiting for upload in the compilation service, excluding the \
             active upload");
        metric COMPONENT_COMPILATION_TIME_SECONDS: Histogram = Metric::histogram(
            "golem_component_compilation_time_seconds", Seconds, &[], DEFAULT_TIME_BUCKETS,
            "Time of a successful Wasmtime component build in the compilation service; excludes \
             the queue wait, the download, compiled-blob cache hits and the upload, but includes \
             filesystem-cache loads when enabled");
        metric COMPONENT_COMPILATION_ENQUEUE_TOTAL: IntCounterVec = Metric::new(
            "golem_component_compilation_enqueue_total", NoUnit, &["outcome"],
            "Cache-warming compilation requests that the registry service sent. `outcome` is \
             `accepted` (the compilation service admitted the request, not that compilation \
             finished), `rejected` (its compilation queue was full), `failed` (transport errors, \
             application errors and malformed responses) or `timeout` (the registry exhausted its \
             overall enqueue budget). These outcomes do not fail the component upload, and \
             uploads with precompilation disabled are not counted");
        metric COMPILATION_TIME_SECONDS: Histogram = Metric::histogram(
            "golem_compilation_time_seconds", Seconds, &[], DEFAULT_TIME_BUCKETS,
            "Time of a compilation of a WASM component to native code in the worker executor");
    }

    section "Shard manager" {
        metric SHARD_MANAGER_IS_LEADER: Gauge = Metric::new(
            "golem_shard_manager_is_leader", NoUnit, &[],
            "1 while this replica holds the etcd leadership lease, 0 while it stands by \
             (distributed mode only)");
        metric SHARD_MANAGER_IS_SERVING: Gauge = Metric::new(
            "golem_shard_manager_is_serving", NoUnit, &[],
            "1 while this replica serves shard manager calls: a leader whose startup has \
             completed, or a shard manager in local mode; 0 otherwise. A replica that has just \
             won the election holds the leadership but serves only after its startup");
        metric SHARD_MANAGER_LEADER_SINCE_TIMESTAMP_SECONDS: Gauge = Metric::new(
            "golem_shard_manager_leader_since_timestamp_seconds", Seconds, &[],
            "Unix time at which this replica was elected, 0 while it stands by (distributed mode \
             only)");
        metric SHARD_MANAGER_CAMPAIGN_ATTEMPT_FAILURES_TOTAL: IntCounter = Metric::new(
            "golem_shard_manager_campaign_attempt_failures_total", NoUnit, &[],
            "Attempts to reach etcd on the way to leadership that failed retriably and were \
             retried (distributed mode only)");
        metric SHARD_MANAGER_EXECUTOR_LEASE_EXPIRED_TOTAL: IntCounter = Metric::new(
            "golem_shard_manager_executor_lease_expired_total", NoUnit, &[],
            "Executor leases that lapsed without a renewal and had their shards released. Only \
             the leader counts, so sum it over all replicas. This counter also sees an executor \
             that no longer answers a scrape");
    }

    section "Worker executor: sharding" intro "The shard lease metrics take their label values \
        from a fixed set. A label value is exported at 0 before its first event, so a query \
        reads 0 instead of no data. To find a single agent, use the logs at the site of the \
        refusal." {
        metric ASSIGNED_SHARD_COUNT: Gauge = Metric::new(
            "golem_assigned_shard_count", NoUnit, &[],
            "Shards assigned to this executor");
        metric STALE_SHARD_DELIVERY_TOTAL: CounterVec = Metric::new(
            "golem_stale_shard_delivery_total", NoUnit, &["delivery"],
            "Shard deliveries dropped for naming a revision older than the last one applied; \
             `delivery` is `register`, `assign`, `revoke` or `renewal`. One is normal when a \
             push and a renewal cross; a sustained rate is not");
        metric SHARD_LEASE_EXPIRY_TIMESTAMP_SECONDS: Gauge = Metric::new(
            "golem_shard_lease_expiry_timestamp_seconds", Seconds, &[],
            "Unix time at which the shard lease of this executor lapses unless it is renewed. \
             Absent on a single-shard executor, which has no lease, and before the executor \
             registers. `golem_shard_lease_expiry_timestamp_seconds - time()` is the remaining \
             lease time, and it keeps counting down when the renewal loop stops");
        metric SHARD_LEASE_RENEWALS_TOTAL: IntCounterVec = Metric::new(
            "golem_shard_lease_renewals_total", NoUnit, &["outcome"],
            "Shard lease renewal attempts; `outcome` is `granted`, `not_registered`, `timeout`, \
             `lease_not_found` or `failed`. On `lease_not_found` the executor drops its shards \
             and registers again");
        metric SHARD_ADMISSION_REFUSALS_TOTAL: IntCounterVec = Metric::new(
            "golem_shard_admission_refusals_total", NoUnit, &["reason"],
            "Agent admissions refused by the shard assignment; `reason` is `not_registered`, \
             `lease_lapsed` (the executor fenced itself) or `not_owned` (the agent is on another \
             executor's shard, which is normal during a rebalance)");
    }

    section "Worker executor: agent resource release" intro "Process-local observations of how \
        long an executor takes to release the resources of an agent after it accepts a stop or \
        unloads the agent." {
        metric AGENT_RESOURCE_RELEASE_SECONDS: HistogramVec = Metric::histogram(
            "golem_agent_resource_release_seconds", Seconds,
            &["origin", "stage", "cause", "outcome"], RESOURCE_RELEASE_BUCKETS,
            "Elapsed seconds from accepted stop or ordinary unload to physical owner facts");
        metric AGENT_STOP_HOST_OPERATION_EXIT_SECONDS: HistogramVec = Metric::histogram(
            "golem_agent_stop_host_operation_exit_seconds", Seconds, &["operation", "outcome"],
            RESOURCE_RELEASE_BUCKETS,
            "Elapsed seconds from accepted stop to actual tracked driver exit");
        metric AGENT_FILESYSTEM_LIFECYCLE_SECONDS: HistogramVec = Metric::histogram(
            "golem_agent_filesystem_lifecycle_seconds", Seconds, &["operation", "outcome"],
            RESOURCE_RELEASE_BUCKETS,
            "Elapsed seconds from owning filesystem deletion acceptance through verified \
             deletion, including repair");
        metric AGENT_RESOURCE_CLEANUP_FAILURES_TOTAL: CounterVec = Metric::new(
            "golem_agent_resource_cleanup_failures_total", NoUnit, &["stage", "reason"],
            "Distinct bounded failure facts per logical cleanup obligation");
        metric AGENT_RESOURCE_RELEASE_UNFINISHED: GaugeVec = Metric::new(
            "golem_agent_resource_release_unfinished", NoUnit,
            &["origin", "stage", "cause", "health"],
            "Outstanding applicable release receipts; end_to_end counts episodes");
        metric AGENT_RESOURCE_RELEASE_OLDEST_UNFINISHED_AGE_SECONDS: GaugeVec = Metric::new(
            "golem_agent_resource_release_oldest_unfinished_age_seconds", Seconds,
            &["origin", "stage", "cause", "health"],
            "Oldest process-local unfinished release age in seconds");
    }

    section "Worker executor: agents" {
        metric WORKER_EXECUTOR_CALL_TOTAL: CounterVec = Metric::new(
            "golem_worker_executor_call_total", NoUnit, &["api"],
            "Calls to the worker layer");
        metric WORKER_COUNT_BY_STATUS: GaugeVec = Metric::new(
            "golem_worker_count_by_status", NoUnit, &["status"],
            "In-memory workers per status");
        metric WORKER_EVICTION_TOTAL: CounterVec = Metric::new(
            "golem_worker_eviction_total", NoUnit, &["class"],
            "Workers evicted from memory");
        metric WORKER_MEMORY_RESIDENT_COUNT: GaugeVec = Metric::new(
            "golem_worker_memory_resident_count", NoUnit, &["executor_id"],
            "Workers currently holding a memory permit and running an invocation loop on this \
             executor");
        metric WORKER_WAITING_FOR_MEMORY_COUNT: GaugeVec = Metric::new(
            "golem_worker_waiting_for_memory_count", NoUnit, &["executor_id"],
            "Workers blocked waiting to acquire a memory permit on this executor");
        metric WORKER_ADMISSION_WAIT_SECONDS: HistogramVec = Metric::histogram(
            "golem_worker_admission_wait_seconds", Seconds, &["executor_id", "phase"],
            ADMISSION_WAIT_BUCKETS,
            "Time a starting worker spent blocked in one phase of admission before it could \
             become resident, labelled by phase (resolve_component_charge, concurrency_slot, \
             memory). Observed once per phase per worker start. Sum across phases for the total \
             wait; read per phase to see which resource is the constraint. \
             golem_worker_waiting_for_memory_count says how many workers are waiting, this says \
             for how long");
        metric WORKER_STORE_ALIVE_COUNT: GaugeVec = Metric::new(
            "golem_worker_store_alive_count", NoUnit, &["executor_id"],
            "Live wasmtime Store contexts on this executor, counted by the Store's own lifetime: \
             incremented when a worker's Store is constructed and decremented when it is dropped. \
             Diverging above the resident-worker count means Stores are retained after the owning \
             worker was deleted");
        metric OWNER_GROUP_ALIVE_COUNT: GaugeVec = Metric::new(
            "golem_owner_group_alive_count", NoUnit, &["executor_id"],
            "Owner-routed active agent groups on this executor");
        metric PRIMARY_STORE_ALIVE_COUNT: GaugeVec = Metric::new(
            "golem_primary_store_alive_count", NoUnit, &["executor_id"],
            "Live primary agent Stores on this executor");
        metric ENTITY_STORE_ALIVE_COUNT: GaugeVec = Metric::new(
            "golem_entity_store_alive_count", NoUnit, &["executor_id", "entity_kind"],
            "Live transient entity Stores on this executor");
        metric ENTITY_INVOCATION_ACTIVE_COUNT: GaugeVec = Metric::new(
            "golem_entity_invocation_active_count", NoUnit, &["entity_kind", "execution_mode"],
            "Entity invocation bodies currently active on this executor");
        metric ENTITY_INVOCATION_TOTAL: CounterVec = Metric::new(
            "golem_entity_invocation_total", NoUnit,
            &["entity_kind", "execution_mode", "outcome"],
            "Entity invocation bodies by terminal outcome");
        metric WORKER_KV_CACHE_VALUE_SIZE_BYTES: HistogramVec = Metric::histogram(
            "golem_worker_kv_cache_value_size_bytes", Bytes, &["executor_id"],
            BLOB_SIZE_BUCKETS,
            "Bytes of a value written to the Worker-namespace KV cache (worker status blob \
             size)");
        metric WORKER_MEMORY_POOL_TOTAL_BYTES: GaugeVec = Metric::new(
            "golem_worker_memory_pool_total_bytes", Bytes, &["executor_id"],
            "Usable memory ceiling (usable_ratio * measured limit) the admission gate admits \
             against on this executor");
        metric WORKER_MEMORY_POOL_USED_BYTES: GaugeVec = Metric::new(
            "golem_worker_memory_pool_used_bytes", Bytes, &["executor_id"],
            "Total linear memory granted to live workers and reserved by the admission gate on \
             this executor");
        metric WORKER_ADMISSION_RSS_BYTES: GaugeVec = Metric::new(
            "golem_worker_admission_rss_bytes", Bytes, &["executor_id"],
            "Measured resident memory (probe snapshot) the admission gate last read on this \
             executor");
        metric WORKER_MEMORY_GROW_REJECTED_TOTAL: CounterVec = Metric::new(
            "golem_worker_memory_grow_rejected_total", NoUnit, &["executor_id"],
            "Invocations interrupted because a worker's linear-memory grow could not be admitted \
             by the gate (out-of-memory trap, retried via reacquire)");
        metric AGENT_STATUS_FLUSH_TOTAL: CounterVec = Metric::new(
            "golem_agent_status_flush_total", NoUnit, &["reason"],
            "Cached agent status blob flushes by reason (background/forced)");
        metric AGENT_STATUS_FLUSH_FAILED_TOTAL: CounterVec = Metric::new(
            "golem_agent_status_flush_failed_total", NoUnit, &["reason"],
            "Cached agent status blob flushes that failed, by reason (background/forced)");
        metric REPLAY_METADATA_SCAN_ENTRIES_TOTAL: CounterVec = Metric::new(
            "golem_replay_metadata_scan_entries_total", NoUnit, &["start"],
            "Oplog entries scanned for recovery metadata when a replay generation starts, by \
             the start of the replayed history (snapshot/initial)");
        metric AGENT_STATUS_RECOMPUTE_TOTAL: CounterVec = Metric::new(
            "golem_agent_status_recompute_total", NoUnit, &["source"],
            "Agent status recomputes by the baseline used (cache/checkpoint/full)");
        metric AGENT_STATUS_CHECKPOINT_WRITE_TOTAL: CounterVec = Metric::new(
            "golem_agent_status_checkpoint_write_total", NoUnit, &["reason"],
            "Clean agent status checkpoint writes by reason (snapshot/idle)");
        metric AGENT_STATUS_CHECKPOINT_WRITE_FAILED_TOTAL: CounterVec = Metric::new(
            "golem_agent_status_checkpoint_write_failed_total", NoUnit, &["reason"],
            "Clean agent status checkpoint writes that failed, by reason (snapshot/idle)");
        metric INVOCATION_RESULT_RESOLUTION_TOTAL: CounterVec = Metric::new(
            "golem_invocation_result_resolution_total", NoUnit, &["outcome"],
            "Invocation-result resolutions by the lookup path and outcome");
        metric INVOCATION_RESULT_INDEX_CATCH_UP_CHUNKS_TOTAL: Counter = Metric::new(
            "golem_invocation_result_index_catch_up_chunks_total", NoUnit, &[],
            "Physical invocation-result index catch-up chunks completed");
        metric INVOCATION_RESULT_INDEX_CATCH_UP_ENTRIES_TOTAL: Counter = Metric::new(
            "golem_invocation_result_index_catch_up_entries_total", NoUnit, &[],
            "Oplog entries processed while catching up the physical invocation-result index");
        metric INVOCATION_LISTINGS_TOTAL: Counter = Metric::new(
            "golem_invocation_listings_total", NoUnit, &[],
            "Listings of the invocations of one agent");
        metric INVOCATION_LISTING_RESULTS_TOTAL: Counter = Metric::new(
            "golem_invocation_listing_results_total", NoUnit, &[],
            "Invocation results read to list the invocations of one agent");
        metric AGENT_IDENTITY_RESOLUTION_TOTAL: CounterVec = Metric::new(
            "golem_agent_identity_resolution_total", NoUnit, &["event"],
            "Mode-hint events observed during authoritative agent identity resolution; multiple \
             events may occur per resolution");
        metric STALE_RUNNING_WORKER_TOTAL: CounterVec = Metric::new(
            "golem_stale_running_worker_total", NoUnit, &["reason"],
            "Stale recovery-index members removed by reason");
        metric STATUS_CACHE_PUBLICATION_TOTAL: CounterVec = Metric::new(
            "golem_status_cache_publication_total", NoUnit, &["cache", "event"],
            "Status cache publication events by cache and reconciliation path; fallback and \
             eventual reconciliation are separate events");
        metric DERIVED_CACHE_PUBLICATION_FAILED_TOTAL: CounterVec = Metric::new(
            "golem_derived_cache_publication_failed_total", NoUnit, &["cache"],
            "Derived-cache publications that failed while atomically applying expiry");
        metric FOREIGN_STREAM_FINGERPRINT_MISMATCH_TOTAL: Counter = Metric::new(
            "golem_foreign_stream_fingerprint_mismatch_total", NoUnit, &[],
            "Foreign durable-stream accesses rejected because the current agent fingerprint \
             differs");
    }

    section "Worker executor: invocations and the Wasm runtime" {
        metric CREATE_WORKER_SECONDS: Histogram = Metric::histogram(
            "golem_create_worker_seconds", Seconds, &[], DEFAULT_TIME_BUCKETS,
            "Time of the creation of a worker");
        metric CREATE_WORKER_FAILURE_TOTAL: CounterVec = Metric::new(
            "golem_create_worker_failure_total", NoUnit, &["error"],
            "Failed worker creations");
        metric INVOCATION_TOTAL: CounterVec = Metric::new(
            "golem_invocation_total", NoUnit, &["mode", "outcome"],
            "Invocations; `mode` is `live` or `replay`");
        metric INVOCATION_FUEL_CONSUMPTION: Histogram = Metric::histogram(
            "golem_invocation_fuel_consumption", NoUnit, &[], FUEL_BUCKETS,
            "Fuel consumed by an invocation");
        metric ALLOCATED_MEMORY_BYTES: Histogram = Metric::histogram(
            "golem_allocated_memory_bytes", Bytes, &[], MEMORY_SIZE_BUCKETS,
            "Store's accounted total of linear-memory bytes and GC heap capacity, sampled after \
             each positive memory-growth notification");
        metric WORKER_ALLOCATED_LINEAR_MEMORY_BYTES: Histogram = Metric::histogram(
            "golem_worker_allocated_linear_memory_bytes", Bytes, &[], MEMORY_SIZE_BUCKETS,
            "Per-Store allocated linear-memory bytes plus GC heap capacity, sampled after \
             instance reconciliation. Linear memories contribute current data_size values for \
             unique backings; this is not measured resident memory");
        metric HOST_FUNCTION_CALL_TOTAL: CounterVec = Metric::new(
            "golem_host_function_call_total", NoUnit, &["interface", "name"],
            "Calls to specific host functions");
        metric RESUME_WORKER_SECONDS: Histogram = Metric::histogram(
            "golem_resume_worker_seconds", Seconds, &[], DEFAULT_TIME_BUCKETS,
            "Time of the resumption of a worker");
        metric REPLAYED_FUNCTIONS: Histogram = Metric::histogram(
            "golem_replayed_functions", NoUnit, &[], DEFAULT_COUNT_BUCKETS,
            "Functions replayed per worker resumption");
        metric IN_FUNCTION_RETRY_TOTAL: Counter = Metric::new(
            "golem_in_function_retry_total", NoUnit, &[],
            "In-function retries (retries inside a host function without oplog replay)");
        metric CUSTOM_INVOCATION_SCOPE_OPEN_TOTAL: Counter = Metric::new(
            "golem_custom_invocation_scope_open_total", NoUnit, &[],
            "Live custom-durability ownership scopes opened");
        metric AGENT_PERMISSION_AUTHORIZATION_TOTAL: CounterVec = Metric::new(
            "golem_agent_permission_authorization_total", NoUnit,
            &["permission_class", "outcome"],
            "Live host-call permission authorization decisions");
        metric AGENT_PERMISSION_AUTHORITY_SYNC_TOTAL: CounterVec = Metric::new(
            "golem_agent_permission_authority_sync_total", NoUnit, &["path"],
            "Host-call authority boundary checks by path");
        metric AGENT_PERMISSION_AUTHORITY_SYNC_SECONDS: Histogram = Metric::histogram(
            "golem_agent_permission_authority_sync_seconds", Seconds, &[], DEFAULT_TIME_BUCKETS,
            "Time spent refreshing host-call permission authority on the slow path");
        metric EVENT_TOTAL: CounterVec = Metric::new(
            "golem_event_total", NoUnit, &["event"],
            "Events produced by workers");
        metric EVENT_BROADCAST_TOTAL: CounterVec = Metric::new(
            "golem_event_broadcast_total", NoUnit, &["event"],
            "Events broadcast by the executor");
        metric RDBMS_SUCCESS_SECONDS: HistogramVec = Metric::histogram(
            "golem_rdbms_success_seconds", Seconds, &["rdbms_type", "api"], DEFAULT_TIME_BUCKETS,
            "Time of a successful `golem:rdbms` call");
        metric RDBMS_FAILURE_TOTAL: CounterVec = Metric::new(
            "golem_rdbms_failure_total", NoUnit, &["rdbms_type", "api"],
            "Failed `golem:rdbms` calls");
    }

    section "Worker executor: memory" {
        metric MIMALLOC_PEAK_RSS_BYTES: GaugeVec = Metric::new(
            "golem_mimalloc_peak_rss_bytes", Bytes, &["executor_id"],
            "Peak resident set size of the executor process since it started, from getrusage");
        metric MIMALLOC_CURRENT_COMMIT_BYTES: GaugeVec = Metric::new(
            "golem_mimalloc_current_commit_bytes", Bytes, &["executor_id"],
            "Heap address space that mimalloc committed; on Linux it goes down only when \
             mimalloc unmaps memory, so it is close to a high-water mark and can exceed the \
             resident heap");
        metric MIMALLOC_PEAK_COMMIT_BYTES: GaugeVec = Metric::new(
            "golem_mimalloc_peak_commit_bytes", Bytes, &["executor_id"],
            "Peak of golem_mimalloc_current_commit_bytes since the executor started");
    }

    section "Worker executor: resources" {
        metric FUEL_BORROW_TOTAL: Counter = Metric::new(
            "golem_fuel_borrow_total", NoUnit, &[],
            "Fuel borrowed");
        metric FUEL_RETURN_TOTAL: Counter = Metric::new(
            "golem_fuel_return_total", NoUnit, &[],
            "Fuel returned");
        metric EPHEMERAL_OVERDRAFT_FUEL_TOTAL: Counter = Metric::new(
            "golem_ephemeral_overdraft_fuel_total", NoUnit, &[],
            "Ephemeral overdraft fuel consumed");
        metric STORAGE_BYTE_SECONDS_TOTAL: CounterVec = Metric::new(
            "golem_storage_byte_seconds_total", ByteSeconds, &["agent_mode"],
            "Filesystem storage byte-seconds billed, by agent mode");
        metric MEMORY_BYTE_SECONDS_TOTAL: CounterVec = Metric::new(
            "golem_memory_byte_seconds_total", ByteSeconds, &["agent_mode"],
            "Allocated linear-memory byte-seconds delivered to the registry, by agent mode. The \
             registry receives them in whole GB-seconds of 2^30 byte-seconds");
        metric RESOURCE_USAGE_BATCH_UPDATE_FAILURE_TOTAL: Counter = Metric::new(
            "golem_resource_usage_batch_update_failure_total", NoUnit, &[],
            "Resource usage batches dropped after registry update failures");
    }

    section "Worker executor: promises and the scheduler" {
        metric PROMISE_CREATED_TOTAL: Counter = Metric::new(
            "golem_promise_created_total", NoUnit, &[],
            "Promises created");
        metric PROMISE_SCHEDULED_COMPLETION_TOTAL: Counter = Metric::new(
            "golem_promise_scheduled_completion_total", NoUnit, &[],
            "Scheduled promise completions");
        metric PROMISE_COMPLETION_SECONDS: HistogramVec = Metric::histogram(
            "golem_promise_completion_seconds", Seconds, &["executor_id", "outcome"],
            DEFAULT_TIME_BUCKETS,
            "Wall time of complete_promise from call entry to return, labelled by outcome");
        metric PROMISE_PENDING_COUNT: GaugeVec = Metric::new(
            "golem_promise_pending_count", NoUnit, &["executor_id"],
            "Distinct PromiseIds in Pending state in this executor's PromiseRegistry");
        metric PROMISE_DATA_SIZE_BYTES: HistogramVec = Metric::histogram(
            "golem_promise_data_size_bytes", Bytes, &["executor_id"], BLOB_SIZE_BUCKETS,
            "Bytes of the data payload submitted to complete_promise at call time");
        metric SCHEDULED_ACTION_LAG_SECONDS: HistogramVec = Metric::histogram(
            "golem_scheduled_action_lag_seconds", Seconds, &["executor_id"],
            SCHEDULER_LAG_BUCKETS,
            "Wall-clock delay between scheduled_at and the time the action fires");
        metric SCHEDULER_QUEUE_DEPTH: GaugeVec = Metric::new(
            "golem_scheduler_queue_depth", NoUnit, &["executor_id"],
            "Matching actions to process at the start of each scheduler iteration");
        metric SCHEDULER_DUE_ACTION_BACKLOG_AFTER_TICK: GaugeVec = Metric::new(
            "golem_scheduler_due_action_backlog_after_tick", NoUnit, &["executor_id"],
            "Due scheduled actions not yet acknowledged after scheduler tick processing");
        metric SCHEDULER_TICK_DURATION_SECONDS: HistogramVec = Metric::histogram(
            "golem_scheduler_tick_duration_seconds", Seconds, &["executor_id"],
            SCHEDULER_TICK_DURATION_BUCKETS,
            "Wall time of a single scheduler iteration");
        metric SCHEDULED_ACTION_PROCESSING_SECONDS: HistogramVec = Metric::histogram(
            "golem_scheduled_action_processing_seconds", Seconds, &["executor_id", "action_kind"],
            SCHEDULER_TICK_DURATION_BUCKETS,
            "Time to process a claimed scheduled action before acknowledgement");
        metric SCHEDULED_ACTION_SIZE_BYTES: HistogramVec = Metric::histogram(
            "golem_scheduled_action_size_bytes", Bytes, &["executor_id", "action_kind"],
            BLOB_SIZE_BUCKETS,
            "Serialized blob size of a ScheduledAction at insert time");
    }

    section "Worker executor: oplog" {
        metric OPLOG_SVC_CALL_TOTAL: CounterVec = Metric::new(
            "golem_oplog_svc_call_total", NoUnit, &["api"],
            "Calls to the oplog layer");
        metric OPLOG_SCHEDULED_ARCHIVE_SECONDS: HistogramVec = Metric::histogram(
            "golem_oplog_scheduled_archive_seconds", Seconds, &["type"], DEFAULT_TIME_BUCKETS,
            "Time of a scheduled archive step of the oplog of a worker; `type` is \
             `intermediate` or `final`");
        metric OPLOG_RATE_LIMITED_TOTAL: Counter = Metric::new(
            "golem_oplog_rate_limited_total", NoUnit, &[],
            "Oplog add calls that were delayed by the rate limiter");
        metric OPLOG_SWEEP_OUTCOME_TOTAL: CounterVec = Metric::new(
            "golem_oplog_sweep_outcome_total", NoUnit, &["route", "outcome"],
            "Keys the oplog sweep examined, by what it decided about each. One outcome per key, \
             so the outcomes sum to what a tick decided");
        metric OPLOG_SWEEP_TICK_SECONDS: HistogramVec = Metric::histogram(
            "golem_oplog_sweep_tick_seconds", Seconds, &["route"], OPLOG_SWEEP_TICK_BUCKETS,
            "Time of one oplog sweep tick");
        metric OPLOG_SWEEP_TRUNCATED_TOTAL: CounterVec = Metric::new(
            "golem_oplog_sweep_truncated_total", NoUnit, &["route"],
            "Oplog sweep ticks that hit a budget or their deadline before reaching the end of \
             the namespace");
        metric OPLOG_STORAGE_RETRY_TOTAL: CounterVec = Metric::new(
            "golem_oplog_storage_retry_total", NoUnit, &["op"],
            "Oplog storage operation retries due to transient errors or indeterminate writes");
        metric OPLOG_ARCHIVE_MAINTENANCE_FAILURE_TOTAL: CounterVec = Metric::new(
            "golem_oplog_archive_maintenance_failure_total", NoUnit, &["operation"],
            "Failed background oplog archive maintenance operations");
        metric OPLOG_EPOCH_FENCE_TOTAL: CounterVec = Metric::new(
            "golem_oplog_epoch_fence_total", NoUnit, &["op", "outcome"],
            "Oplog operations checked against the shard epoch: `op` is `record` for an open \
             recording its epoch, `append` for a write and `drop_prefix` for a trim after \
             archiving, and `archive_record`, `archive_append`, `archive_drop_prefix` and \
             `archive_delete_empty` for the same on a compressed archive level or a blob level's \
             manifest; `outcome` is `accepted` or `refused`");
    }

    section "Ephemeral agents" {
        metric EPHEMERAL_PROMISE_WAITING: Gauge = Metric::new(
            "golem_ephemeral_promise_waiting", NoUnit, &[],
            "Ephemeral agents currently waiting on promises");
        metric EPHEMERAL_NON_SUSPENDING_FAILURE_TOTAL: CounterVec = Metric::new(
            "golem_ephemeral_non_suspending_failure_total", NoUnit, &["reason"],
            "Ephemeral failures that replace suspension");
        metric EPHEMERAL_INVOCATION_ATTEMPT_TOTAL: CounterVec = Metric::new(
            "golem_ephemeral_invocation_attempt_total", NoUnit, &["disposition"],
            "Ephemeral invocation attempts by freshness disposition");
        metric EPHEMERAL_KNOWN_FRESH_VALIDATION_FAILURE_TOTAL: Counter = Metric::new(
            "golem_ephemeral_known_fresh_validation_failure_total", NoUnit, &[],
            "Rejected KnownFresh invocation requests");
        metric EPHEMERAL_LOWER_OPLOG_EXISTENCE_READ_TOTAL: Counter = Metric::new(
            "golem_ephemeral_lower_oplog_existence_read_total", NoUnit, &[],
            "Lower oplog existence reads for ephemeral agents");
        metric EPHEMERAL_INACTIVE_INVOCATION_FAILURE_TOTAL: Counter = Metric::new(
            "golem_ephemeral_inactive_invocation_failure_total", NoUnit, &[],
            "Attempts to invoke or resume inactive ephemeral agents");
        metric EPHEMERAL_PHANTOM_INVOCATION_REJECTION_TOTAL: IntCounterVec = Metric::new(
            "golem_ephemeral_phantom_invocation_rejection_total", NoUnit, &["reason"],
            "Ephemeral invocation requests that the worker service rejected for an invalid \
             phantom ID, by reason");
    }

    section "Durable stream sessions" intro "For configuration, recovery, and deletion \
        diagnostics, see [Operating Durable Stream Sessions](/next/operate/durable_streams)." {
        metric DURABLE_STREAM_LIFECYCLE_TOTAL: CounterVec = Metric::new(
            "golem_durable_stream_lifecycle_total", NoUnit, &["event"],
            "Durable stream registrations and terminal transitions");
        metric DURABLE_STREAM_OPEN: IntGauge = Metric::new(
            "golem_durable_stream_open", NoUnit, &[],
            "Open durable streams owned by loaded producers on this executor");
        metric DURABLE_STREAM_PRODUCER_OPERATION_TOTAL: CounterVec = Metric::new(
            "golem_durable_stream_producer_operation_total", NoUnit, &["operation", "outcome"],
            "Durable producer operations by bounded operation and commit outcome");
        metric DURABLE_STREAM_CATCH_UP_DISTANCE_EVENTS: Histogram = Metric::histogram(
            "golem_durable_stream_catch_up_distance_events", NoUnit, &[], EVENT_COUNT_BUCKETS,
            "Committed events read before a durable reader joins the live tail");
        metric DURABLE_STREAM_LIVE_JOIN_TOTAL: CounterVec = Metric::new(
            "golem_durable_stream_live_join_total", NoUnit, &["outcome"],
            "Durable live-tail join attempts by outcome");
        metric DURABLE_STREAM_ACTIVE_READERS: IntGauge = Metric::new(
            "golem_durable_stream_active_readers", NoUnit, &[],
            "Active durable live-tail readers on this executor");
        metric DURABLE_STREAM_BACKPRESSURE_TOTAL: IntCounter = Metric::new(
            "golem_durable_stream_backpressure_total", NoUnit, &[],
            "Durable event fan-outs that waited on a full live-reader queue");
        metric DURABLE_STREAM_ATTEMPT_TOTAL: CounterVec = Metric::new(
            "golem_durable_stream_attempt_total", NoUnit, &["operation", "outcome"],
            "Durable Stream Session start, resume, and takeover attempts by outcome");
        metric DURABLE_STREAM_ATTACHMENT_EPOCH: Histogram = Metric::histogram(
            "golem_durable_stream_attachment_epoch", NoUnit, &[], EPOCH_BUCKETS,
            "Accepted durable Stream Session attachment epochs");
        metric DURABLE_STREAM_JOURNAL_LAG_EVENTS: Histogram = Metric::histogram(
            "golem_durable_stream_journal_lag_events", NoUnit, &[], EVENT_COUNT_BUCKETS,
            "Committed source events not yet recorded in the value-only consumer journal, \
             sampled at creation, terminal, and at most every 100 ms during consumption");
        metric DURABLE_STREAM_ATTACHMENT_OPERATION_TOTAL: CounterVec = Metric::new(
            "golem_durable_stream_attachment_operation_total", NoUnit, &["operation", "outcome"],
            "Cross-oplog attachment operations by bounded operation and outcome");
        metric DURABLE_STREAM_LEASE_REMAINING_SECONDS: Histogram = Metric::histogram(
            "golem_durable_stream_lease_remaining_seconds", Seconds, &[], LEASE_REMAINING_BUCKETS,
            "Remaining attachment lease time when inspected or reconciled");
        metric DURABLE_STREAM_RECONCILIATION_TOTAL: CounterVec = Metric::new(
            "golem_durable_stream_reconciliation_total", NoUnit, &["outcome"],
            "Attachment reconciliation candidates by outcome");
        metric DURABLE_STREAM_CASCADE_TOTAL: CounterVec = Metric::new(
            "golem_durable_stream_cascade_total", NoUnit, &["outcome"],
            "Producer deletion cascade dependents by durable outcome");
        metric DURABLE_STREAM_LIMIT_VIOLATION_TOTAL: CounterVec = Metric::new(
            "golem_durable_stream_limit_violation_total", NoUnit, &["limit"],
            "Rejected durable Stream Session operations by frozen limit category");
        metric DURABLE_STREAM_LOAD_REJECTION_TOTAL: IntCounterVec = Metric::new(
            "golem_durable_stream_load_rejection_total", NoUnit, &["reason"],
            "Durable stream requests that the worker service rejected by per-stream reader, \
             per-node reader, catch-up-rate, or append-rate limits; `append-rate-limit` \
             identifies append admission failures");
        metric DURABLE_STREAM_APPEND_TOTAL: IntCounterVec = Metric::new(
            "golem_durable_stream_append_total", NoUnit, &["outcome"],
            "Durable stream append requests of the worker service by outcome: `accepted`, \
             `duplicate`, `fenced` or `gap`");
    }

    section "Agent filesystems" intro "These metrics are for the local XFS disk of the agent \
        filesystems of an executor. An executor exports them at 0 from the start when it uses \
        an XFS storage mode. The project metrics are only for `ManagedXfs`. For the usage of \
        the disk, use the disk metrics of your platform, for example \
        `kubelet_volume_stats_available_bytes` on Kubernetes." {
        metric FILESYSTEM_PRESSURE_RECOVERY_SECONDS: HistogramVec = Metric::histogram(
            "golem_filesystem_pressure_recovery_seconds", Seconds, &["outcome"],
            DEFAULT_TIME_BUCKETS,
            "Time of a recovery of free space on the agent filesystem disk after a write failed \
             for space. The recovery unloads idle agents until the free space reaches the \
             target. `outcome` is `recovered`, `target_not_reached` (no idle agent was left to \
             unload), `deadline`, `observation_failed` (the free space could not be read), \
             `cleanup_failed` (the cleanup of an unloaded agent failed), `not_under_pressure` \
             (another recovery freed the space first) or `unavailable` (the disk does not \
             report its free space)");
        metric FILESYSTEM_PRESSURE_UNLOAD_ATTEMPTS_TOTAL: IntCounterVec = Metric::new(
            "golem_filesystem_pressure_unload_attempts_total", NoUnit, &["outcome"],
            "Unloads of idle agents that a recovery of free space on the agent filesystem disk \
             tried. `outcome` is `unloaded`, `ineligible` (the agent got new work or is being \
             deleted before the unload ran; the recovery goes on with the next agent), \
             `cleanup_failed` (the agent stopped, but the cleanup of its filesystem failed), \
             `task_failed` (the unload task panicked) or `deadline`");
        metric FILESYSTEM_STORAGE_REFUSALS_TOTAL: IntCounterVec = Metric::new(
            "golem_filesystem_storage_refusals_total", NoUnit, &["cause"],
            "Agent filesystem calls that failed because no space was left. `cause` is \
             `agent_quota` (the agent used up its own quota, which is normal) or \
             `physical_capacity` (the disk is full, and a recovery did not free enough)");
        metric FILESYSTEM_PROJECT_CLEANUP_FAILURES_TOTAL: IntCounterVec = Metric::new(
            "golem_filesystem_project_cleanup_failures_total", NoUnit, &["operation"],
            "Failed cleanups of managed XFS projects. The project stays reserved, and its space \
             stays used, until a later cleanup succeeds. `operation` is `delete` (the delete of \
             an agent filesystem), `drop` (the fallback cleanup of an agent filesystem that was \
             dropped without a delete) or `stale` (the cleanup of a project left over at the \
             path of a new agent filesystem)");
        metric FILESYSTEM_PROJECT_STALE_RECLAIMS_TOTAL: IntCounter = Metric::new(
            "golem_filesystem_project_stale_reclaims_total", NoUnit, &[],
            "Managed XFS projects that a new agent filesystem found left over at its path, for \
             example after a crash, and cleaned. A rate that keeps going up shows that cleanups \
             fail");
    }

    section "Filesystem snapshots" intro "For the label values and what to watch, see \
        [Filesystem Snapshots](/next/operate/filesystem_snapshots/monitoring#metrics)." {
        metric FILESYSTEM_SNAPSHOT_CAPTURE_SECONDS: HistogramVec = Metric::histogram(
            "golem_filesystem_snapshot_capture_seconds", Seconds, &["outcome"],
            DEFAULT_TIME_BUCKETS,
            "Time that a capture of an agent filesystem, or a check of its initial files on an \
             executor without filesystem snapshots, stops the file calls, by outcome");
        metric FILESYSTEM_SNAPSHOT_UPLOAD_SECONDS: HistogramVec = Metric::histogram(
            "golem_filesystem_snapshot_upload_seconds", Seconds, &["kind", "outcome"],
            DEFAULT_TIME_BUCKETS,
            "Time of an upload of a filesystem snapshot, with its retries, by kind and outcome");
        metric FILESYSTEM_SNAPSHOT_UPLOADED_BYTES_TOTAL: CounterVec = Metric::new(
            "golem_filesystem_snapshot_uploaded_bytes_total", Bytes, &["kind"],
            "Bytes of the trees of the uploaded filesystem snapshots, by kind");
        metric FILESYSTEM_SNAPSHOT_UPLOADS_IN_PROGRESS: Gauge = Metric::new(
            "golem_filesystem_snapshot_uploads_in_progress", NoUnit, &[],
            "Store attempts of uploads of filesystem snapshots that hold a slot now");
        metric FILESYSTEM_SNAPSHOT_RESTORE_SECONDS: HistogramVec = Metric::histogram(
            "golem_filesystem_snapshot_restore_seconds", Seconds, &["outcome"],
            DEFAULT_TIME_BUCKETS,
            "Time of a restore of a filesystem snapshot, with the waits for a restore slot and \
             the retries, by outcome");
        metric FILESYSTEM_SNAPSHOT_DROPPED_CONFIRMATIONS_TOTAL: CounterVec = Metric::new(
            "golem_filesystem_snapshot_dropped_confirmations_total", NoUnit, &["reason"],
            "Uploaded filesystem snapshots that no confirmation record names, by reason");
        metric FILESYSTEM_SNAPSHOT_LEAKED_CLEANUPS_TOTAL: CounterVec = Metric::new(
            "golem_filesystem_snapshot_leaked_cleanups_total", NoUnit, &["operation"],
            "Clean-ups of filesystem snapshots that left snapshots in blob storage, by \
             operation");
        metric FILESYSTEM_SNAPSHOT_CLEANUPS_PENDING: Gauge = Metric::new(
            "golem_filesystem_snapshot_cleanups_pending", NoUnit, &[],
            "Agents with pending or running clean-up work of filesystem snapshots");
        metric FILESYSTEM_SNAPSHOT_COPY_SECONDS: HistogramVec = Metric::histogram(
            "golem_filesystem_snapshot_copy_seconds", Seconds, &["outcome"], DEFAULT_TIME_BUCKETS,
            "Time of a copy of the filesystem snapshots of a fork source, by outcome");
        metric FILESYSTEM_SNAPSHOT_STORED_BYTES_TOTAL: IntCounterVec = Metric::new(
            "golem_filesystem_snapshot_stored_bytes_total", Bytes, &["source"],
            "Bytes of the repository files that the filesystem snapshot store wrote to blob \
             storage, by source");
        metric FILESYSTEM_SNAPSHOT_REMOVED_BYTES_TOTAL: IntCounterVec = Metric::new(
            "golem_filesystem_snapshot_removed_bytes_total", Bytes, &["source"],
            "Bytes that the filesystem snapshot store deleted from blob storage, by source");
        metric FILESYSTEM_SNAPSHOT_RELEASED_BYTES_TOTAL: IntCounter = Metric::new(
            "golem_filesystem_snapshot_released_bytes_total", Bytes, &[],
            "Packed bytes of the deleted filesystem snapshots, which a later prune can remove \
             when no other snapshot uses them");
        metric FILESYSTEM_SNAPSHOT_RESTORED_AGE_SECONDS: Histogram = Metric::histogram(
            "golem_filesystem_snapshot_restored_age_seconds", Seconds, &[], SNAPSHOT_AGE_BUCKETS,
            "Time from the save of a filesystem snapshot to its successful restore");
        metric FILESYSTEM_SNAPSHOT_SKIPPED_TOTAL: IntCounterVec = Metric::new(
            "golem_filesystem_snapshot_skipped_total", NoUnit, &["kind", "reason"],
            "Filesystem snapshots that the admission skipped, by kind and reason");
        metric FILESYSTEM_SNAPSHOT_DELETE_SECONDS: HistogramVec = Metric::histogram(
            "golem_filesystem_snapshot_delete_seconds", Seconds, &["operation", "outcome"],
            SNAPSHOT_STORE_BUCKETS,
            "Time of a delete of filesystem snapshots, with its prune, by operation and \
             outcome");
        metric FILESYSTEM_SNAPSHOT_PRUNE_SECONDS: HistogramVec = Metric::histogram(
            "golem_filesystem_snapshot_prune_seconds", Seconds, &["outcome"],
            SNAPSHOT_STORE_BUCKETS,
            "Time of a prune of a filesystem snapshot repository after a delete, with its \
             check, by outcome");
        metric FILESYSTEM_SNAPSHOT_PRUNE_SKIPPED_TOTAL: IntCounterVec = Metric::new(
            "golem_filesystem_snapshot_prune_skipped_total", NoUnit, &["reason"],
            "Deletes of filesystem snapshots after which no prune ran, by reason");
        metric FILESYSTEM_SNAPSHOT_RETENTION_RUNS_TOTAL: IntCounterVec = Metric::new(
            "golem_filesystem_snapshot_retention_runs_total", NoUnit, &["outcome"],
            "Retention runs after a confirmed or committed filesystem snapshot, by outcome");
    }

    section "Worker service: HTTP sessions and OpenAPI" {
        metric HTTP_SESSION_REATTACH_ATTEMPTS_TOTAL: IntCounter = Metric::new(
            "golem_http_session_reattach_attempts_total", NoUnit, &[],
            "HTTP session reattach attempts");
        metric HTTP_SESSION_REATTACH_SECONDS: HistogramVec = Metric::histogram(
            "golem_http_session_reattach_seconds", Seconds, &["outcome"],
            PROMETHEUS_DEFAULT_BUCKETS,
            "Time of an HTTP session reattach, by outcome");
        metric HTTP_SESSION_STALE_FENCING_TOTAL: IntCounter = Metric::new(
            "golem_http_session_stale_fencing_total", NoUnit, &[],
            "Stale HTTP sessions that a newer attachment fenced");
        metric HTTP_SESSION_EXPLICIT_CANCELLATIONS_TOTAL: IntCounter = Metric::new(
            "golem_http_session_explicit_cancellations_total", NoUnit, &[],
            "HTTP sessions that ended by cancellation");
        metric HTTP_SESSION_TERMINAL_TOTAL: IntCounterVec = Metric::new(
            "golem_http_session_terminal_total", NoUnit, &["cause"],
            "HTTP sessions that ended without a completed exchange, by cause");
        metric HTTP_SESSION_CLEANUP_SECONDS: HistogramVec = Metric::histogram(
            "golem_http_session_cleanup_seconds", Seconds, &["outcome"],
            PROMETHEUS_DEFAULT_BUCKETS,
            "Time of the cleanup of an HTTP session that did not complete, by outcome");
        metric OPENAPI_CACHE_TOTAL: IntCounterVec = Metric::new(
            "golem_openapi_cache_total", NoUnit, &["outcome"],
            "OpenAPI cache lookups and stale fencing by bounded outcome");
        metric OPENAPI_GENERATION_SECONDS: HistogramVec = Metric::histogram(
            "golem_openapi_generation_seconds", Seconds, &["outcome"], PROMETHEUS_DEFAULT_BUCKETS,
            "Time of an OpenAPI generation, by success or bounded failure category");
    }
}

/// Renders the `operate/metrics` documentation page from the catalog.
pub fn render_docs_page() -> String {
    let mut page = String::from(
        "{/* This page is generated from golem-common/src/metrics/catalog.rs by \
         `cargo make generate-docs-metrics`. Do not edit it by hand. */}\n\n\
         # Metrics\n\n\
         ## Overview\n\n\
         Every Golem service exposes **Prometheus metrics** on its HTTP interface, at the \
         `/metrics` endpoint. Every metric name starts with `golem_`. A metric with a unit ends \
         with the unit of its values in base units (`_seconds`, `_bytes` or `_byte_seconds`), \
         and a counter ends with `_total`. The generated Tokio runtime metrics keep the names \
         of their library. No label holds an agent, worker, component, environment or account \
         identifier.\n\n\
         A service exports a metric only when it uses the feature that the metric measures.\n\n\
         <div className=\"metrics-reference\">\n",
    );
    for section in SECTIONS {
        page.push_str(&format!("\n## {}\n\n", section.title));
        if !section.intro.is_empty() {
            page.push_str(section.intro);
            page.push_str("\n\n");
        }
        page.push_str("| Metric | Type | Labels | Description |\n| --- | --- | --- | --- |\n");
        for def in section.metrics {
            let labels = def
                .labels
                .iter()
                .map(|label| format!("`{label}`"))
                .collect::<Vec<_>>()
                .join(", ");
            page.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                def.name,
                def.metric_type.as_str(),
                labels,
                escape_table_cell(def.help)
            ));
        }
        for group in section.groups {
            page.push_str(&format!(
                "| `{}*` | generated | | {} |\n",
                group.prefix,
                escape_table_cell(group.help)
            ));
        }
    }
    page.push_str(
        "\n## OTLP plugin\n\n\
         The [OTLP plugin](/next/develop/observability#setting-up-the-otlp-plugin) exports \
         durable stream summaries separately from the service's Prometheus endpoint:\n\n\
         | Metric | Type | Labels | Description |\n\
         | --- | --- | --- | --- |\n\
         | `golem.durable_stream.event_count` | counter | `event`, `outcome` | Durable stream \
         summary events. `event` distinguishes registration, aggregated items, stream \
         end/cancellation, and session result/finish/cancellation/expiry. Terminal summaries \
         may include `outcome` (`success` or `error`). |\n\n\
         Item counts are aggregated into the metric value. The plugin does not create one span \
         or label value per streamed item and does not export item payloads through this \
         summary.\n\n\
         `session-cancellation` counts cancellation-related journal records (request, consumer \
         intent, and application), not unique logical cancellations. One cancellation can \
         produce several records. A session result and its eventual stream or session terminal \
         are separate events; an early result is not stream completion.\n\n\
         Collector delivery is best effort even though source lifecycle state advances when a \
         batch is accepted.\n\n\
         </div>\n\n\
         For application-level observability with traces, logs, and metrics exported via \
         OpenTelemetry, see the [Observability](/next/develop/observability) page.\n",
    );
    page
}

fn escape_table_cell(text: &str) -> String {
    text.replace('|', "\\|")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use test_r::test;

    const FORBIDDEN_LABELS: &[&str] = &[
        "agent_id",
        "worker_id",
        "environment_id",
        "component_id",
        "account_id",
    ];

    /// Name parts that measure a quantity in a unit other than a Prometheus base unit.
    const NON_BASE_UNIT_WORDS: &[&str] = &[
        "ms",
        "millis",
        "milliseconds",
        "micros",
        "microseconds",
        "nanos",
        "nanoseconds",
        "minutes",
        "hours",
        "kb",
        "mb",
        "gb",
        "kib",
        "mib",
        "gib",
    ];

    /// The naming and labelling rules of one catalog entry, or `Ok` when it follows them.
    fn check_entry(def: &MetricDef) -> Result<(), String> {
        let name = def.name;
        if !name.starts_with("golem_") {
            return Err(format!("{name}: does not start with golem_"));
        }
        if !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(format!("{name}: is not lowercase snake case"));
        }
        let base = match def.metric_type {
            MetricType::Counter => name
                .strip_suffix("_total")
                .ok_or_else(|| format!("{name}: a counter ends in _total"))?,
            MetricType::Gauge | MetricType::Histogram => {
                for suffix in ["_total", "_count", "_sum", "_bucket"] {
                    if def.metric_type == MetricType::Histogram && name.ends_with(suffix) {
                        return Err(format!("{name}: a histogram name ends in {suffix}"));
                    }
                }
                if name.ends_with("_total") {
                    return Err(format!("{name}: only a counter ends in _total"));
                }
                name
            }
        };
        // The longest unit suffix that the name ends in names the unit of its values.
        let named_unit = Unit::ALL
            .iter()
            .copied()
            .filter(|unit| unit.suffix().is_some_and(|suffix| base.ends_with(suffix)))
            .max_by_key(|unit| unit.suffix().map(str::len))
            .unwrap_or(Unit::None);
        if named_unit != def.unit {
            return Err(match def.unit.suffix() {
                Some(suffix) => format!("{name}: a metric in {:?} ends in {suffix}", def.unit),
                None => format!("{name}: a metric without a unit ends in a unit suffix"),
            });
        }
        if let Some(word) = base
            .split('_')
            .find(|word| NON_BASE_UNIT_WORDS.contains(word))
        {
            return Err(format!(
                "{name}: `{word}` is not a base unit; measure in seconds or bytes"
            ));
        }
        if def.unit == Unit::None && base.split('_').any(|word| word == "duration") {
            return Err(format!("{name}: a duration is measured in seconds"));
        }
        if def.help.trim().is_empty() {
            return Err(format!("{name}: has no help text"));
        }
        let mut labels = BTreeSet::new();
        for label in def.labels {
            if FORBIDDEN_LABELS.contains(label) {
                return Err(format!("{name}: label {label} is an unbounded identifier"));
            }
            if label.starts_with("__")
                || (def.metric_type == MetricType::Histogram && *label == "le")
            {
                return Err(format!("{name}: label {label} is reserved"));
            }
            if !labels.insert(*label) {
                return Err(format!("{name}: label {label} is declared twice"));
            }
        }
        Ok(())
    }

    /// The series names of the catalog must be unique, including the `_bucket`, `_sum` and
    /// `_count` series of the histograms: two services that declare the same name would
    /// otherwise panic at their first registration, at run time.
    fn check_unique(defs: &[&MetricDef]) -> Result<(), String> {
        let mut series = BTreeSet::new();
        for def in defs {
            let names = match def.metric_type {
                MetricType::Histogram => vec![
                    def.name.to_string(),
                    format!("{}_bucket", def.name),
                    format!("{}_sum", def.name),
                    format!("{}_count", def.name),
                ],
                _ => vec![def.name.to_string()],
            };
            for name in names {
                if !series.insert(name.clone()) {
                    return Err(format!("{name}: is declared twice"));
                }
            }
        }
        for group in groups() {
            if let Some(def) = defs.iter().find(|def| def.name.starts_with(group.prefix)) {
                return Err(format!(
                    "{}: is inside the group {}",
                    def.name, group.prefix
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn every_catalog_entry_follows_the_naming_and_label_rules() {
        let errors: Vec<String> = metrics().filter_map(|def| check_entry(def).err()).collect();
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    #[test]
    fn catalog_names_are_unique() {
        let defs: Vec<_> = metrics().collect();
        check_unique(&defs).unwrap();
    }

    #[test]
    fn groups_are_prefixed() {
        for group in groups() {
            assert!(group.prefix.starts_with("golem_") && group.prefix.ends_with('_'));
        }
    }

    #[test]
    fn the_checks_reject_each_kind_of_bad_entry() {
        fn def(
            name: &'static str,
            metric_type: MetricType,
            unit: Unit,
            labels: &'static [&'static str],
        ) -> MetricDef {
            MetricDef {
                name,
                help: "help",
                metric_type,
                unit,
                labels,
                buckets: &[],
            }
        }
        let rejected = [
            def(
                "external_call_retry_total",
                MetricType::Counter,
                Unit::None,
                &[],
            ),
            def(
                "golem_external_call_retry",
                MetricType::Counter,
                Unit::None,
                &[],
            ),
            def(
                "golem_queue_length_total",
                MetricType::Gauge,
                Unit::None,
                &[],
            ),
            def(
                "golem_call_duration",
                MetricType::Histogram,
                Unit::Seconds,
                &[],
            ),
            def("golem_call_count", MetricType::Histogram, Unit::None, &[]),
            def("golem_call_seconds", MetricType::Histogram, Unit::None, &[]),
            def(
                "golem_call_duration",
                MetricType::Histogram,
                Unit::None,
                &[],
            ),
            def(
                "golem_call_duration_ms",
                MetricType::Histogram,
                Unit::None,
                &[],
            ),
            def(
                "golem_memory_gb_seconds_total",
                MetricType::Counter,
                Unit::Seconds,
                &[],
            ),
            def(
                "golem_memory_byte_seconds_total",
                MetricType::Counter,
                Unit::Seconds,
                &[],
            ),
            def(
                "golem_upload_size_total",
                MetricType::Counter,
                Unit::Bytes,
                &[],
            ),
            def(
                "golem_invocation_total",
                MetricType::Counter,
                Unit::None,
                &["agent_id"],
            ),
            def(
                "golem_invocation_total",
                MetricType::Counter,
                Unit::None,
                &["mode", "mode"],
            ),
        ];
        for bad in &rejected {
            assert!(check_entry(bad).is_err(), "{} was accepted", bad.name);
        }
        assert!(
            check_entry(&def(
                "golem_upload_bytes_total",
                MetricType::Counter,
                Unit::Bytes,
                &["kind"]
            ))
            .is_ok()
        );

        let call = def(
            "golem_call_seconds",
            MetricType::Histogram,
            Unit::Seconds,
            &[],
        );
        let duplicate = def("golem_call_seconds", MetricType::Gauge, Unit::Seconds, &[]);
        let shadow = def(
            "golem_call_seconds_count",
            MetricType::Gauge,
            Unit::None,
            &[],
        );
        assert!(check_unique(&[&call, &duplicate]).is_err());
        assert!(check_unique(&[&call, &shadow]).is_err());
        assert!(check_unique(&[&call]).is_ok());
    }

    /// Fails when the generated documentation page differs from the catalog.
    /// `UPDATE_GOLDENFILES=1` regenerates it; `cargo make generate-docs-metrics` does that.
    #[test]
    fn docs_page_matches_the_catalog() {
        use std::io::Write;

        let mut mint = goldenfile::Mint::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/src/content/next/operate"
        ));
        let mut page = mint.new_goldenfile("metrics.mdx").unwrap();
        page.write_all(render_docs_page().as_bytes()).unwrap();
    }

    #[test]
    fn the_collector_check_rejects_families_that_differ_from_the_catalog() {
        let declared = INVOCATION_TOTAL.build();
        assert!(check_collector(&declared).is_ok());

        let wrong_labels =
            CounterVec::new(Opts::new(INVOCATION_TOTAL.def.name, "help"), &["agent_id"]).unwrap();
        assert!(check_collector(&wrong_labels).is_err());

        let wrong_type = GaugeVec::new(
            Opts::new(INVOCATION_TOTAL.def.name, "help"),
            INVOCATION_TOTAL.def.labels,
        )
        .unwrap();
        assert!(check_collector(&wrong_type).is_err());

        let in_a_group = IntCounter::new("golem_process_anything_total", "help").unwrap();
        assert!(check_collector(&in_a_group).is_err());
    }

    /// A dashboard panel that queries a name the services do not export stays empty, and nothing
    /// else reports it.
    #[test]
    fn every_dashboard_metric_is_in_the_catalog() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../integration-tests/monitoring/grafana/dashboards");
        let mut problems = Vec::new();
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let dashboard: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let mut exprs = Vec::new();
            collect_exprs(&dashboard, &mut exprs);
            for expr in exprs {
                for name in metric_names(expr) {
                    checked += 1;
                    if !is_declared(&name) {
                        problems.push(format!("{}: {name} in `{expr}`", path.display()));
                    }
                }
            }
        }
        assert!(
            checked > 0,
            "no PromQL expression found in {}",
            dir.display()
        );
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    fn collect_exprs<'a>(value: &'a serde_json::Value, exprs: &mut Vec<&'a str>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    match value {
                        serde_json::Value::String(expr) if key == "expr" => exprs.push(expr),
                        _ => collect_exprs(value, exprs),
                    }
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    collect_exprs(value, exprs);
                }
            }
            _ => {}
        }
    }

    /// The metric names of a PromQL expression: the identifiers that are not a function call, a
    /// keyword, a label in a selector or grouping, or part of a string, a range or a number.
    fn metric_names(expr: &str) -> BTreeSet<String> {
        const KEYWORDS: &[&str] = &[
            "by",
            "without",
            "on",
            "ignoring",
            "group_left",
            "group_right",
            "and",
            "or",
            "unless",
            "offset",
            "bool",
        ];
        const GROUPING: &[&str] = &[
            "by",
            "without",
            "on",
            "ignoring",
            "group_left",
            "group_right",
        ];
        let chars: Vec<char> = expr.chars().collect();
        let mut names = BTreeSet::new();
        let mut i = 0;
        let mut skip_group = false;
        while i < chars.len() {
            let c = chars[i];
            let close = match c {
                '"' => Some('"'),
                '\'' => Some('\''),
                '{' => Some('}'),
                '[' => Some(']'),
                '(' if skip_group => Some(')'),
                _ => None,
            };
            if let Some(close) = close {
                skip_group = false;
                i += 1;
                while i < chars.len() && chars[i] != close {
                    i += 1;
                }
                i += 1;
                continue;
            }
            if c.is_ascii_alphabetic() || c == '_' || c == ':' {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == ':')
                {
                    i += 1;
                }
                let token: String = chars[start..i].iter().collect();
                let mut next = i;
                while next < chars.len() && chars[next].is_whitespace() {
                    next += 1;
                }
                let is_call = chars.get(next) == Some(&'(');
                if GROUPING.contains(&token.as_str()) {
                    skip_group = true;
                } else if !is_call && !KEYWORDS.contains(&token.as_str()) {
                    names.insert(token);
                }
                continue;
            }
            if c.is_ascii_digit() {
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                continue;
            }
            i += 1;
        }
        names
    }

    fn is_declared(name: &str) -> bool {
        find(name).is_some()
            || find_group(name).is_some()
            || ["_bucket", "_sum", "_count"].iter().any(|suffix| {
                name.strip_suffix(suffix)
                    .and_then(find)
                    .is_some_and(|def| def.metric_type == MetricType::Histogram)
            })
    }
}
