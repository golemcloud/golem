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

mod config;
mod results;

pub use config::{BenchmarkConfig, BenchmarkSuite, BenchmarkSuiteItem, RunConfig};
pub use results::{BenchmarkResult, BenchmarkRunResult, BenchmarkSuiteResult, ResultKey};

use crate::config::benchmark::TestMode;
use async_trait::async_trait;
use itertools::Itertools;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, Instrument, Level};

#[derive(Debug, Clone)]
pub struct BenchmarkRecorder {
    state: Arc<Mutex<BenchmarkRecorderState>>,
}

impl Default for BenchmarkRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchmarkRecorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(BenchmarkRecorderState::new())),
        }
    }

    pub fn count(&self, key: &ResultKey, value: u64) {
        self.state.lock().unwrap().count(key, value);
    }

    pub fn counts(&self) -> HashMap<ResultKey, Vec<u64>> {
        self.state.lock().unwrap().counts.clone()
    }

    pub fn duration(&self, key: &ResultKey, value: Duration) {
        self.state.lock().unwrap().duration(key, value);
    }

    pub fn durations(&self) -> HashMap<ResultKey, Vec<Duration>> {
        self.state.lock().unwrap().durations.clone()
    }
}

#[derive(Debug)]
pub struct BenchmarkRecorderState {
    durations: HashMap<ResultKey, Vec<Duration>>,
    counts: HashMap<ResultKey, Vec<u64>>,
}

impl Default for BenchmarkRecorderState {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchmarkRecorderState {
    pub fn new() -> Self {
        Self {
            durations: HashMap::new(),
            counts: HashMap::new(),
        }
    }

    pub fn count(&mut self, key: &ResultKey, value: u64) {
        self.counts.entry(key.clone()).or_default().push(value);
    }

    pub fn duration(&mut self, key: &ResultKey, value: Duration) {
        self.durations.entry(key.clone()).or_default().push(value);
    }
}

#[async_trait]
pub trait Benchmark: Send + Sync + 'static {
    type BenchmarkContext: Send + Sync + 'static;
    type IterationContext: Send + Sync + 'static;

    fn name() -> &'static str;
    fn description() -> &'static str;

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> Self::BenchmarkContext;

    async fn cleanup(benchmark_context: Self::BenchmarkContext);

    async fn create(mode: &TestMode, config: RunConfig) -> Self;

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
    ) -> Self::IterationContext;

    async fn warmup(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
    );

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    );

    async fn cleanup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
    );
}

#[async_trait]
pub trait BenchmarkApi {
    async fn run_benchmark_internal(
        mode: &TestMode,
        verbosity: Level,
        item: &BenchmarkSuiteItem,
        otlp: bool,
    ) -> BenchmarkResult;

    async fn run_benchmark(
        mode: &TestMode,
        verbosity: Level,
        item: &BenchmarkSuiteItem,
        primary_only: bool,
        otlp: bool,
    ) -> BenchmarkResult {
        let mut results = Self::run_benchmark_internal(mode, verbosity, item, otlp).await;
        results.drop_zero_counts();
        results.drop_details();
        if primary_only {
            results.primary_only();
        }

        results
    }
}

async fn run_benchmark<B: Benchmark>(
    benchmark_context: &B::BenchmarkContext,
    mode: &TestMode,
    config: RunConfig,
    cluster_size: usize,
    iterations: usize,
    run_name: &str,
) -> BenchmarkRunResult {
    let span = tracing::info_span!(
        "benchmark",
        name = B::name(),
        cluster_size = cluster_size,
        size = config.size,
        length = config.length,
        run = run_name
    );
    let _enter = span.enter();
    info!("Starting benchmark iterations {}", B::name());

    let benchmark = B::create(mode, config.clone())
        .instrument(span.clone())
        .await;
    let mut aggregated_results = BenchmarkRunResult::new(config.clone());

    for iteration in 0..iterations {
        let span = tracing::info_span!(
            "benchmark",
            name = B::name(),
            run = run_name,
            iteration = iteration
        );
        let _enter = span.enter();
        info!("Starting iteration");

        let context = benchmark
            .setup_iteration(benchmark_context)
            .instrument(span.clone())
            .await;

        info!("Starting warmup");
        benchmark
            .warmup(benchmark_context, &context)
            .instrument(span.clone())
            .await;
        info!("Finished warmup");

        info!("Starting benchmark");
        let recorder = BenchmarkRecorder::new();
        benchmark
            .run(benchmark_context, &context, recorder.clone())
            .instrument(span.clone())
            .await;
        info!("Finished benchmark");

        benchmark
            .cleanup_iteration(benchmark_context, context)
            .instrument(span.clone())
            .await;
        aggregated_results.add(recorder);

        info!("Finished iteration");
    }

    aggregated_results
}

#[async_trait]
impl<B: Benchmark> BenchmarkApi for B {
    async fn run_benchmark_internal(
        mode: &TestMode,
        verbosity: Level,
        item: &BenchmarkSuiteItem,
        otlp: bool,
    ) -> BenchmarkResult {
        let span = tracing::info_span!("benchmark", name = B::name());
        let _enter = span.enter();
        info!("Initializing benchmark {}", B::name());

        let runs = item.runs(mode);

        let runs_cnt = runs.len();
        let mut current_run = 0;

        let mut results = Vec::new();

        let groups = runs
            .iter()
            .chunk_by(|r| r.cluster_size)
            .into_iter()
            .map(|(cluster_size, group)| (cluster_size, group.collect::<Vec<_>>()))
            .collect::<Vec<_>>();

        for (cluster_size, runs) in groups {
            let span =
                tracing::info_span!("benchmark", name = B::name(), cluster_size = cluster_size);
            let _enter = span.enter();

            info!("Creating benchmark context");
            let context = B::create_benchmark_context(
                mode,
                verbosity,
                cluster_size,
                item.disable_compilation_cache.unwrap_or_default(),
                otlp,
            )
            .instrument(span.clone())
            .await;

            for config in runs {
                current_run += 1;
                let run_name = format!("{current_run}/{runs_cnt}");
                results.push(
                    run_benchmark::<B>(
                        &context,
                        mode,
                        config.clone(),
                        cluster_size,
                        item.iterations,
                        &run_name,
                    )
                    .instrument(span.clone())
                    .await,
                );
            }

            info!("Stopping benchmark context");
            B::cleanup(context).instrument(span.clone()).await;
        }

        BenchmarkResult {
            name: if item.disable_compilation_cache.unwrap_or_default() {
                format!("{}-no-cache", B::name())
            } else {
                B::name().to_string()
            },
            description: B::description().to_string(),
            runs,
            results,
        }
    }
}
