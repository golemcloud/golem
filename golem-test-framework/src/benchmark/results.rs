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

use crate::benchmark::config::RunConfig;
use crate::benchmark::BenchmarkRecorder;
use chrono::{DateTime, Utc};
use cli_table::format::{Border, Justify, Separator};
use cli_table::{Cell, CellStruct, Style, Table};
use colored::Colorize;
use itertools::Itertools;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, DurationMilliSecondsWithFrac};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write;
use std::fmt::{Debug, Display, Formatter};
use std::path::Path;
use std::time::Duration;
use sysinfo::System;

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
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Display for ResultKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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

            fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
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

#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurationResult {
    #[serde_as(as = "DurationMilliSecondsWithFrac")]
    pub avg: Duration,
    #[serde_as(as = "DurationMilliSecondsWithFrac")]
    pub min: Duration,
    #[serde_as(as = "DurationMilliSecondsWithFrac")]
    pub max: Duration,
    #[serde_as(as = "DurationMilliSecondsWithFrac")]
    pub median: Duration,
    #[serde_as(as = "DurationMilliSecondsWithFrac")]
    pub p90: Duration,
    #[serde_as(as = "DurationMilliSecondsWithFrac")]
    pub p95: Duration,
    #[serde_as(as = "DurationMilliSecondsWithFrac")]
    pub p99: Duration,
    #[serde_as(as = "Vec<DurationMilliSecondsWithFrac>")]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub all: Vec<Duration>,
    #[serde_as(as = "Vec<Vec<DurationMilliSecondsWithFrac>>")]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub per_iteration: Vec<Vec<Duration>>,
    #[serde(skip)]
    locked: bool,
}

impl DurationResult {
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn add_iteration(&mut self, durations: &[Duration]) {
        assert!(!self.locked);

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

        fn percentile(k: f64, sorted_series: &[Duration]) -> Duration {
            assert!(!sorted_series.is_empty());
            assert!((0.0..=100.0).contains(&k));

            let n = sorted_series.len();
            let p = (k / 100.0) * (n as f64 - 1.0);
            let p0 = p.floor() as usize;
            let p1 = p.ceil() as usize;
            if p0 == p1 {
                sorted_series[p0]
            } else {
                let d = p - (p0 as f64);
                sorted_series[p0].mul_f64(1.0 - d) + sorted_series[p1].mul_f64(d)
            }
        }

        let mut sorted = self.all.clone();
        sorted.sort();
        self.median = percentile(50.0, &sorted);
        self.p90 = percentile(90.0, &sorted);
        self.p95 = percentile(95.0, &sorted);
        self.p99 = percentile(99.0, &sorted);
    }

    pub fn drop_details(&mut self) {
        self.all = vec![];
        self.per_iteration = vec![];
        self.locked = true;
    }
}

impl Default for DurationResult {
    fn default() -> Self {
        Self {
            avg: Duration::ZERO,
            min: Duration::MAX,
            max: Duration::ZERO,
            median: Duration::ZERO,
            p90: Duration::ZERO,
            p95: Duration::ZERO,
            p99: Duration::ZERO,
            all: Vec::new(),
            per_iteration: Vec::new(),
            locked: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CountResult {
    pub avg: u64,
    pub min: u64,
    pub max: u64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub all: Vec<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub per_iteration: Vec<Vec<u64>>,
    #[serde(skip)]
    locked: bool,
}

impl CountResult {
    pub fn add_iteration(&mut self, counts: &[u64]) {
        assert!(!self.locked);

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

    pub fn drop_details(&mut self) {
        self.all = vec![];
        self.per_iteration = vec![];
        self.locked = true;
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
            locked: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfigView {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    cluster_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
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
    pub median: Duration,
    pub p90: Duration,
    pub p95: Duration,
    pub p99: Duration,
}

impl From<&DurationResult> for DurationResultView {
    fn from(value: &DurationResult) -> Self {
        Self {
            avg: value.avg,
            min: value.min,
            max: value.max,
            median: value.median,
            p90: value.p90,
            p95: value.p95,
            p99: value.p99,
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
#[serde(rename_all = "camelCase")]
pub struct BenchmarkResultItemView {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    config: Option<RunConfigView>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    duration: Option<DurationResultView>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    count: Option<CountResultView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkResultView {
    name: String,
    description: String,
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
                    writeln!(f, "  Avg:    {:.2?}", duration_result.avg)?;
                    writeln!(f, "  Min:    {:.2?}", duration_result.min)?;
                    writeln!(f, "  Max:    {:.2?}", duration_result.max)?;
                    writeln!(f, "  Median: {:.2?}", duration_result.median)?;
                    writeln!(f, "  p90:    {:.2?}", duration_result.p90)?;
                    writeln!(f, "  p95:    {:.2?}", duration_result.p95)?;
                    writeln!(f, "  p99:    {:.2?}", duration_result.p99)?;
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
                    title.push("Duration Median".cell().bold(true));
                    title.push("Duration p90".cell().bold(true));
                    title.push("Duration p95".cell().bold(true));
                    title.push("Duration p99".cell().bold(true));
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
                            d.map(|d| format!("{d:.2?}"))
                                .unwrap_or("".to_string())
                                .cell()
                                .justify(Justify::Right)
                        }

                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.avg)));
                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.min)));
                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.max)));
                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.median)));
                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.p90)));
                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.p95)));
                        record.push(duration_cell(item.duration.as_ref().map(|d| &d.p99)));
                    }
                    if show_count {
                        fn count_cell(c: Option<u64>) -> CellStruct {
                            c.map(|c| format!("{c}"))
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

                writeln!(f, "{res}")?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkSuiteResultCollection {
    pub runs: Vec<BenchmarkSuiteResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkSuiteResult {
    pub suite: String,
    pub environment: String,
    pub version: String,
    pub timestamp: DateTime<Utc>,
    pub results: Vec<BenchmarkResult>,
}

impl BenchmarkSuiteResult {
    pub fn new(suite: &str) -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        let mut environment = String::new();
        for (idx, cpu) in sys.cpus().iter().enumerate() {
            let _ = writeln!(
                &mut environment,
                "CPU #{idx}: vendor={} brand={}",
                cpu.vendor_id(),
                cpu.brand()
            );
        }
        let _ = writeln!(
            &mut environment,
            "Total memory: {} Gb",
            sys.total_memory() / 1024 / 1024 / 1024
        );
        let _ = writeln!(
            &mut environment,
            "System name={}, os={}, kernel={}, hostname={}",
            System::name().unwrap_or_default(),
            System::long_os_version().unwrap_or_default(),
            System::kernel_version().unwrap_or_default(),
            System::host_name().unwrap_or_default()
        );

        Self {
            suite: suite.to_string(),
            environment,
            version: golem_common::golem_version().to_string(),
            timestamp: Utc::now(),
            results: vec![],
        }
    }

    pub fn add(&mut self, result: BenchmarkResult) {
        self.results.push(result);
    }

    pub fn view(&self) -> BenchmarkSuiteResultView {
        BenchmarkSuiteResultView {
            suite: self.suite.clone(),
            environment: self.environment.clone(),
            timestamp: self.timestamp,
            results: self.results.iter().map(|r| r.view()).collect(),
        }
    }

    pub fn save_to_json(&self, path: &Path) -> anyhow::Result<()> {
        let collection = BenchmarkSuiteResultCollection {
            runs: vec![self.clone()],
        };
        let json = serde_json::to_string_pretty(&collection)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn add_to_json(&self, path: &Path) -> anyhow::Result<()> {
        let collection = if path.exists() {
            let existing_raw = std::fs::read_to_string(path)?;
            let mut collection: BenchmarkSuiteResultCollection =
                serde_json::from_str(&existing_raw)?;
            collection.runs.push(self.clone());
            collection
        } else {
            BenchmarkSuiteResultCollection {
                runs: vec![self.clone()],
            }
        };
        let json = serde_json::to_string_pretty(&collection)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkSuiteResultView {
    pub suite: String,
    pub environment: String,
    pub timestamp: DateTime<Utc>,
    pub results: Vec<BenchmarkResultView>,
}

impl Display for BenchmarkSuiteResultView {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}: {}", "Benchmark suite".bold(), self.suite)?;
        writeln!(f, "{}: {}", "Ran at         ".bold(), self.timestamp)?;
        writeln!(f, "{}\n{}", "Environment".bold(), self.environment)?;

        writeln!(f)?;
        for result in &self.results {
            writeln!(f, "{} '{}'", "Benchmark".bold(), result.name)?;
            writeln!(f, "{}", result.description.blue())?;
            writeln!(f)?;
            writeln!(f, "{}", result)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub name: String,
    pub description: String,
    pub runs: Vec<RunConfig>,
    pub results: Vec<BenchmarkRunResult>,
}

impl BenchmarkResult {
    pub fn primary_only(&mut self) {
        for run_result in &mut self.results {
            run_result.keep_primary_only();
        }
    }

    pub fn drop_details(&mut self) {
        for run_result in &mut self.results {
            run_result.drop_details();
        }
    }

    pub fn drop_zero_counts(&mut self) {
        for run_result in &mut self.results {
            run_result.drop_zero_counts();
        }
    }

    pub fn view(&self) -> BenchmarkResultView {
        let show_cluster_size = self.runs.iter().map(|c| c.cluster_size).unique().count() > 1;
        let show_size = self.runs.iter().map(|c| c.size).unique().count() > 1;
        let show_length = self.runs.iter().map(|c| c.length).unique().count() > 1;
        let show_config = show_cluster_size || show_size || show_length;

        let mut all_keys = Vec::new();
        for res in &self.results {
            all_keys.extend(res.count_results.keys().cloned());
            all_keys.extend(res.duration_results.keys().cloned());
        }
        all_keys.sort();
        all_keys.dedup();

        let mut results: HashMap<ResultKey, Vec<BenchmarkResultItemView>> = HashMap::new();

        for key in all_keys {
            for result in &self.results {
                let config = RunConfigView {
                    cluster_size: if show_cluster_size {
                        Some(result.run_config.cluster_size)
                    } else {
                        None
                    },
                    size: if show_size {
                        Some(result.run_config.size)
                    } else {
                        None
                    },
                    length: if show_length {
                        Some(result.run_config.length)
                    } else {
                        None
                    },
                };

                let item = BenchmarkResultItemView {
                    config: if show_config { Some(config) } else { None },
                    duration: result.duration_results.get(&key).map(|d| d.into()),
                    count: result.count_results.get(&key).map(|c| c.into()),
                };

                if item.duration.is_some() || item.count.is_some() {
                    let items = results.entry(key.clone()).or_default();
                    items.push(item);
                }
            }
        }

        BenchmarkResultView {
            name: self.name.clone(),
            description: self.description.clone(),
            results,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRunResult {
    pub run_config: RunConfig,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub duration_results: HashMap<ResultKey, DurationResult>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub count_results: HashMap<ResultKey, CountResult>,
}

impl BenchmarkRunResult {
    pub fn new(run_config: RunConfig) -> Self {
        Self {
            run_config,
            duration_results: HashMap::new(),
            count_results: HashMap::new(),
        }
    }

    pub fn keep_primary_only(&mut self) {
        self.duration_results.retain(|key, _| key.primary);
        self.count_results.retain(|key, _| key.primary);
    }

    pub fn drop_zero_counts(&mut self) {
        self.count_results.retain(|_, result| result.max > 0);
    }

    pub fn drop_details(&mut self) {
        for duration_result in self.duration_results.values_mut() {
            duration_result.drop_details();
        }
        for count_result in self.count_results.values_mut() {
            count_result.drop_details();
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
