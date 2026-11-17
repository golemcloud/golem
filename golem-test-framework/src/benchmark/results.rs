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

use crate::benchmark::config::RunConfig;
use crate::benchmark::{BenchmarkPartition, BenchmarkRecorder, BenchmarkSuite};
use chrono::{DateTime, Utc};
use colored::Colorize;
use comfy_table::presets::NOTHING;
use comfy_table::{Attribute, Cell, CellAlignment, ContentArrangement, Table};
use itertools::Itertools;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DurationMilliSecondsWithFrac, serde_as};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// Limit distinct messages in the console summary. JSON retains all messages.
pub const MAX_FAILURE_SAMPLES: usize = 5;

/// Failed attempts or benchmark phases recorded against one measurement key.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureResult {
    pub count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub samples: Vec<String>,
    /// Every failure, in attempt order within each recorded iteration.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub per_iteration: Vec<Vec<String>>,
}

impl FailureResult {
    pub fn add_iteration(&mut self, messages: &[String]) {
        self.count += messages.len() as u64;
        self.per_iteration.push(messages.to_vec());
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
            // When every count is a single reported value (avg == min == max),
            // the three-column avg/min/max table is noise — collapse it to one
            // "Count" column. Real distributions (where the three differ for any
            // row) keep all three.
            let count_is_single_value = items.iter().all(|i| {
                i.count
                    .as_ref()
                    .map(|c| c.avg == c.min && c.min == c.max)
                    .unwrap_or(true)
            });

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
                if count_is_single_value {
                    header.push(bold("Count"));
                } else {
                    header.push(bold("Count Avg"));
                    header.push(bold("Count Min"));
                    header.push(bold("Count Max"));
                }
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
                    if count_is_single_value {
                        record.push(count_cell(item.count.as_ref().map(|c| c.avg)));
                    } else {
                        record.push(count_cell(item.count.as_ref().map(|c| c.avg)));
                        record.push(count_cell(item.count.as_ref().map(|c| c.min)));
                        record.push(count_cell(item.count.as_ref().map(|c| c.max)));
                    }
                }
                tbl.add_row(record);
            }

            writeln!(f, "{tbl}")?;
        }

        Ok(())
    }
}

/// Cloud-mode run metadata collected by the buildspec and passed via environment variables.
/// All fields are optional — missing env vars produce `None` rather than failing the run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMetadata {
    /// The `golem-oss` commit SHA that was built and deployed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub golem_oss_commit_sha: Option<String>,
    /// The `golem-cloud` (kubernetes manifests) commit SHA that was deployed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kubernetes_manifest_commit_sha: Option<String>,
    /// Number of Ready `worker-executor` pods observed at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub observed_cluster_size: Option<u32>,
    /// Container image tag of the deployed `worker-executor`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_executor_image_tag: Option<String>,
    /// Container image tag of the deployed `registry-service`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub registry_service_image_tag: Option<String>,
    /// Container image tag of the deployed `worker-service`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_service_image_tag: Option<String>,
    /// Image digest of the deployed `worker-executor`, when the manifest pins
    /// one. A tag can be moved; the digest is what actually identifies the
    /// build a run tested, which is the whole point of recording it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_executor_image_digest: Option<String>,
    /// Image digest of the deployed `registry-service`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub registry_service_image_digest: Option<String>,
    /// Image digest of the deployed `worker-service`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_service_image_digest: Option<String>,
    /// Aurora ACU capacity for the main (`golem_dev`) cluster at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub aurora_acu_main: Option<f64>,
    /// Aurora ACU capacity for the indexed-storage cluster at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub aurora_acu_indexed: Option<f64>,
    /// Aurora ACU capacity for the keyvalue-storage cluster at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub aurora_acu_keyvalue: Option<f64>,
    /// Ready replica count for `worker-executor` at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_executor_replicas: Option<u32>,
    /// Ready replica count for `worker-service` at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_service_replicas: Option<u32>,
    /// Ready replica count for `registry-service` at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub registry_service_replicas: Option<u32>,
    /// Ready replica count for `compilation-service` at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compilation_service_replicas: Option<u32>,
    /// Ready replica count for `debugging-service` at run start.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub debugging_service_replicas: Option<u32>,
    /// Free-form note from the `workflow_dispatch` trigger.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

impl RunMetadata {
    /// Reads all `GOLEM_BENCH_*` environment variables and returns a populated
    /// `RunMetadata`.  Missing variables produce `None` for that field.
    pub fn from_env() -> Self {
        fn env_str(key: &str) -> Option<String> {
            std::env::var(key).ok().filter(|v| !v.is_empty())
        }
        fn env_u32(key: &str) -> Option<u32> {
            env_str(key).map(|v| {
                v.parse()
                    .unwrap_or_else(|_| panic!("{key} must be an unsigned integer, got {v:?}"))
            })
        }
        fn env_f64(key: &str) -> Option<f64> {
            env_str(key).map(|v| {
                v.parse()
                    .unwrap_or_else(|_| panic!("{key} must be a number, got {v:?}"))
            })
        }

        Self {
            golem_oss_commit_sha: env_str("GOLEM_BENCH_OSS_COMMIT_SHA"),
            kubernetes_manifest_commit_sha: env_str("GOLEM_BENCH_K8S_MANIFEST_COMMIT_SHA"),
            observed_cluster_size: env_u32("GOLEM_BENCH_OBSERVED_CLUSTER_SIZE"),
            worker_executor_image_tag: env_str("GOLEM_BENCH_WORKER_EXECUTOR_IMAGE_TAG"),
            registry_service_image_tag: env_str("GOLEM_BENCH_REGISTRY_SERVICE_IMAGE_TAG"),
            worker_service_image_tag: env_str("GOLEM_BENCH_WORKER_SERVICE_IMAGE_TAG"),
            worker_executor_image_digest: env_str("GOLEM_BENCH_WORKER_EXECUTOR_IMAGE_DIGEST"),
            registry_service_image_digest: env_str("GOLEM_BENCH_REGISTRY_SERVICE_IMAGE_DIGEST"),
            worker_service_image_digest: env_str("GOLEM_BENCH_WORKER_SERVICE_IMAGE_DIGEST"),
            aurora_acu_main: env_f64("GOLEM_BENCH_AURORA_ACU_MAIN"),
            aurora_acu_indexed: env_f64("GOLEM_BENCH_AURORA_ACU_INDEXED"),
            aurora_acu_keyvalue: env_f64("GOLEM_BENCH_AURORA_ACU_KEYVALUE"),
            worker_executor_replicas: env_u32("GOLEM_BENCH_WORKER_EXECUTOR_REPLICAS"),
            worker_service_replicas: env_u32("GOLEM_BENCH_WORKER_SERVICE_REPLICAS"),
            registry_service_replicas: env_u32("GOLEM_BENCH_REGISTRY_SERVICE_REPLICAS"),
            compilation_service_replicas: env_u32("GOLEM_BENCH_COMPILATION_SERVICE_REPLICAS"),
            debugging_service_replicas: env_u32("GOLEM_BENCH_DEBUGGING_SERVICE_REPLICAS"),
            note: env_str("GOLEM_BENCH_RUN_NOTE"),
        }
    }

    /// Returns `true` if every field is `None` (nothing was read from env).
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
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
pub struct BenchmarkArtifact {
    pub path: String,
    pub sha256: String,
}

impl BenchmarkArtifact {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut file = std::fs::File::open(path)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            sha256: format!("{:x}", hash.finalize()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkArtifacts {
    pub runner: BenchmarkArtifact,
    pub services: BTreeMap<String, BenchmarkArtifact>,
    pub fixtures: BTreeMap<String, BenchmarkArtifact>,
}

impl BenchmarkArtifacts {
    pub const SERVICE_EXECUTABLES: [(&'static str, &'static str); 5] = [
        (
            "golem-component-compilation-service",
            "golem-component-compilation-service",
        ),
        ("golem-worker-service", "golem-worker-service"),
        ("golem-worker-executor", "worker-executor"),
        ("golem-shard-manager", "golem-shard-manager"),
        ("golem-registry-service", "golem-registry-service"),
    ];

    /// Hash exact input files before running a suite. Missing inputs fail the
    /// suite rather than silently producing incomplete provenance.
    pub fn collect(
        runner: &Path,
        services: &BTreeMap<String, std::path::PathBuf>,
        fixtures: &BTreeMap<String, std::path::PathBuf>,
    ) -> anyhow::Result<Self> {
        let hash_all = |paths: &BTreeMap<String, std::path::PathBuf>| {
            paths
                .iter()
                .map(|(name, path)| Ok((name.clone(), BenchmarkArtifact::from_path(path)?)))
                .collect::<anyhow::Result<BTreeMap<_, _>>>()
        };
        Ok(Self {
            runner: BenchmarkArtifact::from_path(runner)?,
            services: hash_all(services)?,
            fixtures: hash_all(fixtures)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkSuiteResult {
    /// Result format version. This producer writes schema 2 only.
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub harness_version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partitions: Vec<BenchmarkPartition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suite_fingerprint: Option<String>,
    pub suite: String,
    pub environment: String,
    pub version: String,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub runner: Option<BenchmarkRunner>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<BenchmarkSource>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub artifacts: Option<BenchmarkArtifacts>,
    /// Suite-level run-id. Set in cloud mode to `bench-{run_id}` to allow
    /// cross-run correlation and garbage collection of orphaned state.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run_id: Option<String>,
    /// Cloud-mode run metadata populated from `GOLEM_BENCH_*` environment variables.
    /// `None` in Spawned or Provided modes where cluster metadata is not available.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run_metadata: Option<RunMetadata>,
    pub results: Vec<BenchmarkResult>,
}

/// Bump when the measurement lifecycle or common workload contract changes.
pub const HARNESS_VERSION: u32 = 1;

fn deserialize_schema_version<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != 2 {
        return Err(D::Error::custom("benchmark schema_version must be 2"));
    }
    Ok(version)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasurementEnvironment {
    pub runner_id: String,
    pub hardware_fingerprint: String,
    pub storage_profile: String,
    pub sockets_enabled: bool,
}

impl MeasurementEnvironment {
    /// Explicit provenance for a remote deployment; never substitute the load generator's CPU.
    pub fn remote_from_env() -> Self {
        let raw = std::env::var("GOLEM_BENCH_MEASUREMENT_ENVIRONMENT")
            .expect("remote benchmarks require GOLEM_BENCH_MEASUREMENT_ENVIRONMENT JSON with runner_id, hardware_fingerprint, storage_profile and sockets_enabled");
        let environment: Self =
            serde_json::from_str(&raw).expect("invalid GOLEM_BENCH_MEASUREMENT_ENVIRONMENT");
        assert!(
            !environment.runner_id.is_empty()
                && !environment.hardware_fingerprint.is_empty()
                && !environment.storage_profile.is_empty(),
            "remote measurement identity fields must not be empty"
        );
        environment
    }

    pub fn for_mode(mode: &crate::config::benchmark::TestMode) -> Self {
        if !matches!(mode, crate::config::benchmark::TestMode::Spawned { .. }) {
            return Self::remote_from_env();
        }
        let sys = System::new_all();
        Self {
            runner_id: format!("local-{}", std::env::consts::ARCH),
            hardware_fingerprint: Self::hardware_fingerprint(
                std::env::consts::ARCH,
                &sys.cpus()
                    .iter()
                    .map(|cpu| (cpu.vendor_id().to_string(), cpu.brand().to_string()))
                    .collect::<Vec<_>>(),
                sys.total_memory(),
            ),
            storage_profile: "postgres-kv-indexed-scheduler+filesystem-blob+sqlite-shard"
                .to_string(),
            sockets_enabled: benchmark_sockets_enabled(),
        }
    }

    pub fn series_key(&self, harness_version: u32, entry_version: u32) -> String {
        let tuple = serde_json::to_vec(&(
            harness_version,
            entry_version,
            &self.runner_id,
            &self.hardware_fingerprint,
            &self.storage_profile,
            self.sockets_enabled,
        ))
        .expect("series identity is serializable");
        format!("{:x}", Sha256::digest(tuple))
    }

    pub fn hardware_fingerprint(
        architecture: &str,
        cpus: &[(String, String)],
        memory_bytes: u64,
    ) -> String {
        let description = serde_json::to_vec(&(architecture, cpus, memory_bytes))
            .expect("hardware identity is serializable");
        format!("{:x}", Sha256::digest(description))
    }
}

/// The launcher and record share the operator's effective setting.
pub fn benchmark_sockets_enabled() -> bool {
    resolve_sockets_enabled(
        std::env::var("GOLEM_BENCH_SOCKETS_ENABLED").ok().as_deref(),
        std::env::var("GOLEM__AGENT_SOCKETS__ENABLED")
            .ok()
            .as_deref(),
    )
}

fn resolve_sockets_enabled(benchmark_override: Option<&str>, native: Option<&str>) -> bool {
    benchmark_override.or(native).is_some_and(|value| {
        value
            .parse()
            .expect("the effective agent sockets setting must be true or false")
    })
}

/// Optional fields used by ceiling measurements in the same result record.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CeilingMeasurement {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lower_bound: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_load: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spread: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limiting_component: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shard_manager_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stores: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<BTreeMap<String, f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_agent_memory_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_agent_disk_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_bound: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_bound: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause_millis: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_criterion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_interval: Option<[f64; 2]>,
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
            schema_version: 2,
            harness_version: HARNESS_VERSION,
            partitions: vec![],
            suite_fingerprint: None,
            suite: suite.to_string(),
            environment,
            version: golem_common::golem_version().to_string(),
            timestamp: Utc::now(),
            runner: None,
            source: None,
            artifacts: None,
            run_id: None,
            run_metadata: None,
            results: vec![],
        }
    }

    pub fn add(&mut self, result: BenchmarkResult) {
        let mut result = result;
        if let Some(runner) = &self.runner {
            result.measurement_environment.runner_id = runner.id.clone();
            result.series_key = result
                .measurement_environment
                .series_key(self.harness_version, result.entry_version);
        } else {
            self.runner = Some(BenchmarkRunner {
                id: result.measurement_environment.runner_id.clone(),
                label: None,
            });
        }
        self.results.push(result);
    }

    /// Forms one complete run without pooling samples or replacing entry provenance.
    pub fn merge_partials(
        suite: &BenchmarkSuite,
        mode: &crate::config::benchmark::TestMode,
        partials: Vec<Self>,
    ) -> anyhow::Result<Self> {
        use anyhow::{bail, ensure};
        let mut expected = BTreeMap::new();
        let mut expected_partitions = BTreeSet::new();
        for item in &suite.benchmarks {
            let partition = item
                .partition
                .ok_or_else(|| anyhow::anyhow!("{} has no partition", item.name))?;
            expected_partitions.insert(partition);
            let name = if item.disable_compilation_cache.unwrap_or_default() {
                format!("{}-no-cache", item.name)
            } else {
                item.name.clone()
            };
            let runs = item.runs(mode);
            ensure!(!runs.is_empty(), "{name} has no workload configurations");
            for config in runs {
                let key = serde_json::to_string(&(&name, config))?;
                ensure!(
                    expected
                        .insert(key, (partition, item.version, item.iterations))
                        .is_none(),
                    "duplicate expanded suite entry {name}"
                );
            }
        }
        let first = partials
            .first()
            .ok_or_else(|| anyhow::anyhow!("no partial results"))?;
        ensure!(
            first
                .source
                .as_ref()
                .and_then(|s| s.commit_sha.as_deref())
                .is_some_and(|s| !s.is_empty()),
            "partial results require a source commit"
        );
        ensure!(
            first.run_id.as_ref().is_some_and(|id| !id.is_empty()),
            "partial results require a common run ID"
        );
        let mut merged = first.clone();
        merged.results.clear();
        merged.partitions.clear();
        merged.environment.clear();
        let fingerprint = suite.fingerprint();
        let mut seen_partitions = BTreeSet::new();
        let mut seen_entries = BTreeSet::new();
        let mut artifacts = first
            .artifacts
            .clone()
            .ok_or_else(|| anyhow::anyhow!("partial results require artifact provenance"))?;
        for partial in &partials {
            ensure!(
                partial.schema_version == 2 && partial.harness_version == first.harness_version,
                "schema or harness mismatch"
            );
            ensure!(
                partial.suite == suite.name
                    && partial.suite_fingerprint.as_ref() == Some(&fingerprint),
                "suite definition mismatch"
            );
            ensure!(
                partial.source == first.source && partial.version == first.version,
                "source mismatch"
            );
            ensure!(
                partial.run_id == first.run_id && partial.timestamp == first.timestamp,
                "logical run mismatch"
            );
            ensure!(
                partial.runner == first.runner
                    && partial.runner.as_ref().is_some_and(|r| !r.id.is_empty()),
                "runner mismatch"
            );
            ensure!(
                partial.run_metadata == first.run_metadata,
                "run metadata mismatch"
            );
            ensure!(
                partial.partitions.len() == 1,
                "expected one partition per partial result"
            );
            let partition = partial.partitions[0];
            ensure!(
                expected_partitions.contains(&partition) && seen_partitions.insert(partition),
                "unexpected or duplicate partition"
            );
            let provenance = partial
                .artifacts
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing artifact provenance"))?;
            if matches!(mode, crate::config::benchmark::TestMode::Spawned { .. }) {
                ensure!(
                    BenchmarkArtifacts::SERVICE_EXECUTABLES
                        .iter()
                        .all(|(name, _)| provenance.services.contains_key(*name)),
                    "missing service provenance"
                );
            }
            ensure!(
                suite
                    .select_partition(partition)?
                    .fixture_names()
                    .iter()
                    .all(|name| provenance.fixtures.contains_key(*name)),
                "missing fixture provenance"
            );
            ensure!(
                provenance.runner == artifacts.runner && provenance.services == artifacts.services,
                "binary provenance mismatch"
            );
            for (name, fixture) in &provenance.fixtures {
                if let Some(previous) = artifacts.fixtures.insert(name.clone(), fixture.clone()) {
                    ensure!(
                        previous == *fixture,
                        "conflicting fixture provenance for {name}"
                    );
                }
            }
            let _ = writeln!(
                &mut merged.environment,
                "{partition:?}: {}",
                partial.environment
            );
            for entry in &partial.results {
                ensure!(
                    entry.measurement_environment.runner_id == partial.runner.as_ref().unwrap().id,
                    "entry runner mismatch"
                );
                ensure!(
                    entry.series_key
                        == entry
                            .measurement_environment
                            .series_key(partial.harness_version, entry.entry_version),
                    "invalid series key for {}",
                    entry.name
                );
                ensure!(
                    !entry
                        .measurement_environment
                        .hardware_fingerprint
                        .is_empty()
                        && !entry.measurement_environment.storage_profile.is_empty(),
                    "missing measurement identity"
                );
                ensure!(
                    !entry.results.is_empty() && entry.runs.len() == entry.results.len(),
                    "missing configurations for {}",
                    entry.name
                );
                let declared: BTreeSet<_> = entry
                    .runs
                    .iter()
                    .map(serde_json::to_string)
                    .collect::<Result<_, _>>()?;
                let actual: BTreeSet<_> = entry
                    .results
                    .iter()
                    .map(|r| serde_json::to_string(&r.run_config))
                    .collect::<Result<_, _>>()?;
                ensure!(
                    declared == actual && actual.len() == entry.results.len(),
                    "configuration mismatch for {}",
                    entry.name
                );
                for result in &entry.results {
                    let key = serde_json::to_string(&(&entry.name, &result.run_config))?;
                    let Some(&(owner, version, iterations)) = expected.get(&key) else {
                        bail!("unexpected entry {}", entry.name)
                    };
                    ensure!(
                        owner == partition
                            && version == entry.entry_version
                            && iterations == entry.iterations,
                        "entry contract mismatch for {}",
                        entry.name
                    );
                    ensure!(
                        seen_entries.insert(key),
                        "duplicate expanded entry {}",
                        entry.name
                    );
                    ensure!(
                        result.iteration_results.len() == iterations
                            && result
                                .iteration_results
                                .iter()
                                .enumerate()
                                .all(|(index, evidence)| evidence.index == index),
                        "incomplete iteration evidence for {}",
                        entry.name
                    );
                }
                merged.results.push(entry.clone());
            }
        }
        ensure!(
            seen_partitions == expected_partitions && seen_entries.len() == expected.len(),
            "missing partitions or expanded entries"
        );
        merged.partitions = seen_partitions.into_iter().collect();
        merged.artifacts = Some(artifacts);
        Ok(merged)
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
    pub entry_version: u32,
    pub iterations: usize,
    pub series_key: String,
    pub measurement_environment: MeasurementEnvironment,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ceiling: Option<CeilingMeasurement>,
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

    pub fn drop_zero_counts(&mut self, retain_selected: bool) {
        for run_result in &mut self.results {
            run_result.drop_zero_counts(retain_selected);
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
    /// One record per requested iteration; lifecycle failures remain separate.
    pub iteration_results: Vec<IterationResult>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub duration_results: HashMap<ResultKey, DurationResult>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub count_results: HashMap<ResultKey, CountResult>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub failures: HashMap<ResultKey, FailureResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IterationResult {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// Durations in milliseconds, matching the aggregate duration encoding.
    pub duration_samples: HashMap<ResultKey, Vec<f64>>,
    pub count_samples: HashMap<ResultKey, Vec<u64>>,
    pub failures: HashMap<ResultKey, Vec<String>>,
}

impl BenchmarkRunResult {
    pub fn new(run_config: RunConfig) -> Self {
        Self {
            run_config,
            iteration_results: Vec::new(),
            duration_results: HashMap::new(),
            count_results: HashMap::new(),
            failures: HashMap::new(),
        }
    }

    pub fn keep_primary_only(&mut self) {
        self.duration_results.retain(|key, _| key.primary);
        self.count_results.retain(|key, _| key.primary);
    }

    pub fn failure_count(&self) -> u64 {
        self.failures.values().map(|f| f.count).sum()
    }

    pub fn drop_zero_counts(&mut self, retain_selected: bool) {
        self.count_results
            .retain(|key, result| result.max > 0 || (retain_selected && key.primary));
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

    pub fn add_iteration(&mut self, index: usize, recorder: BenchmarkRecorder) {
        self.iteration_results.push(IterationResult {
            index,
            skipped: None,
            duration_samples: recorder
                .durations()
                .into_iter()
                .map(|(key, values)| {
                    (
                        key,
                        values
                            .into_iter()
                            .map(|value| value.as_secs_f64() * 1000.0)
                            .collect(),
                    )
                })
                .collect(),
            count_samples: recorder.counts(),
            failures: recorder.failures(),
        });
        self.add(recorder);
    }

    pub fn skip_iterations(&mut self, iterations: usize, reason: &str) {
        self.iteration_results
            .extend((0..iterations).map(|index| IterationResult {
                index,
                skipped: Some(reason.to_string()),
                duration_samples: HashMap::new(),
                count_samples: HashMap::new(),
                failures: HashMap::new(),
            }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn suite_result() -> BenchmarkSuiteResult {
        BenchmarkSuiteResult {
            schema_version: 2,
            harness_version: HARNESS_VERSION,
            partitions: vec![],
            suite_fingerprint: None,
            suite: "CI".to_string(),
            environment: "test environment".to_string(),
            version: "0.0.0".to_string(),
            timestamp: Utc::now(),
            runner: None,
            source: None,
            artifacts: None,
            run_id: None,
            run_metadata: None,
            results: vec![],
        }
    }

    #[test]
    fn schema_two_requires_explicit_versions() {
        assert!(
            serde_json::from_value::<BenchmarkSuiteResult>(serde_json::json!({
                "suite": "CI",
                "environment": "test environment",
                "version": "0.0.0",
                "timestamp": "2026-08-19T00:00:00Z",
                "results": []
            }))
            .is_err()
        );
        let mut json = serde_json::to_value(suite_result()).unwrap();
        json["schema_version"] = serde_json::json!(1);
        assert!(serde_json::from_value::<BenchmarkSuiteResult>(json).is_err());
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

    #[test]
    fn benchmark_artifact_provenance_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let runner = directory.path().join("runner");
        let fixture = directory.path().join("fixture.wasm");
        std::fs::write(&runner, b"abc").unwrap();
        std::fs::write(&fixture, b"").unwrap();
        let artifacts = BenchmarkArtifacts::collect(
            &runner,
            &BTreeMap::from([("worker-executor".into(), runner.clone())]),
            &BTreeMap::from([("fixture".into(), fixture.clone())]),
        )
        .unwrap();
        assert_eq!(
            artifacts.runner.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            artifacts.fixtures["fixture"].sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let mut suite = suite_result();
        suite.artifacts = Some(artifacts);
        let json = serde_json::to_string(&suite).unwrap();
        assert_eq!(
            serde_json::from_str::<BenchmarkSuiteResult>(&json).unwrap(),
            suite
        );
        assert!(BenchmarkArtifact::from_path(&directory.path().join("absent")).is_err());
    }

    #[test]
    fn benchmark_retains_primary_counts_raw_samples_and_selected_zeros() {
        let mut result = BenchmarkRunResult::new(run_config());
        let recorder = BenchmarkRecorder::new();
        recorder.count(&ResultKey::primary("seeded-sessions"), 0);
        recorder.count(&ResultKey::primary("items"), 7);
        recorder.count(&ResultKey::secondary("diagnostic-zero"), 0);
        recorder.duration(&ResultKey::primary("first-item"), Duration::from_millis(3));
        recorder.duration(&ResultKey::primary("first-item"), Duration::from_millis(17));
        result.add(recorder);
        let mut compact = result.clone();
        compact.drop_zero_counts(false);
        assert!(
            !compact
                .count_results
                .contains_key(&ResultKey::primary("seeded-sessions"))
        );
        result.drop_zero_counts(true);
        result.keep_primary_only();
        let json = serde_json::to_string(&result).unwrap();
        let parsed: BenchmarkRunResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.count_results.len(), 2);
        assert_eq!(
            parsed.count_results[&ResultKey::primary("seeded-sessions")].all,
            vec![0]
        );
        assert_eq!(
            parsed.count_results[&ResultKey::primary("items")].all,
            vec![7]
        );
        assert_eq!(
            parsed.duration_results[&ResultKey::primary("first-item")].all,
            vec![Duration::from_millis(3), Duration::from_millis(17)]
        );
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
        assert_eq!(failure.per_iteration.iter().map(Vec::len).sum::<usize>(), 8);
        assert_eq!(failure.per_iteration[1][4], "status 408");
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
    fn run_result_without_failures_round_trips() {
        let mut result = BenchmarkRunResult::new(run_config());
        let recorder = BenchmarkRecorder::new();
        recorder.duration(&"invocation".into(), Duration::from_millis(5));
        result.add(recorder);

        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("failures"));

        let parsed: BenchmarkRunResult = serde_json::from_value(serde_json::json!({
            "iteration_results": [],
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
        assert!(parsed.failures.is_empty());
        assert_eq!(parsed.failure_count(), 0);
    }

    #[test]
    fn benchmark_failures_survive_primary_only_filter() {
        let mut result = BenchmarkRunResult::new(run_config());
        let recorder = recorder_with_failures(&["status 503 for http://x: overloaded"]);
        recorder.failure(&ResultKey::secondary("worker-1"), "status 503");
        result.add(recorder);
        result.keep_primary_only();
        result.drop_details();

        assert_eq!(result.failures.len(), 2);
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
            entry_version: 1,
            iterations: 1,
            series_key: measurement_environment().series_key(HARNESS_VERSION, 1),
            measurement_environment: measurement_environment(),
            ceiling: None,
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
            entry_version: 1,
            iterations: 1,
            series_key: measurement_environment().series_key(HARNESS_VERSION, 1),
            measurement_environment: measurement_environment(),
            ceiling: None,
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

    #[test]
    fn schema_two_requires_harness_version() {
        let mut result = serde_json::to_value(suite_result()).unwrap();
        result.as_object_mut().unwrap().remove("harness_version");
        assert!(serde_json::from_value::<BenchmarkSuiteResult>(result).is_err());
    }

    fn measurement_environment() -> MeasurementEnvironment {
        MeasurementEnvironment {
            runner_id: "runner-a".into(),
            hardware_fingerprint: "cpu-a".into(),
            storage_profile: "postgres".into(),
            sockets_enabled: false,
        }
    }

    #[test]
    fn sockets_identity_honors_inherited_setting_and_benchmark_override() {
        assert!(!resolve_sockets_enabled(None, None));
        assert!(resolve_sockets_enabled(None, Some("true")));
        assert!(!resolve_sockets_enabled(None, Some("false")));
        assert!(resolve_sockets_enabled(Some("true"), None));
        assert!(!resolve_sockets_enabled(Some("false"), None));
        assert!(!resolve_sockets_enabled(Some("false"), Some("true")));
        assert!(resolve_sockets_enabled(Some("true"), Some("false")));
    }

    #[test]
    fn series_identity_uses_only_the_six_contract_inputs() {
        let environment = measurement_environment();
        let key = environment.series_key(1, 3);
        assert_eq!(key, environment.series_key(1, 3));
        assert_ne!(key, environment.series_key(2, 3));
        assert_ne!(key, environment.series_key(1, 4));
        for field in 0..4 {
            let mut changed = environment.clone();
            match field {
                0 => changed.runner_id = "runner-b".into(),
                1 => changed.hardware_fingerprint = "cpu-b".into(),
                2 => changed.storage_profile = "redis".into(),
                _ => changed.sockets_enabled = true,
            }
            assert_ne!(key, changed.series_key(1, 3));
        }
        let cpus = vec![("vendor".into(), "brand".into())];
        let x86 = MeasurementEnvironment::hardware_fingerprint("x86_64", &cpus, 1024);
        assert_eq!(
            x86,
            MeasurementEnvironment::hardware_fingerprint("x86_64", &cpus, 1024)
        );
        assert_ne!(
            x86,
            MeasurementEnvironment::hardware_fingerprint("aarch64", &cpus, 1024)
        );

        let mut suite = suite_result();
        let entry = BenchmarkResult {
            name: "case-path-a".into(),
            entry_version: 3,
            iterations: 3,
            series_key: environment.series_key(HARNESS_VERSION, 3),
            measurement_environment: environment,
            ceiling: None,
            description: "test case".into(),
            runs: vec![run_config()],
            results: vec![],
            run_id: None,
        };
        suite.add(entry.clone());
        let mut other = entry;
        other.name = "case-path-b".into();
        other.runs[0].size = 29;
        other.ceiling = Some(CeilingMeasurement {
            running_bound: Some(7),
            pending_bound: Some(13),
            pause_millis: Some(31),
            ..Default::default()
        });
        suite.source = Some(BenchmarkSource {
            repository: None,
            commit_sha: Some("different-commit".into()),
            source_ref: None,
        });
        suite.artifacts = Some(BenchmarkArtifacts {
            runner: BenchmarkArtifact {
                path: "runner".into(),
                sha256: "changed-runner".into(),
            },
            services: BTreeMap::new(),
            fixtures: BTreeMap::from([(
                "fixture".into(),
                BenchmarkArtifact {
                    path: "fixture.wasm".into(),
                    sha256: "changed-fixture".into(),
                },
            )]),
        });
        suite.add(other);
        let parsed: BenchmarkSuiteResult =
            serde_json::from_str(&serde_json::to_string(&suite).unwrap()).unwrap();
        assert_ne!(parsed.results[0].name, parsed.results[1].name);
        assert_ne!(parsed.results[0].runs, parsed.results[1].runs);
        assert_eq!(parsed.results[0].series_key, parsed.results[1].series_key);
    }

    #[test]
    fn partials_merge_once_with_complete_identity_and_evidence() {
        use crate::config::BenchmarkCliParameters;
        use clap::Parser;
        let params =
            BenchmarkCliParameters::parse_from(["benchmarks", "benchmark", "noop", "spawned"]);
        let mode = params.benchmark_config.mode();
        let suite: BenchmarkSuite = serde_yaml::from_str("name: CI\nbenchmarks:\n  - name: startup\n    version: 2\n    partition: startup\n    iterations: 3\n    clusterSize: [1]\n    size: [7, 11]\n    length: [13]\n    disableCompilationCache: true\n  - name: streaming-rpc\n    version: 4\n    partition: bulk\n    iterations: 3\n    clusterSize: [5]\n    size: [17]\n    length: [19]\n").unwrap();
        let mut template = suite_result();
        template.run_id = Some("workflow-17-attempt-2".into());
        template.suite_fingerprint = Some(suite.fingerprint());
        template.source = Some(BenchmarkSource {
            repository: Some("golemcloud/golem".into()),
            commit_sha: Some("commit-a".into()),
            source_ref: None,
        });
        template.artifacts = Some(BenchmarkArtifacts {
            runner: BenchmarkArtifact {
                path: "benchmarks".into(),
                sha256: "runner-hash".into(),
            },
            services: BenchmarkArtifacts::SERVICE_EXECUTABLES
                .into_iter()
                .map(|(name, path)| {
                    (
                        name.into(),
                        BenchmarkArtifact {
                            path: path.into(),
                            sha256: format!("hash-{name}"),
                        },
                    )
                })
                .collect(),
            fixtures: BTreeMap::new(),
        });
        let partials: Vec<_> = suite
            .benchmarks
            .iter()
            .enumerate()
            .map(|(job, item)| {
                let mut partial = template.clone();
                partial.partitions = vec![item.partition.unwrap()];
                for name in suite
                    .select_partition(item.partition.unwrap())
                    .unwrap()
                    .fixture_names()
                {
                    partial.artifacts.as_mut().unwrap().fixtures.insert(
                        name.into(),
                        BenchmarkArtifact {
                            path: format!("{name}.wasm"),
                            sha256: format!("hash-{name}"),
                        },
                    );
                }
                let mut environment = measurement_environment();
                environment.hardware_fingerprint = format!("machine-{job}");
                let runs = item.runs(mode);
                let results = runs
                    .iter()
                    .map(|config| {
                        let mut result = BenchmarkRunResult::new(config.clone());
                        for (index, duration) in [11, 0, 37].into_iter().enumerate() {
                            let recorder = BenchmarkRecorder::new();
                            if index == 1 {
                                recorder.failure(&"invocation".into(), "failed middle iteration");
                            } else {
                                recorder.duration(
                                    &"invocation".into(),
                                    Duration::from_millis(duration + config.size as u64),
                                );
                            }
                            result.add_iteration(index, recorder);
                        }
                        result
                    })
                    .collect();
                partial.add(BenchmarkResult {
                    name: if item.disable_compilation_cache.unwrap_or_default() {
                        format!("{}-no-cache", item.name)
                    } else {
                        item.name.clone()
                    },
                    entry_version: item.version,
                    iterations: item.iterations,
                    series_key: environment.series_key(HARNESS_VERSION, item.version),
                    measurement_environment: environment,
                    ceiling: None,
                    description: "test".into(),
                    runs,
                    results,
                    run_id: None,
                });
                partial.artifacts.as_mut().unwrap().fixtures.insert(
                    format!("fixture-{job}"),
                    BenchmarkArtifact {
                        path: format!("fixture-{job}.wasm"),
                        sha256: format!("hash-{job}"),
                    },
                );
                partial
            })
            .collect();
        let merged = BenchmarkSuiteResult::merge_partials(&suite, mode, partials.clone()).unwrap();
        assert_eq!(
            merged.results,
            partials
                .iter()
                .flat_map(|p| p.results.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(merged.artifacts.as_ref().unwrap().fixtures.len(), 4);
        assert_eq!(merged.failure_count(), 3);
        assert_ne!(
            merged.results[0]
                .measurement_environment
                .hardware_fingerprint,
            merged.results[1]
                .measurement_environment
                .hardware_fingerprint
        );
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("merged.json");
        merged.save_to_json(&output).unwrap();
        let saved: BenchmarkSuiteResultCollection =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(saved.runs, vec![merged]);

        for mutation in 0..16 {
            let mut wrong = partials.clone();
            match mutation {
                0 => wrong[1].source.as_mut().unwrap().commit_sha = Some("commit-b".into()),
                1 => wrong[1].harness_version += 1,
                2 => wrong[1].run_id = Some("other-attempt".into()),
                3 => wrong[1].timestamp += chrono::Duration::seconds(1),
                4 => wrong[1].suite_fingerprint = Some("other-suite".into()),
                5 => wrong[1].runner.as_mut().unwrap().id = "other-runner".into(),
                6 => wrong[1].partitions = wrong[0].partitions.clone(),
                7 => {
                    let duplicate = wrong[0].results[0].clone();
                    wrong[0].results.push(duplicate);
                }
                8 => wrong[1].results[0].entry_version += 1,
                9 => wrong[1].results[0].results[0]
                    .iteration_results
                    .pop()
                    .map(|_| ())
                    .unwrap(),
                10 => {
                    wrong.pop();
                }
                11 => {
                    wrong[1].artifacts.as_mut().unwrap().runner.sha256 = "different-binary".into()
                }
                12 => {
                    let mut fixture =
                        wrong[0].artifacts.as_ref().unwrap().fixtures["fixture-0"].clone();
                    fixture.sha256 = "conflict".into();
                    wrong[1]
                        .artifacts
                        .as_mut()
                        .unwrap()
                        .fixtures
                        .insert("fixture-0".into(), fixture);
                }
                13 => {
                    for partial in &mut wrong {
                        partial.artifacts.as_mut().unwrap().services.clear();
                    }
                }
                14 => {
                    wrong[1]
                        .artifacts
                        .as_mut()
                        .unwrap()
                        .fixtures
                        .remove("golem_it_agent_rpc_rust_release");
                }
                _ => wrong[1].artifacts = None,
            }
            assert!(
                BenchmarkSuiteResult::merge_partials(&suite, mode, wrong).is_err(),
                "mutation {mutation} was accepted"
            );
        }
    }
}
