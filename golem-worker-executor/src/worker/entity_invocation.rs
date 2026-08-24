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

use super::entity_slot::EntitySlot;
use super::instance::InstanceHost;
use super::owner_lane::{EntityCallMode, OwnerInvocationId, OwnerLane, OwnerLaneError};
use crate::workerctx::WorkerCtx;
use golem_common::model::entity::{EntityInvocationScope, InvocationExecutionMode};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::task::AbortHandle;
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, info_span};
use wasmtime::Store;
use wasmtime::component::Instance;

struct EntityInvocationMetricsGuard {
    entity_kind: &'static str,
    execution_mode: &'static str,
    outcome: &'static str,
}

impl EntityInvocationMetricsGuard {
    fn new(scope: &EntityInvocationScope) -> Self {
        let entity_kind = scope.invocation_id().entity().kind_label();
        let execution_mode = match scope.mode() {
            InvocationExecutionMode::Live => "live",
            InvocationExecutionMode::ReplayingCompleted => "replaying_completed",
            InvocationExecutionMode::ReplayingIncomplete => "replaying_incomplete",
        };
        crate::metrics::workers::inc_entity_invocation_active(entity_kind, execution_mode);
        Self {
            entity_kind,
            execution_mode,
            outcome: "cancelled",
        }
    }

    fn finish<R>(&mut self, result: &Result<R, WorkerExecutorError>) {
        self.outcome = if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        };
    }
}

impl Drop for EntityInvocationMetricsGuard {
    fn drop(&mut self) {
        crate::metrics::workers::dec_entity_invocation_active(
            self.entity_kind,
            self.execution_mode,
        );
        crate::metrics::workers::record_entity_invocation(
            self.entity_kind,
            self.execution_mode,
            self.outcome,
        );
    }
}

/// Caller-facing handle for one already-initiated entity body.
///
/// Dropping the handle detaches rather than aborts the body. This is required for fire-and-forget
/// calls and for completion-discarded asynchronous futures.
pub struct EntityInvocationHandle<R> {
    invocation: OwnerInvocationId,
    mode: EntityCallMode,
    lane: OwnerLane,
    task: Option<JoinHandle<Result<R, WorkerExecutorError>>>,
}

impl<R: Send + 'static> EntityInvocationHandle<R> {
    pub fn invocation(&self) -> &OwnerInvocationId {
        &self.invocation
    }

    pub fn mode(&self) -> EntityCallMode {
        self.mode
    }

    /// Returns a task abort capability used by durable cancellation and owner lifecycle fencing.
    /// Aborting drops the transient Store; it never creates independently recoverable entity
    /// state.
    pub fn abort_handle(&self) -> AbortHandle {
        self.task
            .as_ref()
            .expect("entity invocation handle has not been joined")
            .abort_handle()
    }

    /// Waits for a result and makes a deferred capable body causally eligible at this await point.
    pub async fn await_result(self, caller: &OwnerInvocationId) -> Result<R, WorkerExecutorError> {
        let lane_wait = if self.mode != EntityCallMode::Synchronous {
            match self
                .lane
                .await_invocations(caller, [self.invocation.clone()])
            {
                Ok(wait) => Some(wait),
                Err(OwnerLaneError::InactiveInvocation(invocation))
                    if invocation == self.invocation =>
                {
                    // The body completed off-lane before the caller observed its result.
                    None
                }
                Err(error) => return Err(WorkerExecutorError::runtime(error.to_string())),
            }
        } else {
            None
        };
        let result = self.join().await;
        if let Some(lane_wait) = lane_wait {
            lane_wait.wait().await;
        }
        result
    }

    /// Joins after eligibility was established by a batched poll through [`OwnerLane`].
    pub async fn join(mut self) -> Result<R, WorkerExecutorError> {
        let task = self
            .task
            .take()
            .expect("entity invocation handle can only be joined once");
        task.await.map_err(|error| {
            WorkerExecutorError::runtime(if error.is_panic() {
                "Entity body task panicked".to_string()
            } else {
                format!("Entity body task was cancelled: {error}")
            })
        })?
    }
}

pub(crate) fn start_entity_invocation<Ctx, R, F, Finalize, Finalized>(
    host: InstanceHost<Ctx>,
    slot: Arc<EntitySlot>,
    lane: OwnerLane,
    parent: OwnerInvocationId,
    scope: EntityInvocationScope,
    mode: EntityCallMode,
    invoke: F,
    finalize: Finalize,
) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
where
    Ctx: WorkerCtx,
    R: Send + 'static,
    F: Send + 'static,
    F: for<'a> FnOnce(
        &'a Instance,
        &'a mut Store<Ctx>,
    )
        -> Pin<Box<dyn Future<Output = Result<R, WorkerExecutorError>> + Send + 'a>>,
    Finalize: FnOnce(Result<R, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<R, WorkerExecutorError>> + Send + 'static,
{
    let registration = slot.register(&scope)?;
    let invocation_id = scope.invocation_id().clone();
    let invocation = OwnerInvocationId::Entity(invocation_id.clone());
    let ticket = lane
        .register_entity(
            parent,
            scope.invocation_id().clone(),
            mode,
            scope.activation().filesystem(),
        )
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let entity = scope.invocation_id().entity();
    let executable = scope.activation().executable();
    let span = info_span!(
        "entity_invocation",
        owner_environment_id = %scope.owner_id().environment_id,
        owner_agent_id = %scope.owner_id().agent_id,
        entity_kind = entity.kind_label(),
        entity_name = entity.name(),
        invocation_start_index = scope.invocation_id().start_index().as_u64(),
        executable_component_id = %executable.component_id,
        executable_component_revision = executable.component_revision.get(),
        activation_fingerprint = %scope.activation().fingerprint(),
        execution_mode = ?scope.mode(),
    );
    let task = tokio::spawn(
        async move {
            let mut metrics = EntityInvocationMetricsGuard::new(&scope);
            debug!("Entity invocation started");
            let result = if start_rx.await.is_err() {
                finalize(Err(WorkerExecutorError::runtime(
                    "Entity invocation was fenced before its body started",
                )))
                .await
            } else {
                match ticket.acquire().await {
                    Ok(permit) => {
                        let result = match host.instantiate_entity_scoped(&scope).await {
                            Ok(hosted) => {
                                hosted
                                    .invoke_scoped_registered(scope, &registration, invoke)
                                    .await
                            }
                            Err(error) => Err(error),
                        };
                        let finalized = finalize(result).await;
                        drop(registration);
                        permit.complete_and_wait().await;
                        finalized
                    }
                    Err(error) => {
                        finalize(Err(WorkerExecutorError::runtime(error.to_string()))).await
                    }
                }
            };
            metrics.finish(&result);
            debug!(succeeded = result.is_ok(), "Entity invocation finished");
            result
        }
        .instrument(span),
    );
    let abort = task.abort_handle();
    if let Err(error) = slot.attach_abort(&invocation_id, abort.clone()) {
        abort.abort();
        return Err(error);
    }
    if start_tx.send(()).is_err() {
        abort.abort();
        return Err(WorkerExecutorError::runtime(
            "Entity invocation was fenced before its body started",
        ));
    }

    Ok(EntityInvocationHandle {
        invocation,
        mode,
        lane,
        task: Some(task),
    })
}
