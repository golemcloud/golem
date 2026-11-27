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

//! The local answer to an invocation handle operation: authority over the invocation is
//! established before anything about its owner or the invocation is answered.

use super::*;
use crate::worker::CancelInvocationOutcome;
use crate::worker::invocations::{InvocationState, ObservedInvocationResult};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationLifecycleState, invocation_handle_success,
};
use golem_common::model::card::AgentInvocationIdPattern;
use golem_common::model::{AgentInvocationOutput, InvocationId};

/// What an authorized caller reaches: the invocation's owner, or no owner.
enum HandleAccess<Ctx: WorkerCtx> {
    Owner(Arc<Worker<Ctx>>),
    NotFound,
}

impl<Ctx: WorkerCtx> DirectWorkerInvocationRpc<Ctx> {
    /// Answers a handle operation on an invocation whose owner this executor hosts.
    pub(super) async fn local_invocation_handle(
        &self,
        target: &OwnedAgentId,
        invocation_id: &golem_common::model::OwnedInvocationId,
        caller: &Principal,
        operation: InvocationHandleOperation,
        auth: &AuthCtx,
    ) -> Result<InvocationHandleSuccess, WorkerExecutorError> {
        let key = invocation_id.id();
        let resolved_target = ResolvedTarget::existing(target.clone());
        let management_authorization = self.authorize(
            auth.actor_account_id(),
            &resolved_target,
            if operation == InvocationHandleOperation::Cancel {
                AgentVerb::CancelInvocation
            } else {
                AgentVerb::View
            },
            AgentResourcePattern::InvocationId(AgentInvocationIdPattern::Uuid(key.0)),
            auth,
        );
        match self
            .authorized_access(target, &key, caller, management_authorization)
            .await?
        {
            HandleAccess::Owner(worker) => {
                let result = answer(&worker, target, key, operation).await;
                answer_after_deletion(&worker, result).await
            }
            HandleAccess::NotFound => Ok(not_found()),
        }
    }

    /// Loads the invocation's owner and establishes the caller's authority over the invocation:
    /// the caller started it, or holds management authority over the owner. Only a manager sees
    /// an operational failure to read the owner. Another caller learns only that authority could
    /// not be established yet, so an outage neither discloses its cause nor denies the starter
    /// for good.
    async fn authorized_access<A>(
        &self,
        target: &OwnedAgentId,
        key: &InvocationId,
        caller: &Principal,
        management_authorization: impl Future<Output = Result<A, RpcError>>,
    ) -> Result<HandleAccess<Ctx>, WorkerExecutorError> {
        match Worker::get_exact_existing_suspended(self, target).await {
            Ok(worker) => {
                prove_starter(&worker, key, caller, management_authorization).await?;
                addresses_agent_invocations(&worker)?;
                Ok(HandleAccess::Owner(worker))
            }
            Err(WorkerExecutorError::AgentNotFound { .. }) => managed(management_authorization)
                .await
                .map(|()| HandleAccess::NotFound)
                .map_err(|error| match error {
                    RpcError::Denied { details } => WorkerExecutorError::permission_denied(details),
                    _ => unestablished(),
                }),
            Err(error) => Err(match managed(management_authorization).await {
                Ok(()) => error,
                Err(_) => unestablished(),
            }),
        }
    }
}

/// Management authority over the owner, which also covers an owner that the call does not find.
async fn managed<A>(
    management_authorization: impl Future<Output = Result<A, RpcError>>,
) -> Result<(), RpcError> {
    match management_authorization.await {
        Ok(_) | Err(RpcError::NotFound { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

/// The caller's authority over an invocation of a loaded owner. A starter check that fails falls
/// back to management authorization, which a manager passes whatever the check answered. Without
/// it, the caller is denied when the owner has no starter evidence for the caller, or a deletion
/// has removed it; a check that failed on a live owner is not yet a denial.
async fn prove_starter<Ctx: WorkerCtx, A>(
    worker: &Arc<Worker<Ctx>>,
    key: &InvocationId,
    caller: &Principal,
    management_authorization: impl Future<Output = Result<A, RpcError>>,
) -> Result<(), WorkerExecutorError> {
    match worker.invocation_was_started_by(key, caller).await {
        Ok(true) => Ok(()),
        proof => match managed(management_authorization).await {
            Ok(()) => Ok(()),
            Err(RpcError::Denied { details }) => Err(match proof {
                Err(_) if !worker.answers_as_deleted().await => unestablished(),
                _ => WorkerExecutorError::permission_denied(details),
            }),
            Err(_) => Err(unestablished()),
        },
    }
}

/// A tool or oplog-processor owner has an agent-shaped address, so only the loaded owner shows
/// that the ID does not name an agent invocation. The refusal waits for authority, as not-found
/// does.
fn addresses_agent_invocations<Ctx: WorkerCtx>(
    worker: &Arc<Worker<Ctx>>,
) -> Result<(), WorkerExecutorError> {
    if worker.get_initial_worker_metadata().owner_kind
        == golem_common::model::agent::OwnerKind::ComponentAgent
    {
        Ok(())
    } else {
        Err(WorkerExecutorError::invalid_request(
            "invocation handles address agent invocations",
        ))
    }
}

fn unestablished() -> WorkerExecutorError {
    WorkerExecutorError::runtime(
        "authority over the invocation could not be established; try again",
    )
}

fn not_found() -> InvocationHandleSuccess {
    InvocationHandleSuccess {
        result: Some(not_found_result()),
    }
}

fn not_found_result() -> invocation_handle_success::Result {
    invocation_handle_success::Result::NotFound(golem_schema::proto::golem::common::Empty {})
}

/// Answers the operation for an authorized caller.
async fn answer<Ctx: WorkerCtx>(
    worker: &Arc<Worker<Ctx>>,
    target: &OwnedAgentId,
    key: InvocationId,
    operation: InvocationHandleOperation,
) -> Result<InvocationHandleSuccess, WorkerExecutorError> {
    let result = match worker.get_invocation(&key).await? {
        None => not_found_result(),
        Some(entry) => match operation {
            InvocationHandleOperation::Status => invocation_handle_success::Result::Status(
                InvocationLifecycleState::from(entry.state) as i32,
            ),
            InvocationHandleOperation::Await => awaited(worker, target, key).await?,
            InvocationHandleOperation::Cancel => cancelled(worker, key, entry.state).await?,
            InvocationHandleOperation::Unspecified => unreachable!(),
        },
    };
    Ok(InvocationHandleSuccess {
        result: Some(result),
    })
}

/// Waits for the invocation's result.
async fn awaited<Ctx: WorkerCtx>(
    worker: &Arc<Worker<Ctx>>,
    target: &OwnedAgentId,
    key: InvocationId,
) -> Result<invocation_handle_success::Result, WorkerExecutorError> {
    let base = golem_api_grpc::proto::golem::worker::InvocationSessionResult {
        agent_id: Some(target.agent_id.into()),
        idempotency_key: Some(key.into()),
        agent_fingerprint: Some(worker.get_initial_worker_metadata().fingerprint.0.into()),
        ..Default::default()
    };
    Ok(match worker.observe_invocation_result(&key).await? {
        Err(error) => invocation_handle_success::Result::InvocationFailure(error.into()),
        Ok(ObservedInvocationResult::NotFound) => not_found_result(),
        Ok(ObservedInvocationResult::Invocation(output)) => {
            invocation_handle_success::Result::Output(method_output(base, output)?)
        }
        Ok(ObservedInvocationResult::Session {
            value,
            component_revision,
        }) => invocation_handle_success::Result::Output(
            golem_api_grpc::proto::golem::worker::InvocationSessionResult {
                new_stream_mappings: value.proto_mappings(),
                result: Some(invocation_session_result::Result::MethodResult(value.value)),
                component_revision: Some(component_revision.get()),
                ..base
            },
        ),
    })
}

/// The recorded output of an agent method invocation.
fn method_output(
    base: golem_api_grpc::proto::golem::worker::InvocationSessionResult,
    output: AgentInvocationOutput,
) -> Result<golem_api_grpc::proto::golem::worker::InvocationSessionResult, WorkerExecutorError> {
    match output.result {
        AgentInvocationResult::AgentMethod { output: value } => Ok(
            golem_api_grpc::proto::golem::worker::InvocationSessionResult {
                result: Some(invocation_session_result::Result::MethodResult(
                    value.try_into().map_err(WorkerExecutorError::runtime)?,
                )),
                component_revision: output.component_revision.map(|revision| revision.get()),
                fuel_consumed: output.consumed_fuel,
                status: output.invocation_status.map(|status| {
                    golem_api_grpc::proto::golem::worker::InvocationStatus::from(status) as i32
                }),
                oplog_index: output.oplog_index.map(u64::from),
                background_failure: output
                    .background_failure
                    .map(|failure| WorkerExecutorError::from(failure).into()),
                ..base
            },
        ),
        _ => Err(WorkerExecutorError::invalid_request(
            "invocation handle does not name an agent method",
        )),
    }
}

/// Withdraws an invocation that has not started. A finished invocation is not cancelled, and a
/// running one refuses the cancellation.
async fn cancelled<Ctx: WorkerCtx>(
    worker: &Arc<Worker<Ctx>>,
    key: InvocationId,
    state: InvocationState,
) -> Result<invocation_handle_success::Result, WorkerExecutorError> {
    let cancelled = match state {
        InvocationState::Completed
        | InvocationState::Failed
        | InvocationState::Cancelled
        | InvocationState::BackgroundFailed => Ok(false),
        _ => match worker.cancel_invocation(key).await? {
            CancelInvocationOutcome::Withdrawn => Ok(true),
            CancelInvocationOutcome::NotPending => Ok(false),
            CancelInvocationOutcome::Running => {
                Err(WorkerExecutorError::CannotCancelRunningInvocation)
            }
        },
    };
    cancelled.map(invocation_handle_success::Result::Cancelled)
}

/// The answer once the owner's deletion is accounted for. The authorized caller's invocation
/// ended with its owner's deletion, which an await that followed the owner to a later generation
/// reports as the owner not being found. A failed deletion keeps the agent until a retry or an
/// eviction replaces its generation, so its refusal is not the invocation's answer: the caller
/// tries again.
async fn answer_after_deletion<Ctx: WorkerCtx>(
    worker: &Arc<Worker<Ctx>>,
    result: Result<InvocationHandleSuccess, WorkerExecutorError>,
) -> Result<InvocationHandleSuccess, WorkerExecutorError> {
    if matches!(result, Err(WorkerExecutorError::AgentNotFound { .. }))
        || worker.answers_as_deleted().await
    {
        Ok(not_found())
    } else {
        match result {
            Err(WorkerExecutorError::InvalidRequest { .. })
                if worker.deletion_owns_retirement().await =>
            {
                Err(WorkerExecutorError::runtime(
                    "the agent's failed deletion blocks the invocation handle; try again",
                ))
            }
            result => result,
        }
    }
}
