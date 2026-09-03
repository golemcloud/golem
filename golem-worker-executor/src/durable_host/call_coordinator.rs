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
}

impl<'a> DurableCallAdmission<'a> {
    pub(crate) fn new(function_type: &'a DurableFunctionType, host_function: &'a str) -> Self {
        Self {
            function_type,
            host_function,
        }
    }
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
        self.ctx.synchronize_agent_wallet_at_boundary().await?;
        let begin_index = self.ctx.begin_function(admission.function_type).await?;
        Ok(DurableCallBoundary::from_begin_index(begin_index))
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
        let begin_index = self.ctx.begin_function(admission.function_type).await?;
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
    ) -> Result<(), WorkerExecutorError> {
        self.ctx
            .end_function(function_type, boundary.begin_index())
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
                .await;
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

    let fast_result = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        assert!(
            ctx.state.is_live(),
            "live permission authorization must not run during replay"
        );
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

pub(super) async fn lock_synchronized_card_event_boundary_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<tokio::sync::OwnedMutexGuard<()>, WorkerExecutorError>
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
) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let boundary_lock = store.with(|mut access| {
        get_ctx(access.data_mut())
            .state
            .card_event_boundary_lock
            .clone()
    });
    loop {
        let boundary_guard = boundary_lock.clone().lock_owned().await;
        let (authority_checked, authority_open, card_interest_index) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                requires_agent_authority && ctx.state.is_live(),
                ctx.state.card_interest_index.authority_is_open(),
                ctx.state.card_interest_index.clone(),
            )
        });
        if authority_checked && !authority_open {
            drop(boundary_guard);
            if !wait_for_authority {
                return Ok(None);
            }
            card_interest_index.wait_until_authority_open().await;
            continue;
        }
        synchronize_agent_wallet_at_boundary_access(store, get_ctx).await?;
        if requires_agent_authority && !authority_checked {
            let authority_open_after_replay = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                !ctx.state.is_live() || ctx.state.card_interest_index.authority_is_open()
            });
            if !authority_open_after_replay {
                drop(boundary_guard);
                if !wait_for_authority {
                    return Ok(None);
                }
                card_interest_index.wait_until_authority_open().await;
                continue;
            }
        }
        let has_pending_source_transfer = store.with(|mut access| {
            get_ctx(access.data_mut())
                .state
                .card_event_boundary_scan
                .as_ref()
                .is_some_and(|scan| {
                    scan.pending.iter().any(|pending| {
                        matches!(&pending.event, QueuedCardEvent::TransferStarted(_))
                    })
                })
        });
        let retries = if has_pending_source_transfer {
            prepare_pending_source_card_transfers_access(store, get_ctx).await?
        } else {
            Vec::new()
        };
        if retries.is_empty() {
            return Ok(Some(boundary_guard));
        }
        // Target delivery takes the target worker's boundary lock. Release the source lock while
        // delivering, then reconcile again before returning the guard that protects live Start.
        drop(boundary_guard);
        complete_pending_source_card_transfers_access(store, get_ctx, retries).await?;
    }
}

async fn synchronize_agent_wallet_at_boundary_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    process_pending_replay_events_access(store, get_ctx).await?;
    drain_card_revocations_and_expiry_access(store, get_ctx).await
}

async fn drain_card_revocations_and_expiry_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let is_live = store.with(|mut access| get_ctx(access.data_mut()).state.is_live());
    if !is_live {
        return Ok(());
    }

    loop {
        let pending_events = crate::durable_host::next_drainable_card_events(
            pending_card_events_at_boundary_access(store, get_ctx).await?,
        );
        let Some(pending_event) = pending_events.first() else {
            break;
        };
        match &pending_event.event {
            QueuedCardEvent::Revoke(_) => {
                let card_ids = pending_events
                    .into_iter()
                    .filter_map(|pending_event| match pending_event.event {
                        QueuedCardEvent::Revoke(event) => Some(event.card_id),
                        QueuedCardEvent::Install(_)
                        | QueuedCardEvent::TransferStarted(_)
                        | QueuedCardEvent::TransferReceived(_) => None,
                    })
                    .collect::<Vec<_>>();
                apply_card_revocations_access(store, get_ctx, card_ids).await?;
            }
            QueuedCardEvent::Install(event) => {
                let Some(card) = event.card.clone() else {
                    return Err(WorkerExecutorError::runtime(
                        "queued card install is missing card payload",
                    ));
                };
                apply_card_install_access(store, get_ctx, pending_event.oplog_index, card).await?;
            }
            QueuedCardEvent::TransferReceived(event) => {
                let Some(card) = event.card.clone() else {
                    return Err(WorkerExecutorError::runtime(
                        "received card transfer is missing card payload",
                    ));
                };
                apply_received_card_transfer_access(
                    store,
                    get_ctx,
                    pending_event.oplog_index,
                    event.transfer_id,
                    event.source_card_id,
                    card,
                )
                .await?;
            }
            QueuedCardEvent::TransferStarted(_) => {
                unreachable!("filtered above")
            }
        }
    }
    remove_expired_wallet_cards_access(store, get_ctx).await?;

    let expired_scope_root_ids = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        crate::durable_host::expired_wallet_card_ids_at(
            &ctx.state.invocation_scope_root_cards,
            chrono::Utc::now(),
        )
    });
    apply_card_revocations_access(store, get_ctx, expired_scope_root_ids).await
}

async fn pending_card_events_at_boundary_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<Vec<golem_common::model::PendingCardEventRef>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (worker, oplog) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (ctx.public_state.worker().clone(), ctx.state.oplog.clone())
    });
    let status = worker.get_non_detached_last_known_status().await;
    let current_idx = oplog.current_oplog_index().await;
    let unread_range = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        match &mut ctx.state.card_event_boundary_scan {
            Some(scan) => {
                scan.synchronize(status.oplog_idx, &status.pending_card_events, current_idx)
            }
            None => {
                ctx.state.card_event_boundary_scan =
                    Some(crate::durable_host::CardEventBoundaryScan::new(
                        status.oplog_idx,
                        status.pending_card_events,
                    ));
            }
        }
        ctx.state
            .card_event_boundary_scan
            .as_ref()
            .expect("card event boundary scan must be initialized")
            .unread_range(current_idx)
    });

    if let Some((start, count)) = unread_range {
        let entries = oplog.read_exact(start, count).await;
        store.with(|mut access| {
            get_ctx(access.data_mut())
                .state
                .card_event_boundary_scan
                .as_mut()
                .expect("card event boundary scan must be initialized")
                .fold_through(current_idx, &entries);
        });
    }

    Ok(store.with(|mut access| {
        get_ctx(access.data_mut())
            .state
            .card_event_boundary_scan
            .as_ref()
            .expect("card event boundary scan must be initialized")
            .pending
            .clone()
    }))
}

async fn admit_card_to_wallet_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    card: &golem_common::model::card::StoredCard,
) -> Result<Result<u64, CardInstallFailure>, WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let card_id = card.card_id();
    let (owned_agent_id, mut candidate_card_ids, interest_index, card_service) =
        store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.owned_agent_id.clone(),
                ctx.interested_card_ids(),
                ctx.state.card_interest_index.clone(),
                ctx.state.card_service.clone(),
            )
        });
    if !candidate_card_ids.contains(&card_id) {
        candidate_card_ids.push(card_id);
    }
    interest_index
        .set_card_interest(owned_agent_id, &candidate_card_ids)
        .await;

    let card_state = card_service
        .check_cards(vec![card_id])
        .await?
        .remove(&card_id);
    let failure = match card_state {
        Some(CardState::Live(registered_card)) if registered_card.as_ref() == card => None,
        Some(CardState::Live(_)) => Some(CardInstallFailure::NotPermitted),
        Some(CardState::Revoked) => Some(CardInstallFailure::CardRevoked),
        Some(CardState::Unknown) | None => Some(CardInstallFailure::NotFound),
    };

    let (result, owned_agent_id, interested_card_ids, interest_index) =
        store.with(|mut access| -> Result<_, WorkerExecutorError> {
            let ctx = get_ctx(access.data_mut());
            let result = if let Some(failure) = failure {
                Err(failure)
            } else {
                if crate::durable_host::add_wallet_card(
                    &mut ctx.state.agent_wallet_cards,
                    &mut ctx.state.wallet_generation,
                    card.clone(),
                )? {
                    ctx.rederive_agent_effective_surface_from_wallet();
                }
                Ok(ctx.state.wallet_generation)
            };
            Ok((
                result,
                ctx.owned_agent_id.clone(),
                ctx.interested_card_ids(),
                ctx.state.card_interest_index.clone(),
            ))
        })?;
    interest_index
        .set_card_interest(owned_agent_id, &interested_card_ids)
        .await;
    Ok(result)
}

async fn apply_card_install_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    queued_event_index: OplogIndex,
    card: golem_common::model::card::StoredCard,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let card_id = card.card_id();
    let result = admit_card_to_wallet_access(store, get_ctx, &card).await?;
    let worker = store.with(|mut access| get_ctx(access.data_mut()).public_state.worker().clone());
    let entry = match result {
        Ok(wallet_generation) => {
            OplogEntry::card_installed(Some(queued_event_index), card, Some(wallet_generation))
        }
        Err(reason) => OplogEntry::card_install_failed(queued_event_index, card_id, reason),
    };
    worker.add_and_commit_oplog(entry).await;
    Ok(())
}

async fn apply_received_card_transfer_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    queued_event_index: OplogIndex,
    transfer_id: uuid::Uuid,
    source_card_id: Option<golem_common::model::card::CardId>,
    card: golem_common::model::card::StoredCard,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let card_id = card.card_id();
    let result = admit_card_to_wallet_access(store, get_ctx, &card).await?;
    let (agent_id, worker) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.owned_agent_id.agent_id.clone(),
            ctx.public_state.worker().clone(),
        )
    });
    let entry = match result {
        Ok(wallet_generation) => OplogEntry::card_transferred(
            transfer_id,
            source_card_id,
            card_id,
            golem_common::model::card::CardHolder::Agent(
                golem_common::model::card::AgentCardHolder { agent_id },
            ),
            card,
            Some(wallet_generation),
        ),
        Err(reason) => OplogEntry::card_install_failed(queued_event_index, card_id, reason),
    };
    worker.add_and_commit_oplog(entry).await;
    Ok(())
}

async fn remove_expired_wallet_cards_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (expired_card_generations, owned_agent_id, interested_card_ids, interest_index, worker) =
        store.with(|mut access| -> Result<_, WorkerExecutorError> {
            let ctx = get_ctx(access.data_mut());
            let expired_card_ids = crate::durable_host::expired_wallet_card_ids_at(
                &ctx.state.agent_wallet_cards,
                chrono::Utc::now(),
            );
            let mut expired_card_generations = Vec::with_capacity(expired_card_ids.len());
            for card_id in expired_card_ids {
                if crate::durable_host::remove_wallet_card(
                    &mut ctx.state.agent_wallet_cards,
                    &mut ctx.state.wallet_generation,
                    card_id,
                )? {
                    expired_card_generations.push((card_id, ctx.state.wallet_generation));
                }
            }
            if !expired_card_generations.is_empty() {
                ctx.rederive_agent_effective_surface_from_wallet();
            }
            Ok((
                expired_card_generations,
                ctx.owned_agent_id.clone(),
                ctx.interested_card_ids(),
                ctx.state.card_interest_index.clone(),
                ctx.public_state.worker().clone(),
            ))
        })?;

    if expired_card_generations.is_empty() {
        return Ok(());
    }
    interest_index
        .set_card_interest(owned_agent_id, &interested_card_ids)
        .await;
    for (card_id, wallet_generation) in expired_card_generations {
        worker
            .add_and_commit_oplog(OplogEntry::card_expired(card_id, Some(wallet_generation)))
            .await;
    }
    Ok(())
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
    let replay_state =
        store.with(|mut access| get_ctx(access.data_mut()).state.replay_state.clone());
    let replay_events = replay_state.take_new_replay_events();
    for event in replay_events {
        match event {
            crate::durable_host::replay_state::ReplayEvent::ForkReplayed { new_phantom_id } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    ctx.state.current_phantom_id = Some(new_phantom_id);
                });
            }
            crate::durable_host::replay_state::ReplayEvent::UpdateReplayed { new_revision } => {
                tracing::debug!(
                    "Updating worker state to component metadata revision {new_revision}"
                );
                update_state_to_new_component_revision_access(store, get_ctx, new_revision).await?;
            }
            crate::durable_host::replay_state::ReplayEvent::InvocationWalletPinned {
                wallet_pin,
            } => {
                store.with(|mut access| -> Result<(), WorkerExecutorError> {
                    let ctx = get_ctx(access.data_mut());
                    if crate::durable_host::apply_invocation_wallet_pin(
                        &mut ctx.state.agent_wallet_cards,
                        ctx.state.wallet_id_hash,
                        &mut ctx.state.wallet_generation,
                        wallet_pin,
                    )? {
                        ctx.rederive_agent_effective_surface_from_wallet();
                    }
                    Ok(())
                })?;
            }
            crate::durable_host::replay_state::ReplayEvent::CardInstalled {
                card,
                wallet_generation,
            } => {
                store.with(|mut access| -> Result<(), WorkerExecutorError> {
                    let ctx = get_ctx(access.data_mut());
                    let card_id = card.card_id();
                    tracing::debug!(card_id = %card_id, "Applying replayed card installation");
                    if crate::durable_host::add_wallet_card(
                        &mut ctx.state.agent_wallet_cards,
                        &mut ctx.state.wallet_generation,
                        card,
                    )? {
                        ctx.rederive_agent_effective_surface_from_wallet();
                    }
                    crate::durable_host::adopt_recorded_wallet_generation(
                        &mut ctx.state.wallet_generation,
                        wallet_generation,
                    )
                })?;
            }
            crate::durable_host::replay_state::ReplayEvent::CardDerived {
                wallet_generation,
                ..
            } => {
                store.with(|mut access| {
                    let ctx = get_ctx(access.data_mut());
                    crate::durable_host::adopt_recorded_wallet_generation(
                        &mut ctx.state.wallet_generation,
                        wallet_generation,
                    )
                })?;
            }
            crate::durable_host::replay_state::ReplayEvent::CardTransferStarted {
                card_id,
                source_holder,
                source_wallet_generation,
                ..
            } => {
                store.with(|mut access| -> Result<(), WorkerExecutorError> {
                    let ctx = get_ctx(access.data_mut());
                    if source_holder.as_ref().is_none_or(|source_holder| {
                        crate::durable_host::card_holder_is_agent(
                            source_holder,
                            &ctx.owned_agent_id.agent_id,
                        )
                    }) && crate::durable_host::transfer_started_removes_source_membership(
                        ctx.state.agent_wallet_cards.get(&card_id),
                        &source_holder,
                        &ctx.owned_agent_id.agent_id,
                    ) && crate::durable_host::remove_wallet_card(
                        &mut ctx.state.agent_wallet_cards,
                        &mut ctx.state.wallet_generation,
                        card_id,
                    )? {
                        ctx.rederive_agent_effective_surface_from_wallet();
                    }
                    crate::durable_host::adopt_recorded_wallet_generation(
                        &mut ctx.state.wallet_generation,
                        source_wallet_generation,
                    )
                })?;
            }
            crate::durable_host::replay_state::ReplayEvent::CardTransferred {
                target_holder,
                card,
                target_wallet_generation,
                ..
            } => {
                store.with(|mut access| -> Result<(), WorkerExecutorError> {
                    let ctx = get_ctx(access.data_mut());
                    if crate::durable_host::card_holder_is_agent(
                        &target_holder,
                        &ctx.owned_agent_id.agent_id,
                    ) && crate::durable_host::add_wallet_card(
                        &mut ctx.state.agent_wallet_cards,
                        &mut ctx.state.wallet_generation,
                        card,
                    )? {
                        ctx.rederive_agent_effective_surface_from_wallet();
                    }
                    crate::durable_host::adopt_recorded_wallet_generation(
                        &mut ctx.state.wallet_generation,
                        target_wallet_generation,
                    )
                })?;
            }
            crate::durable_host::replay_state::ReplayEvent::CardTransferConfirmed { .. } => {}
            crate::durable_host::replay_state::ReplayEvent::CardRevokedCascade {
                card_ids,
                local_wallet_generation,
            } => {
                store.with(|mut access| -> Result<(), WorkerExecutorError> {
                    let ctx = get_ctx(access.data_mut());
                    let wallet_changed = crate::durable_host::remove_wallet_cards(
                        &mut ctx.state.agent_wallet_cards,
                        &mut ctx.state.wallet_generation,
                        &card_ids,
                    )?;
                    let scope_changed = ctx.clear_invocation_scope_if_roots_include(&card_ids);
                    if wallet_changed || scope_changed {
                        ctx.rederive_agent_effective_surface_from_wallet();
                    }
                    crate::durable_host::adopt_recorded_wallet_generation(
                        &mut ctx.state.wallet_generation,
                        local_wallet_generation,
                    )
                })?;
            }
            crate::durable_host::replay_state::ReplayEvent::CardRevoked {
                card_id,
                wallet_generation,
            }
            | crate::durable_host::replay_state::ReplayEvent::CardExpired {
                card_id,
                wallet_generation,
            } => {
                store.with(|mut access| -> Result<(), WorkerExecutorError> {
                    let ctx = get_ctx(access.data_mut());
                    let wallet_changed = crate::durable_host::remove_wallet_card(
                        &mut ctx.state.agent_wallet_cards,
                        &mut ctx.state.wallet_generation,
                        card_id,
                    )?;
                    let scope_changed = ctx.clear_invocation_scope_if_roots_include(&[card_id]);
                    if wallet_changed || scope_changed {
                        ctx.rederive_agent_effective_surface_from_wallet();
                    }
                    crate::durable_host::adopt_recorded_wallet_generation(
                        &mut ctx.state.wallet_generation,
                        wallet_generation,
                    )
                })?;
            }
            crate::durable_host::replay_state::ReplayEvent::ReplayFinished => {
                tracing::debug!("Replaying oplog finished");
                finalize_pending_automatic_update_access(store, get_ctx).await?;
                check_post_replay_wallet_liveness_access(store, get_ctx).await?;
            }
        }
    }
    Ok(())
}

async fn check_post_replay_wallet_liveness_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (
        owned_agent_id,
        interested_card_ids,
        invocation_scope_card,
        interest_index,
        card_service,
        worker,
    ) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.owned_agent_id.clone(),
            ctx.interested_card_ids(),
            ctx.state.invocation_scope_card.clone(),
            ctx.state.card_interest_index.clone(),
            ctx.state.card_service.clone(),
            ctx.public_state.worker().clone(),
        )
    });

    interest_index
        .set_card_interest(owned_agent_id, &interested_card_ids)
        .await;
    if interested_card_ids.is_empty() {
        return Ok(());
    }

    let card_states = card_service.check_cards(interested_card_ids).await?;
    let live_scope_root_cards = crate::durable_host::live_scope_root_cards_from_states(
        invocation_scope_card.as_ref(),
        &card_states,
    )?;
    store.with(|mut access| {
        get_ctx(access.data_mut()).state.invocation_scope_root_cards = live_scope_root_cards;
    });
    let revoked_card_ids = card_states
        .into_iter()
        .filter_map(|(card_id, state)| (state == CardState::Revoked).then_some(card_id))
        .collect::<Vec<_>>();
    worker
        .queue_card_revocations_locked(&revoked_card_ids)
        .await;
    Ok(())
}

async fn prepare_pending_source_card_transfers_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<
    Vec<crate::durable_host::permissions::PendingSourceCardTransferRetry>,
    WorkerExecutorError,
>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (is_live, oplog, source_agent_id) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.state.is_live(),
            ctx.state.oplog.clone(),
            ctx.owned_agent_id.agent_id.clone(),
        )
    });
    if !is_live {
        return Ok(Vec::new());
    }

    let pending_events = pending_card_events_at_boundary_access(store, get_ctx).await?;
    let mut retries = Vec::new();
    let mut first_error = None;
    for pending in pending_events {
        if let QueuedCardEvent::TransferStarted(transfer) = &pending.event {
            let retry =
                crate::durable_host::permissions::prepare_pending_source_card_transfer_retry(
                    oplog.as_ref(),
                    &source_agent_id,
                    &pending,
                    transfer,
                )
                .await;
            match retry {
                Ok(Some(mut retry)) => {
                    match ensure_pending_source_card_transfer_started_access(store, get_ctx, &retry)
                        .await
                    {
                        Ok(()) => {
                            retry.started = true;
                            retries.push(retry);
                        }
                        Err(error) if first_error.is_none() => first_error = Some(error),
                        Err(_) => {}
                    }
                }
                Ok(None) => {}
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(retries),
    }
}

async fn ensure_pending_source_card_transfer_started_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    retry: &crate::durable_host::permissions::PendingSourceCardTransferRetry,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    if retry.started {
        return Ok(());
    }

    let (worker, source_agent_id) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.public_state.worker().clone(),
            ctx.owned_agent_id.agent_id.clone(),
        )
    });
    let target_holder =
        golem_common::model::card::CardHolder::Agent(golem_common::model::card::AgentCardHolder {
            agent_id: retry.target_agent_id.clone(),
        });

    let refresh_interest = store.with(|mut access| -> Result<Option<_>, WorkerExecutorError> {
        let ctx = get_ctx(access.data_mut());
        let changed = retry.remove_source_membership
            && crate::durable_host::remove_wallet_card(
                &mut ctx.state.agent_wallet_cards,
                &mut ctx.state.wallet_generation,
                retry.source_card_id,
            )?;
        if changed {
            ctx.rederive_agent_effective_surface_from_wallet();
            Ok(Some((
                ctx.owned_agent_id.clone(),
                ctx.interested_card_ids(),
                ctx.state.card_interest_index.clone(),
            )))
        } else {
            Ok(None)
        }
    })?;
    if let Some((owned_agent_id, interested_card_ids, interest_index)) = refresh_interest {
        interest_index
            .set_card_interest(owned_agent_id, &interested_card_ids)
            .await;
    }

    worker
        .add_and_commit_oplog(OplogEntry::card_transfer_started(
            retry.transfer_id,
            retry.source_card_id,
            Some(golem_common::model::card::CardHolder::Agent(
                golem_common::model::card::AgentCardHolder {
                    agent_id: source_agent_id,
                },
            )),
            target_holder,
            store.with(|mut access| Some(get_ctx(access.data_mut()).state.wallet_generation)),
        ))
        .await;

    Ok(())
}

async fn complete_pending_source_card_transfers_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    retries: Vec<crate::durable_host::permissions::PendingSourceCardTransferRetry>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    for retry in retries {
        complete_pending_source_card_transfer_access(store, get_ctx, retry).await?;
    }

    Ok(())
}

async fn complete_pending_source_card_transfer_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    retry: crate::durable_host::permissions::PendingSourceCardTransferRetry,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let (worker, worker_proxy, environment_id) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (
            ctx.public_state.worker().clone(),
            ctx.worker_proxy().clone(),
            ctx.owned_agent_id.environment_id,
        )
    });
    let target_holder =
        golem_common::model::card::CardHolder::Agent(golem_common::model::card::AgentCardHolder {
            agent_id: retry.target_agent_id.clone(),
        });

    worker_proxy
        .deliver_card_transfer(
            &retry.target_agent_id,
            environment_id,
            retry.transfer_id,
            retry.source_card_id,
            &retry.installed_card,
        )
        .await
        .map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "permission-card transfer delivery failed: {error}"
            ))
        })?;

    let boundary_lock = store.with(|mut access| {
        get_ctx(access.data_mut())
            .state
            .card_event_boundary_lock
            .clone()
    });
    let _boundary_guard = boundary_lock.lock().await;
    let (oplog, source_agent_id) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (ctx.state.oplog.clone(), ctx.owned_agent_id.agent_id.clone())
    });
    if retry.is_confirmed(oplog.as_ref(), &source_agent_id).await? {
        return Ok(());
    }
    worker
        .add_and_commit_oplog(OplogEntry::card_transfer_confirmed(
            retry.transfer_id,
            retry.source_card_id,
            retry.installed_card.card_id(),
            target_holder,
        ))
        .await;
    Ok(())
}

async fn apply_card_revocations_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    mut card_ids: Vec<golem_common::model::card::CardId>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    card_ids.sort_unstable();
    card_ids.dedup();
    if card_ids.is_empty() {
        return Ok(());
    }

    let (
        owned_agent_id,
        interested_card_ids,
        interest_index,
        worker,
        affected_wallets,
        wallet_generation,
    ) = store.with(|mut access| -> Result<_, WorkerExecutorError> {
        let ctx = get_ctx(access.data_mut());
        let wallet_changed = crate::durable_host::remove_wallet_cards(
            &mut ctx.state.agent_wallet_cards,
            &mut ctx.state.wallet_generation,
            &card_ids,
        )?;
        let scope_changed = ctx.clear_invocation_scope_if_roots_include(&card_ids);
        if wallet_changed || scope_changed {
            ctx.rederive_agent_effective_surface_from_wallet();
        }
        let affected_wallets = if wallet_changed {
            vec![golem_common::model::card::CardHolder::Agent(
                golem_common::model::card::AgentCardHolder {
                    agent_id: ctx.owned_agent_id.agent_id.clone(),
                },
            )]
        } else {
            Vec::new()
        };
        Ok((
            ctx.owned_agent_id.clone(),
            ctx.interested_card_ids(),
            ctx.state.card_interest_index.clone(),
            ctx.public_state.worker().clone(),
            affected_wallets,
            ctx.state.wallet_generation,
        ))
    })?;

    interest_index
        .set_card_interest(owned_agent_id, &interested_card_ids)
        .await;
    worker
        .add_and_commit_oplog(OplogEntry::CardRevokedCascade {
            timestamp: Timestamp::now_utc(),
            revoked_card_ids: card_ids,
            affected_wallets,
            local_wallet_generation: Some(wallet_generation),
        })
        .await;
    Ok(())
}

struct AccessRevisionUpdateInputs {
    component_service: Arc<dyn ComponentService>,
    file_loader: Arc<FileLoader>,
    filesystem_generation_handle: FilesystemGenerationHandle,
    owned_agent_id: golem_common::model::OwnedAgentId,
    agent_id: Option<ParsedAgentId>,
    initial_agent_config: Vec<golem_common::model::worker::TypedAgentConfigEntry>,
    current_revision: ComponentRevision,
}

type AccessRevisionUpdateAgentState = (
    HashMap<Vec<String>, golem_common::schema::TypedSchemaValue>,
    BTreeMap<golem_common::model::card::CardId, golem_common::model::card::StoredCard>,
);

struct AccessRevisionUpdate {
    metadata: Component,
    agent_state: Option<AccessRevisionUpdateAgentState>,
}

async fn finalize_pending_automatic_update_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let pending_update = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let pending_update = ctx
            .state
            .pending_update
            .try_lock()
            .map_err(|_| {
                WorkerExecutorError::runtime(
                    "p3 accessor durable call path cannot inspect pending component update state",
                )
            })?
            .take();
        Ok::<_, WorkerExecutorError>(pending_update)
    });

    let pending_update = if let Some(pending_update) = pending_update? {
        pending_update
    } else {
        return Ok(());
    };

    match pending_update.description {
        UpdateDescription::Automatic { target_revision } => {
            tracing::debug!("Finalizing pending automatic update");
            if let Err(error) =
                update_state_to_new_component_revision_access(store, get_ctx, target_revision).await
            {
                let stringified_error = format!("Applying worker update failed: {error}");
                record_worker_update_failed_access(
                    store,
                    get_ctx,
                    target_revision,
                    stringified_error,
                )
                .await?;
                return Err(error);
            }

            let (component_size, active_plugins) = store.with(|mut access| {
                let ctx = get_ctx(access.data_mut());
                (
                    ctx.state.component_metadata.component_size,
                    HashSet::from_iter({
                        ctx.agent_type_provision_config()
                            .map(|c| c.plugins.as_slice())
                            .unwrap_or_default()
                            .iter()
                            .map(|installation| installation.environment_plugin_grant_id)
                    }),
                )
            });
            record_worker_update_succeeded_access(
                store,
                get_ctx,
                target_revision,
                component_size,
                active_plugins,
            )
            .await?;
            tracing::debug!("Finalizing automatic update to revision {target_revision}");
            Ok(())
        }
        _ => Err(WorkerExecutorError::runtime(
            "pending replay event finalization expected an automatic update description",
        )),
    }
}

async fn update_state_to_new_component_revision_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    new_revision: ComponentRevision,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let inputs = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        AccessRevisionUpdateInputs {
            component_service: ctx.state.component_service.clone(),
            file_loader: ctx.state.file_loader.clone(),
            filesystem_generation_handle: ctx.filesystem_generation_handle(),
            owned_agent_id: ctx.owned_agent_id.clone(),
            agent_id: ctx.state.agent_id.clone(),
            initial_agent_config: ctx.state.initial_agent_config.clone(),
            current_revision: ctx.state.component_metadata.revision,
        }
    });

    if new_revision <= inputs.current_revision {
        tracing::debug!("Update {new_revision} was already applied, skipping");
        return Ok(());
    }

    let update = prepare_revision_update_access(&inputs, new_revision).await?;
    store.with(|mut access| -> Result<(), WorkerExecutorError> {
        let ctx = get_ctx(access.data_mut());
        apply_revision_update_access(ctx, update)
    })
}

async fn prepare_revision_update_access(
    inputs: &AccessRevisionUpdateInputs,
    new_revision: ComponentRevision,
) -> Result<AccessRevisionUpdate, WorkerExecutorError> {
    let metadata = inputs
        .component_service
        .get_metadata(inputs.owned_agent_id.component_id(), Some(new_revision))
        .await?;

    let provision_config = inputs.agent_id.as_ref().and_then(|agent_id| {
        metadata
            .metadata
            .agent_type_provision_configs()
            .get(&agent_id.agent_type)
            .cloned()
    });

    let agent_state = if let Some(agent_id) = &inputs.agent_id {
        let agent_type = metadata
            .metadata
            .find_agent_type_by_name_ref(&agent_id.agent_type)
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "Agent type {} not found in updated agent metadata",
                    agent_id.agent_type
                ))
            })?;

        let updated_agent_config = effective_agent_config(
            inputs.initial_agent_config.clone(),
            provision_config
                .as_ref()
                .map(|c| c.config.clone())
                .unwrap_or_default(),
        )?;
        validate_agent_config(&updated_agent_config, agent_type)?;

        let initial_card = super::agent_initial_card_from_component_metadata(&metadata, agent_id)?;
        let initial_wallet_cards = BTreeMap::from([(initial_card.card_id(), initial_card)]);
        Some((updated_agent_config, initial_wallet_cards))
    } else {
        None
    };

    crate::services::agent_filesystem::update_initial_files(
        &inputs.filesystem_generation_handle,
        Arc::clone(&inputs.file_loader),
        inputs.owned_agent_id.environment_id,
        provision_config
            .as_ref()
            .map(|c| c.files.clone())
            .unwrap_or_default(),
    )
    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    .await
    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;

    Ok(AccessRevisionUpdate {
        metadata,
        agent_state,
    })
}

fn apply_revision_update_access<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    update: AccessRevisionUpdate,
) -> Result<(), WorkerExecutorError> {
    ctx.state.component_metadata = update.metadata;

    if let Some((agent_config, initial_wallet_cards)) = update.agent_state {
        ctx.state.agent_config = agent_config;
        ctx.state.cached_agent_config_retry_policies = None;
        crate::durable_host::replace_wallet_cards(
            &mut ctx.state.agent_wallet_cards,
            &mut ctx.state.wallet_generation,
            initial_wallet_cards,
        )?;
        ctx.rederive_agent_effective_surface_from_wallet();
    }
    Ok(())
}

async fn record_worker_update_failed_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    target_revision: ComponentRevision,
    details: String,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    let public_state = store.with(|mut access| get_ctx(access.data_mut()).public_state.clone());
    public_state
        .worker()
        .add_and_commit_oplog(OplogEntry::failed_update(
            target_revision,
            Some(details.clone()),
        ))
        .await;
    tracing::warn!(
        "Worker failed to update to {}: {}, update attempt aborted",
        target_revision,
        details
    );
    Ok(())
}

async fn record_worker_update_succeeded_access<T, D, Ctx>(
    store: &Accessor<T, D>,
    get_ctx: fn(&mut T) -> &mut DurableWorkerCtx<Ctx>,
    target_revision: ComponentRevision,
    component_size: u64,
    active_plugins: HashSet<
        golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId,
    >,
) -> Result<(), WorkerExecutorError>
where
    T: 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
{
    tracing::info!("Worker update to {} finished successfully", target_revision);
    let (public_state, linear_memory) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        (ctx.public_state.clone(), ctx.linear_memory_tracker())
    });
    let worker = public_state.worker();
    worker
        .persist_successful_update(
            &linear_memory,
            target_revision,
            component_size,
            active_plugins,
        )
        .await;
    Ok(())
}
