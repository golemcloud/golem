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

//! `wasm-rpc.schedule`: schedules an invocation of a remote method for a given time. The call
//! returns the invocation's handle once the target has durably accepted it, and the handle can
//! cancel it until it starts. The call's `End` records the invocation's final ID or the target's
//! permanent refusal, so replay rebuilds the outcome without contacting the target.

use super::{
    CallResolution, InvocationEntry, PreparedRpcInvocation, RemoteCall, RpcCallKind, RpcCallTarget,
    WasmRpcEntry, activate_known_target, create_scheduled_ephemeral_target, interruptible,
    permanent_refusal, prepare_rpc_invocation, replay_call,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessPermissionCheck, Cancellable, DurableCallSession,
    reserve_invocation_id_before_await,
};
use crate::durable_host::with_import_access;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::card::PermissionTarget;
use golem_common::model::invocation_context::{InvocationContextSpan, InvocationContextStack};
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcSchedule;
use golem_common::model::oplog::payload::denied;
use golem_common::model::oplog::types::SerializableDateTime;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRpcScheduledInvocation,
    HostResponseGolemRpcInvocationAccepted,
};
use golem_common::model::{
    AgentFingerprint, IdempotencyKey, InvocationRequestKey, OwnedAgentId, OwnedInvocationId,
};
use golem_common::schema::schema_value::SchemaValue;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::{Accessor, HasSelf, Resource};

type ScheduleCall = DurableCallSession<GolemRpcWasmRpcSchedule, Cancellable>;

/// What a schedule validated and captured in the guest's call, before its first `await`.
struct PreparedSchedule {
    rpc: PreparedRpcInvocation,
    scheduled_at: jiff::Timestamp,
    /// The context the scheduled invocation runs in: a child of the scheduling span.
    stack: InvocationContextStack,
    /// The remote steps of the schedule. Its stack is the context of the call that creates an
    /// ephemeral target.
    remote: RemoteCall,
}

pub(super) async fn schedule<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: Resource<WasmRpcEntry>,
    datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
    method_name: String,
    input: core_wire::SchemaValueTree,
) -> anyhow::Result<Result<Resource<InvocationEntry>, RpcError>> {
    reserve_invocation_id_before_await(accessor, accessor.getter())?;
    let request = accessor
        .with(|mut access| prepare_schedule(access.get(), &this, datetime, method_name, input))?;
    let (prepared, target, call_target) = match request {
        Ok(request) => request,
        Err(error) => return Ok(Err(error)),
    };

    let (handle, denial) = start(accessor, &prepared, target, &call_target, datetime).await?;
    let response = match denial {
        Some(denial) => complete_denied(accessor, handle, &denial).await?,
        None => resolve(accessor, handle, &this, prepared, &call_target).await?,
    };
    accepted(accessor, response)
}

/// Starts the schedule's durable call under the target's authorization, or claims its recorded
/// `Start`. Returns the live denial, if the target refused the call.
async fn start<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    prepared: &PreparedSchedule,
    target: PermissionTarget,
    call_target: &RpcCallTarget,
    datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
) -> anyhow::Result<(ScheduleCall, Option<String>)> {
    let (permission_check, decision) = AccessPermissionCheck::new(Ok(vec![target]));
    let request_target = call_target.clone();
    let request_method_name = prepared.rpc.method_name.clone();
    let request_input = prepared.rpc.input_value.clone();
    let request_datetime: SerializableDateTime = datetime.into();
    let handle = ScheduleCall::start_access_with_agent_authority_and_indexed_request(
        accessor,
        accessor.getter(),
        DurableFunctionType::WriteRemote,
        AccessClaimOptions {
            permission_check: Some(permission_check),
            ..Default::default()
        },
        move |start_index, denied| {
            let (idempotency_key, remote_agent_id) = request_target.at(start_index)?;
            Ok((
                HostRequestGolemRpcScheduledInvocation {
                    remote_agent_id: remote_agent_id.agent_id(),
                    request_key: InvocationRequestKey::Internal(idempotency_key),
                    method_name: request_method_name,
                    input: if denied {
                        SchemaValue::Tuple {
                            elements: Vec::new(),
                        }
                    } else {
                        request_input
                    },
                    datetime: request_datetime,
                    remote_agent_type: None,
                    remote_agent_parameters: None,
                },
                None,
            ))
        },
    )
    .await?;
    let denial = match decision.await.ok().flatten() {
        Some(Err(denial)) if handle.is_live() => Some(denial),
        _ => None,
    };
    Ok((handle, denial))
}

/// Records the live denial as the call's result.
async fn complete_denied<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: ScheduleCall,
    denial: &str,
) -> anyhow::Result<HostResponseGolemRpcInvocationAccepted> {
    Ok(handle
        .complete_access(
            accessor,
            accessor.getter(),
            denied::rpc::invocation_accepted(&(), denial),
        )
        .await?)
}

/// Replays the recorded outcome of a completed schedule, or schedules the invocation now and
/// records the target's acceptance.
async fn resolve<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: ScheduleCall,
    this: &Resource<WasmRpcEntry>,
    prepared: PreparedSchedule,
    call_target: &RpcCallTarget,
) -> anyhow::Result<HostResponseGolemRpcInvocationAccepted> {
    let first_live_dispatch = handle.is_live();
    match replay_call(accessor, handle).await? {
        CallResolution::Recorded(response) => Ok(response),
        CallResolution::Live(mut handle) => {
            // The request's idempotency key derives from the call's own `Start` index, so a
            // repair of an incomplete `Start` schedules the same invocation again and the target
            // deduplicates it.
            let (idempotency_key, remote_agent_id) = call_target
                .at(handle.start_index())
                .map_err(|error| handle.trap(error))?;
            let result = schedule_live(
                accessor,
                &mut handle,
                this,
                prepared,
                &remote_agent_id,
                idempotency_key,
                first_live_dispatch,
            )
            .await?;
            Ok(handle
                .complete_access(
                    accessor,
                    accessor.getter(),
                    HostResponseGolemRpcInvocationAccepted {
                        result: result.map_err(Into::into),
                    },
                )
                .await?)
        }
    }
}

/// Validates the request as `trigger` does, and captures what the schedule needs after its first
/// await. An unknown method, an input that does not match the method, or a method with streams
/// is the guest's error, which the call returns before it opens a durable record. An invalid
/// time traps.
fn prepare_schedule<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: &Resource<WasmRpcEntry>,
    datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
    method_name: String,
    input: core_wire::SchemaValueTree,
) -> anyhow::Result<Result<(PreparedSchedule, PermissionTarget, RpcCallTarget), RpcError>> {
    let rpc = match prepare_rpc_invocation(ctx, this, RpcCallKind::Scheduled, method_name, input)? {
        Ok(rpc) => rpc,
        Err(error) => return Ok(Err(error)),
    };
    let scheduled_at = i32::try_from(datetime.nanoseconds)
        .ok()
        .and_then(|nanos| jiff::Timestamp::new(datetime.seconds, nanos).ok())
        .ok_or_else(|| {
            anyhow::Error::from(WorkerExecutorError::runtime(format!(
                "Received invalid datetime from wasi: seconds={}, nanoseconds={}",
                datetime.seconds, datetime.nanoseconds
            )))
        })?;
    let target =
        ctx.outbound_agent_method_target(this, &rpc.logical_remote_agent_id, &rpc.method_name)?;
    let call_target = RpcCallTarget::new(
        ctx.idempotency_key_position(None),
        &rpc.logical_remote_agent_id,
        rpc.ephemeral.as_ref(),
        &rpc.method_name,
    );
    let invocation_state = ctx.state.invocation_state();
    let stack = InvocationContextStack::new(
        invocation_state.invocation_context.trace_id.clone(),
        InvocationContextSpan::external_parent(invocation_state.current_span_id.clone()),
        invocation_state.invocation_context.trace_states.clone(),
    );
    let remote = RemoteCall::of(ctx, &rpc.connection_span_id);
    Ok(Ok((
        PreparedSchedule {
            rpc,
            scheduled_at,
            stack,
            remote,
        },
        target,
        call_target,
    )))
}

/// Commits the scheduled invocation in the target's oplog and returns its final ID, or the
/// target's permanent refusal. An interrupt and a transient failure trap and leave the call's
/// `Start` incomplete for repair.
async fn schedule_live<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut ScheduleCall,
    this: &Resource<WasmRpcEntry>,
    prepared: PreparedSchedule,
    remote_agent_id: &OwnedAgentId,
    idempotency_key: IdempotencyKey,
    first_live_dispatch: bool,
) -> anyhow::Result<Result<OwnedInvocationId, InternalRpcError>> {
    let target_fingerprint = if prepared.rpc.is_ephemeral() {
        create_scheduled_ephemeral_target(
            accessor,
            handle,
            &prepared.rpc,
            &prepared.remote,
            remote_agent_id,
            &idempotency_key,
            first_live_dispatch,
        )
        .await?
    } else {
        Ok(activate_known_target(accessor, this).map_err(|error| handle.trap(error))?)
    };
    match target_fingerprint {
        Ok(target_fingerprint) => {
            schedule_at_target(
                accessor,
                handle,
                prepared,
                remote_agent_id,
                idempotency_key,
                target_fingerprint,
            )
            .await
        }
        Err(error) => Ok(Err(error)),
    }
}

/// Schedules the invocation at the target whose fingerprint is known.
async fn schedule_at_target<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut ScheduleCall,
    prepared: PreparedSchedule,
    remote_agent_id: &OwnedAgentId,
    idempotency_key: IdempotencyKey,
    target_fingerprint: AgentFingerprint,
) -> anyhow::Result<Result<OwnedInvocationId, InternalRpcError>> {
    let auth_ctx = handle.agent_auth_ctx().clone();
    let request_key = InvocationRequestKey::Internal(idempotency_key);
    let invocation_id = request_key.finalize(
        remote_agent_id.agent_id(),
        target_fingerprint,
        &prepared.rpc.method_name,
    );
    let input = prepared
        .rpc
        .input_value
        .try_into()
        .map_err(|error: String| handle.trap(anyhow::anyhow!(error)))?;
    let scheduled = prepared.remote.schedule(
        remote_agent_id,
        target_fingerprint,
        request_key,
        &invocation_id,
        prepared.rpc.method_name,
        input,
        prepared.stack,
        prepared.scheduled_at,
        &auth_ctx,
    );
    // The schedule's wake is not recorded: the handle cancels the invocation by its ID, and a
    // wake that finds no pending invocation has no effect.
    let result = interruptible(accessor, handle, scheduled).await?;
    Ok(permanent_refusal(handle, result)?.map(|_| invocation_id))
}

fn accepted<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    response: HostResponseGolemRpcInvocationAccepted,
) -> anyhow::Result<Result<Resource<InvocationEntry>, RpcError>> {
    Ok(match response.result {
        Ok(id) => Ok(with_import_access(accessor, |mut access| {
            access.get().table().push(InvocationEntry { id })
        })?),
        Err(error) => Err(InternalRpcError::from(error).into()),
    })
}

#[cfg(test)]
mod tests;
