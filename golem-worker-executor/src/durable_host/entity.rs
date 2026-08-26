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

//! Durable owner-oplog boundary for transient entity bodies.
//!
//! The tool call surface and middleware-chain dispatcher supply only dispatch behavior. They begin
//! an entity record here, launch the returned invocation scope through `ActiveAgent`, then hand the
//! body handle back to [`EntityInvocationDurability::drive_access`].

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{
    AccessClaimOptions, Cancellable, DurableCallSession, ReconstructionReplayOutcome,
};
use crate::worker::entity_invocation::EntityInvocationHandle;
use crate::worker::instance::HistoricalReconstruction;
use crate::worker::owner_lane::OwnerInvocationId;
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use golem_common::model::entity::{
    AgentEntity, EntityActivation, EntityCallMode, EntityInvocationId, EntityInvocationRequest,
    EntityInvocationScope, InvocationExecutionMode, OwnedAgentEntityId,
};
use golem_common::model::oplog::host_functions::GolemEntityInvoke;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostRequestEntityInvocation, HostResponseEntityInvocation,
    OplogIndex,
};
use golem_common::schema::TypedSchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::future::Future;
use std::sync::Arc;
use wasmtime::component::{Accessor, HasData};

pub enum EntityInvocationDurabilityOutcome {
    Completed(Box<HostResponseEntityInvocation>),
    Cancelled,
}

enum EntityReconstructionResolution<R, H> {
    Replayed(R),
    Cancelled,
    Incomplete(H),
}

#[derive(Debug)]
enum EntityReconstructionOutcome<R, H> {
    Replayed(R),
    Cancelled,
    Incomplete { response: R, handle: H },
}

/// Task-owned durable state for one entity body. Its `Start` index is the entity invocation ID;
/// dropping it before a terminal records cancellation through the generic concurrent-call path.
pub struct EntityInvocationDurability {
    handle: DurableCallSession<GolemEntityInvoke, Cancellable>,
    scope: EntityInvocationScope,
    parent: OwnerInvocationId,
    call_mode: EntityCallMode,
    historical_reconstruction: Option<HistoricalReconstruction>,
}

impl EntityInvocationDurability {
    pub async fn start_access<T, D, Ctx>(
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        parent: OwnerInvocationId,
        entity: AgentEntity,
        activation: Arc<EntityActivation>,
        calling_principal: golem_common::model::agent::Principal,
        call_mode: EntityCallMode,
        input: TypedSchemaValue,
    ) -> Result<Self, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let parent_start_index = parent.start_index();
        let metadata = desert_rust::serialize_to_byte_vec(&EntityInvocationRequest {
            entity: entity.clone(),
            activation: activation.as_ref().clone(),
            calling_principal: calling_principal.clone(),
            call_mode,
        })
        .map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "failed to encode entity invocation metadata: {error}"
            ))
        })?;
        let request = HostRequestEntityInvocation { metadata, input };
        let request_identity: HostRequest = request.clone().into();
        let handle =
            DurableCallSession::<GolemEntityInvoke, Cancellable>::start_access_with_options(
                store,
                get_ctx,
                DurableFunctionType::WriteLocal,
                AccessClaimOptions {
                    request_identity: Some(request_identity),
                    parent_start_index: Some(parent_start_index),
                    ..AccessClaimOptions::default()
                },
                async move |_| Ok(request),
            )
            .await?;
        let owner =
            store.with(|mut access| get_ctx(access.data_mut()).state.owned_agent_id.clone());
        let invocation_id =
            EntityInvocationId::new(OwnedAgentEntityId { owner, entity }, handle.start_index())
                .map_err(WorkerExecutorError::runtime)?;
        let execution_mode = if handle.is_live() {
            InvocationExecutionMode::Live
        } else {
            let replay =
                store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
            if replay.has_visible_terminal(handle.start_index()).await? {
                InvocationExecutionMode::ReplayingCompleted
            } else {
                InvocationExecutionMode::ReplayingIncomplete
            }
        };
        let scope = EntityInvocationScope::new(
            invocation_id,
            parent_start_index,
            activation,
            calling_principal,
            execution_mode,
        )
        .map_err(WorkerExecutorError::runtime)?;
        let historical_reconstruction = (!handle.is_live()).then(|| {
            store.with(|mut access| {
                get_ctx(access.data_mut())
                    .owner_execution
                    .register_historical_reconstruction(handle.start_index())
            })
        });

        Ok(Self {
            handle,
            scope,
            parent,
            call_mode,
            historical_reconstruction,
        })
    }

    pub fn scope(&self) -> &EntityInvocationScope {
        &self.scope
    }

    /// Drives body reconstruction and the outer durable terminal together. A completed replay does
    /// not release its recorded response until the fresh body has reconstructed local effects; a
    /// cancellation aborts the transient Store; and an incomplete Start switches to live and is
    /// completed under the original Start index.
    pub async fn drive_access<T, D, Ctx>(
        self,
        store: &Accessor<T, D>,
        get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
        body: EntityInvocationHandle<HostResponseEntityInvocation>,
    ) -> Result<EntityInvocationDurabilityOutcome, WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
        Ctx: WorkerCtx,
    {
        let Self {
            mut handle,
            scope,
            parent,
            call_mode,
            mut historical_reconstruction,
        } = self;
        let invocation = scope.invocation_id().clone();
        let abort = body.abort_handle();
        let body = async move {
            match call_mode {
                EntityCallMode::FireAndForget => body.join().await,
                EntityCallMode::Synchronous | EntityCallMode::Asynchronous => {
                    body.await_result(&parent).await
                }
            }
        };
        tokio::pin!(body);

        if handle.is_live() {
            let response = match body.await {
                Ok(response) => response,
                Err(error) => {
                    let _ = handle.trap(error.clone());
                    return Err(error);
                }
            };
            let response = handle
                .complete_access(store, get_ctx, response)
                .await
                .map_err(|error| error.source)?;
            return Ok(EntityInvocationDurabilityOutcome::Completed(Box::new(
                response,
            )));
        }

        let replay = async {
            Ok(
                match handle.replay_reconstruction_access(store, get_ctx).await? {
                    ReconstructionReplayOutcome::Replayed(response) => {
                        EntityReconstructionResolution::Replayed(response)
                    }
                    ReconstructionReplayOutcome::Cancelled => {
                        EntityReconstructionResolution::Cancelled
                    }
                    ReconstructionReplayOutcome::Incomplete(handle) => {
                        EntityReconstructionResolution::Incomplete(handle)
                    }
                },
            )
        };
        let (replay_state, active_reconstruction_bodies) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.owner_execution.historical_reconstruction_bodies(),
            )
        });
        let unconsumed_scope = replay_state
            .await_unconsumed_scope_entry(invocation.start_index(), active_reconstruction_bodies);
        match coordinate_entity_reconstruction(
            &invocation,
            scope.mode(),
            body,
            replay,
            unconsumed_scope,
            || abort.abort(),
            historical_reconstruction.take(),
        )
        .await?
        {
            EntityReconstructionOutcome::Cancelled => {
                Ok(EntityInvocationDurabilityOutcome::Cancelled)
            }
            EntityReconstructionOutcome::Replayed(recorded) => Ok(
                EntityInvocationDurabilityOutcome::Completed(Box::new(recorded)),
            ),
            EntityReconstructionOutcome::Incomplete {
                response,
                handle: live_handle,
            } => {
                let response = live_handle
                    .complete_access(store, get_ctx, response)
                    .await
                    .map_err(|error| error.source)?;
                Ok(EntityInvocationDurabilityOutcome::Completed(Box::new(
                    response,
                )))
            }
        }
    }
}

trait ReconstructionGuard {
    fn body_settled(&mut self);
}

impl ReconstructionGuard for () {
    fn body_settled(&mut self) {}
}

impl ReconstructionGuard for HistoricalReconstruction {
    fn body_settled(&mut self) {
        HistoricalReconstruction::body_settled(self);
    }
}

async fn coordinate_entity_reconstruction<R, H, B, F, S, A, G>(
    invocation: &EntityInvocationId,
    execution_mode: InvocationExecutionMode,
    body: B,
    replay: F,
    structural_stall: S,
    abort: A,
    mut historical_reconstruction: Option<G>,
) -> Result<EntityReconstructionOutcome<R, H>, WorkerExecutorError>
where
    R: PartialEq,
    B: Future<Output = Result<R, WorkerExecutorError>>,
    F: Future<Output = Result<EntityReconstructionResolution<R, H>, WorkerExecutorError>>,
    S: Future<Output = Result<OplogIndex, WorkerExecutorError>>,
    A: FnOnce(),
    G: ReconstructionGuard,
{
    tokio::pin!(body);
    tokio::pin!(replay);
    tokio::pin!(structural_stall);

    enum First<B, R> {
        Body(B),
        Replay(R),
    }
    let first = tokio::select! {
        body_result = &mut body => First::Body(body_result),
        replay_result = &mut replay => First::Replay(replay_result),
    };
    let (body_result, replay_result) = match first {
        First::Replay(replay_result) => (None, replay_result),
        First::Body(body_result) => {
            if let Some(reconstruction) = historical_reconstruction.as_mut() {
                reconstruction.body_settled();
            }
            match body_result {
                Err(error) => match replay.as_mut().now_or_never() {
                    Some(replay_result) => (Some(Err(error)), replay_result),
                    None => {
                        return Err(match execution_mode {
                            InvocationExecutionMode::ReplayingCompleted => {
                                replay_body_failure(invocation, error)
                            }
                            InvocationExecutionMode::ReplayingIncomplete
                            | InvocationExecutionMode::Live => error,
                        });
                    }
                },
                Ok(response) => {
                    let replay_result = tokio::select! {
                        biased;
                        replay_result = &mut replay => replay_result,
                        stalled_at = &mut structural_stall => {
                            let stalled_at = stalled_at?;
                            return Err(WorkerExecutorError::unexpected_oplog_entry(
                                format!("completed replay body for {invocation}"),
                                format!("entity body returned before consuming its recorded descendant at {stalled_at}"),
                            ));
                        }
                    };
                    (Some(Ok(response)), replay_result)
                }
            }
        }
    };
    let mut abort = Some(abort);
    let replay_result = match replay_result {
        Ok(result) => result,
        Err(error) => {
            abort.take().unwrap()();
            if body_result.is_none() {
                let _ = body.await;
                if let Some(reconstruction) = historical_reconstruction.as_mut() {
                    reconstruction.body_settled();
                }
            }
            return Err(error);
        }
    };

    match replay_result {
        EntityReconstructionResolution::Cancelled => {
            abort.take().unwrap()();
            if body_result.is_none() {
                let _ = body.await;
                if let Some(reconstruction) = historical_reconstruction.as_mut() {
                    reconstruction.body_settled();
                }
            }
            Ok(EntityReconstructionOutcome::Cancelled)
        }
        EntityReconstructionResolution::Replayed(recorded) => {
            let reconstructed = match body_result {
                Some(response) => {
                    response.map_err(|error| replay_body_failure(invocation, error))?
                }
                None => {
                    let response = body.await;
                    if let Some(reconstruction) = historical_reconstruction.as_mut() {
                        reconstruction.body_settled();
                    }
                    response.map_err(|error| replay_body_failure(invocation, error))?
                }
            };
            if reconstructed != recorded {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    format!("recorded entity invocation result for {invocation}"),
                    format!("reconstructed a different result for {invocation}"),
                ));
            }
            Ok(EntityReconstructionOutcome::Replayed(recorded))
        }
        EntityReconstructionResolution::Incomplete(handle) => {
            // The remainder is a live continuation. Release the historical fence before waiting
            // for a lane grant that may depend on the primary crossing its live-transition gate.
            if let Some(reconstruction) = historical_reconstruction.as_mut() {
                reconstruction.body_settled();
            }
            drop(historical_reconstruction.take());
            let response = match body_result {
                Some(response) => response?,
                None => body.await?,
            };
            Ok(EntityReconstructionOutcome::Incomplete { response, handle })
        }
    }
}

fn replay_body_failure(
    invocation: &EntityInvocationId,
    error: WorkerExecutorError,
) -> WorkerExecutorError {
    WorkerExecutorError::unexpected_oplog_entry(
        format!("reconstructable entity invocation body for {invocation}"),
        format!("entity replay body failed before its recorded terminal: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::tool::ToolName;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use test_r::test;
    use tokio::sync::oneshot;

    fn no_structural_stall() -> impl Future<Output = Result<OplogIndex, WorkerExecutorError>> {
        std::future::pending()
    }

    fn invocation() -> EntityInvocationId {
        let owner = golem_common::model::OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "owner".to_string(),
            },
        );
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner,
                entity: AgentEntity::Tool(ToolName::try_from("entity").unwrap()),
            },
            OplogIndex::from_u64(2),
        )
        .unwrap()
    }

    #[test]
    async fn cancellation_after_body_completion_does_not_poll_body_twice() {
        let (body_completed, wait_for_body) = oneshot::channel();
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = body_completed.send(());
                Ok::<_, WorkerExecutorError>(7)
            },
            async move {
                let _ = wait_for_body.await;
                Ok(EntityReconstructionResolution::<u64, ()>::Cancelled)
            },
            no_structural_stall(),
            move || abort_flag.store(true, Ordering::Release),
            None::<()>,
        )
        .await
        .unwrap();

        assert!(matches!(result, EntityReconstructionOutcome::Cancelled));
        assert!(aborted.load(Ordering::Acquire));
    }

    #[test]
    async fn cancellation_aborts_and_drains_a_running_body() {
        let (body_cancelled, wait_for_cancellation) = oneshot::channel();
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = wait_for_cancellation.await;
                Err::<u64, _>(WorkerExecutorError::runtime("body was cancelled"))
            },
            async { Ok(EntityReconstructionResolution::<u64, ()>::Cancelled) },
            no_structural_stall(),
            move || {
                abort_flag.store(true, Ordering::Release);
                let _ = body_cancelled.send(());
            },
            None::<()>,
        )
        .await
        .unwrap();

        assert!(matches!(result, EntityReconstructionOutcome::Cancelled));
        assert!(aborted.load(Ordering::Acquire));
    }

    struct ReconstructionFence(Arc<Mutex<Option<oneshot::Sender<()>>>>);

    impl Drop for ReconstructionFence {
        fn drop(&mut self) {
            if let Some(released) = self.0.lock().unwrap().take() {
                let _ = released.send(());
            }
        }
    }

    impl ReconstructionGuard for ReconstructionFence {
        fn body_settled(&mut self) {}
    }

    struct DropProbe(Arc<AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    impl ReconstructionGuard for DropProbe {
        fn body_settled(&mut self) {}
    }

    #[test]
    async fn replay_failure_aborts_and_drains_body_and_releases_fence() {
        let (body_cancelled, wait_for_cancellation) = oneshot::channel();
        let aborted = Arc::new(AtomicBool::new(false));
        let abort_flag = aborted.clone();
        let fence_dropped = Arc::new(AtomicBool::new(false));
        let error = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = wait_for_cancellation.await;
                Err::<u64, _>(WorkerExecutorError::runtime("body was cancelled"))
            },
            async {
                Err::<EntityReconstructionResolution<u64, ()>, _>(WorkerExecutorError::runtime(
                    "replay failed",
                ))
            },
            no_structural_stall(),
            move || {
                abort_flag.store(true, Ordering::Release);
                let _ = body_cancelled.send(());
            },
            Some(DropProbe(fence_dropped.clone())),
        )
        .await
        .expect_err("replay failure must terminate reconstruction");

        assert!(error.to_string().contains("replay failed"));
        assert!(aborted.load(Ordering::Acquire));
        assert!(fence_dropped.load(Ordering::Acquire));
    }

    #[test]
    async fn incomplete_replay_releases_historical_fence_before_waiting_for_body() {
        let (fence_released, wait_for_fence) = oneshot::channel();
        let fence = ReconstructionFence(Arc::new(Mutex::new(Some(fence_released))));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingIncomplete,
                async move {
                    let _ = wait_for_fence.await;
                    Ok::<_, WorkerExecutorError>(9)
                },
                async { Ok(EntityReconstructionResolution::<u64, ()>::Incomplete(())) },
                no_structural_stall(),
                || {},
                Some(fence),
            ),
        )
        .await
        .expect("the live continuation must not deadlock behind its historical fence")
        .unwrap();

        assert!(matches!(
            result,
            EntityReconstructionOutcome::Incomplete {
                response: 9,
                handle: ()
            }
        ));
    }

    #[test]
    async fn replayed_response_divergence_is_permanent() {
        let error = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async { Ok::<_, WorkerExecutorError>(7) },
            async { Ok(EntityReconstructionResolution::<u64, ()>::Replayed(8)) },
            no_structural_stall(),
            || {},
            None::<()>,
        )
        .await
        .expect_err("a reconstructed result must match the recorded outer response");

        assert!(
            error
                .to_string()
                .contains("reconstructed a different result")
        );
    }

    #[test]
    async fn replayed_response_waits_for_and_agrees_with_body() {
        let (body_completed, wait_for_body) = oneshot::channel();
        let result = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = body_completed.send(());
                Ok::<_, WorkerExecutorError>(7)
            },
            async move {
                let _ = wait_for_body.await;
                Ok(EntityReconstructionResolution::<u64, ()>::Replayed(7))
            },
            no_structural_stall(),
            || {},
            None::<()>,
        )
        .await
        .unwrap();

        assert!(matches!(result, EntityReconstructionOutcome::Replayed(7)));
    }

    #[test]
    async fn body_failure_before_replayed_terminal_is_not_hidden() {
        let (body_completed, wait_for_body) = oneshot::channel();
        let fence_dropped = Arc::new(AtomicBool::new(false));
        let error = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingCompleted,
            async move {
                let _ = body_completed.send(());
                Err::<u64, _>(WorkerExecutorError::runtime("body replay failed"))
            },
            async move {
                let _ = wait_for_body.await;
                Ok(EntityReconstructionResolution::<u64, ()>::Replayed(7))
            },
            no_structural_stall(),
            || {},
            Some(DropProbe(fence_dropped.clone())),
        )
        .await
        .expect_err("a replayed terminal must not hide body failure");

        assert!(error.to_string().contains("body replay failed"));
        assert!(fence_dropped.load(Ordering::Acquire));
    }

    #[test]
    async fn completed_body_failure_does_not_wait_for_blocked_replay() {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingCompleted,
                async { Err::<u64, _>(WorkerExecutorError::runtime("body replay failed")) },
                std::future::pending::<
                    Result<EntityReconstructionResolution<u64, ()>, WorkerExecutorError>,
                >(),
                no_structural_stall(),
                || {},
                None::<()>,
            ),
        )
        .await
        .expect("body failure must not wait for an outer terminal blocked by that body")
        .expect_err("completed reconstruction body failure is permanent");

        assert!(error.to_string().contains("body replay failed"));
    }

    #[test]
    async fn completed_body_underconsumption_is_structural_divergence() {
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            coordinate_entity_reconstruction(
                &invocation(),
                InvocationExecutionMode::ReplayingCompleted,
                async { Ok::<_, WorkerExecutorError>(7) },
                std::future::pending::<
                    Result<EntityReconstructionResolution<u64, ()>, WorkerExecutorError>,
                >(),
                async { Ok(OplogIndex::from_u64(3)) },
                || {},
                None::<()>,
            ),
        )
        .await
        .expect("structural divergence must not wait for an unreachable outer terminal")
        .expect_err("underconsumed recorded descendants are permanent divergence");

        assert!(error.to_string().contains("recorded descendant at 3"));
    }

    #[test]
    async fn incomplete_body_failure_remains_a_live_failure() {
        let error = coordinate_entity_reconstruction(
            &invocation(),
            InvocationExecutionMode::ReplayingIncomplete,
            async { Err::<u64, _>(WorkerExecutorError::runtime("live continuation failed")) },
            async { Ok(EntityReconstructionResolution::<u64, ()>::Incomplete(())) },
            no_structural_stall(),
            || {},
            None::<()>,
        )
        .await
        .expect_err("the live continuation failure must propagate");

        let message = error.to_string();
        assert!(message.contains("live continuation failed"));
        assert!(!message.contains("reconstructable entity invocation body"));
    }
}
