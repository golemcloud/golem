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
