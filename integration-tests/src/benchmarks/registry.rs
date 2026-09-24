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

use crate::benchmarks;
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkApi, BenchmarkResult, BenchmarkSuiteItem,
};
use golem_test_framework::config::benchmark::TestMode;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use tracing::Level;

pub type BenchmarkRunFn = Box<
    dyn for<'a> Fn(
        &'a TestMode,
        Level,
        &'a BenchmarkSuiteItem,
        bool,
        bool,
    ) -> Pin<Box<dyn Future<Output = BenchmarkResult> + 'a>>,
>;

pub type BenchmarkRegistry = BTreeMap<&'static str, BenchmarkRunFn>;

pub fn benchmark_registry() -> BenchmarkRegistry {
    let mut benchmarks_by_name: BenchmarkRegistry = BTreeMap::new();
    benchmarks_by_name.insert(
        "cold-start-unknown-small",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::cold_start_unknown::ColdStartUnknownSmall,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "cold-start-unknown-medium",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::cold_start_unknown::ColdStartUnknownMedium,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "latency-small",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<benchmarks::latency::LatencySmall>(
                mode,
                verbosity,
                item,
                primary_only,
                otlp,
            ))
        }),
    );
    benchmarks_by_name.insert(
        "latency-medium",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<benchmarks::latency::LatencyMedium>(
                mode,
                verbosity,
                item,
                primary_only,
                otlp,
            ))
        }),
    );
    benchmarks_by_name.insert(
        "sleep",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<benchmarks::sleep::Sleep>(
                mode,
                verbosity,
                item,
                primary_only,
                otlp,
            ))
        }),
    );
    benchmarks_by_name.insert(
        "durability-overhead",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::durability_overhead::DurabilityOverhead,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "idempotency-key-lookup",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::idempotency_key::IdempotencyKeyLookup,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "throughput-echo",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<benchmarks::throughput::ThroughputEcho>(
                mode,
                verbosity,
                item,
                primary_only,
                otlp,
            ))
        }),
    );
    benchmarks_by_name.insert(
        "throughput-large-input",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(
                run_benchmark::<benchmarks::throughput::ThroughputLargeInput>(
                    mode,
                    verbosity,
                    item,
                    primary_only,
                    otlp,
                ),
            )
        }),
    );
    benchmarks_by_name.insert(
        "throughput-cpu-intensive",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::throughput::ThroughputCpuIntensive,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "streaming-tool",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<benchmarks::streaming::Streaming<true>>(
                mode,
                verbosity,
                item,
                primary_only,
                otlp,
            ))
        }),
    );
    benchmarks_by_name.insert(
        "streaming-rpc",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<benchmarks::streaming::Streaming<false>>(
                mode,
                verbosity,
                item,
                primary_only,
                otlp,
            ))
        }),
    );
    benchmarks_by_name
}

async fn run_benchmark<B: Benchmark>(
    mode: &TestMode,
    verbosity: Level,
    item: &BenchmarkSuiteItem,
    primary_only: bool,
    otlp: bool,
) -> BenchmarkResult {
    B::run_benchmark(mode, verbosity, item, primary_only, true, true, otlp).await
}
