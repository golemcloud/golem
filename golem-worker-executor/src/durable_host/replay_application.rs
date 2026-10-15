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

use super::replay_state::ReplayEvent;
use super::revision_update::{
    AgentInputs, RevisionFileInputs, RevisionUpdate, RevisionUpdateInputs, prepare_revision_update,
};
use super::{DurableWorkerCtx, WalletSnapshot};
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::model::card::{CardId, ScopeCard, StoredCard};
use golem_common::model::oplog::UpdateDescription;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

/// Only outstanding preparations are linked across Stores. Local application never blocks a
/// different Store's preparation, and dropping an accessor does not drop accepted preparation.
#[derive(Default)]
pub(crate) struct ReplayPreparationOrder {
    pub(crate) tail: Weak<ReplayApplication>,
    pub(crate) finalization: Weak<ReplayApplication>,
}

pub(crate) struct ReplayApplication {
    pub(crate) result:
        Shared<BoxFuture<'static, Result<Arc<PreparedReplayApplication>, WorkerExecutorError>>>,
}

pub(crate) struct PreparedReplayApplication {
    wallet: WalletSnapshot,
    installed_revision: golem_common::model::component::ComponentRevision,
    update: Option<Arc<RevisionUpdate>>,
    event: ReplayEvent,
    finalized_update: bool,
    scope: Option<ScopeCard>,
    live_scope_roots: Option<BTreeMap<CardId, StoredCard>>,
}

/// Transfer the entire published batch while holding the owner's short reservation mutex. Every
/// shared-effect position is reserved before another Store can drain a later batch.
pub(super) fn reserve<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<(), WorkerExecutorError> {
    if ctx.state.snapshotting_mode {
        return Ok(());
    }
    let owner = ctx.state.owner_wallet.clone();
    let mut order = owner.replay_preparations.lock().unwrap();
    let root = ctx.runtime == super::OwnerRuntime::Agent;
    let events = ctx.state.replay_state.take_new_replay_events_for(root);
    for event in events {
        let (event, receipt) = event.into_projection();
        let receipt = receipt.or_else(|| {
            matches!(event, ReplayEvent::ReplayFinished).then(|| owner.latest_replay_receipt())
        });
        let pending_update = if matches!(event, ReplayEvent::ReplayFinished) {
            ctx.state
                .pending_update
                .try_lock()
                .map_err(|_| WorkerExecutorError::runtime("pending replay update is locked"))?
                .take()
        } else {
            None
        };
        let local_predecessor = ctx.state.replay_applications.back().cloned();
        let predecessor = order.tail.upgrade();
        let initial_installed_revision = ctx.state.component_metadata.revision;
        let initial_wallet = WalletSnapshot {
            cards: ctx.state.agent_wallet_cards.clone(),
            generation: ctx.state.wallet_generation,
            wallet_id_hash: ctx.state.wallet_id_hash,
            metadata: Arc::new(ctx.owner_component_metadata().clone()),
        };
        let inputs = RevisionUpdateInputs::of(ctx);
        let file_inputs = RevisionFileInputs::of(ctx);
        let agent = AgentInputs::of(ctx);
        let owner_id = ctx.owned_agent_id.clone();
        let worker = ctx.public_state.worker();
        let baseline_role = ctx.state.baseline_role.clone();
        let linear_memory = ctx.linear_memory_tracker();
        let scope = ctx.current_invocation_scope_card();
        let interest = ctx.state.card_interest_index.clone();
        let cards = ctx.state.card_service.clone();
        // Registration captures the exact generation before this work becomes independently
        // driven. A finalizer must never enter normal synchronization and join its own result.
        let admission = owner.append_tasks.register();
        let boundary = matches!(event, ReplayEvent::ReplayFinished).then(|| owner.lock_boundary());
        let (send, receive) = tokio::sync::oneshot::channel();
        let application = Arc::new(ReplayApplication {
            result: async move {
                receive.await.unwrap_or_else(|_| {
                    Err(WorkerExecutorError::runtime(
                        "accepted replay preparation did not complete",
                    ))
                })
            }
            .boxed()
            .shared(),
        });
        let finalizes_update = pending_update.is_some();
        let captured_owner = owner.clone();
        let task_worker = worker.clone();
        let task_application = application.clone();
        let work = async move {
            let _application = task_application;
            let result = async {
                let _admission = admission.ok_or(WorkerExecutorError::Interrupted {
                    kind: golem_service_base::error::worker_executor::InterruptKind::Jump,
                })?;
                if let Some(predecessor) = predecessor {
                    predecessor.result.clone().await?;
                }
                let (mut wallet, mut installed_revision) = match local_predecessor {
                    Some(previous) => {
                        let previous = previous.result.clone().await?;
                        (previous.wallet.clone(), previous.installed_revision)
                    }
                    None => (initial_wallet, initial_installed_revision),
                };
                captured_owner.validate_current()?;
                let projection = match receipt {
                    Some(receipt) => {
                        let projected = receipt.result.await?;
                        if matches!(
                            event,
                            ReplayEvent::InvocationWalletPinned { .. }
                                | ReplayEvent::ReplayFinished
                        ) {
                            wallet = projected.wallet.clone();
                        }
                        Some(projected)
                    }
                    None => None,
                };
                let target_revision = match (&event, &pending_update) {
                    (ReplayEvent::UpdateReplayed { new_revision }, _) => Some(*new_revision),
                    (ReplayEvent::ReplayFinished, Some(update)) => {
                        if matches!(update.description, UpdateDescription::SnapshotBased { .. }) {
                            return Err(WorkerExecutorError::runtime(
                                "replay finalization requires an automatic update",
                            ));
                        }
                        Some(*update.description.target_revision())
                    }
                    _ => None,
                };
                let update = if let Some(revision) = target_revision
                    && revision > installed_revision
                {
                    let prepared = async {
                        let update = if let Some(projected) = &projection
                            && matches!(event, ReplayEvent::UpdateReplayed { .. })
                        {
                            projected.update.clone().ok_or_else(|| {
                                super::revision_update::UpdateStateError::Metadata(
                                    WorkerExecutorError::runtime(
                                        "physical revision update has no prepared metadata",
                                    ),
                                )
                            })?
                        } else {
                            Arc::new(prepare_revision_update(inputs, revision, || agent).await?)
                        };
                        update.apply_initial_files(file_inputs).await?;
                        update
                            .project_wallet(&mut wallet)
                            .map_err(super::revision_update::UpdateStateError::WalletCards)?;
                        Ok(update)
                    }
                    .await;
                    match prepared {
                        Ok(update) => {
                            installed_revision = revision;
                            Some(update)
                        }
                        Err(error) => {
                            if let Some(pending) = &pending_update {
                                let action = crate::worker::start_outcome::decide(
                                    &baseline_role,
                                    Some(&pending.reference),
                                    crate::worker::start_outcome::RawStartError::UpdateState(
                                        &error,
                                    ),
                                    &owner_id.agent_id,
                                    task_worker.retired_for_lost_shard(),
                                );
                                return Err(
                                    super::perform_at_update_point(&task_worker, action).await
                                );
                            }
                            return Err(error.to_worker_executor_error());
                        }
                    }
                } else {
                    None
                };
                if let Some(projection) = &projection {
                    if !matches!(
                        event,
                        ReplayEvent::InvocationWalletPinned { .. }
                            | ReplayEvent::ReplayFinished
                            | ReplayEvent::UpdateReplayed { .. }
                    ) {
                        // Physical order validates versions. Membership changes only when this
                        // Store receives the event, even if it already observed a later version.
                        wallet.apply_delivered_membership(&owner_id.agent_id, &event)?;
                        wallet.generation = wallet.generation.max(projection.wallet.generation);
                    } else if matches!(event, ReplayEvent::UpdateReplayed { .. }) {
                        wallet.generation = projection.wallet.generation;
                    }
                } else if update.is_none() {
                    wallet.apply_replay_event(&owner_id.agent_id, &event)?;
                }
                let finalized_update = pending_update.is_some();
                let live_scope_roots = if matches!(event, ReplayEvent::ReplayFinished) {
                    let boundary = boundary
                        .expect("ReplayFinished reserves its boundary")
                        .await?;
                    if let Some(pending) = pending_update {
                        let plugins = captured_owner
                            .baseline
                            .owner_context()
                            .agent()
                            .and_then(|agent| {
                                wallet
                                    .metadata
                                    .metadata
                                    .agent_type_provision_config(&agent.agent_type)
                            })
                            .map(|config| {
                                config
                                    .plugins
                                    .iter()
                                    .map(|plugin| plugin.environment_plugin_grant_id)
                                    .collect()
                            })
                            .unwrap_or_default();
                        task_worker
                            .persist_successful_update_with_boundary(
                                &linear_memory,
                                *pending.description.target_revision(),
                                wallet.metadata.component_size,
                                plugins,
                                crate::worker::start_outcome::success_details_of(
                                    &pending.reference,
                                ),
                                captured_owner.clone(),
                                futures::future::ready(Ok(boundary.clone())).boxed(),
                            )
                            .await?;
                    }
                    let mut ids = captured_owner
                        .snapshot()
                        .cards
                        .keys()
                        .copied()
                        .collect::<Vec<_>>();
                    if let Some(scope) = &scope {
                        ids.extend(scope.root_card_ids.iter().copied());
                    }
                    ids.sort_unstable();
                    ids.dedup();
                    if ids.is_empty() || !interest.authority_is_open() {
                        None
                    } else {
                        let states = cards.check_cards(ids).await?;
                        let roots =
                            super::live_scope_root_cards_from_states(scope.as_ref(), &states)?;
                        let revoked = states
                            .into_iter()
                            .filter_map(|(id, state)| {
                                (state == crate::services::card::CardState::Revoked).then_some(id)
                            })
                            .collect::<Vec<_>>();
                        task_worker.queue_card_revocations_locked(&revoked).await?;
                        Some(roots)
                    }
                } else {
                    None
                };
                Ok(Arc::new(PreparedReplayApplication {
                    wallet,
                    installed_revision,
                    update,
                    event,
                    finalized_update,
                    scope,
                    live_scope_roots,
                }))
            }
            .await;
            if result.is_err() {
                captured_owner.retire();
                task_worker.recover_wallet_generation();
            }
            let _ = send.send(result);
        };
        if worker.tasks.spawn_metadata(work).is_err() {
            // The unaccepted future has released its admission and boundary. Reconstruction
            // must recover the transferred suffix; this generation cannot continue past it.
            owner.retire();
            drop(application);
            drop(order);
            worker.recover_wallet_generation();
            return Err(WorkerExecutorError::Interrupted {
                kind: golem_service_base::error::worker_executor::InterruptKind::Jump,
            });
        }
        order.tail = Arc::downgrade(&application);
        if finalizes_update {
            order.finalization = Arc::downgrade(&application);
        }
        ctx.state.replay_applications.push_back(application);
    }
    Ok(())
}

pub(super) fn apply<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    application: &Arc<ReplayApplication>,
    prepared: &PreparedReplayApplication,
) -> bool {
    if !ctx
        .state
        .replay_applications
        .front()
        .is_some_and(|front| Arc::ptr_eq(front, application))
    {
        return false;
    }
    if let Some(update) = &prepared.update {
        update.install_metadata(ctx);
    }
    ctx.state.agent_wallet_cards = prepared.wallet.cards.clone();
    ctx.state.wallet_generation = if ctx.state.entity_replay_wallet.is_some() {
        ctx.state.wallet_generation.max(prepared.wallet.generation)
    } else {
        prepared.wallet.generation
    };
    let revoked: &[CardId] = match &prepared.event {
        ReplayEvent::CardRevokedCascade { card_ids, .. } => card_ids,
        ReplayEvent::CardRevoked { card_id, .. } | ReplayEvent::CardExpired { card_id, .. } => {
            std::slice::from_ref(card_id)
        }
        _ => &[],
    };
    ctx.clear_invocation_scope_if_roots_include(revoked);
    if let ReplayEvent::ForkReplayed { new_phantom_id } = &prepared.event {
        ctx.state.current_phantom_id = Some(*new_phantom_id);
    }
    if prepared.finalized_update {
        ctx.state.automatic_update_unsettled = false;
    }
    if ctx.current_invocation_scope_card() == prepared.scope
        && let Some(roots) = &prepared.live_scope_roots
    {
        ctx.state.invocation_scope_root_cards = roots.clone();
        ctx.state.authority_initialized = false;
    }
    ctx.rederive_agent_effective_surface_from_wallet();
    ctx.state.replay_applications.pop_front();
    true
}
