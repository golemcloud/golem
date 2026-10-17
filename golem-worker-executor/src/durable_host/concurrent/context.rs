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
use golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal;
use golem_service_base::model::auth::AuthCtx;

/// Synchronous access to real context data. The closure cannot return a borrow of the data,
/// so neither a Store window nor a mutex guard can escape into an asynchronous operation.
pub(crate) trait DurableAccess<T: 'static, D: HasData + ?Sized>: Sync {
    fn with<R>(&self, f: impl FnOnce(ContextWindow<'_, T>) -> R) -> R;

    fn custom_invocation_scope(&self)
    -> Result<Option<CustomInvocationScope>, WorkerExecutorError>;

    fn reserve_call_invocation_id<Ctx: WorkerCtx>(
        &self,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        observed_by_custom_invocation: bool,
    ) -> Result<Option<golem_common::model::oplog::DurableInvocationId>, WorkerExecutorError>;
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

    fn reserve_call_invocation_id<Ctx: WorkerCtx>(
        &self,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        observed_by_custom_invocation: bool,
    ) -> Result<Option<golem_common::model::oplog::DurableInvocationId>, WorkerExecutorError> {
        super::call::reserve_guest_import_invocation_id(
            self,
            get_ctx,
            observed_by_custom_invocation,
        )
    }
}

/// An independently clonable handle to the actual native context, not a WorkerCtx facade.
/// Only synchronous windows acquire the mutex; protocol waits retain owned handles instead.
pub(crate) struct SharedDurableContext<Ctx: WorkerCtx> {
    ctx: Arc<SharedNativeContext<Ctx>>,
    custom_invocation_scope: Option<CustomInvocationScope>,
}

struct SharedNativeContext<Ctx> {
    // Field order keeps the entity registered until the actual context is destroyed,
    // including when aborted children retain the last shared handle during teardown.
    ctx: std::sync::Mutex<Ctx>,
    _registration: crate::worker::entity_slot::EntitySlotRegistration,
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
    pub(crate) fn new(
        ctx: Ctx,
        custom_invocation_scope: Option<CustomInvocationScope>,
        registration: crate::worker::entity_slot::EntitySlotRegistration,
    ) -> Self {
        Self {
            ctx: Arc::new(SharedNativeContext {
                ctx: std::sync::Mutex::new(ctx),
                _registration: registration,
            }),
            custom_invocation_scope,
        }
    }

    pub(crate) fn with_ctx<R>(&self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R {
        self.with_worker_ctx(|ctx| f(ctx.durable_ctx_mut()))
    }

    pub(crate) fn with_worker_ctx<R>(&self, f: impl FnOnce(&mut Ctx) -> R) -> R {
        f(&mut self
            .ctx
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

    fn reserve_call_invocation_id<C: WorkerCtx>(
        &self,
        get_ctx: fn(&mut Ctx) -> &mut DurableWorkerCtx<C>,
        observed_by_custom_invocation: bool,
    ) -> Result<Option<golem_common::model::oplog::DurableInvocationId>, WorkerExecutorError> {
        if observed_by_custom_invocation {
            return Ok(None);
        }
        self.with_worker_ctx(|ctx| {
            super::call::reserve_import_invocation_id(get_ctx(ctx), None)
                .map(|source| source.reserved())
        })
    }
}

/// Access to the real durable context while the shared call protocol owns its handles.
/// The borrowed implementation keeps the exclusive borrow across the entire operation;
/// the accessor implementation opens only synchronous Store windows.
pub(crate) trait DurableContext<Ctx: WorkerCtx> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R;

    fn drain(&mut self) -> impl Future<Output = Result<usize, TerminalCallError>> + Send;

    fn publish_direct_replay_tail(
        &mut self,
        start_index: OplogIndex,
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

    /// Completes retained replay applications before exposing live continuation.
    fn process_replay_events(
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

    async fn publish_direct_replay_tail(
        &mut self,
        start_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        super::call::publish_incomplete_replay_tail_at(
            self,
            Some(SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                index: start_index.into(),
            }),
        )
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

    async fn process_replay_events(&mut self) -> Result<(), WorkerExecutorError> {
        process_pending_replay_events_access(self.store, self.get_ctx).await
    }

    async fn agent_auth_ctx(&mut self) -> Result<AuthCtx, WorkerExecutorError> {
        super::super::call_coordinator::capture_live_agent_authority_access(
            self.store,
            self.get_ctx,
            |ctx| DurableWorkerCtx::agent_auth_ctx(ctx),
        )
        .await?
        .ok_or_else(|| {
            WorkerExecutorError::runtime("incomplete durable call did not switch to live execution")
        })
    }
}

impl<Ctx: WorkerCtx> DurableContext<Ctx> for SharedDurableContext<Ctx> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R {
        SharedDurableContext::with_ctx(self, f)
    }

    async fn drain(&mut self) -> Result<usize, TerminalCallError> {
        drain_dropped_call_events_context(self, |ctx| ctx.durable_ctx_mut()).await
    }

    async fn publish_direct_replay_tail(
        &mut self,
        start_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        AccessorContext::new(self, |ctx| ctx.durable_ctx_mut())
            .publish_direct_replay_tail(start_index)
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

    async fn process_replay_events(&mut self) -> Result<(), WorkerExecutorError> {
        process_pending_replay_events_access(self, |ctx| ctx.durable_ctx_mut()).await
    }

    async fn agent_auth_ctx(&mut self) -> Result<AuthCtx, WorkerExecutorError> {
        super::super::call_coordinator::capture_live_agent_authority_access(
            self,
            |ctx| ctx.durable_ctx_mut(),
            |ctx| DurableWorkerCtx::agent_auth_ctx(ctx),
        )
        .await?
        .ok_or_else(|| {
            WorkerExecutorError::runtime("incomplete durable call did not switch to live execution")
        })
    }
}

impl<Ctx: WorkerCtx> DurableContext<Ctx> for DurableWorkerCtx<Ctx> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DurableWorkerCtx<Ctx>) -> R) -> R {
        f(self)
    }

    async fn drain(&mut self) -> Result<usize, TerminalCallError> {
        drain_queued_dropped_call_events(self).await
    }

    async fn publish_direct_replay_tail(
        &mut self,
        start_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        self.switch_to_live_at(Some(
            SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                index: start_index.into(),
            },
        ))
        .await
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

    async fn process_replay_events(&mut self) -> Result<(), WorkerExecutorError> {
        self.process_pending_replay_events().await
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

#[cfg(test)]
mod tests {
    use super::SharedNativeContext;
    use crate::worker::entity_slot::EntitySlot;
    use golem_common::model::component::ComponentId;
    use golem_common::model::entity::{
        EntityActivation, EntityActivationPolicy, EntityInvocationId, EntityInvocationScope,
        ExecutableTarget, FilesystemCapability, InvocationExecutionMode, OwnedAgentEntityId,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::tool::{SecretKeyScope, ToolFilesystemAccess};
    use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId};
    use std::sync::{Arc, Mutex};
    use test_r::test;

    #[test]
    fn shared_context_drops_before_its_registration() {
        let owner = OwnedAgentId::new(
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
            IdempotencyKey::fresh(),
            false,
            IdempotencyKey::fresh(),
        )
        .unwrap();
        let registration = slot.register(&scope, Default::default()).unwrap();
        struct DropProbe {
            slot: Arc<EntitySlot>,
            observed: Arc<Mutex<Option<usize>>>,
        }
        impl Drop for DropProbe {
            fn drop(&mut self) {
                *self.observed.lock().unwrap() = Some(self.slot.active_invocation_count());
            }
        }
        let observed = Arc::new(Mutex::new(None));
        let backing = Arc::new(SharedNativeContext {
            ctx: Mutex::new(DropProbe {
                slot: slot.clone(),
                observed: observed.clone(),
            }),
            _registration: registration,
        });
        let child = backing.clone();
        drop(backing);
        assert_eq!(slot.active_invocation_count(), 1);
        assert_eq!(*observed.lock().unwrap(), None);
        drop(child);
        assert_eq!(*observed.lock().unwrap(), Some(1));
        assert_eq!(slot.active_invocation_count(), 0);
    }
}
