// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::cloud::AccountId;
use crate::command::shared_args::{
    NewWorkerArgument, WorkerFunctionArgument, WorkerFunctionName, WorkerNameArg,
};
use crate::command::worker::WorkerSubcommand;
use crate::command_handler::Handlers;
use crate::context::{Context, GolemClients};
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::fuzzy::{Error, FuzzySearch};
use crate::model::component::{function_params_types, show_exported_functions, Component};
use crate::model::invoke_result_view::InvokeResultView;
use crate::model::text::fmt::{format_export, log_error, log_fuzzy_match, log_text_view, log_warn};
use crate::model::text::help::{
    ArgumentError, AvailableComponentNamesHelp, AvailableFunctionNamesHelp, ComponentNameHelp,
    ParameterErrorTableView, WorkerNameHelp,
};
use crate::model::text::worker::{WorkerCreateView, WorkerGetView};
use crate::model::to_oss::ToOss;
use crate::model::{
    ComponentName, ComponentNameMatchKind, IdempotencyKey, ProjectName, WorkerMetadata,
    WorkerMetadataView, WorkerName, WorkerNameMatch, WorkersMetadataResponseView,
};
use anyhow::{anyhow, bail};
use colored::Colorize;
use golem_client::api::WorkerClient as WorkerClientOss;
use golem_client::model::{
    InvokeParameters as InvokeParametersOss, ScanCursor,
    WorkerCreationRequest as WorkerCreationRequestOss,
};
use golem_cloud_client::api::WorkerClient as WorkerClientCloud;
use golem_wasm_rpc::json::OptionallyTypeAnnotatedValueJson;
use golem_wasm_rpc::parse_type_annotated_value;
use golem_wasm_rpc_stubgen::commands::app::ComponentSelectMode;
use golem_wasm_rpc_stubgen::log::{log_action, logln, LogColorize};
use itertools::{EitherOrBoth, Itertools};
use std::sync::Arc;
use uuid::Uuid;

pub struct WorkerCommandHandler {
    ctx: Arc<Context>,
}

impl WorkerCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&mut self, subcommand: WorkerSubcommand) -> anyhow::Result<()> {
        match subcommand {
            WorkerSubcommand::New {
                worker_name,
                arguments,
                env,
            } => self.new_worker(worker_name, arguments, env).await,
            WorkerSubcommand::Invoke {
                worker_name,
                function_name,
                arguments,
                enqueue,
                idempotency_key,
            } => {
                self.invoke(
                    worker_name,
                    &function_name,
                    arguments,
                    enqueue,
                    idempotency_key,
                )
                .await
            }
            WorkerSubcommand::Get { worker_name } => self.get(worker_name).await,
            WorkerSubcommand::List {
                component_name,
                filter: filters,
                scan_cursor,
                max_count,
                precise,
            } => {
                self.list(
                    component_name.component_name,
                    filters,
                    scan_cursor,
                    max_count,
                    precise,
                )
                .await
            }
        }
    }

    async fn new_worker(
        &mut self,
        worker_name: WorkerNameArg,
        arguments: Vec<NewWorkerArgument>,
        env: Vec<(String, String)>,
    ) -> anyhow::Result<()> {
        let worker_name = worker_name.worker_name;

        self.ctx.silence_app_context_init().await;

        let worker_name_match = self.match_worker_name(worker_name).await?;

        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                worker_name_match.project.as_ref(),
                worker_name_match.component_name_match_kind,
                &worker_name_match.component_name,
            )
            .await?;

        // TODO: should we fail on explicit names for ephemeral?
        let worker_name = worker_name_match
            .worker_name
            .unwrap_or_else(|| Uuid::new_v4().to_string().into());

        // TODO: log args / env?
        log_action(
            "Creating",
            format!(
                "new worker {} / {}",
                worker_name_match.component_name.0.blue().bold(),
                worker_name.0.green().bold(),
            ),
        );

        // TODO: should use the returned API response? (like component version)
        let _ = match self.ctx.golem_clients().await? {
            GolemClients::Oss(clients) => clients
                .worker
                .launch_new_worker(
                    &component.versioned_component_id.component_id,
                    &WorkerCreationRequestOss {
                        name: worker_name.0.clone(),
                        args: arguments,
                        env: env.into_iter().collect(),
                    },
                )
                .await
                .map_service_error()?,
            GolemClients::Cloud(_) => {
                todo!()
            }
        };

        logln("");
        self.ctx.log_handler().log_view(&WorkerCreateView {
            component_name: worker_name_match.component_name,
            worker_name: Some(worker_name),
        });

        Ok(())
    }

    async fn invoke(
        &mut self,
        worker_name: WorkerNameArg,
        function_name: &WorkerFunctionName,
        arguments: Vec<WorkerFunctionArgument>,
        enqueue: bool,
        idempotency_key: Option<IdempotencyKey>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        // TODO: should always generate idempotency key if not provided?
        let idempotency_key = idempotency_key.map(|key| {
            if key.0 == "-" {
                let key = IdempotencyKey::new();
                log_action(
                    "Using",
                    format!(
                        "auto generated idempotency key: {}",
                        key.0.log_color_highlight()
                    ),
                );
                key.0
            } else {
                log_action(
                    "Using",
                    format!("requested idempotency key: {}", key.0.log_color_highlight()),
                );
                key.0
            }
        });

        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;

        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                worker_name_match.project.as_ref(),
                worker_name_match.component_name_match_kind,
                &worker_name_match.component_name,
            )
            .await?;

        let component_functions = show_exported_functions(&component.metadata.exports);
        let fuzzy_search = FuzzySearch::new(component_functions.iter().map(|s| s.as_str()));
        let function_name = match fuzzy_search.find(function_name) {
            Ok(match_) => {
                log_fuzzy_match(&match_);
                match_.option
            }
            Err(error) => {
                // TODO: extract common ambiguous messages?
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
                    "invocation for worker {} / {} / {} ",
                    worker_name_match.component_name.0.blue().bold(),
                    worker_name_match
                        .worker_name
                        .as_ref()
                        .map(|wn| wn.0.as_str())
                        .unwrap_or("-")
                        .green()
                        .bold(),
                    format_export(&function_name)
                ),
            );
        } else {
            log_action(
                "Invoking",
                format!(
                    "worker {} / {} / {} ",
                    worker_name_match.component_name.0.blue().bold(),
                    worker_name_match
                        .worker_name
                        .as_ref()
                        .map(|wn| wn.0.as_str())
                        .unwrap_or("-")
                        .green()
                        .bold(),
                    format_export(&function_name)
                ),
            );
        }

        let arguments = wave_args_to_invoke_args(&component, &function_name, arguments)?;

        let result_view = match self.ctx.golem_clients().await? {
            GolemClients::Oss(clients) => {
                let result = match worker_name_match.worker_name {
                    Some(worker_name) => {
                        if enqueue {
                            clients
                                .worker
                                .invoke_function(
                                    &component.versioned_component_id.component_id,
                                    worker_name.0.as_str(),
                                    idempotency_key.as_deref(),
                                    function_name.as_str(),
                                    &InvokeParametersOss { params: arguments },
                                )
                                .await
                                .map_service_error()?;
                            None
                        } else {
                            Some(
                                clients
                                    .worker
                                    .invoke_and_await_function(
                                        &component.versioned_component_id.component_id,
                                        worker_name.0.as_str(),
                                        idempotency_key.as_deref(),
                                        function_name.as_str(),
                                        &InvokeParametersOss { params: arguments },
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
                                    idempotency_key.as_deref(),
                                    function_name.as_str(),
                                    &InvokeParametersOss { params: arguments },
                                )
                                .await
                                .map_service_error()?;
                            None
                        } else {
                            Some(
                                clients
                                    .worker
                                    .invoke_and_await_function_without_name(
                                        &component.versioned_component_id.component_id,
                                        idempotency_key.as_deref(),
                                        function_name.as_str(),
                                        &InvokeParametersOss { params: arguments },
                                    )
                                    .await
                                    .map_service_error()?,
                            )
                        }
                    }
                };
                // TODO: handle json format and include idempotency key in it
                result
                    .map(|result| {
                        InvokeResultView::try_parse_or_json(
                            result,
                            &component,
                            function_name.as_str(),
                        )
                    })
                    .transpose()?
            }
            GolemClients::Cloud(_) => {
                todo!()
            }
        };

        match result_view {
            Some(view) => {
                logln("");
                self.ctx.log_handler().log_view(&view);
            }
            None => {
                log_action("Enqueued", "invocation");
            }
        }

        Ok(())
    }

    async fn get(&mut self, worker_name: WorkerNameArg) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let worker_name_match = self.match_worker_name(worker_name.worker_name).await?;

        let component = self
            .ctx
            .component_handler()
            .component_by_name(
                worker_name_match.project.as_ref(),
                &worker_name_match.component_name,
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

        let Some(worker_name) = worker_name_match.worker_name else {
            // TODO: do not allow ephemeral ones
            log_error("Worker name is required");
            logln("");
            log_text_view(&WorkerNameHelp);
            logln("");
            bail!(NonSuccessfulExit);
        };

        let result = match self.ctx.golem_clients().await? {
            GolemClients::Oss(clients) => {
                let result = clients
                    .worker
                    .get_worker_metadata(
                        &component.versioned_component_id.component_id,
                        &worker_name.0,
                    )
                    .await
                    .map_service_error()?;

                WorkerMetadata::from_oss(worker_name_match.component_name, result)
            }
            GolemClients::Cloud(clients) => {
                let result = clients
                    .worker
                    .get_worker_metadata(
                        &component.versioned_component_id.component_id,
                        &worker_name.0,
                    )
                    .await
                    .map_service_error()?;

                WorkerMetadata::from_cloud(worker_name_match.component_name, result)
            }
        };

        self.ctx
            .log_handler()
            .log_view(&WorkerGetView::from(result));

        Ok(())
    }

    async fn list(
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
            .must_select_components_by_app_or_name(component_name.as_ref())
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

        let scan_cursor = scan_cursor.as_ref().map(scan_cursor_to_string);

        let mut view = WorkersMetadataResponseView::default();

        for component_name in &selected_components.component_names {
            match self
                .ctx
                .component_handler()
                .component_by_name(selected_components.project.as_ref(), component_name)
                .await?
            {
                Some(component) => {
                    let mut current_scan_cursor = scan_cursor.clone();
                    loop {
                        let result_cursor = match self.ctx.golem_clients().await? {
                            GolemClients::Oss(clients) => {
                                let results = clients
                                    .worker
                                    .get_workers_metadata(
                                        &component.versioned_component_id.component_id,
                                        Some(&filters),
                                        current_scan_cursor.as_deref(),
                                        max_count.or(Some(self.ctx.http_batch_size())),
                                        Some(precise),
                                    )
                                    .await
                                    .map_service_error()?;

                                view.workers.extend(results.workers.into_iter().map(|meta| {
                                    WorkerMetadataView::from(WorkerMetadata::from_oss(
                                        component_name.clone(),
                                        meta,
                                    ))
                                }));

                                results.cursor
                            }
                            GolemClients::Cloud(clients) => {
                                let results = clients
                                    .worker
                                    .get_workers_metadata(
                                        &component.versioned_component_id.component_id,
                                        Some(&filters),
                                        current_scan_cursor.as_deref(),
                                        max_count.or(Some(self.ctx.http_batch_size())),
                                        Some(precise),
                                    )
                                    .await
                                    .map_service_error()?;

                                view.workers.extend(results.workers.into_iter().map(|meta| {
                                    WorkerMetadataView::from(WorkerMetadata::from_cloud(
                                        component_name.clone(),
                                        meta,
                                    ))
                                }));

                                results.cursor.to_oss()
                            }
                        };

                        match result_cursor {
                            Some(next_cursor) => {
                                if max_count.is_none() {
                                    current_scan_cursor = Some(scan_cursor_to_string(&next_cursor));
                                } else {
                                    view.cursors.insert(
                                        component_name.to_string().clone(),
                                        scan_cursor_to_string(&next_cursor),
                                    );
                                    break;
                                }
                            }
                            None => {
                                break;
                            }
                        }
                    }
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

    pub async fn match_worker_name(
        &mut self,
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
                    .opt_select_components(vec![], &ComponentSelectMode::CurrentDir)
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
                        log_error(
                            "Cannot infer the component name for the worker as the current directory is not part of an application."
                        );
                        logln("");
                        logln(
                            "Switch to an application directory or specify the full component name as part of the worker name!",
                        );
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
                        log_error(format!("Missing {} part in worker name!", name));
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

                let (account_id, project_name, component_name, worker_name): (
                    Option<AccountId>,
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
                    other => panic!("Unexpected segment count: {}", other),
                };

                if worker_name.is_empty() {
                    logln("");
                    log_error("Missing component part in worker name!");
                    logln("");
                    log_text_view(&WorkerNameHelp);
                    logln("");
                    bail!(NonSuccessfulExit);
                }

                let project = self
                    .ctx
                    .cloud_project_handler()
                    .opt_select_project(account_id.as_ref(), project_name.as_ref())
                    .await?;

                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ComponentSelectMode::All)
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
) -> anyhow::Result<Vec<OptionallyTypeAnnotatedValueJson>> {
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
        .map(|(wave, typ)| parse_type_annotated_value(typ, wave))
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

fn scan_cursor_to_string(cursor: &ScanCursor) -> String {
    format!("{}/{}", cursor.layer, cursor.cursor)
}
