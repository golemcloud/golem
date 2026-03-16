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

use crate::app::context::validated_to_anyhow;
use crate::app::template::{
    AppTemplateAgent, AppTemplateCommon, AppTemplateComponent, AppTemplateName, TemplatePlan,
    TemplatePlanBuilder, TemplatePlanStep,
};
use crate::command_handler::app::AppCommandHandler;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::{HintError, NonSuccessfulExit, ShowClapHelpTarget};
use crate::fs;
use crate::log::{
    log_action, log_anyhow_error, log_error, log_failed_to, log_finished_ok,
    log_skipping_up_to_date, logln, LogColorize, LogIndent,
};
use crate::model::text::diff::log_unified_diff;
use crate::model::GuestLanguage;
use crate::validation::ValidationBuilder;
use anyhow::{anyhow, bail};
use colored::Colorize;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_common::model::diff;
use heck::ToKebabCase;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::debug;

#[derive(Debug)]
struct ExistingComponent {
    pub language: GuestLanguage,
    pub dir: PathBuf, // relative path, defined in the manifest 'dir' property
    pub component_dir: PathBuf, // canonical resolved dir
    pub manifest_source_dir: PathBuf, // source of the manifest, where the component is defined
}

#[derive(Debug)]
struct NewCommandContext {
    pub application_name_candidate: String,
    pub app_dir: PathBuf,
    pub existing_components: BTreeMap<ComponentName, ExistingComponent>,
}

#[derive(Debug)]
struct NewCommandSelections {
    pub application_name: ApplicationName,
    pub template_names: Vec<AppTemplateName>,
    pub component_name: Option<ComponentName>,
}

#[derive(Debug)]
struct NewTemplateComponentMapping {
    pub template_to_component: BTreeMap<AppTemplateName, ComponentName>,
}

#[derive(Debug)]
struct NewTemplateInputs {
    pub common_templates: BTreeMap<AppTemplateName, AppTemplateCommon>,
    pub component_templates: BTreeMap<ComponentName, AppTemplateComponent>,
    pub agent_templates: BTreeMap<AppTemplateName, AppTemplateAgent>,
    pub all_component_directories: BTreeMap<ComponentName, PathBuf>,
}

pub struct TemplateHandler {
    ctx: Arc<Context>,
}

impl TemplateHandler {
    pub fn new(handler: &AppCommandHandler) -> Self {
        Self {
            ctx: handler.ctx.clone(),
        }
    }

    pub async fn cmd_new(
        &self,
        application_path: Option<PathBuf>,
        application_name: Option<ApplicationName>,
        component_name: Option<ComponentName>,
        template_names: Vec<AppTemplateName>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let context = self
            .resolve_new_command_context(application_path, application_name)
            .await?;

        let selections = self.resolve_new_command_selections(
            context.application_name_candidate,
            template_names,
            component_name,
        )?;

        let template_mapping = self.resolve_new_template_component_mapping(
            &selections.application_name,
            &selections.template_names,
            selections.component_name.as_ref(),
            &context.existing_components,
        )?;

        debug!(
            "template to component map: {:#?}",
            template_mapping.template_to_component
        );

        let template_inputs = self.resolve_new_template_inputs(
            &selections.application_name,
            &context.app_dir,
            &selections.template_names,
            &template_mapping.template_to_component,
            &context.existing_components,
        )?;

        let template_plan = self.build_new_template_plan(
            &selections.application_name,
            &context.app_dir,
            &template_mapping.template_to_component,
            &template_inputs,
        )?;

        self.validate_new_template_plan(&template_plan)?;
        self.log_new_template_plan(&template_plan);
        self.apply_new_template_plan(&template_plan)?;

        logln("");
        log_finished_ok("applying template(s)");

        Ok(())
    }

    async fn resolve_new_command_context(
        &self,
        application_path: Option<PathBuf>,
        application_name: Option<ApplicationName>,
    ) -> anyhow::Result<NewCommandContext> {
        let is_dot_application_path = application_path.as_deref() == Some(Path::new("."));

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.opt()?;

        match app_ctx {
            Some(app_ctx) => {
                if !is_dot_application_path {
                    logln("");
                    log_error("Cannot create new application in existing application directory");
                    logln("");
                    logln("To add new agents or component to the current application, use the 'golem new .' command!");
                    logln("");
                    logln("To create a new application, switch to new directory without one!");
                    bail!(NonSuccessfulExit);
                }

                if application_name.is_some() {
                    logln("");
                    log_error(
                        "Specifying the application name is not allowed in an existing application directory",
                    );
                    logln("");
                    logln("Use `golem new .` for adding new templates to the current application,");
                    logln("or switch to a different directory!");
                }

                let existing_components = app_ctx
                    .deployable_component_names()
                    .into_iter()
                    .map(|component_name| {
                        let component = app_ctx.application().component(&component_name);
                        let existing_component = ExistingComponent {
                            language: component.guess_language().ok_or_else(|| {
                                anyhow!(
                                    "Failed to determine language for component {}",
                                    component_name
                                )
                            })?,
                            dir: component
                                .dir()
                                .unwrap_or_else(|| Path::new(","))
                                .to_path_buf(),
                            component_dir: component.component_dir().to_path_buf(),
                            manifest_source_dir: fs::parent_or_err(component.source())?
                                .to_path_buf(),
                        };

                        Ok::<_, anyhow::Error>((component_name, existing_component))
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()?;

                Ok(NewCommandContext {
                    application_name_candidate: app_ctx.application().application_name().0.clone(),
                    app_dir: app_ctx.application().app_root_dir().to_path_buf(),
                    existing_components,
                })
            }
            None => {
                let application_path = fs::canonicalize_path(
                    application_path
                        .as_deref()
                        .unwrap_or_else(|| Path::new(".")),
                )?;
                let application_name_candidate = match application_name {
                    Some(application_name) => application_name.0,
                    None => fs::file_name_to_str(&application_path)?.to_string(),
                };

                Ok(NewCommandContext {
                    application_name_candidate,
                    app_dir: application_path,
                    existing_components: BTreeMap::new(),
                })
            }
        }
    }

    fn resolve_new_command_selections(
        &self,
        application_name_candidate: String,
        template_names: Vec<AppTemplateName>,
        component_name: Option<ComponentName>,
    ) -> anyhow::Result<NewCommandSelections> {
        let application_name = match application_name_candidate.parse::<ApplicationName>() {
            Ok(application_name) => application_name,
            Err(err) => match self
                .ctx
                .interactive_handler()
                .select_new_app_name(Some(&application_name_candidate))?
            {
                Some(application_name) => application_name,
                None => {
                    logln("");
                    log_error(format!("In non-interactive mode, APPLICATION_PATH must end with a valid application name: {}", err));
                    bail!(HintError::ShowClapHelp(ShowClapHelpTarget::AppNew));
                }
            },
        };

        let template_names = if template_names.is_empty() {
            match self
                .ctx
                .interactive_handler()
                .select_new_app_templates_ts()?
            {
                Some(template_names) => template_names,
                None => {
                    logln("");
                    log_error("In non-interactive mode, at least one template must be specified");
                    bail!(HintError::ShowClapHelp(ShowClapHelpTarget::AppNew));
                }
            }
        } else {
            template_names
        };

        Ok(NewCommandSelections {
            application_name,
            template_names,
            component_name,
        })
    }

    fn resolve_new_template_component_mapping(
        &self,
        application_name: &ApplicationName,
        template_names: &[AppTemplateName],
        component_name: Option<&ComponentName>,
        existing_components: &BTreeMap<ComponentName, ExistingComponent>,
    ) -> anyhow::Result<NewTemplateComponentMapping> {
        let mut template_to_component = BTreeMap::new();
        let mut validation = ValidationBuilder::new();

        for template_name in template_names {
            match component_name {
                Some(component_name) => {
                    if let Some(existing_component) = existing_components.get(component_name) {
                        if template_name.language() != existing_component.language {
                            validation.add_error(format!(
                                "Cannot add {} template {} to existing {} component {}, language mismatch!",
                                template_name.language().name().log_color_highlight(),
                                template_name.as_str().log_color_error_highlight(),
                                existing_component.language.name().log_color_highlight(),
                                component_name.as_str().log_color_highlight(),
                            ));
                            continue;
                        }
                    }

                    template_to_component.insert(template_name.clone(), component_name.clone());
                }
                None => {
                    let matching_components = existing_components
                        .iter()
                        .filter_map(|(component_name, component)| {
                            (component.language == template_name.language())
                                .then_some(component_name)
                        })
                        .collect::<Vec<_>>();

                    match matching_components.as_slice() {
                        [] => {
                            let component_name = ComponentName::try_from(
                                format!(
                                    "{}:{}-main",
                                    application_name.0,
                                    template_name.language().id()
                                )
                                .as_str(),
                            )
                            .map_err(|err| anyhow!(err))?;
                            template_to_component.insert(template_name.clone(), component_name);
                        }
                        [component_name] => {
                            template_to_component
                                .insert(template_name.clone(), (*component_name).clone());
                        }
                        _ => {
                            // TODO: FCL: interactive selection
                            todo!("interactive component selection")
                        }
                    }
                }
            }
        }

        validated_to_anyhow(
            "Failed to map templates to components",
            validation.build(()),
            None,
        )?;

        Ok(NewTemplateComponentMapping {
            template_to_component,
        })
    }

    fn resolve_new_template_inputs(
        &self,
        application_name: &ApplicationName,
        app_dir: &Path,
        template_names: &[AppTemplateName],
        template_to_component: &BTreeMap<AppTemplateName, ComponentName>,
        existing_components: &BTreeMap<ComponentName, ExistingComponent>,
    ) -> anyhow::Result<NewTemplateInputs> {
        let app_template_repo = self.ctx.app_template_repo()?;

        let mut common_templates = BTreeMap::new();
        let mut component_templates = BTreeMap::new();
        let mut agent_templates = BTreeMap::new();

        for template_name in template_names {
            let component_name = template_to_component.get(template_name).ok_or_else(|| {
                anyhow!(
                    "Illegal state: template {} has no assigned component",
                    template_name
                )
            })?;

            if let Some(common_template) =
                app_template_repo.common_template(template_name.language())?
            {
                if !common_templates.contains_key(&common_template.0.name) {
                    common_templates
                        .insert(common_template.0.name.clone(), common_template.clone());
                }
            }

            if !component_templates.contains_key(component_name) {
                if let Some(component_template) =
                    app_template_repo.component_templates(template_name.language())?
                {
                    component_templates.insert(component_name.clone(), component_template.clone());
                }
            }

            match app_template_repo.agent_template(template_name) {
                Ok(agent_template) => {
                    agent_templates.insert(template_name.clone(), agent_template.clone());
                }
                Err(_) => {
                    logln("");
                    log_error(format!(
                        "Template not found: {}",
                        template_name.as_str().log_color_error_highlight()
                    ));
                    logln("");
                    AppCommandHandler::new(self.ctx.clone())
                        .log_templates_help(Some(template_name.language()), None)?;
                    bail!(NonSuccessfulExit);
                }
            }
        }

        let existing_component_names = existing_components.keys().cloned().collect::<BTreeSet<_>>();
        let component_names_to_add_or_update =
            component_templates.keys().cloned().collect::<BTreeSet<_>>();
        let all_component_names = component_names_to_add_or_update
            .union(&existing_component_names)
            .cloned()
            .collect::<BTreeSet<_>>();

        let all_component_directories = if all_component_names.len() == 1 {
            let mut all_component_directories = BTreeMap::new();

            for component_name in &all_component_names {
                match existing_components.get(component_name) {
                    Some(component) => {
                        all_component_directories
                            .insert(component_name.clone(), component.dir.clone());
                    }
                    None => {
                        all_component_directories
                            .insert(component_name.clone(), PathBuf::from("."));
                    }
                }
            }

            all_component_directories
        } else {
            let app_prefix = format!("{}:", application_name.0);
            let mut all_component_directories = BTreeMap::new();

            // Promoting root single components to multi component layout
            for component_name in &all_component_names {
                let new_component_dir = || {
                    let new_component_dir = match component_name.as_ref().strip_prefix(&app_prefix)
                    {
                        Some(suffix) => suffix.to_string(),
                        None => component_name.as_str().to_kebab_case(),
                    };

                    let full_new_component_dir = app_dir.join(&new_component_dir);
                    if full_new_component_dir.exists() {
                        bail!(
                            "Failed to apply template(s): cannot promote application component {} to multi-component layout: directory {} already exists!",
                            component_name.as_str().log_color_highlight(),
                            full_new_component_dir.log_color_error_highlight(),
                        );
                    }

                    Ok(PathBuf::from(new_component_dir))
                };

                match existing_components.get(component_name) {
                    Some(component) => {
                        if component.component_dir != app_dir {
                            all_component_directories
                                .insert(component_name.clone(), component.dir.clone());
                            continue;
                        }

                        if component.manifest_source_dir != component.component_dir {
                            bail!(
                                "Cannot add template(s), the current application uses a custom layout."
                            )
                        }

                        let new_component_dir = new_component_dir()?;

                        // TODO: with declarative plan, log and approve, and based on the component template
                        match component.language {
                            GuestLanguage::TypeScript => {
                                log_action(
                                    "Promoting",
                                    format!(
                                        "component {} to multi-component layout",
                                        component_name.as_str().log_color_highlight()
                                    ),
                                );
                                let source = app_dir;
                                let target = app_dir.join(&new_component_dir);

                                std::fs::create_dir_all(&target)?;

                                std::fs::rename(source.join("src"), target.join("src"))?;
                                std::fs::rename(
                                    source.join("tsconfig.json"),
                                    target.join("tsconfig.json"),
                                )?;
                            }
                            GuestLanguage::Rust => {
                                // TODO: FCL
                                todo!("implement rust multi-component promotion")
                            }
                        }

                        all_component_directories.insert(component_name.clone(), new_component_dir);
                    }
                    None => {
                        all_component_directories
                            .insert(component_name.clone(), new_component_dir()?);
                    }
                }
            }

            all_component_directories
        };

        debug!(
            "all component directories: {:#?}",
            all_component_directories
        );

        // We extend the component templates again to include promoted existing components
        for component_name in all_component_directories.keys() {
            if let Some(component) = existing_components.get(component_name) {
                if let Some(component_template) =
                    app_template_repo.component_template(component.language)?
                {
                    component_templates.insert(component_name.clone(), component_template.clone());
                }
            }
        }

        Ok(NewTemplateInputs {
            common_templates,
            component_templates,
            agent_templates,
            all_component_directories,
        })
    }

    fn build_new_template_plan(
        &self,
        application_name: &ApplicationName,
        app_dir: &Path,
        template_to_component: &BTreeMap<AppTemplateName, ComponentName>,
        template_inputs: &NewTemplateInputs,
    ) -> anyhow::Result<TemplatePlan> {
        let mut template_plan_builder = TemplatePlanBuilder::new();

        for (common_template_name, common_template) in &template_inputs.common_templates {
            template_plan_builder.add(
                common_template_name.as_str(),
                &common_template.generate(application_name, app_dir, self.ctx.sdk_overrides())?,
            );
        }

        for (component_name, component_template) in &template_inputs.component_templates {
            let component_dir = template_inputs
                .all_component_directories
                .get(component_name)
                .ok_or_else(|| {
                    anyhow!(
                        "Illegal state: missing component directory for: {}",
                        component_name
                    )
                })?;

            template_plan_builder.add(
                component_template.0.name.as_str(),
                &component_template.generate(
                    application_name,
                    app_dir,
                    component_name,
                    component_dir,
                    self.ctx.sdk_overrides(),
                )?,
            );
        }

        for (agent_template_name, agent_template) in &template_inputs.agent_templates {
            let component_name =
                template_to_component
                    .get(agent_template_name)
                    .ok_or_else(|| {
                        anyhow!(
                            "Illegal state: template {} has no assigned component",
                            agent_template_name
                        )
                    })?;

            let component_dir = template_inputs
                .all_component_directories
                .get(component_name)
                .ok_or_else(|| {
                    anyhow!(
                        "Illegal state: missing component directory for: {}",
                        component_name
                    )
                })?;

            template_plan_builder.add(
                agent_template_name.as_str(),
                &agent_template.generate(
                    application_name,
                    app_dir,
                    component_name,
                    component_dir,
                    self.ctx.sdk_overrides(),
                )?,
            );
        }

        debug!("template plan steps: {:#?}", template_plan_builder);

        Ok(template_plan_builder.build())
    }

    fn validate_new_template_plan(&self, template_plan: &TemplatePlan) -> anyhow::Result<()> {
        let overwrites = template_plan.overwrites().collect::<Vec<_>>();
        let failed_plans = template_plan.failed_plans().collect::<Vec<_>>();

        if !overwrites.is_empty() || !failed_plans.is_empty() {
            logln("");
            log_failed_to("plan the required changes to apply the selected template(s)");
            let _indent = self.ctx.log_handler().nested_text_view_indent();

            logln("");

            if !overwrites.is_empty() {
                logln(
                    "Already existing non-mergeable files:"
                        .log_color_help_group()
                        .to_string(),
                );
                for path in &overwrites {
                    logln(format!("  - {}", path.log_color_highlight()));
                }
                logln("");
            }

            if !failed_plans.is_empty() {
                logln(
                    "Template planning errors:"
                        .log_color_help_group()
                        .to_string(),
                );
                for (path, err) in &failed_plans {
                    logln(format!("  - {}:", path.log_color_highlight()));
                    let _indent = LogIndent::prefix("    ");
                    log_anyhow_error(err)
                }
                logln("");
            }

            bail!(NonSuccessfulExit);
        }

        Ok(())
    }

    fn log_new_template_plan(&self, template_plan: &TemplatePlan) {
        // TODO: FCL: create a safe subset of the template plan?
        // TODO: FCL: review and approve
        logln("");
        log_action(
            "Planned",
            "required changes for applying the selected template(s)",
        );
        let _indent = self.ctx.log_handler().nested_text_view_indent();
        for (path, step) in template_plan.file_steps() {
            if let Ok(step) = step {
                match step {
                    TemplatePlanStep::Create { .. } => {
                        logln(format!(
                            "- {} {}",
                            "create".green(),
                            path.log_color_highlight()
                        ));
                    }
                    TemplatePlanStep::Overwrite { .. } => {
                        logln(format!(
                            "- {} {}",
                            "overwrite".red(),
                            path.log_color_highlight()
                        ));
                    }
                    TemplatePlanStep::Merge { current, new } => {
                        logln(format!(
                            "- {} {}",
                            "update".green(),
                            path.log_color_highlight()
                        ));
                        let _indent = LogIndent::new();
                        let _indent = self.ctx.log_handler().nested_text_view_indent();
                        log_unified_diff(&diff::unified_diff(current, new));
                    }
                    TemplatePlanStep::SkipSame { .. } => {
                        logln(format!(
                            "- {} {}",
                            "skip".yellow(),
                            path.log_color_highlight(),
                        ));
                    }
                }
            }
        }
    }

    fn apply_new_template_plan(&self, template_plan: &TemplatePlan) -> anyhow::Result<()> {
        logln("");
        log_action("Applying", "template(s)");
        let _indent = LogIndent::new();

        for (path, step) in template_plan.file_steps() {
            if let Ok(step) = step {
                match step {
                    TemplatePlanStep::Create { new } => {
                        log_action("Creating", format!("{}", path.log_color_highlight()));
                        fs::write_str(path, new)?;
                    }
                    TemplatePlanStep::Overwrite { new, .. } => {
                        log_action("Overwriting", format!("{}", path.log_color_highlight()));
                        fs::write_str(path, new)?;
                    }
                    TemplatePlanStep::Merge { new, .. } => {
                        log_action("Updating", format!("{}", path.log_color_highlight()));
                        fs::write_str(path, new)?;
                    }
                    TemplatePlanStep::SkipSame { .. } => {
                        log_skipping_up_to_date(format!("updating {}", path.log_color_highlight()));
                    }
                }
            }
        }

        Ok(())
    }
}
