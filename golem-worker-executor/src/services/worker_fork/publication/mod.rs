// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use crate::services::agent_filesystem_snapshots::PublishFound;
use crate::services::oplog::{Oplog, OplogOps, OplogService, OplogServiceOps, UnreadableOplog};
use golem_common::model::agent::AgentMode;
use golem_common::model::durable_stream::{StreamId, StreamOffset, StreamSessionRecord};
use golem_common::model::oplog::host_functions::GolemApiFork;
use golem_common::model::oplog::{
    DurableFunctionType, DurableInvocationId, FilesystemSnapshotName, HostPayloadPair, HostRequest,
    HostRequestNoInput, HostResponse, HostResponseGolemApiFork, OplogEntry, OplogIndex,
};
use golem_common::model::{AgentFingerprint, OwnedAgentId, Timestamp};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tracing::warn;
use uuid::Uuid;

pub(crate) fn request_hash(
    source: &OwnedAgentId,
    fingerprint: AgentFingerprint,
    cut: OplogIndex,
    selected: Option<(StreamId, Option<StreamOffset>)>,
    guest_result: Option<(Option<OplogIndex>, Option<DurableInvocationId>, Uuid)>,
) -> Result<[u8; 32], String> {
    let bytes = golem_common::serialization::serialize(&(
        "golem-fork-request-v1".to_string(),
        source.environment_id,
        source.agent_id.clone(),
        fingerprint,
        cut,
        selected,
        guest_result,
    ))
    .map_err(|error| error.to_string())?;
    Ok(*blake3::hash(&bytes).as_bytes())
}

/// What a reconciliation found at the target of a fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExistingFork {
    /// No durable target exists.
    Absent,
    /// The target is a fork of this request, with this instance id.
    Matching(AgentFingerprint),
    /// The target has a `Create` with this instance id, and it is not a fork of this request.
    Other(AgentFingerprint),
}

impl ExistingFork {
    /// Whether the target `target` is a fork of this request. Another target is a conflict.
    pub(crate) fn matches(self, target: &OwnedAgentId) -> Result<bool, WorkerExecutorError> {
        match self {
            Self::Absent => Ok(false),
            Self::Matching(_) => Ok(true),
            Self::Other(_) => Err(conflict(target)),
        }
    }
}

/// The answer of a plain fork attempt at the cut `cut` whose copy lacks the snapshot `name` of the
/// baseline of the target, after its reconciliation found `existing`, with the instance id of the
/// live target that the stage of the attempt lost to. A fork of this request answers success,
/// another target answers a conflict, and without a target the fork is refused with an error that
/// names the cut and the snapshot.
pub(crate) fn missing_baseline_answer(
    existing: ExistingFork,
    target: &OwnedAgentId,
    cut: OplogIndex,
    name: &FilesystemSnapshotName,
) -> (Option<AgentFingerprint>, Result<(), WorkerExecutorError>) {
    match existing {
        ExistingFork::Matching(live) => (Some(live), Ok(())),
        ExistingFork::Other(live) => (Some(live), Err(conflict(target))),
        ExistingFork::Absent => (
            None,
            Err(WorkerExecutorError::invalid_request(format!(
                "Cannot fork worker at oplog index {cut}: the filesystem snapshot {name} of the target's baseline is not in the store"
            ))),
        ),
    }
}

/// The error of a fork whose target exists and is not a fork of its request.
pub(crate) fn conflict(target: &OwnedAgentId) -> WorkerExecutorError {
    WorkerExecutorError::worker_already_exists(target.agent_id.clone())
}

/// Reconcile without opening a writer or depending on the fork still being suspended. A fork
/// may already have run, archived, or lost its response; its creation prefix is immutable. Once
/// the reconciliation read the `Create` of the target, it gives the instance id of the target,
/// also when the target is not a fork of this request.
pub(crate) async fn existing_fork(
    service: &dyn OplogService,
    target: &OwnedAgentId,
    cut: OplogIndex,
    hash: [u8; 32],
) -> Result<ExistingFork, WorkerExecutorError> {
    let mode = AgentMode::Durable;
    let conflict = || conflict(target);
    if service.exists(target, AgentMode::Ephemeral).await? {
        return Err(conflict());
    }
    if !service.exists(target, mode).await? {
        return Ok(ExistingFork::Absent);
    }
    let initial = service
        .read_source(target, mode, OplogIndex::INITIAL, 1)
        .await?;
    let Some(OplogEntry::Create { parameters, .. }) = initial.get(&OplogIndex::INITIAL) else {
        return Err(conflict());
    };
    let other = ExistingFork::Other(AgentFingerprint(parameters.instance_id));
    if service.get_last_index(target, mode).await? < cut.next() {
        return Ok(other);
    }
    if parameters.agent_id != target.agent_id || parameters.environment_id != target.environment_id
    {
        return Ok(other);
    }
    let entries = service.read_source(target, mode, cut.next(), 1).await?;
    let Some(OplogEntry::StreamSession { record, .. }) = entries.get(&cut.next()) else {
        return Ok(other);
    };
    let record = service
        .download_payload(target, mode, record.clone())
        .await
        .map_err(WorkerExecutorError::runtime)?;
    if !record.has_supported_format() {
        return Ok(other);
    }
    match record {
        StreamSessionRecord::ForkCut(record)
            if record.request_hash == hash
                && record.cut_index == cut
                && record.creation_fingerprint.0 == parameters.instance_id =>
        {
            Ok(ExistingFork::Matching(AgentFingerprint(
                parameters.instance_id,
            )))
        }
        _ => Ok(other),
    }
}

/// The instance id of the live durable target, from its `Create`, or `None` when no target with
/// a `Create` is there or the oplog of the target does not read, so the outcome stays unknown.
pub(crate) async fn live_instance(
    service: &dyn OplogService,
    target: &OwnedAgentId,
) -> Option<AgentFingerprint> {
    let mode = AgentMode::Durable;
    let initial = async {
        if !service.exists(target, mode).await? {
            return Ok::<_, UnreadableOplog>(None);
        }
        service
            .read_source(target, mode, OplogIndex::INITIAL, 1)
            .await
            .map(Some)
    }
    .await
    .unwrap_or_else(|error| {
        warn!(
            agent_id = %target,
            error = %error,
            "Failed to read the oplog of a fork target after a refused publication"
        );
        None
    })?;
    match initial.get(&OplogIndex::INITIAL) {
        Some(OplogEntry::Create { parameters, .. }) => {
            Some(AgentFingerprint(parameters.instance_id))
        }
        _ => None,
    }
}

/// What a plain fork found after a publication that did not give `true`, from the `outcome` of the
/// publication and the `reconciled` target. A read of a `Create` gives the live instance id: a fork
/// of this request succeeds, and another target is a conflict. Without such a read, a refused
/// publication is a loss that the snapshot service checks, and an error leaves the outcome
/// unknown.
pub(crate) fn found_after(
    target: &OwnedAgentId,
    outcome: Result<bool, String>,
    reconciled: Result<ExistingFork, WorkerExecutorError>,
) -> PublishFound<Result<(), WorkerExecutorError>, WorkerExecutorError> {
    match (reconciled, outcome) {
        (Ok(ExistingFork::Matching(live)), _) => PublishFound::Live(live, Ok(())),
        (Ok(ExistingFork::Other(live)), _) => PublishFound::Live(live, Err(conflict(target))),
        (Ok(ExistingFork::Absent), Ok(_)) => PublishFound::Refused(Err(
            WorkerExecutorError::runtime("Fork publication lost its target before reconciliation"),
        )),
        (Err(error), Ok(_)) => PublishFound::Refused(Err(error)),
        (_, Err(error)) => PublishFound::Unknown(WorkerExecutorError::runtime(error)),
    }
}

/// Complete the copied fork call before publishing the target. The synthetic pair must never
/// become visible without its terminal, even if the executor dies during creation.
pub(crate) async fn write_guest_result(
    oplog: &dyn Oplog,
    copied_scope_start: Option<OplogIndex>,
    invocation_id: Option<DurableInvocationId>,
    forked_phantom_id: Uuid,
) -> Result<(), WorkerExecutorError> {
    let request = HostRequest::NoInput(HostRequestNoInput {});
    let response = HostResponse::GolemApiFork(HostResponseGolemApiFork {
        forked_phantom_id,
        result: Ok(golem_common::model::ForkResult::Forked),
    });
    let request_payload = oplog
        .upload_payload(&request)
        .await
        .map_err(WorkerExecutorError::runtime)?;
    let response_payload = oplog
        .upload_payload(&response)
        .await
        .map_err(WorkerExecutorError::runtime)?;
    let now = Timestamp::now_utc();
    oplog
        .add_pair(
            OplogEntry::Start {
                timestamp: now,
                parent_start_index: copied_scope_start,
                function_name: GolemApiFork::HOST_FUNCTION_NAME,
                invocation_id,
                observational_owner: None,
                request: Some(request_payload),
                durable_function_type: DurableFunctionType::WriteRemote,
                span_started: None,
                denial: None,
            },
            Box::new(move |start_index| OplogEntry::End {
                timestamp: now,
                start_index,
                response: Some(response_payload),
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            }),
        )
        .await?;
    if let Some(start_index) = copied_scope_start {
        oplog
            .add(OplogEntry::End {
                timestamp: now,
                start_index,
                response: None,
                forced_commit: true,
                span_finished: None,
                span_attributes: None,
            })
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
