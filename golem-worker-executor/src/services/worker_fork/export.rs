// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::{DefaultWorkerFork, admission::Admission, stream_cut};
use crate::durable_host::durable_stream::DurableStreamStore;
use crate::services::HasOplog;
use crate::services::oplog::{CommitLevel, OplogService, OplogServiceOps};
use crate::storage::keyvalue::KeyValueStorageNamespace;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ForkStreamSlotRejection, ForkStreamSlotRequest, ForkStreamSlotResponse, ForkStreamSlotSuccess,
    fork_stream_slot_rejection::Reason, fork_stream_slot_response,
};
use golem_common::model::agent::AgentMode;
use golem_common::model::durable_stream::{
    StreamExportFork, StreamForkCutRecord, StreamId, StreamItemsPayload, StreamOffset,
    StreamSessionRecord,
};
use golem_common::model::oplog::OplogEntry;
use golem_common::model::{AgentFingerprint, AgentId, OplogIndex, OwnedAgentId, Timestamp};
use golem_common::schema::SchemaGraph;
use golem_common::serialization::{deserialize, serialize};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, desert_rust::BinaryCodec)]
pub(super) struct Candidate {
    pub export: StreamExportFork,
    pub horizon: OplogIndex,
    pub cut: OplogIndex,
    pub selected: StreamId,
    pub retained_through: Option<StreamOffset>,
    pub initial: Option<StreamItemsPayload>,
}

enum Error {
    Rejected(ForkStreamSlotRejection),
    Worker(WorkerExecutorError),
}

impl From<WorkerExecutorError> for Error {
    fn from(error: WorkerExecutorError) -> Self {
        Self::Worker(error)
    }
}

fn reject(reason: Reason) -> Error {
    Error::Rejected(ForkStreamSlotRejection {
        reason: reason as i32,
        copied_bytes: 0,
        retry_after_seconds: 0,
    })
}

pub(super) async fn fork<Ctx: WorkerCtx>(
    service: &DefaultWorkerFork<Ctx>,
    request: ForkStreamSlotRequest,
) -> ForkStreamSlotResponse {
    let result = match execute(service, &request).await {
        Ok(success) => fork_stream_slot_response::Result::Success(success),
        Err(Error::Rejected(rejection)) => fork_stream_slot_response::Result::Rejected(rejection),
        Err(Error::Worker(error)) => fork_stream_slot_response::Result::Failure(error.into()),
    };
    ForkStreamSlotResponse {
        result: Some(result),
    }
}

async fn execute<Ctx: WorkerCtx>(
    service: &DefaultWorkerFork<Ctx>,
    request: &ForkStreamSlotRequest,
) -> Result<ForkStreamSlotSuccess, Error> {
    let source_id: AgentId = request
        .source_agent_id
        .clone()
        .and_then(|id| id.try_into().ok())
        .ok_or_else(|| WorkerExecutorError::invalid_request("missing source agent"))?;
    let target_id: AgentId = request
        .target_agent_id
        .clone()
        .and_then(|id| id.try_into().ok())
        .ok_or_else(|| WorkerExecutorError::invalid_request("missing target agent"))?;
    let environment_id = request
        .environment_id
        .and_then(|id| id.try_into().ok())
        .ok_or_else(|| WorkerExecutorError::invalid_request("missing environment"))?;
    let auth: AuthCtx = request
        .auth_ctx
        .clone()
        .ok_or_else(|| WorkerExecutorError::invalid_request("missing auth context"))?
        .try_into()
        .map_err(WorkerExecutorError::invalid_request)?;
    let source = OwnedAgentId::new(environment_id, &source_id);
    let target = OwnedAgentId::new(environment_id, &target_id);
    if source_id.component_id != target_id.component_id || source == target {
        return Err(reject(Reason::Conflict));
    }
    service.shard_service.check_worker(&source_id)?;

    // The target receipt wins before consulting the source: its tail, tombstone or even
    // existence may have changed since the successful request whose response was lost.
    if let Some(receipt) = creation_record(service.oplog_service.as_ref(), &target).await? {
        let export = receipt
            .export
            .as_ref()
            .filter(|export| matches_request(export, request, &source_id))
            .ok_or_else(|| reject(Reason::Conflict))?;
        resume(service, &target_id, &auth).await?;
        return Ok(response(export, receipt.cut_index, true));
    }
    if service
        .oplog_service
        .exists(&target, AgentMode::Durable)
        .await
        || service
            .oplog_service
            .exists(&target, AgentMode::Ephemeral)
            .await
    {
        return Err(reject(Reason::Conflict));
    }
    let metadata = service
        .worker_service
        .get(&source)
        .await?
        .ok_or_else(|| reject(Reason::NotFound))?
        .initial_worker_metadata;
    let namespace = KeyValueStorageNamespace::ExportForkAdmissions {
        environment_id,
        agent_id: source_id.clone(),
        fingerprint: metadata.fingerprint,
    };
    match service
        .export_fork_admission
        .check(
            namespace.clone(),
            &target_id,
            &request.session,
            request.max_forks_per_session,
            request.max_forks_per_second,
            Timestamp::now_utc().to_millis(),
        )
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    {
        Some(Admission::LimitReached) => return Err(reject(Reason::Conflict)),
        Some(Admission::RateLimited {
            retry_after_seconds,
        }) => {
            return Err(Error::Rejected(ForkStreamSlotRejection {
                reason: Reason::RateLimited as i32,
                copied_bytes: 0,
                retry_after_seconds,
            }));
        }
        _ => {}
    }
    let saved = service
        .export_fork_admission
        .candidate(namespace.clone(), &target_id)
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
    let mut candidate = match saved {
        Some(bytes) => {
            let candidate: Candidate = deserialize(&bytes).map_err(WorkerExecutorError::runtime)?;
            if !matches_request(&candidate.export, request, &source_id) {
                return Err(reject(Reason::Conflict));
            }
            candidate
        }
        None => prepare_candidate(service, request, &source, metadata.fingerprint).await?,
    };
    let identity = (
        source.clone(),
        metadata.fingerprint,
        (
            request.session.clone(),
            request.slot.clone(),
            request.expected_method.clone(),
        ),
        (
            request.source_path.clone(),
            request.fork_offset.clone(),
            request.sub_offset,
        ),
        (
            candidate.export.content_type.clone(),
            candidate.export.initial_content_hash.clone(),
            request.closed,
        ),
    );
    let hash =
        *blake3::hash(&serialize(&identity).map_err(WorkerExecutorError::runtime)?).as_bytes();
    let mut stage_id = Uuid::new_v4();
    let result = async {
        // Stage before charging limits. A rejected byte budget never consumes admission.
        let (mut oplog, copied_bytes) = stage(
            service,
            &source,
            &target_id,
            metadata.created_by,
            stage_id,
            hash,
            &candidate,
            request.max_copied_bytes,
        )
        .await?;
        let admission = service
            .export_fork_admission
            .reserve(
                namespace,
                target_id.clone(),
                request.session.clone(),
                hash.to_vec(),
                serialize(&candidate).map_err(WorkerExecutorError::runtime)?,
                request.max_forks_per_session,
                request.max_forks_per_second,
                Timestamp::now_utc().to_millis(),
            )
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        match admission {
            Admission::Reserved {
                candidate: winner, ..
            } => {
                let winner: Candidate =
                    deserialize(&winner).map_err(WorkerExecutorError::runtime)?;
                if winner != candidate {
                    drop(oplog);
                    service
                        .oplog_service
                        .discard_staged(&target, AgentMode::Durable, stage_id)
                        .await
                        .map_err(WorkerExecutorError::runtime)?;
                    stage_id = Uuid::new_v4();
                    candidate = winner;
                    (oplog, _) = stage(
                        service,
                        &source,
                        &target_id,
                        metadata.created_by,
                        stage_id,
                        hash,
                        &candidate,
                        request.max_copied_bytes,
                    )
                    .await?;
                }
            }
            Admission::Conflict | Admission::LimitReached => return Err(reject(Reason::Conflict)),
            Admission::RateLimited {
                retry_after_seconds,
            } => {
                return Err(Error::Rejected(ForkStreamSlotRejection {
                    reason: Reason::RateLimited as i32,
                    copied_bytes,
                    retry_after_seconds,
                }));
            }
        }
        oplog.commit(CommitLevel::Always).await;
        let last = oplog.current_oplog_index().await;
        drop(oplog);
        let target_lifecycle = service.oplog_service.lock_lifecycle(&target.agent_id).await;
        let published = service
            .oplog_service
            .publish_staged(&target, AgentMode::Durable, stage_id, last)
            .await;
        let result = match published {
            Ok(true) => Ok(response(&candidate.export, candidate.cut, false)),
            outcome => {
                if let Some(receipt) =
                    creation_record(service.oplog_service.as_ref(), &target).await?
                {
                    let export = receipt
                        .export
                        .as_ref()
                        .filter(|export| matches_request(export, request, &source_id))
                        .ok_or_else(|| reject(Reason::Conflict))?;
                    Ok(response(export, receipt.cut_index, true))
                } else {
                    Err(
                        WorkerExecutorError::runtime(outcome.err().unwrap_or_else(|| {
                            "Fork publication lost target before reconciliation".into()
                        }))
                        .into(),
                    )
                }
            }
        };
        drop(target_lifecycle);
        result
    }
    .await;
    let cleanup = service
        .oplog_service
        .discard_staged(&target, AgentMode::Durable, stage_id)
        .await;
    let success = result?;
    cleanup.map_err(WorkerExecutorError::runtime)?;
    resume(service, &target_id, &auth).await?;
    Ok(success)
}

async fn resume<Ctx: WorkerCtx>(
    service: &DefaultWorkerFork<Ctx>,
    target: &AgentId,
    auth: &AuthCtx,
) -> Result<(), WorkerExecutorError> {
    match service.worker_proxy.resume(target, true, auth).await {
        Ok(())
        | Err(crate::services::worker_proxy::WorkerProxyError::InternalError(
            WorkerExecutorError::PreviousInvocationFailed { .. }
            | WorkerExecutorError::PreviousInvocationExited,
        )) => Ok(()),
        Err(error) => Err(WorkerExecutorError::failed_to_resume_worker(
            target.clone(),
            error.into(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn stage<Ctx: WorkerCtx>(
    service: &DefaultWorkerFork<Ctx>,
    source: &OwnedAgentId,
    target: &AgentId,
    account: golem_common::model::account::AccountId,
    stage_id: Uuid,
    hash: [u8; 32],
    candidate: &Candidate,
    max_bytes: u64,
) -> Result<(std::sync::Arc<dyn crate::services::oplog::Oplog>, u64), Error> {
    service
        .copy_source_oplog(
            account,
            source,
            target,
            candidate.cut,
            candidate.export.source_fingerprint,
            stage_id,
            hash,
            Some((candidate.selected, candidate.retained_through)),
            Some(max_bytes),
            Some(candidate),
        )
        .await
        .map_err(|error| {
            if error.to_string().contains("fork copied bytes") {
                Error::Rejected(ForkStreamSlotRejection {
                    reason: Reason::TooLarge as i32,
                    copied_bytes: max_bytes.saturating_add(1),
                    retry_after_seconds: 0,
                })
            } else {
                Error::Worker(error)
            }
        })
}

async fn prepare_candidate<Ctx: WorkerCtx>(
    service: &DefaultWorkerFork<Ctx>,
    request: &ForkStreamSlotRequest,
    source: &OwnedAgentId,
    fingerprint: AgentFingerprint,
) -> Result<Candidate, Error> {
    let worker = Worker::find_durable_stream_worker(service, source)
        .await?
        .ok_or_else(|| reject(Reason::NotFound))?;
    worker
        .commit_oplog_and_update_state(CommitLevel::Always)
        .await;
    let slot = worker
        .resolve_export_fork_slot(&request.session, &request.slot, &request.expected_method)
        .await?
        .ok_or_else(|| reject(Reason::NotFound))?;
    if slot.tombstoned {
        return Err(reject(Reason::Conflict));
    }
    let handle = slot.handle.ok_or_else(|| reject(Reason::NotFound))?;
    let snapshot = slot
        .snapshot
        .ok_or_else(|| WorkerExecutorError::runtime("missing stream snapshot"))?;
    let content_type = if slot.bytes {
        "application/octet-stream"
    } else {
        "application/json"
    };
    if request
        .content_type
        .as_deref()
        .is_some_and(|ct| ct != content_type)
    {
        return Err(reject(Reason::Conflict));
    }
    if (!request.initial_content.is_empty() || request.closed) && !slot.writable {
        return Err(reject(Reason::ReadOnly));
    }
    let initial = initial_payload(&slot.graph, slot.bytes, &request.initial_content)?;
    let requested_offset = request
        .fork_offset
        .as_deref()
        .map(|bytes| {
            <[u8; 24]>::try_from(bytes)
                .ok()
                .and_then(|bytes| StreamOffset::from_bytes(bytes).ok())
                .ok_or_else(|| reject(Reason::InvalidOffset))
        })
        .transpose()?;
    let origin = StreamOffset::new(OplogIndex::NONE, 0);
    let anchor = requested_offset.unwrap_or_else(|| {
        snapshot
            .terminal
            .or_else(|| {
                snapshot
                    .batches
                    .last()
                    .map(|(index, count)| StreamOffset::new(*index, count - 1))
            })
            .unwrap_or(origin)
    });
    let max_sub = if slot.bytes {
        if anchor == origin {
            snapshot
                .batches
                .first()
                .map_or(0, |(_, count)| u64::from(*count))
        } else {
            snapshot
                .batches
                .iter()
                .enumerate()
                .find(|(_, (index, _))| *index == anchor.producer_oplog_index())
                .map_or(0, |(position, (_, count))| {
                    if anchor.sub_index().checked_add(1) == Some(*count) {
                        snapshot
                            .batches
                            .get(position + 1)
                            .map_or(0, |(_, count)| u64::from(*count))
                    } else {
                        u64::from(count.saturating_sub(anchor.sub_index().saturating_add(1)))
                    }
                })
        }
    } else {
        u64::MAX
    };
    let mut cut = stream_cut::resolve_stream_fork_cut(
        snapshot.registration_index,
        &snapshot.batches,
        snapshot.terminal,
        Some(anchor),
        request.sub_offset,
        max_sub,
    )
    .map_err(|_| reject(Reason::InvalidOffset))?;
    if cut.retained_items == 0 {
        // Input registration is part of an atomic acceptance batch. An empty prefix
        // retains that batch, including its queued invocation, but no stream items.
        let oplog = worker.oplog();
        while crate::worker::cut_point::streaming_acceptance_spans_cut(
            oplog.as_ref(),
            cut.oplog_index,
            snapshot.horizon,
        )
        .await
        .map_err(WorkerExecutorError::runtime)?
        {
            cut.oplog_index = cut.oplog_index.next();
        }
    }
    Ok(Candidate {
        export: StreamExportFork {
            source: source.agent_id.clone(),
            source_environment_id: source.environment_id,
            source_fingerprint: fingerprint,
            source_path: request.source_path.clone(),
            session: request.session.clone(),
            slot: request.slot.clone(),
            expected_method: request.expected_method.clone(),
            requested_offset,
            anchor: Some(anchor),
            sub_offset: request.sub_offset,
            content_type: content_type.into(),
            initial_content_hash: blake3::hash(&request.initial_content).as_bytes().to_vec(),
            closed: request.closed,
        },
        horizon: snapshot.horizon,
        cut: cut.oplog_index,
        selected: handle.stream_id,
        retained_through: cut.last_item_offset,
        initial,
    })
}

pub(crate) fn response(
    export: &StreamExportFork,
    cut: OplogIndex,
    replayed: bool,
) -> ForkStreamSlotSuccess {
    ForkStreamSlotSuccess {
        replayed,
        source_path: export.source_path.clone(),
        fork_offset: export
            .anchor
            .map(|offset| offset.0.to_vec())
            .unwrap_or_default(),
        sub_offset: export.sub_offset,
        oplog_index: cut.as_u64(),
    }
}

pub(super) fn matches_request(
    export: &StreamExportFork,
    request: &ForkStreamSlotRequest,
    source: &AgentId,
) -> bool {
    export.source == *source
        && request.environment_id.and_then(|id| id.try_into().ok())
            == Some(export.source_environment_id)
        && export.source_path == request.source_path
        && export.session == request.session
        && export.slot == request.slot
        && export.expected_method == request.expected_method
        && export.requested_offset.map(|offset| offset.0.to_vec()) == request.fork_offset
        && export.sub_offset == request.sub_offset
        && request
            .content_type
            .as_ref()
            .is_none_or(|ct| ct == &export.content_type)
        && export.initial_content_hash == blake3::hash(&request.initial_content).as_bytes()
        && export.closed == request.closed
}

/// The first marker authored for this incarnation is its immutable creation receipt, even
/// when later Revert entries hide it from the active stream projection.
pub(crate) async fn creation_record(
    service: &dyn OplogService,
    target: &OwnedAgentId,
) -> Result<Option<StreamForkCutRecord>, WorkerExecutorError> {
    let mode = AgentMode::Durable;
    let horizon = service.get_last_index(target, mode).await;
    if horizon == OplogIndex::NONE {
        return Ok(None);
    }
    let initial = service
        .read_source(target, mode, OplogIndex::INITIAL, 1)
        .await;
    let Some(OplogEntry::Create { instance_id, .. }) = initial.get(&OplogIndex::INITIAL) else {
        return Ok(None);
    };
    let mut covered = OplogIndex::INITIAL;
    while covered < horizon {
        let count = (horizon.as_u64() - covered.as_u64()).min(1024);
        let entries = service
            .read_source(target, mode, covered.next(), count)
            .await;
        if entries.len() as u64 != count {
            return Err(WorkerExecutorError::runtime(
                "incomplete fork creation history",
            ));
        }
        for (index, entry) in entries {
            covered = index;
            if let OplogEntry::StreamSession { record, .. } = entry {
                let record = service
                    .download_payload(target, mode, record)
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
                if let StreamSessionRecord::ForkCut(cut) = record
                    && cut.target == target.agent_id
                    && cut.target_environment_id == target.environment_id
                    && cut.target_fingerprint.0 == *instance_id
                    && cut.revert.is_none()
                {
                    return Ok(Some(cut));
                }
            }
        }
    }
    Ok(None)
}

pub(super) fn initial_payload(
    graph: &SchemaGraph,
    binary: bool,
    body: &[u8],
) -> Result<Option<StreamItemsPayload>, WorkerExecutorError> {
    if body.is_empty() {
        return Ok(None);
    }
    let payload = if binary {
        StreamItemsPayload::PackedU8(body.to_vec())
    } else {
        let json: serde_json::Value = serde_json::from_slice(body)
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
        let values = match json {
            serde_json::Value::Array(values) => values,
            value => vec![value],
        };
        if values.is_empty() {
            return Ok(None);
        }
        let mut encoded = Vec::with_capacity(values.len());
        for json in values {
            let value =
                golem_schema::schema::render::from_untrusted_json_value(graph, &graph.root, &json)
                    .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
            let value: golem_api_grpc::proto::golem::schema::SchemaValue = value
                .try_into()
                .map_err(WorkerExecutorError::invalid_request)?;
            encoded.push(value.encode_to_vec());
        }
        StreamItemsPayload::Values(encoded)
    };
    DurableStreamStore::validate_external_input(Some(&payload))
        .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
    Ok(Some(payload))
}
