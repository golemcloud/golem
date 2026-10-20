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
    CallReplayOutcome, CardId, CardResourcePattern, CardRevokeResult, CardVerb, DurabilityHost,
    DurableCallSession, DurableFunctionType, DurableWorkerCtx, GolemPermissionsRevokePersist,
    HostRequestPermissionCardRevoke, HostResponsePermissionCardsRevoked, NotCancellable,
    PermissionCardHandleRep, PermissionCardResolver, PermissionCardRevokeError, Resource,
    WorkerCtx, WorkerExecutorError, anyhow, ensure_card_permission, permission_card_scope_card,
    permission_card_snapshot, permissions_revoke, permissions_types,
    resolve_permission_card_within_recorded_call, scope_card_operation_error,
};

pub(super) fn ensure_revoke_authority(
    target_and_ancestor_ids: &[CardId],
    active_wallet_card_ids: &[CardId],
) -> Result<(), permissions_types::PermissionError> {
    if active_wallet_card_ids
        .iter()
        .any(|card_id| target_and_ancestor_ids.contains(card_id))
    {
        Ok(())
    } else {
        Err(permissions_types::PermissionError::NotPermitted(
            "revoke requires possession of the target card or one of its live ancestors"
                .to_string(),
        ))
    }
}

pub(super) fn capture_revoke_authority<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<Vec<CardId>, permissions_types::PermissionError> {
    ensure_card_permission(ctx, CardVerb::Revoke, CardResourcePattern::Any)?;
    Ok(ctx.state.agent_effective_surface.source_card_ids.clone())
}

pub(super) async fn permission_card_revoke_response<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: &Resource<PermissionCardHandleRep>,
    card_id: CardId,
    authority: Result<Vec<CardId>, permissions_types::PermissionError>,
) -> Result<HostResponsePermissionCardsRevoked, WorkerExecutorError> {
    let active_wallet_card_ids = match authority {
        Ok(card_ids) => card_ids,
        Err(error) => {
            return Ok(HostResponsePermissionCardsRevoked {
                result: Err(permission_error_to_revoke_error(error)?),
            });
        }
    };
    let card = match resolve_permission_card_within_recorded_call(ctx, handle).await {
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
    let ancestor_ids = ctx
        .state
        .card_service
        .live_ancestor_ids_including_self(&card)
        .await?;
    if let Err(error) = ensure_revoke_authority(&ancestor_ids, &active_wallet_card_ids) {
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

pub(super) fn permission_error_to_revoke_error(
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

pub(super) async fn complete_permission_card_revoke<Ctx: WorkerCtx>(
    mut handle: DurableCallSession<GolemPermissionsRevokePersist, NotCancellable>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    card_handle: &Resource<PermissionCardHandleRep>,
    card_id: CardId,
    authority: Result<Vec<CardId>, permissions_types::PermissionError>,
) -> anyhow::Result<HostResponsePermissionCardsRevoked> {
    let response = match permission_card_revoke_response(ctx, card_handle, card_id, authority).await
    {
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
    let entity_parent_start_index = ctx.entity_parent_start_index();
    // The cascade removes queued revocations of these cards from the pending card events, which
    // the card event boundary reads from the agent status only. A live revoke commits and folds
    // the cascade before returning, including in a save hook; the finish commit of an ephemeral
    // agent would not fold it.
    let commit_immediately = !ctx.state.snapshotting_mode;
    if let Err(error) = ctx
        .apply_card_revoked_cascade(
            entity_parent_start_index,
            &locally_revoked_card_ids,
            commit_immediately,
        )
        .await
    {
        handle.abandon_for_trap();
        return Err(error.into());
    }
    handle.complete(ctx, response).await.map_err(Into::into)
}

pub(super) async fn revoke_and_persist_card<Ctx: WorkerCtx>(
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
    let (mut handle, captured_authority) = DurableCallSession::<
        GolemPermissionsRevokePersist,
        NotCancellable,
    >::start_with_agent_authority_capture(
        ctx,
        HostRequestPermissionCardRevoke { card_id: card_id.0 },
        DurableFunctionType::WriteRemote,
        capture_revoke_authority,
    )
    .await?;
    let response = if handle.is_live() {
        let Some(authority) = captured_authority else {
            return Err(handle.trap(WorkerExecutorError::runtime(
                "live permission-card revoke has no captured authority",
            )));
        };
        complete_permission_card_revoke(handle, ctx, card_handle, card_id, authority).await?
    } else {
        match handle.replay(ctx).await? {
            CallReplayOutcome::Replayed(response) => response,
            CallReplayOutcome::Incomplete(mut handle) => {
                let authority = match ctx
                    .with_agent_authority_at_boundary(capture_revoke_authority)
                    .await
                {
                    Ok(authority) => authority,
                    Err(error) => return Err(handle.trap(error)),
                };
                complete_permission_card_revoke(handle, ctx, card_handle, card_id, authority)
                    .await?
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
