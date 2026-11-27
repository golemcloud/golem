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
    InvocationEntry, OpenRpcCall, PreparedRpcInvocation, RemoteCall, RpcCallKind, RpcCallOpening,
    WasmRpcEntry, activate_known_target, claim_completed_preparation, dispatch_target,
    interruptible, known_fresh_dispatch_allowed, open_rpc_call, preparation_request,
    prepare_ephemeral_identity, rpc_span_finished,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    CallReplayOutcome, Cancellable, DurableCallSession, finish_span_in_memory,
};
use crate::durable_host::durability::InternalRetryResult;
use crate::durable_host::with_import_access;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::oplog::HostResponseGolemRpcInvocationAccepted;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcTrigger;
use golem_common::model::oplog::payload::denied;
use golem_common::model::{
    AgentFingerprint, IdempotencyKey, InvocationRequestKey, OwnedAgentId, OwnedInvocationId,
};
use golem_schema::schema::wit::wire as core_wire;
use wasmtime::component::{Accessor, HasSelf, Resource};

type TriggerCall = DurableCallSession<GolemRpcWasmRpcTrigger, Cancellable>;

pub(super) async fn trigger<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: Resource<WasmRpcEntry>,
    method_name: String,
    input: core_wire::SchemaValueTree,
) -> anyhow::Result<Result<Resource<InvocationEntry>, RpcError>> {
    let call = match open_rpc_call::<_, _, GolemRpcWasmRpcTrigger>(
        accessor,
        &this,
        RpcCallKind::Triggered,
        method_name,
        input,
        None,
    )
    .await?
    {
        RpcCallOpening::Rejected(error) => return Ok(Err(error)),
        RpcCallOpening::Denied {
            handle,
            span_id,
            denial,
        } => {
            let response = denied::rpc::invocation_accepted(&(), &denial);
            let finished = rpc_span_finished::<()>(
                &span_id,
                &Err(InternalRpcError::Denied { details: denial }),
            );
            let response = handle
                .complete_access_with_span(accessor, accessor.getter(), response, finished)
                .await?;
            return accepted(accessor, response, &span_id);
        }
        RpcCallOpening::Open(call) => call,
    };
    let OpenRpcCall {
        mut handle,
        prepared,
        span_id,
        idempotency_key,
        remote_agent_id,
        started_live,
        known_fingerprint,
        ..
    } = call;

    if !started_live {
        // A completed trigger repeats no preparation; its replay claims the recorded one.
        let denied = handle
            .recorded_denial_access(accessor, accessor.getter())
            .await
            .map_err(|error| handle.trap(error))?;
        let recorded_preparation = if prepared.is_ephemeral() && !denied {
            Some((
                handle.host_internal_owner(),
                preparation_request(
                    handle.start_index(),
                    &prepared,
                    &remote_agent_id,
                    &idempotency_key,
                ),
            ))
        } else {
            None
        };
        match handle.replay_access(accessor, accessor.getter()).await? {
            CallReplayOutcome::Replayed(response) => {
                if let Some((owner, request)) = recorded_preparation {
                    claim_completed_preparation(accessor, owner, request).await?;
                }
                return accepted(accessor, response, &span_id);
            }
            CallReplayOutcome::Incomplete(live) => handle = live,
        }
    }

    let freshness = if known_fresh_dispatch_allowed(started_live, prepared.is_ephemeral()) {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    };
    let fingerprint = match known_fingerprint {
        Some(fingerprint) => Ok(fingerprint),
        None => {
            prepare_ephemeral_identity(
                accessor,
                &mut handle,
                &prepared,
                &remote_agent_id,
                &idempotency_key,
                &span_id,
                freshness,
                "trigger",
            )
            .await?
        }
    };
    let result = match fingerprint {
        Ok(fingerprint) => {
            dispatch(
                accessor,
                &mut handle,
                &this,
                &prepared,
                &remote_agent_id,
                &idempotency_key,
                fingerprint,
                &span_id,
            )
            .await?
        }
        Err(error) => Err(error),
    };
    let finished = rpc_span_finished(&span_id, &result);
    let response = handle
        .complete_access_with_span(
            accessor,
            accessor.getter(),
            HostResponseGolemRpcInvocationAccepted {
                result: result.map_err(Into::into),
            },
            finished,
        )
        .await?;
    accepted(accessor, response, &span_id)
}

/// Finishes the call's span and gives the guest the handle of the accepted invocation, or the
/// recorded refusal.
fn accepted<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: HostResponseGolemRpcInvocationAccepted,
    span_id: &golem_common::model::invocation_context::SpanId,
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
#[allow(clippy::too_many_arguments)]
async fn dispatch<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut TriggerCall,
    this: &Resource<WasmRpcEntry>,
    prepared: &PreparedRpcInvocation,
    remote_agent_id: &OwnedAgentId,
    idempotency_key: &IdempotencyKey,
    fingerprint: AgentFingerprint,
    span_id: &golem_common::model::invocation_context::SpanId,
) -> anyhow::Result<Result<OwnedInvocationId, InternalRpcError>> {
    let expected_callee_fingerprint = if prepared.is_ephemeral() {
        None
    } else {
        Some(activate_known_target(accessor, this).map_err(|error| handle.trap(error))?)
    };
    let auth_ctx = handle.agent_auth_ctx().clone();
    let retry_properties = prepared.retry_properties("trigger", remote_agent_id);
    let remote = RemoteCall::capture(accessor, span_id);
    let result = loop {
        // A durable target exists, and preparation already created an ephemeral one.
        let invocation = remote.rpc.invoke(
            dispatch_target(remote_agent_id, prepared.ephemeral.as_ref()),
            Some(idempotency_key.clone()),
            InvocationFreshnessDisposition::MayExist,
            expected_callee_fingerprint,
            prepared.method_name.clone(),
            prepared.input_value.clone(),
            remote.created_by,
            &remote.agent_id,
            &prepared.env,
            remote.stack.clone(),
            &remote.caller,
            prepared.config.clone(),
            &auth_ctx,
        );
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
    Ok(result.map(|()| {
        InvocationRequestKey::Internal(idempotency_key.clone()).finalize(
            remote_agent_id.agent_id(),
            fingerprint,
            &prepared.method_name,
        )
    }))
}
