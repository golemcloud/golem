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

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const LOCAL_OBSERVABILITY_CAPABILITY_PATH: &str = "/_golem/local/observability/capability";
pub const LOCAL_OBSERVABILITY_FAMILIES_PATH: &str = "/_golem/local/observability/families";
pub const LOCAL_OBSERVABILITY_SERIES_PATH: &str = "/_golem/local/observability/series";
pub const LOCAL_OBSERVABILITY_SAMPLES_PATH: &str = "/_golem/local/observability/samples";
pub const LOCAL_OTLP_METRICS_PATH: &str = "/v1/metrics";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalObservabilityStatus {
    Ready,
    Disabled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalObservabilityCapability {
    pub status: LocalObservabilityStatus,
    pub message: Option<String>,
    pub exporter_name: String,
    pub exporter_version: String,
    pub sample_interval_seconds: u64,
    pub retention_days: u64,
    pub retention_bytes: u64,
    pub discarded_points: u64,
    pub last_service_sample_unix_nanos: Option<i64>,
    pub last_agent_ingest_unix_nanos: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalMetricSource {
    Services,
    Agents,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalMetricKind {
    Counter,
    Gauge,
    Histogram,
    Event,
    Unknown,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricScope {
    pub environment_id: Option<Uuid>,
    #[serde(default)]
    pub component_ids: Vec<Uuid>,
    #[serde(default)]
    pub agent_ids: Vec<String>,
    pub agent_type: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricTimeRange {
    pub from_unix_nanos: i64,
    pub to_unix_nanos: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricFamiliesRequest {
    pub source: LocalMetricSource,
    pub scope: LocalMetricScope,
    pub range: LocalMetricTimeRange,
    pub name_filter: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricFamily {
    pub name: String,
    pub description: String,
    pub unit: String,
    pub kind: LocalMetricKind,
    pub category: String,
    pub series_count: u64,
    pub latest_value: Option<f64>,
    pub latest_unix_nanos: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricFamiliesResponse {
    pub values: Vec<LocalMetricFamily>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricSeriesRequest {
    pub source: LocalMetricSource,
    pub scope: LocalMetricScope,
    pub range: LocalMetricTimeRange,
    pub family: String,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricSeries {
    pub series_key: String,
    pub statistic: String,
    pub attributes: BTreeMap<String, String>,
    pub environment_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub latest_value: Option<f64>,
    pub latest_unix_nanos: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricSeriesResponse {
    pub values: Vec<LocalMetricSeries>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricSamplesRequest {
    pub source: LocalMetricSource,
    pub scope: LocalMetricScope,
    pub range: LocalMetricTimeRange,
    pub family: String,
    pub series_key: String,
    pub max_points: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricSample {
    pub timestamp_unix_nanos: i64,
    pub value: f64,
    pub statistic: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMetricSamplesResponse {
    pub samples: Vec<LocalMetricSample>,
    pub observed_from_unix_nanos: Option<i64>,
    pub observed_to_unix_nanos: Option<i64>,
    pub original_point_count: u64,
}
