use super::*;
use golem_common::model::component::ComponentDto;
use golem_common::model::oplog::PublicOplogEntryWithIndex;
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

#[derive(Clone, Default)]
struct Gol40K1AuditSinkState {
    attempts: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
    committed: Arc<std::sync::Mutex<BTreeMap<String, serde_json::Value>>>,
    block_first_after_commit: Arc<std::sync::atomic::AtomicBool>,
    committed_while_blocked: Arc<tokio::sync::Notify>,
}

async fn start_gol40_k1_audit_sink(
    block_first_after_commit: bool,
) -> (String, Gol40K1AuditSinkState, tokio::task::JoinHandle<()>) {
    async fn commit(
        State(state): State<Gol40K1AuditSinkState>,
        headers: axum::http::HeaderMap,
        axum::Json(record): axum::Json<serde_json::Value>,
    ) -> axum::http::StatusCode {
        let key = headers
            .get("idempotency-key")
            .expect("AUDIT-1 requires an idempotency-key header")
            .to_str()
            .expect("audit idempotency keys are ASCII")
            .to_string();
        assert_eq!(
            record["policyInvocationKey"], key,
            "the body and transport carry one durable record identity"
        );
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

    let state = Gol40K1AuditSinkState {
        block_first_after_commit: Arc::new(std::sync::atomic::AtomicBool::new(
            block_first_after_commit,
        )),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/gol40-k1-records", listener.local_addr().unwrap());
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/gol40-k1-records", post(commit))
                .with_state(task_state),
        )
        .await
        .expect("serve GOL-40 K1 audit sink");
    });
    (url, state, task)
}

struct Gol40K1AuditDeployment {
    context: TestContext,
    environment: Arc<TestEnvironmentStateService>,
    executor: TestWorkerExecutor,
    caller_component: ComponentDto,
    deployment: ToolDeploymentState,
    definition: ToolMiddleware,
    agent_type: AgentTypeName,
    tool_name: ToolName,
}

async fn gol40_k1_audit_deployment(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    audit: &PrecompiledComponent,
    caller_agent_type: &str,
    tool_name: &str,
    occurrences: &[(&str, &str)],
) -> anyhow::Result<Gol40K1AuditDeployment> {
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
            .join("golem_audit_middleware_release.wasm"),
        false,
        true,
    )
    .await?;
    let definition = audit_metadata
        .tool_middlewares
        .iter()
        .find(|definition| definition.name == "audit")
        .expect("shipped audit middleware metadata")
        .clone();
    let agent_type = AgentTypeName(caller_agent_type.to_string());
    let tool_name = ToolName::try_from(tool_name).unwrap();
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
        occurrences
            .iter()
            .map(|(label, sink_url)| {
                (
                    definition.name.as_str(),
                    audit_middleware_parameters(&definition, label, sink_url),
                )
            })
            .collect(),
    );
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment.clone()),
    );
    Ok(Gol40K1AuditDeployment {
        context,
        environment,
        executor,
        caller_component,
        deployment,
        definition,
        agent_type,
        tool_name,
    })
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn audit_1_deployed_records_exact_success_error_stream_and_owner_summaries_after_sink_commit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (sink_url, sink, sink_task) = start_gol40_k1_audit_sink(false).await;
    let fixture = gol40_k1_audit_deployment(
        last_unique_id,
        deps,
        provider,
        caller,
        audit,
        "ToolStreamingCaller",
        "streaming",
        &[("summary-contract", sink_url.as_str())],
    )
    .await?;
    let agent_id = agent_id!("ToolStreamingCaller", "gol40-k1-audit-summary");
    let sensitive_input = b"gol40-k1-stream-payload-must-not-be-recorded".to_vec();
    let success: StreamEvidence = fixture
        .executor
        .invoke_and_await_agent(
            &fixture.caller_component,
            &agent_id,
            "collect",
            data_value!("echo", sensitive_input.clone(), 7_u32),
        )
        .await?
        .into_typed()?;
    assert_eq!(success.output, sensitive_input);
    let error: StreamEvidence = fixture
        .executor
        .invoke_and_await_agent(
            &fixture.caller_component,
            &agent_id,
            "result_before_stdout",
            data_value!("declared-error"),
        )
        .await?
        .into_typed()?;
    assert!(error.completion.contains("Declared"));

    let committed = sink.committed.lock().unwrap();
    assert_eq!(committed.len(), 2, "one committed record per logical call");
    let success_record = committed
        .values()
        .find(|record| record["outcome"]["kind"] == "success")
        .expect("success audit record");
    assert_eq!(success_record["version"], 1);
    assert_eq!(success_record["occurrenceLabel"], "summary-contract");
    assert_eq!(success_record["toolName"], "streaming");
    assert_eq!(success_record["commandPath"], serde_json::json!(["run"]));
    assert_eq!(success_record["principal"]["kind"], "anonymous");
    assert!(
        success_record["owner"]["agentId"]
            .as_str()
            .is_some_and(|owner| owner.contains("gol40-k1-audit-summary"))
    );
    assert_eq!(success_record["stdout"]["declared"], true);
    assert_eq!(success_record["stdout"]["chunks"], 7);
    assert_eq!(success_record["stdout"]["bytes"], sensitive_input.len());
    assert_eq!(success_record["stdout"]["terminal"], "finished");
    assert_eq!(success_record["stderr"]["declared"], false);
    let error_record = committed
        .values()
        .find(|record| record["outcome"]["kind"] == "error")
        .expect("error audit record");
    assert_eq!(
        error_record["outcome"]["errorKind"], "tool",
        "unexpected error record: {error_record}"
    );
    assert!(error_record["outcome"]["customName"].is_string());
    assert_eq!(error_record["stdout"]["bytes"], 7);
    for (key, record) in committed.iter() {
        assert_eq!(record["policyInvocationKey"], key.as_str());
    }
    let encoded = serde_json::to_string(&*committed)?;
    assert!(!encoded.contains("gol40-k1-stream-payload-must-not-be-recorded"));
    assert!(!encoded.contains("declared-error"));
    drop(committed);
    assert_eq!(sink.attempts.lock().unwrap().len(), 2);
    sink_task.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn audit_2_deployed_duplicate_occurrences_use_distinct_keys_and_pass_opaque_secret_without_reveal(
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

    let (sink_url, sink, sink_task) = start_gol40_k1_audit_sink(false).await;
    let mut fixture = gol40_k1_audit_deployment(
        last_unique_id,
        deps,
        provider,
        caller,
        audit,
        "ToolSecretCaller",
        "secret-policy-probe",
        &[
            ("security", sink_url.as_str()),
            ("operations", sink_url.as_str()),
        ],
    )
    .await?;
    let plaintext = "gol40-k1-audit-must-never-record-this-plaintext";
    fixture.environment.set_agent_secret(AgentSecret {
        id: AgentSecretId::new(),
        environment_id: fixture.context.default_environment_id,
        path: CanonicalAgentSecretPath(vec!["toolSecret".to_string()]),
        revision: AgentSecretRevision::INITIAL,
        secret_type: SchemaGraph::anonymous(SchemaType::string()),
        secret_value: Some(SchemaValue::String(plaintext.to_string())),
    });
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: fixture.agent_type.clone(),
    };
    let occurrences = &mut fixture
        .deployment
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .get_mut(&fixture.tool_name)
        .unwrap()
        .occurrences;
    for occurrence in occurrences.iter_mut() {
        occurrence.secret_keys_readable = SecretKeyScope::Keys(Default::default());
        occurrence.secret_keys_revealable = SecretKeyScope::Keys(Default::default());
    }
    fixture.environment.set_tool_deployment(
        fixture.context.default_environment_id,
        fixture.caller_component.id,
        fixture.caller_component.revision,
        Some(fixture.deployment.clone()),
    );

    let agent_id = agent_id!("ToolSecretCaller", "gol40-k1-audit-secret");
    let evidence = fixture
        .executor
        .invoke_and_await_agent(
            &fixture.caller_component,
            &agent_id,
            "inspect_secret_policy",
            data_value!(),
        )
        .await?
        .into_typed::<SecretPolicyEvidence>()?;
    assert!(
        evidence.leaf_revealed,
        "the leaf independently consumes the secret"
    );
    assert!(
        evidence.middleware.is_empty(),
        "audit preserves the leaf result"
    );

    let committed = sink.committed.lock().unwrap();
    assert_eq!(
        committed.len(),
        2,
        "each configured occurrence commits once"
    );
    let mut labels = committed
        .values()
        .map(|record| record["occurrenceLabel"].as_str().unwrap())
        .collect::<Vec<_>>();
    labels.sort_unstable();
    assert_eq!(labels, ["operations", "security"]);
    let keys = committed.keys().collect::<Vec<_>>();
    assert_ne!(
        keys[0], keys[1],
        "duplicate occurrences have distinct identities"
    );
    for (key, record) in committed.iter() {
        assert_eq!(record["policyInvocationKey"], key.as_str());
        assert_eq!(record["toolName"], "secret-policy-probe");
        assert_eq!(record["input"]["secretHandles"], 1);
        assert_eq!(record["outcome"]["kind"], "success");
        assert_eq!(record["principal"]["kind"], "anonymous");
        assert!(!record.to_string().contains(plaintext));
        assert!(!record.to_string().contains("toolSecret"));
    }
    drop(committed);
    assert_eq!(sink.attempts.lock().unwrap().len(), 2);
    sink_task.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("2m")]
async fn audit_3_crash_after_sink_commit_retries_same_key_deduplicates_and_replays_pinned_policy(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (sink_url, sink, sink_task) = start_gol40_k1_audit_sink(true).await;
    let fixture = gol40_k1_audit_deployment(
        last_unique_id,
        deps,
        provider,
        caller,
        audit,
        "ToolStreamingCaller",
        "middleware-probe",
        &[("pinned-before-crash", sink_url.as_str())],
    )
    .await?;
    let Gol40K1AuditDeployment {
        context,
        environment,
        mut executor,
        caller_component,
        mut deployment,
        definition,
        agent_type,
        tool_name,
    } = fixture;
    let agent_id = agent_id!("ToolStreamingCaller", "gol40-k1-audit-crash");
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
        agent_type_name: agent_type,
    };
    deployment
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences[0]
        .parameters =
        audit_middleware_parameters(&definition, "changed-after-crash", sink_url.as_str());
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment),
            ..Default::default()
        },
    )
    .await?;
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

    {
        let attempts = sink.attempts.lock().unwrap();
        assert_eq!(
            attempts.len(),
            2,
            "delivery retries after the ambiguous crash"
        );
        assert_eq!(
            attempts[0].0, attempts[1].0,
            "replay preserves record identity"
        );
        assert_eq!(
            attempts[0].1, attempts[1].1,
            "replay preserves record contents"
        );
        assert_eq!(attempts[1].1["occurrenceLabel"], "pinned-before-crash");
    }
    assert_eq!(
        sink.committed.lock().unwrap().len(),
        1,
        "the endpoint-scoped (sink URL, policy key) identity commits once"
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
    let audit_starts = gol40_k1_audit_entity_starts(&oplog);
    assert_eq!(audit_starts.len(), 1, "one logical audit entity invocation");
    sink_task.abort();
    Ok(())
}

fn gol40_k1_audit_entity_starts(oplog: &[PublicOplogEntryWithIndex]) -> Vec<OplogIndex> {
    oplog
        .iter()
        .filter_map(|entry| match (&entry.entry, &entry.attribution) {
            (PublicOplogEntry::Start(parameters), PublicOplogEntryAttribution::Entity(entity))
                if parameters.function_name == "golem::entity::invoke"
                    && entity.invocation.entity.kind == PublicAgentEntityKind::ToolMiddleware
                    && entity.invocation.entity.name == "audit" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .collect()
}
