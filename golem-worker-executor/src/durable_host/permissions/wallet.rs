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
    CallReplayOutcome, CardManagedByRuntimeDerived, CardResourcePattern, CardTransferData,
    CardVerb, CommitLevel, DurabilityHost, DurableCallSession, DurableFunctionType,
    DurableWorkerCtx, GolemPermissionsInstallDecision, GolemPermissionsInstallTransfer,
    GolemPermissionsSelfVersion, GolemPermissionsSelfWallet, HasWorker, HostRequestNoInput,
    HostResponsePermissionCardTransferComplete, HostResponsePermissionWallet,
    HostResponsePermissionWalletVersion, NotCancellable, PermissionCardEntry,
    PermissionCardHandleRep, PermissionCardResolver, ResolvedCall, Resource, ScopeCard, StoredCard,
    WorkerCtx, anyhow, decide_install, derive_error_to_permission_error, deserialize, durable_now,
    ensure_card_permission, execute_source_card_transfer, instantiate_polymorphic_card_for_agent,
    load_card_transfer_request, permissions_types, permissions_wallet, record_card_decision,
    require_agent_install_target, resolve_permission_card_handle, serialize,
};
use crate::durable_host::authorization::context::{installed_child_card_id, transfer_id};
use golem_common::model::oplog::OplogIndex;
use golem_service_base::error::worker_executor::WorkerExecutorError;

/// Reads the wallet that a live `self-wallet` call lists. The result is recorded: replay applies
/// the card events of an invocation at their recorded positions, but an unrecorded read has no
/// position, so replay could apply an event committed between two reads before both of them.
pub(super) async fn observe_self_wallet<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<HostResponsePermissionWallet> {
    let cards = ctx
        .active_agent_wallet_cards_snapshot()
        .await?
        .iter()
        .map(|card| {
            serialize(card).map_err(|err| {
                anyhow!(
                    "failed to serialize observed wallet card {}: {err}",
                    card.card_id()
                )
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    // A scope card lent to this invocation is listed only to a holder that may inspect cards;
    // for any other holder it stays sealed: it still authorizes, but is not enumerated. The
    // card is read after the boundary has applied revocations and expiry, so a scope card
    // invalidated at this boundary is never handed out again.
    let scope_card = ctx
        .with_agent_authority_at_boundary(|ctx| {
            let scope_card = ctx.state.invocation_scope_card.clone()?;
            ensure_card_permission(ctx, CardVerb::Inspect, CardResourcePattern::Any)
                .ok()
                .map(|()| scope_card)
        })
        .await?
        .map(|scope_card| {
            serialize(&scope_card).map_err(|err| {
                anyhow!(
                    "failed to serialize lent scope card {}: {err}",
                    scope_card.scope_card_id
                )
            })
        })
        .transpose()?;

    Ok(HostResponsePermissionWallet { cards, scope_card })
}

impl<Ctx: WorkerCtx> permissions_wallet::Host for DurableWorkerCtx<Ctx> {
    async fn self_wallet(&mut self) -> anyhow::Result<Vec<permissions_types::WalletCard>> {
        let observed = DurableCallSession::<GolemPermissionsSelfWallet, NotCancellable>::invoke(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async |ctx| observe_self_wallet(ctx).await,
        )
        .await?;

        let mut wallet_cards = observed
            .cards
            .iter()
            .map(|bytes| {
                let card: StoredCard = deserialize(bytes).map_err(|err| {
                    anyhow!("failed to deserialize an observed wallet card: {err}")
                })?;
                let template = matches!(card, StoredCard::Polymorphic(_));
                let handle = self.table().push(PermissionCardHandleRep::new(
                    PermissionCardEntry::from_card(card),
                ))?;
                Ok(if template {
                    permissions_types::WalletCard::Template(handle)
                } else {
                    permissions_types::WalletCard::Concrete(handle)
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        if let Some(scope_card) = observed.scope_card {
            let scope_card: ScopeCard = deserialize(&scope_card).map_err(|err| {
                anyhow!("failed to deserialize an observed lent scope card: {err}")
            })?;
            let invocation_key = self.state.get_current_idempotency_key().ok_or_else(|| {
                anyhow::anyhow!("an admitted scope card requires an active invocation")
            })?;
            let root_card_ids = scope_card.root_card_ids.clone();
            let handle = self.table().push(PermissionCardHandleRep::new(
                PermissionCardEntry::from_scope(scope_card, invocation_key),
            ))?;
            self.state
                .invocation_scope_handles
                .insert(handle.rep(), root_card_ids);
            wallet_cards.push(permissions_types::WalletCard::Scope(handle));
        }

        Ok(wallet_cards)
    }

    async fn self_version(&mut self) -> anyhow::Result<permissions_types::WalletVersionToken> {
        let observed = DurableCallSession::<GolemPermissionsSelfVersion, NotCancellable>::invoke(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async |ctx| {
                let _ = ctx.active_agent_wallet_cards_snapshot().await?;
                Ok::<_, anyhow::Error>(HostResponsePermissionWalletVersion {
                    wallet_id_hash: ctx.wallet_id_hash().to_vec(),
                    generation: ctx.wallet_generation(),
                })
            },
        )
        .await?;

        Ok(permissions_types::WalletVersionToken {
            wallet_id_hash: observed.wallet_id_hash,
            generation: observed.generation,
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
            let source_card = match resolve_permission_card_handle(self, &card)
                .await?
                .and_then(|card| card.into_persistent("install-card"))
            {
                Ok(card) => card,
                Err(error) => return Ok(Err(error)),
            };
            let target = match record_card_decision::<_, GolemPermissionsInstallDecision, _, _>(
                self,
                vec![source_card.card_id().0],
                async |ctx| decide_install(ctx, &source_card, target).await,
            )
            .await?
            {
                Ok(target) => target,
                Err(error) => return Ok(Err(error)),
            };

            let invocation_key = self
                .state
                .get_current_idempotency_key()
                .ok_or_else(|| anyhow!("permission-card transfer requires an active invocation"))?;
            let begun =
                DurableCallSession::<GolemPermissionsInstallTransfer, NotCancellable>::begin(
                    self,
                    DurableFunctionType::WriteLocal,
                )
                .await?;
            // The transfer derives from the call's own `Start` index, which live and replay
            // agree on.
            let created_at = match &source_card {
                StoredCard::Concrete(_) => None,
                StoredCard::Polymorphic(_) => Some(durable_now(self).await?),
            };
            let owned_agent_id = self.owned_agent_id.clone();
            let holder = target.holder.clone();
            let target_agent_id = target.agent_id;
            let transfer_at = move |operation_index: OplogIndex| {
                let (installed_card, installed_card_provenance) = match (&source_card, created_at) {
                    (StoredCard::Polymorphic(source), Some(created_at)) => (
                        StoredCard::Concrete(instantiate_polymorphic_card_for_agent(
                            source,
                            &holder,
                            installed_child_card_id(
                                &owned_agent_id,
                                &invocation_key,
                                operation_index,
                            ),
                            created_at,
                        )),
                        Some(CardManagedByRuntimeDerived {
                            environment_id: owned_agent_id.environment_id,
                            agent_id: owned_agent_id.agent_id.clone(),
                            invocation_key: invocation_key.clone(),
                            oplog_index: operation_index,
                        }),
                    ),
                    _ => (source_card.clone(), None),
                };
                CardTransferData {
                    transfer_id: transfer_id(&owned_agent_id, &invocation_key, operation_index),
                    source_card,
                    installed_card,
                    installed_card_provenance,
                    target_agent_id,
                }
            };
            let mut transfer = None;
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let built = std::sync::Arc::new(std::sync::Mutex::new(None));
                    let slot = built.clone();
                    let handle = begun
                        .start_live_at_index(self, move |start_index| {
                            let transfer = transfer_at(start_index);
                            let request = transfer
                                .request()
                                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                            *slot.lock().unwrap() = Some(transfer);
                            Ok(request)
                        })
                        .await?;
                    transfer = built.lock().unwrap().take();
                    handle
                }
                ResolvedCall::Replay(handle) => handle,
            };
            if handle.is_live() {
                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::Always)
                    .await?;
            }
            let start_index = handle.start_index();

            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => {
                        self.process_pending_replay_events().await?;
                        return Ok(response.result.map_err(derive_error_to_permission_error));
                    }
                    CallReplayOutcome::Incomplete(live) => {
                        handle = live;
                        transfer = Some(CardTransferData::from_request(
                            load_card_transfer_request(self, start_index).await?,
                        )?);
                    }
                }
            }

            let transfer = transfer.expect("a live transfer has its request");
            let result = match execute_source_card_transfer(self, start_index, &transfer).await {
                Ok(result) => result,
                Err(error) => {
                    handle.abandon_for_trap();
                    return Err(error);
                }
            };
            let response = handle
                .complete(self, HostResponsePermissionCardTransferComplete { result })
                .await?;
            Ok(response.result.map_err(derive_error_to_permission_error))
        }
        .await;
        self.drop_permission_card_handle(card);
        result
    }
}
