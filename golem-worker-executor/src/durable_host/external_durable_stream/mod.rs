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

use crate::durable_host::authorization::targets::{http_target, secret_target};
use crate::durable_host::concurrent::{
    AccessClaimOptions, CallReplayOutcome, Cancellable, DurableCallSession, NotCancellable,
    authorize_live_permissions_at_serialized_access,
};
use crate::durable_host::secrets::types::SecretEntry;
use crate::durable_host::secrets::{
    canonical_config_key, canonical_secret_resource, environment_owner, secret_entry,
    validate_secret_value,
};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::agent::durable_streams as wit;
use crate::services::active_agents::MemoryGrant;
use crate::services::external_durable_stream::{
    append_memory_reservation, preflight_append, read_memory_reservation, validate_read,
};
use crate::services::{HasActiveAgents, HasExternalDurableStreamService, HasWorker};
use crate::workerctx::WorkerCtx;
use golem_common::model::card::SecretVerb;
use golem_common::model::oplog::host_functions::{
    GolemAgentDurableStreamReaderNew, GolemAgentDurableStreamReaderRead,
    GolemAgentDurableStreamWriterAppend, GolemAgentDurableStreamWriterNew, HostFunctionName,
};
use golem_common::model::oplog::payload::external_durable_stream::*;
use golem_common::model::oplog::payload::{
    HostRequestDurableStreamAppend, HostRequestDurableStreamRead,
    HostRequestDurableStreamReaderNew, HostRequestDurableStreamWriterNew,
    HostResponseDurableStreamAppend, HostResponseDurableStreamRead,
    HostResponseDurableStreamResource,
};
use golem_common::model::oplog::{DurableFunctionType, HostPayloadPair};
use golem_common::schema::SchemaValue;
use golem_schema::schema::wit::SecretHandleRep;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::time::Duration;
use wasmtime::component::{Accessor, HasSelf, Resource};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub struct DurableStreamReaderEntry {
    resource_id: String,
    options: DurableStreamReaderOptions,
    auth: Option<SecretEntry>,
}

impl DurableStreamReaderEntry {
    fn from_request(request: HostRequestDurableStreamReaderNew) -> anyhow::Result<Self> {
        Ok(Self {
            resource_id: request
                .options
                .resource_id(request.auth.as_ref())
                .map_err(anyhow::Error::msg)?,
            options: request.options,
            auth: request
                .auth
                .as_ref()
                .map(SecretEntry::from_snapshot)
                .transpose()?,
        })
    }

    fn read_request(&self, request: &HostRequestDurableStreamRead) -> DurableStreamReadRequest {
        DurableStreamReadRequest {
            url: self.options.url.clone(),
            checkpoint: request.checkpoint.clone(),
            mode: self.options.mode,
            transport: request.transport,
            content_type: request.content_type.clone(),
            timeout_ms: self.options.timeout_ms,
        }
    }
}

#[derive(Clone)]
pub struct DurableStreamWriterEntry {
    resource_id: String,
    options: DurableStreamWriterOptions,
    auth: Option<SecretEntry>,
}

impl DurableStreamWriterEntry {
    fn from_request(request: HostRequestDurableStreamWriterNew) -> anyhow::Result<Self> {
        Ok(Self {
            resource_id: request
                .options
                .resource_id(request.auth.as_ref())
                .map_err(anyhow::Error::msg)?,
            options: request.options,
            auth: request
                .auth
                .as_ref()
                .map(SecretEntry::from_snapshot)
                .transpose()?,
        })
    }

    fn append_request(
        &self,
        request: &HostRequestDurableStreamAppend,
    ) -> DurableStreamAppendRequest {
        DurableStreamAppendRequest {
            url: self.options.url.clone(),
            content_type: self.options.content_type.clone(),
            payload: request.payload.clone(),
            producer: DurableStreamProducer {
                id: self.options.producer_id.clone(),
                epoch: self.options.producer_epoch,
                sequence: request.sequence,
            },
            close: request.close,
            timeout_ms: self.options.timeout_ms,
        }
    }
}

fn validate_resource_id(expected: &str, recorded: &str) -> anyhow::Result<()> {
    if expected != recorded {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "matching durable stream resource identity",
            "different durable stream descriptor or pinned authentication",
        )
        .into());
    }
    Ok(())
}

impl<Ctx: WorkerCtx> wit::Host for DurableWorkerCtx<Ctx> {}

impl<Ctx: WorkerCtx> wit::HostDurableStreamReader for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        options: wit::DurableStreamReaderOptions,
        auth: Option<Resource<SecretHandleRep>>,
    ) -> anyhow::Result<Resource<DurableStreamReaderEntry>> {
        let request = HostRequestDurableStreamReaderNew {
            options: options.into(),
            auth: auth
                .as_ref()
                .map(|auth| secret_entry(self, auth).map(SecretEntry::to_snapshot))
                .transpose()?,
        };
        let mut entry = DurableStreamReaderEntry::from_request(request.clone())?;
        let mut call =
            DurableCallSession::<GolemAgentDurableStreamReaderNew, NotCancellable>::start(
                self,
                request,
                DurableFunctionType::ReadLocal,
            )
            .await?;
        if !call.is_live() {
            let recorded = call
                .recorded_request(self)
                .await
                .map_err(|error| call.trap(error))?;
            let restored = DurableStreamReaderEntry::from_request(recorded)
                .map_err(|error| call.trap(error))?;
            validate_resource_id(&entry.resource_id, &restored.resource_id)
                .map_err(|error| call.trap(error))?;
            entry = restored;
            match call.replay(self).await? {
                CallReplayOutcome::Replayed(response) => {
                    validate_resource_id(&entry.resource_id, &response.resource_id)?;
                    return Ok(self.table().push(entry)?);
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        call.complete(
            self,
            HostResponseDurableStreamResource {
                resource_id: entry.resource_id.clone(),
            },
        )
        .await?;
        Ok(self.table().push(entry)?)
    }

    async fn drop(&mut self, resource: Resource<DurableStreamReaderEntry>) -> anyhow::Result<()> {
        self.table().delete(resource)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> wit::HostDurableStreamWriter for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        options: wit::DurableStreamWriterOptions,
        auth: Option<Resource<SecretHandleRep>>,
    ) -> anyhow::Result<Resource<DurableStreamWriterEntry>> {
        let request = HostRequestDurableStreamWriterNew {
            options: options.into(),
            auth: auth
                .as_ref()
                .map(|auth| secret_entry(self, auth).map(SecretEntry::to_snapshot))
                .transpose()?,
        };
        let mut entry = DurableStreamWriterEntry::from_request(request.clone())?;
        let mut call =
            DurableCallSession::<GolemAgentDurableStreamWriterNew, NotCancellable>::start(
                self,
                request,
                DurableFunctionType::ReadLocal,
            )
            .await?;
        if !call.is_live() {
            let recorded = call
                .recorded_request(self)
                .await
                .map_err(|error| call.trap(error))?;
            let restored = DurableStreamWriterEntry::from_request(recorded)
                .map_err(|error| call.trap(error))?;
            validate_resource_id(&entry.resource_id, &restored.resource_id)
                .map_err(|error| call.trap(error))?;
            entry = restored;
            match call.replay(self).await? {
                CallReplayOutcome::Replayed(response) => {
                    validate_resource_id(&entry.resource_id, &response.resource_id)?;
                    return Ok(self.table().push(entry)?);
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        call.complete(
            self,
            HostResponseDurableStreamResource {
                resource_id: entry.resource_id.clone(),
            },
        )
        .await?;
        Ok(self.table().push(entry)?)
    }

    async fn drop(&mut self, resource: Resource<DurableStreamWriterEntry>) -> anyhow::Result<()> {
        self.table().delete(resource)?;
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> wit::HostDurableStreamReaderWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn read(
        accessor: &Accessor<U, Self>,
        resource: Resource<DurableStreamReaderEntry>,
        request: wit::DurableStreamReadRequest,
    ) -> anyhow::Result<Result<wit::DurableStreamBatch, wit::DurableStreamError>> {
        let (reader, limit) = accessor.with(|mut access| {
            let ctx = access.get();
            Ok::<_, anyhow::Error>((
                ctx.table().get(&resource)?.clone(),
                ctx.state.config.durable_stream.external_batch_max_size,
            ))
        })?;
        let recorded = HostRequestDurableStreamRead {
            resource_id: reader.resource_id.clone(),
            checkpoint: DurableStreamCheckpoint {
                offset: request.checkpoint.offset,
                cursor: request.checkpoint.cursor,
            },
            transport: match request.transport {
                wit::DurableStreamTransport::CatchUp => DurableStreamTransport::CatchUp,
                wit::DurableStreamTransport::LongPoll => DurableStreamTransport::LongPoll,
                wit::DurableStreamTransport::Sse => DurableStreamTransport::Sse,
            },
            content_type: request.content_type,
        };
        let request = reader.read_request(&recorded);
        let mut call = DurableCallSession::<GolemAgentDurableStreamReaderRead, Cancellable>::start_access_with_options(
            accessor,
            accessor.getter(),
            DurableFunctionType::ReadRemote,
            AccessClaimOptions {
                request_identity: Some(recorded.clone().into()),
                ..Default::default()
            },
            async move |_| Ok(recorded),
        ).await?;
        if !call.is_live() {
            match call.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(response) => {
                    return Ok(response.result.map(Into::into).map_err(Into::into));
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let service = accessor.with(|mut access| {
            access
                .get()
                .public_state
                .worker()
                .external_durable_streams()
        });
        let (result, _memory) = match live_attempt(
            accessor,
            &GolemAgentDurableStreamReaderRead::HOST_FUNCTION_NAME,
            validate_read(&request, limit),
            reader.auth,
            request.timeout_ms,
            read_memory_reservation(request.transport, limit),
            async |token| service.read_batch(&request, token, limit).await,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => return Err(call.trap(error)),
        };
        let response = call
            .complete_access(
                accessor,
                accessor.getter(),
                HostResponseDurableStreamRead { result },
            )
            .await?;
        Ok(response.result.map(Into::into).map_err(Into::into))
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> wit::HostDurableStreamWriterWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn append(
        accessor: &Accessor<U, Self>,
        resource: Resource<DurableStreamWriterEntry>,
        request: wit::DurableStreamAppendRequest,
    ) -> anyhow::Result<Result<wit::DurableStreamAppendReceipt, wit::DurableStreamError>> {
        let (writer, limit) = accessor.with(|mut access| {
            let ctx = access.get();
            Ok::<_, anyhow::Error>((
                ctx.table().get(&resource)?.clone(),
                ctx.state.config.durable_stream.external_batch_max_size,
            ))
        })?;
        let recorded = HostRequestDurableStreamAppend {
            resource_id: writer.resource_id.clone(),
            payload: match request.payload {
                wit::DurableStreamAppendPayload::Json(values) => {
                    DurableStreamAppendPayload::Json(values)
                }
                wit::DurableStreamAppendPayload::Bytes(bytes) => {
                    DurableStreamAppendPayload::Bytes(bytes)
                }
            },
            sequence: request.sequence,
            close: request.close,
        };
        let request = writer.append_request(&recorded);
        let mut call = DurableCallSession::<GolemAgentDurableStreamWriterAppend, Cancellable>::start_access_with_options(
            accessor,
            accessor.getter(),
            DurableFunctionType::WriteRemote,
            AccessClaimOptions {
                request_identity: Some(recorded.clone().into()),
                ..Default::default()
            },
            async move |_| Ok(recorded),
        ).await?;
        if !call.is_live() {
            match call.replay_access(accessor, accessor.getter()).await? {
                CallReplayOutcome::Replayed(response) => {
                    return Ok(response.result.map(Into::into).map_err(Into::into));
                }
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let service = accessor.with(|mut access| {
            access
                .get()
                .public_state
                .worker()
                .external_durable_streams()
        });
        let (result, _memory) = match live_attempt(
            accessor,
            &GolemAgentDurableStreamWriterAppend::HOST_FUNCTION_NAME,
            preflight_append(&request, limit),
            writer.auth,
            request.timeout_ms,
            append_memory_reservation(limit),
            async |token| service.append_batch(&request, token, limit).await,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => return Err(call.trap(error)),
        };
        let response = call
            .complete_access(
                accessor,
                accessor.getter(),
                HostResponseDurableStreamAppend { result },
            )
            .await?;
        Ok(response.result.map(Into::into).map_err(Into::into))
    }
}

async fn live_attempt<U: Send + 'static, Ctx: WorkerCtx, T>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    function_name: &HostFunctionName,
    url: Result<reqwest::Url, DurableStreamError>,
    auth: Option<SecretEntry>,
    timeout_ms: u64,
    reservation: Option<u64>,
    operation: impl AsyncFnOnce(Option<&str>) -> Result<T, DurableStreamError>,
) -> anyhow::Result<(Result<T, DurableStreamError>, Option<MemoryGrant>)> {
    let url = match url {
        Ok(url) => url,
        Err(error) => return Ok((Err(error), None)),
    };
    let permissions = accessor.with(|mut access| {
        let ctx = access.get();
        let mut targets = vec![http_target(url.as_str()).map_err(|_| denied())?.permission];
        if let Some(entry) = &auth {
            let key = canonical_config_key(entry).map_err(|_| denied())?;
            if ctx.entity_invocation_scope().is_some_and(|scope| {
                !scope
                    .activation()
                    .policy()
                    .secret_keys_revealable()
                    .contains(&key)
            }) {
                return Err(denied());
            }
            let resource = canonical_secret_resource(entry).map_err(|_| denied())?;
            targets.push(
                secret_target(environment_owner(ctx), SecretVerb::Reveal, &resource)
                    .map_err(|_| denied())?,
            );
        }
        Ok(targets)
    });
    let permissions = match permissions {
        Ok(permissions) => permissions,
        Err(error) => return Ok((Err(error), None)),
    };
    let _permit = match authorize_live_permissions_at_serialized_access(
        accessor,
        accessor.getter(),
        &permissions,
    )
    .await?
    {
        Ok(permit) => permit,
        Err(_) => return Ok((Err(denied()), None)),
    };
    let (active_agents, service, environment, interrupt) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            ctx.public_state.worker().active_agents(),
            ctx.state.environment_state_service.clone(),
            ctx.owner_component_metadata().environment_id,
            ctx.create_interrupt_signal(),
        )
    });
    // The codec owns the allocation estimate; the host holds admission through durable handoff.
    let reservation = reservation
        .ok_or_else(|| anyhow::anyhow!("External durable stream batch limit is too large"))?;
    let memory = match active_agents.try_acquire(reservation).await {
        Some(memory) => memory,
        None => {
            return Ok((
                Err(DurableStreamError::new(
                    DurableStreamErrorKind::Unavailable,
                    "Insufficient memory for durable stream batch",
                )),
                None,
            ));
        }
    };
    accessor.with(|mut access| {
        let ctx = access.get();
        ctx.state
            .check_and_increment_http_call_count(function_name)?;
        ctx.record_monthly_http_call(function_name)
    })?;
    let action = async {
        let token = if let Some(entry) = auth {
            let key = canonical_config_key(&entry).map_err(|_| denied())?;
            let secret = service
                .get_agent_secret_revision(environment, entry.secret_id, key, entry.pinned_revision)
                .await
                .map_err(|_| {
                    DurableStreamError::new(
                        DurableStreamErrorKind::Unavailable,
                        "Secret service unavailable",
                    )
                })?
                .ok_or_else(|| {
                    DurableStreamError::new(
                        DurableStreamErrorKind::InvalidRequest,
                        "Pinned authentication secret unavailable",
                    )
                })?;
            let value = secret.secret_value.as_ref().ok_or_else(|| {
                DurableStreamError::new(
                    DurableStreamErrorKind::InvalidRequest,
                    "Authentication secret has no value",
                )
            })?;
            validate_secret_value(&secret, value).map_err(|_| {
                DurableStreamError::new(
                    DurableStreamErrorKind::InvalidRequest,
                    "Authentication secret is invalid",
                )
            })?;
            match value {
                SchemaValue::String(token) => Some(token.clone()),
                _ => {
                    return Err(DurableStreamError::new(
                        DurableStreamErrorKind::InvalidRequest,
                        "Authentication requires a string secret",
                    ));
                }
            }
        } else {
            None
        };
        operation(token.as_deref()).await
    };
    let result = tokio::select! {
        result = tokio::time::timeout(Duration::from_millis(timeout_ms), action) => {
            result.unwrap_or_else(|_| Err(DurableStreamError::new(DurableStreamErrorKind::Timeout, "Durable stream attempt timed out")))
        }
        kind = interrupt => return Err(kind.into()),
    };
    Ok((result, Some(memory)))
}

fn denied() -> DurableStreamError {
    DurableStreamError::new(
        DurableStreamErrorKind::PermissionDenied,
        "Durable stream access denied",
    )
}

impl From<wit::DurableStreamReaderOptions> for DurableStreamReaderOptions {
    fn from(value: wit::DurableStreamReaderOptions) -> Self {
        Self {
            url: value.url,
            mode: match value.mode {
                wit::DurableStreamMode::Json => DurableStreamMode::Json,
                wit::DurableStreamMode::Bytes => DurableStreamMode::Bytes,
            },
            timeout_ms: value.timeout_ms,
        }
    }
}

impl From<wit::DurableStreamWriterOptions> for DurableStreamWriterOptions {
    fn from(value: wit::DurableStreamWriterOptions) -> Self {
        Self {
            url: value.url,
            content_type: value.content_type,
            producer_id: value.producer_id,
            producer_epoch: value.producer_epoch,
            timeout_ms: value.timeout_ms,
        }
    }
}

impl From<DurableStreamBatch> for wit::DurableStreamBatch {
    fn from(value: DurableStreamBatch) -> Self {
        Self {
            payload: value.payload,
            content_type: value.content_type,
            next: wit::DurableStreamCheckpoint {
                offset: value.next.offset,
                cursor: value.next.cursor,
            },
            up_to_date: value.up_to_date,
            closed: value.closed,
        }
    }
}

impl From<DurableStreamAppendReceipt> for wit::DurableStreamAppendReceipt {
    fn from(value: DurableStreamAppendReceipt) -> Self {
        Self {
            next_offset: value.next_offset,
            epoch: value.epoch,
            sequence: value.sequence,
            closed: value.closed,
        }
    }
}

impl From<DurableStreamError> for wit::DurableStreamError {
    fn from(value: DurableStreamError) -> Self {
        Self {
            kind: match value.kind {
                DurableStreamErrorKind::InvalidRequest => {
                    wit::DurableStreamErrorKind::InvalidRequest
                }
                DurableStreamErrorKind::PermissionDenied => {
                    wit::DurableStreamErrorKind::PermissionDenied
                }
                DurableStreamErrorKind::NotFound => wit::DurableStreamErrorKind::NotFound,
                DurableStreamErrorKind::Gone => wit::DurableStreamErrorKind::Gone,
                DurableStreamErrorKind::Closed => wit::DurableStreamErrorKind::Closed,
                DurableStreamErrorKind::SequenceConflict => {
                    wit::DurableStreamErrorKind::SequenceConflict
                }
                DurableStreamErrorKind::Fenced => wit::DurableStreamErrorKind::Fenced,
                DurableStreamErrorKind::ProducerDiverged => {
                    wit::DurableStreamErrorKind::ProducerDiverged
                }
                DurableStreamErrorKind::ProtocolError => wit::DurableStreamErrorKind::ProtocolError,
                DurableStreamErrorKind::PayloadTooLarge => {
                    wit::DurableStreamErrorKind::PayloadTooLarge
                }
                DurableStreamErrorKind::Timeout => wit::DurableStreamErrorKind::Timeout,
                DurableStreamErrorKind::Transport => wit::DurableStreamErrorKind::Transport,
                DurableStreamErrorKind::RateLimited => wit::DurableStreamErrorKind::RateLimited,
                DurableStreamErrorKind::Unavailable => wit::DurableStreamErrorKind::Unavailable,
            },
            message: value.message,
            retry_after_ms: value.retry_after_ms,
            producer_epoch: value.producer_epoch,
            expected_sequence: value.expected_sequence,
        }
    }
}
