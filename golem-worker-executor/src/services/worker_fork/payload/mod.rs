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
        OplogEntry::ToolAttachmentRead { payload, .. } => {
            if let Some(payload) = payload {
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
        | OplogEntry::Jump { .. }
        | OplogEntry::Interrupted { .. }
        | OplogEntry::Resumed { .. }
        | OplogEntry::Exited { .. }
        | OplogEntry::PendingUpdate {
            description:
                UpdateDescription::Automatic { .. }
                | UpdateDescription::SnapshotAssistedAutomatic { .. },
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
        | OplogEntry::CompletionDelivered { .. }
        | OplogEntry::ToolAttachmentControl { .. }
        | OplogEntry::SnapshotConfirmed { .. } => {}
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
mod tests;
