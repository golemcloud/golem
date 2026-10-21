use crate::services::oplog::{
    CommitLevel, Oplog, OplogPayloadDownloadError, OplogReadError, OplogReadSource, OplogService,
    OplogServiceOps, UnreadableOplog,
};
use crate::services::{HasComponentService, HasConfig, HasOplogService, HasWorkerService};
use golem_common::base_model::OplogIndex;
use golem_common::base_model::durable_stream::StreamSessionRecord;
use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{
    AgentError, AgentResourceId, OplogEntry, OplogErrorKind, OplogPayload, QueuedCardEvent,
};
use golem_common::model::regions::DeletedRegions;
use golem_common::model::{
    AgentFingerprint, AgentResourceDescription, AgentStatus, AgentStatusRecord, AssistedSelection,
    AuthoritativeSnapshot, AuthoritativeSnapshotKind, AutomaticSnapshot, DurableStreamSessionIndex,
    ExportForkAdmissions, FailedUpdateRecord, IdempotencyKey, InvocationResultMembership,
    OplogProcessorCheckpointState, OwnedAgentId, PendingCardEventRef, PendingInvocationRef,
    PendingUpdateKind, PendingUpdateRef, ReceivedCardTransferIndex, ReceivedCardTransferState,
    RetryConfig, RetryPolicyState, SnapshotFiles, SuccessfulUpdateRecord, UsableAutomaticSnapshot,
};
use golem_common::serialization::try_deserialize;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

pub(crate) mod regions;
pub(crate) mod update_queue;

pub(crate) use regions::{
    RegionFold, deleted_regions, fold_regions_from, revert_validation_regions, skipped_regions,
};
use update_queue::{UpdateStep, manual_update_target_revision_of};

use execution::calculate_latest_worker_status;
use fields::calculate_active_plugins;
use fields::calculate_durable_stream_sessions;
use fields::calculate_export_fork_admissions;
use fields::calculate_invocation_results;
use fields::calculate_oplog_processor_checkpoints;
pub(crate) use fields::calculate_pending_card_events;
use fields::calculate_pending_invocations;
use fields::calculate_received_card_transfers;
use fields::calculate_revoked_cards;
use fields::calculate_total_linear_memory_size;
use fields::calculate_update_fields;
use fields::collect_resources;
pub(crate) use fields::fold_invocation_result_entries;
pub use reconstruction::calculate_last_known_status;
pub use reconstruction::calculate_last_known_status_for_existing_worker;
pub use reconstruction::calculate_last_known_status_with_checkpoint;
pub use reconstruction::calculate_last_known_status_with_checkpoint_reader;
pub(super) use reconstruction::calculate_status_with_reader;
pub(super) use reconstruction::fold_committed_status;
#[cfg(test)]
use reconstruction::hydrate_initial_pending_evidence;
pub(crate) use reconstruction::skipped_regions_at;
pub use reconstruction::try_fold_status_from;
#[cfg(test)]
use reconstruction::try_fold_status_from_reader;

mod execution;
mod fields;
mod reconstruction;

/// Why the status of an agent could not be folded from its oplog.
#[derive(Clone, Debug)]
pub enum StatusError {
    /// The oplog of the agent does not read, so the agent cannot be recovered from it.
    Unreadable(UnreadableOplog),
    /// Any other failure, as text.
    Other(String),
}

impl Display for StatusError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(error) => write!(f, "{error}"),
            Self::Other(error) => f.write_str(error),
        }
    }
}

impl From<UnreadableOplog> for StatusError {
    fn from(error: UnreadableOplog) -> Self {
        Self::Unreadable(error)
    }
}

impl From<String> for StatusError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

impl From<&str> for StatusError {
    fn from(error: &str) -> Self {
        Self::Other(error.to_string())
    }
}

/// An unreadable oplog keeps its type, so the agent fails with it; any other failure is a
/// runtime error.
impl From<StatusError> for WorkerExecutorError {
    fn from(error: StatusError) -> Self {
        match error {
            StatusError::Unreadable(error) => error.into(),
            StatusError::Other(error) => WorkerExecutorError::runtime(error),
        }
    }
}

/// One immutable projection boundary, shared by region discovery and every baseline attempt.
pub(super) struct StatusOplogReader<'a> {
    service: Arc<dyn OplogService>,
    opened: Option<&'a dyn Oplog>,
    owned_agent_id: &'a OwnedAgentId,
    agent_mode: AgentMode,
    horizon: OplogIndex,
}

// update a worker status with new entries. Returns None if the status cannot be calculated from the new entries alone and needs to be recalculated from the beginning.
pub fn update_status_with_new_entries(
    agent_mode: AgentMode,
    last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    // TODO: changing the retry policy will cause inconsistencies when reading existing oplogs.
    default_retry_policy: &RetryConfig,
) -> Result<Option<AgentStatusRecord>, String> {
    update_status_with_new_entries_internal(
        agent_mode,
        last_known,
        new_entries,
        default_retry_policy,
        true,
        true,
    )
}

fn update_status_with_new_entries_internal(
    agent_mode: AgentMode,
    mut last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    default_retry_policy: &RetryConfig,
    validate_baseline: bool,
    finalize_oplog_processor_checkpoints: bool,
) -> Result<Option<AgentStatusRecord>, String> {
    let pending_updates = std::mem::take(&mut last_known.pending_updates);
    let RegionFold {
        deleted,
        skipped,
        queue,
        steps,
    } = fold_regions_from(&last_known, pending_updates, &new_entries);

    // If the last known status is from a deleted region based on the latest deleted region status,
    // we cannot fold the new status from the new entries only, and need to recalculate the whole status
    // (Note that this is a rare case - for Jumps, this is not happening if the executor successfully writes out
    // the new status before performing the jump; for Reverts, the status is recalculated anyway, but only once, when
    // the revert is applied)
    if validate_baseline && baseline_is_invalidated(&last_known, &deleted, &skipped) {
        return Ok(None);
    }

    let pending_updates = queue.into_open().0;
    Ok(Some(update_status_with_precomputed_regions(
        agent_mode,
        last_known,
        new_entries,
        default_retry_policy,
        FoldedRegions {
            deleted,
            skipped,
            steps: &steps,
        },
        pending_updates,
        finalize_oplog_processor_checkpoints,
    )?))
}

fn baseline_is_invalidated(
    baseline: &AgentStatusRecord,
    deleted_regions: &DeletedRegions,
    skipped_regions: &DeletedRegions,
) -> bool {
    // A baseline taken inside a region a later revert deleted still carries folded state from
    // entries that no longer exist, even when the baseline's own skipped coverage hides that
    // region in the comparison below.
    if deleted_regions.is_in_deleted_region(baseline.oplog_idx) {
        return true;
    }

    let baseline_without_overrides = if baseline.skipped_regions.is_overridden() {
        let mut cloned = baseline.skipped_regions.clone();
        cloned.merge_override();
        cloned
    } else {
        baseline.skipped_regions.clone()
    };
    let new_without_overrides = if skipped_regions.is_overridden() {
        let mut cloned = skipped_regions.clone();
        cloned.merge_override();
        cloned
    } else {
        skipped_regions.clone()
    };
    // Removing a temporary snapshot override exposes prefix entries absent from the baseline.
    // Both newly hidden and newly visible prefix entries require a different baseline.
    let has_uncovered_prefix = |regions: &DeletedRegions, other: &DeletedRegions| {
        regions.regions().any(|region| {
            if region.start > baseline.oplog_idx {
                return false;
            }
            let relevant_end = region.end.min(baseline.oplog_idx);
            !other.regions().any(|other_region| {
                other_region.start <= region.start && other_region.end >= relevant_end
            })
        })
    };
    new_without_overrides != baseline_without_overrides
        && (has_uncovered_prefix(&new_without_overrides, &baseline_without_overrides)
            || has_uncovered_prefix(&baseline_without_overrides, &new_without_overrides))
}

/// The regions and the update steps of a region fold, which the status after the folded range
/// takes: it moves the regions in.
struct FoldedRegions<'a> {
    deleted: DeletedRegions,
    skipped: DeletedRegions,
    steps: &'a BTreeMap<OplogIndex, UpdateStep>,
}

/// The status after `new_entries`, with the regions and the update steps of `regions`, which a
/// region fold gave for a range that holds `new_entries`. `pending_updates` are the pending
/// updates of the status after `new_entries`; the caller takes them from the queue of the region
/// fold.
fn update_status_with_precomputed_regions(
    agent_mode: AgentMode,
    last_known: AgentStatusRecord,
    new_entries: BTreeMap<OplogIndex, OplogEntry>,
    default_retry_policy: &RetryConfig,
    regions: FoldedRegions<'_>,
    pending_updates: VecDeque<PendingUpdateRef>,
    finalize_oplog_processor_checkpoints: bool,
) -> Result<AgentStatusRecord, String> {
    let FoldedRegions {
        deleted,
        skipped,
        steps,
    } = regions;
    let deleted_regions = &deleted;
    let skipped_regions = &skipped;
    let active_plugins = last_known.active_plugins.clone();
    let (status, last_error_kind, current_retry_state, overridden_retry_config) =
        calculate_latest_worker_status(
            last_known.status,
            last_known.last_error_kind,
            last_known.current_retry_state,
            last_known.overridden_retry_config,
            default_retry_policy,
            skipped_regions,
            deleted_regions,
            &new_entries,
        );

    let pending_invocations =
        calculate_pending_invocations(last_known.pending_invocations, steps, &new_entries);
    let pending_card_events =
        calculate_pending_card_events(last_known.pending_card_events, &new_entries);
    let received_card_transfers =
        calculate_received_card_transfers(last_known.received_card_transfers, &new_entries);
    let durable_stream_sessions = calculate_durable_stream_sessions(
        last_known.durable_stream_sessions,
        deleted_regions,
        &new_entries,
    )?;
    let export_fork_admissions = calculate_export_fork_admissions(
        last_known.export_fork_admissions,
        deleted_regions,
        &new_entries,
    )?;
    let mut pending_durable_stream_cancellations = last_known.pending_durable_stream_cancellations;
    for (index, entry) in &new_entries {
        if deleted_regions.is_in_deleted_region(*index) {
            continue;
        }
        if let OplogEntry::StreamSession {
            record: OplogPayload::Inline(record),
            ..
        } = entry
        {
            match record.as_ref() {
                StreamSessionRecord::ConsumerCancelIntent(intent) => {
                    if !pending_durable_stream_cancellations.iter().any(|existing| {
                        existing.session_key == intent.session_key
                            && existing.source == intent.source
                    }) {
                        pending_durable_stream_cancellations.insert(intent.clone());
                    }
                }
                StreamSessionRecord::ConsumerCancelApplied(receipt) => {
                    pending_durable_stream_cancellations.remove(&receipt.intent);
                }
                _ => {}
            }
        }
    }
    let has_durable_stream_history = last_known.has_durable_stream_history
        || new_entries.values().any(|entry| {
            matches!(
                entry,
                OplogEntry::StreamRegistered { .. }
                    | OplogEntry::StreamItems { .. }
                    | OplogEntry::StreamEnd { .. }
                    | OplogEntry::StreamCancel { .. }
                    | OplogEntry::StreamSession { .. }
            )
        });
    let UpdateFields {
        failed_updates,
        successful_updates,
        component_revision,
        component_size,
        component_revision_for_replay,
        component_revision_start_index,
        authoritative_snapshot,
        last_automatic_snapshot,
        previous_usable_automatic_snapshot,
    } = calculate_update_fields(
        UpdateFields {
            failed_updates: last_known.failed_updates,
            successful_updates: last_known.successful_updates,
            component_revision: last_known.component_revision,
            component_size: last_known.component_size,
            component_revision_for_replay: last_known.component_revision_for_replay,
            component_revision_start_index: last_known.component_revision_start_index,
            authoritative_snapshot: last_known.authoritative_snapshot,
            last_automatic_snapshot: last_known.last_automatic_snapshot,
            previous_usable_automatic_snapshot: last_known.previous_usable_automatic_snapshot,
        },
        deleted_regions,
        steps,
        &new_entries,
    )?;

    let (invocation_results, current_idempotency_key, cancelled_idempotency_key) =
        calculate_invocation_results(
            last_known.invocation_results,
            last_known.current_idempotency_key,
            last_known.cancelled_idempotency_key,
            deleted_regions,
            &new_entries,
        );

    let total_linear_memory_size = calculate_total_linear_memory_size(
        last_known.total_linear_memory_size,
        skipped_regions,
        &new_entries,
    );

    let owned_resources =
        collect_resources(last_known.owned_resources, skipped_regions, &new_entries);

    let active_plugins = calculate_active_plugins(active_plugins, deleted_regions, &new_entries);

    let revoked_cards = calculate_revoked_cards(last_known.revoked_cards, &new_entries);

    let oplog_processor_checkpoints = calculate_oplog_processor_checkpoints(
        last_known.oplog_processor_checkpoints,
        &active_plugins,
        deleted_regions,
        &new_entries,
        finalize_oplog_processor_checkpoints,
    );

    Ok(AgentStatusRecord {
        oplog_idx: new_entries
            .keys()
            .max()
            .cloned()
            .unwrap_or(last_known.oplog_idx),
        status,
        last_error_kind,
        overridden_retry_config,
        pending_invocations,
        pending_card_events,
        skipped_regions: skipped,
        pending_updates,
        failed_updates,
        successful_updates,
        invocation_results,
        received_card_transfers,
        durable_stream_sessions,
        export_fork_admissions,
        has_durable_stream_history,
        pending_durable_stream_cancellations,
        current_idempotency_key,
        cancelled_idempotency_key,
        component_revision,
        component_size,
        owned_resources,
        total_linear_memory_size,
        active_plugins,
        oplog_processor_checkpoints,
        revoked_cards,
        deleted_regions: deleted,
        component_revision_for_replay,
        component_revision_start_index,
        current_retry_state,
        authoritative_snapshot,
        last_automatic_snapshot,
        previous_usable_automatic_snapshot,
        agent_mode,
    })
}

/// The fields of the status that the component updates and the automatic snapshot entries decide.
#[derive(Debug)]
struct UpdateFields {
    failed_updates: Vec<FailedUpdateRecord>,
    successful_updates: Vec<SuccessfulUpdateRecord>,
    component_revision: ComponentRevision,
    component_size: u64,
    component_revision_for_replay: ComponentRevision,
    component_revision_start_index: OplogIndex,
    authoritative_snapshot: Option<AuthoritativeSnapshot>,
    last_automatic_snapshot: Option<AutomaticSnapshot>,
    previous_usable_automatic_snapshot: Option<UsableAutomaticSnapshot>,
}

#[cfg(test)]
#[path = "tests.rs"]
mod test;
