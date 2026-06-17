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

use clap::Parser;
use golem_client::api::RegistryServiceClient;
use golem_common::base_model::agent::LegacyParsedAgentId;
use golem_common::model::AgentId;
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkApi, BenchmarkConfig, BenchmarkResult, BenchmarkSuite, BenchmarkSuiteItem,
    BenchmarkSuiteResult, RunMetadata,
};
use golem_test_framework::config::benchmark::{TestMode, cloud_bench_run_id};
use golem_test_framework::config::{
    BenchmarkCliParameters, BenchmarkTestDependencies, TestDependencies,
};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use integration_tests::benchmarks::{
    cleanup_account, cleanup_user_state, delete_workers, invoke_and_await_agent,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use tracing::{Level, debug, info, warn};

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
    benchmarks_by_name.insert(
        "throughput-saturation-counters",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::throughput_saturation::ThroughputSaturationCounters,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "throughput-saturation-echo-rust",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::throughput_saturation::ThroughputSaturationEchoRust,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );
    benchmarks_by_name.insert(
        "throughput-saturation-echo-ts",
        Box::new(|mode, verbosity, item, primary_only, otlp| {
            Box::pin(run_benchmark::<
                integration_tests::benchmarks::throughput_saturation::ThroughputSaturationEchoTs,
            >(mode, verbosity, item, primary_only, otlp))
        }),
    );

    let params = BenchmarkCliParameters::parse_from(std::env::args_os());
    let tracer_provider = BenchmarkTestDependencies::init_logging(&params);

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

                cloud_preflight_warmup(
                    params.benchmark_config.mode(),
                    params.service_verbosity(),
                    params.otlp,
                )
                .await;
                let mut result = f(
                    params.benchmark_config.mode(),
                    params.service_verbosity(),
                    &item,
                    params.primary_only,
                    params.otlp,
                )
                .await;
                // Attach the run_id to result metadata (cloud mode only).
                if let Some(run_id) = cloud_bench_run_id() {
                    result.run_id = Some(format!("bench-{run_id}"));
                }
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

            // Validate every benchmark name up-front so a typo exits immediately
            // without running warmup or any prior benchmark.
            for benchmark in &suite.benchmarks {
                if !benchmarks_by_name.contains_key(benchmark.name.as_str()) {
                    print_non_existing_benchmark(&mut benchmarks_by_name, &benchmark.name);
                    // print_non_existing_benchmark calls std::process::exit(1)
                    unreachable!();
                }
            }

            // Pre-flight warmup runs after all names are validated.
            cloud_preflight_warmup(
                params.benchmark_config.mode(),
                params.service_verbosity(),
                params.otlp,
            )
            .await;

            let mut suite_result = BenchmarkSuiteResult::new(&suite.name);
            for benchmark in suite.benchmarks {
                info!("Running {benchmark:?}");

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
                }
                // no else: we already validated all names above
            }

            // Attach the run_id and run_metadata to result metadata (cloud mode only).
            if let Some(run_id) = cloud_bench_run_id() {
                suite_result.run_id = Some(format!("bench-{run_id}"));

                // Read GOLEM_BENCH_* env vars set by the buildspec before invoking
                // the binary. Missing vars produce None rather than failing the run.
                let metadata = RunMetadata::from_env();
                if !metadata.is_empty() {
                    suite_result.run_metadata = Some(metadata);
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

    if let Some(provider) = tracer_provider {
        let _ = provider.shutdown();
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

// ── Pre-flight warmup constants ───────────────────────────────────────────────

/// WASM file name (without `.wasm`) of the component used for warmup
/// invocations.  Must be present in `--component-directory`.
const WARMUP_COMPONENT_WASM: &str = "benchmark_agent_rust_release";
/// Registry display name for the warmup component.
const WARMUP_COMPONENT_NAME: &str = "benchmark:agent-rust";
/// Agent type whose `echo` method is invoked during warmup.
const WARMUP_AGENT_TYPE: &str = "RustBenchmarkAgent";
/// Instance ID of the throwaway warmup agent.
const WARMUP_AGENT_INSTANCE: &str = "warmup";
/// Total wall-clock budget for the 50 warmup invocations.  If the budget
/// fires (e.g. the platform is slow to cold-start on the first invocation)
/// a warning is logged and the benchmark continues — warmup is best-effort.
const WARMUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(180);

/// Pre-flight warmup for cloud mode. Runs once at suite/benchmark start;
/// is a no-op for all non-cloud modes.
///
/// Executes 50 throwaway `invoke_and_await_agent` calls against a short-lived
/// user/env/component. Each call exercises the full stack:
/// gateway → registry-service (component lookup) → worker-service
/// → worker-executor, warming NLB target-group routing and HTTP/2 sessions at
/// every hop so they don't contaminate the first measured iteration.
///
/// The entire invocation phase is bounded by a 3-minute timeout. If the
/// timeout fires (e.g. because of a gateway routing issue on the first cold
/// start), a warning is logged and the benchmark continues — warm-up is
/// best-effort.
///
/// If uploading the warmup component fails (e.g. the file is absent from the
/// component directory), a warning is logged and the agent-invocation phase
/// is skipped; the throwaway account is still cleaned up.
async fn cloud_preflight_warmup(mode: &TestMode, verbosity: Level, otlp: bool) {
    if !matches!(mode, TestMode::Cloud { .. }) {
        return;
    }

    info!("Pre-flight warmup: creating throwaway user/env/component (50 invocations)...");

    let deps = BenchmarkTestDependencies::new(mode, verbosity, 0, false, otlp).await;

    let user = match deps.user().await {
        Ok(u) => u,
        Err(e) => {
            warn!("Pre-flight warmup: failed to create user (skipping): {e:?}");
            deps.kill_all().await;
            return;
        }
    };

    let registry_client = user.registry_service_client().await;
    let prefix = user.deps.bench_name_prefix().unwrap_or_default();

    let app = match registry_client
        .create_application(
            &user.account_id.0,
            &ApplicationCreation {
                name: ApplicationName(format!("{prefix}app-warmup")),
            },
        )
        .await
    {
        Ok(a) => a,
        Err(e) => {
            warn!("Pre-flight warmup: failed to create app (skipping): {e:?}");
            cleanup_account(&user).await;
            deps.kill_all().await;
            return;
        }
    };

    let env = match registry_client
        .create_environment(
            &app.id.0,
            &EnvironmentCreation {
                name: EnvironmentName(format!("{prefix}env-warmup")),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
        )
        .await
    {
        Ok(e) => e,
        Err(e) => {
            warn!("Pre-flight warmup: failed to create env (skipping): {e:?}");
            // delete app explicitly before account (cascading delete is incomplete)
            if let Err(del_err) = registry_client
                .delete_application(&app.id.0, app.revision.into())
                .await
            {
                warn!(
                    "Pre-flight warmup: failed to delete app {} after env-creation \
                     failure (best-effort, app may be orphaned): {del_err:?}",
                    app.id.0
                );
            }
            cleanup_account(&user).await;
            deps.kill_all().await;
            return;
        }
    };

    let component = match user
        .component(&env.id, WARMUP_COMPONENT_WASM)
        .name(WARMUP_COMPONENT_NAME)
        .store()
        .await
    {
        Ok(c) => c,
        Err(e) => {
            warn!(
                "Pre-flight warmup: failed to upload warmup component \
                 ({WARMUP_COMPONENT_WASM}.wasm) — ensure it exists in the \
                 component directory: {e:?}"
            );
            cleanup_user_state(&user, &env.id).await;
            deps.kill_all().await;
            return;
        }
    };

    let warmup_agent: LegacyParsedAgentId = agent_id!(WARMUP_AGENT_TYPE, WARMUP_AGENT_INSTANCE);

    // Bound the 50 invocations with a total wall-clock budget.
    let invoke_result = tokio::time::timeout(WARMUP_BUDGET, async {
        for i in 0..50usize {
            let result = invoke_and_await_agent(
                &user,
                &component,
                &warmup_agent,
                "echo",
                data_value!("warmup"),
            )
            .await;
            info!(
                "Pre-flight warmup invocation {}/50: {}ms",
                i + 1,
                result.accumulated_time.as_millis()
            );
        }
    })
    .await;

    if invoke_result.is_err() {
        warn!(
            "Pre-flight warmup: invocation phase timed out after {}s (continuing anyway)",
            WARMUP_BUDGET.as_secs()
        );
    }

    if let Ok(worker_id) = AgentId::from_agent_id(component.id, &warmup_agent) {
        delete_workers(&user, &[worker_id]).await;
    }
    cleanup_user_state(&user, &env.id).await;
    deps.kill_all().await;

    info!("Cloud pre-flight warmup complete.");
}
