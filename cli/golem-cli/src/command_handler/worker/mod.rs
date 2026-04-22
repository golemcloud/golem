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

mod stream;
mod stream_output;

use crate::command::shared_args::{
    AgentIdArgs, PostDeployArgs, StreamArgs, WorkerFunctionArgument, WorkerFunctionName,
};
use crate::command::worker::AgentSubcommand;
use crate::command_handler::Handlers;
use crate::command_handler::worker::stream::WorkerConnection;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::{AnyhowMapServiceError, ServiceError};
use crate::fuzzy::{Error, FuzzySearch};
use crate::log::{
    LogColorize, LogIndent, log_action, log_error, log_error_action, log_failed_to, log_warn,
    log_warn_action, logln,
};
use crate::model::component::{ComponentNameMatchKind, show_exported_agent_constructors};
use crate::model::deploy::{TryUpdateAllWorkersResult, WorkerUpdateAttempt};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::fmt::{log_fuzzy_match, log_text_view};
use crate::model::text::help::{
    AvailableAgentConstructorsHelp, AvailableFunctionNamesHelp, WorkerNameHelp,
};
use crate::model::text::worker::{
    FileNodeView, WorkerCreateView, WorkerFilesView, WorkerGetView, format_agent_name_match,
    format_timestamp,
};
use anyhow::{Context as AnyhowContext, anyhow, bail};
use chrono::{DateTime, Utc};
use colored::Colorize;

use crate::agent_id_display::SourceLanguage;
use crate::model::environment::{
    EnvironmentReference, EnvironmentResolveMode, ResolvedEnvironmentIdentity,
};
use crate::model::worker::{
    AgentMetadata, AgentMetadataView, AgentNameMatch, AgentUpdateMode, AgentsMetadataResponseView,
    RawAgentId,
};
use golem_client::api::{AgentClient, ComponentClient, WorkerClient};
use golem_client::model::ScanCursor;
use golem_client::model::{
    AgentInvocationMode, AgentInvocationRequest, ComponentDto, RevertWorkerTarget,
    UpdateWorkerRequest,
};
use golem_common::model::agent::{
    AgentType, AgentTypeName, DataValue, ParsedAgentId, UntypedJsonDataValue,
};
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component_metadata::ParsedFunctionSite;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::oplog::{OplogCursor, PublicOplogEntry};
use golem_common::model::worker::{
    AgentConfigEntryDto, RevertLastInvocations, RevertToOplogIndex, UpdateRecord,
};
use golem_common::model::{IdempotencyKey, OplogIndex};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::execute;
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use inquire::Confirm;
use itertools::Itertools;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Stdout, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use terminal_size::terminal_size;
use tokio::time::{sleep, timeout};
use tracing::debug;
use uuid::Uuid;

pub struct WorkerCommandHandler {
    ctx: Arc<Context>,
}

impl WorkerCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn handle_command(
        &self,
        subcommand: AgentSubcommand,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + '_>> {
        Box::pin(async move {
            match subcommand {
                AgentSubcommand::New {
                    agent_id: agent_name,
                    env,
                    wasi_config,
                    config,
                } => self.cmd_new(agent_name, env, wasi_config, config).await,
                AgentSubcommand::Invoke {
                    agent_id: agent_name,
                    function_name,
                    arguments,
                    trigger,
                    idempotency_key,
                    no_stream,
                    stream_args,
                    post_deploy_args,
                    schedule_at,
                } => {
                    self.cmd_invoke(
                        agent_name,
                        &function_name,
                        arguments,
                        trigger,
                        idempotency_key,
                        no_stream,
                        stream_args,
                        post_deploy_args,
                        schedule_at,
                    )
                    .await
                }
                AgentSubcommand::Get {
                    agent_id: agent_name,
                } => self.cmd_get(agent_name).await,
                AgentSubcommand::Delete {
                    agent_id: agent_name,
                } => self.cmd_delete(agent_name).await,
                AgentSubcommand::List {
                    agent_type_name,
                    component_name,
                    filter: filters,
                    scan_cursor,
                    max_count,
                    precise,
                    refresh,
                } => {
                    self.cmd_list(
                        agent_type_name,
                        component_name,
                        filters,
                        scan_cursor,
                        max_count,
                        precise,
                        refresh,
                    )
                    .await
                }
                AgentSubcommand::Stream {
                    agent_id: agent_name,
                    stream_args,
                } => self.cmd_stream(agent_name, stream_args).await,
                AgentSubcommand::ReplStream {
                    agent_type_name,
                    parameters,
                    idempotency_key,
                    phantom_id,
                    stream_args,
                } => {
                    self.cmd_repl_stream(
                        agent_type_name,
                        parameters,
                        idempotency_key,
                        phantom_id,
                        stream_args,
                    )
                    .await
                }
                AgentSubcommand::Interrupt {
                    agent_id: agent_name,
                } => self.cmd_interrupt(agent_name).await,
                AgentSubcommand::Update {
                    agent_id: agent_name,
                    mode,
                    target_revision,
                    r#await,
                    disable_wakeup,
                } => {
                    self.cmd_update(
                        agent_name,
                        mode.unwrap_or(AgentUpdateMode::Automatic),
                        target_revision,
                        r#await,
                        disable_wakeup,
                    )
                    .await
                }
                AgentSubcommand::Resume {
                    agent_id: agent_name,
                } => self.cmd_resume(agent_name).await,
                AgentSubcommand::SimulateCrash {
                    agent_id: agent_name,
                } => self.cmd_simulate_crash(agent_name).await,
                AgentSubcommand::Oplog {
                    agent_id: agent_name,
                    from,
                    query,
                } => self.cmd_oplog(agent_name, from, query).await,
                AgentSubcommand::Revert {
                    agent_id: agent_name,
                    last_oplog_index,
                    number_of_invocations,
                } => {
                    self.cmd_revert(agent_name, last_oplog_index, number_of_invocations)
                        .await
                }
                AgentSubcommand::CancelInvocation {
                    agent_id: agent_name,
                    idempotency_key,
                } => {
                    self.cmd_cancel_invocation(agent_name, idempotency_key)
                        .await
                }
                AgentSubcommand::Files { agent_name, path } => {
                    self.cmd_files(agent_name, path).await
                }
                AgentSubcommand::FileContents {
                    agent_name,
                    path,
                    output,
                } => self.cmd_file_contents(agent_name, path, output).await,
                AgentSubcommand::ActivatePlugin {
                    agent_id: agent_name,
                    plugin_name,
                    plugin_priority,
                } => {
                    self.cmd_activate_plugin(agent_name, plugin_name, plugin_priority)
                        .await
                }
                AgentSubcommand::DeactivatePlugin {
                    agent_id: agent_name,
                    plugin_name,
                    plugin_priority,
                } => {
                    self.cmd_deactivate_plugin(agent_name, plugin_name, plugin_priority)
                        .await
                }
            }
        })
    }

    async fn cmd_new(
        &self,
        agent_name: AgentIdArgs,
        env: Vec<(String, String)>,
        wasi_config: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let agent_name = agent_name.agent_id;
        let agent_name_match = self.match_agent_name(agent_name).await?;
        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                &agent_name_match.environment,
                agent_name_match.component_name_match_kind,
                &agent_name_match.component_name,
                Some((&agent_name_match.agent_name).into()),
                None,
                None,
                false,
            )
            .await?;

        let agent_name =
            self.try_recanonicalize_agent_name(&agent_name_match.agent_name, &component);
        let agent_name =
            match self.validate_worker_and_function_names(&component, &agent_name, None)? {
                Some((agent_id, agent_type)) => {
                    RawAgentId(normalize_public_agent_id(&agent_id, &agent_type)?.to_string())
                }
                None => agent_name,
            };

        log_action(
            "Creating",
            format!("new agent {}", format_agent_name_match(&agent_name_match)),
        );

        self.new_worker(
            component.id.0,
            agent_name.0.clone(),
            env.into_iter().collect(),
            BTreeMap::from_iter(wasi_config),
            config,
        )
        .await?;

        logln("");
        self.ctx.log_handler().log_view(&WorkerCreateView {
            component_name: agent_name_match.component_name,
            agent_name: Some(agent_name),
        });

        Ok(())
    }

    async fn cmd_invoke(
        &self,
        agent_name: AgentIdArgs,
        function_name: &WorkerFunctionName,
        arguments: Vec<WorkerFunctionArgument>,
        trigger: bool,
        idempotency_key: Option<IdempotencyKey>,
        no_stream: bool,
        stream_args: StreamArgs,
        post_deploy_args: Option<PostDeployArgs>,
        schedule_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        fn new_idempotency_key() -> IdempotencyKey {
            let key = IdempotencyKey::fresh();
            log_action(
                "Using",
                format!(
                    "generated idempotency key: {}",
                    key.value.log_color_highlight()
                ),
            );
            key
        }

        let idempotency_key = match idempotency_key {
            Some(idempotency_key) if idempotency_key.value == "-" => new_idempotency_key(),
            Some(idempotency_key) => {
                log_action(
                    "Using",
                    format!(
                        "requested idempotency key: {}",
                        idempotency_key.value.log_color_highlight()
                    ),
                );
                idempotency_key
            }
            None => new_idempotency_key(),
        };

        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;

        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                &agent_name_match.environment,
                agent_name_match.component_name_match_kind,
                &agent_name_match.component_name,
                Some((&agent_name_match.agent_name).into()),
                post_deploy_args.as_ref(),
                None,
                false,
            )
            .await?;

        // First, validate without the function name
        let agent_name =
            self.try_recanonicalize_agent_name(&agent_name_match.agent_name, &component);
        let agent_id_and_type =
            self.validate_worker_and_function_names(&component, &agent_name, None)?;

        let (agent_id, agent_type) =
            agent_id_and_type.ok_or_else(|| anyhow!("Agent invoke requires an agent component"))?;
        let agent_id = normalize_public_agent_id(&agent_id, &agent_type)?;

        // If the function name is fully qualified (e.g., "rust:agent/foo-agent.{fun-string}"),
        // extract the simple method name for fuzzy matching.
        let method_pattern = extract_simple_method_name(function_name);

        let matched_method_name = resolve_agent_method_name(&method_pattern, &agent_type);
        let method_name = match matched_method_name {
            Ok(match_) => {
                log_fuzzy_match(&match_);
                match_.option
            }
            Err(error) => match error {
                Error::Ambiguous {
                    highlighted_options,
                    ..
                } => {
                    logln("");
                    log_error(format!(
                        "The requested method name ({}) is ambiguous.",
                        function_name.log_color_error_highlight()
                    ));
                    logln("");
                    logln("Did you mean one of");
                    for option in highlighted_options {
                        logln(format!(" - {}", option.bold()));
                    }
                    logln("?");
                    logln("");
                    log_text_view(&AvailableFunctionNamesHelp::new_agent(
                        &component,
                        &agent_id,
                        &agent_type,
                    ));

                    bail!(NonSuccessfulExit);
                }
                Error::NotFound { .. } => {
                    logln("");
                    log_error(format!(
                        "The requested method name ({}) was not found.",
                        function_name.log_color_error_highlight()
                    ));
                    logln("");
                    log_text_view(&AvailableFunctionNamesHelp::new_agent(
                        &component,
                        &agent_id,
                        &agent_type,
                    ));

                    bail!(NonSuccessfulExit);
                }
            },
        };

        // Update agent_name with normalized agent id
        let agent_name_match = AgentNameMatch {
            agent_name: agent_id.to_string().into(),
            ..agent_name_match
        };

        let mode = if trigger {
            log_action(
                "Triggering",
                format!(
                    "invocation for agent {}/{}",
                    format_agent_name_match(&agent_name_match),
                    method_name.log_color_highlight()
                ),
            );
            AgentInvocationMode::Schedule
        } else {
            log_action(
                "Invoking",
                format!(
                    "agent {}/{} ",
                    format_agent_name_match(&agent_name_match),
                    method_name.log_color_highlight()
                ),
            );
            AgentInvocationMode::Await
        };

        let source_language = SourceLanguage::from(agent_type.source_language.as_str());
        let method = agent_type
            .methods
            .iter()
            .find(|m| m.name == method_name)
            .ok_or_else(|| anyhow!("Method '{}' not found in agent type", method_name))?;
        let joined_args = arguments.join(",");
        let method_parameters = crate::agent_id_display::parse_agent_id_params(
            &joined_args,
            &method.input_schema,
            &source_language,
        )
        .map_err(|e| anyhow!("Failed to parse method parameters: {e}"))?;
        let method_parameters = UntypedJsonDataValue::from(method_parameters);

        let mut connect_handle = if !no_stream {
            let connection = WorkerConnection::new(
                self.ctx.worker_service_url().clone(),
                self.ctx.auth_token().await?,
                &component.id,
                agent_id.to_string(),
                stream_args.into(),
                self.ctx.allow_insecure(),
                self.ctx.format(),
                if trigger {
                    None
                } else {
                    Some(idempotency_key.clone())
                },
            )
            .await?;
            Some(tokio::task::spawn(
                async move { connection.run_forever().await },
            ))
        } else {
            None
        };

        let environment = &agent_name_match.environment;

        let request = AgentInvocationRequest {
            app_name: environment.application_name.to_string(),
            env_name: environment.environment_name.to_string(),
            agent_type_name: agent_id.agent_type.0.clone(),
            parameters: UntypedJsonDataValue::from(agent_id.parameters.clone()),
            phantom_id: agent_id.phantom_id,
            method_name: method_name.clone(),
            method_parameters,
            mode,
            schedule_at,
            idempotency_key: Some(idempotency_key.value.clone()),
            deployment_revision: None,
            owner_account_email: None,
        };

        let clients = self.ctx.golem_clients().await?;
        let result = clients
            .agent
            .invoke_agent(Some(&idempotency_key.value), &request)
            .await
            .map_service_error()?;

        if let Some(handle) = connect_handle.take() {
            let _ = timeout(Duration::from_secs(3), handle).await;
        }

        if trigger {
            log_action("Triggered", "invocation");
            self.ctx
                .log_handler()
                .log_view(&InvokeResultView::new_trigger(idempotency_key));
        } else {
            logln("");
            self.ctx
                .log_handler()
                .log_view(&InvokeResultView::new_agent_invoke(
                    idempotency_key,
                    result,
                    &agent_type,
                    &method_name,
                ));
        }

        Ok(())
    }

    async fn cmd_stream(
        &self,
        agent_name: AgentIdArgs,
        stream_args: StreamArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_action(
            "Connecting",
            format!("to agent {}", format_agent_name_match(&agent_name_match)),
        );

        let connection = WorkerConnection::new(
            self.ctx.worker_service_url().clone(),
            self.ctx.auth_token().await?,
            &component.id,
            agent_name.0.clone(),
            stream_args.into(),
            self.ctx.allow_insecure(),
            self.ctx.format(),
            None,
        )
        .await?;

        connection.run_forever().await;

        Ok(())
    }

    async fn cmd_repl_stream(
        &self,
        agent_type_name: String,
        parameters: String,
        idempotency_key: IdempotencyKey,
        phantom_id: Option<Uuid>,
        stream_args: StreamArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await?;

        let parameters: UntypedJsonDataValue = serde_json::from_str(&parameters)
            .with_context(|| "Failed to deserialize agent parameters".to_string())?;

        let Some(agent_type) = self
            .ctx
            .app_handler()
            .get_agent_type_by_name(&environment, &agent_type_name)
            .await?
        else {
            bail!("Agent type not found: {}", agent_type_name);
        };

        let typed_parameters = DataValue::try_from_untyped_json(
            parameters,
            agent_type.agent_type.constructor.input_schema.clone(),
        )
        .map_err(|err| {
            anyhow!("Failed to match agent type parameters to the latest metadata: {err}")
        })?;
        let agent_id = build_repl_agent_id(&agent_type.agent_type, typed_parameters, phantom_id)?;
        let agent_name = RawAgentId(agent_id.to_string());

        let connection = WorkerConnection::new(
            self.ctx.worker_service_url().clone(),
            self.ctx.auth_token().await?,
            &agent_type.implemented_by.component_id,
            agent_name.0.clone(),
            stream_args.into(),
            self.ctx.allow_insecure(),
            self.ctx.format(),
            Some(idempotency_key),
        )
        .await?;

        connection.run_forever().await;

        Ok(())
    }

    async fn cmd_simulate_crash(&self, agent_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_action(
            "Simulating crash",
            format!("for agent {}", format_agent_name_match(&agent_name_match)),
        );

        self.interrupt_worker(&component, &agent_name, true).await?;

        log_action(
            "Simulated crash",
            format!("for agent {}", format_agent_name_match(&agent_name_match)),
        );

        Ok(())
    }

    async fn cmd_oplog(
        &self,
        agent_name: AgentIdArgs,
        from: Option<u64>,
        query: Option<String>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        let batch_size = self.ctx.http_batch_size();
        let mut cursor = Option::<OplogCursor>::None;
        let mut had_entries = false;
        loop {
            let mut entries = Vec::<(u64, PublicOplogEntry)>::new();
            cursor = {
                let clients = self.ctx.golem_clients().await?;

                let result = clients
                    .worker
                    .get_oplog(
                        &component.id.0,
                        &agent_name.0,
                        from,
                        batch_size,
                        cursor.as_ref(),
                        query.as_deref(),
                    )
                    .await
                    .map_service_error()?;

                entries.extend(
                    result
                        .entries
                        .into_iter()
                        .map(|entry| (entry.oplog_index.as_u64(), entry.entry)),
                );
                result.next
            };

            if !entries.is_empty() {
                had_entries = true;
                self.ctx.log_handler().log_view(&entries);
            }

            if cursor.is_none() {
                break;
            }
        }

        if !had_entries {
            log_warn("No results.")
        }

        Ok(())
    }

    async fn cmd_revert(
        &self,
        agent_name: AgentIdArgs,
        last_oplog_index: Option<u64>,
        number_of_invocations: Option<u64>,
    ) -> anyhow::Result<()> {
        if last_oplog_index.is_none() && number_of_invocations.is_none() {
            log_error(format!(
                "One of [{}, {}] must be specified",
                "last-oplog-index".log_color_highlight(),
                "number of invocations".log_color_highlight()
            ));
            bail!(NonSuccessfulExit)
        }

        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_action(
            "Reverting",
            format!("agent {}", format_agent_name_match(&agent_name_match)),
        );

        let clients = self.ctx.golem_clients().await?;

        {
            let target = {
                if let Some(last_oplog_index) = last_oplog_index {
                    RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                        last_oplog_index: OplogIndex::from_u64(last_oplog_index),
                    })
                } else if let Some(number_of_invocations) = number_of_invocations {
                    RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                        number_of_invocations,
                    })
                } else {
                    bail!("Expected either last_oplog_index or number_of_invocations")
                }
            };

            clients
                .worker
                .revert_worker(&component.id.0, &agent_name.0, &target)
                .await
                .map(|_| ())
                .map_service_error()?
        }

        log_action(
            "Reverted",
            format!("agent {}", format_agent_name_match(&agent_name_match)),
        );

        Ok(())
    }

    async fn cmd_cancel_invocation(
        &self,
        agent_name: AgentIdArgs,
        idempotency_key: IdempotencyKey,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_warn_action(
            "Canceling invocation",
            format!(
                "for agent {} using idempotency key: {}",
                format_agent_name_match(&agent_name_match),
                idempotency_key.value.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;

        let canceled = clients
            .worker
            .cancel_invocation(&component.id.0, &agent_name.0, &idempotency_key.value)
            .await
            .map(|result| result.canceled)
            .map_service_error()?;

        // TODO: json / yaml response?
        if canceled {
            log_action("Canceled", "");
        } else {
            log_warn_action("Failed", "to cancel, invocation already started");
        }

        Ok(())
    }

    async fn cmd_list(
        &self,
        agent_type_name: Option<String>,
        component_name: Option<ComponentName>,
        filters: Vec<String>,
        scan_cursor: Option<ScanCursor>,
        max_count: Option<u64>,
        precise: bool,
        refresh: Option<u64>,
    ) -> anyhow::Result<()> {
        let (components, filters) = self
            .resolve_list_components(agent_type_name, component_name, filters)
            .await?;

        if let Some(interval_ms) = refresh {
            self.list_with_refresh(
                &components,
                &filters,
                scan_cursor.as_ref(),
                max_count,
                precise,
                interval_ms,
            )
            .await
        } else {
            let view = self
                .list_agents(
                    &components,
                    &filters,
                    scan_cursor.as_ref(),
                    max_count,
                    precise,
                    false,
                )
                .await?;
            self.ctx.log_handler().log_view(&view);
            Ok(())
        }
    }

    async fn list_with_refresh(
        &self,
        components: &[ComponentDto],
        filters: &[String],
        scan_cursor: Option<&ScanCursor>,
        max_count: Option<u64>,
        precise: bool,
        interval_ms: u64,
    ) -> anyhow::Result<()> {
        let duration = Duration::from_millis(interval_ms);
        let mut screen = AlternateScreenGuard::enter()?;
        let result: anyhow::Result<()> = async {
            loop {
                let term_height = terminal_size()
                    .map(|(_, h)| h.0 as usize)
                    .unwrap_or(usize::MAX);

                // Fetch first — while the previous frame is still visible
                let output = match self
                    .list_agents(components, filters, scan_cursor, max_count, precise, true)
                    .await
                {
                    Ok(view) => self
                        .ctx
                        .log_handler()
                        .render_view_truncated(&view, term_height),
                    Err(e) => format!("Error: {e:#}"),
                };

                // Clear and write in one buffered flush — no blank-screen gap
                queue!(screen.stdout_mut(), MoveTo(0, 0), Clear(ClearType::All))?;
                write!(screen.stdout_mut(), "{output}")?;
                screen.stdout_mut().flush()?;

                tokio::select! {
                    _ = sleep(duration) => {}
                    _ = tokio::signal::ctrl_c() => { return Ok(()); }
                }
            }
        }
        .await;
        screen.leave()?;
        result
    }

    async fn resolve_list_components(
        &self,
        agent_type_name: Option<String>,
        component_name: Option<ComponentName>,
        filters: Vec<String>,
    ) -> anyhow::Result<(Vec<ComponentDto>, Vec<String>)> {
        let clients = self.ctx.golem_clients().await?;
        let component_handler = self.ctx.component_handler();

        match agent_type_name {
            Some(agent_type_name) => {
                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;
                debug!("Finding agent type {}", agent_type_name);
                let Some(agent_type) = self
                    .ctx
                    .app_handler()
                    .get_agent_type_by_name(&environment, &agent_type_name)
                    .await?
                else {
                    log_error(format!(
                        "Agent type {} not found",
                        agent_type_name.log_color_highlight()
                    ));
                    bail!(NonSuccessfulExit)
                };

                let mut filters = filters;
                filters.insert(
                    0,
                    format!(
                        "name startswith {}(",
                        agent_type.agent_type.type_name.0.clone()
                    ),
                );

                Ok((
                    vec![
                        component_handler
                            .get_component_revision_by_id(
                                &agent_type.implemented_by.component_id,
                                agent_type.implemented_by.component_revision,
                            )
                            .await?,
                    ],
                    filters,
                ))
            }
            None => {
                let selected_components = self
                    .ctx
                    .component_handler()
                    .must_select_components_by_app_dir_or_name(component_name.as_ref())
                    .await?;

                let environment = &selected_components.environment;

                environment
                    .with_current_deployment_revision_or_default_warn(
                        |current_deployment_revision| async move {
                            let mut components =
                                Vec::with_capacity(selected_components.component_names.len());
                            for component_name in selected_components.component_names {
                                match clients
                                    .component
                                    .get_deployment_component(
                                        &environment.environment_id.0,
                                        current_deployment_revision.into(),
                                        component_name.as_str(),
                                    )
                                    .await
                                    .map_service_error_not_found_as_opt()?
                                {
                                    Some(component) => {
                                        components.push(component);
                                    }
                                    None => {
                                        log_error(format!(
                                            "Component not found: {}",
                                            component_name.0.log_color_error_highlight()
                                        ));
                                        bail!(NonSuccessfulExit)
                                    }
                                }
                            }

                            Ok((components, filters))
                        },
                    )
                    .await
            }
        }
    }

    async fn list_agents(
        &self,
        components: &[ComponentDto],
        filters: &[String],
        scan_cursor: Option<&ScanCursor>,
        max_count: Option<u64>,
        precise: bool,
        stable_sort: bool,
    ) -> anyhow::Result<AgentsMetadataResponseView> {
        if scan_cursor.is_some() && components.len() != 1 {
            log_error(format!(
                "Cursor cannot be used with multiple components selected! ({})",
                components
                    .iter()
                    .map(|component| &component.component_name)
                    .map(|cn| cn.0.log_color_highlight())
                    .join(", ")
            ));
            logln("");
            logln(
                "Switch to an application directory with only one component or explicitly specify the requested component name.",
            );
            logln("");
            bail!(NonSuccessfulExit);
        }

        let mut view = AgentsMetadataResponseView::default();

        for component in components {
            let (workers, component_scan_cursor) = self
                .list_component_workers(
                    &component.component_name,
                    &component.id,
                    Some(filters),
                    scan_cursor,
                    max_count,
                    precise,
                )
                .await?;

            view.agents.extend(workers.into_iter().map(|w| {
                let raw_agent_name = w.agent_id.agent_id.clone();
                let source_language = ParsedAgentId::parse_agent_type_name(&raw_agent_name)
                    .ok()
                    .and_then(|type_name| {
                        component
                            .metadata
                            .agent_types()
                            .iter()
                            .find(|at| at.type_name == type_name)
                            .map(|at| SourceLanguage::from(at.source_language.as_str()))
                    })
                    .unwrap_or_default();

                let mut view = AgentMetadataView::from(w);

                if source_language.is_known()
                    && let Ok(parsed) = ParsedAgentId::parse(&raw_agent_name, &component.metadata)
                {
                    view.agent_name =
                        crate::agent_id_display::render_agent_id(&parsed, &source_language).into();
                }

                view.with_source_language(source_language)
            }));
            component_scan_cursor
                .into_iter()
                .for_each(|component_scan_cursor| {
                    view.cursors.insert(
                        component.component_name.to_string(),
                        scan_cursor_to_string(&component_scan_cursor),
                    );
                });
        }

        if stable_sort {
            view.agents.sort_by(|a, b| {
                a.component_name
                    .cmp(&b.component_name)
                    .then_with(|| a.agent_name.0.cmp(&b.agent_name.0))
            });
        }

        Ok(view)
    }

    async fn cmd_interrupt(&self, agent_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_action(
            "Interrupting",
            format!("agent {}", format_agent_name_match(&agent_name_match)),
        );

        self.interrupt_worker(&component, &agent_name, false)
            .await?;

        log_action(
            "Interrupted",
            format!("agent {}", format_agent_name_match(&agent_name_match)),
        );

        Ok(())
    }

    async fn cmd_resume(&self, agent_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_action(
            "Resuming",
            format!("agent {}", format_agent_name_match(&agent_name_match)),
        );

        self.resume_worker(&component, &agent_name).await?;

        log_action(
            "Resumed",
            format!("agent {}", format_agent_name_match(&agent_name_match)),
        );

        Ok(())
    }

    async fn cmd_update(
        &self,
        agent_name: AgentIdArgs,
        mode: AgentUpdateMode,
        target_revision: Option<ComponentRevision>,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let environment = &agent_name_match.environment;

        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        let target_revision = match target_revision {
            Some(target_revision) => target_revision,
            None => {
                let Some(current_deployed_revision) = self
                    .ctx
                    .component_handler()
                    .get_current_deployed_server_component_by_name(
                        environment,
                        &component.component_name,
                    )
                    .await?
                else {
                    bail!(
                        "Component {} not found, while getting latest component version",
                        component.component_name
                    );
                };

                if !self.ctx.interactive_handler().confirm_update_to_current(
                    &component.component_name,
                    &agent_name,
                    current_deployed_revision.revision,
                )? {
                    bail!(NonSuccessfulExit)
                }

                current_deployed_revision.revision
            }
        };

        self.update_worker(
            &component.component_name,
            &component.id,
            &agent_name.0,
            mode,
            target_revision,
            await_update,
            disable_wakeup,
        )
        .await?;

        Ok(())
    }

    async fn cmd_get(&self, agent_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let result = {
            let result = clients
                .worker
                .get_worker_metadata(&component.id.0, &agent_name.0)
                .await
                .map_service_error()?;

            AgentMetadata::from(agent_name_match.component_name, result)
        };

        self.ctx
            .log_handler()
            .log_view(&WorkerGetView::from_metadata(result, true));

        Ok(())
    }

    async fn cmd_delete(&self, agent_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_warn_action(
            "Deleting",
            format!("agent {}", format_agent_name_match(&agent_name_match)),
        );

        self.delete(component.id.0, &agent_name.0).await?;

        log_action(
            "Deleted",
            format!("agent {}", format_agent_name_match(&agent_name_match)),
        );

        Ok(())
    }

    async fn cmd_files(&self, agent_name: AgentIdArgs, path: String) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_action(
            "Listing files",
            format!(
                "for agent {} at path {}",
                format_agent_name_match(&agent_name_match),
                path.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        let nodes = match clients
            .worker
            .get_files(&component.id.0, &agent_name.0, &path)
            .await
            .map_service_error()
        {
            Ok(nodes) => nodes,
            Err(e) => {
                log_warn_action(
                    "Failed to list files",
                    format!(
                        "for agent {} at path {}: {e}",
                        format_agent_name_match(&agent_name_match),
                        path.log_color_error_highlight()
                    ),
                );
                return Err(e);
            }
        };

        // Convert nodes to WorkerFilesView with human-readable timestamps
        let view = WorkerFilesView {
            nodes: nodes
                .nodes
                .into_iter()
                .map(|n| FileNodeView {
                    name: n.name,
                    last_modified: format_timestamp(n.last_modified),
                    kind: n.kind.to_string(),
                    permissions: n
                        .permissions
                        .map(|p| p.as_compact_str())
                        .unwrap_or_else(|| "-")
                        .to_string(),
                    size: n.size.unwrap_or(0),
                })
                .collect(),
        };

        self.ctx.log_handler().log_view(&view);

        log_action(
            "Listed files",
            format!(
                "for agent {} at path {}",
                format_agent_name_match(&agent_name_match),
                path.log_color_highlight()
            ),
        );

        Ok(())
    }

    async fn cmd_file_contents(
        &self,
        agent_name: AgentIdArgs,
        path: String,
        output: Option<String>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        log_action(
            "Downloading file",
            format!(
                "from agent {} at path {}",
                format_agent_name_match(&agent_name_match),
                path.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        let file_contents = match clients
            .worker
            .get_file_content(&component.id.0, &agent_name.0, &path)
            .await
            .map_service_error()
        {
            Ok(contents) => contents,
            Err(e) => {
                log_warn_action(
                    "Failed to download file",
                    format!(
                        "from agent {} at path {}: {e}",
                        format_agent_name_match(&agent_name_match),
                        path.log_color_error_highlight()
                    ),
                );
                return Err(e);
            }
        };

        let output_path = if let Some(ref output) = output {
            output.clone()
        } else {
            std::path::Path::new(&path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "output.bin".to_string())
        };

        // Check if file exists and ask for confirmation if needed
        if Path::new(&output_path).exists() {
            let should_overwrite = self.confirm_file_overwrite(&output_path)?;
            if !should_overwrite {
                log_action(
                    "File download cancelled",
                    format!("by user for file {}", output_path.log_color_highlight()),
                );
                return Ok(());
            }
        }

        match File::create(&output_path).and_then(|mut file| file.write_all(&file_contents)) {
            Ok(_) => {
                log_action(
                    "File saved",
                    format!("to {}", output_path.log_color_highlight()),
                );
                Ok(())
            }
            Err(e) => {
                log_warn_action(
                    "Failed to save file",
                    format!("to {}: {e}", output_path.log_color_error_highlight()),
                );
                Err(e.into())
            }
        }
    }

    async fn cmd_activate_plugin(
        &self,
        agent_name: AgentIdArgs,
        plugin_name: String,
        explicit_priority: Option<i32>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        let plugin_priority = self.resolve_plugin_priority(
            &component,
            &agent_name,
            &plugin_name,
            explicit_priority,
        )?;

        log_action(
            "Activating plugin",
            format!(
                "{} for agent {}",
                plugin_name.log_color_highlight(),
                format_agent_name_match(&agent_name_match)
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        clients
            .worker
            .activate_plugin(&component.id.0, &agent_name.0, plugin_priority)
            .await
            .map(|_| ())
            .map_service_error()?;

        log_action(
            "Activated plugin",
            format!(
                "{} for agent {}",
                plugin_name.log_color_highlight(),
                format_agent_name_match(&agent_name_match)
            ),
        );

        Ok(())
    }

    async fn cmd_deactivate_plugin(
        &self,
        agent_name: AgentIdArgs,
        plugin_name: String,
        explicit_priority: Option<i32>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_name_match = self.match_agent_name(agent_name.agent_id).await?;
        let (component, agent_name) = self
            .component_by_agent_name_match(&agent_name_match)
            .await?;

        let plugin_priority = self.resolve_plugin_priority(
            &component,
            &agent_name,
            &plugin_name,
            explicit_priority,
        )?;

        log_action(
            "Deactivating plugin",
            format!(
                "{} for agent {}",
                plugin_name.log_color_highlight(),
                format_agent_name_match(&agent_name_match)
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        clients
            .worker
            .deactivate_plugin(&component.id.0, &agent_name.0, plugin_priority)
            .await
            .map(|_| ())
            .map_service_error()?;

        log_action(
            "Deactivated plugin",
            format!(
                "{} for agent {}",
                plugin_name.log_color_highlight(),
                format_agent_name_match(&agent_name_match)
            ),
        );

        Ok(())
    }

    fn resolve_plugin_priority(
        &self,
        component: &ComponentDto,
        agent_name: &RawAgentId,
        plugin_name: &str,
        explicit_priority: Option<i32>,
    ) -> anyhow::Result<i32> {
        let agent_type_name = ParsedAgentId::parse_agent_type_name(&agent_name.0)
            .map(|n| n.0)
            .unwrap_or_default();

        let agent_type = AgentTypeName(agent_type_name.clone());
        let plugins = component
            .metadata
            .agent_type_plugins(&agent_type)
            .unwrap_or_default();

        let matching: Vec<_> = plugins
            .iter()
            .filter(|p| p.plugin_name == plugin_name)
            .collect();

        match matching.len() {
            0 => {
                let available: Vec<_> = plugins
                    .iter()
                    .map(|p| format!("{} (priority {})", p.plugin_name, p.priority.0))
                    .collect();
                logln("");
                log_error(format!(
                    "Plugin {} is not installed for agent type {}.",
                    plugin_name.log_color_error_highlight(),
                    agent_type_name.log_color_error_highlight(),
                ));
                if !available.is_empty() {
                    logln(format!(
                        "Installed plugins: {}",
                        available.join(", ").log_color_highlight()
                    ));
                }
                logln("");
                bail!(NonSuccessfulExit);
            }
            1 => Ok(matching[0].priority.0),
            _ => {
                if let Some(priority) = explicit_priority {
                    if matching.iter().any(|p| p.priority.0 == priority) {
                        Ok(priority)
                    } else {
                        let priorities: Vec<_> =
                            matching.iter().map(|p| p.priority.0).collect();
                        logln("");
                        log_error(format!(
                            "Plugin {} has no installation with priority {}. Available priorities: {:?}",
                            plugin_name.log_color_error_highlight(),
                            priority,
                            priorities,
                        ));
                        logln("");
                        bail!(NonSuccessfulExit);
                    }
                } else {
                    let priorities: Vec<_> = matching.iter().map(|p| p.priority.0).collect();
                    logln("");
                    log_error(format!(
                        "Multiple installations of plugin {} found (priorities: {:?}).",
                        plugin_name.log_color_error_highlight(),
                        priorities,
                    ));
                    logln("Use --plugin-priority to specify which installation to target.");
                    logln("Use `golem component plugin get` to list installed plugins.");
                    logln("");
                    bail!(NonSuccessfulExit);
                }
            }
        }
    }

    async fn new_worker(
        &self,
        component_id: Uuid,
        agent_name: String,
        env: HashMap<String, String>,
        wasi_config: BTreeMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .launch_new_worker(
                &component_id,
                &golem_client::model::AgentCreationRequest {
                    name: agent_name,
                    env,
                    wasi_config,
                    config,
                },
            )
            .await
            .map(|_| ())
            .map_service_error()
    }

    pub async fn worker_metadata(
        &self,
        component_id: Uuid,
        component_name: &ComponentName,
        agent_name: &RawAgentId,
    ) -> anyhow::Result<AgentMetadata> {
        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .worker
            .get_worker_metadata(&component_id, &agent_name.0)
            .await
            .map_service_error()?;

        Ok(AgentMetadata::from(component_name.clone(), result))
    }

    async fn delete(&self, component_id: Uuid, agent_name: &str) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .delete_worker(&component_id, agent_name)
            .await
            .map(|_| ())
            .map_service_error()
    }

    pub async fn update_component_workers(
        &self,
        component_name: &ComponentName,
        component_id: &ComponentId,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<TryUpdateAllWorkersResult> {
        let (workers, _) = self
            .list_component_workers(component_name, component_id, None, None, None, false)
            .await?;

        if workers.is_empty() {
            log_warn_action(
                "Skipping",
                format!("updating agents for component {component_name}, no agents found"),
            );
            return Ok(TryUpdateAllWorkersResult::default());
        }

        log_action(
            "Updating",
            format!(
                "all agents ({}) for component {} to revision {}",
                workers.len().to_string().log_color_highlight(),
                component_name.0.blue().bold(),
                target_revision.to_string().log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        let mut update_results = TryUpdateAllWorkersResult::default();
        for worker in &workers {
            let result = self
                .update_worker(
                    component_name,
                    &worker.agent_id.component_id,
                    &worker.agent_id.agent_id,
                    update_mode,
                    target_revision,
                    false,
                    disable_wakeup,
                )
                .await;

            match result {
                Ok(_) => {
                    update_results.triggered.push(WorkerUpdateAttempt {
                        component_name: component_name.clone(),
                        target_revision,
                        agent_name: worker.agent_id.agent_id.as_str().into(),
                        error: None,
                    });
                }
                Err(error) => {
                    update_results.triggered.push(WorkerUpdateAttempt {
                        component_name: component_name.clone(),
                        target_revision,
                        agent_name: worker.agent_id.agent_id.as_str().into(),
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        if await_update {
            for worker in workers {
                let _ = self
                    .await_update_result(
                        &worker.agent_id.component_id,
                        &worker.agent_id.agent_id,
                        target_revision,
                    )
                    .await;
            }
        }

        Ok(update_results)
    }

    async fn update_worker(
        &self,
        component_name: &ComponentName,
        component_id: &ComponentId,
        agent_name: &str,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Triggering update",
            format!(
                "for agent {}/{} to revision {} using {} update mode",
                component_name.0.bold().blue(),
                agent_name.bold().green(),
                target_revision.to_string().log_color_highlight(),
                update_mode.to_string().log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .worker
            .update_worker(
                &component_id.0,
                agent_name,
                &UpdateWorkerRequest {
                    mode: match update_mode {
                        AgentUpdateMode::Automatic => {
                            golem_client::model::AgentUpdateMode::Automatic
                        }
                        AgentUpdateMode::Manual => golem_client::model::AgentUpdateMode::Manual,
                    },
                    target_revision: target_revision.into(),
                    disable_wakeup: Some(disable_wakeup),
                },
            )
            .await
            .map(|_| ())
            .map_service_error();

        match result {
            Ok(_) => {
                log_action("Triggered update", "");

                if await_update {
                    self.await_update_result(component_id, agent_name, target_revision)
                        .await?;
                }

                Ok(())
            }
            Err(error) => {
                log_failed_to("trigger update for agent");
                let _indent = LogIndent::new();
                log_error(error.to_string());
                Err(anyhow!(error))
            }
        }
    }

    async fn await_update_result(
        &self,
        component_id: &ComponentId,
        agent_name: &str,
        target_revision: ComponentRevision,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        loop {
            let metadata = clients
                .worker
                .get_worker_metadata(&component_id.0, agent_name)
                .await?;
            for update_record in metadata.updates {
                let mut latest_success = None;
                let mut latest_failure = None;
                let mut pending_count = 0;
                match update_record {
                    UpdateRecord::PendingUpdate(details)
                        if details.target_revision == target_revision =>
                    {
                        pending_count += 1;
                    }
                    UpdateRecord::SuccessfulUpdate(details)
                        if details.target_revision == target_revision =>
                    {
                        match latest_success {
                            None => latest_success = Some(details),
                            Some(previous_success)
                                if previous_success.timestamp < details.timestamp =>
                            {
                                latest_success = Some(details);
                            }
                            _ => {}
                        }
                    }
                    UpdateRecord::FailedUpdate(details)
                        if details.target_revision == target_revision =>
                    {
                        match latest_failure {
                            None => latest_failure = Some(details),
                            Some(previous_failure)
                                if previous_failure.timestamp < details.timestamp =>
                            {
                                latest_failure = Some(details);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }

                if pending_count > 0 {
                    log_action("Agent update", "is still pending");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                } else if let Some(success) = latest_success {
                    log_action(
                        "Agent update",
                        format!(
                            "to revision {} succeeded at {}",
                            success.target_revision.to_string().log_color_highlight(),
                            success.timestamp.to_string().log_color_highlight()
                        ),
                    );
                    return Ok(());
                } else if let Some(failure) = latest_failure {
                    let error = failure.details.unwrap_or("unknown reason".to_string());
                    log_error_action(
                        "Agent update",
                        format!(
                            "to revision {} failed at {}: {}",
                            failure.target_revision.to_string().log_color_highlight(),
                            failure.timestamp.to_string().log_color_highlight(),
                            error
                        ),
                    );
                    return Err(anyhow!(error));
                } else {
                    log_error_action(
                        "Agent update",
                        "is not pending anymore, but no outcome has been found",
                    );
                    return Err(anyhow!(
                        "Unexpected agent state: update is not pending anymore, but no outcome has been found"
                    ));
                }
            }
        }
    }

    pub async fn redeploy_component_workers(
        &self,
        component_name: &ComponentName,
        component_id: &ComponentId,
    ) -> anyhow::Result<()> {
        let (workers, _) = self
            .list_component_workers(component_name, component_id, None, None, None, false)
            .await?;

        if workers.is_empty() {
            log_warn_action(
                "Skipping",
                format!("redeploying agents for component {component_name}, no agent found"),
            );
            return Ok(());
        }

        log_action(
            "Redeploying",
            format!(
                "all agents ({}) for component {}",
                workers.len().to_string().log_color_highlight(),
                component_name.0.blue().bold(),
            ),
        );
        let _indent = LogIndent::new();

        if !self
            .ctx
            .interactive_handler()
            .confirm_redeploy_agents(workers.len())?
        {
            bail!(NonSuccessfulExit);
        }

        for worker in workers {
            self.redeploy_worker(component_name, worker).await?;
        }

        Ok(())
    }

    pub async fn delete_component_workers(
        &self,
        component_name: &ComponentName,
        component_id: &ComponentId,
        show_skip: bool,
    ) -> anyhow::Result<usize> {
        let (workers, _) = self
            .list_component_workers(component_name, component_id, None, None, None, false)
            .await?;

        if workers.is_empty() {
            if show_skip {
                log_warn_action(
                    "Skipping",
                    format!("deleting agents for component {component_name}, no agent found"),
                );
            }
            return Ok(0);
        }

        log_action(
            "Deleting",
            format!(
                "all agents ({}) for component {}",
                workers.len().to_string().log_color_highlight(),
                component_name.0.blue().bold(),
            ),
        );
        let _indent = LogIndent::new();

        if !self
            .ctx
            .interactive_handler()
            .confirm_deleting_agents(workers.len())?
        {
            bail!(NonSuccessfulExit);
        }

        for worker in &workers {
            self.delete_worker(component_name, worker).await?;
        }

        Ok(workers.len())
    }

    async fn redeploy_worker(
        &self,
        component_name: &ComponentName,
        worker_metadata: AgentMetadata,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Redeploying",
            format!(
                "agent {}/{} to latest version",
                component_name.0.bold().blue(),
                worker_metadata.agent_id.agent_id.bold().green(),
            ),
        );
        let _indent = LogIndent::new();

        self.delete_worker(component_name, &worker_metadata).await?;

        log_action(
            "Recreating",
            format!(
                "agent {}/{}",
                component_name.0.bold().blue(),
                worker_metadata.agent_id.agent_id.bold().green(),
            ),
        );
        self.new_worker(
            worker_metadata.agent_id.component_id.0,
            worker_metadata.agent_id.agent_id,
            worker_metadata.env,
            worker_metadata.wasi_config,
            worker_metadata.config,
        )
        .await?;
        log_action("Recreated", "agent");

        Ok(())
    }

    pub async fn delete_worker(
        &self,
        component_name: &ComponentName,
        worker_metadata: &AgentMetadata,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Deleting",
            format!(
                "agent {}/{}",
                component_name.0.bold().blue(),
                worker_metadata.agent_id.agent_id.bold().green(),
            ),
        );
        self.delete(
            worker_metadata.agent_id.component_id.0,
            &worker_metadata.agent_id.agent_id,
        )
        .await?;
        log_action("Deleted", "agent");
        Ok(())
    }

    pub async fn list_component_workers(
        &self,
        component_name: &ComponentName,
        component_id: &ComponentId,
        filters: Option<&[String]>,
        start_scan_cursor: Option<&ScanCursor>,
        max_count: Option<u64>,
        precise: bool,
    ) -> anyhow::Result<(Vec<AgentMetadata>, Option<ScanCursor>)> {
        let clients = self.ctx.golem_clients().await?;
        let mut workers = Vec::<AgentMetadata>::new();
        let mut final_result_cursor = Option::<ScanCursor>::None;

        let start_scan_cursor = start_scan_cursor.map(scan_cursor_to_string);
        let mut current_scan_cursor = start_scan_cursor.clone();
        loop {
            let result_cursor = {
                let results = clients
                    .worker
                    .get_workers_metadata(
                        &component_id.0,
                        filters,
                        current_scan_cursor.as_deref(),
                        max_count.or(Some(self.ctx.http_batch_size())),
                        Some(precise),
                    )
                    .await
                    .map_service_error()?;

                workers.extend(
                    results
                        .workers
                        .into_iter()
                        .map(|meta| AgentMetadata::from(component_name.clone(), meta)),
                );

                results.cursor
            };

            match result_cursor {
                Some(next_cursor) => {
                    if max_count.is_none() {
                        current_scan_cursor = Some(scan_cursor_to_string(&next_cursor));
                    } else {
                        final_result_cursor = Some(next_cursor);
                        break;
                    }
                }
                None => {
                    break;
                }
            }
        }

        Ok((workers, final_result_cursor))
    }

    async fn component_by_agent_name_match(
        &self,
        agent_name_match: &AgentNameMatch,
    ) -> anyhow::Result<(ComponentDto, RawAgentId)> {
        let component = self
            .ctx
            .component_handler()
            .resolve_component(
                &agent_name_match.environment,
                &agent_name_match.component_name,
                Some((&agent_name_match.agent_name).into()),
            )
            .await?;

        let Some(component) = component else {
            log_error(format!(
                "Component {} not found",
                agent_name_match
                    .component_name
                    .0
                    .log_color_error_highlight()
            ));
            logln("");
            bail!(NonSuccessfulExit);
        };

        // Try to re-canonicalize the agent name using language-aware parsing.
        // Language is auto-detected from agent type metadata.
        let agent_name =
            self.try_recanonicalize_agent_name(&agent_name_match.agent_name, &component);

        Ok((component, agent_name))
    }

    pub(crate) fn try_recanonicalize_agent_name(
        &self,
        agent_name: &RawAgentId,
        component: &ComponentDto,
    ) -> RawAgentId {
        let raw = &agent_name.0;

        // Extract type name and params using ParsedAgentId::parse_agent_type_name
        // and manual splitting for the params portion
        let Some(paren_pos) = raw.find('(') else {
            return agent_name.clone();
        };
        let type_name = &raw[..paren_pos];

        // Find matching closing paren (account for nesting and phantom id)
        let mut nesting = 0i32;
        let mut close_pos = None;
        for (i, ch) in raw[paren_pos..].char_indices() {
            match ch {
                '(' => nesting += 1,
                ')' => {
                    nesting -= 1;
                    if nesting == 0 {
                        close_pos = Some(paren_pos + i);
                        break;
                    }
                }
                _ => {}
            }
        }

        let Some(close_pos) = close_pos else {
            return agent_name.clone();
        };

        let params_str = &raw[paren_pos + 1..close_pos];
        let phantom_str = if close_pos + 1 < raw.len() {
            Some(&raw[close_pos + 1..])
        } else {
            None
        };

        // Look up the agent type from component metadata
        let agent_type = component
            .metadata
            .agent_types()
            .iter()
            .find(|at| at.type_name.0 == type_name);

        let Some(agent_type) = agent_type else {
            return agent_name.clone();
        };

        // Derive source language from agent type metadata
        let source_language = SourceLanguage::from(agent_type.source_language.as_str());

        // Try language-aware parse
        let Ok(data_value) = crate::agent_id_display::parse_agent_id_params(
            params_str,
            &agent_type.constructor.input_schema,
            &source_language,
        ) else {
            return agent_name.clone();
        };

        // Re-canonicalize using structural format
        let Ok(canonical) =
            golem_common::model::agent::structural_format::format_structural(&data_value)
        else {
            return agent_name.clone();
        };

        let mut new_id = format!("{}({canonical})", agent_type.type_name.0);
        if let Some(phantom) = phantom_str {
            new_id.push_str(phantom);
        }
        RawAgentId(new_id)
    }

    async fn resume_worker(
        &self,
        component: &ComponentDto,
        agent_name: &RawAgentId,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .resume_worker(&component.id.0, &agent_name.0)
            .await
            .map(|_| ())
            .map_service_error()?;

        Ok(())
    }

    async fn interrupt_worker(
        &self,
        component: &ComponentDto,
        agent_name: &RawAgentId,
        recover_immediately: bool,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .interrupt_worker(&component.id.0, &agent_name.0, Some(recover_immediately))
            .await
            .map(|_| ())
            .map_service_error()?;

        Ok(())
    }

    async fn match_agent_name_in_environment(
        &self,
        environment: ResolvedEnvironmentIdentity,
        agent_name: String,
    ) -> anyhow::Result<AgentNameMatch> {
        let parsed_agent_type_name = match ParsedAgentId::parse_agent_type_name(&agent_name) {
            Ok(agent_type_name) => agent_type_name,
            Err(err) => {
                logln("");
                log_error(format!(
                    "Failed to parse agent name ({}) as agent id: {err}",
                    agent_name.log_color_error_highlight()
                ));
                logln("");
                log_text_view(&WorkerNameHelp);
                bail!(NonSuccessfulExit);
            }
        };

        let Some(agent_type) = self
            .ctx
            .app_handler()
            .get_agent_type_by_name(&environment, &parsed_agent_type_name.0)
            .await?
        else {
            logln("");
            log_error(format!(
                "Agent type {} is not found in the selected deployment.",
                parsed_agent_type_name.0.log_color_error_highlight()
            ));
            logln("");
            logln(
                "Deploy the component that defines this agent type or specify environment/application/account prefixes.",
            );
            logln("");
            log_text_view(&WorkerNameHelp);
            bail!(NonSuccessfulExit);
        };

        let component = self
            .ctx
            .component_handler()
            .get_component_revision_by_id(
                &agent_type.implemented_by.component_id,
                agent_type.implemented_by.component_revision,
            )
            .await?;

        Ok(AgentNameMatch {
            environment,
            component_name_match_kind: ComponentNameMatchKind::Unknown,
            component_name: component.component_name,
            agent_name: agent_name.into(),
            source_language: SourceLanguage::from(agent_type.agent_type.source_language.as_str()),
        })
    }

    pub async fn match_agent_name(&self, agent_name: RawAgentId) -> anyhow::Result<AgentNameMatch> {
        let segments = split_agent_name(&agent_name.0);
        match segments.len() {
            // <AGENT>
            1 => {
                let agent_name = segments[0].to_string();

                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;

                self.match_agent_name_in_environment(environment, agent_name)
                    .await
            }
            // <ENVIRONMENT>/<AGENT>
            // <APPLICATION>/<ENVIRONMENT>/<AGENT>
            // <ACCOUNT>/<APPLICATION>/<ENVIRONMENT>/<AGENT>
            2..=4 => {
                fn non_empty<'a>(name: &'a str, value: &'a str) -> anyhow::Result<&'a str> {
                    if value.is_empty() {
                        log_error(format!(
                            "Missing {name} part in agent name!",
                            name = name.log_color_highlight()
                        ));
                        logln("");
                        log_text_view(&WorkerNameHelp);
                        bail!(NonSuccessfulExit);
                    }
                    Ok(value)
                }

                fn validated<'a, T>(name: &'a str, value: &'a str) -> anyhow::Result<T>
                where
                    T: FromStr<Err = String>,
                {
                    let value = non_empty(name, value)?;
                    match T::from_str(value) {
                        Ok(value) => Ok(value),
                        Err(err) => {
                            log_error(format!(
                                "Invalid {name} part in agent name, value: {value}, error: {err}",
                                name = name.log_color_highlight(),
                                value = value.log_color_error_highlight(),
                                err = err.log_color_error_highlight()
                            ));
                            logln("");
                            log_text_view(&WorkerNameHelp);
                            bail!(NonSuccessfulExit);
                        }
                    }
                }

                fn validated_account(value: &str) -> anyhow::Result<String> {
                    Ok(non_empty("account", value)?.to_string())
                }

                fn validated_application(value: &str) -> anyhow::Result<ApplicationName> {
                    validated("application", value)
                }

                fn validated_environment(value: &str) -> anyhow::Result<EnvironmentName> {
                    validated("environment", value)
                }

                fn validated_worker(value: &str) -> anyhow::Result<String> {
                    Ok(non_empty("agent", value)?.to_string())
                }

                let (environment_reference, agent_name): (Option<EnvironmentReference>, String) =
                    match segments.len() {
                        2 => (
                            Some(EnvironmentReference::Environment {
                                environment_name: validated_environment(segments[0])?,
                            }),
                            validated_worker(segments[1])?,
                        ),
                        3 => (
                            Some(EnvironmentReference::ApplicationEnvironment {
                                application_name: validated_application(segments[0])?,
                                environment_name: validated_environment(segments[1])?,
                            }),
                            validated_worker(segments[2])?,
                        ),
                        4 => (
                            Some(EnvironmentReference::AccountApplicationEnvironment {
                                account_email: validated_account(segments[0])?,
                                application_name: validated_application(segments[1])?,
                                environment_name: validated_environment(segments[2])?,
                            }),
                            validated_worker(segments[3])?,
                        ),
                        other => panic!("Unexpected segment count: {other}"),
                    };

                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_opt_environment_reference(
                        EnvironmentResolveMode::Any,
                        environment_reference.as_ref(),
                    )
                    .await?;

                self.match_agent_name_in_environment(environment, agent_name)
                    .await
            }
            _ => {
                logln("");
                log_error(format!(
                    "Failed to parse agent name: {}",
                    agent_name.0.log_color_error_highlight()
                ));
                logln("");
                log_text_view(&WorkerNameHelp);
                bail!(NonSuccessfulExit);
            }
        }
    }

    pub fn validate_worker_and_function_names(
        &self,
        component: &ComponentDto,
        agent_name: &RawAgentId,
        function_name: Option<&str>,
    ) -> anyhow::Result<Option<(ParsedAgentId, AgentType)>> {
        if !component.metadata.is_agent() {
            return Ok(None);
        }

        match ParsedAgentId::parse_and_resolve_type(&agent_name.0, &component.metadata) {
            Ok((agent_id, agent_type)) => match function_name {
                Some(function_name) => {
                    if let Some((namespace, package, interface)) = component
                        .metadata
                        .find_function(function_name)
                        .ok()
                        .flatten()
                        .and_then(|f| match f.name.site {
                            ParsedFunctionSite::Global => None,
                            ParsedFunctionSite::Interface { .. } => None,
                            ParsedFunctionSite::PackagedInterface {
                                namespace,
                                package,
                                interface,
                                ..
                            } => Some((namespace, package, interface)),
                        })
                    {
                        let component_name = format!("{namespace}:{package}");
                        if *interface == agent_id.agent_type.0
                            && component.component_name.0 == component_name
                        {
                            return Ok(Some((agent_id, agent_type.clone())));
                        }
                    }

                    logln("");
                    log_error(format!(
                        "Incompatible agent type ({}) and method ({})",
                        agent_id.agent_type.as_str().log_color_error_highlight(),
                        function_name.log_color_error_highlight()
                    ));
                    logln("");
                    log_text_view(&AvailableFunctionNamesHelp::new_agent(
                        component,
                        &agent_id,
                        &agent_type,
                    ));
                    bail!(NonSuccessfulExit);
                }

                None => Ok(Some((agent_id, agent_type.clone()))),
            },
            Err(err) => {
                logln("");
                log_error(format!(
                    "Failed to parse agent name ({}) as agent id: {err}",
                    agent_name.0.log_color_error_highlight()
                ));
                logln("");
                log_text_view(&AvailableAgentConstructorsHelp {
                    component_name: component.component_name.0.clone(),
                    constructors: show_exported_agent_constructors(
                        component.metadata.agent_types(),
                        true,
                    ),
                });

                bail!(NonSuccessfulExit);
            }
        }
    }

    fn confirm_file_overwrite(&self, output_path: &str) -> anyhow::Result<bool> {
        let result = Confirm::new(&format!(
            "File {} already exists. Do you want to overwrite it?",
            output_path.log_color_highlight()
        ))
        .with_default(false)
        .prompt()?;

        Ok(result)
    }
}

struct AlternateScreenGuard {
    stdout: Stdout,
    active: bool,
}

impl AlternateScreenGuard {
    fn enter() -> anyhow::Result<Self> {
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide)?;

        Ok(Self {
            stdout,
            active: true,
        })
    }

    fn stdout_mut(&mut self) -> &mut Stdout {
        &mut self.stdout
    }

    fn leave(&mut self) -> anyhow::Result<()> {
        if self.active {
            execute!(self.stdout, Show, LeaveAlternateScreen)?;
            self.active = false;
        }

        Ok(())
    }
}

impl Drop for AlternateScreenGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(self.stdout, Show, LeaveAlternateScreen);
            self.active = false;
        }
    }
}

/// Extracts the simple method name from a potentially fully qualified function name.
///
/// For example, `rust:agent/foo-agent.{fun-string}` → `fun-string`.
/// Returns the input unchanged if it is already a simple name.
fn extract_simple_method_name(function_name: &str) -> String {
    if let Some(inner) = function_name
        .strip_suffix('}')
        .and_then(|s| s.rsplit_once('.'))
        .and_then(|(_, rest)| rest.strip_prefix('{'))
    {
        inner.to_string()
    } else {
        function_name.to_string()
    }
}

/// Fuzzy-matches a method name pattern against the original method names from agent metadata,
/// returning the matched method name on success.
fn resolve_agent_method_name(
    provided_method_name: &str,
    agent_type: &AgentType,
) -> crate::fuzzy::Result {
    let mut alias_to_original: HashMap<String, String> = HashMap::new();
    let mut aliases: Vec<String> = Vec::new();

    for m in &agent_type.methods {
        let original = m.name.clone();
        alias_to_original.insert(original.clone(), original.clone());
        aliases.push(original);
    }

    let fuzzy_search = FuzzySearch::new(aliases.iter().map(|s| s.as_str()));
    fuzzy_search.find(provided_method_name).map(|m| {
        let original = alias_to_original
            .get(&m.option)
            .cloned()
            .unwrap_or(m.option.clone());
        crate::fuzzy::Match {
            option: original,
            ..m
        }
    })
}

fn scan_cursor_to_string(cursor: &ScanCursor) -> String {
    format!("{}/{}", cursor.layer, cursor.cursor)
}

fn parse_worker_error(status: u16, body: Vec<u8>) -> ServiceError {
    let error: anyhow::Result<
        Option<golem_client::Error<golem_client::api::WorkerError>>,
        serde_json::Error,
    > = match status {
        400 => serde_json::from_slice(&body).map(|body| {
            Some(golem_client::Error::Item(
                golem_client::api::WorkerError::Error400(body),
            ))
        }),
        401 => serde_json::from_slice(&body).map(|body| {
            Some(golem_client::Error::Item(
                golem_client::api::WorkerError::Error401(body),
            ))
        }),
        403 => serde_json::from_slice(&body).map(|body| {
            Some(golem_client::Error::Item(
                golem_client::api::WorkerError::Error403(body),
            ))
        }),
        404 => serde_json::from_slice(&body).map(|body| {
            Some(golem_client::Error::Item(
                golem_client::api::WorkerError::Error404(body),
            ))
        }),
        409 => serde_json::from_slice(&body).map(|body| {
            Some(golem_client::Error::Item(
                golem_client::api::WorkerError::Error409(body),
            ))
        }),
        500 => serde_json::from_slice(&body).map(|body| {
            Some(golem_client::Error::Item(
                golem_client::api::WorkerError::Error500(body),
            ))
        }),
        _ => Ok(None),
    };

    match error.ok().flatten() {
        Some(error) => error.into(),
        None => {
            golem_client::Error::<golem_client::api::WorkerError>::unexpected(status, body.into())
                .into()
        }
    }
}

fn split_agent_name(agent_name: &str) -> Vec<&str> {
    match agent_name.find('(') {
        Some(constructor_open_parentheses_idx) => {
            let splittable = &agent_name[0..constructor_open_parentheses_idx];
            let last_slash_idx = splittable.rfind('/');
            match last_slash_idx {
                Some(last_slash_idx) => {
                    let mut segments = agent_name[0..last_slash_idx]
                        .split('/')
                        .collect::<Vec<&str>>();
                    segments.push(&agent_name[last_slash_idx + 1..]);
                    segments
                }
                None => {
                    vec![agent_name]
                }
            }
        }
        None => agent_name.split("/").collect(),
    }
}

fn build_repl_agent_id(
    agent_type: &AgentType,
    typed_parameters: DataValue,
    phantom_id: Option<Uuid>,
) -> anyhow::Result<ParsedAgentId> {
    ParsedAgentId::new_auto_phantom(
        agent_type.type_name.clone(),
        typed_parameters,
        phantom_id,
        agent_type.mode,
    )
    .map_err(|e| anyhow!("Failed to format agent ID: {e}"))
}

fn normalize_public_agent_id(
    agent_id: &ParsedAgentId,
    agent_type: &AgentType,
) -> anyhow::Result<ParsedAgentId> {
    build_repl_agent_id(agent_type, agent_id.parameters.clone(), agent_id.phantom_id)
}

#[cfg(test)]
mod tests {
    use super::{
        build_repl_agent_id, extract_simple_method_name, normalize_public_agent_id,
        split_agent_name,
    };
    use golem_common::model::Empty;
    use golem_common::model::agent::{
        AgentConstructor, AgentMethod, AgentMode, AgentType, AgentTypeName, DataSchema,
        ElementValues, NamedElementSchemas, ParsedAgentId, Snapshotting,
    };
    use pretty_assertions::assert_eq;
    use test_r::test;
    use uuid::Uuid;

    fn test_agent_type(mode: AgentMode) -> AgentType {
        AgentType {
            type_name: AgentTypeName("repl-agent".to_string()),
            description: String::new(),
            source_language: String::new(),
            constructor: AgentConstructor {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
            },
            methods: vec![AgentMethod {
                name: "run".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
                output_schema: DataSchema::Tuple(NamedElementSchemas::empty()),
                http_endpoint: vec![],
            }],
            dependencies: vec![],
            mode,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }

    fn empty_tuple() -> golem_common::model::agent::DataValue {
        golem_common::model::agent::DataValue::Tuple(ElementValues { elements: vec![] })
    }

    #[test]
    fn test_split_agent_name() {
        assert_eq!(split_agent_name("a"), vec!["a"]);
        assert_eq!(split_agent_name("a()"), vec!["a()"]);
        assert_eq!(split_agent_name("a(\"///\")"), vec!["a(\"///\")"]);
        assert_eq!(split_agent_name("a/b"), vec!["a", "b"]);
        assert_eq!(split_agent_name("a/b()"), vec!["a", "b()"]);
        assert_eq!(split_agent_name("a/b(\"///\")"), vec!["a", "b(\"///\")"]);
        assert_eq!(split_agent_name("a/b/c"), vec!["a", "b", "c"]);
        assert_eq!(split_agent_name("a/b/c()"), vec!["a", "b", "c()"]);
        assert_eq!(split_agent_name("a/b/c(\"/\")"), vec!["a", "b", "c(\"/\")"]);
        assert_eq!(split_agent_name("/"), vec!["", ""]);
        assert_eq!(split_agent_name("a(/"), vec!["a(/"]);
    }

    #[test]
    fn test_extract_simple_method_name_simple() {
        assert_eq!(extract_simple_method_name("fun_string"), "fun_string");
    }

    #[test]
    fn test_extract_simple_method_name_qualified_kebab() {
        assert_eq!(
            extract_simple_method_name("rust:agent/FooAgent.{fun-string}"),
            "fun-string"
        );
    }

    #[test]
    fn test_extract_simple_method_name_qualified_underscore() {
        assert_eq!(
            extract_simple_method_name("rust:agent/FooAgent.{fun_string}"),
            "fun_string"
        );
    }

    #[test]
    fn repl_agent_id_auto_generates_phantom_for_ephemeral_agents() {
        let agent_id =
            build_repl_agent_id(&test_agent_type(AgentMode::Ephemeral), empty_tuple(), None)
                .unwrap();

        assert!(agent_id.phantom_id.is_some());
    }

    #[test]
    fn repl_agent_id_keeps_durable_agents_non_phantom() {
        let agent_id =
            build_repl_agent_id(&test_agent_type(AgentMode::Durable), empty_tuple(), None).unwrap();

        assert!(agent_id.phantom_id.is_none());
    }

    #[test]
    fn repl_agent_id_preserves_explicit_phantom_id() {
        let explicit_phantom_id = Uuid::new_v4();
        let agent_id = build_repl_agent_id(
            &test_agent_type(AgentMode::Ephemeral),
            empty_tuple(),
            Some(explicit_phantom_id),
        )
        .unwrap();

        assert_eq!(agent_id.phantom_id, Some(explicit_phantom_id));
    }

    #[test]
    fn normalize_public_agent_id_auto_generates_phantom_for_ephemeral_agents() {
        let agent_id =
            ParsedAgentId::new(AgentTypeName("repl-agent".to_string()), empty_tuple(), None)
                .unwrap();
        let normalized =
            normalize_public_agent_id(&agent_id, &test_agent_type(AgentMode::Ephemeral)).unwrap();

        assert!(normalized.phantom_id.is_some());
    }
}
