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

use super::attachment::attachment_lease_expiry;
use super::*;
use crate::services::rpc::RpcError;
use golem_common::model::durable_stream::StreamCancellationRequest;

/// Attachment control that resolves the producer owner before each durable operation.
pub struct RoutedStreamAttachmentControl {
    rpc: Arc<dyn Rpc>,
    mapping: StreamSessionMappingRecord,
    auth_ctx: Arc<AuthCtx>,
}

/// Segment source that routes reads to the producer while preserving attachment identity.
pub struct RoutedAttachedStreamSegmentSource {
    rpc: Arc<dyn Rpc>,
    mapping: StreamSessionMappingRecord,
    auth_ctx: Arc<AuthCtx>,
    metadata: Arc<DurableStreamStore>,
}

impl RoutedAttachedStreamSegmentSource {
    /// Creates a routed source for attached reads and waits.
    pub fn new(
        rpc: Arc<dyn Rpc>,
        mapping: StreamSessionMappingRecord,
        auth_ctx: Arc<AuthCtx>,
        metadata: Arc<DurableStreamStore>,
    ) -> Self {
        Self {
            rpc,
            mapping,
            auth_ctx,
            metadata,
        }
    }

    #[tracing::instrument(
        name = "durable_stream.read_routed",
        level = "debug",
        skip_all,
        fields(
            attachment_id = %request.attachment.attachment_id.0,
            stream_id = %request.attachment.stream_id,
            epoch = request.attachment.epoch,
            wait_for_events = request.wait_for_events,
            has_cursor = request.after.is_some()
        )
    )]
    async fn read_routed(
        &self,
        request: AttachedStreamSegmentRequest,
    ) -> Result<Vec<u8>, StreamStoreError> {
        let mut attempt = 0_u64;
        DurableStreamRemoteError::retry_while_unavailable(|| {
            attempt += 1;
            let attempt = attempt;
            let request = request.clone();
            async move {
                tracing::debug!(attempt, phase = "begin", "Routed stream read attempt");
                let result = self.rpc.read_durable_stream_segment(
                    golem_common::model::durable_stream::DurableStreamReadRequest::AttachedConsumer(
                        Box::new(request),
                    ),
                    &self.auth_ctx,
                ).await;
                tracing::debug!(
                    attempt,
                    outcome = match &result {
                        Ok(_) => "success",
                        Err(DurableStreamRemoteError::Unavailable) => "unavailable",
                        Err(DurableStreamRemoteError::Other(_)) => "error",
                    },
                    "Routed stream read attempt completed"
                );
                result
            }
        })
        .await
        .inspect(|_| {
            tracing::debug!(outcome = "success", "Routed stream read completed");
        })
        .map_err(|error| {
            tracing::debug!(outcome = "error", "Routed stream read failed");
            StreamStoreError::Oplog(error.to_string())
        })
    }
}

#[async_trait]
impl AttachedStreamSegmentSource for RoutedAttachedStreamSegmentSource {
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
    ) -> Result<usize, StreamStoreError> {
        if self.mapping.handle != *handle {
            return Err(StreamStoreError::InvalidHandle);
        }
        self.metadata.journal_lag_events(handle, after).await
    }

    async fn read_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        _now_millis: u64,
        after: Option<StreamOffset>,
        through: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        if self.mapping.handle != *handle {
            return Err(StreamStoreError::InvalidHandle);
        }
        let payload = self
            .read_routed(AttachedStreamSegmentRequest {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attachment: attachment.clone(),
                mapping: self.mapping.clone(),
                after,
                through,
                wait_for_events: false,
            })
            .await?;
        golem_common::serialization::deserialize(&payload).map_err(StreamStoreError::CorruptHistory)
    }

    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        _now_millis: u64,
        after: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        if self.mapping.handle != *handle {
            return Err(StreamStoreError::InvalidHandle);
        }
        let payload = self
            .read_routed(AttachedStreamSegmentRequest {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attachment: attachment.clone(),
                mapping: self.mapping.clone(),
                after,
                through: None,
                wait_for_events: true,
            })
            .await?;
        golem_common::serialization::deserialize(&payload).map_err(StreamStoreError::CorruptHistory)
    }
}

impl RoutedStreamAttachmentControl {
    /// Creates routed attachment control with the caller's authorization context.
    pub fn new(
        rpc: Arc<dyn Rpc>,
        mapping: StreamSessionMappingRecord,
        auth_ctx: Arc<AuthCtx>,
    ) -> Self {
        Self {
            rpc,
            mapping,
            auth_ctx,
        }
    }

    /// Sends one control to the producer, retrying while the producer is unavailable (recovering,
    /// fenced or being deleted). Retrying is safe: every control is idempotent on the producer.
    async fn execute_routed(
        &self,
        operation: StreamAttachmentControlOperation,
    ) -> Result<bool, RpcError> {
        DurableStreamRemoteError::retry_while_unavailable(|| {
            self.rpc.control_durable_stream_attachment(
                StreamAttachmentControlRequest {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    mapping: Some(self.mapping.clone()),
                    operation: operation.clone(),
                },
                &self.auth_ctx,
            )
        })
        .await
    }

    async fn execute(
        &self,
        operation: StreamAttachmentControlOperation,
    ) -> Result<bool, StreamStoreError> {
        self.execute_routed(operation)
            .await
            .map_err(|error| StreamStoreError::Oplog(error.to_string()))
    }

    /// Tells the producer that this consumer is being deleted. A producer that no longer exists
    /// has nothing left to finalize, so its absence completes the consumer's side.
    pub async fn finalize_deleted_consumer(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<(), StreamStoreError> {
        match self
            .execute_routed(StreamAttachmentControlOperation::Finalize {
                key,
                reason: StreamAttachmentFinalizationReason::ConsumerDeleted,
                now_millis,
            })
            .await
        {
            Ok(_) | Err(RpcError::NotFound { .. }) => Ok(()),
            Err(error) => Err(StreamStoreError::Oplog(error.to_string())),
        }
    }

    /// Forwards cancellation and waits for the producer's durable receipt, not callback delivery.
    pub async fn cancel_stream(
        &self,
        key: StreamAttachmentKey,
        cancellation: StreamCancellationRequest,
    ) -> Result<bool, StreamStoreError> {
        self.execute(StreamAttachmentControlOperation::Cancel { key, cancellation })
            .await
    }
}

#[async_trait]
impl StreamAttachmentControl for RoutedStreamAttachmentControl {
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError> {
        let replayed = self
            .execute(StreamAttachmentControlOperation::Prepare {
                key: key.clone(),
                now_millis,
            })
            .await?;
        Ok(ProducerWriteOutcome {
            value: StreamAttachmentView {
                key,
                state: StreamAttachmentState::Prepared,
                lease_expires_at_millis: Some(attachment_lease_expiry(now_millis)?),
            },
            replayed,
        })
    }

    async fn activate_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError> {
        let replayed = self
            .execute(StreamAttachmentControlOperation::Activate {
                key: key.clone(),
                now_millis,
            })
            .await?;
        Ok(ProducerWriteOutcome {
            value: StreamAttachmentView {
                key,
                state: StreamAttachmentState::Active,
                lease_expires_at_millis: None,
            },
            replayed,
        })
    }

    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<StreamAttachmentView, StreamStoreError> {
        self.execute(StreamAttachmentControlOperation::Detach { key: key.clone() })
            .await?;
        Ok(StreamAttachmentView {
            key: key.clone(),
            state: StreamAttachmentState::Active,
            lease_expires_at_millis: None,
        })
    }

    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKey,
        reason: StreamAttachmentFinalizationReason,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError> {
        let replayed = self
            .execute(StreamAttachmentControlOperation::Finalize {
                key: key.clone(),
                reason,
                now_millis,
            })
            .await?;
        Ok(ProducerWriteOutcome {
            value: StreamAttachmentView {
                key,
                state: StreamAttachmentState::Finalized(reason),
                lease_expires_at_millis: None,
            },
            replayed,
        })
    }

    #[cfg(test)]
    async fn inspect_attachments(&self) -> Vec<StreamAttachmentView> {
        Vec::new()
    }
}
