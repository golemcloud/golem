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

use super::*;
use golem_common::model::Timestamp;
use golem_common::model::card::{AccountCardHolder, CardHolder, CardId};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::durable_stream::{
    DURABLE_STREAM_FORMAT_VERSION, StreamConsumerDeletingRecord, StreamSessionRecord,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogPayload;
use golem_common::model::oplog::UpdateDescription;
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
                .unwrap_or_else(|| OplogEntry::grow_memory(0));
            async move { Ok::<_, std::convert::Infallible>(entry) }
        },
        idx(cut),
        idx(end),
        skipped,
    )
    .await
    .unwrap()
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
                .unwrap_or_else(|| OplogEntry::grow_memory(0));
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
        filesystem_snapshot: None,
    }
}

fn snapshot_assisted_update(revision: u64) -> UpdateDescription {
    UpdateDescription::SnapshotAssistedAutomatic {
        target_revision: ComponentRevision::new(revision).unwrap(),
        source_component_revision: ComponentRevision::new(revision - 1).unwrap(),
        source_revision_start_index: OplogIndex::INITIAL,
        snapshot_index: idx(2),
        snapshot_revision: ComponentRevision::new(revision - 1).unwrap(),
        filesystem_snapshot: None,
    }
}

#[test]
fn successful_snapshot_update_boundary_is_validated() {
    let update = snapshot_update(2);
    let entries = BTreeMap::from([
        (idx(3), OplogEntry::pending_update(update.clone(), None)),
        (
            idx(5),
            OplogEntry::successful_update(
                *update.target_revision(),
                100,
                None,
                Default::default(),
                None,
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
        (idx(3), OplogEntry::pending_update(update.clone(), None)),
        (
            idx(5),
            OplogEntry::failed_update(
                *update.target_revision(),
                Some("failed".to_string()),
                None,
                None,
                None,
            ),
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
fn assisted_update_outcome_can_be_removed_while_request_is_retained() {
    for outcome in [
        OplogEntry::successful_update(
            ComponentRevision::new(2).unwrap(),
            100,
            None,
            Default::default(),
            None,
        ),
        OplogEntry::failed_update(
            ComponentRevision::new(2).unwrap(),
            Some("failed".to_string()),
            None,
            None,
            None,
        ),
    ] {
        let entries = BTreeMap::from([
            (
                idx(3),
                OplogEntry::pending_update(snapshot_assisted_update(2), None),
            ),
            (idx(5), outcome),
        ]);
        assert_eq!(
            validate_snapshot_update_boundaries(&entries, idx(3), &DeletedRegions::new()),
            Ok(())
        );
    }
}

#[test]
fn unapplied_snapshot_update_can_be_fully_removed_or_retained() {
    let update = snapshot_update(2);
    let entries = BTreeMap::from([(idx(3), OplogEntry::pending_update(update, None))]);

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
        (idx(2), OplogEntry::pending_update(snapshot.clone(), None)),
        (
            idx(3),
            OplogEntry::successful_update(
                *snapshot.target_revision(),
                100,
                None,
                Default::default(),
                None,
            ),
        ),
        (idx(5), OplogEntry::pending_update(automatic.clone(), None)),
        (
            idx(6),
            OplogEntry::successful_update(
                *automatic.target_revision(),
                100,
                None,
                Default::default(),
                None,
            ),
        ),
    ]);

    assert_eq!(
        validate_snapshot_update_boundaries(&entries, idx(1), &deleted(vec![(2, 3)])),
        Ok(())
    );
}

#[test]
fn a_cut_between_a_manual_admission_and_its_failure_without_a_pending_update_is_accepted() {
    let target_revision = ComponentRevision::new(2).unwrap();
    let entries = BTreeMap::from([
        (
            idx(2),
            OplogEntry::PendingAgentInvocation {
                timestamp: Timestamp::now_utc(),
                idempotency_key: golem_common::model::IdempotencyKey::fresh(),
                payload: golem_common::model::oplog::OplogPayload::Inline(Box::new(
                    golem_common::model::AgentInvocationPayload::ManualUpdate { target_revision },
                )),
                trace_id: golem_common::model::invocation_context::TraceId::generate(),
                trace_states: Vec::new(),
                invocation_context: Vec::new(),
            },
        ),
        (
            idx(3),
            OplogEntry::failed_update(target_revision, None, None, Some(idx(2)), None),
        ),
    ]);

    assert_eq!(
        validate_snapshot_update_boundaries(&entries, idx(2), &DeletedRegions::new()),
        Ok(())
    );
}

/// A snapshot-based update that pairs a manual update invocation gives the same boundaries
/// as one without that invocation, at each cut and with the invocation in a deleted region.
#[test]
fn a_manual_admission_does_not_change_the_boundaries_of_a_snapshot_based_update() {
    let update = snapshot_update(2);
    let admission = OplogEntry::PendingAgentInvocation {
        timestamp: Timestamp::now_utc(),
        idempotency_key: golem_common::model::IdempotencyKey::fresh(),
        payload: OplogPayload::Inline(Box::new(
            golem_common::model::AgentInvocationPayload::ManualUpdate {
                target_revision: *update.target_revision(),
            },
        )),
        trace_id: golem_common::model::invocation_context::TraceId::generate(),
        trace_states: Vec::new(),
        invocation_context: Vec::new(),
    };
    let without = BTreeMap::from([
        (
            idx(3),
            OplogEntry::pending_update(update.clone(), Some(idx(2))),
        ),
        (
            idx(5),
            OplogEntry::successful_update(
                *update.target_revision(),
                100,
                None,
                Default::default(),
                None,
            ),
        ),
    ]);
    let with = without
        .clone()
        .into_iter()
        .chain([(idx(2), admission)])
        .collect::<BTreeMap<_, _>>();

    (1..=6).for_each(|cut| {
        [DeletedRegions::new(), deleted(vec![(2, 2)])]
            .iter()
            .for_each(|deleted| {
                assert_eq!(
                    validate_snapshot_update_boundaries(&with, idx(cut), deleted),
                    validate_snapshot_update_boundaries(&without, idx(cut), deleted),
                    "cut {cut}"
                )
            })
    });
    assert!(validate_snapshot_update_boundaries(&with, idx(4), &DeletedRegions::new()).is_err());
}

#[test]
fn a_failure_whose_attempt_index_names_an_update_behind_the_head_ends_nothing() {
    let manual = snapshot_update(3);
    let automatic = UpdateDescription::Automatic {
        target_revision: ComponentRevision::new(2).unwrap(),
    };
    let failure = |target: u64, attempt: u64| {
        OplogEntry::failed_update(
            ComponentRevision::new(target).unwrap(),
            None,
            None,
            Some(idx(attempt)),
            None,
        )
    };
    let entries = BTreeMap::from([
        (idx(2), OplogEntry::pending_update(automatic, None)),
        (idx(3), OplogEntry::pending_update(manual, None)),
        (idx(4), failure(3, 3)),
        (idx(5), failure(2, 2)),
    ]);

    assert_eq!(
        validate_snapshot_update_boundaries(&entries, idx(3), &DeletedRegions::new()),
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
async fn span_closed_on_either_side_of_the_cut_is_accepted() {
    let span_closed = OplogEntry::SpanClosed {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        resource_start_index: Some(idx(2)),
        span_finished: golem_common::model::oplog::SpanFinished {
            span_id: golem_common::model::invocation_context::SpanId::generate(),
            finished_at: Timestamp::now_utc(),
            outcome: golem_common::model::oplog::SpanOutcome::Completed,
        },
    };
    let entries = HashMap::from([
        (3, OplogEntry::end(idx(2), None, false, None, None)),
        (5, span_closed),
    ]);
    // The hint closes the span of the call at 2; it pairs with nothing, so a cut just before
    // or just after it (or between the call and the hint) is clean.
    for cut in [3, 4, 5, 6] {
        assert_eq!(scan(&entries, cut, 6, &deleted(vec![])).await, None);
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
        OplogEntry::begin_remote_transaction(TransactionId::new(Uuid::nil()), idx(3)),
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
        OplogEntry::begin_remote_transaction(TransactionId::new(Uuid::nil()), idx(9)),
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
        (5, OplogEntry::pre_commit_remote_transaction(idx(2))),
        (6, OplogEntry::committed_remote_transaction(idx(3))),
    ]);
    assert_eq!(
        scan(&entries, 4, 6, &deleted(vec![])).await,
        Some(SpanningConstruct::RemoteTransaction {
            begin_index: idx(2),
            reference_index: idx(5),
        })
    );
}
