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

use super::execution::{classify_rpc_error, known_fresh_dispatch_allowed};
use super::{
    InvocationEntry, PreparedRpcInvocation, RemoteCall, RpcCallKind, RpcCallTarget, WasmRpcEntry,
    activate_known_target, dispatch_target, interruptible, prepare_rpc_invocation,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessPermissionCheck, CallReplayOutcome, Cancellable, DurableCallSession,
    reserve_invocation_id_before_await,
};
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::with_import_access;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::agent::InvocationFreshnessDisposition;
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
    AgentInvocation, IdempotencyKey, InvocationRequestKey, OwnedAgentId, OwnedInvocationId,
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

    let (permission_check, decision) = AccessPermissionCheck::new(Ok(vec![target]));
    let request_target = call_target.clone();
    let request_method_name = prepared.rpc.method_name.clone();
    let request_input = prepared.rpc.input_value.clone();
    let request_datetime: SerializableDateTime = datetime.into();
    let mut handle = ScheduleCall::start_access_with_agent_authority_and_indexed_request(
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
    // The live decision holds its authorization permit until the call completes.
    let decision = decision.await.ok().flatten();
    let started_live = handle.is_live();

    if started_live && let Some(Err(denial)) = &decision {
        let response = handle
            .complete_access(
                accessor,
                accessor.getter(),
                denied::rpc::invocation_accepted(&(), denial),
            )
            .await?;
        return accepted(accessor, response);
    }

    if !started_live {
        match handle.replay_access(accessor, accessor.getter()).await? {
            CallReplayOutcome::Replayed(response) => return accepted(accessor, response),
            CallReplayOutcome::Incomplete(live) => handle = live,
        }
    }

    // The request's idempotency key derives from the call's own `Start` index, so a repair of an
    // incomplete `Start` schedules the same invocation again and the target deduplicates it.
    let (idempotency_key, remote_agent_id) = call_target
        .at(handle.start_index())
        .map_err(|error| handle.trap(error))?;
    let result = schedule_live(
        accessor,
        &mut handle,
        &this,
        prepared,
        &remote_agent_id,
        idempotency_key,
        started_live,
    )
    .await?;
    let response = handle
        .complete_access(
            accessor,
            accessor.getter(),
            HostResponseGolemRpcInvocationAccepted {
                result: result.map_err(Into::into),
            },
        )
        .await?;
    accepted(accessor, response)
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
    let auth_ctx = handle.agent_auth_ctx().clone();
    let remote = &prepared.remote;

    let target_fingerprint = if prepared.rpc.is_ephemeral() {
        // The ephemeral target is created now, so the schedule commits in its oplog and the
        // wake pins this incarnation. The target runs nothing before the deadline. Only the
        // first live dispatch may create a fresh target; a repair may find it.
        let freshness = if known_fresh_dispatch_allowed(first_live_dispatch, true) {
            with_import_access(accessor, |mut access| access.get().public_state.worker())
                .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                .await
                .map_err(|error| handle.trap(error))?;
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };
        let invocation = crate::worker::invocation::native_invocation_start(
            &prepared.rpc.method_name,
            &prepared.rpc.input_value,
            false,
        )
        .map_err(|error| handle.trap(anyhow::Error::from(error)))?;
        let demand = remote.rpc.create_demand(
            dispatch_target(remote_agent_id, prepared.rpc.ephemeral.as_ref()),
            &prepared.rpc.method_name,
            Some(invocation),
            remote.created_by,
            &remote.agent_id,
            &prepared.rpc.env,
            remote.stack.clone(),
            &remote.caller,
            prepared.rpc.config.clone(),
            &auth_ctx,
            Some(idempotency_key.clone()),
            freshness,
        );
        match interruptible(accessor, handle, demand).await? {
            Ok(demand) => demand.fingerprint(),
            Err(error) => return refusal(handle, error),
        }
    } else {
        activate_known_target(accessor, this).map_err(|error| handle.trap(error))?
    };

    let request_key = InvocationRequestKey::Internal(idempotency_key);
    let invocation_id = request_key.finalize(
        remote_agent_id.agent_id(),
        target_fingerprint,
        &prepared.rpc.method_name,
    );
    let invocation = AgentInvocation::AgentMethod {
        idempotency_key: invocation_id.id(),
        method_name: prepared.rpc.method_name,
        allow_streams: false,
        input: prepared
            .rpc
            .input_value
            .try_into()
            .map_err(|error: String| handle.trap(anyhow::anyhow!(error)))?,
        invocation_context: prepared.stack,
        caller: remote.caller.clone(),
        scope_card: None,
    };
    let scheduled = remote.rpc.schedule_invocation(
        remote_agent_id,
        target_fingerprint,
        request_key,
        invocation,
        prepared.scheduled_at,
        &remote.agent_id,
        &auth_ctx,
    );
    // The schedule's wake is not recorded: the handle cancels the invocation by its ID, and a
    // wake that finds no pending invocation has no effect.
    match interruptible(accessor, handle, scheduled).await? {
        Ok(_) => Ok(Ok(invocation_id)),
        Err(error) => refusal(handle, error),
    }
}

/// Records a permanent refusal of the target as the call's result, and traps on a transient
/// failure, which a repair of the call retries.
fn refusal(
    handle: &mut ScheduleCall,
    error: InternalRpcError,
) -> anyhow::Result<Result<OwnedInvocationId, InternalRpcError>> {
    match classify_rpc_error(&error) {
        HostFailureKind::Permanent => Ok(Err(error)),
        HostFailureKind::Transient => Err(handle.trap(anyhow::Error::from(error))),
    }
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
