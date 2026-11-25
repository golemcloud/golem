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
    AgentPermission, ManagedAgent, WorkerResult, WorkerService, WorkerServiceError,
    native_tool_result,
};
use crate::api::invocation::{
    Invocation, InvocationBackgroundFailure, InvocationFailure, InvocationKind,
    InvocationMethodResult, InvocationOutcome, InvocationPage, InvocationState, InvocationSummary,
    InvocationToolResult,
};
use chrono::{DateTime, Utc};
use golem_api_grpc::proto::golem::worker::{Cursor, InvocationStart};
use golem_api_grpc::proto::golem::workerexecutor::v1 as executor;
use golem_common::SafeDisplay;
use golem_common::model::caller::Caller;
use golem_common::model::card::{AgentInvocationIdPattern, AgentResourcePattern, AgentVerb};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::{ToolBindingOwner, ToolName, ToolSource};
use golem_common::model::{
    AgentId, AgentInvocationResult, BackgroundFailure, IdempotencyKey, InvocationOwner,
    OwnedAgentId, OwnedInvocationId, RESERVED_TYPE_TAG, StandaloneToolInvocation,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::time::SystemTime;
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
        let agent_id = self
            .invocation_address(environment_id, invocation_id, &auth_ctx)
            .await?;
        let agent = self
            .agent_for_invocation_read(
                environment_id,
                &agent_id,
                &auth_ctx,
                AgentVerb::View,
                AgentResourcePattern::Any,
            )
            .await?;
        let summary = self
            .invocation_summary(environment_id, &agent_id, invocation_id, auth_ctx.clone())
            .await?;
        let result = match summary.state {
            InvocationState::Completed
            | InvocationState::BackgroundFailed
            | InvocationState::Failed => {
                self.invocation_outcome(&agent, invocation_id, &summary, auth_ctx)
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
        self.require_agent_address(&OwnedAgentId::new(environment_id, agent_id))
            .await?;
        self.agent_for_invocation_read(
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
                .map(|state| executor::InvocationLifecycleState::from(state) as i32)
                .collect(),
            kind: query
                .kind
                .map(|kind| executor::InvocationKind::from(kind) as i32),
            since: query.since.map(|since| SystemTime::from(since).into()),
            until: query.until.map(|until| SystemTime::from(until).into()),
            page_size: query.page_size.unwrap_or(0),
            cursor: query.cursor.map(|value| Cursor { value }),
            ..Default::default()
        };
        let page = self
            .worker_client
            .list_invocations(agent_id, request, environment_id, auth_ctx)
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
        agent_id: Option<&AgentId>,
        idempotency_key: &IdempotencyKey,
        operation: Option<String>,
        tool_name: Option<&ToolName>,
        command: Option<&str>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<InvocationSummary>> {
        if let Some(tool_name) = tool_name {
            if operation.is_some() {
                return Err(WorkerServiceError::TypeChecker(
                    "operation and tool are mutually exclusive".into(),
                ));
            }
            if !auth_ctx.is_system() {
                self.agent_types
                    .authorize_environment(environment_id, &auth_ctx)
                    .await?;
            }
            let state = self
                .agent_types
                .tool_deployment(environment_id)
                .await?
                .ok_or_else(|| {
                    WorkerServiceError::InvocationNotFound(format!(
                        "tool {tool_name} is not deployed"
                    ))
                })?;
            let tool = state.registered_tools.get(tool_name).ok_or_else(|| {
                WorkerServiceError::InvocationNotFound(format!("tool {tool_name} is not deployed"))
            })?;
            let definitions = if let Some(agent_id) = agent_id {
                self.require_agent_address(&OwnedAgentId::new(environment_id, agent_id))
                    .await?;
                let agent = self
                    .agent_for_invocation_read(
                        environment_id,
                        agent_id,
                        &auth_ctx,
                        AgentVerb::View,
                        AgentResourcePattern::Any,
                    )
                    .await?;
                let agent_type_name = agent
                    .metadata
                    .agent_type_name
                    .clone()
                    .ok_or_else(|| WorkerServiceError::AgentNotFound(*agent_id))?;
                vec![
                    self.effective_tool_definition(
                        &agent.component,
                        &ToolBindingOwner::AgentType { agent_type_name },
                        tool_name.as_str(),
                    )
                    .await?
                    .0,
                ]
            } else {
                if !matches!(tool.registration.source, ToolSource::Component { .. }) {
                    return Err(WorkerServiceError::InvocationNotFound(format!(
                        "tool {tool_name} has no standalone invocation lookup"
                    )));
                }
                state
                    .tool_bindings
                    .iter()
                    .filter(|(owner, bindings)| {
                        matches!(owner, ToolBindingOwner::ComponentBaseline { .. })
                            && bindings.contains_key(tool_name)
                    })
                    .map(|(owner, _)| {
                        state
                            .tool_middleware_chains
                            .get(owner)
                            .and_then(|chains| chains.get(tool_name))
                            .map(|chain| chain.effective_definition.clone())
                            .unwrap_or_else(|| tool.registration.definition.clone())
                    })
                    .collect()
            };
            let mut paths = std::collections::BTreeSet::new();
            for definition in definitions {
                // A command may be exposed by one baseline's middleware but not another's.
                if let Ok(candidates) = tool_command_paths(&definition, command) {
                    paths.extend(candidates);
                }
            }
            if command.is_some() && paths.is_empty() {
                return Err(WorkerServiceError::TypeChecker(format!(
                    "unknown tool command {}",
                    command.unwrap()
                )));
            }
            let mut matches = Vec::new();
            for path in paths {
                if let Some(agent_id) = agent_id {
                    let operation =
                        golem_common::model::InvocationOperation::tool_operation(tool_name, &path);
                    match self
                        .lookup_agent_invocations(
                            environment_id,
                            agent_id,
                            idempotency_key,
                            Some(operation),
                            auth_ctx.clone(),
                        )
                        .await
                    {
                        Ok(found) => matches.extend(found),
                        Err(WorkerServiceError::InvocationNotFound(_)) => {}
                        Err(error) => return Err(error),
                    }
                } else {
                    let Some(scope) = auth_ctx.idempotency_scope() else {
                        continue;
                    };
                    let unique = StandaloneToolInvocation::unique_part(
                        environment_id,
                        tool_name,
                        Some(scope),
                        &path.join(" "),
                        Some(idempotency_key),
                    );
                    let id =
                        OwnedInvocationId::new(InvocationOwner::Tool(tool_name.clone()), unique);
                    match self
                        .get_invocation(environment_id, &id, auth_ctx.clone())
                        .await
                    {
                        Ok(found) => matches.push(found.summary),
                        Err(
                            WorkerServiceError::InvocationNotFound(_)
                            | WorkerServiceError::AgentNotFound(_),
                        ) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
            if matches.is_empty() {
                return Err(WorkerServiceError::InvocationNotFound(format!(
                    "no invocation of {tool_name} has this idempotency key"
                )));
            }
            return Ok(matches);
        }
        if command.is_some() {
            return Err(WorkerServiceError::TypeChecker(
                "command requires tool".into(),
            ));
        }
        let agent_id = agent_id
            .ok_or_else(|| WorkerServiceError::TypeChecker("agent or tool is required".into()))?;
        self.lookup_agent_invocations(
            environment_id,
            agent_id,
            idempotency_key,
            operation,
            auth_ctx,
        )
        .await
    }

    async fn lookup_agent_invocations(
        &self,
        environment_id: EnvironmentId,
        agent_id: &AgentId,
        idempotency_key: &IdempotencyKey,
        operation: Option<String>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<InvocationSummary>> {
        self.require_agent_address(&OwnedAgentId::new(environment_id, agent_id))
            .await?;
        self.agent_for_invocation_read(
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
                environment_id,
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
        let agent_id = self
            .invocation_address(environment_id, invocation_id, &auth_ctx)
            .await?;
        if agent_id.type_tag() == RESERVED_TYPE_TAG {
            return Err(WorkerServiceError::InvocationNotFound(
                invocation_id.to_string(),
            ));
        }
        let agent = self
            .agent_for_invocation_read(
                environment_id,
                &agent_id,
                &auth_ctx,
                AgentVerb::CancelInvocation,
                AgentResourcePattern::InvocationId(AgentInvocationIdPattern::Uuid(
                    invocation_id.uuid(),
                )),
            )
            .await?;
        // The executor rejects an unknown ID as an invalid request, so the ID is resolved first.
        let summary = self
            .invocation_summary(environment_id, &agent_id, invocation_id, auth_ctx.clone())
            .await?;
        if summary.state.is_terminal() {
            return Ok(summary);
        }
        self.worker_client
            .cancel_invocation(
                &agent_id,
                &invocation_id.id(),
                environment_id,
                auth_ctx.clone(),
                None,
                Some(agent.metadata.fingerprint),
            )
            .await?;
        self.invocation_summary(environment_id, &agent_id, invocation_id, auth_ctx)
            .await
    }

    async fn deployed_tool(
        &self,
        environment_id: EnvironmentId,
        name: &ToolName,
    ) -> WorkerResult<golem_common::model::tool::RegisteredTool> {
        self.agent_types
            .tool_deployment(environment_id)
            .await?
            .and_then(|state| state.registered_tools.get(name).cloned())
            .ok_or_else(|| {
                WorkerServiceError::InvocationNotFound(format!("tool {name} is not deployed"))
            })
    }

    async fn invocation_address(
        &self,
        environment_id: EnvironmentId,
        id: &OwnedInvocationId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<AgentId> {
        match id.owner() {
            InvocationOwner::Agent(agent_id) => Ok(*agent_id),
            InvocationOwner::Tool(name) => {
                if !auth_ctx.is_system() {
                    self.agent_types
                        .authorize_environment(environment_id, auth_ctx)
                        .await?;
                }
                let tool = self.deployed_tool(environment_id, name).await?;
                if !matches!(tool.registration.source, ToolSource::Component { .. }) {
                    return Err(WorkerServiceError::InvocationNotFound(id.to_string()));
                }
                Ok(AgentId::standalone_tool_invocation(
                    environment_id,
                    tool.type_id,
                    id.id(),
                ))
            }
        }
    }

    /// The invocation owner after the caller is authorized for `verb` on `resource` of it.
    /// This also accepts standalone tool owners and tag-0 owners for result reads.
    async fn agent_for_invocation_read(
        &self,
        environment_id: EnvironmentId,
        agent_id: &AgentId,
        auth_ctx: &AuthCtx,
        verb: AgentVerb,
        resource: AgentResourcePattern,
    ) -> WorkerResult<ManagedAgent> {
        self.managed_agent(
            &OwnedAgentId::new(environment_id, agent_id),
            None,
            auth_ctx,
            &[AgentPermission::Agent(verb, resource)],
        )
        .await
    }

    async fn invocation_summary(
        &self,
        environment_id: EnvironmentId,
        agent_id: &AgentId,
        invocation_id: &OwnedInvocationId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<InvocationSummary> {
        let summary = self
            .worker_client
            .get_invocation(agent_id, &invocation_id.id(), environment_id, auth_ctx)
            .await?
            .ok_or_else(|| WorkerServiceError::InvocationNotFound(invocation_id.to_string()))?;
        let mut summary = summary_from_proto(agent_id, environment_id, summary)?;
        summary.invocation_id = invocation_id.to_string();
        Ok(summary)
    }

    /// The result of a terminal invocation, read through the existing result lookup.
    async fn invocation_outcome(
        &self,
        agent: &ManagedAgent,
        invocation_id: &OwnedInvocationId,
        summary: &InvocationSummary,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Option<InvocationOutcome>> {
        let agent_id = &agent.agent_id.agent_id;
        let start = InvocationStart {
            target: Some(golem_api_grpc::proto::golem::worker::AgentTarget {
                target: Some(
                    golem_api_grpc::proto::golem::worker::agent_target::Target::Id(
                        (*agent_id).into(),
                    ),
                ),
            }),
            environment_id: Some(agent.agent_id.environment_id.into()),
            idempotency_key: Some(IdempotencyKey::new(invocation_id.uuid().to_string()).into()),
            caller: Some(Caller::anonymous().into()),
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            ..Default::default()
        };
        let output = match self.lookup_agent_invocation(start, auth_ctx).await {
            Ok(output) => output,
            Err(error) => {
                if summary.state == InvocationState::Failed
                    && let Some(failure) = recorded_failure(&error)
                {
                    return Ok(Some(InvocationOutcome::Failure(failure)));
                }
                warn!(
                    invocation_id = %invocation_id,
                    error = error.to_safe_string(),
                    "Failed to read the result of a terminal invocation"
                );
                return Ok(None);
            }
        };
        let background_failure = output.background_failure.map(background_failure);
        let decoded = match output.result {
            AgentInvocationResult::AgentMethod { output: value } => {
                let method_name = summary.operation.as_deref().ok_or_else(|| {
                    WorkerServiceError::Internal(format!(
                        "the method of invocation {invocation_id} is not known"
                    ))
                });
                match method_name.and_then(|method_name| {
                    agent
                        .metadata
                        .agent_type_name
                        .as_ref()
                        .ok_or_else(|| {
                            WorkerServiceError::Internal(format!(
                                "the agent type of invocation {invocation_id} is not known"
                            ))
                        })
                        .map(|agent_type_name| (method_name, agent_type_name))
                }) {
                    Ok((method_name, agent_type_name)) => self
                        .agent_method_output_to_external(
                            agent.component.id,
                            output
                                .component_revision
                                .unwrap_or(agent.component.revision),
                            agent_type_name,
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
        let decoded = decoded.unwrap_or_else(|error| {
            warn!(
                invocation_id = %invocation_id,
                error = error.to_safe_string(),
                "Failed to decode the result of a completed invocation"
            );
            None
        });
        Ok(match background_failure {
            Some(InvocationFailure { error }) => Some(InvocationOutcome::BackgroundFailure(
                InvocationBackgroundFailure {
                    value: match decoded {
                        Some(InvocationOutcome::Method(InvocationMethodResult { value })) => {
                            Some(value)
                        }
                        _ => None,
                    },
                    error,
                },
            )),
            None => decoded,
        })
    }
}

fn tool_command_paths(
    tool: &golem_common::schema::tool::Tool,
    command: Option<&str>,
) -> WorkerResult<Vec<Vec<String>>> {
    if let Some(command) = command {
        let path = command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        return tool
            .canonical_command_path(&path)
            .map(|path| vec![path])
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker(format!("unknown tool command {command}"))
            });
    }
    fn visit(
        tool: &golem_common::schema::tool::Tool,
        index: usize,
        path: Vec<String>,
        paths: &mut Vec<Vec<String>>,
    ) {
        let node = &tool.commands.nodes[index];
        if node.body.is_some() {
            paths.push(path.clone());
        }
        for child in &node.subcommands {
            let index = child.as_usize().expect("validated command index");
            let mut path = path.clone();
            path.push(tool.commands.nodes[index].name.clone());
            visit(tool, index, path, paths);
        }
    }
    let mut paths = Vec::new();
    if !tool.commands.nodes.is_empty() {
        visit(tool, 0, Vec::new(), &mut paths);
    }
    Ok(paths)
}

/// The failure of the agent during the background work of an invocation that recorded its
/// result, shown as the error that a fresh invocation of the failed agent gets.
pub(super) fn background_failure(failure: BackgroundFailure) -> InvocationFailure {
    InvocationFailure {
        error: WorkerServiceError::GolemError(WorkerExecutorError::from(failure)).to_safe_string(),
    }
}

/// The recorded failure of an invocation, as the result lookup gives it. Any other error is a
/// failure to read the result, such as an unavailable executor, and is not the outcome.
fn recorded_failure(error: &WorkerServiceError) -> Option<InvocationFailure> {
    match error {
        WorkerServiceError::AcceptedInvocation { source, .. } => recorded_failure(source),
        WorkerServiceError::GolemError(
            WorkerExecutorError::InvocationFailed { .. }
            | WorkerExecutorError::PreviousInvocationFailed { .. }
            | WorkerExecutorError::PermissionDenied { .. }
            | WorkerExecutorError::InvalidRequest { .. },
        ) => Some(InvocationFailure {
            error: error.to_safe_string(),
        }),
        _ => None,
    }
}

fn summary_from_proto(
    agent_id: &AgentId,
    environment_id: EnvironmentId,
    summary: executor::InvocationSummary,
) -> WorkerResult<InvocationSummary> {
    let invalid = |what: &str| WorkerServiceError::Internal(format!("invalid invocation {what}"));
    let id = summary.invocation_id.ok_or_else(|| invalid("ID"))?;
    let invocation_id =
        OwnedInvocationId::new(*agent_id, golem_common::model::InvocationId(id.into()));
    let state = InvocationState::try_from(summary.state).map_err(|_| invalid("state"))?;
    let kind = InvocationKind::try_from(summary.kind).ok();
    let timestamp = |timestamp: prost_types::Timestamp| {
        SystemTime::try_from(timestamp)
            .map(DateTime::<Utc>::from)
            .map_err(|_| invalid("timestamp"))
    };
    Ok(InvocationSummary {
        invocation_id: invocation_id.to_string(),
        agent_id: *agent_id,
        environment_id,
        state,
        kind,
        operation: summary.operation,
        accepted_at: summary.accepted_at.map(timestamp).transpose()?,
        changed_at: timestamp(summary.changed_at.ok_or_else(|| invalid("change time"))?)?,
    })
}

impl From<InvocationState> for executor::InvocationLifecycleState {
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
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match executor::InvocationLifecycleState::try_from(value) {
            Ok(executor::InvocationLifecycleState::Pending) => Ok(Self::Pending),
            Ok(executor::InvocationLifecycleState::Scheduled) => Ok(Self::Scheduled),
            Ok(executor::InvocationLifecycleState::Running) => Ok(Self::Running),
            Ok(executor::InvocationLifecycleState::Waiting) => Ok(Self::Waiting),
            Ok(executor::InvocationLifecycleState::Retrying) => Ok(Self::Retrying),
            Ok(executor::InvocationLifecycleState::Interrupted) => Ok(Self::Interrupted),
            Ok(executor::InvocationLifecycleState::Completed) => Ok(Self::Completed),
            Ok(executor::InvocationLifecycleState::BackgroundFailed) => Ok(Self::BackgroundFailed),
            Ok(executor::InvocationLifecycleState::Failed) => Ok(Self::Failed),
            Ok(executor::InvocationLifecycleState::Cancelled) => Ok(Self::Cancelled),
            Ok(executor::InvocationLifecycleState::Unspecified) | Err(_) => {
                Err(format!("invalid invocation state {value}"))
            }
        }
    }
}

impl From<InvocationKind> for executor::InvocationKind {
    fn from(kind: InvocationKind) -> Self {
        match kind {
            InvocationKind::Method => Self::Method,
            InvocationKind::Constructor => Self::Constructor,
            InvocationKind::Tool => Self::Tool,
        }
    }
}

impl TryFrom<i32> for InvocationKind {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match executor::InvocationKind::try_from(value) {
            Ok(executor::InvocationKind::Method) => Ok(Self::Method),
            Ok(executor::InvocationKind::Constructor) => Ok(Self::Constructor),
            Ok(executor::InvocationKind::Tool) => Ok(Self::Tool),
            Ok(executor::InvocationKind::Unspecified) | Err(_) => {
                Err(format!("invalid invocation kind {value}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkerServiceError, recorded_failure, tool_command_paths};
    use golem_common::model::oplog::AgentError;
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use test_r::test;

    #[test]
    fn lookup_includes_commands_that_also_have_subcommands() {
        use golem_common::schema::tool::{
            CommandBody, CommandIndex, CommandNode, CommandTree, Tool,
        };
        let body = CommandBody {
            positionals: Default::default(),
            options: vec![],
            flags: vec![],
            constraints: vec![],
            stdin: None,
            stdout: None,
            stderr: None,
            result: None,
            errors: vec![],
            annotations: None,
        };
        let node = |name: &str, subcommands, body| CommandNode {
            name: name.into(),
            aliases: vec![],
            doc: Default::default(),
            http: None,
            globals: Default::default(),
            subcommands,
            body,
        };
        let tool = Tool {
            version: "1.0.0".into(),
            requires_filesystem: false,
            http_mount: None,
            schema: golem_common::schema::SchemaGraph::empty(),
            commands: CommandTree {
                nodes: vec![
                    node("tool", vec![CommandIndex(1)], None),
                    node("x", vec![CommandIndex(2)], Some(body.clone())),
                    node("y", vec![], Some(body)),
                ],
            },
        };
        assert_eq!(
            tool_command_paths(&tool, None).unwrap(),
            vec![vec!["x"], vec!["x", "y"]]
        );
        assert_eq!(
            tool_command_paths(&tool, Some("x")).unwrap(),
            vec![vec!["x"]]
        );
        assert_eq!(
            tool_command_paths(&tool, Some("x y")).unwrap(),
            vec![vec!["x", "y"]]
        );
        assert!(tool_command_paths(&tool, Some("y")).is_err());
    }

    #[test]
    fn only_a_recorded_failure_is_the_outcome_of_a_failed_invocation() {
        let failed = WorkerServiceError::GolemError(WorkerExecutorError::InvocationFailed {
            error: AgentError::DeterministicTrap("boom".to_string()),
            stderr: String::new(),
        });
        assert!(recorded_failure(&failed).is_some_and(|failure| failure.error.contains("boom")));
        assert!(
            recorded_failure(&WorkerServiceError::GolemError(
                WorkerExecutorError::permission_denied("denied")
            ))
            .is_some()
        );
        assert!(
            recorded_failure(&WorkerServiceError::GolemError(
                WorkerExecutorError::ShardingNotReady
            ))
            .is_none()
        );
        assert!(
            recorded_failure(&WorkerServiceError::Internal("unreachable".to_string())).is_none()
        );
    }
}
