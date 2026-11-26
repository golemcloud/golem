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
use super::trigger::activate_known_target;
use super::{
    InvocationEntry, RpcCallTarget, WasmRpcEntry, WasmRpcEntryPayload, dispatch_target,
    find_agent_method, remote_method_uses_streams,
};
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    AccessClaimOptions, AccessPermissionCheck, CallReplayOutcome, Cancellable, DurableCallSession,
    reserve_invocation_id_before_await,
};
use crate::durable_host::durability::{DurabilityHost, HostFailureKind};
use crate::preview2::golem::agent::host::RpcError;
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use golem_common::model::agent::{AgentConfigEntryDto, InvocationFreshnessDisposition};
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
use golem_schema::schema::wit::decode_value_with;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::{Accessor, HasSelf, Resource};

type ScheduleCall = DurableCallSession<GolemRpcWasmRpcSchedule, Cancellable>;

/// What a schedule validated and captured in the guest's call, before its first `await`.
struct PreparedSchedule {
    ephemeral: Option<super::EphemeralRpcTarget>,
    method_name: String,
    input_value: SchemaValue,
    scheduled_at: jiff::Timestamp,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    /// The context the scheduled invocation runs in: a child of the scheduling span.
    stack: InvocationContextStack,
    /// The context of the call that creates an ephemeral target.
    inherited_stack: InvocationContextStack,
    caller: golem_common::model::caller::Caller,
}

pub(super) async fn schedule<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    this: Resource<WasmRpcEntry>,
    datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
    method_name: String,
    input: core_wire::SchemaValueTree,
) -> anyhow::Result<Result<Resource<InvocationEntry>, RpcError>> {
    reserve_invocation_id_before_await(accessor, accessor.getter())?;
    // A guest bug, such as an unknown method, an input that does not match the method or an
    // invalid time, traps before the call opens a durable record.
    let (prepared, target, call_target) = accessor.with(|mut access| {
        let ctx = access.get();
        ctx.check_read_only_allows("golem::rpc::wasm-rpc::schedule")
            .map_err(wasmtime::Error::from)?;
        let (
            logical_remote_agent_id,
            ephemeral,
            remote_agent_type,
            remote_method_streams,
            span_id,
            env,
            config,
        ) = {
            let payload = ctx
                .table()
                .get(&this)?
                .payload
                .downcast_ref::<WasmRpcEntryPayload>()
                .ok_or_else(|| anyhow::anyhow!("invalid RPC resource payload"))?;
            let (env, config) = payload.target_activation.target_creation_data();
            (
                payload.remote_agent_id.clone(),
                payload.ephemeral.clone(),
                payload.remote_agent_type.clone(),
                payload.remote_method_streams.clone(),
                payload.span_id.clone(),
                env,
                config,
            )
        };
        // Lifting the input first consumes the owned handles it carries, so none leaks when the
        // method check fails.
        let input_value = decode_value_with(input, ctx)
            .map_err(|err| anyhow::anyhow!("Invalid RPC input: {err}"))?;
        let method = find_agent_method(&remote_agent_type, &method_name)?;
        method
            .validate_input(&remote_agent_type.schema, &input_value)
            .map_err(|error| anyhow::anyhow!("Invalid RPC input: {error}"))?;
        if remote_method_uses_streams(&remote_method_streams, &method.name)? {
            return Err(anyhow::anyhow!(
                "live streams cannot be used in scheduled invocations"
            ));
        }
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
            ctx.outbound_agent_method_target(&this, &logical_remote_agent_id, &method_name)?;
        let call_target = RpcCallTarget::new(
            ctx.idempotency_key_position(None),
            &logical_remote_agent_id,
            ephemeral.as_ref(),
            &method_name,
        );
        let invocation_state = ctx.state.invocation_state();
        let stack = InvocationContextStack::new(
            invocation_state.invocation_context.trace_id.clone(),
            InvocationContextSpan::external_parent(invocation_state.current_span_id.clone()),
            invocation_state.invocation_context.trace_states.clone(),
        );
        let prepared = PreparedSchedule {
            ephemeral,
            method_name,
            input_value,
            scheduled_at,
            env,
            config,
            stack,
            inherited_stack: ctx.clone_as_inherited_stack(&span_id),
            caller: ctx.outgoing_caller(),
        };
        Ok::<_, anyhow::Error>((prepared, target, call_target))
    })?;

    let (permission_check, decision) = AccessPermissionCheck::new(Ok(vec![target]));
    let request_target = call_target.clone();
    let request_method_name = prepared.method_name.clone();
    let request_input = prepared.input_value.clone();
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
    let (rpc, created_by, agent_id, worker) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            ctx.rpc(),
            ctx.created_by(),
            *ctx.agent_id(),
            ctx.public_state.worker().clone(),
        )
    });
    let interrupt = || accessor.with(|mut access| access.get().create_interrupt_signal());

    let target_fingerprint = if prepared.ephemeral.is_some() {
        // The ephemeral target is created now, so the schedule commits in its oplog and the
        // wake pins this incarnation. The target runs nothing before the deadline. Only the
        // first live dispatch may create a fresh target; a repair may find it.
        let freshness = if known_fresh_dispatch_allowed(first_live_dispatch, true) {
            worker
                .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                .await?;
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };
        let invocation = crate::worker::invocation::native_invocation_start(
            &prepared.method_name,
            &prepared.input_value,
            false,
        )
        .map_err(|error| handle.trap(anyhow::Error::from(error)))?;
        let demand = rpc.create_demand(
            dispatch_target(remote_agent_id, prepared.ephemeral.as_ref()),
            &prepared.method_name,
            Some(invocation),
            created_by,
            &agent_id,
            &prepared.env,
            prepared.inherited_stack.clone(),
            &prepared.caller,
            prepared.config.clone(),
            &auth_ctx,
            Some(idempotency_key.clone()),
            freshness,
        );
        let interrupt = interrupt();
        let result = tokio::select! {
            biased;
            kind = interrupt => return Err(handle.trap(kind)),
            result = demand => result,
        };
        match result {
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
        &prepared.method_name,
    );
    let invocation = AgentInvocation::AgentMethod {
        idempotency_key: invocation_id.id(),
        method_name: prepared.method_name,
        allow_streams: false,
        input: prepared
            .input_value
            .try_into()
            .map_err(|error: String| handle.trap(anyhow::anyhow!(error)))?,
        invocation_context: prepared.stack,
        caller: prepared.caller,
        scope_card: None,
    };
    let scheduled = rpc.schedule_invocation(
        remote_agent_id,
        target_fingerprint,
        request_key,
        invocation,
        prepared.scheduled_at,
        &agent_id,
        &auth_ctx,
    );
    let interrupt = interrupt();
    let result = tokio::select! {
        biased;
        kind = interrupt => return Err(handle.trap(kind)),
        result = scheduled => result,
    };
    // The schedule's wake is not recorded: the handle cancels the invocation by its ID, and a
    // wake that finds no pending invocation has no effect.
    match result {
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
        Ok(id) => {
            Ok(accessor.with(|mut access| access.get().table().push(InvocationEntry { id }))?)
        }
        Err(error) => Err(InternalRpcError::from(error).into()),
    })
}

#[cfg(test)]
mod tests;
