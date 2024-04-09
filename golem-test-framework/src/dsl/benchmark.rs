// Copyright 2024 Golem Cloud
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

use crate::config::CliParams;
use async_trait::async_trait;
use clap::Args;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, Instrument};

#[derive(Debug, Clone, Args)]
pub struct BenchmarkConfig {
    #[arg(long, default_value = "3")]
    pub iterations: usize,
    #[arg(long, default_value = "10")]
    pub size: usize,
    #[arg(long, default_value = "100")]
    pub length: usize,
}

pub type ResultKey = String;

#[derive(Debug, Clone)]
pub struct DurationResult {
    pub avg: Duration,
    pub min: Duration,
    pub max: Duration,
    pub all: Vec<Duration>,
    pub per_iteration: Vec<Vec<Duration>>,
}

impl DurationResult {
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn add_iteration(&mut self, durations: &[Duration]) {
        self.per_iteration.push(durations.to_vec());
        self.all.extend_from_slice(durations);

        self.min = Duration::MAX;
        self.max = Duration::ZERO;
        self.avg = Duration::ZERO;

        for duration in &self.all {
            self.min = self.min.min(*duration);
            self.max = self.max.max(*duration);
            self.avg += *duration;
        }
        self.avg /= self.all.len() as u32;
    }
}

impl Default for DurationResult {
    fn default() -> Self {
        Self {
            avg: Duration::ZERO,
            min: Duration::MAX,
            max: Duration::ZERO,
            all: Vec::new(),
            per_iteration: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CountResult {
    pub avg: u64,
    pub min: u64,
    pub max: u64,
    pub all: Vec<u64>,
    pub per_iteration: Vec<Vec<u64>>,
}

impl CountResult {
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn add_iteration(&mut self, counts: &[u64]) {
        self.per_iteration.push(counts.to_vec());
        self.all.extend_from_slice(counts);

        self.min = u64::MAX;
        self.max = 0;
        self.avg = 0;

        for count in &self.all {
            self.min = self.min.min(*count);
            self.max = self.max.max(*count);
            self.avg += *count;
        }
        self.avg /= self.all.len() as u64;
    }
}

impl Default for CountResult {
    fn default() -> Self {
        Self {
            avg: 0,
            min: u64::MAX,
            max: 0,
            all: Vec::new(),
            per_iteration: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    duration_results: HashMap<ResultKey, DurationResult>,
    count_results: HashMap<ResultKey, CountResult>,
}

impl Default for BenchmarkResult {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchmarkResult {
    pub fn new() -> Self {
        Self {
            duration_results: HashMap::new(),
            count_results: HashMap::new(),
        }
    }

    pub fn add(&mut self, record: BenchmarkRecorder) {
        for (key, durations) in record.durations() {
            if durations.is_empty() {
                continue;
            }

            let results = self.duration_results.entry(key.clone()).or_default();
            results.add_iteration(&durations);
        }

        for (key, counts) in record.counts() {
            if counts.is_empty() {
                continue;
            }

            let results = self.count_results.entry(key.clone()).or_default();
            results.add_iteration(&counts);
        }
    }
}

impl Display for BenchmarkResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut all_keys = Vec::new();
        all_keys.extend(self.count_results.keys().cloned());
        all_keys.extend(self.duration_results.keys().cloned());
        all_keys.dedup();

        for key in all_keys {
            writeln!(f, "Results for '{}':", key)?;

            if let Some(duration_result) = self.duration_results.get(&key) {
                writeln!(f, "Duration:")?;
                writeln!(f, "  Avg: {:?}", duration_result.avg)?;
                writeln!(f, "  Min: {:?}", duration_result.min)?;
                writeln!(f, "  Max: {:?}", duration_result.max)?;
            }

            if let Some(count_result) = self.count_results.get(&key) {
                writeln!(f, "Count:")?;
                writeln!(f, "  Avg: {:?}", count_result.avg)?;
                writeln!(f, "  Min: {:?}", count_result.min)?;
                writeln!(f, "  Max: {:?}", count_result.max)?;
            }
        }

        Ok(())
    }
}

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
    type IterationContext: Send + Sync + 'static;

    fn name() -> &'static str;
    async fn create(params: CliParams) -> Self;

    async fn setup_iteration(&self) -> Self::IterationContext;
    async fn warmup(&self, context: &Self::IterationContext);
    async fn run(&self, context: &Self::IterationContext, recorder: BenchmarkRecorder);
    async fn cleanup_iteration(&self, context: Self::IterationContext);
}

#[async_trait]
pub trait BenchmarkApi {
    async fn run_benchmark(params: CliParams) -> BenchmarkResult;
}

#[async_trait]
impl<B: Benchmark> BenchmarkApi for B {
    async fn run_benchmark(params: CliParams) -> BenchmarkResult {
        let span = tracing::info_span!("benchmark", name = B::name());
        let _enter = span.enter();
        info!("Initializing benchmark {}", B::name());

        let benchmark = B::create(params.clone()).instrument(span.clone()).await;
        let mut aggregated_results = BenchmarkResult::new();

        for iteration in 0..params.benchmark_config.iterations {
            let span = tracing::info_span!("benchmark", name = B::name(), iteration = iteration);
            let _enter = span.enter();
            info!("Starting iteration");

            let context = benchmark.setup_iteration().instrument(span.clone()).await;

            info!("Starting warmup");
            benchmark.warmup(&context).instrument(span.clone()).await;
            info!("Finished warmup");

            info!("Starting benchmark");
            let recorder = BenchmarkRecorder::new();
            benchmark
                .run(&context, recorder.clone())
                .instrument(span.clone())
                .await;
            info!("Finished benchmark");

            benchmark
                .cleanup_iteration(context)
                .instrument(span.clone())
                .await;
            aggregated_results.add(recorder);

            info!("Finished iteration");
        }

        aggregated_results
    }
}
