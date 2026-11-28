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

//! The parts that every live remote step of an RPC call shares: what it sends on behalf of the
//! calling agent, and how it waits for the remote answer.

use super::{PreparedRpcInvocation, RpcCall, dispatch_target};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{Cancellable, DurableCallSession};
use crate::durable_host::durability::DurabilityHost;
use crate::durable_host::with_import_access;
use crate::services::rpc::{Rpc, RpcDemand, RpcError as InternalRpcError};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use golem_api_grpc::proto::golem::worker::InvocationStart;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentMethodInput, InvocationFreshnessDisposition};
use golem_common::model::caller::Caller;
use golem_common::model::invocation_context::{InvocationContextStack, SpanId};
use golem_common::model::oplog::HostPayloadPair;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, InvocationRequestKey, OwnedAgentId,
    OwnedInvocationId,
};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_service_base::model::auth::AuthCtx;
use std::future::Future;
use std::sync::Arc;
use wasmtime::component::{Accessor, HasSelf};

/// What a live remote step sends on behalf of the calling agent.
pub struct RemoteCall {
    rpc: Arc<dyn Rpc>,
    created_by: AccountId,
    agent_id: AgentId,
    /// The invocation context of the remote step: a child of `span_id`.
    stack: InvocationContextStack,
    caller: Caller,
}

impl RemoteCall {
    pub fn of<Ctx: WorkerCtx>(ctx: &mut DurableWorkerCtx<Ctx>, span_id: &SpanId) -> Self {
        Self {
            rpc: ctx.rpc(),
            created_by: ctx.created_by(),
            agent_id: *ctx.agent_id(),
            stack: ctx.clone_as_inherited_stack(span_id),
            caller: ctx.outgoing_caller(),
        }
    }

    pub fn capture<U: Send + 'static, Ctx: WorkerCtx>(
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        span_id: &SpanId,
    ) -> Self {
        with_import_access(accessor, |mut access| Self::of(access.get(), span_id))
    }

    /// Creates or finds the instance of the target and returns its demand. The `invocation`
    /// is the first invocation that the target receives, so an ephemeral instance starts with
    /// it.
    pub async fn create_demand(
        &self,
        prepared: &PreparedRpcInvocation,
        remote_agent_id: &OwnedAgentId,
        invocation: InvocationStart,
        idempotency_key: &IdempotencyKey,
        freshness: InvocationFreshnessDisposition,
        auth_ctx: &AuthCtx,
    ) -> Result<Box<dyn RpcDemand>, InternalRpcError> {
        self.rpc
            .create_demand(
                dispatch_target(remote_agent_id, prepared.ephemeral.as_ref()),
                &prepared.method_name,
                Some(invocation),
                self.created_by,
                &self.agent_id,
                &prepared.env,
                self.stack.clone(),
                &self.caller,
                prepared.config.clone(),
                auth_ctx,
                Some(idempotency_key.clone()),
                freshness,
            )
            .await
    }

    /// Delivers the call's invocation to its target with the call's idempotency key, and
    /// returns once the target accepted it. The target already exists: a durable target always
    /// does, and preparation created an ephemeral one.
    pub async fn invoke(
        &self,
        call: &RpcCall,
        expected_callee_fingerprint: Option<AgentFingerprint>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), InternalRpcError> {
        let prepared = call.prepared();
        self.rpc
            .invoke(
                call.dispatch_target(),
                Some(call.idempotency_key().clone()),
                InvocationFreshnessDisposition::MayExist,
                expected_callee_fingerprint,
                prepared.method_name.clone(),
                prepared.input_value.clone(),
                self.created_by,
                &self.agent_id,
                &prepared.env,
                self.stack.clone(),
                &self.caller,
                prepared.config.clone(),
                auth_ctx,
            )
            .await
    }

    /// Schedules an invocation of `method_name` at the target's incarnation
    /// `target_fingerprint`, to run at `scheduled_at` in `invocation_context`, on behalf of the
    /// calling agent. Returns the invocation's final ID, which derives from `idempotency_key`.
    pub async fn schedule(
        &self,
        remote_agent_id: &OwnedAgentId,
        target_fingerprint: AgentFingerprint,
        idempotency_key: IdempotencyKey,
        method_name: String,
        input: AgentMethodInput,
        invocation_context: InvocationContextStack,
        scheduled_at: jiff::Timestamp,
        auth_ctx: &AuthCtx,
    ) -> Result<OwnedInvocationId, InternalRpcError> {
        let request_key = InvocationRequestKey::Internal(idempotency_key);
        let invocation_id =
            request_key.finalize(remote_agent_id.agent_id(), target_fingerprint, &method_name);
        let invocation = AgentInvocation::AgentMethod {
            idempotency_key: invocation_id.id(),
            method_name,
            allow_streams: false,
            input,
            invocation_context,
            caller: self.caller.clone(),
            scope_card: None,
        };
        self.rpc
            .schedule_invocation(
                remote_agent_id,
                target_fingerprint,
                request_key,
                invocation,
                scheduled_at,
                &self.agent_id,
                auth_ctx,
            )
            .await
            .map(|()| invocation_id)
    }
}

/// Waits for a short remote step of a live call, such as the target's acceptance. An interrupt
/// of the agent traps the call and leaves its `Start` incomplete for repair.
pub async fn interruptible<U, Ctx, Pair, T>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut DurableCallSession<Pair, Cancellable>,
    step: impl Future<Output = T>,
) -> anyhow::Result<T>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
{
    let interrupt = with_import_access(accessor, |mut access| {
        access.get().create_interrupt_signal()
    });
    tokio::select! {
        biased;
        kind = interrupt => Err(handle.trap(kind)),
        result = step => Ok(result),
    }
}

/// Waits for a long remote step, such as an invocation's result. The wait is an RPC wait, so
/// the agent can suspend while it lasts; an interrupt or a suspension ends it with its kind.
pub async fn suspendable<U, Ctx, T>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    step: impl Future<Output = T>,
) -> Result<T, InterruptKind>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let (rpc_wait, interrupt) = with_import_access(accessor, |mut access| {
        let ctx = access.get();
        let wait = accessor.runtime_activity().and_then(|activity| {
            ctx.runtime_suspension.as_ref().and_then(|runtime| {
                runtime.rpc_wait(
                    activity,
                    ctx.state.config.suspend.rpc_suspend_after,
                    ctx.state.config.suspend.rpc_resume_after,
                )
            })
        });
        (wait, ctx.create_interrupt_signal())
    });
    match rpc_wait {
        Some(wait) => wait.wait_result(step, interrupt).await,
        None => tokio::select! {
            biased;
            kind = interrupt => Err(kind),
            result = step => Ok(result),
        },
    }
}
