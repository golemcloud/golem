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

use super::*;
use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
use golem_common::schema::{InputSchema, OutputSchema, SchemaGraph, SchemaType};
use golem_service_base::custom_api::{
    CompiledInputSchema, CompiledOutputSchema, RouteBehaviour, RouterMethod,
};
use serde_json::json;
use test_r::test;
use tokio::sync::{mpsc, oneshot};

pub(in crate::custom_api) async fn inputs(providers: usize) -> Arc<OpenApiInputs> {
    let resolved = test_resolver(provider_routes(providers))
        .resolve_matching_route(
            &poem::Request::builder()
                .uri("/openapi.json".parse().unwrap())
                .header("host", "example.com")
                .finish(),
        )
        .await
        .unwrap();
    let inputs = resolved.openapi_inputs.unwrap();
    Arc::new(OpenApiInputs {
        key: inputs.key.clone(),
        freshness: inputs.freshness.clone(),
        public_origin: inputs.public_origin.clone(),
        routes: inputs.routes.clone(),
    })
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
    json!({"openapi":"3.1.0","info":{"title":"Provider","version":"1"},
        "paths":{"/":{"get":{"responses":{"200":{"description":"ok"}}}}}})
    .to_string()
}

type Call = (
    IdempotencyKey,
    oneshot::Sender<Result<String, OpenApiError>>,
);

struct ControlledInvoker {
    calls: mpsc::UnboundedSender<Call>,
    cleanups: mpsc::UnboundedSender<IdempotencyKey>,
}

#[async_trait]
impl ProviderInvoker for ControlledInvoker {
    async fn invoke(&self, call: &ProviderCall) -> Result<String, OpenApiError> {
        let (tx, rx) = oneshot::channel();
        self.calls.send((call.key.clone(), tx)).unwrap();
        rx.await.unwrap()
    }

    async fn cleanup(&self, call: &ProviderCall) {
        self.cleanups.send(call.key.clone()).unwrap();
        std::future::pending::<()>().await;
    }
}

pub(in crate::custom_api) fn controlled() -> (
    OpenApiService,
    mpsc::UnboundedReceiver<Call>,
    mpsc::UnboundedReceiver<IdempotencyKey>,
) {
    let (calls, call_rx) = mpsc::unbounded_channel();
    let (cleanups, cleanup_rx) = mpsc::unbounded_channel();
    (
        OpenApiService {
            invoker: Arc::new(ControlledInvoker { calls, cleanups }),
            admission: Arc::new(Semaphore::new(GENERATION_CONCURRENCY)),
            cache: Arc::new(Mutex::new(CacheState::default())),
            processing_hook: None,
        },
        call_rx,
        cleanup_rx,
    )
}

pub(in crate::custom_api) fn paused(future: impl std::future::Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::pause();
            future.await;
        });
}

#[test]
#[test_r::timeout("30s")]
async fn fanout_is_bounded_and_completion_order_does_not_change_json_or_yaml() {
    let inputs = inputs(9).await;
    let (service, mut calls, mut cleanups) = controlled();
    let first = tokio::spawn({
        let service = service.clone();
        let inputs = inputs.clone();
        async move { service.generate(inputs).await }
    });
    let mut running = Vec::new();
    for _ in 0..8 {
        running.push(calls.recv().await.unwrap());
    }
    assert!(calls.try_recv().is_err());
    let released = running.pop().unwrap();
    released.1.send(Ok(document())).unwrap();
    running.push(calls.recv().await.unwrap());
    for (_, reply) in running.into_iter().rev() {
        reply.send(Ok(document())).unwrap();
    }
    let first = first.await.unwrap().unwrap();
    service.clear();
    let second = tokio::spawn({
        let service = service.clone();
        async move { service.generate(inputs).await }
    });
    for _ in 0..9 {
        calls.recv().await.unwrap().1.send(Ok(document())).unwrap();
    }
    let second = second.await.unwrap().unwrap();
    assert_eq!(first.json, second.json);
    assert_eq!(first.yaml, second.yaml);
    let json: Value = serde_json::from_slice(&first.json).unwrap();
    let yaml: Value = serde_yaml::from_slice(&first.yaml).unwrap();
    assert_eq!(json, yaml);
    assert_eq!(json["paths"].as_object().unwrap().len(), 10);
    assert_eq!(json["servers"], json!([{"url":"https://example.com"}]));
    assert!(cleanups.try_recv().is_err());
}

#[test]
fn provider_timeout_cleans_up_and_retains_admission_until_cleanup_deadline() {
    paused(async {
        let inputs = inputs(1).await;
        let (service, mut calls, mut cleanups) = controlled();
        let task = tokio::spawn({
            let service = service.clone();
            async move { service.generate(inputs).await }
        });
        let (key, _reply) = calls.recv().await.unwrap();
        tokio::time::advance(PROVIDER_TIMEOUT + Duration::from_nanos(1)).await;
        assert_eq!(
            task.await.unwrap().unwrap_err().category(),
            "provider-timeout"
        );
        assert_eq!(cleanups.recv().await.unwrap(), key);
        assert_eq!(service.admission.available_permits(), 7);
        tokio::time::advance(CLEANUP_TIMEOUT + Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(service.admission.available_permits(), 8);
    });
}

#[test]
fn generation_deadline_covers_multiple_provider_batches() {
    paused(async {
        let inputs = inputs(64).await;
        let (service, mut calls, mut cleanups) = controlled();
        let task = tokio::spawn(async move { service.generate(inputs).await });
        for _ in 0..7 {
            let mut batch = Vec::new();
            for _ in 0..8 {
                batch.push(calls.recv().await.unwrap().1);
            }
            tokio::time::advance(Duration::from_secs(4)).await;
            for reply in batch {
                reply.send(Ok(document())).unwrap();
            }
        }
        let mut last = Vec::new();
        for _ in 0..8 {
            last.push(calls.recv().await.unwrap().1);
        }
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            task.await.unwrap().unwrap_err().category(),
            "generation-timeout"
        );
        for _ in 0..8 {
            cleanups.recv().await.unwrap();
        }
        assert!(last.iter().all(oneshot::Sender::is_closed));
    });
}

#[test]
fn provider_deadline_accepts_exactly_five_seconds() {
    struct Delayed(Duration);
    #[async_trait]
    impl ProviderInvoker for Delayed {
        async fn invoke(&self, _: &ProviderCall) -> Result<String, OpenApiError> {
            let started = Instant::now();
            // Sleep rounds wakeups to timer ticks; advance sets the exact
            // completion instant needed to distinguish the inclusive boundary.
            tokio::time::advance(self.0).await;
            assert_eq!(started.elapsed(), self.0, "provider test completion time");
            Ok(document())
        }
        async fn cleanup(&self, _: &ProviderCall) {}
    }
    paused(async {
        let inputs = inputs(1).await;
        let admission = Arc::new(Semaphore::new(1));
        let lease = Arc::new(admission.acquire_owned().await.unwrap());
        assert_eq!(
            invoke_provider(
                Arc::new(Delayed(PROVIDER_TIMEOUT)),
                &inputs.routes[1],
                lease.clone()
            )
            .await
            .unwrap(),
            document()
        );
        assert_eq!(
            invoke_provider(
                Arc::new(Delayed(PROVIDER_TIMEOUT + Duration::from_millis(1))),
                &inputs.routes[1],
                lease
            )
            .await
            .unwrap_err()
            .category(),
            "provider-timeout"
        );
    });
}

#[test]
#[test_r::timeout("30s")]
async fn first_failure_cancels_peers_and_never_starts_queued_providers() {
    let inputs = inputs(9).await;
    let (service, mut calls, mut cleanups) = controlled();
    let task = tokio::spawn(async move { service.generate(inputs).await });
    let mut running = Vec::new();
    for _ in 0..8 {
        running.push(calls.recv().await.unwrap());
    }
    running
        .pop()
        .unwrap()
        .1
        .send(Err(OpenApiError::new("provider-invocation")))
        .unwrap();
    assert_eq!(
        task.await.unwrap().unwrap_err().category(),
        "provider-invocation"
    );
    for _ in 0..8 {
        cleanups.recv().await.unwrap();
    }
    assert!(calls.try_recv().is_err());
    for (_, reply) in running {
        assert!(reply.is_closed());
    }
}

#[test]
#[test_r::timeout("30s")]
async fn dropped_waiter_does_not_release_generation_admission() {
    let (service, mut calls, mut cleanups) = controlled();
    let mut replies = Vec::new();
    for index in 0..8 {
        let mut inputs = inputs(1).await;
        Arc::get_mut(&mut inputs).unwrap().key.domain.0 = format!("domain{index}.test");
        let task = tokio::spawn({
            let service = service.clone();
            let inputs = inputs.clone();
            async move { service.generate(inputs).await }
        });
        replies.push(calls.recv().await.unwrap().1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
    assert_eq!(
        service
            .generate(inputs(1).await)
            .await
            .unwrap_err()
            .category(),
        "admission"
    );
    assert!(cleanups.try_recv().is_err());
    for reply in replies {
        reply.send(Ok(document())).unwrap();
    }
    timeout(Duration::from_secs(4), async {
        while service.admission.available_permits() != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(cleanups.try_recv().is_err());
}

#[test]
#[test_r::timeout("30s")]
async fn provider_utf8_limit_is_inclusive_before_parsing() {
    let inputs = inputs(1).await;
    for (size, expected) in [
        (PROVIDER_BYTE_LIMIT, "provider-json"),
        (PROVIDER_BYTE_LIMIT + 1, "provider-size"),
    ] {
        let (service, mut calls, mut cleanups) = controlled();
        let task = tokio::spawn({
            let inputs = inputs.clone();
            async move { service.generate(inputs).await }
        });
        let mut text = "é".repeat(size / 2);
        if size % 2 != 0 {
            text.push('x');
        }
        calls.recv().await.unwrap().1.send(Ok(text)).unwrap();
        assert_eq!(task.await.unwrap().unwrap_err().category(), expected);
        assert!(
            cleanups.try_recv().is_err(),
            "completed provider needs no interruption"
        );
    }
}

#[test]
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
}

#[test]
#[test_r::timeout("30s")]
async fn diagnostics_and_observability_do_not_expose_provider_values() {
    let (service, mut calls, _cleanups) = controlled();
    let inputs = inputs(1).await;
    let canary = "provider-private-value-canary";
    let valid = document().replace("ok", canary);
    let task = tokio::spawn({
        let service = service.clone();
        let inputs = inputs.clone();
        async move { service.generate(inputs).await }
    });
    calls.recv().await.unwrap().1.send(Ok(valid)).unwrap();
    let doc = task.await.unwrap().unwrap();
    assert!(std::str::from_utf8(&doc.json).unwrap().contains(canary));
    assert!(!format!("{doc:?}").contains(canary));
    service.clear();
    let task = tokio::spawn({
        let service = service.clone();
        let inputs = inputs.clone();
        async move { service.generate(inputs).await }
    });
    let mut invalid: Value = serde_json::from_str(&document()).unwrap();
    invalid["components"] =
        json!({"schemas":{"S":{"$ref":format!("https://example.com/{canary}")}}});
    calls
        .recv()
        .await
        .unwrap()
        .1
        .send(Ok(invalid.to_string()))
        .unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.category(), "unsupported-reference");
    assert!(!format!("{error:?} {error}").contains(canary));
    let cached = service.generate(inputs).await.unwrap_err();
    assert_eq!(cached.category(), error.category());
    assert!(calls.try_recv().is_err());
    let metrics = prometheus::gather();
    for name in ["openapi_cache_total", "openapi_generation_seconds"] {
        let family = metrics.iter().find(|metric| metric.name() == name).unwrap();
        for metric in family.get_metric() {
            assert_eq!(metric.get_label().len(), 1);
            assert_eq!(metric.get_label()[0].name(), "outcome");
            assert!(!metric.get_label()[0].value().contains(canary));
        }
    }
}
