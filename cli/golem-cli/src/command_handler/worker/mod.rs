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
    NewWorkerArgument, StreamArgs, WorkerFunctionArgument, WorkerFunctionName, WorkerNameArg,
};
use crate::command::worker::WorkerSubcommand;
use crate::command_handler::worker::stream::WorkerConnection;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::{AnyhowMapServiceError, ServiceError};
use crate::error::NonSuccessfulExit;
use crate::fuzzy::{Error, FuzzySearch};
use crate::log::{log_action, log_error_action, log_warn_action, logln, LogColorize, LogIndent};
use crate::model::app::ApplicationComponentSelectMode;
use crate::model::component::{function_params_types, show_exported_functions, Component};
use crate::model::deploy::{TryUpdateAllWorkersResult, WorkerUpdateAttempt};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::fmt::{
    format_export, format_worker_name_match, log_error, log_fuzzy_match, log_text_view, log_warn,
};
use crate::model::text::help::{
    ArgumentError, AvailableComponentNamesHelp, AvailableFunctionNamesHelp, ComponentNameHelp,
    ParameterErrorTableView, WorkerNameHelp,
};
use crate::model::text::worker::{WorkerCreateView, WorkerGetView};
use crate::model::worker::fuzzy_match_function_name;
use crate::model::{
    ComponentName, ComponentNameMatchKind, IdempotencyKey, ProjectName, ProjectReference,
    WorkerMetadata, WorkerMetadataView, WorkerName, WorkerNameMatch, WorkerUpdateMode,
    WorkersMetadataResponseView,
};
use anyhow::{anyhow, bail};
use colored::Colorize;
use golem_client::api::WorkerClient;
use golem_client::model::{
    ComponentType, InvokeResult, PublicOplogEntry, ScanCursor, UpdateRecord,
};
use golem_client::model::{
    InvokeParameters as InvokeParametersCloud, RevertLastInvocations as RevertLastInvocationsCloud,
    RevertToOplogIndex as RevertToOplogIndexCloud, RevertWorkerTarget as RevertWorkerTargetCloud,
    UpdateWorkerRequest as UpdateWorkerRequestCloud,
    WorkerCreationRequest as WorkerCreationRequestCloud,
};
use golem_common::model::public_oplog::OplogCursor;
use golem_common::model::worker::WasiConfigVars;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::OptionallyValueAndTypeJson;
use golem_wasm_rpc::{parse_value_and_type, ValueAndType};
use itertools::{EitherOrBoth, Itertools};
use std::collections::HashMap;
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

    pub async fn handle_command(&self, subcommand: WorkerSubcommand) -> anyhow::Result<()> {
        match subcommand {
            WorkerSubcommand::New {
                worker_name,
                arguments,
                env,
            } => self.cmd_new(worker_name, arguments, env).await,
            WorkerSubcommand::Invoke {
                worker_name,
                function_name,
                arguments,
                enqueue,
                idempotency_key,
                stream,
                stream_args,
            } => {
                self.cmd_invoke(
                    worker_name,
                    &function_name,
                    arguments,
                    enqueue,
                    idempotency_key,
                    stream,
                    stream_args,
                )
                .await
            }
            WorkerSubcommand::Get { worker_name } => self.cmd_get(worker_name).await,
            WorkerSubcommand::Delete { worker_name } => self.cmd_delete(worker_name).await,
            WorkerSubcommand::List {
                component_name,
                filter: filters,
                scan_cursor,
                max_count,
                precise,
            } => {
                self.cmd_list(
                    component_name.component_name,
                    filters,
                    scan_cursor,
                    max_count,
                    precise,
                )
                .await
            }
            WorkerSubcommand::Stream {
                worker_name,
                stream_args,
            } => self.cmd_stream(worker_name, stream_args).await,
            WorkerSubcommand::Interrupt { worker_name } => self.cmd_interrupt(worker_name).await,
            WorkerSubcommand::Update {
                worker_name,
                mode,
                target_version,
                r#await,
            } => {
                self.cmd_update(
                    worker_name,
                    mode.unwrap_or(WorkerUpdateMode::Automatic),
                    target_version,
                    r#await,
                )
                .await
            }
            WorkerSubcommand::Resume { worker_name } => self.cmd_resume(worker_name).await,
            WorkerSubcommand::SimulateCrash { worker_name } => {
                self.cmd_simulate_crash(worker_name).await
            }
            WorkerSubcommand::Oplog {
                worker_name,
                from,
                query,
            } => self.cmd_oplog(worker_name, from, query).await,
            WorkerSubcommand::Revert {
                worker_name,
                last_oplog_index,
                number_of_invocations,
            } => {
                self.cmd_revert(worker_name, last_oplog_index, number_of_invocations)
                    .await
            }
            WorkerSubcommand::CancelInvocation {
                worker_name,
                idempotency_key,
            } => {
                self.cmd_cancel_invocation(worker_name, idempotency_key)
                    .await
            }
        }
    }

    async fn cmd_new(
        &self,
        worker_name: WorkerNameArg,
        arguments: Vec<NewWorkerArgument>,
        env: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let worker_name = worker_name.worker_name;
        let mut worker_name_match = self.match_worker_name(worker_name).await?;
        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                worker_name_match.project.as_ref(),
                worker_name_match.component_name_match_kind,
                &worker_name_match.component_name,
                worker_name_match.worker_name.as_ref().map(|wn| wn.into()),
            )
            .await?;

        if component.component_type == ComponentType::Ephemeral
            && worker_name_match.worker_name.is_some()
        {
            log_error("Cannot use explicit name for ephemeral worker!");
            logln("");
            logln("Use '-' as worker name for ephemeral workers");
            logln("");
            bail!(NonSuccessfulExit);
        }

        if worker_name_match.worker_name.is_none() {
            worker_name_match.worker_name = Some(Uuid::new_v4().to_string().into());
        }
        let worker_name = worker_name_match.worker_name.clone().unwrap().0;

        log_action(
            "Creating",
            format!(
                "new worker {}",
                format_worker_name_match(&worker_name_match)
            ),
        );

        self.new_worker(
            component.versioned_component_id.component_id,
            worker_name.clone(),
            arguments,
            env.into_iter().collect(),
        )
        .await?;

        logln("");
        self.ctx.log_handler().log_view(&WorkerCreateView {
            component_name: worker_name_match.component_name,
            worker_name: Some(worker_name.into()),
        });

        Ok(())
    }

    async fn cmd_invoke(
        &self,
        worker_name: WorkerNameArg,
        function_name: &WorkerFunctionName,
        arguments: Vec<WorkerFunctionArgument>,
        enqueue: bool,
        idempotency_key: Option<IdempotencyKey>,
        stream: bool,
        stream_args: StreamArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        fn new_idempotency_key() -> IdempotencyKey {
            let key = IdempotencyKey::new();
            log_action(
                "Using",
                format!("generated idempotency key: {}", key.0.log_color_highlight()),
            );
            key
        }

        let idempotency_key = match idempotency_key {
            Some(idempotency_key) if idempotency_key.0 == "-" => new_idempotency_key(),
            Some(idempotency_key) => {
                log_action(
                    "Using",
                    format!(
                        "requested idempotency key: {}",
                        idempotency_key.0.log_color_highlight()
                    ),
                );
                idempotency_key
            }
            None => new_idempotency_key(),
        };

        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;

        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                worker_name_match.project.as_ref(),
                worker_name_match.component_name_match_kind,
                &worker_name_match.component_name,
                worker_name_match.worker_name.as_ref().map(|wn| wn.into()),
            )
            .await?;

        let matched_function_name =
            fuzzy_match_function_name(function_name, component.metadata.exports());
        let function_name = match matched_function_name {
            Ok(match_) => {
                log_fuzzy_match(&match_);
                match_.option
            }
            Err(error) => {
                let component_functions =
                    show_exported_functions(component.metadata.exports(), false);

                match error {
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
                        log_text_view(&AvailableFunctionNamesHelp {
                            component_name: worker_name_match.component_name.0,
                            function_names: component_functions,
                        });

                        bail!(NonSuccessfulExit);
                    }
                    Error::NotFound { .. } => {
                        logln("");
                        log_error(format!(
                            "The requested function name ({}) was not found.",
                            function_name.log_color_error_highlight()
                        ));
                        logln("");
                        log_text_view(&AvailableFunctionNamesHelp {
                            component_name: worker_name_match.component_name.0,
                            function_names: component_functions,
                        });

                        bail!(NonSuccessfulExit);
                    }
                }
            }
        };

        if enqueue {
            log_action(
                "Enqueueing",
                format!(
                    "invocation for worker {}/{}",
                    format_worker_name_match(&worker_name_match),
                    format_export(&function_name)
                ),
            );
        } else {
            log_action(
                "Invoking",
                format!(
                    "worker {}/{} ",
                    format_worker_name_match(&worker_name_match),
                    format_export(&function_name)
                ),
            );
        }

        let arguments = wave_args_to_invoke_args(&component, &function_name, arguments)?;

        let result = self
            .invoke_worker(
                &component,
                worker_name_match.worker_name.as_ref(),
                &function_name,
                arguments,
                idempotency_key.clone(),
                enqueue,
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
                log_action("Enqueued", "invocation");
                self.ctx
                    .log_handler()
                    .log_view(&InvokeResultView::new_enqueue(idempotency_key));
            }
        }

        Ok(())
    }

    async fn cmd_stream(
        &self,
        worker_name: WorkerNameArg,
        stream_args: StreamArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Connecting",
            format!("to worker {}", format_worker_name_match(&worker_name_match)),
        );

        let connection = WorkerConnection::new(
            self.ctx.worker_service_url().clone(),
            self.ctx.auth_token().await?,
            component.versioned_component_id.component_id,
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

    async fn cmd_simulate_crash(&self, worker_name: WorkerNameArg) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Simulating crash",
            format!(
                "for worker {}",
                format_worker_name_match(&worker_name_match)
            ),
        );

        self.interrupt_worker(&component, &worker_name, true)
            .await?;

        log_action(
            "Simulated crash",
            format!(
                "for worker {}",
                format_worker_name_match(&worker_name_match)
            ),
        );

        Ok(())
    }

    async fn cmd_oplog(
        &self,
        worker_name: WorkerNameArg,
        from: Option<u64>,
        query: Option<String>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
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
                        &component.versioned_component_id.component_id,
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
                        .map(|entry| (entry.oplog_index, entry.entry)),
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
        worker_name: WorkerNameArg,
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
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Reverting",
            format!("worker {}", format_worker_name_match(&worker_name_match)),
        );

        let clients = self.ctx.golem_clients().await?;

        {
            let target = {
                if let Some(last_oplog_index) = last_oplog_index {
                    RevertWorkerTargetCloud::RevertToOplogIndex(RevertToOplogIndexCloud {
                        last_oplog_index,
                    })
                } else if let Some(number_of_invocations) = number_of_invocations {
                    RevertWorkerTargetCloud::RevertLastInvocations(RevertLastInvocationsCloud {
                        number_of_invocations,
                    })
                } else {
                    bail!("Expected either last_oplog_index or number_of_invocations")
                }
            };

            clients
                .worker
                .revert_worker(
                    &component.versioned_component_id.component_id,
                    &worker_name.0,
                    &target,
                )
                .await
                .map(|_| ())
                .map_service_error()?
        }

        log_action(
            "Reverted",
            format!("worker {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn cmd_cancel_invocation(
        &self,
        worker_name: WorkerNameArg,
        idempotency_key: IdempotencyKey,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_warn_action(
            "Canceling invocation",
            format!(
                "for worker {} using idempotency key: {}",
                format_worker_name_match(&worker_name_match),
                idempotency_key.0.log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;

        let canceled = clients
            .worker
            .cancel_invocation(
                &component.versioned_component_id.component_id,
                &worker_name.0,
                &idempotency_key.0,
            )
            .await
            .map(|result| (result.canceled))
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
        component_name: Option<ComponentName>,
        filters: Vec<String>,
        scan_cursor: Option<ScanCursor>,
        max_count: Option<u64>,
        precise: bool,
    ) -> anyhow::Result<()> {
        let selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

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
    }

    async fn cmd_interrupt(&self, worker_name: WorkerNameArg) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Interrupting",
            format!("worker {}", format_worker_name_match(&worker_name_match)),
        );

        self.interrupt_worker(&component, &worker_name, false)
            .await?;

        log_action(
            "Interrupted",
            format!("worker {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn cmd_resume(&self, worker_name: WorkerNameArg) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_action(
            "Resuming",
            format!("worker {}", format_worker_name_match(&worker_name_match)),
        );

        self.resume_worker(&component, &worker_name).await?;

        log_action(
            "Resumed",
            format!("worker {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn cmd_update(
        &self,
        worker_name: WorkerNameArg,
        mode: WorkerUpdateMode,
        target_version: Option<u64>,
        await_update: bool,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        let target_version = match target_version {
            Some(target_version) => target_version,
            None => {
                let Some(latest_version) = self
                    .ctx
                    .component_handler()
                    .latest_component_version_by_id(component.versioned_component_id.component_id)
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
            component.versioned_component_id.component_id,
            &worker_name.0,
            mode,
            target_version,
            await_update,
        )
        .await?;

        Ok(())
    }

    async fn cmd_get(&self, worker_name: WorkerNameArg) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let result = {
            let result = clients
                .worker
                .get_worker_metadata(
                    &component.versioned_component_id.component_id,
                    &worker_name.0,
                )
                .await
                .map_service_error()?;

            WorkerMetadata::from(worker_name_match.component_name, result)
        };

        self.ctx
            .log_handler()
            .log_view(&WorkerGetView::from_metadata(result, true));

        Ok(())
    }

    async fn cmd_delete(&self, worker_name: WorkerNameArg) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;
        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;
        let (component, worker_name) = self
            .component_by_worker_name_match(&worker_name_match)
            .await?;

        log_warn_action(
            "Deleting",
            format!("worker {}", format_worker_name_match(&worker_name_match)),
        );

        self.delete(
            component.versioned_component_id.component_id,
            &worker_name.0,
        )
        .await?;

        log_action(
            "Deleted",
            format!("worker {}", format_worker_name_match(&worker_name_match)),
        );

        Ok(())
    }

    async fn new_worker(
        &self,
        component_id: Uuid,
        worker_name: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .launch_new_worker(
                &component_id,
                &WorkerCreationRequestCloud {
                    name: worker_name,
                    args,
                    env,
                    wasi_config_vars: WasiConfigVars::default(),
                },
            )
            .await
            .map(|_| ())
            .map_service_error()
    }

    pub async fn invoke_worker(
        &self,
        component: &Component,
        worker_name: Option<&WorkerName>,
        function_name: &str,
        arguments: Vec<OptionallyValueAndTypeJson>,
        idempotency_key: IdempotencyKey,
        enqueue: bool,
        stream_args: Option<StreamArgs>,
    ) -> anyhow::Result<Option<InvokeResult>> {
        let mut connect_handle = match &worker_name {
            Some(worker_name) => match stream_args {
                Some(stream_args) => {
                    let connection = WorkerConnection::new(
                        self.ctx.worker_service_url().clone(),
                        self.ctx.auth_token().await?,
                        component.versioned_component_id.component_id,
                        worker_name.0.clone(),
                        stream_args.into(),
                        self.ctx.allow_insecure(),
                        self.ctx.format(),
                        if enqueue {
                            None
                        } else {
                            Some(golem_common::model::IdempotencyKey::new(
                                idempotency_key.0.clone(),
                            ))
                        },
                    )
                    .await?;
                    Some(tokio::task::spawn(
                        async move { connection.run_forever().await },
                    ))
                }
                None => None,
            },
            None => None,
        };

        let clients = self.ctx.golem_clients().await?;

        let result = match &worker_name {
            Some(worker_name) => {
                if enqueue {
                    clients
                        .worker
                        .invoke_function(
                            &component.versioned_component_id.component_id,
                            &worker_name.0,
                            Some(&idempotency_key.0),
                            function_name,
                            &InvokeParametersCloud { params: arguments },
                        )
                        .await
                        .map_service_error()?;
                    None
                } else {
                    Some(
                        clients
                            .worker_invoke
                            .invoke_and_await_function(
                                &component.versioned_component_id.component_id,
                                &worker_name.0,
                                Some(&idempotency_key.0),
                                function_name,
                                &InvokeParametersCloud { params: arguments },
                            )
                            .await
                            .map_service_error()?,
                    )
                }
            }
            None => {
                if enqueue {
                    clients
                        .worker
                        .invoke_function_without_name(
                            &component.versioned_component_id.component_id,
                            Some(&idempotency_key.0),
                            function_name,
                            &InvokeParametersCloud { params: arguments },
                        )
                        .await
                        .map_service_error()?;
                    None
                } else {
                    Some(
                        clients
                            .worker_invoke
                            .invoke_and_await_function_without_name(
                                &component.versioned_component_id.component_id,
                                Some(&idempotency_key.0),
                                function_name,
                                &InvokeParametersCloud { params: arguments },
                            )
                            .await
                            .map_service_error()?,
                    )
                }
            }
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
        component_id: Uuid,
        update_mode: WorkerUpdateMode,
        target_version: u64,
        await_update: bool,
    ) -> anyhow::Result<TryUpdateAllWorkersResult> {
        let (workers, _) = self
            .list_component_workers(component_name, component_id, None, None, None, false)
            .await?;

        if workers.is_empty() {
            log_warn_action(
                "Skipping",
                format!("updating workers for component {component_name}, no workers found"),
            );
            return Ok(TryUpdateAllWorkersResult::default());
        }

        log_action(
            "Updating",
            format!(
                "all workers ({}) for component {} to version {}",
                workers.len().to_string().log_color_highlight(),
                component_name.0.blue().bold(),
                target_version.to_string().log_color_highlight()
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
                    target_version,
                    false,
                )
                .await;

            match result {
                Ok(_) => {
                    update_results.triggered.push(WorkerUpdateAttempt {
                        component_name: component_name.clone(),
                        target_version,
                        worker_name: worker.worker_id.worker_name.as_str().into(),
                        error: None,
                    });
                }
                Err(error) => {
                    update_results.triggered.push(WorkerUpdateAttempt {
                        component_name: component_name.clone(),
                        target_version,
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
                        target_version,
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
        update_mode: WorkerUpdateMode,
        target_version: u64,
        await_update: bool,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Triggering update",
            format!(
                "for worker {}/{} to version {} using {} update mode",
                component_name.0.bold().blue(),
                worker_name.bold().green(),
                target_version.to_string().log_color_highlight(),
                update_mode.to_string().log_color_highlight()
            ),
        );

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .worker
            .update_worker(
                &component_id,
                worker_name,
                &UpdateWorkerRequestCloud {
                    mode: match update_mode {
                        WorkerUpdateMode::Automatic => {
                            golem_client::model::WorkerUpdateMode::Automatic
                        }
                        WorkerUpdateMode::Manual => golem_client::model::WorkerUpdateMode::Manual,
                    },
                    target_version,
                },
            )
            .await
            .map(|_| ())
            .map_service_error();

        match result {
            Ok(_) => {
                log_action("Triggered update", "");

                if await_update {
                    self.await_update_result(&component_id, worker_name, target_version)
                        .await?;
                }

                Ok(())
            }
            Err(error) => {
                log_error_action("Failed", "to trigger update for worker, error:");
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
        target_version: u64,
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
                    log_action("Worker update", "is still pending");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                } else if let Some(success) = latest_success {
                    log_action(
                        "Worker update",
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
                        "Worker update",
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
                        "Worker update",
                        "is not pending anymore, but no outcome has been found",
                    );
                    return Err(anyhow ! (
                "Unexpected worker state: update is not pending anymore, but no outcome has been found"
                ));
                }
            }
        }
    }

    pub async fn redeploy_component_workers(
        &self,
        component_name: &ComponentName,
        component_id: Uuid,
    ) -> anyhow::Result<()> {
        let (workers, _) = self
            .list_component_workers(component_name, component_id, None, None, None, false)
            .await?;

        if workers.is_empty() {
            log_warn_action(
                "Skipping",
                format!("redeploying workers for component {component_name}, no workers found"),
            );
            return Ok(());
        }

        log_action(
            "Redeploying",
            format!(
                "all workers ({}) for component {}",
                workers.len().to_string().log_color_highlight(),
                component_name.0.blue().bold(),
            ),
        );
        let _indent = LogIndent::new();

        if !self
            .ctx
            .interactive_handler()
            .confirm_redeploy_workers(workers.len())?
        {
            bail!(NonSuccessfulExit);
        }

        for worker in workers {
            self.redeploy_worker(component_name, worker).await?;
        }

        Ok(())
    }

    async fn redeploy_worker(
        &self,
        component_name: &ComponentName,
        worker_metadata: WorkerMetadata,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Redeploying",
            format!(
                "worker {}/{} to latest version",
                component_name.0.bold().blue(),
                worker_metadata.worker_id.worker_name.bold().green(),
            ),
        );
        let _indent = LogIndent::new();

        log_warn_action(
            "Deleting",
            format!(
                "worker {}/{}",
                component_name.0.bold().blue(),
                worker_metadata.worker_id.worker_name.bold().green(),
            ),
        );
        self.delete(
            worker_metadata.worker_id.component_id.0,
            &worker_metadata.worker_id.worker_name,
        )
        .await?;
        log_action("Deleted", "worker");

        log_action(
            "Recreating",
            format!(
                "worker {}/{}",
                component_name.0.bold().blue(),
                worker_metadata.worker_id.worker_name.bold().green(),
            ),
        );
        self.new_worker(
            worker_metadata.worker_id.component_id.0,
            worker_metadata.worker_id.worker_name,
            worker_metadata.args,
            worker_metadata.env,
        )
        .await?;
        log_action("Recreated", "worker");

        Ok(())
    }

    pub async fn list_component_workers(
        &self,
        component_name: &ComponentName,
        component_id: Uuid,
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
                        &component_id,
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
    ) -> anyhow::Result<(Component, WorkerName)> {
        let Some(worker_name) = &worker_name_match.worker_name else {
            log_error("Worker name is required");
            logln("");
            log_text_view(&WorkerNameHelp);
            logln("");
            bail!(NonSuccessfulExit);
        };

        let component = self
            .ctx
            .component_handler()
            .component(
                worker_name_match.project.as_ref(),
                (&worker_name_match.component_name).into(),
                worker_name_match.worker_name.as_ref().map(|wn| wn.into()),
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

        Ok((component, worker_name.clone()))
    }

    async fn resume_worker(
        &self,
        component: &Component,
        worker_name: &WorkerName,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .resume_worker(
                &component.versioned_component_id.component_id,
                &worker_name.0,
            )
            .await
            .map(|_| ())
            .map_service_error()?;

        Ok(())
    }

    async fn interrupt_worker(
        &self,
        component: &Component,
        worker_name: &WorkerName,
        recover_immediately: bool,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .worker
            .interrupt_worker(
                &component.versioned_component_id.component_id,
                &worker_name.0,
                Some(recover_immediately),
            )
            .await
            .map(|_| ())
            .map_service_error()?;

        Ok(())
    }

    pub async fn match_worker_name(
        &self,
        worker_name: WorkerName,
    ) -> anyhow::Result<WorkerNameMatch> {
        fn to_opt_worker_name(worker_name: String) -> Option<WorkerName> {
            (worker_name != "-").then(|| worker_name.into())
        }

        let segments = worker_name.0.split("/").collect::<Vec<&str>>();
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
                                    "Switch to a different directory with only one component or specify the full or partial component name as part of the worker name!",
                                );
                            logln("");
                            log_text_view(&WorkerNameHelp);
                            logln("");
                            log_text_view(&AvailableComponentNamesHelp(
                                app_ctx.application.component_names().cloned().collect(),
                            ));
                            bail!(NonSuccessfulExit);
                        }

                        Ok(WorkerNameMatch {
                            account: None,
                            project: None,
                            component_name_match_kind: ComponentNameMatchKind::AppCurrentDir,
                            component_name: selected_component_names
                                .iter()
                                .next()
                                .unwrap()
                                .as_str()
                                .into(),
                            worker_name: to_opt_worker_name(worker_name),
                        })
                    }
                    None => {
                        logln("");
                        log_error("Cannot infer the component name for the worker as the current directory is not part of an application.");
                        logln("");
                        logln("Switch to an application directory or specify the full component name as part of the worker name!");
                        logln("");
                        log_text_view(&WorkerNameHelp);
                        // TODO: hint for deployed component names?
                        bail!(NonSuccessfulExit);
                    }
                }
            }
            // [ACCOUNT]/[PROJECT]/<COMPONENT>/<WORKER>
            2..=4 => {
                fn empty_checked<'a>(name: &'a str, value: &'a str) -> anyhow::Result<&'a str> {
                    if value.is_empty() {
                        log_error(format!("Missing {name} part in worker name!"));
                        logln("");
                        log_text_view(&ComponentNameHelp);
                        bail!(NonSuccessfulExit);
                    }
                    Ok(value)
                }

                fn empty_checked_account(value: &str) -> anyhow::Result<&str> {
                    empty_checked("account", value)
                }

                fn empty_checked_project(value: &str) -> anyhow::Result<&str> {
                    empty_checked("project", value)
                }

                fn empty_checked_component(value: &str) -> anyhow::Result<&str> {
                    empty_checked("component", value)
                }

                fn empty_checked_worker(value: &str) -> anyhow::Result<&str> {
                    empty_checked("worker", value)
                }

                let (account_email, project_name, component_name, worker_name): (
                    Option<String>,
                    Option<ProjectName>,
                    ComponentName,
                    String,
                ) = match segments.len() {
                    2 => (
                        None,
                        None,
                        empty_checked_component(segments[0])?.into(),
                        empty_checked_component(segments[1])?.into(),
                    ),
                    3 => (
                        None,
                        Some(empty_checked_project(segments[0])?.into()),
                        empty_checked_component(segments[1])?.into(),
                        empty_checked_component(segments[2])?.into(),
                    ),
                    4 => (
                        Some(empty_checked_account(segments[0])?.into()),
                        Some(empty_checked_project(segments[1])?.into()),
                        empty_checked_component(segments[2])?.into(),
                        empty_checked_worker(segments[3])?.into(),
                    ),
                    other => panic!("Unexpected segment count: {other}"),
                };

                if worker_name.is_empty() {
                    logln("");
                    log_error("Missing component part in worker name!");
                    logln("");
                    log_text_view(&WorkerNameHelp);
                    bail!(NonSuccessfulExit);
                }

                let account = if let Some(ref account_email) = account_email {
                    Some(
                        self.ctx
                            .select_account_by_email_or_error(account_email)
                            .await?,
                    )
                } else {
                    None
                };

                let project = match (account_email, project_name) {
                    (Some(account_email), Some(project_name)) => {
                        self.ctx
                            .cloud_project_handler()
                            .opt_select_project(Some(&ProjectReference::WithAccount {
                                account_email,
                                project_name,
                            }))
                            .await?
                    }
                    (None, Some(project_name)) => {
                        self.ctx
                            .cloud_project_handler()
                            .opt_select_project(Some(&ProjectReference::JustName(project_name)))
                            .await?
                    }
                    _ => None,
                };

                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.opt()?;
                match app_ctx {
                    Some(app_ctx) => {
                        let fuzzy_search = FuzzySearch::new(
                            app_ctx.application.component_names().map(|cn| cn.as_str()),
                        );
                        match fuzzy_search.find(&component_name.0) {
                            Ok(match_) => {
                                log_fuzzy_match(&match_);
                                Ok(WorkerNameMatch {
                                    account,
                                    project,
                                    component_name_match_kind: ComponentNameMatchKind::App,
                                    component_name: match_.option.into(),
                                    worker_name: to_opt_worker_name(worker_name),
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
                                        account,
                                        project,
                                        component_name_match_kind: ComponentNameMatchKind::Unknown,
                                        component_name,
                                        worker_name: to_opt_worker_name(worker_name),
                                    })
                                }
                            },
                        }
                    }
                    None => Ok(WorkerNameMatch {
                        account,
                        project,
                        component_name_match_kind: ComponentNameMatchKind::Unknown,
                        component_name,
                        worker_name: to_opt_worker_name(worker_name),
                    }),
                }
            }
            _ => {
                logln("");
                log_error(format!(
                    "Failed to parse worker name: {}",
                    worker_name.0.log_color_error_highlight()
                ));
                logln("");
                log_text_view(&WorkerNameHelp);
                bail!(NonSuccessfulExit);
            }
        }
    }
}

fn wave_args_to_invoke_args(
    component: &Component,
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
