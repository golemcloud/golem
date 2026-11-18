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

//! The environment-scoped invocation reads and cancel.

use super::{
    WorkerResult, WorkerService, WorkerServiceError, authorize_agent_permission, native_tool_result,
};
use crate::api::invocation::{
    Invocation, InvocationFailure, InvocationKind, InvocationMethodResult, InvocationOutcome,
    InvocationPage, InvocationState, InvocationSummary, InvocationToolResult,
};
use crate::service::component::ComponentServiceError;
use chrono::{DateTime, Utc};
use golem_api_grpc::proto::golem::worker::{Cursor, InvocationStart};
use golem_api_grpc::proto::golem::workerexecutor::v1 as executor;
use golem_common::SafeDisplay;
use golem_common::model::agent::{OwnerKind, ParsedAgentKey, Principal};
use golem_common::model::card::{AgentInvocationIdPattern, AgentResourcePattern, AgentVerb};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, AgentInvocationResult, IdempotencyKey, OwnedInvocationId};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use tracing::warn;

/// The filters and page of an invocation listing.
#[derive(Debug, Clone, Default)]
pub struct InvocationListQuery {
    pub operation: Option<String>,
    pub states: Vec<InvocationState>,
    pub kind: Option<InvocationKind>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub page_size: Option<u32>,
    pub cursor: Option<String>,
}

impl WorkerService {
    pub async fn get_invocation(
        &self,
        environment_id: EnvironmentId,
        invocation_id: &OwnedInvocationId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Invocation> {
        let agent_id = invocation_id.agent_id();
        let component = self
            .component_for_invocation_read(
                environment_id,
                agent_id,
                &auth_ctx,
                AgentVerb::View,
                AgentResourcePattern::Any,
            )
            .await?;
        let summary = self
            .invocation_summary(&component, invocation_id, auth_ctx.clone())
            .await?;
        let result = match summary.state {
            InvocationState::Completed | InvocationState::Failed => {
                self.invocation_outcome(&component, invocation_id, &summary, auth_ctx)
                    .await?
            }
            _ => None,
        };
        Ok(Invocation { summary, result })
    }

    pub async fn list_invocations(
        &self,
        environment_id: EnvironmentId,
        agent_id: &AgentId,
        query: InvocationListQuery,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<InvocationPage> {
        let component = self
            .component_for_invocation_read(
                environment_id,
                agent_id,
                &auth_ctx,
                AgentVerb::View,
                AgentResourcePattern::Any,
            )
            .await?;
        let request = executor::ListInvocationsRequest {
            operation: query.operation,
            states: query
                .states
                .into_iter()
                .map(|state| state_to_proto(state) as i32)
                .collect(),
            kind: query.kind.map(|kind| kind_to_proto(kind) as i32),
            since: query.since.map(timestamp_to_proto),
            until: query.until.map(timestamp_to_proto),
            page_size: query.page_size.unwrap_or(0),
            cursor: query.cursor.map(|value| Cursor { value }),
            ..Default::default()
        };
        let page = self
            .worker_client
            .list_invocations(agent_id, request, component.environment_id, auth_ctx)
            .await?;
        Ok(InvocationPage {
            invocations: page
                .invocations
                .into_iter()
                .map(|summary| summary_from_proto(agent_id, environment_id, summary))
                .collect::<WorkerResult<_>>()?,
            next_cursor: page.next_cursor.map(|cursor| cursor.value),
        })
    }

    /// Every invocation that the caller started on the agent with `idempotency_key`. None is
    /// not found.
    pub async fn lookup_invocations(
        &self,
        environment_id: EnvironmentId,
        agent_id: &AgentId,
        idempotency_key: &IdempotencyKey,
        operation: Option<String>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<InvocationSummary>> {
        let component = self
            .component_for_invocation_read(
                environment_id,
                agent_id,
                &auth_ctx,
                AgentVerb::View,
                AgentResourcePattern::Any,
            )
            .await?;
        let invocations = self
            .worker_client
            .lookup_invocations(
                agent_id,
                idempotency_key,
                operation,
                component.environment_id,
                auth_ctx,
            )
            .await?;
        if invocations.is_empty() {
            return Err(WorkerServiceError::InvocationNotFound(format!(
                "no invocation of {agent_id} has this idempotency key"
            )));
        }
        invocations
            .into_iter()
            .map(|summary| summary_from_proto(agent_id, environment_id, summary))
            .collect()
    }

    /// Cancels a pending or scheduled invocation, and answers the invocation's state after the
    /// request. A running invocation is a `CannotCancelRunningInvocation` error.
    pub async fn cancel_invocation_by_id(
        &self,
        environment_id: EnvironmentId,
        invocation_id: &OwnedInvocationId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<InvocationSummary> {
        let agent_id = invocation_id.agent_id();
        let component = self
            .component_for_invocation_read(
                environment_id,
                agent_id,
                &auth_ctx,
                AgentVerb::CancelInvocation,
                AgentResourcePattern::InvocationId(AgentInvocationIdPattern::Uuid(
                    invocation_id.uuid(),
                )),
            )
            .await?;
        // The executor rejects an unknown ID as an invalid request, so the ID is resolved first.
        let summary = self
            .invocation_summary(&component, invocation_id, auth_ctx.clone())
            .await?;
        if summary.state.is_terminal() {
            return Ok(summary);
        }
        self.worker_client
            .cancel_invocation(
                agent_id,
                &invocation_id.id(),
                component.environment_id,
                auth_ctx.clone(),
                None,
            )
            .await?;
        self.invocation_summary(&component, invocation_id, auth_ctx)
            .await
    }

    /// The component of an agent named by an environment-scoped request, after the caller is
    /// authorized for `verb` on `resource` of it. An agent of another environment, and the owner of a
    /// standalone tool invocation, are not found.
    async fn component_for_invocation_read(
        &self,
        environment_id: EnvironmentId,
        agent_id: &AgentId,
        auth_ctx: &AuthCtx,
        verb: AgentVerb,
        resource: AgentResourcePattern,
    ) -> WorkerResult<Component> {
        let not_found = || WorkerServiceError::AgentNotFound(agent_id.clone());
        if OwnerKind::is_reserved_instance_name(&agent_id.agent_id) {
            return Err(not_found());
        }
        let component = match self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await
        {
            Ok(component) => component,
            Err(ComponentServiceError::ComponentNotFound) => return Err(not_found()),
            Err(error) => return Err(error.into()),
        };
        if component.environment_id != environment_id {
            return Err(not_found());
        }
        authorize_agent_permission(auth_ctx, &component, agent_id, verb, resource)?;
        Ok(component)
    }

    async fn invocation_summary(
        &self,
        component: &Component,
        invocation_id: &OwnedInvocationId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<InvocationSummary> {
        let agent_id = invocation_id.agent_id();
        let summary = self
            .worker_client
            .get_invocation(
                agent_id,
                &invocation_id.id(),
                component.environment_id,
                auth_ctx,
            )
            .await?
            .ok_or_else(|| WorkerServiceError::InvocationNotFound(invocation_id.to_string()))?;
        summary_from_proto(agent_id, component.environment_id, summary)
    }

    /// The result of a terminal invocation, read through the existing result lookup.
    async fn invocation_outcome(
        &self,
        component: &Component,
        invocation_id: &OwnedInvocationId,
        summary: &InvocationSummary,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<InvocationOutcome>> {
        let agent_id = invocation_id.agent_id();
        let start = InvocationStart {
            agent_id: Some(agent_id.clone().into()),
            idempotency_key: Some(IdempotencyKey::new(invocation_id.uuid().to_string()).into()),
            principal: Some(Principal::anonymous().into()),
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            ..Default::default()
        };
        let output = match self.lookup_agent_invocation(start, auth_ctx).await {
            Ok(output) => output,
            Err(error) if summary.state == InvocationState::Failed => {
                return Ok(Some(InvocationOutcome::Failure(InvocationFailure {
                    error: error.to_safe_string(),
                })));
            }
            Err(error) => {
                warn!(
                    invocation_id = %invocation_id,
                    error = error.to_safe_string(),
                    "Failed to read the result of a completed invocation"
                );
                return Ok(None);
            }
        };
        let decoded = match output.result {
            AgentInvocationResult::AgentMethod { output: value } => {
                let method_name = summary.operation.as_deref().ok_or_else(|| {
                    WorkerServiceError::Internal(format!(
                        "the method of invocation {invocation_id} is not known"
                    ))
                });
                match method_name.and_then(|method_name| {
                    ParsedAgentKey::parse_agent_type_name(&agent_id.agent_id)
                        .map_err(WorkerServiceError::Internal)
                        .map(|agent_type_name| (method_name, agent_type_name))
                }) {
                    Ok((method_name, agent_type_name)) => self
                        .agent_method_output_to_external(
                            agent_id.component_id,
                            output.component_revision.unwrap_or(component.revision),
                            &agent_type_name,
                            method_name,
                            value,
                        )
                        .await
                        .map(|value| {
                            Some(InvocationOutcome::Method(InvocationMethodResult { value }))
                        }),
                    Err(error) => Err(error),
                }
            }
            AgentInvocationResult::ExternalTool { result } => native_tool_result(result)
                .map(|result| Some(InvocationOutcome::Tool(InvocationToolResult { result }))),
            _ => Ok(None),
        };
        Ok(decoded.unwrap_or_else(|error| {
            warn!(
                invocation_id = %invocation_id,
                error = error.to_safe_string(),
                "Failed to decode the result of a completed invocation"
            );
            None
        }))
    }
}

fn summary_from_proto(
    agent_id: &AgentId,
    environment_id: EnvironmentId,
    summary: executor::InvocationSummary,
) -> WorkerResult<InvocationSummary> {
    let invalid = |what: &str| WorkerServiceError::Internal(format!("invalid invocation {what}"));
    let id = summary.invocation_id.ok_or_else(|| invalid("ID"))?;
    let invocation_id = OwnedInvocationId::new(
        agent_id.clone(),
        golem_common::model::InvocationId(id.into()),
    );
    let state = match executor::InvocationLifecycleState::try_from(summary.state) {
        Ok(executor::InvocationLifecycleState::Pending) => InvocationState::Pending,
        Ok(executor::InvocationLifecycleState::Scheduled) => InvocationState::Scheduled,
        Ok(executor::InvocationLifecycleState::Running) => InvocationState::Running,
        Ok(executor::InvocationLifecycleState::Waiting) => InvocationState::Waiting,
        Ok(executor::InvocationLifecycleState::Retrying) => InvocationState::Retrying,
        Ok(executor::InvocationLifecycleState::Interrupted) => InvocationState::Interrupted,
        Ok(executor::InvocationLifecycleState::Completed) => InvocationState::Completed,
        Ok(executor::InvocationLifecycleState::Failed) => InvocationState::Failed,
        Ok(executor::InvocationLifecycleState::Cancelled) => InvocationState::Cancelled,
        Ok(executor::InvocationLifecycleState::Unspecified) | Err(_) => {
            return Err(invalid("state"));
        }
    };
    let kind = match executor::InvocationKind::try_from(summary.kind) {
        Ok(executor::InvocationKind::Method) => Some(InvocationKind::Method),
        Ok(executor::InvocationKind::Constructor) => Some(InvocationKind::Constructor),
        Ok(executor::InvocationKind::Tool) => Some(InvocationKind::Tool),
        Ok(executor::InvocationKind::Unspecified) | Err(_) => None,
    };
    Ok(InvocationSummary {
        invocation_id: invocation_id.to_string(),
        agent_id: agent_id.clone(),
        environment_id,
        state,
        kind,
        operation: summary.operation,
        accepted_at: summary.accepted_at.map(timestamp_from_proto).transpose()?,
        changed_at: timestamp_from_proto(
            summary.changed_at.ok_or_else(|| invalid("change time"))?,
        )?,
    })
}

fn timestamp_from_proto(timestamp: prost_types::Timestamp) -> WorkerResult<DateTime<Utc>> {
    u32::try_from(timestamp.nanos)
        .ok()
        .and_then(|nanos| DateTime::from_timestamp(timestamp.seconds, nanos))
        .ok_or_else(|| WorkerServiceError::Internal("invalid invocation timestamp".to_string()))
}

fn timestamp_to_proto(timestamp: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: timestamp.timestamp(),
        nanos: timestamp.timestamp_subsec_nanos() as i32,
    }
}

fn state_to_proto(state: InvocationState) -> executor::InvocationLifecycleState {
    match state {
        InvocationState::Pending => executor::InvocationLifecycleState::Pending,
        InvocationState::Scheduled => executor::InvocationLifecycleState::Scheduled,
        InvocationState::Running => executor::InvocationLifecycleState::Running,
        InvocationState::Waiting => executor::InvocationLifecycleState::Waiting,
        InvocationState::Retrying => executor::InvocationLifecycleState::Retrying,
        InvocationState::Interrupted => executor::InvocationLifecycleState::Interrupted,
        InvocationState::Completed => executor::InvocationLifecycleState::Completed,
        InvocationState::Failed => executor::InvocationLifecycleState::Failed,
        InvocationState::Cancelled => executor::InvocationLifecycleState::Cancelled,
    }
}

fn kind_to_proto(kind: InvocationKind) -> executor::InvocationKind {
    match kind {
        InvocationKind::Method => executor::InvocationKind::Method,
        InvocationKind::Constructor => executor::InvocationKind::Constructor,
        InvocationKind::Tool => executor::InvocationKind::Tool,
    }
}
