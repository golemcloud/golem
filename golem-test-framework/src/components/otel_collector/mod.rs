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

use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, info};

mod docker;

pub use docker::DockerOtelCollector;

#[async_trait]
pub trait OtelCollector: Send + Sync {
    fn otlp_http_endpoint(&self) -> String;
    fn output_dir(&self) -> &Path;
    async fn kill(&self);
}

/// Reads and parses the OTLP JSON lines file for logs.
/// Each line is an `ExportLogsServiceRequest` JSON object.
pub async fn read_otlp_logs(output_dir: &Path) -> anyhow::Result<Vec<OtlpLogRecord>> {
    let path = output_dir.join("otlp-logs.jsonl");
    read_otlp_log_records(&path).await
}

/// Reads and parses the OTLP JSON lines file for metrics.
pub async fn read_otlp_metrics(output_dir: &Path) -> anyhow::Result<Vec<OtlpMetricRecord>> {
    let path = output_dir.join("otlp-metrics.jsonl");
    read_otlp_metric_records(&path).await
}

// --- Deserialization types for OTLP JSON file exporter output ---

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileExportLogsRequest {
    pub resource_logs: Vec<ResourceLogs>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLogs {
    pub scope_logs: Vec<ScopeLogs>,
    #[serde(default)]
    pub resource: Option<OtlpResource>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeLogs {
    pub log_records: Vec<OtlpLogRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpLogRecord {
    #[serde(default)]
    pub time_unix_nano: Option<String>,
    #[serde(default)]
    pub severity_number: Option<u32>,
    #[serde(default)]
    pub severity_text: Option<String>,
    #[serde(default)]
    pub body: Option<OtlpAnyValue>,
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub span_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileExportMetricsRequest {
    pub resource_metrics: Vec<ResourceMetrics>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMetrics {
    pub scope_metrics: Vec<ScopeMetrics>,
    #[serde(default)]
    pub resource: Option<OtlpResource>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeMetrics {
    pub metrics: Vec<OtlpMetricRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpMetricRecord {
    pub name: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpResource {
    #[serde(default)]
    pub attributes: Vec<OtlpKeyValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpKeyValue {
    pub key: String,
    #[serde(default)]
    pub value: Option<OtlpAnyValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpAnyValue {
    #[serde(default)]
    pub string_value: Option<String>,
    #[serde(default)]
    pub int_value: Option<String>,
    #[serde(default)]
    pub bool_value: Option<bool>,
}

async fn read_otlp_log_records(path: &Path) -> anyhow::Result<Vec<OtlpLogRecord>> {
    let content = tokio::fs::read_to_string(path).await?;
    let mut records = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let request: FileExportLogsRequest = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("Failed to parse OTLP logs line: {e}\nLine: {line}"))?;
        for rl in request.resource_logs {
            for sl in rl.scope_logs {
                records.extend(sl.log_records);
            }
        }
    }
    Ok(records)
}

async fn read_otlp_metric_records(path: &Path) -> anyhow::Result<Vec<OtlpMetricRecord>> {
    let content = tokio::fs::read_to_string(path).await?;
    let mut records = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let request: FileExportMetricsRequest = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("Failed to parse OTLP metrics line: {e}\nLine: {line}"))?;
        for rm in request.resource_metrics {
            for sm in rm.scope_metrics {
                records.extend(sm.metrics);
            }
        }
    }
    Ok(records)
}

pub async fn wait_for_otlp_logs(
    output_dir: &Path,
    min_records: usize,
    timeout: Duration,
) -> anyhow::Result<Vec<OtlpLogRecord>> {
    let start = Instant::now();
    loop {
        match read_otlp_logs(output_dir).await {
            Ok(records) if records.len() >= min_records => return Ok(records),
            Ok(records) => {
                debug!(
                    "OTLP logs file has {} records, waiting for at least {min_records}",
                    records.len()
                );
            }
            Err(e) => {
                debug!("Error reading OTLP logs: {e}");
            }
        }
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timed out waiting for {min_records} OTLP log records after {}s",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

pub async fn wait_for_otlp_metrics(
    output_dir: &Path,
    min_records: usize,
    timeout: Duration,
) -> anyhow::Result<Vec<OtlpMetricRecord>> {
    let start = Instant::now();
    loop {
        match read_otlp_metrics(output_dir).await {
            Ok(records) if records.len() >= min_records => return Ok(records),
            Ok(records) => {
                debug!(
                    "OTLP metrics file has {} records, waiting for at least {min_records}",
                    records.len()
                );
            }
            Err(e) => {
                debug!("Error reading OTLP metrics: {e}");
            }
        }
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timed out waiting for {min_records} OTLP metric records after {}s",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn wait_for_collector_startup(health_url: &str, timeout: Duration) {
    info!(
        "Waiting for OTel Collector start at {health_url}, timeout: {}s",
        timeout.as_secs()
    );
    let client = reqwest::Client::new();
    let start = Instant::now();
    loop {
        match client.get(health_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!("OTel Collector is ready");
                return;
            }
            Ok(resp) => {
                debug!("OTel Collector not ready yet, status: {}", resp.status());
            }
            Err(e) => {
                debug!("OTel Collector not ready yet: {e}");
            }
        }
        if start.elapsed() > timeout {
            panic!("Failed to verify that OTel Collector is running at {health_url}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
