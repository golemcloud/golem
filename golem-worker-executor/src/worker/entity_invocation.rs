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
use super::entity_slot::EntitySlotRegistration;
use super::instance::{ClosureEntityInvocationBody, EntityInvocationBody, InstanceHost};
use super::owner_lane::{
    EntityCallMode, OwnerInvocationId, OwnerInvocationPermit, OwnerInvocationTicket, OwnerLane,
    OwnerLaneError, OwnerLaneWait,
};
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
    executor_tasks: crate::services::active_agents::InvocationLoops,
    lane_await_required: bool,
    task: Option<JoinHandle<Option<EntityInvocationCompletion<R>>>>,
}

pub(crate) struct EntityInvocationCompletion<R> {
    result: Result<R, WorkerExecutorError>,
    resources: EntityInvocationResources,
}

pub(crate) struct EntityInvocationResources {
    retained: Option<RetainedEntityControl>,
    permit: Option<OwnerInvocationPermit>,
    lane_wait: Option<OwnerLaneWait>,
}

enum RetainedEntityCommand {
    Prepare(tokio::sync::oneshot::Sender<Result<(), WorkerExecutorError>>),
    Settle(tokio::sync::oneshot::Sender<Result<(), WorkerExecutorError>>),
}

struct RetainedEntityControl {
    commands: tokio::sync::mpsc::UnboundedSender<(
        super::suspension::ExternalActivity,
        RetainedEntityCommand,
    )>,
    disposal: tokio_util::sync::CancellationToken,
    task: Option<JoinHandle<Option<()>>>,
    suspension: Arc<super::suspension::OwnerSuspension>,
}

impl Drop for RetainedEntityControl {
    fn drop(&mut self) {
        self.disposal.cancel();
    }
}

// Field order keeps the registration alive throughout Store/context destruction, including
// abandonment of an unpolled task during executor shutdown.
struct RetainedEntityOwner {
    hosted: Option<Box<dyn RetainedEntityStore>>,
    _registration: EntitySlotRegistration,
}

impl RetainedEntityControl {
    fn spawn(
        hosted: Box<dyn RetainedEntityStore>,
        registration: EntitySlotRegistration,
        suspension: Arc<super::suspension::OwnerSuspension>,
        executor_tasks: &crate::services::active_agents::InvocationLoops,
    ) -> Self {
        let cancellation = registration.cancellation();
        let owner = RetainedEntityOwner {
            hosted: Some(hosted),
            _registration: registration,
        };
        let disposal = tokio_util::sync::CancellationToken::new();
        let dispose = disposal.clone();
        let (commands, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let task = executor_tasks.spawn_entity(async move {
            let mut owner = owner;
            // The callback future is nested inside the owner's lifetime. Cancelling this work
            // drops a consuming settlement future before releasing the registration.
            let work = async {
                while let Some((_activity, command)) = receive.recv().await {
                    match command {
                        RetainedEntityCommand::Prepare(reply) => {
                            let result = match owner.hosted.as_mut() {
                                Some(hosted) => hosted.prepare_parent_end().await,
                                None => Ok(()),
                            };
                            if reply.send(result).is_err() {
                                break;
                            }
                        }
                        RetainedEntityCommand::Settle(reply) => {
                            let result = match owner.hosted.take() {
                                Some(hosted) => hosted.settle().await,
                                None => Ok(()),
                            };
                            let _ = reply.send(result);
                            break;
                        }
                    }
                }
            };
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {}
                _ = dispose.cancelled() => {}
                _ = work => {}
            }
            drop(owner);
        });
        Self {
            commands,
            disposal,
            task: Some(task),
            suspension,
        }
    }

    async fn join(&mut self) -> Result<(), WorkerExecutorError> {
        if let Some(task) = self.task.take() {
            task.await.map_err(|error| {
                WorkerExecutorError::runtime(format!("Retained entity Store task failed: {error}"))
            })?;
        }
        Ok(())
    }
}

pub(crate) trait RetainedEntityStore: Send {
    fn prepare_parent_end(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send + '_>>;

    fn settle(
        self: Box<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send>>;
}

type EntityInvocationFuture<'a, R> = Pin<
    Box<
        dyn Future<
                Output = (
                    Result<R, WorkerExecutorError>,
                    Option<Box<dyn RetainedEntityStore>>,
                ),
            > + Send
            + 'a,
    >,
>;

pub(crate) trait EntityInvocationRunner<R>: Send + 'static {
    fn run<'a>(
        self,
        scope: EntityInvocationScope,
        registration: &'a EntitySlotRegistration,
        abort: tokio_util::sync::CancellationToken,
    ) -> EntityInvocationFuture<'a, R>;
}

struct ComponentEntityRunner<Ctx: WorkerCtx, F> {
    host: InstanceHost<Ctx>,
    body: F,
}

impl<Ctx, R, F> EntityInvocationRunner<R> for ComponentEntityRunner<Ctx, F>
where
    Ctx: WorkerCtx,
    R: Send + 'static,
    F: EntityInvocationBody<Ctx, R>,
{
    fn run<'a>(
        self,
        scope: EntityInvocationScope,
        registration: &'a EntitySlotRegistration,
        _abort: tokio_util::sync::CancellationToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = (
                        Result<R, WorkerExecutorError>,
                        Option<Box<dyn RetainedEntityStore>>,
                    ),
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let invocation = OwnerInvocationId::Entity(scope.invocation_id().clone());
            tokio::select! {
                result = async move {
                    match self.host.instantiate_entity_scoped(&scope).await {
                        Ok(hosted_instance) => {
                            let (result, retained) = hosted_instance
                                .invoke_scoped_registered_retained(scope, registration, self.body)
                                .await;
                            (
                                result,
                                Some(Box::new(RetainedHostedInstance {
                                    hosted: retained,
                                    invocation,
                                }) as Box<dyn RetainedEntityStore>),
                            )
                        }
                        Err(error) => (Err(error), None),
                    }
                } => result,
                _ = _abort.cancelled() => (
                    Err(WorkerExecutorError::runtime("Entity body was cancelled")),
                    None,
                ),
            }
        })
    }
}

struct ClosureEntityRunner<F>(F);

impl<R, F> EntityInvocationRunner<R> for ClosureEntityRunner<F>
where
    R: Send + 'static,
    F: Send + 'static,
    F: for<'a> FnOnce(
        EntityInvocationScope,
        &'a EntitySlotRegistration,
        tokio_util::sync::CancellationToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = (
                        Result<R, WorkerExecutorError>,
                        Option<Box<dyn RetainedEntityStore>>,
                    ),
                > + Send
                + 'a,
        >,
    >,
{
    fn run<'a>(
        self,
        scope: EntityInvocationScope,
        registration: &'a EntitySlotRegistration,
        abort: tokio_util::sync::CancellationToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = (
                        Result<R, WorkerExecutorError>,
                        Option<Box<dyn RetainedEntityStore>>,
                    ),
                > + Send
                + 'a,
        >,
    > {
        self.0(scope, registration, abort)
    }
}

struct RetainedHostedInstance<Ctx: WorkerCtx> {
    hosted: super::instance::HostedInstance<Ctx>,
    invocation: OwnerInvocationId,
}

pub(crate) struct RetainedNativeContext<Ctx: WorkerCtx> {
    context: Ctx,
    execution: Arc<super::instance::OwnerExecution>,
    invocation: OwnerInvocationId,
    parent_end_prepared: bool,
}

impl<Ctx: WorkerCtx> RetainedNativeContext<Ctx> {
    pub(crate) fn new(
        context: Ctx,
        execution: Arc<super::instance::OwnerExecution>,
        invocation: OwnerInvocationId,
    ) -> Self {
        Self {
            context,
            execution,
            invocation,
            parent_end_prepared: false,
        }
    }

    pub(crate) fn context_mut(&mut self) -> &mut Ctx {
        &mut self.context
    }

    async fn prepare_native_parent_end(&mut self) -> Result<(), WorkerExecutorError> {
        if self.parent_end_prepared {
            return Ok(());
        }
        crate::durable_host::tool::prepare_tool_parent_end_owned(
            &self.execution,
            self.invocation.clone(),
        )
        .await?;

        // Native handlers use the context's host resource table directly. Destroy every
        // body-owned resource while the context and its drop-event receiver are still alive, then
        // record the resulting durable cancellations before removing the entity scope.
        *self.context.durable_ctx_mut().table() = wasmtime::component::ResourceTable::new();
        crate::durable_host::drain_queued_dropped_call_events(self.context.durable_ctx_mut())
            .await
            .map_err(|error| error.source)?;
        self.context.set_entity_invocation_scope(None)?;
        self.parent_end_prepared = true;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> RetainedEntityStore for RetainedNativeContext<Ctx> {
    fn prepare_parent_end(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send + '_>> {
        Box::pin(self.prepare_native_parent_end())
    }

    fn settle(
        self: Box<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send>> {
        Box::pin(async move {
            crate::durable_host::tool::settle_tool_children_owned(&self.execution, self.invocation)
                .await
        })
    }
}

impl<Ctx: WorkerCtx> RetainedEntityStore for RetainedHostedInstance<Ctx> {
    fn prepare_parent_end(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send + '_>> {
        Box::pin(async {
            self.hosted
                .prepare_tool_parent_end(self.invocation.clone())
                .await
        })
    }

    fn settle(
        self: Box<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send>> {
        Box::pin(async move {
            self.hosted.settle_tool_children(self.invocation).await?;
            Ok(())
        })
    }
}

impl<R> EntityInvocationCompletion<R> {
    pub(crate) fn into_parts(self) -> (Result<R, WorkerExecutorError>, EntityInvocationResources) {
        (self.result, self.resources)
    }
}

impl EntityInvocationResources {
    fn from_finished_body(
        hosted: Option<Box<dyn RetainedEntityStore>>,
        registration: EntitySlotRegistration,
        permit: Option<OwnerInvocationPermit>,
        suspension: Arc<super::suspension::OwnerSuspension>,
        executor_tasks: crate::services::active_agents::InvocationLoops,
    ) -> Self {
        Self {
            retained: hosted.map(|hosted| {
                RetainedEntityControl::spawn(hosted, registration, suspension, &executor_tasks)
            }),
            permit,
            lane_wait: None,
        }
    }

    pub(crate) async fn prepare_parent_end(&mut self) -> Result<(), WorkerExecutorError> {
        let Some(retained) = self.retained.as_mut() else {
            return Ok(());
        };
        let (reply, result) = tokio::sync::oneshot::channel();
        retained
            .commands
            .send((
                retained.suspension.register_external(),
                RetainedEntityCommand::Prepare(reply),
            ))
            .map_err(|_| {
                WorkerExecutorError::runtime("Retained entity Store preparation was abandoned")
            })?;
        result.await.map_err(|_| {
            WorkerExecutorError::runtime("Retained entity Store preparation was abandoned")
        })?
    }

    pub(crate) async fn release_for_owner_failure(&mut self) -> Result<(), WorkerExecutorError> {
        if let Some(permit) = self.permit.take() {
            permit.complete();
        }
        if let Some(mut retained) = self.retained.take() {
            retained.disposal.cancel();
            retained.join().await?;
        }
        Ok(())
    }

    pub(crate) async fn settle_after_parent_end(mut self) -> Result<(), WorkerExecutorError> {
        if let Some(permit) = self.permit.take() {
            permit.complete();
        }
        let settlement = match self.retained.as_mut() {
            Some(retained) => {
                let (reply, result) = tokio::sync::oneshot::channel();
                let settlement = if retained
                    .commands
                    .send((
                        retained.suspension.register_external(),
                        RetainedEntityCommand::Settle(reply),
                    ))
                    .is_ok()
                {
                    result.await.map_err(|_| {
                        WorkerExecutorError::runtime(
                            "Retained entity Store settlement was abandoned",
                        )
                    })?
                } else {
                    Err(WorkerExecutorError::runtime(
                        "Retained entity Store settlement was abandoned",
                    ))
                };
                retained.join().await?;
                settlement
            }
            None => Ok(()),
        };
        if let Some(lane_wait) = self.lane_wait.take() {
            lane_wait.wait().await;
        }
        settlement?;
        Ok(())
    }

    pub(crate) async fn release(mut self) -> Result<(), WorkerExecutorError> {
        self.prepare_parent_end().await?;
        self.settle_after_parent_end().await
    }
}

impl<R: Send + 'static> EntityInvocationHandle<R> {
    pub(crate) fn executor_tasks(&self) -> crate::services::active_agents::InvocationLoops {
        self.executor_tasks.clone()
    }

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
        let completion = self.await_completion(caller).await?;
        let (result, resources) = completion.into_parts();
        resources.release().await?;
        result
    }

    pub(crate) async fn await_completion(
        self,
        caller: &OwnerInvocationId,
    ) -> Result<EntityInvocationCompletion<R>, WorkerExecutorError> {
        let lane_wait = if self.mode != EntityCallMode::Synchronous && self.lane_await_required {
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
        let mut completion = self.join_completion().await?;
        completion.resources.lane_wait = lane_wait;
        Ok(completion)
    }

    /// Joins after eligibility was established by a batched poll through [`OwnerLane`].
    pub async fn join(self) -> Result<R, WorkerExecutorError> {
        let completion = self.join_completion().await?;
        let (result, resources) = completion.into_parts();
        resources.release().await?;
        result
    }

    pub(crate) async fn join_completion(
        mut self,
    ) -> Result<EntityInvocationCompletion<R>, WorkerExecutorError> {
        let task = self
            .task
            .take()
            .expect("entity invocation handle can only be joined once");
        task.await
            .map_err(|error| {
                WorkerExecutorError::runtime(if error.is_panic() {
                    "Entity body task panicked".to_string()
                } else {
                    format!("Entity body task was cancelled: {error}")
                })
            })?
            .ok_or_else(|| {
                WorkerExecutorError::runtime("Entity body was abandoned during executor shutdown")
            })
    }
}

pub(crate) fn start_entity_invocation<Ctx, R, F, Finalize, Finalized>(
    host: InstanceHost<Ctx>,
    slot: Arc<EntitySlot>,
    lane: OwnerLane,
    suspension: Arc<super::suspension::OwnerSuspension>,
    executor_tasks: crate::services::active_agents::InvocationLoops,
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
    let ticket = lane
        .register_entity(
            parent,
            scope.invocation_id().clone(),
            mode,
            scope.activation().filesystem(),
        )
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let run = ComponentEntityRunner {
        host,
        body: ClosureEntityInvocationBody(invoke),
    };
    start_entity_invocation_inner(
        slot,
        lane,
        suspension,
        executor_tasks,
        scope,
        mode,
        Some(ticket),
        run,
        finalize,
    )
}

pub(crate) fn start_registered_entity_invocation<Ctx, R, F, Finalize, Finalized>(
    host: InstanceHost<Ctx>,
    slot: Arc<EntitySlot>,
    lane: OwnerLane,
    suspension: Arc<super::suspension::OwnerSuspension>,
    executor_tasks: crate::services::active_agents::InvocationLoops,
    scope: EntityInvocationScope,
    mode: EntityCallMode,
    ticket: OwnerInvocationTicket,
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
    let run = ComponentEntityRunner {
        host,
        body: ClosureEntityInvocationBody(invoke),
    };
    let mut handle = start_entity_invocation_inner(
        slot,
        lane,
        suspension,
        executor_tasks,
        scope,
        mode,
        Some(ticket),
        run,
        finalize,
    )?;
    handle.lane_await_required = false;
    Ok(handle)
}

/// Starts a body whose caller already owns the registered and granted lane node. This is used by
/// staged tool operations, which keep the permit under operation/owner terminal arbitration until
/// durable completion rather than transferring it into the sidecar task.
pub(crate) fn start_pre_acquired_entity_invocation<Ctx, R, F, Finalize, Finalized>(
    host: InstanceHost<Ctx>,
    slot: Arc<EntitySlot>,
    lane: OwnerLane,
    suspension: Arc<super::suspension::OwnerSuspension>,
    executor_tasks: crate::services::active_agents::InvocationLoops,
    scope: EntityInvocationScope,
    mode: EntityCallMode,
    invoke: F,
    finalize: Finalize,
) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
where
    Ctx: WorkerCtx,
    R: Send + 'static,
    F: EntityInvocationBody<Ctx, R>,
    Finalize: FnOnce(Result<R, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<R, WorkerExecutorError>> + Send + 'static,
{
    let run = ComponentEntityRunner { host, body: invoke };
    start_entity_invocation_inner(
        slot,
        lane,
        suspension,
        executor_tasks,
        scope,
        mode,
        None,
        run,
        finalize,
    )
}

pub(crate) fn start_native_entity_invocation<R, Run, Finalize, Finalized>(
    slot: Arc<EntitySlot>,
    lane: OwnerLane,
    suspension: Arc<super::suspension::OwnerSuspension>,
    executor_tasks: crate::services::active_agents::InvocationLoops,
    parent: Option<OwnerInvocationId>,
    scope: EntityInvocationScope,
    mode: EntityCallMode,
    run: Run,
    finalize: Finalize,
) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
where
    R: Send + 'static,
    Run: Send + 'static,
    Run: for<'a> FnOnce(
        EntityInvocationScope,
        &'a EntitySlotRegistration,
        tokio_util::sync::CancellationToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = (
                        Result<R, WorkerExecutorError>,
                        Option<Box<dyn RetainedEntityStore>>,
                    ),
                > + Send
                + 'a,
        >,
    >,
    Finalize: FnOnce(Result<R, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<R, WorkerExecutorError>> + Send + 'static,
{
    let ticket = parent
        .map(|parent| {
            lane.register_entity(
                parent,
                scope.invocation_id().clone(),
                mode,
                scope.activation().filesystem(),
            )
        })
        .transpose()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    start_entity_invocation_inner(
        slot,
        lane,
        suspension,
        executor_tasks,
        scope,
        mode,
        ticket,
        ClosureEntityRunner(run),
        finalize,
    )
}

pub(crate) fn start_registered_native_entity_invocation<R, Run, Finalize, Finalized>(
    slot: Arc<EntitySlot>,
    lane: OwnerLane,
    suspension: Arc<super::suspension::OwnerSuspension>,
    executor_tasks: crate::services::active_agents::InvocationLoops,
    scope: EntityInvocationScope,
    mode: EntityCallMode,
    ticket: OwnerInvocationTicket,
    run: Run,
    finalize: Finalize,
) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
where
    R: Send + 'static,
    Run: Send + 'static,
    Run: for<'a> FnOnce(
        EntityInvocationScope,
        &'a EntitySlotRegistration,
        tokio_util::sync::CancellationToken,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = (
                        Result<R, WorkerExecutorError>,
                        Option<Box<dyn RetainedEntityStore>>,
                    ),
                > + Send
                + 'a,
        >,
    >,
    Finalize: FnOnce(Result<R, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<R, WorkerExecutorError>> + Send + 'static,
{
    let mut handle = start_entity_invocation_inner(
        slot,
        lane,
        suspension,
        executor_tasks,
        scope,
        mode,
        Some(ticket),
        ClosureEntityRunner(run),
        finalize,
    )?;
    handle.lane_await_required = false;
    Ok(handle)
}

fn start_entity_invocation_inner<R, Run, Finalize, Finalized>(
    slot: Arc<EntitySlot>,
    lane: OwnerLane,
    suspension: Arc<super::suspension::OwnerSuspension>,
    executor_tasks: crate::services::active_agents::InvocationLoops,
    scope: EntityInvocationScope,
    mode: EntityCallMode,
    ticket: Option<OwnerInvocationTicket>,
    run: Run,
    finalize: Finalize,
) -> Result<EntityInvocationHandle<R>, WorkerExecutorError>
where
    R: Send + 'static,
    Run: EntityInvocationRunner<R>,
    Finalize: FnOnce(Result<R, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<R, WorkerExecutorError>> + Send + 'static,
{
    let cancellation = tokio_util::sync::CancellationToken::new();
    let registration = slot.register(&scope, cancellation.clone())?;
    let participant = suspension.register_entity(scope.invocation_id().clone());
    let invocation_id = scope.invocation_id().clone();
    let invocation = OwnerInvocationId::Entity(invocation_id.clone());
    let lane_await_required = ticket.is_some();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let entity = scope.invocation_id().entity();
    let executable = scope.activation().executable_opt();
    let span = info_span!(
        "entity_invocation",
        owner_environment_id = %scope.owner_id().environment_id,
        owner_agent_id = %scope.owner_id().agent_id,
        entity_kind = entity.kind_label(),
        entity_name = entity.name(),
        invocation_start_index = scope.invocation_id().start_index().as_u64(),
        executable_component_id = tracing::field::display(executable.map(|target| target.component_id.to_string()).unwrap_or_else(|| "native".to_string())),
        executable_component_revision = executable.map(|target| target.component_revision.get()),
        activation_fingerprint = %scope.activation().fingerprint(),
        execution_mode = ?scope.mode(),
    );
    let task_executor = executor_tasks.clone();
    let task = executor_tasks.spawn_entity(
        async move {
            let _participant = participant;
            let mut metrics = EntityInvocationMetricsGuard::new(&scope);
            debug!("Entity invocation started");
            let mut permit = None;
            let mut hosted = None;
            let result = if start_rx.await.is_err() {
                Err(WorkerExecutorError::runtime(
                    "Entity invocation was fenced before its body started",
                ))
            } else {
                permit = match ticket {
                    Some(ticket) => match ticket.acquire().await {
                        Ok(permit) => Some(permit),
                        Err(error) => {
                            let result =
                                finalize(Err(WorkerExecutorError::runtime(error.to_string())))
                                    .await;
                            metrics.finish(&result);
                            return EntityInvocationCompletion {
                                result,
                                resources: EntityInvocationResources::from_finished_body(
                                    hosted,
                                    registration,
                                    permit,
                                    suspension.clone(),
                                    task_executor,
                                ),
                            };
                        }
                    },
                    None => None,
                };
                let (result, retained) = run.run(scope, &registration, cancellation).await;
                hosted = retained;
                result
            };
            let result = finalize(result).await;
            metrics.finish(&result);
            debug!(succeeded = result.is_ok(), "Entity invocation finished");
            EntityInvocationCompletion {
                result,
                resources: EntityInvocationResources::from_finished_body(
                    hosted,
                    registration,
                    permit,
                    suspension,
                    task_executor,
                ),
            }
        }
        .instrument(span),
    );
    let task_abort = task.abort_handle();
    if start_tx.send(()).is_err() {
        task_abort.abort();
        return Err(WorkerExecutorError::runtime(
            "Entity invocation was fenced before its body started",
        ));
    }

    Ok(EntityInvocationHandle {
        invocation,
        mode,
        lane,
        executor_tasks,
        lane_await_required,
        task: Some(task),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::OplogIndex;
    use std::sync::Mutex;
    use test_r::test;

    fn completed_handle(
        lane: OwnerLane,
        lane_await_required: bool,
    ) -> EntityInvocationHandle<&'static str> {
        let executor_tasks = crate::services::active_agents::InvocationLoops::new(
            tokio_util::sync::CancellationToken::new(),
        );
        let task = executor_tasks.spawn_entity(async move {
            EntityInvocationCompletion {
                result: Ok("completed"),
                resources: EntityInvocationResources {
                    retained: None,
                    permit: None,
                    lane_wait: None,
                },
            }
        });
        EntityInvocationHandle {
            invocation: OwnerInvocationId::Agent(OplogIndex::from_u64(2)),
            mode: EntityCallMode::Asynchronous,
            lane,
            executor_tasks,
            lane_await_required,
            task: Some(task),
        }
    }

    #[test]
    async fn pre_acquired_body_does_not_repeat_lane_await_after_parent_end() {
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "owner".to_string(),
            },
        ));
        let inactive_parent = OwnerInvocationId::Agent(OplogIndex::from_u64(1));

        let completion = completed_handle(lane.clone(), false)
            .await_completion(&inactive_parent)
            .await
            .expect("pre-acquired body only joins its completion");
        assert_eq!(completion.result.unwrap(), "completed");

        let error = match completed_handle(lane, true)
            .await_completion(&inactive_parent)
            .await
        {
            Ok(_) => panic!("ordinary asynchronous body must require an active caller"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("is not active"));
    }

    struct RecordingStore {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl RetainedEntityStore for RecordingStore {
        fn prepare_parent_end(
            &mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send + '_>> {
            Box::pin(async {
                self.events.lock().unwrap().push("prepare");
                Ok(())
            })
        }

        fn settle(
            self: Box<Self>,
        ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send>> {
            Box::pin(async move {
                self.events.lock().unwrap().push("settle");
                Ok(())
            })
        }
    }

    fn recording_resources(
        events: Option<Arc<Mutex<Vec<&'static str>>>>,
        suspension: Arc<super::super::suspension::OwnerSuspension>,
        executor: crate::services::active_agents::InvocationLoops,
    ) -> EntityInvocationResources {
        use golem_common::model::entity::{
            EntityActivation, EntityActivationPolicy, EntityInvocationId, ExecutableTarget,
            FilesystemCapability, OwnedAgentEntityId,
        };
        use golem_common::model::tool::{SecretKeyScope, ToolFilesystemAccess};
        let owner = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "owner".into(),
            },
        );
        let activation = Arc::new(
            EntityActivation::new(
                ExecutableTarget::new(ComponentId::new(), 1_u64.try_into().unwrap()),
                1_u64.try_into().unwrap(),
                EntityActivationPolicy::ToolMiddleware {
                    middleware_name: "retained".try_into().unwrap(),
                    provision: Default::default(),
                    config_keys_readable: Default::default(),
                    secret_keys_readable: SecretKeyScope::All,
                    secret_keys_revealable: SecretKeyScope::All,
                    filesystem_access: ToolFilesystemAccess::Unset,
                },
                FilesystemCapability::Incapable,
            )
            .unwrap(),
        );
        let entity = OwnedAgentEntityId {
            owner: owner.clone(),
            entity: activation.entity(),
        };
        let slot = Arc::new(EntitySlot::new(entity.clone()));
        let scope = EntityInvocationScope::new(
            EntityInvocationId::new(entity, OplogIndex::from_u64(2)).unwrap(),
            OplogIndex::from_u64(1),
            activation,
            golem_common::model::agent::Principal::Agent(
                golem_common::model::agent::AgentPrincipal {
                    agent_id: owner.agent_id,
                },
            ),
            InvocationExecutionMode::Live,
            golem_common::model::IdempotencyKey::fresh(),
            false,
            false,
            golem_common::model::IdempotencyKey::fresh(),
        )
        .unwrap();
        let registration = slot.register(&scope, Default::default()).unwrap();
        EntityInvocationResources::from_finished_body(
            events
                .map(|events| Box::new(RecordingStore { events }) as Box<dyn RetainedEntityStore>),
            registration,
            None,
            suspension,
            executor,
        )
    }

    #[test]
    async fn retained_store_prepares_before_terminal_commit_and_settlement() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let executor_tasks = crate::services::active_agents::InvocationLoops::new(
            tokio_util::sync::CancellationToken::new(),
        );
        let mut resources = recording_resources(
            Some(events.clone()),
            super::super::suspension::OwnerSuspension::new(),
            executor_tasks,
        );

        resources.prepare_parent_end().await.unwrap();
        events.lock().unwrap().push("terminal");
        resources.settle_after_parent_end().await.unwrap();

        assert_eq!(*events.lock().unwrap(), ["prepare", "terminal", "settle"]);
    }

    #[test]
    async fn retained_store_callbacks_are_not_polled_after_executor_shutdown() {
        use super::super::suspension::tests::{blocked_timer, eligible_now};
        use std::time::{Duration, Instant};

        let events = Arc::new(Mutex::new(Vec::new()));
        let shutdown = tokio_util::sync::CancellationToken::new();
        let executor_tasks = crate::services::active_agents::InvocationLoops::new(shutdown.clone());
        let suspension = super::super::suspension::OwnerSuspension::new();
        let (_store, _, _timer) =
            blocked_timer(&suspension, Instant::now() + Duration::from_secs(30));
        assert!(eligible_now(&suspension));
        shutdown.cancel();
        executor_tasks.wait_for_exit().await.unwrap();

        let mut preparation = recording_resources(
            Some(events.clone()),
            suspension.clone(),
            executor_tasks.clone(),
        );
        assert!(preparation.prepare_parent_end().await.is_err());
        assert!(eligible_now(&suspension));

        let settlement =
            recording_resources(Some(events.clone()), suspension.clone(), executor_tasks);
        assert!(settlement.settle_after_parent_end().await.is_err());
        assert!(eligible_now(&suspension));
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    async fn absent_retained_store_release_remains_a_noop_during_shutdown() {
        let executor = crate::services::active_agents::InvocationLoops::new(Default::default());
        executor.wait_for_exit().await.unwrap();
        let resources = recording_resources(
            None,
            super::super::suspension::OwnerSuspension::new(),
            executor,
        );
        resources.release().await.unwrap();
    }
}
