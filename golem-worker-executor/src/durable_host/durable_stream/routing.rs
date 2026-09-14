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

pub(crate) struct RoutedStreamAttachmentControl {
    rpc: Arc<dyn Rpc>,
    mapping: StreamSessionMappingRecord,
    auth_ctx: AuthCtx,
}

pub(crate) struct RoutedAttachedStreamSegmentSource {
    rpc: Arc<dyn Rpc>,
    mapping: StreamSessionMappingRecord,
    auth_ctx: AuthCtx,
    metadata: Arc<DurableStreamProducer>,
}

impl RoutedAttachedStreamSegmentSource {
    pub(crate) fn new(
        rpc: Arc<dyn Rpc>,
        mapping: StreamSessionMappingRecord,
        auth_ctx: AuthCtx,
        metadata: Arc<DurableStreamProducer>,
    ) -> Self {
        Self {
            rpc,
            mapping,
            auth_ctx,
            metadata,
        }
    }

    async fn read_routed(
        &self,
        request: AttachedStreamSegmentRequest,
    ) -> Result<Vec<u8>, DurableStreamProducerError> {
        let mut delay = std::time::Duration::from_millis(100);
        loop {
            match self
                .rpc
                .read_durable_stream_segment(
                    golem_common::model::durable_stream::DurableStreamReadRequest::AttachedConsumer(
                        Box::new(request.clone()),
                    ),
                    &self.auth_ctx,
                )
                .await
            {
                Ok(payload) => return Ok(payload),
                Err(DurableStreamReadError::Other(error)) => {
                    return Err(DurableStreamProducerError::Oplog(error.to_string()));
                }
                Err(DurableStreamReadError::Unavailable) => {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(std::time::Duration::from_secs(1));
                }
            }
        }
    }
}

#[async_trait]
impl AttachedStreamSegmentSource for RoutedAttachedStreamSegmentSource {
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
    ) -> Result<usize, DurableStreamProducerError> {
        if self.mapping.handle != *handle {
            return Err(DurableStreamProducerError::InvalidHandle);
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
    ) -> Result<Vec<CommittedProducerStreamEvent>, DurableStreamProducerError> {
        if self.mapping.handle != *handle {
            return Err(DurableStreamProducerError::InvalidHandle);
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
        golem_common::serialization::deserialize(&payload)
            .map_err(DurableStreamProducerError::CorruptHistory)
    }

    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        _now_millis: u64,
        after: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, DurableStreamProducerError> {
        if self.mapping.handle != *handle {
            return Err(DurableStreamProducerError::InvalidHandle);
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
        golem_common::serialization::deserialize(&payload)
            .map_err(DurableStreamProducerError::CorruptHistory)
    }
}

impl RoutedStreamAttachmentControl {
    pub(crate) fn new(
        rpc: Arc<dyn Rpc>,
        mapping: StreamSessionMappingRecord,
        auth_ctx: AuthCtx,
    ) -> Self {
        Self {
            rpc,
            mapping,
            auth_ctx,
        }
    }

    async fn execute(
        &self,
        operation: StreamAttachmentControlOperation,
    ) -> Result<bool, DurableStreamProducerError> {
        self.rpc
            .control_durable_stream_attachment(
                StreamAttachmentControlRequest {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    mapping: Some(self.mapping.clone()),
                    operation,
                },
                &self.auth_ctx,
            )
            .await
            .map_err(|error| DurableStreamProducerError::Oplog(error.to_string()))
    }

    pub(crate) async fn cancel_stream(
        &self,
        key: StreamAttachmentKey,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    ) -> Result<bool, DurableStreamProducerError> {
        self.execute(StreamAttachmentControlOperation::Cancel {
            key,
            role,
            reason,
            details,
        })
        .await
    }
}

#[async_trait]
impl StreamAttachmentControl for RoutedStreamAttachmentControl {
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError> {
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
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError> {
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
                lease_expires_at_millis: Some(attachment_lease_expiry(now_millis)?),
            },
            replayed,
        })
    }

    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<StreamAttachmentView, DurableStreamProducerError> {
        self.execute(StreamAttachmentControlOperation::Detach { key: key.clone() })
            .await?;
        Ok(StreamAttachmentView {
            key: key.clone(),
            state: StreamAttachmentState::Active,
            lease_expires_at_millis: None,
        })
    }

    async fn renew_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError> {
        let replayed = self
            .execute(StreamAttachmentControlOperation::Renew {
                key: key.clone(),
                now_millis,
            })
            .await?;
        Ok(ProducerWriteOutcome {
            value: StreamAttachmentView {
                key,
                state: StreamAttachmentState::Active,
                lease_expires_at_millis: Some(attachment_lease_expiry(now_millis)?),
            },
            replayed,
        })
    }

    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKey,
        reason: StreamAttachmentFinalizationReason,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError> {
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
