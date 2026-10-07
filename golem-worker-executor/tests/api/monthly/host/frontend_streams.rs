use super::*;
use anyhow::{Context as _, ensure};
use golem_api_grpc::proto::golem::worker::{
    ExternalToolInvocation, InputStreamEnd, InputStreamItem, InvocationRequest, InvocationStart,
    ToolByteStreamRole, input_stream_item, invocation_request, invocation_response,
    invocation_session_completion, invocation_session_result,
};
use golem_common::model::agent::extraction::extract_component_metadata;
use golem_common::model::agent::{GolemUserPrincipal, Principal};
use golem_common::schema::{SchemaGraph, SchemaType, TypedSchemaValue};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::durable_host::durable_session::DurableSourceObserverForTest;
use golem_worker_executor::worker::MonthlyClockForTest;
use golem_worker_executor_test_utils::agent_deployments_service::TestEnvironmentStateService;
use golem_worker_executor_test_utils::start_with_resource_limits_and_overrides;
use test_r::{inherit_test_dep, test};
use tokio_stream::wrappers::ReceiverStream;

inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_provider")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);

#[test]
#[timeout("2m")]
async fn durable_native_stdin_monthly_memory_pending_source(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let resource = support::Resource::Memory;
    let policy = resource.policy();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let _shutdown = shutdown.clone().drop_guard();
    let metering = resource.metering();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown,
    );
    let context = TestContext::new(last_unique_id);
    let environment = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_resource_limits_and_overrides(
        deps,
        &context,
        limits.clone(),
        TestExecutorOverrides {
            configure: Some(Arc::new(move |config| {
                config.resource_usage_metering = metering
            })),
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
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(crate::tool_streaming::deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let id = executor
        .start_agent(
            &caller_component.id,
            agent_id!("ToolStreamingCaller", "monthly-quiet-stdin"),
        )
        .await?;
    wait_for_invocation_pair(&executor, &id, OplogIndex::INITIAL).await?;
    let worker = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &id))
        .await
        .unwrap()
        .primary();
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let fingerprint = executor.get_worker_metadata(&id).await?.fingerprint;
    let key = IdempotencyKey::fresh();
    let observer = DurableSourceObserverForTest::install(key.clone());
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let input = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "mode".into(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![SchemaValue::String("echo".into())],
        },
    );
    let (sender, receiver) = tokio::sync::mpsc::channel(8);
    sender
        .send(InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                agent_id: Some(id.clone().into()),
                idempotency_key: Some(key.clone().into()),
                auth_ctx: Some(executor.auth_ctx().into()),
                principal: Some(
                    Principal::GolemUser(GolemUserPrincipal {
                        account_id: context.account_id,
                    })
                    .into(),
                ),
                environment_id: Some(context.default_environment_id.into()),
                component_owner_account_id: Some(context.account_id.into()),
                expected_callee_fingerprint: Some(fingerprint.0.into()),
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                external_tool: Some(ExternalToolInvocation {
                    tool_name: "streaming".into(),
                    command_path: vec!["run".into()],
                    input: Some(input.try_into().map_err(anyhow::Error::msg)?),
                    stdin: true,
                    stdout: true,
                    stderr: false,
                    fresh_owner: false,
                    expected_deployment_revision: None,
                }),
                ..Default::default()
            })),
        })
        .await?;
    let mut responses = executor
        .client
        .clone()
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let response = responses
        .message()
        .await?
        .context("native stdin session acceptance")?;
    let Some(invocation_response::Response::Accepted(accepted)) = response.response else {
        anyhow::bail!("session rejected: {response:?}");
    };
    let mapping = accepted
        .stream_mappings
        .iter()
        .find(|mapping| mapping.tool_byte_stream_role == Some(ToolByteStreamRole::Stdin as i32))
        .context("stdin mapping")?;
    let stdin_id = mapping.transport_stream_id;
    let durable_stream_id = mapping.handle.as_ref().unwrap().stream_id;
    tokio::time::timeout(Duration::from_secs(10), observer.pending.notified())
        .await
        .context("actual durable reader.next Pending")?;
    ensure!(
        observer.ordinal.load(Ordering::SeqCst) == 0
            && observer.returned.load(Ordering::SeqCst) == 0
    );
    ensure!(worker.concurrent_agent_permit_is_held().await);
    let mut raw = worker.raw_interrupt_for_test();
    tokio::time::timeout(Duration::from_secs(10), polls.recv())
        .await?
        .context("monthly window timer")?;
    let account = limits.initialize_account(context.account_id).await?;
    let mut exhausted = policy.clone();
    exhausted.available_memory_gb_seconds = 0;
    registry.set_policy(exhausted);
    let refresh = applied_refresh(&limits, &registry, context.account_id).await?;
    let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv())
        .await?
        .context("monthly acceptance")?;
    ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
    ensure!(matches!(
        tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??,
        InterruptKind::Suspend(_)
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("physical unload with quiet stdin")?;
    worker.join_accepted_stops_for_test().await?;
    worker.retained_cleanup_for_test().await?;
    refresh.await?;
    ensure!(worker.unload_succeeded_for_test());
    ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
    ensure!(
        observer.dropped.load(Ordering::SeqCst) > 0
            && observer.returned.load(Ordering::SeqCst) == 0
    );
    let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
    ensure!(
        !stopped
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Cancelled(_)))
    );
    // Keep the same accepted session and key. Submit the first byte only after physical release.
    let bytes = vec![17, 255];
    sender
        .send(InvocationRequest {
            request: Some(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id: stdin_id,
                sequence: 0,
                payload: Some(input_stream_item::Payload::PackedU8(bytes.clone())),
                durable_stream_id,
                epoch: accepted.epoch,
            })),
        })
        .await?;
    sender
        .send(InvocationRequest {
            request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                transport_stream_id: stdin_id,
                sequence: bytes.len() as u64,
                durable_stream_id,
                epoch: accepted.epoch,
            })),
        })
        .await?;
    registry.set_policy(policy);
    applied_refresh(&limits, &registry, context.account_id)
        .await?
        .await?;
    executor.resume(&id, false).await?;
    let mut output = Vec::new();
    let mut results = 0;
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(response) = responses.message().await? {
            match response.response {
                Some(invocation_response::Response::OutputItem(item)) => {
                    output.extend(item.packed_u8)
                }
                Some(invocation_response::Response::OutputEnd(_)) => {}
                Some(invocation_response::Response::Result(result)) => {
                    let Some(invocation_session_result::Result::ToolResult(result)) = result.result
                    else {
                        anyhow::bail!("wrong result");
                    };
                    let result: golem_common::model::oplog::PublicExternalToolResult =
                        result.try_into().map_err(anyhow::Error::msg)?;
                    ensure!(
                        matches!(
                            result,
                            golem_common::model::oplog::PublicExternalToolResult::Success(_)
                        ),
                        "{result:?}"
                    );
                    results += 1;
                }
                Some(invocation_response::Response::InputAck(_)) => {}
                Some(invocation_response::Response::Finished(finished)) => {
                    ensure!(matches!(
                        finished.outcome,
                        Some(invocation_session_completion::Outcome::Success(_))
                    ));
                    break;
                }
                other => anyhow::bail!("unexpected resumed session response: {other:?}"),
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("ordinary same-key native stdin continuation")??;
    ensure!(results == 1 && output == bytes);
    Ok(())
}
