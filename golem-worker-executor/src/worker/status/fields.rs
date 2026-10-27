use super::*;
use golem_common::base_model::oplog::QueuedCardEventTransfer;
use golem_common::model::card::{
    SourceCardTransferConfirmed, SourceCardTransferIndex, SourceCardTransferRecords,
    SourceCardTransferStarted, SourceCardTransferState, StoredCard,
};

pub(super) fn calculate_revoked_cards(
    mut revoked_cards: HashSet<golem_common::model::card::CardId>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashSet<golem_common::model::card::CardId> {
    for entry in entries.values() {
        match entry {
            OplogEntry::CardRevoked { card_id, .. } => {
                revoked_cards.insert(*card_id);
            }
            OplogEntry::CardRevokedCascade {
                revoked_card_ids, ..
            } => revoked_cards.extend(revoked_card_ids.iter().copied()),
            OplogEntry::CardInstalled { card, .. } => {
                revoked_cards.remove(&card.card_id());
            }
            _ => {}
        }
    }

    revoked_cards
}

/// The pending invocations after `entries`. The update entries act through their `steps`, which
/// must hold the step of every manual update invocation of `entries`: a manual update invocation
/// leaves the list when a `PendingUpdate` pairs it, when a `FailedUpdate` ends it, or when an
/// entry in a deleted region consumes it, so a revert does not make it run again.
///
/// Skipped regions do not matter here: they represent jumps and updates, and anything that happens
/// in them is part of the history (for example a new pending invocation arrives during an earlier
/// iteration of a retried transaction). In a deleted region, an incoming invocation that has not
/// been processed yet stays pending, and an invocation that was attempted there leaves the list.
///
/// The list keeps a lightweight reference to the originating oplog entry; the full invocation
/// payload stays in the oplog and is hydrated on demand by the paths that execute the invocation.
pub(super) fn calculate_pending_invocations(
    initial: Vec<PendingInvocationRef>,
    steps: &BTreeMap<OplogIndex, UpdateStep>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Vec<PendingInvocationRef> {
    entries
        .iter()
        .fold(initial, |mut result, (oplog_idx, entry)| {
            let step = steps.get(oplog_idx);
            if let Some(index) = step.and_then(UpdateStep::consumed_admission) {
                result.retain(|invocation| invocation.oplog_index != index);
            }
            match entry {
                OplogEntry::PendingAgentInvocation {
                    timestamp,
                    idempotency_key,
                    ..
                } => {
                    // A manual update is the only invocation variant without a semantic
                    // idempotency key, so we capture its target revision and drop the (freshly
                    // generated, unused) idempotency key for it. The region fold decoded the
                    // invocation and gave a manual update its step.
                    let manual_update_target_revision =
                        step.and_then(UpdateStep::manual_admission_target);
                    let idempotency_key = if manual_update_target_revision.is_some() {
                        None
                    } else {
                        Some(idempotency_key.clone())
                    };
                    result.push(PendingInvocationRef {
                        timestamp: *timestamp,
                        oplog_index: *oplog_idx,
                        idempotency_key,
                        manual_update_target_revision,
                    });
                }
                OplogEntry::AgentInvocationStarted {
                    idempotency_key, ..
                }
                | OplogEntry::CancelPendingInvocation {
                    idempotency_key, ..
                } => {
                    result.retain(|invocation| !invocation.has_idempotency_key(idempotency_key));
                }
                _ => {}
            }
            result
        })
}

pub(crate) fn calculate_pending_card_events(
    initial: Vec<PendingCardEventRef>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Vec<PendingCardEventRef> {
    let mut result = initial;

    for (oplog_idx, entry) in entries {
        match entry {
            OplogEntry::CardEventQueued {
                timestamp,
                entity_parent_start_index,
                event,
            } => {
                result.push(PendingCardEventRef {
                    timestamp: *timestamp,
                    oplog_index: *oplog_idx,
                    entity_parent_start_index: *entity_parent_start_index,
                    event: event.as_ref().clone(),
                });
            }
            OplogEntry::CardInstalled {
                queued_event_index: Some(queued_event_index),
                ..
            } => {
                result.retain(|event| {
                    event.oplog_index != *queued_event_index
                        || !matches!(event.event, QueuedCardEvent::Install(_))
                });
            }
            OplogEntry::CardInstallFailed {
                queued_event_index, ..
            } => {
                result.retain(|event| {
                    event.oplog_index != *queued_event_index
                        || !matches!(
                            event.event,
                            QueuedCardEvent::Install(_) | QueuedCardEvent::TransferReceived(_)
                        )
                });
            }
            OplogEntry::CardRevoked {
                queued_event_index, ..
            } => {
                result.retain(|event| {
                    event.oplog_index != *queued_event_index
                        || !matches!(event.event, QueuedCardEvent::Revoke(_))
                });
            }
            OplogEntry::CardRevokedCascade {
                revoked_card_ids, ..
            } => {
                result.retain(|event| {
                    !matches!(
                        &event.event,
                        QueuedCardEvent::Revoke(revoke)
                            if revoked_card_ids.contains(&revoke.card_id)
                    )
                });
            }
            OplogEntry::CardTransferConfirmed {
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                ..
            } => {
                result.retain(|event| {
                    !matches!(
                        &event.event,
                        QueuedCardEvent::TransferStarted(transfer)
                            if transfer.transfer_id == *transfer_id
                                && transfer.card_id == *source_card_id
                                && transfer.card.as_ref().is_some_and(|card| card.card_id() == *installed_card_id)
                                && transfer.target_holder == *target_holder
                    )
                });
            }
            OplogEntry::CardTransferred {
                transfer_id,
                source_card_id,
                installed_card_id,
                card,
                ..
            } => {
                result.retain(|event| {
                    !matches!(
                        &event.event,
                        QueuedCardEvent::TransferReceived(receipt)
                            if receipt.transfer_id == *transfer_id
                                && receipt.source_card_id == *source_card_id
                                && receipt.card_id == *installed_card_id
                                && receipt.card.as_ref() == Some(card)
                    )
                });
            }
            _ => {}
        }
    }

    result
}

pub(super) fn calculate_received_card_transfers(
    mut transfers: ReceivedCardTransferIndex,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> ReceivedCardTransferIndex {
    for entry in entries.values() {
        let OplogEntry::CardEventQueued { event, .. } = entry else {
            continue;
        };
        let QueuedCardEvent::TransferReceived(receipt) = event.as_ref() else {
            continue;
        };

        let Some(card) = &receipt.card else {
            transfers.insert(receipt.transfer_id, ReceivedCardTransferState::Conflict);
            continue;
        };

        match transfers.get(&receipt.transfer_id) {
            None => {
                transfers.insert(
                    receipt.transfer_id,
                    ReceivedCardTransferState::Received {
                        source_card_id: receipt.source_card_id,
                        card: card.clone(),
                    },
                );
            }
            Some(ReceivedCardTransferState::Conflict) => {}
            Some(ReceivedCardTransferState::Received {
                source_card_id,
                card: recorded_card,
            }) => {
                if recorded_card != card || *source_card_id != receipt.source_card_id {
                    transfers.insert(receipt.transfer_id, ReceivedCardTransferState::Conflict);
                }
            }
        }
    }

    transfers
}

/// Folds the source-side records of each transfer that the agent started. The first record of
/// each kind is kept; a later record of the same kind that differs from it puts the transfer in
/// conflict.
pub(super) fn calculate_source_card_transfers(
    mut transfers: SourceCardTransferIndex,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> SourceCardTransferIndex {
    enum SourceRecord<'a> {
        Derived(&'a StoredCard),
        Queued(&'a QueuedCardEventTransfer),
        Started(SourceCardTransferStarted),
        Confirmed(SourceCardTransferConfirmed),
    }

    fn keep_first<T: PartialEq>(slot: &mut Option<T>, value: T) -> bool {
        match slot {
            None => {
                *slot = Some(value);
                true
            }
            Some(existing) => *existing == value,
        }
    }

    for entry in entries.values() {
        let (transfer_id, record) = match entry {
            OplogEntry::CardDerived {
                transfer_id: Some(transfer_id),
                card,
                ..
            } => (*transfer_id, SourceRecord::Derived(card.as_ref())),
            OplogEntry::CardEventQueued { event, .. } => {
                let QueuedCardEvent::TransferStarted(transfer) = event.as_ref() else {
                    continue;
                };
                (transfer.transfer_id, SourceRecord::Queued(transfer))
            }
            OplogEntry::CardTransferStarted {
                transfer_id,
                card_id,
                source_holder,
                target_holder,
                ..
            } => (
                *transfer_id,
                SourceRecord::Started(SourceCardTransferStarted {
                    card_id: *card_id,
                    source_holder: source_holder.clone(),
                    target_holder: target_holder.clone(),
                }),
            ),
            OplogEntry::CardTransferConfirmed {
                transfer_id,
                source_card_id,
                installed_card_id,
                target_holder,
                ..
            } => (
                *transfer_id,
                SourceRecord::Confirmed(SourceCardTransferConfirmed {
                    source_card_id: *source_card_id,
                    installed_card_id: *installed_card_id,
                    target_holder: target_holder.clone(),
                }),
            ),
            _ => continue,
        };

        let mut records = match transfers.get(&transfer_id) {
            None => SourceCardTransferRecords::default(),
            Some(SourceCardTransferState::Recorded(records)) => records.clone(),
            Some(SourceCardTransferState::Conflict) => continue,
        };
        let consistent = match record {
            SourceRecord::Derived(card) => keep_first(&mut records.derived_card, card.clone()),
            SourceRecord::Queued(transfer) => keep_first(&mut records.queued, transfer.clone()),
            SourceRecord::Started(started) => keep_first(&mut records.started, started),
            SourceRecord::Confirmed(confirmed) => keep_first(&mut records.confirmed, confirmed),
        };
        transfers.insert(
            transfer_id,
            if consistent {
                SourceCardTransferState::Recorded(records)
            } else {
                SourceCardTransferState::Conflict
            },
        );
    }

    transfers
}

// Jumps retain external stream effects; explicit reverts discard the removed history.
pub(super) fn calculate_durable_stream_sessions(
    mut sessions: DurableStreamSessionIndex,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<DurableStreamSessionIndex, String> {
    for (oplog_idx, entry) in entries {
        if !deleted_regions.is_in_deleted_region(*oplog_idx) {
            sessions.apply_oplog_entry(*oplog_idx, entry)?;
        }
    }
    Ok(sessions)
}

// Jumps retain admitted external effects. Reverts force a fold from an earlier baseline,
// where records in their deleted regions are excluded before this index is reconstructed.
pub(super) fn calculate_export_fork_admissions(
    mut admissions: ExportForkAdmissions,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<ExportForkAdmissions, String> {
    for (oplog_idx, entry) in entries {
        if deleted_regions.is_in_deleted_region(*oplog_idx) {
            continue;
        }
        if let OplogEntry::Create { parameters, .. } = entry {
            admissions = ExportForkAdmissions {
                owner_fingerprint: Some(AgentFingerprint(parameters.instance_id)),
                ..Default::default()
            };
            continue;
        }
        let OplogEntry::StreamSession { record, .. } = entry else {
            continue;
        };
        let decoded;
        let record = match record {
            OplogPayload::Inline(record) => record.as_ref(),
            OplogPayload::SerializedInline {
                cached: Some(record),
                ..
            }
            | OplogPayload::External {
                cached: Some(record),
                ..
            } => record.as_ref(),
            OplogPayload::SerializedInline {
                bytes,
                cached: None,
            } => {
                decoded = try_deserialize(bytes)
                    .map_err(|error| {
                        format!("failed to decode inline durable stream session record: {error}")
                    })?
                    .ok_or_else(|| {
                        "failed to decode inline durable stream session record: unsupported serialization version"
                            .to_string()
                    })?;
                &decoded
            }
            OplogPayload::External { cached: None, .. } => {
                return Err("durable stream session record payload has not been loaded".into());
            }
        };
        if let StreamSessionRecord::ExportForkAdmitted(record) = record {
            if admissions.owner_fingerprint != Some(record.candidate.export.source_fingerprint) {
                continue;
            }
            if !admissions.reservations.contains_key(&record.target) {
                let count = admissions
                    .session_counts
                    .entry(record.candidate.export.session.clone())
                    .or_default();
                *count = count.saturating_add(1);
            }
            admissions.reservations.insert(
                record.target.clone(),
                golem_common::model::ExportForkReservation {
                    oplog_index: *oplog_idx,
                    request_hash: record.request_hash.clone(),
                    session: record.candidate.export.session.clone(),
                },
            );
            admissions.updated_millis = record.updated_millis;
            admissions.credit_millis = Some(record.credit_millis);
        }
    }
    Ok(admissions)
}

impl UpdateFields {
    /// The fields after the entry `entry` at `oplog_idx`, which is outside the deleted regions,
    /// with the update `step` that the update queue gave for it. The outcome of an update reads
    /// the update that the queue paired with it, never the details of the entry, so the baseline
    /// that a success promotes is the one that the paired update selected. An outcome whose step
    /// is not an outcome step is an error: the region fold did not run over this entry.
    pub(super) fn after(
        mut self,
        oplog_idx: OplogIndex,
        entry: &OplogEntry,
        step: &UpdateStep,
    ) -> Result<Self, String> {
        let missing_step = || {
            format!("the region fold gave no outcome step for the update outcome at {oplog_idx}")
        };
        match (entry, step) {
            (OplogEntry::Create { parameters, .. }, _) => {
                self.component_revision = parameters.component_revision;
                self.component_revision_for_replay = parameters.component_revision;
                self.component_revision_start_index = oplog_idx;
                self.component_size = parameters.component_size;
            }
            (
                OplogEntry::FailedUpdate {
                    timestamp,
                    target_revision,
                    details,
                    snapshot_assisted_details,
                    snapshot_fault,
                    ..
                },
                step,
            ) => {
                let pending_update = match step {
                    UpdateStep::FailedQueued(applied) => applied.clone(),
                    UpdateStep::FailedAdmission(admission) => Some(PendingUpdateRef {
                        timestamp: admission.timestamp,
                        oplog_index: admission.index,
                        admission_index: admission.index,
                        target_revision: *target_revision,
                        kind: PendingUpdateKind::SnapshotBased {
                            filesystem_snapshot: None,
                        },
                    }),
                    _ => return Err(missing_step()),
                };
                self.failed_updates.push(FailedUpdateRecord {
                    timestamp: *timestamp,
                    target_revision: *target_revision,
                    details: details.clone(),
                    pending_update,
                    snapshot_assisted_details: snapshot_assisted_details.clone(),
                    snapshot_fault: *snapshot_fault,
                });
            }
            (
                OplogEntry::SuccessfulUpdate {
                    timestamp,
                    target_revision,
                    new_component_size,
                    snapshot_assisted_details,
                    ..
                },
                step,
            ) => {
                let applied_update = match step {
                    UpdateStep::Succeeded(applied) => applied.as_ref(),
                    _ => return Err(missing_step()),
                };
                self.successful_updates.push(SuccessfulUpdateRecord {
                    timestamp: *timestamp,
                    target_revision: *target_revision,
                    oplog_index: oplog_idx,
                    filesystem_snapshot: applied_update
                        .and_then(|update| update.kind.filesystem_snapshot().cloned()),
                    pending_update: applied_update.cloned(),
                    snapshot_assisted_details: snapshot_assisted_details.clone(),
                });
                self.component_revision = *target_revision;
                self.component_revision_start_index = oplog_idx;
                self.component_size = *new_component_size;

                self.last_automatic_snapshot = None;
                self.previous_usable_automatic_snapshot = None;

                match applied_update {
                    Some(PendingUpdateRef {
                        kind: PendingUpdateKind::SnapshotBased { .. },
                        oplog_index,
                        ..
                    }) => {
                        self.component_revision_for_replay = *target_revision;
                        self.authoritative_snapshot = Some(AuthoritativeSnapshot {
                            index: *oplog_index,
                            kind: AuthoritativeSnapshotKind::ManualUpdate,
                        });
                    }
                    Some(PendingUpdateRef {
                        kind: PendingUpdateKind::SnapshotAssistedAutomatic(selection),
                        ..
                    }) => {
                        let AssistedSelection { snapshot, .. } = selection.as_ref();
                        self.component_revision_for_replay = snapshot.component_revision;
                        self.authoritative_snapshot = Some(AuthoritativeSnapshot {
                            index: snapshot.index,
                            kind: AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                                filesystem_snapshot: snapshot.filesystem_snapshot.clone(),
                            },
                        });
                    }
                    Some(PendingUpdateRef {
                        kind: PendingUpdateKind::Automatic,
                        ..
                    })
                    | None => {}
                }
            }
            (
                OplogEntry::Snapshot {
                    timestamp,
                    filesystem_snapshot,
                    ..
                },
                _,
            ) => {
                // A usable candidate becomes the fallback, also when the new record reuses its
                // filesystem snapshot name: each record has its own application snapshot, which
                // can fail to load on its own.
                if let Some(usable) = self
                    .last_automatic_snapshot
                    .take()
                    .and_then(AutomaticSnapshot::into_usable)
                {
                    self.previous_usable_automatic_snapshot = Some(usable);
                }
                self.last_automatic_snapshot = Some(AutomaticSnapshot {
                    index: oplog_idx,
                    timestamp: *timestamp,
                    component_revision: self.component_revision,
                    files: SnapshotFiles::named(filesystem_snapshot.clone()),
                });
            }
            (
                OplogEntry::SnapshotConfirmed {
                    filesystem_snapshot,
                    ..
                },
                _,
            ) => {
                self.last_automatic_snapshot =
                    self.last_automatic_snapshot
                        .take()
                        .map(|last| AutomaticSnapshot {
                            files: last.files.confirmed(filesystem_snapshot),
                            ..last
                        });
            }
            _ => {}
        }
        Ok(self)
    }
}

/// Gives `fields` after each entry of `entries` outside the deleted regions, with the update
/// steps of the region fold. An entry without a step has the step [`UpdateStep::Unchanged`].
pub(super) fn calculate_update_fields(
    fields: UpdateFields,
    deleted_regions: &DeletedRegions,
    steps: &BTreeMap<OplogIndex, UpdateStep>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<UpdateFields, String> {
    entries
        .iter()
        .filter(|(oplog_idx, _)| !deleted_regions.is_in_deleted_region(**oplog_idx))
        .try_fold(fields, |fields, (oplog_idx, entry)| {
            fields.after(
                *oplog_idx,
                entry,
                steps.get(oplog_idx).unwrap_or(&UpdateStep::Unchanged),
            )
        })
}

pub(super) fn calculate_invocation_results(
    invocation_results: InvocationResultMembership,
    current_idempotency_key: Option<IdempotencyKey>,
    cancelled_idempotency_key: Option<IdempotencyKey>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (
    InvocationResultMembership,
    Option<IdempotencyKey>,
    Option<IdempotencyKey>,
) {
    let mut invocation_results = invocation_results;
    let revert_count = entries
        .values()
        .filter(|entry| matches!(entry, OplogEntry::Revert { .. }))
        .count() as u64;
    invocation_results.set_revert_generation(
        invocation_results
            .revert_generation()
            .wrapping_add(revert_count),
    );
    let mut current_idempotency_key = current_idempotency_key;
    let mut cancelled_idempotency_key = cancelled_idempotency_key;

    fold_invocation_result_entries(
        &mut current_idempotency_key,
        &mut cancelled_idempotency_key,
        deleted_regions,
        entries,
        |key, index| invocation_results.insert(key.clone(), index),
    );

    (
        invocation_results,
        current_idempotency_key,
        cancelled_idempotency_key,
    )
}

pub(crate) fn fold_invocation_result_entries(
    current_idempotency_key: &mut Option<IdempotencyKey>,
    cancelled_idempotency_key: &mut Option<IdempotencyKey>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    mut observe_result: impl FnMut(&IdempotencyKey, OplogIndex),
) {
    for (oplog_idx, entry) in entries {
        // Skipping entries in deleted regions (by revert)
        if deleted_regions.is_in_deleted_region(*oplog_idx) {
            *cancelled_idempotency_key = None;
            continue;
        }

        match entry {
            OplogEntry::AgentInvocationStarted {
                idempotency_key, ..
            } => {
                *cancelled_idempotency_key = None;
                *current_idempotency_key = Some(idempotency_key.clone());
            }
            OplogEntry::AgentInvocationFinished { .. } => {
                *cancelled_idempotency_key = None;
                if let Some(idempotency_key) = &*current_idempotency_key {
                    observe_result(idempotency_key, *oplog_idx);
                }
                *current_idempotency_key = None;
            }
            OplogEntry::CancelPendingInvocation {
                idempotency_key, ..
            } => {
                *cancelled_idempotency_key = Some(idempotency_key.clone());
            }
            OplogEntry::Error {
                kind: OplogErrorKind::Invocation,
                error: AgentError::PermissionDenied(_),
                ..
            } => {
                if let Some(idempotency_key) = cancelled_idempotency_key.take() {
                    observe_result(&idempotency_key, *oplog_idx);
                } else if let Some(idempotency_key) = &*current_idempotency_key {
                    observe_result(idempotency_key, *oplog_idx);
                }
            }
            OplogEntry::Error {
                kind: OplogErrorKind::Invocation,
                ..
            } => {
                *cancelled_idempotency_key = None;
                if let Some(idempotency_key) = &*current_idempotency_key {
                    observe_result(idempotency_key, *oplog_idx);
                }
            }
            OplogEntry::Exited { .. } => {
                *cancelled_idempotency_key = None;
                if let Some(idempotency_key) = &*current_idempotency_key {
                    observe_result(idempotency_key, *oplog_idx);
                }
            }
            _ => {
                *cancelled_idempotency_key = None;
            }
        }
    }
}

pub(super) fn calculate_total_linear_memory_size(
    total: u64,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> u64 {
    let mut result = total;
    for (idx, entry) in entries {
        // Skipping entries in skipped regions as they are not applied during replay
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::Create { parameters, .. } => {
                result = parameters.initial_total_linear_memory_size;
            }
            OplogEntry::GrowMemory { delta, .. } => {
                result = result.saturating_add(*delta);
            }
            OplogEntry::SuccessfulUpdate {
                new_total_linear_memory_size: Some(new_total),
                ..
            } => {
                result = *new_total;
            }
            _ => {}
        }
    }
    result
}

pub(super) fn collect_resources(
    initial: HashMap<AgentResourceId, AgentResourceDescription>,
    skipped_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashMap<AgentResourceId, AgentResourceDescription> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions as they are not applied during replay
        if skipped_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::CreateResource {
                id,
                timestamp,
                resource_type_id,
                ..
            } => {
                result.insert(
                    *id,
                    AgentResourceDescription {
                        created_at: *timestamp,
                        resource_owner: resource_type_id.owner.clone(),
                        resource_name: resource_type_id.name.clone(),
                    },
                );
            }
            OplogEntry::DropResource { id, .. } => {
                result.remove(id);
            }

            _ => {}
        }
    }
    result
}

pub(super) fn calculate_active_plugins(
    initial: HashSet<EnvironmentPluginGrantId>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> HashSet<EnvironmentPluginGrantId> {
    let mut result = initial;
    for (idx, entry) in entries {
        // Skipping entries in deleted regions as they are not applied during replay
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::Create { parameters, .. } => {
                result = parameters.initial_active_plugins.clone();
            }
            OplogEntry::ActivatePlugin {
                plugin_grant_id, ..
            } => {
                result.insert(*plugin_grant_id);
            }
            OplogEntry::DeactivatePlugin {
                plugin_grant_id, ..
            } => {
                result.remove(plugin_grant_id);
            }
            OplogEntry::SuccessfulUpdate {
                new_active_plugins, ..
            } => {
                result = new_active_plugins.clone();
            }
            _ => {}
        }
    }
    result
}

pub(super) fn calculate_oplog_processor_checkpoints(
    mut result: HashMap<EnvironmentPluginGrantId, OplogProcessorCheckpointState>,
    active_plugins: &HashSet<EnvironmentPluginGrantId>,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    finalize: bool,
) -> HashMap<EnvironmentPluginGrantId, OplogProcessorCheckpointState> {
    for (idx, entry) in entries {
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::OplogProcessorCheckpoint {
                plugin_grant_id,
                target_agent_id,
                confirmed_up_to,
                sending_up_to,
                last_batch_start,
                ..
            } => {
                result.insert(
                    *plugin_grant_id,
                    OplogProcessorCheckpointState {
                        target_agent_id: Some(target_agent_id.clone()),
                        confirmed_up_to: *confirmed_up_to,
                        sending_up_to: *sending_up_to,
                        last_batch_start: *last_batch_start,
                    },
                );
            }
            OplogEntry::ActivatePlugin {
                plugin_grant_id, ..
            } => {
                result
                    .entry(*plugin_grant_id)
                    .or_insert(OplogProcessorCheckpointState {
                        target_agent_id: None,
                        confirmed_up_to: *idx,
                        sending_up_to: *idx,
                        last_batch_start: *idx,
                    });
            }
            OplogEntry::DeactivatePlugin {
                plugin_grant_id, ..
            } => {
                // Remove non-in-flight checkpoint so a later same-fold ActivatePlugin
                // can seed a fresh checkpoint at the new activation index
                let keep_in_flight = result
                    .get(plugin_grant_id)
                    .is_some_and(|state| state.sending_up_to > state.confirmed_up_to);
                if !keep_in_flight {
                    result.remove(plugin_grant_id);
                }
            }
            OplogEntry::SuccessfulUpdate {
                new_active_plugins, ..
            } => {
                result.retain(|grant_id, state| {
                    new_active_plugins.contains(grant_id)
                        || state.sending_up_to > state.confirmed_up_to
                });
                for grant_id in new_active_plugins {
                    result
                        .entry(*grant_id)
                        .or_insert(OplogProcessorCheckpointState {
                            target_agent_id: None,
                            confirmed_up_to: *idx,
                            sending_up_to: *idx,
                            last_batch_start: *idx,
                        });
                }
            }
            _ => {}
        }
    }

    if finalize {
        result.retain(|grant_id, state| {
            active_plugins.contains(grant_id) || state.sending_up_to > state.confirmed_up_to
        });
    }

    result
}
