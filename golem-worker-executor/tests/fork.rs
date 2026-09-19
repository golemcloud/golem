// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_api_grpc::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamMapping, StreamInvocationIdentity, UpdateMode, invocation_request,
    invocation_response,
};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ResumeWorkerRequest, resume_worker_response, worker_executor_client::WorkerExecutorClient,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentInvocationMode, InvocationFreshnessDisposition, Principal};
use golem_common::model::card::{CardId, ScopeCard, StoredCard};
use golem_common::model::component::ComponentRevision;
use golem_common::model::durable_stream::{
    DurableStreamReadRequest, StreamAttachmentControlRequest,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::{AgentConfigEntryDto, ResolvedRevert, RevertWorkerTarget};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocationOutput, IdempotencyKey, InvocationStatus,
    OwnedAgentId, PromiseId,
};
use golem_common::schema::SchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_worker_executor::services::rpc::{
    DurableRpcInvocationResult, DurableStreamReadError, RemoteInvocationRpc, Rpc, RpcDemand,
    RpcError,
};
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use golem_worker_executor_test_utils::{
    TestContext, TestExecutorOverrides, TestWorkerExecutor, WorkerExecutorTestDependencies,
    start_with_overrides,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

type Client = WorkerExecutorClient<OtelGrpcService<Channel>>;

/// Routes fork activation back to the single test executor instead of requiring worker-service.
pub(crate) async fn start_with_local_resume(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    lose_resume_response: bool,
) -> anyhow::Result<TestWorkerExecutor> {
    start_with_resume_checkpoint(deps, context, lose_resume_response, None).await
}

#[derive(Default)]
pub(crate) struct RemoteRpcEvidence {
    pub(crate) sessions: std::sync::atomic::AtomicUsize,
    pub(crate) joined_observer_acceptances: std::sync::atomic::AtomicUsize,
    pub(crate) resume_attach_requests: std::sync::atomic::AtomicUsize,
}

/// Keeps fork activation local, but forces durable streaming RPC through the executor's tonic API.
pub(crate) async fn start_with_remote_streaming_rpc(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
) -> anyhow::Result<(TestWorkerExecutor, Arc<RemoteRpcEvidence>)> {
    let client = Arc::new(Mutex::new(None));
    let proxy = Arc::new(OnceLock::<Arc<dyn WorkerProxy>>::new());
    let shard = Arc::new(OnceLock::<Arc<dyn ShardService>>::new());
    let evidence = Arc::new(RemoteRpcEvidence::default());
    let environment_id = context.default_environment_id;
    let account_id = context.account_id;
    let executor = start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            wrap_shard_service: Some(Arc::new({
                let shard = shard.clone();
                move |inner| {
                    let _ = shard.set(inner.clone());
                    inner
                }
            })),
            wrap_worker_proxy: Some(Arc::new({
                let client = client.clone();
                let proxy = proxy.clone();
                let evidence = evidence.clone();
                move |inner| {
                    let wrapped: Arc<dyn WorkerProxy> = Arc::new(LocalResumeProxy {
                        inner,
                        client: client.clone(),
                        environment_id,
                        component_owner_account_id: Some(account_id),
                        evidence: Some(evidence.clone()),
                        lose_response: Arc::new(AtomicBool::new(false)),
                        checkpoint: Arc::new(Mutex::new(None)),
                    });
                    let _ = proxy.set(wrapped.clone());
                    wrapped
                }
            })),
            wrap_rpc: Some(Arc::new(move |inner| {
                Arc::new(StreamingRemoteRpc {
                    inner,
                    remote: RemoteInvocationRpc::new(
                        proxy.get().expect("worker proxy initialized").clone(),
                        shard.get().expect("shard service initialized").clone(),
                    ),
                })
            })),
            ..Default::default()
        },
    )
    .await?;
    *client.lock().unwrap() = Some(executor.client.clone());
    Ok((executor, evidence))
}

async fn start_with_resume_checkpoint(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    lose_resume_response: bool,
    checkpoint: Option<Arc<tokio::sync::Notify>>,
) -> anyhow::Result<TestWorkerExecutor> {
    let client = Arc::new(Mutex::new(None));
    let target = client.clone();
    let lose_response = Arc::new(AtomicBool::new(lose_resume_response));
    let checkpoint = Arc::new(Mutex::new(checkpoint));
    let environment_id = context.default_environment_id;
    let executor = start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            wrap_worker_proxy: Some(Arc::new(move |inner| {
                Arc::new(LocalResumeProxy {
                    inner,
                    client: target.clone(),
                    environment_id,
                    component_owner_account_id: None,
                    evidence: None,
                    lose_response: lose_response.clone(),
                    checkpoint: checkpoint.clone(),
                })
            })),
            ..Default::default()
        },
    )
    .await?;
    *client.lock().unwrap() = Some(executor.client.clone());
    Ok(executor)
}

struct StreamingRemoteRpc {
    inner: Arc<dyn Rpc>,
    remote: RemoteInvocationRpc,
}

#[async_trait]
impl Rpc for StreamingRemoteRpc {
    async fn create_demand(
        &self,
        agent: &OwnedAgentId,
        method: &str,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        self.inner
            .create_demand(agent, method, created_by, caller, env, stack, config, auth)
            .await
    }

    async fn invoke_and_await(
        &self,
        agent: &OwnedAgentId,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        card: Option<ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        self.inner
            .invoke_and_await(
                agent, key, freshness, method, input, created_by, caller, env, stack, config, auth,
                card,
            )
            .await
    }

    async fn invoke_and_await_streaming(
        &self,
        agent: &OwnedAgentId,
        key: IdempotencyKey,
        method: String,
        input: ProtoSchemaValue,
        mappings: Vec<DurableStreamMapping>,
        fingerprint: AgentFingerprint,
        attempt: uuid::Uuid,
        origin: StreamInvocationIdentity,
        accepted: tokio::sync::oneshot::Sender<Vec<DurableStreamMapping>>,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
        card: Option<ScopeCard>,
    ) -> Result<DurableRpcInvocationResult, RpcError> {
        self.remote
            .invoke_and_await_streaming(
                agent,
                key,
                method,
                input,
                mappings,
                fingerprint,
                attempt,
                origin,
                accepted,
                created_by,
                caller,
                env,
                stack,
                config,
                auth,
                card,
            )
            .await
    }

    async fn control_durable_stream_attachment(
        &self,
        request: StreamAttachmentControlRequest,
        auth: &AuthCtx,
    ) -> Result<bool, RpcError> {
        self.inner
            .control_durable_stream_attachment(request, auth)
            .await
    }

    async fn read_durable_stream_segment(
        &self,
        request: DurableStreamReadRequest,
        auth: &AuthCtx,
    ) -> Result<Vec<u8>, DurableStreamReadError<RpcError>> {
        self.inner.read_durable_stream_segment(request, auth).await
    }

    async fn invoke(
        &self,
        agent: &OwnedAgentId,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        method: String,
        input: SchemaValue,
        created_by: AccountId,
        caller: &AgentId,
        env: &[(String, String)],
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth: &AuthCtx,
    ) -> Result<(), RpcError> {
        self.inner
            .invoke(
                agent, key, freshness, method, input, created_by, caller, env, stack, config, auth,
            )
            .await
    }
}

struct LocalResumeProxy {
    inner: Arc<dyn WorkerProxy>,
    client: Arc<Mutex<Option<Client>>>,
    environment_id: EnvironmentId,
    component_owner_account_id: Option<AccountId>,
    evidence: Option<Arc<RemoteRpcEvidence>>,
    lose_response: Arc<AtomicBool>,
    checkpoint: Arc<Mutex<Option<Arc<tokio::sync::Notify>>>>,
}

#[async_trait]
impl WorkerProxy for LocalResumeProxy {
    async fn prepare(
        &self,
        agent: &OwnedAgentId,
        method: &str,
        caller: &AgentId,
        env: HashMap<String, String>,
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        auth: &AuthCtx,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        self.inner
            .prepare(agent, method, caller, env, stack, config, principal, auth)
            .await
    }

    async fn resume(
        &self,
        agent_id: &AgentId,
        force: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        let mut client = self
            .client
            .lock()
            .unwrap()
            .clone()
            .expect("test executor connected");
        let response = client
            .resume_worker(ResumeWorkerRequest {
                agent_id: Some(agent_id.clone().into()),
                environment_id: Some(self.environment_id.into()),
                force: Some(force),
                auth_ctx: Some(auth_ctx.clone().into()),
                principal: None,
            })
            .await?
            .into_inner();
        match response.result {
            Some(resume_worker_response::Result::Success(_)) => {
                let checkpoint = self.checkpoint.lock().unwrap().take();
                if let Some(checkpoint) = checkpoint {
                    checkpoint.notify_one();
                    std::future::pending::<()>().await;
                }
                if self.lose_response.swap(false, Ordering::SeqCst) {
                    Err(WorkerProxyError::InternalError(
                        WorkerExecutorError::runtime("lost fork resume response"),
                    ))
                } else {
                    Ok(())
                }
            }
            Some(resume_worker_response::Result::Failure(error)) => Err(
                WorkerProxyError::InternalError(WorkerExecutorError::try_from(error).map_err(
                    |error| WorkerProxyError::InternalError(WorkerExecutorError::runtime(error)),
                )?),
            ),
            None => Err(WorkerProxyError::InternalError(
                WorkerExecutorError::runtime("missing resume result"),
            )),
        }
    }

    async fn start(
        &self,
        agent: &OwnedAgentId,
        method: &str,
        caller: &AgentId,
        env: HashMap<String, String>,
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        auth: &AuthCtx,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        self.inner
            .start(agent, method, caller, env, stack, config, principal, auth)
            .await
    }

    async fn invoke_agent(
        &self,
        agent: &AgentId,
        method: String,
        input: SchemaValue,
        mode: AgentInvocationMode,
        schedule_at: Option<DateTime<Utc>>,
        key: Option<IdempotencyKey>,
        freshness: InvocationFreshnessDisposition,
        caller: AgentId,
        env: HashMap<String, String>,
        stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        principal: Principal,
        environment: EnvironmentId,
        auth: &AuthCtx,
        card: Option<ScopeCard>,
    ) -> Result<AgentInvocationOutput, WorkerProxyError> {
        self.inner
            .invoke_agent(
                agent,
                method,
                input,
                mode,
                schedule_at,
                key,
                freshness,
                caller,
                env,
                stack,
                config,
                principal,
                environment,
                auth,
                card,
            )
            .await
    }

    async fn invoke_agent_session(
        &self,
        request: golem_worker_executor::services::worker_proxy::InvocationRequestStream,
    ) -> Result<
        golem_worker_executor::services::worker_proxy::InvocationResponseStream,
        WorkerProxyError,
    > {
        let evidence = self.evidence.clone();
        let account_id = self.component_owner_account_id;
        let (requests, receiver) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            let mut request = request;
            while let Some(mut request) = request.next().await {
                if let Some(evidence) = &evidence {
                    evidence.sessions.fetch_add(1, Ordering::SeqCst);
                    match &mut request.request {
                        Some(invocation_request::Request::Start(start)) => {
                            start.component_owner_account_id = account_id.map(Into::into);
                        }
                        Some(invocation_request::Request::ResumeAttach(_)) => {
                            evidence
                                .resume_attach_requests
                                .fetch_add(1, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                }
                if requests.send(request).await.is_err() {
                    break;
                }
            }
        });
        let mut client = self
            .client
            .lock()
            .unwrap()
            .clone()
            .expect("test executor connected");
        let mut responses = client
            .invoke_agent_session(tokio_stream::wrappers::ReceiverStream::new(receiver))
            .await?
            .into_inner();
        let evidence = self.evidence.clone();
        let (outbound, inbound) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            loop {
                let response = responses.message().await;
                if let Ok(Some(response)) = &response
                    && let Some(invocation_response::Response::Accepted(accepted)) =
                        &response.response
                    && accepted.joined_origin_observer
                    && let Some(evidence) = &evidence
                {
                    evidence
                        .joined_observer_acceptances
                        .fetch_add(1, Ordering::SeqCst);
                }
                let response = match response {
                    Ok(Some(response)) => Ok(response),
                    Ok(None) => break,
                    Err(error) => Err(error),
                };
                if outbound.send(response).await.is_err() {
                    break;
                }
            }
        });
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(
            inbound,
        )))
    }

    async fn control_durable_stream_attachment(
        &self,
        request: golem_common::model::durable_stream::StreamAttachmentControlRequest,
        auth: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner
            .control_durable_stream_attachment(request, auth)
            .await
    }

    async fn read_durable_stream_segment(
        &self,
        request: golem_common::model::durable_stream::DurableStreamReadRequest,
        auth: &AuthCtx,
    ) -> Result<
        Vec<u8>,
        golem_worker_executor::services::rpc::DurableStreamReadError<WorkerProxyError>,
    > {
        self.inner.read_durable_stream_segment(request, auth).await
    }

    async fn deliver_card_transfer(
        &self,
        agent: &AgentId,
        environment: EnvironmentId,
        transfer: uuid::Uuid,
        source_card: CardId,
        card: &StoredCard,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .deliver_card_transfer(agent, environment, transfer, source_card, card)
            .await
    }

    async fn update(
        &self,
        agent: &OwnedAgentId,
        revision: ComponentRevision,
        mode: UpdateMode,
        disable_wakeup: bool,
        auth: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .update(agent, revision, mode, disable_wakeup, auth)
            .await
    }

    async fn fork_worker(
        &self,
        source: &AgentId,
        target: &AgentId,
        cut: &OplogIndex,
        auth: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner.fork_worker(source, target, cut, auth).await
    }

    async fn revert(
        &self,
        agent: &AgentId,
        target: RevertWorkerTarget,
        resolved: Option<ResolvedRevert>,
        auth: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner.revert(agent, target, resolved, auth).await
    }

    async fn complete_promise(
        &self,
        id: PromiseId,
        data: Vec<u8>,
        auth: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner.complete_promise(id, data, auth).await
    }

    async fn cancel_invocation(
        &self,
        agent: &AgentId,
        key: IdempotencyKey,
        auth: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        self.inner.cancel_invocation(agent, key, auth).await
    }

    async fn lookup_invocation_status(
        &self,
        agent: &AgentId,
        key: IdempotencyKey,
        env: Option<EnvironmentId>,
        auth: &AuthCtx,
    ) -> Result<InvocationStatus, WorkerProxyError> {
        self.inner
            .lookup_invocation_status(agent, key, env, auth)
            .await
    }

    async fn process_oplog_entries(
        &self,
        agent: &AgentId,
        env: EnvironmentId,
        revision: ComponentRevision,
        key: IdempotencyKey,
        account: AccountId,
        config: Vec<(String, String)>,
        metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        first: OplogIndex,
        entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        auth: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        self.inner
            .process_oplog_entries(
                agent, env, revision, key, account, config, metadata, first, entries, auth,
            )
            .await
    }
}

use crate::Tracing;
use golem_common::model::ScanCursor;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{LastUniqueId, PrecompiledComponent};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("agent_sdk_rust")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[test]
#[timeout("120s")]
async fn exported_fork_initial_content_and_receipt_survive_lost_resume_response(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::InvocationStart;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        AppendToStreamSlotRequest, ExternalStreamProducer, ForkStreamSlotRequest,
        ReadStreamSlotRequest, TypedStreamSlotItems, append_to_stream_slot_request,
        append_to_stream_slot_response, create_stream_session_response,
        fork_stream_slot_rejection::Reason, fork_stream_slot_response, read_stream_slot_response,
        stream_slot_item,
    };
    use golem_common::schema::{schema_value_to_proto_with_streams, stream::SchemaValueStream};
    use prost::Message;
    use uuid::Uuid;
    let context = TestContext::new(last_unique_id);
    let executor = start_with_local_resume(deps, &context, true).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let source = executor
        .start_agent(
            &component.id,
            agent_id!("DurableStreamAgent", "export-fork-retry"),
        )
        .await?;
    let target = AgentId::from_agent_id(
        component.id,
        &golem_common::phantom_agent_id!("DurableStreamAgent", Uuid::new_v4(), "export-fork-retry"),
    )
    .map_err(anyhow::Error::msg)?;
    let session = Uuid::new_v4().to_string();
    let input = schema_value_to_proto_with_streams(
        SchemaValue::Record {
            fields: vec![SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                0u64,
            ))],
        },
        |stream| stream.take_host_endpoint::<u64>(),
    )
    .map_err(anyhow::Error::msg)?;
    let created = executor
        .client
        .clone()
        .create_stream_session(InvocationStart {
            agent_id: Some(source.clone().into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(AuthCtx::System.into()),
            component_owner_account_id: Some(component.account_id.into()),
            idempotency_key: Some(IdempotencyKey::new(session.clone()).into()),
            method_name: Some("echo".into()),
            input: Some(input),
            ..Default::default()
        })
        .await?
        .into_inner();
    assert!(
        matches!(
            created.result,
            Some(create_stream_session_response::Result::Success(_))
        ),
        "{created:?}"
    );
    let append = |agent: &AgentId, value: &str| AppendToStreamSlotRequest {
        agent_id: Some(agent.clone().into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(AuthCtx::System.into()),
        session: session.clone(),
        slot: "input".into(),
        expected_method: "echo".into(),
        payload: Some(append_to_stream_slot_request::Payload::Values(
            TypedStreamSlotItems {
                values: vec![
                    golem_api_grpc::proto::golem::schema::SchemaValue::try_from(
                        SchemaValue::String(value.into()),
                    )
                    .unwrap()
                    .encode_to_vec(),
                ],
            },
        )),
        close: false,
        producer: Some(ExternalStreamProducer {
            id: "writer".into(),
            epoch: 0,
            sequence: 0,
        }),
    };
    let accepted = executor
        .client
        .clone()
        .append_to_stream_slot(append(&source, "shared"))
        .await?
        .into_inner();
    let Some(append_to_stream_slot_response::Result::Accepted(prefix)) = accepted.result else {
        anyhow::bail!("{accepted:?}");
    };
    let request = ForkStreamSlotRequest {
        source_agent_id: Some(source.clone().into()),
        target_agent_id: Some(target.clone().into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(AuthCtx::System.into()),
        session: session.clone(),
        slot: "input".into(),
        expected_method: "echo".into(),
        source_path: "/source/input".into(),
        initial_content: br#"["fork-initial", "fork-second"]"#.to_vec(),
        max_forks_per_session: 1,
        max_forks_per_second: 100,
        max_copied_bytes: 64 * 1024 * 1024,
        ..Default::default()
    };
    let mut too_large = request.clone();
    too_large.max_copied_bytes = 0;
    let rejected = executor
        .client
        .clone()
        .fork_stream_slot(too_large)
        .await?
        .into_inner();
    assert!(
        matches!(rejected.result, Some(fork_stream_slot_response::Result::Rejected(ref rejection)) if rejection.reason == Reason::TooLarge as i32),
        "{rejected:?}"
    );
    let lost = executor
        .client
        .clone()
        .fork_stream_slot(request.clone())
        .await?
        .into_inner();
    assert!(
        matches!(
            lost.result,
            Some(fork_stream_slot_response::Result::Failure(_))
        ),
        "{lost:?}"
    );
    let fork_append = executor
        .client
        .clone()
        .append_to_stream_slot(append(&target, "fork-writer"))
        .await?
        .into_inner();
    assert!(
        matches!(
            fork_append.result,
            Some(append_to_stream_slot_response::Result::Accepted(_))
        ),
        "fork must not inherit writer state: {fork_append:?}"
    );
    let mut source_append = append(&source, "source-only");
    source_append.producer.as_mut().unwrap().sequence = 1;
    let accepted = executor
        .client
        .clone()
        .append_to_stream_slot(source_append)
        .await?
        .into_inner();
    assert!(matches!(
        accepted.result,
        Some(append_to_stream_slot_response::Result::Accepted(_))
    ));
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    let executor = start_with_local_resume(deps, &context, false).await?;
    let mut retry = request.clone();
    retry.max_forks_per_session = 0;
    retry.max_forks_per_second = 0;
    retry.max_copied_bytes = 0;
    let retried = executor
        .client
        .clone()
        .fork_stream_slot(retry)
        .await?
        .into_inner();
    let Some(fork_stream_slot_response::Result::Success(receipt)) = retried.result else {
        anyhow::bail!("{retried:?}");
    };
    assert!(receipt.replayed);
    assert_eq!(
        receipt.fork_offset, prefix.offset,
        "default tail must retain the first cut"
    );
    for (agent, expected) in [
        (&source, vec!["shared", "source-only"]),
        (
            &target,
            vec!["shared", "fork-initial", "fork-second", "fork-writer"],
        ),
    ] {
        let read = executor
            .client
            .clone()
            .read_stream_slot(ReadStreamSlotRequest {
                agent_id: Some(agent.clone().into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                session: session.clone(),
                slot: "input".into(),
                expected_method: "echo".into(),
                max_items: 100,
                max_bytes: 1_000_000,
                ..Default::default()
            })
            .await?
            .into_inner()
            .message()
            .await?
            .unwrap();
        let Some(read_stream_slot_response::Result::Success(read)) = read.result else {
            anyhow::bail!("{read:?}");
        };
        let actual = read
            .items
            .iter()
            .map(|item| {
                let Some(stream_slot_item::Content::Value(bytes)) = &item.content else {
                    panic!("expected typed item")
                };
                SchemaValue::try_from(
                    golem_api_grpc::proto::golem::schema::SchemaValue::decode(bytes.as_slice())
                        .unwrap(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            expected
                .into_iter()
                .map(|value| SchemaValue::String(value.into()))
                .collect::<Vec<_>>()
        );
        if agent == &target {
            assert_eq!(read.fork, Some(receipt.clone()));
        }
    }
    let mut conflict = request.clone();
    conflict.sub_offset = 1;
    let rejected = executor
        .client
        .clone()
        .fork_stream_slot(conflict)
        .await?
        .into_inner();
    assert!(
        matches!(rejected.result, Some(fork_stream_slot_response::Result::Rejected(ref rejection)) if rejection.reason == Reason::Conflict as i32)
    );
    let mut over_count = request;
    over_count.target_agent_id = Some(
        AgentId::from_agent_id(
            component.id,
            &golem_common::phantom_agent_id!(
                "DurableStreamAgent",
                Uuid::new_v4(),
                "export-fork-retry"
            ),
        )
        .map_err(anyhow::Error::msg)?
        .into(),
    );
    let rejected = executor
        .client
        .clone()
        .fork_stream_slot(over_count.clone())
        .await?
        .into_inner();
    assert!(
        matches!(rejected.result, Some(fork_stream_slot_response::Result::Rejected(ref rejection)) if rejection.reason == Reason::Conflict as i32),
        "{rejected:?}"
    );
    over_count.max_forks_per_session = 2;
    over_count.max_forks_per_second = 0;
    let rejected = executor
        .client
        .clone()
        .fork_stream_slot(over_count)
        .await?
        .into_inner();
    assert!(
        matches!(rejected.result, Some(fork_stream_slot_response::Result::Rejected(ref rejection)) if rejection.reason == Reason::RateLimited as i32 && rejection.retry_after_seconds > 0),
        "{rejected:?}"
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn guest_fork_retries_same_child_after_crash_before_caller_result(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let checkpoint = Arc::new(tokio::sync::Notify::new());
    let executor =
        start_with_resume_checkpoint(deps, &context, false, Some(checkpoint.clone())).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let source = agent_id!("GolemHostApi", "fork-crash");
    let source_id = executor.start_agent(&component.id, source.clone()).await?;
    let key = IdempotencyKey::fresh();
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let source = source.clone();
        let key = key.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &source,
                    &key,
                    "self_fork_result",
                    data_value!(),
                )
                .await
        }
    });
    tokio::select! {
        _ = checkpoint.notified() => {}
        result = &mut invocation => anyhow::bail!("fork returned before checkpoint: {result:?}"),
    }
    let (cursor, before) = executor
        .get_workers_metadata(&component.id, None, ScanCursor::default(), 100, true)
        .await?;
    assert!(cursor.is_none());
    assert_eq!(before.len(), 2);
    let child = before
        .iter()
        .find(|agent| agent.agent_id != source_id)
        .unwrap()
        .agent_id
        .clone();
    executor.shutdown_and_wait_for_invocation_loops().await?;
    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_local_resume(deps, &context, false).await?;
    let result = executor
        .invoke_and_await_agent_with_key(
            &component,
            &source,
            &key,
            "self_fork_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(result, Ok("original".to_string()));
    let (cursor, after) = executor
        .get_workers_metadata(&component.id, None, ScanCursor::default(), 100, true)
        .await?;
    assert!(cursor.is_none());
    assert_eq!(after.len(), 2, "recovery must not create a second child");
    assert!(after.iter().any(|agent| agent.agent_id == child));
    let result = executor
        .invoke_and_await_agent(&component, &source, "self_fork_result", data_value!())
        .await?
        .into_typed::<Result<String, String>>()?;
    assert_eq!(result, Ok("original".to_string()));
    let (_, fresh) = executor
        .get_workers_metadata(&component.id, None, ScanCursor::default(), 100, true)
        .await?;
    assert_eq!(
        fresh.len(),
        3,
        "a fresh logical call must create a distinct child"
    );
    let rejected = executor
        .invoke_and_await_agent(
            &component,
            &source,
            "self_fork_atomic_result",
            data_value!(),
        )
        .await?
        .into_typed::<Result<String, String>>()?;
    assert!(rejected.unwrap_err().contains("atomic region"));
    let (_, after_rejection) = executor
        .get_workers_metadata(&component.id, None, ScanCursor::default(), 100, true)
        .await?;
    assert_eq!(after_rejection.len(), 3);
    Ok(())
}

#[test]
#[timeout("120s")]
async fn guest_fork_same_key_on_phantom_siblings_creates_distinct_children(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let key = IdempotencyKey::new("same-key-for-two-independent-agents".into());
    for phantom in [uuid::Uuid::from_u128(41), uuid::Uuid::from_u128(42)] {
        let source = golem_common::phantom_agent_id!("GolemHostApi", phantom, "siblings");
        executor.start_agent(&component.id, source.clone()).await?;
        let result = executor
            .invoke_and_await_agent_with_key(
                &component,
                &source,
                &key,
                "self_fork_result",
                data_value!(),
            )
            .await?
            .into_typed::<Result<String, String>>()?;
        assert_eq!(result, Ok("original".to_string()));
    }
    let (cursor, agents) = executor
        .get_workers_metadata(&component.id, None, ScanCursor::default(), 100, true)
        .await?;
    assert!(cursor.is_none());
    assert_eq!(agents.len(), 4);
    Ok(())
}
