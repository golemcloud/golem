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

use super::{
    DurableCallTrapContext, OplogEntryVersion, PersistedDurableFunctionInvocation,
    is_write_side_effect, reserve_invocation_id,
};
use crate::durable_host::concurrent::{
    self, DropEvent, Resolution, ResolutionOutcome, finish_prepared_access_to_live,
};
use crate::durable_host::replay_state::{CustomStartClaimOutcome, ReplayState, ReplayToLiveRole};
use crate::durable_host::{BeginReplayToLive, DurableWorkerCtx, PublicDurableWorkerState};
use crate::metrics::wasm::record_custom_invocation_scope_open;
use crate::preview2::golem::durability::durability;
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::oplog::OplogOps;
use crate::services::{HasShutdownToken, HasWorker};
use crate::workerctx::WorkerCtx;
use golem_common::model::Timestamp;
use golem_common::model::entity::{AgentEntity, InvocationExecutionMode, OwnerRuntime};
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostResponse, OplogEntry, OplogIndex,
};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{Mutex, Notify, oneshot};
use tracing::{debug, warn};
use wasmtime::AsContextMut;
use wasmtime::component::{Access, Accessor, HasSelf, Resource, TerminalConsumption};

#[derive(Debug, Clone)]
pub(crate) struct ActiveCustomInvocation {
    pub parent_start_index: Option<OplogIndex>,
    initiating_children: Arc<AtomicUsize>,
}

/// How a custom durable invocation that found no claimable recorded `Start` continues live.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CustomLiveContinuation {
    /// The shared cursor reached the replay target: the primary agent settles the replay-to-live
    /// transition, an incomplete entity continues live locally.
    ReplayTail,
    /// The claiming Store already continued live locally while the shared cursor may still replay
    /// other owners' records; only the Store's own resources switch.
    LocalTail,
}

/// The Store identity a custom durable invocation was admitted from, captured synchronously with
/// worker state before the claim. Mirrors `PreparedAccessStart` for the accessor path so both
/// paths take the same replay-to-live transition for the same Store.
struct CustomStoreAdmission<Ctx: WorkerCtx> {
    primary_runtime: bool,
    replaying_incomplete_entity: bool,
    store_continued_live: bool,
    tool_entity: bool,
    tool_operation: Option<crate::durable_host::tool::operation::OwnerToolOperation>,
    local_live_tail: Arc<AtomicBool>,
    public_state: PublicDurableWorkerState<Ctx>,
}

impl<Ctx: WorkerCtx> CustomStoreAdmission<Ctx> {
    fn role(&self) -> ReplayToLiveRole {
        if self.primary_runtime {
            ReplayToLiveRole::PrimaryAgent
        } else {
            ReplayToLiveRole::NonPrimary
        }
    }

    async fn continue_to_live<U: Send + 'static>(
        &self,
        accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
        replay_state: &ReplayState,
        linear_memory: &LinearMemoryTracker,
        continuation: CustomLiveContinuation,
        settling_what: &str,
    ) -> Result<(), WorkerExecutorError> {
        let pending = match continuation {
            CustomLiveContinuation::ReplayTail => {
                match crate::durable_host::begin_replay_to_live(
                    self.replaying_incomplete_entity,
                    self.tool_entity,
                    self.tool_operation.clone(),
                    &self.public_state,
                    linear_memory,
                    replay_state,
                    self.role(),
                    self.local_live_tail.clone(),
                )
                .await?
                {
                    BeginReplayToLive::ReplayResumed => {
                        return Err(WorkerExecutorError::runtime(format!(
                            "replay target grew while {settling_what} was settling"
                        )));
                    }
                    BeginReplayToLive::Pending(pending) => pending,
                }
            }
            CustomLiveContinuation::LocalTail => {
                crate::durable_host::begin_local_live_continuation(
                    self.replaying_incomplete_entity,
                    self.tool_entity,
                    self.tool_operation.clone(),
                    &self.public_state,
                    linear_memory,
                    self.role(),
                    self.local_live_tail.clone(),
                    replay_state,
                    replay_state.replay_target(),
                )
                .await?
            }
        };
        finish_prepared_access_to_live(pending, self.primary_runtime, accessor, accessor.getter())
            .await?
            .require_live()
    }
}
#[derive(Debug)]
struct CustomChildInitiation {
    counter: Arc<AtomicUsize>,
}

impl CustomChildInitiation {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl Drop for CustomChildInitiation {
    fn drop(&mut self) {
        let previous = self.counter.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "custom child initiation count underflow");
    }
}

#[derive(Debug, Clone, Copy)]
enum CustomBeginVerdict {
    Delivered,
    Cancelled,
    Suppressed,
}

type CustomBeginCoordinator =
    tokio::task::JoinHandle<Result<Option<OplogIndex>, WorkerExecutorError>>;

pub struct CustomBeginLifecycle {
    start_result: Mutex<Option<Result<OplogIndex, WorkerExecutorError>>>,
    start_ready: Notify,
    verdict: std::sync::Mutex<Option<oneshot::Sender<CustomBeginVerdict>>>,
    coordinator: std::sync::Mutex<Option<CustomBeginCoordinator>>,
    initiation: std::sync::Mutex<Option<CustomChildInitiation>>,
    cleanup_sink: UnboundedSender<DropEvent>,
    settled: AtomicBool,
}

impl Debug for CustomBeginLifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomBeginLifecycle")
            .field("settled", &self.settled.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl CustomBeginLifecycle {
    fn new(
        verdict: oneshot::Sender<CustomBeginVerdict>,
        initiation: Option<CustomChildInitiation>,
        cleanup_sink: UnboundedSender<DropEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            start_result: Mutex::new(None),
            start_ready: Notify::new(),
            verdict: std::sync::Mutex::new(Some(verdict)),
            coordinator: std::sync::Mutex::new(None),
            initiation: std::sync::Mutex::new(initiation),
            cleanup_sink,
            settled: AtomicBool::new(false),
        })
    }

    fn set_coordinator(&self, coordinator: CustomBeginCoordinator) {
        *self.coordinator.lock().unwrap() = Some(coordinator);
    }

    async fn complete_start(&self, result: Result<OplogIndex, WorkerExecutorError>) {
        *self.start_result.lock().await = Some(result);
        self.start_ready.notify_waiters();
    }

    async fn wait_start(&self) -> Result<OplogIndex, WorkerExecutorError> {
        loop {
            let notified = self.start_ready.notified();
            if let Some(result) = self.start_result.lock().await.clone() {
                return result;
            }
            notified.await;
        }
    }

    fn promote_to_active(&self) {
        self.release_initiation();
    }

    pub(crate) fn release_initiation(&self) {
        self.initiation.lock().unwrap().take();
    }

    fn settle(self: &Arc<Self>, verdict: CustomBeginVerdict) {
        if self.settled.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(sender) = self.verdict.lock().unwrap().take() {
            let _ = sender.send(verdict);
        }
        let coordinator = self.coordinator.lock().unwrap().take();
        if matches!(
            verdict,
            CustomBeginVerdict::Cancelled | CustomBeginVerdict::Suppressed
        ) {
            if let Some(terminal) = coordinator {
                let _ = self
                    .cleanup_sink
                    .send(DropEvent::SettleCustomInvocationBegin {
                        lifecycle: self.clone(),
                        terminal: Some(terminal),
                    });
            }
        } else {
            drop(coordinator);
        }
    }
}

struct CustomBeginTerminalGuard {
    lifecycle: Option<Arc<CustomBeginLifecycle>>,
}

impl CustomBeginTerminalGuard {
    fn consume(mut self, consumption: TerminalConsumption) {
        let lifecycle = self
            .lifecycle
            .take()
            .expect("custom begin terminal observer is invoked at most once");
        match consumption {
            TerminalConsumption::Delivered => lifecycle.settle(CustomBeginVerdict::Delivered),
            TerminalConsumption::NotDelivered => lifecycle.settle(CustomBeginVerdict::Cancelled),
            // Observer replacement on the custom-begin subtask is a forbidden pattern: the begin
            // host function registers exactly one terminal observer and performs no further
            // observable sub-operations on the same subtask. Settle as suppressed (the same
            // verdict as a dropped guard) so the coordinator treats the begin as unobserved.
            TerminalConsumption::Superseded => {
                tracing::error!(
                    "custom-begin terminal observer was superseded by a newer observer on the \
                     same host subtask; this is an invariant breach"
                );
                lifecycle.settle(CustomBeginVerdict::Suppressed);
            }
        }
    }
}

impl Drop for CustomBeginTerminalGuard {
    fn drop(&mut self) {
        if let Some(lifecycle) = self.lifecycle.take() {
            lifecycle.settle(CustomBeginVerdict::Suppressed);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CustomInvocationScope {
    pub scope_id: u64,
    pub owner_start_index: OplogIndex,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CustomInvocationContext {
    pub scopes: Vec<CustomInvocationScope>,
}

#[derive(Debug)]
pub struct CustomInvocationContextSnapshot {
    anchor: Option<OplogIndex>,
    context: Option<Arc<CustomInvocationContext>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeRemoval {
    Top,
    OutOfOrder,
    Missing,
}

impl CustomInvocationContext {
    pub(crate) fn current(&self) -> Option<CustomInvocationScope> {
        self.scopes.last().copied()
    }

    fn remove(&mut self, scope_id: u64) -> ScopeRemoval {
        let Some(position) = self
            .scopes
            .iter()
            .position(|scope| scope.scope_id == scope_id)
        else {
            return ScopeRemoval::Missing;
        };
        let removal = if position + 1 == self.scopes.len() {
            ScopeRemoval::Top
        } else {
            ScopeRemoval::OutOfOrder
        };
        self.scopes.remove(position);
        removal
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct OpenCustomInvocationScope {
    pub owner_start_index: OplogIndex,
    pub resource_rep: u32,
}

#[derive(Debug)]
pub struct LiveCustomDurableInvocation {
    scope_id: u64,
    start_index: Option<OplogIndex>,
    anchor: Option<OplogIndex>,
    context: Arc<CustomInvocationContext>,
    finishing: bool,
}
impl<Ctx: WorkerCtx> durability::HostLiveCustomDurableInvocation for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> durability::HostCustomInvocationContext for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> durability::HostCustomInvocationContextWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    fn drop(
        mut access: Access<'_, U, Self>,
        resource: Resource<CustomInvocationContextSnapshot>,
    ) -> anyhow::Result<()> {
        access.get().table().delete(resource)?;
        Ok(())
    }
}

fn open_live_custom_durable_invocation<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    start_index: OplogIndex,
    previous: Option<Arc<CustomInvocationContext>>,
    install: bool,
) -> anyhow::Result<Resource<LiveCustomDurableInvocation>> {
    accessor.with(|mut access| {
        let scope_id = {
            let ctx = access.get();
            let owner_invocation = ctx
                .state
                .active_custom_invocations
                .get(&start_index)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "custom durable invocation {start_index} is not active in live execution"
                    )
                })?;
            if let Some(current) = previous
                .as_deref()
                .and_then(CustomInvocationContext::current)
            {
                let registered = ctx
                    .state
                    .custom_invocation_scopes
                    .get(&current.scope_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "inherited custom invocation scope {} is no longer active",
                            current.scope_id
                        )
                    })?;
                if registered.owner_start_index != current.owner_start_index {
                    return Err(anyhow::anyhow!(
                        "inherited custom invocation scope {} has stale attribution",
                        current.scope_id
                    ));
                }
                if current.owner_start_index != start_index
                    && owner_invocation.parent_start_index != Some(current.owner_start_index)
                {
                    return Err(anyhow::anyhow!(
                        "custom durable invocation {start_index} is not nested under the current invocation {}",
                        current.owner_start_index
                    ));
                }
            }
            let scope_id = ctx.state.next_custom_invocation_scope_id;
            ctx.state.next_custom_invocation_scope_id = scope_id.checked_add(1).ok_or_else(|| {
                anyhow::anyhow!("custom invocation scope identity space exhausted")
            })?;
            scope_id
        };
        let mut next = previous.as_deref().cloned().unwrap_or_default();
        next.scopes.push(CustomInvocationScope {
            scope_id,
            owner_start_index: start_index,
        });
        let next = Arc::new(next);
        let anchor = access.get().invocation_anchor();
        let resource = access.get().table().push(LiveCustomDurableInvocation {
            scope_id,
            start_index: Some(start_index),
            anchor,
            context: next.clone(),
            finishing: false,
        })?;
        access.get().state.custom_invocation_scopes.insert(
            scope_id,
            OpenCustomInvocationScope {
                owner_start_index: start_index,
                resource_rep: resource.rep(),
            },
        );
        if install
            && let Err(err) = access.as_context_mut().set_guest_task_context(next) {
                let _ = access.get().table().delete(resource);
                access.get().state.custom_invocation_scopes.remove(&scope_id);
                return Err(err.into());
            }
        record_custom_invocation_scope_open();
        Ok(resource)
    })
}

fn close_live_custom_durable_invocation<U: Send + 'static, Ctx: WorkerCtx>(
    access: &mut Access<'_, U, HasSelf<DurableWorkerCtx<Ctx>>>,
    resource: Resource<LiveCustomDurableInvocation>,
) -> anyhow::Result<Option<OplogIndex>> {
    let start_index = deactivate_live_custom_durable_invocation(access, &resource)?;
    access.get().table().delete(resource)?;
    Ok(start_index)
}

fn deactivate_live_custom_durable_invocation<U: Send + 'static, Ctx: WorkerCtx>(
    access: &mut Access<'_, U, HasSelf<DurableWorkerCtx<Ctx>>>,
    resource: &Resource<LiveCustomDurableInvocation>,
) -> anyhow::Result<Option<OplogIndex>> {
    let resource_rep = resource.rep();
    let invocation = access.get().table().get_mut(resource)?;
    let scope_id = invocation.scope_id;
    let Some(start_index) = invocation.start_index.take() else {
        return Ok(None);
    };
    let current = access
        .as_context_mut()
        .guest_task_context::<CustomInvocationContext>()?;
    let registered = access
        .get()
        .state
        .custom_invocation_scopes
        .remove(&scope_id)
        .ok_or_else(|| {
            anyhow::anyhow!("custom invocation scope {} is no longer active", scope_id)
        })?;
    if registered.owner_start_index != start_index || registered.resource_rep != resource_rep {
        return Err(anyhow::anyhow!(
            "custom invocation scope {} has stale attribution",
            scope_id
        ));
    }

    let mut next = current.as_deref().cloned().unwrap_or_default();
    match next.remove(scope_id) {
        ScopeRemoval::Top => {}
        ScopeRemoval::OutOfOrder => {
            warn!(
                scope_id,
                "custom invocation resource was consumed out of nesting order"
            );
        }
        ScopeRemoval::Missing => return Ok(Some(start_index)),
    }
    if next.scopes.is_empty() {
        access.as_context_mut().clear_guest_task_context()?;
    } else {
        access
            .as_context_mut()
            .set_guest_task_context(Arc::new(next))?;
    }
    Ok(Some(start_index))
}

impl<U: Send + 'static, Ctx: WorkerCtx> durability::HostLiveCustomDurableInvocationWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    fn enter(
        mut access: Access<'_, U, Self>,
        resource: Resource<LiveCustomDurableInvocation>,
    ) -> anyhow::Result<()> {
        let invocation = access.get().table().get(&resource)?;
        let anchor = invocation.anchor;
        let context = invocation.context.clone();
        anyhow::ensure!(
            anchor == access.get().invocation_anchor(),
            "custom invocation context belongs to another invocation"
        );
        for scope in &context.scopes {
            anyhow::ensure!(
                access
                    .get()
                    .state
                    .custom_invocation_scopes
                    .get(&scope.scope_id)
                    .is_some_and(
                        |registered| registered.owner_start_index == scope.owner_start_index
                    ),
                "custom invocation scope {} is no longer active",
                scope.scope_id
            );
        }
        access.as_context_mut().set_guest_task_context(context)?;
        Ok(())
    }

    fn drop(
        mut access: Access<'_, U, Self>,
        resource: Resource<LiveCustomDurableInvocation>,
    ) -> anyhow::Result<()> {
        if let Some(start_index) = close_live_custom_durable_invocation(&mut access, resource)? {
            access
                .get()
                .state
                .active_custom_invocations
                .remove(&start_index);
        }
        Ok(())
    }

    async fn finish(
        accessor: &Accessor<U, Self>,
        resource: Resource<LiveCustomDurableInvocation>,
        response: golem_common::schema::wit::wire::TypedSchemaValue,
        forced_commit: bool,
    ) -> anyhow::Result<()> {
        finish_custom_invocation(accessor, resource, response, forced_commit, false).await
    }

    async fn finish_async(
        accessor: &Accessor<U, Self>,
        resource: Resource<LiveCustomDurableInvocation>,
        response: golem_common::schema::wit::wire::TypedSchemaValue,
        forced_commit: bool,
    ) -> anyhow::Result<()> {
        finish_custom_invocation(accessor, resource, response, forced_commit, true).await
    }
}

async fn finish_custom_invocation<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    resource: Resource<LiveCustomDurableInvocation>,
    response: golem_common::schema::wit::wire::TypedSchemaValue,
    forced_commit: bool,
    async_completion: bool,
) -> anyhow::Result<()> {
    let invocation = accessor.with(|mut access| {
            let ctx = access.get();
            let live = ctx.table().get(&resource)?;
            anyhow::ensure!(!live.finishing, "custom durable invocation is already finishing");
            let start_index = live.start_index.ok_or_else(|| anyhow::anyhow!(
                "custom durable invocation is already closed"
            ))?;
            let invocation = ctx
                .state
                .active_custom_invocations
                .get(&start_index)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "custom durable invocation {start_index} is not active in live execution"
                    )
                })?;
            if ctx
                .state
                .active_custom_invocations
                .values()
                .any(|invocation| invocation.parent_start_index == Some(start_index))
            {
                return Err(anyhow::anyhow!(
                    "cannot finish custom durable invocation {start_index} while a nested custom durable invocation is still active"
                ));
            }
            if invocation.initiating_children.load(Ordering::Acquire) != 0 {
                return Err(anyhow::anyhow!(
                    "cannot finish custom durable invocation {start_index} while a nested custom durable invocation is being started"
                ));
            }
            let response = golem_common::schema::wit::decode_typed_rejecting_quota_with(
                response, ctx,
            )
            .map_err(|e| {
                anyhow::anyhow!("Failed to decode durable function response schema value: {e}")
            })?;
            let oplog = ctx.state.oplog.clone();
            let worker = ctx.public_state.worker();
            ctx.table().get_mut(&resource)?.finishing = true;
            if !async_completion {
                close_live_custom_durable_invocation(&mut access, Resource::new_own(resource.rep()))?;
            }
            Ok::<_, anyhow::Error>((start_index, response, oplog, worker))
        })?;
    let (start_index, response, oplog, worker) = invocation;

    concurrent::drain_dropped_call_events_access(accessor, accessor.getter()).await?;
    let response = oplog
        .upload_payload_owned(HostResponse::Custom(response))
        .await
        .map_err(|err| anyhow::anyhow!("Failed to store durable function response: {err}"))?;
    let end = OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index,
        response: Some(response),
        forced_commit,
        span_finished: None,
        span_attributes: None,
    };
    let delivery =
        if async_completion {
            let recording = worker.runtime_append_tasks().register().ok_or(
                WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Restart,
                },
            )?;
            let tasks = worker.tasks.clone();
            let (handoff, terminal) = concurrent::prepare_access_terminal_effect(
                &tasks,
                recording,
                async move {
                    worker
                        .commit_oplog_and_update_state(crate::services::oplog::CommitLevel::Always)
                        .await?;
                    Ok(())
                },
                |(), _| {},
                |_| async {},
            )?;
            let append = oplog.enqueue_add(end);
            assert!(handoff.send(((), append)).is_ok());
            let mut delivery = accessor
                .with(|mut access| custom_completion_delivery(access.get(), start_index, None));
            delivery.wait_terminal(terminal).await?;
            Some(delivery)
        } else {
            worker.add_and_commit_oplog(end).await?;
            None
        };
    let checkpoint = accessor.with(|mut access| {
        let ctx = access.get();
        // Async callers retain the scope for destruction of an undelivered result.
        // Their owned resource removes it when finish completes or is cancelled.
        if !async_completion {
            ctx.state.active_custom_invocations.remove(&start_index);
        }
        ctx.state
            .at_clean_checkpoint_boundary()
            .then(|| ctx.public_state.worker())
    });
    if let Some(worker) = checkpoint {
        worker.checkpoint_status_mid_invocation().await;
    }
    if let Some(delivery) = delivery {
        delivery.deliver_at_accessor_terminal(accessor).await?;
    }
    Ok(())
}

fn custom_completion_delivery<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    start_index: OplogIndex,
    replay: Option<concurrent::ReplayDeliveryDisposition>,
) -> concurrent::CompletionDelivery {
    let recorder = ctx
        .state
        .completion_marker_recorder()
        .for_store(ctx.runtime_suspension.clone());
    let trap_context = DurableCallTrapContext {
        retry_from: start_index,
    };
    let cleanup = ctx
        .state
        .dropped_call_event_sender()
        .expect("dropped-call event sender is always available");
    let teardown = concurrent::TeardownProbe::new(
        ctx.public_state.worker().shutdown_token(),
        ctx.stream_runtime_teardown_probe(),
    );
    if let Some(disposition) = replay {
        concurrent::CompletionDelivery::replay_delivered(
            disposition,
            start_index,
            recorder,
            trap_context,
            Some(cleanup),
            teardown,
        )
    } else {
        concurrent::CompletionDelivery::live(
            start_index,
            recorder,
            trap_context,
            cleanup,
            concurrent::LiveCallPermit::new(ctx.state.live_host_call_counter()),
            teardown,
        )
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> durability::HostWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    fn exchange_custom_invocation_context(
        mut access: Access<'_, U, Self>,
        next: Option<Resource<CustomInvocationContextSnapshot>>,
    ) -> anyhow::Result<Resource<CustomInvocationContextSnapshot>> {
        let current = access
            .as_context_mut()
            .guest_task_context::<CustomInvocationContext>()?;
        let ctx = access.get();
        let anchor = ctx.invocation_anchor();
        let replacement = if let Some(resource) = &next {
            let snapshot = ctx.table().get(resource)?;
            anyhow::ensure!(
                snapshot.anchor == anchor,
                "custom invocation context belongs to another invocation"
            );
            let context = snapshot.context.clone();
            if let Some(context) = &context {
                for scope in &context.scopes {
                    let registered = ctx
                        .state
                        .custom_invocation_scopes
                        .get(&scope.scope_id)
                        .copied()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "custom invocation scope {} is no longer active",
                                scope.scope_id
                            )
                        })?;
                    anyhow::ensure!(
                        registered.owner_start_index == scope.owner_start_index,
                        "custom invocation scope {} has stale attribution",
                        scope.scope_id
                    );
                    let live =
                        ctx.table()
                            .get(&Resource::<LiveCustomDurableInvocation>::new_borrow(
                                registered.resource_rep,
                            ))?;
                    anyhow::ensure!(
                        live.scope_id == scope.scope_id
                            && live.start_index == Some(scope.owner_start_index),
                        "custom invocation scope {} has stale resource ownership",
                        scope.scope_id
                    );
                }
            }
            context
        } else {
            None
        };
        let previous = ctx.table().push(CustomInvocationContextSnapshot {
            anchor,
            context: current,
        })?;
        if let Some(resource) = next {
            access.get().table().delete(resource)?;
            if let Some(context) = replacement {
                access.as_context_mut().set_guest_task_context(context)?;
            } else {
                access.as_context_mut().clear_guest_task_context()?;
            }
        }
        Ok(previous)
    }

    async fn begin_custom_durable_invocation(
        accessor: &Accessor<U, Self>,
        function_name: String,
        request: golem_common::schema::wit::wire::TypedSchemaValue,
        function_type: durability::DurableFunctionType,
    ) -> anyhow::Result<durability::CustomDurableInvocation> {
        begin_custom_invocation(accessor, function_name, request, function_type, false).await
    }

    async fn begin_custom_durable_invocation_async(
        accessor: &Accessor<U, Self>,
        function_name: String,
        request: golem_common::schema::wit::wire::TypedSchemaValue,
        function_type: durability::DurableFunctionType,
    ) -> anyhow::Result<durability::CustomDurableInvocation> {
        begin_custom_invocation(accessor, function_name, request, function_type, true).await
    }
}

async fn begin_custom_invocation<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    function_name: String,
    request: golem_common::schema::wit::wire::TypedSchemaValue,
    function_type: durability::DurableFunctionType,
    async_completion: bool,
) -> anyhow::Result<durability::CustomDurableInvocation> {
    // Capture attribution before the first await. The immutable Arc is the initiation-time
    // snapshot inherited by this host task and its continuations.
    let custom_invocation_context = accessor.guest_task_context::<CustomInvocationContext>()?;
    let activity = accessor
        .has_guest_visible_subtask()
        .then(|| accessor.runtime_activity())
        .flatten();
    let (
        request,
        function_name,
        invocation_id,
        function_type,
        parent_start_index,
        recorded_parent_start_index,
        is_live,
        admission,
        oplog,
        worker,
        replay_state,
        linear_memory,
        child_initiation,
        cleanup_sink,
    ) = accessor.with(|mut access| {
        let ctx = access.get();
        let request = golem_common::schema::wit::decode_typed_rejecting_quota_with(request, ctx)
            .map_err(|e| {
                anyhow::anyhow!("Failed to decode durable function request schema value: {e}")
            })?;
        let function_name = HostFunctionName::Custom(function_name);
        let function_type: DurableFunctionType = function_type.into();
        if is_write_side_effect(&function_type) {
            DurableWorkerCtx::check_read_only_allows(
                ctx,
                "golem::durability::begin-custom-durable-invocation",
            )
            .map_err(anyhow::Error::msg)?;
        }

        let contextual_parent = custom_invocation_context
            .as_deref()
            .and_then(CustomInvocationContext::current)
            .map(|scope| {
                let registered = ctx
                    .state
                    .custom_invocation_scopes
                    .get(&scope.scope_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "custom invocation scope {} is no longer active",
                            scope.scope_id
                        )
                    })?;
                if registered.owner_start_index != scope.owner_start_index
                    || !ctx
                        .state
                        .active_custom_invocations
                        .contains_key(&scope.owner_start_index)
                {
                    return Err(anyhow::anyhow!(
                        "custom invocation scope {} has stale ownership attribution",
                        scope.scope_id
                    ));
                }
                Ok(scope.owner_start_index)
            })
            .transpose()?;
        let parent_start_index = contextual_parent;
        let anchor = parent_start_index
            .or_else(|| ctx.invocation_anchor())
            .ok_or_else(|| {
                anyhow::anyhow!("custom durability requires an active agent invocation")
            })?;
        if let Some(parent) = parent_start_index
            && !ctx.state.active_custom_invocations.contains_key(&parent)
        {
            return Err(anyhow::anyhow!(
                "custom durable invocation parent {parent} is not active"
            ));
        }
        let invocation_id = reserve_invocation_id(ctx, activity, anchor)?;
        let child_initiation = parent_start_index
            .and_then(|parent| ctx.state.active_custom_invocations.get(&parent))
            .map(|parent| CustomChildInitiation::new(parent.initiating_children.clone()));
        // A root custom invocation of an entity body records the entity invocation `Start` as
        // its parent, like every other durable call of that body, so replay can tell it from
        // a top-level call of the primary agent. The custom invocation tree itself (ids,
        // child initiation, active invocations) keeps using the custom parent.
        let recorded_parent_start_index =
            parent_start_index.or_else(|| ctx.entity_parent_start_index());
        Ok::<_, anyhow::Error>((
            HostRequest::Custom(request),
            function_name,
            invocation_id,
            function_type,
            parent_start_index,
            recorded_parent_start_index,
            ctx.state.durable_call_is_live(),
            CustomStoreAdmission {
                primary_runtime: *ctx.runtime() == OwnerRuntime::Agent,
                replaying_incomplete_entity: ctx.entity_invocation_scope().is_some_and(|scope| {
                    scope.mode() == InvocationExecutionMode::ReplayingIncomplete
                }),
                store_continued_live: ctx.state.store_continued_live(),
                tool_entity: matches!(
                    ctx.runtime(),
                    OwnerRuntime::Entity(AgentEntity::Tool(_) | AgentEntity::ToolMiddleware(_))
                ),
                tool_operation: ctx.entity_tool_operation(),
                local_live_tail: ctx.state.local_live_tail(),
                public_state: ctx.public_state.clone(),
            },
            ctx.state.oplog.clone(),
            ctx.public_state.worker(),
            ctx.state.replay_state.clone(),
            ctx.linear_memory_tracker(),
            child_initiation,
            ctx.state
                .dropped_call_event_sender()
                .expect("dropped-call event sender is always available"),
        ))
    })?;

    concurrent::drain_dropped_call_events_access(accessor, accessor.getter()).await?;

    // A call admitted after the live transition may still own a recorded `Start` the cursor
    // retained (see `WorkerState::durable_call_is_live`), so it claims first and records a new
    // `Start` only once replay reports that no `Start` carries its invocation id. The
    // continuation to live mirrors the accessor path (`claim_replay_access_with_options`):
    // only the primary agent may reach the shared replay tail, an incomplete entity continues
    // live locally, and a completed entity body without its recorded `Start` has diverged.
    let claimed = if is_live {
        None
    } else {
        match replay_state
            .claim_custom_start_for_store(
                &function_name,
                &function_type,
                recorded_parent_start_index,
                invocation_id,
                admission.store_continued_live,
            )
            .await?
        {
            CustomStartClaimOutcome::Claimed(claimed) => Some(claimed),
            outcome @ (CustomStartClaimOutcome::ReplayEnded
            | CustomStartClaimOutcome::StoreAlreadyLive) => {
                let replay_ended = matches!(outcome, CustomStartClaimOutcome::ReplayEnded);
                if !admission.replaying_incomplete_entity
                    && !(admission.primary_runtime && replay_ended)
                {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                            format!("custom durable Start {{ invocation_id: {invocation_id} }}"),
                            format!(
                                "replay continuation at {} is valid only for the primary agent replay tail or an incomplete entity",
                                replay_state.last_replayed_index()
                            ),
                        )
                        .into());
                }
                admission
                    .continue_to_live(
                        accessor,
                        &replay_state,
                        &linear_memory,
                        if replay_ended {
                            CustomLiveContinuation::ReplayTail
                        } else {
                            CustomLiveContinuation::LocalTail
                        },
                        "a new custom invocation",
                    )
                    .await?;
                None
            }
        }
    };

    let Some(claimed) = claimed else {
        let recording =
            worker
                .runtime_append_tasks()
                .register()
                .ok_or(WorkerExecutorError::Interrupted {
                    kind: InterruptKind::Restart,
                })?;
        let (verdict_tx, verdict_rx) = oneshot::channel();
        let lifecycle = CustomBeginLifecycle::new(verdict_tx, child_initiation, cleanup_sink);
        let coordinator_lifecycle = lifecycle.clone();
        let start_function_type = function_type.clone();
        let start_invocation_id = invocation_id;
        let start = tokio::spawn(async move {
            let persisted_request = oplog.upload_payload_owned(request).await.map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "Failed to store durable function request: {err}"
                ))
            })?;
            // A refused `Start` fails the begin: the guest must not perform a side effect the
            // shard's new owner, finding no `Start`, would perform again.
            worker
                .add_and_commit_oplog(OplogEntry::Start {
                    timestamp: Timestamp::now_utc(),
                    parent_start_index: recorded_parent_start_index,
                    function_name,
                    invocation_id: Some(start_invocation_id),
                    observational_owner: None,
                    request: Some(persisted_request),
                    durable_function_type: start_function_type,
                    span_started: None,
                    denial: None,
                })
                .await
                .map_err(WorkerExecutorError::from)
        });
        let cancellation_worker = accessor.with(|mut access| access.get().public_state.worker());
        let coordinator = tokio::spawn(async move {
            let _recording = recording;
            let start_result = start
                .await
                .map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "custom invocation Start recorder task failed: {err}"
                    ))
                })
                .and_then(|result| result);
            coordinator_lifecycle
                .complete_start(start_result.clone())
                .await;
            let verdict = verdict_rx.await.unwrap_or(CustomBeginVerdict::Suppressed);
            match (verdict, start_result) {
                (CustomBeginVerdict::Cancelled, Ok(start_index)) => {
                    cancellation_worker
                        .add_and_commit_oplog(OplogEntry::Cancelled {
                            timestamp: Timestamp::now_utc(),
                            start_index,
                            partial: None,
                            span_finished: None,
                        })
                        .await?;
                    Ok(Some(start_index))
                }
                (_, Err(err)) if matches!(verdict, CustomBeginVerdict::Cancelled) => Err(err),
                _ => Ok(None),
            }
        });
        lifecycle.set_coordinator(coordinator);
        let terminal_guard = CustomBeginTerminalGuard {
            lifecycle: Some(lifecycle.clone()),
        };
        accessor
            .register_terminal_observer(Box::new(move |consumption| {
                terminal_guard.consume(consumption)
            }))
            .map_err(|err| {
                anyhow::anyhow!("failed to observe custom invocation begin delivery: {err}")
            })?;
        let start_index = lifecycle.wait_start().await?;
        accessor.with(|mut access| {
            access.get().state.active_custom_invocations.insert(
                start_index,
                ActiveCustomInvocation {
                    parent_start_index,
                    initiating_children: Arc::new(AtomicUsize::new(0)),
                },
            );
        });
        lifecycle.promote_to_active();
        return Ok(durability::CustomDurableInvocation::Live(
            open_live_custom_durable_invocation(
                accessor,
                start_index,
                custom_invocation_context,
                !async_completion,
            )?,
        ));
    };

    let start_index = claimed.handle.start_idx();
    match replay_state
        .await_resolution_outcome(claimed.handle)
        .await?
    {
        ResolutionOutcome::Incomplete => {
            admission
                .continue_to_live(
                    accessor,
                    &replay_state,
                    &linear_memory,
                    CustomLiveContinuation::ReplayTail,
                    "an incomplete custom invocation",
                )
                .await?;
            accessor.with(|mut access| {
                access.get().state.active_custom_invocations.insert(
                    start_index,
                    ActiveCustomInvocation {
                        parent_start_index,
                        initiating_children: Arc::new(AtomicUsize::new(0)),
                    },
                );
            });
            Ok(durability::CustomDurableInvocation::Live(
                open_live_custom_durable_invocation(
                    accessor,
                    start_index,
                    custom_invocation_context,
                    !async_completion,
                )?,
            ))
        }
        ResolutionOutcome::Resolved(Resolution::Completed {
            response,
            delivery_marker,
            ..
        }) => {
            let response = response.ok_or_else(|| {
                WorkerExecutorError::unexpected_oplog_entry(
                    "End { response: Some(..) }",
                    "End { response: None }".to_string(),
                )
            })?;
            let response = oplog.download_payload(response).await.map_err(|err| {
                WorkerExecutorError::runtime(format!("End payload cannot be downloaded: {err}"))
            })?;
            let invocation = PersistedDurableFunctionInvocation {
                timestamp: claimed.timestamp,
                function_name: claimed.function_name.to_string(),
                response,
                function_type: claimed.durable_function_type,
                oplog_entry_version: OplogEntryVersion::V2,
            };
            if async_completion {
                let delivery = accessor.with(|mut access| {
                    custom_completion_delivery(
                        access.get(),
                        start_index,
                        Some(delivery_marker.map_or(
                            concurrent::ReplayDeliveryDisposition::AtReplayTail,
                            concurrent::ReplayDeliveryDisposition::AtMarker,
                        )),
                    )
                });
                delivery.deliver_at_accessor_terminal(accessor).await?;
            }
            Ok(durability::CustomDurableInvocation::Replayed(
                invocation.try_into()?,
            ))
        }
        ResolutionOutcome::Resolved(Resolution::Cancelled { cancelled_idx, .. }) => {
            debug!(
                "custom durable invocation begin was cancelled at {cancelled_idx}; parking replay until the guest cancels it again"
            );
            std::future::pending::<durability::CustomDurableInvocation>().await;
            unreachable!()
        }
        ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
            end_idx,
            marker_idx,
            ..
        }) => {
            if async_completion {
                std::future::pending::<()>().await;
            }
            Err(WorkerExecutorError::unexpected_oplog_entry(
                    "End",
                    format!(
                        "End at {end_idx} marked CompletionDiscarded at {marker_idx} for a custom durable invocation"
                    ),
                ).into())
        }
    }
}

#[cfg(test)]
mod tests;
