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
    AgentIdArgs, DeployArgs, StreamArgs, WorkerFunctionArgument, WorkerFunctionName,
};
use crate::command::worker::AgentSubcommand;
use crate::command_handler::worker::stream::WorkerConnection;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::{AnyhowMapServiceError, ServiceError};
use crate::error::NonSuccessfulExit;
use crate::fuzzy::{Error, FuzzySearch};
use crate::log::{log_action, log_error_action, log_warn_action, logln, LogColorize, LogIndent};
use crate::model::app::ApplicationComponentSelectMode;
use crate::model::component::{
    agent_interface_name, function_params_types, show_exported_agent_constructors,
    ComponentNameMatchKind,
};
use crate::model::deploy::{TryUpdateAllWorkersResult, WorkerUpdateAttempt};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::fmt::{
    format_export, format_worker_name_match, log_error, log_fuzzy_match, log_text_view, log_warn,
};
use crate::model::text::help::{
    ArgumentError, AvailableAgentConstructorsHelp, AvailableComponentNamesHelp,
    AvailableFunctionNamesHelp, ParameterErrorTableView, WorkerNameHelp,
};
use crate::model::text::worker::{
    format_timestamp, FileNodeView, WorkerCreateView, WorkerFilesView, WorkerGetView,
};
use anyhow::{anyhow, bail};
use colored::Colorize;

use crate::model::environment::{EnvironmentReference, EnvironmentResolveMode};
use crate::model::worker::{
    fuzzy_match_function_name, AgentUpdateMode, WorkerMetadata, WorkerName, WorkerNameMatch,
};
use golem_client::api::WorkerClient;
use golem_client::model::{
    ComponentDto, InvokeParameters, RevertWorkerTarget, UpdateWorkerRequest, WorkerCreationRequest,
};
use golem_client::model::{InvokeResult, ScanCursor};
use golem_common::model::agent::AgentId;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentName;
use golem_common::model::oplog::{OplogCursor, PublicOplogEntry};
use golem_common::model::worker::{
    RevertLastInvocations, RevertToOplogIndex, UpdateRecord, WasiConfigVars,
};
use golem_common::model::{IdempotencyKey, OplogIndex};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::OptionallyValueAndTypeJson;
use golem_wasm::{parse_value_and_type, ValueAndType};
use inquire::Confirm;
use itertools::{EitherOrBoth, Itertools};
use rib::ParsedFunctionSite;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
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
            } => self.cmd_new(worker_name, env).await,
            AgentSubcommand::Invoke {
                agent_id: worker_name,
                function_name,
                arguments,
                trigger,
                idempotency_key,
                stream,
                stream_args,
                deploy_args,
            } => {
                self.cmd_invoke(
                    worker_name,
                    &function_name,
                    arguments,
                    trigger,
                    idempotency_key,
                    stream,
                    stream_args,
                    deploy_args,
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
                component_name,
                agent_type_name,
                filter: filters,
                scan_cursor,
                max_count,
                precise,
            } => {
                self.cmd_list(
                    component_name.component_name,
                    agent_type_name.agent_type_name,
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
            AgentSubcommand::Interrupt {
                agent_id: worker_name,
            } => self.cmd_interrupt(worker_name).await,
            AgentSubcommand::Update {
                agent_id: worker_name,
                mode,
                target_revision,
                r#await,
            } => {
                self.cmd_update(
                    worker_name,
                    mode.unwrap_or(AgentUpdateMode::Automatic),
                    target_revision,
                    r#await,
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
        stream: bool,
        stream_args: StreamArgs,
        deploy_args: Option<DeployArgs>,
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
                deploy_args.as_ref(),
            )
            .await?;

        // First, validate without the function name
        let agent_id_and_type = self.validate_worker_and_function_names(
            &component,
            &worker_name_match.worker_name,
            None,
        )?;

        let matched_function_name = fuzzy_match_function_name(
            function_name,
            component.metadata.exports(),
            agent_id_and_type
                .as_ref()
                .and_then(|a| agent_interface_name(&component, a.wrapper_agent_type()))
                .as_deref(),
        );
        let function_name = match matched_function_name {
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
                        "The requested function name ({}) is ambiguous.",
                        function_name.log_color_error_highlight()
                    ));
                    logln("");
                    logln("Did you mean one of");
                    for option in highlighted_options {
                        logln(format!(" - {}", option.bold()));
                    }
                    logln("?");
                    logln("");
                    log_text_view(&AvailableFunctionNamesHelp::new(
                        &component,
                        agent_id_and_type.as_ref(),
                    ));

                    bail!(NonSuccessfulExit);
                }
                Error::NotFound { .. } => {
                    logln("");
                    log_error(format!(
                        "The requested function name ({}) was not found.",
                        function_name.log_color_error_highlight()
                    ));
                    logln("");
                    log_text_view(&AvailableFunctionNamesHelp::new(
                        &component,
                        agent_id_and_type.as_ref(),
                    ));

                    bail!(NonSuccessfulExit);
                }
            },
        };

        // Re-validate with function-name
        let agent_id = self.validate_worker_and_function_names(
            &component,
            &worker_name_match.worker_name,
            Some(&function_name),
        )?;

        // Update worker_name with normalized agent id if needed
        let worker_name_match = {
            if let Some(agent_id) = agent_id {
                WorkerNameMatch {
                    worker_name: agent_id.to_string().into(),
                    ..worker_name_match
                }
            } else {
                worker_name_match
            }
        };

        if trigger {
            log_action(
                "Triggering",
                format!(
                    "invocation for agent {}/{}",
                    format_worker_name_match(&worker_name_match),
                    format_export(&function_name)
                ),
            );
        } else {
            log_action(
                "Invoking",
                format!(
                    "agent {}/{} ",
                    format_worker_name_match(&worker_name_match),
                    format_export(&function_name)
                ),
            );
        }

        let arguments = wave_args_to_invoke_args(&component, &function_name, arguments)?;

        let result = self
            .invoke_worker(
                &component,
                &worker_name_match.worker_name,
                &function_name,
                arguments,
                idempotency_key.clone(),
                trigger,
                stream.then_some(stream_args),
            )
            .await?;

        match result {
            Some(result) => {
                logln("");
                self.ctx
                    .log_handler()
                    .log_view(&InvokeResultView::new_invoke(
                        idempotency_key,
                        result,
                        &component,
                        &function_name,
                    ));
            }
            None => {
                log_action("Triggered", "invocation");
                self.ctx
                    .log_handler()
                    .log_view(&InvokeResultView::new_trigger(idempotency_key));
            }
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
        _component_name: Option<ComponentName>,
        _agent_type_name: Option<String>,
        mut _filters: Vec<String>,
        _scan_cursor: Option<ScanCursor>,
        _max_count: Option<u64>,
        _precise: bool,
    ) -> anyhow::Result<()> {
        // TODO: atomic
        /*
        let mut selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        if let Some(agent_type_name) = agent_type_name {
            let clients = self.ctx.golem_clients().await?;

            debug!("Finding agent type {}", agent_type_name);
            let response = clients
                .agent_types
                .get_agent_type(
                    &agent_type_name,
                    selected_components
                        .project
                        .as_ref()
                        .map(|p| &p.project_id.0),
                )
                .await?;

            debug!(
                "Fetching component metadata for {}",
                response.implemented_by.0
            );
            let component_metadata = clients
                .component
                .get_latest_component_metadata(&response.implemented_by.0)
                .await?;

            logln(format!(
                "Agent type {} is implemented by component {}\n",
                agent_type_name.log_color_highlight(),
                component_metadata.component_name.log_color_highlight()
            ));
            selected_components.component_names =
                vec![ComponentName(component_metadata.component_name)];

            filters.insert(0, format!("name startswith {agent_type_name}("));
        }

        if scan_cursor.is_some() && selected_components.component_names.len() != 1 {
            log_error(format!(
                "Cursor cannot be used with multiple components selected! ({})",
                selected_components
                    .component_names
                    .iter()
                    .map(|cn| cn.0.log_color_highlight())
                    .join(", ")
            ));
            logln("");
            logln("Switch to an application directory with only one component or explicitly specify the requested component name.");
            logln("");
            bail!(NonSuccessfulExit);
        }

        let mut view = WorkersMetadataResponseView::default();

        for component_name in &selected_components.component_names {
            match self
                .ctx
                .component_handler()
                .component(
                    selected_components.project.as_ref(),
                    component_name.into(),
                    None,
                )
                .await?
            {
                Some(component) => {
                    let (workers, scan_cursor) = self
                        .list_component_workers(
                            component_name,
                            component.versioned_component_id.component_id,
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
                            component_name.to_string(),
                            scan_cursor_to_string(&scan_cursor),
                        );
                    });
                }
                None => {
                    log_warn(format!(
                        "Component not found: {}",
                        component_name.0.log_color_error_highlight()
                    ));
                }
            }
        }

        self.ctx.log_handler().log_view(&view);

        Ok(())
         */
        todo!()
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
        _worker_name: AgentIdArgs,
        _mode: AgentUpdateMode,
        _target_revision: Option<ComponentRevision>, // TODO: atomic ComponentRevision
        _await_update: bool,
    ) -> anyhow::Result<()> {
        // TODO: atomic
        /*
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.agent_id).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        let target_version = match target_version {
            Some(target_version) => target_version,
            None => {
                let Some(latest_version) = self
                    .ctx
                    .component_handler()
                    .latest_component_version_by_id(component.versioned_component_id.component_id.0)
                    .await?
                else {
                    bail!(
                        "Component {} not found, while getting latest component version",
                        component.component_name
                    );
                };

                if !self.ctx.interactive_handler().confirm_update_to_latest(
                    &component.component_name,
                    &worker_name,
                    latest_version,
                )? {
                    bail!(NonSuccessfulExit)
                }

                latest_version
            }
        };

        self.update_worker(
            &component.component_name,
            component.versioned_component_id.component_id.0,
            &worker_name.0,
            mode,
            target_version.0,
            await_update,
        )
        .await?;

        Ok(())
         */
        todo!()
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
                "for worker {} at path {}",
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
                        "for worker {} at path {}: {e}",
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
                "for worker {} at path {}",
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
                "from worker {} at path {}",
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
                        "from worker {} at path {}: {e}",
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
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .launch_new_worker(
                &component_id,
                &WorkerCreationRequest {
                    name: worker_name,
                    env,
                    config_vars: WasiConfigVars::default(),
                },
            )
            .await
            .map(|_| ())
            .map_service_error()
    }

    pub async fn invoke_worker(
        &self,
        component: &ComponentDto,
        worker_name: &WorkerName,
        function_name: &str,
        arguments: Vec<OptionallyValueAndTypeJson>,
        idempotency_key: IdempotencyKey,
        enqueue: bool,
        stream_args: Option<StreamArgs>,
    ) -> anyhow::Result<Option<InvokeResult>> {
        let mut connect_handle = match stream_args {
            Some(stream_args) => {
                let connection = WorkerConnection::new(
                    self.ctx.worker_service_url().clone(),
                    self.ctx.auth_token().await?,
                    &component.id,
                    worker_name.0.clone(),
                    stream_args.into(),
                    self.ctx.allow_insecure(),
                    self.ctx.format(),
                    if enqueue {
                        None
                    } else {
                        Some(idempotency_key.clone())
                    },
                )
                .await?;
                Some(tokio::task::spawn(
                    async move { connection.run_forever().await },
                ))
            }
            None => None,
        };

        let clients = self.ctx.golem_clients().await?;

        let result = if enqueue {
            clients
                .worker
                .invoke_function(
                    &component.id.0,
                    &worker_name.0,
                    Some(&idempotency_key.value),
                    function_name,
                    &InvokeParameters { params: arguments },
                )
                .await
                .map_service_error()?;
            None
        } else {
            Some(
                clients
                    .worker_invoke
                    .invoke_and_await_function(
                        &component.id.0,
                        &worker_name.0,
                        Some(&idempotency_key.value),
                        function_name,
                        &InvokeParameters { params: arguments },
                    )
                    .await
                    .map_service_error()?,
            )
        };

        if enqueue && connect_handle.is_some() {
            // When 'enqueue' is used together with 'stream' we switch to infinite worker streaming here
            if let Some(handle) = connect_handle.take() {
                let _ = handle.await;
            }
        } else {
            // When a result is returned, we wait a few seconds to make sure we get all the log lines
            // but the worker stream task will quit as soon as the invocation end marker is reached

            if let Some(handle) = connect_handle.take() {
                let _ = timeout(Duration::from_secs(3), handle).await;
            }
        }

        Ok(result)
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
                    worker.worker_id.component_id.0,
                    &worker.worker_id.worker_name,
                    update_mode,
                    target_revision,
                    false,
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
                        &worker.worker_id.component_id.0,
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
        component_id: Uuid,
        worker_name: &str,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        await_update: bool,
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
                &component_id,
                worker_name,
                &UpdateWorkerRequest {
                    mode: match update_mode {
                        AgentUpdateMode::Automatic => {
                            golem_client::model::WorkerUpdateMode::Automatic
                        }
                        AgentUpdateMode::Manual => golem_client::model::WorkerUpdateMode::Manual,
                    },
                    target_revision: target_revision.0,
                },
            )
            .await
            .map(|_| ())
            .map_service_error();

        match result {
            Ok(_) => {
                log_action("Triggered update", "");

                if await_update {
                    self.await_update_result(&component_id, worker_name, target_revision)
                        .await?;
                }

                Ok(())
            }
            Err(error) => {
                log_error_action("Failed", "to trigger update for agent, error:");
                let _indent = LogIndent::new();
                logln(format!("{error}"));
                Err(anyhow!(error))
            }
        }
    }

    async fn await_update_result(
        &self,
        component_id: &Uuid,
        worker_name: &str,
        target_version: ComponentRevision,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        loop {
            let metadata = clients
                .worker
                .get_worker_metadata(component_id, worker_name)
                .await?;
            for update_record in metadata.updates {
                let mut latest_success = None;
                let mut latest_failure = None;
                let mut pending_count = 0;
                match update_record {
                    UpdateRecord::PendingUpdate(details)
                        if details.target_version == target_version =>
                    {
                        pending_count += 1;
                    }
                    UpdateRecord::SuccessfulUpdate(details)
                        if details.target_version == target_version =>
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
                        if details.target_version == target_version =>
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
                            "to version {} succeeded at {}",
                            success.target_version.to_string().log_color_highlight(),
                            success.timestamp.to_string().log_color_highlight()
                        ),
                    );
                    return Ok(());
                } else if let Some(failure) = latest_failure {
                    let error = failure.details.unwrap_or("unknown reason".to_string());
                    log_error_action(
                        "Agent update",
                        format!(
                            "to version {} succeeded at {}: {}",
                            failure.target_version.to_string().log_color_highlight(),
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
                format!("redeploying agents for component {component_name}, no workers found"),
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
                    format!("deleting agents for component {component_name}, no workers found"),
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
                                app_ctx.application.component_names().cloned().collect(),
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
                            app_ctx.application.component_names().map(|cn| cn.as_str()),
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
                                        app_ctx.application.component_names().cloned().collect(),
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
    ) -> anyhow::Result<Option<AgentId>> {
        if !component.metadata.is_agent() {
            return Ok(None);
        }

        match AgentId::parse(&worker_name.0, &component.metadata) {
            Ok(agent_id) => match function_name {
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
                            return Ok(Some(agent_id));
                        }
                    }

                    logln("");
                    log_error(format!(
                        "Incompatible agent type ({}) and method ({})",
                        agent_id.agent_type.log_color_error_highlight(),
                        function_name.log_color_error_highlight()
                    ));
                    logln("");
                    log_text_view(&AvailableFunctionNamesHelp::new(component, Some(&agent_id)));
                    bail!(NonSuccessfulExit);
                }

                None => Ok(Some(agent_id)),
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

fn wave_args_to_invoke_args(
    component: &ComponentDto,
    function_name: &str,
    wave_args: Vec<String>,
) -> anyhow::Result<Vec<OptionallyValueAndTypeJson>> {
    let types = function_params_types(component, function_name)?;

    if types.len() != wave_args.len() {
        logln("");
        log_error(format!(
            "Wrong number of parameters: expected {}, got {}",
            types.len(),
            wave_args.len()
        ));
        logln("");
        log_text_view(&ParameterErrorTableView(
            types
                .into_iter()
                .zip_longest(wave_args)
                .map(|zipped| match zipped {
                    EitherOrBoth::Both(typ, value) => ArgumentError {
                        type_: Some(typ.clone()),
                        value: Some(value),
                        error: None,
                    },
                    EitherOrBoth::Left(typ) => ArgumentError {
                        type_: Some(typ.clone()),
                        value: None,
                        error: Some("missing argument".log_color_error().to_string()),
                    },
                    EitherOrBoth::Right(value) => ArgumentError {
                        type_: None,
                        value: Some(value),
                        error: Some("extra argument".log_color_error().to_string()),
                    },
                })
                .collect::<Vec<_>>(),
        ));
        logln("");
        bail!(NonSuccessfulExit);
    }

    let type_annotated_values = wave_args
        .iter()
        .zip(types.iter())
        .map(|(wave, typ)| lenient_parse_type_annotated_value(typ, wave))
        .collect::<Vec<_>>();

    if type_annotated_values
        .iter()
        .any(|parse_result| parse_result.is_err())
    {
        logln("");
        log_error("Argument WAVE parse error(s)!");
        logln("");
        log_text_view(&ParameterErrorTableView(
            type_annotated_values
                .into_iter()
                .zip(types)
                .zip(wave_args)
                .map(|((parsed, typ), value)| (parsed, typ, value))
                .map(|(parsed, typ, value)| ArgumentError {
                    type_: Some(typ.clone()),
                    value: Some(value),
                    error: parsed
                        .err()
                        .map(|err| err.log_color_error_highlight().to_string()),
                })
                .collect::<Vec<_>>(),
        ));
        logln("");
        bail!(NonSuccessfulExit);
    }

    type_annotated_values
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| anyhow!(err))?
        .into_iter()
        .map(|tav| tav.try_into())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| anyhow!("Failed to convert type annotated value: {err}"))
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
    use assert2::assert;
    use test_r::test;

    #[test]
    fn test_split_worker_name() {
        assert!(split_worker_name("a") == vec!["a"]);
        assert!(split_worker_name("a()") == vec!["a()"]);
        assert!(split_worker_name("a(\"///\")") == vec!["a(\"///\")"]);
        assert!(split_worker_name("a/b") == vec!["a", "b"]);
        assert!(split_worker_name("a/b()") == vec!["a", "b()"]);
        assert!(split_worker_name("a/b(\"///\")") == vec!["a", "b(\"///\")"]);
        assert!(split_worker_name("a/b/c") == vec!["a", "b", "c"]);
        assert!(split_worker_name("a/b/c()") == vec!["a", "b", "c()"]);
        assert!(split_worker_name("a/b/c(\"/\")") == vec!["a", "b", "c(\"/\")"]);
        assert!(split_worker_name("/") == vec!["", ""]);
        assert!(split_worker_name("a(/") == vec!["a(/"]);
    }
}
