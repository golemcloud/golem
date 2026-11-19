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

use crate::command::invocation::{
    InvocationKindArg, InvocationSelector, InvocationStateArg, InvocationSubcommand,
};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::error::{NonSuccessfulExit, PipedExitCode};
use crate::log::{log_error, logln};
use crate::model::agent::RawAgentId;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::invocation::{InvocationCancelView, InvocationGetView, InvocationListView};
use anyhow::{anyhow, bail};
use chrono::{DateTime, Utc};
use golem_client::api::InvocationClient;
use golem_client::model::{InvocationKind, InvocationState};
use golem_common::model::OwnedInvocationId;
use std::sync::Arc;
use uuid::Uuid;

/// The exit code of `invocation get` for a cancelled invocation, as for an interrupted command.
const CANCELLED_EXIT: u8 = 130;

pub struct InvocationCommandHandler {
    ctx: Arc<Context>,
}

impl InvocationCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: InvocationSubcommand) -> anyhow::Result<()> {
        match command {
            InvocationSubcommand::Get { selector } => self.cmd_get(selector).await,
            InvocationSubcommand::List {
                agent,
                operation,
                status,
                kind,
                since,
                until,
                page_size,
                cursor,
            } => {
                self.cmd_list(
                    agent, operation, status, kind, since, until, page_size, cursor,
                )
                .await
            }
            InvocationSubcommand::Cancel { selector } => self.cmd_cancel(selector).await,
        }
    }

    async fn cmd_get(&self, selector: InvocationSelector) -> anyhow::Result<()> {
        let (environment_id, invocation_id) = self.resolve(selector).await?;
        let clients = self.ctx.golem_clients().await?;
        let invocation = clients
            .invocation
            .get_invocation(&environment_id, &invocation_id)
            .await
            .map_service_error()?;
        let cancelled = invocation.summary.state == InvocationState::Cancelled;
        self.ctx
            .log_handler()
            .log_output(InvocationGetView(invocation))?;
        if cancelled {
            bail!(PipedExitCode(CANCELLED_EXIT));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn cmd_list(
        &self,
        agent: RawAgentId,
        operation: Option<String>,
        status: Vec<InvocationStateArg>,
        kind: Option<InvocationKindArg>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        page_size: Option<u32>,
        cursor: Option<String>,
    ) -> anyhow::Result<()> {
        let (environment_id, agent) = self.resolve_agent(agent).await?;
        let states = status.into_iter().map(state).collect::<Vec<_>>();
        let clients = self.ctx.golem_clients().await?;
        let page = clients
            .invocation
            .list_invocations(
                &environment_id,
                &agent,
                operation.as_deref(),
                (!states.is_empty()).then_some(states.as_slice()),
                kind.map(self::kind).as_ref(),
                since.as_ref(),
                until.as_ref(),
                page_size,
                cursor.as_deref(),
            )
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(InvocationListView(page))?;
        Ok(())
    }

    async fn cmd_cancel(&self, selector: InvocationSelector) -> anyhow::Result<()> {
        let (environment_id, invocation_id) = self.resolve(selector).await?;
        let clients = self.ctx.golem_clients().await?;
        let summary = clients
            .invocation
            .cancel_invocation(&environment_id, &invocation_id)
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(InvocationCancelView(summary))?;
        Ok(())
    }

    /// The environment and the invocation ID that a selector names.
    async fn resolve(&self, selector: InvocationSelector) -> anyhow::Result<(Uuid, String)> {
        let InvocationSelector {
            invocation_id,
            agent,
            idempotency_key,
            method,
        } = selector;
        if let Some(invocation_id) = invocation_id {
            invocation_id
                .parse::<OwnedInvocationId>()
                .map_err(|error| anyhow!(error))?;
            let environment = self
                .ctx
                .environment_handler()
                .resolve_environment(EnvironmentResolveMode::Any)
                .await?;
            return Ok((environment.environment_id.0, invocation_id));
        }
        let (Some(agent), Some(idempotency_key)) = (agent, idempotency_key) else {
            bail!("select an invocation by its ID, or by --agent and --idempotency-key");
        };
        let (environment_id, agent_text) = self.resolve_agent(agent).await?;
        let clients = self.ctx.golem_clients().await?;
        let mut found = clients
            .invocation
            .lookup_invocations(
                &environment_id,
                &agent_text,
                method.as_deref(),
                &idempotency_key,
            )
            .await
            .map_service_error_not_found_as_opt()?
            .map(|found| found.invocations)
            .unwrap_or_default();
        match found.len() {
            1 => Ok((environment_id, found.remove(0).invocation_id)),
            0 => {
                log_error(format!(
                    "No invocation of agent {agent_text} has the idempotency key {idempotency_key}"
                ));
                bail!(NonSuccessfulExit)
            }
            _ => {
                log_error(format!(
                    "The idempotency key {idempotency_key} was used for more than one operation of agent {agent_text}"
                ));
                logln("Select one with --method:");
                for invocation in &found {
                    logln(format!(
                        "  {}",
                        invocation.operation.as_deref().unwrap_or("<constructor>")
                    ));
                }
                bail!(NonSuccessfulExit)
            }
        }
    }

    /// The environment of an agent, and the agent as `<component UUID>/<agent name>`.
    async fn resolve_agent(&self, agent: RawAgentId) -> anyhow::Result<(Uuid, String)> {
        self.ctx.silence_app_context_init().await;
        let agent_handler = self.ctx.agent_handler();
        let agent_id_match = agent_handler.match_agent_id(agent).await?;
        let (component, agent_id) = agent_handler
            .component_by_agent_id_match(&agent_id_match)
            .await?;
        Ok((
            component.environment_id.0,
            format!("{}/{}", component.id.0, agent_id.0),
        ))
    }
}

fn state(state: InvocationStateArg) -> InvocationState {
    match state {
        InvocationStateArg::Scheduled => InvocationState::Scheduled,
        InvocationStateArg::Pending => InvocationState::Pending,
        InvocationStateArg::Running => InvocationState::Running,
        InvocationStateArg::Waiting => InvocationState::Waiting,
        InvocationStateArg::Retrying => InvocationState::Retrying,
        InvocationStateArg::Interrupted => InvocationState::Interrupted,
        InvocationStateArg::Completed => InvocationState::Completed,
        InvocationStateArg::Failed => InvocationState::Failed,
        InvocationStateArg::Cancelled => InvocationState::Cancelled,
    }
}

fn kind(kind: InvocationKindArg) -> InvocationKind {
    match kind {
        InvocationKindArg::Method => InvocationKind::Method,
        InvocationKindArg::Constructor => InvocationKind::Constructor,
        InvocationKindArg::Tool => InvocationKind::Tool,
    }
}
