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

//! The oplog entries as an oplog processor receives them.
//!
//! A processor receives raw entries, and the raw form carries some records as opaque bytes. In
//! the stored oplog these records keep a user's verified claims, so that code that reads the
//! claims replays the same way. A processor never gets them: the oplog never gives a user's
//! claims to a guest, and a processor's own oplog keeps a copy of every batch that it receives.
//! A processor also never gets the delivery state that the executor keeps for each processor
//! installation, because that state holds the installation's parameters.

#[cfg(test)]
mod tests;

use crate::model::StoredAgentInvocationPayload;
use crate::model::entity::EntityInvocationRequest;
use crate::model::oplog::{HostRequest, OplogEntry, OplogPayload};
use crate::serialization::{deserialize, serialize};
use desert_rust::BinaryCodec;
use std::fmt::Debug;

impl OplogEntry {
    /// The entry as an oplog processor receives it: every caller without a user's verified
    /// claims, and no delivery state of a processor installation. An entry without either keeps
    /// its bytes.
    pub fn for_oplog_processor(mut self) -> Result<Self, String> {
        hide_from_oplog_processor(&mut self)?;
        Ok(self)
    }
}

/// Removes from `entry` what an oplog processor never receives. Returns whether `entry` changed.
///
/// The match names every entry kind, so that a new kind needs a decision here.
fn hide_from_oplog_processor(entry: &mut OplogEntry) -> Result<bool, String> {
    match entry {
        OplogEntry::Create { parameters, .. } => {
            Ok(parameters.create_request.caller.remove_claims())
        }
        OplogEntry::Start { request, .. } => match request {
            Some(request) => update_inline(request, remove_claims_from_host_request),
            None => Ok(false),
        },
        OplogEntry::HostStreamFrame { payload, .. } => {
            update_inline(payload, remove_claims_from_host_request)
        }
        OplogEntry::AgentInvocationStarted { payload, .. }
        | OplogEntry::PendingAgentInvocation { payload, .. } => {
            update_inline(payload, remove_claims_from_invocation)
        }
        OplogEntry::PendingUpdate { requester, .. } => Ok(requester.remove_claims()),
        OplogEntry::OplogProcessorCheckpoint { delivery, .. } => Ok(delivery.take().is_some()),
        OplogEntry::End { .. }
        | OplogEntry::Cancelled { .. }
        | OplogEntry::AgentInvocationFinished { .. }
        | OplogEntry::AgentInvocationFailed { .. }
        | OplogEntry::AgentInvocationSettled { .. }
        | OplogEntry::Suspend { .. }
        | OplogEntry::Error { .. }
        | OplogEntry::RecoverySucceeded { .. }
        | OplogEntry::Jump { .. }
        | OplogEntry::Interrupted { .. }
        | OplogEntry::Exited { .. }
        | OplogEntry::SuccessfulUpdate { .. }
        | OplogEntry::FailedUpdate { .. }
        | OplogEntry::GrowMemory { .. }
        | OplogEntry::MemorySizeReconciled { .. }
        | OplogEntry::CreateResource { .. }
        | OplogEntry::DropResource { .. }
        | OplogEntry::Log { .. }
        | OplogEntry::Restart { .. }
        | OplogEntry::Resumed { .. }
        | OplogEntry::ActivatePlugin { .. }
        | OplogEntry::DeactivatePlugin { .. }
        | OplogEntry::Revert { .. }
        | OplogEntry::CancelPendingInvocation { .. }
        | OplogEntry::BeginRemoteTransaction { .. }
        | OplogEntry::PreCommitRemoteTransaction { .. }
        | OplogEntry::PreRollbackRemoteTransaction { .. }
        | OplogEntry::CommittedRemoteTransaction { .. }
        | OplogEntry::RolledBackRemoteTransaction { .. }
        | OplogEntry::Snapshot { .. }
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
        | OplogEntry::ToolAttachmentRead { .. }
        | OplogEntry::ToolAttachmentControl { .. }
        | OplogEntry::StreamRegistered { .. }
        | OplogEntry::StreamItems { .. }
        | OplogEntry::StreamTerminal { .. }
        | OplogEntry::StreamSession { .. }
        | OplogEntry::CompletionDiscarded { .. }
        | OplogEntry::CompletionDelivered { .. }
        | OplogEntry::SnapshotConfirmed { .. }
        | OplogEntry::SpanClosed { .. } => Ok(false),
    }
}

/// Applies `update` to the value of an inline payload, and encodes the value again only when it
/// changed. An external payload is only a reference: a processor cannot read its bytes, and
/// `enrich-oplog-entries` never shows claims. Its in-memory copy goes.
fn update_inline<T>(
    payload: &mut OplogPayload<T>,
    update: impl FnOnce(&mut T) -> Result<bool, String>,
) -> Result<bool, String>
where
    T: BinaryCodec + Debug + Clone + PartialEq,
{
    match payload {
        OplogPayload::Inline(value) => update(value),
        OplogPayload::SerializedInline { bytes, cached } => {
            let mut value: T = deserialize(bytes)?;
            let changed = update(&mut value)?;
            if changed {
                *bytes = serialize(&value)?;
                *cached = None;
            }
            Ok(changed)
        }
        OplogPayload::External { cached, .. } => {
            *cached = None;
            Ok(false)
        }
    }
}

/// Removes a user's verified claims from every caller of a stored invocation request. Returns
/// whether the request changed.
fn remove_claims_from_invocation(
    payload: &mut StoredAgentInvocationPayload,
) -> Result<bool, String> {
    match payload {
        StoredAgentInvocationPayload::ManualUpdate { requester } => Ok(requester.remove_claims()),
        StoredAgentInvocationPayload::AgentInitialization { caller, .. }
        | StoredAgentInvocationPayload::AgentMethod { caller, .. }
        | StoredAgentInvocationPayload::ExternalTool { caller, .. }
        | StoredAgentInvocationPayload::LoadSnapshot { caller, .. }
        | StoredAgentInvocationPayload::SaveSnapshot { caller } => Ok(caller.remove_claims()),
        // The batch of an oplog processor whose own oplog is processed.
        StoredAgentInvocationPayload::ProcessOplogEntries {
            caller, entries, ..
        } => {
            let mut changed = caller.remove_claims();
            for entry in entries {
                changed |= hide_from_oplog_processor(entry)?;
            }
            Ok(changed)
        }
    }
}

/// Removes a user's verified claims from the caller that a tool-call request records. No other
/// host request records a caller.
fn remove_claims_from_host_request(request: &mut HostRequest) -> Result<bool, String> {
    let HostRequest::EntityInvocation(request) = request else {
        return Ok(false);
    };
    // The entity code encodes this record without the version byte of `serialize`.
    let mut metadata: EntityInvocationRequest = desert_rust::deserialize(&request.metadata)
        .map_err(|error| format!("failed to decode entity invocation metadata: {error}"))?;
    if !metadata.caller.remove_claims() {
        return Ok(false);
    }
    request.metadata = desert_rust::serialize_to_byte_vec(&metadata)
        .map_err(|error| format!("failed to encode entity invocation metadata: {error}"))?;
    Ok(true)
}
