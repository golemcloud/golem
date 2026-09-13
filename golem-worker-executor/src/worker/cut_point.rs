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
//! (fork). Paired durable constructs must not span such a cut — a durable call or scope whose
//! `Start` survives but whose `End`/`Cancelled` is cut off replays as an *incomplete* call
//! (re-executed when safe, hard error when not), and an open atomic region or remote transaction
//! loses its committed/rolled-back outcome. Instead of silently degrading to those recovery
//! semantics, the cut is validated up front and rejected with a clear error.

use golem_common::base_model::OplogIndex;
use golem_common::model::oplog::{OplogEntry, OplogIndexRange};
use golem_common::model::regions::DeletedRegions;
use std::fmt::{Display, Formatter};
use std::future::Future;

/// A paired durable construct whose two halves lie on opposite sides of a cut point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanningConstruct {
    /// A durable host call or durable scope: its `Start` is at or before the cut, while its
    /// `End`/`Cancelled` terminal lies after it.
    DurableCall {
        start_index: OplogIndex,
        terminal_index: OplogIndex,
    },
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
            SpanningConstruct::DurableCall {
                start_index,
                terminal_index,
            } => write!(
                f,
                "an active durable call or scope (started at oplog index {start_index}, terminated at {terminal_index})"
            ),
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

/// Scans the oplog entries in `(cut_point, scan_end]` for evidence that a paired durable
/// construct spans the cut: an entry after the cut that references its opening entry at or
/// before the cut. Returns the first such construct found, or `None` when the cut is clean.
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
            // Completion markers are hints, not second terminals, but they carry the delivery
            // status and timing of their call's `End`. A cut between the `End` and its marker
            // would leave a completed call that looks markerless, making replay tail-gate a
            // delivery the recorded run actually consumed at the marker's position, so reject
            // it the same way as one splitting a `Start` from its terminal.
            OplogEntry::End { start_index, .. }
            | OplogEntry::Cancelled { start_index, .. }
            | OplogEntry::CompletionDiscarded { start_index, .. }
            | OplogEntry::CompletionDelivered { start_index, .. } => Some((
                *start_index,
                SpanningConstruct::DurableCall {
                    start_index: *start_index,
                    terminal_index: idx,
                },
            )),
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
            | OplogEntry::AgentInvocationStarted { .. }
            | OplogEntry::AgentInvocationFinished { .. }
            | OplogEntry::Suspend { .. }
            | OplogEntry::Error { .. }
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
            | OplogEntry::ActivatePlugin { .. }
            | OplogEntry::DeactivatePlugin { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::CancelPendingInvocation { .. }
            | OplogEntry::StartSpan { .. }
            | OplogEntry::FinishSpan { .. }
            | OplogEntry::SetSpanAttribute { .. }
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

/// Returns the first durable stream entry in an inclusive raw oplog range.
/// Fork and revert reject such histories until identities, offsets and cross-oplog dependencies
/// can be rewritten without weakening their incarnation guarantees.
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
    use golem_common::model::component::ComponentId;
    use golem_common::model::durable_stream::{
        DURABLE_STREAM_FORMAT_VERSION, StreamConsumerDeletingRecordV1, StreamSessionRecordV1,
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
            OplogPayload::Inline(Box::new(StreamSessionRecordV1::ConsumerDeleting(
                StreamConsumerDeletingRecordV1 {
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
        )
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
            (2, OplogEntry::end(idx(1), None, false)),
            (4, OplogEntry::end(idx(3), None, false)),
        ]);
        // Cut after a completed call: the End at 4 references 3, which is also after the cut
        assert_eq!(scan(&entries, 2, 5, &deleted(vec![])).await, None);
    }

    #[test]
    async fn end_after_cut_referencing_surviving_start_is_rejected() {
        let entries = HashMap::from([(5, OplogEntry::end(idx(3), None, false))]);
        assert_eq!(
            scan(&entries, 4, 6, &deleted(vec![])).await,
            Some(SpanningConstruct::DurableCall {
                start_index: idx(3),
                terminal_index: idx(5),
            })
        );
    }

    #[test]
    async fn cancelled_after_cut_referencing_surviving_start_is_rejected() {
        let entries = HashMap::from([(7, OplogEntry::cancelled(idx(2), None))]);
        assert_eq!(
            scan(&entries, 5, 8, &deleted(vec![])).await,
            Some(SpanningConstruct::DurableCall {
                start_index: idx(2),
                terminal_index: idx(7),
            })
        );
    }

    #[test]
    async fn cut_at_start_index_itself_is_rejected() {
        // Adjacent Start(3)/End(4) pair: a cut at 3 keeps the Start but drops the End
        let entries = HashMap::from([(4, OplogEntry::end(idx(3), None, false))]);
        assert_eq!(
            scan(&entries, 3, 4, &deleted(vec![])).await,
            Some(SpanningConstruct::DurableCall {
                start_index: idx(3),
                terminal_index: idx(4),
            })
        );
    }

    #[test]
    async fn completion_discarded_after_cut_referencing_surviving_start_is_rejected() {
        // Start(2)/End(3) survive the cut at 4, but the CompletionDiscarded marker at 5 is cut
        // off: the surviving prefix reflects the discarded completion while replay without the
        // marker would deliver the response, so the cut is rejected.
        let entries = HashMap::from([
            (3, OplogEntry::end(idx(2), None, false)),
            (5, OplogEntry::completion_discarded(idx(2))),
        ]);
        assert_eq!(
            scan(&entries, 4, 6, &deleted(vec![])).await,
            Some(SpanningConstruct::DurableCall {
                start_index: idx(2),
                terminal_index: idx(5),
            })
        );
    }

    #[test]
    async fn completion_delivered_after_cut_referencing_surviving_start_is_rejected() {
        let entries = HashMap::from([
            (3, OplogEntry::end(idx(2), None, false)),
            (5, OplogEntry::completion_delivered(idx(2))),
        ]);
        assert_eq!(
            scan(&entries, 4, 6, &deleted(vec![])).await,
            Some(SpanningConstruct::DurableCall {
                start_index: idx(2),
                terminal_index: idx(5),
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
                    source_holder: None,
                    target_holder: target_holder.clone(),
                    source_wallet_generation: Some(1),
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
        let entries = HashMap::from([(5, OplogEntry::end(idx(3), None, false))]);
        assert_eq!(scan(&entries, 4, 6, &deleted(vec![(5, 6)])).await, None);
    }

    #[test]
    async fn orphan_terminal_with_deleted_start_is_ignored() {
        // The Start at 3 lies in a deleted region: its terminal is an orphan that replay drains
        let entries = HashMap::from([(5, OplogEntry::end(idx(3), None, false))]);
        assert_eq!(scan(&entries, 4, 6, &deleted(vec![(2, 3)])).await, None);
    }

    #[test]
    async fn terminal_referencing_start_after_cut_is_accepted() {
        // Both halves after the cut: nothing spans it
        let entries = HashMap::from([(6, OplogEntry::end(idx(5), None, false))]);
        assert_eq!(scan(&entries, 4, 6, &deleted(vec![])).await, None);
    }

    #[test]
    async fn first_spanning_construct_is_reported() {
        let entries = HashMap::from([
            (5, OplogEntry::end_atomic_region(None, idx(2))),
            (6, OplogEntry::end(idx(3), None, false)),
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
