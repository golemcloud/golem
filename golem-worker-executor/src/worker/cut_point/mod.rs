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

//! Validation of oplog *cut points*.
//!
//! External fork and revert both split the oplog at a caller-chosen index (the *cut point*):
//! entries at or before the cut survive, everything after it is dropped (revert) or never copied
//! (fork). A retained `Start` without its terminal uses ordinary incomplete-call recovery;
//! a retained accessor `End` without its completion marker is delivered at the replay tail.
//! Outcomes beyond the cut do not constrain these calls. Remote transactions still require a
//! cut that preserves their committed/rolled-back outcome.

use crate::worker::status::update_queue::UpdateQueue;
use golem_common::base_model::OplogIndex;
use golem_common::model::PendingUpdateKind;
use golem_common::model::oplog::{OplogEntry, OplogIndexRange};
use golem_common::model::regions::DeletedRegions;
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevertUpdateBoundaryError {
    SplitSnapshotUpdate {
        pending_index: OplogIndex,
        outcome_index: OplogIndex,
    },
}

impl Display for RevertUpdateBoundaryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RevertUpdateBoundaryError::SplitSnapshotUpdate {
                pending_index,
                outcome_index,
            } => write!(
                f,
                "the cut point retains a snapshot-based PendingUpdate at oplog index {pending_index} but removes its outcome at oplog index {outcome_index}"
            ),
        }
    }
}

/// Validates snapshot-update boundaries. Update outcomes are paired with pending updates by the
/// update queue of the status fold. A cut may remove a snapshot-based update entirely or retain
/// it entirely, including a pending update that does not have an outcome yet. A cut may split a
/// snapshot-assisted automatic update, or a manual update invocation from its outcome when no
/// `PendingUpdate` paired them, because no snapshot baseline exists before its outcome.
pub fn validate_snapshot_update_boundaries(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    cut_point: OplogIndex,
    deleted_regions: &DeletedRegions,
) -> Result<(), RevertUpdateBoundaryError> {
    // A manual update invocation changes only the manual admissions, and no queue element ends
    // through one, so the check does not decode the invocation payloads.
    entries
        .iter()
        .filter(|(_, entry)| !matches!(entry, OplogEntry::PendingAgentInvocation { .. }))
        .try_fold(UpdateQueue::default(), |queue, (idx, entry)| {
            let (queue, step) =
                queue.after(*idx, entry, deleted_regions.is_in_deleted_region(*idx));
            match step.ended() {
                Some(paired)
                    if matches!(paired.kind, PendingUpdateKind::SnapshotBased { .. })
                        && paired.oplog_index <= cut_point
                        && cut_point < *idx =>
                {
                    Err(RevertUpdateBoundaryError::SplitSnapshotUpdate {
                        pending_index: paired.oplog_index,
                        outcome_index: *idx,
                    })
                }
                _ => Ok(queue),
            }
        })
        .map(|_| ())
}

/// A paired durable construct whose two halves lie on opposite sides of a cut point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanningConstruct {
    /// A remote transaction begun at or before the cut with a pre-commit/pre-rollback,
    /// committed/rolled-back, or retried-begin entry after it.
    RemoteTransaction {
        begin_index: OplogIndex,
        reference_index: OplogIndex,
    },
}

impl Display for SpanningConstruct {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SpanningConstruct::RemoteTransaction {
                begin_index,
                reference_index,
            } => write!(
                f,
                "an open remote transaction (begun at oplog index {begin_index}, referenced at {reference_index})"
            ),
        }
    }
}

/// Scans `(cut_point, scan_end]` for a remote-transaction outcome that
/// references an opening entry at or before the cut. Ordinary call terminals and completion
/// markers may lie beyond the cut: replay recovers from precisely the retained prefix.
///
/// Entries inside `skipped_regions` are ignored on both sides: a terminal inside an
/// already-deleted region is dead, and a terminal whose referenced opening entry lies in a
/// deleted region is an orphan that replay drains without effect. A failed `read` ends the scan
/// with its error.
pub async fn find_construct_spanning_cut_point<Read, ReadFut, E>(
    read: Read,
    cut_point: OplogIndex,
    scan_end: OplogIndex,
    skipped_regions: &DeletedRegions,
) -> Result<Option<SpanningConstruct>, E>
where
    Read: Fn(OplogIndex) -> ReadFut,
    ReadFut: Future<Output = Result<OplogEntry, E>>,
{
    for idx in OplogIndexRange::new(cut_point.next(), scan_end) {
        if skipped_regions.is_in_deleted_region(idx) {
            continue;
        }
        let entry = read(idx).await?;
        let spanning = match &entry {
            OplogEntry::PreCommitRemoteTransaction { begin_index, .. }
            | OplogEntry::PreRollbackRemoteTransaction { begin_index, .. }
            | OplogEntry::CommittedRemoteTransaction { begin_index, .. }
            | OplogEntry::RolledBackRemoteTransaction { begin_index, .. } => Some((
                *begin_index,
                SpanningConstruct::RemoteTransaction {
                    begin_index: *begin_index,
                    reference_index: idx,
                },
            )),
            OplogEntry::BeginRemoteTransaction {
                original_begin_index,
                ..
            } => Some((
                *original_begin_index,
                SpanningConstruct::RemoteTransaction {
                    begin_index: *original_begin_index,
                    reference_index: idx,
                },
            )),
            // Keep this exhaustive: every new entry that can reference an earlier opening entry
            // must be considered before it is allowed across a fork/snapshot cut point.
            OplogEntry::Create { .. }
            | OplogEntry::Start { .. }
            | OplogEntry::End { .. }
            | OplogEntry::Cancelled { .. }
            | OplogEntry::CompletionDiscarded { .. }
            | OplogEntry::CompletionDelivered { .. }
            | OplogEntry::AgentInvocationStarted { .. }
            | OplogEntry::AgentInvocationFinished { .. }
            | OplogEntry::Suspend { .. }
            | OplogEntry::Error { .. }
            | OplogEntry::RecoverySucceeded { .. }
            | OplogEntry::Jump { .. }
            | OplogEntry::Interrupted { .. }
            | OplogEntry::Exited { .. }
            | OplogEntry::PendingAgentInvocation { .. }
            | OplogEntry::PendingUpdate { .. }
            | OplogEntry::SuccessfulUpdate { .. }
            | OplogEntry::FailedUpdate { .. }
            | OplogEntry::GrowMemory { .. }
            | OplogEntry::CreateResource { .. }
            | OplogEntry::DropResource { .. }
            | OplogEntry::Log { .. }
            | OplogEntry::Restart { .. }
            | OplogEntry::Resumed { .. }
            | OplogEntry::ActivatePlugin { .. }
            | OplogEntry::DeactivatePlugin { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::CancelPendingInvocation { .. }
            | OplogEntry::Snapshot { .. }
            | OplogEntry::SnapshotConfirmed { .. }
            | OplogEntry::SpanClosed { .. }
            | OplogEntry::OplogProcessorCheckpoint { .. }
            | OplogEntry::SetRetryPolicy { .. }
            | OplogEntry::RemoveRetryPolicy { .. }
            | OplogEntry::CardEventQueued { .. }
            | OplogEntry::CardInstalled { .. }
            | OplogEntry::CardInstallFailed { .. }
            | OplogEntry::CardRevoked { .. }
            | OplogEntry::CardExpired { .. }
            | OplogEntry::CardDerived { .. }
            | OplogEntry::CardTransferStarted { .. }
            | OplogEntry::CardTransferred { .. }
            | OplogEntry::CardRevokedCascade { .. }
            | OplogEntry::CardTransferConfirmed { .. }
            | OplogEntry::HostStreamFrame { .. }
            | OplogEntry::ToolAttachmentControl { .. }
            | OplogEntry::ToolAttachmentRead { .. }
            | OplogEntry::StreamRegistered { .. }
            | OplogEntry::StreamItems { .. }
            | OplogEntry::StreamEnd { .. }
            | OplogEntry::StreamCancel { .. }
            | OplogEntry::StreamSession { .. } => None,
        };

        if let Some((opening_index, construct)) = spanning
            && opening_index <= cut_point
            && !skipped_regions.is_in_deleted_region(opening_index)
        {
            return Ok(Some(construct));
        }
    }
    Ok(None)
}

/// A streaming acceptance owns its input registrations, preparation, queued invocation and
/// attachment together. Retaining only part would leave an unstartable invocation or orphan inputs.
pub(crate) async fn streaming_acceptance_spans_cut(
    oplog: &dyn crate::services::oplog::Oplog,
    cut: OplogIndex,
    horizon: OplogIndex,
) -> Result<bool, String> {
    use crate::services::oplog::OplogOps;
    use golem_common::model::durable_stream::StreamSessionRecord;

    if cut >= horizon {
        return Ok(false);
    }
    let left = oplog.read(cut).await.map_err(|error| error.to_string())?;
    let right = oplog
        .read(cut.next())
        .await
        .map_err(|error| error.to_string())?;
    match (left, right) {
        (OplogEntry::StreamRegistered { .. }, _) => {
            for index in OplogIndexRange::new(cut.next(), horizon) {
                match oplog.read(index).await.map_err(|error| error.to_string())? {
                    OplogEntry::StreamRegistered { .. } => continue,
                    OplogEntry::StreamSession { record, .. } => {
                        return Ok(matches!(oplog.download_payload(record).await?,
                        StreamSessionRecord::Prepared(prepared)
                            if prepared.stream_mappings.iter().any(|mapping|
                                matches!(mapping.source,
                                    golem_common::model::durable_stream::StreamRecordReference::Local(id)
                                        if id.0 == cut))));
                    }
                    _ => return Ok(false),
                }
            }
            Ok(false)
        }
        (
            OplogEntry::StreamSession { record, .. },
            OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            },
        ) => Ok(matches!(oplog.download_payload(record).await?,
                StreamSessionRecord::Prepared(prepared) if prepared.attempt.session_key.idempotency_key == idempotency_key)),
        (
            OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            },
            OplogEntry::StreamSession { record, .. },
        ) => Ok(matches!(oplog.download_payload(record).await?,
                StreamSessionRecord::Attached(attached) if attached.pending_invocation_oplog_index == cut
                    && attached.session_key == idempotency_key)),
        (
            OplogEntry::StreamSession { record: left, .. },
            OplogEntry::StreamSession { record: right, .. },
        ) => {
            let StreamSessionRecord::TopologyPrepared(right) =
                oplog.download_payload(right).await?
            else {
                return Ok(false);
            };
            Ok(match oplog.download_payload(left).await? {
                StreamSessionRecord::Attached(left) => {
                    left.session_key == right.session_key.idempotency_key
                }
                StreamSessionRecord::TopologyPrepared(left) => {
                    left.session_key == right.session_key
                }
                _ => false,
            })
        }
        _ => Ok(false),
    }
}

/// Returns the first durable stream entry in an inclusive raw oplog range.
/// Revert uses this to distinguish bare reverts from paired stream-lineage transitions.
pub async fn find_stream_history_in_range<Read, ReadFut>(
    read: Read,
    start: OplogIndex,
    end: OplogIndex,
) -> Option<OplogIndex>
where
    Read: Fn(OplogIndex) -> ReadFut,
    ReadFut: Future<Output = OplogEntry>,
{
    for idx in OplogIndexRange::new(start, end) {
        if matches!(
            read(idx).await,
            OplogEntry::StreamRegistered { .. }
                | OplogEntry::StreamItems { .. }
                | OplogEntry::StreamEnd { .. }
                | OplogEntry::StreamCancel { .. }
                | OplogEntry::StreamSession { .. }
        ) {
            return Some(idx);
        }
    }
    None
}

#[cfg(test)]
mod tests;
