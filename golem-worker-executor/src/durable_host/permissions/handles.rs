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
    Accessor, CallReplayOutcome, CardId, CardState, DateTime, DurabilityHost, DurableCallSession,
    DurableFunctionType, DurableWorkerCtx, GolemPermissionsCardLiveness, HostFailureKind,
    HostPermissionCard, HostPermissionCardWithStore, HostRequestPermissionCardLiveness,
    HostResponsePermissionCardLiveness, IdempotencyKey, InternalRetryResult, NotCancellable,
    PermissionCardEntry, PermissionCardHandleRep, PermissionCardResolver,
    PermissionCardValuePayload, ResolvedPermissionCard, Resource, ScopeCard,
    SerializablePermissionCardLiveness, StoredCard, Utc, Uuid, WorkerCtx, WorkerExecutorError,
    anyhow, deserialize, permission_error_to_anyhow, permissions_types, serialize,
    worker_error_to_permission_error,
};

impl PermissionCardEntry {
    pub(super) fn from_card(card: StoredCard) -> Self {
        Self::Persistent {
            snapshot: snapshot_from_card(&card),
            card: Some(card),
        }
    }

    pub(super) fn from_snapshot(snapshot: PermissionCardValuePayload) -> Self {
        Self::Persistent {
            snapshot,
            card: None,
        }
    }

    pub(super) fn from_scope(card: ScopeCard, invocation_key: IdempotencyKey) -> Self {
        Self::Scope {
            card,
            invocation_key: Some(invocation_key),
        }
    }

    pub(super) fn snapshot(&self) -> Result<PermissionCardValuePayload, WorkerExecutorError> {
        match self {
            Self::Persistent { snapshot, .. } => Ok(snapshot.clone()),
            Self::Scope { .. } => Err(scope_card_snapshot_error()),
        }
    }

    pub(super) fn into_snapshot(self) -> Result<PermissionCardValuePayload, WorkerExecutorError> {
        match self {
            Self::Persistent { snapshot, .. } => Ok(snapshot),
            Self::Scope { .. } => Err(scope_card_snapshot_error()),
        }
    }

    pub(super) fn cached_card(&self) -> Option<StoredCard> {
        match self {
            Self::Persistent { card, .. } => card.clone(),
            Self::Scope { .. } => None,
        }
    }

    pub(super) fn scope_card(&self) -> Option<(&ScopeCard, Option<&IdempotencyKey>)> {
        match self {
            Self::Persistent { .. } => None,
            Self::Scope {
                card,
                invocation_key,
            } => Some((card, invocation_key.as_ref())),
        }
    }

    pub(super) fn retire(&mut self) {
        if let Self::Scope { invocation_key, .. } = self {
            *invocation_key = None;
        }
    }
}

/// Retires the guest's scope-card handle when its scope ends. The table entry is kept until the
/// guest drops the handle, so a stale handle cannot name a reused table slot.
pub(crate) fn retire_invocation_scope_handle(
    table: &mut wasmtime::component::ResourceTable,
    rep: u32,
) {
    let handle = Resource::<PermissionCardHandleRep>::new_borrow(rep);
    if let Ok(entry) = table.get_mut(&handle)
        && let Some(entry) = entry.downcast_mut::<PermissionCardEntry>()
    {
        entry.retire();
    }
}

impl ResolvedPermissionCard {
    pub(super) fn card_id(&self) -> CardId {
        match self {
            Self::Persistent(card) => card.card_id(),
            Self::Scope(card) => card.scope_card_id,
        }
    }

    pub(super) fn parent_ids(&self) -> &[CardId] {
        match self {
            Self::Persistent(card) => card.parent_ids(),
            Self::Scope(card) => &card.root_card_ids,
        }
    }

    pub(super) fn expires_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Persistent(card) => card.expires_at(),
            Self::Scope(card) => card.expires_at,
        }
    }

    pub(super) fn into_persistent(
        self,
        operation: &'static str,
    ) -> Result<StoredCard, permissions_types::PermissionError> {
        match self {
            Self::Persistent(card) => Ok(card),
            Self::Scope(_) => Err(scope_card_operation_error(operation)),
        }
    }
}

pub(super) fn snapshot_from_card(card: &StoredCard) -> PermissionCardValuePayload {
    PermissionCardValuePayload {
        card_id: card.card_id().0,
        parent_ids: card.parent_ids().iter().map(|card_id| card_id.0).collect(),
        expires_at: card.expires_at(),
        polymorphic: matches!(card, StoredCard::Polymorphic(_)),
    }
}

pub(super) fn sorted_uuids(values: impl IntoIterator<Item = Uuid>) -> Vec<Uuid> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

pub(super) fn snapshot_matches_card(
    snapshot: &PermissionCardValuePayload,
    card: &StoredCard,
) -> bool {
    snapshot.card_id == card.card_id().0
        && sorted_uuids(snapshot.parent_ids.iter().copied())
            == sorted_uuids(card.parent_ids().iter().map(|card_id| card_id.0))
        && snapshot.expires_at == card.expires_at()
        && snapshot.polymorphic == matches!(card, StoredCard::Polymorphic(_))
}

pub(super) fn snapshot_mismatch_error(
    source: &'static str,
    snapshot: &PermissionCardValuePayload,
    card: &StoredCard,
) -> permissions_types::PermissionError {
    permissions_types::PermissionError::NotPermitted(format!(
        "permission-card snapshot for {} does not match the {source} card (parents: {:?} vs {:?}, expiry: {}, polymorphic: {} vs {})",
        snapshot.card_id,
        sorted_uuids(snapshot.parent_ids.iter().copied()),
        sorted_uuids(card.parent_ids().iter().map(|card_id| card_id.0)),
        snapshot.expires_at == card.expires_at(),
        snapshot.polymorphic,
        matches!(card, StoredCard::Polymorphic(_)),
    ))
}

pub(super) fn invalid_handle_error(error: impl std::fmt::Display) -> WorkerExecutorError {
    WorkerExecutorError::runtime(format!("invalid permission-card handle: {error}"))
}

pub(super) fn scope_card_snapshot_error() -> WorkerExecutorError {
    WorkerExecutorError::runtime(
        "scope-card handles cannot be serialized as persistent permission-card snapshots",
    )
}

pub(super) fn scope_card_operation_error(
    operation: &'static str,
) -> permissions_types::PermissionError {
    permissions_types::PermissionError::NotPermitted(format!(
        "{operation} does not accept scope cards"
    ))
}

pub(super) fn permission_card_snapshot_from_rep(
    rep: &PermissionCardHandleRep,
) -> Result<PermissionCardValuePayload, WorkerExecutorError> {
    if let Some(entry) = rep.downcast_ref::<PermissionCardEntry>() {
        entry.snapshot()
    } else if let Some(snapshot) = rep.downcast_ref::<PermissionCardValuePayload>() {
        Ok(snapshot.clone())
    } else {
        Err(WorkerExecutorError::runtime(
            "permission-card resource had unexpected payload type",
        ))
    }
}

pub(super) fn permission_card_snapshot<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<PermissionCardValuePayload, WorkerExecutorError> {
    let rep = ctx.table().get(handle).map_err(invalid_handle_error)?;
    permission_card_snapshot_from_rep(rep)
}

pub(super) fn permission_card_cached_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<Option<StoredCard>, WorkerExecutorError> {
    let rep = ctx.table().get(handle).map_err(invalid_handle_error)?;
    Ok(rep
        .downcast_ref::<PermissionCardEntry>()
        .and_then(PermissionCardEntry::cached_card))
}

pub(super) fn permission_card_scope_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<Option<(ScopeCard, Option<IdempotencyKey>)>, WorkerExecutorError> {
    let rep = ctx.table().get(handle).map_err(invalid_handle_error)?;
    Ok(rep
        .downcast_ref::<PermissionCardEntry>()
        .and_then(PermissionCardEntry::scope_card)
        .map(|(card, invocation_key)| (card.clone(), invocation_key.cloned())))
}

pub(crate) fn resolve_invocation_scope_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<ScopeCard, String> {
    let invocation_key = ctx
        .state
        .get_current_idempotency_key()
        .ok_or_else(|| "scope cards require an active invocation".to_string())?;
    let scope_card = permission_card_scope_card(ctx, handle)
        .map_err(|error| format!("invalid permission-card handle: {error}"))?;
    resolve_owned_scope_card(scope_card, &invocation_key)
}

pub(super) fn resolve_owned_scope_card(
    scope_card: Option<(ScopeCard, Option<IdempotencyKey>)>,
    invocation_key: &IdempotencyKey,
) -> Result<ScopeCard, String> {
    let Some((card, owner_invocation_key)) = scope_card else {
        return Err("the scope-card argument must be a scope card".to_string());
    };
    if owner_invocation_key.as_ref() == Some(invocation_key) {
        Ok(card)
    } else {
        Err("the scope-card handle is not valid in this invocation".to_string())
    }
}

/// The card behind a persistent handle, as the handle's cached copy or the caller's wallet holds
/// it, without asking the registry whether it is still live.
pub(super) fn local_permission_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
    card_id: CardId,
) -> Result<Option<StoredCard>, WorkerExecutorError> {
    if let Some(card) = permission_card_cached_card(ctx, handle)? {
        return Ok(Some(card));
    }
    Ok(ctx
        .agent_wallet_cards_snapshot()
        .into_iter()
        .find(|card| card.card_id() == card_id))
}

/// Resolves a persistent handle inside a durable call that records its own outcome, so the
/// registry lookup of a card that is neither cached nor in the wallet needs no recording here.
pub(super) async fn resolve_permission_card_within_recorded_call<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<StoredCard, permissions_types::PermissionError> {
    let snapshot = permission_card_snapshot(ctx, handle).map_err(|error| {
        permissions_types::PermissionError::NotPermitted(format!(
            "invalid permission-card handle: {error}"
        ))
    })?;

    let card_id = CardId(snapshot.card_id);
    if let Some(card) = local_permission_card(ctx, handle, card_id).map_err(|error| {
        permissions_types::PermissionError::NotPermitted(format!(
            "invalid permission-card handle: {error}"
        ))
    })? {
        if snapshot_matches_card(&snapshot, &card) {
            return Ok(card);
        }
        return Err(snapshot_mismatch_error("local", &snapshot, &card));
    }

    let card_state = ctx
        .state
        .card_service
        .check_cards(vec![card_id])
        .await
        .map_err(worker_error_to_permission_error)?
        .remove(&card_id);

    match card_state {
        Some(CardState::Live(card)) => {
            let card = *card;
            if snapshot_matches_card(&snapshot, &card) {
                Ok(card)
            } else {
                Err(snapshot_mismatch_error("registry", &snapshot, &card))
            }
        }
        Some(CardState::Revoked) | Some(CardState::Unknown) | None => {
            Err(card_revoked_error(card_id))
        }
    }
}

pub(super) fn card_revoked_error(card_id: CardId) -> permissions_types::PermissionError {
    permissions_types::PermissionError::CardRevoked(format!(
        "permission card {} is revoked",
        card_id.0
    ))
}

pub(super) fn card_expired_error(card_id: CardId) -> permissions_types::PermissionError {
    permissions_types::PermissionError::CardExpired(format!(
        "permission card {} is expired",
        card_id.0
    ))
}

pub(super) fn card_is_expired_at(
    snapshot: &PermissionCardValuePayload,
    now: DateTime<Utc>,
) -> bool {
    snapshot
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
}

/// Asks the card service whether the card behind a persistent handle is still live. A live card
/// resolves to the handle's cached copy or the caller's wallet copy when there is one, so the
/// resolved card does not depend on how the card service renders it. A card the service omits
/// from its answer has no settled state yet, so the omission is a transient failure.
pub(super) async fn observe_permission_card_liveness<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
    snapshot: &PermissionCardValuePayload,
) -> Result<SerializablePermissionCardLiveness, WorkerExecutorError> {
    let card_id = CardId(snapshot.card_id);
    let state = ctx
        .state
        .card_service
        .check_cards(vec![card_id])
        .await?
        .remove(&card_id)
        .ok_or_else(|| {
            WorkerExecutorError::runtime(format!(
                "the card service returned no state for permission card {card_id}"
            ))
        })?;
    let expired = card_is_expired_at(snapshot, Utc::now());
    match state {
        _ if expired => Ok(SerializablePermissionCardLiveness::Expired),
        CardState::Live(registered) => {
            let card = local_permission_card(ctx, handle, card_id)?.unwrap_or(*registered);
            let card = serialize(&card).map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to serialize permission card {card_id}: {error}"
                ))
            })?;
            Ok(SerializablePermissionCardLiveness::Live(card))
        }
        CardState::Revoked | CardState::Unknown => Ok(SerializablePermissionCardLiveness::Revoked),
    }
}

/// Resolves a persistent handle for one guest-visible operation. The card may be revoked or
/// expire at any time after the handle was created, so every operation asks the card service and
/// records the answer; replay returns the recorded answer instead of asking again. A failed
/// lookup traps and leaves the `Start` incomplete, so recovery asks again.
pub(super) async fn resolve_permission_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> anyhow::Result<Result<StoredCard, permissions_types::PermissionError>> {
    let snapshot = match permission_card_snapshot(ctx, handle) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Ok(Err(permissions_types::PermissionError::NotPermitted(
                format!("invalid permission-card handle: {error}"),
            )));
        }
    };
    let card_id = CardId(snapshot.card_id);

    let mut call = DurableCallSession::<GolemPermissionsCardLiveness, NotCancellable>::start(
        ctx,
        HostRequestPermissionCardLiveness {
            card_id: snapshot.card_id,
        },
        DurableFunctionType::ReadRemote,
    )
    .await?;
    let response = 'result: {
        if !call.is_live() {
            let recorded = match call.recorded_request(ctx).await {
                Ok(recorded) => recorded,
                Err(error) => return Err(call.trap(error)),
            };
            if recorded.card_id != snapshot.card_id {
                return Err(call.trap(WorkerExecutorError::unexpected_oplog_entry(
                    format!("card-liveness request for permission card {card_id}"),
                    format!(
                        "card-liveness request for permission card {}",
                        recorded.card_id
                    ),
                )));
            }
            match call.replay(ctx).await? {
                CallReplayOutcome::Replayed(response) => break 'result response,
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let liveness = loop {
            let result = observe_permission_card_liveness(ctx, handle, &snapshot).await;
            match call
                .try_trigger_retry_or_loop(ctx, &result, |_| HostFailureKind::Transient)
                .await?
            {
                InternalRetryResult::Persist => break result,
                InternalRetryResult::RetryInternally => continue,
            }
        };
        let liveness = match liveness {
            Ok(liveness) => liveness,
            Err(error) => return Err(call.trap(error)),
        };
        call.complete(ctx, HostResponsePermissionCardLiveness { liveness })
            .await?
    };

    match response.liveness {
        SerializablePermissionCardLiveness::Live(card) => {
            let card: StoredCard = deserialize(&card).map_err(|error| {
                anyhow!("failed to deserialize the recorded permission card {card_id}: {error}")
            })?;
            if snapshot_matches_card(&snapshot, &card) {
                Ok(Ok(card))
            } else {
                Ok(Err(snapshot_mismatch_error("resolved", &snapshot, &card)))
            }
        }
        SerializablePermissionCardLiveness::Revoked => Ok(Err(card_revoked_error(card_id))),
        SerializablePermissionCardLiveness::Expired => Ok(Err(card_expired_error(card_id))),
    }
}

pub(super) async fn resolve_permission_card_handle<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> anyhow::Result<Result<ResolvedPermissionCard, permissions_types::PermissionError>> {
    let scope_card = match permission_card_scope_card(ctx, handle) {
        Ok(scope_card) => scope_card,
        Err(error) => {
            return Ok(Err(permissions_types::PermissionError::NotPermitted(
                format!("invalid permission-card handle: {error}"),
            )));
        }
    };
    match scope_card {
        Some((card, _)) => Ok(Ok(ResolvedPermissionCard::Scope(card))),
        None => Ok(resolve_permission_card(ctx, handle)
            .await?
            .map(ResolvedPermissionCard::Persistent)),
    }
}

/// Resolves a handle for an accessor, which has no error result: a revoked, expired or invalid
/// card traps, as an unknown handle does.
pub(super) async fn resolve_permission_card_handle_or_trap<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> anyhow::Result<ResolvedPermissionCard> {
    resolve_permission_card_handle(ctx, handle)
        .await?
        .map_err(permission_error_to_anyhow)
}

/// The id of the card behind a handle, or `None` for an invalid handle.
pub(super) fn handle_card_id<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Option<Uuid> {
    match permission_card_scope_card(ctx, handle).ok()? {
        Some((card, _)) => Some(card.scope_card_id.0),
        None => permission_card_snapshot(ctx, handle)
            .ok()
            .map(|snapshot| snapshot.card_id),
    }
}

impl<Ctx: WorkerCtx> HostPermissionCard for DurableWorkerCtx<Ctx> {}

impl<T: Send + 'static, Ctx: WorkerCtx> HostPermissionCardWithStore<T>
    for crate::durable_host::schema_value_stream::CoreTypesHost<Ctx>
{
    async fn drop(
        accessor: &Accessor<T, Self>,
        rep: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<()> {
        accessor.with(|mut access| {
            let ctx = access.get();
            DurabilityHost::observe_function_call(ctx, "golem::core::permission-card", "drop");
            ctx.state.invocation_scope_handles.remove(&rep.rep());
            ctx.table().delete(rep)?;
            Ok(())
        })
    }
}

impl<Ctx: WorkerCtx> PermissionCardResolver for DurableWorkerCtx<Ctx> {
    type Error = WorkerExecutorError;

    fn snapshot_permission_card_handle(
        &mut self,
        handle: Resource<PermissionCardHandleRep>,
    ) -> Result<PermissionCardValuePayload, Self::Error> {
        self.state.invocation_scope_handles.remove(&handle.rep());
        let rep = self.table().delete(handle).map_err(invalid_handle_error)?;
        match rep.into_payload::<PermissionCardEntry>() {
            Ok(entry) => entry.into_snapshot(),
            Err(rep) => rep
                .into_payload::<PermissionCardValuePayload>()
                .map_err(|_| {
                    WorkerExecutorError::runtime(
                        "permission-card resource had unexpected payload type",
                    )
                }),
        }
    }

    fn permission_card_handle_from_snapshot(
        &mut self,
        snapshot: &PermissionCardValuePayload,
    ) -> Result<Resource<PermissionCardHandleRep>, Self::Error> {
        let entry = if let Some(card) = self
            .agent_wallet_cards_snapshot()
            .into_iter()
            .find(|card| card.card_id() == CardId(snapshot.card_id))
        {
            if !snapshot_matches_card(snapshot, &card) {
                return Err(WorkerExecutorError::runtime(format!(
                    "permission-card snapshot for {} does not match the stored card",
                    snapshot.card_id
                )));
            }
            PermissionCardEntry::from_card(card.clone())
        } else {
            PermissionCardEntry::from_snapshot(snapshot.clone())
        };

        self.table()
            .push(PermissionCardHandleRep::new(entry))
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "failed to create permission-card handle: {e}"
                ))
            })
    }

    fn drop_permission_card_handle(&mut self, handle: Resource<PermissionCardHandleRep>) {
        self.state.invocation_scope_handles.remove(&handle.rep());
        let _ = self.table().delete(handle);
    }
}

impl<Ctx: WorkerCtx> permissions_types::Host for DurableWorkerCtx<Ctx> {
    async fn id(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<permissions_types::CardId> {
        DurabilityHost::observe_function_call(self, "golem::permissions::types", "id");
        let card = resolve_permission_card_handle_or_trap(self, &c).await?;
        Ok(card.card_id().into())
    }

    async fn parents(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<Vec<permissions_types::CardId>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::types", "parents");
        let card = resolve_permission_card_handle_or_trap(self, &c).await?;
        Ok(card.parent_ids().iter().copied().map(Into::into).collect())
    }

    async fn expires_at(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<Option<permissions_types::Timestamp>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::types", "expires-at");
        let card = resolve_permission_card_handle_or_trap(self, &c).await?;
        Ok(card.expires_at().map(Into::into))
    }
}
