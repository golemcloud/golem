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

use crate::Tracing;
use golem_client::api::RegistryServiceClient;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_common::model::invocation_context::{SpanId, TraceId};
use golem_test_framework::components::jaeger::{DockerJaeger, Jaeger, JaegerQueryClient};
use golem_test_framework::components::otel_collector::{
    DockerOtelCollector, OtelCollector, OtlpKeyValue, OtlpTraceRecord, wait_for_otlp_logs_matching,
    wait_for_otlp_metrics, wait_for_otlp_traces,
};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use reqwest::Client;
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, test, test_dep, timeout};
use tracing::info;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test_dep(scope = PerWorker)]
async fn create_jaeger(_tracing: &Tracing) -> DockerJaeger {
    DockerJaeger::new().await
}

#[test_dep(scope = PerWorker)]
async fn create_otel_collector(_tracing: &Tracing) -> DockerOtelCollector {
    DockerOtelCollector::new().await
}

async fn find_otlp_plugin_grant(
    client: &impl RegistryServiceClient,
    environment_id: &EnvironmentId,
) -> anyhow::Result<EnvironmentPluginGrantId> {
    let grants = client
        .list_environment_environment_plugin_grants(&environment_id.0)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to list plugin grants: {e}"))?;

    let grant = grants
        .values
        .iter()
        .find(|g| g.plugin.name == "golem-otlp-exporter")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "golem-otlp-exporter plugin grant not found. Available grants: {:?}",
                grants
                    .values
                    .iter()
                    .map(|g| &g.plugin.name)
                    .collect::<Vec<_>>()
            )
        })?;

    Ok(grant.id)
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn otlp_basic_trace_export(
    deps: &EnvBasedTestDependencies,
    jaeger: &DockerJaeger,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let otlp_grant_id = find_otlp_plugin_grant(&client, &env.id).await?;
    info!("Found OTLP plugin grant: {otlp_grant_id:?}");

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("endpoint".to_string(), jaeger.otlp_http_endpoint());

    let _component = user
        .component(&env.id, "golem_it_agent_invocation_context_release")
        .name("golem-it:agent-invocation-context")
        .with_parametrized_plugin("InvocationContextAgent", &otlp_grant_id, 0, plugin_params)
        .store()
        .await?;

    let domain = user.register_domain(&env.id).await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        scheme: Default::default(),
        domain: domain.clone(),
        agents: BTreeMap::from_iter([(
            AgentTypeName("InvocationContextAgent".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_prefix: HttpApiDeploymentCreation::default_webhooks_prefix(),
        openapi_endpoint_prefix: HttpApiDeploymentCreation::default_openapi_endpoint_prefix(),
    };

    client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    user.deploy_environment(env.id).await?;

    let trace_id = TraceId::generate();
    let parent_span_id = SpanId::generate();

    let http_client = Client::builder().build().unwrap();
    let response = http_client
        .post(format!(
            "http://localhost:{}/otlp-test/test-path-1",
            deps.worker_service().custom_request_port()
        ))
        .header("host", domain.0.clone())
        .header("traceparent", format!("00-{trace_id}-{parent_span_id}-01"))
        .header("tracestate", "test=value")
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;
    info!("HTTP response: {status} - {body}");

    let jaeger_client = JaegerQueryClient::new(&jaeger.query_url());

    let jaeger_trace_id = format!("{trace_id}");
    info!("Waiting for trace {jaeger_trace_id} in Jaeger");

    let parent_span_id_str = format!("{parent_span_id}");
    let external_parents = HashSet::from([parent_span_id_str.as_str()]);
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut complete_span_count = None;
    let trace = loop {
        if let Some(trace) = jaeger_client.get_trace(&jaeger_trace_id).await? {
            let span_count = trace.spans.len();
            if span_count >= 5 && trace.disconnected_spans(&external_parents).is_empty() {
                if complete_span_count == Some(span_count) {
                    break trace;
                }
                complete_span_count = Some(span_count);
            } else {
                complete_span_count = None;
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("Timed out waiting for complete trace {jaeger_trace_id}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };

    info!("Found trace with {} spans", trace.spans.len());

    assert!(
        !trace.spans.is_empty(),
        "Trace should have at least one span"
    );
    assert_eq!(trace.trace_id, jaeger_trace_id);

    trace.dump_spans(&external_parents);

    let unknown = trace.unknown_name_spans();
    assert!(
        unknown.is_empty(),
        "Found spans with 'unknown' name: {unknown:?}"
    );

    let disconnected = trace.disconnected_spans(&external_parents);
    assert!(
        disconnected.is_empty(),
        "Found disconnected spans: {disconnected:?}"
    );

    let errors = trace.error_spans();
    assert!(
        errors.is_empty(),
        "Found spans with ERROR status: {errors:?}"
    );

    // A span must not escape its parent's time window. A child that outlives its
    // parent means the parent does not contain the operation it appears to
    // contain, which makes critical-path analysis meaningless.
    let outliving = trace.spans_outliving_parent();
    assert!(
        outliving.is_empty(),
        "Found spans outliving their parent (child, parent): {outliving:?}"
    );

    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn otlp_all_signals_export(
    deps: &EnvBasedTestDependencies,
    otel_collector: &DockerOtelCollector,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let otlp_grant_id = find_otlp_plugin_grant(&client, &env.id).await?;
    info!("Found OTLP plugin grant: {otlp_grant_id:?}");

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("endpoint".to_string(), otel_collector.otlp_http_endpoint());
    plugin_params.insert("signals".to_string(), "traces,logs,metrics".to_string());
    plugin_params.insert("service-name-mode".to_string(), "agent-type".to_string());

    let _component = user
        .component(&env.id, "golem_it_agent_invocation_context_release")
        .name("golem-it:agent-invocation-context")
        .with_parametrized_plugin("InvocationContextAgent", &otlp_grant_id, 0, plugin_params)
        .store()
        .await?;

    let domain = user.register_domain(&env.id).await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        scheme: Default::default(),
        domain: domain.clone(),
        agents: BTreeMap::from_iter([(
            AgentTypeName("InvocationContextAgent".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_prefix: HttpApiDeploymentCreation::default_webhooks_prefix(),
        openapi_endpoint_prefix: HttpApiDeploymentCreation::default_openapi_endpoint_prefix(),
    };

    client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    user.deploy_environment(env.id).await?;

    let trace_id = TraceId::generate();
    let parent_span_id = SpanId::generate();

    let http_client = Client::builder().build().unwrap();
    let response = http_client
        .post(format!(
            "http://localhost:{}/otlp-test/test-path-1",
            deps.worker_service().custom_request_port()
        ))
        .header("host", domain.0.clone())
        .header("traceparent", format!("00-{trace_id}-{parent_span_id}-01"))
        .header("tracestate", "test=value")
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;
    info!("HTTP response: {status} - {body}");
    assert!(
        status.is_success(),
        "HTTP invocation failed: {status} - {body}"
    );

    let output_dir = otel_collector.output_dir();

    let trace_id = trace_id.to_string();
    let parent_span_id = parent_span_id.to_string();
    let traces = wait_for_otlp_traces(output_dir, Duration::from_secs(90), |records| {
        invocation_context_trace_ready(records, &trace_id, &parent_span_id)
    })
    .await?;
    let trace: Vec<_> = traces
        .iter()
        .filter(|record| record.span.trace_id == trace_id)
        .collect();
    let span_ids: HashSet<_> = trace
        .iter()
        .map(|record| record.span.span_id.as_str())
        .collect();

    for record in &trace {
        assert_eq!(
            attribute(&record.resource.attributes, "service.name"),
            Some("InvocationContextAgent")
        );
        assert!(
            attribute(&record.resource.attributes, "golem.agent.id")
                .is_some_and(|id| id.contains("InvocationContextAgent(")),
            "resource should retain the full agent identity: {record:?}"
        );
        assert_ne!(
            record.span.status.as_ref().map(|status| status.code),
            Some(2)
        );
        assert!(
            record.span.start_time_unix_nano.parse::<u128>()?
                <= record.span.end_time_unix_nano.parse::<u128>()?,
            "span has a negative duration: {record:?}"
        );
        assert_eq!(record.span.trace_state.as_deref(), Some("test=value"));
        if let Some(parent) = record.span.parent_span_id.as_deref() {
            assert!(
                parent == parent_span_id || span_ids.contains(parent),
                "span has a parent outside the exported trace: {record:?}"
            );
        }
    }

    for (name, kind) in [
        ("custom", 1),
        ("custom2", 1),
        ("rpc-connection", 3),
        ("rpc-invocation", 3),
    ] {
        assert!(
            trace
                .iter()
                .any(|record| record.span.name == name && record.span.kind == kind),
            "missing {name} span with OTLP kind {kind}: {trace:?}"
        );
    }

    let custom = trace
        .iter()
        .find(|record| record.span.name == "custom")
        .unwrap();
    let custom2 = trace
        .iter()
        .find(|record| record.span.name == "custom2")
        .unwrap();
    assert_eq!(
        custom2.span.parent_span_id.as_deref(),
        Some(custom.span.span_id.as_str())
    );
    assert_eq!(attribute(&custom.span.attributes, "x"), Some("1"));
    assert_eq!(attribute(&custom.span.attributes, "y"), Some("2"));
    assert_eq!(attribute(&custom2.span.attributes, "z"), Some("3"));

    for rpc in trace
        .iter()
        .filter(|record| record.span.name == "rpc-invocation")
    {
        let parent = rpc.span.parent_span_id.as_deref().unwrap();
        assert!(
            trace
                .iter()
                .any(|record| record.span.span_id == parent && record.span.name == "rpc-connection"),
            "RPC invocation is not parented by its connection: {rpc:?}"
        );
        assert!(attribute(&rpc.span.attributes, "function_name").is_some());
    }

    info!("Waiting for OTLP log records");
    let logs = wait_for_otlp_logs_matching(output_dir, Duration::from_secs(90), |records| {
        records
            .iter()
            .filter(|record| log_body(record).is_some_and(|body| body.contains("Sending context")))
            .count()
            >= 3
    })
    .await?;
    info!("Collected {} OTLP log records", logs.len());

    let has_sending_context = logs.iter().any(|r| {
        r.body
            .as_ref()
            .and_then(|b| b.string_value.as_deref())
            .is_some_and(|s| s.contains("Sending context"))
    });
    assert!(
        has_sending_context,
        "Expected a log record containing 'Sending context', got: {logs:?}"
    );

    let has_stdout_severity = logs
        .iter()
        .any(|r| r.severity_text.as_deref() == Some("STDOUT"));
    assert!(
        has_stdout_severity,
        "Expected at least one log record with severity_text=STDOUT (from println!), got: {logs:?}"
    );

    for log in logs
        .iter()
        .filter(|record| log_body(record).is_some_and(|body| body.contains("Sending context")))
    {
        assert_eq!(log.trace_id.as_deref(), Some(trace_id.as_str()));
        assert!(
            log.span_id
                .as_deref()
                .is_some_and(|span_id| span_ids.contains(span_id)),
            "persisted log was not correlated with its emit-time span: {log:?}"
        );
    }

    // Wait for metric records — the invocation produces many metrics from
    // Create, AgentInvocationStarted/Finished, Start/End (real host calls),
    // GrowMemory, Log entries.
    info!("Waiting for OTLP metric records");
    let metrics = wait_for_otlp_metrics(output_dir, 5, Duration::from_secs(90)).await?;
    info!("Collected {} OTLP metric records", metrics.len());

    let metric_names: HashSet<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
    info!("Unique metric names: {metric_names:?}");

    let expected_metrics = [
        "golem.invocation.count",
        "golem.invocation.fuel_consumed",
        "golem.host_call.count",
        "golem.log.count",
    ];
    for expected in &expected_metrics {
        assert!(
            metric_names.contains(expected),
            "Expected metric '{expected}' not found. Got: {metric_names:?}"
        );
    }

    Ok(())
}

fn invocation_context_trace_ready(
    records: &[OtlpTraceRecord],
    trace_id: &str,
    parent_span_id: &str,
) -> bool {
    let trace: Vec<_> = records
        .iter()
        .filter(|record| record.span.trace_id == trace_id)
        .collect();
    let span_ids: HashSet<_> = trace
        .iter()
        .map(|record| record.span.span_id.as_str())
        .collect();
    ["test1", "test2", "test3"].iter().all(|method| {
        trace.iter().any(|record| {
            record.span.name == "invoke-exported-function"
                && attribute(&record.span.attributes, "function_name") == Some(*method)
        })
    }) && [
        ("custom", 1),
        ("custom2", 1),
        ("rpc-connection", 2),
        ("rpc-invocation", 2),
    ]
    .iter()
    .all(|(name, count)| {
        trace
            .iter()
            .filter(|record| record.span.name == *name)
            .count()
            >= *count
    }) && trace.iter().all(|record| {
        record
            .span
            .parent_span_id
            .as_deref()
            .is_none_or(|parent| parent == parent_span_id || span_ids.contains(parent))
    })
}

#[test]
fn trace_readiness_waits_for_leaf_method_not_initializations() {
    fn record(id: &str, parent: &str, name: &str, method: &str) -> OtlpTraceRecord {
        OtlpTraceRecord {
            resource: Default::default(),
            span: serde_json::from_value(serde_json::json!({
                "traceId": "trace",
                "spanId": id,
                "parentSpanId": parent,
                "name": name,
                "kind": 1,
                "startTimeUnixNano": "1",
                "endTimeUnixNano": "2",
                "attributes": [{"key": "function_name", "value": {"stringValue": method}}]
            }))
            .unwrap(),
        }
    }

    let mut records = vec![
        record("i1", "external", "invoke-exported-function", "initialize"),
        record("t1", "external", "invoke-exported-function", "test1"),
        record("c1", "t1", "rpc-connection", ""),
        record("r1", "c1", "rpc-invocation", ""),
        record("i2", "r1", "invoke-exported-function", "initialize"),
        record("t2", "r1", "invoke-exported-function", "test2"),
        record("s1", "t2", "custom", ""),
        record("s2", "s1", "custom2", ""),
        record("c2", "s2", "rpc-connection", ""),
        record("r2", "c2", "rpc-invocation", ""),
        record("i3", "r2", "invoke-exported-function", "initialize"),
    ];
    assert!(!invocation_context_trace_ready(
        &records, "trace", "external"
    ));

    let mut leaf = record("t3", "r2", "invoke-exported-function", "test3");
    leaf.span.trace_id = "other-trace".to_string();
    records.push(leaf);
    assert!(!invocation_context_trace_ready(
        &records, "trace", "external"
    ));

    records.last_mut().unwrap().span.trace_id = "trace".to_string();
    assert!(invocation_context_trace_ready(
        &records, "trace", "external"
    ));

    records.last_mut().unwrap().span.parent_span_id = Some("missing-parent".to_string());
    assert!(!invocation_context_trace_ready(
        &records, "trace", "external"
    ));
}

fn attribute<'a>(attributes: &'a [OtlpKeyValue], key: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| value.string_value.as_deref())
}

fn log_body(
    record: &golem_test_framework::components::otel_collector::OtlpLogRecord,
) -> Option<&str> {
    record.body.as_ref()?.string_value.as_deref()
}
