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

pub(crate) mod invocation_session;
pub(crate) mod oplog_export;
pub mod oplog_serve;
mod stream;
mod stream_output;

pub(crate) use stream::insecure_connector;

use crate::command::shared_args::{
    AgentFunctionArgument, AgentFunctionName, AgentIdArgs, PostDeployArgs, StreamArgs,
};
use crate::command::shared_args::{OplogExportArgs, OplogViewerArgs};
use crate::command::worker::{AgentSubcommand, InvocationStdinFormat, InvocationStdoutFormat};
use crate::command_handler::Handlers;
use crate::command_handler::agent::oplog_serve::{
    OplogViewerServer, open_in_browser, oplog_page_entry_view, serve_oplog_viewer,
};
use crate::command_handler::agent::stream::{AgentConnection, LiveOutput};
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::{MapServiceError, ServiceError};
use crate::fs;
use crate::fuzzy::{Error, FuzzySearch};
use crate::log::{
    LogColorize, LogIndent, LogOutput, Output as LogOutputTarget, end_for_closed_output,
    error_message_for_output, is_closed_pipe, log_action, log_error, log_error_action,
    log_failed_to, log_warn, log_warn_action, logln, message_for_output, try_print_bytes,
};
use crate::model::agent::action_result::{
    AgentDeleteView, AgentFileContentsResult, AgentInterruptResult, AgentPluginToggleResult,
    AgentResumeResult, AgentRevertResult, AgentSimulateCrashResult,
};
use crate::model::agent::files::{AgentFilesView, FileNodeView};
use crate::model::agent::oplog::{AgentOplogEntryView, AgentOplogPageView};
use crate::model::agent::oplog_html::{
    OplogViewerDocument, OplogViewerMode, render_oplog_viewer_html,
};
use crate::model::agent::{
    AgentAddress, AgentCreateView, AgentGetView, AgentListCursor, AgentUpsertView,
    agent_id_display, format_agent_id_match, format_timestamp,
};
use crate::model::component::ComponentNameMatchKind;
use crate::model::deploy::{AgentUpdateMeta, TryUpdateAllWorkersView};
use crate::model::help::{
    AgentNameHelp, ArgumentError, AvailableAgentConstructorsHelp, AvailableFunctionNamesHelp,
    ParameterErrorTableView,
};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text_format::{log_fuzzy_match, log_text_view};
use anyhow::{anyhow, bail};
use chrono::{DateTime, Utc};
use colored::Colorize;
use golem_common::model::invocation_session_public::PublicErrorCode;

use crate::agent_id_display::SourceLanguage;
use crate::context::GlobalEnvironmentSelector;
use crate::model::agent::{
    AgentActionError, AgentIdMatch, AgentListMode, AgentMetadata, AgentMetadataView,
    AgentUpdateMode, AgentsMetadataResponseView, BulkAgentActionResult, RawAgentId,
    RedeployAgentError,
};
use crate::model::environment::{
    EnvironmentReference, EnvironmentResolveMode, ResolvedEnvironmentIdentity,
};
use golem_client::api::{AgentClient, ComponentClient, EnvironmentClient, WorkerClient};
use golem_client::model::ScanCursor;
use golem_client::model::{
    AgentInvocationMode, AgentInvocationRequest, AgentKeyRequest, ComponentDto, RevertWorkerTarget,
    UpdateWorkerRequest, WorkersMetadataRequest,
};
use golem_common::model::agent::typed_constructor_parameters;
use golem_common::model::agent::{
    AgentConfigEntryDto, RevertLastInvocations, RevertToOplogIndex, UpdateRecord,
};
use golem_common::model::agent::{
    AgentConfigSource, AgentMode, AgentTypeName, DeployedRegisteredAgentType, ParsedAgentKey,
};
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentRevision;
use golem_common::model::component::{ComponentId, ComponentName};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::oplog::{OplogCursor, PublicOplogPageEntry};
use golem_common::model::{
    AgentFilter, AgentFingerprint, AgentId, FilterComparator, IdempotencyKey, OplogIndex,
};
use golem_common::schema::agent::{AgentTypeKind, AgentTypeSchema, InputSchema};
use golem_common::schema::{ExternalSchemaValue, SchemaGraph, SchemaType, SchemaValue};

use crate::command_handler::interactive::Ask;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::execute;
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use inquire::Confirm;
use itertools::{EitherOrBoth, Itertools};
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Stdout, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use terminal_size::terminal_size;
use tokio::time::sleep;
use tracing::debug;
use uuid::Uuid;

fn client_update_mode(mode: AgentUpdateMode) -> golem_common::model::agent::AgentUpdateMode {
    match mode {
        AgentUpdateMode::Automatic => golem_common::model::agent::AgentUpdateMode::Automatic,
        AgentUpdateMode::Manual => golem_common::model::agent::AgentUpdateMode::Manual,
    }
}

fn is_selected_update_attempt(
    pending_update_index: Option<OplogIndex>,
    selected_update_attempt_index: OplogIndex,
) -> bool {
    pending_update_index == Some(selected_update_attempt_index)
}

fn pending_update_progress(
    target_revision: ComponentRevision,
    selected_update_attempt_index: OplogIndex,
) -> String {
    format!(
        "to revision {} for attempt index {} is still pending",
        target_revision.to_string().log_color_highlight(),
        selected_update_attempt_index
            .to_string()
            .log_color_highlight(),
    )
}

pub struct AgentCommandHandler {
    ctx: Arc<Context>,
}

/// Above this many embedded entries the static viewer page becomes slow to open, so the command
/// points at the served viewer.
const STATIC_VIEWER_ENTRY_WARNING_THRESHOLD: usize = 20_000;

impl AgentCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub fn handle_command(
        &self,
        subcommand: AgentSubcommand,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + '_>> {
        Box::pin(async move {
            match subcommand {
                AgentSubcommand::Create {
                    agent_id,
                    keyed,
                    idempotency_key,
                    env,
                    config,
                } => {
                    self.cmd_create(agent_id, keyed, idempotency_key, env, config)
                        .await
                }
                AgentSubcommand::Upsert { agent_id } => self.cmd_upsert(agent_id).await,
                AgentSubcommand::Invoke {
                    agent_id,
                    function_name,
                    arguments,
                    trigger,
                    idempotency_key,
                    no_stream,
                    stream_args,
                    stdin_format,
                    stdout_format,
                    save_session,
                    resume_session,
                    takeover_session,
                    post_deploy_args,
                    schedule_at,
                    expected_fingerprint,
                } => {
                    self.cmd_invoke(
                        agent_id,
                        &function_name,
                        arguments,
                        trigger,
                        idempotency_key,
                        no_stream,
                        stream_args,
                        stdin_format,
                        stdout_format,
                        save_session,
                        resume_session,
                        takeover_session,
                        post_deploy_args,
                        schedule_at,
                        expected_fingerprint.map(AgentFingerprint),
                    )
                    .await
                }
                AgentSubcommand::Get { agent_id } => self.cmd_get(agent_id).await,
                AgentSubcommand::Delete { agent_id } => self.cmd_delete(agent_id).await,
                AgentSubcommand::List {
                    agent_type_name,
                    component_name,
                    filter: filters,
                    mode,
                    scan_cursor,
                    max_count,
                    page,
                    precise,
                    refresh,
                } => {
                    self.cmd_list(
                        agent_type_name,
                        component_name,
                        filters,
                        mode,
                        scan_cursor,
                        max_count,
                        page,
                        precise,
                        refresh,
                    )
                    .await
                }
                AgentSubcommand::Stream {
                    agent_id,
                    stream_args,
                } => self.cmd_stream(agent_id, stream_args).await,
                AgentSubcommand::ReplStream {
                    agent_type_name,
                    stream_args,
                } => self.cmd_repl_stream(agent_type_name, stream_args).await,
                AgentSubcommand::Interrupt { agent_id } => self.cmd_interrupt(agent_id).await,
                AgentSubcommand::Update {
                    agent_id,
                    mode,
                    target_revision,
                    r#await,
                    disable_wakeup,
                } => {
                    self.cmd_update(
                        agent_id,
                        mode.unwrap_or(AgentUpdateMode::Automatic),
                        target_revision,
                        r#await,
                        disable_wakeup,
                    )
                    .await
                }
                AgentSubcommand::Resume { agent_id } => self.cmd_resume(agent_id).await,
                AgentSubcommand::SimulateCrash { agent_id } => {
                    self.cmd_simulate_crash(agent_id).await
                }
                AgentSubcommand::Oplog {
                    agent_id,
                    from,
                    query,
                    omit_payloads,
                    page,
                    cursor,
                    viewer,
                    export,
                } => {
                    if page {
                        self.cmd_oplog_page(agent_id, from, cursor, query, omit_payloads)
                            .await
                    } else {
                        self.cmd_oplog(agent_id, from, query, omit_payloads, viewer, export)
                            .await
                    }
                }
                AgentSubcommand::Revert {
                    agent_id,
                    last_oplog_index,
                    number_of_invocations,
                } => {
                    self.cmd_revert(agent_id, last_oplog_index, number_of_invocations)
                        .await
                }
                AgentSubcommand::Files { agent_id, path } => self.cmd_files(agent_id, path).await,
                AgentSubcommand::FileContents {
                    agent_id,
                    path,
                    output,
                } => self.cmd_file_contents(agent_id, path, output).await,
                AgentSubcommand::ActivatePlugin {
                    agent_id,
                    plugin_name,
                    plugin_priority,
                } => {
                    self.cmd_activate_plugin(agent_id, plugin_name, plugin_priority)
                        .await
                }
                AgentSubcommand::DeactivatePlugin {
                    agent_id,
                    plugin_name,
                    plugin_priority,
                } => {
                    self.cmd_deactivate_plugin(agent_id, plugin_name, plugin_priority)
                        .await
                }
            }
        })
    }

    /// The address of a create or upsert, which must be a key.
    async fn key_address(
        &self,
        agent_id: AgentIdArgs,
        post_deploy_args: Option<&PostDeployArgs>,
    ) -> anyhow::Result<(AgentIdMatch, ParsedAgentKey)> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        self.ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                &agent_id_match.environment,
                agent_id_match.component_name_match_kind,
                &agent_id_match.component_name,
                None,
                post_deploy_args,
                None,
                false,
            )
            .await?;
        validate_ordinary_agent_type(&agent_id_match.agent_type.agent_type)?;
        let AgentAddress::Key(key) = &agent_id_match.address else {
            log_error(format!(
                "Agent {} is an agent ID; this command takes {}",
                agent_id_match.display_name().log_color_error_highlight(),
                "<AGENT_TYPE>(<AGENT_PARAMETERS>)".log_color_highlight()
            ));
            bail!(NonSuccessfulExit);
        };
        let key = (**key).clone();
        Ok((agent_id_match, key))
    }

    async fn cmd_create(
        &self,
        agent_id: AgentIdArgs,
        keyed: bool,
        idempotency_key: Option<IdempotencyKey>,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    ) -> anyhow::Result<()> {
        let (agent_id_match, key) = self.key_address(agent_id, None).await?;

        log_action(
            "Creating",
            format!(
                "{}agent {}",
                if keyed { "keyed " } else { "" },
                format_agent_id_match(&agent_id_match)
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        let reference = clients
            .agent
            .create_agent(
                &agent_id_match.environment.environment_id.0,
                &agent_id_match.agent_type_name.0,
                idempotency_key.as_ref().map(|key| key.value.as_str()),
                &golem_client::model::CreateAgentRequest {
                    parameters: key_parameters(&key)?,
                    config: Some(config),
                    env: Some(env.into_iter().collect()),
                    keyed: Some(keyed),
                },
            )
            .await
            .map_service_error()?;

        logln("");
        self.ctx.log_handler().log_output(AgentCreateView {
            component_name: agent_id_match.component_name,
            agent_type: agent_id_match.agent_type_name,
            agent_id: AgentId(reference.agent_id),
            agent_fingerprint: AgentFingerprint(reference.agent_fingerprint),
        })?;

        Ok(())
    }

    async fn cmd_upsert(&self, agent_id: AgentIdArgs) -> anyhow::Result<()> {
        let (agent_id_match, key) = self.key_address(agent_id, None).await?;

        log_action(
            "Upserting",
            format!("agent {}", format_agent_id_match(&agent_id_match)),
        );

        let clients = self.ctx.golem_clients().await?;
        let reference = clients
            .agent
            .upsert_agent(
                &agent_id_match.environment.environment_id.0,
                &agent_id_match.agent_type_name.0,
                &AgentKeyRequest {
                    parameters: key_parameters(&key)?,
                },
            )
            .await
            .map_service_error()?;

        logln("");
        self.ctx.log_handler().log_output(AgentUpsertView {
            component_name: agent_id_match.component_name,
            agent_type: agent_id_match.agent_type_name,
            agent_id: AgentId(reference.agent_id),
            agent_fingerprint: AgentFingerprint(reference.agent_fingerprint),
        })?;

        Ok(())
    }

    async fn cmd_invoke(
        &self,
        agent_id: AgentIdArgs,
        function_name: &AgentFunctionName,
        arguments: Vec<AgentFunctionArgument>,
        trigger: bool,
        idempotency_key: Option<IdempotencyKey>,
        no_stream: bool,
        stream_args: StreamArgs,
        stdin_format: InvocationStdinFormat,
        stdout_format: InvocationStdoutFormat,
        save_session: Option<std::path::PathBuf>,
        resume_session: Option<std::path::PathBuf>,
        takeover_session: Option<std::path::PathBuf>,
        post_deploy_args: Option<PostDeployArgs>,
        schedule_at: Option<DateTime<Utc>>,
        expected_fingerprint: Option<AgentFingerprint>,
    ) -> anyhow::Result<()> {
        let _raw_output_guard = (stdout_format == InvocationStdoutFormat::Raw)
            .then(|| LogOutput::new(LogOutputTarget::None));
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

        let session_mode = match (save_session, resume_session, takeover_session) {
            (save_session, None, None) => {
                invocation_session::InvocationSessionMode::Start { save_session }
            }
            (None, Some(path), None) => invocation_session::InvocationSessionMode::Resume {
                path,
                takeover: false,
            },
            (None, None, Some(path)) => invocation_session::InvocationSessionMode::Resume {
                path,
                takeover: true,
            },
            _ => unreachable!("clap enforces mutually exclusive session state options"),
        };
        let idempotency_key = if let Some(path) = session_mode.resume_path() {
            invocation_session::load_session_idempotency_key(path)?
        } else {
            match idempotency_key {
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
            }
        };

        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;

        let component = match &agent_id_match.address {
            AgentAddress::Id(id) => {
                self.recorded_agent_component(agent_id_match.environment.environment_id, *id)
                    .await?
                    .0
            }
            AgentAddress::Key(_) => {
                self.ctx
                    .component_handler()
                    .component_by_name_with_auto_deploy(
                        &agent_id_match.environment,
                        agent_id_match.component_name_match_kind,
                        &agent_id_match.component_name,
                        None,
                        post_deploy_args.as_ref(),
                        None,
                        false,
                    )
                    .await?
            }
        };

        let component_metadata: ComponentMetadata = component.metadata.clone().into();
        let Some(agent_type) = component_metadata
            .agent_types()
            .iter()
            .find(|agent_type| agent_type.type_name == agent_id_match.agent_type_name)
            .cloned()
        else {
            bail!(
                "Agent type {} is not implemented by component {}",
                agent_id_match.agent_type_name,
                component.component_name
            );
        };
        validate_ordinary_agent_type(&agent_type)?;

        let matched_method_name = resolve_agent_method_name(function_name, &agent_type);
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
                        &agent_type,
                    ));

                    bail!(NonSuccessfulExit);
                }
            },
        };

        if trigger {
            log_action(
                "Triggering",
                format!(
                    "invocation for agent {}/{}",
                    format_agent_id_match(&agent_id_match),
                    method_name.log_color_highlight()
                ),
            );
        } else {
            log_action(
                "Invoking",
                format!(
                    "agent {}/{} ",
                    format_agent_id_match(&agent_id_match),
                    method_name.log_color_highlight()
                ),
            );
        }

        let method_uses_streams = agent_type
            .methods
            .iter()
            .find(|method| method.name == method_name)
            .is_some_and(|method| method.uses_streams(&agent_type.schema));
        if !method_uses_streams && stdin_format == InvocationStdinFormat::Raw {
            bail!("--stdin-format raw requires a direct stream<binary> or stream<u8> parameter");
        }
        if !method_uses_streams && stdout_format == InvocationStdoutFormat::Raw {
            bail!("--stdout-format raw requires a direct stream<binary> or stream<u8> result");
        }

        if trigger && method_uses_streams {
            bail!("Streaming agent methods require an attached invocation session");
        }

        if !method_uses_streams && session_mode.uses_checkpoint() {
            bail!("durable session save/resume options require a streaming agent method");
        }

        let selected_agent_name = agent_id_match.display_name();
        let environment = &agent_id_match.environment;
        let wait_for_invocation = !trigger && stdout_format == InvocationStdoutFormat::Value;
        let live_output = (!no_stream).then(|| {
            Arc::new(LiveOutput::new(
                self.ctx.clone(),
                environment.environment_id,
                stream_args.into(),
                wait_for_invocation,
            ))
        });
        // An ID address names the agent before the call, so its output can be shown from the
        // start. An awaited call by key with an expected fingerprint needs an existing agent, so
        // its ID is looked up before the call for the same reason. Any other key address names
        // the agent only when the server accepts the invocation.
        let known_agent_id = match &agent_id_match.address {
            AgentAddress::Id(agent_id) => Some(*agent_id),
            AgentAddress::Key(_)
                if live_output.is_some()
                    && expected_fingerprint.is_some()
                    && !trigger
                    && !method_uses_streams =>
            {
                Some(self.agent_id_of(&agent_id_match).await?)
            }
            AgentAddress::Key(_) => None,
        };
        if let (Some(live_output), Some(agent_id)) = (&live_output, &known_agent_id) {
            live_output.open(agent_id, &selected_agent_name).await?;
        }

        // The invocation session addresses an agent by its key, without a fingerprint check.
        let session_key = match &agent_id_match.address {
            AgentAddress::Key(key) if expected_fingerprint.is_none() => Some((**key).clone()),
            _ => None,
        };

        if method_uses_streams {
            let Some(key) = session_key else {
                bail!(
                    "Streaming agent methods are invoked by an agent key, without an expected fingerprint"
                );
            };
            let result = invocation_session::invoke(
                self.ctx.clone(),
                invocation_session::InvocationSessionArgs {
                    environment_id: environment.environment_id,
                    application_name: environment.application_name.to_string(),
                    environment_name: environment.environment_name.to_string(),
                    agent_type,
                    parsed_agent_id: key,
                    method_name,
                    arguments,
                    config: Vec::new(),
                    idempotency_key,
                    stdin_format,
                    stdout_format,
                    selected_agent_name,
                    session_mode,
                    live_output: live_output.clone(),
                },
            )
            .await;

            if let Some(live_output) = live_output {
                live_output.finish().await;
            }
            return result;
        }

        let source_language = SourceLanguage::from(agent_type.source_language.as_str());
        let method_parameters = parse_method_parameters_with_error_table(
            &agent_type,
            &method_name,
            arguments,
            &source_language,
        )?;

        if !trigger && let Some(key) = session_key {
            let invocation = invocation_session::invoke_scalar(
                self.ctx.clone(),
                invocation_session::ScalarInvocationArgs {
                    environment_id: environment.environment_id,
                    application_name: environment.application_name.to_string(),
                    environment_name: environment.environment_name.to_string(),
                    agent_type: agent_type.clone(),
                    parsed_agent_id: key,
                    method_name: method_name.clone(),
                    method_parameters,
                    idempotency_key,
                    selected_agent_name,
                    live_output: live_output.clone(),
                },
            )
            .await;
            if let Some(live_output) = live_output {
                live_output.finish().await;
            }
            let invocation = invocation.map_err(|error| {
                self.with_previous_invocation_failed_hint(error, &agent_id_match)
            })?;
            logln("");
            self.ctx
                .log_handler()
                .log_output(InvokeResultView::new_agent_invoke(
                    invocation.idempotency_key,
                    invocation.result,
                    &agent_type,
                    &method_name,
                ))?;
            return Ok(());
        }

        let (agent_id, parameters) = match (known_agent_id, &agent_id_match.address) {
            (Some(agent_id), _) => (Some(agent_id.0), None),
            (None, AgentAddress::Key(key)) => (None, Some(key_parameters(key)?)),
            (None, AgentAddress::Id(agent_id)) => (Some(agent_id.0), None),
        };
        let request = AgentInvocationRequest {
            agent_id,
            parameters,
            method: method_name.clone(),
            method_parameters: ExternalSchemaValue::try_from(method_parameters)
                .map_err(anyhow::Error::msg)?,
            mode: if trigger {
                AgentInvocationMode::Schedule
            } else {
                AgentInvocationMode::Await
            },
            schedule_at,
            expected_fingerprint: expected_fingerprint.map(|fingerprint| fingerprint.0),
        };

        let clients = self.ctx.golem_clients().await?;
        let result = clients
            .agent
            .invoke_agent(
                &environment.environment_id.0,
                &agent_id_match.agent_type_name.0,
                Some(&idempotency_key.value),
                &request,
            )
            .await
            .map_service_error()
            .map_err(|error| {
                self.with_previous_invocation_failed_hint(error.into(), &agent_id_match)
            });
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(live_output) = live_output {
                    live_output.finish().await;
                }
                return Err(error);
            }
        };

        // The key the server accepted the invocation under, which cancels and looks it up.
        let accepted_key = golem_common::model::InvocationId(result.idempotency_key);
        if let Some(live_output) = live_output {
            live_output
                .accepted(
                    &AgentId(result.agent_id),
                    &selected_agent_name,
                    accepted_key,
                )
                .await?;
            live_output.finish().await;
        }

        if trigger {
            log_action("Triggered", "invocation");
            self.ctx
                .log_handler()
                .log_output(InvokeResultView::new_trigger(accepted_key))?;
        } else {
            let background_failure = result.background_failure.map(|failure| failure.error);
            let result = result
                .result
                .map(golem_common::schema::TypedSchemaValue::from);
            logln("");
            self.ctx.log_handler().log_output(
                InvokeResultView::new_agent_invoke(accepted_key, result, &agent_type, &method_name)
                    .with_background_failure(background_failure),
            )?;
        }

        Ok(())
    }

    async fn cmd_stream(
        &self,
        agent_id: AgentIdArgs,
        stream_args: StreamArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let agent_id = self.agent_id_of(&agent_id_match).await?;

        log_action(
            "Connecting",
            format!("to agent {}", format_agent_id_match(&agent_id_match)),
        );

        let connection = AgentConnection::new(
            self.ctx.worker_service_url().clone(),
            self.ctx.auth_token().await?,
            &agent_id_match.environment.environment_id,
            &agent_id,
            agent_id_match.display_name(),
            stream_args.into(),
            self.ctx.allow_insecure(),
            self.ctx.format(),
            self.ctx.agent_stream_ping_interval(),
            false,
        )
        .await?;

        connection
            .run_forever()
            .await
            .map_err(|error| self.with_previous_invocation_failed_hint(error, &agent_id_match))?;

        Ok(())
    }

    async fn cmd_repl_stream(
        &self,
        agent_type_name: AgentTypeName,
        stream_args: StreamArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await?;

        // The REPL writes the accepted invocation ID to stdin once the call returns.
        use tokio::io::AsyncBufReadExt;
        let stdin = crate::command_handler::interactive::async_stdin("the accepted invocation ID")?;
        let mut lines = tokio::io::BufReader::new(stdin).lines();
        let Some(line) = lines.next_line().await? else {
            return Ok(());
        };
        let invocation = line
            .trim()
            .parse::<golem_common::model::OwnedInvocationId>()
            .map_err(|error| anyhow!("stdin contains an invalid invocation ID: {error}"))?;

        let live_output = LiveOutput::new(
            self.ctx.clone(),
            environment.environment_id,
            stream_args.into(),
            true,
        );
        live_output
            .accepted(
                invocation.agent_id(),
                &agent_id_display(Some(&agent_type_name), invocation.agent_id()),
                invocation.id(),
            )
            .await?;
        live_output.finish().await;

        Ok(())
    }

    async fn cmd_simulate_crash(&self, agent_id: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let agent_id = self.agent_id_of(&agent_id_match).await?;

        log_action(
            "Simulating crash",
            format!("for agent {}", format_agent_id_match(&agent_id_match)),
        );

        self.interrupt_agent(&agent_id_match, &agent_id, true)
            .await?;

        log_action(
            "Simulated crash",
            format!("for agent {}", format_agent_id_match(&agent_id_match)),
        );

        self.ctx
            .log_handler()
            .log_output(AgentSimulateCrashResult {
                simulated: true,
                agent_id,
            })?;

        Ok(())
    }

    async fn cmd_oplog(
        &self,
        agent_id: AgentIdArgs,
        from: Option<u64>,
        query: Option<String>,
        omit_payloads: bool,
        viewer: OplogViewerArgs,
        export: OplogExportArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let lookup_error = |error| {
            if export.obfuscate {
                oplog_export::read_failed(error)
            } else {
                error
            }
        };
        let (agent_id_match, component, agent_id) = {
            let _log_output = export
                .obfuscate
                .then(|| LogOutput::new(LogOutputTarget::TracingDebug));
            let agent_id_match = self
                .match_agent_id(agent_id.agent_id)
                .await
                .map_err(lookup_error)?;
            let (component, agent_id) = self
                .component_by_agent_id_match(&agent_id_match)
                .await
                .map_err(lookup_error)?;
            (agent_id_match, component, agent_id)
        };

        let batch_size = self.ctx.http_batch_size();
        if export.obfuscate {
            let options = oplog_export::export_options(&export, from, query, batch_size);
            if viewer.serve {
                return oplog_serve::serve_oplog_export_viewer(
                    self.ctx.clone(),
                    agent_id_match.environment.environment_id,
                    agent_id,
                    options,
                    viewer.port,
                    !viewer.no_open,
                )
                .await;
            }
            return oplog_export::cmd_oplog_export(
                self.ctx.clone(),
                agent_id_match.environment.environment_id,
                agent_id,
                options,
                viewer,
            )
            .await;
        }
        let viewer_document =
            |mode: OplogViewerMode, entries: Vec<AgentOplogEntryView>| OplogViewerDocument {
                mode,
                agent_id: format_agent_id_match(&agent_id_match),
                component_name: component.component_name.0.clone(),
                component_id: component.id.0.to_string(),
                generated_at: Utc::now().to_rfc3339(),
                cli_version: crate::version().to_string(),
                from,
                query: query.clone(),
                page_size: batch_size,
                entries,
            };

        if viewer.serve {
            let page = render_oplog_viewer_html(&viewer_document(OplogViewerMode::Live, vec![]))?;
            return serve_oplog_viewer(
                OplogViewerServer {
                    ctx: self.ctx.clone(),
                    environment_id: agent_id_match.environment.environment_id,
                    agent_id,
                    page,
                    page_size: batch_size,
                    omit_payloads,
                    export: None,
                },
                viewer.port,
                !viewer.no_open,
            )
            .await;
        }

        let html = viewer.html;
        let mut cursor = Option::<OplogCursor>::None;
        let mut had_entries = false;
        let print_entries = html.is_none() || self.ctx.format().is_structured();
        let mut collected_entries = Vec::<AgentOplogEntryView>::new();
        loop {
            let mut entries = Vec::<PublicOplogPageEntry>::new();
            cursor = {
                let clients = self.ctx.golem_clients().await?;

                let result = clients
                    .worker
                    .get_oplog(
                        &agent_id_match.environment.environment_id.0,
                        &agent_id.0,
                        from,
                        batch_size,
                        cursor.as_ref(),
                        query.as_deref(),
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .map_service_error()?;

                entries.extend(result.entries);
                result.next
            };

            if !entries.is_empty() {
                had_entries = true;
                for entry in entries {
                    let view = oplog_page_entry_view(
                        &self.ctx,
                        &agent_id_match.environment.environment_id,
                        &agent_id,
                        entry,
                        omit_payloads,
                    )
                    .await?;
                    if html.is_some() {
                        collected_entries.push(view.clone());
                    }
                    if print_entries {
                        self.ctx.log_handler().log_output(view)?;
                    }
                }
            }

            if cursor.is_none() {
                break;
            }
        }

        if let Some(html_path) = html {
            let entry_count = collected_entries.len();
            let rendered = render_oplog_viewer_html(&viewer_document(
                OplogViewerMode::Static,
                collected_entries,
            ))?;
            if let Some(parent) = html_path.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent)?;
            }
            fs::write_str(&html_path, rendered)?;
            log_action(
                "Generated",
                format!(
                    "HTML oplog viewer with {} entries: {}",
                    entry_count,
                    html_path.display().to_string().log_color_highlight()
                ),
            );
            if entry_count > STATIC_VIEWER_ENTRY_WARNING_THRESHOLD {
                log_warn(format!(
                    "The page embeds {entry_count} entries; large pages are slow to open. Use {} to load the oplog page by page instead.",
                    "--serve".log_color_highlight()
                ));
            }
            if viewer.open {
                let absolute = fs::absolute_lexical_path(&html_path)?;
                match url::Url::from_file_path(&absolute) {
                    Ok(file_url) => open_in_browser(file_url.as_str()),
                    Err(()) => log_warn(format!(
                        "Could not build a file URL for {}",
                        absolute.display()
                    )),
                }
            }
        } else if !self.ctx.format().is_structured() && !had_entries {
            log_warn("No results.")
        }

        Ok(())
    }

    /// One server page of the oplog, as one `agent.oplog-page` document.
    async fn cmd_oplog_page(
        &self,
        agent_id: AgentIdArgs,
        from: Option<u64>,
        cursor: Option<OplogCursor>,
        query: Option<String>,
        omit_payloads: bool,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let agent_id = self.agent_id_of(&agent_id_match).await?;
        let environment_id = agent_id_match.environment.environment_id;

        let page = self
            .ctx
            .golem_clients()
            .await?
            .worker
            .get_oplog(
                &environment_id.0,
                &agent_id.0,
                from,
                self.ctx.http_batch_size(),
                cursor.as_ref(),
                query.as_deref(),
                None,
                None,
                None,
                None,
            )
            .await
            .map_service_error()?;

        let mut entries = Vec::with_capacity(page.entries.len());
        for entry in page.entries {
            entries.push(
                oplog_page_entry_view(&self.ctx, &environment_id, &agent_id, entry, omit_payloads)
                    .await?,
            );
        }
        self.ctx.log_handler().log_output(AgentOplogPageView {
            entries,
            cursor: page.next.map(|cursor| cursor.to_string()),
        })
    }

    async fn cmd_revert(
        &self,
        agent_id: AgentIdArgs,
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
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let agent_id = self.agent_id_of(&agent_id_match).await?;

        if !self.ctx.interactive_handler().confirm_revert_agent(
            &agent_id_match.display_name(),
            last_oplog_index,
            number_of_invocations,
        )? {
            bail!(NonSuccessfulExit);
        }

        log_action(
            "Reverting",
            format!("agent {}", format_agent_id_match(&agent_id_match)),
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
                .revert_worker(
                    &agent_id_match.environment.environment_id.0,
                    &agent_id.0,
                    None,
                    &target,
                )
                .await
                .map(|_| ())
                .map_service_error()?
        }

        log_action(
            "Reverted",
            format!("agent {}", format_agent_id_match(&agent_id_match)),
        );

        self.ctx.log_handler().log_output(AgentRevertResult {
            reverted: true,
            agent_id,
            last_oplog_index,
            number_of_invocations,
        })?;

        Ok(())
    }

    async fn cmd_list(
        &self,
        agent_type_name: Option<AgentTypeName>,
        component_name: Option<ComponentName>,
        filters: Vec<String>,
        mode: AgentListMode,
        scan_cursor: Option<AgentListCursor>,
        max_count: Option<u64>,
        page: bool,
        precise: bool,
        refresh: Option<u64>,
    ) -> anyhow::Result<()> {
        if refresh.is_some() && self.ctx.format().is_structured() {
            bail!("Refresh mode is only supported with --format text");
        }

        let mode_overlay = if mode == AgentListMode::All && !user_set_mode_filter(&filters) {
            Some(all_modes_filter())
        } else {
            None
        };
        let filters = apply_list_mode_filter(filters, mode);
        let targets = self
            .resolve_list_components(agent_type_name, component_name)
            .await?;

        if page {
            let view = self
                .list_agents_page(
                    targets,
                    &filters,
                    mode_overlay.as_ref(),
                    scan_cursor.as_ref(),
                    max_count.unwrap_or(self.ctx.http_batch_size()),
                    precise,
                )
                .await?;
            self.ctx.log_handler().log_output(view)
        } else if let Some(interval_ms) = refresh {
            self.list_with_refresh(
                &targets,
                &filters,
                mode_overlay.as_ref(),
                scan_cursor.as_ref(),
                max_count,
                precise,
                interval_ms,
            )
            .await
        } else {
            let view = self
                .list_agents(
                    &targets,
                    &filters,
                    mode_overlay.as_ref(),
                    scan_cursor.as_ref(),
                    max_count,
                    precise,
                    false,
                )
                .await?;
            self.ctx.log_handler().log_output(view)?;
            Ok(())
        }
    }

    async fn list_with_refresh(
        &self,
        targets: &[AgentListTarget],
        filters: &[String],
        mode_overlay: Option<&AgentFilter>,
        scan_cursor: Option<&AgentListCursor>,
        max_count: Option<u64>,
        precise: bool,
        interval_ms: u64,
    ) -> anyhow::Result<()> {
        let duration = Duration::from_millis(interval_ms);
        let mut screen = match AlternateScreenGuard::enter() {
            Ok(screen) => screen,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(is_closed_pipe) =>
            {
                end_for_closed_output();
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let result: anyhow::Result<()> = async {
            loop {
                let term_height = terminal_size()
                    .map(|(_, h)| h.0 as usize)
                    .unwrap_or(usize::MAX);

                // Fetch first — while the previous frame is still visible
                let output = self
                    .list_agents(
                        targets,
                        filters,
                        mode_overlay,
                        scan_cursor,
                        max_count,
                        precise,
                        true,
                    )
                    .await
                    .and_then(|view| {
                        self.ctx
                            .log_handler()
                            .render_view_truncated(view, term_height)
                    })
                    .unwrap_or_else(|e| format!("Error: {e:#}"));

                // Clear and write in one buffered flush — no blank-screen gap
                let drawn = (|| {
                    queue!(screen.stdout_mut(), MoveTo(0, 0), Clear(ClearType::All))?;
                    write!(screen.stdout_mut(), "{output}")?;
                    screen.stdout_mut().flush()
                })();
                match drawn {
                    Ok(()) => {}
                    Err(error) if is_closed_pipe(&error) => {
                        end_for_closed_output();
                        return Ok(());
                    }
                    Err(error) => return Err(error.into()),
                }

                tokio::select! {
                    _ = sleep(duration) => {}
                    _ = tokio::signal::ctrl_c() => { return Ok(()); }
                }
            }
        }
        .await;
        match screen.leave() {
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(is_closed_pipe) =>
            {
                end_for_closed_output()
            }
            other => other?,
        }
        result
    }

    /// The latest declaration on each component that has implemented a type. Reflection
    /// still uses the current deployment; management also needs these retained declarations.
    async fn historical_agent_types(
        &self,
        environment_id: EnvironmentId,
    ) -> anyhow::Result<Vec<DeployedRegisteredAgentType>> {
        let clients = self.ctx.golem_clients().await?;
        let mut deployments = clients
            .environment
            .list_deployments(&environment_id.0, None)
            .await
            .map_service_error()?
            .values;
        deployments.sort_by_key(|deployment| std::cmp::Reverse(deployment.revision));
        let mut seen = BTreeSet::new();
        let mut types = Vec::new();
        for deployment in deployments {
            for agent_type in clients
                .environment
                .list_deployment_agent_types(&environment_id.0, deployment.revision.into())
                .await
                .map_service_error()?
                .values
            {
                if seen.insert((
                    agent_type.implemented_by.component_id.0,
                    agent_type.agent_type.type_name.0.clone(),
                )) {
                    types.push(agent_type);
                }
            }
        }
        Ok(types)
    }

    /// Finds ownership through the public recorded-component filter, never by interpreting
    /// the UUID tag or assuming that the current implementer owns an existing agent.
    async fn recorded_agent_component(
        &self,
        environment_id: EnvironmentId,
        agent_id: AgentId,
    ) -> anyhow::Result<(ComponentDto, DeployedRegisteredAgentType)> {
        let clients = self.ctx.golem_clients().await?;
        let metadata = clients
            .worker
            .get_worker_metadata(&environment_id.0, &agent_id.0, None)
            .await
            .map_service_error()?;
        let historical = self.historical_agent_types(environment_id).await?;
        self.recorded_component_for_metadata(
            &AgentMetadata::from(ComponentName(String::new()), metadata),
            &historical,
        )
        .await
    }

    async fn recorded_component_for_metadata(
        &self,
        metadata: &AgentMetadata,
        historical: &[DeployedRegisteredAgentType],
    ) -> anyhow::Result<(ComponentDto, DeployedRegisteredAgentType)> {
        let clients = self.ctx.golem_clients().await?;
        let environment_id = metadata.environment_id;
        let agent_id = metadata.agent_id;
        let name = metadata
            .agent_type_name
            .as_ref()
            .ok_or_else(|| anyhow!("Agent {agent_id} has no agent type"))?;
        for mut agent_type in historical.iter().cloned() {
            if &agent_type.agent_type.type_name != name {
                continue;
            }
            if historical.iter().any(|other| {
                other.implemented_by.component_name == agent_type.implemented_by.component_name
                    && other.implemented_by.component_id != agent_type.implemented_by.component_id
            }) {
                continue;
            }
            let mut cursor = None;
            let matches_owner = loop {
                let Some(matches) = clients
                    .agent
                    .find_agents_metadata(
                        &environment_id.0,
                        &name.0,
                        Some(&agent_type.implemented_by.component_name),
                        &WorkersMetadataRequest {
                            filter: Some(AgentFilter::new_id(FilterComparator::Equal, agent_id)),
                            cursor,
                            count: Some(self.ctx.http_batch_size()),
                            precise: Some(false),
                        },
                    )
                    .await
                    .map_service_error_not_found_as_opt()?
                else {
                    break false;
                };
                if matches.workers.iter().any(|agent| {
                    agent.agent_id == agent_id
                        && agent.fingerprint == metadata.fingerprint
                        && agent.component_revision == metadata.component_revision
                }) {
                    break true;
                }
                cursor = matches.cursor;
                if cursor.is_none() {
                    break false;
                }
            };
            if !matches_owner {
                continue;
            }
            let component = self
                .ctx
                .component_handler()
                .get_component_revision_by_id(
                    &agent_type.implemented_by.component_id,
                    metadata.component_revision,
                )
                .await?;
            let declaration = component
                .metadata
                .agent_types
                .iter()
                .find(|declaration| &declaration.declaration.type_name == name)
                .ok_or_else(|| anyhow!("The recorded component has no declaration for {name}"))?;
            agent_type.agent_type = declaration.declaration.clone();
            agent_type.implemented_by.component_revision = metadata.component_revision;
            return Ok((component, agent_type));
        }
        bail!("Cannot establish the recorded component of agent {agent_id}")
    }

    async fn resolve_list_components(
        &self,
        agent_type_name: Option<AgentTypeName>,
        component_name: Option<ComponentName>,
    ) -> anyhow::Result<Vec<AgentListTarget>> {
        let clients = self.ctx.golem_clients().await?;
        let component_handler = self.ctx.component_handler();

        match agent_type_name {
            Some(agent_type_name) => {
                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;
                debug!("Finding agent type {}", agent_type_name.0);
                let historical = Arc::new(
                    self.historical_agent_types(environment.environment_id)
                        .await?,
                );
                let Some(agent_type) = historical
                    .iter()
                    .find(|agent_type| agent_type.agent_type.type_name == agent_type_name)
                else {
                    log_error(format!(
                        "Agent type {} not found",
                        agent_type_name.0.log_color_highlight()
                    ));
                    bail!(NonSuccessfulExit)
                };

                Ok(vec![AgentListTarget {
                    component: component_handler
                        .get_component_revision_by_id(
                            &agent_type.implemented_by.component_id,
                            agent_type.implemented_by.component_revision,
                        )
                        .await?,
                    agent_types: vec![agent_type.agent_type.type_name.clone()],
                    component_name_filter: component_name,
                    historical,
                }])
            }
            None => {
                let selected_components = self
                    .ctx
                    .component_handler()
                    .must_select_components_by_app_dir_or_name(component_name.as_ref())
                    .await?;

                let environment = &selected_components.environment;
                let historical = Arc::new(
                    self.historical_agent_types(environment.environment_id)
                        .await?,
                );

                environment
                    .with_current_deployment_revision_or_default_warn(
                        |current_deployment_revision| async move {
                            let mut targets =
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
                                        targets.push(AgentListTarget {
                                            agent_types: component_agent_types(
                                                &component,
                                                &historical,
                                            ),
                                            component_name_filter: Some(
                                                component.component_name.clone(),
                                            ),
                                            historical: historical.clone(),
                                            component,
                                        });
                                    }
                                    None => {
                                        let Some(previous) = historical.iter().find(|ty| {
                                            ty.implemented_by.component_name == component_name.0
                                        }) else {
                                            log_error(format!(
                                                "Component not found: {}",
                                                component_name.0.log_color_error_highlight()
                                            ));
                                            bail!(NonSuccessfulExit)
                                        };
                                        let component = component_handler
                                            .get_component_revision_by_id(
                                                &previous.implemented_by.component_id,
                                                previous.implemented_by.component_revision,
                                            )
                                            .await?;
                                        targets.push(AgentListTarget {
                                            agent_types: component_agent_types(
                                                &component,
                                                &historical,
                                            ),
                                            component_name_filter: Some(component_name),
                                            historical: historical.clone(),
                                            component,
                                        });
                                    }
                                }
                            }

                            Ok(targets)
                        },
                    )
                    .await
            }
        }
    }

    async fn list_agents(
        &self,
        targets: &[AgentListTarget],
        filters: &[String],
        mode_overlay: Option<&AgentFilter>,
        scan_cursor: Option<&AgentListCursor>,
        max_count: Option<u64>,
        precise: bool,
        stable_sort: bool,
    ) -> anyhow::Result<AgentsMetadataResponseView> {
        if let Some(scan_cursor) = scan_cursor {
            let listed = targets
                .iter()
                .flat_map(|target| {
                    target
                        .agent_types
                        .iter()
                        .map(move |agent_type| (target.component.id, agent_type.0.as_str()))
                })
                .collect::<BTreeSet<_>>();
            let unlisted = scan_cursor
                .scans()
                .filter(|scan| !listed.contains(scan))
                .collect::<Vec<_>>();
            if !unlisted.is_empty() {
                log_error(format!(
                    "The cursor continues agent types that are not listed: {}",
                    unlisted
                        .iter()
                        .map(|(component, agent_type)| format!("{component}/{agent_type}"))
                        .join(", ")
                ));
                logln("");
                logln(
                    "Pass the cursor with the agent type or component selection that returned it.",
                );
                logln("");
                bail!(NonSuccessfulExit);
            }
        }

        let mut view = AgentsMetadataResponseView::default();
        let mut next_cursor = AgentListCursor::default();

        for target in targets {
            let (agents, type_scan_cursors) = self
                .list_component_agents(
                    &target.component,
                    &target.agent_types,
                    target
                        .component_name_filter
                        .as_ref()
                        .map(|name| name.0.as_str()),
                    Some(filters),
                    mode_overlay,
                    scan_cursor,
                    max_count,
                    precise,
                )
                .await?;

            view.agents
                .extend(self.agent_views(agents, &target.historical).await?);
            for (agent_type, type_scan_cursor) in type_scan_cursors {
                next_cursor.insert(target.component.id, &agent_type, type_scan_cursor);
            }
        }
        view.cursor = (!next_cursor.is_empty()).then(|| next_cursor.encode());

        if stable_sort {
            view.agents.sort_by(|a, b| {
                a.component_name
                    .cmp(&b.component_name)
                    .then_with(|| a.agent_type.cmp(&b.agent_type))
                    .then_with(|| a.agent_id.0.cmp(&b.agent_id.0))
            });
        }

        Ok(view)
    }

    /// The views of listed agents, with their keys rendered in their source language.
    async fn agent_views(
        &self,
        agents: Vec<AgentMetadata>,
        historical: &[DeployedRegisteredAgentType],
    ) -> anyhow::Result<Vec<AgentMetadataView>> {
        let mut views = Vec::with_capacity(agents.len());
        for mut agent in agents {
            let Ok((agent_component, _)) = self
                .recorded_component_for_metadata(&agent, historical)
                .await
            else {
                agent.component_name = ComponentName(String::new());
                views.push(AgentMetadataView::from(agent));
                continue;
            };

            agent.component_name = agent_component.component_name.clone();
            let parsed_agent_type_name = agent.agent_type_name.clone();
            let component_metadata: ComponentMetadata = agent_component.metadata.into();

            let defaults = parsed_agent_type_name.as_ref().and_then(|agent_type_name| {
                component_metadata
                    .agent_type_provision_configs()
                    .get(agent_type_name)
                    .cloned()
            });

            let source_language = parsed_agent_type_name
                .as_ref()
                .and_then(|type_name| {
                    component_metadata
                        .agent_types()
                        .iter()
                        .find(|at| &at.type_name == type_name)
                        .map(|at| SourceLanguage::from(at.source_language.as_str()))
                })
                .unwrap_or_default();

            let secret_config_paths = parsed_agent_type_name
                .as_ref()
                .map(|agent_type_name| {
                    secret_config_paths_for_agent_type(
                        component_metadata.agent_types(),
                        agent_type_name,
                    )
                })
                .unwrap_or_default();

            let parsed = agent.parsed_key(&component_metadata);
            views.push(
                AgentMetadataView::from(agent)
                    .with_defaults(defaults)
                    .with_secret_config_paths(secret_config_paths)
                    .with_source_language(source_language)
                    .with_rendered_key(parsed.as_ref()),
            );
        }
        Ok(views)
    }

    /// One page across the selected components and their agent types, in name order.
    async fn list_agents_page(
        &self,
        mut targets: Vec<AgentListTarget>,
        filters: &[String],
        mode_overlay: Option<&AgentFilter>,
        cursor: Option<&AgentListCursor>,
        count: u64,
        precise: bool,
    ) -> anyhow::Result<AgentsMetadataResponseView> {
        targets.sort_by(|a, b| {
            a.component
                .component_name
                .0
                .cmp(&b.component.component_name.0)
        });
        for target in &mut targets {
            target.agent_types.sort_by(|a, b| a.0.cmp(&b.0));
        }
        let types = targets
            .iter()
            .flat_map(|target| {
                target
                    .agent_types
                    .iter()
                    .map(move |agent_type| (target, agent_type))
            })
            .collect::<Vec<_>>();
        let names = types
            .iter()
            .map(|(target, agent_type)| (target.component.id, agent_type.0.as_str()))
            .collect::<Vec<_>>();
        let (agents, cursor) = read_page(
            &names,
            cursor,
            count,
            async |position, scan_cursor, limit| {
                let (target, agent_type) = types[position];
                let component = &target.component;
                let mut start = AgentListCursor::default();
                start.insert(
                    component.id,
                    agent_type,
                    scan_cursor.unwrap_or_else(|| ScanCursor::new(String::new())),
                );
                let (agents, cursors) = self
                    .list_component_agents(
                        component,
                        std::slice::from_ref(agent_type),
                        target
                            .component_name_filter
                            .as_ref()
                            .map(|name| name.0.as_str()),
                        Some(filters),
                        mode_overlay,
                        Some(&start),
                        Some(limit),
                        precise,
                    )
                    .await?;
                Ok((
                    self.agent_views(agents, &target.historical).await?,
                    cursors.into_iter().next().map(|(_, cursor)| cursor),
                ))
            },
        )
        .await?;
        Ok(AgentsMetadataResponseView { agents, cursor })
    }

    async fn cmd_interrupt(&self, agent_id: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let (component, agent_id) = self.component_by_agent_id_match(&agent_id_match).await?;

        if component
            .metadata
            .agent_types
            .iter()
            .map(|agent_type| &agent_type.declaration)
            .find(|agent_type| agent_type.type_name == agent_id_match.agent_type_name)
            .is_some_and(|agent_type| agent_type.mode == AgentMode::Ephemeral)
            && !self
                .ctx
                .interactive_handler()
                .confirm_interrupt_ephemeral_agent()?
        {
            bail!(NonSuccessfulExit);
        }

        log_action(
            "Interrupting",
            format!("agent {}", format_agent_id_match(&agent_id_match)),
        );

        self.interrupt_agent(&agent_id_match, &agent_id, false)
            .await?;

        log_action(
            "Interrupted",
            format!("agent {}", format_agent_id_match(&agent_id_match)),
        );

        self.ctx.log_handler().log_output(AgentInterruptResult {
            interrupted: true,
            agent_id,
        })?;

        Ok(())
    }

    async fn cmd_resume(&self, agent_id: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let agent_id = self.agent_id_of(&agent_id_match).await?;

        log_action(
            "Resuming",
            format!("agent {}", format_agent_id_match(&agent_id_match)),
        );

        let clients = self.ctx.golem_clients().await?;
        clients
            .worker
            .resume_worker(
                &agent_id_match.environment.environment_id.0,
                &agent_id.0,
                None,
            )
            .await
            .map(|_| ())
            .map_service_error()
            .map_err(|error| {
                self.with_previous_invocation_failed_hint(error.into(), &agent_id_match)
            })?;

        log_action(
            "Resumed",
            format!("agent {}", format_agent_id_match(&agent_id_match)),
        );

        self.ctx.log_handler().log_output(AgentResumeResult {
            resumed: true,
            agent_id,
        })?;

        Ok(())
    }

    async fn cmd_update(
        &self,
        agent_id: AgentIdArgs,
        mode: AgentUpdateMode,
        target_revision: Option<ComponentRevision>,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;

        let (component, agent_id) = self.component_by_agent_id_match(&agent_id_match).await?;
        let environment = &agent_id_match.environment;

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
                        "Component {} not found, while getting current component revision",
                        component.component_name
                    );
                };

                if !self.ctx.interactive_handler().confirm_update_to_current(
                    &component.component_name,
                    &agent_id_match.display_name(),
                    current_deployed_revision.revision,
                )? {
                    bail!(NonSuccessfulExit)
                }

                current_deployed_revision.revision
            }
        };

        let from_revision = self
            .agent_metadata(
                agent_id_match.environment.environment_id,
                &component.component_name,
                &agent_id,
            )
            .await?
            .map(|metadata| metadata.component_revision)
            .unwrap_or(target_revision);
        let meta = AgentUpdateMeta {
            component_name: component.component_name.clone(),
            agent_id,
            from_revision,
            revision: target_revision,
        };

        let mut update_results = TryUpdateAllWorkersView::default();
        update_results.agents.push(meta);
        match self
            .update_agent(
                &component.component_name,
                &agent_id_match.environment.environment_id,
                &agent_id,
                &agent_id_match.display_name(),
                mode,
                target_revision,
                await_update,
                disable_wakeup,
            )
            .await
        {
            Ok(_) => {}
            Err(error) => {
                update_results.errors.push(AgentActionError {
                    component_name: component.component_name.clone(),
                    agent_id,
                    error: error_message_for_output(&error),
                });
                self.ctx.log_handler().log_output(update_results)?;
                return Err(error);
            }
        }

        self.ctx.log_handler().log_output(update_results)?;

        Ok(())
    }

    async fn cmd_get(&self, agent_id: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let (environment, address) = self.resolve_agent_address(agent_id.agent_id).await?;
        let (type_name, agent_id) = if let Some((name, id)) = parse_agent_id_address(&address) {
            (name, id)
        } else {
            let matched = self
                .match_agent_id_in_environment(environment.clone(), address)
                .await?;
            (
                Some(matched.agent_type_name.0.clone()),
                self.agent_id_of(&matched).await?,
            )
        };

        let clients = self.ctx.golem_clients().await?;
        let result = clients
            .worker
            .get_worker_metadata(&environment.environment_id.0, &agent_id.0, None)
            .await
            .map_service_error()?;
        if type_name.is_some_and(|name| {
            result
                .agent_type_name
                .as_ref()
                .is_none_or(|actual| actual.0 != name)
        }) {
            bail!("Agent ID {agent_id} does not belong to the specified agent type");
        }
        let historical = self
            .historical_agent_types(environment.environment_id)
            .await
            .unwrap_or_default();
        let metadata = AgentMetadata::from(ComponentName(String::new()), result);
        let metadata_view = self
            .agent_views(vec![metadata], &historical)
            .await?
            .remove(0);

        self.ctx
            .log_handler()
            .log_output(AgentGetView::from_metadata(metadata_view, true))?;

        Ok(())
    }

    async fn cmd_delete(&self, agent_id: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let agent_id = self.agent_id_of(&agent_id_match).await?;

        log_warn_action(
            "Deleting",
            format!("agent {}", format_agent_id_match(&agent_id_match)),
        );

        self.delete(&agent_id_match.environment.environment_id, &agent_id)
            .await?;

        self.ctx.log_handler().log_output(AgentDeleteView {
            deleted: true,
            agent_id,
        })?;

        Ok(())
    }

    async fn cmd_files(&self, agent_id: AgentIdArgs, path: String) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let agent_id = self.agent_id_of(&agent_id_match).await?;

        log_action(
            "Listing files",
            format!(
                "for agent {} at path {}",
                format_agent_id_match(&agent_id_match),
                path.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        let nodes = match clients
            .worker
            .get_files(
                &agent_id_match.environment.environment_id.0,
                &agent_id.0,
                &path,
                None,
            )
            .await
            .map_service_error()
        {
            Ok(nodes) => nodes,
            Err(e) => {
                log_warn_action(
                    "Failed to list files",
                    format!(
                        "for agent {} at path {}: {e}",
                        format_agent_id_match(&agent_id_match),
                        path.log_color_error_highlight()
                    ),
                );
                return Err(e.into());
            }
        };

        // Convert nodes to AgentFilesView with human-readable timestamps
        let view = AgentFilesView {
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

        self.ctx.log_handler().log_output(view)?;

        log_action(
            "Listed files",
            format!(
                "for agent {} at path {}",
                format_agent_id_match(&agent_id_match),
                path.log_color_highlight()
            ),
        );

        Ok(())
    }

    async fn cmd_file_contents(
        &self,
        agent_id: AgentIdArgs,
        path: String,
        output: Option<String>,
    ) -> anyhow::Result<()> {
        let stream_to_stdout = output.as_deref() == Some("-");
        // Keep progress and errors separate from the raw byte stream so this
        // mode is safe to pipe into another process or redirect to a file.
        let _stderr_logs = stream_to_stdout.then(|| LogOutput::new(LogOutputTarget::Stderr));
        self.cmd_file_contents_inner(agent_id, path, output, stream_to_stdout)
            .await
    }

    async fn cmd_file_contents_inner(
        &self,
        agent_id: AgentIdArgs,
        path: String,
        output: Option<String>,
        stream_to_stdout: bool,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let agent_id = self.agent_id_of(&agent_id_match).await?;

        log_action(
            "Downloading file",
            format!(
                "from agent {} at path {}",
                format_agent_id_match(&agent_id_match),
                path.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        let file_contents = match clients
            .worker
            .get_file_content(
                &agent_id_match.environment.environment_id.0,
                &agent_id.0,
                &path,
                None,
            )
            .await
            .map_service_error()
        {
            Ok(contents) => contents,
            Err(e) => {
                log_warn_action(
                    "Failed to download file",
                    format!(
                        "from agent {} at path {}: {e}",
                        format_agent_id_match(&agent_id_match),
                        path.log_color_error_highlight()
                    ),
                );
                return Err(e.into());
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

        if stream_to_stdout {
            try_print_bytes(&file_contents)?;
            return Ok(());
        }

        // Check if file exists and ask for confirmation if needed
        if Path::new(&output_path).exists() {
            let should_overwrite = self.confirm_file_overwrite(&output_path)?;
            if !should_overwrite {
                log_action(
                    "File download cancelled",
                    format!("by user for file {}", output_path.log_color_highlight()),
                );
                self.ctx.log_handler().log_output(AgentFileContentsResult {
                    saved: false,
                    agent_id,
                    path,
                    output_path: output_path.into(),
                    bytes: 0,
                })?;
                return Ok(());
            }
        }

        match File::create(&output_path).and_then(|mut file| file.write_all(&file_contents)) {
            Ok(_) => {
                log_action(
                    "File saved",
                    format!("to {}", output_path.log_color_highlight()),
                );
                self.ctx.log_handler().log_output(AgentFileContentsResult {
                    saved: true,
                    agent_id,
                    path,
                    output_path: output_path.into(),
                    bytes: file_contents.len(),
                })?;
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
        agent_id: AgentIdArgs,
        plugin_name: String,
        explicit_priority: Option<i32>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let (component, agent_id) = self.component_by_agent_id_match(&agent_id_match).await?;

        let plugin_priority = self.resolve_plugin_priority(
            &component,
            &agent_id_match.agent_type_name,
            &plugin_name,
            explicit_priority,
        )?;

        log_action(
            "Activating plugin",
            format!(
                "{} for agent {}",
                plugin_name.log_color_highlight(),
                format_agent_id_match(&agent_id_match)
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        clients
            .worker
            .activate_plugin(
                &agent_id_match.environment.environment_id.0,
                &agent_id.0,
                plugin_priority,
                None,
            )
            .await
            .map(|_| ())
            .map_service_error()?;

        log_action(
            "Activated plugin",
            format!(
                "{} for agent {}",
                plugin_name.log_color_highlight(),
                format_agent_id_match(&agent_id_match)
            ),
        );

        self.ctx.log_handler().log_output(AgentPluginToggleResult {
            activated: true,
            agent_id,
            plugin: plugin_name.clone(),
            priority: plugin_priority,
        })?;

        Ok(())
    }

    async fn cmd_deactivate_plugin(
        &self,
        agent_id: AgentIdArgs,
        plugin_name: String,
        explicit_priority: Option<i32>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let agent_id_match = self.match_agent_id(agent_id.agent_id).await?;
        let (component, agent_id) = self.component_by_agent_id_match(&agent_id_match).await?;

        let plugin_priority = self.resolve_plugin_priority(
            &component,
            &agent_id_match.agent_type_name,
            &plugin_name,
            explicit_priority,
        )?;

        log_action(
            "Deactivating plugin",
            format!(
                "{} for agent {}",
                plugin_name.log_color_highlight(),
                format_agent_id_match(&agent_id_match)
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        clients
            .worker
            .deactivate_plugin(
                &agent_id_match.environment.environment_id.0,
                &agent_id.0,
                plugin_priority,
                None,
            )
            .await
            .map(|_| ())
            .map_service_error()?;

        log_action(
            "Deactivated plugin",
            format!(
                "{} for agent {}",
                plugin_name.log_color_highlight(),
                format_agent_id_match(&agent_id_match)
            ),
        );

        self.ctx.log_handler().log_output(AgentPluginToggleResult {
            activated: false,
            agent_id,
            plugin: plugin_name.clone(),
            priority: plugin_priority,
        })?;

        Ok(())
    }

    fn resolve_plugin_priority(
        &self,
        component: &ComponentDto,
        agent_type: &AgentTypeName,
        plugin_name: &str,
        explicit_priority: Option<i32>,
    ) -> anyhow::Result<i32> {
        let component_metadata: ComponentMetadata = component.metadata.clone().into();
        let plugins = component_metadata
            .agent_type_plugins(agent_type)
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
                    agent_type.0.log_color_error_highlight(),
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
                        let priorities: Vec<_> = matching.iter().map(|p| p.priority.0).collect();
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
                    logln("Use `golem component get` to list installed plugins.");
                    logln("");
                    bail!(NonSuccessfulExit);
                }
            }
        }
    }

    pub async fn agent_metadata(
        &self,
        environment_id: EnvironmentId,
        component_name: &ComponentName,
        agent_id: &AgentId,
    ) -> anyhow::Result<Option<AgentMetadata>> {
        let clients = self.ctx.golem_clients().await?;

        Ok(clients
            .worker
            .get_worker_metadata(&environment_id.0, &agent_id.0, None)
            .await
            .map_service_error_not_found_as_opt()?
            .map(|result| AgentMetadata::from(component_name.clone(), result)))
    }

    async fn delete(
        &self,
        environment_id: &EnvironmentId,
        agent_id: &AgentId,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .delete_worker(&environment_id.0, &agent_id.0, None)
            .await
            .map(|_| ())
            .map_service_error()?;

        Ok(())
    }

    pub async fn update_component_agents(
        &self,
        component: &ComponentDto,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<TryUpdateAllWorkersView> {
        let component_name = &component.component_name;
        let historical = self
            .historical_agent_types(component.environment_id)
            .await?;
        let agent_filters = [
            // only consider durable agents
            AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable).to_string(),
            // only consider agents in previous revisions that can be upgraded
            AgentFilter::new_revision(FilterComparator::Less, target_revision).to_string(),
        ];
        let (agents_to_update, _) = self
            .list_component_agents(
                component,
                &component_agent_types(component, &historical),
                Some(component.component_name.0.as_str()),
                Some(&agent_filters),
                None,
                None,
                None,
                true,
            )
            .await?;

        if agents_to_update.is_empty() {
            return Ok(TryUpdateAllWorkersView::default());
        }

        log_action(
            "Updating",
            format!(
                "all agents ({}) for component {} to revision {}",
                agents_to_update.len().to_string().log_color_highlight(),
                component_name.0.blue().bold(),
                target_revision.to_string().log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        let client_update_mode = client_update_mode(update_mode);
        let mut update_results = TryUpdateAllWorkersView::default();
        let mut agents_awaiting_update = Vec::new();
        for agent in &agents_to_update {
            let result = self
                .update_agent(
                    component_name,
                    &agent.environment_id,
                    &agent.agent_id,
                    &agent.display_name(),
                    update_mode,
                    target_revision,
                    false,
                    disable_wakeup,
                )
                .await;

            match result {
                Ok(update_attempt_index) => {
                    agents_awaiting_update.push((agent, update_attempt_index));
                }
                Err(error) => {
                    update_results.errors.push(AgentActionError {
                        component_name: component_name.clone(),
                        agent_id: agent.agent_id,
                        error: error_message_for_output(&error),
                    });
                }
            }
            update_results.agents.push(AgentUpdateMeta {
                component_name: component_name.clone(),
                agent_id: agent.agent_id,
                from_revision: agent.component_revision,
                revision: target_revision,
            });
        }

        if await_update {
            for (agent, update_attempt_index) in agents_awaiting_update {
                if let Err(error) = self
                    .await_update_result(
                        &agent.environment_id,
                        &agent.agent_id,
                        target_revision,
                        client_update_mode,
                        update_attempt_index,
                    )
                    .await
                {
                    update_results.errors.push(AgentActionError {
                        component_name: component_name.clone(),
                        agent_id: agent.agent_id,
                        error: error_message_for_output(&error),
                    });
                }
            }
        }

        Ok(update_results)
    }

    async fn update_agent(
        &self,
        component_name: &ComponentName,
        environment_id: &EnvironmentId,
        agent_id: &AgentId,
        agent_name: &str,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<OplogIndex> {
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
        let client_update_mode = client_update_mode(update_mode);
        let result = clients
            .worker
            .update_worker(
                &environment_id.0,
                &agent_id.0,
                None,
                &UpdateWorkerRequest {
                    mode: client_update_mode,
                    target_revision: target_revision.into(),
                    disable_wakeup: Some(disable_wakeup),
                },
            )
            .await
            .map_service_error();

        match result {
            Ok(response) => {
                log_action("Triggered update", "");
                let update_attempt_index = OplogIndex::from_u64(response.update_attempt_index);

                if await_update {
                    self.await_update_result(
                        environment_id,
                        agent_id,
                        target_revision,
                        client_update_mode,
                        update_attempt_index,
                    )
                    .await?;
                }

                Ok(update_attempt_index)
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
        environment_id: &EnvironmentId,
        agent_id: &AgentId,
        target_revision: ComponentRevision,
        update_mode: golem_common::model::agent::AgentUpdateMode,
        selected_update_attempt_index: OplogIndex,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        loop {
            let metadata = clients
                .worker
                .get_worker_metadata(&environment_id.0, &agent_id.0, None)
                .await?;
            let mut pending = false;
            let mut successes = Vec::new();
            let mut failures = Vec::new();
            for update_record in metadata.updates {
                match update_record {
                    UpdateRecord::PendingUpdate(details)
                        if details.target_revision == target_revision
                            && details.mode == update_mode
                            && is_selected_update_attempt(
                                details.pending_update_index,
                                selected_update_attempt_index,
                            ) =>
                    {
                        pending = true;
                    }
                    UpdateRecord::SuccessfulUpdate(details)
                        if details.target_revision == target_revision
                            && details.mode == update_mode
                            && is_selected_update_attempt(
                                details.pending_update_index,
                                selected_update_attempt_index,
                            ) =>
                    {
                        successes.push(details);
                    }
                    UpdateRecord::FailedUpdate(details)
                        if details.target_revision == target_revision
                            && details.mode == update_mode
                            && is_selected_update_attempt(
                                details.pending_update_index,
                                selected_update_attempt_index,
                            ) =>
                    {
                        failures.push(details);
                    }
                    _ => {}
                }
            }
            let latest_success = successes.into_iter().max_by(|a, b| {
                a.timestamp
                    .partial_cmp(&b.timestamp)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let latest_failure = failures.into_iter().max_by(|a, b| {
                a.timestamp
                    .partial_cmp(&b.timestamp)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            if pending {
                log_action(
                    "Agent update",
                    pending_update_progress(target_revision, selected_update_attempt_index),
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            } else {
                // An agent can be re-updated to the same revision, so a success and a failure can both
                // exist for it — report whichever happened last.
                let report_failure = match (&latest_success, &latest_failure) {
                    (Some(success), Some(failure)) => failure.timestamp > success.timestamp,
                    (None, Some(_)) => true,
                    _ => false,
                };
                if report_failure {
                    let failure = latest_failure.expect("report_failure implies a failure exists");
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

    /// Redeploys all agents of a component. Like updating, this is best-effort: a failure on one
    /// agent does not stop the others, it is reported in the result instead. Successful agents
    /// are returned with the revision they were running at before (their "from" revision).
    pub async fn redeploy_component_agents(
        &self,
        component: &ComponentDto,
    ) -> anyhow::Result<BulkAgentActionResult<(AgentId, AgentId, ComponentRevision)>> {
        let component_name = &component.component_name;
        let historical = self
            .historical_agent_types(component.environment_id)
            .await?;
        let (agents, _) = self
            .list_component_agents(
                component,
                &component_agent_types(component, &historical),
                Some(component.component_name.0.as_str()),
                None,
                None,
                None,
                None,
                false,
            )
            .await?;

        if agents.is_empty() {
            log_warn_action(
                "Skipping",
                format!("redeploying agents for component {component_name}, no agent found"),
            );
            return Ok(BulkAgentActionResult::default());
        }

        log_action(
            "Redeploying",
            format!(
                "all agents ({}) for component {}",
                agents.len().to_string().log_color_highlight(),
                component_name.0.blue().bold(),
            ),
        );
        let _indent = LogIndent::new();

        if !self
            .ctx
            .interactive_handler()
            .confirm_redeploy_agents(agents.len())?
        {
            bail!(NonSuccessfulExit);
        }

        let component_metadata: ComponentMetadata = component.metadata.clone().into();
        let mut result = BulkAgentActionResult::with_capacity(agents.len());
        for agent in agents {
            let agent_id = agent.agent_id;
            let agent_name = agent.display_name();
            let from_revision = agent.component_revision;
            match self
                .redeploy_agent(component_name, &component_metadata, agent)
                .await
            {
                Ok(recreated) => result.succeeded.push((agent_id, recreated, from_revision)),
                Err(error) => {
                    log_error_action(
                        "Failed",
                        format!(
                            "redeploying agent {}/{}: {error}",
                            component_name.0.bold().blue(),
                            agent_name.bold().green(),
                        ),
                    );
                    result.errors.push(AgentActionError {
                        component_name: component_name.clone(),
                        agent_id,
                        error: message_for_output(&error),
                    });
                }
            }
        }

        Ok(result)
    }

    /// Deletes all agents of a component. Best-effort: a failure on one agent does not stop the
    /// others, it is reported in the result instead.
    pub async fn delete_component_agents(
        &self,
        component: &ComponentDto,
        show_skip: bool,
    ) -> anyhow::Result<BulkAgentActionResult<AgentId>> {
        let component_name = &component.component_name;
        let historical = self
            .historical_agent_types(component.environment_id)
            .await?;
        let (agents, _) = self
            .list_component_agents(
                component,
                &component_agent_types(component, &historical),
                Some(component.component_name.0.as_str()),
                None,
                None,
                None,
                None,
                false,
            )
            .await?;

        if agents.is_empty() {
            if show_skip {
                log_warn_action(
                    "Skipping",
                    format!("deleting agents for component {component_name}, no agent found"),
                );
            }
            return Ok(BulkAgentActionResult::default());
        }

        log_action(
            "Deleting",
            format!(
                "all agents ({}) for component {}",
                agents.len().to_string().log_color_highlight(),
                component_name.0.blue().bold(),
            ),
        );
        let _indent = LogIndent::new();

        if !self
            .ctx
            .interactive_handler()
            .confirm_deleting_agents(agents.len())?
        {
            bail!(NonSuccessfulExit);
        }

        let mut result = BulkAgentActionResult::with_capacity(agents.len());
        for agent in &agents {
            match self.delete_agent(component_name, agent).await {
                Ok(()) => result.succeeded.push(agent.agent_id),
                Err(error) => {
                    log_error_action(
                        "Failed",
                        format!(
                            "deleting agent {}/{}: {error:#}",
                            component_name.0.bold().blue(),
                            agent.display_name().bold().green(),
                        ),
                    );
                    result.errors.push(AgentActionError {
                        component_name: component_name.clone(),
                        agent_id: agent.agent_id,
                        error: error_message_for_output(&error),
                    });
                }
            }
        }

        Ok(result)
    }

    /// Redeploys an agent by deleting it and creating it again from its constructor parameters,
    /// env and config. A keyed agent is recreated with its key, which gives it the same agent ID.
    /// An agent without a key is recreated without a key, so it gets a new agent ID, which this
    /// returns. The error says which step failed, as a failed recreation leaves the agent
    /// deleted.
    async fn redeploy_agent(
        &self,
        component_name: &ComponentName,
        component_metadata: &ComponentMetadata,
        agent_metadata: AgentMetadata,
    ) -> Result<AgentId, RedeployAgentError> {
        let agent_name = agent_metadata.display_name();
        let (Some(agent_type), Some(constructor_parameters)) = (
            agent_metadata.agent_type_name.clone(),
            agent_metadata.constructor_parameters.as_deref(),
        ) else {
            return Err(RedeployAgentError::NoConstructorParameters);
        };
        // A durable agent of a type that the current revision made ephemeral could not be
        // created again, so it is not deleted.
        if component_metadata
            .agent_types()
            .iter()
            .find(|declared| declared.type_name == agent_type)
            .is_some_and(|declared| declared.mode == AgentMode::Ephemeral)
        {
            return Err(RedeployAgentError::InvalidParameters(anyhow!(
                "agent type {agent_type} is ephemeral in the current component revision"
            )));
        }
        let keyed = agent_metadata.canonical_key.is_some();
        let parameters = redeploy_parameters(
            &agent_type,
            constructor_parameters,
            keyed,
            component_metadata,
        )
        .map_err(RedeployAgentError::InvalidParameters)?;

        log_warn_action(
            "Redeploying",
            format!(
                "agent {}/{} to current version",
                component_name.0.bold().blue(),
                agent_name.bold().green(),
            ),
        );
        let _indent = LogIndent::new();

        self.delete_agent(component_name, &agent_metadata)
            .await
            .map_err(RedeployAgentError::Delete)?;

        log_action(
            "Recreating",
            format!(
                "agent {}/{}",
                component_name.0.bold().blue(),
                agent_name.bold().green(),
            ),
        );
        let recreated = self
            .recreate_agent(
                &agent_metadata.environment_id,
                &agent_metadata.agent_id,
                keyed,
                &agent_type,
                parameters,
                agent_metadata.env,
                agent_metadata.config,
            )
            .await
            .map_err(RedeployAgentError::Recreate)?;
        if keyed {
            log_action("Recreated", "agent");
        } else {
            log_action(
                "Recreated",
                format!(
                    "agent as {}, a new agent ID because the agent has no key",
                    agent_id_display(Some(&agent_type), &recreated)
                        .bold()
                        .green()
                ),
            );
        }

        Ok(recreated)
    }

    /// Creates the agent of `parameters` again. A keyed agent must get the ID of the deleted
    /// agent; an agent without a key gets a new ID.
    #[allow(clippy::too_many_arguments)]
    async fn recreate_agent(
        &self,
        environment_id: &EnvironmentId,
        deleted_agent_id: &AgentId,
        keyed: bool,
        agent_type: &AgentTypeName,
        parameters: ExternalSchemaValue,
        env: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
    ) -> anyhow::Result<AgentId> {
        let clients = self.ctx.golem_clients().await?;
        let reference = clients
            .agent
            .create_agent(
                &environment_id.0,
                &agent_type.0,
                None,
                &golem_client::model::CreateAgentRequest {
                    parameters,
                    config: Some(config),
                    env: Some(env),
                    keyed: Some(keyed),
                },
            )
            .await
            .map_service_error()?;
        if keyed && reference.agent_id != deleted_agent_id.0 {
            bail!(
                "the agent was recreated at ID {}, not at the deleted agent's ID {}",
                reference.agent_id,
                deleted_agent_id.0
            );
        }
        Ok(AgentId(reference.agent_id))
    }

    pub async fn delete_agent(
        &self,
        component_name: &ComponentName,
        agent_metadata: &AgentMetadata,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Deleting",
            format!(
                "agent {}/{}",
                component_name.0.bold().blue(),
                agent_metadata.display_name().bold().green(),
            ),
        );
        self.delete(&agent_metadata.environment_id, &agent_metadata.agent_id)
            .await?;
        log_action("Deleted", "agent");
        Ok(())
    }

    /// Lists the agents of the given agent types of `component`. Listing is per agent type, so
    /// the result is the union over the types; a type that the deployment does not have gives no
    /// agents. With a start cursor, each type resumes at its cursor, and a type that the cursor
    /// does not name was fully listed, so it is skipped. The returned cursors are the ones of the
    /// types that still have more results.
    pub async fn list_component_agents(
        &self,
        component: &ComponentDto,
        agent_types: &[AgentTypeName],
        component_name_filter: Option<&str>,
        filters: Option<&[String]>,
        mode_overlay: Option<&AgentFilter>,
        start_cursor: Option<&AgentListCursor>,
        max_count: Option<u64>,
        precise: bool,
    ) -> anyhow::Result<(Vec<AgentMetadata>, Vec<(AgentTypeName, ScanCursor)>)> {
        let clients = self.ctx.golem_clients().await?;
        let mut agents = Vec::<AgentMetadata>::new();
        let mut final_result_cursors = Vec::<(AgentTypeName, ScanCursor)>::new();

        // The structured `find_agents_metadata` POST endpoint is used for all
        // listing: string filters are converted to a structured `AgentFilter`
        // and combined with the optional mode overlay (`--mode all`), which the
        // string-filter GET endpoint cannot express as it only supports `And`
        // composition.
        let string_filter = match filters {
            Some(filters) if !filters.is_empty() => Some(
                AgentFilter::from(filters.to_vec())
                    .map_err(|e| anyhow::anyhow!("Invalid agent filter: {e}"))?,
            ),
            _ => None,
        };
        let filter = match (string_filter, mode_overlay) {
            (Some(f), Some(mode_overlay)) => Some(f.and(mode_overlay.clone())),
            (Some(f), None) => Some(f),
            (None, Some(mode_overlay)) => Some(mode_overlay.clone()),
            (None, None) => None,
        };

        for agent_type in agent_types {
            let mut current_scan_cursor = match start_cursor {
                None => None,
                Some(start_cursor) => match start_cursor.resume_at(component.id, agent_type) {
                    Some(resume_at) => Some(resume_at),
                    None => continue,
                },
            };
            loop {
                let Some(results) = clients
                    .agent
                    .find_agents_metadata(
                        &component.environment_id.0,
                        &agent_type.0,
                        component_name_filter,
                        &WorkersMetadataRequest {
                            filter: filter.clone(),
                            cursor: current_scan_cursor.clone(),
                            count: max_count.or(Some(self.ctx.http_batch_size())),
                            precise: Some(precise),
                        },
                    )
                    .await
                    .map_service_error_not_found_as_opt()?
                else {
                    break;
                };

                agents.extend(
                    results
                        .workers
                        .into_iter()
                        .map(|meta| AgentMetadata::from(component.component_name.clone(), meta)),
                );

                match results.cursor {
                    Some(next_cursor) => {
                        if max_count.is_none() {
                            current_scan_cursor = Some(next_cursor);
                        } else {
                            final_result_cursors.push((agent_type.clone(), next_cursor));
                            break;
                        }
                    }
                    None => break,
                }
            }
        }

        Ok((agents, final_result_cursors))
    }

    /// The addressed agent's ID and its component, at the agent's own component revision.
    pub(crate) async fn component_by_agent_id_match(
        &self,
        agent_id_match: &AgentIdMatch,
    ) -> anyhow::Result<(ComponentDto, AgentId)> {
        let agent_id = self.agent_id_of(agent_id_match).await?;
        let (component, _) = self
            .recorded_agent_component(agent_id_match.environment.environment_id, agent_id)
            .await?;
        Ok((component, agent_id))
    }
}

/// A component and the agent types of it that a listing covers.
struct AgentListTarget {
    component: ComponentDto,
    agent_types: Vec<AgentTypeName>,
    component_name_filter: Option<ComponentName>,
    historical: Arc<Vec<DeployedRegisteredAgentType>>,
}

/// Current and historical types of this component, not its types' current implementers.
fn component_agent_types(
    component: &ComponentDto,
    historical: &[DeployedRegisteredAgentType],
) -> Vec<AgentTypeName> {
    component
        .metadata
        .agent_types
        .iter()
        .map(|agent_type| agent_type.declaration.type_name.clone())
        .chain(
            historical
                .iter()
                .filter(|agent_type| agent_type.implemented_by.component_id == component.id)
                .map(|agent_type| agent_type.agent_type.type_name.clone()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Reads an ID address: a bare agent UUID, or `Type[<uuid>]`. Any other text is a key.
fn parse_agent_id_address(text: &str) -> Option<(Option<String>, AgentId)> {
    if let Ok(uuid) = Uuid::parse_str(text) {
        return Some((None, AgentId(uuid)));
    }
    let (agent_type, rest) = text.split_once('[')?;
    if agent_type.is_empty() || agent_type.contains('(') {
        return None;
    }
    let uuid = Uuid::parse_str(rest.strip_suffix(']')?).ok()?;
    Some((Some(agent_type.to_string()), AgentId(uuid)))
}

/// Reads a key address, `Type(parameters)`, against the agent types of `component`. The
/// parameters may use the source-language syntax of the agent type.
pub(crate) fn parse_agent_key(text: &str, component: &ComponentDto) -> Option<ParsedAgentKey> {
    let paren_pos = text.find('(')?;
    let close_pos = text.rfind(')')?;
    if close_pos + 1 != text.len() || close_pos < paren_pos {
        return None;
    }
    let type_name = &text[..paren_pos];
    let params_str = &text[paren_pos + 1..close_pos];

    let agent_type = component
        .metadata
        .agent_types
        .iter()
        .map(|agent_type| &agent_type.declaration)
        .find(|at| at.type_name.0 == type_name)?;
    let source_language = SourceLanguage::from(agent_type.source_language.as_str());

    let value = crate::agent_id_display::parse_agent_id_params(
        params_str,
        &agent_type.schema,
        &agent_type.constructor.input_schema,
        &source_language,
    )
    .ok()?;
    Some(ParsedAgentKey::new(
        agent_type.type_name.clone(),
        typed_constructor_parameters(agent_type, value),
    ))
}

/// The constructor parameters of a key in the REST wire form.
fn key_parameters(key: &ParsedAgentKey) -> anyhow::Result<ExternalSchemaValue> {
    ExternalSchemaValue::try_from(key.parameters.value().clone()).map_err(anyhow::Error::msg)
}

fn redeploy_parameters(
    agent_type: &AgentTypeName,
    constructor_parameters: &str,
    keyed: bool,
    component_metadata: &ComponentMetadata,
) -> anyhow::Result<ExternalSchemaValue> {
    let parsed =
        ParsedAgentKey::parse_parameters(agent_type, constructor_parameters, component_metadata)
            .map_err(|error| {
                anyhow!(
                    "the constructor parameters do not match the constructor of agent type {agent_type}: {error}"
                )
            })?;
    if keyed && parsed.canonical_parameters().map_err(anyhow::Error::msg)? != constructor_parameters
    {
        anyhow::bail!(
            "the current constructor changes the canonical parameters of agent type {agent_type}; redeploy would change its key"
        );
    }
    key_parameters(&parsed)
}

impl AgentCommandHandler {
    fn with_previous_invocation_failed_hint(
        &self,
        error: anyhow::Error,
        agent_id_match: &AgentIdMatch,
    ) -> anyhow::Error {
        let previous_invocation_failed = error
            .downcast_ref::<ServiceError>()
            .is_some_and(|service_error| service_error.is_previous_invocation_failed())
            || error
                .downcast_ref::<invocation_session::InvocationFailure>()
                .is_some_and(|failure| failure.code == PublicErrorCode::PreviousInvocationFailed);
        if !previous_invocation_failed {
            return error;
        }

        let command = render_revert_command(
            &crate::command_name(),
            self.ctx
                .explicit_profile_name()
                .map(|profile_name| profile_name.0.as_str()),
            self.ctx.global_environment_selector(),
            agent_id_match.environment_reference(),
            &revert_address(agent_id_match),
        );

        // The original error stays the source, so that its code and details reach structured
        // error reports; `{:#}` prints the hint and then the original error.
        error.context(format!(
            "The previous invocation failed and the agent cannot accept new invocations.\n\nTo discard the failed invocation and restore the preceding agent state, run:\n\n  {command}\n\nThis permanently removes the failed invocation and any later recorded agent state.\nExternal side effects already performed by that invocation may not be undone.\n\nThe error of the server"
        ))
    }

    async fn interrupt_agent(
        &self,
        agent_id_match: &AgentIdMatch,
        agent_id: &AgentId,
        recover_immediately: bool,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .interrupt_worker(
                &agent_id_match.environment.environment_id.0,
                &agent_id.0,
                Some(recover_immediately),
                None,
            )
            .await
            .map(|_| ())
            .map_service_error()?;

        Ok(())
    }

    async fn match_agent_id_in_environment(
        &self,
        environment: ResolvedEnvironmentIdentity,
        agent_id: String,
    ) -> anyhow::Result<AgentIdMatch> {
        if let Some((type_name, id)) = parse_agent_id_address(&agent_id) {
            let (component, agent_type) = self
                .recorded_agent_component(environment.environment_id, id)
                .await?;
            if type_name.is_some_and(|name| name != agent_type.agent_type.type_name.0) {
                bail!("Agent ID {id} does not belong to the specified agent type");
            }
            return Ok(AgentIdMatch {
                environment,
                component_name_match_kind: ComponentNameMatchKind::Unknown,
                component_name: component.component_name,
                agent_type_name: agent_type.agent_type.type_name.clone(),
                source_language: SourceLanguage::from(
                    agent_type.agent_type.source_language.as_str(),
                ),
                agent_type,
                address: AgentAddress::Id(id),
            });
        }
        let agent_type_name = match ParsedAgentKey::parse_agent_type_name(&agent_id) {
            Ok(agent_type_name) => agent_type_name,
            Err(err) => {
                logln("");
                log_error(format!(
                    "Failed to parse agent name ({}) as agent id: {err}",
                    agent_id.log_color_error_highlight()
                ));
                logln("");
                log_text_view(&AgentNameHelp);
                self.log_available_deployed_agent_constructors(&environment)
                    .await;
                bail!(NonSuccessfulExit);
            }
        };
        let agent_type = match self
            .ctx
            .app_handler()
            .get_agent_type_by_name(&environment, &agent_type_name.0)
            .await?
        {
            Some(agent_type) => agent_type,
            None => self
                .historical_agent_types(environment.environment_id)
                .await?
                .into_iter()
                .find(|agent_type| agent_type.agent_type.type_name == agent_type_name)
                .ok_or_else(|| anyhow!("Agent type {agent_type_name} not found"))?,
        };

        let component = self
            .ctx
            .component_handler()
            .get_component_revision_by_id(
                &agent_type.implemented_by.component_id,
                agent_type.implemented_by.component_revision,
            )
            .await?;

        let address = match parse_agent_key(&agent_id, &component) {
            Some(key) => AgentAddress::Key(Box::new(key)),
            None => {
                logln("");
                log_error(format!(
                    "Failed to parse the constructor parameters of agent {}",
                    agent_id.log_color_error_highlight()
                ));
                logln("");
                log_text_view(&AvailableAgentConstructorsHelp::for_component(
                    &component,
                    Some(&agent_type.agent_type.type_name),
                ));
                bail!(NonSuccessfulExit);
            }
        };

        Ok(AgentIdMatch {
            environment,
            component_name_match_kind: ComponentNameMatchKind::Unknown,
            component_name: component.component_name,
            agent_type_name: agent_type.agent_type.type_name.clone(),
            source_language: SourceLanguage::from(agent_type.agent_type.source_language.as_str()),
            agent_type,
            address,
        })
    }

    /// The ID of the addressed agent. A key is looked up, so a key that no current agent has
    /// fails as not found; the CLI never derives an agent ID itself.
    pub async fn agent_id_of(&self, agent_id_match: &AgentIdMatch) -> anyhow::Result<AgentId> {
        match &agent_id_match.address {
            AgentAddress::Id(agent_id) => Ok(*agent_id),
            AgentAddress::Key(key) => {
                let clients = self.ctx.golem_clients().await?;
                let reference = clients
                    .agent
                    .lookup_agent(
                        &agent_id_match.environment.environment_id.0,
                        &agent_id_match.agent_type_name.0,
                        &AgentKeyRequest {
                            parameters: key_parameters(key)?,
                        },
                    )
                    .await
                    .map_service_error()?;
                Ok(AgentId(reference.agent_id))
            }
        }
    }

    pub async fn match_agent_id(&self, agent_id: RawAgentId) -> anyhow::Result<AgentIdMatch> {
        let (environment, address) = self.resolve_agent_address(agent_id).await?;
        self.match_agent_id_in_environment(environment, address)
            .await
    }

    async fn resolve_agent_address(
        &self,
        agent_id: RawAgentId,
    ) -> anyhow::Result<(ResolvedEnvironmentIdentity, String)> {
        let segments = split_agent_id(&agent_id.0);
        match segments.len() {
            // <AGENT>
            1 => {
                let agent_id = segments[0].to_string();

                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;

                Ok((environment, agent_id))
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
                        log_text_view(&AgentNameHelp);
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
                            log_text_view(&AgentNameHelp);
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

                fn validated_agent(value: &str) -> anyhow::Result<String> {
                    Ok(non_empty("agent", value)?.to_string())
                }

                let (environment_reference, agent_id): (Option<EnvironmentReference>, String) =
                    match segments.len() {
                        2 => (
                            Some(EnvironmentReference::Environment {
                                environment_name: validated_environment(segments[0])?,
                            }),
                            validated_agent(segments[1])?,
                        ),
                        3 => (
                            Some(EnvironmentReference::ApplicationEnvironment {
                                application_name: validated_application(segments[0])?,
                                environment_name: validated_environment(segments[1])?,
                            }),
                            validated_agent(segments[2])?,
                        ),
                        4 => (
                            Some(EnvironmentReference::AccountApplicationEnvironment {
                                account_email: validated_account(segments[0])?,
                                application_name: validated_application(segments[1])?,
                                environment_name: validated_environment(segments[2])?,
                            }),
                            validated_agent(segments[3])?,
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

                Ok((environment, agent_id))
            }
            _ => {
                logln("");
                log_error(format!(
                    "Failed to parse agent name: {}",
                    agent_id.0.log_color_error_highlight()
                ));
                logln("");
                log_text_view(&AgentNameHelp);
                bail!(NonSuccessfulExit);
            }
        }
    }

    async fn log_available_deployed_agent_constructors(
        &self,
        environment: &ResolvedEnvironmentIdentity,
    ) {
        if let Ok(agent_types) = self.ctx.app_handler().list_agent_types(environment).await {
            log_text_view(&AvailableAgentConstructorsHelp::for_deployed_agent_types(
                &agent_types,
            ));
            logln("");
        }
    }

    fn confirm_file_overwrite(&self, output_path: &str) -> anyhow::Result<bool> {
        let result = Confirm::new(&format!(
            "File {} already exists. Do you want to overwrite it?",
            output_path.log_color_highlight()
        ))
        .with_default(false)
        .ask()?;

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

/// Fuzzy-matches a method name pattern against the original method names from agent metadata,
/// returning the matched method name on success.
fn resolve_agent_method_name(
    provided_method_name: &str,
    agent_type: &AgentTypeSchema,
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

fn parse_method_parameters_with_error_table(
    agent_type: &AgentTypeSchema,
    method_name: &str,
    arguments: Vec<AgentFunctionArgument>,
    source_language: &SourceLanguage,
) -> anyhow::Result<SchemaValue> {
    let method = agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| anyhow!("Method '{}' not found in agent type", method_name))?;

    let InputSchema::Parameters(fields) = &method.input_schema;
    let element_schemas = fields.iter().collect::<Vec<_>>();

    if element_schemas.len() != arguments.len() {
        logln("");
        log_error(format!(
            "Wrong number of parameters: expected {}, got {}",
            element_schemas.len(),
            arguments.len()
        ));
        logln("");

        let rows = element_schemas
            .iter()
            .zip_longest(arguments.iter())
            .enumerate()
            .map(|(idx, pair)| match pair {
                EitherOrBoth::Both(schema, value) => ArgumentError {
                    argument_index: idx + 1,
                    parameter_type: Some((agent_type.schema.clone(), schema.schema.clone())),
                    value: Some(value.clone()),
                    error: parse_method_argument_schema_value(
                        value,
                        &agent_type.schema,
                        &schema.schema,
                        source_language,
                    )
                    .err()
                    .map(|err| err.message),
                    source_language: source_language.clone(),
                },
                EitherOrBoth::Left(schema) => ArgumentError {
                    argument_index: idx + 1,
                    parameter_type: Some((agent_type.schema.clone(), schema.schema.clone())),
                    value: None,
                    error: Some("missing argument".to_string()),
                    source_language: source_language.clone(),
                },
                EitherOrBoth::Right(value) => ArgumentError {
                    argument_index: idx + 1,
                    parameter_type: None,
                    value: Some(value.clone()),
                    error: Some("extra argument".to_string()),
                    source_language: source_language.clone(),
                },
            })
            .collect::<Vec<_>>();

        log_text_view(&ParameterErrorTableView(rows));
        logln("");
        bail!(NonSuccessfulExit);
    }

    let mut values = Vec::with_capacity(element_schemas.len());
    let mut rows = Vec::with_capacity(element_schemas.len());
    let mut has_error = false;

    for (idx, (schema, value)) in element_schemas.iter().zip(arguments.iter()).enumerate() {
        match parse_method_argument_schema_value(
            value,
            &agent_type.schema,
            &schema.schema,
            source_language,
        ) {
            Ok(parsed) => {
                values.push(parsed);
                rows.push(ArgumentError {
                    argument_index: idx + 1,
                    parameter_type: Some((agent_type.schema.clone(), schema.schema.clone())),
                    value: Some(value.clone()),
                    error: None,
                    source_language: source_language.clone(),
                });
            }
            Err(err) => {
                has_error = true;
                rows.push(ArgumentError {
                    argument_index: idx + 1,
                    parameter_type: Some((agent_type.schema.clone(), schema.schema.clone())),
                    value: Some(value.clone()),
                    error: Some(err.message),
                    source_language: source_language.clone(),
                });
            }
        }
    }

    if has_error {
        logln("");
        log_error("Argument parse error(s)!");
        logln("");
        log_text_view(&ParameterErrorTableView(rows));
        logln("");
        bail!(NonSuccessfulExit);
    }

    Ok(SchemaValue::Record { fields: values })
}

pub(super) fn parse_method_argument_schema_value(
    value: &str,
    graph: &SchemaGraph,
    schema: &SchemaType,
    source_language: &SourceLanguage,
) -> Result<SchemaValue, crate::agent_id_display::ParseError> {
    let parsed =
        crate::agent_id_display::parse_value_for_language(value, graph, schema, source_language);
    if parsed.is_ok() {
        return parsed;
    }

    if matches!(schema, SchemaType::String { .. }) {
        let quoted =
            serde_json::to_string(value).map_err(|err| crate::agent_id_display::ParseError {
                position: 0,
                message: format!("failed to quote string value: {err}"),
            })?;

        return crate::agent_id_display::parse_value_for_language(
            &quoted,
            graph,
            schema,
            source_language,
        );
    }

    parsed
}

/// One page of `count` items across the agent types in `component_names`, in their order,
/// under one budget. `scan(position, scan_cursor, limit)` lists the items of the type at
/// `position` from `scan_cursor`, and gives the scan cursor of its next items. It asks for `limit`
/// items, but a scan of a Redis indexed storage can list more, and the page keeps all of them, so
/// `count` is a target there and a limit elsewhere.
/// The page ends at the first scan that gives a scan cursor, or when `count` items are listed.
/// The next cursor includes the unfinished type and every type not yet scanned.
async fn read_page<T>(
    scans: &[(ComponentId, &str)],
    cursor: Option<&AgentListCursor>,
    count: u64,
    mut scan: impl AsyncFnMut(
        usize,
        Option<ScanCursor>,
        u64,
    ) -> anyhow::Result<(Vec<T>, Option<ScanCursor>)>,
) -> anyhow::Result<(Vec<T>, Option<String>)> {
    if count == 0 {
        bail!("A page needs a --max-count of at least 1");
    }
    if let Some(cursor) = cursor {
        for (component, agent_type) in cursor.scans() {
            if !scans.contains(&(component, agent_type)) {
                bail!(
                    "The page cursor is for component {component} and agent type {agent_type}, which is not selected"
                );
            }
        }
    }
    let mut items = Vec::new();
    let mut remaining = count;
    let mut next = AgentListCursor::default();
    let mut stopped = false;
    for (position, (component, name)) in scans.iter().enumerate() {
        let agent_type = AgentTypeName((*name).to_string());
        let scan_cursor = match cursor {
            None => None,
            Some(cursor) => match cursor.resume_at(*component, &agent_type) {
                Some(cursor) => cursor.into_option(),
                None => continue,
            },
        };
        if stopped || remaining == 0 {
            next.insert(
                *component,
                &agent_type,
                scan_cursor.unwrap_or_else(|| ScanCursor::new(String::new())),
            );
            continue;
        }
        let (listed, next_scan_cursor) = scan(position, scan_cursor, remaining).await?;
        remaining = remaining.saturating_sub(listed.len() as u64);
        items.extend(listed);
        if let Some(next_scan_cursor) = next_scan_cursor {
            next.insert(*component, &agent_type, next_scan_cursor);
            stopped = true;
        }
    }
    Ok((items, (!next.is_empty()).then(|| next.encode())))
}

fn secret_config_paths_for_agent_type(
    agent_types: &[AgentTypeSchema],
    agent_type_name: &AgentTypeName,
) -> BTreeSet<String> {
    agent_types
        .iter()
        .find(|agent_type| &agent_type.type_name == agent_type_name)
        .map(|agent_type| {
            agent_type
                .config
                .iter()
                .filter(|config| config.source == AgentConfigSource::Secret)
                .map(|config| config.path.join("."))
                .collect()
        })
        .unwrap_or_default()
}

/// Injects a `mode == ...` filter string at the front of `filters` based on
/// the user-supplied `--mode` flag, unless the user already provided their own
/// `mode ...` filter via `--filter`.
///
/// `AgentListMode::All` never injects a string filter here; instead
/// [`all_modes_filter`] returns a structured `Or` overlay that the listing
/// path sends via the structured `find_workers_metadata` endpoint so the
/// executor scans both durable and ephemeral oplogs. For `Durable` /
/// `Ephemeral`, the corresponding equality filter is prepended so the
/// executor narrows the oplog scan to only the matching mode.
fn apply_list_mode_filter(filters: Vec<String>, mode: AgentListMode) -> Vec<String> {
    if user_set_mode_filter(&filters) {
        return filters;
    }
    let mode_filter = match mode {
        AgentListMode::All => return filters,
        AgentListMode::Durable => "mode == durable",
        AgentListMode::Ephemeral => "mode == ephemeral",
    };
    let mut out = Vec::with_capacity(filters.len() + 1);
    out.push(mode_filter.to_string());
    out.extend(filters);
    out
}

/// Returns `true` if `filters` contains a user-supplied `mode ...` constraint anywhere,
/// also inside `and`, `or`, `not` or parentheses, meaning the caller should not add its own
/// mode overlay. A filter that does not parse is refused later, when the list builds its
/// structured filter.
fn user_set_mode_filter(filters: &[String]) -> bool {
    filters
        .iter()
        .filter_map(|filter| filter.parse::<AgentFilter>().ok())
        .any(|filter| filter.mentions_mode())
}

/// Structured filter matching agents in either durability mode, used for
/// `--mode all` so the executor scans both durable and ephemeral oplogs
/// (its `modes_from_filter` returns `None` for an `Or` containing `Mode`),
/// while the post-scan matcher accepts both modes. Sent via the structured
/// `find_workers_metadata` endpoint because the string-filter GET endpoint
/// only supports `And` composition.
fn all_modes_filter() -> AgentFilter {
    AgentFilter::new_or(vec![
        AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable),
        AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Ephemeral),
    ])
}

/// The agent address that the suggested revert command uses: the canonical key text for a key,
/// or `Type[<uuid>]` for an ID.
fn revert_address(agent_id_match: &AgentIdMatch) -> String {
    match &agent_id_match.address {
        AgentAddress::Key(key) => key.to_string(),
        AgentAddress::Id(agent_id) => {
            agent_id_display(Some(&agent_id_match.agent_type_name), agent_id)
        }
    }
}

fn render_revert_command(
    executable: &str,
    explicit_profile_name: Option<&str>,
    global_environment_selector: Option<&GlobalEnvironmentSelector>,
    matched_environment_reference: Option<&EnvironmentReference>,
    agent_id: &str,
) -> String {
    fn quote(value: &str) -> String {
        shlex::try_quote(value)
            .expect("CLI arguments cannot contain NUL bytes")
            .into_owned()
    }

    let mut args = vec![quote(executable)];
    if let Some(profile_name) = explicit_profile_name {
        args.extend(["--profile".to_string(), quote(profile_name)]);
    }
    match global_environment_selector {
        Some(GlobalEnvironmentSelector::Environment(environment)) => {
            args.extend(["--environment".to_string(), quote(&environment.to_string())]);
        }
        Some(GlobalEnvironmentSelector::Local) => args.push("--local".to_string()),
        Some(GlobalEnvironmentSelector::Cloud) => args.push("--cloud".to_string()),
        None => {}
    }
    args.extend(["agent".to_string(), "revert".to_string()]);

    let agent_id = match global_environment_selector {
        Some(_) => agent_id.to_string(),
        None => match matched_environment_reference {
            Some(environment) => format!("{environment}/{agent_id}"),
            None => agent_id.to_string(),
        },
    };
    args.extend([
        quote(&agent_id),
        "--number-of-invocations".to_string(),
        "1".to_string(),
    ]);

    args.join(" ")
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

fn split_agent_id(agent_id: &str) -> Vec<&str> {
    match agent_id.find('(') {
        Some(constructor_open_parentheses_idx) => {
            let splittable = &agent_id[0..constructor_open_parentheses_idx];
            let last_slash_idx = splittable.rfind('/');
            match last_slash_idx {
                Some(last_slash_idx) => {
                    let mut segments = agent_id[0..last_slash_idx]
                        .split('/')
                        .collect::<Vec<&str>>();
                    segments.push(&agent_id[last_slash_idx + 1..]);
                    segments
                }
                None => {
                    vec![agent_id]
                }
            }
        }
        None => agent_id.split("/").collect(),
    }
}

fn validate_ordinary_agent_type(agent_type: &AgentTypeSchema) -> anyhow::Result<()> {
    if agent_type.kind == AgentTypeKind::HttpRouter {
        bail!(
            "HTTP router '{}' is managed by its HTTP deployment and cannot be created or invoked as an ordinary agent",
            agent_type.type_name
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AgentListCursor, AgentListMode, AgentUpdateMode, ScanCursor, apply_list_mode_filter,
        is_selected_update_attempt, parse_agent_id_address, parse_agent_key,
        parse_method_argument_schema_value, parse_method_parameters_with_error_table,
        pending_update_progress, read_page, redeploy_parameters, render_revert_command,
        split_agent_id, validate_ordinary_agent_type,
    };
    use crate::agent_id_display::SourceLanguage;
    use crate::context::GlobalEnvironmentSelector;
    use crate::model::environment::EnvironmentReference;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{AgentMode, AgentTypeName, ParsedAgentKey, Snapshotting};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::component::{
        ComponentDto, ComponentId, ComponentName, ComponentRevision,
    };
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::{AgentFilter, FilterComparator, OplogIndex, StringFilterComparator};
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField,
        OutputSchema,
    };
    use golem_common::schema::schema_type::{NamedFieldType, VariantCaseType};
    use golem_common::schema::{SchemaGraph, SchemaType, SchemaTypeDef, SchemaValue, TypeId};
    use golem_schema::Empty;
    use itertools::Itertools;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;
    use test_r::test;

    /// Reads every page of `components` (name and item count) with pages of `count`, over a fake
    /// scan whose scan cursor is the index of the next item. Gives each page's items and cursor,
    /// and each scan's component position and scan cursor.
    async fn read_all_pages(
        components: &[(&str, usize)],
        count: u64,
    ) -> (
        Vec<(Vec<String>, Option<String>)>,
        Vec<(usize, Option<String>)>,
    ) {
        let names = components
            .iter()
            .map(|(name, _)| (ComponentId::new(), *name))
            .collect::<Vec<_>>();
        let mut pages = Vec::new();
        let mut scans = Vec::new();
        let mut cursor = None::<AgentListCursor>;
        loop {
            let (items, next) = read_page(
                &names,
                cursor.as_ref(),
                count,
                async |position, scan_cursor: Option<ScanCursor>, limit| {
                    scans.push((position, scan_cursor.as_ref().map(ToString::to_string)));
                    let (name, len) = components[position];
                    let start = scan_cursor.map_or(0, |cursor| cursor.as_str().parse().unwrap());
                    let end = (start + limit as usize).min(len);
                    let items = (start..end).map(|item| format!("{name}#{item}")).collect();
                    let next = (end < len).then(|| ScanCursor::new(end.to_string()));
                    Ok((items, next))
                },
            )
            .await
            .unwrap();
            pages.push((items, next.clone()));
            let Some(next) = next else {
                return (pages, scans);
            };
            cursor = Some(AgentListCursor::decode(&next).unwrap());
            assert!(pages.len() < 100, "the pages do not end");
        }
    }

    #[test]
    async fn a_page_that_ends_a_component_at_its_limit_names_the_next_component() {
        let (pages, scans) = read_all_pages(&[("a:x", 2), ("b:y", 3)], 2).await;
        assert_eq!(
            pages
                .iter()
                .map(|(items, cursor)| (items.clone(), cursor.is_some()))
                .collect::<Vec<_>>(),
            vec![
                (vec!["a:x#0".to_string(), "a:x#1".to_string()], true),
                (vec!["b:y#0".to_string(), "b:y#1".to_string()], true),
                (vec!["b:y#2".to_string()], false),
            ]
        );
        // The cursor for an unscanned type starts it at its beginning; no scan asks for 0 items.
        assert_eq!(
            scans,
            vec![(0, None), (1, None), (1, Some("2".to_string()))]
        );

        // The last component ending at the limit ends the pages.
        let (pages, _) = read_all_pages(&[("a:x", 1), ("b:y", 1)], 2).await;
        assert_eq!(
            pages,
            vec![(vec!["a:x#0".to_string(), "b:y#0".to_string()], None)]
        );
    }

    #[test]
    async fn page_reaches_the_second_component_of_a_moved_type() {
        let names = [(ComponentId::new(), "T"), (ComponentId::new(), "T")];
        let scan = async |position, _: Option<ScanCursor>, _| Ok((vec![position], None));
        let (first, token) = read_page(&names, None, 1, scan).await.unwrap();
        let cursor = AgentListCursor::decode(token.as_deref().unwrap()).unwrap();
        let (second, end) = read_page(&names, Some(&cursor), 1, scan).await.unwrap();
        assert_eq!((first, second, end), (vec![0], vec![1], None));
    }

    #[test]
    fn await_update_attempt_filter_ignores_earlier_same_target_outcomes() {
        let selected = OplogIndex::from_u64(11);
        assert!(!is_selected_update_attempt(
            Some(OplogIndex::from_u64(10)),
            selected,
        ));
        assert!(is_selected_update_attempt(
            Some(OplogIndex::from_u64(11)),
            selected,
        ));
        assert!(!is_selected_update_attempt(
            Some(OplogIndex::from_u64(12)),
            selected,
        ));
        assert!(!is_selected_update_attempt(None, selected));
    }

    #[test]
    fn pending_update_progress_identifies_exact_attempt() {
        let progress =
            pending_update_progress(ComponentRevision::new(3).unwrap(), OplogIndex::from_u64(17));
        assert_eq!(
            strip_ansi_escapes::strip_str(progress),
            "to revision 3 for attempt index 17 is still pending"
        );
    }

    #[test]
    fn update_mode_exposes_only_automatic_and_manual() {
        assert_eq!(
            "auto".parse::<AgentUpdateMode>().unwrap(),
            AgentUpdateMode::Automatic
        );
        assert_eq!(
            "manual".parse::<AgentUpdateMode>().unwrap(),
            AgentUpdateMode::Manual
        );
        assert!(
            "snapshot-assisted-automatic"
                .parse::<AgentUpdateMode>()
                .is_err()
        );
    }

    #[test]
    fn revert_command_preserves_scope_and_shell_sensitive_arguments() {
        let environment = EnvironmentReference::ApplicationEnvironment {
            application_name: ApplicationName("my app".to_string()),
            environment_name: EnvironmentName("staging env".to_string()),
        };
        let command = render_revert_command(
            "/opt/Golem CLI/golem",
            Some("team profile"),
            None,
            Some(&environment),
            "MyAgent(\"a b's\")",
        );

        assert_eq!(
            shlex::split(&command).unwrap(),
            vec![
                "/opt/Golem CLI/golem",
                "--profile",
                "team profile",
                "agent",
                "revert",
                "my app/staging env/MyAgent(\"a b's\")",
                "--number-of-invocations",
                "1",
            ]
        );
        assert!(!command.contains("--yes"));
    }

    #[test]
    fn revert_command_preserves_global_environment_selector() {
        let short_environment = EnvironmentReference::Environment {
            environment_name: EnvironmentName("staging env".to_string()),
        };
        let full_environment = EnvironmentReference::ApplicationEnvironment {
            application_name: ApplicationName("another app".to_string()),
            environment_name: EnvironmentName("production".to_string()),
        };
        let agent_id = "MyAgent(\"id\")";

        for selector in [
            GlobalEnvironmentSelector::Environment(short_environment),
            GlobalEnvironmentSelector::Environment(full_environment),
            GlobalEnvironmentSelector::Local,
            GlobalEnvironmentSelector::Cloud,
        ] {
            let command = render_revert_command("golem", None, Some(&selector), None, agent_id);
            let args = shlex::split(&command).unwrap();

            assert!(!args.contains(&"--profile".to_string()));

            match selector {
                GlobalEnvironmentSelector::Environment(environment) => {
                    assert!(args.windows(2).any(|args| {
                        args == ["--environment".to_string(), environment.to_string()]
                    }));
                }
                GlobalEnvironmentSelector::Local => assert!(args.contains(&"--local".to_string())),
                GlobalEnvironmentSelector::Cloud => assert!(args.contains(&"--cloud".to_string())),
            }
            assert!(args.contains(&agent_id.to_string()));
        }
    }

    #[test]
    fn apply_list_mode_filter_default_durable_with_no_filters_injects_durable() {
        let result = apply_list_mode_filter(vec![], AgentListMode::Durable);
        assert_eq!(result, vec!["mode == durable".to_string()]);
    }

    #[test]
    fn apply_list_mode_filter_default_durable_with_other_filters_prepends_durable() {
        let result =
            apply_list_mode_filter(vec!["status = Running".to_string()], AgentListMode::Durable);
        assert_eq!(
            result,
            vec![
                "mode == durable".to_string(),
                "status = Running".to_string()
            ]
        );
    }

    #[test]
    fn apply_list_mode_filter_ephemeral_injects_ephemeral() {
        let result = apply_list_mode_filter(vec![], AgentListMode::Ephemeral);
        assert_eq!(result, vec!["mode == ephemeral".to_string()]);
    }

    #[test]
    fn apply_list_mode_filter_all_does_not_inject() {
        let input = vec!["status = Running".to_string()];
        let result = apply_list_mode_filter(input.clone(), AgentListMode::All);
        assert_eq!(result, input);
    }

    #[test]
    fn apply_list_mode_filter_does_not_override_user_supplied_mode_filter() {
        let input = vec!["mode == ephemeral".to_string()];
        let result = apply_list_mode_filter(input.clone(), AgentListMode::Durable);
        assert_eq!(result, input);
    }

    #[test]
    fn apply_list_mode_filter_user_mode_filter_case_insensitive() {
        let input = vec!["mode == EPHEMERAL".to_string()];
        let result = apply_list_mode_filter(input.clone(), AgentListMode::Durable);
        assert_eq!(result, input);
    }

    #[test]
    fn apply_list_mode_filter_finds_user_mode_filter_inside_compound_filters() {
        for filter in [
            "(mode == ephemeral)",
            "status == Idle or mode == ephemeral",
            "not mode == durable",
        ] {
            let input = vec!["status == Running".to_string(), filter.to_string()];
            let result = apply_list_mode_filter(input.clone(), AgentListMode::Durable);
            assert_eq!(result, input, "filter: {filter}");
        }
    }

    fn test_agent_type_schema(mode: AgentMode) -> AgentTypeSchema {
        AgentTypeSchema {
            kind: golem_common::schema::agent::AgentTypeKind::Regular,
            type_name: AgentTypeName("repl-agent".to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            methods: vec![AgentMethodSchema {
                name: "run".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            }],
            dependencies: vec![],
            mode,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }

    #[test]
    fn method_arguments_are_every_declared_field() {
        let mut agent_type = test_agent_type_schema(AgentMode::Durable);
        agent_type.methods[0].input_schema = InputSchema::Parameters(vec![
            NamedField::user_supplied("name", SchemaType::string()),
            NamedField::user_supplied("count", SchemaType::u32()),
        ]);
        for language in [
            SourceLanguage::TypeScript,
            SourceLanguage::Rust,
            SourceLanguage::Scala,
            SourceLanguage::MoonBit,
            SourceLanguage::Go,
        ] {
            let arguments = ["\"alice\"", "7"].map(str::to_string).to_vec();
            assert_eq!(
                parse_method_parameters_with_error_table(&agent_type, "run", arguments, &language)
                    .unwrap(),
                SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("alice".to_string()),
                        SchemaValue::U32(7)
                    ],
                },
            );
        }
        // A declared field is never left out: too few arguments are refused.
        assert!(
            parse_method_parameters_with_error_table(
                &agent_type,
                "run",
                vec!["\"alice\"".to_string()],
                &SourceLanguage::TypeScript,
            )
            .is_err()
        );
        agent_type.methods[0].input_schema = InputSchema::Parameters(vec![]);
        assert_eq!(
            parse_method_parameters_with_error_table(
                &agent_type,
                "run",
                vec![],
                &SourceLanguage::TypeScript,
            )
            .unwrap(),
            SchemaValue::Record { fields: vec![] },
        );
    }

    #[test]
    fn ordinary_create_and_invoke_reject_http_routers_by_kind() {
        let regular = test_agent_type_schema(AgentMode::Durable);
        validate_ordinary_agent_type(&regular).unwrap();

        let mut router = regular;
        router.kind = golem_common::schema::agent::AgentTypeKind::HttpRouter;
        router.type_name = AgentTypeName("OrdinaryLookingName".to_string());
        let error = validate_ordinary_agent_type(&router)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("HTTP router 'OrdinaryLookingName'"),
            "{error}"
        );
        assert!(error.contains("cannot be created or invoked"), "{error}");
    }

    #[test]
    fn test_split_agent_id() {
        assert_eq!(split_agent_id("a"), vec!["a"]);
        assert_eq!(split_agent_id("a()"), vec!["a()"]);
        assert_eq!(split_agent_id("a(\"///\")"), vec!["a(\"///\")"]);
        assert_eq!(split_agent_id("a/b"), vec!["a", "b"]);
        assert_eq!(split_agent_id("a/b()"), vec!["a", "b()"]);
        assert_eq!(split_agent_id("a/b(\"///\")"), vec!["a", "b(\"///\")"]);
        assert_eq!(split_agent_id("a/b/c"), vec!["a", "b", "c"]);
        assert_eq!(split_agent_id("a/b/c()"), vec!["a", "b", "c()"]);
        assert_eq!(split_agent_id("a/b/c(\"/\")"), vec!["a", "b", "c(\"/\")"]);
        assert_eq!(split_agent_id("/"), vec!["", ""]);
        assert_eq!(split_agent_id("a(/"), vec!["a(/"]);
    }

    #[test]
    fn id_addresses_are_a_bare_uuid_or_a_type_with_a_bracketed_uuid() {
        let id = golem_common::model::AgentId::from_parts(7, 42);
        assert_eq!(parse_agent_id_address(&id.to_string()), Some((None, id)));
        assert_eq!(
            parse_agent_id_address(&format!("Counter[{id}]")),
            Some((Some("Counter".to_string()), id))
        );
        assert_eq!(
            parse_agent_id_address(&format!("Counter(\"a\")[{id}]")),
            None
        );
        assert_eq!(parse_agent_id_address("Counter(\"[x]\")"), None);
        assert_eq!(parse_agent_id_address("Counter[not-a-uuid]"), None);
        assert_eq!(parse_agent_id_address(&format!("[{id}]")), None);
    }

    #[test]
    fn environment_application_and_account_prefixes_split_off_key_and_id_addresses() {
        let id = golem_common::model::AgentId::from_parts(7, 42);
        let typed_id = format!("Counter[{id}]");
        let bare_id = id.to_string();
        let key = r#"Counter("a/b")"#;
        for (address, agent) in [
            (typed_id.as_str(), Some((Some("Counter".to_string()), id))),
            (bare_id.as_str(), Some((None, id))),
            (key, None),
        ] {
            for prefix in [
                vec![],
                vec!["env"],
                vec!["app", "env"],
                vec!["me@example.com", "app", "env"],
            ] {
                let text = prefix.iter().copied().chain([address]).join("/");
                let segments = split_agent_id(&text);
                let expected_segments = prefix.iter().copied().chain([address]).collect::<Vec<_>>();
                assert_eq!(segments, expected_segments, "{text}");
                assert_eq!(
                    parse_agent_id_address(segments.last().unwrap()),
                    agent,
                    "{text}"
                );
            }
        }
    }

    fn counter_component() -> ComponentDto {
        let mut agent_type = test_agent_type_schema(AgentMode::Durable);
        agent_type.type_name = AgentTypeName("CounterAgent".to_string());
        agent_type.source_language = "rust".to_string();
        agent_type.constructor.input_schema = InputSchema::Parameters(vec![
            NamedField::user_supplied("name", SchemaType::string()),
            NamedField::user_supplied("count", SchemaType::u32()),
        ]);
        ComponentDto {
            id: ComponentId(uuid::Uuid::nil()),
            revision: ComponentRevision::new(0).unwrap(),
            environment_id: EnvironmentId(uuid::Uuid::nil()),
            component_name: ComponentName("counter:main".to_string()),
            hash: Default::default(),
            wasm_hash: Default::default(),
            package_metadata: None,
            application_id: ApplicationId(uuid::Uuid::nil()),
            account_id: AccountId(uuid::Uuid::nil()),
            component_size: 0,
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
            metadata: ComponentMetadata::from_parts(
                Default::default(),
                vec![],
                vec![agent_type],
                BTreeMap::new(),
            )
            .into(),
        }
    }

    #[test]
    fn component_selection_includes_past_types_but_not_another_components_types() {
        let component = counter_component();
        let mut moved = component.metadata.agent_types[0].declaration.clone();
        moved.type_name = AgentTypeName("Moved".into());
        let previous = golem_common::model::agent::DeployedRegisteredAgentType {
            type_id: golem_common::model::type_id::TypeId::try_from(1u32).unwrap(),
            agent_type: moved,
            implemented_by: golem_common::model::agent::RegisteredAgentTypeImplementer {
                component_id: component.id,
                component_revision: component.revision,
                component_name: component.component_name.0.clone(),
                account_id: component.account_id,
                account_email: golem_common::model::account::AccountEmail::new("owner@example.com"),
            },
            webhook_prefix_authority_and_path: None,
        };
        let mut other = previous.clone();
        other.implemented_by.component_id = ComponentId::new();
        other.agent_type.type_name = AgentTypeName("OnlyOnOtherComponent".into());
        assert_eq!(
            super::component_agent_types(&component, &[previous, other]),
            vec![
                AgentTypeName("CounterAgent".into()),
                AgentTypeName("Moved".into())
            ]
        );
    }

    #[test]
    fn key_addresses_name_an_agent_type_of_the_component_with_its_constructor_parameters() {
        let component = counter_component();

        let key = parse_agent_key(r#"CounterAgent("c1", 7)"#, &component).unwrap();
        assert_eq!(key.agent_type, AgentTypeName("CounterAgent".to_string()));
        assert_eq!(
            key.parameters.value(),
            &SchemaValue::Record {
                fields: vec![SchemaValue::String("c1".to_string()), SchemaValue::U32(7)],
            }
        );
        assert_eq!(key.to_string(), r#"CounterAgent("c1",7)"#);
        assert_eq!(parse_agent_key(&key.to_string(), &component), Some(key));

        for not_a_key in [
            r#"OtherAgent("c1", 7)"#,
            r#"CounterAgent("c1")"#,
            r#"CounterAgent("c1", 7)x"#,
            r#"CounterAgent("c1", "not a number")"#,
            "CounterAgent[00000007-0000-8000-8000-00000000002a]",
            "00000007-0000-8000-8000-00000000002a",
        ] {
            assert!(
                parse_agent_key(not_a_key, &component).is_none(),
                "{not_a_key} must not parse as a key"
            );
        }
    }

    #[test]
    fn keyed_redeploy_rejects_constructor_rounding_before_mutation() {
        let name = AgentTypeName("Meter".to_string());
        let component = |field_type| {
            let mut ty = test_agent_type_schema(AgentMode::Durable);
            ty.type_name = name.clone();
            ty.constructor.input_schema =
                InputSchema::Parameters(vec![NamedField::user_supplied("reading", field_type)]);
            ComponentMetadata::from_parts(Default::default(), vec![], vec![ty], BTreeMap::new())
        };
        let old = component(SchemaType::f64());
        let stored = ParsedAgentKey::parse_parameters(&name, "16777217.0", &old)
            .unwrap()
            .canonical_parameters()
            .unwrap();
        let new = component(SchemaType::f32());
        let error = redeploy_parameters(&name, &stored, true, &new).unwrap_err();
        assert!(error.to_string().contains("redeploy would change its key"));
        assert!(redeploy_parameters(&name, &stored, true, &old).is_ok());
        assert!(redeploy_parameters(&name, "16777216.0", true, &new).is_ok());
        assert!(redeploy_parameters(&name, &stored, false, &new).is_ok());
    }

    #[test]
    fn agent_list_filters_compare_the_key_and_the_id() {
        let id = golem_common::model::AgentId::from_parts(7, 42);
        let filters = apply_list_mode_filter(
            vec![
                r#"key == CounterAgent("c1")"#.to_string(),
                format!("id == {id}"),
            ],
            AgentListMode::Durable,
        );
        assert_eq!(
            AgentFilter::from(filters).unwrap(),
            AgentFilter::new_and(vec![
                AgentFilter::new_mode(FilterComparator::Equal, AgentMode::Durable),
                AgentFilter::new_key(
                    StringFilterComparator::Equal,
                    r#"CounterAgent("c1")"#.to_string()
                ),
                AgentFilter::new_id(FilterComparator::Equal, id),
            ])
        );
        assert_eq!(
            AgentFilter::from(vec![r#"key startswith "CounterAgent(""#.to_string()]).unwrap(),
            AgentFilter::new_key(
                StringFilterComparator::StartsWith,
                "CounterAgent(".to_string()
            )
        );

        for invalid in ["id == not-a-uuid", r#"name == CounterAgent("c1")"#] {
            assert!(
                AgentFilter::from(vec![invalid.to_string()]).is_err(),
                "{invalid} must be rejected"
            );
        }
    }

    #[test]
    fn parse_method_argument_schema_value_parses_unstructured_text_inline() {
        let ty = SchemaType::text(Default::default());
        let graph = SchemaGraph::anonymous(ty.clone());
        let parsed = parse_method_argument_schema_value(
            r#"Text("hello")"#,
            &graph,
            &ty,
            &SourceLanguage::Rust,
        )
        .unwrap();

        assert!(matches!(parsed, SchemaValue::Text(_)));
    }

    #[test]
    fn parse_method_argument_schema_value_parses_unstructured_binary_data_url() {
        let ty = SchemaType::binary(Default::default());
        let graph = SchemaGraph::anonymous(ty.clone());
        let parsed = parse_method_argument_schema_value(
            r#"Binary("data:application/octet-stream;base64,SGVsbG8")"#,
            &graph,
            &ty,
            &SourceLanguage::Rust,
        )
        .unwrap();

        assert!(matches!(parsed, SchemaValue::Binary(_)));
    }

    // The shapes the SDKs produce for unstructured and multimodal parameters, as seen in the
    // extracted agent types of the code-first test apps. The literals below are the ones used
    // by the `*_with_rpc_and_all_types` integration tests, pinned here so a parser regression
    // shows up without a server round-trip.

    fn variant_case(name: &str, payload: SchemaType) -> VariantCaseType {
        VariantCaseType {
            name: name.to_string(),
            payload: Some(payload),
            metadata: Default::default(),
        }
    }

    fn unstructured_text_type() -> SchemaType {
        SchemaType::variant(vec![
            variant_case("inline", SchemaType::text(Default::default())),
            variant_case("url", SchemaType::url(Default::default())),
        ])
    }

    fn unstructured_binary_type() -> SchemaType {
        SchemaType::variant(vec![
            variant_case("inline", SchemaType::binary(Default::default())),
            variant_case("url", SchemaType::url(Default::default())),
        ])
    }

    fn multimodal_type() -> SchemaType {
        SchemaType::list(SchemaType::variant(vec![
            variant_case("Text", unstructured_text_type()),
            variant_case("Binary", unstructured_binary_type()),
        ]))
    }

    /// `MultimodalAdvanced<TextImageData>` with `Data` being a user-defined (named) record, as
    /// the Rust SDK extracts it.
    fn multimodal_advanced_graph() -> SchemaGraph {
        let data_id = TypeId("rust_code_first_rust_main.model.Data".to_string());
        SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: data_id.clone(),
                name: Some("Data".to_string()),
                body: SchemaType::record(vec![
                    NamedFieldType {
                        name: "id".to_string(),
                        body: SchemaType::u32(),
                        metadata: Default::default(),
                    },
                    NamedFieldType {
                        name: "name".to_string(),
                        body: SchemaType::string(),
                        metadata: Default::default(),
                    },
                ]),
            }],
            root: SchemaType::list(SchemaType::variant(vec![
                variant_case("Text", SchemaType::string()),
                variant_case("Image", SchemaType::list(SchemaType::u8())),
                variant_case("Data", SchemaType::ref_to(data_id)),
            ])),
        }
    }

    fn assert_parses(language: SourceLanguage, ty: SchemaType, input: &str) {
        assert_parses_in_graph(language, SchemaGraph::anonymous(ty), input);
    }

    fn assert_parses_in_graph(language: SourceLanguage, graph: SchemaGraph, input: &str) {
        parse_method_argument_schema_value(input, &graph, &graph.root, &language)
            .unwrap_or_else(|err| panic!("failed to parse {input:?} as {language:?}: {err}"));
    }

    #[test]
    fn parse_method_argument_schema_value_parses_rust_unstructured_and_multimodal_literals() {
        for input in [
            r#"Url(Url("https://example.com/foo"))"#,
            r#"Inline(Text("foo"))"#,
            r#"Inline(Text("foo", "en"))"#,
        ] {
            assert_parses(SourceLanguage::Rust, unstructured_text_type(), input);
        }
        for input in [
            r#"Url(Url("https://example.com/foo"))"#,
            r#"Inline(Binary("data:text/plain;base64,Zm9v"))"#,
        ] {
            assert_parses(SourceLanguage::Rust, unstructured_binary_type(), input);
        }
        assert_parses(
            SourceLanguage::Rust,
            multimodal_type(),
            r#"[Text(Url(Url("https://example.com/foo"))), Binary(Inline(Binary("data:text/plain;base64,Zm9v")))]"#,
        );
        assert_parses_in_graph(
            SourceLanguage::Rust,
            multimodal_advanced_graph(),
            r#"[Text("foo"), Image([1, 2, 3]), Data(Data { id: 1, name: "foo" })]"#,
        );
    }

    #[test]
    fn parse_method_argument_schema_value_parses_ts_unstructured_and_multimodal_literals() {
        for input in [
            r#"{tag: "url", value: Url("https://example.com/foo")}"#,
            r#"{tag: "inline", value: Text("foo")}"#,
        ] {
            assert_parses(SourceLanguage::TypeScript, unstructured_text_type(), input);
        }
        for input in [
            r#"{tag: "url", value: Url("https://example.com/foo")}"#,
            r#"{tag: "inline", value: Binary("data:application/json;base64,e30")}"#,
        ] {
            assert_parses(
                SourceLanguage::TypeScript,
                unstructured_binary_type(),
                input,
            );
        }
        assert_parses(
            SourceLanguage::TypeScript,
            SchemaType::list(SchemaType::variant(vec![
                variant_case("text", unstructured_text_type()),
                variant_case("binary", unstructured_binary_type()),
            ])),
            r#"[{tag: "text", value: {tag: "inline", value: Text("data")}}, {tag: "binary", value: {tag: "url", value: Url("https://example.com/foo")}}]"#,
        );
        assert_parses(
            SourceLanguage::TypeScript,
            SchemaType::list(SchemaType::variant(vec![
                variant_case("image", SchemaType::list(SchemaType::u8())),
                variant_case("text", SchemaType::string()),
            ])),
            r#"[{tag: "text", value: "foo"}, {tag: "image", value: [1, 2, 3]}]"#,
        );
    }
}
