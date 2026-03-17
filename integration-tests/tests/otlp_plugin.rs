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
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use reqwest::Client;
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;
use test_r::{inherit_test_dep, test, test_dep, timeout};
use tracing::info;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test_dep]
async fn create_jaeger(_tracing: &Tracing) -> DockerJaeger {
    DockerJaeger::new().await
}

async fn find_otlp_plugin_grant(
    client: &impl RegistryServiceClient,
    environment_id: &EnvironmentId,
) -> anyhow::Result<EnvironmentPluginGrantId> {
    let grants = client
        .list_environment_plugin_grants(&environment_id.0)
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
        .component(&env.id, "golem_it_agent_invocation_context")
        .name("golem-it:agent-invocation-context")
        .with_parametrized_plugin(&otlp_grant_id, 0, plugin_params)
        .store()
        .await?;

    let domain = user.register_domain(&env.id).await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        agents: BTreeMap::from_iter([(
            AgentTypeName("InvocationContextAgent".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
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

    let trace = jaeger_client
        .wait_for_trace_with_min_spans(&jaeger_trace_id, 5, Duration::from_secs(90))
        .await?;

    info!("Found trace with {} spans", trace.spans.len());

    assert!(!trace.spans.is_empty(), "Trace should have at least one span");
    assert_eq!(trace.trace_id, jaeger_trace_id);

    let parent_span_id_str = format!("{parent_span_id}");
    let external_parents = HashSet::from([parent_span_id_str.as_str()]);
    trace.dump_spans(&external_parents);

    let unknown = trace.unknown_name_spans();
    assert!(unknown.is_empty(), "Found spans with 'unknown' name: {unknown:?}");

    let disconnected = trace.disconnected_spans(&external_parents);
    assert!(disconnected.is_empty(), "Found disconnected spans: {disconnected:?}");

    let errors = trace.error_spans();
    assert!(errors.is_empty(), "Found spans with ERROR status: {errors:?}");

    Ok(())
}
