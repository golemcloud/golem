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
use tokio::task::JoinHandle;
use tracing::{Instrument, debug, info_span};
use wasmtime::Store;
use wasmtime::component::Instance;

/// Once a guest result is observed, local abort must not discard its settlement or cleanup.
#[derive(Clone, Default)]
pub(crate) struct EntityBodyFinalization(Arc<std::sync::atomic::AtomicBool>);

impl EntityBodyFinalization {
    pub(crate) fn begin(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) async fn abortable(&self, abort: &tokio_util::sync::CancellationToken) {
        abort.cancelled().await;
        if self.0.load(std::sync::atomic::Ordering::Acquire) {
            std::future::pending::<()>().await;
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum EntityBodyOutcome<R> {
    Completed(R),
    Cancelled,
}

impl<R> EntityBodyOutcome<R> {
    pub(crate) fn completed(self) -> Result<R, WorkerExecutorError> {
        match self {
            Self::Completed(value) => Ok(value),
            Self::Cancelled => Err(WorkerExecutorError::runtime("Entity body was cancelled")),
        }
    }
}

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

    fn finish<R>(&mut self, result: &Result<EntityBodyOutcome<R>, WorkerExecutorError>) {
        self.outcome = match result {
            Ok(EntityBodyOutcome::Completed(_)) => "succeeded",
            Ok(EntityBodyOutcome::Cancelled) => "cancelled",
            Err(_) => "failed",
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
    lane_await_required: bool,
    abort: tokio_util::sync::CancellationToken,
    task: Option<JoinHandle<Option<EntityInvocationCompletion<R>>>>,
}

/// Aborts the body without tearing down its filesystem drain or completion resources.
pub struct EntityInvocationAbortHandle(tokio_util::sync::CancellationToken);

impl EntityInvocationAbortHandle {
    pub fn abort(&self) {
        self.0.cancel();
    }
}

pub(crate) struct EntityInvocationCompletion<R> {
    result: Result<EntityBodyOutcome<R>, WorkerExecutorError>,
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
    deferred_permit: Arc<std::sync::Mutex<Option<OwnerInvocationPermit>>>,
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
    _deferred_permit: Arc<std::sync::Mutex<Option<OwnerInvocationPermit>>>,
}

impl RetainedEntityControl {
    fn spawn(
        hosted: Box<dyn RetainedEntityStore>,
        registration: EntitySlotRegistration,
        suspension: Arc<super::suspension::OwnerSuspension>,
        executor_tasks: &crate::services::active_agents::InvocationLoops,
    ) -> Self {
        let cancellation = registration.cancellation();
        let filesystem = hosted.filesystem_invocation();
        let deferred_permit = Arc::new(std::sync::Mutex::new(None));
        let owner = RetainedEntityOwner {
            hosted: Some(hosted),
            _registration: registration,
            _deferred_permit: deferred_permit.clone(),
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
            drop(owner.hosted.take());
            if let Some(filesystem) = filesystem {
                filesystem.close_and_drain().await;
            }
            owner._deferred_permit.lock().unwrap().take();
            drop(owner);
        });
        Self {
            commands,
            disposal,
            task: Some(task),
            suspension,
            deferred_permit,
        }
    }

    async fn join(&mut self) -> Result<(), WorkerExecutorError> {
        if let Some(task) = self.task.as_mut() {
            let result = task.await.map_err(|error| {
                WorkerExecutorError::runtime(format!("Retained entity Store task failed: {error}"))
            });
            self.task.take();
            result?;
        }
        Ok(())
    }
}

pub(crate) trait RetainedEntityStore: Send {
    fn filesystem_invocation(
        &self,
    ) -> Option<crate::services::agent_filesystem::FilesystemInvocation>;

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
                    Result<EntityBodyOutcome<R>, WorkerExecutorError>,
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

pub(crate) async fn await_component_entity_body<R>(
    abort: tokio_util::sync::CancellationToken,
    lifecycle: Option<tokio_util::sync::CancellationToken>,
    filesystem: Option<crate::services::agent_filesystem::FilesystemInvocation>,
    finalization: EntityBodyFinalization,
    body: impl Future<
        Output = (
            Result<EntityBodyOutcome<R>, WorkerExecutorError>,
            Option<Box<dyn RetainedEntityStore>>,
        ),
    >,
) -> (
    Result<EntityBodyOutcome<R>, WorkerExecutorError>,
    Option<Box<dyn RetainedEntityStore>>,
) {
    let result = {
        tokio::select! {
            biased;
            result = body => result,
            _ = async {
                match lifecycle {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => (Err(WorkerExecutorError::Interrupted {
                kind: golem_service_base::error::worker_executor::InterruptKind::Interrupt(
                    golem_common::model::Timestamp::now_utc(),
                ),
            }), None),
            _ = finalization.abortable(&abort) => (
                Ok(EntityBodyOutcome::Cancelled),
                None,
            ),
        }
    };
    // The body (and its Store) is gone before closing admission. A retained Store owns its
    // own drain; an interrupted body has no retained owner to join abandoned filesystem calls.
    if result.1.is_none()
        && let Some(filesystem) = filesystem
    {
        filesystem.close_and_drain().await;
    }
    result
}

struct ComponentEntityRunner<Ctx: WorkerCtx, F> {
    host: InstanceHost<Ctx>,
    body: F,
}

pub(crate) fn is_trapped_tool_response<R: 'static>(response: &R) -> bool {
    use golem_common::model::oplog::HostResponseEntityInvocation;
    use golem_common::model::oplog::payload::types::{
        SerializableEntityBodyExecution, SerializableToolOperationTerminal,
    };
    use golem_common::schema::FromSchema;

    (response as &dyn std::any::Any)
        .downcast_ref::<HostResponseEntityInvocation>()
        .and_then(|response| response.result.as_ref().ok())
        .and_then(|value| SerializableToolOperationTerminal::from_value(value.value()).ok())
        .is_some_and(|terminal| terminal.body_execution == SerializableEntityBodyExecution::Trapped)
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
                        Result<EntityBodyOutcome<R>, WorkerExecutorError>,
                        Option<Box<dyn RetainedEntityStore>>,
                    ),
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let invocation = OwnerInvocationId::Entity(scope.invocation_id().clone());
            let execution = self.host.owner_execution();
            let (filesystem, drain) = match self.host.create_entity_filesystem() {
                Ok(filesystem) => filesystem,
                Err(error) => return (Err(error), None),
            };
            let finalization = EntityBodyFinalization::default();
            let body_finalization = finalization.clone();
            await_component_entity_body(_abort, Some(registration.cancellation()), Some(drain), finalization, async move {
                match self
                    .host
                    .instantiate_entity_with_scope(Some(&scope), filesystem)
                    .await
                {
                    Ok(mut hosted_instance) => {
                        hosted_instance.set_entity_body_finalization(body_finalization.clone());
                        let (result, retained) = hosted_instance
                            .invoke_scoped_registered_retained(scope, registration, self.body)
                            .await;
                        body_finalization.begin();
                        if matches!(&result, Ok(EntityBodyOutcome::Cancelled))
                            || matches!(&result, Ok(EntityBodyOutcome::Completed(response)) if is_trapped_tool_response(response)) {
                            let mut retained = RetainedDisposedContext {
                                context: retained.into_cleanup_context(),
                                execution,
                                invocation,
                                prepared: false,
                            };
                            // Cleanup host calls belong to the reconstructed body too. Join them
                            // before validating its terminal or releasing its reconstruction fence.
                            let result = match retained.prepare_parent_end().await {
                                Ok(()) => result,
                                Err(error) => Err(error),
                            };
                            return (
                                result,
                                Some(Box::new(retained) as Box<dyn RetainedEntityStore>),
                            );
                        }
                        (
                            result,
                            Some(Box::new(RetainedHostedInstance {
                                hosted: retained,
                                invocation,
                            })
                                as Box<dyn RetainedEntityStore>),
                        )
                    }
                    Err(error) => (Err(error), None),
                }
            })
            .await
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
                        Result<EntityBodyOutcome<R>, WorkerExecutorError>,
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
                        Result<EntityBodyOutcome<R>, WorkerExecutorError>,
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

struct RetainedDisposedContext<Ctx: WorkerCtx> {
    context: super::ObservedPrimaryStore<Ctx>,
    execution: Arc<super::instance::OwnerExecution>,
    invocation: OwnerInvocationId,
    prepared: bool,
}

async fn retained_cleanup_failure<Ctx: WorkerCtx>(
    operation: Option<crate::durable_host::tool::operation::OwnerToolOperation>,
    error: anyhow::Error,
    retry_from: golem_common::model::oplog::OplogIndex,
    agent_mode: golem_common::model::agent::AgentMode,
) -> WorkerExecutorError {
    if let Some(operation) = operation {
        let trap = crate::model::TrapType::from_error::<Ctx>(&error, retry_from, agent_mode);
        // Elect before converting to the transport error, which cannot carry host retry markers.
        // Trap selection also withdraws this operation's uncommitted cancellation.
        operation.select_trap(trap).await;
    }
    WorkerExecutorError::from(error)
}

impl<Ctx: WorkerCtx> RetainedEntityStore for RetainedDisposedContext<Ctx> {
    fn filesystem_invocation(
        &self,
    ) -> Option<crate::services::agent_filesystem::FilesystemInvocation> {
        self.context
            .durable_ctx()
            .filesystem_generation_handle()
            .invocation()
    }

    fn prepare_parent_end(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), WorkerExecutorError>> + Send + '_>> {
        Box::pin(async move {
            if self.prepared {
                return Ok(());
            }
            crate::durable_host::tool::prepare_tool_parent_end_owned(
                &self.execution,
                self.invocation.clone(),
            )
            .await?;
            if let Err(error) = crate::durable_host::rdbms::cleanup_trapped_transactions(
                self.context.durable_ctx_mut(),
            )
            .await
            {
                use crate::workerctx::InvocationHooks;
                let ctx = self.context.durable_ctx();
                return Err(retained_cleanup_failure::<Ctx>(
                    ctx.entity_tool_operation(),
                    error,
                    ctx.get_current_retry_point().await,
                    ctx.agent_mode(),
                )
                .await);
            }
            self.context
                .durable_ctx_mut()
                .dispose_trapped_guest_resources();
            if let Some(filesystem) = self.filesystem_invocation() {
                filesystem.close_and_drain().await;
            }
            crate::durable_host::tool::durability::finish_producer_disposals(
                self.context.durable_ctx_mut(),
            )
            .await?;
            self.context
                .durable_ctx_mut()
                .set_invocation_principal(None);
            self.context.set_entity_invocation_scope(None)?;
            self.prepared = true;
            Ok(())
        })
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

pub(crate) struct RetainedNativeContext<Ctx: WorkerCtx> {
    // Revoke task admission and transfer child joins before destroying the context.
    reconstruction_tasks: super::tasks::NativeReconstructionTasks,
    context: crate::native_tool::NativeToolContext<Ctx>,
    _usage: crate::services::linear_memory::GuestResourceUsageGuard,
    execution: Arc<super::instance::OwnerExecution>,
    invocation: OwnerInvocationId,
    parent_end_prepared: bool,
}

impl<Ctx: WorkerCtx> RetainedNativeContext<Ctx> {
    pub(crate) fn new(
        context: Ctx,
        registration: EntitySlotRegistration,
        execution: Arc<super::instance::OwnerExecution>,
        invocation: OwnerInvocationId,
        reconstruction_tasks: super::tasks::NativeReconstructionTasks,
    ) -> Self {
        let usage = context.durable_ctx().begin_guest_resource_usage();
        context.durable_ctx().activate_live_resource_usage();
        Self {
            reconstruction_tasks,
            context: crate::native_tool::NativeToolContext::new(context, registration),
            _usage: usage,
            execution,
            invocation,
            parent_end_prepared: false,
        }
    }

    pub(crate) fn context_mut(&mut self) -> &mut crate::native_tool::NativeToolContext<Ctx> {
        &mut self.context
    }

    async fn prepare_native_parent_end(&mut self) -> Result<(), WorkerExecutorError> {
        if self.parent_end_prepared {
            return Ok(());
        }
        let monitor = self.reconstruction_tasks.monitor();
        {
            tokio::select! {
                biased;
                error = monitor.wait_for_failure() => return Err(error),
                error = monitor.wait_for_body_failure() => return Err(error),
                result = crate::durable_host::tool::prepare_tool_parent_end_owned(
                    &self.execution,
                    self.invocation.clone(),
                ) => result?,
            }
        }

        tokio::select! {
            biased;
            result = self.reconstruction_tasks.seal_and_drain() => result?,
            error = monitor.wait_for_body_failure() => return Err(error),
        }
        if let Some(error) = monitor.take_body_failure() {
            return Err(error);
        }

        // Native handlers use the context's host resource table directly. Destroy every
        // body-owned resource while the context and its drop-event receiver are still alive, then
        // record the resulting durable cancellations before removing the entity scope.
        self.context
            .shared
            .with_ctx(|ctx| *ctx.table() = wasmtime::component::ResourceTable::new());
        crate::durable_host::tool::durability::finish_producer_disposals(&mut self.context.shared)
            .await?;
        self.context.shared.with_worker_ctx(|ctx| {
            ctx.durable_ctx_mut().set_invocation_principal(None);
            ctx.set_entity_invocation_scope(None)
        })?;
        self.parent_end_prepared = true;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> RetainedEntityStore for RetainedNativeContext<Ctx> {
    fn filesystem_invocation(
        &self,
    ) -> Option<crate::services::agent_filesystem::FilesystemInvocation> {
        self.context
            .shared
            .with_ctx(|ctx| ctx.filesystem_generation_handle().invocation())
    }

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
    fn filesystem_invocation(
        &self,
    ) -> Option<crate::services::agent_filesystem::FilesystemInvocation> {
        self.hosted.filesystem_invocation()
    }

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
    pub(crate) fn into_parts(
        self,
    ) -> (
        Result<EntityBodyOutcome<R>, WorkerExecutorError>,
        EntityInvocationResources,
    ) {
        (self.result, self.resources)
    }
}

impl Drop for EntityInvocationResources {
    fn drop(&mut self) {
        if let Some(retained) = self.retained.as_ref()
            && let Some(permit) = self.permit.take()
        {
            *retained.deferred_permit.lock().unwrap() = Some(permit);
        }
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
        if let Some(retained) = self.retained.as_mut() {
            retained.disposal.cancel();
            retained.join().await?;
        }
        self.retained.take();
        if let Some(permit) = self.permit.take() {
            permit.complete();
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
    pub fn invocation(&self) -> &OwnerInvocationId {
        &self.invocation
    }

    pub fn mode(&self) -> EntityCallMode {
        self.mode
    }

    /// Requests body disposal while keeping the invocation task alive to join its filesystem
    /// work. Executor shutdown still abandons the complete task for ordinary reconstruction.
    pub fn abort_handle(&self) -> EntityInvocationAbortHandle {
        EntityInvocationAbortHandle(self.abort.clone())
    }

    pub fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits for a result and makes a deferred capable body causally eligible at this await point.
    pub async fn await_result(self, caller: &OwnerInvocationId) -> Result<R, WorkerExecutorError> {
        let completion = self.await_completion(caller).await?;
        let (result, resources) = completion.into_parts();
        resources.release().await?;
        result?.completed()
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
        result?.completed()
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
    Finalize:
        FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
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
    F: EntityInvocationBody<Ctx, R>,
    Finalize:
        FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
{
    let run = ComponentEntityRunner { host, body: invoke };
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
    Finalize:
        FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
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
                        Result<EntityBodyOutcome<R>, WorkerExecutorError>,
                        Option<Box<dyn RetainedEntityStore>>,
                    ),
                > + Send
                + 'a,
        >,
    >,
    Finalize:
        FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
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
                        Result<EntityBodyOutcome<R>, WorkerExecutorError>,
                        Option<Box<dyn RetainedEntityStore>>,
                    ),
                > + Send
                + 'a,
        >,
    >,
    Finalize:
        FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
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
    Finalize:
        FnOnce(Result<EntityBodyOutcome<R>, WorkerExecutorError>) -> Finalized + Send + 'static,
    Finalized: Future<Output = Result<EntityBodyOutcome<R>, WorkerExecutorError>> + Send + 'static,
{
    let lifecycle_cancellation = tokio_util::sync::CancellationToken::new();
    let registration = slot.register(&scope, lifecycle_cancellation.clone())?;
    let cancellation = tokio_util::sync::CancellationToken::new();
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
    let abort = cancellation.clone();
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
                    Some(ticket) => match tokio::select! {
                        biased;
                        _ = lifecycle_cancellation.cancelled() => Err(WorkerExecutorError::Interrupted {
                            kind: golem_service_base::error::worker_executor::InterruptKind::Interrupt(
                                golem_common::model::Timestamp::now_utc(),
                            ),
                        }),
                        _ = cancellation.cancelled() => {
                            let result = finalize(Ok(EntityBodyOutcome::Cancelled)).await;
                            metrics.finish(&result);
                            return EntityInvocationCompletion {
                                result,
                                resources: EntityInvocationResources::from_finished_body(
                                    hosted, registration, permit, suspension.clone(), task_executor,
                                ),
                            };
                        },
                        acquired = ticket.acquire() => acquired.map_err(|error| {
                            WorkerExecutorError::runtime(error.to_string())
                        }),
                    } {
                        Ok(permit) => Some(permit),
                        Err(error) => {
                            let result = finalize(Err(error)).await;
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
        lane_await_required,
        abort,
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

    fn cleanup_operation() -> (
        Arc<crate::durable_host::tool::operation::OwnerToolOperations>,
        crate::durable_host::tool::operation::OwnerToolOperation,
    ) {
        use crate::durable_host::tool::operation::{OwnerToolOperations, tests::replay_operation};
        let template = replay_operation(42);
        let owner = OwnerToolOperations::new();
        let operation = owner
            .create(template.context().clone())
            .accept(template.invocation_id().unwrap())
            .unwrap();
        (owner, operation)
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn retained_cleanup_failure_preserves_retry_and_exhaustion_through_election() {
        use crate::durable_host::durability::{
            ClassifiedHostError, DurableCallTrapContext, HostFailureKind,
            SemanticTrapRetryOverride, SemanticTrapRetryOverrideMarker, SemanticTrapRetryVerdict,
            mark_durable_call_trap_context,
        };
        use crate::durable_host::tool::operation::OwnerFailureWinner;
        use crate::model::TrapType;
        use golem_common::model::RetryPolicyState;
        use golem_common::model::agent::AgentMode;
        use golem_common::model::oplog::AgentError;

        for cancelling in [false, true] {
            for exhausted in [false, true] {
                let (owner, operation) = cleanup_operation();
                if cancelling {
                    assert!(operation.begin_cancel());
                }
                let payload = SemanticTrapRetryOverride {
                    retry_from: OplogIndex::from_u64(17),
                    policy_name: "transaction-status".to_string(),
                    verdict: SemanticTrapRetryVerdict::Retry(std::time::Duration::from_millis(
                        1873,
                    )),
                    retry_policy_state: RetryPolicyState::Counter(4),
                };
                let error = anyhow::Error::new(ClassifiedHostError {
                    kind: if exhausted {
                        HostFailureKind::Permanent
                    } else {
                        HostFailureKind::Transient
                    },
                    message: "transaction status resolution".to_string(),
                });
                let error = if exhausted {
                    mark_durable_call_trap_context(
                        error,
                        DurableCallTrapContext {
                            retry_from: OplogIndex::from_u64(17),
                        },
                    )
                } else {
                    anyhow::Error::new(SemanticTrapRetryOverrideMarker {
                        payload: payload.clone(),
                        inner: error,
                    })
                };
                let transport = retained_cleanup_failure::<crate::workerctx::default::Context>(
                    Some(operation),
                    error,
                    OplogIndex::from_u64(9),
                    AgentMode::Durable,
                )
                .await;
                assert!(
                    transport
                        .to_string()
                        .contains("transaction status resolution")
                );
                let Some(OwnerFailureWinner::Trap(TrapType::Error {
                    error,
                    retry_from,
                    semantic_trap_retry_override,
                })) = owner.selected_owner_failure()
                else {
                    panic!("cleanup must elect its typed trap before transport conversion");
                };
                if exhausted {
                    assert!(matches!(error, AgentError::PermanentError(_)));
                    assert_eq!(retry_from, OplogIndex::from_u64(17));
                    assert!(semantic_trap_retry_override.is_none());
                } else {
                    assert!(matches!(error, AgentError::TransientError(_)));
                    assert_eq!(retry_from, payload.retry_from);
                    let selected = semantic_trap_retry_override.unwrap();
                    assert_eq!(selected.policy_name, payload.policy_name);
                    assert_eq!(selected.verdict, payload.verdict);
                    assert_eq!(selected.retry_policy_state, payload.retry_policy_state);
                }
            }
        }
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn retained_cleanup_failure_preserves_lifecycle_and_existing_winner() {
        use crate::durable_host::tool::operation::OwnerFailureWinner;
        use crate::model::TrapType;
        use golem_common::model::agent::AgentMode;
        use golem_service_base::error::worker_executor::InterruptKind;

        for kind in [InterruptKind::Jump, InterruptKind::ShardLost] {
            let (owner, operation) = cleanup_operation();
            assert!(operation.begin_cancel());
            retained_cleanup_failure::<crate::workerctx::default::Context>(
                Some(operation.clone()),
                anyhow::Error::new(kind),
                OplogIndex::INITIAL,
                AgentMode::Durable,
            )
            .await;
            assert!(matches!(owner.selected_owner_failure(),
                Some(OwnerFailureWinner::Trap(TrapType::Interrupt(selected))) if selected == kind));
            retained_cleanup_failure::<crate::workerctx::default::Context>(
                Some(operation),
                anyhow::anyhow!("later cleanup failure"),
                OplogIndex::INITIAL,
                AgentMode::Durable,
            )
            .await;
            assert!(matches!(owner.selected_owner_failure(),
                Some(OwnerFailureWinner::Trap(TrapType::Interrupt(selected))) if selected == kind));
        }
        let (owner, operation) = cleanup_operation();
        owner
            .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
            .await;
        retained_cleanup_failure::<crate::workerctx::default::Context>(
            Some(operation),
            anyhow::anyhow!("cleanup failure"),
            OplogIndex::INITIAL,
            AgentMode::Durable,
        )
        .await;
        assert!(matches!(
            owner.selected_owner_failure(),
            Some(OwnerFailureWinner::Lifecycle(InterruptKind::Jump))
        ));

        let (owner, operation) = cleanup_operation();
        assert!(operation.begin_cancel());
        operation.resolve_cancel(true).await;
        retained_cleanup_failure::<crate::workerctx::default::Context>(
            Some(operation.clone()),
            anyhow::anyhow!("cleanup after cancellation"),
            OplogIndex::INITIAL,
            AgentMode::Durable,
        )
        .await;
        assert!(owner.selected_owner_failure().is_none());
        assert!(matches!(
            operation.winner_if_active(),
            Some(crate::durable_host::tool::operation::ToolOperationWinner::Cancelled)
        ));

        let (owner, operation) = cleanup_operation();
        let fence = crate::services::oplog::OplogFence {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: "cleanup-fenced".to_string(),
            },
            expected_epoch: golem_common::model::ShardEpoch(7),
            actual_epoch: Some(golem_common::model::ShardEpoch(8)),
        };
        retained_cleanup_failure::<crate::workerctx::default::Context>(
            Some(operation),
            anyhow::Error::new(crate::services::oplog::OplogError::Fenced(fence)),
            OplogIndex::INITIAL,
            AgentMode::Durable,
        )
        .await;
        assert!(matches!(
            owner.selected_owner_failure(),
            Some(OwnerFailureWinner::Trap(TrapType::Interrupt(
                InterruptKind::ShardLost
            )))
        ));
    }

    #[test]
    fn only_encoded_trapped_tool_terminal_requests_physical_disposal() {
        use golem_common::model::oplog::HostResponseEntityInvocation;
        use golem_common::model::oplog::payload::types::{
            SerializableEntityBodyExecution, SerializableToolOperationTerminal,
            SerializableToolRpcError,
        };
        use golem_common::schema::IntoTypedSchemaValue;

        for execution in [
            SerializableEntityBodyExecution::Executed,
            SerializableEntityBodyExecution::Skipped,
            SerializableEntityBodyExecution::Trapped,
        ] {
            let response = HostResponseEntityInvocation {
                result: Ok(SerializableToolOperationTerminal {
                    body_execution: execution,
                    result: Err(SerializableToolRpcError::Denied("denied".into())),
                    admission_refusal: None,
                }
                .into_typed_schema_value()
                .unwrap()),
            };
            assert_eq!(
                is_trapped_tool_response(&response),
                execution == SerializableEntityBodyExecution::Trapped
            );
        }
        assert!(!is_trapped_tool_response(&HostResponseEntityInvocation {
            result: Err("owner failure".into()),
        }));
        assert!(!is_trapped_tool_response(&17_u8));
    }

    #[test]
    async fn component_body_still_observes_latched_cancellation() {
        use crate::services::agent_filesystem as fs;

        let (filesystem, control) = fs::resident_for_unload_test().await;
        let (_, drain) = fs::resident_generation_handle(&filesystem).for_invocation();
        let abort = tokio_util::sync::CancellationToken::new();
        abort.cancel();
        let (result, retained) = await_component_entity_body::<u8>(
            abort,
            None,
            Some(drain),
            Default::default(),
            std::future::pending(),
        )
        .await;
        assert_eq!(result.unwrap(), EntityBodyOutcome::Cancelled);
        assert!(retained.is_none());
        control.push_delete_and_verify(Ok(()));
        fs::delete(fs::seal(filesystem)).await.unwrap();
    }

    #[test]
    async fn component_body_completion_keeps_its_result_without_cancellation() {
        use crate::services::agent_filesystem as fs;

        let (filesystem, control) = fs::resident_for_unload_test().await;
        let (_, drain) = fs::resident_generation_handle(&filesystem).for_invocation();
        let (result, retained) = await_component_entity_body(
            tokio_util::sync::CancellationToken::new(),
            None,
            Some(drain),
            Default::default(),
            std::future::ready((Ok(EntityBodyOutcome::Completed(17)), None)),
        )
        .await;
        assert_eq!(result.unwrap(), EntityBodyOutcome::Completed(17));
        assert!(retained.is_none());
        control.push_delete_and_verify(Ok(()));
        fs::delete(fs::seal(filesystem)).await.unwrap();
    }

    #[test]
    async fn component_body_keeps_failure_and_lifecycle_distinct_from_local_abort() {
        let abort = tokio_util::sync::CancellationToken::new();
        abort.cancel();
        let expected = WorkerExecutorError::InvocationFailed {
            error: golem_common::model::oplog::AgentError::OutOfMemory,
            stderr: String::new(),
        };
        let (result, retained) = await_component_entity_body::<()>(
            abort.clone(),
            None,
            None,
            Default::default(),
            std::future::ready((Err(expected.clone()), None)),
        )
        .await;
        assert_eq!(result.unwrap_err(), expected);
        assert!(retained.is_none());

        let lifecycle = tokio_util::sync::CancellationToken::new();
        lifecycle.cancel();
        let (result, retained) = await_component_entity_body::<()>(
            abort,
            Some(lifecycle),
            None,
            Default::default(),
            std::future::pending(),
        )
        .await;
        assert!(matches!(
            result,
            Err(WorkerExecutorError::Interrupted { .. })
        ));
        assert!(retained.is_none());
    }

    #[test]
    async fn observed_component_failure_survives_abort_during_pending_finalization() {
        for expected in [
            WorkerExecutorError::InvocationFailed {
                error: golem_common::model::oplog::AgentError::OutOfMemory,
                stderr: String::new(),
            },
            WorkerExecutorError::recovery_required("trapped body cleanup failed"),
        ] {
            let abort = tokio_util::sync::CancellationToken::new();
            let finalization = EntityBodyFinalization::default();
            let body_finalization = finalization.clone();
            let (release, gate) = tokio::sync::oneshot::channel();
            let failure = expected.clone();
            let body = async move {
                body_finalization.begin();
                gate.await.unwrap();
                (Err::<EntityBodyOutcome<()>, _>(failure), None)
            };
            let mut run = Box::pin(await_component_entity_body(
                abort.clone(),
                None,
                None,
                finalization,
                body,
            ));
            // The body changes phase in its first poll, before the abort branch is polled.
            abort.cancel();
            assert!(futures::poll!(run.as_mut()).is_pending());
            assert!(!release.is_closed(), "abort discarded observed failure");
            release.send(()).unwrap();
            let (result, _) = run.await;
            assert_eq!(result.unwrap_err(), expected);
        }
    }

    #[test]
    #[test_r::timeout("5s")]
    async fn cancelled_active_body_drains_its_write_and_close_before_releasing_the_lane() {
        use crate::sandbox_filesystem::{SandboxHandle, SandboxWriteAttempt};

        use crate::services::agent_filesystem as fs;
        use futures::FutureExt;

        let (filesystem, control) = fs::resident_for_unload_test().await;
        let generation = fs::resident_generation_handle(&filesystem);
        let (body_filesystem, drain) = generation.for_invocation();
        control.push_open(Ok(SandboxHandle::scripted_file(706)));
        let file = fs::open(
            &body_filesystem,
            fs::PathTarget::at_root(&body_filesystem, "active").unwrap(),
            fs::OpenOptions::Existing {
                expected: fs::ObjectKind::File,
                access: fs::AccessMode::ReadWrite,
                follow: fs::Follow::Yes,
            },
        )
        .unwrap()
        .await
        .unwrap();
        let fs::OpenNode::File(file) = file.node else {
            panic!("scripted open returned a directory");
        };
        control.push_write(Ok(SandboxWriteAttempt::completed(4)));
        control.push_close(Ok(()));
        let write = control.block("write");
        let close = control.block("close");
        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "active-cleanup".into(),
            },
        ));
        let permit = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let next = lane
            .enter_primary(OplogIndex::from_u64(2))
            .unwrap()
            .acquire();
        tokio::pin!(next);
        let abort = tokio_util::sync::CancellationToken::new();
        let cancelled = abort.clone();
        let task = tokio::spawn(async move {
            let result = await_component_entity_body::<()>(
                cancelled,
                None,
                Some(drain),
                Default::default(),
                async move {
                    fs::write(
                        &body_filesystem,
                        &file,
                        fs::WritePlacement::At(3),
                        bytes::Bytes::from_static(b"data"),
                    )
                    .unwrap()
                    .await
                    .unwrap();
                    std::future::pending().await
                },
            )
            .await;
            permit.complete();
            result
        });
        write.wait_started().await;
        abort.cancel();
        close.wait_started().await;
        assert!(next.as_mut().now_or_never().is_none());
        write.release();
        write.wait_completed().await;
        assert!(next.as_mut().now_or_never().is_none());
        close.release();
        let (result, retained) = task.await.unwrap();
        assert_eq!(result.unwrap(), EntityBodyOutcome::Cancelled);
        assert!(retained.is_none());
        next.await.unwrap().complete();

        control.push_delete_and_verify(Ok(()));
        fs::delete(fs::seal(filesystem)).await.unwrap();
    }

    fn completed_handle(
        lane: OwnerLane,
        lane_await_required: bool,
    ) -> EntityInvocationHandle<&'static str> {
        let executor_tasks = crate::services::active_agents::InvocationLoops::new(
            tokio_util::sync::CancellationToken::new(),
        );
        let task = executor_tasks.spawn_entity(async move {
            EntityInvocationCompletion {
                result: Ok(EntityBodyOutcome::Completed("completed")),
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
            lane_await_required,
            abort: tokio_util::sync::CancellationToken::new(),
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
        assert_eq!(
            completion.result.unwrap(),
            EntityBodyOutcome::Completed("completed")
        );

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
        filesystem: Option<crate::services::agent_filesystem::FilesystemInvocation>,
    }

    impl RetainedEntityStore for RecordingStore {
        fn filesystem_invocation(
            &self,
        ) -> Option<crate::services::agent_filesystem::FilesystemInvocation> {
            self.filesystem.clone()
        }

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
        filesystem: Option<crate::services::agent_filesystem::FilesystemInvocation>,
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
            golem_common::model::IdempotencyKey::fresh(),
        )
        .unwrap();
        let registration = slot.register(&scope, Default::default()).unwrap();
        EntityInvocationResources::from_finished_body(
            events.map(|events| {
                Box::new(RecordingStore { events, filesystem }) as Box<dyn RetainedEntityStore>
            }),
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
            None,
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
            None,
            suspension.clone(),
            executor_tasks.clone(),
        );
        assert!(preparation.prepare_parent_end().await.is_err());
        assert!(eligible_now(&suspension));

        let settlement = recording_resources(
            Some(events.clone()),
            None,
            suspension.clone(),
            executor_tasks,
        );
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
            None,
            super::super::suspension::OwnerSuspension::new(),
            executor,
        );
        resources.release().await.unwrap();
    }

    #[test]
    async fn failure_settlement_after_retained_task_panic_does_not_panic() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let poison = events.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = poison.lock().unwrap();
            panic!("simulate retained Store callback panic");
        });
        let executor = crate::services::active_agents::InvocationLoops::new(Default::default());
        let mut resources = recording_resources(
            Some(events),
            None,
            super::super::suspension::OwnerSuspension::new(),
            executor,
        );
        assert!(resources.prepare_parent_end().await.is_err());
        assert!(resources.release_for_owner_failure().await.is_err());
        // Owner failure cleanup still settles resources after a disposal error.
        let _ = resources.settle_after_parent_end().await;
    }

    #[test]
    #[test_r::timeout("5s")]
    async fn abandoned_failure_release_holds_lane_until_filesystem_close_finishes() {
        use crate::sandbox_filesystem::SandboxHandle;
        use crate::services::agent_filesystem as fs;
        use futures::FutureExt;

        let (filesystem, control) = fs::resident_for_unload_test().await;
        let (body, invocation) = fs::resident_generation_handle(&filesystem).for_invocation();
        control.push_open(Ok(SandboxHandle::scripted_file(705)));
        let file = fs::open(
            &body,
            fs::PathTarget::at_root(&body, "retained").unwrap(),
            fs::OpenOptions::Existing {
                expected: fs::ObjectKind::File,
                access: fs::AccessMode::Read,
                follow: fs::Follow::Yes,
            },
        )
        .unwrap()
        .await
        .unwrap();
        control.push_close(Ok(()));
        let close_gate = control.block("close");
        drop(file);
        close_gate.wait_started().await;

        let lane = OwnerLane::new(golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "cleanup".into(),
            },
        ));
        let permit = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let next = lane
            .enter_primary(OplogIndex::from_u64(2))
            .unwrap()
            .acquire();
        tokio::pin!(next);
        let mut resources = recording_resources(
            Some(Arc::new(Mutex::new(Vec::new()))),
            Some(invocation),
            super::super::suspension::OwnerSuspension::new(),
            crate::services::active_agents::InvocationLoops::new(Default::default()),
        );
        resources.permit = Some(permit);
        assert!(
            resources
                .release_for_owner_failure()
                .now_or_never()
                .is_none()
        );
        drop(resources);
        assert!(next.as_mut().now_or_never().is_none());
        close_gate.release();
        next.await.unwrap().complete();

        control.push_delete_and_verify(Ok(()));
        fs::delete(fs::seal(filesystem)).await.unwrap();
    }
}
