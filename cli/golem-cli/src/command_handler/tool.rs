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

use crate::command::tool::{
    ToolGrantCreateArgs, ToolGrantSubcommand, ToolInvokeArgs, ToolMiddlewareGrantCreateArgs,
    ToolMiddlewareGrantSubcommand, ToolMiddlewareReleaseSubcommand, ToolMiddlewareSubcommand,
    ToolReleaseSubcommand, ToolSubcommand,
};
use crate::command_handler::Handlers;
use crate::command_handler::agent::invocation_session::CliSessionRequestProvider;
use crate::command_handler::log::render_command_output_document_masked;
use crate::context::Context;
use crate::error::PipedExitCode;
use crate::error::service::MapServiceError;
use crate::log::{LogColorize, log_action};
use crate::model::environment::{
    EnvironmentResolveMode, EnvironmentToolGrantCreateView, EnvironmentToolGrantDeleteView,
    EnvironmentToolGrantGetView, EnvironmentToolGrantListView, EnvironmentToolGrantRestoreView,
    EnvironmentToolGrantView,
};
use crate::model::format::Format;
use crate::model::tool_deployment::{DeployedToolListView, DeployedToolView};
use crate::model::tool_invoke::{ToolInvocationSessionView, ToolInvokeView};
use crate::model::tool_middleware::*;
use crate::model::tool_release::{ToolReleaseListView, ToolReleaseView};
use anyhow::{anyhow, bail};
use golem_client::api::{
    AgentClient, EnvironmentClient, EnvironmentToolGrantsClient,
    EnvironmentToolMiddlewareGrantsClient, ToolMiddlewareReleasesClient, ToolReleasesClient,
};
use golem_client::invocation_session::{
    InvocationSession, InvocationSessionStateSnapshot, drive_native_tool_session_until,
};
use golem_client::model::{NativeToolInvocationMode, NativeToolInvocationRequest};
use golem_common::base_model::environment_tool_grant::{
    EnvironmentToolGrantCreation, EnvironmentToolGrantDeletion,
};
use golem_common::base_model::tool_release::{
    ToolReleaseByCoordinates, ToolReleaseById, ToolReleaseReference,
};
use golem_common::model::environment_tool_middleware_grant::{
    EnvironmentToolMiddlewareGrantCreation, EnvironmentToolMiddlewareGrantDeletion,
};
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_VERSION, PublicClientMessage, PublicNativeToolTarget, PublicTypedValue,
};
use golem_common::model::tool_middleware_release::{
    ToolMiddlewareReleaseByCoordinates, ToolMiddlewareReleaseById, ToolMiddlewareReleaseReference,
};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::{ExternalTypedSchemaValue, SchemaGraph, SchemaType, SchemaValue};
use std::io::Read;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

pub struct ToolCommandHandler {
    ctx: Arc<Context>,
}

impl ToolCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: ToolSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ToolSubcommand::List => self.cmd_list().await,
            ToolSubcommand::Get { tool_name } => self.cmd_get(tool_name).await,
            ToolSubcommand::Invoke(args) => self.cmd_invoke(args).await,
            ToolSubcommand::Release { subcommand } => self.handle_release(subcommand).await,
            ToolSubcommand::Grant { subcommand } => self.handle_grant(subcommand).await,
            ToolSubcommand::Middleware { subcommand } => self.handle_middleware(subcommand).await,
        }
    }

    async fn handle_middleware(&self, command: ToolMiddlewareSubcommand) -> anyhow::Result<()> {
        match command {
            ToolMiddlewareSubcommand::List => {
                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;
                let middlewares = environment
                    .with_current_deployment_revision_or_default_warn(|revision| async move {
                        Ok(self
                            .ctx
                            .golem_clients()
                            .await?
                            .environment
                            .list_deployment_registered_tool_middlewares(
                                &environment.environment_id.0,
                                revision.into(),
                            )
                            .await
                            .map_service_error()?
                            .values)
                    })
                    .await?;
                self.ctx
                    .log_handler()
                    .log_output(DeployedToolMiddlewareListView { middlewares })?;
            }
            ToolMiddlewareSubcommand::Get { middleware_name } => {
                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;
                let revision = environment.current_deployment_or_err()?.deployment_revision;
                let middleware = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment
                    .get_deployment_registered_tool_middleware(
                        &environment.environment_id.0,
                        revision.into(),
                        middleware_name.as_str(),
                    )
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(DeployedToolMiddlewareView { middleware })?;
            }
            ToolMiddlewareSubcommand::Release { subcommand } => {
                self.handle_middleware_release(subcommand).await?
            }
            ToolMiddlewareSubcommand::Grant { subcommand } => {
                self.handle_middleware_grant(subcommand).await?
            }
        }
        Ok(())
    }

    async fn handle_middleware_release(
        &self,
        command: ToolMiddlewareReleaseSubcommand,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        let output = match command {
            ToolMiddlewareReleaseSubcommand::List { account_id } => {
                let account_id = account_id.unwrap_or(*clients.account_id());
                let releases = clients
                    .tool_middleware_releases
                    .list_account_tool_middleware_releases(&account_id.0)
                    .await
                    .map_service_error()?
                    .values;
                return self
                    .ctx
                    .log_handler()
                    .log_output(ToolMiddlewareReleaseListView { releases });
            }
            ToolMiddlewareReleaseSubcommand::Get { release_id } => clients
                .tool_middleware_releases
                .get_tool_middleware_release(&release_id.0)
                .await
                .map_service_error()?,
            ToolMiddlewareReleaseSubcommand::DePublish { release_id } => clients
                .tool_middleware_releases
                .de_publish_tool_middleware_release(&release_id.0)
                .await
                .map_service_error()?,
            ToolMiddlewareReleaseSubcommand::Restore { release_id } => clients
                .tool_middleware_releases
                .restore_tool_middleware_release(&release_id.0)
                .await
                .map_service_error()?,
        };
        self.ctx
            .log_handler()
            .log_output(ToolMiddlewareReleaseView { release: output })?;
        Ok(())
    }

    async fn handle_middleware_grant(
        &self,
        command: ToolMiddlewareGrantSubcommand,
    ) -> anyhow::Result<()> {
        match command {
            ToolMiddlewareGrantSubcommand::Create(args) => {
                self.cmd_middleware_grant_create(args).await?
            }
            ToolMiddlewareGrantSubcommand::List => {
                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;
                let grants = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_middleware_grants
                    .list_environment_tool_middleware_grants(&environment.environment_id.0)
                    .await
                    .map_service_error()?
                    .values
                    .into_iter()
                    .map(Into::into)
                    .collect();
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolMiddlewareGrantListView { grants })?;
            }
            ToolMiddlewareGrantSubcommand::Get { grant_id } => {
                let grant = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_middleware_grants
                    .get_environment_tool_middleware_grant(&grant_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolMiddlewareGrantGetView {
                        grant: grant.into(),
                    })?;
            }
            ToolMiddlewareGrantSubcommand::Delete { grant_id } => {
                self.ctx
                    .golem_clients()
                    .await?
                    .environment_tool_middleware_grants
                    .delete_environment_tool_middleware_grant(
                        &grant_id.0,
                        &EnvironmentToolMiddlewareGrantDeletion { automatic: false },
                    )
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolMiddlewareGrantDeleteView { grant_id })?;
            }
            ToolMiddlewareGrantSubcommand::Restore { grant_id } => {
                let grant = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_middleware_grants
                    .restore_environment_tool_middleware_grant(&grant_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolMiddlewareGrantRestoreView {
                        grant: grant.into(),
                    })?;
            }
        }
        Ok(())
    }

    async fn cmd_middleware_grant_create(
        &self,
        args: ToolMiddlewareGrantCreateArgs,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;
        let release = match (args.release_id, args.account, args.name, args.version) {
            (Some(release_id), None, None, None) => {
                ToolMiddlewareReleaseReference::ById(ToolMiddlewareReleaseById { release_id })
            }
            (None, Some(account), Some(name), Some(version)) => {
                ToolMiddlewareReleaseReference::ByCoordinates(ToolMiddlewareReleaseByCoordinates {
                    account,
                    name,
                    version,
                })
            }
            _ => unreachable!("clap validates the tool middleware release reference"),
        };
        let grant = self
            .ctx
            .golem_clients()
            .await?
            .environment_tool_middleware_grants
            .create_environment_tool_middleware_grant(
                &environment.environment_id.0,
                &EnvironmentToolMiddlewareGrantCreation {
                    release,
                    automatic: false,
                },
            )
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(EnvironmentToolMiddlewareGrantCreateView {
                grant: grant.into(),
            })?;
        Ok(())
    }

    async fn cmd_invoke(&self, args: ToolInvokeArgs) -> anyhow::Result<()> {
        if args.input.as_deref() == Some("-")
            && args
                .stdin
                .as_deref()
                .is_some_and(|path| path == Path::new("-"))
        {
            bail!("--input '-' cannot be used together with --stdin '-'");
        }
        if args.lookup && args.input.is_some() {
            bail!("--lookup cannot be used with --input");
        }
        if (args.stdin.is_some() || args.stdout) && (args.trigger || args.schedule_at.is_some()) {
            bail!("--trigger and --schedule-at cannot be used with live streams");
        }
        if args.lookup
            && args
                .idempotency_key
                .as_ref()
                .is_none_or(|key| key.value == "-")
        {
            bail!("--lookup requires an explicit --idempotency-key other than '-'");
        }
        let key = match args.idempotency_key {
            Some(key) if key.value != "-" => key,
            _ => IdempotencyKey::fresh(),
        };
        log_action(
            "Using",
            format!("idempotency key: {}", key.value.log_color_highlight()),
        );
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;
        let (target_environment, agent_id, component_id) = if let Some(raw) = args.agent {
            let matched = self.ctx.agent_handler().match_agent_id(raw).await?;
            let component = self
                .ctx
                .component_handler()
                .resolve_component(&matched.environment, &matched.component_name, None)
                .await?
                .ok_or_else(|| anyhow!("Component '{}' is not deployed", matched.component_name))?;
            (
                matched.environment,
                Some(AgentId {
                    component_id: component.id,
                    agent_id: matched.agent_id.0,
                }),
                None,
            )
        } else if let Some(name) = args.component {
            let component = self
                .ctx
                .component_handler()
                .resolve_component(&environment, &name, None)
                .await?
                .ok_or_else(|| anyhow!("Component '{name}' is not deployed"))?;
            (environment.clone(), None, Some(component.id))
        } else {
            unreachable!("clap requires exactly one native tool target")
        };
        let input = args
            .input
            .map(|source| -> anyhow::Result<ExternalTypedSchemaValue> {
                let json = if source == "-" {
                    std::io::read_to_string(std::io::stdin())
                        .map_err(|e| anyhow!("Failed to read tool input from stdin: {e}"))?
                } else if let Some(path) = source.strip_prefix('@') {
                    std::fs::read_to_string(path)
                        .map_err(|e| anyhow!("Failed to read tool input '{path}': {e}"))?
                } else {
                    source
                };
                serde_json::from_str(&json)
                    .map_err(|e| anyhow!("Invalid schema-typed tool input: {e}"))
            })
            .transpose()?;
        let live = args.stdin.is_some() || args.stdout;
        if live {
            let public_target = match (&agent_id, component_id) {
                (Some(agent_id), None) => PublicNativeToolTarget::Agent {
                    component_id: agent_id.component_id.0,
                    agent_id: agent_id.agent_id.clone(),
                },
                (None, Some(component_id)) => PublicNativeToolTarget::Component {
                    component_id: component_id.0,
                },
                _ => unreachable!("native tool target was resolved above"),
            };
            let input = public_typed_value(input)?;
            let initial = InvocationSessionStateSnapshot {
                delivered_output_cursors: Default::default(),
                pending_operation: Some(PublicClientMessage::ToolStart {
                    attempt_id: uuid::Uuid::new_v4(),
                    application: target_environment.application_name.to_string(),
                    environment: target_environment.environment_name.to_string(),
                    idempotency_key: key.value.clone(),
                    tool_name: args.tool_name.to_string(),
                    command_path: args.command_path,
                    target: public_target.clone(),
                    input,
                    stdin: args.stdin.is_some(),
                    stdout: args.stdout,
                    version: INVOCATION_SESSION_VERSION,
                }),
                session_token: None,
            };
            let raw_to_process_stdout =
                live_report_destination(args.stdout, args.output.as_deref())
                    == LiveReportDestination::Stderr;
            let mut reader = match args.stdin {
                Some(path) if path == Path::new("-") => Some(process_stdin_reader()),
                Some(path) => Some(Box::pin(tokio::fs::File::open(&path).await.map_err(
                    |error| anyhow!("Failed to open tool stdin '{}': {error}", path.display()),
                )?) as Pin<Box<dyn AsyncRead + Send>>),
                None => None,
            };
            let mut writer: Pin<Box<dyn AsyncWrite + Send>> = match args.output {
                Some(path) => Box::pin(tokio::fs::File::create(&path).await.map_err(|error| {
                    anyhow!("Failed to create tool stdout '{}': {error}", path.display())
                })?),
                None if args.stdout => Box::pin(tokio::io::stdout()),
                None => Box::pin(tokio::io::sink()),
            };
            let session = InvocationSession::open(
                Arc::new(CliSessionRequestProvider::new(self.ctx.clone())),
                None,
                initial,
                false,
                Arc::new(()),
            )
            .await
            .map_err(anyhow::Error::msg)?;
            let cancelled = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            let result =
                drive_native_tool_session_until(session, reader.take(), &mut writer, cancelled)
                    .await
                    .map_err(|error| match &error {
                        golem_client::invocation_session::SessionTransportError::Io(io_error)
                            if io_error.kind() == std::io::ErrorKind::Interrupted =>
                        {
                            anyhow!(PipedExitCode(130))
                        }
                        _ => anyhow!(error),
                    })?;
            let view = ToolInvocationSessionView {
                target: public_target,
                idempotency_key: key,
                result,
            };
            if raw_to_process_stdout {
                let format = if self.ctx.format() == Format::Text {
                    Format::PrettyJson
                } else {
                    self.ctx.format()
                };
                eprintln!(
                    "{}",
                    render_command_output_document_masked(
                        format,
                        self.ctx.should_colorize(),
                        self.ctx.masking_config(),
                        view,
                    )?
                );
                return Ok(());
            }
            return self.ctx.log_handler().log_output(view);
        }
        let mode = if args.lookup {
            NativeToolInvocationMode::Lookup
        } else if args.trigger {
            NativeToolInvocationMode::Schedule
        } else {
            NativeToolInvocationMode::Await
        };
        let input = if args.lookup {
            input
        } else {
            Some(
                tool_input_or_empty(input)
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            )
        };
        let clients = self.ctx.golem_clients().await?;
        let response = clients
            .agent
            .invoke_tool(
                Some(&key.value),
                &NativeToolInvocationRequest {
                    app_name: target_environment.application_name.to_string(),
                    env_name: target_environment.environment_name.to_string(),
                    agent_id,
                    component_id: component_id.map(|id| id.0),
                    tool_name: args.tool_name.to_string(),
                    command_path: args.command_path,
                    input,
                    mode,
                    schedule_at: args.schedule_at,
                    idempotency_key: Some(key.value.clone()),
                },
            )
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(ToolInvokeView { response })
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;
        let tools = environment
            .with_current_deployment_revision_or_default_warn(|revision| async move {
                Ok(self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment
                    .list_deployment_registered_tools(
                        &environment.environment_id.0,
                        revision.into(),
                    )
                    .await
                    .map_service_error()?
                    .values)
            })
            .await?;
        self.ctx
            .log_handler()
            .log_output(DeployedToolListView { tools })?;
        Ok(())
    }

    async fn cmd_get(
        &self,
        tool_name: golem_common::base_model::tool::ToolName,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;
        let revision = environment.current_deployment_or_err()?.deployment_revision;
        let tool = self
            .ctx
            .golem_clients()
            .await?
            .environment
            .get_deployment_registered_tool(
                &environment.environment_id.0,
                revision.into(),
                tool_name.as_str(),
            )
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(DeployedToolView { tool })?;
        Ok(())
    }

    async fn handle_release(&self, subcommand: ToolReleaseSubcommand) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        match subcommand {
            ToolReleaseSubcommand::List { account_id } => {
                let account_id = account_id.unwrap_or(*clients.account_id());
                let releases = clients
                    .tool_releases
                    .list_account_tool_releases(&account_id.0)
                    .await
                    .map_service_error()?
                    .values;
                self.ctx
                    .log_handler()
                    .log_output(ToolReleaseListView { releases })?;
            }
            ToolReleaseSubcommand::Get { release_id } => {
                let release = clients
                    .tool_releases
                    .get_tool_release(&release_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(ToolReleaseView { release })?;
            }
            ToolReleaseSubcommand::DePublish { release_id } => {
                let release = clients
                    .tool_releases
                    .de_publish_tool_release(&release_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(ToolReleaseView { release })?;
            }
            ToolReleaseSubcommand::Restore { release_id } => {
                let release = clients
                    .tool_releases
                    .restore_tool_release(&release_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(ToolReleaseView { release })?;
            }
        }
        Ok(())
    }

    async fn handle_grant(&self, subcommand: ToolGrantSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ToolGrantSubcommand::Create(args) => self.cmd_grant_create(args).await,
            ToolGrantSubcommand::List => {
                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;
                let grants = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_grants
                    .list_environment_tool_grants(&environment.environment_id.0)
                    .await
                    .map_service_error()?
                    .values
                    .into_iter()
                    .map(Into::into)
                    .collect();
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolGrantListView { grants })?;
                Ok(())
            }
            ToolGrantSubcommand::Get { grant_id } => {
                let grant = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_grants
                    .get_environment_tool_grant(&grant_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolGrantGetView {
                        grant: grant.into(),
                    })?;
                Ok(())
            }
            ToolGrantSubcommand::Delete { grant_id } => {
                self.ctx
                    .golem_clients()
                    .await?
                    .environment_tool_grants
                    .delete_environment_tool_grant(
                        &grant_id.0,
                        &EnvironmentToolGrantDeletion { automatic: false },
                    )
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolGrantDeleteView { grant_id })?;
                Ok(())
            }
            ToolGrantSubcommand::Restore { grant_id } => {
                let grant = self
                    .ctx
                    .golem_clients()
                    .await?
                    .environment_tool_grants
                    .restore_environment_tool_grant(&grant_id.0)
                    .await
                    .map_service_error()?;
                self.ctx
                    .log_handler()
                    .log_output(EnvironmentToolGrantRestoreView {
                        grant: grant.into(),
                    })?;
                Ok(())
            }
        }
    }

    async fn cmd_grant_create(&self, args: ToolGrantCreateArgs) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;
        let release = match (args.release_id, args.account, args.name, args.version) {
            (Some(release_id), None, None, None) => {
                ToolReleaseReference::ById(ToolReleaseById { release_id })
            }
            (None, Some(account), Some(name), Some(version)) => {
                ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
                    account,
                    name,
                    version,
                })
            }
            _ => unreachable!("clap validates the tool release reference"),
        };
        let grant = self
            .ctx
            .golem_clients()
            .await?
            .environment_tool_grants
            .create_environment_tool_grant(
                &environment.environment_id.0,
                &EnvironmentToolGrantCreation {
                    release,
                    automatic: false,
                },
            )
            .await
            .map_service_error()?;
        self.ctx
            .log_handler()
            .log_output(EnvironmentToolGrantCreateView {
                grant: EnvironmentToolGrantView::from(grant),
            })?;
        Ok(())
    }
}

fn tool_input_or_empty(
    input: Option<ExternalTypedSchemaValue>,
) -> golem_common::schema::TypedSchemaValue {
    input
        .map(ExternalTypedSchemaValue::into_inner)
        .unwrap_or_else(|| {
            golem_common::schema::TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(vec![])),
                SchemaValue::Record { fields: vec![] },
            )
        })
}

fn public_typed_value(input: Option<ExternalTypedSchemaValue>) -> anyhow::Result<PublicTypedValue> {
    let (schema, value) = tool_input_or_empty(input).into_parts();
    let value = golem_common::schema::public_json::encode_public_schema_value(
        &schema,
        &schema.root,
        &value,
        |_, _| {
            Err(golem_common::schema::public_json::PublicSchemaValueError::new(
            golem_common::model::invocation_session_public::PublicErrorCode::UnsupportedValue,
            "typed tool input cannot contain streams; use --stdin",
        ))
        },
    )?;
    Ok(PublicTypedValue { schema, value })
}

fn process_stdin_reader() -> Pin<Box<dyn AsyncRead + Send>> {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = [0; 64 * 1024];
        loop {
            let item = match stdin.read(&mut buffer) {
                Ok(0) => return,
                Ok(count) => Ok(bytes::Bytes::copy_from_slice(&buffer[..count])),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => Err(error),
            };
            let failed = item.is_err();
            if sender.blocking_send(item).is_err() || failed {
                return;
            }
        }
    });
    Box::pin(tokio_util::io::StreamReader::new(
        tokio_stream::wrappers::ReceiverStream::new(receiver),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiveReportDestination {
    Stdout,
    Stderr,
}

fn live_report_destination(stdout: bool, output: Option<&Path>) -> LiveReportDestination {
    if stdout && output.is_none() {
        LiveReportDestination::Stderr
    } else {
        LiveReportDestination::Stdout
    }
}

#[cfg(test)]
mod tests {
    use super::{LiveReportDestination, live_report_destination, public_typed_value};
    use golem_common::schema::{
        NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
    };
    use std::path::Path;
    use test_r::test;

    #[test]
    fn native_session_input_uses_public_schema_json_not_internal_value_encoding() {
        let graph = SchemaGraph::anonymous(SchemaType::record(vec![
            NamedFieldType {
                name: "mode".to_string(),
                body: SchemaType::string(),
                metadata: Default::default(),
            },
            NamedFieldType {
                name: "count".to_string(),
                body: SchemaType::u64(),
                metadata: Default::default(),
            },
        ]));
        let value = TypedSchemaValue::new(
            graph.clone(),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("marker-echo".to_string()),
                    SchemaValue::U64(u64::MAX),
                ],
            },
        );
        let public = public_typed_value(Some(value.try_into().unwrap())).unwrap();
        assert_eq!(public.schema, graph);
        assert_eq!(
            public.value,
            serde_json::json!({"mode": "marker-echo", "count": "18446744073709551615"})
        );
        assert_eq!(
            public_typed_value(None).unwrap().value,
            serde_json::json!({})
        );
    }

    #[test]
    fn raw_process_stdout_keeps_structured_report_on_stderr() {
        assert_eq!(
            live_report_destination(true, None),
            LiveReportDestination::Stderr
        );
        assert_eq!(
            live_report_destination(true, Some(Path::new("raw.bin"))),
            LiveReportDestination::Stdout
        );
        assert_eq!(
            live_report_destination(false, None),
            LiveReportDestination::Stdout
        );
    }
}
