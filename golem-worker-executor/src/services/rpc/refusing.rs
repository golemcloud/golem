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

//! The RPC of a never-live owner. It refuses every call, so no other agent is reached.

use super::*;

pub(crate) struct RefusingRpc;

#[async_trait]
impl Rpc for RefusingRpc {
    async fn cancel_scheduled_invocation(
        &self,
        _target: &OwnedAgentId,
        _key: golem_common::model::InvocationId,
        _receipt: golem_common::model::ScheduleId,
        _owner: &AgentId,
        _auth: &AuthCtx,
    ) -> Result<bool, RpcError> {
        Err(RpcError::Denied {
            details: crate::durable_host::never_live::refusal("rpc::cancel_scheduled_invocation")
                .to_string(),
        })
    }

    async fn schedule_invocation(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _expected_fingerprint: AgentFingerprint,
        _request_key: InvocationRequestKey,
        _invocation: AgentInvocation,
        _scheduled_at: chrono::DateTime<chrono::Utc>,
        _auth_ctx: &AuthCtx,
    ) -> Result<golem_common::model::ScheduleId, RpcError> {
        Err(RpcError::Denied {
            details: crate::durable_host::never_live::refusal("rpc::schedule_invocation")
                .to_string(),
        })
    }

    async fn create_and_initialize(
        &self,
        _target: AgentTarget,
        _environment_id: EnvironmentId,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _caller: &Caller,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _idempotency_key: IdempotencyKey,
    ) -> Result<Box<dyn RpcDemand>, WorkerExecutorError> {
        Err(crate::durable_host::never_live::refusal(
            "rpc::create_and_initialize",
        ))
    }

    async fn create_demand(
        &self,
        _target: AgentTarget,
        _environment_id: EnvironmentId,
        _method_name: &str,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _caller: &Caller,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        Err(RpcError::Denied {
            details: crate::durable_host::never_live::refusal("rpc::create_demand").to_string(),
        })
    }

    async fn invoke_and_await(
        &self,
        _target: AgentTarget,
        _environment_id: EnvironmentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: InvocationFreshnessDisposition,
        _expected_callee_fingerprint: Option<AgentFingerprint>,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _caller: &Caller,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _scope_card: Option<ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        Err(RpcError::Denied {
            details: crate::durable_host::never_live::refusal("rpc::invoke_and_await").to_string(),
        })
    }

    async fn invoke_and_await_streaming(
        &self,
        _target: AgentTarget,
        _environment_id: EnvironmentId,
        _idempotency_key: IdempotencyKey,
        _method_name: String,
        _method_parameters: ProtoSchemaValue,
        _input_mappings: Vec<DurableStreamMapping>,
        _expected_callee_fingerprint: AgentFingerprint,
        _attempt_id: uuid::Uuid,
        _origin_invocation: StreamInvocationIdentity,
        _accepted_inputs: tokio::sync::oneshot::Sender<InvocationAccepted>,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _caller: &Caller,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _scope_card: Option<ScopeCard>,
    ) -> Result<DurableRpcInvocationResult, RpcError> {
        Err(RpcError::Denied {
            details: crate::durable_host::never_live::refusal("rpc::invoke_and_await_streaming")
                .to_string(),
        })
    }

    async fn control_durable_stream_attachment(
        &self,
        _request: StreamAttachmentControlRequest,
        _auth_ctx: &AuthCtx,
    ) -> Result<bool, DurableStreamRemoteError<RpcError>> {
        Err(RpcError::Denied {
            details: crate::durable_host::never_live::refusal(
                "rpc::control_durable_stream_attachment",
            )
            .to_string(),
        }
        .into())
    }

    async fn read_durable_stream_segment(
        &self,
        _request: DurableStreamReadRequest,
        _auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, DurableStreamRemoteError<RpcError>> {
        Err(RpcError::Denied {
            details: crate::durable_host::never_live::refusal("rpc::read_durable_stream_segment")
                .to_string(),
        }
        .into())
    }

    async fn invoke(
        &self,
        _target: AgentTarget,
        _environment_id: EnvironmentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: InvocationFreshnessDisposition,
        _expected_callee_fingerprint: Option<AgentFingerprint>,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _caller: &Caller,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), RpcError> {
        Err(RpcError::Denied {
            details: crate::durable_host::never_live::refusal("rpc::invoke").to_string(),
        })
    }
}
