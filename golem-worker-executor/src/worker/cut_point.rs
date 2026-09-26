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
//! Outcomes beyond the cut do not constrain these calls. Atomic regions and remote transactions
//! still require a cut that preserves their committed/rolled-back outcome.

use golem_common::base_model::OplogIndex;
use golem_common::model::oplog::{OplogEntry, OplogIndexRange, UpdateDescription};
use golem_common::model::regions::DeletedRegions;
use std::collections::{BTreeMap, VecDeque};
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

/// Validates snapshot-update boundaries. Update outcomes are paired with pending updates in oplog
/// order, matching the status reducer. A cut may remove an update entirely or retain it entirely,
/// including a pending update that does not have an outcome yet.
pub fn validate_snapshot_update_boundaries(
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    cut_point: OplogIndex,
    deleted_regions: &DeletedRegions,
) -> Result<(), RevertUpdateBoundaryError> {
    let mut pending_updates = VecDeque::new();

    for (idx, entry) in entries {
        if deleted_regions.is_in_deleted_region(*idx) {
            continue;
        }

        match entry {
            OplogEntry::PendingUpdate { description, .. } => pending_updates.push_back((
                *idx,
                matches!(description, UpdateDescription::SnapshotBased { .. }),
            )),
            OplogEntry::SuccessfulUpdate { .. } => {
                if let Some((pending_index, true)) = pending_updates.pop_front()
                    && pending_index <= cut_point
                    && cut_point < *idx
                {
                    return Err(RevertUpdateBoundaryError::SplitSnapshotUpdate {
                        pending_index,
                        outcome_index: *idx,
                    });
                }
            }
            OplogEntry::FailedUpdate { .. } => {
                if let Some((pending_index, true)) = pending_updates.pop_front()
                    && pending_index <= cut_point
                    && cut_point < *idx
                {
                    return Err(RevertUpdateBoundaryError::SplitSnapshotUpdate {
                        pending_index,
                        outcome_index: *idx,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// A paired durable construct whose two halves lie on opposite sides of a cut point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanningConstruct {
    /// An atomic region begun at or before the cut whose `EndAtomicRegion` lies after it.
    AtomicRegion {
        begin_index: OplogIndex,
        end_index: OplogIndex,
    },
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
            SpanningConstruct::AtomicRegion {
                begin_index,
                end_index,
            } => write!(
                f,
                "an open atomic region (begun at oplog index {begin_index}, ended at {end_index})"
            ),
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

/// Scans `(cut_point, scan_end]` for an atomic-region or remote-transaction outcome that
/// references an opening entry at or before the cut. Ordinary call terminals and completion
/// markers may lie beyond the cut: replay recovers from precisely the retained prefix.
///
/// Entries inside `skipped_regions` are ignored on both sides: a terminal inside an
/// already-deleted region is dead, and a terminal whose referenced opening entry lies in a
/// deleted region is an orphan that replay drains without effect.
pub async fn find_construct_spanning_cut_point<Read, ReadFut>(
    read: Read,
    cut_point: OplogIndex,
    scan_end: OplogIndex,
    skipped_regions: &DeletedRegions,
) -> Option<SpanningConstruct>
where
    Read: Fn(OplogIndex) -> ReadFut,
    ReadFut: Future<Output = OplogEntry>,
{
    for idx in OplogIndexRange::new(cut_point.next(), scan_end) {
        if skipped_regions.is_in_deleted_region(idx) {
            continue;
        }
        let entry = read(idx).await;
        let spanning = match &entry {
            OplogEntry::EndAtomicRegion { begin_index, .. } => Some((
                *begin_index,
                SpanningConstruct::AtomicRegion {
                    begin_index: *begin_index,
                    end_index: idx,
                },
            )),
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
                original_begin_index: Some(original_begin_index),
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
            | OplogEntry::NoOp { .. }
            | OplogEntry::Jump { .. }
            | OplogEntry::Interrupted { .. }
            | OplogEntry::Exited { .. }
            | OplogEntry::BeginAtomicRegion { .. }
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
            | OplogEntry::BeginRemoteTransaction {
                original_begin_index: None,
                ..
            }
            | OplogEntry::Snapshot { .. }
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
            return Some(construct);
        }
    }
    None
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
    let left = oplog.read(cut).await;
    let right = oplog.read(cut.next()).await;
    match (left, right) {
        (OplogEntry::StreamRegistered { .. }, _) => {
            for index in OplogIndexRange::new(cut.next(), horizon) {
                match oplog.read(index).await {
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
mod tests {
    use super::*;
    use golem_common::model::Timestamp;
    use golem_common::model::card::{AccountCardHolder, CardHolder, CardId};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::durable_stream::{
        DURABLE_STREAM_FORMAT_VERSION, StreamConsumerDeletingRecord, StreamSessionRecord,
    };
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::OplogPayload;
    use golem_common::model::regions::{DeletedRegionsBuilder, OplogRegion};
    use golem_common::model::{AgentFingerprint, AgentId, TransactionId};
    use std::collections::HashMap;
    use test_r::test;
    use uuid::Uuid;

    fn idx(i: u64) -> OplogIndex {
        OplogIndex::from_u64(i)
    }

    fn deleted(regions: Vec<(u64, u64)>) -> DeletedRegions {
        DeletedRegionsBuilder::from_regions(regions.into_iter().map(|(start, end)| OplogRegion {
            start: idx(start),
            end: idx(end),
        }))
        .build()
    }

    async fn scan(
        entries: &HashMap<u64, OplogEntry>,
        cut: u64,
        end: u64,
        skipped: &DeletedRegions,
    ) -> Option<SpanningConstruct> {
        find_construct_spanning_cut_point(
            |i: OplogIndex| {
                let entry = entries
                    .get(&u64::from(i))
                    .cloned()
                    .unwrap_or_else(|| OplogEntry::no_op(None));
                async move { entry }
            },
            idx(cut),
            idx(end),
            skipped,
        )
        .await
    }

    async fn scan_stream_history(
        entries: &HashMap<u64, OplogEntry>,
        start: u64,
        end: u64,
    ) -> Option<OplogIndex> {
        find_stream_history_in_range(
            |i: OplogIndex| {
                let entry = entries
                    .get(&u64::from(i))
                    .cloned()
                    .unwrap_or_else(|| OplogEntry::no_op(None));
                async move { entry }
            },
            idx(start),
            idx(end),
        )
        .await
    }

    fn stream_entry() -> OplogEntry {
        OplogEntry::stream_session(
            None,
            OplogPayload::Inline(Box::new(StreamSessionRecord::ConsumerDeleting(
                StreamConsumerDeletingRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    consumer_environment_id: EnvironmentId(Uuid::from_u128(1)),
                    consumer: AgentId {
                        component_id: ComponentId(Uuid::from_u128(2)),
                        agent_id: "consumer".to_string(),
                    },
                    consumer_fingerprint: AgentFingerprint(Uuid::from_u128(3)),
                    deleting_at_millis: 100,
                },
            ))),
            None,
        )
    }

    fn snapshot_update(revision: u64) -> UpdateDescription {
        UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::new(revision).unwrap(),
            payload: OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
        }
    }

    #[test]
    fn successful_snapshot_update_boundary_is_validated() {
        let update = snapshot_update(2);
        let entries = BTreeMap::from([
            (idx(3), OplogEntry::pending_update(update.clone())),
            (
                idx(5),
                OplogEntry::successful_update(
                    *update.target_revision(),
                    100,
                    None,
                    Default::default(),
                ),
            ),
        ]);

        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(2), &DeletedRegions::new()),
            Ok(())
        );
        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(3), &DeletedRegions::new()),
            Err(RevertUpdateBoundaryError::SplitSnapshotUpdate {
                pending_index: idx(3),
                outcome_index: idx(5),
            })
        );
        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(4), &DeletedRegions::new()),
            Err(RevertUpdateBoundaryError::SplitSnapshotUpdate {
                pending_index: idx(3),
                outcome_index: idx(5),
            })
        );
        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(5), &DeletedRegions::new()),
            Ok(())
        );
    }

    #[test]
    fn failed_snapshot_update_boundary_is_rejected() {
        let update = snapshot_update(2);
        let entries = BTreeMap::from([
            (idx(3), OplogEntry::pending_update(update.clone())),
            (
                idx(5),
                OplogEntry::failed_update(*update.target_revision(), Some("failed".to_string())),
            ),
        ]);

        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(3), &DeletedRegions::new()),
            Err(RevertUpdateBoundaryError::SplitSnapshotUpdate {
                pending_index: idx(3),
                outcome_index: idx(5),
            })
        );
    }

    #[test]
    fn unapplied_snapshot_update_can_be_fully_removed_or_retained() {
        let update = snapshot_update(2);
        let entries = BTreeMap::from([(idx(3), OplogEntry::pending_update(update))]);

        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(2), &DeletedRegions::new()),
            Ok(())
        );
        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(3), &DeletedRegions::new()),
            Ok(())
        );
    }

    #[test]
    fn automatic_and_deleted_snapshot_updates_do_not_grant_an_exception() {
        let snapshot = snapshot_update(2);
        let automatic = UpdateDescription::Automatic {
            target_revision: ComponentRevision::new(3).unwrap(),
        };
        let entries = BTreeMap::from([
            (idx(2), OplogEntry::pending_update(snapshot.clone())),
            (
                idx(3),
                OplogEntry::successful_update(
                    *snapshot.target_revision(),
                    100,
                    None,
                    Default::default(),
                ),
            ),
            (idx(5), OplogEntry::pending_update(automatic.clone())),
            (
                idx(6),
                OplogEntry::successful_update(
                    *automatic.target_revision(),
                    100,
                    None,
                    Default::default(),
                ),
            ),
        ]);

        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(1), &deleted(vec![(2, 3)])),
            Ok(())
        );
    }

    #[test]
    async fn stream_history_is_found_in_the_raw_requested_range() {
        let entries = HashMap::from([(3, stream_entry()), (7, stream_entry())]);
        assert_eq!(scan_stream_history(&entries, 1, 5).await, Some(idx(3)));
        assert_eq!(scan_stream_history(&entries, 4, 8).await, Some(idx(7)));
    }

    #[test]
    async fn clean_cut_is_accepted() {
        let entries = HashMap::from([
            (2, OplogEntry::end(idx(1), None, false, None, None)),
            (4, OplogEntry::end(idx(3), None, false, None, None)),
        ]);
        // Completed calls on either side do not constrain the cut.
        assert_eq!(scan(&entries, 2, 5, &deleted(vec![])).await, None);
    }

    #[test]
    async fn durable_call_cuts_use_the_retained_prefix_not_future_completion() {
        for entries in [
            HashMap::from([(5, OplogEntry::cancelled(idx(2), None, None))]),
            HashMap::from([(5, OplogEntry::end(idx(2), None, false, None, None))]),
            HashMap::from([
                (5, OplogEntry::end(idx(2), None, false, None, None)),
                (8, OplogEntry::completion_delivered(idx(2))),
            ]),
            HashMap::from([
                (5, OplogEntry::end(idx(2), None, false, None, None)),
                (8, OplogEntry::completion_discarded(idx(2))),
            ]),
        ] {
            for cut in [2, 4, 5, 7, 8] {
                assert_eq!(scan(&entries, cut, 8, &deleted(vec![])).await, None);
            }
        }
    }

    #[test]
    async fn recoverable_call_does_not_hide_a_later_transaction_boundary() {
        let entries = HashMap::from([
            (5, OplogEntry::end(idx(2), None, false, None, None)),
            (7, OplogEntry::committed_remote_transaction(idx(3))),
        ]);
        assert_eq!(
            scan(&entries, 4, 8, &deleted(vec![])).await,
            Some(SpanningConstruct::RemoteTransaction {
                begin_index: idx(3),
                reference_index: idx(7),
            })
        );
    }

    #[test]
    async fn completion_discarded_with_start_in_deleted_region_is_ignored() {
        // A marker after the cut whose referenced Start lies in a deleted region is an orphan of
        // an abandoned timeline; it must not invalidate the cut.
        let entries = HashMap::from([(5, OplogEntry::completion_discarded(idx(2)))]);
        assert_eq!(scan(&entries, 4, 6, &deleted(vec![(2, 3)])).await, None);
    }

    #[test]
    async fn end_atomic_region_after_cut_is_rejected() {
        let entries = HashMap::from([(6, OplogEntry::end_atomic_region(None, idx(2)))]);
        assert_eq!(
            scan(&entries, 4, 6, &deleted(vec![])).await,
            Some(SpanningConstruct::AtomicRegion {
                begin_index: idx(2),
                end_index: idx(6),
            })
        );
    }

    #[test]
    async fn remote_transaction_terminal_after_cut_is_rejected() {
        let entries = HashMap::from([(9, OplogEntry::committed_remote_transaction(idx(4)))]);
        assert_eq!(
            scan(&entries, 6, 9, &deleted(vec![])).await,
            Some(SpanningConstruct::RemoteTransaction {
                begin_index: idx(4),
                reference_index: idx(9),
            })
        );
    }

    #[test]
    async fn pre_commit_after_cut_is_rejected() {
        let entries = HashMap::from([(8, OplogEntry::pre_commit_remote_transaction(idx(4)))]);
        assert_eq!(
            scan(&entries, 6, 8, &deleted(vec![])).await,
            Some(SpanningConstruct::RemoteTransaction {
                begin_index: idx(4),
                reference_index: idx(8),
            })
        );
    }

    #[test]
    async fn retried_transaction_begin_after_cut_is_rejected() {
        let entries = HashMap::from([(
            10,
            OplogEntry::begin_remote_transaction(TransactionId::new(Uuid::nil()), Some(idx(3))),
        )]);
        assert_eq!(
            scan(&entries, 7, 10, &deleted(vec![])).await,
            Some(SpanningConstruct::RemoteTransaction {
                begin_index: idx(3),
                reference_index: idx(10),
            })
        );
    }

    #[test]
    async fn original_transaction_begin_after_cut_is_accepted() {
        let entries = HashMap::from([(
            10,
            OplogEntry::begin_remote_transaction(TransactionId::new(Uuid::nil()), None),
        )]);
        assert_eq!(scan(&entries, 7, 10, &deleted(vec![])).await, None);
    }

    #[test]
    async fn card_transfer_confirmation_after_cut_is_accepted() {
        let transfer_id = Uuid::new_v4();
        let source_card_id = CardId::new();
        let installed_card_id = CardId::new();
        let target_holder = CardHolder::Account(AccountCardHolder {
            account_id: Uuid::new_v4(),
        });
        let entries = HashMap::from([
            (
                3,
                OplogEntry::CardTransferStarted {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    transfer_id,
                    card_id: source_card_id,
                    source_holder: target_holder.clone(),
                    target_holder: target_holder.clone(),
                    source_wallet_generation: 1,
                },
            ),
            (
                5,
                OplogEntry::CardTransferConfirmed {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    transfer_id,
                    source_card_id,
                    installed_card_id,
                    target_holder,
                },
            ),
        ]);

        assert_eq!(scan(&entries, 4, 5, &deleted(vec![])).await, None);
    }

    #[test]
    async fn terminal_inside_deleted_region_is_ignored() {
        let entries = HashMap::from([(5, OplogEntry::end(idx(3), None, false, None, None))]);
        assert_eq!(scan(&entries, 4, 6, &deleted(vec![(5, 6)])).await, None);
    }

    #[test]
    async fn orphan_terminal_with_deleted_start_is_ignored() {
        // The Start at 3 lies in a deleted region: its terminal is an orphan that replay drains
        let entries = HashMap::from([(5, OplogEntry::end(idx(3), None, false, None, None))]);
        assert_eq!(scan(&entries, 4, 6, &deleted(vec![(2, 3)])).await, None);
    }

    #[test]
    async fn terminal_referencing_start_after_cut_is_accepted() {
        // Both halves after the cut: nothing spans it
        let entries = HashMap::from([(6, OplogEntry::end(idx(5), None, false, None, None))]);
        assert_eq!(scan(&entries, 4, 6, &deleted(vec![])).await, None);
    }

    #[test]
    async fn first_spanning_construct_is_reported() {
        let entries = HashMap::from([
            (5, OplogEntry::end_atomic_region(None, idx(2))),
            (6, OplogEntry::committed_remote_transaction(idx(3))),
        ]);
        assert_eq!(
            scan(&entries, 4, 6, &deleted(vec![])).await,
            Some(SpanningConstruct::AtomicRegion {
                begin_index: idx(2),
                end_index: idx(5),
            })
        );
    }
}
