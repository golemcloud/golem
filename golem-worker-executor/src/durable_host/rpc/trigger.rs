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

//! `wasm-rpc.trigger`: invokes a remote method without awaiting its result. The call returns the
//! invocation's handle once the target has durably accepted the invocation, and its `End`
//! records the invocation's final ID, so replay rebuilds the handle without contacting the
//! target.

use super::execution::classify_rpc_error;
use super::{
    CallResolution, InvocationEntry, RemoteCall, RpcCall, RpcCallKind, RpcCallOpening,
    WasmRpcEntry, activate_known_target, interruptible, open_rpc_call, rpc_span_finished,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{Cancellable, DurableCallSession, finish_span_in_memory};
use crate::durable_host::durability::InternalRetryResult;
use crate::durable_host::with_import_access;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::HostResponseGolemRpcInvocationAccepted;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcTrigger;
use golem_common::model::oplog::payload::denied;
use golem_common::model::{AgentFingerprint, OwnedInvocationId};
use golem_schema::schema::wit::wire as core_wire;
use wasmtime::component::{Accessor, HasSelf, Resource};

type TriggerCall = DurableCallSession<GolemRpcWasmRpcTrigger, Cancellable>;

pub async fn trigger<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: Resource<WasmRpcEntry>,
    method_name: String,
    input: core_wire::SchemaValueTree,
) -> anyhow::Result<Result<Resource<InvocationEntry>, RpcError>> {
    match open_rpc_call::<_, _, GolemRpcWasmRpcTrigger>(
        accessor,
        &this,
        RpcCallKind::Triggered,
        method_name,
        input,
        None,
    )
    .await?
    {
        RpcCallOpening::Rejected(error) => Ok(Err(error)),
        RpcCallOpening::Denied {
            handle,
            span_id,
            denial,
        } => {
            let response = complete_denied(accessor, handle, &span_id, denial).await?;
            accepted(accessor, response, &span_id)
        }
        RpcCallOpening::Open { handle, call } => {
            let response = resolve(accessor, handle, &call, &this).await?;
            accepted(accessor, response, call.span_id())
        }
    }
}

/// Records the live denial as the call's result.
async fn complete_denied<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: TriggerCall,
    span_id: &SpanId,
    denial: String,
) -> anyhow::Result<HostResponseGolemRpcInvocationAccepted> {
    let response = denied::rpc::invocation_accepted(&(), &denial);
    let finished =
        rpc_span_finished::<()>(span_id, &Err(InternalRpcError::Denied { details: denial }));
    Ok(handle
        .complete_access_with_span(accessor, accessor.getter(), response, finished)
        .await?)
}

/// Replays the recorded acceptance of a completed call, or runs the call now: prepares an
/// ephemeral target, delivers the invocation and records the target's acceptance.
async fn resolve<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: TriggerCall,
    call: &RpcCall,
    this: &Resource<WasmRpcEntry>,
) -> anyhow::Result<HostResponseGolemRpcInvocationAccepted> {
    match call.replay(accessor, handle).await? {
        CallResolution::Recorded((response, completed)) => {
            completed.claim_preparation(accessor).await?;
            Ok(response)
        }
        CallResolution::Live(mut handle) => {
            let result = match call.target_fingerprint(accessor, &mut handle).await? {
                Ok(fingerprint) => dispatch(accessor, &mut handle, call, this, fingerprint).await?,
                Err(error) => Err(error),
            };
            let finished = rpc_span_finished(call.span_id(), &result);
            Ok(handle
                .complete_access_with_span(
                    accessor,
                    accessor.getter(),
                    HostResponseGolemRpcInvocationAccepted {
                        result: result.map_err(Into::into),
                    },
                    finished,
                )
                .await?)
        }
    }
}

/// Finishes the call's span and gives the guest the handle of the accepted invocation, or the
/// recorded refusal.
fn accepted<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: HostResponseGolemRpcInvocationAccepted,
    span_id: &SpanId,
) -> anyhow::Result<Result<Resource<InvocationEntry>, RpcError>> {
    with_import_access(accessor, |mut access| {
        let ctx = access.get();
        finish_span_in_memory(ctx, span_id)?;
        Ok(match response.result {
            Ok(id) => Ok(ctx.table().push(InvocationEntry { id })?),
            Err(error) => Err(InternalRpcError::from(error).into()),
        })
    })
}

/// Delivers the invocation to the target with the call's idempotency key and returns its final
/// ID once the target accepted it. A durable target is pinned to the incarnation the client
/// knows.
async fn dispatch<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut TriggerCall,
    call: &RpcCall,
    this: &Resource<WasmRpcEntry>,
    fingerprint: AgentFingerprint,
) -> anyhow::Result<Result<OwnedInvocationId, InternalRpcError>> {
    let expected_callee_fingerprint = if call.prepared().is_ephemeral() {
        None
    } else {
        Some(activate_known_target(accessor, this).map_err(|error| handle.trap(error))?)
    };
    let auth_ctx = handle.agent_auth_ctx().clone();
    let retry_properties = call.retry_properties();
    let result = loop {
        let remote = RemoteCall::capture(accessor, call.span_id());
        let invocation = remote.invoke(call, expected_callee_fingerprint, &auth_ctx);
        let result = interruptible(accessor, handle, invocation).await?;
        match handle
            .try_trigger_retry_or_loop_context(
                accessor,
                accessor.getter(),
                &result,
                classify_rpc_error,
                retry_properties.clone(),
            )
            .await?
        {
            InternalRetryResult::Persist => break result,
            InternalRetryResult::RetryInternally => continue,
        }
    };
    Ok(result.map(|()| call.final_id(fingerprint)))
}
