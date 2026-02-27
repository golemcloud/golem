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

use crate::command::{
    builtin_exec_subcommands, help_target_to_command, GolemCliCommandPartialMatch,
    GolemCliGlobalFlags,
};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::{ContextInitHintError, HintError, ShowClapHelpTarget};
use crate::log::Output::Stdout;
use crate::log::{log_action, log_error, logln, set_log_output, LogColorize};
use crate::model::app::{ApplicationComponentSelectMode, DynamicHelpSections};
use crate::model::component::ComponentNameMatchKind;
use crate::model::format::Format;
use crate::model::text::fmt::{log_text_view, NestedTextViewIndent};
use crate::model::text::help::{AvailableFunctionNamesHelp, EnvironmentNameHelp, WorkerNameHelp};
use colored::Colorize;
use indoc::indoc;
use std::sync::Arc;

pub struct ErrorHandler {
    ctx: Arc<Context>,
}

impl ErrorHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_partial_match(
        &self,
        partial_match: GolemCliCommandPartialMatch,
    ) -> anyhow::Result<()> {
        match partial_match {
            GolemCliCommandPartialMatch::AppHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::CurrentDir)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_all(
                        builtin_exec_subcommands(),
                    ))?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::AppMissingSubcommandHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::CurrentDir)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_all(
                        builtin_exec_subcommands(),
                    ))?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::ComponentHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::CurrentDir)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::ComponentMissingSubcommandHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::CurrentDir)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::AgentHelp => {
                // TODO: show agents
                Ok(())
            }
            GolemCliCommandPartialMatch::WorkerInvokeMissingFunctionName { worker_name } => {
                self.ctx.silence_app_context_init().await;
                logln("");
                log_action(
                    "Checking",
                    format!(
                        "provided agent name: {}",
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

                    let environment_formatted = match worker_name_match.environment_reference() {
                        Some(env) => {
                            format!(" environment: {} /", env.to_string().log_color_highlight())
                        }
                        None => "".to_string(),
                    };

                    logln(format!(
                        "[{}]{} component: {} / agent: {}, {}",
                        "ok".green(),
                        environment_formatted,
                        worker_name_match.component_name.0.log_color_highlight(),
                        worker_name_match.worker_name.0.log_color_highlight(),
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
                    .resolve_component(
                        &worker_name_match.environment,
                        &worker_name_match.component_name,
                        Some((&worker_name_match.worker_name).into()),
                    )
                    .await
                {
                    let agent_id = self
                        .ctx
                        .worker_handler()
                        .validate_worker_and_function_names(
                            &component,
                            &worker_name_match.worker_name,
                            None,
                        )?;

                    if let Some((agent_id, agent_type)) = agent_id.as_ref() {
                        log_text_view(&AvailableFunctionNamesHelp::new_agent(
                            &component, agent_id, agent_type,
                        ));
                    }
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
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::CurrentDir)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::ProfileSwitchMissingProfileName => {
                // TODO: atomic: show available profiles

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
                logln(" - use the '--cloud' or '-C' flag");
                logln(" - use 'profile switch cloud' ");
                logln(" - set the GOLEM_PROFILE environment variable to 'cloud'");
                logln("");
                Ok(())
            }
            HintError::EnvironmentHasNoDeployment => {
                log_error(
                    "The requested operation requires an existing deployment for the environment!",
                );
                logln("");
                logln("Use 'golem deploy' for deploying, or select a different environment.");
                logln("");
                Ok(())
            }
            HintError::ShowClapHelp(help_target) => {
                // TODO: we should print to STDERR to match normal help behaviour,
                //       but 'print_long_help' is hardcoded to use STDOUT.
                //       Using 'render_help' is also option, but that loses colors / highlights.
                //       To make it a bit more consistent, we switch to STDOUT for custom help as well.
                help_target_to_command(*help_target).print_long_help()?;
                set_log_output(Stdout);

                match help_target {
                    ShowClapHelpTarget::AppNew => {
                        self.ctx.app_handler().log_languages_help();
                    }
                    ShowClapHelpTarget::ComponentNew => {
                        self.ctx
                            .app_handler()
                            .log_templates_help(None, None, self.ctx.dev_mode());
                    }
                }
                Ok(())
            }
        }
    }

    pub fn handle_context_init_hint_errors(
        _global_flags: &GolemCliGlobalFlags,
        hint_error: &ContextInitHintError,
    ) -> anyhow::Result<()> {
        match hint_error {
            ContextInitHintError::CannotUseShortEnvRefWithLocalOrCloudFlags => {
                log_error("Cannot use short (name only) environment reference with --local or --cloud flags!");
                logln("");
                log_text_view(&EnvironmentNameHelp);
                Ok(())
            }
            ContextInitHintError::CannotSelectEnvironmentWithoutManifest {
                requested_environment_name,
            } => {
                log_error(format!(
                    "Environment '{}' not found!",
                    requested_environment_name.0.log_color_highlight()
                ));

                logln("");

                logln(indoc! { "
                    No application manifests were detected!

                    Switch to a directory that contains an application manifest (golem.yaml),
                    or use the --app_manifest_path flag.
                "});

                Ok(())
            }
            ContextInitHintError::EnvironmentNotFound {
                requested_environment_name,
                manifest_environment_names,
            } => {
                log_error(format!(
                    "Environment '{}' not found!",
                    requested_environment_name.0.log_color_highlight()
                ));

                logln("");

                logln("Available environments:".log_color_help_group().to_string());
                for environment_name in manifest_environment_names {
                    logln(format!("- {}", environment_name.0));
                }

                Ok(())
            }
            ContextInitHintError::ProfileNotFound {
                profile_name,
                available_profile_names,
            } => {
                log_error(format!(
                    "Profile '{}' not found!",
                    profile_name.0.log_color_highlight()
                ));

                logln(
                    "Available profile names:"
                        .log_color_help_group()
                        .to_string(),
                );
                for environment_name in available_profile_names {
                    logln(format!("- {}", environment_name.0));
                }

                Ok(())
            }
        }
    }
}
