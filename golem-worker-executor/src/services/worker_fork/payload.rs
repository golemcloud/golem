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

use desert_rust::BinaryCodec;
use golem_common::model::oplog::{
    OplogEntry, OplogPayload, PayloadId, RawOplogPayload, UpdateDescription,
};
use std::fmt::Debug;
use std::future::Future;

/// Rehome external blobs without decoding or re-encoding their contents. Inline payloads are
/// already independent of the source. Cached external values are not durable ownership.
pub(super) async fn copy_entry_payloads<Copy, CopyFuture>(
    entry: &mut OplogEntry,
    mut copy: Copy,
) -> Result<(), String>
where
    Copy: FnMut(PayloadId, Vec<u8>) -> CopyFuture,
    CopyFuture: Future<Output = Result<RawOplogPayload, String>>,
{
    match entry {
        OplogEntry::Start { request, .. } => {
            if let Some(payload) = request {
                copy_payload(payload, &mut copy).await?;
            }
        }
        OplogEntry::End { response, .. }
        | OplogEntry::Cancelled {
            partial: response, ..
        } => {
            if let Some(payload) = response {
                copy_payload(payload, &mut copy).await?;
            }
        }
        OplogEntry::AgentInvocationStarted { payload, .. }
        | OplogEntry::PendingAgentInvocation { payload, .. } => {
            copy_payload(payload, &mut copy).await?
        }
        OplogEntry::AgentInvocationFinished { result, .. } => {
            copy_payload(result, &mut copy).await?
        }
        OplogEntry::Snapshot { data, .. }
        | OplogEntry::PendingUpdate {
            description: UpdateDescription::SnapshotBased { payload: data, .. },
            ..
        } => copy_payload(data, &mut copy).await?,
        OplogEntry::HostStreamFrame { payload, .. } => copy_payload(payload, &mut copy).await?,
        OplogEntry::StreamRegistered { record, .. } => copy_payload(record, &mut copy).await?,
        OplogEntry::StreamItems { record, .. } => copy_payload(record, &mut copy).await?,
        OplogEntry::StreamEnd { record, .. } => copy_payload(record, &mut copy).await?,
        OplogEntry::StreamCancel { record, .. } => copy_payload(record, &mut copy).await?,
        OplogEntry::StreamSession { record, .. } => copy_payload(record, &mut copy).await?,
        // Keep exhaustive so additions to the oplog require an ownership audit.
        OplogEntry::Create { .. }
        | OplogEntry::Suspend { .. }
        | OplogEntry::Error { .. }
        | OplogEntry::RecoverySucceeded { .. }
        | OplogEntry::NoOp { .. }
        | OplogEntry::Jump { .. }
        | OplogEntry::Interrupted { .. }
        | OplogEntry::Resumed { .. }
        | OplogEntry::Exited { .. }
        | OplogEntry::BeginAtomicRegion { .. }
        | OplogEntry::EndAtomicRegion { .. }
        | OplogEntry::PendingUpdate {
            description: UpdateDescription::Automatic { .. },
            ..
        }
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
        | OplogEntry::BeginRemoteTransaction { .. }
        | OplogEntry::PreCommitRemoteTransaction { .. }
        | OplogEntry::PreRollbackRemoteTransaction { .. }
        | OplogEntry::CommittedRemoteTransaction { .. }
        | OplogEntry::RolledBackRemoteTransaction { .. }
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
        | OplogEntry::CompletionDiscarded { .. }
        | OplogEntry::CompletionDelivered { .. } => {}
    }
    Ok(())
}

async fn copy_payload<T, Copy, CopyFuture>(
    payload: &mut OplogPayload<T>,
    copy: &mut Copy,
) -> Result<(), String>
where
    T: BinaryCodec + Debug + Clone + PartialEq,
    Copy: FnMut(PayloadId, Vec<u8>) -> CopyFuture,
    CopyFuture: Future<Output = Result<RawOplogPayload, String>>,
{
    if let OplogPayload::External {
        payload_id,
        md5_hash,
        ..
    } = payload
    {
        *payload = copy(payload_id.clone(), md5_hash.clone())
            .await?
            .into_payload()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::OplogIndex;
    use std::sync::Arc;
    use test_r::test;

    #[test]
    async fn copies_external_payload_even_when_cached_and_preserves_bytes() {
        let source_id = PayloadId::new();
        let source_hash = vec![3; 16];
        let source_bytes = vec![3, 19, 41, 97];
        let mut payload = OplogPayload::External {
            payload_id: source_id.clone(),
            md5_hash: source_hash.clone(),
            cached: Some(Arc::new(vec![99u8])),
        };
        let mut copies = 0;
        copy_payload(&mut payload, &mut |id, hash| {
            copies += 1;
            assert_eq!(id, source_id);
            assert_eq!(hash, source_hash);
            std::future::ready(Ok(RawOplogPayload::SerializedInline(source_bytes.clone())))
        })
        .await
        .unwrap();
        assert_eq!(copies, 1);
        assert!(
            matches!(payload, OplogPayload::SerializedInline { bytes, cached: None } if bytes == source_bytes)
        );
    }

    #[test]
    async fn inline_and_absent_payloads_do_not_touch_blob_storage() {
        let mut payload = OplogPayload::Inline(Box::new(vec![1u8, 9]));
        copy_payload(&mut payload, &mut |_,
                                         _|
         -> std::future::Ready<
            Result<RawOplogPayload, String>,
        > {
            panic!("inline payload must not be copied")
        })
        .await
        .unwrap();
        assert_eq!(payload, OplogPayload::Inline(Box::new(vec![1u8, 9])));
        let mut entry = OplogEntry::end(OplogIndex::INITIAL, None, false, None, None);
        copy_entry_payloads(
            &mut entry,
            |_, _| -> std::future::Ready<Result<RawOplogPayload, String>> {
                panic!("absent payload must not be copied")
            },
        )
        .await
        .unwrap();
    }

    #[test]
    async fn visits_every_stream_payload_and_snapshot_update() {
        fn external<T: BinaryCodec + Debug + Clone + PartialEq>() -> OplogPayload<T> {
            OplogPayload::External {
                payload_id: PayloadId::new(),
                md5_hash: vec![11; 16],
                cached: None,
            }
        }
        let mut entries = vec![
            OplogEntry::stream_registered(None, external(), None),
            OplogEntry::stream_items(None, external(), None),
            OplogEntry::stream_end(None, external(), None),
            OplogEntry::stream_cancel(None, external(), None),
            OplogEntry::stream_session(None, external(), None),
            OplogEntry::pending_update(UpdateDescription::SnapshotBased {
                target_revision: golem_common::model::component::ComponentRevision::INITIAL,
                payload: external(),
                mime_type: "application/octet-stream".to_string(),
            }),
        ];
        for entry in &mut entries {
            let mut copies = 0;
            copy_entry_payloads(entry, |_, _| {
                copies += 1;
                std::future::ready(Ok(RawOplogPayload::SerializedInline(vec![3, 19, 41])))
            })
            .await
            .unwrap();
            assert_eq!(copies, 1);
            copy_entry_payloads(
                entry,
                |_, _| -> std::future::Ready<Result<RawOplogPayload, String>> {
                    panic!("copied inline payload must not retain a source reference")
                },
            )
            .await
            .unwrap();
        }
    }

    #[test]
    async fn failed_copy_leaves_source_reference_intact() {
        let mut payload: OplogPayload<Vec<u8>> = OplogPayload::External {
            payload_id: PayloadId::new(),
            md5_hash: vec![5; 16],
            cached: None,
        };
        let original = payload.clone();
        let result = copy_payload(&mut payload, &mut |_, _| {
            std::future::ready(Err("upload failed".to_string()))
        })
        .await;
        assert_eq!(result, Err("upload failed".to_string()));
        assert_eq!(payload, original);
    }
}
