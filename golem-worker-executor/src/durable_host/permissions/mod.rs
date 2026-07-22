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

//! Host-side binding for the opaque `golem:core/types.permission-card`
//! resource and the [`PermissionCardResolver`] boundary bridge.
//!
//! The resource itself is defined in `golem:core/types` (so it can travel
//! inside a `schema-value-tree` as an opaque owned handle) and is bound to the
//! opaque [`PermissionCardHandleRep`] by golem-schema. The only operation the
//! core interface declares for it is `drop`, which releases the handle from the
//! resource table.
//!
//! The resolver stores the trusted [`PermissionCardValuePayload`] snapshot in
//! the handle rep, optionally paired with the locally resolved [`StoredCard`].
//! Lifting a handle consumes it and returns the snapshot; lowering a snapshot
//! materializes a fresh handle and verifies it against the local wallet when the
//! card is already known locally. No durable oplog entry is written here; the
//! durable/cross-executor representation is the snapshot embedded in the
//! surrounding value, never the live handle.

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::permissions::derive as permissions_derive;
use crate::preview2::golem::permissions::inspect as permissions_inspect;
use crate::preview2::golem::permissions::kernel_introspection as permissions_kernel;
use crate::preview2::golem::permissions::revoke as permissions_revoke;
use crate::preview2::golem::permissions::types as permissions_types;
use crate::preview2::golem::permissions::wallet as permissions_wallet;
use crate::services::HasWorker;
use crate::services::card::{CardRevokeResult, CardState};
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use chrono::{DateTime, TimeDelta, Utc};
use golem_common::base_model::oplog::QueuedCardEventTransfer;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    AgentCardHolder, AgentPermissionMonomorphizationContext, Card, CardAlgebraError, CardClass,
    CardHolder, CardId, CardManagedBy, CardManagedByRuntimeDerived, CardParseError,
    CardResourcePattern, CardVerb, ClassPermissionTarget, DelegationSurface, EffectiveSurface,
    PermissionPattern, PermissionTarget, PolymorphicCard, PolymorphicPermissionPattern,
    RenderedPermissionFields, ScopeCard, StoredCard, WalletDerivationParent,
    agent_delegation_surface_from_wallet, instantiate_polymorphic_card_for_agent,
    monomorphize_card_for_agent, parse_permission_fields, permission_class_metadata,
};
use golem_common::model::oplog::host_functions::{
    GolemPermissionsDerivePersist, GolemPermissionsInstallChildPersist,
    GolemPermissionsInstallTransfer, GolemPermissionsRevokePersist,
};
use golem_common::model::oplog::payload::types::PermissionCardRevokeError;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostRequestPermissionCardDerive,
    HostRequestPermissionCardRevoke, HostRequestPermissionCardTransfer,
    HostResponsePermissionCardDerived, HostResponsePermissionCardTransferComplete,
    HostResponsePermissionCardsRevoked, OplogEntry, OplogIndex, QueuedCardEvent,
};
use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId, PendingCardEventRef, Timestamp};
use golem_common::serialization::{deserialize, serialize};
use golem_schema::schema::schema_value::PermissionCardValuePayload;
use golem_schema::schema::wit::wire::HostPermissionCard;
use golem_schema::schema::wit::{PermissionCardHandleRep, PermissionCardResolver};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use uuid::Uuid;
use wasmtime::component::Resource;

#[derive(Clone, Debug)]
enum PermissionCardEntry {
    Persistent {
        snapshot: PermissionCardValuePayload,
        card: Option<StoredCard>,
    },
    Scope(ScopeCard),
}

impl PermissionCardEntry {
    fn from_card(card: StoredCard) -> Self {
        Self::Persistent {
            snapshot: snapshot_from_card(&card),
            card: Some(card),
        }
    }

    fn from_snapshot(snapshot: PermissionCardValuePayload) -> Self {
        Self::Persistent {
            snapshot,
            card: None,
        }
    }

    fn snapshot(&self) -> Result<PermissionCardValuePayload, WorkerExecutorError> {
        match self {
            Self::Persistent { snapshot, .. } => Ok(snapshot.clone()),
            Self::Scope(_) => Err(scope_card_snapshot_error()),
        }
    }

    fn into_snapshot(self) -> Result<PermissionCardValuePayload, WorkerExecutorError> {
        match self {
            Self::Persistent { snapshot, .. } => Ok(snapshot),
            Self::Scope(_) => Err(scope_card_snapshot_error()),
        }
    }

    fn cached_card(&self) -> Option<StoredCard> {
        match self {
            Self::Persistent { card, .. } => card.clone(),
            Self::Scope(_) => None,
        }
    }

    fn scope_card(&self) -> Option<&ScopeCard> {
        match self {
            Self::Persistent { .. } => None,
            Self::Scope(card) => Some(card),
        }
    }
}

impl From<ScopeCard> for PermissionCardEntry {
    fn from(card: ScopeCard) -> Self {
        Self::Scope(card)
    }
}

#[derive(Clone, Debug)]
enum ResolvedPermissionCard {
    Persistent(StoredCard),
    Scope(ScopeCard),
}

impl ResolvedPermissionCard {
    fn card_id(&self) -> CardId {
        match self {
            Self::Persistent(card) => card.card_id(),
            Self::Scope(card) => card.scope_card_id,
        }
    }

    fn parent_ids(&self) -> &[CardId] {
        match self {
            Self::Persistent(card) => card.parent_ids(),
            Self::Scope(card) => &card.root_card_ids,
        }
    }

    fn expires_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Persistent(card) => card.expires_at(),
            Self::Scope(_) => None,
        }
    }

    fn is_polymorphic(&self) -> bool {
        matches!(self, Self::Persistent(StoredCard::Polymorphic(_)))
    }

    fn into_persistent(
        self,
        operation: &'static str,
    ) -> Result<StoredCard, permissions_types::PermissionError> {
        match self {
            Self::Persistent(card) => Ok(card),
            Self::Scope(_) => Err(scope_card_operation_error(operation)),
        }
    }
}

impl From<DateTime<Utc>> for permissions_types::Timestamp {
    fn from(timestamp: DateTime<Utc>) -> Self {
        Self {
            seconds: timestamp.timestamp(),
            nanoseconds: timestamp.timestamp_subsec_nanos(),
        }
    }
}

fn snapshot_from_card(card: &StoredCard) -> PermissionCardValuePayload {
    PermissionCardValuePayload {
        card_id: card.card_id().0,
        parent_ids: card.parent_ids().iter().map(|card_id| card_id.0).collect(),
        expires_at: card.expires_at(),
        polymorphic: matches!(card, StoredCard::Polymorphic(_)),
    }
}

fn sorted_uuids(values: impl IntoIterator<Item = Uuid>) -> Vec<Uuid> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

fn snapshot_matches_card(snapshot: &PermissionCardValuePayload, card: &StoredCard) -> bool {
    snapshot.card_id == card.card_id().0
        && sorted_uuids(snapshot.parent_ids.iter().copied())
            == sorted_uuids(card.parent_ids().iter().map(|card_id| card_id.0))
        && snapshot.expires_at == card.expires_at()
        && snapshot.polymorphic == matches!(card, StoredCard::Polymorphic(_))
}

fn snapshot_mismatch_error(card_id: Uuid) -> permissions_types::PermissionError {
    permissions_types::PermissionError::NotPermitted(format!(
        "permission-card snapshot for {card_id} does not match the stored card"
    ))
}

fn permission_error_to_anyhow(error: permissions_types::PermissionError) -> anyhow::Error {
    anyhow!("permission-card error: {error:?}")
}

fn worker_error_to_permission_error(
    error: WorkerExecutorError,
) -> permissions_types::PermissionError {
    permissions_types::PermissionError::NotPermitted(format!(
        "permission-card store lookup failed: {error}"
    ))
}

fn invalid_handle_error(error: impl std::fmt::Display) -> WorkerExecutorError {
    WorkerExecutorError::runtime(format!("invalid permission-card handle: {error}"))
}

fn scope_card_snapshot_error() -> WorkerExecutorError {
    WorkerExecutorError::runtime(
        "scope-card handles cannot be serialized as persistent permission-card snapshots",
    )
}

fn scope_card_operation_error(operation: &'static str) -> permissions_types::PermissionError {
    permissions_types::PermissionError::NotPermitted(format!(
        "{operation} does not accept scope cards"
    ))
}

fn permission_card_snapshot_from_rep(
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

fn permission_card_snapshot<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<PermissionCardValuePayload, WorkerExecutorError> {
    let rep = ctx.table().get(handle).map_err(invalid_handle_error)?;
    permission_card_snapshot_from_rep(rep)
}

fn permission_card_cached_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<Option<StoredCard>, WorkerExecutorError> {
    let rep = ctx.table().get(handle).map_err(invalid_handle_error)?;
    Ok(rep
        .downcast_ref::<PermissionCardEntry>()
        .and_then(PermissionCardEntry::cached_card))
}

fn permission_card_scope_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<Option<ScopeCard>, WorkerExecutorError> {
    let rep = ctx.table().get(handle).map_err(invalid_handle_error)?;
    Ok(rep
        .downcast_ref::<PermissionCardEntry>()
        .and_then(PermissionCardEntry::scope_card)
        .cloned())
}

async fn resolve_permission_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<StoredCard, permissions_types::PermissionError> {
    let snapshot = permission_card_snapshot(ctx, handle).map_err(|error| {
        permissions_types::PermissionError::NotPermitted(format!(
            "invalid permission-card handle: {error}"
        ))
    })?;

    if let Some(card) = permission_card_cached_card(ctx, handle).map_err(|error| {
        permissions_types::PermissionError::NotPermitted(format!(
            "invalid permission-card handle: {error}"
        ))
    })? {
        if snapshot_matches_card(&snapshot, &card) {
            return Ok(card);
        }
        return Err(snapshot_mismatch_error(snapshot.card_id));
    }

    let card_id = CardId(snapshot.card_id);
    if let Some(card) = ctx.state.agent_wallet_cards.get(&card_id).cloned() {
        if snapshot_matches_card(&snapshot, &card) {
            return Ok(card);
        }
        return Err(snapshot_mismatch_error(snapshot.card_id));
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
                Err(snapshot_mismatch_error(snapshot.card_id))
            }
        }
        Some(CardState::Revoked) => Err(permissions_types::PermissionError::CardRevoked(format!(
            "permission card {} is revoked",
            card_id.0
        ))),
        Some(CardState::Unknown) | None => Err(permissions_types::PermissionError::CardRevoked(
            format!("permission card {} is unknown or revoked", card_id.0),
        )),
    }
}

async fn resolve_permission_card_handle<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> Result<ResolvedPermissionCard, permissions_types::PermissionError> {
    let scope_card = permission_card_scope_card(ctx, handle).map_err(|error| {
        permissions_types::PermissionError::NotPermitted(format!(
            "invalid permission-card handle: {error}"
        ))
    })?;
    match scope_card {
        Some(card) => Ok(ResolvedPermissionCard::Scope(card)),
        None => resolve_permission_card(ctx, handle)
            .await
            .map(ResolvedPermissionCard::Persistent),
    }
}

async fn resolve_permission_card_handle_or_trap<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> anyhow::Result<ResolvedPermissionCard> {
    resolve_permission_card_handle(ctx, handle)
        .await
        .map_err(permission_error_to_anyhow)
}

fn parse_pattern_grant(
    g: &permissions_types::PatternGrant,
) -> Result<PermissionPattern, permissions_types::PermissionError> {
    parse_permission_fields(&g.class, &g.owner, &g.recipient, &g.verb, &g.resource_id)
        .map_err(permission_error_from_parse)
}

fn permission_error_from_parse(error: CardParseError) -> permissions_types::PermissionError {
    match error {
        CardParseError::UnknownClass(class) => {
            permissions_types::PermissionError::UnknownResourceClass(class)
        }
        CardParseError::UnknownVerb { verb, .. } => {
            permissions_types::PermissionError::InvalidVerb(verb)
        }
        CardParseError::InvalidOwnerPath { owner, .. } => {
            permissions_types::PermissionError::InvalidOwner(owner)
        }
        CardParseError::InvalidRecipientPath(recipient) => {
            permissions_types::PermissionError::InvalidRecipient(recipient)
        }
        CardParseError::Malformed(message) => {
            permissions_types::PermissionError::InvalidPattern(message)
        }
        CardParseError::InvalidResource { resource, .. } => {
            permissions_types::PermissionError::InvalidPattern(resource)
        }
        CardParseError::SlotVariableInConcreteGrant(value) => {
            permissions_types::PermissionError::InvalidPattern(value)
        }
    }
}

fn pattern_grant_from_fields(fields: RenderedPermissionFields) -> permissions_types::PatternGrant {
    permissions_types::PatternGrant {
        class: fields.class.to_string(),
        owner: fields.owner,
        recipient: fields.recipient,
        verb: fields.verb,
        resource_id: fields.resource,
    }
}

fn concrete_pattern_grant(
    pattern: &PermissionPattern,
) -> Result<permissions_types::PatternGrant, permissions_types::PermissionError> {
    pattern
        .render_fields()
        .map(pattern_grant_from_fields)
        .map_err(permissions_types::PermissionError::InvalidPattern)
}

fn polymorphic_pattern_grant(
    pattern: &PolymorphicPermissionPattern,
) -> Result<permissions_types::PatternGrant, permissions_types::PermissionError> {
    pattern
        .render_fields()
        .map(pattern_grant_from_fields)
        .map_err(permissions_types::PermissionError::InvalidPattern)
}

fn concrete_pattern_grants(
    patterns: &[PermissionPattern],
) -> Result<Vec<permissions_types::PatternGrant>, permissions_types::PermissionError> {
    patterns.iter().map(concrete_pattern_grant).collect()
}

fn polymorphic_pattern_grants(
    patterns: &[PolymorphicPermissionPattern],
) -> Result<Vec<permissions_types::PatternGrant>, permissions_types::PermissionError> {
    patterns.iter().map(polymorphic_pattern_grant).collect()
}

fn card_view(
    card: &ResolvedPermissionCard,
) -> Result<permissions_types::CardView, permissions_types::PermissionError> {
    match card {
        ResolvedPermissionCard::Persistent(StoredCard::Concrete(card)) => {
            Ok(permissions_types::CardView {
                lower_positive: concrete_pattern_grants(&card.lower_positive)?,
                lower_negative: concrete_pattern_grants(&card.lower_negative)?,
                upper_positive: concrete_pattern_grants(&card.upper_positive)?,
                upper_negative: concrete_pattern_grants(&card.upper_negative)?,
            })
        }
        ResolvedPermissionCard::Persistent(StoredCard::Polymorphic(card)) => {
            Ok(permissions_types::CardView {
                lower_positive: polymorphic_pattern_grants(&card.lower_positive)?,
                lower_negative: polymorphic_pattern_grants(&card.lower_negative)?,
                upper_positive: polymorphic_pattern_grants(&card.upper_positive)?,
                upper_negative: polymorphic_pattern_grants(&card.upper_negative)?,
            })
        }
        ResolvedPermissionCard::Scope(card) => Ok(permissions_types::CardView {
            lower_positive: concrete_pattern_grants(&card.lower_positive)?,
            lower_negative: concrete_pattern_grants(&card.lower_negative)?,
            upper_positive: concrete_pattern_grants(&card.upper_positive)?,
            upper_negative: concrete_pattern_grants(&card.upper_negative)?,
        }),
    }
}

fn card_verb_name(verb: CardVerb) -> &'static str {
    match verb {
        CardVerb::Derive => "derive",
        CardVerb::Revoke => "revoke",
        CardVerb::Inspect => "inspect",
        CardVerb::Install => "install",
    }
}

fn permission_error_from_algebra(error: CardAlgebraError) -> permissions_types::PermissionError {
    match error {
        CardAlgebraError::InvalidOwnerPath(owner) => {
            permissions_types::PermissionError::InvalidOwner(owner)
        }
        CardAlgebraError::InvalidRecipientPath(recipient) => {
            permissions_types::PermissionError::InvalidRecipient(recipient)
        }
        CardAlgebraError::LowerBoundTooBroad { grant } => {
            permissions_types::PermissionError::LowerBoundTooBroad(
                grant
                    .render()
                    .unwrap_or_else(|_| "lower-bound grant is too broad".to_string()),
            )
        }
        CardAlgebraError::UpperBoundTooBroad { grant } => {
            permissions_types::PermissionError::UpperBoundTooBroad(
                grant
                    .and_then(|grant| grant.render().ok())
                    .unwrap_or_else(|| "upper-bound grant is too broad".to_string()),
            )
        }
    }
}

fn invalid_lifespan_error(message: impl std::fmt::Display) -> permissions_types::PermissionError {
    permissions_types::PermissionError::InvalidPattern(format!(
        "invalid ISO-8601 lifespan: {message}"
    ))
}

fn parse_lifespan(
    lifespan: Option<&permissions_types::Duration>,
) -> Result<Option<TimeDelta>, permissions_types::PermissionError> {
    let Some(lifespan) = lifespan else {
        return Ok(None);
    };
    if !lifespan.iso_8601.starts_with('P') {
        return Err(invalid_lifespan_error(
            "duration must start with the 'P' designator",
        ));
    }

    let duration = golem_schema::schema::canonical::duration::from_text(&lifespan.iso_8601)
        .map_err(invalid_lifespan_error)?;
    Ok(Some(TimeDelta::nanoseconds(duration.nanoseconds)))
}

fn derive_expires_at(
    now: DateTime<Utc>,
    lifespan: Option<&permissions_types::Duration>,
    parent_expires_at: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, permissions_types::PermissionError> {
    if parent_expires_at.is_some_and(|expires_at| expires_at <= now) {
        return Err(permissions_types::PermissionError::CardExpired(
            "cannot derive from an expired parent card".to_string(),
        ));
    }

    let lifespan = parse_lifespan(lifespan)?;
    let Some(lifespan) = lifespan else {
        return if parent_expires_at.is_some() {
            Err(permissions_types::PermissionError::CardExpired(
                "an indefinite child card would outlive its parent card".to_string(),
            ))
        } else {
            Ok(None)
        };
    };
    let expires_at = now
        .checked_add_signed(lifespan)
        .ok_or_else(|| invalid_lifespan_error("resulting expiry is out of range"))?;

    if parent_expires_at.is_some_and(|parent_expires_at| expires_at > parent_expires_at) {
        return Err(permissions_types::PermissionError::CardExpired(
            "child card expiry would exceed its parent card expiry".to_string(),
        ));
    }

    Ok(Some(expires_at))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DerivedGrantSets {
    lower_positive: Vec<PermissionPattern>,
    lower_negative: Vec<PermissionPattern>,
    upper_positive: Vec<PermissionPattern>,
    upper_negative: Vec<PermissionPattern>,
}

fn parse_grants(
    grants: &[permissions_types::PatternGrant],
) -> Result<Vec<PermissionPattern>, permissions_types::PermissionError> {
    grants.iter().map(parse_pattern_grant).collect()
}

fn derive_grant_sets(
    parent: &Card,
    lower_positive_to_retain: &[permissions_types::PatternGrant],
    lower_negative_to_add: &[permissions_types::PatternGrant],
    upper_positive_to_retain: &[permissions_types::PatternGrant],
    upper_negative_to_add: &[permissions_types::PatternGrant],
) -> Result<DerivedGrantSets, permissions_types::PermissionError> {
    let lower_positive = parse_grants(lower_positive_to_retain)?;
    let mut lower_negative = parent.lower_negative.clone();
    lower_negative.extend(parse_grants(lower_negative_to_add)?);
    let upper_positive = parse_grants(upper_positive_to_retain)?;
    let mut upper_negative = parent.upper_negative.clone();
    upper_negative.extend(parse_grants(upper_negative_to_add)?);

    DelegationSurface::from_cards(std::slice::from_ref(parent))
        .validate_attenuation(
            &lower_positive,
            &lower_negative,
            &upper_positive,
            &upper_negative,
        )
        .map_err(permission_error_from_algebra)?;

    Ok(DerivedGrantSets {
        lower_positive,
        lower_negative,
        upper_positive,
        upper_negative,
    })
}

fn parse_derived_grant_sets(
    lower_positive: &[permissions_types::PatternGrant],
    lower_negative: &[permissions_types::PatternGrant],
    upper_positive: &[permissions_types::PatternGrant],
    upper_negative: &[permissions_types::PatternGrant],
) -> Result<DerivedGrantSets, permissions_types::PermissionError> {
    Ok(DerivedGrantSets {
        lower_positive: parse_grants(lower_positive)?,
        lower_negative: parse_grants(lower_negative)?,
        upper_positive: parse_grants(upper_positive)?,
        upper_negative: parse_grants(upper_negative)?,
    })
}

fn select_wallet_derivation_parent(
    wallet: &[StoredCard],
    context: &golem_common::model::card::AgentPermissionMonomorphizationContext,
    grants: &DerivedGrantSets,
) -> Result<Card, permissions_types::PermissionError> {
    let surface = agent_delegation_surface_from_wallet(context, wallet);
    let parent_id = match surface
        .select_wallet_derivation_parent(
            &grants.lower_positive,
            &grants.lower_negative,
            &grants.upper_positive,
            &grants.upper_negative,
        )
        .map_err(permission_error_from_algebra)?
    {
        WalletDerivationParent::Single(parent_id) => parent_id,
        WalletDerivationParent::MultipleRequired => {
            return Err(
                permissions_types::PermissionError::WalletMultiSourceRequired(
                    "the requested child card requires more than one wallet source; derive from an explicit parent card instead"
                        .to_string(),
                ),
            );
        }
        WalletDerivationParent::NotPermitted => {
            return Err(permissions_types::PermissionError::NotPermitted(
                "no active wallet card permits the requested derivation".to_string(),
            ));
        }
    };

    let parent = wallet
        .iter()
        .find(|card| card.card_id() == parent_id)
        .expect("wallet derivation selector returned a card outside the wallet");
    Ok(monomorphize_card_for_agent(parent, context))
}

async fn durable_now<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<DateTime<Utc>> {
    let now =
        <DurableWorkerCtx<Ctx> as wasmtime_wasi::p2::bindings::clocks::wall_clock::Host>::now(ctx)
            .await?;
    let seconds = i64::try_from(now.seconds)
        .map_err(|_| anyhow!("durable wall-clock timestamp is out of range"))?;
    DateTime::from_timestamp(seconds, now.nanoseconds)
        .ok_or_else(|| anyhow!("durable wall-clock timestamp is invalid"))
}

async fn persist_runtime_card<Ctx, Pair, DeriveCardId, BuildCard>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    derive_card_id: DeriveCardId,
    build_card: BuildCard,
) -> anyhow::Result<StoredCard>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<
            Req = HostRequestPermissionCardDerive,
            Resp = HostResponsePermissionCardDerived,
        >,
    DeriveCardId: FnOnce(&DurableWorkerCtx<Ctx>, &IdempotencyKey, OplogIndex) -> CardId,
    BuildCard: FnOnce(CardId) -> StoredCard,
{
    let invocation_key = ctx
        .state
        .get_current_idempotency_key()
        .ok_or_else(|| anyhow!("runtime permission-card creation requires an active invocation"))?;
    // Runtime-card creation is byte-idempotent for its deterministic card ID. It therefore uses
    // the re-executable write class so a committed Start without an End can safely retry; the
    // generic remote-write class would make retryability depend on the guest's idempotence flag.
    let begun =
        CallHandle::<Pair, NotCancellable>::begin(ctx, DurableFunctionType::WriteLocal).await?;
    let oplog_index = begun.begin_index();
    let card_id = derive_card_id(ctx, &invocation_key, oplog_index);
    let provenance = CardManagedByRuntimeDerived {
        environment_id: ctx.owned_agent_id.environment_id,
        agent_id: ctx.owned_agent_id.agent_id.clone(),
        invocation_key,
        oplog_index,
    };
    let card = build_card(card_id);
    let request = HostRequestPermissionCardDerive {
        card: serialize(&card).map_err(|err| {
            anyhow!("failed to serialize runtime permission card {card_id}: {err}")
        })?,
        provenance: serialize(&provenance).map_err(|err| {
            anyhow!("failed to serialize provenance for permission card {card_id}: {err}")
        })?,
    };
    let handle = if begun.is_live() {
        begun.start_live(ctx, request).await?
    } else {
        begun.start_replay(ctx).await?
    };

    let card_for_creation = card.clone();
    let response = handle
        .run(ctx, async move |ctx| {
            let created = ctx
                .state
                .card_service
                .create_runtime_card(card_for_creation, provenance)
                .await?;
            if created.card_id() != card_id {
                return Err(anyhow!(
                    "runtime permission-card creation returned {}, expected {card_id}",
                    created.card_id()
                ));
            }
            let card = serialize(&created).map_err(|err| {
                anyhow!("failed to serialize runtime permission card {card_id}: {err}")
            })?;
            Ok::<_, anyhow::Error>(HostResponsePermissionCardDerived { card })
        })
        .await?;

    let created: StoredCard = deserialize(&response.card)
        .map_err(|err| anyhow!("failed to deserialize runtime permission card {card_id}: {err}"))?;
    if created.card_id() != card_id {
        return Err(anyhow!(
            "replayed runtime permission-card creation returned {}, expected {card_id}",
            created.card_id()
        ));
    }

    if ctx.state.snapshotting_mode.is_none() {
        let wallet_generation = Some(ctx.state.wallet_generation);
        if let Some((replayed_card, replayed_generation)) = ctx
            .state
            .replay_state
            .pending_card_derivation(card_id)
            .await
        {
            if replayed_card != created || replayed_generation != wallet_generation {
                return Err(anyhow!(
                    "replayed CardDerived audit event does not match permission card {card_id}"
                ));
            }
        } else if ctx.state.is_live() {
            // A replay that ended immediately after the durable response is live here as well. In
            // that crash window the persisted response proves registry creation succeeded, so
            // reconstruct the missing audit entry before exposing the handle.
            ctx.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::CardDerived {
                    timestamp: Timestamp::now_utc(),
                    card: created.clone(),
                    wallet_generation,
                })
                .await;
        } else {
            return Err(anyhow!(
                "replayed runtime permission-card creation {card_id} is missing its CardDerived audit event"
            ));
        }
        ctx.process_pending_replay_events().await?;
    }

    Ok(created)
}

async fn persist_derived_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    parent_id: CardId,
    grants: DerivedGrantSets,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Resource<PermissionCardHandleRep>> {
    let created = persist_runtime_card::<Ctx, GolemPermissionsDerivePersist, _, _>(
        ctx,
        |ctx, invocation_key, oplog_index| ctx.derive_card_id(invocation_key, oplog_index),
        |card_id| {
            StoredCard::Concrete(Card {
                card_id,
                parent_ids: vec![parent_id],
                lower_positive: grants.lower_positive,
                lower_negative: grants.lower_negative,
                upper_positive: grants.upper_positive,
                upper_negative: grants.upper_negative,
                created_at,
                expires_at,
                system_card: false,
                managed_by: None,
            })
        },
    )
    .await?;

    ctx.table()
        .push(PermissionCardHandleRep::new(
            PermissionCardEntry::from_card(created),
        ))
        .map_err(Into::into)
}

#[allow(
    dead_code,
    reason = "kept as a standalone durable primitive; transfers persist children within their outer call"
)]
pub(crate) async fn persist_installed_child_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    source: &PolymorphicCard,
    target: &AgentPermissionMonomorphizationContext,
) -> anyhow::Result<StoredCard> {
    let created_at = durable_now(ctx).await?;
    persist_runtime_card::<Ctx, GolemPermissionsInstallChildPersist, _, _>(
        ctx,
        |ctx, invocation_key, oplog_index| {
            ctx.derive_installed_child_card_id(invocation_key, oplog_index)
        },
        |child_card_id| {
            StoredCard::Concrete(instantiate_polymorphic_card_for_agent(
                source,
                target,
                child_card_id,
                created_at,
            ))
        },
    )
    .await
}

async fn derive_and_persist_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    parent: &Card,
    grants: DerivedGrantSets,
    lifespan: Option<&permissions_types::Duration>,
) -> anyhow::Result<Result<Resource<PermissionCardHandleRep>, permissions_types::PermissionError>> {
    let now = durable_now(ctx).await?;
    let expires_at = match derive_expires_at(now, lifespan, parent.expires_at) {
        Ok(expires_at) => expires_at,
        Err(error) => return Ok(Err(error)),
    };
    persist_derived_card(ctx, parent.card_id, grants, now, expires_at)
        .await
        .map(Ok)
}

fn ensure_card_permission<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    verb: CardVerb,
    resource: CardResourcePattern,
) -> Result<(), permissions_types::PermissionError> {
    ensure_card_permission_in_surface(
        &ctx.state.agent_effective_surface,
        &ctx.state.created_by_email,
        verb,
        resource,
    )
}

fn ensure_card_permission_in_surface(
    surface: &EffectiveSurface,
    created_by_email: &AccountEmail,
    verb: CardVerb,
    resource: CardResourcePattern,
) -> Result<(), permissions_types::PermissionError> {
    let request = PermissionTarget::Card(ClassPermissionTarget::<CardClass> {
        verb: Some(verb),
        owner: AccountOwnerPattern::Account {
            account: created_by_email.clone(),
        },
        resource,
    });

    if surface
        .authorize(&request)
        .map_err(permission_error_from_algebra)?
    {
        Ok(())
    } else {
        Err(permissions_types::PermissionError::NotPermitted(format!(
            "card:{} is not permitted for this agent",
            card_verb_name(verb)
        )))
    }
}

fn effective_surface_without_card(
    surface: &EffectiveSurface,
    excluded_card_id: CardId,
) -> EffectiveSurface {
    let Some(index) = surface
        .source_card_ids
        .iter()
        .position(|card_id| *card_id == excluded_card_id)
    else {
        return surface.clone();
    };

    let mut surface = surface.clone();
    surface.source_card_ids.remove(index);
    surface.lower.remove(index);
    surface.upper.remove(index);
    surface
}

fn ensure_install_permission_in_surface(
    surface: &EffectiveSurface,
    created_by_email: &AccountEmail,
    card_id: CardId,
    target: &RecipientPattern,
) -> Result<(), permissions_types::PermissionError> {
    ensure_card_permission_in_surface(
        &effective_surface_without_card(surface, card_id),
        created_by_email,
        CardVerb::Install,
        CardResourcePattern::InstallTarget(target.clone()),
    )
}

fn agent_recipient_pattern(context: &AgentPermissionMonomorphizationContext) -> RecipientPattern {
    RecipientPattern::Agent {
        account: context.account.clone(),
        application: context.application.clone(),
        environment: context.environment.clone(),
        component: context.component.clone(),
        agent_type: context.agent_type.clone(),
    }
}

#[derive(Clone, Debug)]
struct InstallAgentTarget {
    agent_id: AgentId,
    context: AgentPermissionMonomorphizationContext,
}

const UNSUPPORTED_INSTALL_TARGET_ERROR: &str = "install-card currently supports only agent targets";

fn require_agent_install_target(
    target: permissions_types::Holder,
) -> Result<AgentId, permissions_types::PermissionError> {
    match target {
        permissions_types::Holder::Agent(target) => Ok(target.into()),
        permissions_types::Holder::Account(_) | permissions_types::Holder::App(_) => {
            Err(permissions_types::PermissionError::NotPermitted(
                UNSUPPORTED_INSTALL_TARGET_ERROR.to_string(),
            ))
        }
    }
}

async fn resolve_install_target_context<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    target: AgentId,
) -> Result<InstallAgentTarget, permissions_types::PermissionError> {
    let owned_target = OwnedAgentId::new(ctx.owned_agent_id().environment_id(), &target);
    let target_revision = ctx
        .state
        .worker_service
        .get(&owned_target)
        .await
        .map(|metadata| {
            metadata
                .last_known_status
                .map(|status| status.component_revision)
                .unwrap_or(
                    metadata
                        .initial_worker_metadata
                        .last_known_status
                        .component_revision,
                )
        });
    let component = ctx
        .state
        .component_service
        .get_metadata(target.component_id, target_revision)
        .await
        .map_err(|error| {
            permissions_types::PermissionError::NotPermitted(format!(
                "install-card target metadata lookup failed: {error}"
            ))
        })?;
    let agent_type = ParsedAgentId::parse_agent_type_name(&target.agent_id).map_err(|error| {
        permissions_types::PermissionError::InvalidRecipient(format!(
            "invalid install target agent ID: {error}"
        ))
    })?;

    Ok(InstallAgentTarget {
        agent_id: target.clone(),
        context: AgentPermissionMonomorphizationContext {
            account: component.account_email,
            application: component.application_name,
            environment: component.environment_name,
            component: component.component_name,
            agent_name: target.agent_id,
            agent_type,
        },
    })
}

#[derive(Clone, Debug)]
struct CardTransferData {
    transfer_id: Uuid,
    source_card: StoredCard,
    installed_card: StoredCard,
    installed_card_provenance: Option<CardManagedByRuntimeDerived>,
    target_agent_id: AgentId,
}

impl CardTransferData {
    fn request(&self) -> anyhow::Result<HostRequestPermissionCardTransfer> {
        Ok(HostRequestPermissionCardTransfer {
            transfer_id: self.transfer_id,
            source_card: serialize(&self.source_card).map_err(|error| {
                anyhow!(
                    "failed to serialize source permission card {}: {error}",
                    self.source_card.card_id()
                )
            })?,
            installed_card: serialize(&self.installed_card).map_err(|error| {
                anyhow!(
                    "failed to serialize installed permission card {}: {error}",
                    self.installed_card.card_id()
                )
            })?,
            installed_card_provenance: self
                .installed_card_provenance
                .as_ref()
                .map(serialize)
                .transpose()
                .map_err(|error| {
                    anyhow!("failed to serialize installed permission-card provenance: {error}")
                })?,
            target_agent_id: self.target_agent_id.clone(),
        })
    }

    fn from_request(
        request: HostRequestPermissionCardTransfer,
    ) -> Result<Self, WorkerExecutorError> {
        let source_card: StoredCard = deserialize(&request.source_card).map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "failed to deserialize durable source permission card: {error}"
            ))
        })?;
        let installed_card: StoredCard = deserialize(&request.installed_card).map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "failed to deserialize durable installed permission card: {error}"
            ))
        })?;
        let installed_card_provenance = request
            .installed_card_provenance
            .map(|provenance| {
                deserialize(&provenance).map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "failed to deserialize durable installed permission-card provenance: {error}"
                    ))
                })
            })
            .transpose()?;

        match &source_card {
            StoredCard::Concrete(_)
                if source_card != installed_card || installed_card_provenance.is_some() =>
            {
                return Err(WorkerExecutorError::unexpected_oplog_entry(
                    "a concrete source card transferred without modification",
                    format!(
                        "source card {} and installed card {} differ",
                        source_card.card_id(),
                        installed_card.card_id()
                    ),
                ));
            }
            StoredCard::Polymorphic(_) => {
                let StoredCard::Concrete(installed_child) = &installed_card else {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "a concrete child for a polymorphic source transfer",
                        "a polymorphic installed card",
                    ));
                };
                if installed_child.parent_ids != [source_card.card_id()] {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "an installed child parented directly to its polymorphic source",
                        format!(
                            "installed card {} has parents {:?}",
                            installed_child.card_id, installed_child.parent_ids
                        ),
                    ));
                }
                if installed_card_provenance.is_none() {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        "runtime provenance for an installed polymorphic child",
                        "missing provenance",
                    ));
                }
            }
            StoredCard::Concrete(_) => {}
        }

        Ok(Self {
            transfer_id: request.transfer_id,
            source_card,
            installed_card,
            installed_card_provenance,
            target_agent_id: request.target_agent_id,
        })
    }

    fn source_holder<Ctx: WorkerCtx>(&self, ctx: &DurableWorkerCtx<Ctx>) -> CardHolder {
        CardHolder::Agent(AgentCardHolder {
            agent_id: ctx.owned_agent_id.agent_id.clone(),
        })
    }

    fn target_holder(&self) -> CardHolder {
        CardHolder::Agent(AgentCardHolder {
            agent_id: self.target_agent_id.clone(),
        })
    }
}

fn installed_card_matches_request(
    actual: &StoredCard,
    requested: &StoredCard,
    provenance: Option<&CardManagedByRuntimeDerived>,
) -> bool {
    if provenance.is_none() {
        return actual == requested;
    }
    match (actual, requested) {
        (StoredCard::Concrete(actual), StoredCard::Concrete(requested)) => {
            actual.card_id == requested.card_id
                && actual.parent_ids == requested.parent_ids
                && actual.lower_positive == requested.lower_positive
                && actual.lower_negative == requested.lower_negative
                && actual.upper_positive == requested.upper_positive
                && actual.upper_negative == requested.upper_negative
                && actual.created_at.timestamp_micros() == requested.created_at.timestamp_micros()
                && actual.expires_at.map(|value| value.timestamp_micros())
                    == requested.expires_at.map(|value| value.timestamp_micros())
                && actual.system_card == requested.system_card
                && actual.managed_by == provenance.cloned().map(CardManagedBy::RuntimeDerived)
                && requested.managed_by.is_none()
        }
        (StoredCard::Polymorphic(actual), StoredCard::Polymorphic(requested)) => {
            actual == requested
        }
        _ => false,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SourceTransferProgress {
    queued: bool,
    queued_card: Option<StoredCard>,
    derived_card: Option<StoredCard>,
    started: bool,
    confirmed: bool,
}

fn transfer_progress_mismatch(message: impl std::fmt::Display) -> WorkerExecutorError {
    WorkerExecutorError::unexpected_oplog_entry(
        "durable permission-card transfer state matching its persisted request",
        message.to_string(),
    )
}

async fn source_transfer_progress<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    start_index: OplogIndex,
    transfer: &CardTransferData,
) -> Result<SourceTransferProgress, WorkerExecutorError> {
    let current_index = ctx.state.oplog.current_oplog_index().await;
    if current_index < start_index {
        return Err(transfer_progress_mismatch(format!(
            "transfer start index {start_index} is beyond current oplog index {current_index}"
        )));
    }

    let entries = ctx
        .state
        .oplog
        .read_many(
            start_index,
            current_index.as_u64() - start_index.as_u64() + 1,
        )
        .await;
    let source_card_id = transfer.source_card.card_id();
    let installed_card_id = transfer.installed_card.card_id();
    let source_holder = transfer.source_holder(ctx);
    let target_holder = transfer.target_holder();
    let mut progress = SourceTransferProgress::default();

    for entry in entries.values() {
        match entry {
            OplogEntry::CardDerived { card, .. }
                if card.card_id() == installed_card_id
                    && transfer.installed_card_provenance.is_some() =>
            {
                if !installed_card_matches_request(
                    card,
                    &transfer.installed_card,
                    transfer.installed_card_provenance.as_ref(),
                ) {
                    return Err(transfer_progress_mismatch(format!(
                        "derived card {} conflicts with transfer {}",
                        installed_card_id, transfer.transfer_id
                    )));
                }
                if progress
                    .derived_card
                    .as_ref()
                    .is_some_and(|existing| existing != card)
                {
                    return Err(transfer_progress_mismatch(format!(
                        "transfer {} has multiple distinct derived card payloads",
                        transfer.transfer_id
                    )));
                }
                progress.derived_card = Some(card.clone());
            }
            OplogEntry::CardEventQueued {
                event: QueuedCardEvent::TransferStarted(event),
                ..
            } if event.transfer_id == transfer.transfer_id => {
                let Some(queued_card) = event.card.as_ref() else {
                    return Err(transfer_progress_mismatch(format!(
                        "queued transfer {} is missing its installed card payload",
                        transfer.transfer_id
                    )));
                };
                if event.card_id != source_card_id
                    || !installed_card_matches_request(
                        queued_card,
                        &transfer.installed_card,
                        transfer.installed_card_provenance.as_ref(),
                    )
                    || event.target_holder != target_holder
                {
                    return Err(transfer_progress_mismatch(format!(
                        "queued transfer {} has a conflicting payload or target",
                        transfer.transfer_id
                    )));
                }
                if progress
                    .queued_card
                    .as_ref()
                    .is_some_and(|existing| existing != queued_card)
                {
                    return Err(transfer_progress_mismatch(format!(
                        "queued transfer {} has multiple distinct installed card payloads",
                        transfer.transfer_id
                    )));
                }
                progress.queued = true;
                progress.queued_card = Some(queued_card.clone());
            }
            OplogEntry::CardTransferStarted {
                transfer_id,
                card_id,
                source_holder: recorded_source_holder,
                target_holder: recorded_target_holder,
                ..
            } if *transfer_id == transfer.transfer_id => {
                if *card_id != source_card_id
                    || recorded_source_holder.as_ref() != Some(&source_holder)
                    || *recorded_target_holder != target_holder
                {
                    return Err(transfer_progress_mismatch(format!(
                        "source transfer intent {} conflicts with its persisted request",
                        transfer.transfer_id
                    )));
                }
                progress.started = true;
            }
            OplogEntry::CardTransferConfirmed {
                transfer_id,
                source_card_id: recorded_source_card_id,
                installed_card_id: recorded_installed_card_id,
                target_holder: recorded_target_holder,
                ..
            } if *transfer_id == transfer.transfer_id => {
                if *recorded_source_card_id != source_card_id
                    || *recorded_installed_card_id != installed_card_id
                    || *recorded_target_holder != target_holder
                {
                    return Err(transfer_progress_mismatch(format!(
                        "source transfer receipt {} conflicts with its persisted request",
                        transfer.transfer_id
                    )));
                }
                progress.confirmed = true;
            }
            _ => {}
        }
    }

    if progress.started && !progress.queued {
        return Err(transfer_progress_mismatch(format!(
            "source transfer intent {} has no preceding queued payload",
            transfer.transfer_id
        )));
    }
    if progress.confirmed && !progress.started {
        return Err(transfer_progress_mismatch(format!(
            "source transfer receipt {} has no source intent",
            transfer.transfer_id
        )));
    }
    if progress.queued != progress.queued_card.is_some() {
        return Err(transfer_progress_mismatch(format!(
            "queued transfer {} has inconsistent payload state",
            transfer.transfer_id
        )));
    }
    if transfer.installed_card_provenance.is_some()
        && (progress.queued || progress.started || progress.confirmed)
        && progress.derived_card.is_none()
    {
        return Err(transfer_progress_mismatch(format!(
            "polymorphic transfer {} has no preceding CardDerived audit event",
            transfer.transfer_id
        )));
    }

    Ok(progress)
}

async fn load_card_transfer_request<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    start_index: OplogIndex,
) -> Result<HostRequestPermissionCardTransfer, WorkerExecutorError> {
    let entry = ctx.state.oplog.read(start_index).await;
    let (function_name, request) = match entry {
        OplogEntry::Start {
            function_name,
            request: Some(request),
            ..
        } => (function_name, request),
        other => {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "permission-card transfer Start with a request",
                format!("{other:?}"),
            ));
        }
    };
    if function_name != GolemPermissionsInstallTransfer::HOST_FUNCTION_NAME {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            GolemPermissionsInstallTransfer::FQFN,
            function_name.to_string(),
        ));
    }
    let request: HostRequest =
        ctx.state
            .oplog
            .download_payload(request)
            .await
            .map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to load durable permission-card transfer request: {error}"
                ))
            })?;
    request.try_into().map_err(|error| {
        WorkerExecutorError::unexpected_oplog_entry(
            "permission-card transfer request payload",
            error,
        )
    })
}

async fn complete_source_card_transfer<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    transfer_id: Uuid,
    source_card_id: CardId,
    installed_card: &StoredCard,
    target_agent_id: &AgentId,
    started: bool,
    remove_source_membership: bool,
) -> Result<(), WorkerExecutorError> {
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: target_agent_id.clone(),
    });

    if !started {
        if remove_source_membership
            && super::remove_wallet_card(
                &mut ctx.state.agent_wallet_cards,
                &mut ctx.state.wallet_generation,
                source_card_id,
            )?
        {
            ctx.rederive_agent_effective_surface_from_wallet();
            let wallet_card_ids = ctx
                .state
                .agent_wallet_cards
                .keys()
                .copied()
                .collect::<Vec<_>>();
            ctx.state
                .card_interest_index
                .set_card_interest(ctx.owned_agent_id.clone(), &wallet_card_ids)
                .await;
        }

        ctx.public_state
            .worker()
            .add_and_commit_oplog(OplogEntry::card_transfer_started(
                transfer_id,
                source_card_id,
                Some(CardHolder::Agent(AgentCardHolder {
                    agent_id: ctx.owned_agent_id.agent_id.clone(),
                })),
                target_holder.clone(),
                Some(ctx.state.wallet_generation),
            ))
            .await;
    }

    ctx.worker_proxy()
        .deliver_card_transfer(
            target_agent_id,
            ctx.owned_agent_id.environment_id,
            transfer_id,
            source_card_id,
            installed_card,
        )
        .await
        .map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "permission-card transfer delivery failed: {error}"
            ))
        })?;

    ctx.public_state
        .worker()
        .add_and_commit_oplog(OplogEntry::card_transfer_confirmed(
            transfer_id,
            source_card_id,
            installed_card.card_id(),
            target_holder,
        ))
        .await;

    Ok(())
}

async fn execute_source_card_transfer<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    start_index: OplogIndex,
    transfer: &CardTransferData,
) -> anyhow::Result<()> {
    ctx.process_pending_replay_events().await?;
    let progress = source_transfer_progress(ctx, start_index, transfer).await?;
    if progress.confirmed {
        return Ok(());
    }

    let mut installed_card = progress
        .queued_card
        .clone()
        .or_else(|| progress.derived_card.clone())
        .unwrap_or_else(|| transfer.installed_card.clone());
    if !progress.queued {
        if matches!(&transfer.source_card, StoredCard::Polymorphic(_))
            && progress.derived_card.is_none()
        {
            let provenance = transfer.installed_card_provenance.clone().ok_or_else(|| {
                anyhow!("polymorphic transfer is missing installed child provenance")
            })?;
            let persisted = ctx
                .state
                .card_service
                .create_runtime_card(transfer.installed_card.clone(), provenance)
                .await?;
            if !installed_card_matches_request(
                &persisted,
                &transfer.installed_card,
                transfer.installed_card_provenance.as_ref(),
            ) {
                return Err(anyhow!(
                    "persisted installed permission card {} differs from its durable transfer payload",
                    persisted.card_id()
                ));
            }
            installed_card = persisted;
            ctx.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::CardDerived {
                    timestamp: Timestamp::now_utc(),
                    card: installed_card.clone(),
                    wallet_generation: Some(ctx.state.wallet_generation),
                })
                .await;
        }

        ctx.public_state
            .worker()
            .add_and_commit_oplog(OplogEntry::card_event_queued(
                QueuedCardEvent::transfer_started_with_source(
                    transfer.transfer_id,
                    transfer.source_card.card_id(),
                    installed_card.clone(),
                    transfer.target_holder(),
                ),
            ))
            .await;
    }

    complete_source_card_transfer(
        ctx,
        transfer.transfer_id,
        transfer.source_card.card_id(),
        &installed_card,
        &transfer.target_agent_id,
        progress.started,
        matches!(&transfer.source_card, StoredCard::Concrete(_)),
    )
    .await?;

    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PendingSourceTransferProgress {
    started: bool,
    confirmed: bool,
}

async fn pending_source_transfer_progress<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    pending: &PendingCardEventRef,
    transfer: &QueuedCardEventTransfer,
    installed_card: &StoredCard,
    target_agent_id: &AgentId,
) -> Result<PendingSourceTransferProgress, WorkerExecutorError> {
    let current_index = ctx.state.oplog.current_oplog_index().await;
    if current_index < pending.oplog_index {
        return Err(transfer_progress_mismatch(format!(
            "pending transfer index {} is beyond current oplog index {current_index}",
            pending.oplog_index
        )));
    }

    let source_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: ctx.owned_agent_id.agent_id.clone(),
    });
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: target_agent_id.clone(),
    });
    let mut progress = PendingSourceTransferProgress::default();
    let mut found_queued_event = false;
    let mut next_index = pending.oplog_index;

    while next_index <= current_index {
        let remaining = current_index.as_u64() - next_index.as_u64() + 1;
        let count = remaining.min(1024);
        let entries = ctx.state.oplog.read_many(next_index, count).await;

        for entry in entries.values() {
            match entry {
                OplogEntry::CardEventQueued {
                    event: QueuedCardEvent::TransferStarted(recorded),
                    ..
                } if recorded.transfer_id == transfer.transfer_id => {
                    if recorded.card_id != transfer.card_id
                        || recorded.card.as_ref() != Some(installed_card)
                        || recorded.target_holder != target_holder
                    {
                        return Err(transfer_progress_mismatch(format!(
                            "queued transfer {} has a conflicting retry payload",
                            transfer.transfer_id
                        )));
                    }
                    found_queued_event = true;
                }
                OplogEntry::CardTransferStarted {
                    transfer_id,
                    card_id,
                    source_holder: recorded_source_holder,
                    target_holder: recorded_target_holder,
                    ..
                } if *transfer_id == transfer.transfer_id => {
                    if *card_id != transfer.card_id
                        || !recorded_source_holder
                            .as_ref()
                            .is_none_or(|holder| holder == &source_holder)
                        || *recorded_target_holder != target_holder
                    {
                        return Err(transfer_progress_mismatch(format!(
                            "source transfer intent {} conflicts with its queued retry payload",
                            transfer.transfer_id
                        )));
                    }
                    progress.started = true;
                }
                OplogEntry::CardTransferConfirmed {
                    transfer_id,
                    source_card_id,
                    installed_card_id,
                    target_holder: recorded_target_holder,
                    ..
                } if *transfer_id == transfer.transfer_id => {
                    if *source_card_id != transfer.card_id
                        || *installed_card_id != installed_card.card_id()
                        || *recorded_target_holder != target_holder
                    {
                        return Err(transfer_progress_mismatch(format!(
                            "source transfer receipt {} conflicts with its queued retry payload",
                            transfer.transfer_id
                        )));
                    }
                    progress.confirmed = true;
                }
                _ => {}
            }
        }

        next_index = OplogIndex::from_u64(next_index.as_u64() + count);
    }

    if !found_queued_event {
        return Err(transfer_progress_mismatch(format!(
            "pending transfer {} has no durable queued retry payload",
            transfer.transfer_id
        )));
    }
    if progress.confirmed && !progress.started {
        return Err(transfer_progress_mismatch(format!(
            "source transfer receipt {} has no source intent",
            transfer.transfer_id
        )));
    }

    Ok(progress)
}

fn pending_transfer_target_agent_id(
    transfer_id: Uuid,
    target_holder: &CardHolder,
) -> Result<&AgentId, WorkerExecutorError> {
    match target_holder {
        CardHolder::Agent(target_holder) => Ok(&target_holder.agent_id),
        CardHolder::Account(_) | CardHolder::Application(_) => Err(transfer_progress_mismatch(
            format!("pending transfer {transfer_id} targets an unsupported non-agent holder"),
        )),
    }
}

async fn retry_pending_source_card_transfer<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    pending: &PendingCardEventRef,
    transfer: &QueuedCardEventTransfer,
) -> Result<(), WorkerExecutorError> {
    let installed_card = transfer.card.as_ref().ok_or_else(|| {
        transfer_progress_mismatch(format!(
            "pending transfer {} is missing its installed card payload",
            transfer.transfer_id
        ))
    })?;
    let target_agent_id =
        pending_transfer_target_agent_id(transfer.transfer_id, &transfer.target_holder)?;
    let progress =
        pending_source_transfer_progress(ctx, pending, transfer, installed_card, target_agent_id)
            .await?;
    if progress.confirmed {
        return Ok(());
    }

    let remove_source_membership = if transfer.card_id == installed_card.card_id() {
        if !matches!(installed_card, StoredCard::Concrete(_)) {
            return Err(transfer_progress_mismatch(format!(
                "concrete source transfer {} carries a polymorphic installed card",
                transfer.transfer_id
            )));
        }
        true
    } else {
        let StoredCard::Concrete(installed_child) = installed_card else {
            return Err(transfer_progress_mismatch(format!(
                "polymorphic source transfer {} carries a polymorphic installed card",
                transfer.transfer_id
            )));
        };
        if installed_child.parent_ids != [transfer.card_id] {
            return Err(transfer_progress_mismatch(format!(
                "installed card {} is not parented directly to pending transfer source {}",
                installed_child.card_id, transfer.card_id
            )));
        }
        false
    };

    complete_source_card_transfer(
        ctx,
        transfer.transfer_id,
        transfer.card_id,
        installed_card,
        target_agent_id,
        progress.started,
        remove_source_membership,
    )
    .await
}

pub(super) async fn retry_pending_source_card_transfers<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError> {
    if !ctx.state.is_live() {
        return Ok(());
    }

    let pending_events = ctx.pending_card_events_at_boundary().await?;
    let mut first_error = None;
    for pending in pending_events {
        if let QueuedCardEvent::TransferStarted(transfer) = &pending.event
            && let Err(error) = retry_pending_source_card_transfer(ctx, &pending, transfer).await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn ensure_revoke_authority(
    target_and_ancestor_ids: &[CardId],
    active_wallet: &[StoredCard],
) -> Result<(), permissions_types::PermissionError> {
    if active_wallet
        .iter()
        .any(|card| target_and_ancestor_ids.contains(&card.card_id()))
    {
        Ok(())
    } else {
        Err(permissions_types::PermissionError::NotPermitted(
            "revoke requires possession of the target card or one of its live ancestors"
                .to_string(),
        ))
    }
}

async fn permission_card_revoke_response<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
    card_id: CardId,
) -> Result<HostResponsePermissionCardsRevoked, WorkerExecutorError> {
    if let Err(error) = ensure_card_permission(ctx, CardVerb::Revoke, CardResourcePattern::Any) {
        return Ok(HostResponsePermissionCardsRevoked {
            result: Err(permission_error_to_revoke_error(error)?),
        });
    }
    let card = match resolve_permission_card(ctx, handle).await {
        Ok(card) => card,
        Err(error) => {
            return Ok(HostResponsePermissionCardsRevoked {
                result: Err(permission_error_to_revoke_error(error)?),
            });
        }
    };
    if card.card_id() != card_id {
        return Err(WorkerExecutorError::runtime(format!(
            "permission-card revoke handle resolved to {}, expected {card_id}",
            card.card_id()
        )));
    }
    let active_wallet = ctx.active_agent_wallet_cards_snapshot().await?;
    let ancestor_ids = ctx
        .state
        .card_service
        .live_ancestor_ids_including_self(&card)
        .await?;
    if let Err(error) = ensure_revoke_authority(&ancestor_ids, &active_wallet) {
        return Ok(HostResponsePermissionCardsRevoked {
            result: Err(permission_error_to_revoke_error(error)?),
        });
    }

    let result = match ctx.state.card_service.revoke_card(card_id).await? {
        CardRevokeResult::Revoked(card_ids) => {
            Ok(card_ids.into_iter().map(|card_id| card_id.0).collect())
        }
        CardRevokeResult::AlreadyRevoked(message) => {
            Err(PermissionCardRevokeError::AlreadyRevoked(message))
        }
        CardRevokeResult::NotPermitted(message) => {
            Err(PermissionCardRevokeError::NotPermitted(message))
        }
    };
    Ok(HostResponsePermissionCardsRevoked { result })
}

fn permission_error_to_revoke_error(
    error: permissions_types::PermissionError,
) -> Result<PermissionCardRevokeError, WorkerExecutorError> {
    match error {
        permissions_types::PermissionError::CardRevoked(message) => {
            Ok(PermissionCardRevokeError::CardRevoked(message))
        }
        permissions_types::PermissionError::NotPermitted(message) => {
            Ok(PermissionCardRevokeError::NotPermitted(message))
        }
        error => Err(WorkerExecutorError::runtime(format!(
            "unexpected permission-card revoke error: {error:?}"
        ))),
    }
}

async fn complete_permission_card_revoke<Ctx: WorkerCtx>(
    mut handle: CallHandle<GolemPermissionsRevokePersist, NotCancellable>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    card_handle: &Resource<PermissionCardHandleRep>,
    card_id: CardId,
) -> anyhow::Result<HostResponsePermissionCardsRevoked> {
    let response = match permission_card_revoke_response(ctx, card_handle, card_id).await {
        Ok(response) => response,
        Err(error) => {
            handle.abandon_for_trap();
            return Err(error.into());
        }
    };
    let locally_revoked_card_ids = match &response.result {
        Ok(card_ids) => card_ids.iter().copied().map(CardId).collect::<Vec<_>>(),
        Err(PermissionCardRevokeError::AlreadyRevoked(_))
        | Err(PermissionCardRevokeError::CardRevoked(_)) => vec![card_id],
        Err(PermissionCardRevokeError::NotPermitted(_)) => Vec::new(),
    };
    if let Err(error) = ctx
        .apply_card_revoked_cascade(&locally_revoked_card_ids, false)
        .await
    {
        handle.abandon_for_trap();
        return Err(error.into());
    }
    handle.complete(ctx, response).await.map_err(Into::into)
}

async fn revoke_and_persist_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    card_handle: &Resource<PermissionCardHandleRep>,
) -> anyhow::Result<Result<u32, permissions_types::PermissionError>> {
    let snapshot = match permission_card_snapshot(ctx, card_handle) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Ok(Err(permissions_types::PermissionError::NotPermitted(
                format!("invalid permission-card handle: {error}"),
            )));
        }
    };
    let card_id = CardId(snapshot.card_id);
    let handle = CallHandle::<GolemPermissionsRevokePersist, NotCancellable>::start(
        ctx,
        HostRequestPermissionCardRevoke { card_id: card_id.0 },
        DurableFunctionType::WriteRemote,
    )
    .await?;
    let response = if handle.is_live() {
        complete_permission_card_revoke(handle, ctx, card_handle, card_id).await?
    } else {
        match handle.replay(ctx).await? {
            CallReplayOutcome::Replayed(response) => response,
            CallReplayOutcome::Incomplete(handle) => {
                complete_permission_card_revoke(handle, ctx, card_handle, card_id).await?
            }
        }
    };

    match response.result {
        Ok(revoked_card_ids) => {
            let revoked_card_ids = revoked_card_ids.into_iter().map(CardId).collect::<Vec<_>>();
            ctx.state
                .card_service
                .record_revoked_cards(&revoked_card_ids)
                .await;
            let count = u32::try_from(revoked_card_ids.len())
                .map_err(|_| anyhow!("permission-card revoke count exceeds u32"))?;
            Ok(Ok(count))
        }
        Err(PermissionCardRevokeError::AlreadyRevoked(message)) => {
            ctx.state
                .card_service
                .record_revoked_cards(&[card_id])
                .await;
            Ok(Err(permissions_types::PermissionError::CardRevoked(
                message,
            )))
        }
        Err(PermissionCardRevokeError::CardRevoked(message)) => {
            ctx.state
                .card_service
                .record_revoked_cards(&[card_id])
                .await;
            Ok(Err(permissions_types::PermissionError::CardRevoked(
                message,
            )))
        }
        Err(PermissionCardRevokeError::NotPermitted(message)) => Ok(Err(
            permissions_types::PermissionError::NotPermitted(message),
        )),
    }
}

impl<Ctx: WorkerCtx> HostPermissionCard for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<PermissionCardHandleRep>) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "golem::core::permission-card", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> PermissionCardResolver for DurableWorkerCtx<Ctx> {
    type Error = WorkerExecutorError;

    fn snapshot_permission_card_handle(
        &mut self,
        handle: Resource<PermissionCardHandleRep>,
    ) -> Result<PermissionCardValuePayload, Self::Error> {
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
        let entry = if let Some(card) = self.state.agent_wallet_cards.get(&CardId(snapshot.card_id))
        {
            if !snapshot_matches_card(snapshot, card) {
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

    async fn is_polymorphic(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<bool> {
        DurabilityHost::observe_function_call(self, "golem::permissions::types", "is-polymorphic");
        let card = resolve_permission_card_handle_or_trap(self, &c).await?;
        Ok(card.is_polymorphic())
    }
}

impl<Ctx: WorkerCtx> permissions_inspect::Host for DurableWorkerCtx<Ctx> {
    async fn inspect_card(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<Result<permissions_types::CardView, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(self, "golem::permissions::inspect", "inspect-card");
        let result = async {
            ensure_card_permission(self, CardVerb::Inspect, CardResourcePattern::Any)?;
            let card = resolve_permission_card_handle(self, &c).await?;
            card_view(&card)
        }
        .await;
        Ok(result)
    }
}

impl<Ctx: WorkerCtx> permissions_derive::Host for DurableWorkerCtx<Ctx> {
    async fn derive(
        &mut self,
        parent: Resource<PermissionCardHandleRep>,
        lower_positive_to_retain: Vec<permissions_types::PatternGrant>,
        lower_negative_to_add: Vec<permissions_types::PatternGrant>,
        upper_positive_to_retain: Vec<permissions_types::PatternGrant>,
        upper_negative_to_add: Vec<permissions_types::PatternGrant>,
        lifespan: Option<permissions_types::Duration>,
    ) -> anyhow::Result<Result<Resource<PermissionCardHandleRep>, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(self, "golem::permissions::derive", "derive");

        async {
            ensure_card_permission(self, CardVerb::Derive, CardResourcePattern::Any)?;
            let parent = resolve_permission_card_handle(self, &parent)
                .await?
                .into_persistent("derive")?;
            let StoredCard::Concrete(parent) = parent else {
                return Ok(Err(
                    permissions_types::PermissionError::CardPolymorphicNotUsable(
                        "derive requires a concrete parent card".to_string(),
                    ),
                ));
            };
            let grants = match derive_grant_sets(
                &parent,
                &lower_positive_to_retain,
                &lower_negative_to_add,
                &upper_positive_to_retain,
                &upper_negative_to_add,
            ) {
                Ok(grants) => grants,
                Err(error) => return Ok(Err(error)),
            };
            derive_and_persist_card(self, &parent, grants, lifespan.as_ref()).await
        }
        .await
    }

    async fn derive_from_wallet(
        &mut self,
        lower_positive: Vec<permissions_types::PatternGrant>,
        lower_negative: Vec<permissions_types::PatternGrant>,
        upper_positive: Vec<permissions_types::PatternGrant>,
        upper_negative: Vec<permissions_types::PatternGrant>,
        lifespan: Option<permissions_types::Duration>,
    ) -> anyhow::Result<Result<Resource<PermissionCardHandleRep>, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(
            self,
            "golem::permissions::derive",
            "derive-from-wallet",
        );

        async {
            ensure_card_permission(self, CardVerb::Derive, CardResourcePattern::Any)?;
            let grants = parse_derived_grant_sets(
                &lower_positive,
                &lower_negative,
                &upper_positive,
                &upper_negative,
            )?;
            let wallet = self.active_agent_wallet_cards_snapshot().await?;
            let Some(agent_id) = self.state.agent_id.as_ref() else {
                return Ok(Err(permissions_types::PermissionError::NotPermitted(
                    "derive-from-wallet is only available to an agent".to_string(),
                )));
            };
            let context = super::agent_monomorphization_context(
                &self.state.component_metadata,
                &self.owned_agent_id,
                agent_id,
            );
            let parent = match select_wallet_derivation_parent(&wallet, &context, &grants) {
                Ok(parent) => parent,
                Err(error) => return Ok(Err(error)),
            };
            derive_and_persist_card(self, &parent, grants, lifespan.as_ref()).await
        }
        .await
    }
}

impl<Ctx: WorkerCtx> permissions_revoke::Host for DurableWorkerCtx<Ctx> {
    async fn revoke_card(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<Result<u32, permissions_types::PermissionError>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::revoke", "revoke-card");
        let result = match permission_card_scope_card(self, &c) {
            Ok(Some(_)) => Ok(Err(scope_card_operation_error("revoke-card"))),
            Ok(None) | Err(_) => revoke_and_persist_card(self, &c).await,
        };
        self.drop_permission_card_handle(c);
        result
    }
}

impl<Ctx: WorkerCtx> permissions_wallet::Host for DurableWorkerCtx<Ctx> {
    async fn self_wallet(&mut self) -> anyhow::Result<Vec<Resource<PermissionCardHandleRep>>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::wallet", "self-wallet");
        let cards = self.active_agent_wallet_cards_snapshot().await?;
        cards
            .into_iter()
            .map(|card| {
                self.table().push(PermissionCardHandleRep::new(
                    PermissionCardEntry::from_card(card),
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    async fn self_version(&mut self) -> anyhow::Result<permissions_types::WalletVersionToken> {
        DurabilityHost::observe_function_call(self, "golem::permissions::wallet", "self-version");
        let _ = self.active_agent_wallet_cards_snapshot().await?;

        let wallet_id_hash = self.wallet_id_hash().to_vec();
        let generation = self.wallet_generation();

        Ok(permissions_types::WalletVersionToken {
            wallet_id_hash,
            generation,
        })
    }

    async fn install_card(
        &mut self,
        card: Resource<PermissionCardHandleRep>,
        target: permissions_types::Holder,
    ) -> anyhow::Result<Result<(), permissions_types::PermissionError>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::wallet", "install-card");
        let result: anyhow::Result<Result<(), permissions_types::PermissionError>> = async {
            let target = match require_agent_install_target(target) {
                Ok(target) => target,
                Err(error) => return Ok(Err(error)),
            };
            let source_card = match resolve_permission_card_handle(self, &card).await {
                Ok(card) => match card.into_persistent("install-card") {
                    Ok(card) => card,
                    Err(error) => return Ok(Err(error)),
                },
                Err(error) => return Ok(Err(error)),
            };
            let target = match resolve_install_target_context(self, target).await {
                Ok(target) => target,
                Err(error) => return Ok(Err(error)),
            };
            let target_recipient = agent_recipient_pattern(&target.context);
            if let Err(error) = ensure_install_permission_in_surface(
                &self.state.agent_effective_surface,
                &self.state.created_by_email,
                source_card.card_id(),
                &target_recipient,
            ) {
                return Ok(Err(error));
            }

            let invocation_key = self
                .state
                .get_current_idempotency_key()
                .ok_or_else(|| anyhow!("permission-card transfer requires an active invocation"))?;
            let begun = CallHandle::<GolemPermissionsInstallTransfer, NotCancellable>::begin(
                self,
                DurableFunctionType::WriteLocal,
            )
            .await?;
            let operation_index = begun.begin_index();
            let transfer_id = self.derive_transfer_id(&invocation_key, operation_index);
            let (installed_card, installed_card_provenance) = match &source_card {
                StoredCard::Concrete(_) => (source_card.clone(), None),
                StoredCard::Polymorphic(source) => {
                    let created_at = durable_now(self).await?;
                    (
                        StoredCard::Concrete(instantiate_polymorphic_card_for_agent(
                            source,
                            &target.context,
                            self.derive_installed_child_card_id(&invocation_key, operation_index),
                            created_at,
                        )),
                        Some(CardManagedByRuntimeDerived {
                            environment_id: self.owned_agent_id.environment_id,
                            agent_id: self.owned_agent_id.agent_id.clone(),
                            invocation_key: invocation_key.clone(),
                            oplog_index: operation_index,
                        }),
                    )
                }
            };
            let mut transfer = CardTransferData {
                transfer_id,
                source_card,
                installed_card,
                installed_card_provenance,
                target_agent_id: target.agent_id,
            };
            let request = transfer.request()?;
            let mut handle = if begun.is_live() {
                begun.start_live(self, request).await?
            } else {
                begun.start_replay(self).await?
            };
            if handle.is_live() {
                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::Always)
                    .await;
            }
            let start_index = handle.start_index();

            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(_) => {
                        self.process_pending_replay_events().await?;
                        return Ok(Ok(()));
                    }
                    CallReplayOutcome::Incomplete(live) => {
                        handle = live;
                        transfer = CardTransferData::from_request(
                            load_card_transfer_request(self, start_index).await?,
                        )?;
                    }
                }
            }

            if let Err(error) = execute_source_card_transfer(self, start_index, &transfer).await {
                handle.abandon_for_trap();
                return Err(error);
            }
            handle
                .complete(self, HostResponsePermissionCardTransferComplete {})
                .await?;
            Ok(Ok(()))
        }
        .await;
        self.drop_permission_card_handle(card);
        result
    }
}

impl<Ctx: WorkerCtx> permissions_kernel::Host for DurableWorkerCtx<Ctx> {
    async fn list_modules(&mut self) -> anyhow::Result<Vec<permissions_kernel::KernelModule>> {
        DurabilityHost::observe_function_call(
            self,
            "golem::permissions::kernel-introspection",
            "list-modules",
        );
        Ok(permission_class_metadata()
            .into_iter()
            .map(|metadata| permissions_kernel::KernelModule {
                class_name: metadata.class_name.to_string(),
                verbs: metadata
                    .verbs
                    .iter()
                    .map(|verb| (*verb).to_string())
                    .collect(),
            })
            .collect())
    }

    async fn validate_grant(
        &mut self,
        g: permissions_types::PatternGrant,
    ) -> anyhow::Result<Result<(), permissions_types::PermissionError>> {
        DurabilityHost::observe_function_call(
            self,
            "golem::permissions::kernel-introspection",
            "validate-grant",
        );
        Ok(parse_pattern_grant(&g).map(|_| ()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::application::ApplicationName;
    use golem_common::model::card::owner::PolymorphicAccountOwnerPattern;
    use golem_common::model::card::recipient::RecipientPattern;
    use golem_common::model::card::{
        AccountCardHolder, AgentPermissionMonomorphizationContext, ApplicationCardHolder,
        PolymorphicCard, PolymorphicClassPermissionPattern, card_matches_agent_recipient,
    };
    use golem_common::model::component::ComponentName;
    use golem_common::model::environment::EnvironmentName;
    use test_r::test;

    fn valid_filesystem_grant() -> permissions_types::PatternGrant {
        permissions_types::PatternGrant {
            class: "filesystem".to_string(),
            owner: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            verb: "read".to_string(),
            resource_id: "/data/**".to_string(),
        }
    }

    fn scope_card(card_id: CardId, root_card_ids: Vec<CardId>) -> ScopeCard {
        let grant = parse_pattern_grant(&valid_filesystem_grant()).unwrap();
        ScopeCard {
            scope_card_id: card_id,
            root_card_ids,
            lower_positive: vec![grant.clone()],
            lower_negative: vec![grant.clone()],
            upper_positive: vec![grant.clone()],
            upper_negative: vec![grant],
        }
    }

    fn comparable_grants(
        grants: &[permissions_types::PatternGrant],
    ) -> Vec<(String, String, String, String, String)> {
        grants
            .iter()
            .map(|grant| {
                (
                    grant.class.clone(),
                    grant.owner.clone(),
                    grant.recipient.clone(),
                    grant.verb.clone(),
                    grant.resource_id.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn scope_and_persistent_cards_share_the_metadata_and_inspection_shape() {
        let card_id = CardId(Uuid::from_u128(10));
        let parent_ids = vec![CardId(Uuid::from_u128(1)), CardId(Uuid::from_u128(2))];
        let scope = scope_card(card_id, parent_ids.clone());
        let persistent = StoredCard::Concrete(Card {
            card_id,
            parent_ids,
            lower_positive: scope.lower_positive.clone(),
            lower_negative: scope.lower_negative.clone(),
            upper_positive: scope.upper_positive.clone(),
            upper_negative: scope.upper_negative.clone(),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        });
        let persistent = ResolvedPermissionCard::Persistent(persistent);
        let scope = ResolvedPermissionCard::Scope(scope);

        assert_eq!(scope.card_id(), persistent.card_id());
        assert_eq!(scope.parent_ids(), persistent.parent_ids());
        assert_eq!(scope.expires_at(), persistent.expires_at());
        assert_eq!(scope.is_polymorphic(), persistent.is_polymorphic());

        let persistent_view = card_view(&persistent).unwrap();
        let scope_view = card_view(&scope).unwrap();
        assert_eq!(
            comparable_grants(&scope_view.lower_positive),
            comparable_grants(&persistent_view.lower_positive)
        );
        assert_eq!(
            comparable_grants(&scope_view.lower_negative),
            comparable_grants(&persistent_view.lower_negative)
        );
        assert_eq!(
            comparable_grants(&scope_view.upper_positive),
            comparable_grants(&persistent_view.upper_positive)
        );
        assert_eq!(
            comparable_grants(&scope_view.upper_negative),
            comparable_grants(&persistent_view.upper_negative)
        );
    }

    #[test]
    fn persistent_operations_reject_scope_cards() {
        let scope = scope_card(CardId(Uuid::from_u128(10)), Vec::new());

        for operation in ["derive", "revoke-card", "install-card"] {
            assert!(matches!(
                ResolvedPermissionCard::Scope(scope.clone()).into_persistent(operation),
                Err(permissions_types::PermissionError::NotPermitted(message))
                    if message == format!("{operation} does not accept scope cards")
            ));
        }
    }

    #[test]
    fn scope_cards_cannot_use_persistent_snapshot_transport() {
        let entry = PermissionCardEntry::Scope(scope_card(
            CardId(Uuid::from_u128(10)),
            vec![CardId(Uuid::from_u128(1))],
        ));

        let error = entry.into_snapshot().unwrap_err();
        assert!(error.to_string().contains(
            "scope-card handles cannot be serialized as persistent permission-card snapshots"
        ));
    }

    fn derive_authorization_surface() -> EffectiveSurface {
        let recipient =
            RecipientPattern::parse("alice@example.com/shop/prod/cart-svc/CartAgent").unwrap();
        let grant = parse_pattern_grant(&permissions_types::PatternGrant {
            class: "card".to_string(),
            owner: "alice@example.com".to_string(),
            recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            verb: "derive".to_string(),
            resource_id: "*".to_string(),
        })
        .unwrap();
        let card = Card {
            card_id: CardId::new(),
            parent_ids: Vec::new(),
            lower_positive: vec![grant],
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        };

        EffectiveSurface::from_cards(&[card], &recipient).unwrap()
    }

    fn install_authority_card(card_id: CardId, target: &RecipientPattern) -> StoredCard {
        let grant = parse_pattern_grant(&permissions_types::PatternGrant {
            class: "card".to_string(),
            owner: "alice@example.com".to_string(),
            recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            verb: "install".to_string(),
            resource_id: target.render(),
        })
        .unwrap();
        StoredCard::Concrete(Card {
            card_id,
            parent_ids: Vec::new(),
            lower_positive: vec![grant],
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        })
    }

    #[test]
    fn account_and_application_install_targets_are_rejected_at_the_wit_host_boundary() {
        let unsupported_targets = [
            permissions_types::Holder::Account(Uuid::new_v4().into()),
            permissions_types::Holder::App(Uuid::new_v4().into()),
        ];

        for target in unsupported_targets {
            let error = require_agent_install_target(target).unwrap_err();
            let permissions_types::PermissionError::NotPermitted(message) = error else {
                panic!("unsupported install target returned {error:?}")
            };
            assert_eq!(message, UNSUPPORTED_INSTALL_TARGET_ERROR);
        }
    }

    #[test]
    fn replay_recovery_rejects_account_and_application_transfer_targets() {
        let unsupported_targets = [
            CardHolder::Account(AccountCardHolder {
                account_id: Uuid::new_v4(),
            }),
            CardHolder::Application(ApplicationCardHolder {
                application_id: Uuid::new_v4(),
            }),
        ];

        for target in unsupported_targets {
            let transfer_id = Uuid::new_v4();
            let error = pending_transfer_target_agent_id(transfer_id, &target).unwrap_err();
            assert!(
                error.to_string().contains(&format!(
                    "pending transfer {transfer_id} targets an unsupported non-agent holder"
                )),
                "unexpected replay recovery error: {error}"
            );
        }
    }

    #[test]
    fn install_card_authority_is_scoped_to_the_concrete_target() {
        let account = AccountEmail::from("alice@example.com");
        let caller =
            RecipientPattern::parse("alice@example.com/shop/prod/cart-svc/CartAgent").unwrap();
        let target =
            RecipientPattern::parse("alice@example.com/shop/prod/worker-svc/WorkerAgent").unwrap();
        let other_target =
            RecipientPattern::parse("alice@example.com/shop/prod/other-svc/OtherAgent").unwrap();
        let authority = install_authority_card(CardId::new(), &target);
        let surface = EffectiveSurface::from_cards(
            &[monomorphize_card_for_agent(
                &authority,
                &wallet_derivation_context(),
            )],
            &caller,
        )
        .unwrap();

        assert!(
            ensure_install_permission_in_surface(&surface, &account, CardId::new(), &target,)
                .is_ok()
        );
        assert!(matches!(
            ensure_install_permission_in_surface(&surface, &account, CardId::new(), &other_target,),
            Err(permissions_types::PermissionError::NotPermitted(_))
        ));
    }

    #[test]
    fn install_card_cannot_authorize_its_own_installation() {
        let account = AccountEmail::from("alice@example.com");
        let caller =
            RecipientPattern::parse("alice@example.com/shop/prod/cart-svc/CartAgent").unwrap();
        let target =
            RecipientPattern::parse("alice@example.com/shop/prod/worker-svc/WorkerAgent").unwrap();
        let transferred_card_id = CardId::new();
        let transferred = install_authority_card(transferred_card_id, &target);
        let card = monomorphize_card_for_agent(&transferred, &wallet_derivation_context());
        let surface = EffectiveSurface::from_cards(&[card], &caller).unwrap();

        assert!(matches!(
            ensure_install_permission_in_surface(&surface, &account, transferred_card_id, &target,),
            Err(permissions_types::PermissionError::NotPermitted(_))
        ));
    }

    #[test]
    fn install_card_recipients_must_cover_the_target() {
        let context = wallet_derivation_context();
        let matching_recipient =
            RecipientPattern::parse("alice@example.com/shop/prod/*/*").unwrap();
        let mismatching_recipient =
            RecipientPattern::parse("alice@example.com/shop/staging/*/*").unwrap();
        let matching = wallet_parent(
            1,
            &permissions_types::PatternGrant {
                recipient: matching_recipient.render(),
                ..valid_filesystem_grant()
            },
        );
        let mismatching = wallet_parent(
            2,
            &permissions_types::PatternGrant {
                recipient: mismatching_recipient.render(),
                ..valid_filesystem_grant()
            },
        );
        let matching_negative_only = StoredCard::Concrete(Card {
            card_id: CardId::new(),
            parent_ids: Vec::new(),
            lower_positive: Vec::new(),
            lower_negative: vec![
                parse_pattern_grant(&permissions_types::PatternGrant {
                    recipient: matching_recipient.render(),
                    ..valid_filesystem_grant()
                })
                .unwrap(),
            ],
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        });
        let polymorphic = StoredCard::Polymorphic(PolymorphicCard {
            card_id: CardId::new(),
            parent_ids: Vec::new(),
            lower_positive: vec![PolymorphicPermissionPattern::Card(
                PolymorphicClassPermissionPattern {
                    verb: Some(CardVerb::Inspect),
                    owner: PolymorphicAccountOwnerPattern::Account,
                    recipient: matching_recipient,
                    resource: CardResourcePattern::Any,
                },
            )],
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            expires_at: None,
            system_card: false,
        });

        assert!(card_matches_agent_recipient(&matching, &context));
        assert!(card_matches_agent_recipient(&polymorphic, &context));
        assert!(!card_matches_agent_recipient(&mismatching, &context));
        assert!(!card_matches_agent_recipient(
            &matching_negative_only,
            &context
        ));
    }

    #[test]
    fn installed_child_request_matches_registry_canonicalization() {
        let requested = StoredCard::Concrete(Card {
            card_id: CardId::new(),
            parent_ids: vec![CardId::new()],
            lower_positive: vec![parse_pattern_grant(&valid_filesystem_grant()).unwrap()],
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap(),
            expires_at: Some(DateTime::from_timestamp(1_800_000_000, 987_654_321).unwrap()),
            system_card: false,
            managed_by: None,
        });
        let provenance = CardManagedByRuntimeDerived {
            environment_id: golem_common::model::environment::EnvironmentId(Uuid::new_v4()),
            agent_id: AgentId {
                component_id: golem_common::model::component::ComponentId(Uuid::new_v4()),
                agent_id: "source-agent".to_string(),
            },
            invocation_key: IdempotencyKey::fresh(),
            oplog_index: OplogIndex::from_u64(42),
        };
        let mut canonical = requested.clone();
        if let StoredCard::Concrete(canonical_card) = &mut canonical {
            canonical_card.created_at =
                DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap();
            canonical_card.expires_at =
                Some(DateTime::from_timestamp(1_800_000_000, 987_654_000).unwrap());
            canonical_card.managed_by = Some(CardManagedBy::RuntimeDerived(provenance.clone()));
        }

        assert!(installed_card_matches_request(
            &canonical,
            &requested,
            Some(&provenance),
        ));

        if let StoredCard::Concrete(canonical_card) = &mut canonical {
            canonical_card.parent_ids.push(CardId::new());
        }
        assert!(!installed_card_matches_request(
            &canonical,
            &requested,
            Some(&provenance),
        ));
    }

    #[test]
    fn derive_requires_the_global_card_derive_permission() {
        let account = AccountEmail::from("alice@example.com");
        let surface = derive_authorization_surface();

        assert!(
            ensure_card_permission_in_surface(
                &surface,
                &account,
                CardVerb::Derive,
                CardResourcePattern::Any,
            )
            .is_ok()
        );
        assert!(matches!(
            ensure_card_permission_in_surface(
                &EffectiveSurface::default(),
                &account,
                CardVerb::Derive,
                CardResourcePattern::Any,
            ),
            Err(permissions_types::PermissionError::NotPermitted(_))
        ));
        assert!(matches!(
            ensure_card_permission_in_surface(
                &surface,
                &account,
                CardVerb::Revoke,
                CardResourcePattern::Any,
            ),
            Err(permissions_types::PermissionError::NotPermitted(_))
        ));
    }

    #[test]
    fn revoke_requires_possession_of_the_target_or_a_live_ancestor() {
        let target = wallet_parent(3, &valid_filesystem_grant());
        let ancestor = wallet_parent(2, &valid_filesystem_grant());
        let unrelated = wallet_parent(4, &valid_filesystem_grant());
        let target_and_ancestors = [
            CardId(Uuid::from_u128(1)),
            ancestor.card_id(),
            target.card_id(),
        ];

        assert!(
            ensure_revoke_authority(&target_and_ancestors, std::slice::from_ref(&target)).is_ok()
        );
        assert!(
            ensure_revoke_authority(&target_and_ancestors, std::slice::from_ref(&ancestor)).is_ok()
        );
        assert!(matches!(
            ensure_revoke_authority(&target_and_ancestors, std::slice::from_ref(&unrelated)),
            Err(permissions_types::PermissionError::NotPermitted(_))
        ));
        assert!(matches!(
            ensure_revoke_authority(&target_and_ancestors, &[]),
            Err(permissions_types::PermissionError::NotPermitted(_))
        ));
    }

    #[test]
    fn attenuation_errors_preserve_the_failed_bound() {
        let grant = parse_pattern_grant(&valid_filesystem_grant()).unwrap();
        let rendered = grant.render().unwrap();

        assert!(matches!(
            permission_error_from_algebra(CardAlgebraError::LowerBoundTooBroad {
                grant: Box::new(grant.clone()),
            }),
            permissions_types::PermissionError::LowerBoundTooBroad(message)
                if message == rendered
        ));
        assert!(matches!(
            permission_error_from_algebra(CardAlgebraError::UpperBoundTooBroad {
                grant: Some(Box::new(grant)),
            }),
            permissions_types::PermissionError::UpperBoundTooBroad(message)
                if message == rendered
        ));
        assert!(matches!(
            permission_error_from_algebra(CardAlgebraError::UpperBoundTooBroad { grant: None }),
            permissions_types::PermissionError::UpperBoundTooBroad(message)
                if message == "upper-bound grant is too broad"
        ));
    }

    fn concrete_parent(grant: &permissions_types::PatternGrant) -> Card {
        let positive = parse_pattern_grant(grant).unwrap();
        let mut lower_negative = grant.clone();
        lower_negative.resource_id = "/data/private/**".to_string();
        let mut upper_negative = grant.clone();
        upper_negative.resource_id = "/data/secret/**".to_string();

        Card {
            card_id: CardId::new(),
            parent_ids: Vec::new(),
            lower_positive: vec![positive.clone()],
            lower_negative: vec![parse_pattern_grant(&lower_negative).unwrap()],
            upper_positive: vec![positive],
            upper_negative: vec![parse_pattern_grant(&upper_negative).unwrap()],
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        }
    }

    fn wallet_derivation_context() -> AgentPermissionMonomorphizationContext {
        AgentPermissionMonomorphizationContext {
            account: AccountEmail::from("alice@example.com"),
            application: ApplicationName::try_from("shop").unwrap(),
            environment: EnvironmentName::try_from("prod").unwrap(),
            component: ComponentName("cart-svc".to_string()),
            agent_name: "cart-1".to_string(),
            agent_type: AgentTypeName("CartAgent".to_string()),
        }
    }

    fn wallet_parent(card_id: u128, grant: &permissions_types::PatternGrant) -> StoredCard {
        StoredCard::Concrete(Card {
            card_id: CardId(Uuid::from_u128(card_id)),
            parent_ids: Vec::new(),
            lower_positive: vec![parse_pattern_grant(grant).unwrap()],
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        })
    }

    #[test]
    fn wallet_derivation_returns_the_deterministically_selected_parent() {
        let parent_grant = valid_filesystem_grant();
        let mut child_grant = parent_grant.clone();
        child_grant.resource_id = "/data/public.txt".to_string();
        let wallet = [
            wallet_parent(1, &parent_grant),
            wallet_parent(2, &parent_grant),
        ];
        let grants = parse_derived_grant_sets(&[child_grant], &[], &[], &[]).unwrap();

        let parent =
            select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &grants)
                .unwrap();

        assert_eq!(parent.card_id, CardId(Uuid::from_u128(2)));
    }

    #[test]
    fn wallet_derivation_maps_multi_source_selection_to_typed_error() {
        let mut first = valid_filesystem_grant();
        first.resource_id = "/data/first.txt".to_string();
        let mut second = valid_filesystem_grant();
        second.resource_id = "/data/second.txt".to_string();
        let wallet = [wallet_parent(1, &first), wallet_parent(2, &second)];
        let grants = parse_derived_grant_sets(&[first, second], &[], &[], &[]).unwrap();

        assert!(matches!(
            select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &grants,),
            Err(permissions_types::PermissionError::WalletMultiSourceRequired(_))
        ));
    }

    #[test]
    fn wallet_derivation_rejects_an_empty_active_wallet() {
        let grants = parse_derived_grant_sets(&[], &[], &[], &[]).unwrap();

        assert!(matches!(
            select_wallet_derivation_parent(&[], &wallet_derivation_context(), &grants,),
            Err(permissions_types::PermissionError::NotPermitted(_))
        ));
    }

    #[test]
    fn wallet_derivation_selects_parents_by_raw_recipient_not_holder_projection() {
        let mut grant_for_another_holder = valid_filesystem_grant();
        grant_for_another_holder.recipient =
            "alice@example.com/shop/prod/cart-svc/OtherAgent".to_string();
        let wallet = [wallet_parent(1, &grant_for_another_holder)];
        let grants = parse_derived_grant_sets(
            std::slice::from_ref(&grant_for_another_holder),
            &[],
            &[],
            &[],
        )
        .unwrap();

        let parent =
            select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &grants)
                .expect("raw recipient delegation must be permitted");

        assert_eq!(parent.card_id, CardId(Uuid::from_u128(1)));
    }

    #[test]
    fn wallet_derivation_treats_negative_lists_as_complete_child_bounds() {
        let positive = valid_filesystem_grant();
        let parent = concrete_parent(&positive);
        let lower_negative = concrete_pattern_grant(&parent.lower_negative[0]).unwrap();
        let upper_negative = concrete_pattern_grant(&parent.upper_negative[0]).unwrap();
        let wallet = [StoredCard::Concrete(parent.clone())];
        let incomplete = parse_derived_grant_sets(
            std::slice::from_ref(&positive),
            &[],
            std::slice::from_ref(&positive),
            std::slice::from_ref(&upper_negative),
        )
        .unwrap();

        assert!(matches!(
            select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &incomplete,),
            Err(permissions_types::PermissionError::LowerBoundTooBroad(_))
        ));

        let complete = parse_derived_grant_sets(
            std::slice::from_ref(&positive),
            std::slice::from_ref(&lower_negative),
            std::slice::from_ref(&positive),
            std::slice::from_ref(&upper_negative),
        )
        .unwrap();
        assert_eq!(
            select_wallet_derivation_parent(&wallet, &wallet_derivation_context(), &complete,)
                .unwrap()
                .card_id,
            parent.card_id
        );
    }

    #[test]
    fn derive_grants_retain_positives_and_append_negatives() {
        let retained = valid_filesystem_grant();
        let parent = concrete_parent(&retained);
        let mut lower_negative_to_add = retained.clone();
        lower_negative_to_add.resource_id = "/data/tmp/**".to_string();
        let mut upper_negative_to_add = retained.clone();
        upper_negative_to_add.resource_id = "/data/admin/**".to_string();

        let grants = derive_grant_sets(
            &parent,
            std::slice::from_ref(&retained),
            std::slice::from_ref(&lower_negative_to_add),
            std::slice::from_ref(&retained),
            std::slice::from_ref(&upper_negative_to_add),
        )
        .unwrap();

        assert_eq!(grants.lower_positive, parent.lower_positive);
        assert_eq!(grants.upper_positive, parent.upper_positive);
        assert_eq!(
            grants.lower_negative,
            [
                parent.lower_negative.clone(),
                vec![parse_pattern_grant(&lower_negative_to_add).unwrap()]
            ]
            .concat()
        );
        assert_eq!(
            grants.upper_negative,
            [
                parent.upper_negative.clone(),
                vec![parse_pattern_grant(&upper_negative_to_add).unwrap()]
            ]
            .concat()
        );
    }

    #[test]
    fn derive_grants_reject_a_positive_broader_than_the_parent() {
        let parent_grant = valid_filesystem_grant();
        let parent = concrete_parent(&parent_grant);
        let mut broader = parent_grant.clone();
        broader.resource_id = "/**".to_string();

        assert!(matches!(
            derive_grant_sets(
                &parent,
                &[broader],
                &[],
                std::slice::from_ref(&parent_grant),
                &[],
            ),
            Err(permissions_types::PermissionError::LowerBoundTooBroad(_))
        ));
    }

    #[test]
    fn derive_grants_reject_an_upper_positive_broader_than_the_parent() {
        let parent_grant = valid_filesystem_grant();
        let parent = concrete_parent(&parent_grant);
        let mut broader = parent_grant.clone();
        broader.resource_id = "/**".to_string();

        assert!(matches!(
            derive_grant_sets(
                &parent,
                std::slice::from_ref(&parent_grant),
                &[],
                &[broader],
                &[],
            ),
            Err(permissions_types::PermissionError::UpperBoundTooBroad(_))
        ));
    }

    #[test]
    fn parse_pattern_grant_rejects_invalid_structured_fields_after_separator_injection() {
        let grant = permissions_types::PatternGrant {
            class: "filesystem".to_string(),
            owner: "acme/shop/prod/cart/agent) @ acme/shop/prod/cart/agent : read : /tmp"
                .to_string(),
            recipient: "not a valid recipient".to_string(),
            verb: "not-a-real-verb".to_string(),
            resource_id: "not a valid resource".to_string(),
        };

        assert!(parse_pattern_grant(&grant).is_err());
    }

    #[test]
    fn parse_pattern_grant_accepts_valid_account_emails_in_paths() {
        let grant = permissions_types::PatternGrant {
            class: "filesystem".to_string(),
            owner: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            verb: "read".to_string(),
            resource_id: "/data/**".to_string(),
        };

        assert!(parse_pattern_grant(&grant).is_ok());
    }

    #[test]
    fn parse_pattern_grant_accepts_valid_agent_owner_type_wildcard() {
        let grant = permissions_types::PatternGrant {
            class: "filesystem".to_string(),
            owner: "alice@example.com/shop/prod/cart-svc/CartAgent(*)".to_string(),
            recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            verb: "read".to_string(),
            resource_id: "/data/**".to_string(),
        };

        assert!(parse_pattern_grant(&grant).is_ok());
    }

    #[test]
    fn concrete_pattern_grant_preserves_valid_structured_fields_with_delimiters() {
        let grant = permissions_types::PatternGrant {
            class: "filesystem".to_string(),
            owner: "alice@example.com/shop/prod/cart-svc/CartAgent(\"id) @ spoof : read : tmp\")"
                .to_string(),
            recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            verb: "read".to_string(),
            resource_id: "/data/**".to_string(),
        };

        let pattern = parse_pattern_grant(&grant).expect("structured grant should be valid");
        let rendered = concrete_pattern_grant(&pattern).expect("valid pattern should inspect");

        assert_eq!(rendered.class, grant.class);
        assert_eq!(rendered.owner, grant.owner);
        assert_eq!(rendered.recipient, grant.recipient);
        assert_eq!(rendered.verb, grant.verb);
        assert_eq!(rendered.resource_id, grant.resource_id);
    }

    #[test]
    fn concrete_pattern_grant_preserves_multi_segment_tool_command() {
        let grant = permissions_types::PatternGrant {
            class: "tool".to_string(),
            owner: "alice@example.com/shop/prod/cli-tools/grep".to_string(),
            recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            verb: "invoke".to_string(),
            resource_id: "search.files --pattern=* --path=src/** -in README.md".to_string(),
        };

        let pattern = parse_pattern_grant(&grant).expect("structured grant should be valid");
        let inspected = concrete_pattern_grant(&pattern).expect("valid pattern should inspect");
        let reparsed =
            parse_pattern_grant(&inspected).expect("an inspected grant should remain valid");

        assert_eq!(reparsed, pattern);
    }

    #[test]
    fn parse_pattern_grant_rejects_verb_suffix_that_completes_resource_separator() {
        let grant = permissions_types::PatternGrant {
            class: "config".to_string(),
            owner: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            recipient: "alice@example.com/shop/prod/cart-svc/CartAgent".to_string(),
            verb: "read :".to_string(),
            resource_id: "ignored".to_string(),
        };

        assert!(matches!(
            parse_pattern_grant(&grant),
            Err(permissions_types::PermissionError::InvalidVerb(verb)) if verb == "read :"
        ));
    }

    fn lifespan(value: &str) -> permissions_types::Duration {
        permissions_types::Duration {
            iso_8601: value.to_string(),
        }
    }

    #[test]
    fn derive_expiry_parses_fixed_iso_8601_duration() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();

        assert_eq!(
            derive_expires_at(now, Some(&lifespan("P1DT2H3M4.5S")), None).unwrap(),
            Some(
                now + TimeDelta::days(1)
                    + TimeDelta::hours(2)
                    + TimeDelta::minutes(3)
                    + TimeDelta::seconds(4)
                    + TimeDelta::milliseconds(500)
            )
        );
        assert_eq!(
            derive_expires_at(now, Some(&lifespan("PT0S")), None).unwrap(),
            Some(now)
        );
    }

    #[test]
    fn derive_expiry_rejects_non_fixed_or_malformed_duration() {
        for value in ["1s", "-PT1S", "P1Y", "P1M", "P1W", "PT", ""] {
            assert!(
                matches!(
                    derive_expires_at(
                        DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                        Some(&lifespan(value)),
                        None,
                    ),
                    Err(permissions_types::PermissionError::InvalidPattern(_))
                ),
                "{value} must be rejected"
            );
        }
    }

    #[test]
    fn derive_expiry_enforces_parent_expiry() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let parent_expires_at = now + TimeDelta::hours(1);

        assert_eq!(
            derive_expires_at(now, Some(&lifespan("PT1H")), Some(parent_expires_at),).unwrap(),
            Some(parent_expires_at)
        );
        assert!(matches!(
            derive_expires_at(
                now,
                Some(&lifespan("PT1H0.000000001S")),
                Some(parent_expires_at),
            ),
            Err(permissions_types::PermissionError::CardExpired(_))
        ));
        assert!(matches!(
            derive_expires_at(now, None, Some(parent_expires_at)),
            Err(permissions_types::PermissionError::CardExpired(_))
        ));
        assert!(matches!(
            derive_expires_at(now, Some(&lifespan("PT0S")), Some(now)),
            Err(permissions_types::PermissionError::CardExpired(_))
        ));
    }

    #[test]
    fn derive_expiry_handles_indefinite_parent_and_timestamp_overflow() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();

        assert_eq!(derive_expires_at(now, None, None).unwrap(), None);
        assert!(matches!(
            derive_expires_at(
                DateTime::<Utc>::MAX_UTC,
                Some(&lifespan("PT0.000000001S")),
                None,
            ),
            Err(permissions_types::PermissionError::InvalidPattern(_))
        ));
    }
}
