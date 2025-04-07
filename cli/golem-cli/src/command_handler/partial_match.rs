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

use crate::command::{builtin_app_subcommands, GolemCliCommandPartialMatch, GolemCliGlobalFlags};
use crate::command_handler::Handlers;
use crate::config::Config;
use crate::context::Context;
use crate::error::{ContextInitHintError, HintError};
use crate::log::{log_action, logln, LogColorize};
use crate::model::component::show_exported_functions;
use crate::model::text::fmt::{log_error, log_text_view, NestedTextViewIndent};
use crate::model::text::help::{AvailableFunctionNamesHelp, WorkerNameHelp};
use crate::model::{ComponentNameMatchKind, Format};
use crate::wasm_rpc_stubgen::commands::app::{ComponentSelectMode, DynamicHelpSections};
use colored::Colorize;
use std::sync::Arc;

pub struct ErrorHandler {
    ctx: Arc<Context>,
}

impl ErrorHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_partial_match(
        &mut self,
        partial_match: GolemCliCommandPartialMatch,
    ) -> anyhow::Result<()> {
        match partial_match {
            GolemCliCommandPartialMatch::AppHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections {
                        components: true,
                        custom_commands: true,
                        builtin_commands: builtin_app_subcommands(),
                    })?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::AppNewMissingLanguage => {
                self.ctx.app_handler().log_languages_help();
                Ok(())
            }
            GolemCliCommandPartialMatch::AppMissingSubcommandHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections {
                        components: true,
                        custom_commands: true,
                        builtin_commands: builtin_app_subcommands(),
                    })?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::ComponentHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections {
                        components: true,
                        custom_commands: false,
                        builtin_commands: builtin_app_subcommands(),
                    })?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::ComponentMissingSubcommandHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections {
                        components: true,
                        custom_commands: false,
                        builtin_commands: builtin_app_subcommands(),
                    })?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::ComponentNewMissingTemplate => {
                self.ctx.app_handler().log_templates_help(None, None);
                Ok(())
            }
            GolemCliCommandPartialMatch::WorkerHelp => {
                // TODO
                Ok(())
            }
            GolemCliCommandPartialMatch::WorkerInvokeMissingFunctionName { worker_name } => {
                self.ctx.silence_app_context_init().await;
                logln("");
                log_action(
                    "Checking",
                    format!(
                        "provided worker name: {}",
                        worker_name.0.log_color_highlight()
                    ),
                );
                let worker_name_match = {
                    let _indent = NestedTextViewIndent::new(Format::Text);
                    let worker_name_match = self
                        .ctx
                        .worker_handler()
                        .match_worker_name(worker_name)
                        .await?;

                    let project_formatted = match &worker_name_match.project {
                        Some(project) => format!(
                            " project: {} /",
                            project.project_name.0.log_color_highlight()
                        ),
                        None => "".to_string(),
                    };

                    logln(format!(
                        "[{}]{} component: {} / worker: {}, {}",
                        "ok".green(),
                        project_formatted,
                        worker_name_match.component_name.0.log_color_highlight(),
                        worker_name_match
                            .worker_name
                            .as_ref()
                            .map(|s| s.0.as_str())
                            .unwrap_or("-")
                            .log_color_highlight(),
                        match worker_name_match.component_name_match_kind {
                            ComponentNameMatchKind::AppCurrentDir =>
                                "component was selected based on current dir",
                            ComponentNameMatchKind::App =>
                                "component was selected from current application",
                            ComponentNameMatchKind::Unknown => "",
                        }
                    ));
                    worker_name_match
                };
                logln("");
                if let Ok(Some(component)) = self
                    .ctx
                    .component_handler()
                    .component_by_name(
                        worker_name_match.project.as_ref(),
                        &worker_name_match.component_name,
                        worker_name_match.worker_name.as_ref(),
                    )
                    .await
                {
                    log_text_view(&AvailableFunctionNamesHelp {
                        component_name: worker_name_match.component_name.0,
                        function_names: show_exported_functions(&component.metadata.exports),
                    });
                    logln("");
                }
                Ok(())
            }
            GolemCliCommandPartialMatch::WorkerInvokeMissingWorkerName => {
                logln("");
                log_text_view(&WorkerNameHelp);
                logln("");

                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    app_ctx.log_dynamic_help(&DynamicHelpSections {
                        components: true,
                        custom_commands: false,
                        builtin_commands: builtin_app_subcommands(),
                    })?
                }

                Ok(())
            }
        }
    }

    pub fn handle_hint_errors(&self, hint_error: &HintError) -> anyhow::Result<()> {
        match hint_error {
            HintError::NoApplicationManifestFound => {
                logln("");
                log_error("No application manifest(s) found!");
                logln("");
                logln(format!(
                    "Switch to a directory that contains an application manifest ({}),",
                    "golem.yaml".log_color_highlight()
                ));
                logln(format!(
                    "or create a new application with the '{}' subcommand!",
                    "app new".log_color_highlight(),
                ));
                Ok(())
            }
            HintError::ExpectedCloudProfile => {
                log_error("The requested operation requires using cloud profile!");
                logln("");
                logln("Switch to cloud profile with one of the following options");
                logln(" - use the '--cloud' or '-c' flag");
                logln(" - use 'profile switch cloud' ");
                logln(" - set the GOLEM_PROFILE environment variable to 'cloud'");
                logln("");
                Ok(())
            }
        }
    }

    pub fn handle_context_init_hint_errors(
        global_flags: &GolemCliGlobalFlags,
        hint_error: &ContextInitHintError,
    ) -> anyhow::Result<()> {
        match hint_error {
            ContextInitHintError::ProfileNotFound(profile_name) => {
                log_error(format!(
                    "Profile '{}' not found!",
                    profile_name.0.log_color_highlight()
                ));

                if let Ok(config) = Config::from_dir(&global_flags.config_dir()) {
                    logln("");
                    logln("Available profiles:".log_color_help_group().to_string());
                    for profile_name in config.profiles.keys() {
                        println!("- {}", profile_name);
                    }
                }

                Ok(())
            }
        }
    }
}
