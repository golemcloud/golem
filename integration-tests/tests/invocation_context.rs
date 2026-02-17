// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use assert2::assert;
use assert2::check;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use golem_client::api::RegistryServiceClient;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_common::model::invocation_context::{SpanId, TraceId};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use reqwest::header::HeaderValue;
use reqwest::Client;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};
use tracing::{info, Instrument};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout("4m")]
#[allow(clippy::await_holding_lock)]
async fn invocation_context_test(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let contexts = Arc::new(Mutex::new(Vec::new()));
    let contexts_clone = contexts.clone();

    let traceparents = Arc::new(Mutex::new(Vec::new()));
    let traceparents_clone = traceparents.clone();

    let tracestates = Arc::new(Mutex::new(Vec::new()));
    let tracestates_clone = tracestates.clone();

    let http_server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/invocation-context",
                post(move |headers: HeaderMap, body: Json<Value>| async move {
                    contexts_clone.lock().unwrap().push(body.0);
                    traceparents_clone
                        .lock()
                        .unwrap()
                        .push(headers.get("traceparent").cloned());
                    tracestates_clone
                        .lock()
                        .unwrap()
                        .push(headers.get("tracestate").cloned());
                    "ok"
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    user.component(&env.id, "golem_it_agent_invocation_context")
        .name("golem-it:agent-invocation-context")
        .with_env(vec![("PORT".to_string(), host_http_port.to_string())])
        .store()
        .await?;

    let domain = user.register_domain(&env.id).await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        agents: BTreeMap::from_iter([(
            AgentTypeName("invocation-context-agent".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )]),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    user.deploy_environment(&env.id).await?;

    let trace_id = TraceId::generate();
    let parent_span_id = SpanId::generate();
    let trace_state = "xxx=yyy".to_string();

    let client = Client::builder().build().unwrap();
    let response = client
        .post(format!(
            "http://localhost:{}/vigoo/test-path-1",
            deps.worker_service().custom_request_port()
        ))
        .header("host", domain.0)
        .header("traceparent", format!("00-{trace_id}-{parent_span_id}-01"))
        .header("tracestate", trace_state.clone())
        .send()
        .await
        .unwrap();

    let status = response.status();
    info!("Response: {status} - {}", response.text().await.unwrap());

    let start = std::time::Instant::now();
    loop {
        let contexts = contexts.lock().unwrap();
        if contexts.len() == 3 {
            break;
        }
        drop(contexts);

        if start.elapsed().as_secs() > 30 {
            check!(false, "Timeout waiting for contexts");
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }

    let dump: Vec<_> = contexts.lock().unwrap().drain(..).collect();
    info!("{:#?}", dump);

    http_server.abort();

    let traceparents = traceparents.lock().unwrap();
    let tracestates = tracestates.lock().unwrap();

    assert!(status.is_success());
    assert_eq!(traceparents.len(), 3);
    assert!(traceparents.iter().all(|tp| tp.is_some()));

    assert_eq!(tracestates.len(), 3);
    let trace_state_clone = trace_state.clone();
    assert!(tracestates
        .iter()
        .all(move |tp| tp == &Some(HeaderValue::from_str(&trace_state_clone).unwrap())));

    assert_eq!(
        dump[0]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        3
    ); // root, gateway, invoke-exported-function
    assert_eq!(
        dump[1]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        6
    ); // + rpc-connection, rpc-invocation, invoke-exported-function
    assert_eq!(
        dump[2]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        11
    ); // + custom1, custom2, rpc-connection, rpc-invocation, invoke-exported-function
    assert_eq!(
        dump[2]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()[9] // last but one
            .as_object()
            .unwrap()
            .get("request.uri"),
        Some(&Value::String("/vigoo/test-path-1".to_string()))
    );
    assert_eq!(
        dump[2].as_object().unwrap().get("trace_id"),
        Some(&Value::String(format!("{trace_id}")))
    );

    Ok(())
}
