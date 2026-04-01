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

use golem_common::base_model::agent::{DataValue, ParsedAgentId};
use golem_common::model::component::ComponentDto;
use golem_common::model::{AgentId, IdempotencyKey};
use golem_test_framework::benchmark::{BenchmarkRecorder, ResultKey};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::Value;
use opentelemetry::propagation::TextMapPropagator;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, Request};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use tracing::{Instrument, info, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub mod cold_start_unknown;
pub mod durability_overhead;
pub mod latency;
pub mod sleep;
pub mod throughput;

/// Injects the current tracing span's OpenTelemetry trace context (traceparent/tracestate)
/// into a reqwest Request's headers so that downstream services can link their
/// spans to the benchmark's trace.
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

#[derive(Debug)]
pub struct InvokeResult {
    pub value: Vec<Value>,
    pub retries: usize,
    pub timeouts: usize,
    pub accumulated_time: Duration,
}

impl InvokeResult {
    pub fn record(&self, recorder: &BenchmarkRecorder, prefix: &str, agent_id: &str) {
        recorder.duration(&format!("{prefix}invocation").into(), self.accumulated_time);
        recorder.duration(
            &ResultKey::secondary(format!("{prefix}worker-{agent_id}")),
            self.accumulated_time,
        );
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
    params: DataValue,
) -> InvokeResult {
    async {
        const TIMEOUT: Duration = Duration::from_secs(180);
        const RETRY_DELAY: Duration = Duration::from_millis(100);

        let key = IdempotencyKey::fresh();

        let mut accumulated_time = Duration::from_secs(0);
        let mut retries = 0;
        let mut timeouts = 0;

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
                    };
                }
                Ok(Err(e)) => {
                    println!("Invocation failed, retrying: {e:?}");
                    retries += 1;
                    accumulated_time += duration;
                    tokio::time::sleep(RETRY_DELAY).await;
                    user.deps.ensure_all_deps_running().await;
                }
                Err(e) => {
                    // timeout
                    // not counting timeouts into the accumulated time
                    timeouts += 1;
                    println!("Invocation timed out, retrying: {e:?}");
                    user.deps.ensure_all_deps_running().await;
                }
            }
        }
    }
    .instrument(tracing::info_span!("invoke_agent", method = method_name))
    .await
}

pub async fn invoke_and_await_http(client: Client, request: impl Fn() -> Request) -> InvokeResult {
    async {
        const TIMEOUT: Duration = Duration::from_secs(180);
        const RETRY_DELAY: Duration = Duration::from_millis(100);

        let key = IdempotencyKey::fresh();

        let mut accumulated_time = Duration::from_secs(0);
        let mut retries = 0;
        let mut timeouts = 0;

        loop {
            let start = SystemTime::now();
            let mut req = request();
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
                            value: vec![Value::String(body)],
                            retries,
                            timeouts,
                            accumulated_time,
                        };
                    } else {
                        // non-200 status
                        println!("Invocation returned with status {}, retrying", r.status());
                        retries += 1;
                        let duration = start.elapsed().expect("SystemTime elapsed failed");
                        accumulated_time += duration;
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
                Ok(Err(e)) => {
                    // reqwest error
                    println!("Invocation failed, retrying: {e:?}");
                    retries += 1;
                    let duration = start.elapsed().expect("SystemTime elapsed failed");
                    accumulated_time += duration;
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                Err(e) => {
                    // timeout
                    // not counting timeouts into the accumulated time
                    timeouts += 1;
                    println!("Invocation timed out, retrying: {e:?}");
                }
            }
        }
    }
    .instrument(tracing::info_span!("invoke_http"))
    .await
}
