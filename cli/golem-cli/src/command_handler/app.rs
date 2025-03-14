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

use crate::command::app::AppSubcommand;
use crate::command::shared_args::{BuildArgs, ComponentTemplatePositionalArgs};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::{HintError, NonSuccessfulExit};
use crate::fuzzy::{Error, FuzzySearch};
use crate::model::text::fmt::{log_error, log_fuzzy_matches, log_text_view};
use crate::model::text::help::AvailableComponentNamesHelp;
use crate::model::ComponentName;
use anyhow::{anyhow, bail};
use colored::Colorize;
use golem_examples::add_component_by_example;
use golem_examples::model::{
    ComposableAppGroupName, Example, ExampleName, GuestLanguage, PackageName,
};
use golem_wasm_rpc_stubgen::commands::app::{ComponentSelectMode, DynamicHelpSections};
use golem_wasm_rpc_stubgen::fs;
use golem_wasm_rpc_stubgen::log::{log_action, logln, LogColorize, LogIndent, LogOutput, Output};
use itertools::Itertools;
use std::path::PathBuf;
use std::sync::Arc;

pub struct AppCommandHandler {
    ctx: Arc<Context>,
}

impl AppCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&mut self, subcommand: AppSubcommand) -> anyhow::Result<()> {
        match subcommand {
            AppSubcommand::New {
                application_name,
                template_name,
            } => self.new_app(&application_name, template_name).await,
            AppSubcommand::Build {
                component_name,
                build: build_args,
            } => {
                self.build(
                    component_name.component_name,
                    Some(build_args),
                    &ComponentSelectMode::All,
                )
                .await
            }
            AppSubcommand::Deploy {
                component_name,
                force_build,
            } => {
                self.ctx
                    .component_handler()
                    .deploy(
                        self.ctx
                            .cloud_project_handler()
                            .opt_select_project(None, None)
                            .await?
                            .as_ref(),
                        component_name.component_name,
                        Some(force_build),
                        &ComponentSelectMode::All,
                    )
                    .await
            }
            AppSubcommand::Clean { component_name } => {
                self.clean(component_name.component_name, &ComponentSelectMode::All)
                    .await
            }
            AppSubcommand::CustomCommand(command) => {
                if command.len() != 1 {
                    bail!(
                        "Expected exactly one custom subcommand, got: {}",
                        command.join(" ").log_color_error_highlight()
                    );
                }

                let app_ctx = self.ctx.app_context_lock().await;
                app_ctx.some_or_err()?.custom_command(&command[0])?;

                Ok(())
            }
        }
    }

    async fn new_app(
        &mut self,
        application_name: &str,
        template_name: ComponentTemplatePositionalArgs,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.opt();
            match app_ctx {
                Ok(None) => {
                    // NOP, there is no app
                }
                _ => {
                    log_error("The current directory is part of an existing application.");
                    logln("");
                    logln("Switch to a directory that is not part of an application or use");
                    logln(
                        "'the component new' command to create a component in the current application.",
                    );
                    bail!(NonSuccessfulExit);
                }
            }
        }

        // Unload app context, so we can reload it after the app is created
        self.ctx.unload_app_context().await;

        let app_dir = PathBuf::from(&application_name);
        if app_dir.exists() {
            bail!(
                "Application directory already exists: {}",
                app_dir.log_color_error_highlight()
            );
        }

        fs::create_dir_all(&app_dir)?;
        log_action(
            "Created",
            format!(
                "application directory: {}",
                app_dir.display().to_string().log_color_highlight()
            ),
        );

        let templates = template_name
            .component_template
            .iter()
            .map(|template_name| {
                self.get_template(template_name).map(|(common, component)| {
                    (
                        PackageName::from_string(format!(
                            "app:{}",
                            template_name.to_string().to_lowercase()
                        ))
                        .unwrap(),
                        common,
                        component,
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        {
            let _indent = LogIndent::new();
            for (component_name, common_template, component_template) in templates {
                match add_component_by_example(
                    common_template,
                    component_template,
                    &app_dir,
                    &component_name,
                ) {
                    Ok(()) => {
                        log_action(
                            "Added",
                            format!(
                                "new app component: {}",
                                component_name.to_string_with_colon().log_color_highlight()
                            ),
                        );
                    }
                    Err(error) => {
                        bail!("Failed to add new app component: {}", error)
                    }
                }
            }
        }

        log_action(
            "Created",
            format!(
                "application {}, loading application manifest..",
                application_name.log_color_highlight()
            ),
        );

        std::env::set_current_dir(&app_dir)?;
        // TODO: check how this interacts with the app manifest dir flag
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        logln("");
        app_ctx.log_dynamic_help(&DynamicHelpSections {
            components: true,
            custom_commands: true,
        })?;

        Ok(())
    }

    pub async fn build(
        &mut self,
        component_names: Vec<ComponentName>,
        build: Option<BuildArgs>,
        default_component_select_mode: &ComponentSelectMode,
    ) -> anyhow::Result<()> {
        if let Some(build) = build {
            self.ctx
                .set_steps_filter(build.step.into_iter().collect())
                .await;
            self.ctx
                .set_skip_up_to_date_checks(build.force_build.force_build)
                .await;
        }
        self.must_select_components(component_names, default_component_select_mode)
            .await?;
        let mut app_ctx = self.ctx.app_context_lock_mut().await;
        app_ctx.some_or_err_mut()?.build().await
    }

    pub async fn clean(
        &mut self,
        component_names: Vec<ComponentName>,
        default_component_select_mode: &ComponentSelectMode,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, default_component_select_mode)
            .await?;
        let app_ctx = self.ctx.app_context_lock().await;
        app_ctx.some_or_err()?.clean()
    }

    async fn must_select_components(
        &mut self,
        component_names: Vec<ComponentName>,
        default: &ComponentSelectMode,
    ) -> anyhow::Result<()> {
        self.opt_select_components(component_names, default)
            .await?
            .then_some(())
            .ok_or(anyhow!(HintError::NoApplicationManifestFound))
    }

    pub async fn opt_select_components(
        &mut self,
        component_names: Vec<ComponentName>,
        default: &ComponentSelectMode,
    ) -> anyhow::Result<bool> {
        self.opt_select_components_internal(component_names, default, false)
            .await
    }

    pub async fn opt_select_components_allow_not_found(
        &mut self,
        component_names: Vec<ComponentName>,
        default: &ComponentSelectMode,
    ) -> anyhow::Result<bool> {
        self.opt_select_components_internal(component_names, default, true)
            .await
    }

    // TODO: forbid matching the same component multiple times
    // Returns false if there is no app
    pub async fn opt_select_components_internal(
        &mut self,
        component_names: Vec<ComponentName>,
        default: &ComponentSelectMode,
        allow_not_found: bool,
    ) -> anyhow::Result<bool> {
        let mut app_ctx = self.ctx.app_context_lock_mut().await;
        let silent_selection = app_ctx.silent_init;
        let Some(app_ctx) = app_ctx.opt_mut()? else {
            return Ok(false);
        };

        if component_names.is_empty() {
            let _log_output = silent_selection.then(|| LogOutput::new(Output::TracingDebug));
            app_ctx.select_components(default)?
        } else {
            let fuzzy_search =
                FuzzySearch::new(app_ctx.application.component_names().map(|cn| cn.as_str()));

            let (found, not_found) =
                fuzzy_search.find_many(component_names.iter().map(|cn| cn.0.as_str()));

            if !not_found.is_empty() {
                if allow_not_found {
                    return Ok(false);
                }

                logln("");
                log_error(format!(
                    "The following requested component names were not found:\n{}",
                    not_found
                        .iter()
                        .map(|error| {
                            match error {
                                Error::Ambiguous {
                                    pattern,
                                    highlighted_options,
                                } => {
                                    format!(
                                        "  - {}, did you mean one of {}?",
                                        pattern.as_str().bold(),
                                        highlighted_options.iter().map(|cn| cn.bold()).join(", ")
                                    )
                                }
                                Error::NotFound { pattern } => {
                                    format!("  - {}", pattern.as_str().bold())
                                }
                            }
                        })
                        .join("\n")
                ));
                logln("");
                log_text_view(&AvailableComponentNamesHelp(
                    app_ctx.application.component_names().cloned().collect(),
                ));

                bail!(NonSuccessfulExit);
            }

            log_fuzzy_matches(&found);

            let _log_output = silent_selection.then(|| LogOutput::new(Output::TracingDebug));
            app_ctx.select_components(&ComponentSelectMode::Explicit(
                found.into_iter().map(|m| m.option.into()).collect(),
            ))?
        }
        Ok(true)
    }

    pub fn get_template(
        &self,
        requested_template_name: &str,
    ) -> anyhow::Result<(Option<&Example>, &Example)> {
        let segments = requested_template_name.split("/").collect::<Vec<_>>();
        let (language, template_name): (String, Option<String>) = match segments.len() {
            1 => (segments[0].to_string(), None),
            2 => (segments[0].to_string(), {
                let template_name = segments[1].to_string();
                if template_name.is_empty() {
                    None
                } else {
                    Some(template_name)
                }
            }),
            _ => {
                log_error("Failed to parse template name");
                self.log_templates_help();
                bail!(NonSuccessfulExit);
            }
        };

        let language = match GuestLanguage::from_string(language) {
            Some(language) => language,
            None => {
                log_error("Failed to parse language part of the template!");
                self.log_templates_help();
                bail!(NonSuccessfulExit);
            }
        };
        let template_name = template_name
            .map(ExampleName::from)
            .unwrap_or_else(|| ExampleName::from("default"));

        let Some(lang_templates) = self.ctx.templates().get(&language) else {
            log_error(format!("No templates found for language: {}", language).as_str());
            self.log_templates_help();
            bail!(NonSuccessfulExit);
        };

        let lang_templates = lang_templates
            .get(&ComposableAppGroupName::default())
            .unwrap();

        let Some(component_template) = lang_templates.components.get(&template_name) else {
            log_error(format!(
                "Template {} not found!",
                requested_template_name.log_color_highlight()
            ));
            self.log_templates_help();
            bail!(NonSuccessfulExit);
        };

        Ok((lang_templates.common.as_ref(), component_template))
    }

    pub fn log_templates_help(&self) {
        logln(format!(
            "\n{}",
            "Available languages and templates:".underline().bold(),
        ));
        for (language, templates) in self.ctx.templates() {
            logln(format!("- {}", language.to_string().bold()));
            for (group, template) in templates {
                if group.as_str() != "default" {
                    panic!("TODO: handle non-default groups")
                }
                for template in template.components.values() {
                    if template.name.as_str() == "default" {
                        logln(format!(
                            "  - {} (default template): {}",
                            language.id().bold(),
                            template.description,
                        ));
                    } else {
                        logln(format!(
                            "  - {}/{}: {}",
                            language.id().bold(),
                            template.name.as_str().bold(),
                            template.description,
                        ));
                    }
                }
            }
        }
    }
}
