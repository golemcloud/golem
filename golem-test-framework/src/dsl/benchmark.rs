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

use crate::config::CliParams;
use async_trait::async_trait;
use clap::Args;
use cli_table::format::{Border, Separator};
use cli_table::{format::Justify, Cell, CellStruct, Style, Table};
use colored::Colorize;
use itertools::Itertools;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
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

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct RunConfig {
    pub cluster_size: usize,
    pub size: usize,
    pub length: usize,
}

#[derive(Clone, Eq, PartialEq, Hash)]
pub struct ResultKey {
    name: String,
    primary: bool,
}

impl ResultKey {
    pub fn primary(name: impl AsRef<str>) -> Self {
        Self {
            name: name.as_ref().to_string(),
            primary: true,
        }
    }

    pub fn secondary(name: impl AsRef<str>) -> Self {
        Self {
            name: name.as_ref().to_string(),
            primary: false,
        }
    }
}

impl Debug for ResultKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Display for ResultKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl PartialOrd<Self> for ResultKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ResultKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

impl From<String> for ResultKey {
    fn from(name: String) -> Self {
        Self::primary(name)
    }
}

impl From<&str> for ResultKey {
    fn from(name: &str) -> Self {
        Self::primary(name)
    }
}

impl Serialize for ResultKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.primary {
            serializer.serialize_str(&self.name)
        } else {
            serializer.serialize_str(&format!("{}__secondary", self.name))
        }
    }
}

impl<'de> Deserialize<'de> for ResultKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ResultKeyVisitor;

        impl Visitor<'_> for ResultKeyVisitor {
            type Value = ResultKey;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct ResultKey")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                let name = v.to_string();
                if name.ends_with("__secondary") {
                    Ok(ResultKey {
                        name: name[0..(name.len() - "__secondary".len())].to_string(),
                        primary: false,
                    })
                } else {
                    Ok(ResultKey {
                        name,
                        primary: true,
                    })
                }
            }
        }

        const FIELDS: &[&str] = &["secs", "nanos"];
        deserializer.deserialize_struct("Duration", FIELDS, ResultKeyVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub runs: Vec<RunConfig>,
    pub results: Vec<(RunConfig, BenchmarkRunResult)>,
}

impl BenchmarkResult {
    pub fn primary_only(self) -> Self {
        Self {
            runs: self.runs,
            results: self
                .results
                .into_iter()
                .map(|(run_config, run_result)| (run_config, run_result.primary_only()))
                .collect(),
        }
    }

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRunResult {
    pub duration_results: HashMap<ResultKey, DurationResult>,
    pub count_results: HashMap<ResultKey, CountResult>,
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

    pub fn primary_only(self) -> Self {
        Self {
            duration_results: self
                .duration_results
                .into_iter()
                .filter(|(k, _)| k.primary)
                .collect(),
            count_results: self
                .count_results
                .into_iter()
                .filter(|(k, _)| k.primary)
                .collect(),
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
    type BenchmarkContext: Send + Sync + 'static;
    type IterationContext: Send + Sync + 'static;

    fn name() -> &'static str;

    async fn create_benchmark_context(
        params: CliParams,
        cluster_size: usize,
    ) -> Self::BenchmarkContext;
    async fn cleanup(benchmark_context: Self::BenchmarkContext);
    async fn create(params: CliParams, config: RunConfig) -> Self;

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
    async fn run_benchmark(params: CliParams) -> BenchmarkResult;
}

async fn run_benchmark<B: Benchmark>(
    benchmark_context: &B::BenchmarkContext,
    params: CliParams,
    config: RunConfig,
    cluster_size: usize,
    run_name: &str,
) -> BenchmarkRunResult {
    let span = tracing::info_span!(
        "benchmark",
        name = B::name(),
        cluster_size = cluster_size,
        run = run_name
    );
    let _enter = span.enter();
    info!("Starting benchmark iterations {}", B::name());

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
    async fn run_benchmark(params: CliParams) -> BenchmarkResult {
        let span = tracing::info_span!("benchmark", name = B::name());
        let _enter = span.enter();
        info!("Initializing benchmark {}", B::name());

        let runs = params.runs();

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
            let context = B::create_benchmark_context(params.clone(), cluster_size)
                .instrument(span.clone())
                .await;

            for config in runs {
                current_run += 1;
                let run_name = format!("{current_run}/{runs_cnt}");
                results.push((
                    config.clone(),
                    run_benchmark::<B>(
                        &context,
                        params.clone(),
                        config.clone(),
                        cluster_size,
                        &run_name,
                    )
                    .instrument(span.clone())
                    .await,
                ));
            }

            info!("Stopping benchmark context");
            B::cleanup(context).instrument(span.clone()).await;
        }

        BenchmarkResult { runs, results }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::dsl::benchmark::{
        BenchmarkResult, BenchmarkRunResult, CountResult, DurationResult, ResultKey, RunConfig,
    };
    use std::collections::HashMap;
    use std::time::Duration;

    #[test]
    fn benchmark_result_is_serializable_to_json() {
        let rc1 = RunConfig {
            cluster_size: 1,
            size: 10,
            length: 20,
        };
        let rc2 = RunConfig {
            cluster_size: 5,
            size: 100,
            length: 20,
        };

        let mut dr1 = HashMap::new();
        let mut cr1 = HashMap::new();
        let mut dr2 = HashMap::new();
        let mut cr2 = HashMap::new();

        dr1.insert(
            ResultKey::primary("a"),
            DurationResult {
                avg: Duration::from_millis(500),
                min: Duration::from_millis(100),
                max: Duration::from_millis(900),
                all: vec![
                    Duration::from_millis(100),
                    Duration::from_millis(500),
                    Duration::from_millis(900),
                ],
                per_iteration: vec![vec![
                    Duration::from_millis(100),
                    Duration::from_millis(500),
                    Duration::from_millis(900),
                ]],
            },
        );

        cr1.insert(
            ResultKey::primary("a"),
            CountResult {
                avg: 500,
                min: 100,
                max: 900,
                all: vec![100, 500, 900],
                per_iteration: vec![vec![100, 500, 900]],
            },
        );

        dr2.insert(
            ResultKey::secondary("b"),
            DurationResult {
                avg: Duration::from_millis(500),
                min: Duration::from_millis(100),
                max: Duration::from_millis(900),
                all: vec![
                    Duration::from_millis(100),
                    Duration::from_millis(500),
                    Duration::from_millis(900),
                ],
                per_iteration: vec![vec![
                    Duration::from_millis(100),
                    Duration::from_millis(500),
                    Duration::from_millis(900),
                ]],
            },
        );

        cr2.insert(
            ResultKey::secondary("b"),
            CountResult {
                avg: 500,
                min: 100,
                max: 900,
                all: vec![100, 500, 900],
                per_iteration: vec![vec![100, 500, 900]],
            },
        );

        let example = BenchmarkResult {
            runs: vec![rc1.clone(), rc2.clone()],
            results: vec![
                (
                    rc1,
                    BenchmarkRunResult {
                        duration_results: dr1,
                        count_results: cr1,
                    },
                ),
                (
                    rc2,
                    BenchmarkRunResult {
                        duration_results: dr2,
                        count_results: cr2,
                    },
                ),
            ],
        };

        let json = serde_json::to_string_pretty(&example).unwrap();
        assert_eq!(
            json,
            r#"{
  "runs": [
    {
      "cluster_size": 1,
      "size": 10,
      "length": 20
    },
    {
      "cluster_size": 5,
      "size": 100,
      "length": 20
    }
  ],
  "results": [
    [
      {
        "cluster_size": 1,
        "size": 10,
        "length": 20
      },
      {
        "duration_results": {
          "a": {
            "avg": {
              "secs": 0,
              "nanos": 500000000
            },
            "min": {
              "secs": 0,
              "nanos": 100000000
            },
            "max": {
              "secs": 0,
              "nanos": 900000000
            },
            "all": [
              {
                "secs": 0,
                "nanos": 100000000
              },
              {
                "secs": 0,
                "nanos": 500000000
              },
              {
                "secs": 0,
                "nanos": 900000000
              }
            ],
            "per_iteration": [
              [
                {
                  "secs": 0,
                  "nanos": 100000000
                },
                {
                  "secs": 0,
                  "nanos": 500000000
                },
                {
                  "secs": 0,
                  "nanos": 900000000
                }
              ]
            ]
          }
        },
        "count_results": {
          "a": {
            "avg": 500,
            "min": 100,
            "max": 900,
            "all": [
              100,
              500,
              900
            ],
            "per_iteration": [
              [
                100,
                500,
                900
              ]
            ]
          }
        }
      }
    ],
    [
      {
        "cluster_size": 5,
        "size": 100,
        "length": 20
      },
      {
        "duration_results": {
          "b__secondary": {
            "avg": {
              "secs": 0,
              "nanos": 500000000
            },
            "min": {
              "secs": 0,
              "nanos": 100000000
            },
            "max": {
              "secs": 0,
              "nanos": 900000000
            },
            "all": [
              {
                "secs": 0,
                "nanos": 100000000
              },
              {
                "secs": 0,
                "nanos": 500000000
              },
              {
                "secs": 0,
                "nanos": 900000000
              }
            ],
            "per_iteration": [
              [
                {
                  "secs": 0,
                  "nanos": 100000000
                },
                {
                  "secs": 0,
                  "nanos": 500000000
                },
                {
                  "secs": 0,
                  "nanos": 900000000
                }
              ]
            ]
          }
        },
        "count_results": {
          "b__secondary": {
            "avg": 500,
            "min": 100,
            "max": 900,
            "all": [
              100,
              500,
              900
            ],
            "per_iteration": [
              [
                100,
                500,
                900
              ]
            ]
          }
        }
      }
    ]
  ]
}"#
        );

        let deserialized = serde_json::from_str::<BenchmarkResult>(&json).unwrap();
        assert_eq!(example, deserialized);
    }
}
