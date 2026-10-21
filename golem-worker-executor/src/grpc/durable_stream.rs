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

use super::{
    AgentFingerprint, AgentId, AuthCtx, DomainExportResult, DurableStreamAttachmentControlRequest,
    DurableStreamReadRequest, DurableStreamRemoteError, DurableStreamSegmentReadRequest,
    EnvironmentId, HasAll, StreamAttachmentControlRequest, UsesAllDeps, Worker, WorkerCtx,
    WorkerExecutorError, WorkerExecutorImpl, durable_stream_attachment_control_response,
    extract_owned_agent_id, golem,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn control_durable_stream_attachment_internal(
        &self,
        request: DurableStreamAttachmentControlRequest,
    ) -> Result<
        durable_stream_attachment_control_response::Result,
        DurableStreamRemoteError<WorkerExecutorError>,
    > {
        let owned_agent_id = extract_owned_agent_id(
            &request,
            |request| &request.producer_agent_id,
            |request| &request.producer_environment_id,
        )?;
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or_else(|| WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        if !matches!(auth_ctx, AuthCtx::System | AuthCtx::Agent(_)) {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream attachment control requires an authenticated internal caller",
            )
            .into());
        }
        if let Some(control) = request.export_control {
            if !matches!(auth_ctx, AuthCtx::System)
                || !request.payload.is_empty()
                || request.consumer_agent_id.is_some()
                || request.consumer_environment_id.is_some()
                || request.expected_consumer_fingerprint.is_some()
            {
                return Err(WorkerExecutorError::invalid_request(
                    "export stream control requires a system caller and no attachment fields",
                )
                .into());
            }
            let result =
                match Worker::<Ctx>::find_durable_stream_worker(self, &owned_agent_id).await? {
                    Some((worker, _response_lease)) => {
                        worker.control_export_stream(control.into()).await?
                    }
                    None => DomainExportResult::NotFound,
                };
            let result = golem::workerexecutor::v1::ExportStreamControlResult::from(result);
            return Ok(
                durable_stream_attachment_control_response::Result::ExportResult(result.into()),
            );
        }
        let control: StreamAttachmentControlRequest =
            golem_common::serialization::deserialize(&request.payload)
                .map_err(WorkerExecutorError::invalid_request)?;
        let consumer_agent_id: AgentId = request
            .consumer_agent_id
            .ok_or_else(|| WorkerExecutorError::invalid_request("consumer_agent_id not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let consumer_environment_id: EnvironmentId = request
            .consumer_environment_id
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request("consumer_environment_id not found")
            })?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        let expected_consumer_fingerprint = AgentFingerprint(
            request
                .expected_consumer_fingerprint
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request("expected_consumer_fingerprint not found")
                })?
                .into(),
        );
        let key = control.operation.key();
        let target_matches = if control.operation.targets_consumer() {
            key.consumer_environment_id == owned_agent_id.environment_id
                && key.consumer == owned_agent_id.agent_id
        } else {
            key.producer_environment_id == owned_agent_id.environment_id
                && key.producer == owned_agent_id.agent_id
        };
        if !target_matches
            || key.consumer != consumer_agent_id
            || control.operation.key().consumer_environment_id != consumer_environment_id
            || key.expected_consumer_fingerprint != expected_consumer_fingerprint
        {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream attachment target does not match the routed worker",
            )
            .into());
        }
        let (worker, _response_lease) =
            Worker::<Ctx>::find_durable_stream_worker(self, &owned_agent_id)
                .await?
                .ok_or_else(|| WorkerExecutorError::worker_not_found(owned_agent_id.agent_id()))?;
        // A producer that is being deleted is unavailable rather than failed: the caller retries
        // until the deletion either finishes (the producer is then not found, which completes a
        // consumer-side finalization) or fails and leaves the producer resident again. A consumer
        // that is being deleted no longer accepts overlays; its own deletion settles its topology.
        let targets_consumer = control.operation.targets_consumer();
        let deleting = |message: &str| {
            if targets_consumer {
                DurableStreamRemoteError::Other(WorkerExecutorError::invalid_request(message))
            } else {
                DurableStreamRemoteError::Unavailable
            }
        };
        let scope = crate::worker::tasks::TaskScope::default();
        scope.bind(&worker.tasks).map_err(deleting)?;
        scope
            .run(worker.control_durable_stream_attachment(control))
            .await
            .ok_or_else(|| deleting("Worker is being deleted"))?
            .map(durable_stream_attachment_control_response::Result::Replayed)
    }

    pub(super) async fn read_durable_stream_segment_internal(
        &self,
        request: DurableStreamSegmentReadRequest,
    ) -> Result<Vec<u8>, DurableStreamRemoteError<WorkerExecutorError>> {
        let owned_agent_id = extract_owned_agent_id(
            &request,
            |request| &request.producer_agent_id,
            |request| &request.producer_environment_id,
        )?;
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or_else(|| WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        if !matches!(auth_ctx, AuthCtx::System | AuthCtx::Agent(_)) {
            return Err(WorkerExecutorError::invalid_request(
                "durable stream segment reads require an authenticated internal caller",
            )
            .into());
        }
        let read: DurableStreamReadRequest =
            golem_common::serialization::deserialize(&request.payload)
                .map_err(WorkerExecutorError::invalid_request)?;
        match &read {
            DurableStreamReadRequest::AttachedConsumer(read) => {
                let consumer_agent_id: AgentId = request
                    .consumer_agent_id
                    .ok_or_else(|| {
                        WorkerExecutorError::invalid_request("consumer_agent_id not found")
                    })?
                    .try_into()
                    .map_err(WorkerExecutorError::invalid_request)?;
                let consumer_environment_id: EnvironmentId = request
                    .consumer_environment_id
                    .ok_or_else(|| {
                        WorkerExecutorError::invalid_request("consumer_environment_id not found")
                    })?
                    .try_into()
                    .map_err(WorkerExecutorError::invalid_request)?;
                let expected_consumer_fingerprint = AgentFingerprint(
                    request
                        .expected_consumer_fingerprint
                        .ok_or_else(|| {
                            WorkerExecutorError::invalid_request(
                                "expected_consumer_fingerprint not found",
                            )
                        })?
                        .into(),
                );
                let key = &read.attachment;
                if key.producer_environment_id != owned_agent_id.environment_id
                    || key.producer != owned_agent_id.agent_id
                    || key.consumer != consumer_agent_id
                    || key.consumer_environment_id != consumer_environment_id
                    || key.expected_consumer_fingerprint != expected_consumer_fingerprint
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "durable stream segment target does not match its producer or consumer",
                    )
                    .into());
                }
            }
            DurableStreamReadRequest::AuthorizedExport(read) => {
                auth_ctx
                    .authorize_system_only("read authorized durable stream export")
                    .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
                if request.consumer_agent_id.is_some()
                    || request.consumer_environment_id.is_some()
                    || request.expected_consumer_fingerprint.is_some()
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "authorized durable stream exports must not carry consumer attachment identity",
                    ).into());
                }
                if read.handle.producer_environment_id != owned_agent_id.environment_id
                    || read.handle.producer != owned_agent_id.agent_id
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "durable stream handle target does not match the routed producer",
                    )
                    .into());
                }
            }
        }
        let (worker, _response_lease) =
            Worker::<Ctx>::find_durable_stream_worker(self, &owned_agent_id)
                .await?
                .ok_or_else(|| WorkerExecutorError::worker_not_found(owned_agent_id.agent_id()))?;
        match read {
            DurableStreamReadRequest::AttachedConsumer(read) => {
                let events = worker.read_durable_stream_segment(*read).await?;
                golem_common::serialization::serialize(&events)
                    .map_err(WorkerExecutorError::runtime)
                    .map_err(Into::into)
            }
            DurableStreamReadRequest::AuthorizedExport(read) => {
                worker.read_durable_stream_by_handle(*read).await
            }
        }
    }
}
