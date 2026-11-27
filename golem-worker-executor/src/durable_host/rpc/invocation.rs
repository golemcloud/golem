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

//! The `golem:agent/host.invocation` resource: a handle that locates an accepted invocation by
//! its ID. Rebuilding, reading and dropping a handle are local. `status`, `await` and `cancel`
//! are durable remote calls that the target authorizes; their recorded results replay without
//! contacting the target. An operational failure, such as an unreachable target or an
//! unestablished owner, traps and leaves the call's `Start` incomplete for repair.

use super::values::{deliver_rpc_result, invoke_and_await_response_to_wire};
use super::{
    CallResolution, InvocationEntry, admit_rpc_result_secret_holds, replay_call,
    replay_call_deferred, suspendable,
};
use crate::durable_host::DurabilityHost;
use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::Cancellable;
use crate::durable_host::concurrent::DurableCallSession;
use crate::durable_host::concurrent::reserve_invocation_id_before_await;
use crate::durable_host::with_import_access;
use crate::preview2::golem::agent::host::HostInvocation;
use crate::preview2::golem::agent::host::HostInvocationWithStore;
use crate::preview2::golem::agent::host::InvocationStatus;
use crate::preview2::golem::agent::host::RpcError;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_api_grpc::proto::golem::worker::{InvocationSessionResult, invocation_session_result};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationHandleOperation, InvocationHandleSuccess, InvocationLifecycleState,
    invocation_handle_success,
};
use golem_common::model::InvocationId;
use golem_common::model::OwnedInvocationId;
use golem_common::model::oplog::host_functions::{
    GolemRpcInvocationAwait, GolemRpcInvocationCancel, GolemRpcInvocationStatus,
};
use golem_common::model::oplog::types::{SerializableInvocationStatus, SerializableRpcError};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestGolemRpcInvocationHandle,
    HostResponseGolemRpcInvocationCancel, HostResponseGolemRpcInvocationStatus,
    HostResponseGolemRpcInvokeAndAwait,
};
use golem_common::schema::schema_value::SchemaValue;
use golem_schema::schema::wit::wire as core_wire;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use wasmtime::component::{Accessor, HasSelf, Resource};

impl<Ctx: WorkerCtx> HostInvocation for DurableWorkerCtx<Ctx> {
    async fn get(
        &mut self,
        id: core_wire::InvocationId,
    ) -> anyhow::Result<Resource<InvocationEntry>> {
        self.observe_function_call("golem::rpc::invocation", "get");
        let agent_id = match id.owner {
            core_wire::InvocationOwner::Agent(agent_id) => agent_id,
            core_wire::InvocationOwner::Tool(tool) => {
                return Err(anyhow::anyhow!(
                    "invocation.get: the invocation ID of tool {tool} has no agent owner"
                ));
            }
        };
        let id = OwnedInvocationId::new(
            golem_common::model::AgentId::from(agent_id),
            InvocationId(id.id.into()),
        );
        Ok(self.table().push(InvocationEntry { id })?)
    }

    async fn id(
        &mut self,
        self_: Resource<InvocationEntry>,
    ) -> anyhow::Result<core_wire::InvocationId> {
        self.observe_function_call("golem::rpc::invocation", "id");
        let entry = self.table().get(&self_)?;
        Ok(wire_invocation_id(&entry.id))
    }

    async fn drop(&mut self, rep: Resource<InvocationEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::invocation", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostInvocationWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn status(
        accessor: &Accessor<U, Self>,
        self_: Resource<InvocationEntry>,
    ) -> anyhow::Result<Result<InvocationStatus, RpcError>> {
        let response = recorded_operation::<_, _, GolemRpcInvocationStatus, _>(
            accessor,
            self_,
            DurableFunctionType::ReadRemote,
            InvocationHandleOperation::Status,
            status_outcome,
            |result| HostResponseGolemRpcInvocationStatus { result },
        )
        .await?;
        Ok(status_to_wire(response.result))
    }

    async fn await_(
        accessor: &Accessor<U, Self>,
        self_: Resource<InvocationEntry>,
    ) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
        let id = handle_target(accessor, &self_)?;
        let handle = start_handle_call::<_, _, GolemRpcInvocationAwait>(
            accessor,
            &id,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let (response, delivery) = match replay_call_deferred(accessor, handle).await? {
            CallResolution::Recorded(recorded) => recorded,
            CallResolution::Live(mut handle) => {
                // `await` never admits work, so repairing an incomplete call asks the target
                // again without triggering the invocation a second time.
                let result = observe_outcome(
                    accessor,
                    &mut handle,
                    &id,
                    InvocationHandleOperation::Await,
                    await_outcome,
                )
                .await?;
                let result = admit_rpc_result_secret_holds(accessor, result)
                    .await
                    .map_err(|error| handle.trap(error))?;
                handle
                    .complete_access_deferred(
                        accessor,
                        accessor.getter(),
                        HostResponseGolemRpcInvokeAndAwait {
                            result: result.map_err(Into::into),
                        },
                    )
                    .await?
            }
        };
        deliver_rpc_result(accessor, delivery, None, |ctx| {
            invoke_and_await_response_to_wire(response.result, ctx)
        })
        .await
    }

    async fn cancel(
        accessor: &Accessor<U, Self>,
        self_: Resource<InvocationEntry>,
    ) -> anyhow::Result<Result<bool, RpcError>> {
        let response = recorded_operation::<_, _, GolemRpcInvocationCancel, _>(
            accessor,
            self_,
            DurableFunctionType::WriteRemote,
            InvocationHandleOperation::Cancel,
            cancel_outcome,
            |result| HostResponseGolemRpcInvocationCancel { result },
        )
        .await?;
        Ok(rpc_result_to_wire(response.result))
    }
}

/// The target's answer to a handle operation, before it is split into a recorded result or a
/// trap.
type Observed = Result<InvocationHandleSuccess, WorkerExecutorError>;

/// Runs a handle operation whose result reaches the guest as soon as it is recorded: replays the
/// recorded response, or asks the target now and records its answer.
async fn recorded_operation<U, Ctx, Pair, T>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    self_: Resource<InvocationEntry>,
    function_type: DurableFunctionType,
    operation: InvocationHandleOperation,
    split: fn(Observed) -> Result<Result<T, InternalRpcError>, WorkerExecutorError>,
    respond: fn(Result<T, SerializableRpcError>) -> Pair::Resp,
) -> anyhow::Result<Pair::Resp>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<Req = HostRequestGolemRpcInvocationHandle>,
{
    let id = handle_target(accessor, &self_)?;
    let handle = start_handle_call::<_, _, Pair>(accessor, &id, function_type).await?;
    match replay_call(accessor, handle).await? {
        CallResolution::Recorded(response) => Ok(response),
        CallResolution::Live(mut handle) => {
            let result = observe_outcome(accessor, &mut handle, &id, operation, split).await?;
            Ok(handle
                .complete_access(
                    accessor,
                    accessor.getter(),
                    respond(result.map_err(Into::into)),
                )
                .await?)
        }
    }
}

/// Starts the durable call of a handle operation under the calling agent's authority, or claims
/// its recorded `Start`.
async fn start_handle_call<U, Ctx, Pair>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    id: &OwnedInvocationId,
    function_type: DurableFunctionType,
) -> anyhow::Result<DurableCallSession<Pair, Cancellable>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<Req = HostRequestGolemRpcInvocationHandle>,
{
    Ok(
        DurableCallSession::<Pair, Cancellable>::start_access_with_agent_authority(
            accessor,
            accessor.getter(),
            function_type,
            HostRequestGolemRpcInvocationHandle {
                invocation_id: id.clone(),
            },
        )
        .await?,
    )
}

/// Asks the target about the invocation and returns the result to record. An interrupt and an
/// operational failure trap the call.
async fn observe_outcome<U, Ctx, Pair, T>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: &mut DurableCallSession<Pair, Cancellable>,
    id: &OwnedInvocationId,
    operation: InvocationHandleOperation,
    split: fn(Observed) -> Result<Result<T, InternalRpcError>, WorkerExecutorError>,
) -> anyhow::Result<Result<T, InternalRpcError>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    Pair: HostPayloadPair,
{
    match observe(accessor, id, operation, handle.agent_auth_ctx().clone()).await {
        Ok(observed) => split(observed).map_err(|error| handle.trap(error)),
        Err(interrupt) => Err(handle.trap(interrupt)),
    }
}

pub(super) fn wire_invocation_id(id: &OwnedInvocationId) -> core_wire::InvocationId {
    core_wire::InvocationId {
        owner: match id.owner() {
            golem_common::model::InvocationOwner::Agent(agent) => {
                core_wire::InvocationOwner::Agent((*agent).into())
            }
            golem_common::model::InvocationOwner::Tool(tool) => {
                core_wire::InvocationOwner::Tool(tool.to_string())
            }
        },
        id: id.uuid().into(),
    }
}

/// Reserves the import's invocation id and reads the handle's target before the first `await`.
fn handle_target<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    self_: &Resource<InvocationEntry>,
) -> anyhow::Result<OwnedInvocationId> {
    reserve_invocation_id_before_await(accessor, accessor.getter())?;
    accessor.with(|mut access| Ok(access.get().table().get(self_)?.id.clone()))
}

/// Asks the target about the invocation as the calling agent. The wait is an RPC wait, so a
/// long `await` lets the agent suspend; an interrupt or a suspension ends it with its kind.
async fn observe<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    id: &OwnedInvocationId,
    operation: InvocationHandleOperation,
    auth: AuthCtx,
) -> Result<Observed, golem_service_base::error::worker_executor::InterruptKind> {
    let (rpc, caller) = with_import_access(accessor, |mut access| {
        let ctx = access.get();
        (ctx.rpc(), ctx.outgoing_caller().immediate)
    });
    suspendable(accessor, async move {
        rpc.invocation_handle(id, &caller, operation, &auth).await
    })
    .await
}

/// The recorded refusal of a handle operation, or `None` when the error is operational and the
/// call must trap instead of recording it.
fn recorded_refusal(error: &WorkerExecutorError) -> Option<InternalRpcError> {
    match error {
        WorkerExecutorError::PermissionDenied { .. }
        | WorkerExecutorError::CannotCancelRunningInvocation
        | WorkerExecutorError::InvalidRequest { .. } => Some(error.clone().into()),
        _ => None,
    }
}

/// Splits the target's answer into a recorded result (the inner `Result`) or a trap (the outer
/// `Err`).
fn outcome(
    observed: Observed,
) -> Result<Result<invocation_handle_success::Result, InternalRpcError>, WorkerExecutorError> {
    match observed {
        Ok(success) => match success.result {
            Some(invocation_handle_success::Result::NotFound(_)) => {
                Ok(Err(InternalRpcError::NotFound {
                    details: "the invocation or its agent does not exist".to_string(),
                }))
            }
            Some(invocation_handle_success::Result::InvocationFailure(failure)) => {
                let error = WorkerExecutorError::try_from(failure).map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "invocation handle returned an undecodable failure: {error}"
                    ))
                })?;
                Ok(Err(error.into()))
            }
            Some(result) => Ok(Ok(result)),
            None => Err(WorkerExecutorError::runtime(
                "invocation handle returned no result",
            )),
        },
        Err(error) => recorded_refusal(&error).map(Err).ok_or(error),
    }
}

fn mismatched(operation: &str) -> WorkerExecutorError {
    WorkerExecutorError::runtime(format!(
        "invocation handle returned a result of another operation than {operation}"
    ))
}

fn status_outcome(
    observed: Observed,
) -> Result<Result<SerializableInvocationStatus, InternalRpcError>, WorkerExecutorError> {
    match outcome(observed)? {
        Ok(invocation_handle_success::Result::Status(state)) => Ok(Ok(lifecycle_status(state)?)),
        Ok(_) => Err(mismatched("status")),
        Err(error) => Ok(Err(error)),
    }
}

fn await_outcome(
    observed: Observed,
) -> Result<Result<SchemaValue, InternalRpcError>, WorkerExecutorError> {
    match outcome(observed)? {
        Ok(invocation_handle_success::Result::Output(output)) => awaited_output(output),
        Ok(_) => Err(mismatched("await")),
        Err(error) => Ok(Err(error)),
    }
}

/// The method result of an awaited invocation. A handle awaits only an agent method's result
/// without streams.
fn awaited_output(
    output: InvocationSessionResult,
) -> Result<Result<SchemaValue, InternalRpcError>, WorkerExecutorError> {
    if !output.new_stream_mappings.is_empty() {
        return Ok(Err(InternalRpcError::ProtocolError {
            details: "an invocation handle cannot await a streaming result".to_string(),
        }));
    }
    match output.result {
        Some(invocation_session_result::Result::MethodResult(value)) => {
            Ok(Ok(SchemaValue::try_from(value).map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "invocation handle returned an undecodable result: {error}"
                ))
            })?))
        }
        Some(invocation_session_result::Result::NoResult(_)) => Ok(Ok(SchemaValue::Tuple {
            elements: Vec::new(),
        })),
        Some(invocation_session_result::Result::ToolResult(_)) => {
            Ok(Err(InternalRpcError::ProtocolError {
                details: "the invocation is not an agent method invocation".to_string(),
            }))
        }
        None => Err(WorkerExecutorError::runtime(
            "invocation handle returned an output without a result",
        )),
    }
}

fn cancel_outcome(
    observed: Observed,
) -> Result<Result<bool, InternalRpcError>, WorkerExecutorError> {
    match outcome(observed)? {
        Ok(invocation_handle_success::Result::Cancelled(cancelled)) => Ok(Ok(cancelled)),
        Ok(_) => Err(mismatched("cancel")),
        Err(error) => Ok(Err(error)),
    }
}

fn lifecycle_status(state: i32) -> Result<SerializableInvocationStatus, WorkerExecutorError> {
    let status = match InvocationLifecycleState::try_from(state) {
        Ok(InvocationLifecycleState::Pending) => Some(SerializableInvocationStatus::Pending),
        Ok(InvocationLifecycleState::Scheduled) => Some(SerializableInvocationStatus::Scheduled),
        Ok(InvocationLifecycleState::Running) => Some(SerializableInvocationStatus::Running),
        Ok(InvocationLifecycleState::Waiting) => Some(SerializableInvocationStatus::Waiting),
        Ok(InvocationLifecycleState::Retrying) => Some(SerializableInvocationStatus::Retrying),
        Ok(InvocationLifecycleState::Interrupted) => {
            Some(SerializableInvocationStatus::Interrupted)
        }
        Ok(InvocationLifecycleState::Completed) => Some(SerializableInvocationStatus::Completed),
        Ok(InvocationLifecycleState::Failed) => Some(SerializableInvocationStatus::Failed),
        Ok(InvocationLifecycleState::Cancelled) => Some(SerializableInvocationStatus::Cancelled),
        Ok(InvocationLifecycleState::BackgroundFailed) => {
            Some(SerializableInvocationStatus::BackgroundFailed)
        }
        Ok(InvocationLifecycleState::Unspecified) | Err(_) => None,
    };
    status.ok_or_else(|| {
        WorkerExecutorError::runtime(format!(
            "invocation handle returned the invalid invocation state {state}"
        ))
    })
}

fn status_to_wire(
    result: Result<SerializableInvocationStatus, SerializableRpcError>,
) -> Result<InvocationStatus, RpcError> {
    rpc_result_to_wire(result).map(|status| match status {
        SerializableInvocationStatus::Pending => InvocationStatus::Pending,
        SerializableInvocationStatus::Scheduled => InvocationStatus::Scheduled,
        SerializableInvocationStatus::Running => InvocationStatus::Running,
        SerializableInvocationStatus::Waiting => InvocationStatus::Waiting,
        SerializableInvocationStatus::Retrying => InvocationStatus::Retrying,
        SerializableInvocationStatus::Interrupted => InvocationStatus::Interrupted,
        SerializableInvocationStatus::Completed => InvocationStatus::Completed,
        SerializableInvocationStatus::Failed => InvocationStatus::Failed,
        SerializableInvocationStatus::Cancelled => InvocationStatus::Cancelled,
        SerializableInvocationStatus::BackgroundFailed => InvocationStatus::BackgroundFailed,
    })
}

fn rpc_result_to_wire<T>(result: Result<T, SerializableRpcError>) -> Result<T, RpcError> {
    result.map_err(|error| InternalRpcError::from(error).into())
}

#[cfg(test)]
mod tests;
