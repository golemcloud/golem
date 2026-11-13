use super::*;

impl<'a> StatusOplogReader<'a> {
    pub fn new(
        this: &impl HasOplogService,
        owned_agent_id: &'a OwnedAgentId,
        agent_mode: AgentMode,
        opened: Option<&'a dyn Oplog>,
        horizon: OplogIndex,
    ) -> Self {
        Self {
            service: this.oplog_service(),
            opened,
            owned_agent_id,
            agent_mode,
            horizon,
        }
    }

    pub(super) async fn read_exact(
        &self,
        first: OplogIndex,
        count: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        if first > self.horizon || count == 0 {
            return Ok(BTreeMap::new());
        }
        let count = count.min(self.horizon.as_u64() - first.as_u64() + 1);
        match self.opened {
            Some(oplog) => oplog.read_exact(first, count).await,
            None => {
                self.service
                    .read_exact(self.owned_agent_id, self.agent_mode, first, count)
                    .await
            }
        }
    }
}

/// Like calculate_last_known_status, but assumes that the oplog exists and has at least a Create entry in it.
pub async fn calculate_last_known_status_for_existing_worker<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_known: Option<AgentStatusRecord>,
) -> Result<AgentStatusRecord, StatusError>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    calculate_last_known_status(this, owned_agent_id, agent_mode, last_known)
        .await?
        .ok_or_else(|| "failed to calculate status for existing worker".into())
}

/// Gets the last cached worker status record and the new oplog entries and calculates the new worker
/// status, falling back to a full recompute from the start of the oplog when the cached baseline can
/// no longer be folded forward (e.g. a jump deleted its index).
///
/// This is the no-checkpoint variant; callers that have a [`WorkerService`] should prefer
/// [`calculate_last_known_status_with_checkpoint`] so a jump-induced full recompute can instead fold
/// forward from the clean status checkpoint.
///
/// [`WorkerService`]: crate::services::worker::WorkerService
pub async fn calculate_last_known_status<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_known: Option<AgentStatusRecord>,
) -> Result<Option<AgentStatusRecord>, StatusError>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    calculate_last_known_status_with_checkpoint_reader(
        this,
        owned_agent_id,
        agent_mode,
        last_known,
        || async { None },
    )
    .await
}

/// Calculates the latest worker status, folding forward from the freshest usable baseline and, on a
/// fold failure, from the *clean* status checkpoint read from `this`'s [`WorkerService`].
///
/// This is the variant production callers should use: the checkpoint is read from
/// `this.worker_service()`, so callers no longer pass a checkpoint-read closure. The read still
/// happens lazily (only when the live cache baseline cannot be folded), so the common path pays no
/// extra read. See [`calculate_last_known_status_with_checkpoint_reader`] for the baseline order.
///
/// [`WorkerService`]: crate::services::worker::WorkerService
pub async fn calculate_last_known_status_with_checkpoint<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    fingerprint: AgentFingerprint,
    agent_mode: AgentMode,
    last_known: Option<AgentStatusRecord>,
) -> Result<Option<AgentStatusRecord>, StatusError>
where
    T: HasOplogService + HasConfig + HasComponentService + HasWorkerService + Sync,
{
    let worker_service = this.worker_service();
    calculate_last_known_status_with_checkpoint_reader(
        this,
        owned_agent_id,
        agent_mode,
        last_known,
        || async move {
            // The checkpoint is only a fold baseline, and this function has no way to report a
            // read failure: falling back to a full recompute costs time, not correctness.
            worker_service
                .read_status_checkpoint(owned_agent_id, fingerprint, agent_mode)
                .await
                .unwrap_or_else(|err| {
                    tracing::error!(
                        "Failed to read the status checkpoint for {owned_agent_id}: {err}"
                    );
                    None
                })
        },
    )
    .await
}

/// Calculates the latest worker status, trying baselines in order of decreasing freshness:
///
/// 1. the live cached status (`last_known`), folded forward — the common cold-load path;
/// 2. on a fold failure (e.g. a jump deleted the cached index), the *clean* status checkpoint
///    (read lazily via `read_checkpoint`), folded forward — this predates any later jump region and
///    avoids re-reading the whole oplog;
/// 3. a full recompute from the start of the oplog.
///
/// The checkpoint read happens only when the live cache baseline is absent or its fold is
/// impossible, so the common path pays no extra read.
///
/// Most callers should use [`calculate_last_known_status_with_checkpoint`], which sources the
/// checkpoint from `this.worker_service()`. This lower-level variant takes an explicit
/// `read_checkpoint` closure for the few callers that cannot satisfy [`HasWorkerService`] for `this`
/// — notably the `DefaultWorkerService` cold path, which reaches its own `read_status_checkpoint`
/// through `&self` without an owning `Arc`.
pub async fn calculate_last_known_status_with_checkpoint_reader<T, Fut>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    last_known: Option<AgentStatusRecord>,
    read_checkpoint: impl FnOnce() -> Fut,
) -> Result<Option<AgentStatusRecord>, StatusError>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
    Fut: std::future::Future<Output = Option<AgentStatusRecord>>,
{
    let horizon = this
        .oplog_service()
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    let reader = StatusOplogReader::new(this, owned_agent_id, agent_mode, None, horizon);
    calculate_status_with_reader(this, &reader, last_known, read_checkpoint).await
}

pub(in crate::worker) async fn calculate_status_with_reader<T, Fut>(
    this: &T,
    reader: &StatusOplogReader<'_>,
    last_known: Option<AgentStatusRecord>,
    read_checkpoint: impl FnOnce() -> Fut,
) -> Result<Option<AgentStatusRecord>, StatusError>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
    Fut: std::future::Future<Output = Option<AgentStatusRecord>>,
{
    // 1. Try folding forward from the live cached status.
    if let Some(last_known) = last_known
        && let Some(status) = try_fold_status_from_reader(this, reader, last_known).await?
    {
        crate::metrics::workers::record_agent_status_recompute("cache");
        return Ok(Some(status));
    }

    // 2. Live cache baseline missing or its fold was impossible (e.g. a jump deleted the cached
    //    index, or a revert moved the oplog behind it): try folding from the clean checkpoint.
    if let Some(checkpoint) = read_checkpoint().await
        && let Some(status) = try_fold_status_from_reader(this, reader, checkpoint).await?
    {
        crate::metrics::workers::record_agent_status_recompute("checkpoint");
        return Ok(Some(status));
    }

    // 3. Fall back to a full recompute from the start of the oplog.
    let status = try_fold_status_from_reader(this, reader, AgentStatusRecord::default()).await?;
    if status.is_some() {
        crate::metrics::workers::record_agent_status_recompute("full");
    }
    Ok(status)
}

/// Folds an acknowledged commit, returning receipt-gap detection and the fixed repair horizon.
#[allow(clippy::too_many_arguments)]
pub(in crate::worker) async fn fold_committed_status<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    oplog: &dyn Oplog,
    agent_mode: AgentMode,
    commit_level: CommitLevel,
    appended_through: OplogIndex,
    baseline: AgentStatusRecord,
    mut entries: BTreeMap<OplogIndex, OplogEntry>,
) -> (
    Result<Option<AgentStatusRecord>, StatusError>,
    bool,
    OplogIndex,
)
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    // Direct commits may return receipts already covered by an earlier catch-up.
    entries.retain(|index, _| *index > baseline.oplog_idx);
    let flushes_buffer = match commit_level {
        CommitLevel::ReportOnly => false,
        CommitLevel::DurableOnly => agent_mode != AgentMode::Ephemeral,
        CommitLevel::Always | CommitLevel::Deferred => true,
    };
    let horizon = baseline
        .oplog_idx
        .max(if flushes_buffer {
            appended_through
        } else {
            OplogIndex::NONE
        })
        .max(
            entries
                .last_key_value()
                .map(|(index, _)| *index)
                .unwrap_or(OplogIndex::NONE),
        );
    let contiguous = match (entries.first_key_value(), entries.last_key_value()) {
        (Some((first, _)), Some((last, _))) => {
            *first == baseline.oplog_idx.next()
                && last.as_u64() - first.as_u64() + 1 == entries.len() as u64
                && *last == horizon
        }
        _ => horizon == baseline.oplog_idx,
    };
    if contiguous {
        (
            update_status_with_new_entries(agent_mode, baseline, entries, &this.config().retry)
                .map_err(StatusError::from),
            false,
            horizon,
        )
    } else {
        // Opened-oplog reads include ephemeral handoffs. Never flush the newer buffered tail.
        let reader = StatusOplogReader::new(this, owned_agent_id, agent_mode, Some(oplog), horizon);
        (
            try_fold_status_from_reader(this, &reader, baseline).await,
            true,
            horizon,
        )
    }
}

/// Folds the oplog entries after `baseline.oplog_idx` onto `baseline`.
///
/// Returns `None` when the fold cannot produce a correct result from `baseline` alone:
/// - the oplog does not exist (no `Create` entry);
/// - `baseline` is ahead of the current last oplog index (e.g. a revert truncated the oplog below a
///   stale checkpoint), so there is nothing to fold forward;
/// - a newly added skipped/deleted region covers `baseline.oplog_idx` (see
///   [`update_status_with_new_entries`]), making the baseline unusable.
///
/// On `None` the caller should retry from an earlier baseline (a checkpoint, or the start of the
/// oplog). A `baseline` of [`AgentStatusRecord::default`] (oplog index 0) folds the whole oplog and
/// never hits the region case, so it always succeeds when the oplog exists.
pub async fn try_fold_status_from<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    baseline: AgentStatusRecord,
) -> Result<Option<AgentStatusRecord>, StatusError>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    let horizon = this
        .oplog_service()
        .get_last_index(owned_agent_id, agent_mode)
        .await?;
    let reader = StatusOplogReader::new(this, owned_agent_id, agent_mode, None, horizon);
    try_fold_status_from_reader(this, &reader, baseline).await
}

pub(in crate::worker) async fn try_fold_status_from_reader<T>(
    this: &T,
    reader: &StatusOplogReader<'_>,
    mut baseline: AgentStatusRecord,
) -> Result<Option<AgentStatusRecord>, StatusError>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    let owned_agent_id = reader.owned_agent_id;
    let agent_mode = reader.agent_mode;
    let full_rebuild = baseline.oplog_idx == OplogIndex::NONE;
    if full_rebuild && baseline.invocation_results.is_empty() {
        baseline.invocation_results = this.config().invocation_results.membership();
    }

    let last_oplog_index = reader.horizon;

    if last_oplog_index == OplogIndex::NONE {
        // Worker status can only be recovered if we have at least the Create oplog entry, otherwise
        // we cannot recover information like the component version.
        return Ok(None);
    }

    if last_oplog_index < baseline.oplog_idx {
        // The baseline is ahead of the oplog (e.g. a revert truncated it below a stale checkpoint);
        // we cannot fold forward, so the caller must retry from an earlier baseline.
        return Ok(None);
    }

    if baseline.oplog_idx == last_oplog_index {
        return Ok(Some(baseline));
    }

    let chunk_size = this
        .config()
        .invocation_results
        .physical_index_catch_up_chunk_size
        .max(1);

    if full_rebuild {
        return fold_status_with_precomputed_regions(this, reader, baseline, chunk_size).await;
    }

    let original_baseline = baseline.clone();
    let mut first = baseline.oplog_idx.next();
    while first <= last_oplog_index {
        let count = (last_oplog_index.as_u64() - first.as_u64() + 1).min(chunk_size);
        let mut entries = reader.read_exact(first, count).await?;
        if entries.is_empty() {
            return Ok(None);
        }
        let next = async {
            let deleted = deleted_regions(baseline.deleted_regions.clone(), &entries);
            hydrate_stream_session_payloads(
                this,
                owned_agent_id,
                agent_mode,
                &deleted,
                &mut entries,
            )
            .await?;
            hydrate_initial_pending_evidence(reader, &mut baseline, &entries).await?;
            let finalize_oplog_processor_checkpoints =
                entries.keys().next_back() == Some(&last_oplog_index);
            update_status_with_new_entries_internal(
                agent_mode,
                baseline,
                entries,
                &this.config().retry,
                true,
                finalize_oplog_processor_checkpoints,
            )
            .map_err(StatusError::from)
        }
        .await;
        baseline = match next {
            Ok(Some(status)) => status,
            Ok(None) | Err(_) => {
                // A later chunk may revert the entry that failed hydration or folding. Resolve
                // the complete deletion set before treating a retained-history error as fatal.
                return fold_status_with_precomputed_regions(
                    this,
                    reader,
                    original_baseline,
                    chunk_size,
                )
                .await;
            }
        };
        first = baseline.oplog_idx.next();
    }
    Ok(Some(baseline))
}

pub(super) async fn fold_status_with_precomputed_regions<T>(
    this: &T,
    reader: &StatusOplogReader<'_>,
    mut baseline: AgentStatusRecord,
    chunk_size: u64,
) -> Result<Option<AgentStatusRecord>, StatusError>
where
    T: HasOplogService + HasConfig + HasComponentService + Sync,
{
    let owned_agent_id = reader.owned_agent_id;
    let agent_mode = reader.agent_mode;
    let last_oplog_index = reader.horizon;
    let start = baseline.oplog_idx.next();
    let Some(region_entries) = read_region_entries(reader, start, chunk_size, true).await? else {
        return Ok(None);
    };
    let pending_updates = std::mem::take(&mut baseline.pending_updates);
    let mut regions = fold_regions_from(&baseline, pending_updates, &region_entries);

    if baseline_is_invalidated(&baseline, &regions.deleted, &regions.skipped) {
        if baseline.oplog_idx == OplogIndex::NONE {
            return Err(UnreadableOplog::new(
                &owned_agent_id.agent_id,
                OplogReadError::corruption(
                    OplogReadSource::Other("status regions"),
                    "oplog regions invalidate the empty status baseline",
                ),
            )
            .into());
        }
        return Ok(None);
    }
    baseline.deleted_regions = regions.deleted.clone();
    baseline.skipped_regions = regions.skipped.clone();
    // The queue after the whole range belongs to the status after the last chunk. A status
    // between two chunks has no pending updates; nothing reads them.
    let mut final_pending_updates = Some(std::mem::take(&mut regions.queue).into_open().0);

    let mut first = start;
    while first <= last_oplog_index {
        let count = (last_oplog_index.as_u64() - first.as_u64() + 1).min(chunk_size);
        let mut entries = reader.read_exact(first, count).await?;
        if entries.is_empty() {
            return Ok(None);
        }
        hydrate_stream_session_payloads(
            this,
            owned_agent_id,
            agent_mode,
            &baseline.deleted_regions,
            &mut entries,
        )
        .await?;
        hydrate_initial_pending_evidence(reader, &mut baseline, &entries).await?;
        let finalize_oplog_processor_checkpoints =
            entries.keys().next_back() == Some(&last_oplog_index);
        let pending_updates = if finalize_oplog_processor_checkpoints {
            final_pending_updates.take().unwrap_or_default()
        } else {
            std::mem::take(&mut baseline.pending_updates)
        };
        baseline = update_status_with_precomputed_regions(
            agent_mode,
            baseline,
            entries,
            &this.config().retry,
            FoldedRegions {
                deleted: regions.deleted.clone(),
                skipped: regions.skipped.clone(),
                steps: &regions.steps,
            },
            pending_updates,
            finalize_oplog_processor_checkpoints,
        )?;
        first = baseline.oplog_idx.next();
    }
    Ok(Some(baseline))
}

pub(super) async fn hydrate_stream_session_payloads<T>(
    this: &T,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    deleted_regions: &DeletedRegions,
    entries: &mut BTreeMap<OplogIndex, OplogEntry>,
) -> Result<(), StatusError>
where
    T: HasOplogService + Sync,
{
    for (index, entry) in entries.iter_mut() {
        if deleted_regions.is_in_deleted_region(*index) {
            continue;
        }
        if let OplogEntry::StreamSession { record, .. } = entry {
            let decoded = this
                .oplog_service()
                .download_payload(owned_agent_id, agent_mode, record.clone())
                .await
                .map_err(|error| {
                    let source = OplogReadSource::Other("stream session payload");
                    let error = match error {
                        OplogPayloadDownloadError::Backend(error) => {
                            OplogReadError::source_unavailable(
                                source,
                                format!("failed to load payload at oplog index {index}: {error:#}"),
                            )
                        }
                        OplogPayloadDownloadError::Corrupt(error) => OplogReadError::corruption(
                            source,
                            format!("failed to load payload at oplog index {index}: {error:#}"),
                        ),
                    };
                    UnreadableOplog::new(&owned_agent_id.agent_id, error)
                })?;
            *record = OplogPayload::Inline(Box::new(decoded));
        }
    }
    Ok(())
}

pub(super) async fn hydrate_initial_pending_evidence(
    reader: &StatusOplogReader<'_>,
    baseline: &mut AgentStatusRecord,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<(), StatusError> {
    let deleted = deleted_regions(baseline.deleted_regions.clone(), entries);
    for (attached_idx, entry) in entries {
        if deleted.is_in_deleted_region(*attached_idx) {
            continue;
        }
        let OplogEntry::StreamSession {
            record: OplogPayload::Inline(record),
            ..
        } = entry
        else {
            continue;
        };
        let StreamSessionRecord::Attached(attached) = record.as_ref() else {
            continue;
        };
        let Some(status) = baseline
            .durable_stream_sessions
            .get(&attached.session_key)
            .cloned()
        else {
            continue;
        };
        if status.lifecycle_error.is_some() || status.validated_initial_pending_invocation.is_some()
        {
            continue;
        }
        let mut status = status;
        if !status.validate_initial_attachment_reference(*attached_idx, attached) {
            baseline
                .durable_stream_sessions
                .insert(attached.session_key.clone(), status);
            continue;
        }
        let persisted_referent;
        let referent = if let Some(entry) = entries.get(&attached.pending_invocation_oplog_index) {
            Some(entry)
        } else {
            persisted_referent = reader
                .read_exact(attached.pending_invocation_oplog_index, 1)
                .await?;
            persisted_referent.get(&attached.pending_invocation_oplog_index)
        };
        match referent {
            Some(OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            }) => {
                status.apply_pending_invocation(
                    attached.pending_invocation_oplog_index,
                    idempotency_key,
                );
                baseline
                    .durable_stream_sessions
                    .insert(attached.session_key.clone(), status);
            }
            _ => {
                status.lifecycle_error = Some(
                    "durable Attached record references a missing or invalid pending invocation"
                        .into(),
                );
                baseline
                    .durable_stream_sessions
                    .insert(attached.session_key.clone(), status);
            }
        }
    }
    Ok(())
}
/// Resolve replay exclusions from a fixed committed horizon, independent of later reverts.
pub(crate) async fn skipped_regions_at(
    this: &(impl HasOplogService + Sync),
    owned_agent_id: &OwnedAgentId,
    horizon: OplogIndex,
) -> Result<DeletedRegions, StatusError> {
    let reader = StatusOplogReader::new(this, owned_agent_id, AgentMode::Durable, None, horizon);
    // A manual update invocation changes only the manual admissions, which no skipped region
    // depends on, so the read does not decode the invocation payloads.
    let entries = read_region_entries(&reader, OplogIndex::INITIAL, 1024, false)
        .await?
        .ok_or("Missing fork source history")?;
    Ok(skipped_regions(&entries))
}

/// The entries from `first` to the horizon of `reader` that the region fold reads: the jumps,
/// the reverts and the update entries, and the manual update invocations when
/// `manual_admissions` is true.
pub(super) async fn read_region_entries(
    reader: &StatusOplogReader<'_>,
    mut first: OplogIndex,
    chunk_size: u64,
    manual_admissions: bool,
) -> Result<Option<BTreeMap<OplogIndex, OplogEntry>>, UnreadableOplog> {
    let mut regions = BTreeMap::new();
    let horizon = reader.horizon;
    while first <= horizon {
        let count = (horizon.as_u64() - first.as_u64() + 1).min(chunk_size);
        let entries = reader.read_exact(first, count).await?;
        let Some(last) = entries.keys().next_back() else {
            return Ok(None);
        };
        first = last.next();
        regions.extend(entries.into_iter().filter(|(_, entry)| {
            matches!(
                entry,
                OplogEntry::Jump { .. }
                    | OplogEntry::Revert { .. }
                    | OplogEntry::PendingUpdate { .. }
                    | OplogEntry::SuccessfulUpdate { .. }
                    | OplogEntry::FailedUpdate { .. }
            ) || manual_admissions
                && matches!(
                    entry,
                    OplogEntry::PendingAgentInvocation { payload, .. }
                        if manual_update_target_revision_of(payload).is_some()
                )
        }));
    }
    Ok(Some(regions))
}
