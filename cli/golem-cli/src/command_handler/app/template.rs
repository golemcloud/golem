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

use crate::app::context::{find_main_source_from, validated_to_anyhow};
use crate::app::template::{
    AppTemplateAgent, AppTemplateCommon, AppTemplateComponent, AppTemplateName,
    MultiComponentLayoutUpgradePlan, MultiComponentLayoutUpgradePlanStep, SafeTemplatePlan,
    SafeTemplatePlanStep, TemplatePlan, TemplatePlanBuilder, UnsafeTemplatePlan,
};
use crate::command_handler::Handlers;
use crate::command_handler::app::AppCommandHandler;
use crate::command_name;
use crate::context::Context;
use crate::error::{HintError, NonSuccessfulExit, ShowClapHelpTarget};
use crate::fs;
use crate::log::{
    LogColorize, LogIndent, log_action, log_anyhow_error, log_error, log_failed_to,
    log_finished_ok, log_skipping_up_to_date, logln,
};
use crate::model::GuestLanguage;
use crate::model::text::diff::log_unified_diff_for_path;
use crate::model::text::fmt::log_text_view;
use crate::model::text::help::{AppNewNextStepsHint, AppNewNextStepsMode};
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
    pub application_path: PathBuf,
    pub existing_components: BTreeMap<ComponentName, ExistingComponent>,
    pub existing_app_mode: bool,
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
            &context.application_path,
            &selections.template_names,
            &template_mapping.template_to_component,
            &context.existing_components,
        )?;

        let template_plan = self.plan_applying_new_template(
            &selections.application_name,
            &context.application_path,
            &template_mapping.template_to_component,
            &template_inputs,
        )?;

        let (safe_template_plan, unsafe_template_plan) = template_plan.partition();

        self.validate_new_template_plan(&unsafe_template_plan)?;
        self.log_new_template_plan(&safe_template_plan);

        if !safe_template_plan.is_empty()
            && !self
                .ctx
                .interactive_handler()
                .confirm_template_plan_apply()?
        {
            bail!(NonSuccessfulExit);
        }

        self.apply_new_template_plan(&safe_template_plan)?;
        create_claude_symlink(&context.application_path)?;

        logln("");
        log_finished_ok("applying template(s)");
        log_text_view(&AppNewNextStepsHint {
            mode: if context.existing_app_mode {
                AppNewNextStepsMode::ExistingApplication
            } else {
                AppNewNextStepsMode::NewApplication
            },
            app_dir: context.application_path.clone(),
            needs_switch_directory: should_switch_to_app_dir_hint(&context.application_path)?,
            binary_name: command_name(),
        });

        Ok(())
    }

    async fn resolve_new_command_context(
        &self,
        application_path: Option<PathBuf>,
        application_name: Option<ApplicationName>,
    ) -> anyhow::Result<NewCommandContext> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.opt()?;

        let application_path = match (application_path, app_ctx.as_ref()) {
            (Some(application_path), _) => application_path,
            (None, Some(_)) => PathBuf::from("."),
            (None, None) => match self.ctx.interactive_handler().select_new_app_path()? {
                Some(application_path) => application_path,
                None => {
                    logln("");
                    log_error(
                        "In non-interactive mode, APPLICATION_PATH must be specified as '.' or a new directory path",
                    );
                    bail!(HintError::ShowClapHelp(ShowClapHelpTarget::AppNew));
                }
            },
        };

        let application_dir = if application_path.is_absolute() {
            fs::normalize_path_lexically(&application_path)
        } else {
            fs::normalize_path_lexically(&std::env::current_dir()?.join(&application_path))
        };

        if let Some(target_main_source) = find_main_source_from(&application_dir) {
            let target_app_root = target_main_source
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| target_main_source.clone());

            if fs::path_eq_normalized(&application_dir, &target_app_root) {
                if let Some(app_ctx) = app_ctx
                    && fs::path_eq_normalized(
                        app_ctx.application().app_root_dir(),
                        &target_app_root,
                    )
                {
                    if application_name.is_some() {
                        logln("");
                        log_error(
                            "Specifying the application name is not allowed in an existing application directory",
                        );
                        logln("");
                        logln(
                            "Use `golem new .` for adding new templates to the current application,",
                        );
                        logln("or switch to a different directory!");
                    }

                    let existing_components = app_ctx
                        .component_names()
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

                    return Ok(NewCommandContext {
                        application_name_candidate: app_ctx
                            .application()
                            .application_name()
                            .0
                            .clone(),
                        application_path: app_ctx.application().app_root_dir().to_path_buf(),
                        existing_components,
                        existing_app_mode: true,
                    });
                }

                logln("");
                log_error(format!(
                    "Target directory is already an existing application: {}",
                    target_app_root.log_color_error_highlight()
                ));
                logln("");
                logln("Switch to that directory and run `golem new .` to add templates there.");
                bail!(NonSuccessfulExit);
            }

            logln("");
            log_error(format!(
                "Cannot create a new application inside an existing application directory: {}",
                target_app_root.log_color_error_highlight()
            ));
            logln("");
            logln("Please choose a target directory that is not nested under another application.");
            bail!(NonSuccessfulExit);
        }

        if is_non_empty_dir(&application_dir)?
            && !self
                .ctx
                .interactive_handler()
                .confirm_new_app_in_non_empty_dir(&application_dir)?
        {
            bail!(NonSuccessfulExit);
        }

        let application_name_candidate = match application_name {
            Some(application_name) => application_name.0,
            None => fs::file_name_to_str(&application_dir)?.to_string(),
        };

        Ok(NewCommandContext {
            application_name_candidate,
            application_path: application_dir,
            existing_components: BTreeMap::new(),
            existing_app_mode: false,
        })
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
                    log_error(format!(
                        "In non-interactive mode, APPLICATION_PATH must end with a valid application name: {}",
                        err
                    ));
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
                    if let Some(existing_component) = existing_components.get(component_name)
                        && template_name.language() != existing_component.language
                    {
                        validation.add_error(format!(
                                "Cannot add {} template {} to existing {} component {}, language mismatch!",
                                template_name.language().name().log_color_highlight(),
                                template_name.as_str().log_color_error_highlight(),
                                existing_component.language.name().log_color_highlight(),
                                component_name.as_str().log_color_highlight(),
                            ));
                        continue;
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
                            let matching_components =
                                matching_components.into_iter().cloned().collect::<Vec<_>>();

                            let selected_component = self
                                .ctx
                                .interactive_handler()
                                .select_component_for_template(
                                    template_name,
                                    matching_components,
                                )?;

                            let Some(selected_component) = selected_component else {
                                logln("");
                                log_error(format!(
                                    "In non-interactive mode, --component-name must be specified when template {} matches multiple components",
                                    template_name.as_str().log_color_error_highlight()
                                ));
                                bail!(HintError::ShowClapHelp(ShowClapHelpTarget::AppNew));
                            };

                            template_to_component.insert(template_name.clone(), selected_component);
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
        application_path: &Path,
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
                && !common_templates.contains_key(&common_template.0.name)
            {
                common_templates.insert(common_template.0.name.clone(), common_template.clone());
            }

            if !component_templates.contains_key(component_name)
                && let Some(component_template) =
                    app_template_repo.component_templates(template_name.language())?
            {
                component_templates.insert(component_name.clone(), component_template.clone());
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

                    let full_new_component_dir = application_path.join(&new_component_dir);
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
                        if component.component_dir != application_path {
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

                        let component_template =
                            app_template_repo.component_template(component.language)?;
                        let upgrade_plan = self.plan_multi_component_layout_upgrade(
                            component,
                            application_path,
                            &new_component_dir,
                            component_template.as_ref(),
                        )?;

                        self.validate_multi_component_layout_upgrade_plan(&upgrade_plan)?;

                        self.log_multi_component_layout_upgrade_plan(component_name, &upgrade_plan);

                        if !upgrade_plan.is_empty()
                            && !self
                                .ctx
                                .interactive_handler()
                                .confirm_multi_component_layout_upgrade(component_name)?
                        {
                            bail!(NonSuccessfulExit);
                        }

                        self.apply_multi_component_layout_upgrade_plan(&upgrade_plan)?;

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
            if let Some(component) = existing_components.get(component_name)
                && let Some(component_template) =
                    app_template_repo.component_template(component.language)?
            {
                component_templates.insert(component_name.clone(), component_template.clone());
            }
        }

        Ok(NewTemplateInputs {
            common_templates,
            component_templates,
            agent_templates,
            all_component_directories,
        })
    }

    fn plan_applying_new_template(
        &self,
        application_name: &ApplicationName,
        application_path: &Path,
        template_to_component: &BTreeMap<AppTemplateName, ComponentName>,
        template_inputs: &NewTemplateInputs,
    ) -> anyhow::Result<TemplatePlan> {
        let mut template_plan_builder = TemplatePlanBuilder::new();

        for (common_template_name, common_template) in &template_inputs.common_templates {
            template_plan_builder.add(
                common_template_name.as_str(),
                &common_template.generate(application_name, application_path)?,
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
                    application_path,
                    component_name,
                    component_dir,
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
                    application_path,
                    component_name,
                    component_dir,
                )?,
            );
        }

        debug!("template plan steps: {:#?}", template_plan_builder);

        Ok(template_plan_builder.build())
    }

    fn plan_multi_component_layout_upgrade(
        &self,
        component: &ExistingComponent,
        application_path: &Path,
        new_component_dir: &Path,
        _component_template: Option<&AppTemplateComponent>,
    ) -> anyhow::Result<MultiComponentLayoutUpgradePlan> {
        let mut upgrade_plan = MultiComponentLayoutUpgradePlan::new();

        match component.language {
            GuestLanguage::TypeScript => {
                let target_root = application_path.join(new_component_dir);

                upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                    source: application_path.join("src"),
                    target: target_root.join("src"),
                });
                upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                    source: application_path.join("tsconfig.json"),
                    target: target_root.join("tsconfig.json"),
                });
            }
            GuestLanguage::Rust => {
                let target_root = application_path.join(new_component_dir);

                upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                    source: application_path.join("Cargo.toml"),
                    target: target_root.join("Cargo.toml"),
                });
                upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                    source: application_path.join("src"),
                    target: target_root.join("src"),
                });

                let cargo_target_dir = application_path.join("target");
                if cargo_target_dir.exists() {
                    upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                        source: cargo_target_dir,
                        target: target_root.join("target"),
                    });
                }
            }
            GuestLanguage::Scala => {
                let target_root = application_path.join(new_component_dir);

                upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                    source: application_path.join("build.sbt"),
                    target: target_root.join("build.sbt"),
                });
                upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                    source: application_path.join("src"),
                    target: target_root.join("src"),
                });
            }
            GuestLanguage::MoonBit => {
                let target_root = application_path.join(new_component_dir);

                upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                    source: application_path.join("moon.pkg"),
                    target: target_root.join("moon.pkg"),
                });
                for entry in std::fs::read_dir(application_path)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("mbt") {
                        let file_name = entry.file_name();
                        upgrade_plan.add(MultiComponentLayoutUpgradePlanStep::Move {
                            source: path.clone(),
                            target: target_root.join(&file_name),
                        });
                    }
                }
            }
        }

        Ok(upgrade_plan)
    }

    fn log_multi_component_layout_upgrade_plan(
        &self,
        component_name: &ComponentName,
        upgrade_plan: &MultiComponentLayoutUpgradePlan,
    ) {
        if upgrade_plan.is_empty() {
            return;
        }

        logln("");
        log_action(
            "Planned",
            format!(
                "multi-component layout upgrade steps for component {}, which is required before adding the selected template(s)",
                component_name.as_str().log_color_highlight()
            ),
        );
        let _indent = self.ctx.log_handler().decorated_indent_primary();

        for step in upgrade_plan.steps() {
            match step {
                MultiComponentLayoutUpgradePlanStep::Move { source, target } => {
                    logln(format!(
                        "- {} {} to {}",
                        "move".yellow(),
                        source.display().to_string().log_color_highlight(),
                        target.display().to_string().log_color_highlight()
                    ));
                }
            }
        }
    }

    fn validate_multi_component_layout_upgrade_plan(
        &self,
        upgrade_plan: &MultiComponentLayoutUpgradePlan,
    ) -> anyhow::Result<()> {
        if upgrade_plan.is_empty() {
            return Ok(());
        }

        let mut validation_errors = Vec::<String>::new();
        let mut targets = BTreeSet::<PathBuf>::new();

        for step in upgrade_plan.steps() {
            match step {
                MultiComponentLayoutUpgradePlanStep::Move { target, .. } => {
                    if !targets.insert(target.clone()) {
                        validation_errors.push(format!(
                            "Duplicate target in plan: {}",
                            target.display().to_string().log_color_error_highlight()
                        ));
                    }

                    if target.exists() {
                        validation_errors.push(format!(
                            "Target path already exists: {}",
                            target.display().to_string().log_color_error_highlight()
                        ));
                    }

                    if let Some(parent) = target.parent() {
                        if parent.exists() {
                            if !parent.is_dir() {
                                validation_errors.push(format!(
                                    "Target parent path is not a directory: {}",
                                    parent.display().to_string().log_color_error_highlight()
                                ));
                            }
                        } else {
                            let mut ancestor = parent.parent();
                            while let Some(path) = ancestor {
                                if path.exists() {
                                    if !path.is_dir() {
                                        validation_errors.push(format!(
                                            "Cannot create target parent directory, ancestor path is not a directory: {}",
                                            path.display().to_string().log_color_error_highlight()
                                        ));
                                    }
                                    break;
                                }
                                ancestor = path.parent();
                            }
                        }
                    }
                }
            }
        }

        if !validation_errors.is_empty() {
            logln("");
            log_failed_to("validate Multi-component layout upgrade plan");
            let _indent = self.ctx.log_handler().decorated_indent_primary();

            logln("");
            logln(
                "Multi-component layout upgrade errors:"
                    .log_color_help_group()
                    .to_string(),
            );
            for error in validation_errors {
                logln(format!("  - {}", error));
            }
            logln("");

            bail!(NonSuccessfulExit);
        }

        Ok(())
    }

    fn apply_multi_component_layout_upgrade_plan(
        &self,
        upgrade_plan: &MultiComponentLayoutUpgradePlan,
    ) -> anyhow::Result<()> {
        if upgrade_plan.is_empty() {
            return Ok(());
        }

        logln("");
        log_action("Applying", "multi-component layout upgrade steps");
        let _indent = LogIndent::new();

        for step in upgrade_plan.steps() {
            match step {
                MultiComponentLayoutUpgradePlanStep::Move { source, target } => {
                    log_action(
                        "Moving",
                        format!(
                            "{} to {}",
                            source.display().to_string().log_color_highlight(),
                            target.display().to_string().log_color_highlight()
                        ),
                    );
                    fs::rename(source, target)?;
                }
            }
        }

        Ok(())
    }

    fn validate_new_template_plan(
        &self,
        unsafe_template_plan: &UnsafeTemplatePlan,
    ) -> anyhow::Result<()> {
        let overwrites = unsafe_template_plan.overwrites().collect::<Vec<_>>();
        let failed_plans = unsafe_template_plan.failed_plans().collect::<Vec<_>>();

        if !overwrites.is_empty() || !failed_plans.is_empty() {
            logln("");
            log_failed_to("plan the required changes to apply the selected template(s)");
            let _indent = self.ctx.log_handler().decorated_indent_primary();

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

    fn log_new_template_plan(&self, safe_template_plan: &SafeTemplatePlan) {
        if safe_template_plan.is_empty() {
            return;
        }

        logln("");
        log_action(
            "Planned",
            "required changes for applying the selected template(s)",
        );
        let _indent = self.ctx.log_handler().decorated_indent_primary();
        for (path, step) in safe_template_plan.file_steps() {
            match step {
                SafeTemplatePlanStep::Create { .. } => {
                    logln(format!(
                        "- {} {}",
                        "create".green(),
                        path.log_color_highlight()
                    ));
                }
                SafeTemplatePlanStep::Merge { current, new } => {
                    logln(format!(
                        "- {} {}",
                        "update".green(),
                        path.log_color_highlight()
                    ));
                    let _indent = LogIndent::new();
                    let _indent = self.ctx.log_handler().decorated_indent_secondary();
                    log_unified_diff_for_path(path, &diff::unified_diff(current, new));
                }
                SafeTemplatePlanStep::SkipSame { .. } => {
                    logln(format!(
                        "- {} {}",
                        "skip".yellow(),
                        path.log_color_highlight(),
                    ));
                }
            }
        }
    }

    fn apply_new_template_plan(&self, safe_template_plan: &SafeTemplatePlan) -> anyhow::Result<()> {
        if safe_template_plan.is_empty() {
            log_skipping_up_to_date("applying template(s)");
            return Ok(());
        }

        logln("");
        log_action("Applying", "template(s)");
        let _indent = LogIndent::new();

        for (path, step) in safe_template_plan.file_steps() {
            match step {
                SafeTemplatePlanStep::Create { new } => {
                    log_action("Creating", format!("{}", path.log_color_highlight()));
                    fs::write_str(path, new)?;
                }
                SafeTemplatePlanStep::Merge { new, .. } => {
                    log_action("Updating", format!("{}", path.log_color_highlight()));
                    fs::write_str(path, new)?;
                }
                SafeTemplatePlanStep::SkipSame { .. } => {
                    log_skipping_up_to_date(format!("updating {}", path.log_color_highlight()));
                }
            }
        }

        Ok(())
    }
}

/// Creates a `.claude` → `.agents` symlink in the application directory so that
/// Claude Code can discover the same skills as Amp/Codex without duplicating files.
pub(crate) fn create_claude_symlink(application_path: &Path) -> anyhow::Result<()> {
    let agents_dir = application_path.join(".agents");
    let claude_link = application_path.join(".claude");

    if !agents_dir.exists() {
        return Ok(());
    }

    if claude_link.exists() || claude_link.is_symlink() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(".agents", &claude_link)?;
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(".agents", &claude_link)?;
    }

    Ok(())
}

fn is_non_empty_dir(path: &Path) -> anyhow::Result<bool> {
    if !path.exists() || !path.is_dir() {
        return Ok(false);
    }

    Ok(std::fs::read_dir(path)?.next().is_some())
}

fn should_switch_to_app_dir_hint(application_dir: &Path) -> anyhow::Result<bool> {
    let current_dir = fs::current_dir_lexical()?;
    let current_main_source = find_main_source_from(&current_dir);

    let Some(current_main_source) = current_main_source else {
        return Ok(true);
    };

    let current_app_root = current_main_source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(current_main_source);

    Ok(!fs::path_eq_normalized(&current_app_root, application_dir))
}
