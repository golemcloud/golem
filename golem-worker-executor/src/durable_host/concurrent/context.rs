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

use super::*;
use golem_service_base::model::auth::AuthCtx;

/// Synchronous access to real context data. The closure cannot return a borrow of the data,
/// so neither a Store window nor a mutex guard can escape into an asynchronous operation.
pub(crate) trait DurableAccess<T: 'static, D: HasData + ?Sized>: Sync {
    fn with<R>(&self, f: impl FnOnce(ContextWindow<'_, T>) -> R) -> R;

    fn custom_invocation_scope(&self)
    -> Result<Option<CustomInvocationScope>, WorkerExecutorError>;
}

pub(crate) struct ContextWindow<'a, T> {
    data: &'a mut T,
}

impl<T> ContextWindow<'_, T> {
    pub(crate) fn data_mut(&mut self) -> &mut T {
        self.data
    }
}

impl<T: 'static, D: HasData + ?Sized> DurableAccess<T, D> for Accessor<T, D> {
    fn with<R>(&self, f: impl FnOnce(ContextWindow<'_, T>) -> R) -> R {
        self.with(|mut access| {
            f(ContextWindow {
                data: access.data_mut(),
            })
        })
    }

    fn custom_invocation_scope(
        &self,
    ) -> Result<Option<CustomInvocationScope>, WorkerExecutorError> {
        Ok(self
            .guest_task_context::<CustomInvocationContext>()
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            .as_deref()
            .and_then(CustomInvocationContext::current))
    }
}

/// An independently clonable handle to the actual native context, not a WorkerCtx facade.
/// Only synchronous windows acquire the mutex; protocol waits retain owned handles instead.
pub(in crate::durable_host) struct SharedDurableContext<Ctx: WorkerCtx> {
    ctx: Arc<std::sync::Mutex<Ctx>>,
    custom_invocation_scope: Option<CustomInvocationScope>,
}

impl<Ctx: WorkerCtx> Clone for SharedDurableContext<Ctx> {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
            custom_invocation_scope: self.custom_invocation_scope,
        }
    }
}

impl<Ctx: WorkerCtx> SharedDurableContext<Ctx> {
    pub(in crate::durable_host) fn new(
        ctx: Ctx,
        custom_invocation_scope: Option<CustomInvocationScope>,
    ) -> Self {
        Self {
            ctx: Arc::new(std::sync::Mutex::new(ctx)),
            custom_invocation_scope,
        }
    }

    /// Captures job-local attribution without changing the context or sibling handles.
    pub(in crate::durable_host) fn for_custom_invocation_scope(
        &self,
        custom_invocation_scope: Option<CustomInvocationScope>,
    ) -> Self {
        Self {
            ctx: self.ctx.clone(),
            custom_invocation_scope,
        }
    }

    pub(in crate::durable_host) fn with_ctx<R>(
        &self,
        f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R,
    ) -> R {
        self.with_worker_ctx(|ctx| f(ctx.durable_ctx_mut()))
    }

    pub(in crate::durable_host) fn with_worker_ctx<R>(&self, f: impl FnOnce(&mut Ctx) -> R) -> R {
        f(&mut self
            .ctx
            .lock()
            .expect("shared durable context mutex poisoned"))
    }
}

impl<Ctx: WorkerCtx> DurableAccess<Ctx, wasmtime::component::HasSelf<Ctx>>
    for SharedDurableContext<Ctx>
{
    fn with<R>(&self, f: impl FnOnce(ContextWindow<'_, Ctx>) -> R) -> R {
        self.with_worker_ctx(|ctx| f(ContextWindow { data: ctx }))
    }

    fn custom_invocation_scope(
        &self,
    ) -> Result<Option<CustomInvocationScope>, WorkerExecutorError> {
        Ok(self.custom_invocation_scope)
    }
}

/// Access to the real durable context while the shared call protocol owns its handles.
/// The borrowed implementation keeps the exclusive borrow across the entire operation;
/// the accessor implementation opens only synchronous Store windows.
pub(in crate::durable_host) trait DurableContext<Ctx: WorkerCtx> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R;

    fn drain(&mut self) -> impl Future<Output = Result<usize, TerminalCallError>> + Send;

    fn publish_direct_replay_tail(
        &mut self,
    ) -> impl Future<Output = Result<(), WorkerExecutorError>> + Send;

    fn finish_scope(
        &mut self,
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
    ) -> impl Future<Output = Result<(), WorkerExecutorError>> + Send {
        self.finish_scope_with_commit(function_type, begin_index, false)
    }

    fn finish_scope_with_commit(
        &mut self,
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> impl Future<Output = Result<(), WorkerExecutorError>> + Send;

    /// The caller holds the card-event boundary lock through live publication finalization.
    fn process_replay_events_locked(
        &mut self,
    ) -> impl Future<Output = Result<(), WorkerExecutorError>> + Send;

    fn agent_auth_ctx(
        &mut self,
    ) -> impl Future<Output = Result<AuthCtx, WorkerExecutorError>> + Send;
}

pub(in crate::durable_host) struct AccessorContext<
    'a,
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
    A: DurableAccess<T, D> + ?Sized = Accessor<T, D>,
> {
    store: &'a A,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    data: PhantomData<fn() -> D>,
}

impl<'a, T: 'static, D: HasData + ?Sized, Ctx: WorkerCtx, A: DurableAccess<T, D> + ?Sized>
    AccessorContext<'a, T, D, Ctx, A>
{
    pub(in crate::durable_host) fn new(
        store: &'a A,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Self {
        Self {
            store,
            get_ctx,
            data: PhantomData,
        }
    }
}

impl<T: 'static, D: HasData + ?Sized, Ctx: WorkerCtx, A: DurableAccess<T, D> + ?Sized>
    DurableContext<Ctx> for AccessorContext<'_, T, D, Ctx, A>
{
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R {
        self.store
            .with(|mut access| f((self.get_ctx)(access.data_mut())))
    }

    async fn drain(&mut self) -> Result<usize, TerminalCallError> {
        drain_dropped_call_events_context(self.store, self.get_ctx).await
    }

    async fn publish_direct_replay_tail(&mut self) -> Result<(), WorkerExecutorError> {
        super::call::publish_incomplete_replay_tail(self)
            .await?
            .require_live()?;
        let _boundary =
            lock_synchronized_card_event_boundary_access(self.store, self.get_ctx).await?;
        Ok(())
    }

    async fn finish_scope_with_commit(
        &mut self,
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        end_durable_function_access(
            self.store,
            self.get_ctx,
            function_type,
            begin_index,
            forced_commit,
        )
        .await
    }

    async fn process_replay_events_locked(&mut self) -> Result<(), WorkerExecutorError> {
        process_pending_replay_events_access(self.store, self.get_ctx).await
    }

    async fn agent_auth_ctx(&mut self) -> Result<AuthCtx, WorkerExecutorError> {
        agent_auth_ctx_at_serialized_access(self.store, self.get_ctx).await
    }
}

impl<Ctx: WorkerCtx> DurableContext<Ctx> for SharedDurableContext<Ctx> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R {
        SharedDurableContext::with_ctx(self, f)
    }

    async fn drain(&mut self) -> Result<usize, TerminalCallError> {
        drain_dropped_call_events_context(self, |ctx| ctx.durable_ctx_mut()).await
    }

    async fn publish_direct_replay_tail(&mut self) -> Result<(), WorkerExecutorError> {
        AccessorContext::new(self, |ctx| ctx.durable_ctx_mut())
            .publish_direct_replay_tail()
            .await
    }

    async fn finish_scope_with_commit(
        &mut self,
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        end_durable_function_access(
            self,
            |ctx| ctx.durable_ctx_mut(),
            function_type,
            begin_index,
            forced_commit,
        )
        .await
    }

    async fn process_replay_events_locked(&mut self) -> Result<(), WorkerExecutorError> {
        process_pending_replay_events_access(self, |ctx| ctx.durable_ctx_mut()).await
    }

    async fn agent_auth_ctx(&mut self) -> Result<AuthCtx, WorkerExecutorError> {
        agent_auth_ctx_at_serialized_access(self, |ctx| ctx.durable_ctx_mut()).await
    }
}

impl<Ctx: WorkerCtx> DurableContext<Ctx> for DurableWorkerCtx<Ctx> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R {
        f(self)
    }

    async fn drain(&mut self) -> Result<usize, TerminalCallError> {
        drain_queued_dropped_call_events(self).await
    }

    async fn publish_direct_replay_tail(&mut self) -> Result<(), WorkerExecutorError> {
        self.switch_to_live().await
    }

    async fn finish_scope_with_commit(
        &mut self,
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
        forced_commit: bool,
    ) -> Result<(), WorkerExecutorError> {
        self.end_durable_function(&function_type, begin_index, forced_commit)
            .await
    }

    async fn process_replay_events_locked(&mut self) -> Result<(), WorkerExecutorError> {
        self.process_pending_replay_events_locked().await
    }

    async fn agent_auth_ctx(&mut self) -> Result<AuthCtx, WorkerExecutorError> {
        self.capture_agent_auth_ctx_at_boundary()
            .await?
            .ok_or_else(|| {
                WorkerExecutorError::runtime(
                    "incomplete durable call did not switch to live execution",
                )
            })
    }
}
