use super::*;
use anyhow::{Context as _, ensure};
use futures::FutureExt;
use golem_api_grpc::proto::golem::worker::{
    ExternalToolInvocation, InputStreamEnd, InputStreamItem, InvocationRequest, InvocationStart,
    ToolByteStreamRole, input_stream_item, invocation_request, invocation_response,
    invocation_session_completion, invocation_session_result,
};
use golem_common::model::agent::extraction::extract_component_metadata;
use golem_common::model::agent::{AgentMode, GolemUserPrincipal, Principal};
use golem_common::model::durable_stream::StreamSessionRecord;
use golem_common::model::oplog::OplogEntry;
use golem_common::schema::{SchemaGraph, SchemaType, TypedSchemaValue};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::durable_host::durable_session::DurableSourceObserverForTest;
use golem_worker_executor::services::oplog::OplogOps;
use golem_worker_executor::services::{HasOplog, HasOplogService};
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
async fn durable_native_stdin_memory_quota_pending_source(
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
    eprintln!(
        "STDIN_STOPPED key={key:?} stream={stdin_id} durable_stream={durable_stream_id:?} epoch={} worker={} startup={:?} generation={} ordinal={} returned={} dropped={}",
        accepted.epoch,
        Arc::as_ptr(&worker) as usize,
        worker.startup_attempt_for_test(),
        worker.resident_generation_for_test(),
        observer.ordinal.load(Ordering::SeqCst),
        observer.returned.load(Ordering::SeqCst),
        observer.dropped.load(Ordering::SeqCst),
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
    let mut receipts = Vec::new();
    let continuation = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(response) = responses.message().await? {
            let receipt = match response.response.as_ref() {
                Some(invocation_response::Response::InputAck(ack)) => format!(
                    "InputAck stream={} durable_stream={:?} epoch={} sequence={} count={} offset={:?}",
                    ack.transport_stream_id, ack.durable_stream_id, ack.epoch,
                    ack.highest_contiguous_sequence, ack.logical_item_count, ack.resulting_offset,
                ),
                Some(invocation_response::Response::OutputItem(item)) => format!(
                    "OutputItem stream={} durable_stream={:?} epoch={} sequence={} count={} packed_bytes={} offset={:?}",
                    item.transport_stream_id, item.durable_stream_id, item.epoch,
                    item.producer_sequence, item.logical_item_count, item.packed_u8.len(), item.durable_offset,
                ),
                Some(invocation_response::Response::OutputEnd(end)) => format!(
                    "OutputEnd stream={} durable_stream={:?} epoch={} sequence={} offset={:?}",
                    end.transport_stream_id, end.durable_stream_id, end.epoch,
                    end.producer_sequence, end.durable_offset,
                ),
                Some(invocation_response::Response::Result(result)) => format!(
                    "Result agent={:?} key={:?} fingerprint={:?} index={:?} tool_result={}",
                    result.agent_id, result.idempotency_key, result.agent_fingerprint, result.oplog_index,
                    matches!(result.result, Some(invocation_session_result::Result::ToolResult(_))),
                ),
                Some(invocation_response::Response::Finished(finished)) => format!(
                    "Finished success={}",
                    matches!(finished.outcome, Some(invocation_session_completion::Outcome::Success(_))),
                ),
                other => format!("Other kind={:?}", other.map(std::mem::discriminant)),
            };
            receipts.push(receipt);
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
    .await;
    for (order, receipt) in receipts.iter().enumerate() {
        eprintln!("STDIN_SESSION_RECEIPT order={order} {receipt}");
    }
    eprintln!(
        "STDIN_CONTINUATION timeout={} inner_error={} output_bytes={} results={} worker={} startup={:?} generation={} loaded={:?} permit={:?} ordinal={} returned={} dropped={}",
        continuation.is_err(),
        matches!(&continuation, Ok(Err(_))),
        output.len(),
        results,
        Arc::as_ptr(&worker) as usize,
        worker.startup_attempt_for_test(),
        worker.resident_generation_for_test(),
        worker.is_loaded().now_or_never(),
        worker.concurrent_agent_permit_is_held().now_or_never(),
        observer.ordinal.load(Ordering::SeqCst),
        observer.returned.load(Ordering::SeqCst),
        observer.dropped.load(Ordering::SeqCst),
    );
    if !matches!(&continuation, Ok(Ok(()))) || results != 1 || output != bytes {
        eprintln!(
            "STDIN_COMMITTED_INSPECTION unavailable=true reason=continuation_or_original_result_oracle_failed"
        );
    }
    continuation.context("ordinary same-key native stdin continuation")??;
    ensure!(results == 1 && output == bytes);
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let service = worker.oplog_service();
    let tip = service.get_last_index(&owned, AgentMode::Durable).await;
    let committed = service
        .read_exact(
            &owned,
            AgentMode::Durable,
            OplogIndex::INITIAL,
            tip.as_u64(),
        )
        .await;
    let oplog = worker.oplog();
    for (index, entry) in committed {
        match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } => eprintln!(
                "STDIN_COMMITTED index={index} kind=AgentInvocationStarted key={idempotency_key:?}"
            ),
            OplogEntry::AgentInvocationFinished { method_name, .. } => eprintln!(
                "STDIN_COMMITTED index={index} kind=AgentInvocationFinished method={method_name:?}"
            ),
            OplogEntry::Start {
                parent_start_index,
                function_name,
                invocation_id,
                ..
            } => eprintln!(
                "STDIN_COMMITTED index={index} kind=Start parent={parent_start_index:?} function={function_name} invocation_id={invocation_id:?}"
            ),
            OplogEntry::End { start_index, .. } => {
                eprintln!("STDIN_COMMITTED index={index} kind=End start={start_index}")
            }
            OplogEntry::Cancelled { start_index, .. } => {
                eprintln!("STDIN_COMMITTED index={index} kind=Cancelled start={start_index}")
            }
            OplogEntry::StreamItems { record, .. } => match oplog.download_payload(record).await {
                Ok(record) => eprintln!(
                    "STDIN_COMMITTED index={index} kind=StreamItems stream={:?} sequence={} count={} offsets={:?}",
                    record.stream_id,
                    record.first_sequence,
                    record.payload.logical_item_count(),
                    record.offsets,
                ),
                Err(_) => eprintln!(
                    "STDIN_COMMITTED index={index} kind=StreamItems payload_inspection_failed=true"
                ),
            },
            OplogEntry::StreamEnd { record, .. } => match oplog.download_payload(record).await {
                Ok(record) => eprintln!(
                    "STDIN_COMMITTED index={index} kind=StreamEnd stream={:?} sequence={} offset={:?}",
                    record.stream_id, record.sequence, record.offset,
                ),
                Err(_) => eprintln!(
                    "STDIN_COMMITTED index={index} kind=StreamEnd payload_inspection_failed=true"
                ),
            },
            OplogEntry::StreamSession { record, .. } => {
                match oplog.download_payload(record).await {
                    Ok(record) => match record {
                        StreamSessionRecord::Prepared(record) => eprintln!(
                            "STDIN_COMMITTED index={index} kind=Prepared key={:?} mappings={}",
                            record.session_key,
                            record.stream_mappings.len(),
                        ),
                        StreamSessionRecord::InputHighWater(record) => eprintln!(
                            "STDIN_COMMITTED index={index} kind=InputHighWater key={:?} stream={:?} epoch={} sequence={} count={} high_water={:?}",
                            record.session_key,
                            record.stream_id,
                            record.epoch,
                            record.first_sequence,
                            record.payload.logical_item_count(),
                            record.high_water,
                        ),
                        StreamSessionRecord::ConsumerItemValue(record) => eprintln!(
                            "STDIN_COMMITTED index={index} kind=ConsumerItemValue key={:?} reader={:?} ordinal={} count={} offset={:?}",
                            record.session_key,
                            record.reader_id,
                            record.consumer_read_ordinal,
                            record.logical_item_count(),
                            record.source_offset,
                        ),
                        StreamSessionRecord::ConsumerTerminal(record) => eprintln!(
                            "STDIN_COMMITTED index={index} kind=ConsumerTerminal key={:?} reader={:?} ordinal={} offset={:?}",
                            record.session_key,
                            record.reader_id,
                            record.consumer_read_ordinal,
                            record.source_offset,
                        ),
                        StreamSessionRecord::InvocationResult(record) => eprintln!(
                            "STDIN_COMMITTED index={index} kind=InvocationResult key={:?}",
                            record.session_key,
                        ),
                        StreamSessionRecord::Finished(record) => eprintln!(
                            "STDIN_COMMITTED index={index} kind=Finished key={:?} success={}",
                            record.session_key,
                            record.result.is_ok(),
                        ),
                        record => eprintln!(
                            "STDIN_COMMITTED index={index} kind=OtherSession discriminant={:?} key={:?}",
                            std::mem::discriminant(&record),
                            record.local_session_key(),
                        ),
                    },
                    Err(_) => eprintln!(
                        "STDIN_COMMITTED index={index} kind=StreamSession payload_inspection_failed=true"
                    ),
                }
            }
            entry => eprintln!(
                "STDIN_COMMITTED index={index} kind=Other discriminant={:?}",
                std::mem::discriminant(&entry)
            ),
        }
    }
    Ok(())
}
