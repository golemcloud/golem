// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use crate::command_handler::worker::stream::WorkerConnection;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::{AnyhowMapServiceError, ServiceError};
use crate::error::NonSuccessfulExit;
use crate::fuzzy::{Error, FuzzySearch};
use crate::log::{
    log_action, log_error, log_error_action, log_failed_to, log_warn, log_warn_action, logln,
    LogColorize, LogIndent,
};
use crate::model::app::ApplicationComponentSelectMode;
use crate::model::component::{show_exported_agent_constructors, ComponentNameMatchKind};
use crate::model::deploy::{TryUpdateAllWorkersResult, WorkerUpdateAttempt};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::fmt::{log_fuzzy_match, log_text_view};
use crate::model::text::help::{
    ArgumentError, AvailableAgentConstructorsHelp, AvailableComponentNamesHelp,
    AvailableFunctionNamesHelp, ParameterErrorTableView, WorkerNameHelp,
};
use crate::model::text::worker::{
    format_timestamp, format_worker_name_match, FileNodeView, WorkerCreateView, WorkerFilesView,
    WorkerGetView,
};
use anyhow::{anyhow, bail, Context as AnyhowContext};
use chrono::{DateTime, Utc};
use colored::Colorize;

use crate::model::environment::{EnvironmentReference, EnvironmentResolveMode};
use crate::model::worker::{
    AgentUpdateMode, WorkerMetadata, WorkerMetadataView, WorkerName, WorkerNameMatch,
    WorkersMetadataResponseView,
};
use golem_client::api::{AgentClient, ComponentClient, EnvironmentClient, WorkerClient};
use golem_client::model::ScanCursor;
use golem_client::model::{
    AgentInvocationMode, AgentInvocationRequest, ComponentDto, RevertWorkerTarget,
    UpdateWorkerRequest, WorkerCreationRequest,
};
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{
    AgentId, AgentType, ComponentModelElementValue, DataSchema, DataValue, ElementSchema,
    ElementValue, ElementValues, UntypedJsonDataValue,
};
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component_metadata::{
    ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite,
};
use golem_common::model::environment::EnvironmentName;
use golem_common::model::oplog::{OplogCursor, PublicOplogEntry};
use golem_common::model::worker::{RevertLastInvocations, RevertToOplogIndex, UpdateRecord};
use golem_common::model::{IdempotencyKey, OplogIndex};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{parse_value_and_type, ValueAndType};
use inquire::Confirm;
use itertools::{EitherOrBoth, Itertools};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::debug;
use uuid::Uuid;

pub struct WorkerCommandHandler {
    ctx: Arc<Context>,
}

impl WorkerCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: AgentSubcommand) -> anyhow::Result<()> {
        match subcommand {
            AgentSubcommand::New {
                agent_id: worker_name,
                env,
                config_vars,
            } => self.cmd_new(worker_name, env, config_vars).await,
            AgentSubcommand::Invoke {
                agent_id: worker_name,
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
                    worker_name,
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
                agent_id: worker_name,
            } => self.cmd_get(worker_name).await,
            AgentSubcommand::Delete {
                agent_id: worker_name,
            } => self.cmd_delete(worker_name).await,
            AgentSubcommand::List {
                agent_type_name,
                component_name,
                filter: filters,
                scan_cursor,
                max_count,
                precise,
            } => {
                self.cmd_list(
                    agent_type_name,
                    component_name,
                    filters,
                    scan_cursor,
                    max_count,
                    precise,
                )
                .await
            }
            AgentSubcommand::Stream {
                agent_id: worker_name,
                stream_args,
            } => self.cmd_stream(worker_name, stream_args).await,
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
                agent_id: worker_name,
            } => self.cmd_interrupt(worker_name).await,
            AgentSubcommand::Update {
                agent_id: worker_name,
                mode,
                target_revision,
                r#await,
                disable_wakeup,
            } => {
                self.cmd_update(
                    worker_name,
                    mode.unwrap_or(AgentUpdateMode::Automatic),
                    target_revision,
                    r#await,
                    disable_wakeup,
                )
                .await
            }
            AgentSubcommand::Resume {
                agent_id: worker_name,
            } => self.cmd_resume(worker_name).await,
            AgentSubcommand::SimulateCrash {
                agent_id: worker_name,
            } => self.cmd_simulate_crash(worker_name).await,
            AgentSubcommand::Oplog {
                agent_id: worker_name,
                from,
                query,
            } => self.cmd_oplog(worker_name, from, query).await,
            AgentSubcommand::Revert {
                agent_id: worker_name,
                last_oplog_index,
                number_of_invocations,
            } => {
                self.cmd_revert(worker_name, last_oplog_index, number_of_invocations)
                    .await
            }
            AgentSubcommand::CancelInvocation {
                agent_id: worker_name,
                idempotency_key,
            } => {
                self.cmd_cancel_invocation(worker_name, idempotency_key)
                    .await
            }
            AgentSubcommand::Files { worker_name, path } => self.cmd_files(worker_name, path).await,
            AgentSubcommand::FileContents {
                worker_name,
                path,
                output,
            } => self.cmd_file_contents(worker_name, path, output).await,
        }
    }

    async fn cmd_new(
        &self,
        worker_name: AgentIdArgs,
        env: Vec<(String, String)>,
        config_vars: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let worker_name = worker_name.agent_id;
        let worker_name_match = self.match_worker_name(worker_name).await?;
        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                &worker_name_match.environment,
                worker_name_match.component_name_match_kind,
                &worker_name_match.component_name,
                Some((&worker_name_match.worker_name).into()),
                None,
                None,
                false,
            )
            .await?;

        self.validate_worker_and_function_names(&component, &worker_name_match.worker_name, None)?;

        log_action(
            "Creating",
            format!("new agent {}", format_worker_name_match(&worker_name_match)),
        );

        self.new_worker(
            component.id.0,
            worker_name_match.worker_name.0.clone(),
            env.into_iter().collect(),
            BTreeMap::from_iter(config_vars),
        )
        .await?;

        logln("");
        self.ctx.log_handler().log_view(&WorkerCreateView {
            component_name: worker_name_match.component_name,
            worker_name: Some(worker_name_match.worker_name),
        });

        Ok(())
    }

    async fn cmd_invoke(
        &self,
        worker_name: AgentIdArgs,
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

        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;

        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                &worker_name_match.environment,
                worker_name_match.component_name_match_kind,
                &worker_name_match.component_name,
                Some((&worker_name_match.worker_name).into()),
                post_deploy_args.as_ref(),
                None,
                false,
            )
            .await?;

        // First, validate without the function name
        let agent_id_and_type = self.validate_worker_and_function_names(
            &component,
            &worker_name_match.worker_name,
            None,
        )?;

        let (agent_id, agent_type) =
            agent_id_and_type.ok_or_else(|| anyhow!("Agent invoke requires an agent component"))?;

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

        // Update worker_name with normalized agent id
        let worker_name_match = WorkerNameMatch {
            worker_name: agent_id.to_string().into(),
            ..worker_name_match
        };

        let mode = if trigger {
            log_action(
                "Triggering",
                format!(
                    "invocation for agent {}/{}",
                    format_worker_name_match(&worker_name_match),
                    method_name.log_color_highlight()
                ),
            );
            AgentInvocationMode::Schedule
        } else {
            log_action(
                "Invoking",
                format!(
                    "agent {}/{} ",
                    format_worker_name_match(&worker_name_match),
                    method_name.log_color_highlight()
                ),
            );
            AgentInvocationMode::Await
        };

        let method_parameters =
            wave_args_to_agent_method_parameters(&agent_type, &method_name, arguments)?;

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

        let environment = &worker_name_match.environment;

        let request = AgentInvocationRequest {
            app_name: environment.application_name.to_string(),
            env_name: environment.environment_name.to_string(),
            agent_type_name: agent_id.agent_type.to_wit_naming().0,
            parameters: UntypedJsonDataValue::from(agent_id.parameters.clone()),
            phantom_id: agent_id.phantom_id,
            method_name: method_name.clone(),
            method_parameters,
            mode,
            schedule_at,
            idempotency_key: Some(idempotency_key.value.clone()),
            component_revision: None,
            deployment_revision: None,
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
        worker_name: AgentIdArgs,
        stream_args: StreamArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Connecting",
            format!("to agent {}", format_worker_name_match(&worker_name_match)),
        );

        let connection = WorkerConnection::new(
            self.ctx.worker_service_url().clone(),
            self.ctx.auth_token().await?,
            &component.id,
            worker_name.0.clone(),
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
            .list_agent_types(&environment)
            .await?
            .into_iter()
            .find(|t| t.agent_type.type_name.0 == agent_type_name)
        else {
            bail!("Agent type not found: {}", agent_type_name);
        };

        let agent_type_name = agent_type.agent_type.type_name;
        let typed_parameters = DataValue::try_from_untyped_json(
            parameters,
            agent_type.agent_type.constructor.input_schema,
        )
        .map_err(|err| {
            anyhow!("Failed to match agent type parameters to the latest metadata: {err}")
        })?;
        let agent_id = AgentId::new(agent_type_name, typed_parameters, phantom_id);
        let worker_name = WorkerName(agent_id.to_string());

        let worker_name_match = self.match_worker_name(worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        let connection = WorkerConnection::new(
            self.ctx.worker_service_url().clone(),
            self.ctx.auth_token().await?,
            &component.id,
            worker_name.0.clone(),
            stream_args.into(),
            self.ctx.allow_insecure(),
            self.ctx.format(),
            Some(idempotency_key),
        )
        .await?;

        connection.run_forever().await;

        Ok(())
    }

    async fn cmd_simulate_crash(&self, worker_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Simulating crash",
            format!("for agent {}", format_worker_name_match(&worker_name_match)),
        );

        self.interrupt_worker(&component, &worker_name, true)
            .await?;

        log_action(
            "Simulated crash",
            format!("for agent {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn cmd_oplog(
        &self,
        worker_name: AgentIdArgs,
        from: Option<u64>,
        query: Option<String>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
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
                        &worker_name.0,
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
        worker_name: AgentIdArgs,
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
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Reverting",
            format!("agent {}", format_worker_name_match(&worker_name_match)),
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
                .revert_worker(&component.id.0, &worker_name.0, &target)
                .await
                .map(|_| ())
                .map_service_error()?
        }

        log_action(
            "Reverted",
            format!("agent {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn cmd_cancel_invocation(
        &self,
        worker_name: AgentIdArgs,
        idempotency_key: IdempotencyKey,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_warn_action(
            "Canceling invocation",
            format!(
                "for agent {} using idempotency key: {}",
                format_worker_name_match(&worker_name_match),
                idempotency_key.value.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;

        let canceled = clients
            .worker
            .cancel_invocation(&component.id.0, &worker_name.0, &idempotency_key.value)
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
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        let component_handler = self.ctx.component_handler();

        let (components, filters) = match agent_type_name {
            Some(agent_type_name) => {
                let environment = self
                    .ctx
                    .environment_handler()
                    .resolve_environment(EnvironmentResolveMode::Any)
                    .await?;
                environment
                    .with_current_deployment_revision_or_default_warn(
                        |current_deployment_revision| async move {
                            debug!("Finding agent type {}", agent_type_name);
                            let Some(agent_type) = clients
                                .environment
                                .get_deployment_agent_type(
                                    &environment.environment_id.0,
                                    current_deployment_revision.into(),
                                    &agent_type_name,
                                )
                                .await
                                .map_service_error_not_found_as_opt()?
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
                                    agent_type.agent_type.wrapper_type_name()
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
                        },
                    )
                    .await?
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
                    .await?
            }
        };

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
            logln("Switch to an application directory with only one component or explicitly specify the requested component name.");
            logln("");
            bail!(NonSuccessfulExit);
        }

        let mut view = WorkersMetadataResponseView::default();

        for component in &components {
            let (workers, scan_cursor) = self
                .list_component_workers(
                    &component.component_name,
                    &component.id,
                    Some(filters.as_slice()),
                    scan_cursor.as_ref(),
                    max_count,
                    precise,
                )
                .await?;

            view.workers
                .extend(workers.into_iter().map(WorkerMetadataView::from));
            scan_cursor.into_iter().for_each(|scan_cursor| {
                view.cursors.insert(
                    component.component_name.to_string(),
                    scan_cursor_to_string(&scan_cursor),
                );
            });
        }

        self.ctx.log_handler().log_view(&view);

        Ok(())
    }

    async fn cmd_interrupt(&self, worker_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Interrupting",
            format!("agent {}", format_worker_name_match(&worker_name_match)),
        );

        self.interrupt_worker(&component, &worker_name, false)
            .await?;

        log_action(
            "Interrupted",
            format!("agent {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn cmd_resume(&self, worker_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Resuming",
            format!("agent {}", format_worker_name_match(&worker_name_match)),
        );

        self.resume_worker(&component, &worker_name).await?;

        log_action(
            "Resumed",
            format!("agent {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn cmd_update(
        &self,
        worker_name: AgentIdArgs,
        mode: AgentUpdateMode,
        target_revision: Option<ComponentRevision>,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let environment = &worker_name_match.environment;

        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
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
                    &worker_name,
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
            &worker_name.0,
            mode,
            target_revision,
            await_update,
            disable_wakeup,
        )
        .await?;

        Ok(())
    }

    async fn cmd_get(&self, worker_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let result = {
            let result = clients
                .worker
                .get_worker_metadata(&component.id.0, &worker_name.0)
                .await
                .map_service_error()?;

            WorkerMetadata::from(worker_name_match.component_name, result)
        };

        self.ctx
            .log_handler()
            .log_view(&WorkerGetView::from_metadata(result, true));

        Ok(())
    }

    async fn cmd_delete(&self, worker_name: AgentIdArgs) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_warn_action(
            "Deleting",
            format!("agent {}", format_worker_name_match(&worker_name_match)),
        );

        self.delete(component.id.0, &worker_name.0).await?;

        log_action(
            "Deleted",
            format!("agent {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn cmd_files(&self, worker_name: AgentIdArgs, path: String) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Listing files",
            format!(
                "for agent {} at path {}",
                format_worker_name_match(&worker_name_match),
                path.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        let nodes = match clients
            .worker
            .get_files(&component.id.0, &worker_name.0, &path)
            .await
            .map_service_error()
        {
            Ok(nodes) => nodes,
            Err(e) => {
                log_warn_action(
                    "Failed to list files",
                    format!(
                        "for agent {} at path {}: {e}",
                        format_worker_name_match(&worker_name_match),
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
                format_worker_name_match(&worker_name_match),
                path.log_color_highlight()
            ),
        );

        Ok(())
    }

    async fn cmd_file_contents(
        &self,
        worker_name: AgentIdArgs,
        path: String,
        output: Option<String>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Downloading file",
            format!(
                "from agent {} at path {}",
                format_worker_name_match(&worker_name_match),
                path.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;
        let file_contents = match clients
            .worker
            .get_file_content(&component.id.0, &worker_name.0, &path)
            .await
            .map_service_error()
        {
            Ok(contents) => contents,
            Err(e) => {
                log_warn_action(
                    "Failed to download file",
                    format!(
                        "from agent {} at path {}: {e}",
                        format_worker_name_match(&worker_name_match),
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

    async fn new_worker(
        &self,
        component_id: Uuid,
        worker_name: String,
        env: HashMap<String, String>,
        config_vars: BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .launch_new_worker(
                &component_id,
                &WorkerCreationRequest {
                    name: worker_name,
                    env,
                    config_vars,
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
        worker_name: &WorkerName,
    ) -> anyhow::Result<WorkerMetadata> {
        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .worker
            .get_worker_metadata(&component_id, &worker_name.0)
            .await
            .map_service_error()?;

        Ok(WorkerMetadata::from(component_name.clone(), result))
    }

    async fn delete(&self, component_id: Uuid, worker_name: &str) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .delete_worker(&component_id, worker_name)
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
                    &worker.worker_id.component_id,
                    &worker.worker_id.worker_name,
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
                        worker_name: worker.worker_id.worker_name.as_str().into(),
                        error: None,
                    });
                }
                Err(error) => {
                    update_results.triggered.push(WorkerUpdateAttempt {
                        component_name: component_name.clone(),
                        target_revision,
                        worker_name: worker.worker_id.worker_name.as_str().into(),
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        if await_update {
            for worker in workers {
                let _ = self
                    .await_update_result(
                        &worker.worker_id.component_id,
                        &worker.worker_id.worker_name,
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
        worker_name: &str,
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
                worker_name.bold().green(),
                target_revision.to_string().log_color_highlight(),
                update_mode.to_string().log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .worker
            .update_worker(
                &component_id.0,
                worker_name,
                &UpdateWorkerRequest {
                    mode: match update_mode {
                        AgentUpdateMode::Automatic => {
                            golem_client::model::WorkerUpdateMode::Automatic
                        }
                        AgentUpdateMode::Manual => golem_client::model::WorkerUpdateMode::Manual,
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
                    self.await_update_result(component_id, worker_name, target_revision)
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
        worker_name: &str,
        target_revision: ComponentRevision,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        loop {
            let metadata = clients
                .worker
                .get_worker_metadata(&component_id.0, worker_name)
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
                    return Err(anyhow!("Unexpected agent state: update is not pending anymore, but no outcome has been found"));
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
        worker_metadata: WorkerMetadata,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Redeploying",
            format!(
                "agent {}/{} to latest version",
                component_name.0.bold().blue(),
                worker_metadata.worker_id.worker_name.bold().green(),
            ),
        );
        let _indent = LogIndent::new();

        self.delete_worker(component_name, &worker_metadata).await?;

        log_action(
            "Recreating",
            format!(
                "agent {}/{}",
                component_name.0.bold().blue(),
                worker_metadata.worker_id.worker_name.bold().green(),
            ),
        );
        self.new_worker(
            worker_metadata.worker_id.component_id.0,
            worker_metadata.worker_id.worker_name,
            worker_metadata.env,
            worker_metadata.config_vars,
        )
        .await?;
        log_action("Recreated", "agent");

        Ok(())
    }

    pub async fn delete_worker(
        &self,
        component_name: &ComponentName,
        worker_metadata: &WorkerMetadata,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Deleting",
            format!(
                "agent {}/{}",
                component_name.0.bold().blue(),
                worker_metadata.worker_id.worker_name.bold().green(),
            ),
        );
        self.delete(
            worker_metadata.worker_id.component_id.0,
            &worker_metadata.worker_id.worker_name,
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
    ) -> anyhow::Result<(Vec<WorkerMetadata>, Option<ScanCursor>)> {
        let clients = self.ctx.golem_clients().await?;
        let mut workers = Vec::<WorkerMetadata>::new();
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
                        .map(|meta| WorkerMetadata::from(component_name.clone(), meta)),
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

    async fn component_by_worker_name_match(
        &self,
        worker_name_match: &WorkerNameMatch,
    ) -> anyhow::Result<(ComponentDto, WorkerName)> {
        let component = self
            .ctx
            .component_handler()
            .resolve_component(
                &worker_name_match.environment,
                &worker_name_match.component_name,
                Some((&worker_name_match.worker_name).into()),
            )
            .await?;

        let Some(component) = component else {
            log_error(format!(
                "Component {} not found",
                worker_name_match
                    .component_name
                    .0
                    .log_color_error_highlight()
            ));
            logln("");
            bail!(NonSuccessfulExit);
        };

        Ok((component, worker_name_match.worker_name.clone()))
    }

    async fn resume_worker(
        &self,
        component: &ComponentDto,
        worker_name: &WorkerName,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .resume_worker(&component.id.0, &worker_name.0)
            .await
            .map(|_| ())
            .map_service_error()?;

        Ok(())
    }

    async fn interrupt_worker(
        &self,
        component: &ComponentDto,
        worker_name: &WorkerName,
        recover_immediately: bool,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .interrupt_worker(&component.id.0, &worker_name.0, Some(recover_immediately))
            .await
            .map(|_| ())
            .map_service_error()?;

        Ok(())
    }

    pub async fn match_worker_name(
        &self,
        worker_name: WorkerName,
    ) -> anyhow::Result<WorkerNameMatch> {
        let segments = split_worker_name(&worker_name.0);
        match segments.len() {
            // <WORKER>
            1 => {
                let worker_name = segments[0].to_string();

                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::CurrentDir)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.opt()?;
                match app_ctx {
                    Some(app_ctx) => {
                        let selected_component_names = app_ctx.selected_component_names();

                        if selected_component_names.len() != 1 {
                            logln("");
                            log_error(
                                format!("Multiple components were selected based on the current directory: {}",
                                        selected_component_names.iter().map(|cn| cn.as_str().log_color_highlight()).join(", ")),
                            );
                            logln("");
                            logln(
                                "Switch to a different directory with only one component or specify the full or partial component name as part of the agent name!",
                            );
                            logln("");
                            log_text_view(&WorkerNameHelp);
                            logln("");
                            log_text_view(&AvailableComponentNamesHelp(
                                app_ctx.application().component_names().cloned().collect(),
                            ));
                            bail!(NonSuccessfulExit);
                        }

                        let environment = self
                            .ctx
                            .environment_handler()
                            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
                            .await?;

                        Ok(WorkerNameMatch {
                            environment,
                            component_name_match_kind: ComponentNameMatchKind::AppCurrentDir,
                            component_name: ComponentName::try_from(
                                selected_component_names.iter().next().unwrap().as_str(),
                            )
                            .map_err(|err| anyhow!(err))?,
                            worker_name: worker_name.into(),
                        })
                    }
                    None => {
                        logln("");
                        log_error("Cannot infer the component name for the agent as the current directory is not part of an application.");
                        logln("");
                        logln("Switch to an application directory or specify the full component name as part of the agent name!");
                        logln("");
                        log_text_view(&WorkerNameHelp);
                        // TODO: hint for deployed component names?
                        bail!(NonSuccessfulExit);
                    }
                }
            }
            // [ACCOUNT]/[APPLICATION]/[ENVIRONMENT]/<COMPONENT>/<WORKER>
            2..=5 => {
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

                fn validated_component(value: &str) -> anyhow::Result<ComponentName> {
                    Ok(ComponentName(non_empty("component", value)?.to_string()))
                }

                fn validated_worker(value: &str) -> anyhow::Result<String> {
                    Ok(non_empty("agent", value)?.to_string())
                }

                let (environment_reference, component_name, worker_name): (
                    Option<EnvironmentReference>,
                    ComponentName,
                    String,
                ) = match segments.len() {
                    2 => (
                        None,
                        validated_component(segments[0])?,
                        validated_worker(segments[1])?,
                    ),
                    3 => (
                        Some(EnvironmentReference::Environment {
                            environment_name: validated_environment(segments[0])?,
                        }),
                        validated_component(segments[1])?,
                        validated_worker(segments[2])?,
                    ),
                    4 => (
                        Some(EnvironmentReference::ApplicationEnvironment {
                            application_name: validated_application(segments[0])?,
                            environment_name: validated_environment(segments[1])?,
                        }),
                        validated_component(segments[2])?,
                        validated_worker(segments[3])?,
                    ),
                    5 => (
                        Some(EnvironmentReference::AccountApplicationEnvironment {
                            account_email: validated_account(segments[0])?,
                            application_name: validated_application(segments[1])?,
                            environment_name: validated_environment(segments[2])?,
                        }),
                        validated_component(segments[3])?,
                        validated_worker(segments[4])?,
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

                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.opt()?;
                match app_ctx {
                    Some(app_ctx) if environment.is_manifest_scoped() => {
                        let fuzzy_search = FuzzySearch::new(
                            app_ctx
                                .application()
                                .component_names()
                                .map(|cn| cn.as_str()),
                        );
                        match fuzzy_search.find(&component_name.0) {
                            Ok(match_) => {
                                log_fuzzy_match(&match_);
                                Ok(WorkerNameMatch {
                                    environment,
                                    component_name_match_kind: ComponentNameMatchKind::App,
                                    component_name: ComponentName(match_.option),
                                    worker_name: worker_name.into(),
                                })
                            }
                            Err(error) => match error {
                                Error::Ambiguous {
                                    highlighted_options,
                                    ..
                                } => {
                                    logln("");
                                    log_error(format!(
                                        "The requested application component name ({}) is ambiguous.",
                                        component_name.0.log_color_error_highlight()
                                    ));
                                    logln("");
                                    logln("Did you mean one of");
                                    for option in highlighted_options {
                                        logln(format!(" - {}", option.bold()));
                                    }
                                    logln("?");
                                    logln("");
                                    log_text_view(&WorkerNameHelp);
                                    logln("");
                                    log_text_view(&AvailableComponentNamesHelp(
                                        app_ctx.application().component_names().cloned().collect(),
                                    ));

                                    bail!(NonSuccessfulExit);
                                }
                                Error::NotFound { .. } => {
                                    // Assuming non-app component
                                    Ok(WorkerNameMatch {
                                        environment,
                                        component_name_match_kind: ComponentNameMatchKind::Unknown,
                                        component_name,
                                        worker_name: worker_name.into(),
                                    })
                                }
                            },
                        }
                    }
                    _ => Ok(WorkerNameMatch {
                        environment,
                        component_name_match_kind: ComponentNameMatchKind::Unknown,
                        component_name,
                        worker_name: worker_name.into(),
                    }),
                }
            }
            _ => {
                logln("");
                log_error(format!(
                    "Failed to parse agent name: {}",
                    worker_name.0.log_color_error_highlight()
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
        worker_name: &WorkerName,
        function_name: Option<&str>,
    ) -> anyhow::Result<Option<(AgentId, AgentType)>> {
        if !component.metadata.is_agent() {
            return Ok(None);
        }

        match AgentId::parse_and_resolve_type(&worker_name.0, &component.metadata) {
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
                        if interface == agent_id.wrapper_agent_type()
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
                    worker_name.0.log_color_error_highlight()
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

/// Extracts the simple method name from a potentially fully qualified function name.
///
/// For example, `rust:agent/foo-agent.{fun-string}` → `fun-string`.
/// Returns the input unchanged if it is already a simple name.
fn extract_simple_method_name(function_name: &str) -> String {
    if let Ok(parsed) = ParsedFunctionName::parse(function_name) {
        if let ParsedFunctionReference::Function { function } = &parsed.function {
            return function.clone();
        }
    }
    function_name.to_string()
}

/// Fuzzy-matches a method name pattern against both original and WIT-named (kebab-case)
/// method names, returning the **original** method name on success.
fn resolve_agent_method_name(
    provided_method_name: &str,
    agent_type: &AgentType,
) -> crate::fuzzy::Result {
    let mut alias_to_original: HashMap<String, String> = HashMap::new();
    let mut aliases: Vec<String> = Vec::new();

    for m in &agent_type.methods {
        let original = m.name.clone();
        let wit = original.to_wit_naming();
        for alias in [original.clone(), wit] {
            if !alias_to_original.contains_key(&alias) {
                alias_to_original.insert(alias.clone(), original.clone());
                aliases.push(alias);
            }
        }
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

fn wave_args_to_agent_method_parameters(
    agent_type: &AgentType,
    method_name: &str,
    wave_args: Vec<String>,
) -> anyhow::Result<UntypedJsonDataValue> {
    let method = agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| anyhow!("Method '{}' not found in agent type", method_name))?;
    let element_schemas = match &method.input_schema {
        DataSchema::Tuple(schemas) => &schemas.elements,
        DataSchema::Multimodal(_) => {
            bail!("Multimodal method parameters are not supported via CLI WAVE arguments")
        }
    };
    if element_schemas.len() != wave_args.len() {
        logln("");
        log_error(format!(
            "Wrong number of parameters: expected {}, got {}",
            element_schemas.len(),
            wave_args.len()
        ));
        logln("");
        let types_and_args: Vec<ArgumentError> = element_schemas
            .iter()
            .zip_longest(wave_args.iter())
            .map(|zipped| match zipped {
                EitherOrBoth::Both(schema, value) => ArgumentError {
                    type_: match &schema.schema {
                        ElementSchema::ComponentModel(cm) => Some(cm.element_type.clone()),
                        _ => None,
                    },
                    value: Some(value.clone()),
                    error: None,
                },
                EitherOrBoth::Left(schema) => ArgumentError {
                    type_: match &schema.schema {
                        ElementSchema::ComponentModel(cm) => Some(cm.element_type.clone()),
                        _ => None,
                    },
                    value: None,
                    error: Some("missing argument".log_color_error().to_string()),
                },
                EitherOrBoth::Right(value) => ArgumentError {
                    type_: None,
                    value: Some(value.clone()),
                    error: Some("extra argument".log_color_error().to_string()),
                },
            })
            .collect();
        log_text_view(&ParameterErrorTableView(types_and_args));
        logln("");
        bail!(NonSuccessfulExit);
    }
    let mut element_values = Vec::with_capacity(element_schemas.len());
    let mut has_errors = false;
    let mut error_entries = Vec::new();
    for (schema, wave_arg) in element_schemas.iter().zip(wave_args.iter()) {
        match &schema.schema {
            ElementSchema::ComponentModel(cm) => {
                match lenient_parse_type_annotated_value(&cm.element_type.to_wit_naming(), wave_arg)
                {
                    Ok(mut vt) => {
                        vt.typ = cm.element_type.clone();
                        element_values.push(ElementValue::ComponentModel(
                            ComponentModelElementValue { value: vt },
                        ));
                        error_entries.push(None);
                    }
                    Err(err) => {
                        has_errors = true;
                        element_values.push(ElementValue::ComponentModel(
                            ComponentModelElementValue {
                                value: golem_wasm::ValueAndType::new(
                                    golem_wasm::Value::Bool(false),
                                    golem_wasm::analysis::analysed_type::bool(),
                                ),
                            },
                        ));
                        error_entries.push(Some(err));
                    }
                }
            }
            _ => bail!("Non-ComponentModel element schema is not supported via CLI WAVE arguments"),
        }
    }
    if has_errors {
        logln("");
        log_error("Argument WAVE parse error(s)!");
        logln("");
        let error_table: Vec<ArgumentError> = element_schemas
            .iter()
            .zip(wave_args.iter())
            .zip(error_entries.iter())
            .map(|((schema, value), err)| ArgumentError {
                type_: match &schema.schema {
                    ElementSchema::ComponentModel(cm) => Some(cm.element_type.clone()),
                    _ => None,
                },
                value: Some(value.clone()),
                error: err
                    .as_ref()
                    .map(|e| e.log_color_error_highlight().to_string()),
            })
            .collect();
        log_text_view(&ParameterErrorTableView(error_table));
        logln("");
        bail!(NonSuccessfulExit);
    }
    Ok(UntypedJsonDataValue::from(DataValue::Tuple(
        ElementValues {
            elements: element_values,
        },
    )))
}

pub fn lenient_parse_type_annotated_value(
    analysed_type: &AnalysedType,
    input: &str,
) -> Result<ValueAndType, String> {
    let patched_input = match analysed_type {
        AnalysedType::Chr(_) => (!input.starts_with('\'')).then(|| format!("'{input}'")),
        AnalysedType::Str(_) => (!input.starts_with('"')).then(|| format!("\"{input}\"")),
        _ => None,
    };

    let input = patched_input.as_deref().unwrap_or(input);
    parse_value_and_type(analysed_type, input)
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

fn split_worker_name(worker_name: &str) -> Vec<&str> {
    match worker_name.find('(') {
        Some(constructor_open_parentheses_idx) => {
            let splittable = &worker_name[0..constructor_open_parentheses_idx];
            let last_slash_idx = splittable.rfind('/');
            match last_slash_idx {
                Some(last_slash_idx) => {
                    let mut segments = worker_name[0..last_slash_idx]
                        .split('/')
                        .collect::<Vec<&str>>();
                    segments.push(&worker_name[last_slash_idx + 1..]);
                    segments
                }
                None => {
                    vec![worker_name]
                }
            }
        }
        None => worker_name.split("/").collect(),
    }
}

#[cfg(test)]
mod tests {
    use crate::command_handler::worker::split_worker_name;
    use pretty_assertions::assert_eq;
    use test_r::test;

    #[test]
    fn test_split_worker_name() {
        assert_eq!(split_worker_name("a"), vec!["a"]);
        assert_eq!(split_worker_name("a()"), vec!["a()"]);
        assert_eq!(split_worker_name("a(\"///\")"), vec!["a(\"///\")"]);
        assert_eq!(split_worker_name("a/b"), vec!["a", "b"]);
        assert_eq!(split_worker_name("a/b()"), vec!["a", "b()"]);
        assert_eq!(split_worker_name("a/b(\"///\")"), vec!["a", "b(\"///\")"]);
        assert_eq!(split_worker_name("a/b/c"), vec!["a", "b", "c"]);
        assert_eq!(split_worker_name("a/b/c()"), vec!["a", "b", "c()"]);
        assert_eq!(
            split_worker_name("a/b/c(\"/\")"),
            vec!["a", "b", "c(\"/\")"]
        );
        assert_eq!(split_worker_name("/"), vec!["", ""]);
        assert_eq!(split_worker_name("a(/"), vec!["a(/"]);
    }
}
