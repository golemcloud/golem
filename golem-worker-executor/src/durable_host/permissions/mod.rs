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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::permissions::derive as permissions_derive;
use crate::preview2::golem::permissions::inspect as permissions_inspect;
use crate::preview2::golem::permissions::kernel_introspection as permissions_kernel;
use crate::preview2::golem::permissions::revoke as permissions_revoke;
use crate::preview2::golem::permissions::types as permissions_types;
use crate::preview2::golem::permissions::wallet as permissions_wallet;
use crate::services::card::CardState;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use golem_common::model::card::owner::AccountOwnerPattern;
use golem_common::model::card::{
    CardAlgebraError, CardClass, CardId, CardParseError, CardResourcePattern, CardVerb,
    ClassPermissionTarget, PermissionPattern, PermissionTarget, PolymorphicPermissionPattern,
    RenderedPermissionFields, StoredCard, parse_permission_fields, permission_class_metadata,
};
use golem_schema::schema::schema_value::PermissionCardValuePayload;
use golem_schema::schema::wit::wire::HostPermissionCard;
use golem_schema::schema::wit::{PermissionCardHandleRep, PermissionCardResolver};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use uuid::Uuid;
use wasmtime::component::Resource;

#[derive(Clone, Debug)]
struct PermissionCardEntry {
    snapshot: PermissionCardValuePayload,
    card: Option<StoredCard>,
}

impl PermissionCardEntry {
    fn from_card(card: StoredCard) -> Self {
        Self {
            snapshot: snapshot_from_card(&card),
            card: Some(card),
        }
    }

    fn from_snapshot(snapshot: PermissionCardValuePayload) -> Self {
        Self {
            snapshot,
            card: None,
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

fn permission_card_snapshot_from_rep(
    rep: &PermissionCardHandleRep,
) -> Result<PermissionCardValuePayload, WorkerExecutorError> {
    if let Some(entry) = rep.downcast_ref::<PermissionCardEntry>() {
        Ok(entry.snapshot.clone())
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
        .and_then(|entry| entry.card.clone()))
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

async fn resolve_permission_card_or_trap<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
) -> anyhow::Result<StoredCard> {
    resolve_permission_card(ctx, handle)
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
    card: &StoredCard,
) -> Result<permissions_types::CardView, permissions_types::PermissionError> {
    match card {
        StoredCard::Concrete(card) => Ok(permissions_types::CardView {
            lower_positive: concrete_pattern_grants(&card.lower_positive)?,
            lower_negative: concrete_pattern_grants(&card.lower_negative)?,
            upper_positive: concrete_pattern_grants(&card.upper_positive)?,
            upper_negative: concrete_pattern_grants(&card.upper_negative)?,
        }),
        StoredCard::Polymorphic(card) => Ok(permissions_types::CardView {
            lower_positive: polymorphic_pattern_grants(&card.lower_positive)?,
            lower_negative: polymorphic_pattern_grants(&card.lower_negative)?,
            upper_positive: polymorphic_pattern_grants(&card.upper_positive)?,
            upper_negative: polymorphic_pattern_grants(&card.upper_negative)?,
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
        CardAlgebraError::DerivationNotSubsumed { grant } => {
            permissions_types::PermissionError::NotPermitted(
                grant
                    .render()
                    .unwrap_or_else(|_| "grant is not permitted".to_string()),
            )
        }
    }
}

fn ensure_card_permission<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    verb: CardVerb,
    resource: CardResourcePattern,
) -> Result<(), permissions_types::PermissionError> {
    let request = PermissionTarget::Card(ClassPermissionTarget::<CardClass> {
        verb: Some(verb),
        owner: AccountOwnerPattern::Account {
            account: ctx.state.created_by_email.clone(),
        },
        resource,
    });

    if ctx
        .state
        .agent_effective_surface
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

fn mutation_not_ready(operation: &str) -> permissions_types::PermissionError {
    permissions_types::PermissionError::NotPermitted(format!(
        "{operation} requires durable permission-card mutation support"
    ))
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
            Ok(entry) => Ok(entry.snapshot),
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
        let card = resolve_permission_card_or_trap(self, &c).await?;
        Ok(card.card_id().into())
    }

    async fn parents(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<Vec<permissions_types::CardId>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::types", "parents");
        let card = resolve_permission_card_or_trap(self, &c).await?;
        Ok(card.parent_ids().iter().copied().map(Into::into).collect())
    }

    async fn expires_at(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<Option<permissions_types::Timestamp>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::types", "expires-at");
        let card = resolve_permission_card_or_trap(self, &c).await?;
        Ok(card.expires_at().map(Into::into))
    }

    async fn is_polymorphic(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<bool> {
        DurabilityHost::observe_function_call(self, "golem::permissions::types", "is-polymorphic");
        let card = resolve_permission_card_or_trap(self, &c).await?;
        Ok(matches!(card, StoredCard::Polymorphic(_)))
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
            let card = resolve_permission_card(self, &c).await?;
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
        _lifespan: Option<permissions_types::Duration>,
    ) -> anyhow::Result<Result<Resource<PermissionCardHandleRep>, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(self, "golem::permissions::derive", "derive");
        let result = async {
            ensure_card_permission(self, CardVerb::Derive, CardResourcePattern::Any)?;
            let parent = resolve_permission_card(self, &parent).await?;
            if matches!(parent, StoredCard::Polymorphic(_)) {
                return Err(
                    permissions_types::PermissionError::CardPolymorphicNotUsable(
                        "derive requires a concrete parent card".to_string(),
                    ),
                );
            }
            for grant in lower_positive_to_retain
                .iter()
                .chain(lower_negative_to_add.iter())
                .chain(upper_positive_to_retain.iter())
                .chain(upper_negative_to_add.iter())
            {
                parse_pattern_grant(grant)?;
            }
            Err(mutation_not_ready("derive"))
        }
        .await;
        Ok(result)
    }

    async fn derive_from_wallet(
        &mut self,
        lower_positive: Vec<permissions_types::PatternGrant>,
        lower_negative: Vec<permissions_types::PatternGrant>,
        upper_positive: Vec<permissions_types::PatternGrant>,
        upper_negative: Vec<permissions_types::PatternGrant>,
        _lifespan: Option<permissions_types::Duration>,
    ) -> anyhow::Result<Result<Resource<PermissionCardHandleRep>, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(
            self,
            "golem::permissions::derive",
            "derive-from-wallet",
        );
        let result = async {
            ensure_card_permission(self, CardVerb::Derive, CardResourcePattern::Any)?;
            for grant in lower_positive
                .iter()
                .chain(lower_negative.iter())
                .chain(upper_positive.iter())
                .chain(upper_negative.iter())
            {
                parse_pattern_grant(grant)?;
            }
            Err(mutation_not_ready("derive-from-wallet"))
        }
        .await;
        Ok(result)
    }
}

impl<Ctx: WorkerCtx> permissions_revoke::Host for DurableWorkerCtx<Ctx> {
    async fn revoke_card(
        &mut self,
        c: Resource<PermissionCardHandleRep>,
    ) -> anyhow::Result<Result<u32, permissions_types::PermissionError>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::revoke", "revoke-card");
        let result = async {
            ensure_card_permission(self, CardVerb::Revoke, CardResourcePattern::Any)?;
            let _card = resolve_permission_card(self, &c).await?;
            Err(mutation_not_ready("revoke-card"))
        }
        .await;
        self.drop_permission_card_handle(c);
        Ok(result)
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
        let cards = self.active_agent_wallet_cards_snapshot().await?;

        let mut wallet_id_hasher = blake3::Hasher::new();
        wallet_id_hasher.update(b"agent");
        wallet_id_hasher.update(self.owned_agent_id.to_string().as_bytes());
        let wallet_id_hash = wallet_id_hasher.finalize().as_bytes().to_vec();

        let mut generation_hasher = blake3::Hasher::new();
        for card_id in sorted_uuids(cards.iter().map(|card| card.card_id().0)) {
            generation_hasher.update(card_id.as_bytes());
        }
        let generation_bytes = generation_hasher.finalize();
        let generation = u64::from_le_bytes(generation_bytes.as_bytes()[0..8].try_into()?);

        Ok(permissions_types::WalletVersionToken {
            wallet_id_hash,
            generation,
        })
    }

    async fn install_card(
        &mut self,
        card: Resource<PermissionCardHandleRep>,
        _target: permissions_types::Holder,
    ) -> anyhow::Result<Result<(), permissions_types::PermissionError>> {
        DurabilityHost::observe_function_call(self, "golem::permissions::wallet", "install-card");
        let result = async {
            ensure_card_permission(self, CardVerb::Install, CardResourcePattern::Any)?;
            let _card = resolve_permission_card(self, &card).await?;
            Err(mutation_not_ready("install-card"))
        }
        .await;
        self.drop_permission_card_handle(card);
        Ok(result)
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
    use test_r::test;

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
}
