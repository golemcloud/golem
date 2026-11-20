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
// See the License for the specific language governing permissions and
// limitations under the License.

//! The worker proxy of a never-live owner. It refuses every call, so no other agent is
//! reached.

use super::*;

pub(crate) struct RefusingWorkerProxy;

#[async_trait]
impl WorkerProxy for RefusingWorkerProxy {
    async fn prepare(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _method_name: &str,
        _caller_agent_id: &AgentId,
        _caller_env: HashMap<String, String>,
        _caller_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _caller: Caller,
        _auth_ctx: &AuthCtx,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::prepare"),
        ))
    }

    async fn create(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _caller_agent_id: &AgentId,
        _caller_env: HashMap<String, String>,
        _caller_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _caller: Caller,
        _auth_ctx: &AuthCtx,
        _idempotency_key: Option<IdempotencyKey>,
    ) -> Result<AgentFingerprint, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::create"),
        ))
    }

    async fn invoke_agent(
        &self,
        _agent_id: &AgentId,
        _method_name: String,
        _method_parameters: SchemaValue,
        _mode: AgentInvocationMode,
        _schedule_at: Option<DateTime<Utc>>,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: InvocationFreshnessDisposition,
        _expected_callee_fingerprint: Option<AgentFingerprint>,
        _caller_agent_id: AgentId,
        _caller_env: HashMap<String, String>,
        _caller_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _caller: Caller,
        _environment_id: EnvironmentId,
        _auth_ctx: &AuthCtx,
        _scope_card: Option<ScopeCard>,
    ) -> Result<AgentInvocationOutput, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::invoke_agent"),
        ))
    }

    async fn invoke_agent_session(
        &self,
        _request: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::invoke_agent_session"),
        ))
    }

    async fn control_durable_stream_attachment(
        &self,
        _request: StreamAttachmentControlRequest,
        _auth_ctx: &AuthCtx,
    ) -> Result<bool, DurableStreamRemoteError<WorkerProxyError>> {
        Err(
            WorkerProxyError::InternalError(crate::durable_host::never_live::refusal(
                "worker_proxy::control_durable_stream_attachment",
            ))
            .into(),
        )
    }

    async fn read_durable_stream_segment(
        &self,
        _request: DurableStreamReadRequest,
        _auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, DurableStreamRemoteError<WorkerProxyError>> {
        Err(
            WorkerProxyError::InternalError(crate::durable_host::never_live::refusal(
                "worker_proxy::read_durable_stream_segment",
            ))
            .into(),
        )
    }

    async fn deliver_card_transfer(
        &self,
        _target_agent_id: &AgentId,
        _environment_id: EnvironmentId,
        _transfer_id: uuid::Uuid,
        _source_card_id: CardId,
        _card: &StoredCard,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::deliver_card_transfer"),
        ))
    }

    async fn update(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _target_revision: ComponentRevision,
        _mode: UpdateMode,
        _disable_wakeup: bool,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::update"),
        ))
    }

    async fn resume(
        &self,
        _owned_agent_id: &AgentId,
        _force: bool,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::resume"),
        ))
    }

    async fn fork_worker(
        &self,
        _source_agent_id: &AgentId,
        _target_agent_id: &AgentId,
        _oplog_index_cutoff: &OplogIndex,
        _caller: Caller,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::fork_worker"),
        ))
    }

    async fn revert(
        &self,
        _agent_id: &AgentId,
        _target: RevertWorkerTarget,
        _resolved_revert: Option<ResolvedRevert>,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::revert"),
        ))
    }

    async fn complete_promise(
        &self,
        _promise_id: PromiseId,
        _data: Vec<u8>,
        _auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::complete_promise"),
        ))
    }

    async fn cancel_invocation(
        &self,
        _agent_id: &AgentId,
        _idempotency_key: InvocationId,
        _auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::cancel_invocation"),
        ))
    }

    async fn cancel_scheduled_invocation(
        &self,
        _target: &OwnedAgentId,
        _key: InvocationId,
        _receipt: ScheduleId,
        _owner: &AgentId,
        _auth: &AuthCtx,
    ) -> Result<bool, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::cancel_scheduled_invocation"),
        ))
    }

    async fn lookup_invocation_status(
        &self,
        _agent_id: &AgentId,
        _idempotency_key: InvocationId,
        _environment_id: Option<EnvironmentId>,
        _auth_ctx: &AuthCtx,
    ) -> Result<InvocationStatus, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::lookup_invocation_status"),
        ))
    }

    async fn process_oplog_entries(
        &self,
        _target_agent_id: &AgentId,
        _environment_id: EnvironmentId,
        _component_revision: ComponentRevision,
        _idempotency_key: InvocationId,
        _account_id: AccountId,
        _config: Vec<(String, String)>,
        _metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        _first_entry_index: OplogIndex,
        _entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            crate::durable_host::never_live::refusal("worker_proxy::process_oplog_entries"),
        ))
    }
}
