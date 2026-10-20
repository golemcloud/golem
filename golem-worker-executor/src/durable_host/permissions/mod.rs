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

mod creation;
mod decisions;
mod derive;
mod grants;
mod handles;
mod inspect;
mod revoke;
mod transfer;
mod wallet;

#[cfg(test)]
use derive::{
    build_scope_card, derive_grant_sets, ensure_persistent_derivation_repair_authority,
    parse_derived_grant_sets, persistent_scope_parent_from_wallet, select_wallet_derivation_parent,
    wallet_parent_for_agent,
};
#[cfg(test)]
use grants::{concrete_pattern_grant, concrete_pattern_grants};
#[cfg(test)]
use handles::resolve_owned_scope_card;
#[cfg(test)]
use revoke::ensure_revoke_authority;
#[cfg(test)]
use transfer::{installed_card_matches_request, pending_transfer_target_agent_id};

use creation::persist_runtime_card;
use creation::refused_runtime_card_parent;
use decisions::derive_error_to_permission_error;
use decisions::permission_error_from_serializable;
use decisions::permission_error_to_anyhow;
use decisions::permission_error_to_derive_error;
use decisions::permission_error_to_serializable;
use decisions::record_card_decision;
use decisions::worker_error_to_permission_error;
use grants::card_verb_name;
use grants::card_view;
use grants::derive_expires_at;
use grants::normalize_tool_grants;
#[cfg(test)]
use grants::parse_grants;
use grants::parse_pattern_grant;
use grants::permission_error_from_algebra;
use grants::permission_error_from_parse;
use handles::handle_card_id;
use handles::permission_card_scope_card;
use handles::permission_card_snapshot;
pub(crate) use handles::resolve_invocation_scope_card;
use handles::resolve_permission_card;
use handles::resolve_permission_card_handle;
use handles::resolve_permission_card_within_recorded_call;
pub(crate) use handles::retire_invocation_scope_handle;
use handles::scope_card_operation_error;
use transfer::agent_recipient_pattern;
use transfer::decide_install;
use transfer::execute_source_card_transfer;
use transfer::load_card_transfer_request;
pub(crate) use transfer::prepare_pending_source_card_transfer_retry;
use transfer::require_agent_install_target;

use crate::durable_host::concurrent::{
    CallReplayOutcome, DurableCallSession, NotCancellable, ResolvedCall,
};
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::permissions::derive as permissions_derive;
use crate::preview2::golem::permissions::inspect as permissions_inspect;
use crate::preview2::golem::permissions::kernel_introspection as permissions_kernel;
use crate::preview2::golem::permissions::revoke as permissions_revoke;
use crate::preview2::golem::permissions::types as permissions_types;
use crate::preview2::golem::permissions::wallet as permissions_wallet;
use crate::services::HasWorker;
use crate::services::card::{CardRevokeResult, CardState, RuntimeCardCreation};
use crate::services::oplog::{CommitLevel, Oplog, OplogOps};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use chrono::{DateTime, TimeDelta, Utc};
use desert_rust::{BinaryCodec, BinaryDeserializer, BinarySerializer};
use golem_common::base_model::oplog::QueuedCardEventTransfer;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::recipient::{RecipientOwnerContext, RecipientPattern};
use golem_common::model::card::{
    AgentCardHolder, AgentPermissionMonomorphizationContext, Card, CardAlgebraError, CardClass,
    CardHolder, CardId, CardManagedBy, CardManagedByRuntimeDerived, CardParseError,
    CardResourcePattern, CardVerb, ClassPermissionTarget, DelegationCard, DelegationSurface,
    EffectiveSurface, PermissionPattern, PermissionTarget, PolymorphicCard,
    PolymorphicPermissionPattern, RenderedPermissionFields, ScopeCard, StoredCard,
    WalletDerivationParent, agent_delegation_surface_from_wallet, card_matches_agent_recipient,
    instantiate_polymorphic_card_for_agent, monomorphize_card_for_agent, parse_permission_fields,
    parse_polymorphic_permission_fields, permission_card_envelope, permission_class_metadata,
};
use golem_common::model::oplog::host_functions::{
    GolemPermissionsCardLiveness, GolemPermissionsDeriveDecision,
    GolemPermissionsDeriveFromWalletDecision, GolemPermissionsDerivePersist,
    GolemPermissionsDeriveScopeDecision, GolemPermissionsInspectDecision,
    GolemPermissionsInstallChildPersist, GolemPermissionsInstallDecision,
    GolemPermissionsInstallTransfer, GolemPermissionsRevokePersist, GolemPermissionsSelfVersion,
    GolemPermissionsSelfWallet,
};
use golem_common::model::oplog::payload::types::{
    PermissionCardDeriveError, PermissionCardRevokeError, SerializablePermissionCardLiveness,
    SerializablePermissionError,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostRequestNoInput,
    HostRequestPermissionCardDecision, HostRequestPermissionCardDerive,
    HostRequestPermissionCardLiveness, HostRequestPermissionCardRevoke,
    HostRequestPermissionCardTransfer, HostResponsePermissionCardDecision,
    HostResponsePermissionCardDerived, HostResponsePermissionCardLiveness,
    HostResponsePermissionCardTransferComplete, HostResponsePermissionCardsRevoked,
    HostResponsePermissionWallet, HostResponsePermissionWalletVersion, OplogEntry, OplogIndex,
    QueuedCardEvent,
};
use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId, PendingCardEventRef, Timestamp};
use golem_common::serialization::{deserialize, serialize};
use golem_schema::schema::schema_value::PermissionCardValuePayload;
use golem_schema::schema::wit::wire::{HostPermissionCard, HostPermissionCardWithStore};
use golem_schema::schema::wit::{PermissionCardHandleRep, PermissionCardResolver};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeSet;
use uuid::Uuid;
use wasmtime::component::{Accessor, Resource};

#[derive(Clone, Debug)]
enum PermissionCardEntry {
    Persistent {
        snapshot: PermissionCardValuePayload,
        card: Option<StoredCard>,
    },
    Scope {
        card: ScopeCard,
        /// The invocation that minted the card, or `None` once its scope ended. A retired entry
        /// stays in the table until the guest drops its handle, so the handle never names another
        /// entry, but it can no longer be attached or used as a parent.
        invocation_key: Option<IdempotencyKey>,
    },
}

#[derive(Clone, Debug)]
enum ResolvedPermissionCard {
    Persistent(StoredCard),
    Scope(ScopeCard),
}

/// The holder identity that template cards resolve against for the caller. It depends on the
/// caller's component, which card events do not change.
fn caller_monomorphization_context<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
) -> AgentPermissionMonomorphizationContext {
    super::owner_monomorphization_context(
        ctx.authority_component_metadata(),
        &ctx.owned_agent_id,
        &ctx.state.owner_context,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DerivedGrantSets<P = PolymorphicPermissionPattern> {
    lower_positive: Vec<P>,
    lower_negative: Vec<P>,
    upper_positive: Vec<P>,
    upper_negative: Vec<P>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScopeDerivationParent {
    root_card_ids: Vec<CardId>,
    expires_at: Option<DateTime<Utc>>,
    delegation: DelegationCard,
}

/// A parent of a scope derivation as the derivation's authorization decision saw it.
#[derive(BinaryCodec)]
enum DecidedScopeParent {
    /// A scope card that is valid in the invocation.
    Scope(ScopeCard),
    /// A persistent card, with the caller's wallet copy of it when the wallet holds it. Its
    /// liveness is resolved after the decision, in its own recorded call.
    Persistent(Option<StoredCard>),
    /// A parent that the derivation cannot use.
    Refused(SerializablePermissionError),
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

#[derive(Clone, Copy)]
enum PersistentDerivationSource {
    ExplicitCard,
    ExplicitTemplateCard,
    Wallet,
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

#[derive(Clone, Debug, BinaryCodec)]
struct InstallAgentTarget {
    agent_id: AgentId,
    holder: AgentPermissionMonomorphizationContext,
}

const UNSUPPORTED_INSTALL_TARGET_ERROR: &str = "install-card currently supports only agent targets";

#[derive(Clone, Debug)]
struct CardTransferData {
    transfer_id: Uuid,
    source_card: StoredCard,
    installed_card: StoredCard,
    installed_card_provenance: Option<CardManagedByRuntimeDerived>,
    target_agent_id: AgentId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SourceTransferProgress {
    queued: bool,
    queued_card: Option<StoredCard>,
    derived_card: Option<StoredCard>,
    started: bool,
    confirmed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PendingSourceTransferProgress {
    started: bool,
    confirmed: bool,
}

#[derive(Clone)]
pub(crate) struct PendingSourceCardTransferRetry {
    pub(crate) transfer_id: Uuid,
    pub(crate) source_card_id: CardId,
    pub(crate) installed_card: StoredCard,
    pub(crate) target_agent_id: AgentId,
    pub(crate) started: bool,
    pending: PendingCardEventRef,
    transfer: QueuedCardEventTransfer,
}

#[cfg(test)]
mod tests;
