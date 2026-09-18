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
    AccessClaimOptions, CallReplayOutcome, Cancellable, DurableCallSession,
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
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::host_functions::{
    GolemAgentAppendDurableStreamBatch, GolemAgentReadDurableStreamBatch,
};
use golem_common::model::oplog::payload::external_durable_stream::*;
use golem_common::model::oplog::payload::{
    HostRequestDurableStreamAppend, HostRequestDurableStreamRead, HostResponseDurableStreamAppend,
    HostResponseDurableStreamRead,
};
use golem_common::schema::SchemaValue;
use golem_schema::schema::wit::SecretHandleRep;
use std::time::Duration;
use wasmtime::component::{Accessor, HasSelf, Resource};

impl<Ctx: WorkerCtx> wit::Host for DurableWorkerCtx<Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> wit::HostWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn read_durable_stream_batch(
        accessor: &Accessor<U, Self>,
        request: wit::DurableStreamReadRequest,
        auth: Option<Resource<SecretHandleRep>>,
    ) -> anyhow::Result<Result<wit::DurableStreamBatch, wit::DurableStreamError>> {
        let request: DurableStreamReadRequest = request.into();
        let (auth, limit) = prepare(accessor, auth)?;
        let recorded = HostRequestDurableStreamRead {
            request: request.clone(),
            auth: auth.as_ref().map(SecretEntry::to_snapshot),
        };
        let mut call = DurableCallSession::<GolemAgentReadDurableStreamBatch, Cancellable>::start_access_with_options(
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
            validate_read(&request, limit),
            auth,
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

    async fn append_durable_stream_batch(
        accessor: &Accessor<U, Self>,
        request: wit::DurableStreamAppendRequest,
        auth: Option<Resource<SecretHandleRep>>,
    ) -> anyhow::Result<Result<wit::DurableStreamAppendReceipt, wit::DurableStreamError>> {
        let request: DurableStreamAppendRequest = request.into();
        let (auth, limit) = prepare(accessor, auth)?;
        let recorded = HostRequestDurableStreamAppend {
            request: request.clone(),
            auth: auth.as_ref().map(SecretEntry::to_snapshot),
        };
        let mut call = DurableCallSession::<GolemAgentAppendDurableStreamBatch, Cancellable>::start_access_with_options(
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
            preflight_append(&request, limit),
            auth,
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

fn prepare<U: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
    auth: Option<Resource<SecretHandleRep>>,
) -> anyhow::Result<(Option<SecretEntry>, usize)> {
    accessor.with(|mut access| {
        let ctx = access.get();
        let entry = auth
            .as_ref()
            .map(|auth| secret_entry(ctx, auth).cloned())
            .transpose()?;
        Ok((
            entry,
            ctx.state.config.durable_stream.external_batch_max_size,
        ))
    })
}

async fn live_attempt<U: Send + 'static, Ctx: WorkerCtx, T>(
    accessor: &Accessor<U, HasSelf<DurableWorkerCtx<Ctx>>>,
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
        ctx.state.check_and_increment_http_call_count()?;
        ctx.record_monthly_http_call()
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

impl From<wit::DurableStreamReadRequest> for DurableStreamReadRequest {
    fn from(value: wit::DurableStreamReadRequest) -> Self {
        Self {
            url: value.url,
            checkpoint: DurableStreamCheckpoint {
                offset: value.checkpoint.offset,
                cursor: value.checkpoint.cursor,
            },
            mode: match value.mode {
                wit::DurableStreamMode::Json => DurableStreamMode::Json,
                wit::DurableStreamMode::Bytes => DurableStreamMode::Bytes,
            },
            transport: match value.transport {
                wit::DurableStreamTransport::CatchUp => DurableStreamTransport::CatchUp,
                wit::DurableStreamTransport::LongPoll => DurableStreamTransport::LongPoll,
                wit::DurableStreamTransport::Sse => DurableStreamTransport::Sse,
            },
            content_type: value.content_type,
            timeout_ms: value.timeout_ms,
        }
    }
}

impl From<wit::DurableStreamAppendRequest> for DurableStreamAppendRequest {
    fn from(value: wit::DurableStreamAppendRequest) -> Self {
        Self {
            url: value.url,
            content_type: value.content_type,
            payload: match value.payload {
                wit::DurableStreamAppendPayload::Json(values) => {
                    DurableStreamAppendPayload::Json(values)
                }
                wit::DurableStreamAppendPayload::Bytes(bytes) => {
                    DurableStreamAppendPayload::Bytes(bytes)
                }
            },
            producer: DurableStreamProducer {
                id: value.producer.id,
                epoch: value.producer.epoch,
                sequence: value.producer.sequence,
            },
            close: value.close,
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
