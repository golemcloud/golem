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

use super::denial::denied_rpc_request;
use super::execution::classify_rpc_error;
use super::{
    InvocationEntry, PreparedRpcInvocation, RpcCallTarget, WasmRpcEntry, WasmRpcEntryPayload,
    WasmRpcTargetActivation, dispatch_target, embedded_invocation_span_builder,
    known_fresh_dispatch_allowed, prepare_rpc_invocation, restore_embedded_span, rpc_span_finished,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessPermissionCheck, CallReplayOutcome, Cancellable, DurableCallSession,
    finish_span_in_memory, reserve_invocation_id_before_await,
};
use crate::durable_host::durability::{DurabilityHost, InternalRetryResult};
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::{ReplayedDemand, RpcError as InternalRpcError};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::oplog::HostPayloadPair;
use golem_common::model::oplog::host_functions::{
    GolemRpcWasmRpcPrepareInvocation, GolemRpcWasmRpcTrigger,
};
use golem_common::model::oplog::payload::denied;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRpcPrepareInvocation,
    HostResponseGolemRpcInvocationAccepted, HostResponseGolemRpcPrepareInvocation, OplogEntry,
};
use golem_common::model::{
    AgentFingerprint, IdempotencyKey, InvocationRequestKey, OwnedAgentId, OwnedInvocationId,
};
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::{Accessor, HasSelf, Resource};

type TriggerCall = DurableCallSession<GolemRpcWasmRpcTrigger, Cancellable>;

pub(super) async fn trigger<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: Resource<WasmRpcEntry>,
    method_name: String,
    input: core_wire::SchemaValueTree,
) -> anyhow::Result<Result<Resource<InvocationEntry>, RpcError>> {
    reserve_invocation_id_before_await(accessor, accessor.getter())?;
    let prepared = accessor.with(|mut access| {
        let ctx = access.get();
        let prepared = match prepare_rpc_invocation(
            ctx,
            &this,
            "golem::rpc::wasm-rpc::trigger",
            false,
            method_name,
            input,
            false,
        )? {
            Ok(prepared) => prepared,
            Err(error) => return Ok::<_, anyhow::Error>(Err(error)),
        };
        let target = ctx.outbound_agent_method_target(
            &this,
            &prepared.logical_remote_agent_id,
            &prepared.method_name,
        )?;
        ctx.record_outbound_rpc_call()?;
        let span_started = embedded_invocation_span_builder(
            ctx,
            &prepared.connection_span_id,
            &prepared.method_name,
        )?;
        let call_target = RpcCallTarget::new(
            ctx.idempotency_key_position(None),
            &prepared.logical_remote_agent_id,
            prepared.ephemeral.as_ref(),
            &prepared.method_name,
        );
        Ok(Ok((prepared, target, span_started, call_target)))
    })?;
    let (prepared, target, span_started, call_target) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return Ok(Err(error)),
    };

    let (permission_check, decision) = AccessPermissionCheck::new(Ok(vec![target]));
    let request_prepared = prepared.clone();
    let request_target = call_target.clone();
    // A tool body that returned before this call finished parks the call's replay; its recorded
    // preparation child is then claimed as an abandoned subtree.
    let parks_incomplete_replay = prepared.is_ephemeral()
        && accessor.with(|mut access| access.get().parks_incomplete_tool_guest_replay());
    let mut handle = TriggerCall::start_access_with_agent_authority_and_indexed_request(
        accessor,
        accessor.getter(),
        DurableFunctionType::WriteRemote,
        AccessClaimOptions {
            permission_check: Some(permission_check),
            host_internal_calls: prepared.is_ephemeral(),
            abandon_incomplete_host_internal_calls: parks_incomplete_replay,
            ..Default::default()
        },
        move |start_index, denied| {
            let (idempotency_key, remote_agent_id) = request_target.at(start_index)?;
            let started = span_started(&idempotency_key);
            let request = if denied {
                denied_rpc_request(
                    &remote_agent_id,
                    &idempotency_key,
                    request_prepared.method_name.clone(),
                )
            } else {
                request_prepared.invoke_request(&remote_agent_id, &idempotency_key, None, None)
            };
            Ok((request, Some(started)))
        },
    )
    .await?;
    // The live decision holds its authorization permit until the call completes.
    let decision = decision.await.ok().flatten();
    let started_live = handle.is_live();

    let started = handle
        .recorded_span_started_access(accessor, accessor.getter())
        .await
        .map_err(|error| handle.trap(error))?
        .ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                handle.start_index(),
                "RPC trigger Start carrying span_started",
                format!(
                    "RPC trigger Start at {} without span metadata",
                    handle.start_index()
                ),
            )
        })
        .map_err(|error| handle.trap(error))?;
    let span = accessor
        .with(|mut access| restore_embedded_span(access.get(), &started))
        .map_err(|error| handle.trap(error))?;
    let span_id = span.span_id().clone();
    handle.close_span_on_cancellation(span_id.clone());

    if started_live && let Some(Err(denial)) = &decision {
        let response = denied::rpc::invocation_accepted(&(), denial);
        let finished = rpc_span_finished::<()>(
            &span_id,
            &Err(InternalRpcError::Denied {
                details: denial.clone(),
            }),
        );
        let response = handle
            .complete_access_with_span(accessor, accessor.getter(), response, finished)
            .await?;
        return accepted(accessor, response, &span_id);
    }

    if !started_live {
        // A completed trigger repeats no preparation; its replay claims the recorded one.
        let denied = start_records_denial(accessor, &handle)
            .await
            .map_err(|error| handle.trap(error))?;
        let recorded_preparation = if prepared.is_ephemeral() && !denied {
            let (idempotency_key, remote_agent_id) = call_target
                .at(handle.start_index())
                .map_err(|error| handle.trap(error))?;
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

    let (idempotency_key, remote_agent_id) = call_target
        .at(handle.start_index())
        .map_err(|error| handle.trap(error))?;
    let freshness = if known_fresh_dispatch_allowed(started_live, prepared.is_ephemeral()) {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    };
    let fingerprint =
        match recorded_target_fingerprint(accessor, &this).map_err(|error| handle.trap(error))? {
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
    accessor.with(|mut access| {
        let ctx = access.get();
        finish_span_in_memory(ctx, span_id)?;
        Ok(match response.result {
            Ok(id) => Ok(ctx.table().push(InvocationEntry { id })?),
            Err(error) => Err(InternalRpcError::from(error).into()),
        })
    })
}

/// Whether the call's `Start` records a denial: the call was refused before it prepared or
/// dispatched anything.
pub(super) async fn start_records_denial<
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &DurableCallSession<Pair, Cancellable>,
) -> Result<bool, WorkerExecutorError> {
    let oplog = accessor.with(|mut access| access.get().state.oplog.clone());
    match oplog.read(handle.start_index()).await? {
        OplogEntry::Start { denial, .. } => Ok(denial.is_some()),
        entry => Err(WorkerExecutorError::unexpected_oplog_entry(
            handle.start_index(),
            format!("{entry:?}"),
            format!("Start at {}", handle.start_index()),
        )),
    }
}

/// The fingerprint of the target's incarnation that the client already knows: every target but
/// an ephemeral one, whose instance each invocation creates.
pub(super) fn recorded_target_fingerprint<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: &Resource<WasmRpcEntry>,
) -> anyhow::Result<Option<AgentFingerprint>> {
    accessor.with(|mut access| {
        let payload = access
            .get()
            .table()
            .get(this)?
            .payload
            .downcast_ref::<WasmRpcEntryPayload>()
            .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
        Ok(payload.target_activation.target_fingerprint())
    })
}

type PrepareCall = DurableCallSession<GolemRpcWasmRpcPrepareInvocation, Cancellable>;

/// The recorded request of the host-internal call that creates the instance of an ephemeral
/// target for one invocation.
pub(super) fn preparation_request(
    start_index: golem_common::model::oplog::OplogIndex,
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
    owner: crate::durable_host::concurrent::HostInternalCallOwner,
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
    span_id: &golem_common::model::invocation_context::SpanId,
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
            let mut freshness = freshness;
            let result = loop {
                let (rpc, created_by, agent_id, stack, caller, interrupt) =
                    accessor.with(|mut access| {
                        let ctx = access.get();
                        (
                            ctx.rpc(),
                            ctx.created_by(),
                            *ctx.agent_id(),
                            ctx.clone_as_inherited_stack(span_id),
                            ctx.outgoing_caller(),
                            ctx.create_interrupt_signal(),
                        )
                    });
                let demand = rpc.create_demand(
                    dispatch_target(remote_agent_id, prepared.ephemeral.as_ref()),
                    &prepared.method_name,
                    Some(invocation.clone()),
                    created_by,
                    &agent_id,
                    &prepared.env,
                    stack,
                    &caller,
                    prepared.config.clone(),
                    &auth_ctx,
                    Some(idempotency_key.clone()),
                    freshness,
                );
                let result = tokio::select! {
                    biased;
                    kind = interrupt => return Err(handle.trap(kind)),
                    result = demand => result.map(|demand| demand.fingerprint()),
                };
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
    let result = loop {
        let (rpc, created_by, agent_id, stack, caller, interrupt) = accessor.with(|mut access| {
            let ctx = access.get();
            (
                ctx.rpc(),
                ctx.created_by(),
                *ctx.agent_id(),
                ctx.clone_as_inherited_stack(span_id),
                ctx.outgoing_caller(),
                ctx.create_interrupt_signal(),
            )
        });
        // A durable target exists, and preparation already created an ephemeral one.
        let invocation = rpc.invoke(
            dispatch_target(remote_agent_id, prepared.ephemeral.as_ref()),
            Some(idempotency_key.clone()),
            InvocationFreshnessDisposition::MayExist,
            expected_callee_fingerprint,
            prepared.method_name.clone(),
            prepared.input_value.clone(),
            created_by,
            &agent_id,
            &prepared.env,
            stack,
            &caller,
            prepared.config.clone(),
            &auth_ctx,
        );
        let result = tokio::select! {
            biased;
            kind = interrupt => return Err(handle.trap(kind)),
            result = invocation => result,
        };
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

/// Marks a durable target as activated at the incarnation the client recorded, without a remote
/// call, and returns that incarnation's fingerprint.
pub(super) fn activate_known_target<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: &Resource<WasmRpcEntry>,
) -> anyhow::Result<AgentFingerprint> {
    accessor.with(|mut access| {
        let payload = access
            .get()
            .table()
            .get_mut(this)?
            .payload
            .downcast_mut::<WasmRpcEntryPayload>()
            .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
        match &payload.target_activation {
            WasmRpcTargetActivation::Activated {
                target_fingerprint, ..
            } => Ok(*target_fingerprint),
            WasmRpcTargetActivation::DeferredEphemeral { .. } => Err(anyhow::anyhow!(
                "An ephemeral RPC target does not support pre-activation"
            )),
            WasmRpcTargetActivation::ReplayPending {
                target_fingerprint,
                env,
                config,
            } => {
                let target_fingerprint = *target_fingerprint;
                payload.target_activation = WasmRpcTargetActivation::Activated {
                    demand: Box::new(ReplayedDemand::new(target_fingerprint)),
                    target_fingerprint,
                    env: env.clone(),
                    config: config.clone(),
                };
                Ok(target_fingerprint)
            }
        }
    })
}
