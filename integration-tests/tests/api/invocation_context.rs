// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::Tracing;
use assert2::check;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use golem_api_grpc::proto::golem::apidefinition::v1::{
    api_definition_request, create_api_definition_request, ApiDefinitionRequest,
    CreateApiDefinitionRequest,
};
use golem_api_grpc::proto::golem::apidefinition::{
    ApiDefinitionId, GatewayBinding, GatewayBindingType, HttpApiDefinition, HttpMethod, HttpRoute,
};
use golem_api_grpc::proto::golem::component::VersionedComponentId;
use golem_api_grpc::proto::golem::rib::Expr;
use golem_client::model::{ApiDefinitionInfo, ApiDeploymentRequest, ApiSite};
use golem_common::model::component_metadata::{DynamicLinkedInstance, DynamicLinkedWasmRpc};
use golem_common::model::invocation_context::{SpanId, TraceId};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
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
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout(120000)]
#[allow(clippy::await_holding_lock)]
async fn invocation_context_test(deps: &EnvBasedTestDependencies) {
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
                format!("0.0.0.0:{}", host_http_port)
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

    let component_id = deps
        .component("golem_ictest")
        .with_dynamic_linking(&[(
            "golem:ictest-client/golem-ictest-client",
            DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                target_interface_name: HashMap::from_iter(vec![(
                    "golem-ictest-api".to_string(),
                    "golem:ictest-exports/golem-ictest-api".to_string(),
                )]),
            }),
        )])
        .store()
        .await;
    let _worker_id = deps
        .start_worker_with(&component_id, "w1", vec![], env.clone())
        .await;

    let api_definition_id = ApiDefinitionId {
        value: Uuid::new_v4().to_string(),
    };
    let request = ApiDefinitionRequest {
        id: Some(api_definition_id.clone()),
        version: "1".to_string(),
        draft: true,
        definition: Some(api_definition_request::Definition::Http(
            HttpApiDefinition {
                routes: vec![HttpRoute {
                    method: HttpMethod::Post as i32,
                    path: "/test-path-1/{name}".to_string(),
                    binding: Some(GatewayBinding {
                        component: Some(VersionedComponentId {
                            component_id: Some(component_id.clone().into()),
                            version: 0,
                        }),
                        worker_name: Some(to_grpc_rib_expr(r#""counter""#)),
                        response: Some(to_grpc_rib_expr(
                            r#"
                                let worker = instance("w1");
                                worker.test1();
                                {
                                   body: "ok",
                                   status: 200,
                                   headers: { Content-Type: "application/json" }
                                }
                            "#,
                        )),
                        idempotency_key: None,
                        binding_type: Some(GatewayBindingType::Default as i32),
                        static_binding: None,
                        invocation_context: Some(to_grpc_rib_expr(
                            r#"
                                {
                                    name: request.path.name,
                                    source: "rib"
                                }
                            "#,
                        )),
                    }),
                    middleware: None,
                }],
            },
        )),
    };

    let _ = deps
        .worker_service()
        .create_api_definition(CreateApiDefinitionRequest {
            api_definition: Some(create_api_definition_request::ApiDefinition::Definition(
                request.clone(),
            )),
        })
        .await
        .unwrap();

    let request = ApiDeploymentRequest {
        api_definitions: vec![ApiDefinitionInfo {
            id: api_definition_id.value,
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
        .create_or_update_api_deployment(request.clone())
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

pub fn to_grpc_rib_expr(expr: &str) -> Expr {
    rib::Expr::from_text(expr).unwrap().into()
}
