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
    builtin_app_subcommands, help_target_to_command, GolemCliCommandPartialMatch,
    GolemCliGlobalFlags,
};
use crate::command_handler::Handlers;
use crate::config::{Config, ProfileName};
use crate::context::Context;
use crate::error::{ContextInitHintError, HintError, ShowClapHelpTarget};
use crate::log::Output::Stdout;
use crate::log::{log_action, logln, set_log_output, LogColorize};
use crate::model::app::{ApplicationComponentSelectMode, DynamicHelpSections};
use crate::model::component::show_exported_functions;
use crate::model::text::fmt::{log_error, log_text_view, NestedTextViewIndent};
use crate::model::text::help::{AvailableFunctionNamesHelp, WorkerNameHelp};
use crate::model::{ComponentNameMatchKind, Format};
use colored::Colorize;
use std::collections::BTreeSet;
use std::path::Path;
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
                let profile = self.ctx.profile_name().clone();

                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_all(
                        profile,
                        builtin_app_subcommands(),
                    ))?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::AppMissingSubcommandHelp => {
                let profile = self.ctx.profile_name().clone();

                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_all(
                        profile,
                        builtin_app_subcommands(),
                    ))?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::ComponentHelp => {
                self.ctx.silence_app_context_init().await;
                self.ctx
                    .app_handler()
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::All)
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
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    logln("");
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?
                }

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
                            project.project_ref.to_string().log_color_highlight()
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
                    .component(
                        worker_name_match.project.as_ref(),
                        (&worker_name_match.component_name).into(),
                        worker_name_match.worker_name.as_ref().map(|wn| wn.into()),
                    )
                    .await
                {
                    log_text_view(&AvailableFunctionNamesHelp {
                        component_name: worker_name_match.component_name.0,
                        function_names: show_exported_functions(component.metadata.exports(), true),
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
                    .opt_select_components(vec![], &ApplicationComponentSelectMode::All)
                    .await?;

                let app_ctx = self.ctx.app_context_lock().await;
                if let Some(app_ctx) = app_ctx.opt()? {
                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?
                }

                Ok(())
            }
            GolemCliCommandPartialMatch::ProfileSwitchMissingProfileName => {
                show_available_profiles_help(self.ctx.config_dir(), vec![].as_slice());

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
                        self.ctx.app_handler().log_templates_help(None, None);
                    }
                    ShowClapHelpTarget::ComponentAddDependency => {}
                }
                Ok(())
            }
        }
    }

    pub fn handle_context_init_hint_errors(
        global_flags: &GolemCliGlobalFlags,
        hint_error: &ContextInitHintError,
    ) -> anyhow::Result<()> {
        match hint_error {
            ContextInitHintError::ProfileNotFound {
                profile_name,
                manifest_profile_names,
            } => {
                log_error(format!(
                    "Profile '{}' not found!",
                    profile_name.0.log_color_highlight()
                ));

                show_available_profiles_help(&global_flags.config_dir(), manifest_profile_names);

                Ok(())
            }
        }
    }
}

fn show_available_profiles_help(config_dir: &Path, manifest_profile_names: &[ProfileName]) {
    let Ok(config) = Config::from_dir(config_dir) else {
        return;
    };

    let profile_names = {
        let mut profile_names = BTreeSet::from_iter(manifest_profile_names.iter().cloned());
        profile_names.extend(config.profiles.keys().cloned());
        profile_names
    };

    logln("");
    logln("Available profiles:".log_color_help_group().to_string());
    for profile_name in profile_names {
        logln(format!("- {profile_name}"));
    }
}
