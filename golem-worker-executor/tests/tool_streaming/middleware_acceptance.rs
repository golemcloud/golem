// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use golem_common::model::oplog::PublicOplogEntryWithIndex;
use golem_human_approval::{
    ApprovalDecision, ApprovalOwner, ApprovalRecord, ApprovalServiceConfig, ApprovalState,
    ApprovalStore,
};
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("audit_middleware")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("rate_limit_middleware")]
    PrecompiledComponent
);

#[derive(Debug, FromSchema)]
struct RateLimitBackendStats {
    attempts: u64,
    committed_charges: u64,
    recorded_decisions: u64,
}

fn rate_limit_parameters(
    definition: &ToolMiddleware,
    policy: &str,
    limit: u64,
    window_milliseconds: u64,
) -> TypedSchemaValue {
    TypedSchemaValue::new(
        definition.parameter_schema.clone(),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String(policy.to_string()),
                SchemaValue::U64(limit),
                SchemaValue::U64(window_milliseconds),
            ],
        },
    )
}

fn oidc_principal(subject: &str) -> Principal {
    Principal::Oidc(OidcPrincipal {
        sub: subject.to_string(),
        issuer: "https://rate-limit.test".to_string(),
        email: None,
        name: None,
        email_verified: None,
        given_name: None,
        family_name: None,
        picture: None,
        preferred_username: None,
        claims: "{}".to_string(),
    })
}

macro_rules! setup_rate_limit_chain {
    ($last:expr, $deps:expr, $provider:expr, $caller:expr, $rate_limit:expr,
     $policy:expr, $limit:expr,
     $context:ident, $environment:ident, $executor:ident, $provider_component:ident,
     $caller_component:ident, $rate_limit_component:ident, $agent_type:ident) => {
        let $context = TestContext::new($last);
        let $environment = Arc::new(TestEnvironmentStateService::default());
        let $executor = start_with_overrides(
            $deps,
            &$context,
            TestExecutorOverrides {
                environment_state_service: Some($environment.clone()),
                ..Default::default()
            },
        )
        .await?;
        let $provider_component = $executor
            .component_dep(&$context.default_environment_id, $provider)
            .store()
            .await?;
        let $caller_component = $executor
            .component_dep(&$context.default_environment_id, $caller)
            .store()
            .await?;
        let $rate_limit_component = $executor
            .component_dep(&$context.default_environment_id, $rate_limit)
            .store()
            .await?;
        let provider_metadata = extract_component_metadata(
            &$deps
                .component_directory
                .join(format!("{}.wasm", $provider.wasm_name)),
            false,
            true,
        )
        .await?;
        let middleware_metadata = extract_component_metadata(
            &$deps
                .component_directory
                .join(format!("{}.wasm", $rate_limit.wasm_name)),
            false,
            true,
        )
        .await?;
        let definition = middleware_metadata
            .tool_middlewares
            .iter()
            .find(|definition| definition.name == "persistent-rate-limit")
            .expect("persistent rate-limit middleware metadata");
        let $agent_type = AgentTypeName("ToolStreamingCaller".to_string());
        let tool_name = ToolName::try_from("middleware-probe").unwrap();
        let mut deployment = deployment_state(
            $context.account_id,
            $provider_component.id,
            $provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            $agent_type.0.as_str(),
            provider_metadata.tools,
        );
        install_middleware_chain(
            &mut deployment,
            &$agent_type,
            &tool_name,
            $rate_limit_component.id,
            $rate_limit_component.revision,
            "golem:rate-limit-middleware",
            &middleware_metadata.tool_middlewares,
            vec![(
                definition.name.as_str(),
                rate_limit_parameters(definition, $policy, $limit, 3_600_000),
            )],
        );
        $environment.set_tool_deployment(
            $context.default_environment_id,
            $caller_component.id,
            $caller_component.revision,
            Some(deployment),
        );
    };
}

#[path = "k3_persistent_rate_limit_acceptance.rs"]
mod k3_persistent_rate_limit_acceptance;

macro_rules! setup_probe_chain {
    ($last:expr, $deps:expr, $provider:expr, $caller:expr, $names:expr,
     $context:ident, $environment:ident, $executor:ident, $provider_component:ident,
     $caller_component:ident, $middleware_metadata:ident, $agent_type:ident, $deployment:ident
     $(, configure = $configure:expr)?) => {
        let $context = TestContext::new($last);
        let $environment = Arc::new(TestEnvironmentStateService::default());
        let overrides = TestExecutorOverrides {
            environment_state_service: Some($environment.clone()),
            ..Default::default()
        };
        $(let overrides = TestExecutorOverrides {
            configure: Some($configure),
            ..overrides
        };)?
        let $executor = start_with_overrides(
            $deps,
            &$context,
            overrides,
        )
        .await?;
        let $provider_component = $executor
            .component_dep(&$context.default_environment_id, $provider)
            .store()
            .await?;
        let $caller_component = $executor
            .component_dep(&$context.default_environment_id, $caller)
            .store()
            .await?;
        let middleware_component = $executor
            .component(
                &$context.default_environment_id,
                "golem_it_tool_streaming_rust_middleware_release",
            )
            .name("golem-it:tool-streaming-middleware-step-acceptance")
            .store()
            .await?;
        let provider_metadata = extract_component_metadata(
            &$deps
                .component_directory
                .join(format!("{}.wasm", $provider.wasm_name)),
            false,
            true,
        )
        .await?;
        let $middleware_metadata = extract_component_metadata(
            &$deps
                .component_directory
                .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
            false,
            true,
        )
        .await?;
        let $agent_type = AgentTypeName("ToolStreamingCaller".to_string());
        let tool_name = ToolName::try_from("middleware-probe").unwrap();
        let mut $deployment = deployment_state(
            $context.account_id,
            $provider_component.id,
            $provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            $agent_type.0.as_str(),
            provider_metadata.tools,
        );
        let occurrences = $names
            .iter()
            .map(|name| {
                let definition = $middleware_metadata
                    .tool_middlewares
                    .iter()
                    .find(|definition| definition.name == *name)
                    .unwrap();
                (
                    definition.name.as_str(),
                    empty_middleware_parameters(definition),
                )
            })
            .collect();
        install_middleware_chain(
            &mut $deployment,
            &$agent_type,
            &tool_name,
            middleware_component.id,
            middleware_component.revision,
            "golem-it:tool-streaming-rust-middleware",
            &$middleware_metadata.tool_middlewares,
            occurrences,
        );
    };
}

pub(super) async fn start_probe_effect_server() -> (
    u16,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    tokio::task::JoinHandle<()>,
) {
    async fn effect(
        Path(value): Path<String>,
        State(effects): State<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> axum::http::StatusCode {
        effects.send(value).expect("record middleware probe effect");
        axum::http::StatusCode::NO_CONTENT
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (effects, received) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/{value}", post(effect))
                .with_state(effects),
        )
        .await
        .expect("serve middleware probe effects");
    });
    (port, received, task)
}

#[derive(Clone, Default)]
struct AuditSinkState {
    attempts: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
    committed: Arc<std::sync::Mutex<std::collections::BTreeMap<String, serde_json::Value>>>,
    block_first_after_commit: Arc<std::sync::atomic::AtomicBool>,
    committed_while_blocked: Arc<tokio::sync::Notify>,
}

async fn start_audit_sink(
    block_first_after_commit: bool,
) -> (String, AuditSinkState, tokio::task::JoinHandle<()>) {
    async fn commit(
        State(state): State<AuditSinkState>,
        headers: axum::http::HeaderMap,
        axum::Json(record): axum::Json<serde_json::Value>,
    ) -> axum::http::StatusCode {
        let key = headers
            .get("idempotency-key")
            .expect("audit request idempotency key")
            .to_str()
            .expect("ASCII audit idempotency key")
            .to_string();
        assert_eq!(record["policyInvocationKey"], key);
        state
            .attempts
            .lock()
            .unwrap()
            .push((key.clone(), record.clone()));
        state.committed.lock().unwrap().entry(key).or_insert(record);
        if state
            .block_first_after_commit
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            state.committed_while_blocked.notify_one();
            std::future::pending::<()>().await;
        }
        axum::http::StatusCode::NO_CONTENT
    }

    let state = AuditSinkState {
        block_first_after_commit: Arc::new(std::sync::atomic::AtomicBool::new(
            block_first_after_commit,
        )),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/records", listener.local_addr().unwrap());
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/records", post(commit))
                .with_state(task_state),
        )
        .await
        .expect("serve audit sink");
    });
    (url, state, task)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromiseCompletion {
    oplog_idx: u64,
    data: Vec<u8>,
}

struct ApprovalHarness {
    request_url: String,
    service_url: String,
    completions: tokio::sync::mpsc::UnboundedReceiver<PromiseCompletion>,
    client: reqwest::Client,
    _store_dir: tempfile::TempDir,
    service_task: tokio::task::JoinHandle<()>,
    callback_task: tokio::task::JoinHandle<()>,
}

impl ApprovalHarness {
    async fn start() -> Self {
        async fn complete(
            headers: axum::http::HeaderMap,
            State(completions): State<tokio::sync::mpsc::UnboundedSender<PromiseCompletion>>,
            axum::Json(completion): axum::Json<PromiseCompletion>,
        ) -> Result<axum::Json<bool>, axum::http::StatusCode> {
            if headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                != Some("Bearer golem-token")
            {
                return Err(axum::http::StatusCode::UNAUTHORIZED);
            }
            completions.send(completion).unwrap();
            Ok(axum::Json(true))
        }

        let callback_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let callback_port = callback_listener.local_addr().unwrap().port();
        let (completion_tx, completions) = tokio::sync::mpsc::unbounded_channel();
        let callback_task = tokio::spawn(async move {
            axum::serve(
                callback_listener,
                Router::new()
                    .route(
                        "/v1/components/{component}/workers/{agent}/complete",
                        post(complete),
                    )
                    .with_state(completion_tx),
            )
            .await
            .unwrap();
        });

        let service_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service_port = service_listener.local_addr().unwrap().port();
        let store_dir = tempfile::tempdir().unwrap();
        let store = ApprovalStore::open(store_dir.path().join("approvals.json")).unwrap();
        let app = golem_human_approval::router(
            store,
            ApprovalServiceConfig {
                request_token: "request-token".to_string(),
                decision_token: "decision-token".to_string(),
                golem_api_url: format!("http://127.0.0.1:{callback_port}"),
                golem_api_token: "golem-token".to_string(),
            },
        );
        let service_task = tokio::spawn(async move {
            axum::serve(service_listener, app).await.unwrap();
        });
        Self {
            request_url: format!("http://127.0.0.1:{service_port}/v1/requests"),
            service_url: format!("http://127.0.0.1:{service_port}"),
            completions,
            client: reqwest::Client::new(),
            _store_dir: store_dir,
            service_task,
            callback_task,
        }
    }

    async fn pending(&self, expected: usize) -> anyhow::Result<Vec<ApprovalRecord>> {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let response = self
                    .client
                    .get(format!("{}/v1/requests", self.service_url))
                    .bearer_auth("decision-token")
                    .send()
                    .await?;
                let records = response
                    .error_for_status()?
                    .json::<Vec<ApprovalRecord>>()
                    .await?;
                if records.len() >= expected {
                    return Ok::<_, reqwest::Error>(records);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for approval request"))?
        .map_err(Into::into)
    }

    async fn decide(
        &self,
        record: &ApprovalRecord,
        owner: ApprovalOwner,
        state: ApprovalState,
        token: &str,
    ) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/v1/requests/{}/decision",
                self.service_url, record.request.request_id
            ))
            .bearer_auth(token)
            .json(&ApprovalDecision {
                owner,
                state,
                decided_by: "operator-7".to_string(),
            })
            .send()
            .await
            .unwrap()
    }

    async fn abandon(&self, record: &ApprovalRecord) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/v1/requests/{}/abandon",
                self.service_url, record.request.request_id
            ))
            .bearer_auth("decision-token")
            .json(&record.request.owner)
            .send()
            .await
            .unwrap()
    }

    async fn completion(&mut self) -> anyhow::Result<PromiseCompletion> {
        tokio::time::timeout(std::time::Duration::from_secs(30), self.completions.recv())
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for approval callback"))?
            .ok_or_else(|| anyhow::anyhow!("approval callback server stopped"))
    }
}

impl Drop for ApprovalHarness {
    fn drop(&mut self) {
        self.service_task.abort();
        self.callback_task.abort();
    }
}

fn configure_approval_parameters(
    deployment: &mut ToolDeploymentState,
    agent_type: &AgentTypeName,
    definition: &ToolMiddleware,
    request_url: &str,
) {
    deployment
        .tool_middleware_chains
        .get_mut(&ToolBindingOwner::AgentType {
            agent_type_name: agent_type.clone(),
        })
        .unwrap()
        .get_mut(&ToolName::try_from("middleware-probe").unwrap())
        .unwrap()
        .occurrences[0]
        .parameters =
        human_approval_middleware_parameters(definition, request_url, "production-change");
}

fn entity_starts<'a>(
    oplog: &'a [PublicOplogEntryWithIndex],
    kind: PublicAgentEntityKind,
    name: &str,
) -> Vec<&'a PublicOplogEntryWithIndex> {
    oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(parameters)
                    if parameters.function_name == "golem::entity::invoke"
            ) && matches!(
                &entry.attribution,
                PublicOplogEntryAttribution::Entity(entity)
                    if entity.invocation.entity.kind == kind
                        && entity.invocation.entity.name == name
            )
        })
        .collect()
}

fn assert_one_terminal_before_finished(
    oplog: &[PublicOplogEntryWithIndex],
    start: OplogIndex,
    finished: OplogIndex,
) {
    let terminals = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start)
                || matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == start)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminals.len(),
        1,
        "entity {start} must settle exactly once before Finished"
    );
    assert!(terminals[0].oplog_index < finished);
    let opening = oplog
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(parameters) if entry.oplog_index == start => {
                parameters.span_started.as_ref()
            }
            _ => None,
        })
        .expect("entity Start carries its span opening");
    let (closing, expected_outcome) = match &terminals[0].entry {
        PublicOplogEntry::End(end) => (
            end.span_finished.as_ref(),
            golem_common::model::oplog::PublicSpanOutcome::Completed,
        ),
        PublicOplogEntry::Cancelled(cancelled) => (
            cancelled.span_finished.as_ref(),
            golem_common::model::oplog::PublicSpanOutcome::Cancelled,
        ),
        _ => unreachable!(),
    };
    let closing = closing.expect("entity terminal carries its span close");
    assert_eq!(closing.span_id, opening.span_id);
    assert_eq!(closing.outcome, expected_outcome);
    assert!(closing.finished_at >= opening.started_at);

    let entry = oplog
        .iter()
        .find(|entry| entry.oplog_index == start)
        .unwrap();
    let PublicOplogEntryAttribution::Entity(entity) = &entry.attribution else {
        panic!("entity Start must have entity attribution");
    };
    if let Some(parent) = entity.ancestors.last() {
        let parent_opening = oplog
            .iter()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(parameters) if entry.oplog_index == parent.start_index => {
                    parameters.span_started.as_ref()
                }
                _ => None,
            })
            .expect("parent entity Start carries its span opening");
        assert_eq!(opening.trace_id, parent_opening.trace_id);
        assert_eq!(
            opening.parent_span_id.as_ref(),
            Some(&parent_opening.span_id)
        );
    }
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn shipped_audit_records_payload_free_success_error_and_stream_summaries(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (sink_url, sink, sink_task) = start_audit_sink(false).await;
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let audit_component = executor
        .component_dep(&context.default_environment_id, audit)
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let audit_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("../builtin-tools/audit-middleware.wasm"),
        false,
        true,
    )
    .await?;
    let definition = audit_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "audit")
        .expect("shipped audit middleware metadata");
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("streaming").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        audit_component.id,
        audit_component.revision,
        "golem:audit-middleware",
        &audit_metadata.tool_middlewares,
        vec![(
            definition.name.as_str(),
            audit_middleware_parameters(definition, "summary-contract", &sink_url),
        )],
    );
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "shipped-audit-summaries");
    let sensitive_input = b"stream-payload-must-not-be-recorded".to_vec();
    let success: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect",
            data_value!("echo", sensitive_input.clone(), 7_u32),
        )
        .await?
        .into_typed()?;
    assert_eq!(success.output, sensitive_input);
    assert_eq!(success.completion, "ok");
    let error: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "result_before_stdout",
            data_value!("declared-error"),
        )
        .await?
        .into_typed()?;
    assert_eq!(error.output, b"marker:");
    assert!(error.completion.contains("Declared"));

    let committed = sink.committed.lock().unwrap();
    assert_eq!(committed.len(), 2);
    let success_record = committed
        .values()
        .find(|record| record["outcome"]["kind"] == "success")
        .expect("success audit record");
    assert_eq!(success_record["toolName"], "streaming");
    assert_eq!(success_record["stdout"]["declared"], true);
    assert_eq!(success_record["stdout"]["bytes"], sensitive_input.len());
    assert_eq!(success_record["stdout"]["terminal"], "finished");
    assert_eq!(success_record["stderr"]["declared"], false);
    let error_record = committed
        .values()
        .find(|record| record["outcome"]["kind"] == "error")
        .expect("error audit record");
    assert_eq!(error_record["outcome"]["errorKind"], "tool");
    assert!(error_record["outcome"]["customName"].is_string());
    assert_eq!(error_record["stdout"]["declared"], true);
    assert_eq!(error_record["stdout"]["bytes"], 7);
    let records = serde_json::to_string(&*committed)?;
    assert!(!records.contains("stream-payload-must-not-be-recorded"));
    assert!(!records.contains("bytes_read"));
    drop(committed);
    sink_task.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
async fn duplicate_secret_policy_occurrences_are_isolated_from_leaf(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_service_base::model::agent_secret::AgentSecret;

    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let secret_path = CanonicalAgentSecretPath(vec!["toolSecret".to_string()]);
    let plaintext = "gol606-runtime-secret";
    environment.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: secret_path.clone(),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String(plaintext.to_string())),
    });
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-secret-policy-middleware")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let definition = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-secret-policy-audit")
        .expect("secret policy middleware metadata");
    let universal_definition = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-secret-policy-audit")
        .expect("universal secret policy middleware metadata");
    let agent_type = AgentTypeName("ToolSecretCaller".to_string());
    let tool_name = ToolName::try_from("secret-policy-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![
            (
                universal_definition.name.as_str(),
                empty_middleware_parameters(universal_definition),
            ),
            (
                definition.name.as_str(),
                secret_policy_middleware_parameters(definition, "restricted"),
            ),
            (
                definition.name.as_str(),
                secret_policy_middleware_parameters(definition, "allowed"),
            ),
        ],
    );
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    let occurrences = &mut deployment
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences;
    occurrences[0].secret_keys_readable = SecretKeyScope::All;
    occurrences[0].secret_keys_revealable = SecretKeyScope::Keys(Default::default());
    occurrences[1].secret_keys_readable = SecretKeyScope::Keys(Default::default());
    occurrences[1].secret_keys_revealable = SecretKeyScope::Keys(Default::default());
    occurrences[2].secret_keys_readable = SecretKeyScope::All;
    occurrences[2].secret_keys_revealable = SecretKeyScope::All;
    let original_deployment = deployment.clone();
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let (promise_port, promise_server, mut promise_arrivals) =
        start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolSecretCaller", "isolated-secret-policy");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([(
                "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".to_string(),
                promise_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "inspect_secret_policy",
        data_value!(),
    );
    tokio::pin!(invocation);
    let checkpoint = tokio::select! {
        result = invocation.as_mut() => panic!("secret policy invocation settled before checkpoint: {result:?}"),
        checkpoint = next_promise_checkpoint(&mut promise_arrivals, "secret-policy-before-access") => checkpoint?,
    };
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Suspended,
            std::time::Duration::from_secs(30),
        )
        .await?;

    let mut changed = original_deployment;
    let changed_occurrences = &mut changed
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences;
    changed_occurrences[1].parameters =
        secret_policy_middleware_parameters(definition, "changed-first");
    changed_occurrences[1].secret_keys_readable = SecretKeyScope::All;
    changed_occurrences[1].secret_keys_revealable = SecretKeyScope::All;
    changed_occurrences[2].parameters =
        secret_policy_middleware_parameters(definition, "changed-second");
    changed_occurrences[2].secret_keys_readable = SecretKeyScope::Keys(Default::default());
    changed_occurrences[2].secret_keys_revealable = SecretKeyScope::Keys(Default::default());
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(changed),
    );
    executor.simulated_crash(&worker_id).await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: checkpoint.oplog_idx,
            },
            vec![],
        )
        .await?;
    let after_forward =
        next_promise_checkpoint(&mut promise_arrivals, "secret-policy-after-forward").await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Suspended,
            std::time::Duration::from_secs(30),
        )
        .await?;
    executor.simulated_crash(&worker_id).await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: after_forward.oplog_idx,
            },
            vec![],
        )
        .await?;
    let evidence = tokio::time::timeout(std::time::Duration::from_secs(30), invocation)
        .await
        .expect("pinned secret-policy invocation completes after restart")?
        .into_typed::<SecretPolicyEvidence>()?;
    assert!(
        evidence.leaf_revealed,
        "leaf authority must remain independent"
    );
    assert_eq!(evidence.middleware.len(), 3);
    let restricted = evidence
        .middleware
        .iter()
        .find(|observation| observation.label == "restricted")
        .unwrap();
    assert_eq!(
        restricted,
        &SecretPolicyObservation {
            label: "restricted".to_string(),
            config_resolved: false,
            configured_secret_revealed: false,
            input_secret_revealed: false,
        }
    );
    let allowed = evidence
        .middleware
        .iter()
        .find(|observation| observation.label == "allowed")
        .unwrap();
    assert_eq!(
        allowed,
        &SecretPolicyObservation {
            label: "allowed".to_string(),
            config_resolved: true,
            configured_secret_revealed: true,
            input_secret_revealed: true,
        }
    );
    let universal = evidence
        .middleware
        .iter()
        .find(|observation| observation.label == "universal")
        .unwrap();
    assert_eq!(
        universal,
        &SecretPolicyObservation {
            label: "universal".to_string(),
            config_resolved: true,
            configured_secret_revealed: false,
            input_secret_revealed: false,
        }
    );
    // Together these observations exercise all four operation shapes independently: restricted
    // hold/pass, universal resolve-without-reveal, leaf reveal of a received handle, and the
    // allowed occurrence's resolve-plus-reveal.
    assert!(!restricted.config_resolved && evidence.leaf_revealed);
    assert!(universal.config_resolved && !universal.configured_secret_revealed);
    assert!(allowed.config_resolved && allowed.configured_secret_revealed);
    let replayed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let host_starts = |function_fragment: &str| {
        replayed_oplog
            .iter()
            .filter(|entry| {
                matches!(
                    &entry.entry,
                    PublicOplogEntry::Start(start)
                        if start.function_name.contains(function_fragment)
                ) && matches!(
                    &entry.attribution,
                    PublicOplogEntryAttribution::Entity(entity)
                        if entity.invocation.entity.kind == PublicAgentEntityKind::ToolMiddleware
                )
            })
            .collect::<Vec<_>>()
    };
    let config_starts = host_starts("config");
    let reveal_starts = host_starts("reveal");
    assert_eq!(config_starts.len(), 3, "one config call per occurrence");
    assert_eq!(
        reveal_starts.len(),
        5,
        "restricted input, universal configured/input, and allowed configured/input reveals"
    );
    let reveal_start_indices = reveal_starts
        .iter()
        .map(|entry| entry.oplog_index)
        .collect::<BTreeSet<_>>();
    for start in config_starts.into_iter().chain(reveal_starts) {
        let terminals = replayed_oplog
            .iter()
            .filter(|entry| {
                matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == start.oplog_index)
                    || matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == start.oplog_index)
            })
            .count();
        assert_eq!(
            terminals, 1,
            "completed secret host call {} must replay exactly once",
            start.oplog_index
        );
    }

    let fresh = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        executor.invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "inspect_secret_policy",
            data_value!(),
        ),
    )
    .await
    .expect("fresh secret-policy invocation uses the changed deployment")?
    .into_typed::<SecretPolicyEvidence>()?;
    assert!(fresh.leaf_revealed);
    assert_eq!(
        fresh.middleware,
        vec![
            SecretPolicyObservation {
                label: "changed-second".to_string(),
                config_resolved: false,
                configured_secret_revealed: false,
                input_secret_revealed: false,
            },
            SecretPolicyObservation {
                label: "changed-first".to_string(),
                config_resolved: true,
                configured_secret_revealed: true,
                input_secret_revealed: true,
            },
            SecretPolicyObservation {
                label: "universal".to_string(),
                config_resolved: true,
                configured_secret_revealed: false,
                input_secret_revealed: false,
            },
        ]
    );

    let public_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let reveal_audits = public_oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if reveal_start_indices.contains(&end.start_index) => {
                let response = end
                    .response
                    .clone()
                    .expect("secret reveal response payload");
                let response_json = serde_json::to_string(&response)
                    .expect("serialize public secret reveal response");
                let response =
                    golem_common::model::oplog::host_functions::host_response_from_typed_schema_value(
                        "golem::secrets::reveal::reveal",
                        response,
                    )
                    .expect("decode public secret reveal response");
                let golem_common::model::oplog::payload::HostResponse::SecretRevealed(response) =
                    response
                else {
                    panic!("expected secret reveal response")
                };
                Some((response.audit, response_json))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(reveal_audits.len(), reveal_start_indices.len());
    assert!(
        reveal_audits
            .iter()
            .all(|(audit, _)| audit.calling_agent == worker_id),
        "every duplicate middleware reveal outcome must retain the calling owner: {reveal_audits:?}"
    );
    let serialized_agent_id = serde_json::to_string(&agent_id.to_string())?;
    assert!(
        reveal_audits
            .iter()
            .all(|(_, response)| response.contains(&serialized_agent_id)),
        "every CLI-shaped reveal response must retain the calling owner"
    );
    assert!(
        !format!("{public_oplog:?}").contains(plaintext),
        "public oplog rendering must not contain secret plaintext"
    );
    let cli_shaped = serde_json::to_string(
        &public_oplog
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "index": entry.oplog_index,
                    "attribution": entry.attribution,
                    "entry": entry.entry,
                })
            })
            .collect::<Vec<_>>(),
    )?;
    assert!(
        !cli_shaped.contains(plaintext),
        "CLI-shaped oplog JSON must not contain secret plaintext"
    );
    assert!(
        cli_shaped.contains("toolSecret"),
        "configured-secret reveal audit payloads must retain the canonical config key"
    );
    promise_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn shipped_audit_duplicate_occurrences_pass_opaque_secret_and_record_safe_attribution(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::model::agent_secret::{
        AgentSecretId, AgentSecretRevision, CanonicalAgentSecretPath,
    };
    use golem_service_base::model::agent_secret::AgentSecret;

    let (sink_url, sink, sink_task) = start_audit_sink(false).await;
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let plaintext = "audit-must-never-record-this-plaintext";
    environment.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: context.default_environment_id,
        path: CanonicalAgentSecretPath(vec!["toolSecret".to_string()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String(plaintext.to_string())),
    });
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let audit_component = executor
        .component_dep(&context.default_environment_id, audit)
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let audit_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("../builtin-tools/audit-middleware.wasm"),
        false,
        true,
    )
    .await?;
    let definition = audit_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "audit")
        .expect("shipped audit middleware metadata");
    let agent_type = AgentTypeName("ToolSecretCaller".to_string());
    let tool_name = ToolName::try_from("secret-policy-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        audit_component.id,
        audit_component.revision,
        "golem:audit-middleware",
        &audit_metadata.tool_middlewares,
        vec![
            (
                definition.name.as_str(),
                audit_middleware_parameters(definition, "security", &sink_url),
            ),
            (
                definition.name.as_str(),
                audit_middleware_parameters(definition, "operations", &sink_url),
            ),
        ],
    );
    let occurrences = &mut deployment
        .tool_middleware_chains
        .get_mut(&ToolBindingOwner::AgentType {
            agent_type_name: agent_type.clone(),
        })
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences;
    for occurrence in occurrences.iter_mut() {
        occurrence.secret_keys_readable = SecretKeyScope::Keys(Default::default());
        occurrence.secret_keys_revealable = SecretKeyScope::Keys(Default::default());
    }
    assert!(occurrences.iter().all(|occurrence| {
        occurrence.secret_keys_readable == SecretKeyScope::Keys(Default::default())
            && occurrence.secret_keys_revealable == SecretKeyScope::Keys(Default::default())
    }));
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("ToolSecretCaller", "shipped-audit-secret");
    let evidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "inspect_secret_policy",
            data_value!(),
        )
        .await?
        .into_typed::<SecretPolicyEvidence>()?;
    assert!(
        evidence.leaf_revealed,
        "leaf independently consumes the secret"
    );
    assert!(
        evidence.middleware.is_empty(),
        "audit does not alter the result"
    );

    let committed = sink.committed.lock().unwrap();
    assert_eq!(committed.len(), 2, "each duplicate occurrence commits once");
    let mut labels = committed
        .values()
        .map(|record| record["occurrenceLabel"].as_str().unwrap())
        .collect::<Vec<_>>();
    labels.sort_unstable();
    assert_eq!(labels, ["operations", "security"]);
    for (key, record) in committed.iter() {
        assert_eq!(record["version"], 1);
        assert_eq!(record["policyInvocationKey"], key.as_str());
        assert_eq!(record["toolName"], "secret-policy-probe");
        assert_eq!(record["input"]["secretHandles"], 1);
        assert_eq!(record["outcome"]["kind"], "success");
        assert_eq!(record["principal"]["kind"], "anonymous");
        assert!(
            record["owner"]["agentId"]
                .as_str()
                .is_some_and(|owner| owner.contains("shipped-audit-secret"))
        );
        assert!(!record.to_string().contains(plaintext));
    }
    let keys = committed.keys().collect::<Vec<_>>();
    assert_ne!(keys[0], keys[1], "occurrences have distinct logical keys");
    drop(committed);
    assert_eq!(sink.attempts.lock().unwrap().len(), 2);
    sink_task.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn shipped_audit_sink_deduplicates_crash_after_commit_and_keeps_pinned_policy(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (sink_url, sink, sink_task) = start_audit_sink(true).await;
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment.clone()),
        ..Default::default()
    };
    let mut executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let audit_component = executor
        .component_dep(&context.default_environment_id, audit)
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let audit_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("../builtin-tools/audit-middleware.wasm"),
        false,
        true,
    )
    .await?;
    let definition = audit_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "audit")
        .expect("shipped audit middleware metadata");
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        audit_component.id,
        audit_component.revision,
        "golem:audit-middleware",
        &audit_metadata.tool_middlewares,
        vec![(
            definition.name.as_str(),
            audit_middleware_parameters(definition, "pinned-before-crash", &sink_url),
        )],
    );
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment.clone()),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "audit-crash-window");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let invocation_key = IdempotencyKey::fresh();
    let invocation = {
        let executor = executor.clone();
        let caller_component = caller_component.clone();
        let agent_id = agent_id.clone();
        let invocation_key = invocation_key.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &caller_component,
                    &agent_id,
                    &invocation_key,
                    "middleware_probe_once",
                    data_value!("logical-call"),
                )
                .await
        })
    };
    sink.committed_while_blocked.notified().await;
    assert_eq!(sink.attempts.lock().unwrap().len(), 1);
    assert_eq!(sink.committed.lock().unwrap().len(), 1);

    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    deployment
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences[0]
        .parameters = audit_middleware_parameters(definition, "changed-after-crash", &sink_url);
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    invocation.abort();
    let _ = invocation.await;
    drop(executor);
    executor = start_with_overrides(deps, &context, overrides).await?;
    let result: String = executor
        .invoke_and_await_agent_with_key(
            &caller_component,
            &agent_id,
            &invocation_key,
            "middleware_probe_once",
            data_value!("logical-call"),
        )
        .await?
        .into_typed()?;
    assert_eq!(result, "leaf(logical-call)");

    let attempts = sink.attempts.lock().unwrap();
    assert_eq!(
        attempts.len(),
        2,
        "delivery attempt repeats after the crash"
    );
    assert_eq!(attempts[0].0, attempts[1].0, "logical key is replay-stable");
    assert_eq!(
        attempts[1].1["occurrenceLabel"], "pinned-before-crash",
        "reconstruction uses the admitted policy occurrence"
    );
    drop(attempts);
    assert_eq!(
        sink.committed.lock().unwrap().len(),
        1,
        "sink commits one logical record"
    );

    let duplicate: String = executor
        .invoke_and_await_agent_with_key(
            &caller_component,
            &agent_id,
            &invocation_key,
            "middleware_probe_once",
            data_value!("logical-call"),
        )
        .await?
        .into_typed()?;
    assert_eq!(duplicate, "leaf(logical-call)");
    assert_eq!(
        sink.attempts.lock().unwrap().len(),
        2,
        "completed replay does not contact the sink"
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        entity_starts(&oplog, PublicAgentEntityKind::ToolMiddleware, "audit").len(),
        1,
        "one logical audit entity invocation"
    );
    sink_task.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn universal_middleware_preserves_native_modes_effects_and_completed_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let mut streaming = native_streaming_tool_metadata();
    streaming.commands.nodes[0].name = "native-streaming".to_string();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            native_tool_metadata: Some(streaming.clone()),
            ..Default::default()
        },
    )
    .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_it_tool_streaming_rust_middleware_release",
        )
        .name("golem-it:tool-streaming-native-middleware-acceptance")
        .store()
        .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_it_tool_streaming_rust_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let universal = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .expect("streaming universal middleware fixture");
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let tool_name = ToolName::try_from("native-streaming").unwrap();
    let mut deployment = native_deployment_state(
        context.account_id,
        agent_type.0.as_str(),
        streaming,
        native_test_tool_metadata(),
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &tool_name,
        middleware_component.id,
        middleware_component.revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            universal.name.as_str(),
            empty_middleware_parameters(universal),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "native-middleware-acceptance");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let helper_effects_before = executor.native_test_helper_effect_count();
    let evidence: Vec<String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "native_modes_stream_cancel_overlap",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        &evidence[..6],
        [
            "sync",
            "fire-and-forget",
            "async",
            "stream",
            "cancel",
            "overlap"
        ]
    );
    assert_eq!(evidence[6], "5");
    let helper_effects = executor.native_test_helper_effect_count();
    assert_eq!(helper_effects, helper_effects_before + 1);

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let native_leaves = oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(parameters)
                    if parameters.function_name == "golem::entity::invoke"
            ) && matches!(
                &entry.attribution,
                PublicOplogEntryAttribution::Entity(entity)
                    if entity.invocation.entity.kind == PublicAgentEntityKind::Tool
                        && entity.invocation.entity.name == "native-streaming"
                        && entity.ancestors.last().is_some_and(|ancestor|
                            ancestor.entity.kind == PublicAgentEntityKind::ToolMiddleware
                                && ancestor.entity.name == "streaming-universal-pass-through")
            )
        })
        .count();
    assert!(
        native_leaves >= 6,
        "every native call mode must enter through middleware"
    );

    executor.simulated_crash(&worker_id).await?;
    executor.resume(&worker_id, true).await?;
    let count: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "native_effect_count",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(count, "5");
    assert_eq!(executor.native_test_helper_effect_count(), helper_effects);

    let replayed_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let replayed_native_leaves = replayed_oplog
        .iter()
        .filter(|entry| {
            matches!(
                &entry.entry,
                PublicOplogEntry::Start(parameters)
                    if parameters.function_name == "golem::entity::invoke"
            ) && matches!(
                &entry.attribution,
                PublicOplogEntryAttribution::Entity(entity)
                    if entity.invocation.entity.kind == PublicAgentEntityKind::Tool
                        && entity.invocation.entity.name == "native-streaming"
                        && entity.ancestors.last().is_some_and(|ancestor|
                            ancestor.entity.kind == PublicAgentEntityKind::ToolMiddleware
                                && ancestor.entity.name == "streaming-universal-pass-through")
            )
        })
        .count();
    assert_eq!(replayed_native_leaves, native_leaves + 1);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn output_redaction_preserves_structured_stream_error_and_completed_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let middleware_component = executor
        .component(
            &context.default_environment_id,
            "golem_output_redaction_release",
        )
        .name("golem:output-redaction-acceptance")
        .store()
        .await?;
    let provider_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    let middleware_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join("golem_output_redaction_release.wasm"),
        false,
        true,
    )
    .await?;
    let redaction = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "output-redaction")
        .expect("output-redaction middleware metadata");
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let mut deployment = deployment_state(
        context.account_id,
        provider_component.id,
        provider_component.revision,
        "golem-it:tool-streaming-rust-provider",
        agent_type.0.as_str(),
        provider_metadata.tools,
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &ToolName::try_from("middleware-probe").unwrap(),
        middleware_component.id,
        middleware_component.revision,
        "golem:output-redaction",
        &middleware_metadata.tool_middlewares,
        vec![(
            redaction.name.as_str(),
            output_redaction_parameters(redaction, vec![("$", "secret", "[redacted]")], vec![]),
        )],
    );
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &ToolName::try_from("streaming").unwrap(),
        middleware_component.id,
        middleware_component.revision,
        "golem:output-redaction",
        &middleware_metadata.tool_middlewares,
        vec![(
            redaction.name.as_str(),
            output_redaction_parameters(redaction, vec![], vec![("marker:", "[redacted]")]),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "output-redaction");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let structured: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_once",
            data_value!("visible-secret"),
        )
        .await?
        .into_typed()?;
    assert_eq!(structured, "leaf(visible-[redacted])");

    let declared: RedactionEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "redaction_stream_case",
            data_value!("declared-error"),
        )
        .await?
        .into_typed()?;
    assert_eq!(declared.output, b"[redacted]");
    assert_eq!(declared.stdout_terminal, "finished");
    assert_eq!(declared.stderr_terminal, "absent");
    assert!(declared.outcome.contains("Declared"));

    let failed: RedactionEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "redaction_stream_case",
            data_value!("stream-failure-success"),
        )
        .await?
        .into_typed()?;
    assert_eq!(failed.output, b"[redacted]");
    assert_eq!(
        failed.stdout_terminal,
        "failed:ByteStreamFailure::Failed(\"stream producer failed\")"
    );
    assert_eq!(failed.stderr_terminal, "absent");
    assert_eq!(failed.outcome, "ok");

    let dual: RedactionEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "redaction_dual_case",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(dual.output, b"[redacted]");
    assert_eq!(dual.stdout_terminal, "finished");
    assert_eq!(dual.stderr, b"stderr-visible");
    assert_eq!(dual.stderr_terminal, "finished");
    assert_eq!(dual.outcome, "ok");

    let before_replay = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let leaf_starts = entity_starts(&before_replay, PublicAgentEntityKind::Tool, "streaming").len();
    assert_eq!(leaf_starts, 3);
    executor.simulated_crash(&worker_id).await?;
    executor.resume(&worker_id, true).await?;
    let reconstructed: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "read_owner_file",
            data_value!("/redaction-replay-output"),
        )
        .await?
        .into_typed()?;
    assert_eq!(reconstructed.as_bytes(), b"[redacted]");
    let after_replay = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        entity_starts(&after_replay, PublicAgentEntityKind::Tool, "streaming").len(),
        leaf_starts,
        "completed replay must not rerun either streaming leaf"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn middleware_race_cancels_loser_drops_observer_and_settles_all_siblings(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-race-settlement"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut arrivals) =
        start_crash_checkpoint_server().await;
    let (promise_port, promise_server, mut promise_arrivals) =
        start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-race-settlement");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
                (
                    "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".to_string(),
                    promise_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("asymmetric-17"),
    );
    tokio::pin!(invocation);
    let cancelled = tokio::select! {
        result = invocation.as_mut() => panic!("race settled before loser admission: {result:?}"),
        arrival = next_crash_checkpoint(&mut arrivals, "middleware-race-cancelled") => arrival?,
    };
    let cancel_ready =
        next_promise_checkpoint(&mut promise_arrivals, "middleware-race-cancel-ready").await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: cancel_ready.oplog_idx,
            },
            vec![],
        )
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor
                .active_entity_metadata(&owner)
                .await
                .is_some_and(|active| {
                    active.tool_operations.operations.iter().any(|operation| {
                        matches!(
                            operation.winner,
                            ToolOperationWinnerMetadata::SelectingCancelled
                                | ToolOperationWinnerMetadata::Cancelled
                        )
                    })
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for loser cancellation selection"))?;
    drop(cancelled);
    let detached = tokio::select! {
        result = invocation.as_mut() => panic!("parent settled before detached child: {result:?}"),
        arrival = next_crash_checkpoint(&mut arrivals, "middleware-race-detached") => arrival?,
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), invocation.as_mut())
            .await
            .is_err()
    );
    detached
        .release
        .send(())
        .expect("release detached observer child");
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(result, "race-winner[leaf(race-sibling(asymmetric-17))]");

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let leaf_starts = oplog
        .iter()
        .filter_map(|entry| match (&entry.entry, &entry.attribution) {
            (PublicOplogEntry::Start(parameters), PublicOplogEntryAttribution::Entity(entity))
                if parameters.function_name == "golem::entity::invoke"
                    && entity.invocation.entity.kind == PublicAgentEntityKind::Tool
                    && entity.invocation.entity.name == "middleware-probe" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        leaf_starts.len(),
        3,
        "cancelled, detached, and successful siblings are admitted"
    );
    assert!(leaf_starts.iter().all(|start| oplog.iter().any(|entry|
        matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == *start)
            || matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if cancelled.start_index == *start)
    )), "every admitted sibling must have settled before the parent result");
    assert_eq!(oplog.iter().filter(|entry| matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if leaf_starts.contains(&cancelled.start_index))).count(), 1);
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("race invocation finished")
        .oplog_index;
    for start in leaf_starts {
        assert_one_terminal_before_finished(&oplog, start, finished);
    }
    let middleware_starts = entity_starts(
        &oplog,
        PublicAgentEntityKind::ToolMiddleware,
        "streaming-race-settlement",
    );
    assert_eq!(middleware_starts.len(), 1);
    assert_one_terminal_before_finished(&oplog, middleware_starts[0].oplog_index, finished);
    checkpoint_server.abort();
    promise_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn filesystem_capable_incapable_capable_chain_serializes_fanout_lanes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &[
            "streaming-filesystem-outer",
            "streaming-incapable-fanout",
            "streaming-filesystem-inner"
        ],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let chain = deployment
        .tool_middleware_chains
        .get_mut(&ToolBindingOwner::AgentType {
            agent_type_name: agent_type.clone(),
        })
        .unwrap()
        .get_mut(&tool_name)
        .unwrap();
    for occurrence in &mut chain.occurrences {
        if occurrence.middleware.definition.name != "streaming-incapable-fanout" {
            occurrence.filesystem_access = ToolFilesystemAccess::Allowed;
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-filesystem-lanes");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let result: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_once",
            data_value!("seed-29"),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        result,
        "fanout[leaf(fs-inner(fanout-left(fs-outer(seed-29))))|leaf(fs-inner(fanout-right(fs-outer(seed-29))))]"
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/middleware-lanes.log")
            .await?,
        b"OLR".as_slice()
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn incapable_middleware_preserves_capable_streaming_staging_and_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-universal-pass-through"],
        context,
        environment_state,
        executor,
        _provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let middleware = deployment
        .registered_tool_middlewares
        .values()
        .next()
        .unwrap();
    let ToolMiddlewareSource::Component {
        component_id,
        component_revision,
        ..
    } = middleware.source.clone();
    let definition = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-universal-pass-through")
        .unwrap();
    install_middleware_chain(
        &mut deployment,
        &agent_type,
        &ToolName::try_from("capable-streaming").unwrap(),
        component_id,
        component_revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(
            definition.name.as_str(),
            empty_middleware_parameters(definition),
        )],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-capable-stream-staging");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let input = b"capable-inner-through-incapable-outer-37".to_vec();
    let result: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("/decorated-capable.bin", input.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&result, &input, 1, input.len() as u64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/decorated-capable.bin")
            .await?,
        input
    );
    let dual_size = 97_u64;
    let dual: Vec<Vec<u8>> = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable_dual",
            data_value!("/decorated-capable-dual.bin", dual_size),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        dual,
        vec![
            vec![b'o'; dual_size as usize],
            vec![b'e'; dual_size as usize]
        ]
    );
    executor.simulated_crash(&worker_id).await?;
    let next = b"fresh-after-reconstruction-19".to_vec();
    let result: StreamEvidence = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "collect_capable",
            data_value!("/decorated-capable-next.bin", next.clone()),
        )
        .await?
        .into_typed()?;
    assert_evidence(&result, &next, 1, next.len() as u64);
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/decorated-capable.bin")
            .await?,
        input
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .unwrap()
        .oplog_index;
    for start in entity_starts(&oplog, PublicAgentEntityKind::Tool, "capable-streaming")
        .into_iter()
        .chain(entity_starts(
            &oplog,
            PublicAgentEntityKind::ToolMiddleware,
            "streaming-universal-pass-through",
        ))
    {
        assert_one_terminal_before_finished(&oplog, start.oplog_index, finished);
    }
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn rate_1_enforces_boundary_before_dispatch_and_partitions_principals(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("rate_limit_middleware")] rate_limit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_rate_limit_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        rate_limit,
        "rate-1",
        3,
        context,
        environment,
        executor,
        provider_component,
        caller_component,
        rate_limit_component,
        agent_type
    );
    let agent_id = agent_id!("ToolStreamingCaller", "rate-1-owner");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let principal_a = oidc_principal("principal-a");

    for ordinal in 1..=3 {
        let result: String = executor
            .invoke_and_await_agent_as_principal(
                &caller_component,
                &agent_id,
                principal_a.clone(),
                "middleware_probe_once",
                data_value!(format!("a-{ordinal}")),
            )
            .await?
            .into_typed()?;
        assert_eq!(result, format!("leaf(a-{ordinal})"));
    }
    let rejected = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &agent_id,
            principal_a,
            "middleware_probe_once",
            data_value!("a-4"),
        )
        .await;
    assert!(rejected.is_err(), "call N+1 must be rejected");

    let independent_agent_id = agent_id!("ToolStreamingCaller", "rate-1-independent-owner");
    let independent_worker_id = executor
        .start_agent(&caller_component.id, independent_agent_id.clone())
        .await?;
    let independent: String = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &independent_agent_id,
            oidc_principal("principal-b"),
            "middleware_probe_once",
            data_value!("b-1"),
        )
        .await?
        .into_typed()?;
    assert_eq!(independent, "leaf(b-1)");

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let independent_oplog = executor
        .get_oplog(&independent_worker_id, OplogIndex::INITIAL)
        .await?;
    assert_eq!(
        entity_starts(&oplog, PublicAgentEntityKind::Tool, "middleware-probe").len()
            + entity_starts(
                &independent_oplog,
                PublicAgentEntityKind::Tool,
                "middleware-probe"
            )
            .len(),
        4,
        "the rejected call must not reach the wrapped tool"
    );
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn rate_2_distinct_concurrent_owners_share_one_atomic_principal_limit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("rate_limit_middleware")] rate_limit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const LIMIT: usize = 11;
    const OWNERS: usize = 32;
    setup_rate_limit_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        rate_limit,
        "rate-2",
        LIMIT as u64,
        context,
        environment,
        executor,
        provider_component,
        caller_component,
        rate_limit_component,
        agent_type
    );
    let mut owners = Vec::with_capacity(OWNERS);
    for ordinal in 0..OWNERS {
        let agent_id = agent_id!("ToolStreamingCaller", format!("rate-2-owner-{ordinal}"));
        let worker_id = executor
            .start_agent(&caller_component.id, agent_id.clone())
            .await?;
        owners.push((agent_id, worker_id));
    }

    let barrier = Arc::new(tokio::sync::Barrier::new(OWNERS));
    let principal = oidc_principal("shared-principal");
    let calls = owners.iter().enumerate().map(|(ordinal, (agent_id, _))| {
        let barrier = barrier.clone();
        let principal = principal.clone();
        let executor = &executor;
        let caller_component = &caller_component;
        let agent_id = agent_id.clone();
        async move {
            barrier.wait().await;
            executor
                .invoke_and_await_agent_as_principal(
                    caller_component,
                    &agent_id,
                    principal,
                    "middleware_probe_once",
                    data_value!(format!("owner-{ordinal}")),
                )
                .await
        }
    });
    let results = futures::future::join_all(calls).await;
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        LIMIT
    );

    let mut leaf_calls = 0;
    for (_, worker_id) in &owners {
        let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        leaf_calls += entity_starts(&oplog, PublicAgentEntityKind::Tool, "middleware-probe").len();
    }
    assert_eq!(leaf_calls, LIMIT, "atomic admission must not lose updates");

    let backend_id = agent_id!("RateLimitBackend", "rate-2");
    let stats: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(stats.attempts, OWNERS as u64);
    assert_eq!(stats.committed_charges, LIMIT as u64);
    assert_eq!(stats.recorded_decisions, OWNERS as u64);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn rate_3_crash_after_leaf_effect_replays_without_an_extra_charge_or_effect(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("rate_limit_middleware")] rate_limit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_rate_limit_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        rate_limit,
        "rate-3",
        2,
        context,
        environment,
        executor,
        provider_component,
        caller_component,
        rate_limit_component,
        agent_type
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut arrivals) =
        start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "rate-3-owner");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                    effect_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent_as_principal(
        &caller_component,
        &agent_id,
        oidc_principal("replay-principal"),
        "middleware_probe_once",
        data_value!("rate-limit-crash(first)"),
    );
    tokio::pin!(invocation);
    let effect = tokio::select! {
        effect = effects.recv() => effect.expect("rate-limit leaf effect"),
        result = invocation.as_mut() => panic!("rate-limit call settled before its leaf checkpoint: {result:?}"),
    };
    assert_eq!(effect, "rate-limit-crash(first)");
    let _original = tokio::select! {
        checkpoint = next_crash_checkpoint(&mut arrivals, "rate-limit-leaf-after-effect") => checkpoint?,
        result = invocation.as_mut() => panic!("rate-limit call settled before its leaf checkpoint: {result:?}"),
    };

    let backend_id = agent_id!("RateLimitBackend", "rate-3");
    let before: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(before.attempts, 1);
    assert_eq!(before.committed_charges, 1);

    executor.simulated_crash(&worker_id).await?;
    let replay = next_crash_checkpoint(&mut arrivals, "rate-limit-leaf-after-effect").await?;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), effects.recv())
            .await
            .is_err(),
        "the committed leaf effect must not repeat during reconstruction"
    );
    replay.release.send(()).expect("release replayed leaf");
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(result, "leaf(rate-limit-crash(first))");

    let after_replay: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(after_replay.attempts, 1);
    assert_eq!(after_replay.committed_charges, 1);

    let fresh: String = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &agent_id,
            oidc_principal("replay-principal"),
            "middleware_probe_once",
            data_value!("fresh"),
        )
        .await?
        .into_typed()?;
    assert_eq!(fresh, "leaf(fresh)");
    let after_fresh: RateLimitBackendStats = executor
        .invoke_and_await_agent(&rate_limit_component, &backend_id, "stats", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(after_fresh.attempts, 2);
    assert_eq!(after_fresh.committed_charges, 2);
    assert_eq!(after_fresh.recorded_decisions, 2);

    checkpoint_server.abort();
    effect_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn mixed_chain_owner_eviction_reconstructs_durable_state_with_fresh_sidecars(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-universal-pass-through", "streaming-lifecycle"],
        context,
        environment_state,
        executor,
        _provider_component,
        caller_component,
        _middleware_metadata,
        agent_type,
        deployment
    );
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    deployment
        .tool_middleware_chains
        .get_mut(&ToolBindingOwner::AgentType {
            agent_type_name: agent_type,
        })
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences[1]
        .filesystem_access = ToolFilesystemAccess::Allowed;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-owner-eviction");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([(
                "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                effect_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    let mut disposals = executor.probe_entity_store_disposal(&worker_id);

    for (input, expected_loads) in [("evict(first)", 1), ("evict(second)", 2)] {
        let result: String = executor
            .invoke_and_await_agent(
                &caller_component,
                &agent_id,
                "middleware_probe_once",
                data_value!(input),
            )
            .await?
            .into_typed()?;
        assert_eq!(result, format!("leaf(lifecycle-effect({input}))"));
        let expected_effect = format!("lifecycle-effect({input})");
        assert_eq!(
            effects.recv().await.as_deref(),
            Some(expected_effect.as_str())
        );
        for _ in 0..3 {
            tokio::time::timeout(std::time::Duration::from_secs(30), disposals.recv())
                .await?
                .expect("universal, monomorphic, and leaf Stores are physically disposed");
        }
        assert_eq!(executor.instance_load_count(&worker_id), expected_loads);

        if input == "evict(first)" {
            executor
                .wait_for_status(
                    &worker_id,
                    AgentStatus::Idle,
                    std::time::Duration::from_secs(30),
                )
                .await?;
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                while executor.worker_eviction_class(&owner).await
                    != Some(golem_worker_executor::worker::EvictionClass::LoadedIdle)
                {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            })
            .await
            .map_err(|_| anyhow::anyhow!("owner did not become loaded-idle after settlement"))?;
            assert!(executor.stop_worker_if_idle(&owner).await?);
            assert!(!executor.worker_is_loaded(&owner).await);
            executor.retire_unloaded_worker(&owner).await?;
            assert!(!executor.worker_is_cached(&owner).await);
        }
    }

    assert_eq!(
        executor.get_worker_metadata(&worker_id).await?.fingerprint,
        fingerprint
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/middleware-lifecycle.log")
            .await?,
        b"PP".as_slice(),
        "owner-backed state survives eviction while each sidecar Store is disposable"
    );
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| matches!(&entry.entry, PublicOplogEntry::Error(_)))
            .count(),
        0,
        "reconstruction must not use stale sidecar resources"
    );
    let entity_starts = oplog
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entity_starts.len(),
        6,
        "three fresh sidecars per invocation"
    );
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .unwrap()
        .oplog_index;
    for start in entity_starts {
        assert_one_terminal_before_finished(&oplog, start.oplog_index, finished);
    }
    effect_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn mixed_chain_suspends_before_leaf_and_after_effect_without_repeating_dispatch(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-universal-pass-through", "streaming-lifecycle"],
        context,
        environment_state,
        executor,
        _provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let original_deployment = deployment.clone();
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let (promise_port, promise_server, mut arrivals) = start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-mixed-suspension");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                    effect_port.to_string(),
                ),
                (
                    "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".to_string(),
                    promise_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("suspend(asymmetric-61)"),
    );
    tokio::pin!(invocation);
    let before_leaf = tokio::select! {
        result = invocation.as_mut() => panic!("mixed chain settled before leaf dispatch: {result:?}"),
        checkpoint = next_promise_checkpoint(&mut arrivals, "lifecycle-before-leaf") => checkpoint?,
    };
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Suspended,
            std::time::Duration::from_secs(30),
        )
        .await?;
    let before_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        entity_starts(
            &before_oplog,
            PublicAgentEntityKind::Tool,
            "middleware-probe"
        )
        .is_empty()
    );
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    let short = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-short-circuit")
        .unwrap();
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let chain = &original_deployment.tool_middleware_chains[&ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    }][&tool_name];
    let (middleware_component_id, middleware_component_revision) =
        match &chain.occurrences[0].middleware.source {
            ToolMiddlewareSource::Component {
                component_id,
                component_revision,
                ..
            } => (*component_id, *component_revision),
        };
    let mut changed = original_deployment;
    install_middleware_chain(
        &mut changed,
        &agent_type,
        &tool_name,
        middleware_component_id,
        middleware_component_revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(short.name.as_str(), empty_middleware_parameters(short))],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(changed),
    );
    executor.simulated_crash(&worker_id).await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: before_leaf.oplog_idx,
            },
            vec![],
        )
        .await?;
    assert_eq!(
        effects.recv().await.as_deref(),
        Some("lifecycle-effect(suspend(asymmetric-61))")
    );
    let after_leaf = next_promise_checkpoint(&mut arrivals, "lifecycle-after-leaf").await?;
    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Suspended,
            std::time::Duration::from_secs(30),
        )
        .await?;
    let after_effect_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let leaf_starts = entity_starts(
        &after_effect_oplog,
        PublicAgentEntityKind::Tool,
        "middleware-probe",
    );
    assert_eq!(leaf_starts.len(), 1);
    assert!(after_effect_oplog.iter().any(
        |entry| matches!(&entry.entry, PublicOplogEntry::End(end) if end.start_index == leaf_starts[0].oplog_index)
    ));

    executor.simulated_crash(&worker_id).await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: after_leaf.oplog_idx,
            },
            vec![],
        )
        .await?;
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(
        result, "lifecycle(leaf(lifecycle-effect(suspend(asymmetric-61))))",
        "both reconstruction windows retain the pinned U/P/leaf plan"
    );
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let fresh: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_once",
            data_value!("fresh"),
        )
        .await?
        .into_typed()?;
    assert_eq!(fresh, "short(fresh)");
    effect_server.abort();
    promise_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn mixed_chain_trap_cascades_through_detached_leaf_and_all_ancestor_stores(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-universal-pass-through", "streaming-lifecycle"],
        context,
        environment_state,
        executor,
        _provider_component,
        caller_component,
        _middleware_metadata,
        _agent_type,
        deployment,
        configure = Arc::new(|config| {
            config.retry = RetryConfig {
                max_attempts: 1,
                min_delay: std::time::Duration::from_millis(1),
                max_delay: std::time::Duration::from_millis(1),
                multiplier: 1.0,
                max_jitter_factor: None,
            };
        })
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut checkpoints) =
        start_crash_checkpoint_server().await;
    let (promise_port, promise_server, mut promises) = start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-mixed-cascade");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
                (
                    "MIDDLEWARE_PROMISE_CHECKPOINT_PORT".to_string(),
                    promise_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let owner = OwnedAgentId::new(context.default_environment_id, &worker_id);
    let mut disposals = executor.probe_entity_store_disposal(&worker_id);
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("cascade(smoke-73)"),
    );
    tokio::pin!(invocation);
    let _blocked = tokio::select! {
        result = invocation.as_mut() => panic!("mixed cascade settled before its child blocked: {result:?}"),
        checkpoint = next_crash_checkpoint(&mut checkpoints, "cascade-blocked-leaf") => checkpoint?,
    };
    let admitted = next_promise_checkpoint(&mut promises, "cascade-child-admitted").await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: admitted.oplog_idx,
            },
            vec![],
        )
        .await?;
    let error = invocation
        .await
        .expect_err("the mixed leaf trap must fail its owner invocation");
    assert!(
        error.to_string().contains("mixed lifecycle cascade trap"),
        "the selected leaf trap must reach the owner boundary: {error:?}"
    );

    let mut destroyed = Vec::new();
    for _ in 0..4 {
        destroyed.push(
            disposals
                .try_recv()
                .expect("all admitted U/P/leaf Stores are destroyed before failure notification"),
        );
    }
    destroyed.sort();
    assert!(disposals.try_recv().is_err());
    if let Some(active) = executor.active_entity_metadata(&owner).await {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.lane.holder.is_none());
        assert_eq!(active.lane.active_invocation_count, 0);
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let invocation_start = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .unwrap()
        .oplog_index;
    let starts = oplog
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name == "golem::entity::invoke"
                    && entry.oplog_index > invocation_start =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        starts.len(),
        4,
        "U, P, detached leaf, and trapping leaf are admitted"
    );
    let mut sorted_starts = starts.clone();
    sorted_starts.sort();
    assert_eq!(destroyed, sorted_starts);
    assert!(oplog.iter().all(|entry| {
        !matches!(&entry.entry, PublicOplogEntry::End(end) if starts.contains(&end.start_index))
            && !matches!(&entry.entry, PublicOplogEntry::Cancelled(cancelled) if starts.contains(&cancelled.start_index))
    }), "owner fencing must not fabricate a normal entity terminal");
    assert_eq!(
        oplog
            .iter()
            .filter(|entry| entry.oplog_index > invocation_start
                && matches!(entry.entry, PublicOplogEntry::Error(_)))
            .count(),
        1
    );
    checkpoint_server.abort();
    promise_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn partial_fanout_restart_replays_completed_child_repairs_pending_and_keeps_pinned_plan(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-partial-fanout"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let original_deployment = deployment.clone();
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let (checkpoint_port, checkpoint_gate_port, checkpoint_server, mut arrivals) =
        start_crash_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-partial-fanout-restart");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                    effect_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_PORT".to_string(),
                    checkpoint_port.to_string(),
                ),
                (
                    "CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("restart-43"),
    );
    tokio::pin!(invocation);
    let first_effect = tokio::select! {
        effect = effects.recv() => effect.expect("first child effect"),
        result = invocation.as_mut() => panic!("partial fanout settled before its pending child: {result:?}"),
    };
    let second_effect = tokio::select! {
        effect = effects.recv() => effect.expect("second child effect"),
        result = invocation.as_mut() => panic!("partial fanout settled before its pending child: {result:?}"),
    };
    assert!(
        [&first_effect, &second_effect]
            .iter()
            .any(|effect| effect.starts_with("partial-completed("))
    );
    assert!(
        [&first_effect, &second_effect]
            .iter()
            .any(|effect| effect.starts_with("partial-pending("))
    );
    let _original_pending =
        next_crash_checkpoint(&mut arrivals, "middleware-partial-pending").await?;
    let completed_start = wait_for_completed_entity_terminal(&executor, &worker_id).await?;
    let before_crash = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        entity_starts(
            &before_crash,
            PublicAgentEntityKind::Tool,
            "middleware-probe"
        )
        .iter()
        .any(|entry| entry.oplog_index == completed_start)
    );

    let short = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "streaming-short-circuit")
        .unwrap();
    let tool_name = ToolName::try_from("middleware-probe").unwrap();
    let mut changed = original_deployment;
    let (middleware_component_id, middleware_component_revision) = match &changed
        .tool_middleware_chains[&ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    }][&tool_name]
        .occurrences[0]
        .middleware
        .source
    {
        ToolMiddlewareSource::Component {
            component_id,
            component_revision,
            ..
        } => (*component_id, *component_revision),
    };
    install_middleware_chain(
        &mut changed,
        &agent_type,
        &tool_name,
        middleware_component_id,
        middleware_component_revision,
        "golem-it:tool-streaming-rust-middleware",
        &middleware_metadata.tool_middlewares,
        vec![(short.name.as_str(), empty_middleware_parameters(short))],
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(changed),
    );
    executor.simulated_crash(&worker_id).await?;
    let replay_pending = tokio::select! {
        checkpoint = next_crash_checkpoint(&mut arrivals, "middleware-partial-pending") => checkpoint?,
        result = invocation.as_mut() => panic!("partial fanout settled before its repaired child was released: {result:?}"),
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), effects.recv())
            .await
            .is_err(),
        "neither independently completed effect may repeat during replay"
    );
    replay_pending
        .release
        .send(())
        .expect("release repaired pending child");
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(
        result, "partial[leaf(partial-completed(restart-43))|leaf(partial-pending(restart-43))]",
        "the in-flight invocation must retain its recorded middleware plan"
    );
    assert_eq!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let middleware_starts = entity_starts(
        &oplog,
        PublicAgentEntityKind::ToolMiddleware,
        "streaming-partial-fanout",
    );
    let leaf_starts = entity_starts(&oplog, PublicAgentEntityKind::Tool, "middleware-probe");
    assert_eq!(middleware_starts.len(), 1);
    assert_eq!(leaf_starts.len(), 2);
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("partial fanout invocation finished")
        .oplog_index;
    for start in middleware_starts.into_iter().chain(leaf_starts) {
        assert_one_terminal_before_finished(&oplog, start.oplog_index, finished);
    }

    for before in &before_crash {
        if let PublicOplogEntry::Start(parameters) = &before.entry
            && parameters.function_name == "golem::entity::invoke"
        {
            let after = oplog
                .iter()
                .find(|entry| entry.oplog_index == before.oplog_index)
                .expect("reconstruction retains the original entity Start");
            let PublicOplogEntry::Start(after) = &after.entry else {
                panic!("reconstruction must preserve the entry kind");
            };
            let mut before = parameters.clone();
            let mut after = after.clone();
            // The public projection converts an attribute map into an unordered list.
            for parameters in [&mut before, &mut after] {
                parameters
                    .span_started
                    .as_mut()
                    .unwrap()
                    .attributes
                    .sort_by(|a, b| a.key.cmp(&b.key));
            }
            assert_eq!(after, before);
        }
    }

    let fresh: String = executor
        .invoke_and_await_agent(
            &caller_component,
            &agent_id,
            "middleware_probe_once",
            data_value!("control"),
        )
        .await?
        .into_typed()?;
    assert_eq!(fresh, "short(control)");
    assert_eq!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    );
    checkpoint_server.abort();
    effect_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn early_return_parent_end_survives_restart_while_detached_child_is_pending(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["streaming-early-return"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (promise_port, promise_server, mut arrivals) = start_promise_checkpoint_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "middleware-early-return-restart");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([(
                "PROVIDER_PROMISE_CHECKPOINT_PORT".to_string(),
                promise_port.to_string(),
            )]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("restart-parent"),
    );
    tokio::pin!(invocation);
    let child = tokio::select! {
        result = invocation.as_mut() => panic!("parent settled before detached child: {result:?}"),
        child = next_promise_checkpoint(&mut arrivals, "middleware-early-child") => child?,
    };
    let parent_start = wait_for_completed_entity_terminal(&executor, &worker_id).await?;
    let before_crash = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert!(
        entity_starts(
            &before_crash,
            PublicAgentEntityKind::ToolMiddleware,
            "streaming-early-return"
        )
        .iter()
        .any(|entry| entry.oplog_index == parent_start)
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), invocation.as_mut())
            .await
            .is_err()
    );

    executor.simulated_crash(&worker_id).await?;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), invocation.as_mut())
            .await
            .is_err()
    );
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: child.oplog_idx,
            },
            vec![],
        )
        .await?;
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(result, "early-return(restart-parent)");

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let middleware_starts = entity_starts(
        &oplog,
        PublicAgentEntityKind::ToolMiddleware,
        "streaming-early-return",
    );
    let leaf_starts = entity_starts(&oplog, PublicAgentEntityKind::Tool, "middleware-probe");
    assert_eq!(middleware_starts.len(), 1);
    assert_eq!(leaf_starts.len(), 1);
    let finished = oplog
        .iter()
        .rfind(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
        .expect("early-return invocation finished")
        .oplog_index;
    for start in middleware_starts.into_iter().chain(leaf_starts) {
        assert_one_terminal_before_finished(&oplog, start.oplog_index, finished);
    }
    promise_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn approval_pending_blocks_dispatch_survives_restart_and_approves_once(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut approval = ApprovalHarness::start().await;
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["human-approval"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let definition = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "human-approval")
        .unwrap();
    configure_approval_parameters(
        &mut deployment,
        &agent_type,
        definition,
        &approval.request_url,
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "human-approval-restart");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "GOLEM_HUMAN_APPROVAL_REQUEST_TOKEN".to_string(),
                    "request-token".to_string(),
                ),
                (
                    "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                    effect_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("approval-restart"),
    );
    tokio::pin!(invocation);
    let requests = tokio::select! {
        requests = approval.pending(1) => requests?,
        result = invocation.as_mut() => anyhow::bail!("approval did not block dispatch: {result:?}"),
    };
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.state, ApprovalState::Pending);
    assert_eq!(request.request.policy, "production-change");
    assert_eq!(request.request.tool_name, "middleware-probe");
    assert!(
        ["anonymous", "oidc", "agent", "golem-user",].contains(&request.request.principal.as_str())
    );
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    executor
        .wait_for_status(
            &worker_id,
            AgentStatus::Suspended,
            std::time::Duration::from_secs(30),
        )
        .await?;
    executor.simulated_crash(&worker_id).await?;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), invocation.as_mut())
            .await
            .is_err()
    );
    assert_eq!(approval.pending(1).await?.len(), 1);
    assert!(
        approval
            .decide(
                request,
                request.request.owner.clone(),
                ApprovalState::Approved,
                "decision-token",
            )
            .await
            .status()
            .is_success()
    );
    let completion = approval.completion().await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id.clone(),
                oplog_idx: OplogIndex::from_u64(completion.oplog_idx),
            },
            completion.data,
        )
        .await?;
    let result: String = invocation.await?.into_typed()?;
    assert_eq!(result, "leaf(approval-restart)");
    assert_eq!(effects.recv().await.as_deref(), Some("approval-restart"));

    assert!(
        approval
            .decide(
                request,
                request.request.owner.clone(),
                ApprovalState::Approved,
                "decision-token",
            )
            .await
            .status()
            .is_success()
    );
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            approval.completions.recv()
        )
        .await
        .is_err()
    );
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        entity_starts(&oplog, PublicAgentEntityKind::Tool, "middleware-probe").len(),
        1
    );
    effect_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn approval_decisions_are_authenticated_owner_bound_and_denial_never_dispatches(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut approval = ApprovalHarness::start().await;
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["human-approval"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let definition = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "human-approval")
        .unwrap();
    configure_approval_parameters(
        &mut deployment,
        &agent_type,
        definition,
        &approval.request_url,
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let agent_id = agent_id!("ToolStreamingCaller", "human-approval-denied");
    let worker_id = executor
        .start_agent_with(
            &caller_component.id,
            agent_id.clone(),
            HashMap::from([
                (
                    "GOLEM_HUMAN_APPROVAL_REQUEST_TOKEN".to_string(),
                    "request-token".to_string(),
                ),
                (
                    "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
                    effect_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let invocation = executor.invoke_and_await_agent(
        &caller_component,
        &agent_id,
        "middleware_probe_once",
        data_value!("approval-denied"),
    );
    tokio::pin!(invocation);
    let requests = tokio::select! {
        requests = approval.pending(1) => requests?,
        result = invocation.as_mut() => anyhow::bail!("approval did not block dispatch: {result:?}"),
    };
    let request = &requests[0];
    assert_eq!(
        approval
            .decide(
                request,
                request.request.owner.clone(),
                ApprovalState::Approved,
                "wrong-token",
            )
            .await
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let mut wrong_owner = request.request.owner.clone();
    wrong_owner.component_id = uuid::Uuid::from_u128(99);
    assert_eq!(
        approval
            .decide(
                request,
                wrong_owner,
                ApprovalState::Approved,
                "decision-token",
            )
            .await
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    assert!(
        approval
            .decide(
                request,
                request.request.owner.clone(),
                ApprovalState::Denied,
                "decision-token",
            )
            .await
            .status()
            .is_success()
    );
    let completion = approval.completion().await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: worker_id,
                oplog_idx: OplogIndex::from_u64(completion.oplog_idx),
            },
            completion.data,
        )
        .await?;
    let error = invocation
        .await
        .expect_err("denial must fail the invocation");
    assert!(format!("{error:?}").contains("human approval was denied"));
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    assert_eq!(
        approval
            .decide(
                request,
                request.request.owner.clone(),
                ApprovalState::Approved,
                "decision-token",
            )
            .await
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    effect_server.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn approval_detach_cancel_and_owner_termination_have_distinct_terminals(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let mut approval = ApprovalHarness::start().await;
    setup_probe_chain!(
        last_unique_id,
        deps,
        provider,
        caller,
        &["human-approval"],
        context,
        environment_state,
        executor,
        provider_component,
        caller_component,
        middleware_metadata,
        agent_type,
        deployment
    );
    let definition = middleware_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "human-approval")
        .unwrap();
    configure_approval_parameters(
        &mut deployment,
        &agent_type,
        definition,
        &approval.request_url,
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let (effect_port, mut effects, effect_server) = start_probe_effect_server().await;
    let env = HashMap::from([
        (
            "GOLEM_HUMAN_APPROVAL_REQUEST_TOKEN".to_string(),
            "request-token".to_string(),
        ),
        (
            "MIDDLEWARE_PROBE_EFFECT_PORT".to_string(),
            effect_port.to_string(),
        ),
    ]);

    let detached_id = agent_id!("ToolStreamingCaller", "human-approval-detached");
    let detached_worker = executor
        .start_agent_with(
            &caller_component.id,
            detached_id.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    executor
        .invoke_agent(
            &caller_component,
            &detached_id,
            "middleware_probe_once",
            data_value!("approval-detached"),
        )
        .await?;
    let requests = approval.pending(1).await?;
    let detached = &requests[0];
    let detached_request_id = detached.request.request_id;
    assert!(
        approval
            .decide(
                detached,
                detached.request.owner.clone(),
                ApprovalState::Approved,
                "decision-token",
            )
            .await
            .status()
            .is_success()
    );
    let completion = approval.completion().await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: detached_worker,
                oplog_idx: OplogIndex::from_u64(completion.oplog_idx),
            },
            completion.data,
        )
        .await?;
    assert_eq!(effects.recv().await.as_deref(), Some("approval-detached"));

    let cancelled_id = agent_id!("ToolStreamingCaller", "human-approval-cancelled");
    let cancelled_worker = executor
        .start_agent_with(
            &caller_component.id,
            cancelled_id.clone(),
            env.clone(),
            Vec::new(),
        )
        .await?;
    let cancelled_invocation = executor.invoke_and_await_agent(
        &caller_component,
        &cancelled_id,
        "middleware_probe_once",
        data_value!("approval-cancelled"),
    );
    tokio::pin!(cancelled_invocation);
    let requests = tokio::select! {
        requests = approval.pending(2) => requests?,
        result = cancelled_invocation.as_mut() => anyhow::bail!("cancellation request did not remain pending: {result:?}"),
    };
    let cancelled = requests
        .iter()
        .find(|record| record.request.request_id != detached_request_id)
        .unwrap();
    let cancelled_request_id = cancelled.request.request_id;
    assert!(
        approval
            .decide(
                cancelled,
                cancelled.request.owner.clone(),
                ApprovalState::Cancelled,
                "decision-token",
            )
            .await
            .status()
            .is_success()
    );
    let completion = approval.completion().await?;
    executor
        .complete_promise(
            &PromiseId {
                agent_id: cancelled_worker,
                oplog_idx: OplogIndex::from_u64(completion.oplog_idx),
            },
            completion.data,
        )
        .await?;
    assert!(cancelled_invocation.await.is_err());

    let terminated_id = agent_id!("ToolStreamingCaller", "human-approval-terminated");
    let terminated_worker = executor
        .start_agent_with(&caller_component.id, terminated_id.clone(), env, Vec::new())
        .await?;
    let terminated_invocation = executor.invoke_and_await_agent(
        &caller_component,
        &terminated_id,
        "middleware_probe_once",
        data_value!("approval-terminated"),
    );
    tokio::pin!(terminated_invocation);
    let requests = tokio::select! {
        requests = approval.pending(3) => requests?,
        result = terminated_invocation.as_mut() => anyhow::bail!("termination request did not remain pending: {result:?}"),
    };
    let terminated = requests
        .iter()
        .find(|record| {
            record.request.request_id != detached_request_id
                && record.request.request_id != cancelled_request_id
        })
        .unwrap();
    executor.delete_worker(&terminated_worker).await?;
    assert!(terminated_invocation.await.is_err());
    assert!(approval.abandon(terminated).await.status().is_success());
    assert_eq!(
        approval
            .decide(
                terminated,
                terminated.request.owner.clone(),
                ApprovalState::Approved,
                "decision-token",
            )
            .await
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    assert!(matches!(
        effects.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    effect_server.abort();
    Ok(())
}
