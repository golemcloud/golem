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
use std::collections::{HashMap, HashSet};
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
    /// Jaeger sends `null` rather than `[]` when no trace matched.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub data: Vec<JaegerTrace>,
}

fn null_as_empty<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
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
    /// Span start, in microseconds since the Unix epoch.
    #[serde(rename = "startTime")]
    pub start_time: u64,
    /// Span duration in microseconds.
    pub duration: u64,
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

    /// Returns `(operation_name, parent_operation_name)` for every span that
    /// escapes its parent's time window - it either started before its parent or
    /// ended after it.
    ///
    /// A span is supposed to represent one operation, and a parent is supposed to
    /// contain the operations of its children; a child that outlives its parent
    /// makes critical-path analysis meaningless. Spans whose parent was not
    /// exported into this trace are skipped, since there is nothing to compare
    /// against, and linked roots (FOLLOWS_FROM only) are not children at all so
    /// they are exempt by construction.
    pub fn spans_outliving_parent(&self) -> Vec<(&str, &str)> {
        let by_id: HashMap<&str, &JaegerSpan> =
            self.spans.iter().map(|s| (s.span_id.as_str(), s)).collect();

        self.spans
            .iter()
            .filter_map(|span| {
                let parent = by_id.get(span.parent_span_id()?)?;
                (span.start_time < parent.start_time || span.end_time() > parent.end_time())
                    .then_some((span.operation_name.as_str(), parent.operation_name.as_str()))
            })
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
    /// Returns the parent span ID from the CHILD_OF reference, if any.
    ///
    /// Only CHILD_OF establishes parentage. A span may also carry FOLLOWS_FROM
    /// references, which is how OpenTelemetry span links are surfaced, and those
    /// must not be mistaken for a parent - a span whose only reference is
    /// FOLLOWS_FROM is the root of its own trace.
    pub fn parent_span_id(&self) -> Option<&str> {
        self.references
            .iter()
            .find(|r| r.ref_type == "CHILD_OF")
            .map(|r| r.span_id.as_str())
    }

    /// Span end, in microseconds since the Unix epoch.
    pub fn end_time(&self) -> u64 {
        self.start_time + self.duration
    }

    /// Returns the value of a tag by key, if present.
    pub fn tag_value(&self, key: &str) -> Option<&serde_json::Value> {
        self.tags.iter().find(|t| t.key == key).map(|t| &t.value)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct JaegerServicesResponse {
    /// Jaeger sends `null` rather than `[]` when it knows of no services yet.
    #[serde(default, deserialize_with = "null_as_empty")]
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

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::{JaegerReference, JaegerSpan, JaegerTrace};

    fn span(
        span_id: &str,
        name: &str,
        start_time: u64,
        duration: u64,
        references: Vec<JaegerReference>,
    ) -> JaegerSpan {
        JaegerSpan {
            trace_id: "trace".to_string(),
            span_id: span_id.to_string(),
            operation_name: name.to_string(),
            start_time,
            duration,
            references,
            tags: vec![],
        }
    }

    fn reference(ref_type: &str, span_id: &str) -> JaegerReference {
        JaegerReference {
            ref_type: ref_type.to_string(),
            trace_id: "trace".to_string(),
            span_id: span_id.to_string(),
        }
    }

    /// A span may carry both a CHILD_OF parent and FOLLOWS_FROM links, and Jaeger
    /// does not guarantee the order. Only the CHILD_OF reference is the parent.
    #[test]
    fn parent_span_id_ignores_non_child_of_references() {
        let with_link_first = span(
            "b",
            "child",
            0,
            1,
            vec![
                reference("FOLLOWS_FROM", "link"),
                reference("CHILD_OF", "a"),
            ],
        );
        assert_eq!(with_link_first.parent_span_id(), Some("a"));

        let only_link = span("b", "root", 0, 1, vec![reference("FOLLOWS_FROM", "link")]);
        assert_eq!(
            only_link.parent_span_id(),
            None,
            "a linked root has no parent"
        );
    }

    /// Acceptance criterion: no span may escape its parent's window. A child that
    /// starts before or ends after its parent means the parent does not actually
    /// contain the operation it appears to contain.
    #[test]
    fn spans_outliving_parent_flags_children_that_escape_their_parent() {
        let trace = JaegerTrace {
            trace_id: "trace".to_string(),
            spans: vec![
                span("a", "parent", 1_000, 1_000, vec![]),
                span(
                    "b",
                    "contained",
                    1_100,
                    500,
                    vec![reference("CHILD_OF", "a")],
                ),
                span(
                    "c",
                    "ends_late",
                    1_500,
                    2_000,
                    vec![reference("CHILD_OF", "a")],
                ),
                span(
                    "d",
                    "starts_early",
                    500,
                    200,
                    vec![reference("CHILD_OF", "a")],
                ),
                // A linked root is not a child, so it is exempt.
                span(
                    "e",
                    "linked",
                    9_000,
                    9_000,
                    vec![reference("FOLLOWS_FROM", "a")],
                ),
            ],
        };

        let offenders: Vec<&str> = trace
            .spans_outliving_parent()
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert!(offenders.contains(&"ends_late"), "got {offenders:?}");
        assert!(offenders.contains(&"starts_early"), "got {offenders:?}");
        assert!(!offenders.contains(&"contained"), "got {offenders:?}");
        assert!(
            !offenders.contains(&"linked"),
            "a linked root is not a child and must not be flagged; got {offenders:?}"
        );
    }

    #[test]
    fn spans_outliving_parent_ignores_parents_outside_the_trace() {
        let trace = JaegerTrace {
            trace_id: "trace".to_string(),
            spans: vec![span(
                "b",
                "remote_child",
                1_000,
                10_000,
                vec![reference("CHILD_OF", "not-in-this-trace")],
            )],
        };

        assert!(
            trace.spans_outliving_parent().is_empty(),
            "cannot compare against a parent that was not exported here"
        );
    }
}
