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

//! Reads of the invocations of one agent. None of them starts the agent.

use super::{
    AuthCtx, HasAll, OwnedAgentId, UsesAllDeps, Worker, WorkerCtx, WorkerExecutorError,
    WorkerExecutorImpl, extract_owned_agent_id, golem,
};
use crate::worker::invocations::{
    InvocationFilter, InvocationKind, InvocationListEntry, InvocationPagePosition, InvocationState,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use desert_rust::BinaryCodec;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    GetInvocationRequest, InvocationLifecycleState, InvocationSummary, ListInvocationsRequest,
    ListInvocationsSuccess, LookupInvocationsRequest,
};
use golem_common::model::{
    AgentFingerprint, IdempotencyKey, InvocationId, InvocationRequestKey, OplogIndex, Timestamp,
};
use golem_common::serialization::{decode, serialize};
use std::sync::Arc;

/// The decoded `list` cursor. It names the agent incarnation it was issued for, so a cursor of
/// another agent, or of an earlier incarnation, is refused.
#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
struct InvocationCursor {
    agent_id: String,
    fingerprint: uuid::Uuid,
    last_change: u64,
    key: uuid::Uuid,
}

fn encode_cursor(
    owned_agent_id: &OwnedAgentId,
    fingerprint: AgentFingerprint,
    position: InvocationPagePosition,
) -> Result<String, WorkerExecutorError> {
    let cursor = InvocationCursor {
        agent_id: owned_agent_id.agent_id.to_string(),
        fingerprint: fingerprint.0,
        last_change: position.last_change.as_u64(),
        key: position.key.0,
    };
    Ok(URL_SAFE_NO_PAD.encode(serialize(&cursor).map_err(WorkerExecutorError::runtime)?))
}

fn decode_cursor(
    value: &str,
    owned_agent_id: &OwnedAgentId,
    fingerprint: AgentFingerprint,
) -> Result<InvocationPagePosition, WorkerExecutorError> {
    let invalid = || WorkerExecutorError::invalid_request("invalid invocation cursor");
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| invalid())?;
    // A client sends the cursor, so it must decode without panicking on any bytes.
    let cursor: InvocationCursor = decode(&bytes).map_err(|_| invalid())?;
    if cursor.agent_id != owned_agent_id.agent_id.to_string() || cursor.fingerprint != fingerprint.0
    {
        return Err(WorkerExecutorError::invalid_request(
            "the invocation cursor belongs to another agent",
        ));
    }
    Ok(InvocationPagePosition {
        last_change: OplogIndex::from_u64(cursor.last_change),
        key: InvocationId(cursor.key),
    })
}

impl From<InvocationState> for InvocationLifecycleState {
    fn from(state: InvocationState) -> Self {
        match state {
            InvocationState::Pending => Self::Pending,
            InvocationState::Scheduled => Self::Scheduled,
            InvocationState::Running => Self::Running,
            InvocationState::Waiting => Self::Waiting,
            InvocationState::Retrying => Self::Retrying,
            InvocationState::Interrupted => Self::Interrupted,
            InvocationState::Completed => Self::Completed,
            InvocationState::BackgroundFailed => Self::BackgroundFailed,
            InvocationState::Failed => Self::Failed,
            InvocationState::Cancelled => Self::Cancelled,
        }
    }
}

impl TryFrom<i32> for InvocationState {
    type Error = WorkerExecutorError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match InvocationLifecycleState::try_from(value) {
            Ok(InvocationLifecycleState::Pending) => Ok(Self::Pending),
            Ok(InvocationLifecycleState::Scheduled) => Ok(Self::Scheduled),
            Ok(InvocationLifecycleState::Running) => Ok(Self::Running),
            Ok(InvocationLifecycleState::Waiting) => Ok(Self::Waiting),
            Ok(InvocationLifecycleState::Retrying) => Ok(Self::Retrying),
            Ok(InvocationLifecycleState::Interrupted) => Ok(Self::Interrupted),
            Ok(InvocationLifecycleState::Completed) => Ok(Self::Completed),
            Ok(InvocationLifecycleState::BackgroundFailed) => Ok(Self::BackgroundFailed),
            Ok(InvocationLifecycleState::Failed) => Ok(Self::Failed),
            Ok(InvocationLifecycleState::Cancelled) => Ok(Self::Cancelled),
            Ok(InvocationLifecycleState::Unspecified) | Err(_) => Err(
                WorkerExecutorError::invalid_request(format!("invalid invocation state {value}")),
            ),
        }
    }
}

impl From<InvocationKind> for golem::workerexecutor::v1::InvocationKind {
    fn from(kind: InvocationKind) -> Self {
        match kind {
            InvocationKind::Method => Self::Method,
            InvocationKind::Constructor => Self::Constructor,
            InvocationKind::Tool => Self::Tool,
        }
    }
}

impl TryFrom<i32> for InvocationKind {
    type Error = WorkerExecutorError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        use golem::workerexecutor::v1::InvocationKind as Proto;
        match Proto::try_from(value) {
            Ok(Proto::Method) => Ok(Self::Method),
            Ok(Proto::Constructor) => Ok(Self::Constructor),
            Ok(Proto::Tool) => Ok(Self::Tool),
            Ok(Proto::Unspecified) | Err(_) => Err(WorkerExecutorError::invalid_request(format!(
                "invalid invocation kind {value}"
            ))),
        }
    }
}

/// The latest bound accepted, the end of year 9999 in milliseconds after the Unix epoch.
const MAX_BOUND_MILLIS: u64 = 253_402_300_799_999;

/// A `since` or `until` bound from the client, in milliseconds like the listed timestamps. The
/// shared conversion assumes a valid timestamp after the Unix epoch, so client input is checked
/// here first.
/// Converts an inclusive bound to whole milliseconds, rounding a lower bound up and an upper
/// bound down so that a sub-millisecond part never widens the range.
fn bound_from_proto(
    name: &str,
    value: prost_types::Timestamp,
    lower: bool,
) -> Result<Timestamp, WorkerExecutorError> {
    let millis = u64::try_from(value.seconds)
        .ok()
        .zip(
            u32::try_from(value.nanos)
                .ok()
                .filter(|nanos| *nanos < 1_000_000_000),
        )
        .and_then(|(seconds, nanos)| {
            seconds.checked_mul(1000)?.checked_add(u64::from(if lower {
                nanos.div_ceil(1_000_000)
            } else {
                nanos / 1_000_000
            }))
        })
        .filter(|millis| *millis <= MAX_BOUND_MILLIS)
        .ok_or_else(|| {
            WorkerExecutorError::invalid_request(format!(
                "{name} must be a valid timestamp between the Unix epoch and the year 9999"
            ))
        })?;
    Ok(Timestamp::from(millis))
}

fn summary(entry: InvocationListEntry) -> InvocationSummary {
    InvocationSummary {
        invocation_id: Some(entry.key.into()),
        state: InvocationLifecycleState::from(entry.state) as i32,
        kind: entry.kind().map_or(
            golem::workerexecutor::v1::InvocationKind::Unspecified,
            Into::into,
        ) as i32,
        operation: entry
            .operation
            .as_ref()
            .and_then(|operation| operation.name())
            .map(str::to_string),
        accepted_at: entry.accepted_at.map(Into::into),
        changed_at: Some(entry.changed_at.into()),
    }
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    /// The existing agent whose invocations a read names, without starting it. A missing or
    /// deleted agent gives `AgentNotFound`.
    async fn agent_for_invocation_read(
        &self,
        owned_agent_id: &OwnedAgentId,
        auth_ctx: &Option<golem::auth::AuthCtx>,
        operation: &str,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        self.ensure_worker_belongs_to_this_executor(owned_agent_id)?;
        Self::validate_auth_ctx(auth_ctx)?;
        self.ensure_not_being_deleted(owned_agent_id).await?;
        self.get_existing_for_management(owned_agent_id, operation)
            .await
    }

    pub(super) async fn get_invocation_internal(
        &self,
        request: GetInvocationRequest,
    ) -> Result<Option<InvocationSummary>, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        let invocation_id: InvocationId = request
            .invocation_id
            .ok_or(WorkerExecutorError::invalid_request(
                "invocation_id not found",
            ))?
            .into();
        let worker = self
            .agent_for_invocation_read(&owned_agent_id, &request.auth_ctx, "get invocation")
            .await?;
        let entry =
            Self::not_found_if_deleting(&worker, worker.get_invocation(&invocation_id).await)
                .await?;
        Ok(entry.map(summary))
    }

    pub(super) async fn list_invocations_internal(
        &self,
        request: ListInvocationsRequest,
    ) -> Result<ListInvocationsSuccess, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        let filter = InvocationFilter {
            operation: request.operation.clone(),
            states: request
                .states
                .iter()
                .map(|state| InvocationState::try_from(*state))
                .collect::<Result<_, _>>()?,
            kind: request.kind.map(InvocationKind::try_from).transpose()?,
            since: request
                .since
                .map(|since| bound_from_proto("since", since, true))
                .transpose()?,
            until: request
                .until
                .map(|until| bound_from_proto("until", until, false))
                .transpose()?,
        };
        let worker = self
            .agent_for_invocation_read(&owned_agent_id, &request.auth_ctx, "list invocations")
            .await?;
        let fingerprint = worker.get_initial_worker_metadata().fingerprint;
        let after = request
            .cursor
            .as_ref()
            .map(|cursor| decode_cursor(&cursor.value, &owned_agent_id, fingerprint))
            .transpose()?;
        let (page, next) = Self::not_found_if_deleting(
            &worker,
            worker
                .list_invocation_page(&filter, after, request.page_size as usize)
                .await,
        )
        .await?;
        Ok(ListInvocationsSuccess {
            invocations: page.into_iter().map(summary).collect(),
            next_cursor: next
                .map(|position| encode_cursor(&owned_agent_id, fingerprint, position))
                .transpose()?
                .map(|value| golem::worker::Cursor { value }),
        })
    }

    pub(super) async fn lookup_invocations_internal(
        &self,
        request: LookupInvocationsRequest,
    ) -> Result<Vec<InvocationSummary>, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        let key: IdempotencyKey = request
            .idempotency_key
            .clone()
            .ok_or(WorkerExecutorError::invalid_request(
                "idempotency_key not found",
            ))?
            .into();
        let auth: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!("invalid auth_ctx: {error}"))
            })?;
        let worker = self
            .agent_for_invocation_read(&owned_agent_id, &request.auth_ctx, "lookup invocations")
            .await?;
        // A caller without an idempotency scope never derived a keyed invocation ID.
        let Some(scope) = auth.idempotency_scope() else {
            return Ok(Vec::new());
        };
        let request_key = InvocationRequestKey::External { scope, key };
        let found = Self::not_found_if_deleting(
            &worker,
            worker
                .lookup_invocations(&request_key, request.operation.as_deref())
                .await,
        )
        .await?;
        Ok(found.into_iter().map(summary).collect())
    }
}

#[cfg(test)]
mod tests;
