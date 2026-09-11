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

use crate::benchmark::BenchmarkRecorder;
use crate::benchmark::config::RunConfig;
use chrono::{DateTime, Utc};
use colored::Colorize;
use comfy_table::presets::NOTHING;
use comfy_table::{Attribute, Cell, CellAlignment, ContentArrangement, Table};
use itertools::Itertools;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DurationMilliSecondsWithFrac, serde_as};
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

/// How many distinct failure messages a `FailureResult` keeps. The count is
/// exact; the samples exist so the report can say *what* failed without the
/// results JSON growing by one string per failed attempt.
pub const MAX_FAILURE_SAMPLES: usize = 5;

/// Failed attempts recorded against one measurement key. A failure is an
/// attempt that did not produce a result and was retried (a non-success HTTP
/// status, a transport error, a timeout), so the durations recorded under the
/// same key are only the attempts that succeeded outright.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureResult {
    pub count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub samples: Vec<String>,
}

impl FailureResult {
    pub fn add_iteration(&mut self, messages: &[String]) {
        self.count += messages.len() as u64;
        for message in messages {
            if self.samples.len() >= MAX_FAILURE_SAMPLES {
                break;
            }
            if !self.samples.contains(message) {
                self.samples.push(message.clone());
            }
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
pub struct FailureView {
    config: RunConfigView,
    key: ResultKey,
    count: u64,
    samples: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkResultView {
    name: String,
    description: String,
    results: HashMap<ResultKey, Vec<BenchmarkResultItemView>>,
    failures: Vec<FailureView>,
}

impl BenchmarkResultView {
    pub fn failure_count(&self) -> u64 {
        self.failures.iter().map(|failure| failure.count).sum()
    }
}

impl Display for BenchmarkResultView {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if !self.failures.is_empty() {
            writeln!(
                f,
                "{}",
                format!(
                    "FAILURES: {} failed attempts in '{}'",
                    self.failure_count(),
                    self.name
                )
                .red()
                .bold()
            )?;
            for failure in &self.failures {
                let config = &failure.config;
                let mut scope = Vec::new();
                if let Some(cluster_size) = config.cluster_size {
                    scope.push(format!("cluster size {cluster_size}"));
                }
                if let Some(length) = config.length {
                    scope.push(format!("length {length}"));
                }
                if let Some(size) = config.size {
                    scope.push(format!("size {size}"));
                }
                let scope = if scope.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", scope.join(", "))
                };
                writeln!(
                    f,
                    "{}",
                    format!(
                        "  {} failed attempts for '{}'{scope}",
                        failure.count, failure.key
                    )
                    .red()
                )?;
                for sample in &failure.samples {
                    writeln!(f, "    - {sample}")?;
                }
            }
            writeln!(f)?;
        }

        for (key, items) in self.results.iter().sorted_by_key(|(k, _)| (*k).clone()) {
            writeln!(f, "{} '{}':", "Results for".bold(), key)?;

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

            fn bold(s: &str) -> Cell {
                Cell::new(s).add_attribute(Attribute::Bold)
            }
            fn right(s: String) -> Cell {
                Cell::new(s).set_alignment(CellAlignment::Right)
            }

            let mut header = Vec::new();
            if show_cluster_size {
                header.push(bold("Cluster size"));
            }
            if show_length {
                header.push(bold("Length"));
            }
            if show_size {
                header.push(bold("Size"));
            }
            if show_duration {
                header.push(bold("Duration Avg"));
                header.push(bold("Duration Min"));
                header.push(bold("Duration Max"));
                header.push(bold("Duration Median"));
                header.push(bold("Duration p90"));
                header.push(bold("Duration p95"));
                header.push(bold("Duration p99"));
            }
            if show_count {
                header.push(bold("Count Avg"));
                header.push(bold("Count Min"));
                header.push(bold("Count Max"));
            }

            let mut tbl = Table::new();
            tbl.load_preset(NOTHING)
                .set_content_arrangement(ContentArrangement::Disabled)
                .set_header(header);

            for item in items.iter().sorted_by_key(|i| &i.config) {
                let mut record = Vec::new();
                if show_cluster_size {
                    record.push(right(
                        item.config
                            .as_ref()
                            .unwrap()
                            .cluster_size
                            .unwrap()
                            .to_string(),
                    ));
                }
                if show_length {
                    record.push(right(
                        item.config.as_ref().unwrap().length.unwrap().to_string(),
                    ));
                }
                if show_size {
                    record.push(right(
                        item.config.as_ref().unwrap().size.unwrap().to_string(),
                    ));
                }
                if show_duration {
                    let duration_cell = |d: Option<&Duration>| {
                        right(d.map(|d| format!("{d:.2?}")).unwrap_or_default())
                    };
                    record.push(duration_cell(item.duration.as_ref().map(|d| &d.avg)));
                    record.push(duration_cell(item.duration.as_ref().map(|d| &d.min)));
                    record.push(duration_cell(item.duration.as_ref().map(|d| &d.max)));
                    record.push(duration_cell(item.duration.as_ref().map(|d| &d.median)));
                    record.push(duration_cell(item.duration.as_ref().map(|d| &d.p90)));
                    record.push(duration_cell(item.duration.as_ref().map(|d| &d.p95)));
                    record.push(duration_cell(item.duration.as_ref().map(|d| &d.p99)));
                }
                if show_count {
                    let count_cell =
                        |c: Option<u64>| right(c.map(|c| format!("{c}")).unwrap_or_default());
                    record.push(count_cell(item.count.as_ref().map(|c| c.avg)));
                    record.push(count_cell(item.count.as_ref().map(|c| c.min)));
                    record.push(count_cell(item.count.as_ref().map(|c| c.max)));
                }
                tbl.add_row(record);
            }

            writeln!(f, "{tbl}")?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkSuiteResultCollection {
    pub runs: Vec<BenchmarkSuiteResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRunner {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkSource {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub commit_sha: Option<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none", default)]
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkSuiteResult {
    pub suite: String,
    pub environment: String,
    pub version: String,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub runner: Option<BenchmarkRunner>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<BenchmarkSource>,
    /// Suite-level run-id. Set in cloud mode to `bench-{run_id}` to allow
    /// cross-run correlation and garbage collection of orphaned state.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run_id: Option<String>,
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
            runner: None,
            source: None,
            run_id: None,
            results: vec![],
        }
    }

    pub fn add(&mut self, result: BenchmarkResult) {
        self.results.push(result);
    }

    /// Total number of failed attempts recorded by every benchmark in the
    /// suite. Non-zero means the run must not be read as a clean measurement.
    pub fn failure_count(&self) -> u64 {
        self.results.iter().map(|r| r.failure_count()).sum()
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

impl BenchmarkSuiteResultView {
    /// The banner printed before and after the per-benchmark output whenever
    /// any benchmark recorded a failed attempt. It is repeated at the end
    /// because the end of the log is what a reader of a long CI run sees.
    fn write_failure_banner(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let failed: Vec<_> = self
            .results
            .iter()
            .filter(|r| r.failure_count() > 0)
            .collect();
        if failed.is_empty() {
            return Ok(());
        }
        let total: u64 = failed.iter().map(|r| r.failure_count()).sum();
        writeln!(
            f,
            "{}",
            format!(
                "FAILURES: {total} failed attempts in {} of {} benchmarks; \
                 the measurements below are not clean",
                failed.len(),
                self.results.len()
            )
            .red()
            .bold()
        )?;
        for result in failed {
            writeln!(
                f,
                "{}",
                format!(
                    "  {}: {} failed attempts",
                    result.name,
                    result.failure_count()
                )
                .red()
            )?;
        }
        writeln!(f)
    }
}

impl Display for BenchmarkSuiteResultView {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}: {}", "Benchmark suite".bold(), self.suite)?;
        writeln!(f, "{}: {}", "Ran at         ".bold(), self.timestamp)?;
        writeln!(f, "{}\n{}", "Environment".bold(), self.environment)?;

        writeln!(f)?;
        self.write_failure_banner(f)?;
        for result in &self.results {
            writeln!(f, "{} '{}'", "Benchmark".bold(), result.name)?;
            writeln!(f, "{}", result.description.blue())?;
            writeln!(f)?;
            writeln!(f, "{}", result)?;
        }
        self.write_failure_banner(f)?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub name: String,
    pub description: String,
    pub runs: Vec<RunConfig>,
    pub results: Vec<BenchmarkRunResult>,
    /// Suite-level run-id. Set in cloud mode to `bench-{run_id}` to allow
    /// cross-run correlation and garbage collection of orphaned state.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run_id: Option<String>,
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

    pub fn failure_count(&self) -> u64 {
        self.results.iter().map(|r| r.failure_count()).sum()
    }

    pub fn view(&self) -> BenchmarkResultView {
        let show_cluster_size = self.runs.iter().map(|c| c.cluster_size).unique().count() > 1;
        let show_size = self.runs.iter().map(|c| c.size).unique().count() > 1;
        let show_length = self.runs.iter().map(|c| c.length).unique().count() > 1;
        let show_config = true;

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

        let mut failures = Vec::new();
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
            for (key, failure) in result.failures.iter().sorted_by_key(|(k, _)| (*k).clone()) {
                failures.push(FailureView {
                    config: config.clone(),
                    key: key.clone(),
                    count: failure.count,
                    samples: failure.samples.clone(),
                });
            }
        }

        BenchmarkResultView {
            name: self.name.clone(),
            description: self.description.clone(),
            results,
            failures,
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
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub failures: HashMap<ResultKey, FailureResult>,
}

impl BenchmarkRunResult {
    pub fn new(run_config: RunConfig) -> Self {
        Self {
            run_config,
            duration_results: HashMap::new(),
            count_results: HashMap::new(),
            failures: HashMap::new(),
        }
    }

    pub fn keep_primary_only(&mut self) {
        self.duration_results.retain(|key, _| key.primary);
        self.count_results.retain(|key, _| key.primary);
        self.failures.retain(|key, _| key.primary);
    }

    pub fn failure_count(&self) -> u64 {
        self.failures.values().map(|f| f.count).sum()
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

        for (key, messages) in record.failures() {
            if messages.is_empty() {
                continue;
            }

            let results = self.failures.entry(key.clone()).or_default();
            results.add_iteration(&messages);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn suite_result() -> BenchmarkSuiteResult {
        BenchmarkSuiteResult {
            suite: "CI".to_string(),
            environment: "test environment".to_string(),
            version: "0.0.0".to_string(),
            timestamp: Utc::now(),
            runner: None,
            source: None,
            run_id: None,
            results: vec![],
        }
    }

    #[test]
    fn legacy_suite_result_deserializes_without_metadata() {
        let result: BenchmarkSuiteResult = serde_json::from_value(serde_json::json!({
            "suite": "CI",
            "environment": "test environment",
            "version": "0.0.0",
            "timestamp": "2026-08-19T00:00:00Z",
            "results": []
        }))
        .unwrap();

        assert_eq!(result.runner, None);
        assert_eq!(result.source, None);
    }

    #[test]
    fn appending_preserves_runner_and_source_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("results.json");
        let mut existing = suite_result();
        existing.runner = Some(BenchmarkRunner {
            id: "amp-orb-a1.xxlarge".to_string(),
            label: Some("Amp orb (a1.xxlarge)".to_string()),
        });
        existing.source = Some(BenchmarkSource {
            repository: Some("golemcloud/golem".to_string()),
            commit_sha: Some("0123456789abcdef".to_string()),
            source_ref: Some("refs/heads/main".to_string()),
        });
        existing.save_to_json(&path).unwrap();

        suite_result().add_to_json(&path).unwrap();

        let raw = std::fs::read_to_string(path).unwrap();
        let collection: BenchmarkSuiteResultCollection = serde_json::from_str(&raw).unwrap();
        assert_eq!(collection.runs.len(), 2);
        assert_eq!(collection.runs[0], existing);
        assert_eq!(collection.runs[1].runner, None);
        assert_eq!(collection.runs[1].source, None);
        assert!(raw.contains("\"commitSha\""));
        assert!(raw.contains("\"ref\""));
        assert!(!raw.contains("\"commit_sha\""));
        assert!(!raw.contains("\"source_ref\""));
    }

    fn run_config() -> RunConfig {
        RunConfig {
            cluster_size: 1,
            size: 10,
            length: 100,
            disable_compilation_cache: false,
        }
    }

    fn recorder_with_failures(messages: &[&str]) -> BenchmarkRecorder {
        let recorder = BenchmarkRecorder::new();
        recorder.duration(&"invocation".into(), Duration::from_millis(5));
        for message in messages {
            recorder.failure(&"invocation".into(), *message);
        }
        recorder
    }

    #[test]
    fn failures_aggregate_across_iterations_with_capped_distinct_samples() {
        let mut result = BenchmarkRunResult::new(run_config());
        result.add(recorder_with_failures(&[
            "status 503",
            "status 503",
            "timed out",
        ]));
        result.add(recorder_with_failures(&[
            "status 502",
            "status 500",
            "status 504",
            "status 429",
            "status 408",
        ]));

        let failure = &result.failures[&ResultKey::primary("invocation")];
        assert_eq!(failure.count, 8);
        assert_eq!(failure.samples.len(), MAX_FAILURE_SAMPLES);
        assert_eq!(
            failure.samples,
            vec![
                "status 503",
                "timed out",
                "status 502",
                "status 500",
                "status 504"
            ]
        );
        assert_eq!(result.failure_count(), 8);
    }

    #[test]
    fn run_result_without_failures_serializes_and_deserializes_as_before() {
        let mut result = BenchmarkRunResult::new(run_config());
        let recorder = BenchmarkRecorder::new();
        recorder.duration(&"invocation".into(), Duration::from_millis(5));
        result.add(recorder);

        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("failures"));

        let legacy: BenchmarkRunResult = serde_json::from_value(serde_json::json!({
            "run_config": {
                "clusterSize": 1, "size": 10, "length": 100, "disableCompilationCache": false
            },
            "duration_results": {
                "invocation": {
                    "avg": 5.0, "min": 5.0, "max": 5.0, "median": 5.0,
                    "p90": 5.0, "p95": 5.0, "p99": 5.0
                }
            }
        }))
        .unwrap();
        assert!(legacy.failures.is_empty());
        assert_eq!(legacy.failure_count(), 0);
    }

    #[test]
    fn failures_round_trip_through_json_and_survive_primary_only() {
        let mut result = BenchmarkRunResult::new(run_config());
        let recorder = recorder_with_failures(&["status 503 for http://x: overloaded"]);
        recorder.failure(&ResultKey::secondary("worker-1"), "status 503");
        result.add(recorder);
        result.keep_primary_only();
        result.drop_details();

        assert_eq!(result.failures.len(), 1);
        let json = serde_json::to_string(&result).unwrap();
        let parsed: BenchmarkRunResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.failures, result.failures);
        assert_eq!(
            parsed.failures[&ResultKey::primary("invocation")].samples,
            vec!["status 503 for http://x: overloaded"]
        );
    }

    #[test]
    fn suite_report_is_loud_about_failures_and_silent_without_them() {
        let mut clean = BenchmarkRunResult::new(run_config());
        clean.add(recorder_with_failures(&[]));
        let mut failing = BenchmarkRunResult::new(run_config());
        failing.add(recorder_with_failures(&[
            "status 503 for http://x: overloaded",
        ]));

        let mut suite = suite_result();
        suite.add(BenchmarkResult {
            name: "clean".to_string(),
            description: "no failures".to_string(),
            runs: vec![run_config()],
            results: vec![clean],
            run_id: None,
        });
        assert_eq!(suite.failure_count(), 0);
        let report = suite.view().to_string();
        assert!(!report.contains("FAILURES"), "{report}");

        suite.add(BenchmarkResult {
            name: "failing".to_string(),
            description: "one retried invocation".to_string(),
            runs: vec![run_config()],
            results: vec![failing],
            run_id: None,
        });
        assert_eq!(suite.failure_count(), 1);
        let report = suite.view().to_string();
        assert!(
            report.contains("FAILURES: 1 failed attempts in 1 of 2 benchmarks"),
            "{report}"
        );
        assert!(report.contains("failing: 1 failed attempts"), "{report}");
        assert!(
            report.contains("1 failed attempts for 'invocation'"),
            "{report}"
        );
        assert!(
            report.contains("status 503 for http://x: overloaded"),
            "{report}"
        );
        assert_eq!(report.matches("FAILURES:").count(), 3, "{report}");
    }
}
