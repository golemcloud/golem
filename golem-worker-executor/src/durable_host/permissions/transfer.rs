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

use super::UNSUPPORTED_INSTALL_TARGET_ERROR;
use super::{
    AgentCardHolder, AgentId, AgentPermissionMonomorphizationContext, CardHolder, CardId,
    CardManagedBy, CardManagedByRuntimeDerived, CardTransferData, DurableWorkerCtx,
    GolemPermissionsInstallTransfer, HasWorker, HostRequest, HostRequestPermissionCardTransfer,
    InstallAgentTarget, Oplog, OplogEntry, OplogIndex, OplogOps, OwnedAgentId, ParsedAgentId,
    PendingCardEventRef, PendingSourceCardTransferRetry, PendingSourceTransferProgress,
    PermissionCardDeriveError, QueuedCardEvent, QueuedCardEventTransfer, RecipientOwnerContext,
    RecipientPattern, RuntimeCardCreation, SourceTransferProgress, StoredCard, Timestamp, Uuid,
    WorkerCtx, WorkerExecutorError, anyhow, card_matches_agent_recipient, deserialize,
    ensure_install_permission_in_surface, permissions_types, refused_runtime_card_parent,
    serialize, worker_error_to_permission_error,
};
use golem_common::model::oplog::HostPayloadPair;

pub(super) fn agent_recipient_pattern(
    context: &AgentPermissionMonomorphizationContext,
) -> RecipientPattern {
    match &context.owner {
        RecipientOwnerContext::AgentType(agent_type) => RecipientPattern::Agent {
            account: context.account.clone(),
            application: context.application.clone(),
            environment: context.environment.clone(),
            component: context.component.clone(),
            agent_type: agent_type.clone(),
        },
        RecipientOwnerContext::ComponentExternalToolOwner => {
            RecipientPattern::ComponentExternalToolOwner {
                account: context.account.clone(),
                application: context.application.clone(),
                environment: context.environment.clone(),
                component: context.component.clone(),
            }
        }
    }
}

pub(super) fn require_agent_install_target(
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

pub(super) async fn resolve_install_target_context<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    target: AgentId,
) -> Result<InstallAgentTarget, permissions_types::PermissionError> {
    let owned_target = OwnedAgentId::new(ctx.owned_agent_id().environment_id(), &target);
    let target_revision = ctx
        .state
        .worker_service
        .get(&owned_target)
        .await
        .map_err(|error| {
            permissions_types::PermissionError::NotPermitted(format!(
                "install-card target metadata lookup failed: {error}"
            ))
        })?
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
        holder: AgentPermissionMonomorphizationContext {
            account: component.account_email,
            application: component.application_name,
            environment: component.environment_name,
            component: component.component_name,
            agent_name: target.agent_id,
            owner: RecipientOwnerContext::AgentType(agent_type),
        },
    })
}

/// Decides whether the caller may install `source_card` on the `target` agent, and resolves the
/// target's holder identity that a template card is instantiated for.
pub(super) async fn decide_install<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    source_card: &StoredCard,
    target: AgentId,
) -> anyhow::Result<Result<InstallAgentTarget, permissions_types::PermissionError>> {
    let target = match resolve_install_target_context(ctx, target).await {
        Ok(target) => target,
        Err(error) => return Ok(Err(error)),
    };
    let target_recipient = agent_recipient_pattern(&target.holder);
    if let Err(error) = ctx
        .with_agent_authority_at_boundary(|ctx| {
            ensure_install_permission_in_surface(
                &ctx.state.agent_effective_surface,
                &ctx.state.created_by_email,
                source_card.card_id(),
                &target_recipient,
            )
        })
        .await
        .map_err(worker_error_to_permission_error)?
    {
        return Ok(Err(error));
    }
    if !card_matches_agent_recipient(source_card, &target.holder) {
        return Ok(Err(
            permissions_types::PermissionError::InstallRecipientMismatch(
                "the target agent does not match any recipient pattern of the card".to_string(),
            ),
        ));
    }
    Ok(Ok(target))
}

impl CardTransferData {
    pub(super) fn request(&self) -> anyhow::Result<HostRequestPermissionCardTransfer> {
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

    pub(super) fn from_request(
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

    pub(super) fn source_holder<Ctx: WorkerCtx>(&self, ctx: &DurableWorkerCtx<Ctx>) -> CardHolder {
        CardHolder::Agent(AgentCardHolder {
            agent_id: ctx.owned_agent_id.agent_id.clone(),
        })
    }

    pub(super) fn target_holder(&self) -> CardHolder {
        CardHolder::Agent(AgentCardHolder {
            agent_id: self.target_agent_id.clone(),
        })
    }
}

pub(super) fn installed_card_matches_request(
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

pub(super) fn transfer_progress_mismatch(message: impl std::fmt::Display) -> WorkerExecutorError {
    WorkerExecutorError::unexpected_oplog_entry(
        "durable permission-card transfer state matching its persisted request",
        message.to_string(),
    )
}

pub(super) async fn source_transfer_progress<Ctx: WorkerCtx>(
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
        .read_exact(
            start_index,
            current_index.as_u64() - start_index.as_u64() + 1,
        )
        .await?;
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
                    card.as_ref(),
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
                    .is_some_and(|existing| existing != card.as_ref())
                {
                    return Err(transfer_progress_mismatch(format!(
                        "transfer {} has multiple distinct derived card payloads",
                        transfer.transfer_id
                    )));
                }
                progress.derived_card = Some(card.as_ref().clone());
            }
            OplogEntry::CardEventQueued { event, .. } => {
                let QueuedCardEvent::TransferStarted(event) = event.as_ref() else {
                    continue;
                };
                if event.transfer_id != transfer.transfer_id {
                    continue;
                }
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
                    || recorded_source_holder != &source_holder
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

pub(super) async fn load_card_transfer_request<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    start_index: OplogIndex,
) -> Result<HostRequestPermissionCardTransfer, WorkerExecutorError> {
    let entry = ctx.state.oplog.read(start_index).await?;
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

pub(super) async fn complete_source_card_transfer<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entity_parent_start_index: Option<OplogIndex>,
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

    ensure_source_card_transfer_started(
        ctx,
        entity_parent_start_index,
        transfer_id,
        source_card_id,
        &target_holder,
        started,
        remove_source_membership,
    )
    .await?;

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
            entity_parent_start_index,
            transfer_id,
            source_card_id,
            installed_card.card_id(),
            target_holder,
        ))
        .await?;

    Ok(())
}

pub(super) async fn ensure_source_card_transfer_started<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entity_parent_start_index: Option<OplogIndex>,
    transfer_id: Uuid,
    source_card_id: CardId,
    target_holder: &CardHolder,
    started: bool,
    remove_source_membership: bool,
) -> Result<(), WorkerExecutorError> {
    if started {
        return Ok(());
    }

    if ctx.state.is_live() {
        return ctx
            .commit_wallet_entry(OplogEntry::card_transfer_started(
                entity_parent_start_index,
                transfer_id,
                source_card_id,
                CardHolder::Agent(AgentCardHolder {
                    agent_id: ctx.owned_agent_id.agent_id.clone(),
                }),
                target_holder.clone(),
                0,
            ))
            .await;
    }

    if remove_source_membership
        && crate::durable_host::remove_wallet_card(
            &mut ctx.state.agent_wallet_cards,
            &mut ctx.state.wallet_generation,
            source_card_id,
        )?
    {
        ctx.rederive_agent_effective_surface_from_wallet();
    }

    ctx.public_state
        .worker()
        .add_and_commit_oplog(OplogEntry::card_transfer_started(
            entity_parent_start_index,
            transfer_id,
            source_card_id,
            CardHolder::Agent(AgentCardHolder {
                agent_id: ctx.owned_agent_id.agent_id.clone(),
            }),
            target_holder.clone(),
            ctx.state.wallet_generation,
        ))
        .await?;

    Ok(())
}

/// Executes the source side of a card transfer. The card service refuses the installed child of
/// a template card whose parent is revoked or expired on every attempt, so that refusal is
/// returned instead of trapping.
pub(super) async fn execute_source_card_transfer<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    start_index: OplogIndex,
    transfer: &CardTransferData,
) -> anyhow::Result<Result<(), PermissionCardDeriveError>> {
    ctx.process_pending_replay_events().await?;
    let progress = source_transfer_progress(ctx, start_index, transfer).await?;
    if progress.confirmed {
        return Ok(Ok(()));
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
            let persisted = match ctx
                .state
                .card_service
                .create_runtime_card(transfer.installed_card.clone(), provenance)
                .await
            {
                Ok(RuntimeCardCreation::Created(persisted)) => *persisted,
                Ok(RuntimeCardCreation::Revoked) => {
                    return Ok(Err(PermissionCardDeriveError::CardRevoked(format!(
                        "installed permission card {} was revoked before the transfer completed",
                        transfer.installed_card.card_id()
                    ))));
                }
                Err(error) => {
                    return match refused_runtime_card_parent(
                        ctx,
                        &transfer.installed_card,
                        transfer.source_card.expires_at(),
                    )
                    .await
                    {
                        Ok(Some(refusal)) => Ok(Err(refusal)),
                        Ok(None) => Err(error.into()),
                        Err(lookup_error) => Err(anyhow!(
                            "{error}; checking the parents of permission card {} also failed: {lookup_error}",
                            transfer.installed_card.card_id()
                        )),
                    };
                }
            };
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
            ctx.commit_wallet_entry(OplogEntry::CardDerived {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: ctx.entity_parent_start_index(),
                card: Box::new(installed_card.clone()),
                wallet_generation: 0,
            })
            .await?;
        }

        ctx.public_state
            .worker()
            .add_and_commit_oplog(OplogEntry::card_event_queued(
                ctx.entity_parent_start_index(),
                Box::new(QueuedCardEvent::transfer_started_with_source(
                    transfer.transfer_id,
                    transfer.source_card.card_id(),
                    installed_card.clone(),
                    transfer.target_holder(),
                )),
            ))
            .await?;
    }

    complete_source_card_transfer(
        ctx,
        ctx.entity_parent_start_index(),
        transfer.transfer_id,
        transfer.source_card.card_id(),
        &installed_card,
        &transfer.target_agent_id,
        progress.started,
        matches!(&transfer.source_card, StoredCard::Concrete(_)),
    )
    .await?;

    Ok(Ok(()))
}

pub(super) async fn pending_source_transfer_progress(
    oplog: &dyn Oplog,
    source_agent_id: &AgentId,
    pending: &PendingCardEventRef,
    transfer: &QueuedCardEventTransfer,
    installed_card: &StoredCard,
    target_agent_id: &AgentId,
) -> Result<PendingSourceTransferProgress, WorkerExecutorError> {
    let current_index = oplog.current_oplog_index().await;
    if current_index < pending.oplog_index {
        return Err(transfer_progress_mismatch(format!(
            "pending transfer index {} is beyond current oplog index {current_index}",
            pending.oplog_index
        )));
    }

    let source_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: source_agent_id.clone(),
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
        let entries = oplog.read_exact(next_index, count).await?;

        for entry in entries.values() {
            match entry {
                OplogEntry::CardEventQueued { event, .. } => {
                    let QueuedCardEvent::TransferStarted(recorded) = event.as_ref() else {
                        continue;
                    };
                    if recorded.transfer_id != transfer.transfer_id {
                        continue;
                    }
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
                        || recorded_source_holder != &source_holder
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

pub(super) fn pending_transfer_target_agent_id(
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

impl PendingSourceCardTransferRetry {
    pub(crate) fn entity_parent_start_index(&self) -> Option<OplogIndex> {
        self.pending.entity_parent_start_index
    }

    pub(crate) async fn is_confirmed(
        &self,
        oplog: &dyn Oplog,
        source_agent_id: &AgentId,
    ) -> Result<bool, WorkerExecutorError> {
        Ok(pending_source_transfer_progress(
            oplog,
            source_agent_id,
            &self.pending,
            &self.transfer,
            &self.installed_card,
            &self.target_agent_id,
        )
        .await?
        .confirmed)
    }
}

pub(crate) async fn prepare_pending_source_card_transfer_retry(
    oplog: &dyn Oplog,
    source_agent_id: &AgentId,
    pending: &PendingCardEventRef,
    transfer: &QueuedCardEventTransfer,
) -> Result<Option<PendingSourceCardTransferRetry>, WorkerExecutorError> {
    let installed_card = transfer.card.as_ref().ok_or_else(|| {
        transfer_progress_mismatch(format!(
            "pending transfer {} is missing its installed card payload",
            transfer.transfer_id
        ))
    })?;
    let target_agent_id =
        pending_transfer_target_agent_id(transfer.transfer_id, &transfer.target_holder)?;
    let progress = pending_source_transfer_progress(
        oplog,
        source_agent_id,
        pending,
        transfer,
        installed_card,
        target_agent_id,
    )
    .await?;
    if progress.confirmed {
        return Ok(None);
    }

    if transfer.card_id == installed_card.card_id() {
        if !matches!(installed_card, StoredCard::Concrete(_)) {
            return Err(transfer_progress_mismatch(format!(
                "concrete source transfer {} carries a polymorphic installed card",
                transfer.transfer_id
            )));
        }
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
    }

    Ok(Some(PendingSourceCardTransferRetry {
        transfer_id: transfer.transfer_id,
        source_card_id: transfer.card_id,
        installed_card: installed_card.clone(),
        target_agent_id: target_agent_id.clone(),
        started: progress.started,
        pending: pending.clone(),
        transfer: transfer.clone(),
    }))
}
