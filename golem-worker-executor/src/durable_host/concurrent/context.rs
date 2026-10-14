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

/// Access to the real durable context while the shared call protocol owns its handles.
/// The borrowed implementation keeps the exclusive borrow across the entire operation;
/// the accessor implementation opens only synchronous Store windows.
pub(in crate::durable_host) trait DurableContext<Ctx: WorkerCtx> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R;

    fn drain(&mut self) -> impl Future<Output = Result<usize, TerminalCallError>> + Send;

    fn finish_scope(
        &mut self,
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
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
> {
    store: &'a Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
}

impl<'a, T: 'static, D: HasData + ?Sized, Ctx: WorkerCtx> AccessorContext<'a, T, D, Ctx> {
    pub(in crate::durable_host) fn new(
        store: &'a Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    ) -> Self {
        Self { store, get_ctx }
    }
}

impl<T: 'static, D: HasData + ?Sized, Ctx: WorkerCtx> DurableContext<Ctx>
    for AccessorContext<'_, T, D, Ctx>
{
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R {
        self.store
            .with(|mut access| f((self.get_ctx)(access.data_mut())))
    }

    async fn drain(&mut self) -> Result<usize, TerminalCallError> {
        drain_dropped_call_events_access(self.store, self.get_ctx).await
    }

    async fn finish_scope(
        &mut self,
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        end_durable_function_access(self.store, self.get_ctx, function_type, begin_index, false)
            .await
    }

    async fn process_replay_events_locked(&mut self) -> Result<(), WorkerExecutorError> {
        process_pending_replay_events_access(self.store, self.get_ctx).await
    }

    async fn agent_auth_ctx(&mut self) -> Result<AuthCtx, WorkerExecutorError> {
        agent_auth_ctx_at_serialized_access(self.store, self.get_ctx).await
    }
}

impl<Ctx: WorkerCtx> DurableContext<Ctx> for DurableWorkerCtx<Ctx> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R {
        f(self)
    }

    async fn drain(&mut self) -> Result<usize, TerminalCallError> {
        drain_queued_dropped_call_events(self).await
    }

    async fn finish_scope(
        &mut self,
        function_type: DurableFunctionType,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        self.end_durable_function(&function_type, begin_index, false)
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
