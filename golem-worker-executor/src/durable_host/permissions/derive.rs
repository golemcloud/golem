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
    AccountEmail, AgentPermissionMonomorphizationContext, BTreeSet, Card, CardId,
    CardResourcePattern, CardVerb, DateTime, DecidedScopeParent, DelegationCard, DelegationSurface,
    DerivedGrantSets, DurabilityHost, DurableWorkerCtx, EffectiveSurface,
    GolemPermissionsDeriveDecision, GolemPermissionsDeriveFromWalletDecision,
    GolemPermissionsDerivePersist, GolemPermissionsDeriveScopeDecision, IdempotencyKey,
    PermissionCardEntry, PermissionCardHandleRep, PermissionPattern, PersistentDerivationSource,
    PolymorphicCard, PolymorphicPermissionPattern, Resource, ScopeCard, ScopeDerivationParent,
    SerializablePermissionError, StoredCard, Utc, WalletDerivationParent, WorkerCtx,
    WorkerExecutorError, agent_delegation_surface_from_wallet, anyhow,
    caller_monomorphization_context, derive_expires_at, durable_now, ensure_card_permission,
    ensure_card_permission_in_surface, handle_card_id, monomorphize_card_for_agent,
    normalize_tool_grants, parse_polymorphic_permission_fields, permission_card_envelope,
    permission_card_scope_card, permission_card_snapshot, permission_error_from_algebra,
    permission_error_from_parse, permission_error_from_serializable,
    permission_error_to_serializable, permissions_derive, permissions_types, persist_runtime_card,
    record_card_decision, resolve_permission_card, resolve_permission_card_handle,
    worker_error_to_permission_error,
};

impl DerivedGrantSets {
    pub(super) fn envelope(
        &self,
    ) -> Result<golem_common::model::card::TemplateCardEnvelope, permissions_types::PermissionError>
    {
        permission_card_envelope(
            &self.lower_positive,
            &self.lower_negative,
            &self.upper_positive,
            &self.upper_negative,
        )
        .map_err(permissions_types::PermissionError::InvalidPattern)
    }

    pub(super) fn as_concrete(&self) -> Option<DerivedGrantSets<PermissionPattern>> {
        let convert = |grants: &[PolymorphicPermissionPattern]| {
            grants
                .iter()
                .map(PolymorphicPermissionPattern::as_concrete)
                .collect::<Option<Vec<_>>>()
        };
        Some(DerivedGrantSets {
            lower_positive: convert(&self.lower_positive)?,
            lower_negative: convert(&self.lower_negative)?,
            upper_positive: convert(&self.upper_positive)?,
            upper_negative: convert(&self.upper_negative)?,
        })
    }

    pub(super) fn into_card(
        self,
        card_id: CardId,
        parent_id: CardId,
        created_at: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
    ) -> StoredCard {
        match self.as_concrete() {
            Some(grants) => StoredCard::Concrete(Card {
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
            }),
            None => StoredCard::Polymorphic(PolymorphicCard {
                card_id,
                parent_ids: vec![parent_id],
                lower_positive: self.lower_positive,
                lower_negative: self.lower_negative,
                upper_positive: self.upper_positive,
                upper_negative: self.upper_negative,
                created_at,
                expires_at,
                system_card: false,
            }),
        }
    }
}

impl ScopeDerivationParent {
    pub(super) fn persistent(card: &Card) -> Self {
        Self {
            root_card_ids: vec![card.card_id],
            expires_at: card.expires_at,
            delegation: DelegationCard {
                source_card_id: Some(card.card_id),
                lower_positive: card.lower_positive.clone(),
                lower_negative: card.lower_negative.clone(),
                upper_positive: card.upper_positive.clone(),
                upper_negative: card.upper_negative.clone(),
            },
        }
    }

    pub(super) fn scope(card: &ScopeCard) -> Self {
        Self {
            root_card_ids: card.root_card_ids.clone(),
            expires_at: card.expires_at,
            delegation: DelegationCard {
                source_card_id: Some(card.scope_card_id),
                lower_positive: card.lower_positive.clone(),
                lower_negative: card.lower_negative.clone(),
                upper_positive: card.upper_positive.clone(),
                upper_negative: card.upper_negative.clone(),
            },
        }
    }
}

pub(super) fn derive_grant_sets(
    parent: &Card,
    lower_positive_to_retain: &[permissions_types::PatternGrant],
    lower_negative_to_add: &[permissions_types::PatternGrant],
    upper_positive_to_retain: &[permissions_types::PatternGrant],
    upper_negative_to_add: &[permissions_types::PatternGrant],
) -> Result<DerivedGrantSets, permissions_types::PermissionError> {
    let mut grants = parse_derived_grant_sets(
        lower_positive_to_retain,
        lower_negative_to_add,
        upper_positive_to_retain,
        upper_negative_to_add,
    )?;
    grants
        .lower_negative
        .splice(0..0, parent.lower_negative.iter().cloned().map(Into::into));
    grants
        .upper_negative
        .splice(0..0, parent.upper_negative.iter().cloned().map(Into::into));
    let envelope = grants.envelope()?;

    DelegationSurface::from_cards(std::slice::from_ref(parent))
        .validate_attenuation(
            &envelope.lower_positive,
            &envelope.lower_negative,
            &envelope.upper_positive,
            &envelope.upper_negative,
        )
        .map_err(permission_error_from_algebra)?;

    Ok(grants)
}

pub(super) fn parse_derived_grant_sets(
    lower_positive: &[permissions_types::PatternGrant],
    lower_negative: &[permissions_types::PatternGrant],
    upper_positive: &[permissions_types::PatternGrant],
    upper_negative: &[permissions_types::PatternGrant],
) -> Result<DerivedGrantSets, permissions_types::PermissionError> {
    let parse = |grants: &[permissions_types::PatternGrant]| {
        grants
            .iter()
            .map(|g| {
                parse_polymorphic_permission_fields(
                    &g.class,
                    &g.owner,
                    &g.recipient,
                    &g.verb,
                    &g.resource_id,
                )
                .map_err(permission_error_from_parse)
            })
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(DerivedGrantSets {
        lower_positive: parse(lower_positive)?,
        lower_negative: parse(lower_negative)?,
        upper_positive: parse(upper_positive)?,
        upper_negative: parse(upper_negative)?,
    })
}

pub(super) fn build_scope_card(
    scope_card_id: CardId,
    parents: &[ScopeDerivationParent],
    grants: DerivedGrantSets,
) -> Result<ScopeCard, permissions_types::PermissionError> {
    if parents.is_empty() {
        return Err(permissions_types::PermissionError::NotPermitted(
            "derive-scope requires at least one parent card".to_string(),
        ));
    }

    let grants = grants.as_concrete().ok_or_else(|| permissions_types::PermissionError::InvalidPattern(
        "derive-scope does not support slot-bearing requested grants; scope cards must be concrete".to_string()
    ))?;
    DelegationSurface {
        cards: parents
            .iter()
            .map(|parent| parent.delegation.clone())
            .collect(),
    }
    .validate_attenuation(
        &grants.lower_positive,
        &grants.lower_negative,
        &grants.upper_positive,
        &grants.upper_negative,
    )
    .map_err(permission_error_from_algebra)?;

    let root_card_ids = parents
        .iter()
        .flat_map(|parent| parent.root_card_ids.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // A derived card never outlives its parent, so the earliest parent expiry is the earliest
    // expiry of every root.
    let expires_at = parents.iter().filter_map(|parent| parent.expires_at).min();

    Ok(ScopeCard {
        scope_card_id,
        root_card_ids,
        lower_positive: grants.lower_positive,
        lower_negative: grants.lower_negative,
        upper_positive: grants.upper_positive,
        upper_negative: grants.upper_negative,
        expires_at,
    })
}

pub(super) fn persistent_scope_parent_from_wallet(
    wallet: &[StoredCard],
    resolved: &StoredCard,
    context: &AgentPermissionMonomorphizationContext,
) -> Result<ScopeDerivationParent, permissions_types::PermissionError> {
    let card = wallet_parent_for_agent(wallet, resolved, context)?;
    Ok(ScopeDerivationParent::persistent(&card))
}

/// Resolves a persistent parent that must be in the caller's active wallet. Template cards are
/// resolved against the caller's holder identity.
pub(super) fn wallet_parent_for_agent(
    wallet: &[StoredCard],
    resolved: &StoredCard,
    context: &AgentPermissionMonomorphizationContext,
) -> Result<Card, permissions_types::PermissionError> {
    let Some(wallet_card) = wallet
        .iter()
        .find(|wallet_card| wallet_card.card_id() == resolved.card_id())
    else {
        return Err(permissions_types::PermissionError::NotPermitted(format!(
            "parent card {} is not in the active invocation wallet",
            resolved.card_id()
        )));
    };
    if wallet_card != resolved {
        return Err(permissions_types::PermissionError::NotPermitted(format!(
            "parent card {} does not match the active invocation wallet",
            resolved.card_id()
        )));
    }
    Ok(monomorphize_card_for_agent(wallet_card, context))
}

pub(super) fn decide_scope_derivation_parent<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    wallet: &[StoredCard],
    handle: &Resource<PermissionCardHandleRep>,
    invocation_key: &IdempotencyKey,
) -> DecidedScopeParent {
    match permission_card_scope_card(ctx, handle) {
        Err(error) => DecidedScopeParent::Refused(permission_error_to_serializable(
            worker_error_to_permission_error(error),
        )),
        Ok(Some((card, owner_invocation_key))) => {
            if owner_invocation_key.as_ref() == Some(invocation_key) {
                DecidedScopeParent::Scope(card)
            } else {
                DecidedScopeParent::Refused(SerializablePermissionError::NotPermitted(
                    "scope-card parent is not valid in this invocation".to_string(),
                ))
            }
        }
        Ok(None) => {
            let card_id = permission_card_snapshot(ctx, handle)
                .ok()
                .map(|snapshot| CardId(snapshot.card_id));
            DecidedScopeParent::Persistent(card_id.and_then(|card_id| {
                wallet
                    .iter()
                    .find(|card| card.card_id() == card_id)
                    .cloned()
            }))
        }
    }
}

/// Resolves the parents of a scope derivation in argument order from its recorded decision. A
/// persistent parent resolves its liveness in its own recorded call and must match the wallet
/// copy that the decision saw.
pub(super) async fn resolve_scope_derivation_parents<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    context: &AgentPermissionMonomorphizationContext,
    handles: &[Resource<PermissionCardHandleRep>],
    decided: Vec<DecidedScopeParent>,
) -> anyhow::Result<Result<Vec<ScopeDerivationParent>, permissions_types::PermissionError>> {
    if decided.len() != handles.len() {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            format!("a derive-scope decision for {} parents", handles.len()),
            format!("a derive-scope decision for {} parents", decided.len()),
        )
        .into());
    }
    let mut parents = Vec::with_capacity(handles.len());
    for (handle, decided) in handles.iter().zip(decided) {
        match decided {
            DecidedScopeParent::Scope(card) => parents.push(ScopeDerivationParent::scope(&card)),
            DecidedScopeParent::Refused(error) => {
                return Ok(Err(permission_error_from_serializable(error)));
            }
            DecidedScopeParent::Persistent(wallet_card) => {
                let resolved = match resolve_permission_card(ctx, handle).await? {
                    Ok(resolved) => resolved,
                    Err(error) => return Ok(Err(error)),
                };
                match persistent_scope_parent_from_wallet(
                    wallet_card.as_slice(),
                    &resolved,
                    context,
                ) {
                    Ok(parent) => parents.push(parent),
                    Err(error) => return Ok(Err(error)),
                }
            }
        }
    }

    Ok(Ok(parents))
}

pub(super) fn select_wallet_derivation_parent(
    wallet: &[StoredCard],
    context: &golem_common::model::card::AgentPermissionMonomorphizationContext,
    grants: &DerivedGrantSets,
) -> Result<Card, permissions_types::PermissionError> {
    let surface = agent_delegation_surface_from_wallet(context, wallet);
    let envelope = grants.envelope()?;
    let parent_id = match surface
        .select_wallet_derivation_parent(
            &envelope.lower_positive,
            &envelope.lower_negative,
            &envelope.upper_positive,
            &envelope.upper_negative,
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

pub(super) fn ensure_persistent_derivation_repair_authority(
    surface: &EffectiveSurface,
    created_by_email: &AccountEmail,
    parent_id: CardId,
    source: PersistentDerivationSource,
    parent_is_current_wallet_card: bool,
) -> Result<(), permissions_types::PermissionError> {
    ensure_card_permission_in_surface(
        surface,
        created_by_email,
        CardVerb::Derive,
        CardResourcePattern::Any,
    )?;
    match source {
        PersistentDerivationSource::Wallet if !parent_is_current_wallet_card => {
            Err(permissions_types::PermissionError::NotPermitted(format!(
                "derive-from-wallet parent {parent_id} is no longer in the active wallet"
            )))
        }
        PersistentDerivationSource::ExplicitTemplateCard if !parent_is_current_wallet_card => {
            Err(permissions_types::PermissionError::NotPermitted(format!(
                "template parent {parent_id} is no longer in the active wallet"
            )))
        }
        _ => Ok(()),
    }?;
    Ok(())
}

pub(super) async fn persist_derived_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    parent_id: CardId,
    parent_expires_at: Option<DateTime<Utc>>,
    grants: DerivedGrantSets,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    source: PersistentDerivationSource,
) -> anyhow::Result<Result<permissions_types::PersistentCard, permissions_types::PermissionError>> {
    let created = match persist_runtime_card::<Ctx, GolemPermissionsDerivePersist, _, _, _>(
        ctx,
        crate::durable_host::authorization::context::derived_card_id,
        move |card_id| grants.into_card(card_id, parent_id, created_at, expires_at),
        parent_expires_at,
        move |ctx| {
            ensure_persistent_derivation_repair_authority(
                &ctx.state.agent_effective_surface,
                &ctx.state.created_by_email,
                parent_id,
                source,
                ctx.agent_wallet_cards_snapshot()
                    .iter()
                    .any(|card| card.card_id() == parent_id),
            )
        },
    )
    .await?
    {
        Ok(created) => created,
        Err(refusal) => return Ok(Err(refusal)),
    };

    let template = matches!(&created, StoredCard::Polymorphic(_));
    let handle = ctx.table().push(PermissionCardHandleRep::new(
        PermissionCardEntry::from_card(created),
    ))?;
    Ok(Ok(if template {
        permissions_types::PersistentCard::Template(handle)
    } else {
        permissions_types::PersistentCard::Concrete(handle)
    }))
}

pub(super) async fn derive_and_persist_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    parent: &Card,
    grants: DerivedGrantSets,
    lifespan: Option<&permissions_types::Duration>,
    source: PersistentDerivationSource,
) -> anyhow::Result<Result<permissions_types::PersistentCard, permissions_types::PermissionError>> {
    let now = durable_now(ctx).await?;
    let expires_at = match derive_expires_at(now, lifespan, parent.expires_at) {
        Ok(expires_at) => expires_at,
        Err(error) => return Ok(Err(error)),
    };
    persist_derived_card(
        ctx,
        parent.card_id,
        parent.expires_at,
        grants,
        now,
        expires_at,
        source,
    )
    .await
}

impl<Ctx: WorkerCtx> permissions_derive::Host for DurableWorkerCtx<Ctx> {
    async fn derive(
        &mut self,
        parent: Resource<PermissionCardHandleRep>,
        mut lower_positive_to_retain: Vec<permissions_types::PatternGrant>,
        mut lower_negative_to_add: Vec<permissions_types::PatternGrant>,
        mut upper_positive_to_retain: Vec<permissions_types::PatternGrant>,
        mut upper_negative_to_add: Vec<permissions_types::PatternGrant>,
        lifespan: Option<permissions_types::Duration>,
    ) -> anyhow::Result<Result<permissions_types::PersistentCard, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(self, "golem::permissions::derive", "derive");

        if let Err(error) = normalize_tool_grants(
            self,
            [
                &mut lower_positive_to_retain,
                &mut lower_negative_to_add,
                &mut upper_positive_to_retain,
                &mut upper_negative_to_add,
            ],
        )
        .await?
        {
            return Ok(Err(error));
        }
        async {
            // A template parent must be in the caller's wallet, so the decision records the
            // wallet's copy of the parent card.
            let parent_card_id = handle_card_id(self, &parent);
            let wallet_parent = match record_card_decision::<
                _,
                GolemPermissionsDeriveDecision,
                Option<StoredCard>,
                _,
            >(
                self,
                parent_card_id.into_iter().collect(),
                async |ctx| {
                    Ok(ctx
                        .with_agent_authority_at_boundary(|ctx| {
                            ensure_card_permission(
                                ctx,
                                CardVerb::Derive,
                                CardResourcePattern::Any,
                            )?;
                            Ok(parent_card_id.and_then(|card_id| {
                                ctx.agent_wallet_cards_snapshot()
                                    .into_iter()
                                    .find(|card| card.card_id().0 == card_id)
                            }))
                        })
                        .await?)
                },
            )
            .await?
            {
                Ok(wallet_parent) => wallet_parent,
                Err(error) => return Ok(Err(error)),
            };
            let wallet = Vec::from_iter(wallet_parent);
            let context = caller_monomorphization_context(self);
            let parent = match resolve_permission_card_handle(self, &parent)
                .await?
                .and_then(|parent| parent.into_persistent("derive"))
            {
                Ok(parent) => parent,
                Err(error) => return Ok(Err(error)),
            };
            let (parent, source) = match parent {
                StoredCard::Concrete(parent) => (parent, PersistentDerivationSource::ExplicitCard),
                template @ StoredCard::Polymorphic(_) => {
                    match wallet_parent_for_agent(&wallet, &template, &context) {
                        Ok(parent) => (parent, PersistentDerivationSource::ExplicitTemplateCard),
                        Err(error) => return Ok(Err(error)),
                    }
                }
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
            derive_and_persist_card(self, &parent, grants, lifespan.as_ref(), source).await
        }
        .await
    }

    async fn derive_from_wallet(
        &mut self,
        mut lower_positive: Vec<permissions_types::PatternGrant>,
        mut lower_negative: Vec<permissions_types::PatternGrant>,
        mut upper_positive: Vec<permissions_types::PatternGrant>,
        mut upper_negative: Vec<permissions_types::PatternGrant>,
        lifespan: Option<permissions_types::Duration>,
    ) -> anyhow::Result<Result<permissions_types::PersistentCard, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(
            self,
            "golem::permissions::derive",
            "derive-from-wallet",
        );

        if let Err(error) = normalize_tool_grants(
            self,
            [
                &mut lower_positive,
                &mut lower_negative,
                &mut upper_positive,
                &mut upper_negative,
            ],
        )
        .await?
        {
            return Ok(Err(error));
        }
        async {
            let grants = match parse_derived_grant_sets(
                &lower_positive,
                &lower_negative,
                &upper_positive,
                &upper_negative,
            ) {
                Ok(grants) => grants,
                Err(error) => return Ok(Err(error)),
            };
            let parent = match record_card_decision::<
                _,
                GolemPermissionsDeriveFromWalletDecision,
                Card,
                _,
            >(self, Vec::new(), async |ctx| {
                Ok(ctx
                    .with_agent_authority_at_boundary(|ctx| {
                        ensure_card_permission(ctx, CardVerb::Derive, CardResourcePattern::Any)?;
                        select_wallet_derivation_parent(
                            &ctx.agent_wallet_cards_snapshot(),
                            &caller_monomorphization_context(ctx),
                            &grants,
                        )
                    })
                    .await?)
            })
            .await?
            {
                Ok(parent) => parent,
                Err(error) => return Ok(Err(error)),
            };
            derive_and_persist_card(
                self,
                &parent,
                grants,
                lifespan.as_ref(),
                PersistentDerivationSource::Wallet,
            )
            .await
        }
        .await
    }

    async fn derive_scope(
        &mut self,
        parents: Vec<Resource<PermissionCardHandleRep>>,
        mut lower_positive: Vec<permissions_types::PatternGrant>,
        mut lower_negative: Vec<permissions_types::PatternGrant>,
        mut upper_positive: Vec<permissions_types::PatternGrant>,
        mut upper_negative: Vec<permissions_types::PatternGrant>,
    ) -> anyhow::Result<Result<Resource<PermissionCardHandleRep>, permissions_types::PermissionError>>
    {
        DurabilityHost::observe_function_call(self, "golem::permissions::derive", "derive-scope");

        if let Err(error) = normalize_tool_grants(
            self,
            [
                &mut lower_positive,
                &mut lower_negative,
                &mut upper_positive,
                &mut upper_negative,
            ],
        )
        .await?
        {
            return Ok(Err(error));
        }
        let card_ids = parents
            .iter()
            .filter_map(|parent| handle_card_id(self, parent))
            .collect();
        let decided = match record_card_decision::<
            _,
            GolemPermissionsDeriveScopeDecision,
            Vec<DecidedScopeParent>,
            _,
        >(self, card_ids, async |ctx| {
            Ok(ctx
                .with_agent_authority_at_boundary(|ctx| {
                    ensure_card_permission(ctx, CardVerb::Derive, CardResourcePattern::Any)?;
                    let invocation_key =
                        ctx.state.get_current_idempotency_key().ok_or_else(|| {
                            permissions_types::PermissionError::NotPermitted(
                                "derive-scope requires an active invocation".to_string(),
                            )
                        })?;
                    let wallet = ctx.agent_wallet_cards_snapshot();
                    Ok(parents
                        .iter()
                        .map(|parent| {
                            decide_scope_derivation_parent(ctx, &wallet, parent, &invocation_key)
                        })
                        .collect())
                })
                .await?)
        })
        .await?
        {
            Ok(decided) => decided,
            Err(error) => return Ok(Err(error)),
        };
        let invocation_key = self
            .state
            .get_current_idempotency_key()
            .ok_or_else(|| anyhow!("derive-scope requires an active invocation"))?;
        let context = caller_monomorphization_context(self);
        let parents =
            match resolve_scope_derivation_parents(self, &context, &parents, decided).await? {
                Ok(parents) => parents,
                Err(error) => return Ok(Err(error)),
            };
        let result = (|| {
            let grants = parse_derived_grant_sets(
                &lower_positive,
                &lower_negative,
                &upper_positive,
                &upper_negative,
            )?;
            let ordinal = self.state.scope_card_mint_ordinal;
            let next_ordinal = ordinal.checked_add(1).ok_or_else(|| {
                permissions_types::PermissionError::NotPermitted(
                    "scope-card mint ordinal exhausted".to_string(),
                )
            })?;
            let scope_card_id = self.derive_scope_card_id(&invocation_key, ordinal);
            let card = build_scope_card(scope_card_id, &parents, grants)?;
            Ok((card, invocation_key, next_ordinal))
        })();
        let (card, invocation_key, next_ordinal) = match result {
            Ok(result) => result,
            Err(error) => return Ok(Err(error)),
        };
        let root_card_ids = card.root_card_ids.clone();
        let handle = self.table().push(PermissionCardHandleRep::new(
            PermissionCardEntry::from_scope(card, invocation_key),
        ))?;
        self.state
            .invocation_scope_handles
            .insert(handle.rep(), root_card_ids);
        self.state.scope_card_mint_ordinal = next_ordinal;
        Ok(Ok(handle))
    }
}
