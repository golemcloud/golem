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

//! Creation of the one-shot instance that runs one invocation of an ephemeral agent type.

use super::execution::classify_rpc_error;
use super::{PreparedRpcInvocation, RemoteCall, dispatch_target, interruptible};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{Cancellable, DurableCallSession, HostInternalCallOwner};
use crate::durable_host::durability::InternalRetryResult;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcPrepareInvocation;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestGolemRpcPrepareInvocation,
    HostResponseGolemRpcPrepareInvocation, OplogIndex,
};
use golem_common::model::{AgentFingerprint, IdempotencyKey, InvocationRequestKey, OwnedAgentId};
use wasmtime::component::{Accessor, HasSelf};

type PrepareCall = DurableCallSession<GolemRpcWasmRpcPrepareInvocation, Cancellable>;

/// The recorded request of the host-internal call that creates the instance of an ephemeral
/// target for one invocation.
pub(super) fn preparation_request(
    start_index: OplogIndex,
    prepared: &PreparedRpcInvocation,
    remote_agent_id: &OwnedAgentId,
    idempotency_key: &IdempotencyKey,
) -> HostRequestGolemRpcPrepareInvocation {
    HostRequestGolemRpcPrepareInvocation {
        invocation_start_index: start_index,
        remote_agent_id: remote_agent_id.agent_id(),
        method_name: prepared.method_name.clone(),
        request_key: InvocationRequestKey::Internal(idempotency_key.clone()),
    }
}

/// Replays the recorded preparation of a completed call to an ephemeral target. The completed
/// call repeats no preparation, so there is no live action.
pub(super) async fn claim_completed_preparation<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    owner: HostInternalCallOwner,
    request: HostRequestGolemRpcPrepareInvocation,
) -> anyhow::Result<HostResponseGolemRpcPrepareInvocation> {
    PrepareCall::invoke_host_internal_access(
        accessor,
        accessor.getter(),
        owner,
        true,
        request,
        DurableFunctionType::WriteRemote,
        async || {
            Err::<HostResponseGolemRpcPrepareInvocation, _>(anyhow::anyhow!(
                "a completed RPC call has no live preparation"
            ))
        },
    )
    .await
}

/// Creates the ephemeral instance that runs the invocation and records its fingerprint as a
/// host-internal call of the calling host function, so repair and replay keep the instance.
#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_ephemeral_identity<
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut DurableCallSession<Pair, Cancellable>,
    prepared: &PreparedRpcInvocation,
    remote_agent_id: &OwnedAgentId,
    idempotency_key: &IdempotencyKey,
    span_id: &SpanId,
    freshness: InvocationFreshnessDisposition,
    verb: &str,
) -> anyhow::Result<Result<AgentFingerprint, InternalRpcError>> {
    let owner = handle.host_internal_owner();
    let request = preparation_request(
        handle.start_index(),
        prepared,
        remote_agent_id,
        idempotency_key,
    );
    let auth_ctx = handle.agent_auth_ctx().clone();
    let response = PrepareCall::invoke_host_internal_access(
        accessor,
        accessor.getter(),
        owner,
        false,
        request,
        DurableFunctionType::WriteRemote,
        async || {
            let invocation = crate::worker::invocation::native_invocation_start(
                &prepared.method_name,
                &prepared.input_value,
                prepared.is_streaming(),
            )
            .map_err(anyhow::Error::from)?;
            let remote = RemoteCall::capture(accessor, span_id);
            let mut freshness = freshness;
            let result = loop {
                let demand = remote.rpc.create_demand(
                    dispatch_target(remote_agent_id, prepared.ephemeral.as_ref()),
                    &prepared.method_name,
                    Some(invocation.clone()),
                    remote.created_by,
                    &remote.agent_id,
                    &prepared.env,
                    remote.stack.clone(),
                    &remote.caller,
                    prepared.config.clone(),
                    &auth_ctx,
                    Some(idempotency_key.clone()),
                    freshness,
                );
                let result = interruptible(accessor, handle, demand)
                    .await?
                    .map(|demand| demand.fingerprint());
                freshness = InvocationFreshnessDisposition::MayExist;
                match handle
                    .try_trigger_retry_or_loop_context(
                        accessor,
                        accessor.getter(),
                        &result,
                        classify_rpc_error,
                        prepared.retry_properties(verb, remote_agent_id),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            Ok::<_, anyhow::Error>(HostResponseGolemRpcPrepareInvocation {
                result: result.map_err(Into::into),
            })
        },
    )
    .await
    .map_err(|error| handle.trap(error))?;
    Ok(response.result.map_err(Into::into))
}
