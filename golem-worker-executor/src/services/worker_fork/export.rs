// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::{DefaultWorkerFork, admission, admission::Admission, stream_cut};
use crate::durable_host::durable_stream::DurableStreamStore;
use crate::services::HasOplog;
use crate::services::oplog::{CommitLevel, OplogOps, OplogService, OplogServiceOps};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ForkStreamSlotRejection, ForkStreamSlotRequest, ForkStreamSlotResponse, ForkStreamSlotSuccess,
    fork_stream_slot_rejection::Reason, fork_stream_slot_response,
};
use golem_common::model::agent::AgentMode;
use golem_common::model::durable_stream::{
    DURABLE_STREAM_FORMAT_VERSION, StreamExportFork, StreamExportForkInitializedRecord,
    StreamForkCutRecord, StreamItemsPayload, StreamOffset, StreamSessionExpiryPolicy,
    StreamSessionRecord,
};
use golem_common::model::oplog::OplogEntry;
use golem_common::model::{
    AgentFingerprint, AgentId, IdempotencyKey, OplogIndex, OwnedAgentId, Timestamp,
};
use golem_common::schema::SchemaGraph;
use golem_common::serialization::serialize;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use uuid::Uuid;

pub(super) use golem_common::model::durable_stream::StreamExportForkCandidate as Candidate;

#[derive(Debug)]
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

    if let Some(receipt) = creation_receipt(service.oplog_service.as_ref(), &target).await? {
        if !receipt.live {
            return Err(reject(Reason::NotFound));
        }
        // The immutable target receipt is authoritative for a retry. Do not consult current
        // source state: the source may have advanced, expired or been deleted after publication.
        let export = matching_receipt_export(&receipt, request, &source_id)?;
        resume(service, &target_id, &auth).await?;
        return Ok(response(
            export,
            receipt.cut.cut_index,
            true,
            &receipt.initialized.session_key,
        ));
    }
    let source_metadata = service
        .worker_service
        .get(&source)
        .await?
        .ok_or_else(|| reject(Reason::NotFound))?;
    let metadata = source_metadata.initial_worker_metadata.clone();
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
    let worker = Worker::find_durable_stream_worker(service, &source)
        .await?
        .ok_or_else(|| reject(Reason::NotFound))?;
    let slot = worker
        .resolve_export_fork_slot(&request.session, &request.slot, &request.expected_method)
        .await?;
    let Some(slot) = slot else {
        return Err(reject(Reason::NotFound));
    };
    if slot.tombstoned {
        return Err(reject(Reason::Conflict));
    }
    let status = worker.get_export_fork_status().await?;
    match admission::check(
        &status.export_fork_admissions,
        &target_id,
        &request.session,
        request.max_forks_per_session,
        request.max_forks_per_second,
        Timestamp::now_utc().to_millis(),
    ) {
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
    let saved = status.export_fork_admissions.reservations.get(&target_id);
    let mut candidate = match saved {
        Some(record) => {
            let candidate = worker
                .read_export_fork_candidate(record.oplog_index)
                .await?;
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
            candidate.export.target_expiry_policy,
        ),
    );
    let hash =
        *blake3::hash(&serialize(&identity).map_err(WorkerExecutorError::runtime)?).as_bytes();
    let mut stage_id = Uuid::new_v4();
    let result = async {
        // Stage before charging limits. A rejected byte budget never consumes admission.
        let (mut oplog, copied_bytes, mut target_fingerprint) = stage(
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
        let admission = worker
            .reserve_export_fork(
                target_id.clone(),
                hash.to_vec(),
                candidate.clone(),
                request.max_forks_per_session,
                request.max_forks_per_second,
            )
            .await?;
        let winner = match admission {
            Admission::Reserved => candidate.clone(),
            Admission::Existing { oplog_index } => {
                worker.read_export_fork_candidate(oplog_index).await?
            }
            Admission::Conflict | Admission::LimitReached => return Err(reject(Reason::Conflict)),
            Admission::Expired => return Err(reject(Reason::NotFound)),
            Admission::InvalidExpiry => return Err(reject(Reason::Conflict)),
            Admission::RateLimited {
                retry_after_seconds,
            } => {
                return Err(Error::Rejected(ForkStreamSlotRejection {
                    reason: Reason::RateLimited as i32,
                    copied_bytes,
                    retry_after_seconds,
                }));
            }
        };
        if winner != candidate {
            drop(oplog);
            service
                .oplog_service
                .discard_staged(&target, AgentMode::Durable, stage_id)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            stage_id = Uuid::new_v4();
            candidate = winner;
            (oplog, _, target_fingerprint) = stage(
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
        oplog.commit(CommitLevel::Always).await;
        let target_lifecycle = service.oplog_service.lock_lifecycle(&target.agent_id).await;
        let expiry_deadline_millis = admitted_publication_deadline(&candidate);
        append_target_initialization(oplog.as_ref(), &candidate, hash, expiry_deadline_millis)
            .await?;
        schedule_expiry(
            service,
            &target,
            target_fingerprint,
            &candidate,
            expiry_deadline_millis,
        )
        .await?;
        oplog.commit(CommitLevel::Always).await;
        let last = oplog.current_oplog_index().await;
        drop(oplog);
        let published = service
            .oplog_service
            .publish_staged(&target, AgentMode::Durable, stage_id, last)
            .await;
        let result = match published {
            Ok(true) => Ok(response(
                &candidate.export,
                candidate.cut,
                false,
                &candidate.target_session_key,
            )),
            outcome => {
                if let Some(receipt) =
                    creation_receipt(service.oplog_service.as_ref(), &target).await?
                {
                    if !receipt.live {
                        return Err(reject(Reason::Conflict));
                    }
                    let export = matching_receipt_export(&receipt, request, &source_id)?;
                    Ok(response(
                        export,
                        receipt.cut.cut_index,
                        true,
                        &receipt.initialized.session_key,
                    ))
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
) -> Result<
    (
        std::sync::Arc<dyn crate::services::oplog::Oplog>,
        u64,
        AgentFingerprint,
    ),
    Error,
> {
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

fn admitted_publication_deadline(candidate: &Candidate) -> Option<u64> {
    // Admission::InvalidExpiry rejects unrepresentable TTLs before publication reaches here.
    match candidate.expiry_policy {
        StreamSessionExpiryPolicy::None => None,
        StreamSessionExpiryPolicy::Sliding { ttl_seconds } => Some(
            Timestamp::now_utc()
                .to_millis()
                .checked_add(
                    ttl_seconds
                        .checked_mul(1_000)
                        .expect("admitted stream TTL must be representable"),
                )
                .expect("admitted stream deadline must be representable"),
        ),
        StreamSessionExpiryPolicy::Absolute { .. } => candidate.expiry_deadline_millis,
    }
}

async fn append_target_initialization(
    oplog: &dyn crate::services::oplog::Oplog,
    candidate: &Candidate,
    request_hash: [u8; 32],
    expiry_deadline_millis: Option<u64>,
) -> Result<(), Error> {
    let initialized =
        StreamSessionRecord::ExportForkInitialized(StreamExportForkInitializedRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            public_session_id: candidate.export.session.clone(),
            session_key: candidate.target_session_key.clone(),
            source_invocation: candidate.source_invocation.clone(),
            request_hash: request_hash.to_vec(),
            expiry_policy: candidate.expiry_policy,
            expiry_deadline_millis,
        });
    let record = oplog
        .upload_payload(&initialized)
        .await
        .map_err(WorkerExecutorError::runtime)?;
    oplog
        .add(OplogEntry::StreamSession {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            record,
        })
        .await;
    Ok(())
}

async fn schedule_expiry<Ctx: WorkerCtx>(
    service: &DefaultWorkerFork<Ctx>,
    target: &OwnedAgentId,
    fingerprint: AgentFingerprint,
    candidate: &Candidate,
    deadline_millis: Option<u64>,
) -> Result<(), Error> {
    let Some(deadline_millis) = deadline_millis else {
        return Ok(());
    };
    // Admission::InvalidExpiry validates the deadline before staged publication.
    let deadline = expiry_datetime(deadline_millis)
        .expect("admitted stream expiry must have a representable deadline");
    service
        .scheduler_service
        .schedule(
            deadline,
            golem_common::model::ScheduledAction::ExpireDurableStreamSession {
                owned_agent_id: target.clone(),
                target_agent_fingerprint: fingerprint,
                public_session_id: candidate.export.session.clone(),
                session_key: candidate.target_session_key.clone(),
                expected_deadline_millis: deadline_millis,
            },
        )
        .await;
    Ok(())
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
    let (expiry_policy, expiry_deadline_millis) =
        effective_expiry_policy(request.expiry_policy, slot.expiry_policy)?;
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
    // The snapshot may include appends after the initial commit. Make its fixed horizon
    // durable before staging reads the source from storage.
    worker
        .commit_oplog_and_update_state(CommitLevel::Always)
        .await;
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
            target_expiry_policy: expiry_policy,
        },
        horizon: snapshot.horizon,
        cut: cut.oplog_index,
        selected: handle.stream_id,
        source_invocation: handle.source_invocation,
        target_session_key: IdempotencyKey::new(Uuid::new_v4().to_string()),
        expiry_policy,
        expiry_deadline_millis,
        retained_through: cut.last_item_offset,
        initial,
    })
}

fn effective_expiry_policy(
    explicit: Option<golem_api_grpc::proto::golem::workerexecutor::v1::StreamSessionExpiryPolicy>,
    inherited: StreamSessionExpiryPolicy,
) -> Result<(StreamSessionExpiryPolicy, Option<u64>), Error> {
    let policy = match explicit {
        Some(policy) => crate::grpc::stream_slots::expiry_policy_from_proto(Some(policy))?,
        None => inherited,
    };
    let deadline = match policy {
        StreamSessionExpiryPolicy::None | StreamSessionExpiryPolicy::Sliding { .. } => None,
        StreamSessionExpiryPolicy::Absolute { expires_at_millis } => Some(expires_at_millis),
    };
    Ok((policy, deadline))
}

fn expiry_datetime(millis: u64) -> Option<chrono::DateTime<chrono::Utc>> {
    i64::try_from(millis)
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
}

pub(crate) fn response(
    export: &StreamExportFork,
    cut: OplogIndex,
    replayed: bool,
    invocation_key: &IdempotencyKey,
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
        invocation_key: Some(invocation_key.clone().into()),
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
        && request.expiry_policy.as_ref().is_none_or(|policy| {
            crate::grpc::stream_slots::expiry_policy_from_proto(Some(*policy))
                .is_ok_and(|policy| policy == export.target_expiry_policy)
        })
}

fn matching_receipt_export<'a>(
    receipt: &'a CreationReceipt,
    request: &ForkStreamSlotRequest,
    source: &AgentId,
) -> Result<&'a StreamExportFork, Error> {
    let export = receipt
        .cut
        .export
        .as_ref()
        .filter(|export| matches_request(export, request, source))
        .ok_or_else(|| reject(Reason::Conflict))?;
    if receipt.initialized.request_hash != receipt.cut.request_hash
        || receipt.initialized.public_session_id != request.session
        || receipt.initialized.expiry_policy != export.target_expiry_policy
    {
        return Err(reject(Reason::Conflict));
    }
    Ok(export)
}

/// The first marker authored for this incarnation is its immutable creation receipt, even
/// when later Revert entries hide it from the active stream projection.
pub(crate) async fn creation_record(
    service: &dyn OplogService,
    target: &OwnedAgentId,
) -> Result<Option<StreamForkCutRecord>, WorkerExecutorError> {
    Ok(creation_receipt(service, target)
        .await?
        .map(|receipt| receipt.cut))
}

struct CreationReceipt {
    cut: StreamForkCutRecord,
    initialized: StreamExportForkInitializedRecord,
    live: bool,
}

async fn creation_receipt(
    service: &dyn OplogService,
    target: &OwnedAgentId,
) -> Result<Option<CreationReceipt>, WorkerExecutorError> {
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
    let mut cut = None;
    let mut initialized = None;
    let mut current_deadline_millis = None;
    let mut live = true;
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
                match record {
                    StreamSessionRecord::ForkCut(record)
                        if cut.is_none()
                            && record.creation_fingerprint.0 == *instance_id
                            && record.revert.is_none() =>
                    {
                        cut = Some(record);
                    }
                    StreamSessionRecord::ExportForkInitialized(record)
                        if initialized.is_none()
                            && cut.as_ref().is_some_and(|cut: &StreamForkCutRecord| {
                                record.request_hash == cut.request_hash
                            }) =>
                    {
                        current_deadline_millis = record.expiry_deadline_millis;
                        initialized = Some(record);
                    }
                    StreamSessionRecord::ExpiryRefreshed(record)
                        if initialized.as_ref().is_some_and(
                            |initialized: &StreamExportForkInitializedRecord| {
                                initialized.session_key == record.session_key
                                    && initialized.public_session_id == record.public_session_id
                            },
                        ) && current_deadline_millis
                            == Some(record.expected_deadline_millis) =>
                    {
                        current_deadline_millis = Some(record.deadline_millis);
                    }
                    StreamSessionRecord::Expired(record)
                        if initialized.as_ref().is_some_and(
                            |initialized: &StreamExportForkInitializedRecord| {
                                initialized.session_key == record.session_key
                                    && initialized.public_session_id == record.public_session_id
                            },
                        ) && current_deadline_millis
                            == Some(record.expected_deadline_millis) =>
                    {
                        live = false;
                    }
                    _ => {}
                }
            }
        }
    }
    match (cut, initialized) {
        (Some(cut), Some(initialized)) => {
            live &= current_deadline_millis
                .is_none_or(|deadline| deadline > Timestamp::now_utc().to_millis());
            Ok(Some(CreationReceipt {
                cut,
                initialized,
                live,
            }))
        }
        (None, None) => Ok(None),
        (Some(cut), None) if cut.export.is_none() => Ok(None),
        _ => Err(WorkerExecutorError::runtime(
            "incomplete fork creation receipt",
        )),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        StreamSessionExpiryPolicy as ProtoExpiryPolicy, stream_session_expiry_policy,
    };
    use test_r::test;

    #[test]
    fn fork_expiry_inherits_or_overrides_the_source_policy() {
        let inherited = StreamSessionExpiryPolicy::Sliding { ttl_seconds: 60 };
        assert_eq!(
            effective_expiry_policy(None, inherited).unwrap(),
            (inherited, None)
        );

        let explicit = ProtoExpiryPolicy {
            kind: Some(stream_session_expiry_policy::Kind::ExpiresAtMillis(5_000)),
        };
        assert_eq!(
            effective_expiry_policy(Some(explicit), inherited).unwrap(),
            (
                StreamSessionExpiryPolicy::Absolute {
                    expires_at_millis: 5_000
                },
                Some(5_000)
            )
        );
        let overflowing = ProtoExpiryPolicy {
            kind: Some(stream_session_expiry_policy::Kind::TtlSeconds(u64::MAX)),
        };
        assert_eq!(
            effective_expiry_policy(Some(overflowing), inherited).unwrap(),
            (
                StreamSessionExpiryPolicy::Sliding {
                    ttl_seconds: u64::MAX
                },
                None
            )
        );

        let out_of_range = ProtoExpiryPolicy {
            kind: Some(stream_session_expiry_policy::Kind::ExpiresAtMillis(
                u64::MAX,
            )),
        };
        assert_eq!(
            effective_expiry_policy(Some(out_of_range), inherited).unwrap(),
            (
                StreamSessionExpiryPolicy::Absolute {
                    expires_at_millis: u64::MAX
                },
                Some(u64::MAX)
            )
        );
    }
}
