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
use cli_table::format::{Border, Separator};
use cli_table::{format::Justify, Cell, CellStruct, Style, Table};
use colored::Colorize;
use itertools::Itertools;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, Instrument};

#[derive(Debug, Clone, Args)]
pub struct BenchmarkConfig {
    /// Number of repetitions of benchmark with the same configuration.
    #[arg(long, default_value = "3")]
    pub iterations: usize,

    /// Cluster size. Can be repeated for multiple benchmarks runs.
    ///
    /// Not applicable to provided cluster.
    /// Total number of runs is multiplication for number of different cluster sizes, sizes and lengths.
    #[arg(long, default_values_t = [3])]
    pub cluster_size: Vec<usize>,

    /// Benchmark-specific size of worker cluster. Can be repeated for multiple benchmarks runs.
    ///
    /// Total number of runs is multiplication for number of different cluster sizes, sizes and lengths.
    #[arg(long, default_values_t = [10])]
    pub size: Vec<usize>,

    /// Benchmark-specific number of work units.
    ///
    /// Total number of runs is multiplication for number of different cluster sizes, sizes and lengths.
    #[arg(long, default_values_t = [100])]
    pub length: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub cluster_size: usize,
    pub size: usize,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunConfigView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    cluster_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    size: Option<usize>,
}

impl PartialOrd<Self> for RunConfigView {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RunConfigView {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.cluster_size, self.length, self.size).cmp(&(
            other.cluster_size,
            other.length,
            other.size,
        ))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DurationResultView {
    pub avg: Duration,
    pub min: Duration,
    pub max: Duration,
}

impl From<&DurationResult> for DurationResultView {
    fn from(value: &DurationResult) -> Self {
        Self {
            avg: value.avg,
            min: value.min,
            max: value.max,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CountResultView {
    pub avg: u64,
    pub min: u64,
    pub max: u64,
}

impl From<&CountResult> for CountResultView {
    fn from(value: &CountResult) -> Self {
        Self {
            avg: value.avg,
            min: value.min,
            max: value.max,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkResultItemView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    config: Option<RunConfigView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    duration: Option<DurationResultView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    count: Option<CountResultView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkResultView {
    results: HashMap<ResultKey, Vec<BenchmarkResultItemView>>,
}

impl Display for BenchmarkResultView {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for (key, items) in self.results.iter().sorted_by_key(|(k, _)| (*k).clone()) {
            writeln!(f, "{} '{}':", "Results for".bold(), key)?;

            if items.len() == 1 {
                let item = items.first().unwrap();

                if let Some(duration_result) = &item.duration {
                    writeln!(f, "Duration:")?;
                    writeln!(f, "  Avg: {:?}", duration_result.avg)?;
                    writeln!(f, "  Min: {:?}", duration_result.min)?;
                    writeln!(f, "  Max: {:?}", duration_result.max)?;
                }

                if let Some(count_result) = &item.count {
                    writeln!(f, "Count:")?;
                    writeln!(f, "  Avg: {:?}", count_result.avg)?;
                    writeln!(f, "  Min: {:?}", count_result.min)?;
                    writeln!(f, "  Max: {:?}", count_result.max)?;
                }
            } else {
                let first_config = items
                    .first()
                    .expect("At lease one result expected")
                    .config
                    .as_ref()
                    .expect("Config expected for multiple results");
                let show_cluster_size = first_config.cluster_size.is_some();
                let show_size = first_config.size.is_some();
                let show_length = first_config.length.is_some();
                let show_duration = items.iter().any(|i| i.duration.is_some());
                let show_count = items.iter().any(|i| i.count.is_some());

                let mut title = Vec::new();
                if show_cluster_size {
                    title.push("Cluster size".cell().bold(true));
                }
                if show_length {
                    title.push("Length".cell().bold(true));
                }
                if show_size {
                    title.push("Size".cell().bold(true));
                }
                if show_duration {
                    title.push("Duration Avg".cell().bold(true));
                    title.push("Duration Min".cell().bold(true));
                    title.push("Duration Max".cell().bold(true));
                }
                if show_count {
                    title.push("Count Avg".cell().bold(true));
                    title.push("Count Min".cell().bold(true));
                    title.push("Count Max".cell().bold(true));
                }

                let tbl = items.iter().sorted_by_key(|i| &i.config).map(|item| {
                    let mut record = Vec::new();
                    if show_cluster_size {
                        record.push(
                            item.config
                                .as_ref()
                                .unwrap()
                                .cluster_size
                                .unwrap()
                                .cell()
                                .justify(Justify::Right),
                        );
                    }
                    if show_length {
                        record.push(
                            item.config
                                .as_ref()
                                .unwrap()
                                .length
                                .unwrap()
                                .cell()
                                .justify(Justify::Right),
                        );
                    }
                    if show_size {
                        record.push(
                            item.config
                                .as_ref()
                                .unwrap()
                                .size
                                .unwrap()
                                .cell()
                                .justify(Justify::Right),
                        );
                    }
                    if show_duration {
                        fn duration_cell(d: Option<&Duration>) -> CellStruct {
                            d.map(|d| format!("{:?}", d))
                                .unwrap_or("".to_string())
                                .cell()
                                .justify(Justify::Right)
                        }

                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.avg)));
                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.min)));
                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.max)));
                    }
                    if show_count {
                        fn count_cell(c: Option<u64>) -> CellStruct {
                            c.map(|c| format!("{}", c))
                                .unwrap_or("".to_string())
                                .cell()
                                .justify(Justify::Right)
                        }

                        record.push(count_cell(item.count.as_ref().map(|c| c.avg)));
                        record.push(count_cell(item.count.as_ref().map(|c| c.min)));
                        record.push(count_cell(item.count.as_ref().map(|c| c.max)));
                    }

                    record
                });

                let res = tbl
                    .table()
                    .title(title)
                    .separator(Separator::builder().build())
                    .border(Border::builder().build())
                    .display()
                    .unwrap();

                writeln!(f, "{}", res)?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    runs: Vec<RunConfig>,
    results: Vec<(RunConfig, BenchmarkRunResult)>,
}

impl BenchmarkResult {
    pub fn view(&self) -> BenchmarkResultView {
        let show_cluster_size = self.runs.iter().map(|c| c.cluster_size).unique().count() > 1;
        let show_size = self.runs.iter().map(|c| c.size).unique().count() > 1;
        let show_length = self.runs.iter().map(|c| c.length).unique().count() > 1;
        let show_config = show_cluster_size || show_size || show_length;

        let mut all_keys = Vec::new();
        for (_, res) in &self.results {
            all_keys.extend(res.count_results.keys().cloned());
            all_keys.extend(res.duration_results.keys().cloned());
        }
        all_keys.sort();
        all_keys.dedup();

        let mut results: HashMap<ResultKey, Vec<BenchmarkResultItemView>> = HashMap::new();

        for key in all_keys {
            for (conf, res) in &self.results {
                let config = RunConfigView {
                    cluster_size: if show_cluster_size {
                        Some(conf.cluster_size)
                    } else {
                        None
                    },
                    size: if show_size { Some(conf.size) } else { None },
                    length: if show_length { Some(conf.length) } else { None },
                };

                let item = BenchmarkResultItemView {
                    config: if show_config { Some(config) } else { None },
                    duration: res.duration_results.get(&key).map(|d| d.into()),
                    count: res.count_results.get(&key).map(|c| c.into()),
                };

                if item.duration.is_some() || item.count.is_some() {
                    let items = results.entry(key.clone()).or_default();
                    items.push(item);
                }
            }
        }

        BenchmarkResultView { results }
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkRunResult {
    duration_results: HashMap<ResultKey, DurationResult>,
    count_results: HashMap<ResultKey, CountResult>,
}

impl Default for BenchmarkRunResult {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchmarkRunResult {
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
    async fn create(params: CliParams, config: RunConfig) -> Self;

    async fn setup_iteration(&self) -> Self::IterationContext;
    async fn warmup(&self, context: &Self::IterationContext);
    async fn run(&self, context: &Self::IterationContext, recorder: BenchmarkRecorder);
    async fn cleanup_iteration(&self, context: Self::IterationContext);
}

#[async_trait]
pub trait BenchmarkApi {
    async fn run_benchmark(params: CliParams) -> BenchmarkResult;
}

async fn run_benchmark<B: Benchmark>(
    params: CliParams,
    config: RunConfig,
    run_name: &str,
) -> BenchmarkRunResult {
    let span = tracing::info_span!("benchmark", name = B::name());
    let _enter = span.enter();
    info!("Initializing benchmark {}", B::name());

    let benchmark = B::create(params.clone(), config.clone())
        .instrument(span.clone())
        .await;
    let mut aggregated_results = BenchmarkRunResult::new();

    for iteration in 0..params.benchmark_config.iterations {
        let span = tracing::info_span!(
            "benchmark",
            name = B::name(),
            run = run_name,
            iteration = iteration
        );
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

#[async_trait]
impl<B: Benchmark> BenchmarkApi for B {
    async fn run_benchmark(params: CliParams) -> BenchmarkResult {
        let runs = params.runs();

        let mut results = Vec::new();

        for (iter, config) in runs.iter().enumerate() {
            let run_name = format!("{}/{}", iter + 1, runs.len());
            results.push((
                config.clone(),
                run_benchmark::<B>(params.clone(), config.clone(), &run_name).await,
            ));
        }

        BenchmarkResult { runs, results }
    }
}
