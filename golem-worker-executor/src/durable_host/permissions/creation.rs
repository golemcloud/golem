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
    AgentPermissionMonomorphizationContext, CallReplayOutcome, CardId, CardManagedByRuntimeDerived,
    CardRevokeResult, CardState, DateTime, DurableCallSession, DurableFunctionType,
    DurableWorkerCtx, GolemPermissionsInstallChildPersist, HostPayloadPair,
    HostRequestPermissionCardDerive, HostResponsePermissionCardDerived, IdempotencyKey,
    NotCancellable, OplogEntry, OplogIndex, PermissionCardDeriveError, PolymorphicCard,
    ResolvedCall, RuntimeCardCreation, StoredCard, Timestamp, Utc, WorkerCtx, WorkerExecutorError,
    agent_recipient_pattern, anyhow, derive_error_to_permission_error, deserialize, durable_now,
    ensure_install_permission_in_surface, instantiate_polymorphic_card_for_agent,
    permission_error_to_derive_error, permissions_types, serialize,
};

/// After the card service refused a runtime card, names the first parent of the card that is no
/// longer live. The card service refuses a child of a revoked or expired parent on every attempt,
/// so that refusal is returned to the guest; any other refusal traps and is retried. A parent the
/// card service omits from its answer has no settled state yet, so the omission is a lookup
/// failure.
///
/// The card service reports an expired card as revoked, so `parent_expires_at`, the expiry of the
/// card's parent, tells the two apart.
pub(super) async fn refused_runtime_card_parent<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    card: &StoredCard,
    parent_expires_at: Option<DateTime<Utc>>,
) -> Result<Option<PermissionCardDeriveError>, WorkerExecutorError> {
    let parent_ids = card.parent_ids().to_vec();
    if parent_ids.is_empty() {
        return Ok(None);
    }
    let states = ctx
        .state
        .card_service
        .check_cards(parent_ids.clone())
        .await?;
    for parent_id in parent_ids {
        match states.get(&parent_id) {
            Some(CardState::Live(_)) => {}
            Some(CardState::Revoked) | Some(CardState::Unknown) => {
                return Ok(Some(
                    if parent_expires_at.is_some_and(|expires_at| expires_at <= Utc::now()) {
                        PermissionCardDeriveError::CardExpired(format!(
                            "parent card {} is expired",
                            parent_id.0
                        ))
                    } else {
                        PermissionCardDeriveError::CardRevoked(format!(
                            "parent card {} is revoked",
                            parent_id.0
                        ))
                    },
                ));
            }
            None => {
                return Err(WorkerExecutorError::runtime(format!(
                    "the card service returned no state for parent card {parent_id}"
                )));
            }
        }
    }
    Ok(None)
}

pub(super) async fn complete_runtime_card_creation<Ctx, Pair>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    mut handle: DurableCallSession<Pair, NotCancellable>,
    card: StoredCard,
    parent_expires_at: Option<DateTime<Utc>>,
    provenance: CardManagedByRuntimeDerived,
) -> anyhow::Result<HostResponsePermissionCardDerived>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<
            Req = HostRequestPermissionCardDerive,
            Resp = HostResponsePermissionCardDerived,
        >,
{
    let card_id = card.card_id();
    let created = match ctx
        .state
        .card_service
        .create_runtime_card(card.clone(), provenance)
        .await
    {
        Ok(RuntimeCardCreation::Created(created)) => *created,
        // A card that an earlier attempt of this call created was revoked before the call
        // completed, for example by a repair that was refused. The card service never creates it
        // again, so the refusal is settled.
        Ok(RuntimeCardCreation::Revoked) => {
            return handle
                .complete(
                    ctx,
                    HostResponsePermissionCardDerived {
                        result: Err(PermissionCardDeriveError::CardRevoked(format!(
                            "permission card {card_id} was revoked before its creation completed"
                        ))),
                    },
                )
                .await
                .map_err(Into::into);
        }
        Err(error) => {
            return match refused_runtime_card_parent(ctx, &card, parent_expires_at).await {
                Ok(Some(refusal)) => handle
                    .complete(
                        ctx,
                        HostResponsePermissionCardDerived {
                            result: Err(refusal),
                        },
                    )
                    .await
                    .map_err(Into::into),
                Ok(None) => Err(handle.trap(error)),
                Err(lookup_error) => Err(handle.trap(anyhow!(
                    "{error}; checking the parents of permission card {card_id} also failed: {lookup_error}"
                ))),
            };
        }
    };
    if created.card_id() != card_id {
        return Err(handle.trap(anyhow!(
            "runtime permission-card creation returned {}, expected {card_id}",
            created.card_id()
        )));
    }
    let card = serialize(&created).map_err(|err| {
        handle.trap(anyhow!(
            "failed to serialize runtime permission card {card_id}: {err}"
        ))
    })?;
    handle
        .complete(ctx, HostResponsePermissionCardDerived { result: Ok(card) })
        .await
        .map_err(Into::into)
}

/// Creates a runtime card in one durable call. An interrupted call is repaired only while
/// `repair_authorization` still permits the creation; otherwise the call completes with that
/// refusal, which the guest has not observed yet.
pub(super) async fn persist_runtime_card<Ctx, Pair, DeriveCardId, BuildCard, RepairAuthorization>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    derive_card_id: DeriveCardId,
    build_card: BuildCard,
    parent_expires_at: Option<DateTime<Utc>>,
    repair_authorization: RepairAuthorization,
) -> anyhow::Result<Result<StoredCard, permissions_types::PermissionError>>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<
            Req = HostRequestPermissionCardDerive,
            Resp = HostResponsePermissionCardDerived,
        >,
    DeriveCardId: FnOnce(&DurableWorkerCtx<Ctx>, &IdempotencyKey, OplogIndex) -> CardId,
    BuildCard: FnOnce(CardId) -> StoredCard,
    RepairAuthorization:
        FnOnce(&mut DurableWorkerCtx<Ctx>) -> Result<(), permissions_types::PermissionError>,
{
    let invocation_key = ctx
        .state
        .get_current_idempotency_key()
        .ok_or_else(|| anyhow!("runtime permission-card creation requires an active invocation"))?;
    // Runtime-card creation is byte-idempotent for its deterministic card ID. It therefore uses
    // the re-executable write class so a committed Start without an End can safely retry; the
    // generic remote-write class would make retryability depend on the guest's idempotence flag.
    let begun =
        DurableCallSession::<Pair, NotCancellable>::begin(ctx, DurableFunctionType::WriteLocal)
            .await?;
    let oplog_index = begun.begin_index();
    let card_id_sequence = ctx.state.current_idempotency_key_oplog_index(oplog_index);
    let card_id = derive_card_id(ctx, &invocation_key, card_id_sequence);
    let provenance = CardManagedByRuntimeDerived {
        environment_id: ctx.owned_agent_id.environment_id,
        agent_id: ctx.owned_agent_id.agent_id.clone(),
        invocation_key,
        oplog_index,
    };
    let card = build_card(card_id);
    let handle = match begun.resolve(ctx).await? {
        ResolvedCall::Live(begun) => {
            let request = HostRequestPermissionCardDerive {
                card: serialize(&card).map_err(|err| {
                    anyhow!("failed to serialize runtime permission card {card_id}: {err}")
                })?,
                provenance: serialize(&provenance).map_err(|err| {
                    anyhow!("failed to serialize provenance for permission card {card_id}: {err}")
                })?,
            };
            begun.start_live(ctx, request).await?
        }
        ResolvedCall::Replay(handle) => handle,
    };

    let start = handle.start_index();
    let owner = ctx.state.owner_wallet.clone();
    let parent = ctx.entity_parent_start_index();
    let was_live = handle.is_live();
    if !was_live {
        let request = handle.recorded_request(ctx).await?;
        let requested: StoredCard = deserialize(&request.card).map_err(|error| {
            anyhow!("failed to deserialize runtime-card request at {start}: {error}")
        })?;
        if requested != card {
            return Err(anyhow!(
                "runtime-card request at {start} differs from the reconstructed card {card_id}"
            ));
        }
    }
    let (response, executed_live) = if was_live {
        (
            complete_runtime_card_creation(
                ctx,
                handle,
                card.clone(),
                parent_expires_at,
                provenance,
            )
            .await?,
            true,
        )
    } else {
        match handle.replay(ctx).await? {
            CallReplayOutcome::Replayed(response) => (response, false),
            CallReplayOutcome::Incomplete(mut handle) => {
                let authorized = match ctx
                    .with_agent_authority_at_boundary(repair_authorization)
                    .await
                {
                    Ok(authorized) => authorized,
                    Err(error) => return Err(handle.trap(error)),
                };
                let response = match authorized {
                    Ok(()) => {
                        complete_runtime_card_creation(
                            ctx,
                            handle,
                            card.clone(),
                            parent_expires_at,
                            provenance,
                        )
                        .await?
                    }
                    Err(refusal) => {
                        // The interrupted attempt may have created the card before the crash. No
                        // holder receives it after this refusal, so it must not stay live.
                        match ctx.state.card_service.revoke_card(card_id).await {
                            Ok(CardRevokeResult::Revoked(_))
                            | Ok(CardRevokeResult::AlreadyRevoked(_)) => {}
                            Ok(CardRevokeResult::NotPermitted(error)) => {
                                return Err(handle.trap(anyhow!(
                                    "the card service refused to revoke the refused runtime permission card {card_id}: {error}"
                                )));
                            }
                            Err(error) => {
                                return Err(handle.trap(anyhow!(
                                    "failed to revoke the refused runtime permission card {card_id}: {error}"
                                )));
                            }
                        }
                        handle
                            .complete(
                                ctx,
                                HostResponsePermissionCardDerived {
                                    result: Err(permission_error_to_derive_error(refusal)),
                                },
                            )
                            .await?
                    }
                };
                (response, true)
            }
        }
    };

    let created = match response.result {
        Ok(created) => created,
        Err(refusal) => return Ok(Err(derive_error_to_permission_error(refusal))),
    };
    let created: StoredCard = deserialize(&created)
        .map_err(|err| anyhow!("failed to deserialize runtime permission card {card_id}: {err}"))?;
    if created.card_id() != card_id {
        return Err(anyhow!(
            "replayed runtime permission-card creation returned {}, expected {card_id}",
            created.card_id()
        ));
    }

    if !ctx.state.snapshotting_mode {
        if !executed_live {
            ctx.state
                .replay_state
                .await_card_derivation(card_id, parent)
                .await?;
            if let Some(hook) = ctx.replay_admission_hook() {
                hook.before_runtime_card_audit_lookup(start).await;
            }
        }
        let found = owner.recorded_derivation(start, parent, &created).await?;
        if !found {
            if !executed_live {
                let replay_exhausted = !ctx.state.replay_state.is_replay();
                if !replay_exhausted || ctx.rejects_live_continuation_at_replay_tail() {
                    return Err(anyhow!(
                        "replayed runtime permission-card creation {card_id} is missing its CardDerived audit event"
                    ));
                }
                if !ctx
                    .continue_live_at_replay_tail(
                        replay_exhausted,
                        format!("repair of runtime-card audit for Start {start}"),
                    )
                    .await?
                {
                    return Err(anyhow!(
                        "replay resumed before runtime-card audit repair for Start {start}"
                    ));
                }
            }
            if !ctx.state.is_live() {
                return Err(anyhow!(
                    "runtime-card audit repair for Start {start} did not acquire live continuation"
                ));
            }
            ctx.commit_wallet_entry(OplogEntry::CardDerived {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: parent,
                card: Box::new(created.clone()),
                wallet_generation: 0,
                transfer_id: None,
            })
            .await?;
        }
        ctx.process_pending_replay_events().await?;
    }

    Ok(Ok(created))
}

#[allow(
    dead_code,
    reason = "kept as a standalone durable primitive; transfers persist children within their outer call"
)]
pub(crate) async fn persist_installed_child_card<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    source: &PolymorphicCard,
    target: &AgentPermissionMonomorphizationContext,
) -> anyhow::Result<Result<StoredCard, permissions_types::PermissionError>> {
    let created_at = durable_now(ctx).await?;
    let source_card_id = source.card_id;
    let target_recipient = agent_recipient_pattern(target);
    persist_runtime_card::<Ctx, GolemPermissionsInstallChildPersist, _, _, _>(
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
        source.expires_at,
        move |ctx| {
            ensure_install_permission_in_surface(
                &ctx.state.agent_effective_surface,
                &ctx.state.created_by_email,
                source_card_id,
                &target_recipient,
            )
        },
    )
    .await
}
