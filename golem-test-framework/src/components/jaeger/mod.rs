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
use std::collections::HashSet;
use std::fmt::Debug;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, info};

mod docker;

#[async_trait]
pub trait Jaeger: Send + Sync {
    fn otlp_http_endpoint(&self) -> String;
    fn query_url(&self) -> String;
    async fn kill(&self);
}

pub use docker::DockerJaeger;

pub struct JaegerQueryClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JaegerQueryResponse {
    pub data: Vec<JaegerTrace>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JaegerTrace {
    #[serde(rename = "traceID")]
    pub trace_id: String,
    pub spans: Vec<JaegerSpan>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JaegerSpan {
    #[serde(rename = "traceID")]
    pub trace_id: String,
    #[serde(rename = "spanID")]
    pub span_id: String,
    #[serde(rename = "operationName")]
    pub operation_name: String,
    pub references: Vec<JaegerReference>,
    pub tags: Vec<JaegerTag>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JaegerReference {
    #[serde(rename = "refType")]
    pub ref_type: String,
    #[serde(rename = "traceID")]
    pub trace_id: String,
    #[serde(rename = "spanID")]
    pub span_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JaegerTag {
    pub key: String,
    #[serde(rename = "type")]
    pub tag_type: String,
    pub value: serde_json::Value,
}

impl JaegerTrace {
    /// Returns the set of all span IDs in this trace.
    pub fn span_ids(&self) -> HashSet<&str> {
        self.spans.iter().map(|s| s.span_id.as_str()).collect()
    }

    /// Logs each span with its parent relationship status.
    ///
    /// `known_external_parent_ids` contains span IDs that are expected to be
    /// outside this trace (e.g. the caller's span ID from a `traceparent` header).
    /// References to these IDs are labelled `[external-caller]` rather than
    /// `[DISCONNECTED]`.
    pub fn dump_spans(&self, known_external_parent_ids: &HashSet<&str>) {
        let span_ids = self.span_ids();
        for span in &self.spans {
            let parent_id = span.parent_span_id().unwrap_or("(root)");
            let parent_status = if parent_id == "(root)" {
                ""
            } else if span_ids.contains(parent_id) {
                " [connected]"
            } else if known_external_parent_ids.contains(parent_id) {
                " [external-caller]"
            } else {
                " [DISCONNECTED]"
            };
            let tags_summary: Vec<String> = span
                .tags
                .iter()
                .filter(|t| {
                    !t.key.starts_with("otel.scope")
                        && t.key != "span.kind"
                        && t.key != "w3c.tracestate"
                })
                .map(|t| format!("{}={}", t.key, t.value))
                .collect();
            info!(
                "  span {} '{}' parent={}{} tags=[{}]",
                span.span_id,
                span.operation_name,
                parent_id,
                parent_status,
                tags_summary.join(", ")
            );
        }
    }

    /// Returns span IDs whose parent references a span not present in this
    /// trace and not listed in `known_external_parent_ids`.
    pub fn disconnected_spans(
        &self,
        known_external_parent_ids: &HashSet<&str>,
    ) -> Vec<(&str, &str)> {
        let span_ids = self.span_ids();
        self.spans
            .iter()
            .filter_map(|s| {
                s.parent_span_id().and_then(|pid| {
                    if !span_ids.contains(pid) && !known_external_parent_ids.contains(pid) {
                        Some((s.span_id.as_str(), pid))
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    /// Returns operation names of spans that have `otel.status_code = ERROR`.
    pub fn error_spans(&self) -> Vec<&str> {
        self.spans
            .iter()
            .filter(|s| {
                s.tags
                    .iter()
                    .any(|t| t.key == "otel.status_code" && t.value == "ERROR")
            })
            .map(|s| s.operation_name.as_str())
            .collect()
    }

    /// Returns span IDs of spans whose operation name is `"unknown"`.
    pub fn unknown_name_spans(&self) -> Vec<&str> {
        self.spans
            .iter()
            .filter(|s| s.operation_name == "unknown")
            .map(|s| s.span_id.as_str())
            .collect()
    }
}

impl JaegerSpan {
    /// Returns the parent span ID from the first CHILD_OF reference, if any.
    pub fn parent_span_id(&self) -> Option<&str> {
        self.references.first().map(|r| r.span_id.as_str())
    }

    /// Returns the value of a tag by key, if present.
    pub fn tag_value(&self, key: &str) -> Option<&serde_json::Value> {
        self.tags.iter().find(|t| t.key == key).map(|t| &t.value)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct JaegerServicesResponse {
    data: Vec<String>,
}

impl JaegerQueryClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn get_trace(&self, trace_id: &str) -> anyhow::Result<Option<JaegerTrace>> {
        let url = format!("{}/api/traces/{}", self.base_url, trace_id);
        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let response = response.error_for_status()?;
        let body: JaegerQueryResponse = response.json().await?;
        Ok(body.data.into_iter().next())
    }

    pub async fn wait_for_trace(
        &self,
        trace_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<JaegerTrace> {
        let start = Instant::now();
        loop {
            match self.get_trace(trace_id).await {
                Ok(Some(trace)) => return Ok(trace),
                Ok(None) => {}
                Err(e) => {
                    debug!("Error fetching trace {trace_id}: {e}");
                }
            }
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timed out waiting for trace {trace_id} after {}s",
                    timeout.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub async fn wait_for_trace_with_min_spans(
        &self,
        trace_id: &str,
        min_spans: usize,
        timeout: Duration,
    ) -> anyhow::Result<JaegerTrace> {
        let start = Instant::now();
        let mut last_count = 0;
        loop {
            match self.get_trace(trace_id).await {
                Ok(Some(trace)) if trace.spans.len() >= min_spans => return Ok(trace),
                Ok(Some(trace)) => {
                    if trace.spans.len() != last_count {
                        info!(
                            "Trace {trace_id} has {} spans so far, waiting for at least {min_spans}",
                            trace.spans.len()
                        );
                        last_count = trace.spans.len();
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    debug!("Error fetching trace {trace_id}: {e}");
                }
            }
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timed out waiting for trace {trace_id} with {min_spans} spans after {}s (last seen: {last_count} spans)",
                    timeout.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub async fn get_services(&self) -> anyhow::Result<Vec<String>> {
        let url = format!("{}/api/services", self.base_url);
        let response = self.client.get(&url).send().await?.error_for_status()?;
        let body: JaegerServicesResponse = response.json().await?;
        Ok(body.data)
    }
}

async fn wait_for_startup(query_url: &str, timeout: Duration) {
    info!(
        "Waiting for Jaeger start at {query_url}, timeout: {}s",
        timeout.as_secs()
    );
    let client = reqwest::Client::new();
    let url = format!("{}/api/services", query_url.trim_end_matches('/'));
    let start = Instant::now();
    loop {
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!("Jaeger is ready at {query_url}");
                return;
            }
            Ok(resp) => {
                debug!("Jaeger not ready yet, status: {}", resp.status());
            }
            Err(e) => {
                debug!("Jaeger not ready yet: {e}");
            }
        }
        if start.elapsed() > timeout {
            panic!("Failed to verify that Jaeger is running at {query_url}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
