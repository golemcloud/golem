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
    benchmarks_by_name.insert(
        "streaming-rpc-history",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::streaming_history::StreamingRpcHistory,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "streaming-rpc-cold-indexed",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::streaming_history::StreamingRpcColdIndexed,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "streaming-rpc-cold-rebuild",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::streaming_history::StreamingRpcColdRebuild,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "streaming-rpc-reconnect",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::streaming_recovery::StreamingRpcReconnect,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "streaming-rpc-recovery",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::streaming_recovery::StreamingRpcRecovery,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "streaming-rpc-recovery-siblings",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::streaming_recovery::StreamingRpcRecoverySiblings,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "streaming-rpc-recovery-nested",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                benchmarks::streaming_recovery::StreamingRpcRecoveryNested,
            >(mode, verbosity, item, primary_only, otlp))
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

#[cfg(test)]
mod tests {
    use super::*;
    use golem_test_framework::benchmark::BenchmarkSuite;
    use test_r::test;

    const STREAMING_NAMES: [&str; 9] = [
        "streaming-tool",
        "streaming-rpc",
        "streaming-rpc-history",
        "streaming-rpc-cold-indexed",
        "streaming-rpc-cold-rebuild",
        "streaming-rpc-reconnect",
        "streaming-rpc-recovery",
        "streaming-rpc-recovery-siblings",
        "streaming-rpc-recovery-nested",
    ];

    fn suite(raw: &str) -> BenchmarkSuite {
        serde_yaml::from_str(raw).expect("benchmark suite must parse")
    }

    fn assert_names_resolve(suite: &BenchmarkSuite) {
        let registry = benchmark_registry();
        for item in &suite.benchmarks {
            assert!(
                registry.contains_key(item.name.as_str()),
                "unregistered benchmark {}",
                item.name
            );
        }
    }

    #[test]
    fn benchmark_registry_resolves_streaming_suites() {
        let daily = suite(include_str!("../../benchmark_suites/ci.yaml"));
        let quick = suite(include_str!("../../benchmark_suites/quick-all.yaml"));
        let smoke = suite(include_str!("../../benchmark_suites/gol-552-smoke.yaml"));

        assert_names_resolve(&daily);
        assert_names_resolve(&quick);
        assert_names_resolve(&smoke);

        for suite in [&daily, &quick, &smoke] {
            let names = suite
                .benchmarks
                .iter()
                .filter(|item| item.name.starts_with("streaming-"))
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, STREAMING_NAMES);
        }

        let expected_daily = [
            ("streaming-tool", vec![1, 10], vec![512]),
            ("streaming-rpc", vec![1, 10], vec![512]),
            ("streaming-rpc-history", vec![0, 32, 128], vec![512]),
            ("streaming-rpc-cold-indexed", vec![0, 128], vec![512]),
            ("streaming-rpc-cold-rebuild", vec![128], vec![512]),
            ("streaming-rpc-reconnect", vec![129], vec![64]),
            ("streaming-rpc-recovery", vec![0, 129], vec![64]),
            ("streaming-rpc-recovery-siblings", vec![129], vec![64]),
            ("streaming-rpc-recovery-nested", vec![129], vec![32]),
        ];
        for (name, sizes, lengths) in expected_daily {
            let item = daily
                .benchmarks
                .iter()
                .find(|item| item.name == name)
                .expect("daily streaming benchmark must exist");
            assert_eq!(item.iterations, 1);
            assert_eq!(item.cluster_size, vec![1]);
            assert_eq!(item.size, sizes);
            assert_eq!(item.length, lengths);
        }
    }
}
