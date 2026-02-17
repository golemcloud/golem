// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use clap::Parser;
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkApi, BenchmarkConfig, BenchmarkResult, BenchmarkSuite, BenchmarkSuiteItem,
    BenchmarkSuiteResult,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::{BenchmarkCliParameters, BenchmarkTestDependencies};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use tracing::{debug, info, Level};

type RunFn = Box<
    dyn for<'a> Fn(
        &'a TestMode,
        Level,
        &'a BenchmarkSuiteItem,
        bool,
        bool,
    ) -> Pin<Box<dyn Future<Output = BenchmarkResult> + 'a>>,
>;

#[tokio::main]
async fn main() {
    let file_limit_increase_result = rlimit::increase_nofile_limit(1000000);
    debug!(
        "File limit increase result: {:?}",
        file_limit_increase_result
    );

    let mut benchmarks_by_name: BTreeMap<&str, RunFn> = BTreeMap::new();
    benchmarks_by_name.insert(
        "cold-start-unknown-small",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::cold_start_unknown::ColdStartUnknownSmall,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "cold-start-unknown-medium",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::cold_start_unknown::ColdStartUnknownMedium,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "latency-small",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::latency::LatencySmall,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "latency-medium",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::latency::LatencyMedium,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "sleep",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(
                run_benchmark::<integration_tests::benchmarks::sleep::Sleep>(
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
        "durability-overhead",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::durability_overhead::DurabilityOverhead,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "throughput-echo",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::throughput::ThroughputEcho,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "throughput-large-input",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::throughput::ThroughputLargeInput,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "throughput-cpu-intensive",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::throughput::ThroughputCpuIntensive,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );

    let params = BenchmarkCliParameters::parse_from(std::env::args_os());
    BenchmarkTestDependencies::init_logging(&params);

    match &params.benchmark_config {
        BenchmarkConfig::Benchmark {
            name,
            iterations,
            cluster_size,
            size,
            length,
            disable_compilation_cache,
            ..
        } => {
            if let Some(f) = benchmarks_by_name.get(name.as_str()) {
                let item = BenchmarkSuiteItem {
                    name: name.clone(),
                    iterations: *iterations,
                    cluster_size: cluster_size.clone(),
                    size: size.clone(),
                    length: length.clone(),
                    disable_compilation_cache: Some(*disable_compilation_cache),
                };
                let result = f(
                    params.benchmark_config.mode(),
                    params.service_verbosity(),
                    &item,
                    params.primary_only,
                    params.otlp,
                )
                .await;
                if params.json {
                    let str = serde_json::to_string(&result)
                        .expect("Failed to serialize BenchmarkResult");
                    println!("{str}");
                } else {
                    println!("{}", result.view());
                }
            } else {
                print_non_existing_benchmark(&mut benchmarks_by_name, name);
            }
        }
        BenchmarkConfig::Suite {
            path,
            save_to_json,
            add_to_json,
            ..
        } => {
            info!("Reading benchmark suite from {path:?}");
            let raw_suite = std::fs::read_to_string(path).expect("Failed to read benchmark suite");
            let suite: BenchmarkSuite =
                serde_yaml::from_str(&raw_suite).expect("Failed to parse benchmark suite");

            let mut suite_result = BenchmarkSuiteResult::new(&suite.name);
            for benchmark in suite.benchmarks {
                info!("Running {benchmark:?}"); // TODO

                if let Some(f) = benchmarks_by_name.get(benchmark.name.as_str()) {
                    let result = f(
                        params.benchmark_config.mode(),
                        params.service_verbosity(),
                        &benchmark,
                        params.primary_only,
                        params.otlp,
                    )
                    .await;
                    suite_result.add(result);
                } else {
                    print_non_existing_benchmark(&mut benchmarks_by_name, &benchmark.name);
                }
            }

            if let Some(path) = save_to_json {
                suite_result
                    .save_to_json(path)
                    .expect("Failed to save JSON result file");
            }
            if let Some(path) = add_to_json {
                suite_result
                    .add_to_json(path)
                    .expect("Failed to add to JSON result file");
            }

            if params.json {
                let str = serde_json::to_string(&suite_result)
                    .expect("Failed to serialize BenchmarkSuiteResult");
                println!("{str}");
            } else {
                println!("{}", suite_result.view());
            }
        }
    }
}

fn print_non_existing_benchmark(benchmarks_by_name: &mut BTreeMap<&str, RunFn>, name: &String) {
    eprintln!("Non-existing benchmark: {name}");
    eprintln!(
        "Use one of: {}",
        benchmarks_by_name
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    std::process::exit(1);
}

async fn run_benchmark<B: Benchmark>(
    mode: &TestMode,
    verbosity: Level,
    item: &BenchmarkSuiteItem,
    primary_only: bool,
    otlp: bool,
) -> BenchmarkResult {
    B::run_benchmark(mode, verbosity, item, primary_only, otlp).await
}
