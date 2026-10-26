// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::{
    AcceptedToolCall, Accessor, Arc, AsContextMut, AtomicBool, AttachmentMemory, DurableWorkerCtx,
    EntityCallMode, EntityInvocationDescriptor, FutureInvokeResultEntry, FutureToolInvokeGet,
    FutureToolInvokeState, HasActiveAgents, HasData, HasSelf, HasWorker, InterruptKind,
    InvocationExecutionMode, InvocationResult, Mutex, Notify, Ordering, OwnedToolEndpoints,
    Resource, SerializableToolRpcError, ToolBodyStdin, ToolCallStdin, ToolCaller, ToolExecution,
    ToolExecutionGetGuard, ToolExecutionState, ToolInvokeResponse, ToolOutputEntry,
    ToolOutputWriterEntry, ToolRpcError, WorkerCtx, WorkerExecutorError,
    admit_tool_response_secret_holds, anyhow, durability, materialize_output_writer, oneshot,
    operation, project_tool_response, register_tool_admission,
};
use super::{Future, execute_accepted_tool_call};

pub(super) fn capable_result_await_cohort(
    plans: &[FutureToolInvokeGet],
    current_parent: &crate::worker::owner_lane::OwnerInvocationId,
) -> Option<Vec<golem_common::model::oplog::OplogIndex>> {
    let mut starts = Vec::new();
    for plan in plans {
        let FutureToolInvokeGet::Active(execution) = plan else {
            continue;
        };
        if execution.filesystem != golem_common::model::entity::FilesystemCapability::Capable {
            continue;
        }
        if &execution.parent != current_parent || execution.result_snapshot().is_some() {
            continue;
        }
        starts.push(execution.start);
    }
    starts.sort_unstable();
    (!starts.is_empty()).then_some(starts)
}

impl ToolExecution {
    pub(super) fn new(
        operation: operation::OwnerToolOperation,
        cancellable: bool,
        producer: Option<std::sync::Weak<crate::worker::suspension::ExternalActivity>>,
    ) -> Arc<Self> {
        let invocation = operation
            .invocation_id()
            .expect("tool execution must have a durable invocation");
        Arc::new(Self {
            parent: operation.context().parent.clone(),
            start: invocation.start_index(),
            invocation,
            filesystem: operation.context().activation.filesystem(),
            operation,
            cancellable,
            state: Mutex::new(ToolExecutionState {
                result: None,
                failure: None,
            }),
            changed: Notify::new(),
            get_active: AtomicBool::new(false),
            cancel: tokio_util::sync::CancellationToken::new(),
            producer,
        })
    }

    pub(super) fn complete(&self, result: anyhow::Result<ToolInvokeResponse>) {
        let mut state = self.state.lock().unwrap();
        if state.result.is_some() || state.failure.is_some() {
            return;
        }
        match result {
            Ok(result) => state.result = Some(result),
            Err(error) => state.failure = Some(error.to_string()),
        }
        drop(state);
        self.changed.notify_waiters();
    }

    pub(super) fn result_snapshot(&self) -> Option<anyhow::Result<ToolInvokeResponse>> {
        let state = self.state.lock().unwrap();
        state
            .result
            .as_ref()
            .map(|result| Ok(result.clone()))
            .or_else(|| {
                state
                    .failure
                    .as_ref()
                    .map(|error| Err(anyhow!(error.clone())))
            })
    }

    pub(super) fn get_plan(self: &Arc<Self>) -> FutureToolInvokeGet {
        FutureToolInvokeGet::Active(self.clone())
    }

    pub(super) async fn result(&self) -> anyhow::Result<ToolInvokeResponse> {
        loop {
            let changed = self.changed.notified();
            if let Some(result) = self.result_snapshot() {
                return result;
            }
            changed.await;
        }
    }

    pub(super) fn cancel(&self) {
        if self.cancel_body() {
            self.operation.cancel_children();
        }
    }

    pub(super) fn cancel_body(&self) -> bool {
        if self.cancellable && self.operation.begin_cancel() {
            self.cancel.cancel();
            true
        } else {
            false
        }
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancellable && self.cancel.is_cancelled()
    }

    pub(super) async fn cancelled(&self) {
        if self.cancellable {
            self.cancel.cancelled().await;
        } else {
            std::future::pending().await
        }
    }
}

impl Drop for ToolExecutionGetGuard {
    fn drop(&mut self) {
        self.0.get_active.store(false, Ordering::Release);
    }
}

pub(super) async fn release_capable_tool_cohort<U, D, Ctx>(
    accessor: &Accessor<U, D>,
    get_ctx: fn(&mut U) -> &mut DurableWorkerCtx<Ctx>,
    parent: &crate::worker::owner_lane::OwnerInvocationId,
    cohort: operation::DeferredAdmissionCohort,
    starts: &[golem_common::model::oplog::OplogIndex],
) -> anyhow::Result<Option<crate::worker::owner_lane::OwnerLaneWait>>
where
    U: Send + 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if starts.is_empty() {
        return Ok(None);
    }
    let (deferred, operations, lane) = accessor.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.owner_execution.deferred_tool_admission(),
            ctx.owner_execution.tool_operations(),
            ctx.owner_execution.lane(),
        )
    });
    release_capable_tool_cohort_owned(&deferred, &operations, &lane, parent, cohort, starts)
        .await
        .map_err(Into::into)
}

pub(crate) async fn release_capable_tool_cohort_owned(
    deferred: &operation::DeferredAdmissionTable,
    operations: &operation::OwnerToolOperations,
    lane: &crate::worker::owner_lane::OwnerLane,
    parent: &crate::worker::owner_lane::OwnerInvocationId,
    cohort: operation::DeferredAdmissionCohort,
    starts: &[golem_common::model::oplog::OplogIndex],
) -> Result<Option<crate::worker::owner_lane::OwnerLaneWait>, WorkerExecutorError> {
    if starts.is_empty() {
        return Ok(None);
    }
    deferred
        .wait_and_register_cohort(parent, cohort, starts, operations, lane)
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
}

pub(crate) async fn prepare_tool_parent_end_owned(
    execution: &crate::worker::instance::OwnerExecution,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    let deferred = execution.deferred_tool_admission();
    let Some(starts) = deferred.close_parent_and_snapshot(&parent) else {
        return Ok(());
    };
    release_capable_tool_cohort_owned(
        &deferred,
        &execution.tool_operations(),
        &execution.lane(),
        &parent,
        operation::DeferredAdmissionCohort::ParentEnd,
        &starts,
    )
    .await?;
    Ok(())
}

pub(crate) async fn settle_tool_children_owned(
    execution: &crate::worker::instance::OwnerExecution,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    let operations = execution.tool_operations();
    let deferred = execution.deferred_tool_admission();
    operations.wait_parent_settled(&parent).await;
    if !deferred.clear_closed_parent(&parent) {
        return Err(WorkerExecutorError::runtime(format!(
            "tool parent {parent:?} settled without draining its closed deferred admissions",
        )));
    }
    Ok(())
}

pub(super) async fn get_tool_invoke_results<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    futures: &[Resource<FutureInvokeResultEntry>],
) -> anyhow::Result<Vec<Result<InvocationResult, ToolRpcError>>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let plans = accessor.with(|mut access| -> anyhow::Result<Vec<_>> {
        futures
            .iter()
            .map(|future| {
                let entry = access.get().table().get(future)?;
                Ok(match &entry.state {
                    FutureToolInvokeState::Ready(response) => {
                        FutureToolInvokeGet::Ready(response.clone())
                    }
                    FutureToolInvokeState::Active(execution) => execution.get_plan(),
                })
            })
            .collect()
    })?;

    get_tool_invoke_plans(accessor, plans).await
}

pub(super) async fn get_tool_invoke_plans<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    plans: Vec<FutureToolInvokeGet>,
) -> anyhow::Result<Vec<Result<InvocationResult, ToolRpcError>>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    observe_tool_invoke_plans(accessor, plans, async |responses| {
        let mut projected = Vec::with_capacity(responses.len());
        for response in responses {
            let response = admit_tool_response_secret_holds(
                accessor,
                response,
                crate::durable_host::concurrent::InvocationIdSource::GuestImport,
            )
            .await?;
            projected.push(project_tool_response(accessor, response));
        }
        Ok(projected)
    })
    .await
}

/// Awaits accepted tool results as one guest-facing durable call and finishes them with
/// `finish` before delivery. The entity `Start` records how a body ended, not when the guest
/// received that result: a body terminal is selected by host tasks whose timing replay does not
/// reproduce. This call records the guest's receipt as its own `CompletionDelivered` marker, so
/// live and replay release the result at the same guest boundary. `finish` may make durable
/// calls; they complete before the delivery, which stays the tail operation.
pub(super) async fn observe_tool_invoke_plans<U, Ctx, R>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    plans: Vec<FutureToolInvokeGet>,
    finish: impl AsyncFnOnce(Vec<ToolInvokeResponse>) -> anyhow::Result<R>,
) -> anyhow::Result<R>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    use crate::durable_host::concurrent::{
        AccessClaimOptions, Cancellable, DeferredCallReplayOutcome, DurableCallSession,
    };
    use crate::services::oplog::OplogOps;
    use golem_common::model::oplog::host_functions::GolemToolAwaitInvokeResults;
    use golem_common::model::oplog::{
        DurableFunctionType, HostRequest, HostRequestGolemToolAwaitInvokeResults,
        HostResponseGolemToolAwaitInvokeResults, OplogEntry,
    };

    let invocations = plans
        .iter()
        .filter_map(|plan| match plan {
            FutureToolInvokeGet::Active(execution) => Some(execution.start),
            _ => None,
        })
        .collect::<Vec<_>>();
    if invocations.is_empty()
        || accessor.with(|mut access| access.get().state.durability_is_suppressed())
    {
        let responses = await_tool_invoke_plans(accessor, plans).await?;
        return finish(responses).await;
    }
    let request = HostRequestGolemToolAwaitInvokeResults { invocations };
    let expected: HostRequest = request.clone().into();
    let mut call =
        DurableCallSession::<GolemToolAwaitInvokeResults, Cancellable>::start_access_with_options(
            accessor,
            accessor.getter(),
            DurableFunctionType::ReadLocal,
            AccessClaimOptions::default(),
            request,
        )
        .await?;
    if !call.is_live() {
        let start = call.start_index();
        let oplog = accessor.with(|mut access| access.get().state.oplog.clone());
        let OplogEntry::Start {
            request: Some(recorded),
            ..
        } = oplog.read(start).await.map_err(WorkerExecutorError::from)?
        else {
            return Err(call.trap(anyhow!("tool result await has no recorded request")));
        };
        let recorded = oplog
            .download_payload(recorded)
            .await
            .map_err(anyhow::Error::msg)?;
        if recorded != expected {
            return Err(call.trap(anyhow!(
                "tool result await awaits different invocations at {start}"
            )));
        }
    }
    let output = match await_tool_invoke_plans(accessor, plans).await {
        Ok(responses) => finish(responses).await,
        Err(error) => Err(error),
    };
    let output = match output {
        Ok(output) => output,
        Err(error) => return Err(call.trap(error)),
    };
    let delivery = 'delivery: {
        if !call.is_live() {
            match call
                .replay_access_deferred(accessor, accessor.getter())
                .await?
            {
                DeferredCallReplayOutcome::Replayed(_, delivery) => break 'delivery delivery,
                DeferredCallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        call.complete_access_deferred(
            accessor,
            accessor.getter(),
            HostResponseGolemToolAwaitInvokeResults {},
        )
        .await?
        .1
    };
    delivery.deliver_at_accessor_terminal(accessor).await?;
    Ok(output)
}

pub(super) async fn await_tool_invoke_plans<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut plans: Vec<FutureToolInvokeGet>,
) -> anyhow::Result<Vec<ToolInvokeResponse>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    let current_parent = accessor.with(|mut access| access.get().owner_invocation_id())?;
    let mut observed = Vec::new();
    let mut guards = Vec::new();
    for plan in &mut plans {
        let FutureToolInvokeGet::Active(execution) = plan else {
            continue;
        };
        if let Some(result) = execution.result_snapshot() {
            observed.push(execution.clone());
            *plan = match result {
                Ok(response) => FutureToolInvokeGet::Ready(Box::new(response)),
                Err(error) => FutureToolInvokeGet::Failed(error.to_string()),
            };
            continue;
        }
        if execution
            .get_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            *plan =
                FutureToolInvokeGet::Ready(Box::new(Err(SerializableToolRpcError::ProtocolError(
                    "tool invocation future already has an outstanding get".to_string(),
                ))));
        } else {
            observed.push(execution.clone());
            guards.push(ToolExecutionGetGuard(execution.clone()));
        }
    }

    let incapable_wait = accessor.with(|mut access| {
        access.get().owner_execution.lane().await_invocations(
            &current_parent,
            observed
                .iter()
                .filter(|&execution| {
                    execution.parent == current_parent
                        && execution.filesystem
                            == golem_common::model::entity::FilesystemCapability::Incapable
                })
                .map(|execution| {
                    crate::worker::owner_lane::OwnerInvocationId::Entity(
                        execution.invocation.clone(),
                    )
                }),
        )
    })?;

    for plan in &mut plans {
        let FutureToolInvokeGet::Active(execution) = plan else {
            continue;
        };
        if let Some(result) = execution.result_snapshot() {
            *plan = match result {
                Ok(response) => FutureToolInvokeGet::Ready(Box::new(response)),
                Err(error) => FutureToolInvokeGet::Failed(error.to_string()),
            };
        }
    }

    let lane_wait =
        if let Some(capable_starts) = capable_result_await_cohort(&plans, &current_parent) {
            let cohort_start = capable_starts[0];
            release_capable_tool_cohort(
                accessor,
                accessor.getter(),
                &current_parent,
                operation::DeferredAdmissionCohort::ResultAwait(cohort_start),
                &capable_starts,
            )
            .await?
        } else {
            None
        };

    let runtime = accessor.with(|mut access| access.get().runtime_suspension.clone());
    let executions = plans
        .iter()
        .filter_map(|plan| match plan {
            FutureToolInvokeGet::Active(execution) => Some(execution.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let responses = futures::future::join_all(plans.into_iter().map(|plan| async move {
        match plan {
            FutureToolInvokeGet::Ready(response) => Ok(response),
            FutureToolInvokeGet::Failed(error) => Err(anyhow!(error)),
            FutureToolInvokeGet::Active(execution) => execution.result().await.map(Box::new),
        }
    }));
    tokio::pin!(responses);
    let mut neutral = None;
    let responses = futures::future::poll_fn(|cx| {
        neutral = runtime.as_ref().and_then(|runtime| {
            let dependencies = executions
                .iter()
                .filter(|execution| {
                    let state = execution.state.lock().unwrap();
                    state.result.is_none() && state.failure.is_none()
                })
                .map(|execution| execution.producer.as_ref()?.upgrade())
                .collect::<Option<Vec<_>>>()?;
            runtime.neutral_externals(
                accessor.runtime_activity()?,
                dependencies.iter().map(Arc::as_ref),
            )
        });
        responses.as_mut().poll(cx)
    })
    .await;
    drop(neutral);
    if let Some(lane_wait) = lane_wait {
        lane_wait.wait().await;
    }
    incapable_wait.wait().await;

    drop(guards);
    responses
        .into_iter()
        .map(|response| response.map(|response| *response))
        .collect()
}

pub async fn fence_tool_operations_for_jump<Ctx: WorkerCtx>(
    store: &mut wasmtime::StoreContextMut<'_, Ctx>,
) -> Result<(), WorkerExecutorError> {
    let operations = store.data().durable_ctx().owner_execution.tool_operations();
    let worker = store.data().durable_ctx().public_state.worker();
    let mut shard_lost = false;
    operations.fence_for_jump(store, |error| {
        let failure = error.root_cause().downcast_ref::<WorkerExecutorError>().cloned()
            .or_else(|| error.root_cause().downcast_ref::<crate::services::oplog::OplogError>()
                .cloned().map(WorkerExecutorError::from));
        if let Some(failure) = failure {
            shard_lost |= worker.retire_if_shard_lost(&failure);
        }
        tracing::debug!(error = %error, "Abandoned Store task failed while draining Jump fence");
    }).await;
    if shard_lost {
        Err(WorkerExecutorError::Interrupted {
            kind: InterruptKind::ShardLost,
        })
    } else {
        Ok(())
    }
}

pub(crate) async fn prepare_tool_parent_end<Ctx: WorkerCtx>(
    store: &mut wasmtime::StoreContextMut<'_, Ctx>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    store
        .as_context_mut()
        .run_concurrent(async move |accessor| -> Result<(), WorkerExecutorError> {
            // Dropped acknowledgments leave accepted effects owned by the cleanup queue.
            // Join their publication before classifying output failures or closing the parent.
            crate::durable_host::concurrent::drain_dropped_call_events_access(accessor, |ctx| {
                ctx.durable_ctx_mut()
            })
            .await
            .map_err(|error| error.source)?;
            let execution =
                accessor.with(|mut access| access.data_mut().durable_ctx().owner_execution.clone());
            prepare_tool_parent_end_owned(&execution, parent).await
        })
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
}

pub(crate) async fn settle_tool_children<Ctx: WorkerCtx>(
    store: &mut wasmtime::StoreContextMut<'_, Ctx>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
) -> Result<(), WorkerExecutorError> {
    use futures::FutureExt;

    let execution = store.data().durable_ctx().owner_execution.clone();
    let operations = execution.tool_operations();
    let primary = store
        .data()
        .durable_ctx()
        .entity_invocation_scope()
        .is_none();
    let settlement = async move {
        let result = settle_tool_children_owned(&execution, parent).await;
        if primary {
            execution
                .tool_operations()
                .join_owner_failure_cleanup()
                .await?;
        }
        result
    }
    .boxed()
    .shared();
    loop {
        let pending = settlement.clone();
        match store
            .as_context_mut()
            .run_concurrent(async move |_accessor| pending.await)
            .await
        {
            Ok(result) => return result,
            // A secondary task error must not strand a registered failure continuation,
            // even while election still waits for another terminal selection to settle.
            Err(_) if operations.has_owner_failure() => {}
            // Cancelling an entity interrupts its guest; its accepted children still have to
            // resolve their own cancellation inside this Store before the parent settles.
            Err(error)
                if crate::worker::invocation::interrupt_kind_in(&error)
                    == Some(InterruptKind::EntityCancellation) => {}
            Err(error) => {
                return Err(durability::native_host_failure(
                    store.data().durable_ctx(),
                    error.into(),
                )
                .await);
            }
        }
    }
}

pub(super) async fn spawn_tool_execution<U, Ctx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    mut accepted: AcceptedToolCall,
    stdout: Option<Resource<ToolOutputEntry>>,
    stderr: Option<Resource<ToolOutputEntry>>,
) -> anyhow::Result<Arc<ToolExecution>>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
{
    register_tool_admission(accessor, &mut accepted)?;
    let caller = ToolCaller::capture(accessor);
    let execution = ToolExecution::new(
        accepted.operation.clone(),
        accepted.durability.scope().mode() != InvocationExecutionMode::ReplayingCompleted,
        caller
            .settlement
            .settlement_activity
            .as_ref()
            .map(Arc::downgrade),
    );
    accepted
        .operation
        .register_execution(Arc::downgrade(&execution));
    if accessor.with(|mut access| {
        access
            .get()
            .entity_cancellation()
            .is_some_and(|cancellation| cancellation.is_cancelled())
    }) {
        execution.cancel();
    }
    let replaying_incomplete = accepted.durability.scope().mode()
        == golem_common::model::entity::InvocationExecutionMode::ReplayingIncomplete;
    let reconstruction_supervisor = (accepted.operation.context().activation.filesystem()
        == golem_common::model::entity::FilesystemCapability::Incapable
        && (accepted.durability.scope().mode()
            == golem_common::model::entity::InvocationExecutionMode::ReplayingCompleted
            || replaying_incomplete))
        .then(oneshot::channel);
    let (completed_supervisor_started, completed_supervisor_ready) = match reconstruction_supervisor
    {
        Some((started, ready)) => (Some(started), Some(ready)),
        None => (None, None),
    };
    let endpoints = accessor.with(|mut access| -> anyhow::Result<_> {
        let ctx = access.get();
        let stdin = accepted
            .stdin
            .take()
            .map(|stdin| -> anyhow::Result<_> {
                Ok(match stdin {
                    ToolCallStdin::Attachment(stdin) => {
                        ToolBodyStdin::Attachment(ctx.table().delete(stdin)?)
                    }
                    ToolCallStdin::Durable(endpoint) => ToolBodyStdin::Durable(endpoint),
                })
            })
            .transpose()?;
        let discard = accepted.durability.call_mode() == EntityCallMode::FireAndForget;
        let EntityInvocationDescriptor::Tool(descriptor) = accepted.durability.operation();
        let stdout = stdout
            .map(|output| ctx.table().delete(output))
            .transpose()?;
        let stderr = stderr
            .map(|output| ctx.table().delete(output))
            .transpose()?;
        let memory = || {
            AttachmentMemory::for_store(
                ctx.public_state.worker().active_agents(),
                accepted.durability.scope().mode() == InvocationExecutionMode::Live,
            )
        };
        let endpoints = OwnedToolEndpoints {
            stdin,
            stdout: materialize_output_writer(
                stdout,
                discard && descriptor.declares_stdout,
                memory,
            ),
            stderr: materialize_output_writer(
                stderr,
                discard && descriptor.declares_stderr,
                memory,
            ),
        };
        if !accepted.operation.attach(
            endpoints.stdin.as_ref().and_then(ToolBodyStdin::controller),
            endpoints
                .stdout
                .as_ref()
                .map(ToolOutputWriterEntry::controller),
            endpoints
                .stderr
                .as_ref()
                .map(ToolOutputWriterEntry::controller),
        ) {
            return Err(anyhow!("owner generation fenced tool invocation"));
        }
        Ok(endpoints)
    })?;
    let tasks = caller.primary.active_agents().invocation_loops();
    let owned_execution = execution.clone();
    accepted.durability.accept_execution_ownership();
    tasks.spawn_entity(async move {
        let result = execute_accepted_tool_call(
            &caller,
            accepted,
            endpoints,
            Some(&owned_execution),
            completed_supervisor_started,
        )
        .await;
        owned_execution.complete(result);
    });
    if let Some(ready) = completed_supervisor_ready
        && ready.await.is_err()
    {
        match execution.result().await {
            Err(error) => return Err(error),
            Ok(_) if replaying_incomplete => return Ok(execution),
            Ok(_) => {
                return Err(anyhow!(
                    "completed reconstruction finished without starting its entity body"
                ));
            }
        }
    }
    Ok(execution)
}
