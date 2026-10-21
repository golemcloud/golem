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
use crate::durable_host::p3::http::test_support::FrameTestOplog;
use crate::durable_host::tail_work::TailWorkTracker;
use crate::services::oplog::Oplog;
use golem_common::model::Timestamp;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostResponse, OplogEntry, OplogIndex, OplogPayload,
};
use test_r::{test, timeout};

/// Seeds `oplog` with the durable prefix a live consume-body loop leaves behind right before
/// the guest-facing transfer of its first chunk: the parent consume-body `Start`, the child
/// chunk `Start`, and the child's persisted `End(Data)`. Returns the child's start index —
/// the index a `CompletionDiscarded` marker for the chunk must reference.
async fn seed_persisted_data_child(oplog: &FrameTestOplog, bytes: &[u8]) -> OplogIndex {
    oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::P3HttpClientConsumeBody,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    let child_start = oplog
        .add(OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(OplogIndex::from_u64(1)),
            function_name: HostFunctionName::P3HttpClientConsumeBodyChunk,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
            denial: None,
        })
        .await
        .unwrap();
    oplog
        .add(OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: child_start,
            response: Some(OplogPayload::Inline(Box::new(
                HostResponse::P3HttpClientConsumeBodyChunk(
                    HostResponseP3HttpClientConsumeBodyChunk {
                        chunk: SerializableP3HttpBodyChunk::Data(bytes.to_vec()),
                    },
                ),
            ))),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        })
        .await
        .unwrap();
    child_start
}

/// A successful live transfer must deliver the chunk to the guest and record the child's
/// `CompletionDelivered` marker.
#[test]
#[timeout("10s")]
async fn durable_demand_delivery_delivers_live_chunk() {
    let oplog = FrameTestOplog::new();
    let child_start = seed_persisted_data_child(&oplog, b"abc").await;
    let seeded_entries = oplog.entry_count();
    let delivery = CompletionDelivery::test_live_armed(oplog.clone(), child_start)
        .await
        .expect("failed to build the live delivery token");
    let tracker = TailWorkTracker::new();
    let activity = tracker.activity();
    let (demand, mut reply) = oneshot::channel();

    let mut transfer = Box::pin(deliver_http_body_reply(
        &activity,
        demand,
        delivery,
        true,
        |delivery| HttpBodyChunkReply::Data {
            bytes: Bytes::from_static(b"abc"),
            delivery,
        },
    ));
    assert!(
        futures::poll!(transfer.as_mut()).is_pending(),
        "the transfer must wait until the producer observes the queued chunk"
    );
    assert_eq!(
        oplog.entry_count(),
        seeded_entries,
        "queueing the chunk is not yet a guest delivery boundary"
    );
    match reply.try_recv() {
        Ok(HttpBodyChunkReply::Data { bytes, delivery }) => {
            assert_eq!(bytes, Bytes::from_static(b"abc"));
            delivery.observed();
        }
        Ok(_) => panic!("expected the delivered data chunk, got a different reply kind"),
        Err(error) => panic!("expected the delivered data chunk, got no reply: {error}"),
    }
    let outcome = transfer.await.expect("live transfer must not fail");

    assert!(matches!(outcome, DemandDelivery::Delivered));
    assert_eq!(
        oplog.entry_count(),
        seeded_entries + 1,
        "a delivered chunk must append exactly one marker"
    );
    let markers = oplog
        .entries()
        .into_iter()
        .filter(|entry| matches!(entry, OplogEntry::CompletionDelivered { .. }))
        .collect::<Vec<_>>();
    match markers.as_slice() {
        [OplogEntry::CompletionDelivered { start_index, .. }] => {
            assert_eq!(
                *start_index, child_start,
                "the marker must reference the delivered child's Start"
            );
        }
        other => panic!("expected exactly one CompletionDelivered marker, got {other:?}"),
    }
}

/// The vanished-demand-receiver regression at the unit level: the child's `End(Data)` is
/// durable but the guest dropped the body reader before the transfer. The helper must report
/// the body abandoned and must reserve the child's `CompletionDiscarded` marker before it
/// returns.
#[test]
#[timeout("10s")]
async fn durable_demand_delivery_records_discard_marker_for_closed_live_receiver() {
    let oplog = FrameTestOplog::new();
    let child_start = seed_persisted_data_child(&oplog, b"abc").await;
    let delivery = CompletionDelivery::test_live_armed(oplog.clone(), child_start)
        .await
        .expect("failed to build the live delivery token");
    let tracker = TailWorkTracker::new();
    let activity = tracker.activity();
    let (demand, reply) = oneshot::channel::<HttpBodyChunkReply>();
    drop(reply);
    let outcome = deliver_http_body_reply(&activity, demand, delivery, true, |delivery| {
        HttpBodyChunkReply::Data {
            bytes: Bytes::from_static(b"abc"),
            delivery,
        }
    })
    .await
    .expect("a discarded transfer must not fail the task");

    assert!(matches!(outcome, DemandDelivery::Abandoned));
    let markers = oplog
        .entries()
        .into_iter()
        .filter(|entry| matches!(entry, OplogEntry::CompletionDiscarded { .. }))
        .collect::<Vec<_>>();
    match markers.as_slice() {
        [OplogEntry::CompletionDiscarded { start_index, .. }] => {
            assert_eq!(
                *start_index, child_start,
                "the marker must reference the discarded child's Start"
            );
        }
        other => panic!("expected exactly one CompletionDiscarded marker, got {other:?}"),
    }
}

/// Replay of a recorded discarded chunk with a repeated guest demand: the helper must never
/// re-deliver the recorded bytes — it parks (as inactive tail work) until the replayed guest
/// drops its in-flight read, then finalizes the body as abandoned without touching the oplog
/// again.
#[test]
#[timeout("10s")]
async fn durable_demand_delivery_parks_replay_discarded_until_reader_drop() {
    let oplog = FrameTestOplog::new();
    let delivery = CompletionDelivery::test_replay_discarded();
    let tracker = TailWorkTracker::new();
    let activity = tracker.activity();
    let (demand, mut reply) = oneshot::channel::<HttpBodyChunkReply>();
    let mut transfer = Box::pin(deliver_http_body_reply(
        &activity,
        demand,
        delivery,
        true,
        |delivery| HttpBodyChunkReply::Data {
            bytes: Bytes::from_static(b"abc"),
            delivery,
        },
    ));
    assert!(
        futures::poll!(transfer.as_mut()).is_pending(),
        "the transfer must park while the replayed guest still holds the body reader"
    );
    assert!(
        !tracker.has_active(),
        "the parked transfer must be a safe park point (inactive tail work)"
    );
    assert!(
        reply.try_recv().is_err(),
        "a replay-discarded chunk must never be re-delivered"
    );

    drop(reply);
    let outcome = transfer
        .await
        .expect("a replay-discarded transfer must not fail the task");
    assert!(matches!(outcome, DemandDelivery::Abandoned));
    assert!(
        tracker.has_active(),
        "the task must be counted active again after un-parking"
    );
    assert_eq!(
        oplog.entry_count(),
        0,
        "replay must not append anything at the delivery boundary"
    );
}
