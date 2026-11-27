// `wasm-rpc.schedule` records the result of `Rpc::schedule_invocation` when
// `classify_rpc_error` calls it permanent, and traps, leaving the `Start` incomplete for repair,
// when it is transient. These tests drive the remote schedule path through a worker-service
// session.

use super::super::execution::classify_rpc_error;
use crate::durable_host::durability::HostFailureKind;
use crate::services::rpc::{RemoteInvocationRpc, Rpc};
use crate::services::shard::ShardServiceDefault;
use crate::services::worker_proxy::{
    InvocationRequestStream, InvocationResponseStream, WorkerProxy, WorkerProxyError,
};
use crate::worker::target::ResolvedTarget;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::{
    InvocationAccepted, InvocationResponse, UpdateMode, invocation_request, invocation_response,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentInvocationMode, InvocationFreshnessDisposition, ResolvedRevert,
    RevertWorkerTarget,
};
use golem_common::model::caller::Caller;
use golem_common::model::card::{CardId, ScopeCard, StoredCard};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, AgentInvocationOutput, IdempotencyKey,
    InvocationId, InvocationRequestKey, InvocationStatus, OwnedAgentId, PromiseId,
};
use golem_common::schema::SchemaValue;
use golem_service_base::model::auth::AuthCtx;
use std::collections::HashMap;
use std::sync::Arc;
use test_r::test;
use tokio_stream::StreamExt;

/// How the worker service answers the schedule's invocation session.
enum SessionAnswer {
    /// The session cannot be opened.
    FailOnOpen,
    /// The session opens, then fails at the transport level before any response, for example a
    /// connection reset after the request was sent.
    FailBeforeResponse,
    /// The target accepts the scheduled invocation as the executor does: the acceptance has no
    /// component revision, because the revision is selected only when the invocation runs.
    Accept(InvocationId),
}

struct ScheduleSession(SessionAnswer);

#[async_trait]
impl WorkerProxy for ScheduleSession {
    async fn invocation_handle(
        &self,
        _: golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleRequest,
    ) -> Result<
        golem_api_grpc::proto::golem::workerexecutor::v1::InvocationHandleSuccess,
        WorkerProxyError,
    > {
        unimplemented!()
    }
    async fn prepare(
        &self,
        _: &ResolvedTarget,
        _: &str,
        _: Option<golem_api_grpc::proto::golem::worker::InvocationStart>,
        _: &AgentId,
        _: HashMap<String, String>,
        _: InvocationContextStack,
        _: Vec<AgentConfigEntryDto>,
        _: Caller,
        _: &AuthCtx,
        _: Option<IdempotencyKey>,
        _: InvocationFreshnessDisposition,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        unimplemented!()
    }
    async fn create(
        &self,
        _: &ResolvedTarget,
        _: bool,
        _: &AgentId,
        _: HashMap<String, String>,
        _: InvocationContextStack,
        _: Vec<AgentConfigEntryDto>,
        _: Caller,
        _: &AuthCtx,
        _: Option<IdempotencyKey>,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        unimplemented!()
    }
    async fn invoke_agent(
        &self,
        _: &ResolvedTarget,
        _: String,
        _: SchemaValue,
        _: AgentInvocationMode,
        _: Option<jiff::Timestamp>,
        _: Option<IdempotencyKey>,
        _: InvocationFreshnessDisposition,
        _: Option<AgentFingerprint>,
        _: AgentId,
        _: HashMap<String, String>,
        _: InvocationContextStack,
        _: Vec<AgentConfigEntryDto>,
        _: Caller,
        _: EnvironmentId,
        _: &AuthCtx,
        _: Option<ScopeCard>,
        _: bool,
    ) -> Result<AgentInvocationOutput, WorkerProxyError> {
        unimplemented!()
    }
    async fn invoke_agent_session(
        &self,
        mut request: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerProxyError> {
        let status = tonic::Status::unavailable("connection reset by peer");
        let final_id = match self.0 {
            SessionAnswer::FailOnOpen => return Err(status.into()),
            SessionAnswer::FailBeforeResponse => {
                return Ok(Box::pin(tokio_stream::iter([Err(status)])));
            }
            SessionAnswer::Accept(final_id) => final_id,
        };
        let Some(invocation_request::Request::Start(start)) =
            request.next().await.and_then(|request| request.request)
        else {
            panic!("a schedule opens its session with a start");
        };
        let target = match start.target.and_then(|target| target.target) {
            Some(golem_api_grpc::proto::golem::worker::agent_target::Target::Id(id)) => id,
            other => panic!("unexpected target {other:?}"),
        };
        let accepted = InvocationAccepted {
            agent_id: Some(target),
            idempotency_key: Some(final_id.into()),
            component_revision: None,
            method_name: start.method_name,
            captured_method_input: start.captured_method_input,
            ..Default::default()
        };
        Ok(Box::pin(tokio_stream::iter([Ok(InvocationResponse {
            response: Some(invocation_response::Response::Accepted(accepted)),
        })])))
    }
    async fn deliver_card_transfer(
        &self,
        _: &AgentId,
        _: EnvironmentId,
        _: uuid::Uuid,
        _: CardId,
        _: &StoredCard,
    ) -> Result<(), WorkerProxyError> {
        unimplemented!()
    }
    async fn update(
        &self,
        _: &OwnedAgentId,
        _: ComponentRevision,
        _: UpdateMode,
        _: bool,
        _: Caller,
        _: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        unimplemented!()
    }
    async fn resume(&self, _: &OwnedAgentId, _: bool, _: &AuthCtx) -> Result<(), WorkerProxyError> {
        unimplemented!()
    }
    async fn fork_worker(
        &self,
        _: &OwnedAgentId,
        _: &AgentId,
        _: &OplogIndex,
        _: Caller,
        _: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        unimplemented!()
    }
    async fn revert(
        &self,
        _: &OwnedAgentId,
        _: RevertWorkerTarget,
        _: Option<ResolvedRevert>,
        _: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        unimplemented!()
    }
    async fn complete_promise(
        &self,
        _: EnvironmentId,
        _: PromiseId,
        _: Vec<u8>,
        _: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        unimplemented!()
    }
    async fn cancel_invocation(
        &self,
        _: &OwnedAgentId,
        _: InvocationId,
        _: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        unimplemented!()
    }
    async fn lookup_invocation_status(
        &self,
        _: &AgentId,
        _: InvocationId,
        _: Option<EnvironmentId>,
        _: &AuthCtx,
    ) -> Result<InvocationStatus, WorkerProxyError> {
        unimplemented!()
    }
    async fn process_oplog_entries(
        &self,
        _: &AgentId,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
        _: InvocationId,
        _: AccountId,
        _: Vec<(String, String)>,
        _: golem_api_grpc::proto::golem::worker::AgentMetadata,
        _: OplogIndex,
        _: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        _: &AuthCtx,
        _: &Caller,
    ) -> Result<(), WorkerProxyError> {
        unimplemented!()
    }
}

/// Schedules `increment` on a remote target through a session that answers with `answer`.
async fn remote_schedule(
    answer: impl FnOnce(InvocationId) -> SessionAnswer,
) -> Result<(), crate::services::rpc::RpcError> {
    let environment_id = EnvironmentId(uuid::Uuid::from_u128(0xe0));
    let scheduler = AgentId::from_parts(7, 0xc0);
    let target = OwnedAgentId::new(environment_id, &AgentId::from_parts(8, 0xc1));
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(0xf1));
    let request_key = InvocationRequestKey::Internal(IdempotencyKey::from("schedule-child"));
    let final_id = request_key.finalize(target.agent_id(), fingerprint, "increment");
    let rpc = RemoteInvocationRpc::new(
        Arc::new(ScheduleSession(answer(final_id.id()))),
        Arc::new(ShardServiceDefault::new()),
    );
    rpc.schedule_invocation(
        &target,
        fingerprint,
        request_key,
        AgentInvocation::AgentMethod {
            idempotency_key: final_id.id(),
            method_name: "increment".to_string(),
            input: SchemaValue::Tuple {
                elements: Vec::new(),
            }
            .try_into()
            .unwrap(),
            allow_streams: false,
            invocation_context: InvocationContextStack::fresh(),
            caller: Caller {
                original: crate::test_callers::user_principal(),
                immediate: crate::test_callers::agent_principal(scheduler),
                tokens: Vec::new(),
            },
            scope_card: None,
        },
        jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
        &scheduler,
        &AuthCtx::System,
    )
    .await
}

#[test]
async fn a_transport_failure_when_opening_the_schedule_session_is_transient() {
    let result = remote_schedule(|_| SessionAnswer::FailOnOpen).await;
    let error = result.expect_err("the session failed before acceptance");
    assert_eq!(
        classify_rpc_error(&error),
        HostFailureKind::Transient,
        "{error:?}"
    );
}

#[test]
async fn a_transport_failure_of_the_schedule_session_before_acceptance_is_transient() {
    // The request was sent; the target may have committed the scheduled invocation. Recording
    // this as the schedule's permanent refusal tells the guest the invocation was not
    // scheduled, while a repair would find it.
    let result = remote_schedule(|_| SessionAnswer::FailBeforeResponse).await;
    let error = result.expect_err("the session failed before acceptance");
    assert_eq!(
        classify_rpc_error(&error),
        HostFailureKind::Transient,
        "{error:?}"
    );
}

#[test]
async fn a_remote_schedule_accepted_without_a_component_revision_is_accepted() {
    let result = remote_schedule(SessionAnswer::Accept).await;
    assert_eq!(
        result,
        Ok(()),
        "the target committed the scheduled invocation"
    );
}
