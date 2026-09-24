// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use super::*;
use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
use golem_common::schema::{InputSchema, OutputSchema, SchemaGraph, SchemaType};
use golem_service_base::custom_api::{
    CompiledInputSchema, CompiledOutputSchema, RouteBehaviour, RouterMethod,
};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

pub(in crate::custom_api) async fn inputs(providers: usize) -> Arc<OpenApiInputs> {
    test_resolver(provider_routes(providers))
        .resolve_matching_route(
            &poem::Request::builder()
                .uri("/openapi.json".parse().unwrap())
                .header("host", "example.com")
                .finish(),
        )
        .await
        .unwrap()
        .openapi_inputs
        .unwrap()
}

pub(in crate::custom_api) fn provider_routes(
    providers: usize,
) -> Vec<golem_service_base::custom_api::CompiledRoute> {
    let mut routes = vec![test_route(0, "/openapi.json", Some("GET"), "reserved")];
    for id in 1..=providers {
        let mut route = test_route(id as i32, &format!("/r{id}"), None, "router");
        let RouteBehaviour::HttpRouter(router) = &mut route.behavior else {
            unreachable!()
        };
        router.openapi_provider_method = Some(RouterMethod {
            method_name: "describe".into(),
            input: CompiledInputSchema {
                graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                input_schema: InputSchema::Parameters(vec![]),
            },
            output: CompiledOutputSchema {
                graph: SchemaGraph::anonymous(SchemaType::string()),
                output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
            },
        });
        routes.push(route);
    }
    routes
}

pub(in crate::custom_api) fn document() -> String {
    json!({"openapi":"3.1.0","info":{"title":"Provider","version":"1"},"paths":{"/":{"get":{"responses":{"200":{"description":"ok"}}}}}}).to_string()
}
type Call = (
    IdempotencyKey,
    oneshot::Sender<Result<String, OpenApiError>>,
);
struct ControlledInvoker(mpsc::UnboundedSender<Call>);
#[async_trait]
impl ProviderInvoker for ControlledInvoker {
    async fn invoke(&self, call: &ProviderCall) -> Result<String, OpenApiError> {
        let (tx, rx) = oneshot::channel();
        self.0.send((call.key.clone(), tx)).unwrap();
        rx.await.unwrap()
    }
}
pub(in crate::custom_api) fn controlled() -> (OpenApiService, mpsc::UnboundedReceiver<Call>) {
    let (calls, call_rx) = mpsc::unbounded_channel();
    (
        OpenApiService {
            invoker: Arc::new(ControlledInvoker(calls)),
            cache: Cache::new(
                Some(CACHE_CAPACITY),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "openapi_documents_test",
            ),
            processing_hook: None,
        },
        call_rx,
    )
}

#[test_r::test]
fn provider_has_no_service_imposed_timeout() {
    struct Delayed;
    #[async_trait]
    impl ProviderInvoker for Delayed {
        async fn invoke(&self, _: &ProviderCall) -> Result<String, OpenApiError> {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(document())
        }
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let route_inputs = inputs(1).await;
            tokio::time::pause();
            let route = route_inputs.routes[1].clone();
            let invocation =
                tokio::spawn(async move { invoke_provider(Arc::new(Delayed), &route).await });
            tokio::task::yield_now().await;
            tokio::time::advance(std::time::Duration::from_secs(60)).await;
            assert_eq!(invocation.await.unwrap().unwrap(), document());
        });
}

#[test_r::test]
async fn provider_size_limit_is_checked() {
    struct Oversized;
    #[async_trait]
    impl ProviderInvoker for Oversized {
        async fn invoke(&self, _: &ProviderCall) -> Result<String, OpenApiError> {
            Ok("x".repeat(PROVIDER_BYTE_LIMIT + 1))
        }
    }
    let route_inputs = inputs(1).await;
    assert_eq!(
        invoke_provider(Arc::new(Oversized), &route_inputs.routes[1])
            .await
            .unwrap_err()
            .category(),
        "provider-size"
    );
}

#[test_r::test]
async fn worker_adapter_uses_derived_private_identity_and_canonical_input() {
    use crate::mcp::InvocationHarness;
    use golem_common::model::AgentInvocationOutput;
    use golem_common::model::agent::{AgentMode, AgentTypeName};
    use golem_common::schema::AgentConstructorSchema;
    let harness = InvocationHarness::new_with_agent_mode(
        AgentInvocationOutput {
            result: AgentInvocationResult::AgentMethod {
                output: SchemaValue::String(document()),
            },
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            agent_id: None,
            idempotency_key: None,
            oplog_index: None,
            agent_fingerprint: None,
        },
        AgentMode::Ephemeral,
        AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        vec![golem_common::schema::AgentMethodSchema {
            name: "describe".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
            output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
            http_endpoint: vec![],
            read_only: None,
        }],
    );
    let mut routes = provider_routes(1);
    let RouteBehaviour::HttpRouter(router) = &mut routes[1].behavior else {
        unreachable!()
    };
    router.component_id = harness.component_id;
    router.agent_type = AgentTypeName("mcp-agent".into());
    let resolved = test_resolver(routes)
        .resolve_matching_route(
            &poem::Request::builder()
                .uri("/r1".parse().unwrap())
                .header("host", "example.com")
                .finish(),
        )
        .await
        .unwrap();
    let call = prepare_call(&resolved.route).unwrap();
    let other = prepare_call(&resolved.route).unwrap();
    assert_ne!(call.key, other.key);
    assert_ne!(call.agent_id, other.agent_id);
    let adapter = WorkerProviderInvoker(harness.worker_service.clone());
    assert_eq!(adapter.invoke(&call).await.unwrap(), document());
    assert_eq!(harness.recorded_agent_id(), call.agent_id);
    let phantom = call
        .agent_id
        .agent_id
        .strip_suffix(']')
        .unwrap()
        .rsplit_once('[')
        .unwrap()
        .1;
    assert!(uuid::Uuid::parse_str(phantom).is_ok());
    assert_eq!(
        harness.recorded_method_params(),
        SchemaValue::Record { fields: vec![] }
    );
    let contexts = harness.contexts.lock().unwrap();
    assert_eq!(contexts.len(), 1);
    assert!(matches!(contexts[0].auth, AuthCtx::System));
    assert_eq!(contexts[0].principal, Principal::anonymous().into());
    assert!(contexts[0].context.is_none());
}
