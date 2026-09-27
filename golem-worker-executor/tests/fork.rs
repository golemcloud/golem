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
    DurableStreamMapping, InvocationAccepted, StreamInvocationIdentity, UpdateMode,
    invocation_request, invocation_response,
};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    CreateStreamSessionRequest, ResumeWorkerRequest, StreamSessionCreationIntent,
    resume_worker_response, worker_executor_client::WorkerExecutorClient,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentInvocationMode, InvocationFreshnessDisposition, Principal};
use golem_common::model::card::{CardId, ScopeCard, StoredCard};
use golem_common::model::component::ComponentRevision;
use golem_common::model::durable_stream::{
    DurableStreamReadRequest, StreamAttachmentControlRequest, StreamSessionRecord,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
use golem_common::model::worker::{AgentConfigEntryDto, ResolvedRevert, RevertWorkerTarget};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocationOutput, IdempotencyKey, InvocationStatus,
    OwnedAgentId, PromiseId,
};
use golem_common::schema::{FromSchema, SchemaValue};
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
        accepted: tokio::sync::oneshot::Sender<InvocationAccepted>,
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
        ReadStreamSlotRequest, StreamSessionExpiryPolicy, StreamSlotReadAdmission,
        TypedStreamSlotItems, append_to_stream_slot_request, append_to_stream_slot_response,
        create_stream_session_response, fork_stream_slot_rejection::Reason,
        fork_stream_slot_response, read_stream_slot_response, stream_session_expiry_policy,
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
    let create_request = CreateStreamSessionRequest {
        public_session_id: session.clone(),
        expiry_policy: Some(StreamSessionExpiryPolicy {
            kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(3_600)),
        }),
        creation_intent: StreamSessionCreationIntent::ExplicitPut as i32,
        invocation: Some(InvocationStart {
            agent_id: Some(source.clone().into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(AuthCtx::System.into()),
            component_owner_account_id: Some(component.account_id.into()),
            idempotency_key: Some(IdempotencyKey::new(session.clone()).into()),
            method_name: Some("echo".into()),
            input: Some(input),
            ..Default::default()
        }),
    };
    let created = executor
        .client
        .clone()
        .create_stream_session(create_request.clone())
        .await?
        .into_inner();
    let result = created.result;
    let Some(create_stream_session_response::Result::Success(created)) = result else {
        panic!("{result:?}");
    };
    assert_eq!(created.session, session);
    assert_ne!(
        created
            .invocation_key
            .as_ref()
            .expect("creation returns the internal invocation key")
            .value,
        session,
        "durable public session IDs must not become invocation idempotency keys"
    );
    let replayed = executor
        .client
        .clone()
        .create_stream_session(create_request)
        .await?
        .into_inner();
    let result = replayed.result;
    let Some(create_stream_session_response::Result::Success(replayed)) = result else {
        panic!("{result:?}");
    };
    assert!(replayed.replayed);
    assert_eq!(replayed.invocation_key, created.invocation_key);
    assert_eq!(
        replayed.expiry_deadline_millis,
        created.expiry_deadline_millis
    );
    let initial_deadline = created
        .expiry_deadline_millis
        .expect("sliding session has a deadline");
    let read_metadata = |admission, invocation_key| ReadStreamSlotRequest {
        agent_id: Some(source.clone().into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(AuthCtx::System.into()),
        session: session.clone(),
        slot: "input".into(),
        expected_method: "echo".into(),
        max_items: 0,
        max_bytes: 1_000_000,
        admission,
        invocation_key,
        ..Default::default()
    };
    let head = executor
        .client
        .clone()
        .read_stream_slot(read_metadata(StreamSlotReadAdmission::Head as i32, None))
        .await?
        .into_inner()
        .message()
        .await?
        .unwrap();
    let Some(read_stream_slot_response::Result::Success(head)) = head.result else {
        anyhow::bail!("{head:?}");
    };
    assert_eq!(head.expiry_deadline_millis, Some(initial_deadline));
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let touched = executor
        .client
        .clone()
        .read_stream_slot(read_metadata(
            StreamSlotReadAdmission::TouchingOriginGet as i32,
            None,
        ))
        .await?
        .into_inner()
        .message()
        .await?
        .unwrap();
    let Some(read_stream_slot_response::Result::Success(touched)) = touched.result else {
        anyhow::bail!("{touched:?}");
    };
    let touched_deadline = touched.expiry_deadline_millis.unwrap();
    assert_eq!(touched_deadline, initial_deadline);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let continuation = executor
        .client
        .clone()
        .read_stream_slot(read_metadata(
            StreamSlotReadAdmission::Continuation as i32,
            touched.invocation_key.clone(),
        ))
        .await?
        .into_inner()
        .message()
        .await?
        .unwrap();
    let Some(read_stream_slot_response::Result::Success(continuation)) = continuation.result else {
        anyhow::bail!("{continuation:?}");
    };
    assert_eq!(continuation.expiry_deadline_millis, Some(touched_deadline));
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
    let initial_append = append(&source, "shared");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let accepted = executor
        .client
        .clone()
        .append_to_stream_slot(initial_append.clone())
        .await?
        .into_inner();
    let Some(append_to_stream_slot_response::Result::Accepted(prefix)) = accepted.result else {
        anyhow::bail!("{accepted:?}");
    };
    let append_deadline = accepted.expiry_deadline_millis.unwrap();
    assert_eq!(append_deadline, touched_deadline);
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let duplicate = executor
        .client
        .clone()
        .append_to_stream_slot(initial_append)
        .await?
        .into_inner();
    assert!(matches!(
        duplicate.result,
        Some(append_to_stream_slot_response::Result::Duplicate(_))
    ));
    let duplicate_deadline = duplicate.expiry_deadline_millis.unwrap();
    assert_eq!(duplicate_deadline, append_deadline);
    let mut gap = append(&source, "gap");
    gap.producer.as_mut().unwrap().sequence = 3;
    let gap = executor
        .client
        .clone()
        .append_to_stream_slot(gap)
        .await?
        .into_inner();
    assert!(matches!(
        gap.result,
        Some(append_to_stream_slot_response::Result::SequenceGap(_))
    ));
    assert_eq!(gap.expiry_deadline_millis, Some(duplicate_deadline));
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
    executor.interrupt(&source).await?;
    executor.append_after_next_oplog_commit(&source);
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
    assert!(
        format!("{lost:?}").contains("lost fork resume response"),
        "fork snapshot must be committed before staging: {lost:?}"
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
    // Live admission must be folded before any status-cache eviction or full oplog refold.
    let live_admissions = executor.export_fork_admissions(&source).await?;
    assert_eq!(live_admissions.reservations.len(), 1);
    assert!(live_admissions.reservations.contains_key(&target));
    assert_eq!(live_admissions.session_counts.get(&session), Some(&1));
    executor.shutdown_and_wait_for_invocation_loops().await?;
    let admitted_before_restart = executor.export_fork_admission_records(&source).await?;
    assert_eq!(admitted_before_restart.len(), 1);
    executor.remove_cached_status(&source).await?;
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
    assert_ne!(receipt.invocation_key, created.invocation_key);
    assert_eq!(
        receipt.fork_offset, prefix.offset,
        "default tail must retain the first cut"
    );
    let admitted_after_restart = executor.export_fork_admission_records(&source).await?;
    assert_eq!(admitted_after_restart, admitted_before_restart);
    let reconstructed = executor.export_fork_admissions(&source).await?;
    assert_eq!(reconstructed.reservations.len(), 1);
    assert_eq!(
        reconstructed.reservations[&target].request_hash,
        admitted_before_restart[0].request_hash
    );
    assert_eq!(reconstructed.session_counts.get(&session), Some(&1));
    assert_eq!(
        reconstructed.updated_millis,
        admitted_before_restart[0].updated_millis
    );
    assert_eq!(
        reconstructed.credit_millis,
        Some(admitted_before_restart[0].credit_millis)
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
                admission: StreamSlotReadAdmission::Head as i32,
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
            assert_eq!(read.invocation_key, receipt.invocation_key);
            assert!(matches!(
                read.expiry_policy.and_then(|policy| policy.kind),
                Some(stream_session_expiry_policy::Kind::TtlSeconds(3_600))
            ));
            assert!(read.expiry_deadline_millis.is_some());
        }
    }
    let nested_target = AgentId::from_agent_id(
        component.id,
        &golem_common::phantom_agent_id!(
            "DurableStreamAgent",
            Uuid::new_v4(),
            "export-fork-nested"
        ),
    )
    .map_err(anyhow::Error::msg)?;
    let mut nested = request.clone();
    nested.source_agent_id = Some(target.clone().into());
    nested.target_agent_id = Some(nested_target.into());
    nested.source_path = "/source/nested-input".into();
    let nested_created = executor
        .client
        .clone()
        .fork_stream_slot(nested.clone())
        .await?
        .into_inner();
    let Some(fork_stream_slot_response::Result::Success(nested_created)) = nested_created.result
    else {
        anyhow::bail!("{nested_created:?}");
    };
    assert!(!nested_created.replayed);
    assert_ne!(nested_created.invocation_key, receipt.invocation_key);
    let nested_replayed = executor
        .client
        .clone()
        .fork_stream_slot(nested)
        .await?
        .into_inner();
    let Some(fork_stream_slot_response::Result::Success(nested_replayed)) = nested_replayed.result
    else {
        anyhow::bail!("{nested_replayed:?}");
    };
    assert!(nested_replayed.replayed);
    assert_eq!(
        nested_replayed.invocation_key,
        nested_created.invocation_key
    );

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
    let mut over_count = request.clone();
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

    // Invalid expiry policies must not reserve the target or consume admission.
    let overflow_target = AgentId::from_agent_id(
        component.id,
        &golem_common::phantom_agent_id!(
            "DurableStreamAgent",
            Uuid::new_v4(),
            "export-fork-overflow-expiry"
        ),
    )
    .map_err(anyhow::Error::msg)?;
    let mut overflowing = request.clone();
    overflowing.target_agent_id = Some(overflow_target.clone().into());
    overflowing.max_forks_per_session = 2;
    overflowing.expiry_policy = Some(StreamSessionExpiryPolicy {
        kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(
            10_000_000_000_000_000,
        )),
    });
    let rejected = executor
        .client
        .clone()
        .fork_stream_slot(overflowing)
        .await?
        .into_inner();
    assert!(matches!(
        rejected.result,
        Some(fork_stream_slot_response::Result::Rejected(_))
    ));
    let mut valid_after_rejection = request.clone();
    valid_after_rejection.target_agent_id = Some(overflow_target.into());
    valid_after_rejection.max_forks_per_session = 2;
    valid_after_rejection.expiry_policy = Some(StreamSessionExpiryPolicy {
        kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(60)),
    });
    let accepted = executor
        .client
        .clone()
        .fork_stream_slot(valid_after_rejection)
        .await?
        .into_inner();
    assert!(
        matches!(
            accepted.result,
            Some(fork_stream_slot_response::Result::Success(_))
        ),
        "a rejected expiry policy consumed fork admission: {accepted:?}"
    );

    // Out-of-range absolute deadlines must not reserve the target or consume admission.
    let out_of_range_target = AgentId::from_agent_id(
        component.id,
        &golem_common::phantom_agent_id!(
            "DurableStreamAgent",
            Uuid::new_v4(),
            "export-fork-out-of-range-expiry"
        ),
    )
    .map_err(anyhow::Error::msg)?;
    let mut out_of_range = request.clone();
    out_of_range.target_agent_id = Some(out_of_range_target.clone().into());
    out_of_range.max_forks_per_session = 4;
    out_of_range.expiry_policy = Some(StreamSessionExpiryPolicy {
        kind: Some(stream_session_expiry_policy::Kind::ExpiresAtMillis(
            u64::MAX,
        )),
    });
    let _rejected = executor.client.clone().fork_stream_slot(out_of_range).await;
    let mut valid_after_rejection = request.clone();
    valid_after_rejection.target_agent_id = Some(out_of_range_target.into());
    valid_after_rejection.max_forks_per_session = 4;
    valid_after_rejection.expiry_policy = Some(StreamSessionExpiryPolicy {
        kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(60)),
    });
    let accepted = executor
        .client
        .clone()
        .fork_stream_slot(valid_after_rejection)
        .await?
        .into_inner();
    assert!(
        matches!(
            accepted.result,
            Some(fork_stream_slot_response::Result::Success(_))
        ),
        "a rejected out-of-range absolute expiry consumed fork admission: {accepted:?}"
    );

    // An expiry that elapses during staging must not reserve the target or consume admission.
    let expiring_during_publication_target = AgentId::from_agent_id(
        component.id,
        &golem_common::phantom_agent_id!(
            "DurableStreamAgent",
            Uuid::new_v4(),
            "export-fork-expiring-during-publication"
        ),
    )
    .map_err(anyhow::Error::msg)?;
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let mut expires_during_publication = request.clone();
    expires_during_publication.target_agent_id =
        Some(expiring_during_publication_target.clone().into());
    expires_during_publication.max_forks_per_session = 6;
    expires_during_publication.expiry_policy = Some(StreamSessionExpiryPolicy {
        kind: Some(stream_session_expiry_policy::Kind::ExpiresAtMillis(
            now_millis + 1,
        )),
    });
    let _rejected = executor
        .client
        .clone()
        .fork_stream_slot(expires_during_publication)
        .await;
    let mut valid_after_rejection = request.clone();
    valid_after_rejection.target_agent_id = Some(expiring_during_publication_target.into());
    valid_after_rejection.max_forks_per_session = 6;
    valid_after_rejection.expiry_policy = Some(StreamSessionExpiryPolicy {
        kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(60)),
    });
    let accepted = executor
        .client
        .clone()
        .fork_stream_slot(valid_after_rejection)
        .await?
        .into_inner();
    assert!(
        matches!(
            accepted.result,
            Some(fork_stream_slot_response::Result::Success(_))
        ),
        "an absolute expiry that elapsed during publication reserved the target: {accepted:?}"
    );

    let expiring_target = AgentId::from_agent_id(
        component.id,
        &golem_common::phantom_agent_id!(
            "DurableStreamAgent",
            Uuid::new_v4(),
            "export-fork-expiring"
        ),
    )
    .map_err(anyhow::Error::msg)?;
    let mut expiring_request = request.clone();
    expiring_request.target_agent_id = Some(expiring_target.clone().into());
    expiring_request.max_forks_per_session = 5;
    expiring_request.expiry_policy = Some(StreamSessionExpiryPolicy {
        kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(0)),
    });
    let expiring = executor
        .client
        .clone()
        .fork_stream_slot(expiring_request.clone())
        .await?
        .into_inner();
    let Some(fork_stream_slot_response::Result::Success(expiring)) = expiring.result else {
        anyhow::bail!("{expiring:?}");
    };
    assert_ne!(expiring.invocation_key, created.invocation_key);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let read = executor
                .client
                .clone()
                .read_stream_slot(ReadStreamSlotRequest {
                    agent_id: Some(expiring_target.clone().into()),
                    environment_id: Some(component.environment_id.into()),
                    auth_ctx: Some(AuthCtx::System.into()),
                    session: session.clone(),
                    slot: "input".into(),
                    expected_method: "echo".into(),
                    admission: StreamSlotReadAdmission::Head as i32,
                    ..Default::default()
                })
                .await?
                .into_inner()
                .message()
                .await?
                .unwrap();
            if matches!(
                read.result,
                Some(read_stream_slot_response::Result::NotFound(_))
            ) {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("fork target did not expire"))??;
    let retry_after_target_expiry = executor
        .client
        .clone()
        .fork_stream_slot(expiring_request)
        .await?
        .into_inner();
    assert!(
        matches!(retry_after_target_expiry.result, Some(fork_stream_slot_response::Result::Rejected(ref rejection)) if rejection.reason == Reason::NotFound as i32),
        "an expired target receipt must not be replayed: {retry_after_target_expiry:?}"
    );

    let concurrent_session = Uuid::new_v4().to_string();
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
        .create_stream_session(CreateStreamSessionRequest {
            public_session_id: concurrent_session.clone(),
            expiry_policy: None,
            creation_intent: StreamSessionCreationIntent::ExplicitPut as i32,
            invocation: Some(InvocationStart {
                agent_id: Some(source.clone().into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                component_owner_account_id: Some(component.account_id.into()),
                idempotency_key: Some(IdempotencyKey::new(concurrent_session.clone()).into()),
                method_name: Some("echo".into()),
                input: Some(input),
                ..Default::default()
            }),
        })
        .await?
        .into_inner();
    assert!(matches!(
        created.result,
        Some(create_stream_session_response::Result::Success(_))
    ));
    let mut concurrent_append = append(&source, "concurrent");
    concurrent_append.session = concurrent_session.clone();
    let accepted = executor
        .client
        .clone()
        .append_to_stream_slot(concurrent_append)
        .await?
        .into_inner();
    assert!(matches!(
        accepted.result,
        Some(append_to_stream_slot_response::Result::Accepted(_))
    ));
    let concurrent_request = |target: AgentId| ForkStreamSlotRequest {
        source_agent_id: Some(source.clone().into()),
        target_agent_id: Some(target.into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(AuthCtx::System.into()),
        session: concurrent_session.clone(),
        slot: "input".into(),
        expected_method: "echo".into(),
        source_path: "/source/input".into(),
        max_forks_per_session: 1,
        max_forks_per_second: 100,
        max_copied_bytes: 64 * 1024 * 1024,
        ..Default::default()
    };
    let new_target = || {
        AgentId::from_agent_id(
            component.id,
            &golem_common::phantom_agent_id!(
                "DurableStreamAgent",
                Uuid::new_v4(),
                "export-fork-concurrent"
            ),
        )
        .map_err(anyhow::Error::msg)
    };
    let left = concurrent_request(new_target()?);
    let right = concurrent_request(new_target()?);
    let mut left_client = executor.client.clone();
    let mut right_client = executor.client.clone();
    let (left, right) = tokio::join!(
        left_client.fork_stream_slot(left),
        right_client.fork_stream_slot(right)
    );
    let results = [left?.into_inner(), right?.into_inner()];
    assert_eq!(
        results
            .iter()
            .filter(|response| matches!(
                response.result,
                Some(fork_stream_slot_response::Result::Success(_))
            ))
            .count(),
        1,
        "{results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|response| matches!(response.result, Some(fork_stream_slot_response::Result::Rejected(ref rejection)) if rejection.reason == Reason::Conflict as i32))
            .count(),
        1,
        "{results:?}"
    );

    let expired_session = Uuid::new_v4().to_string();
    let input = schema_value_to_proto_with_streams(
        SchemaValue::Record {
            fields: vec![SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                0u64,
            ))],
        },
        |stream| stream.take_host_endpoint::<u64>(),
    )
    .map_err(anyhow::Error::msg)?;
    let expired_request = CreateStreamSessionRequest {
        public_session_id: expired_session.clone(),
        expiry_policy: Some(StreamSessionExpiryPolicy {
            kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(0)),
        }),
        creation_intent: StreamSessionCreationIntent::ExplicitPut as i32,
        invocation: Some(InvocationStart {
            agent_id: Some(source.clone().into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(AuthCtx::System.into()),
            component_owner_account_id: Some(component.account_id.into()),
            idempotency_key: Some(IdempotencyKey::new(expired_session.clone()).into()),
            method_name: Some("echo".into()),
            input: Some(input),
            ..Default::default()
        }),
    };
    let first = executor
        .client
        .clone()
        .create_stream_session(expired_request.clone())
        .await?
        .into_inner();
    let Some(create_stream_session_response::Result::Success(first)) = first.result else {
        anyhow::bail!("{first:?}");
    };
    let first_key: IdempotencyKey = first.invocation_key.clone().unwrap().into();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let expired = executor
                .get_oplog(&source, OplogIndex::INITIAL)
                .await?
                .iter()
                .any(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
                    if matches!(
                        StreamSessionRecord::from_value(session.record.value()).unwrap(),
                        StreamSessionRecord::Expired(ref record)
                            if record.session_key == first_key
                    ))
                });
            if expired {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("scheduled stream expiry was not committed"))??;
    let expiry_records = executor
        .get_oplog(&source, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter_map(|entry| {
            let PublicOplogEntry::StreamSession(session) = &entry.entry else {
                return None;
            };
            StreamSessionRecord::from_value(session.record.value()).ok()
        })
        .collect::<Vec<_>>();
    assert!(expiry_records.iter().any(|record| {
        matches!(record, StreamSessionRecord::CancelRequested(record)
            if record.session_key.idempotency_key() == &first_key)
    }));
    assert!(expiry_records.iter().any(|record| {
        matches!(record, StreamSessionRecord::ConsumerCancelIntent(record)
            if record.consumer_invocation == first_key)
    }));
    let expired = executor
        .client
        .clone()
        .read_stream_slot(ReadStreamSlotRequest {
            agent_id: Some(source.clone().into()),
            environment_id: Some(component.environment_id.into()),
            auth_ctx: Some(AuthCtx::System.into()),
            session: expired_session.clone(),
            slot: "input".into(),
            expected_method: "echo".into(),
            admission: StreamSlotReadAdmission::Head as i32,
            ..Default::default()
        })
        .await?
        .into_inner()
        .message()
        .await?
        .unwrap();
    assert!(matches!(
        expired.result,
        Some(read_stream_slot_response::Result::NotFound(_))
    ));
    let recreated = executor
        .client
        .clone()
        .create_stream_session(expired_request)
        .await?
        .into_inner();
    let Some(create_stream_session_response::Result::Success(recreated)) = recreated.result else {
        anyhow::bail!("{recreated:?}");
    };
    assert_ne!(recreated.invocation_key, first.invocation_key);

    let restart_session = Uuid::new_v4().to_string();
    let input = schema_value_to_proto_with_streams(
        SchemaValue::Record {
            fields: vec![SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                0u64,
            ))],
        },
        |stream| stream.take_host_endpoint::<u64>(),
    )
    .map_err(anyhow::Error::msg)?;
    let created_before_restart = executor
        .client
        .clone()
        .create_stream_session(CreateStreamSessionRequest {
            public_session_id: restart_session.clone(),
            expiry_policy: Some(StreamSessionExpiryPolicy {
                kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(1)),
            }),
            creation_intent: StreamSessionCreationIntent::ExplicitPut as i32,
            invocation: Some(InvocationStart {
                agent_id: Some(source.clone().into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                component_owner_account_id: Some(component.account_id.into()),
                idempotency_key: Some(IdempotencyKey::new(restart_session).into()),
                method_name: Some("echo".into()),
                input: Some(input),
                ..Default::default()
            }),
        })
        .await?
        .into_inner();
    let Some(create_stream_session_response::Result::Success(created_before_restart)) =
        created_before_restart.result
    else {
        anyhow::bail!("{created_before_restart:?}");
    };
    executor.shutdown_and_wait_for_invocation_loops().await?;
    drop(executor);
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let executor = start_with_local_resume(deps, &context, false).await?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let expired = executor
                .get_oplog(&source, OplogIndex::INITIAL)
                .await?
                .iter()
                .any(|entry| {
                    matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
                    if matches!(
                        StreamSessionRecord::from_value(session.record.value()).unwrap(),
                        StreamSessionRecord::Expired(ref record)
                            if Some(record.session_key.clone())
                                == created_before_restart.invocation_key.clone().map(Into::into)
                    ))
                });
            if expired {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("scheduled stream expiry did not recover after restart"))??;

    Ok(())
}

#[test]
#[timeout("60s")]
async fn sliding_expiry_refreshes_are_coalesced(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_sdk_rust")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_api_grpc::proto::golem::worker::InvocationStart;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        AppendToStreamSlotRequest, ExternalStreamProducer, ReadStreamSlotRequest,
        StreamSessionExpiryPolicy, StreamSlotReadAdmission, TypedStreamSlotItems,
        append_to_stream_slot_request, append_to_stream_slot_response,
        create_stream_session_response, read_stream_slot_response, stream_session_expiry_policy,
    };
    use golem_common::schema::{schema_value_to_proto_with_streams, stream::SchemaValueStream};
    use prost::Message;
    use uuid::Uuid;

    let context = TestContext::new(last_unique_id);
    let executor = start_with_local_resume(deps, &context, false).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let source = executor
        .start_agent(
            &component.id,
            agent_id!("DurableStreamAgent", "expiry-refresh-coalescing"),
        )
        .await?;
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
        .create_stream_session(CreateStreamSessionRequest {
            public_session_id: session.clone(),
            expiry_policy: Some(StreamSessionExpiryPolicy {
                kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(30)),
            }),
            creation_intent: StreamSessionCreationIntent::ExplicitPut as i32,
            invocation: Some(InvocationStart {
                agent_id: Some(source.clone().into()),
                environment_id: Some(component.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                component_owner_account_id: Some(component.account_id.into()),
                idempotency_key: Some(IdempotencyKey::new(session.clone()).into()),
                method_name: Some("echo".into()),
                input: Some(input),
                ..Default::default()
            }),
        })
        .await?
        .into_inner();
    let Some(create_stream_session_response::Result::Success(created)) = created.result else {
        anyhow::bail!("{created:?}");
    };
    let initial_deadline = created.expiry_deadline_millis.unwrap();
    let session_key: IdempotencyKey = created.invocation_key.clone().unwrap().into();
    let read = |admission, invocation_key| ReadStreamSlotRequest {
        agent_id: Some(source.clone().into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(AuthCtx::System.into()),
        session: session.clone(),
        slot: "input".into(),
        expected_method: "echo".into(),
        max_items: 0,
        max_bytes: 1_000_000,
        admission,
        invocation_key,
        ..Default::default()
    };

    tokio::time::sleep(std::time::Duration::from_millis(3_100)).await;
    for request in [
        read(StreamSlotReadAdmission::Head as i32, None),
        read(
            StreamSlotReadAdmission::Continuation as i32,
            created.invocation_key.clone(),
        ),
    ] {
        let response = executor
            .client
            .clone()
            .read_stream_slot(request)
            .await?
            .into_inner()
            .message()
            .await?
            .unwrap();
        let Some(read_stream_slot_response::Result::Success(response)) = response.result else {
            anyhow::bail!("{response:?}");
        };
        assert_eq!(response.expiry_deadline_millis, Some(initial_deadline));
    }

    let touched = executor
        .client
        .clone()
        .read_stream_slot(read(
            StreamSlotReadAdmission::TouchingOriginGet as i32,
            None,
        ))
        .await?
        .into_inner()
        .message()
        .await?
        .unwrap();
    let Some(read_stream_slot_response::Result::Success(touched)) = touched.result else {
        anyhow::bail!("{touched:?}");
    };
    let touched_deadline = touched.expiry_deadline_millis.unwrap();
    assert!(touched_deadline > initial_deadline);
    let suppressed = executor
        .client
        .clone()
        .read_stream_slot(read(
            StreamSlotReadAdmission::TouchingOriginGet as i32,
            None,
        ))
        .await?
        .into_inner()
        .message()
        .await?
        .unwrap();
    let Some(read_stream_slot_response::Result::Success(suppressed)) = suppressed.result else {
        anyhow::bail!("{suppressed:?}");
    };
    assert_eq!(suppressed.expiry_deadline_millis, Some(touched_deadline));

    let append = AppendToStreamSlotRequest {
        agent_id: Some(source.clone().into()),
        environment_id: Some(component.environment_id.into()),
        auth_ctx: Some(AuthCtx::System.into()),
        session,
        slot: "input".into(),
        expected_method: "echo".into(),
        payload: Some(append_to_stream_slot_request::Payload::Values(
            TypedStreamSlotItems {
                values: vec![
                    golem_api_grpc::proto::golem::schema::SchemaValue::try_from(
                        SchemaValue::String("refresh".into()),
                    )
                    .unwrap()
                    .encode_to_vec(),
                ],
            },
        )),
        producer: Some(ExternalStreamProducer {
            id: "writer".into(),
            epoch: 0,
            sequence: 0,
        }),
        ..Default::default()
    };
    tokio::time::sleep(std::time::Duration::from_millis(3_100)).await;
    let accepted = executor
        .client
        .clone()
        .append_to_stream_slot(append.clone())
        .await?
        .into_inner();
    assert!(matches!(
        accepted.result,
        Some(append_to_stream_slot_response::Result::Accepted(_))
    ));
    let accepted_deadline = accepted.expiry_deadline_millis.unwrap();
    assert!(accepted_deadline > touched_deadline);

    tokio::time::sleep(std::time::Duration::from_millis(3_100)).await;
    let duplicate = executor
        .client
        .clone()
        .append_to_stream_slot(append)
        .await?
        .into_inner();
    assert!(matches!(
        duplicate.result,
        Some(append_to_stream_slot_response::Result::Duplicate(_))
    ));
    assert!(duplicate.expiry_deadline_millis.unwrap() > accepted_deadline);
    let refresh_count = executor
        .get_oplog(&source, OplogIndex::INITIAL)
        .await?
        .iter()
        .filter(|entry| {
            matches!(&entry.entry, PublicOplogEntry::StreamSession(session)
            if matches!(
                StreamSessionRecord::from_value(session.record.value()).unwrap(),
                StreamSessionRecord::ExpiryRefreshed(ref record)
                    if record.session_key == session_key
            ))
        })
        .count();
    assert_eq!(refresh_count, 3);
    executor.shutdown_and_wait_for_invocation_loops().await?;
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
