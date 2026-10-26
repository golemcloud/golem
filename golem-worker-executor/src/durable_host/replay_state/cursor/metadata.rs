use super::*;

/// The first oplog index whose recovery metadata a new replay generation can need.
///
/// A start from an execution snapshot replaces the history `INITIAL.next()..=snapshot_end` with
/// the loaded state and skips it. A snapshot is taken only at a clean boundary: no durable call
/// or scope is open, and no tool attachment endpoint or unread accepted output
/// remains (`PrivateDurableWorkerState::snapshot_boundary_blocker`). Therefore no `Start`,
/// terminal, delivery marker, attachment write, attachment control or attachment re-observation
/// after the snapshot refers to metadata before it, and the scan starts after the snapshot. A
/// consumer stream re-observation reads its original record directly, wherever it is.
///
/// Jump and revert regions after the snapshot stay in the scanned range, because a later
/// re-observation can refer to an observation that such a region removed. When the skipped
/// regions do not cover the whole replaced prefix, the history before the snapshot is still
/// replayed, and the scan starts at `INITIAL`.
pub(in crate::durable_host::replay_state) fn recovery_metadata_floor(
    skipped_regions: &DeletedRegions,
    snapshot_end: Option<OplogIndex>,
) -> OplogIndex {
    snapshot_end
        .filter(|end| {
            skipped_regions
                .regions()
                .any(|region| region.start <= OplogIndex::INITIAL.next() && region.end >= *end)
        })
        .map_or(OplogIndex::INITIAL, |end| end.next())
}

impl ReplayState {
    pub(crate) async fn new_for_owner(
        owned_agent_id: OwnedAgentId,
        oplog: Arc<dyn Oplog>,
        skipped_regions: DeletedRegions,
        initial_snapshot_skip_end: Option<OplogIndex>,
        owner_tool_operations: Arc<crate::durable_host::tool::operation::OwnerToolOperations>,
        last_oplog_index: OplogIndex,
        wallet: Option<Arc<crate::worker::owner_wallet::OwnerWallet>>,
    ) -> Result<Self, WorkerExecutorError> {
        let next_skipped_region = skipped_regions.find_next_deleted_region(OplogIndex::NONE);
        let mut tool_attachment_reconstruction = ToolAttachmentReconstruction::default();
        let metadata_floor = recovery_metadata_floor(&skipped_regions, initial_snapshot_skip_end);
        let (completion_markers, stream_observations, transaction_markers, start_invocation_ids) =
            Self::scan_completion_markers(
                &oplog,
                metadata_floor,
                last_oplog_index,
                &skipped_regions,
                &mut tool_attachment_reconstruction,
            )
            .await?;
        crate::metrics::workers::record_replay_metadata_scan(
            if metadata_floor == OplogIndex::INITIAL {
                "initial"
            } else {
                "snapshot"
            },
            (last_oplog_index.as_u64() + 1).saturating_sub(metadata_floor.as_u64()),
        );
        let concurrent_resolver = ConcurrentReplayResolver::default();
        let reconstruction_claims = concurrent_resolver.reconstruction_claims();
        let cursor = ReplayCursor {
            owned_agent_id,
            oplog,
            wallet,
            owner_tool_operations,
            advance_gate: Arc::new(tokio::sync::Mutex::new(())),
            delivery_failure: std::sync::Mutex::new(None),
            stream_observations: std::sync::Mutex::new(stream_observations),
            start_invocation_ids: std::sync::Mutex::new(start_invocation_ids),
            position: PublishedPosition {
                last_replayed_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
                last_replayed_non_hint_index: AtomicOplogIndex::from_oplog_index(OplogIndex::NONE),
                has_seen_logs: AtomicBool::new(false),
            },
            replay_target: AtomicOplogIndex::from_oplog_index(last_oplog_index),
            transition_phase: AtomicU8::new(ReplayTransitionPhase::Replaying as u8),
            state: Mutex::new(CursorState {
                skipped_regions,
                next_skipped_region,
                initial_snapshot_skip_end,
                skip_hints_after_delivery: false,
                replay_buffer: VecDeque::new(),
                pending_fork_starts: HashSet::new(),
                concurrent_resolver,
                claimed_starts: HashSet::new(),
                custom_subtrees: HashMap::new(),
                retained_starts: std::collections::BTreeMap::new(),
            }),
            unclaimed_retained_starts: RetainedStartCounts::default(),
            reconstruction_claims,
            completion_markers: std::sync::Mutex::new(completion_markers),
            transaction_markers: std::sync::Mutex::new(transaction_markers),
            tool_attachment_reconstruction: std::sync::Mutex::new(tool_attachment_reconstruction),
            attachment_validation_slot: tokio::sync::Semaphore::new(1),
            log_hashes: std::sync::Mutex::new(HashMap::new()),
            pending_replay_events: std::sync::Mutex::new(Vec::new()),
            progress: Notify::new(),
            #[cfg(test)]
            active_body_waits: tokio::sync::watch::Sender::new(0),
            #[cfg(test)]
            primary_publication_gate: std::sync::Mutex::new(None),
            #[cfg(test)]
            retained_claim_miss: std::sync::Mutex::new(None),
        };
        {
            // No concurrency during construction: the replay state is not shared yet, so driving the
            // cursor without anyone to notify is sound.
            let mut tx = cursor.tx().await?;
            tx.move_replay_idx(OplogIndex::INITIAL).await?; // By this we handle initial skipped regions applied by manual updates correctly
            tx.skip_forward().await?;
        }
        if cursor.is_live() {
            cursor.publish_live();
        }
        Ok(Self {
            cursor: Arc::new(cursor),
        })
    }

    /// Scans retained history for delivery, transaction, stream and attachment recovery metadata.
    /// Exactly one visible `CompletionDelivered` or `CompletionDiscarded` may reference a
    /// `Start`; duplicates or a conflicting pair are oplog corruption.
    pub(in crate::durable_host::replay_state) async fn scan_completion_markers(
        oplog: &Arc<dyn Oplog>,
        from: OplogIndex,
        to: OplogIndex,
        skipped_regions: &DeletedRegions,
        attachments: &mut ToolAttachmentReconstruction,
    ) -> Result<
        (
            CompletionMarkers,
            StreamObservationPositions,
            TransactionMarkers,
            StartsByInvocationId,
        ),
        WorkerExecutorError,
    > {
        const CHUNK_SIZE: u64 = 1024;
        let mut markers = CompletionMarkers::default();
        let mut stream_observations = StreamObservationPositions::new();
        let mut transaction_markers = TransactionMarkers::new();
        let mut start_invocation_ids = StartsByInvocationId::new();
        let mut next = from;
        while next <= to {
            let available = u64::from(to) - u64::from(next) + 1;
            let entries = oplog.read_exact(next, CHUNK_SIZE.min(available)).await?;
            let last_read = *entries.last_key_value().unwrap().0;
            for (marker_idx, entry) in entries {
                if marker_idx > to {
                    break;
                }
                if let OplogEntry::ToolAttachmentRead {
                    observation,
                    payload,
                    ..
                } = &entry
                {
                    observation
                        .validate_payload_presence(payload.is_some())
                        .map_err(|error| {
                            WorkerExecutorError::runtime(format!(
                                "invalid attachment payload at {marker_idx}: {error:?}"
                            ))
                        })?;
                    attachments
                        .history
                        .observations
                        .insert(marker_idx, *observation.clone());
                }
                if skipped_regions.is_in_deleted_region(marker_idx)
                    && !matches!(
                        entry,
                        OplogEntry::Start { .. }
                            | OplogEntry::End { .. }
                            | OplogEntry::ToolAttachmentControl { .. }
                    )
                {
                    continue;
                }
                let marker = match entry {
                    entry @ (OplogEntry::BeginRemoteTransaction { .. }
                    | OplogEntry::PreCommitRemoteTransaction { .. }
                    | OplogEntry::PreRollbackRemoteTransaction { .. }
                    | OplogEntry::CommittedRemoteTransaction { .. }
                    | OplogEntry::RolledBackRemoteTransaction { .. }) => {
                        let owner = scope_entry_owner(marker_idx, &entry).unwrap();
                        if !skipped_regions.is_in_deleted_region(owner) {
                            transaction_markers
                                .entry(owner)
                                .or_default()
                                .insert(marker_idx, entry);
                        }
                        None
                    }
                    entry @ OplogEntry::Start { .. } => {
                        let recorded = RecordedStart::of(&entry)
                            .expect("a Start entry has a recorded identity");
                        let OplogEntry::Start {
                            function_name,
                            parent_start_index,
                            observational_owner,
                            invocation_id,
                            request,
                            ..
                        } = entry
                        else {
                            unreachable!("matched a Start entry");
                        };
                        if let Some(invocation_id) = invocation_id {
                            start_invocation_ids
                                .entry(invocation_id)
                                .or_default()
                                .insert(marker_idx, recorded);
                        }
                        let attachment_write = matches!(
                            function_name,
                            HostFunctionName::GolemToolOutputWrite
                                | HostFunctionName::GolemToolStdinWrite
                        );
                        if attachment_write {
                            let request = request.ok_or_else(|| {
                                WorkerExecutorError::runtime("output write Start has no request")
                            })?;
                            let request = oplog
                                .download_payload(request)
                                .await
                                .map_err(WorkerExecutorError::runtime)?;
                            let request = golem_common::model::oplog::HostRequestToolAttachmentWrite::try_from(request)
                                .map_err(WorkerExecutorError::runtime)?;
                            attachments.requests.insert(marker_idx, request.request);
                        } else if matches!(
                            function_name,
                            HostFunctionName::GolemToolOutputFinish
                                | HostFunctionName::GolemToolStdinFinish
                                | HostFunctionName::GolemToolOutputFail
                                | HostFunctionName::GolemToolStdinFail
                        ) {
                            let request = request.ok_or_else(|| {
                                WorkerExecutorError::runtime(
                                    "attachment terminal Start has no request",
                                )
                            })?;
                            let request = oplog
                                .download_payload(request)
                                .await
                                .map_err(WorkerExecutorError::runtime)?;
                            let attachment = if matches!(
                                function_name,
                                HostFunctionName::GolemToolOutputFail
                                    | HostFunctionName::GolemToolStdinFail
                            ) {
                                golem_common::model::oplog::HostRequestToolAttachmentFail::try_from(
                                    request,
                                )
                                .map_err(WorkerExecutorError::runtime)?
                                .attachment
                            } else {
                                golem_common::model::oplog::HostRequestToolAttachmentFinish::try_from(request)
                                    .map_err(WorkerExecutorError::runtime)?.attachment
                            };
                            attachments.terminal_requests.insert(marker_idx, attachment);
                        }
                        attachments.starts.insert(
                            marker_idx,
                            super::super::AttachmentStartOwnership {
                                parent: parent_start_index,
                                observational_owner,
                                custom_root: matches!(function_name, HostFunctionName::Custom(_))
                                    && invocation_id.is_some(),
                            },
                        );
                        None
                    }
                    OplogEntry::End {
                        start_index,
                        response,
                        ..
                    } if attachments.requests.contains_key(&start_index) => {
                        let response = response.ok_or_else(|| {
                            WorkerExecutorError::runtime(format!(
                                "output write End at {marker_idx} has no response"
                            ))
                        })?;
                        let response = oplog
                            .download_payload(response)
                            .await
                            .map_err(WorkerExecutorError::runtime)?;
                        let response =
                            golem_common::model::oplog::HostResponseToolAttachmentWrite::try_from(
                                response,
                            )
                            .map_err(WorkerExecutorError::runtime)?;
                        let request = attachments.requests.get(&start_index).ok_or_else(|| {
                            WorkerExecutorError::runtime("output write End has no indexed request")
                        })?;
                        let metadata = response
                            .outcome
                            .metadata(request, start_index, true)
                            .ok_or_else(|| {
                                WorkerExecutorError::runtime("invalid output write outcome")
                            })?;
                        if request.attachment.creation_start >= start_index
                            || start_index >= marker_idx
                            || !request.attachment.creation_start.is_defined()
                        {
                            return Err(WorkerExecutorError::runtime(
                                "invalid output write chronology",
                            ));
                        }
                        attachments.history.writes.insert(marker_idx, metadata);
                        if response.outcome == golem_common::model::oplog::payload::tool_attachment::ToolAttachmentWriteOutcome::HostResourceExhausted {
                            use golem_common::model::oplog::attachment::{ToolAttachmentControlMetadata, ToolAttachmentControl, ToolAttachmentTerminal};
                            attachments.history.controls.insert(marker_idx, ToolAttachmentControlMetadata {
                                attachment: request.attachment,
                                producer_start: Some(start_index),
                                entity_parent_start: None,
                                transition: ToolAttachmentControl::TerminalSelected(ToolAttachmentTerminal::HostResourceExhausted),
                                retained: true,
                            });
                        }
                        if matches!(response.outcome, golem_common::model::oplog::payload::tool_attachment::ToolAttachmentWriteOutcome::Accepted) {
                            let mut ancestor = start_index;
                            while let Some(parent) = attachments.starts.get(&ancestor).and_then(|ownership| ownership.parent) {
                                if parent >= ancestor {
                                    return Err(WorkerExecutorError::runtime(
                                        "invalid accepted tool output ancestor",
                                    ));
                                }
                                ancestor = parent;
                            }
                            attachments.accepted.insert(start_index, marker_idx);
                        }
                        None
                    }
                    OplogEntry::End {
                        start_index,
                        response,
                        ..
                    } if attachments.terminal_requests.contains_key(&start_index) => {
                        use golem_common::model::oplog::attachment::{
                            ToolAttachmentControl, ToolAttachmentControlMetadata,
                        };
                        use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentTerminalOutcome;
                        let response = response.ok_or_else(|| {
                            WorkerExecutorError::runtime("attachment terminal End has no response")
                        })?;
                        let response = oplog
                            .download_payload(response)
                            .await
                            .map_err(WorkerExecutorError::runtime)?;
                        let response = golem_common::model::oplog::HostResponseToolAttachmentTerminalResult::try_from(response)
                            .map_err(WorkerExecutorError::runtime)?;
                        if let ToolAttachmentTerminalOutcome::Selected(terminal) = response.outcome
                        {
                            attachments.history.controls.insert(
                                marker_idx,
                                ToolAttachmentControlMetadata {
                                    attachment: attachments.terminal_requests[&start_index],
                                    producer_start: Some(start_index),
                                    entity_parent_start: None,
                                    transition: ToolAttachmentControl::TerminalSelected(terminal),
                                    retained: true,
                                },
                            );
                        }
                        None
                    }
                    OplogEntry::CompletionDelivered { start_index, .. } => {
                        Some((start_index, CompletionMarker::Delivered(marker_idx)))
                    }
                    OplogEntry::CompletionDiscarded { start_index, .. } => {
                        Some((start_index, CompletionMarker::Discarded(marker_idx)))
                    }
                    OplogEntry::ToolAttachmentControl {
                        attachment,
                        transition,
                        entity_parent_start_index,
                        ..
                    } => {
                        attachments.history.controls.insert(
                            marker_idx,
                            golem_common::model::oplog::attachment::ToolAttachmentControlMetadata {
                                attachment,
                                producer_start: None,
                                entity_parent_start: entity_parent_start_index,
                                transition,
                                retained: true,
                            },
                        );
                        None
                    }
                    OplogEntry::ToolAttachmentRead {
                        observation,
                        entity_parent_start_index,
                        ..
                    } => {
                        attachments.boundaries.insert(marker_idx);
                        let parent = match observation.observation {
                            golem_common::model::durable_stream::StreamConsumerObservation::Guest {
                                parent_start_index,
                            } => parent_start_index,
                            golem_common::model::durable_stream::StreamConsumerObservation::Host => {
                                entity_parent_start_index
                            }
                        };
                        stream_observations.insert(marker_idx, parent);
                        None
                    }
                    OplogEntry::StreamSession { record, .. } => {
                        use golem_common::model::durable_stream::{
                            StreamConsumerObservation, StreamSessionRecord,
                        };
                        let record: StreamSessionRecord = oplog
                            .download_payload(record)
                            .await
                            .map_err(WorkerExecutorError::runtime)?;
                        if matches!(
                            &record,
                            StreamSessionRecord::ConsumerItemValue(_)
                                | StreamSessionRecord::ConsumerTerminal(_)
                        ) && !record.has_supported_format()
                        {
                            return Err(WorkerExecutorError::runtime(format!(
                                "corrupt consumer observation at {marker_idx}"
                            )));
                        }
                        crate::durable_host::durable_stream::DurableStreamStore::validate_consumer_reobservation(
                            oplog.as_ref(), marker_idx, &record,
                        ).await.map_err(|error| WorkerExecutorError::unexpected_oplog_entry(
                            "unchanged logical consumer input", error.to_string(),
                        ))?;
                        let observation = match record {
                            StreamSessionRecord::ConsumerItemValue(record) => {
                                Some(record.observation)
                            }
                            StreamSessionRecord::ConsumerTerminal(record) => {
                                Some(record.observation)
                            }
                            _ => None,
                        };
                        if let Some(StreamConsumerObservation::Guest { parent_start_index }) =
                            observation
                        {
                            stream_observations.insert(marker_idx, parent_start_index);
                        }
                        None
                    }
                    _ => None,
                };
                if let Some((start_index, marker)) = marker
                    && let Some(previous) = markers.insert(start_index, marker)
                {
                    return Err(WorkerExecutorError::runtime(format!(
                        "corrupt oplog: multiple completion-delivery markers reference the durable call Start at {start_index} ({} at {}, {} at {})",
                        previous.entry_name(),
                        previous.index(),
                        marker.entry_name(),
                        marker.index(),
                    )));
                }
            }
            next = last_read.next();
        }
        attachments.history.retain_history(skipped_regions);
        attachments.retain(|index| !skipped_regions.is_in_deleted_region(index));
        attachments
            .history
            .validate_observations(skipped_regions)
            .map_err(|error| {
                WorkerExecutorError::runtime(format!("invalid attachment history: {error:?}"))
            })?;
        attachments.index_logical_observations();
        Ok((
            markers,
            stream_observations,
            transaction_markers,
            start_invocation_ids,
        ))
    }

    pub(crate) fn transaction_marker(
        &self,
        owner: OplogIndex,
        predicate: impl Fn(&OplogEntry) -> bool,
    ) -> Option<OplogEntry> {
        self.cursor
            .transaction_markers
            .lock()
            .unwrap()
            .get(&owner)?
            .values()
            .find(|entry| predicate(entry))
            .cloned()
    }

    /// Records a live-appended successful-completion delivery marker in the same map populated by
    /// the replay scan.
    pub(in crate::durable_host::replay_state) fn record_completion_marker(
        &self,
        start_index: OplogIndex,
        marker: CompletionMarker,
    ) {
        let previous = self
            .cursor
            .completion_markers
            .lock()
            .unwrap()
            .insert(start_index, marker);
        if let Some(previous) = previous {
            tracing::warn!(
                "duplicate completion-delivery marker recorded for durable call Start {start_index}: {} at {}, {} at {}",
                previous.entry_name(),
                previous.index(),
                marker.entry_name(),
                marker.index(),
            );
        }
    }

    pub fn record_discarded_completion(&self, start_index: OplogIndex, marker_index: OplogIndex) {
        self.record_completion_marker(start_index, CompletionMarker::Discarded(marker_index));
    }

    pub fn record_delivered_completion(&self, start_index: OplogIndex, marker_index: OplogIndex) {
        self.record_completion_marker(start_index, CompletionMarker::Delivered(marker_index));
    }

    /// Sets the replay target. This is a phase-boundary operation (e.g. refreshing the target
    /// before replay resumes); it must not race with concurrent cursor advances.
    ///
    /// The completion-marker map is kept in sync with the visible prefix `[.., target]`:
    ///
    /// - Growing the target makes a previously invisible oplog range visible, so the newly
    ///   visible range `(old_target, new_target]` is scanned for completion-delivery markers
    ///   *before* the new target is published. A cursor whose target grows past a marker must park
    ///   the marked `End` instead of delivering it.
    ///   The merged additions are validated (duplicate markers for the same `Start` are oplog
    ///   corruption) before anything is mutated.
    /// - Shrinking the target hides part of the oplog, so markers beyond the new target are
    ///   removed *before* the smaller target is published — a later regrowth rescans the exposed
    ///   range and rediscovers them (without false duplicate-marker errors), and delivery-time
    ///   validation ([`Self::await_resolution_outcome`]) never sees a marker outside the visible
    ///   prefix.
    ///
    /// Both directions run under the cursor transaction lock, so replay cannot advance while the
    /// map and the target are being updated.
    pub async fn set_replay_target(
        &self,
        new_target: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        let cursor = &*self.cursor;
        self.with_tx(async |tx| {
            let old_target = cursor.replay_target();
            if new_target != old_target
                && let Some(wallet) = &cursor.wallet
            {
                wallet.reopen_replay_prefix()?;
            }
            match new_target.cmp(&old_target) {
                std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Less => {
                    tx.st.replay_buffer.clear();
                    tx.prune_retained_starts(|idx| idx > new_target);
                    cursor
                        .completion_markers
                        .lock()
                        .unwrap()
                        .truncate_after(new_target);
                    cursor.stream_observations.lock().unwrap().retain(|index, _| *index <= new_target);
                    cursor.start_invocation_ids.lock().unwrap().retain(|_, starts| {
                        starts.retain(|index, _| *index <= new_target);
                        !starts.is_empty()
                    });
                    cursor.transaction_markers.lock().unwrap().retain(|_, entries| {
                        entries.retain(|index, _| *index <= new_target);
                        !entries.is_empty()
                    });
                    cursor.tool_attachment_reconstruction.lock().unwrap()
                        .truncate(new_target);
                }
                std::cmp::Ordering::Greater => {
                    let mut attachments = cursor.tool_attachment_reconstruction.lock().unwrap().clone();
                    let (additions, stream_observations, transaction_markers, start_invocation_ids) = Self::scan_completion_markers(
                        &cursor.oplog,
                        old_target.next(),
                        new_target,
                        &tx.st.skipped_regions,
                        &mut attachments,
                    )
                    .await?;
                    if !additions.is_empty() {
                        let mut markers = cursor.completion_markers.lock().unwrap();
                        for (start_index, marker) in additions.iter() {
                            // Rediscovering the exact marker already in the map (recorded live by
                            // this instance before the target
                            // grew over it) is idempotent; only a *different* marker for the same
                            // `Start` is oplog corruption.
                            if let Some(previous) = markers.get(start_index)
                                && previous != marker
                            {
                                return Err(WorkerExecutorError::runtime(format!(
                                    "corrupt oplog: multiple completion-delivery markers reference the durable call Start at {start_index} ({} at {}, {} at {})",
                                    previous.entry_name(),
                                    previous.index(),
                                    marker.entry_name(),
                                    marker.index(),
                                )));
                            }
                        }
                        for (start_index, marker) in additions.by_start {
                            markers.insert(start_index, marker);
                        }
                    }
                    cursor.stream_observations.lock().unwrap().extend(stream_observations);
                    {
                        let mut current = cursor.start_invocation_ids.lock().unwrap();
                        for (invocation_id, starts) in start_invocation_ids {
                            current.entry(invocation_id).or_default().extend(starts);
                        }
                    }
                    {
                        let mut current = cursor.transaction_markers.lock().unwrap();
                        for (owner, entries) in transaction_markers {
                            current.entry(owner).or_default().extend(entries);
                        }
                    }
                    let mut current = cursor.tool_attachment_reconstruction.lock().unwrap();
                    current.starts.extend(attachments.starts);
                    current.accepted.extend(attachments.accepted);
                    current.requests = attachments.requests;
                    current.terminal_requests = attachments.terminal_requests;
                    current.history = attachments.history;
                    current.boundaries = attachments.boundaries;
                    current.index_logical_observations();
                }
            }
            if new_target > cursor.last_replayed_index() {
                cursor
                    .transition_phase
                    .store(ReplayTransitionPhase::Replaying as u8, Ordering::Release);
                cursor
                    .pending_replay_events
                    .lock()
                    .unwrap()
                    .retain(|event| !matches!(event, ReplayEvent::ReplayFinished));
            }
            cursor.replay_target.set(new_target);
            if cursor.is_live()
                && let Some(wallet) = &cursor.wallet
            {
                wallet.finish_replay_prefix();
            }
            if new_target != old_target {
                tx.notify_progress = true;
            }
            Ok(())
        })
        .await
    }
}
