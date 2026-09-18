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

use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentDto;
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_test_framework::benchmark::{BenchmarkRecorder, ResultKey};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::TestDsl;
use opentelemetry::propagation::TextMapPropagator;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, Request};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use tracing::{Instrument, info, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub mod cleanup;
pub mod cold_start_unknown;
pub mod durability_overhead;
pub mod idempotency_key;
pub mod latency;
pub mod sleep;
pub mod streaming;
pub mod throughput;

// Re-export cleanup helpers so callers can use the flat `benchmarks::*` path.
pub use cleanup::{cleanup_account, cleanup_env_and_app, cleanup_user_state};

/// Injects the current tracing span's OpenTelemetry trace context
/// (traceparent/tracestate) into a reqwest Request's headers so that
/// downstream services can link their spans to the benchmark's trace.
fn inject_trace_context(request: &mut Request) {
    let current_span = tracing::Span::current();
    let otel_context = current_span.context();

    let propagator = opentelemetry_sdk::propagation::TraceContextPropagator::new();
    let mut carrier = HashMap::new();
    propagator.inject_context(&otel_context, &mut carrier);

    for (key, value) in carrier {
        if let (Ok(name), Ok(val)) = (HeaderName::from_str(&key), HeaderValue::from_str(&value)) {
            request.headers_mut().insert(name, val);
        }
    }
}

pub async fn delete_workers(
    user: &TestUserContext<BenchmarkTestDependencies>,
    agent_ids: &[AgentId],
) {
    info!("Deleting {} workers...", agent_ids.len());
    for agent_id in agent_ids {
        if let Err(err) = user.delete_worker(agent_id).await {
            warn!("Failed to delete worker: {:?}", err);
        }
    }
    info!("Deleting {} workers completed", agent_ids.len());
}

/// Longest failure message kept per failed attempt. Response bodies can be
/// large; the report needs enough to diagnose, not the whole payload.
const MAX_FAILURE_MESSAGE_LEN: usize = 500;

fn failure_message(message: String) -> String {
    if message.len() <= MAX_FAILURE_MESSAGE_LEN {
        message
    } else {
        let mut end = MAX_FAILURE_MESSAGE_LEN;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}... ({} bytes total)", &message[..end], message.len())
    }
}

#[derive(Debug)]
pub struct InvokeResult {
    pub value: Vec<SchemaValue>,
    pub retries: usize,
    pub timeouts: usize,
    pub accumulated_time: Duration,
    /// One message per failed attempt (retry or timeout), in order.
    pub failures: Vec<String>,
}

impl InvokeResult {
    /// Records the invocation under `{prefix}invocation`.
    ///
    /// An invocation that needed a retry or timed out before succeeding is
    /// recorded under `{prefix}invocation-recovered` instead, and every failed
    /// attempt is recorded as a failure against `{prefix}invocation`. The
    /// primary series therefore only measures invocations that succeeded on
    /// the first attempt, and a run with failures says so in its report and
    /// results instead of folding the retry time into the measured numbers.
    pub fn record(&self, recorder: &BenchmarkRecorder, prefix: &str, agent_id: &str) {
        let invocation_key: ResultKey = format!("{prefix}invocation").into();
        let recovered = !self.failures.is_empty();
        let duration_key: ResultKey = if recovered {
            format!("{prefix}invocation-recovered").into()
        } else {
            invocation_key.clone()
        };
        let worker_key = if recovered {
            ResultKey::secondary(format!("{prefix}worker-{agent_id}-recovered"))
        } else {
            ResultKey::secondary(format!("{prefix}worker-{agent_id}"))
        };
        recorder.duration(&duration_key, self.accumulated_time);
        recorder.duration(&worker_key, self.accumulated_time);
        for failure in &self.failures {
            recorder.failure(&invocation_key, failure.clone());
        }
        recorder.count(
            &format!("{prefix}invocation-retries").into(),
            self.retries as u64,
        );
        recorder.count(
            &ResultKey::secondary(format!("{prefix}worker-{agent_id}-retries")),
            self.retries as u64,
        );
        recorder.count(
            &format!("{prefix}invocation-timeouts").into(),
            self.timeouts as u64,
        );
        recorder.count(
            &ResultKey::secondary(format!("{prefix}worker-{agent_id}-timeouts")),
            self.timeouts as u64,
        );
    }
}

pub async fn invoke_and_await_agent(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    method_name: &str,
    params: TypedSchemaValue,
) -> InvokeResult {
    async {
        const TIMEOUT: Duration = Duration::from_secs(180);
        const RETRY_DELAY: Duration = Duration::from_millis(100);

        let key = IdempotencyKey::fresh();

        let mut accumulated_time = Duration::from_secs(0);
        let mut retries = 0;
        let mut timeouts = 0;
        let mut failures = Vec::new();

        loop {
            let start = SystemTime::now();
            let result = tokio::time::timeout(
                TIMEOUT,
                user.invoke_and_await_agent_with_key(
                    component,
                    agent_id,
                    &key,
                    method_name,
                    params.clone(),
                ),
            )
            .await;
            let duration = start.elapsed().expect("SystemTime elapsed failed");

            match result {
                Ok(Ok(data_value)) => {
                    accumulated_time += duration;
                    let value = data_value
                        .into_return_value()
                        .map(|v| vec![v])
                        .unwrap_or_default();
                    break InvokeResult {
                        value,
                        retries,
                        timeouts,
                        accumulated_time,
                        failures,
                    };
                }
                Ok(Err(e)) => {
                    println!("Invocation failed, retrying: {e:?}");
                    retries += 1;
                    failures.push(failure_message(format!(
                        "{method_name} on {agent_id} failed: {e:?}"
                    )));
                    accumulated_time += duration;
                    tokio::time::sleep(RETRY_DELAY).await;
                    user.deps.ensure_all_deps_running().await;
                }
                Err(e) => {
                    // timeout
                    // not counting timeouts into the accumulated time
                    timeouts += 1;
                    failures.push(failure_message(format!(
                        "{method_name} on {agent_id} timed out after {TIMEOUT:?}: {e:?}"
                    )));
                    println!("Invocation timed out, retrying: {e:?}");
                    user.deps.ensure_all_deps_running().await;
                }
            }
        }
    }
    .instrument(tracing::info_span!("invoke_agent", method = method_name))
    .await
}

/// Whether a non-success HTTP status is worth retrying.
///
/// 5xx, 408 and 429 are transient: the same request can succeed later. Every
/// other 4xx is a permanent client error — the request is malformed, unroutable
/// or unauthorized, so replaying it byte-for-byte will fail identically forever.
/// Retrying those turns a config bug into an unbounded hang instead of an error.
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

pub async fn invoke_and_await_http(client: Client, request: impl Fn() -> Request) -> InvokeResult {
    async {
        const TIMEOUT: Duration = Duration::from_secs(180);
        const RETRY_DELAY: Duration = Duration::from_millis(100);
        /// Upper bound on retries for transient failures. At RETRY_DELAY this is
        /// ~30s of retrying before the benchmark gives up and reports the cause.
        const MAX_RETRIES: usize = 300;
        /// Timeouts are bounded separately: each one already costs TIMEOUT, so a
        /// retry budget of 300 would mean 15 hours of hanging.
        const MAX_TIMEOUTS: usize = 5;

        let key = IdempotencyKey::fresh();

        let mut accumulated_time = Duration::from_secs(0);
        let mut retries = 0;
        let mut timeouts = 0;
        let mut failures = Vec::new();

        loop {
            let start = SystemTime::now();
            let mut req = request();
            let url = req.url().clone();
            req.headers_mut().insert(
                HeaderName::from_str("Idempotency-Key").unwrap(),
                HeaderValue::from_str(&key.value).unwrap(),
            );
            inject_trace_context(&mut req);
            let result = tokio::time::timeout(TIMEOUT, client.execute(req)).await;

            match result {
                Ok(Ok(r)) => {
                    if r.status().is_success() {
                        let body = r.text().await.unwrap_or_default();
                        let duration = start.elapsed().expect("SystemTime elapsed failed");
                        accumulated_time += duration;

                        break InvokeResult {
                            value: vec![SchemaValue::String(body)],
                            retries,
                            timeouts,
                            accumulated_time,
                            failures,
                        };
                    } else {
                        // non-200 status. Read the body before deciding what to
                        // do with it: without it a failure is undiagnosable.
                        let status = r.status();
                        let body = r.text().await.unwrap_or_default();

                        if !is_retryable_status(status) {
                            panic!(
                                "Invocation failed permanently with status {status} for {url}; \
                                 not retrying. Response body: {body}"
                            );
                        }

                        retries += 1;
                        if retries > MAX_RETRIES {
                            panic!(
                                "Invocation still failing with status {status} for {url} after \
                                 {MAX_RETRIES} retries; giving up. Response body: {body}"
                            );
                        }

                        println!(
                            "Invocation returned with status {status} for {url} \
                             (retry {retries}/{MAX_RETRIES}), body: {body}"
                        );
                        failures.push(failure_message(format!(
                            "status {status} for {url}: {body}"
                        )));
                        let duration = start.elapsed().expect("SystemTime elapsed failed");
                        accumulated_time += duration;
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
                Ok(Err(e)) => {
                    // reqwest error
                    retries += 1;
                    if retries > MAX_RETRIES {
                        panic!(
                            "Invocation to {url} still failing after {MAX_RETRIES} retries; \
                             giving up. Last error: {e:?}"
                        );
                    }
                    println!("Invocation failed, retrying ({retries}/{MAX_RETRIES}): {e:?}");
                    failures.push(failure_message(format!("request to {url} failed: {e:?}")));
                    let duration = start.elapsed().expect("SystemTime elapsed failed");
                    accumulated_time += duration;
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                Err(e) => {
                    // timeout
                    // not counting timeouts into the accumulated time
                    timeouts += 1;
                    if timeouts > MAX_TIMEOUTS {
                        panic!(
                            "Invocation to {url} timed out {MAX_TIMEOUTS} times \
                             ({TIMEOUT:?} each); giving up. Last error: {e:?}"
                        );
                    }
                    println!("Invocation timed out, retrying ({timeouts}/{MAX_TIMEOUTS}): {e:?}");
                    failures.push(failure_message(format!(
                        "request to {url} timed out after {TIMEOUT:?}: {e:?}"
                    )));
                }
            }
        }
    }
    .instrument(tracing::info_span!("invoke_http"))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn result(failures: Vec<String>) -> InvokeResult {
        InvokeResult {
            value: vec![],
            retries: failures.len(),
            timeouts: 0,
            accumulated_time: Duration::from_millis(7),
            failures,
        }
    }

    #[test]
    fn clean_invocation_is_recorded_under_the_primary_key() {
        let recorder = BenchmarkRecorder::new();
        result(vec![]).record(&recorder, "hot-", "3");

        let durations = recorder.durations();
        assert_eq!(
            durations[&ResultKey::primary("hot-invocation")],
            vec![Duration::from_millis(7)]
        );
        assert_eq!(
            durations[&ResultKey::secondary("hot-worker-3")],
            vec![Duration::from_millis(7)]
        );
        assert!(!durations.contains_key(&ResultKey::primary("hot-invocation-recovered")));
        assert!(recorder.failures().is_empty());
    }

    #[test]
    fn recovered_invocation_is_kept_out_of_the_primary_series_and_its_failures_recorded() {
        let recorder = BenchmarkRecorder::new();
        result(vec!["status 503 for http://x: busy".to_string()]).record(&recorder, "hot-", "3");

        let durations = recorder.durations();
        assert!(!durations.contains_key(&ResultKey::primary("hot-invocation")));
        assert_eq!(
            durations[&ResultKey::primary("hot-invocation-recovered")],
            vec![Duration::from_millis(7)]
        );
        assert_eq!(
            durations[&ResultKey::secondary("hot-worker-3-recovered")],
            vec![Duration::from_millis(7)]
        );
        assert_eq!(
            recorder.failures()[&ResultKey::primary("hot-invocation")],
            vec!["status 503 for http://x: busy".to_string()]
        );
        assert_eq!(
            recorder.counts()[&ResultKey::primary("hot-invocation-retries")],
            vec![1]
        );
    }

    #[test]
    fn failure_messages_are_truncated_on_a_char_boundary() {
        let short = "x".repeat(MAX_FAILURE_MESSAGE_LEN);
        assert_eq!(failure_message(short.clone()), short);

        let long = format!(
            "{}é{}",
            "x".repeat(MAX_FAILURE_MESSAGE_LEN - 1),
            "y".repeat(10)
        );
        let truncated = failure_message(long.clone());
        assert!(truncated.starts_with(&"x".repeat(MAX_FAILURE_MESSAGE_LEN - 1)));
        assert!(truncated.ends_with(&format!("... ({} bytes total)", long.len())));
        assert!(!truncated.contains('é'));
    }
}
