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
use crate::durable_host::{
    LiveAuthorizationPermit, authority_snapshot_is_current_at, authorize_effective_surface,
    record_permission_decisions,
};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::sync::atomic::Ordering;
use wasmtime::component::{Accessor, HasData};

/// Inputs checked before a durable host call may write or claim its `Start` record.
///
/// The function type selects read-only and durable-scope behavior, while the fully qualified host
/// function name identifies any read-only violation.
#[derive(Clone, Copy)]
pub(crate) struct DurableCallAdmission<'a> {
    function_type: &'a DurableFunctionType,
    host_function: &'a str,
    invocation_id: Option<DurableInvocationId>,
}

impl<'a> DurableCallAdmission<'a> {
    pub(crate) fn new(function_type: &'a DurableFunctionType, host_function: &'a str) -> Self {
        Self {
            function_type,
            host_function,
            invocation_id: None,
        }
    }

    /// The invocation id of the call, recorded in the durable scope `Start` the call opens.
    pub(crate) fn with_invocation_id(mut self, invocation_id: Option<DurableInvocationId>) -> Self {
        self.invocation_id = invocation_id;
        self
    }

    fn wallet_admission(
        self,
        runtime: &OwnerRuntime,
        snapshotting: bool,
        durable_call_is_live: bool,
        automatic_update_unsettled: bool,
    ) -> WalletAdmission {
        if self.is_wallet_independent_value_read()
            && *runtime == OwnerRuntime::Agent
            && !snapshotting
            && durable_call_is_live
        {
            if automatic_update_unsettled {
                WalletAdmission::FinalizeUpdate
            } else {
                WalletAdmission::Independent
            }
        } else {
            WalletAdmission::Synchronize
        }
    }

    fn is_wallet_independent_value_read(self) -> bool {
        use golem_common::model::oplog::host_functions::HostFunctionName;
        matches!(
            HostFunctionName::from(self.host_function),
            HostFunctionName::WallClockNow
                | HostFunctionName::WallClockResolution
                | HostFunctionName::MonotonicClockNow
                | HostFunctionName::MonotonicClockResolution
                | HostFunctionName::P3SystemClockNow
                | HostFunctionName::P3SystemClockGetResolution
                | HostFunctionName::P3MonotonicClockNow
                | HostFunctionName::P3MonotonicClockGetResolution
                | HostFunctionName::RandomGetRandomBytes
                | HostFunctionName::RandomGetRandomU64
                | HostFunctionName::RandomInsecureGetInsecureRandomBytes
                | HostFunctionName::RandomInsecureGetInsecureRandomU64
                | HostFunctionName::RandomInsecureSeedInsecureSeed
                | HostFunctionName::P3RandomRandomGetRandomBytes
                | HostFunctionName::P3RandomRandomGetRandomU64
                | HostFunctionName::P3RandomInsecureGetInsecureRandomBytes
                | HostFunctionName::P3RandomInsecureGetInsecureRandomU64
                | HostFunctionName::P3RandomInsecureSeedGetInsecureSeed
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WalletAdmission {
    Independent,
    FinalizeUpdate,
    Synchronize,
}

/// The result of successful admission and durable-scope recovery for a host call.
///
/// This carries the exact begin index that must later finish the same durable function. For a
/// scoped function it is the scope's `Start` index; for an unscoped function it is the oplog index
/// immediately before the call.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DurableCallBoundary {
    begin_index: OplogIndex,
}

impl DurableCallBoundary {
    pub(crate) fn from_begin_index(begin_index: OplogIndex) -> Self {
        Self { begin_index }
    }

    pub(crate) fn begin_index(self) -> OplogIndex {
        self.begin_index
    }
}

/// Owns the ordering around a durable host call on [`DurableWorkerCtx`].
///
/// Before writing or claiming `Start`, it checks read-only access, stabilizes wallet and authority
/// state, and then opens or recovers the durable scope. After a terminal record, it closes the
/// scope, commits durability, and only then permits a safe checkpoint. This ordering must not move
/// relative to the host call's `Start` and terminal records.
pub(crate) struct DurableCallCoordinator<'a, Ctx: WorkerCtx> {
    ctx: &'a mut DurableWorkerCtx<Ctx>,
}

impl<'a, Ctx: WorkerCtx> DurableCallCoordinator<'a, Ctx> {
    pub(crate) fn new(ctx: &'a mut DurableWorkerCtx<Ctx>) -> Self {
        Self { ctx }
    }

    pub(crate) async fn admit(
        self,
        admission: DurableCallAdmission<'_>,
    ) -> Result<DurableCallBoundary, WorkerExecutorError> {
        self.check_allowed(admission)?;
        match admission.wallet_admission(
            &self.ctx.runtime,
            self.ctx.state.snapshotting_mode,
            self.ctx.state.durable_call_is_live(),
            self.ctx.state.automatic_update_unsettled,
        ) {
            // A fresh clock/random value needs no authority. Waiting for an accessor's wallet
            // reconciliation here can deadlock: this call owns the Store that accessor needs.
            // Automatic updates still fence target-only history until success is committed.
            WalletAdmission::Independent => {}
            WalletAdmission::FinalizeUpdate => {
                let _boundary = self.ctx.lock_synchronized_card_event_boundary().await?;
                if self.ctx.state.automatic_update_unsettled {
                    return Err(WorkerExecutorError::runtime(
                        "automatic update did not finalize before live host-call admission",
                    ));
                }
            }
            WalletAdmission::Synchronize => {
                self.ctx.synchronize_agent_wallet_at_boundary().await?;
            }
        }
        let begin_index = self
            .ctx
            .begin_function(admission.function_type, admission.invocation_id)
            .await?;
        Ok(DurableCallBoundary::from_begin_index(begin_index))
    }

    pub(crate) async fn admit_with_span(
        self,
        admission: DurableCallAdmission<'_>,
        span_started: golem_common::model::oplog::SpanStarted,
    ) -> Result<(DurableCallBoundary, golem_common::model::oplog::SpanStarted), WorkerExecutorError>
    {
        self.check_allowed(admission)?;
        self.ctx.synchronize_agent_wallet_at_boundary().await?;
        let (begin_index, recorded) = self
            .ctx
            .begin_function_with_span(
                admission.function_type,
                span_started,
                admission.invocation_id,
            )
            .await?;
        Ok((DurableCallBoundary::from_begin_index(begin_index), recorded))
    }

    pub(crate) async fn admit_with_agent_authority(
        self,
        admission: DurableCallAdmission<'_>,
    ) -> Result<(DurableCallBoundary, Option<AuthCtx>), WorkerExecutorError> {
        self.admit_with_agent_authority_capture(admission, |ctx| ctx.agent_auth_ctx())
            .await
    }

    pub(crate) async fn admit_with_agent_authority_capture<T>(
        self,
        admission: DurableCallAdmission<'_>,
        mut capture: impl FnMut(&mut DurableWorkerCtx<Ctx>) -> T,
    ) -> Result<(DurableCallBoundary, Option<T>), WorkerExecutorError> {
        self.check_allowed(admission)?;
        let mut captured = self
            .ctx
            .capture_live_agent_authority_at_boundary(&mut capture)
            .await?;
        let begin_index = self
            .ctx
            .begin_function(admission.function_type, admission.invocation_id)
            .await?;
        if captured.is_none() {
            if self.ctx.state.snapshotting_mode && self.ctx.state.is_replay() {
                // Snapshot loading executes guest code without durable host-call records while
                // replay is positioned at the snapshot. Use the authority reconstructed from the
                // persisted snapshot rather than consulting current card state.
                captured = Some(capture(self.ctx));
            } else if self.ctx.state.is_live() {
                captured = self
                    .ctx
                    .capture_live_agent_authority_at_boundary(&mut capture)
                    .await?;
            }
        }
        Ok((DurableCallBoundary::from_begin_index(begin_index), captured))
    }

    pub(crate) async fn finish(
        self,
        function_type: &DurableFunctionType,
        boundary: DurableCallBoundary,
        forced_commit: bool,
        span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<(), WorkerExecutorError> {
        self.ctx
            .end_function_impl(function_type, boundary.begin_index(), span_finished)
            .await?;
        if !self.ctx.state.snapshotting_mode
            && (function_type == &DurableFunctionType::WriteRemote
                || matches!(function_type, DurableFunctionType::WriteRemoteBatched(_))
                || matches!(
                    function_type,
                    DurableFunctionType::WriteRemoteTransaction(_)
                )
                || forced_commit)
        {
            self.ctx
                .public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                .await?;
            // The status checkpoint is only safe after the durable boundary has committed.
            self.ctx.maybe_mid_invocation_checkpoint().await;
        }
        Ok(())
    }

    fn check_allowed(
        &self,
        admission: DurableCallAdmission<'_>,
    ) -> Result<(), WorkerExecutorError> {
        if durability::is_write_side_effect(admission.function_type)
            && let Err(GolemSpecificWasmTrap::WorkerReadOnlyViolation {
                method,
                host_function,
            }) = DurableWorkerCtx::check_read_only_allows(self.ctx, admission.host_function)
        {
            return Err(WorkerExecutorError::ReadOnlyViolation {
                method,
                host_function,
            });
        }

        Ok(())
    }
}

pub(crate) async fn authorize_live_permissions_at_serialized_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    targets: &[golem_common::model::card::PermissionTarget],
) -> Result<Result<LiveAuthorizationPermit, AuthorizationError>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    enum FastResult {
        OperatorAuthorized,
        Stable(Result<(), AuthorizationError>),
        Slow,
    }

    process_pending_replay_events_access(store, get_ctx).await?;
    let fast_result = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        assert!(
            ctx.state.is_live(),
            "live permission authorization must not run during replay"
        );
        if !ctx.state.replay_applications.is_empty() {
            return FastResult::Slow;
        }
        if ctx.operator_authorizes_current_invocation() {
            return FastResult::OperatorAuthorized;
        }
        let published_generation = ctx
            .state
            .published_authority_generation
            .load(Ordering::Acquire);
        let now = chrono::Utc::now();
        if authority_snapshot_is_current_at(
            ctx.state.authority_initialized,
            ctx.state.card_interest_index.authority_is_open(),
            ctx.state.processed_authority_generation,
            published_generation,
            ctx.state.next_authority_expiration,
            now,
        ) {
            let result = authorize_effective_surface(&ctx.state.agent_effective_surface, targets);
            if ctx.authority_snapshot_is_stable(published_generation) {
                FastResult::Stable(result)
            } else {
                FastResult::Slow
            }
        } else {
            FastResult::Slow
        }
    });
    match fast_result {
        FastResult::OperatorAuthorized => {
            record_permission_decisions(targets, true);
            return Ok(Ok(LiveAuthorizationPermit { _private: () }));
        }
        FastResult::Stable(result) => {
            crate::metrics::wasm::record_agent_permission_authority_fast_path();
            record_permission_decisions(targets, result.is_ok());
            return Ok(result.map(|()| LiveAuthorizationPermit { _private: () }));
        }
        FastResult::Slow => {}
    }

    let started = std::time::Instant::now();
    loop {
        let boundary_guard =
            lock_synchronized_card_event_boundary_access_inner(store, get_ctx, true, true)
                .await?
                .expect("waiting authority boundary always returns a guard");
        let result = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            if ctx.operator_authorizes_current_invocation() {
                return Some(Ok(()));
            }
            let generation = ctx
                .state
                .published_authority_generation
                .load(Ordering::Acquire);
            let result = authorize_effective_surface(&ctx.state.agent_effective_surface, targets);
            // A due cached deadline sent us through synchronization. Recompute it from the
            // synchronized wallet before deciding whether the snapshot is stable.
            ctx.refresh_authority_expiration_deadline();
            if ctx.authority_snapshot_is_stable(generation) {
                ctx.adopt_authority_generation(generation);
                Some(result)
            } else {
                None
            }
        });
        if let Some(result) = result {
            crate::metrics::wasm::record_agent_permission_authority_slow_path(started.elapsed());
            record_permission_decisions(targets, result.is_ok());
            return Ok(result.map(|()| LiveAuthorizationPermit { _private: () }));
        }
        drop(boundary_guard);
    }
}

pub(crate) async fn try_agent_auth_ctx_at_serialized_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<Option<AuthCtx>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let Some(_boundary_guard) =
        lock_synchronized_card_event_boundary_access_inner(store, get_ctx, true, false).await?
    else {
        return Ok(None);
    };
    Ok(Some(store.with(|mut access| {
        get_ctx(access.data_mut()).agent_auth_ctx()
    })))
}

pub(crate) async fn agent_auth_ctx_at_serialized_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<AuthCtx, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let _boundary_guard =
        lock_synchronized_card_event_boundary_access_inner(store, get_ctx, true, true)
            .await?
            .expect("waiting authority boundary always returns a guard");
    Ok(store.with(|mut access| get_ctx(access.data_mut()).agent_auth_ctx()))
}

pub(crate) async fn synchronize_live_agent_authority_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if !store.with(|mut access| get_ctx(access.data_mut()).state.is_live()) {
        return Ok(());
    }
    loop {
        let boundary_guard =
            lock_synchronized_card_event_boundary_access_inner(store, get_ctx, true, true)
                .await?
                .expect("waiting authority boundary always returns a guard");
        let stable = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            let generation = ctx
                .state
                .published_authority_generation
                .load(Ordering::Acquire);
            ctx.refresh_authority_expiration_deadline();
            if ctx.authority_snapshot_is_stable(generation) {
                ctx.adopt_authority_generation(generation);
                true
            } else {
                false
            }
        });
        drop(boundary_guard);
        if stable {
            return Ok(());
        }
    }
}

pub(super) async fn lock_synchronized_card_event_boundary_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    Ok(
        lock_synchronized_card_event_boundary_access_inner(store, get_ctx, false, true)
            .await?
            .expect("unrestricted card boundary always returns a guard"),
    )
}

async fn lock_synchronized_card_event_boundary_access_inner<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    requires_agent_authority: bool,
    wait_for_authority: bool,
) -> Result<Option<Option<tokio::sync::OwnedMutexGuard<()>>>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    loop {
        process_pending_replay_events_access(store, get_ctx).await?;
        let live_inputs = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            ctx.state.is_live().then(|| {
                (
                    ctx.state.owner_wallet.clone(),
                    ctx.public_state.worker().clone(),
                    ctx.authority_scope_inputs(),
                )
            })
        });
        if let Some((owner, worker, inputs)) = live_inputs {
            let work = owner.synchronize(
                worker.clone(),
                inputs,
                requires_agent_authority,
                wait_for_authority,
            );
            let (send, receive) = tokio::sync::oneshot::channel();
            worker.tasks.spawn(async move {
                let _ = send.send(work.await);
            });
            let Some(observation) =
                receive
                    .await
                    .map_err(|_| WorkerExecutorError::Interrupted {
                        kind: InterruptKind::Jump,
                    })??
            else {
                return Ok(None);
            };
            let accepted = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                let accepted = ctx.apply_owner_authority_snapshot(observation)?;
                Ok::<_, WorkerExecutorError>(
                    accepted
                        && (!requires_agent_authority
                            || ctx.state.card_interest_index.authority_is_open()),
                )
            })?;
            if accepted {
                return Ok(Some(None));
            }
            continue;
        }
        return Ok(Some(None));
    }
}

pub(super) async fn process_pending_replay_events_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    loop {
        let application = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            super::replay_application::reserve(ctx)?;
            Ok::<_, WorkerExecutorError>(ctx.state.replay_applications.front().cloned())
        })?;
        let Some(application) = application else {
            return Ok(());
        };
        let prepared = application.result.clone().await?;
        store.with(|mut access| {
            super::replay_application::apply(get_ctx(access.data_mut()), &application, &prepared);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{DurableCallAdmission, DurableFunctionType, OwnerRuntime, WalletAdmission};
    use golem_common::model::entity::{AgentEntity, ToolMiddlewareName};
    use test_r::test;

    #[test]
    fn wallet_independent_reads_still_fence_updates_and_replay() {
        let clock = DurableCallAdmission::new(&DurableFunctionType::ReadLocal, "wall_clock::now");
        assert_eq!(
            clock.wallet_admission(&OwnerRuntime::Agent, false, true, false),
            WalletAdmission::Independent
        );
        assert_eq!(
            clock.wallet_admission(&OwnerRuntime::Agent, false, true, true),
            WalletAdmission::FinalizeUpdate
        );
        let entity = OwnerRuntime::Entity(AgentEntity::ToolMiddleware(
            ToolMiddlewareName::try_from("middleware").unwrap(),
        ));
        for (runtime, snapshotting, live) in [
            (OwnerRuntime::Agent, false, false),
            (OwnerRuntime::Agent, true, true),
            (entity, false, true),
        ] {
            for unsettled in [false, true] {
                assert_eq!(
                    clock.wallet_admission(&runtime, snapshotting, live, unsettled),
                    WalletAdmission::Synchronize
                );
            }
        }
        let environment = DurableCallAdmission::new(
            &DurableFunctionType::ReadLocal,
            "cli::environment::get_environment",
        );
        assert_eq!(
            environment.wallet_admission(&OwnerRuntime::Agent, false, true, false),
            WalletAdmission::Synchronize
        );
    }
}
