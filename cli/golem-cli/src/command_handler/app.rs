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

use crate::app::error::CustomCommandError;
use crate::command::app::AppSubcommand;
use crate::command::builtin_app_subcommands;
use crate::command::shared_args::{
    AppOptionalComponentNames, BuildArgs, ForceBuildArg, UpdateOrRedeployArgs,
};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::diagnose::diagnose;
use crate::error::{HintError, NonSuccessfulExit, ShowClapHelpTarget};
use crate::fs;
use crate::fuzzy::{Error, FuzzySearch};
use crate::log::{log_action, logln, LogColorize, LogIndent, LogOutput, Output};
use crate::model::api::HttpApiDeployMode;
use crate::model::app::{ApplicationComponentSelectMode, DynamicHelpSections};
use crate::model::component::Component;
use crate::model::text::fmt::{log_error, log_fuzzy_matches, log_text_view, log_warn};
use crate::model::text::help::AvailableComponentNamesHelp;
use crate::model::{ComponentName, WorkerUpdateMode};
use anyhow::{anyhow, bail};
use colored::Colorize;
use golem_templates::add_component_by_template;
use golem_templates::model::{
    ComposableAppGroupName, GuestLanguage, PackageName, Template, TemplateName,
};
use itertools::Itertools;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use strum::IntoEnumIterator;

pub struct AppCommandHandler {
    ctx: Arc<Context>,
}

impl AppCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: AppSubcommand) -> anyhow::Result<()> {
        match subcommand {
            AppSubcommand::New {
                application_name,
                language,
            } => self.cmd_new(application_name, language).await,
            AppSubcommand::Build {
                component_name,
                build: build_args,
            } => self.cmd_build(component_name, build_args).await,
            AppSubcommand::Deploy {
                component_name,
                force_build,
                update_or_redeploy,
            } => {
                self.cmd_deploy(component_name, force_build, update_or_redeploy)
                    .await
            }
            AppSubcommand::Clean { component_name } => self.cmd_clean(component_name).await,
            AppSubcommand::UpdateWorkers {
                component_name,
                update_mode,
                r#await,
            } => {
                self.cmd_update_workers(component_name.component_name, update_mode, r#await)
                    .await
            }
            AppSubcommand::RedeployWorkers { component_name } => {
                self.cmd_redeploy_workers(component_name.component_name)
                    .await
            }
            AppSubcommand::Diagnose { component_name } => self.cmd_diagnose(component_name).await,
            AppSubcommand::CustomCommand(command) => self.cmd_custom_command(command).await,
        }
    }

    async fn cmd_new(
        &self,
        application_name: Option<String>,
        languages: Vec<GuestLanguage>,
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
                    logln(format!(
                        "the '{}' command to create a component in the current application.",
                        "component new".log_color_highlight()
                    ));
                    logln("");
                    bail!(NonSuccessfulExit);
                }
            }
        }

        let Some((application_name, components)) = ({
            match application_name {
                Some(application_name) => Some((application_name, vec![])),
                None => self
                    .ctx
                    .interactive_handler()
                    .select_new_app_name_and_components()?
                    .map(|new_app| (new_app.app_name, new_app.templated_component_names)),
            }
        }) else {
            log_error("Both APPLICATION_NAME and LANGUAGES are required in non-interactive mode");
            logln("");
            bail!(HintError::ShowClapHelp(ShowClapHelpTarget::AppNew));
        };

        if components.is_empty() && languages.is_empty() {
            log_error("LANGUAGES are required in non-interactive mode");
            logln("");
            logln("Either specify languages or use the new command without APPLICATION_NAME to use the interactive wizard!");
            logln("");
            bail!(HintError::ShowClapHelp(ShowClapHelpTarget::AppNew));
        }

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

        if components.is_empty() {
            let common_templates = languages
                .iter()
                .map(|language| {
                    self.get_template(&language.id())
                        .map(|(common, _component)| common)
                })
                .collect::<Result<Vec<_>, _>>()?;

            {
                let _indent = LogIndent::new();
                // TODO: cleanup add_component_by_example, so we don't have to pass a dummy arg
                let dummy_package_name = PackageName::from_string("app:comp").unwrap();
                for common_template in common_templates.into_iter().flatten() {
                    match add_component_by_template(
                        Some(common_template),
                        None,
                        &app_dir,
                        &dummy_package_name,
                    ) {
                        Ok(()) => {
                            log_action(
                                "Added",
                                format!(
                                    "common template for {}",
                                    common_template.language.name().log_color_highlight()
                                ),
                            );
                        }
                        Err(error) => {
                            bail!("Failed to add common template for new app: {:#}", error)
                        }
                    }
                }
            }
        } else {
            for (template, component_package_name) in &components {
                log_action(
                    "Adding",
                    format!(
                        "component {}",
                        component_package_name
                            .to_string_with_colon()
                            .log_color_highlight()
                    ),
                );
                let (common_template, component_template) = self.get_template(template)?;
                match add_component_by_template(
                    common_template,
                    Some(component_template),
                    &app_dir,
                    component_package_name,
                ) {
                    Ok(()) => {
                        log_action(
                            "Added",
                            format!(
                                "new app component {}",
                                component_package_name
                                    .to_string_with_colon()
                                    .log_color_highlight()
                            ),
                        );
                    }
                    Err(error) => {
                        bail!("Failed to create new app component: {}", error)
                    }
                }
            }
        }

        log_action(
            "Created",
            format!("application {}", application_name.log_color_highlight()),
        );

        logln("");

        if components.is_empty() {
            logln(
                format!(
                    "To add components to the application, switch to the {} directory, and use the `{}` command.",
                    application_name.log_color_highlight(),
                    "component new".log_color_highlight(),
                )
            );
        } else {
            // Unloading app context and switching dir, so we can reload the new app
            self.ctx.unload_app_context().await;
            std::env::set_current_dir(app_dir)?;

            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;

            app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?;
            logln(
                format!(
                    "Switch to the {} directory, and use the `{}` or `{}` commands to use your new application!",
                    application_name.log_color_highlight(),
                    "app build".log_color_highlight(),
                    "app deploy".log_color_highlight(),
                )
            );
        }

        Ok(())
    }

    async fn cmd_build(
        &self,
        component_name: AppOptionalComponentNames,
        build_args: BuildArgs,
    ) -> anyhow::Result<()> {
        self.build(
            component_name.component_name,
            Some(build_args),
            &ApplicationComponentSelectMode::All,
        )
        .await
    }

    async fn cmd_clean(&self, component_name: AppOptionalComponentNames) -> anyhow::Result<()> {
        self.clean(
            component_name.component_name,
            &ApplicationComponentSelectMode::All,
        )
        .await
    }

    async fn cmd_deploy(
        &self,
        component_name: AppOptionalComponentNames,
        force_build: ForceBuildArg,
        update_or_redeploy: UpdateOrRedeployArgs,
    ) -> anyhow::Result<()> {
        self.deploy(component_name, force_build, update_or_redeploy)
            .await
    }

    async fn cmd_custom_command(&self, command: Vec<String>) -> anyhow::Result<()> {
        if command.len() != 1 {
            bail!(
                "Expected exactly one custom subcommand, got: {}",
                command.join(" ").log_color_error_highlight()
            );
        }

        let command = command[0].strip_prefix(":").unwrap_or(&command[0]);

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        if let Err(error) = app_ctx.custom_command(command) {
            match error {
                CustomCommandError::CommandNotFound => {
                    logln("");
                    log_error(format!(
                        "Request command app command {} not found!",
                        command.log_color_error_highlight()
                    ));
                    logln("");

                    app_ctx.log_dynamic_help(&DynamicHelpSections::show_custom_commands(
                        builtin_app_subcommands(),
                    ))?;

                    logln(
                        "Available builtin commands:"
                            .log_color_help_group()
                            .to_string(),
                    );
                    let app_subcommands = builtin_app_subcommands();
                    for subcommand in &app_subcommands {
                        logln(format!("  {}", subcommand.bold()));
                    }
                    logln("");

                    bail!(NonSuccessfulExit)
                }
                CustomCommandError::CommandError { error } => {
                    bail!(
                        "Command {} failed: {error}",
                        command.log_color_error_highlight()
                    )
                }
            }
        }

        Ok(())
    }

    async fn cmd_update_workers(
        &self,
        component_names: Vec<ComponentName>,
        update_mode: WorkerUpdateMode,
        await_update: bool,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, &ApplicationComponentSelectMode::All)
            .await?;

        let components = self.components_for_update_or_redeploy().await?;
        self.ctx
            .component_handler()
            .update_workers_by_components(&components, update_mode, await_update)
            .await?;

        Ok(())
    }

    async fn cmd_redeploy_workers(
        &self,
        component_names: Vec<ComponentName>,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, &ApplicationComponentSelectMode::All)
            .await?;

        let components = self.components_for_update_or_redeploy().await?;
        self.ctx
            .component_handler()
            .redeploy_workers_by_components(&components)
            .await?;

        Ok(())
    }

    async fn cmd_diagnose(&self, component_names: AppOptionalComponentNames) -> anyhow::Result<()> {
        self.diagnose(
            component_names.component_name,
            &ApplicationComponentSelectMode::All,
        )
        .await
    }

    async fn deploy(
        &self,
        component_name: AppOptionalComponentNames,
        force_build: ForceBuildArg,
        update_or_redeploy: UpdateOrRedeployArgs,
    ) -> anyhow::Result<()> {
        let is_any_component_explicitly_selected = !component_name.component_name.is_empty();

        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(None)
            .await?;

        let components = self
            .ctx
            .component_handler()
            .deploy(
                project.as_ref(),
                component_name.component_name,
                Some(force_build),
                &ApplicationComponentSelectMode::All,
                &update_or_redeploy,
            )
            .await?;

        let components = components
            .into_iter()
            .map(|component| (component.component_name.0.clone(), component))
            .collect::<BTreeMap<_, _>>();

        self.ctx
            .api_handler()
            .deploy(
                project.as_ref(),
                if is_any_component_explicitly_selected {
                    HttpApiDeployMode::Matching
                } else {
                    HttpApiDeployMode::All
                },
                &update_or_redeploy,
                &components,
            )
            .await?;

        Ok(())
    }

    pub async fn build(
        &self,
        component_names: Vec<ComponentName>,
        build: Option<BuildArgs>,
        default_component_select_mode: &ApplicationComponentSelectMode,
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
        let mut app_ctx = self.ctx.app_context_lock_mut().await?;
        app_ctx.some_or_err_mut()?.build().await
    }

    pub async fn clean(
        &self,
        component_names: Vec<ComponentName>,
        default_component_select_mode: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, default_component_select_mode)
            .await?;
        let app_ctx = self.ctx.app_context_lock().await;
        app_ctx.some_or_err()?.clean()
    }

    async fn components_for_update_or_redeploy(&self) -> anyhow::Result<Vec<Component>> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let selected_component_names = app_ctx
            .selected_component_names()
            .iter()
            .map(|cn| cn.as_str().into())
            .collect::<Vec<ComponentName>>();

        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(None)
            .await?;

        let mut components = Vec::with_capacity(selected_component_names.len());
        for component_name in &selected_component_names {
            match self
                .ctx
                .component_handler()
                .component(project.as_ref(), component_name.into(), None)
                .await?
            {
                Some(component) => {
                    components.push(component);
                }
                None => {
                    log_warn(format!(
                        "Component {} is not deployed!",
                        component_name.0.log_color_highlight()
                    ));
                }
            }
        }
        Ok(components)
    }

    pub async fn must_select_components(
        &self,
        component_names: Vec<ComponentName>,
        default: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        self.opt_select_components(component_names, default)
            .await?
            .then_some(())
            .ok_or(anyhow!(HintError::NoApplicationManifestFound))
    }

    pub async fn opt_select_components(
        &self,
        component_names: Vec<ComponentName>,
        default: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<bool> {
        self.opt_select_components_internal(component_names, default, false)
            .await
    }

    pub async fn opt_select_components_allow_not_found(
        &self,
        component_names: Vec<ComponentName>,
        default: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<bool> {
        self.opt_select_components_internal(component_names, default, true)
            .await
    }

    // TODO: forbid matching the same component multiple times
    // Returns false if there is no app
    async fn opt_select_components_internal(
        &self,
        component_names: Vec<ComponentName>,
        default: &ApplicationComponentSelectMode,
        allow_not_found: bool,
    ) -> anyhow::Result<bool> {
        let mut app_ctx = self.ctx.app_context_lock_mut().await?;
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
                                    ..
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
            app_ctx.select_components(&ApplicationComponentSelectMode::Explicit(
                found.into_iter().map(|m| m.option.into()).collect(),
            ))?
        }
        Ok(true)
    }

    pub fn get_template(
        &self,
        requested_template_name: &str,
    ) -> anyhow::Result<(Option<&Template>, &Template)> {
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
                self.log_templates_help(None, None);
                bail!(NonSuccessfulExit);
            }
        };

        let language = match GuestLanguage::from_string(language) {
            Some(language) => language,
            None => {
                log_error("Failed to parse language part of the template!");
                self.log_templates_help(None, None);
                bail!(NonSuccessfulExit);
            }
        };
        let template_name = template_name
            .map(TemplateName::from)
            .unwrap_or_else(|| TemplateName::from("default"));

        let Some(lang_templates) = self.ctx.templates().get(&language) else {
            log_error(format!("No templates found for language: {language}").as_str());
            self.log_templates_help(None, None);
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
            self.log_templates_help(None, None);
            bail!(NonSuccessfulExit);
        };

        Ok((lang_templates.common.as_ref(), component_template))
    }

    pub fn log_languages_help(&self) {
        logln(format!("\n{}", "Available languages:".underline().bold(),));
        for language in GuestLanguage::iter() {
            logln(format!(
                "- {}: {}",
                language.name(),
                language.id().log_color_highlight()
            ));
        }
    }

    pub fn log_templates_help(
        &self,
        language_filter: Option<GuestLanguage>,
        template_filter: Option<&str>,
    ) {
        if language_filter.is_none() && template_filter.is_none() {
            logln(format!(
                "\n{}",
                "Available languages and templates:".underline().bold(),
            ));
        } else {
            logln(format!("\n{}", "Matching templates:".underline().bold(),));
        }

        let templates = self
            .ctx
            .templates()
            .iter()
            .filter_map(|(language, templates)| {
                templates
                    .get(&ComposableAppGroupName::default())
                    .and_then(|templates| {
                        let matches_lang = language_filter
                            .map(|language_filter| language_filter == *language)
                            .unwrap_or(true);

                        if matches_lang {
                            let templates = templates
                                .components
                                .iter()
                                .filter(|(template_name, template)| {
                                    template_filter
                                        .map(|template_filter| {
                                            template_name
                                                .as_str()
                                                .to_lowercase()
                                                .contains(template_filter)
                                                || template
                                                    .description
                                                    .to_lowercase()
                                                    .contains(template_filter)
                                        })
                                        .unwrap_or(true)
                                })
                                .collect::<Vec<_>>();

                            (!templates.is_empty()).then_some(templates)
                        } else {
                            None
                        }
                    })
                    .map(|templates| (language, templates))
            })
            .collect::<Vec<_>>();

        for (language, templates) in templates {
            if let Some(language_filter) = language_filter {
                if language_filter != *language {
                    continue;
                }
            }

            logln(format!("- {}", language.to_string().bold()));
            for (template_name, template) in templates {
                if template_name.as_str() == "default" {
                    logln(format!(
                        "  - {}: {}",
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

    pub async fn diagnose(
        &self,
        component_names: Vec<ComponentName>,
        default_component_select_mode: &ApplicationComponentSelectMode,
    ) -> anyhow::Result<()> {
        self.must_select_components(component_names, default_component_select_mode)
            .await?;

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let selected_component_names = app_ctx
            .selected_component_names()
            .iter()
            .collect::<Vec<_>>();

        if selected_component_names.is_empty() {
            log_warn("The application has no components.");
        }

        for component_name in selected_component_names {
            log_action(
                "Diagnosing",
                format!(
                    "component {} for recommended tooling",
                    component_name.as_str().log_color_highlight()
                ),
            );
            let _indent = self.ctx.log_handler().nested_text_view_indent();

            diagnose(
                app_ctx.application.component_source_dir(component_name),
                None,
            );
        }

        Ok(())
    }
}
