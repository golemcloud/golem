use super::*;
use golem_common::base_model::oplog::QueuedCardEventTransfer;
use golem_common::model::InvocationId;
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
                    not_before,
                    invocation_id: idempotency_key,
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
                        Some(*idempotency_key)
                    };
                    result.push(PendingInvocationRef {
                        timestamp: *timestamp,
                        not_before: *not_before,
                        oplog_index: *oplog_idx,
                        idempotency_key,
                        manual_update_target_revision,
                        operation: admitted_operation(step, entry),
                    });
                }
                OplogEntry::AgentInvocationStarted {
                    invocation_id: idempotency_key,
                    ..
                }
                | OplogEntry::CancelPendingInvocation {
                    invocation_id: idempotency_key,
                    ..
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
                record.target,
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
                    failure_class,
                    divergence,
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
                    failure_class: *failure_class,
                    divergence: divergence.clone(),
                });
            }
            (
                OplogEntry::SuccessfulUpdate {
                    timestamp,
                    target_revision,
                    new_component_size,
                    snapshot_assisted_details,
                    bootstrap,
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
                            bootstrap: bootstrap.clone(),
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
                            bootstrap: bootstrap.clone(),
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
                    bootstrap,
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
                    bootstrap: bootstrap.clone(),
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

/// The operation of the request in `entry`, from its update step when the region fold decoded
/// it already. A reconstruction folds its regions over the update entries only, so a pending
/// invocation there has no step and is decoded here.
fn admitted_operation(
    step: Option<&UpdateStep>,
    entry: &OplogEntry,
) -> Option<InvocationOperation> {
    match step {
        Some(UpdateStep::InvocationAdmitted(operation)) => operation.clone(),
        Some(UpdateStep::ManualAdmitted(_)) => None,
        _ => request_operation(entry),
    }
}

pub(super) fn calculate_invocation_results(
    invocation_results: InvocationResultMembership,
    requests: InvocationRequests,
    deleted_regions: &DeletedRegions,
    steps: &BTreeMap<OplogIndex, UpdateStep>,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> (InvocationResultMembership, InvocationRequests) {
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
    let mut requests = requests;

    fold_invocation_result_entries(
        &mut requests,
        deleted_regions,
        entries,
        |index, entry| admitted_operation(steps.get(&index), entry),
        |key, position| {
            // A withdrawal never replaces a result the membership holds. A key that only the
            // Bloom filter may contain still records its withdrawal: the recent results must
            // hold every result after the oldest one they keep, so the persisted index can
            // answer a miss from them.
            if position.end == InvocationEnd::Cancelled && invocation_results.contains_key(key) {
                return;
            }
            // A rejection `Error` names its cancelled request only when the cancellation was
            // folded with it; otherwise the earlier fold has recorded the request.
            let position = match invocation_results.position(key) {
                Some(known) if position.request.is_none() => InvocationResultPosition {
                    request: known.request,
                    operation: known.operation.clone(),
                    accepted_at: known.accepted_at,
                    ..position
                },
                _ => position,
            };
            invocation_results.insert(*key, position)
        },
    );

    (invocation_results, requests)
}

/// The request entries of the invocations whose results a fold has not seen yet: the
/// `AgentInvocationStarted` entry of the running invocation and the `PendingAgentInvocation`
/// entries of the pending ones. A result takes the position of its request from here.
#[derive(Clone, Debug, Default, PartialEq, Eq, desert_rust::BinaryCodec)]
pub struct InvocationRequests {
    pub current: Option<RunningInvocation>,
    pub pending: HashMap<InvocationId, InvocationRequestRef>,
    /// The last invocation cancelled before it started, with its `PendingAgentInvocation` entry.
    /// A rejection `Error` can follow the cancellation and name the same invocation.
    pub cancelled: Option<(InvocationId, InvocationRequestRef)>,
    /// The last invocation cancelled again while it was neither pending nor running: its
    /// rejection pair was written after a user cancellation had withdrawn it, and that
    /// cancellation stays its result. Kept across folds, and in the status as
    /// `AgentStatusRecord::withdrawn_again`, so a split pair is attributed the same.
    pub withdrawn_again: Option<InvocationId>,
}

impl InvocationRequests {
    /// The requests at `status`, whose pending invocations still hold their request entries.
    pub fn of_status(status: &AgentStatusRecord) -> Self {
        Self {
            current: status.running.clone(),
            pending: status
                .pending_invocations
                .iter()
                .filter_map(|pending| {
                    pending.idempotency_key.map(|key| {
                        (
                            key,
                            InvocationRequestRef {
                                index: pending.oplog_index,
                                accepted_at: pending.timestamp,
                                operation: pending.operation.clone(),
                            },
                        )
                    })
                })
                .collect(),
            cancelled: None,
            withdrawn_again: status.withdrawn_again,
        }
    }

    fn request_of(&self, key: &InvocationId) -> Option<&InvocationRequestRef> {
        match &self.current {
            Some(running) if running.key == *key => Some(&running.request),
            _ => self
                .cancelled
                .as_ref()
                .filter(|(cancelled, _)| cancelled == key)
                .map(|(_, request)| request),
        }
    }

    fn is_current(&self, key: &InvocationId) -> bool {
        self.current
            .as_ref()
            .is_some_and(|running| running.key == *key)
    }
}

/// The operations that `status` already knows, by the index of their request entry. Folds that
/// read entries back from the oplog take the operation from here instead of decoding the payload
/// again; an operation that is not here stays `None` and is filled at read time.
pub(crate) fn known_request_operations(
    status: &AgentStatusRecord,
) -> HashMap<OplogIndex, InvocationOperation> {
    let pending = status
        .pending_invocations
        .iter()
        .filter_map(|pending| Some((pending.oplog_index, pending.operation.clone()?)));
    let running = status
        .running
        .iter()
        .filter_map(|running| Some((running.request.index, running.request.operation.clone()?)));
    let finished = status
        .invocation_results
        .iter()
        .filter_map(|(_, position)| Some((position.request?, position.operation.clone()?)));
    pending.chain(running).chain(finished).collect()
}

/// Gives every invocation result in `entries` to `observe_result`, with the position of its
/// request, and keeps `requests` as the running and pending invocations.
///
/// `AgentInvocationFinished` entries name their invocation, so they are attributed by that name
/// and not by their position. An invocation `Error` is attributed to the invocation it names only
/// while that invocation runs, or when it is the agent's `InvalidRequest` or `PermissionDenied`
/// rejection of a cancelled pending invocation. A retry error of a background task that outlives
/// its invocation, or of an entity body, therefore never replaces a finished result. `Exited`
/// names no invocation and is attributed to the running one.
///
/// `operation_of` gives the operation of a `PendingAgentInvocation` entry, and of an
/// `AgentInvocationStarted` entry whose pending entry the fold did not see.
///
/// A withdrawal, a result whose end is [`InvocationEnd::Cancelled`], is the result of its
/// invocation only when the invocation has no result yet: the invocation loop also withdraws a
/// queued duplicate of an invocation that already has one. The fold does not know the results of
/// earlier folds, so `observe_result` decides this.
pub(crate) fn fold_invocation_result_entries(
    requests: &mut InvocationRequests,
    deleted_regions: &DeletedRegions,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    operation_of: impl Fn(OplogIndex, &OplogEntry) -> Option<InvocationOperation>,
    mut observe_result: impl FnMut(&InvocationId, InvocationResultPosition),
) {
    for (oplog_idx, entry) in entries {
        // Pending requests follow `calculate_pending_invocations`, which keeps them in deleted
        // regions too.
        let pending_request = match entry {
            OplogEntry::PendingAgentInvocation {
                invocation_id,
                timestamp,
                ..
            } => {
                requests.pending.insert(
                    *invocation_id,
                    InvocationRequestRef {
                        index: *oplog_idx,
                        accepted_at: *timestamp,
                        operation: operation_of(*oplog_idx, entry),
                    },
                );
                None
            }
            OplogEntry::AgentInvocationStarted { invocation_id, .. }
            | OplogEntry::CancelPendingInvocation { invocation_id, .. } => {
                requests.pending.remove(invocation_id)
            }
            _ => None,
        };

        // Skipping entries in deleted regions (by revert)
        if deleted_regions.is_in_deleted_region(*oplog_idx) {
            continue;
        }

        let changed_at = entry.timestamp();
        let position = |end, request: Option<&InvocationRequestRef>| InvocationResultPosition {
            terminal: *oplog_idx,
            request: request.map(|request| request.index),
            end,
            operation: request.and_then(|request| request.operation.clone()),
            accepted_at: request.map(|request| request.accepted_at),
            changed_at,
        };
        match entry {
            OplogEntry::AgentInvocationStarted { invocation_id, .. } => {
                let (accepted_at, operation) = match pending_request {
                    Some(pending) => (pending.accepted_at, pending.operation),
                    None => (changed_at, operation_of(*oplog_idx, entry)),
                };
                requests.current = Some(RunningInvocation {
                    key: *invocation_id,
                    request: InvocationRequestRef {
                        index: *oplog_idx,
                        accepted_at,
                        operation,
                    },
                    last_change: *oplog_idx,
                    last_change_at: changed_at,
                });
            }
            OplogEntry::AgentInvocationFinished { invocation_id, .. }
            | OplogEntry::AgentInvocationFailed {
                invocation_id,
                entity_parent_start_index: None,
                ..
            } => {
                let end = if matches!(entry, OplogEntry::AgentInvocationFinished { .. }) {
                    InvocationEnd::Completed
                } else {
                    InvocationEnd::Failed
                };
                observe_result(
                    invocation_id,
                    position(end, requests.request_of(invocation_id)),
                );
                if requests.is_current(invocation_id) {
                    requests.current = None;
                }
            }
            OplogEntry::CancelPendingInvocation { invocation_id, .. } => {
                if !requests.is_current(invocation_id) {
                    observe_result(
                        invocation_id,
                        position(InvocationEnd::Cancelled, pending_request.as_ref()),
                    );
                    requests.withdrawn_again = pending_request.is_none().then_some(*invocation_id);
                    requests.cancelled = pending_request.map(|request| (*invocation_id, request));
                }
            }
            OplogEntry::Error {
                kind: OplogErrorKind::Invocation,
                invocation_id: Some(invocation_id),
                entity_parent_start_index,
                error,
                ..
            } if requests.is_current(invocation_id)
                || (entity_parent_start_index.is_none()
                    && requests.withdrawn_again.as_ref() != Some(invocation_id)
                    && matches!(
                        error,
                        AgentError::InvalidRequest(_) | AgentError::PermissionDenied(_)
                    )) =>
            {
                let rejection = entity_parent_start_index.is_none()
                    && matches!(
                        error,
                        AgentError::InvalidRequest(_) | AgentError::PermissionDenied(_)
                    );
                let end = if rejection {
                    InvocationEnd::Failed
                } else {
                    InvocationEnd::Retrying
                };
                observe_result(
                    invocation_id,
                    position(end, requests.request_of(invocation_id)),
                );
                if let Some(running) = &mut requests.current
                    && running.key == *invocation_id
                {
                    running.last_change = *oplog_idx;
                    running.last_change_at = changed_at;
                }
            }
            OplogEntry::Exited { .. } => {
                if let Some(running) = &requests.current {
                    observe_result(
                        &running.key,
                        position(InvocationEnd::Failed, Some(&running.request)),
                    );
                }
            }
            OplogEntry::Interrupted { .. }
            | OplogEntry::Suspend { .. }
            | OplogEntry::Resumed { .. }
            | OplogEntry::Jump { .. } => {
                if let Some(running) = &mut requests.current {
                    running.last_change = *oplog_idx;
                    running.last_change_at = changed_at;
                }
            }
            _ => {}
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
            OplogEntry::MemorySizeReconciled { bytes, .. } => {
                result = *bytes;
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
                delivery,
                ..
            } => {
                result.insert(
                    *plugin_grant_id,
                    OplogProcessorCheckpointState {
                        target_agent_id: Some(*target_agent_id),
                        confirmed_up_to: *confirmed_up_to,
                        sending_up_to: *sending_up_to,
                        last_batch_start: *last_batch_start,
                        delivery: delivery.clone(),
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
                        delivery: None,
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
                            delivery: None,
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
