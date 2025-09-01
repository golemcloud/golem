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

use crate::{Deps, Tracing};
use assert2::check;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use golem_client::model::{
    ApiDefinitionInfo, ApiDeploymentRequest, ApiSite, GatewayBindingComponent, GatewayBindingData,
    GatewayBindingType, HttpApiDefinitionRequest, MethodPattern, RouteRequestData,
};
use golem_common::model::component_metadata::{
    DynamicLinkedInstance, DynamicLinkedWasmRpc, WasmRpcTarget,
};
use golem_common::model::invocation_context::{SpanId, TraceId};
use golem_common::model::ComponentType;
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use reqwest::header::HeaderValue;
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use test_r::{inherit_test_dep, test, timeout};
use tracing::{info, Instrument};
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(Deps);

#[test]
#[tracing::instrument]
#[timeout(120000)]
#[allow(clippy::await_holding_lock)]
async fn invocation_context_test(deps: &Deps) {
    let admin = deps.admin().await;
    let host_http_port = 8588;

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

            let listener = tokio::net::TcpListener::bind(
                format!("0.0.0.0:{host_http_port}")
                    .parse::<SocketAddr>()
                    .unwrap(),
            )
            .await
            .unwrap();
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let mut env = HashMap::new();
    env.insert("PORT".to_string(), host_http_port.to_string());

    let (component_id, component_name) = admin
        .component("golem_ictest")
        .with_dynamic_linking(&[(
            "golem:ictest-client/golem-ictest-client",
            DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                targets: HashMap::from_iter(vec![(
                    "golem-ictest-api".to_string(),
                    WasmRpcTarget {
                        interface_name: "golem:ictest-exports/golem-ictest-api".to_string(),
                        component_name: "golem:ictest".to_string(),
                        component_type: ComponentType::Durable,
                    },
                )]),
            }),
        )])
        .store_and_get_name()
        .await;

    let _worker_id = admin
        .start_worker_with(&component_id, "w1", vec![], env.clone(), vec![])
        .await;

    let api_definition_id = Uuid::new_v4().to_string();

    let request = HttpApiDefinitionRequest {
        id: api_definition_id.clone(),
        version: "1".to_string(),
        draft: true,
        security: None,
        routes: vec![RouteRequestData {
            method: MethodPattern::Post,
            path: "/test-path-1/{name}".to_string(),
            binding: GatewayBindingData {
                component: Some(GatewayBindingComponent {
                    name: component_name.0,
                    version: Some(0),
                }),
                worker_name: None,
                response: Some(
                    r#"
                        let worker = instance("w1");
                        worker.test1();
                        {
                            body: "ok",
                            status: 200,
                            headers: { Content-Type: "application/json" }
                        }
                    "#
                    .to_string(),
                ),
                idempotency_key: None,
                binding_type: Some(GatewayBindingType::Default),
                invocation_context: Some(
                    r#"
                        {
                            name: request.path.name,
                            source: "rib"
                        }
                    "#
                    .to_string(),
                ),
            },
            security: None,
        }],
    };

    let project_id = admin.default_project().await;

    let _ = deps
        .worker_service()
        .create_api_definition(&admin.token, &project_id, &request)
        .await
        .unwrap();

    let request = ApiDeploymentRequest {
        project_id: project_id.0,
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition_id,
            version: "1".to_string(),
        }],
        site: ApiSite {
            host: format!(
                "localhost:{}",
                deps.worker_service().public_custom_request_port()
            ),
            subdomain: None,
        },
    };

    let _ = deps
        .worker_service()
        .create_or_update_api_deployment(&admin.token, request.clone())
        .await
        .unwrap();

    let trace_id = TraceId::generate();
    let parent_span_id = SpanId::generate();
    let trace_state = "xxx=yyy".to_string();

    let client = Client::builder().build().unwrap();
    let response = client
        .post(format!(
            "http://localhost:{}/test-path-1/vigoo",
            deps.worker_service().public_custom_request_port()
        ))
        .header("traceparent", format!("00-{trace_id}-{parent_span_id}-01"))
        .header("tracestate", trace_state.clone())
        .send()
        .await
        .unwrap();

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

    let status = response.status();

    check!(status.is_success());
    check!(traceparents.len() == 3);
    check!(traceparents.iter().all(|tp| tp.is_some()));

    check!(tracestates.len() == 3);
    let trace_state_clone = trace_state.clone();
    check!(tracestates
        .iter()
        .all(move |tp| tp == &Some(HeaderValue::from_str(&trace_state_clone).unwrap())));

    check!(
        dump[0]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 3
    ); // root, gateway, invoke-exported-function
    check!(
        dump[1]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 6
    ); // + rpc-connection, rpc-invocation, invoke-exported-function
    check!(
        dump[2]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 11
    ); // + custom1, custom2, rpc-connection, rpc-invocation, invoke-exported-function
    check!(
        dump[2]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()[9] // last but one
            .as_object()
            .unwrap()
            .get("name")
            == Some(&Value::String("vigoo".to_string()))
    ); // coming from the custom invocation context rib
    check!(
        dump[2]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()[9] // last but one
            .as_object()
            .unwrap()
            .get("source")
            == Some(&Value::String("rib".to_string()))
    ); // coming from the custom invocation context rib
    check!(
        dump[2]
            .as_object()
            .unwrap()
            .get("spans")
            .unwrap()
            .as_array()
            .unwrap()[9] // last but one
            .as_object()
            .unwrap()
            .get("request.uri")
            == Some(&Value::String("/test-path-1/vigoo".to_string()))
    ); // coming from incoming http request
    check!(
        dump[2].as_object().unwrap().get("trace_id") == Some(&Value::String(format!("{trace_id}")))
    ); // coming from the custom invocation context rib
}
