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

use crate::app::context::{to_anyhow, ApplicationContext};
use crate::app::yaml_edit::AppYamlEditor;
use crate::command::component::ComponentSubcommand;
use crate::command::shared_args::{
    BuildArgs, ComponentOptionalComponentNames, ComponentTemplateName, DeployArgs, ForceBuildArg,
};
use crate::command_handler::component::ifs::IfsFileManager;
use crate::command_handler::component::staging::ComponentStager;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::{HintError, NonSuccessfulExit, ShowClapHelpTarget};
use crate::log::{log_action, log_warn_action, logln, LogColorize, LogIndent};
use crate::model::app::DependencyType;
use crate::model::app::{ApplicationComponentSelectMode, DynamicHelpSections};
use crate::model::component::{
    ComponentDeployProperties, ComponentNameMatchKind, ComponentRevisionSelection,
    ComponentSelection, SelectedComponents,
};
use crate::model::deploy::TryUpdateAllWorkersResult;
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::text::fmt::log_error;
use crate::model::worker::AgentUpdateMode;
use crate::validation::ValidationBuilder;
use anyhow::{anyhow, bail, Context as AnyhowContext};
use futures_util::future::OptionFuture;
use golem_client::api::ComponentClient;
use golem_client::model::{ComponentCreation, ComponentDto};
use golem_common::cache::SimpleCache;
use golem_common::model::agent::AgentType;
use golem_common::model::component::{
    ComponentId, ComponentName, ComponentRevision, ComponentUpdate,
};
use golem_common::model::component_metadata::{
    dynamic_linking_to_diffable, DynamicLinkedInstance, DynamicLinkedWasmRpc, WasmRpcTarget,
};
use golem_common::model::deployment::DeploymentPlanComponentEntry;
use golem_common::model::diff;
use golem_templates::add_component_by_template;
use golem_templates::model::{
    ApplicationName as TemplateApplicationName, GuestLanguage, PackageName,
};
use itertools::Itertools;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

pub mod ifs;
mod staging;
// TODO: atomic: pub mod plugin;
// TODO: atomic: pub mod plugin_installation;

pub struct ComponentCommandHandler {
    ctx: Arc<Context>,
}

impl ComponentCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: ComponentSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ComponentSubcommand::New {
                component_template,
                component_name,
            } => self.cmd_new(component_template, component_name).await,
            ComponentSubcommand::Templates { filter } => {
                self.cmd_templates(filter);
                Ok(())
            }
            ComponentSubcommand::Build {
                component_name,
                build: build_args,
            } => self.cmd_build(component_name, build_args).await,
            ComponentSubcommand::Clean { component_name } => self.cmd_clean(component_name).await,
            ComponentSubcommand::AddDependency {
                component_name,
                target_component_name,
                target_component_path,
                target_component_url,
                dependency_type,
            } => {
                self.cmd_add_dependency(
                    component_name,
                    target_component_name,
                    target_component_path,
                    target_component_url,
                    dependency_type,
                )
                .await
            }
            ComponentSubcommand::List { component_name } => {
                self.cmd_list(component_name.component_name).await
            }
            ComponentSubcommand::Get {
                component_name,
                revision,
            } => self.cmd_get(component_name.component_name, revision).await,

            ComponentSubcommand::UpdateAgents {
                component_name,
                update_mode,
                r#await,
            } => {
                self.cmd_update_workers(component_name.component_name, update_mode, r#await)
                    .await
            }
            ComponentSubcommand::RedeployAgents { component_name } => {
                self.cmd_redeploy_workers(component_name.component_name)
                    .await
            }
            ComponentSubcommand::Plugin { subcommand: _ } => {
                // TODO: atomic
                /*
                self.ctx
                    .component_plugin_handler()
                    .handle_command(subcommand)
                    .await
                */
                todo!()
            }
            ComponentSubcommand::Diagnose { component_name } => {
                self.cmd_diagnose(component_name).await
            }
            ComponentSubcommand::ManifestTrace { component_name } => {
                self.cmd_manifest_trace(component_name).await
            }
        }
    }

    async fn cmd_new(
        &self,
        template: Option<ComponentTemplateName>,
        component_package_name: Option<PackageName>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        // Loading app for:
        //   - checking that we are inside an application
        //   - switching to the root dir as a side effect
        //   - getting existing component names
        let (existing_component_names, application_name) = {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;
            (
                app_ctx
                    .application
                    .component_names()
                    .map(|name| name.to_string())
                    .collect::<HashSet<_>>(),
                app_ctx.application.application_name().clone(),
            )
        };

        let Some((template, component_package_name)) = ({
            match (template, component_package_name) {
                (Some(template), Some(component_package_name)) => {
                    Some((template, component_package_name))
                }
                _ => self
                    .ctx
                    .interactive_handler()
                    .select_new_component_template_and_package_name(
                        existing_component_names.clone(),
                    )?,
            }
        }) else {
            log_error(
                "Both TEMPLATE and COMPONENT_PACKAGE_NAME are required in non-interactive mode",
            );
            logln("");
            self.ctx
                .app_handler()
                .log_templates_help(None, None, self.ctx.dev_mode());
            logln("");
            bail!(HintError::ShowClapHelp(ShowClapHelpTarget::ComponentNew));
        };

        let component_name = ComponentName(component_package_name.to_string_with_colon());

        if existing_component_names.contains(component_name.as_str()) {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;

            log_error(format!("Component {component_name} already exists"));
            logln("");
            app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?;
            bail!(NonSuccessfulExit)
        }

        let app_handler = self.ctx.app_handler();
        let (common_template, component_template) =
            app_handler.get_template(&template, self.ctx.dev_mode())?;

        let application_name = TemplateApplicationName::from(application_name.0);

        match add_component_by_template(
            common_template,
            Some(component_template),
            &PathBuf::from("."),
            &application_name,
            &component_package_name,
            Some(self.ctx.template_sdk_overrides()),
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

        Ok(())
    }

    async fn cmd_build(
        &self,
        component_name: ComponentOptionalComponentNames,
        build_args: BuildArgs,
    ) -> anyhow::Result<()> {
        self.ctx
            .app_handler()
            .build(
                component_name.component_name,
                Some(build_args),
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await
    }

    async fn cmd_clean(
        &self,
        component_name: ComponentOptionalComponentNames,
    ) -> anyhow::Result<()> {
        self.ctx
            .app_handler()
            .clean(
                component_name.component_name,
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await
    }

    fn cmd_templates(&self, filter: Option<String>) {
        match filter {
            Some(filter) => {
                if let Some(language) = GuestLanguage::from_string(filter.clone()) {
                    self.ctx.app_handler().log_templates_help(
                        Some(language),
                        None,
                        self.ctx.dev_mode(),
                    );
                } else {
                    self.ctx.app_handler().log_templates_help(
                        None,
                        Some(&filter),
                        self.ctx.dev_mode(),
                    );
                }
            }
            None => self
                .ctx
                .app_handler()
                .log_templates_help(None, None, self.ctx.dev_mode()),
        }
    }

    async fn cmd_list(&self, _component_name: Option<ComponentName>) -> anyhow::Result<()> {
        // TODO: atomic
        /*
        let show_sensitive = self.ctx.show_sensitive();

        let selected_component_names = self
            .opt_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let mut component_views = Vec::<ComponentView>::new();

        let clients = self.ctx.golem_clients().await?;
        if selected_component_names.component_names.is_empty() {
            let results = clients
                .component
                .get_components(
                    selected_component_names
                        .project
                        .as_ref()
                        .map(|p| &p.project_id.0),
                    None,
                )
                .await
                .map_service_error()?;

            component_views.extend(
                results.into_iter().map(|meta| {
                    ComponentView::new_wit_style(show_sensitive, Component::from(meta))
                }),
            );
        } else {
            for component_name in selected_component_names.component_names.iter() {
                let results = clients
                    .component
                    .get_components(
                        selected_component_names
                            .project
                            .as_ref()
                            .map(|p| &p.project_id.0),
                        Some(&component_name.0),
                    )
                    .await
                    .map_service_error()?
                    .into_iter()
                    .map(|meta| ComponentView::new_wit_style(show_sensitive, Component::from(meta)))
                    .collect::<Vec<_>>();

                if results.is_empty() {
                    log_warn(format!(
                        "No versions found for component {}",
                        component_name.0.log_color_highlight()
                    ));
                } else {
                    component_views.extend(results);
                }
            }
        }

        if component_views.is_empty() && component_name.is_some() {
            // Retry selection (this time with not allowing "not founds")
            // so we get error messages for app component names.
            self.ctx
                .app_handler()
                .opt_select_components(
                    component_name.iter().cloned().collect(),
                    &ApplicationComponentSelectMode::CurrentDir,
                )
                .await?;
        }

        self.ctx.log_handler().log_view(&component_views);

        Ok(())
        */
        todo!()
    }

    async fn cmd_get(
        &self,
        _component_name: Option<ComponentName>,
        _revision: Option<u64>,
    ) -> anyhow::Result<()> {
        // TODO: atomic
        /*
        let selected_components = self
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        if version.is_some() && selected_components.component_names.len() > 1 {
            log_error("Version cannot be specified when multiple components are selected!");
            logln("");
            logln(format!(
                "Selected components: {}",
                selected_components
                    .component_names
                    .iter()
                    .map(|cn| cn.0.log_color_highlight())
                    .join(", ")
            ));
            logln("");
            logln("Specify the requested component name or switch to an application directory with exactly one component!");
            logln("");
            bail!(NonSuccessfulExit);
        }

        let mut component_views = Vec::<ComponentView>::new();

        for component_name in &selected_components.component_names {
            let component = self
                .component(
                    selected_components.environment.as_ref(),
                    component_name.into(),
                    version.map(|version| version.into()),
                )
                .await?;

            if let Some(component) = component {
                component_views.push(ComponentView::new_wit_style(
                    self.ctx.show_sensitive(),
                    component,
                ));
            }
        }

        if component_views.is_empty() && component_name.is_some() {
            // Retry selection (this time with not allowing "not founds")
            // so we get error messages for app component names.
            self.ctx
                .app_handler()
                .opt_select_components(
                    component_name.iter().cloned().collect(),
                    &ApplicationComponentSelectMode::CurrentDir,
                )
                .await?;
        }

        let no_matches = component_views.is_empty();
        for component_view in component_views {
            self.ctx
                .log_handler()
                .log_view(&ComponentGetView(component_view));
            logln("");
        }

        if no_matches {
            if version.is_some() && selected_components.component_names.len() == 1 {
                log_error("Component version not found");
                let clients = self.ctx.golem_clients().await?;

                let versions = clients
                    .component
                    .get_components(
                        selected_components
                            .project
                            .as_ref()
                            .map(|p| &p.project_id.0),
                        Some(&selected_components.component_names[0].0),
                    )
                    .await
                    .map_service_error()
                    .map(|components| {
                        components
                            .into_iter()
                            .map(Component::from)
                            .collect::<Vec<_>>()
                    });

                if let Ok(versions) = versions {
                    logln("");
                    logln(
                        "Available component versions:"
                            .log_color_help_group()
                            .to_string(),
                    );
                    for version in versions {
                        logln(format!("- {}", version.versioned_component_id.version));
                    }
                }
            } else {
                log_error("Component not found");
            }

            bail!(NonSuccessfulExit)
        }

        Ok(())
        */
        todo!()
    }

    async fn cmd_update_workers(
        &self,
        _component_name: Option<ComponentName>,
        _update_mode: AgentUpdateMode,
        _await_update: bool,
    ) -> anyhow::Result<()> {
        // TODO: atomic
        /*
        let components = self.components_for_deploy_args(component_name).await?;
        self.update_workers_by_components(&components, update_mode, await_update)
            .await?;

        Ok(())
        */
        todo!()
    }

    async fn cmd_redeploy_workers(
        &self,
        _component_name: Option<ComponentName>,
    ) -> anyhow::Result<()> {
        // TODO: atomic
        /*
        let components = self.components_for_deploy_args(component_name).await?;
        self.redeploy_workers_by_components(&components).await?;

        Ok(())
        */
        todo!()
    }

    async fn cmd_diagnose(
        &self,
        component_names: ComponentOptionalComponentNames,
    ) -> anyhow::Result<()> {
        self.ctx
            .app_handler()
            .diagnose(
                component_names.component_name,
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await
    }

    // TODO: atomic: cleanup before release
    async fn cmd_manifest_trace(
        &self,
        _component_names: ComponentOptionalComponentNames,
    ) -> anyhow::Result<()> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let component_names = app_ctx
            .application
            .component_names()
            .cloned()
            .collect::<Vec<_>>();
        for component_name in component_names {
            log_action(
                "Showing",
                format!(
                    "manifest trace for {}",
                    component_name.as_str().log_color_highlight()
                ),
            );
            let _indent = self.ctx.log_handler().nested_text_view_indent();
            self.ctx.log_handler().log_serializable(
                &app_ctx
                    .application
                    .component(&component_name)
                    .layer_properties()
                    .with_compacted_traces(),
            )
        }

        Ok(())
    }

    async fn cmd_add_dependency(
        &self,
        component_name: Option<ComponentName>,
        target_component_name: Option<ComponentName>,
        target_component_path: Option<PathBuf>,
        target_component_url: Option<Url>,
        dependency_type: Option<DependencyType>,
    ) -> anyhow::Result<()> {
        self.ctx.silence_app_context_init().await;

        let Some((component_name, target_component_source, dependency_type)) = self
            .ctx
            .interactive_handler()
            .create_component_dependency(
                component_name,
                target_component_name,
                target_component_path,
                target_component_url,
                dependency_type,
            )
            .await?
        else {
            log_error("All of COMPONENT_NAME, (TARGET_COMPONENT_NAME/TARGET_COMPONENT_PATH/TARGET_COMPONENT_URL) and DEPENDENCY_TYPE are required in non-interactive mode");
            logln("");
            bail!(HintError::ShowClapHelp(
                ShowClapHelpTarget::ComponentAddDependency
            ));
        };

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let mut editor = AppYamlEditor::new(&app_ctx.application);

        let inserted = editor.insert_or_update_dependency(
            &component_name,
            &target_component_source,
            dependency_type,
        )?;

        editor.update_documents()?;

        if inserted {
            log_action("Added", "component dependency");
        } else {
            log_action("Updated", "component dependency");
        }

        Ok(())
    }

    // TODO: atomic
    /*
    async fn components_for_update_or_redeploy(
        &self,
        component_name: Option<ComponentName>,
    ) -> anyhow::Result<Vec<Component>> {
        let selected_component_names = self
            .opt_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let mut components = Vec::with_capacity(selected_component_names.component_names.len());
        for component_name in &selected_component_names.component_names {
            match self
                .component(
                    selected_component_names.environment.as_ref(),
                    component_name.into(),
                    None,
                )
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
    */

    pub async fn update_workers_by_components(
        &self,
        components: &[ComponentDto],
        update: AgentUpdateMode,
        await_updates: bool,
    ) -> anyhow::Result<()> {
        if components.is_empty() {
            return Ok(());
        }

        log_action("Updating", format!("existing workers using {update} mode"));
        let _indent = LogIndent::new();

        let mut update_results = TryUpdateAllWorkersResult::default();
        for component in components {
            let result = self
                .ctx
                .worker_handler()
                .update_component_workers(
                    &component.component_name,
                    &component.id,
                    update,
                    component.revision,
                    await_updates,
                )
                .await?;
            update_results.extend(result);
        }

        self.ctx.log_handler().log_view(&update_results);
        Ok(())
    }

    pub async fn redeploy_workers_by_components(
        &self,
        components: &[ComponentDto],
    ) -> anyhow::Result<()> {
        if components.is_empty() {
            return Ok(());
        }

        log_action("Redeploying", "existing workers");
        let _indent = LogIndent::new();

        for component in components {
            self.ctx
                .worker_handler()
                .redeploy_component_workers(&component.component_name, &component.id)
                .await?;
        }

        // TODO: json / yaml output?
        // TODO: unlike updating, redeploy is short-circuiting, should we normalize?
        Ok(())
    }

    pub async fn delete_workers(&self, components: &[ComponentDto]) -> anyhow::Result<()> {
        if components.is_empty() {
            return Ok(());
        }

        log_action("Deleting", "existing workers");
        let _indent = LogIndent::new();

        // NOTE: for now we naively keep deleting in a loop until we do not find any more agents,
        //       we do so to help a bit with pending invocations or currently running worker creations,
        //       but this is not a 100% guarantee.
        let mut found_any = true;
        let mut first_round = true;
        while found_any {
            found_any = false;
            for component in components {
                let deleted_count = self
                    .ctx
                    .worker_handler()
                    .delete_component_workers(&component.component_name, &component.id, first_round)
                    .await?;
                if deleted_count > 0 {
                    found_any = true;
                }
            }
            first_round = false;
        }

        // TODO: json / yaml output?
        Ok(())
    }

    pub async fn opt_select_components_by_app_dir_or_name(
        &self,
        component_name: Option<&ComponentName>,
    ) -> anyhow::Result<SelectedComponents> {
        self.select_components_by_app_dir_or_name_internal(component_name, true)
            .await
    }

    pub async fn must_select_components_by_app_dir_or_name(
        &self,
        component_name: Option<&ComponentName>,
    ) -> anyhow::Result<SelectedComponents> {
        self.select_components_by_app_dir_or_name_internal(component_name, false)
            .await
    }

    async fn select_components_by_app_dir_or_name_internal(
        &self,
        _component_name: Option<&ComponentName>,
        _allow_no_matches: bool,
    ) -> anyhow::Result<SelectedComponents> {
        // TODO: atomic
        /*
        fn empty_checked<'a>(name: &'a str, value: &'a str) -> anyhow::Result<&'a str> {
            if value.is_empty() {
                log_error(format!("Missing {name} part in component name!"));
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

        self.ctx.silence_app_context_init().await;

        let (account, project, component_name): (
            Option<AccountDetails>,
            Option<ProjectRefAndId>,
            Option<ComponentName>,
        ) = {
            match component_name {
                Some(component_name) => {
                    let segments = component_name.0.split("/").collect::<Vec<_>>();
                    match segments.len() {
                        1 => (
                            None,
                            None,
                            Some(empty_checked_component(segments[0])?.into()),
                        ),
                        2 => (
                            None,
                            Some(
                                self.ctx
                                    .cloud_project_handler()
                                    .select_project(&ProjectReference::JustName(
                                        empty_checked_project(segments[0])?.into(),
                                    ))
                                    .await?,
                            ),
                            Some(empty_checked_component(segments[1])?.into()),
                        ),
                        3 => {
                            let account_email = empty_checked_account(segments[0])?.to_string();
                            let account = self
                                .ctx
                                .select_account_by_email_or_error(&account_email)
                                .await?;
                            (
                                Some(account.clone()),
                                Some(
                                    self.ctx
                                        .cloud_project_handler()
                                        .select_project(&ProjectReference::WithAccount {
                                            account_email,
                                            project_name: empty_checked_project(segments[1])?
                                                .into(),
                                        })
                                        .await?,
                                ),
                                Some(empty_checked_component(segments[2])?.into()),
                            )
                        }
                        _ => {
                            log_error(format!(
                                "Failed to parse component name: {}",
                                component_name.0.log_color_error_highlight()
                            ));
                            logln("");
                            log_text_view(&ComponentNameHelp);
                            bail!(NonSuccessfulExit);
                        }
                    }
                }
                None => (None, None, None),
            }
        };

        let app_select_success = self
            .ctx
            .app_handler()
            .opt_select_components_allow_not_found(
                component_name.clone().into_iter().collect(),
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await?;

        let selected_component_names = {
            if app_select_success {
                let app_ctx = self.ctx.app_context_lock().await;
                app_ctx
                    .opt()?
                    .map(|app_ctx| {
                        app_ctx
                            .selected_component_names()
                            .iter()
                            .map(|cn| ComponentName::try_from(cn.as_str()))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()
                    .map_err(|err| anyhow!(err))?
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
            } else {
                component_name.clone().into_iter().collect::<Vec<_>>()
            }
        };

        if selected_component_names.is_empty() && component_name.is_none() && !allow_no_matches {
            log_error("No components were selected based on the current directory an no component was requested.");
            logln("");
            logln(
                "Please specify a requested component name or switch to an application directory!",
            );
            logln("");
            bail!(NonSuccessfulExit);
        }

        Ok(SelectedComponents {
            account,
            project,
            component_names: selected_component_names,
        })
         */
        todo!()
    }

    pub async fn component_by_name_with_auto_deploy(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_match_kind: ComponentNameMatchKind,
        component_name: &ComponentName,
        component_revision_selection: Option<ComponentRevisionSelection<'_>>,
        deploy_args: Option<&DeployArgs>,
    ) -> anyhow::Result<ComponentDto> {
        match self
            .resolve_component(
                environment,
                component_name.into(),
                component_revision_selection,
            )
            .await?
        {
            Some(component) => Ok(component),
            None => {
                let should_deploy = match component_match_kind {
                    ComponentNameMatchKind::AppCurrentDir => true,
                    ComponentNameMatchKind::App => true,
                    ComponentNameMatchKind::Unknown => false,
                };

                if !should_deploy {
                    logln("");
                    log_error(format!(
                        "Component {} not found, and not part of the current application",
                        component_name.0.log_color_highlight()
                    ));
                    // TODO: fuzzy match from service to list components?

                    let app_ctx = self.ctx.app_context_lock().await;
                    if let Some(app_ctx) = app_ctx.opt()? {
                        logln("");
                        app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?
                    }

                    bail!(NonSuccessfulExit)
                }

                if self
                    .ctx
                    .interactive_handler()
                    .confirm_auto_deploy_component(component_name)?
                {
                    log_action(
                        "Auto deploying application",
                        format!(
                            "for creating missing component {}",
                            component_name.0.log_color_highlight()
                        ),
                    );
                    self.ctx
                        .app_handler()
                        .deploy(
                            false,
                            false,
                            false,
                            ForceBuildArg { force_build: false },
                            deploy_args.cloned().unwrap_or_else(DeployArgs::none),
                        )
                        .await?;
                    self.ctx
                        .component_handler()
                        .resolve_component(environment, component_name.into(), None)
                        .await?
                        .ok_or_else(|| {
                            anyhow!("Component ({}) not found after deployment", component_name)
                        })
                } else {
                    bail!(NonSuccessfulExit)
                }
            }
        }
    }

    // TODO: merge these 3 args into "component lookup" or "selection" struct
    pub async fn resolve_component(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name_or_id: ComponentSelection<'_>,
        component_version_selection: Option<ComponentRevisionSelection<'_>>,
    ) -> anyhow::Result<Option<ComponentDto>> {
        let component = match component_name_or_id {
            ComponentSelection::Name(component_name) => {
                self.latest_deployed_server_component_by_name(environment, component_name)
                    .await?
            }
            ComponentSelection::Id(component_id) => {
                self.server_component_by_id(&component_id).await?
            }
        };

        match (component, component_version_selection) {
            (Some(component), Some(component_version_selection)) => {
                let revision = match component_version_selection {
                    ComponentRevisionSelection::ByWorkerName(worker_name) => self
                        .ctx
                        .worker_handler()
                        .worker_metadata(component.id.0, &component.component_name, worker_name)
                        .await
                        .ok()
                        .map(|worker_metadata| worker_metadata.component_version),
                    ComponentRevisionSelection::ByExplicitRevision(version) => Some(version),
                };

                match revision {
                    Some(revision) => {
                        let clients = self.ctx.golem_clients().await?;

                        let component = clients
                            .component
                            .get_component_revision(&component.id.0, revision.0)
                            .await
                            .map_service_error()?;

                        Ok(Some(component))
                    }
                    None => Ok(Some(component)),
                }
            }
            (Some(component), None) => Ok(Some(component)),
            (None, _) => Ok(None),
        }
    }

    pub async fn latest_deployed_server_component_by_name(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name: &ComponentName,
    ) -> anyhow::Result<Option<ComponentDto>> {
        self.ctx
            .golem_clients()
            .await?
            .component
            .get_environment_component(&environment.environment_id.0, component_name.0.as_str())
            .await
            .map_service_error_not_found_as_opt()
    }

    pub async fn server_component_by_id(
        &self,
        component_id: &Uuid,
    ) -> anyhow::Result<Option<ComponentDto>> {
        self.ctx
            .golem_clients()
            .await?
            .component
            .get_component(component_id)
            .await
            .map_service_error_not_found_as_opt()
    }

    pub async fn all_deployable_manifest_components(
        &self,
    ) -> anyhow::Result<BTreeMap<ComponentName, ComponentDeployProperties>> {
        let component_names = {
            let app_ctx = self.ctx.app_context_lock().await;
            app_ctx.some_or_err()?.deployable_component_names()
        };

        let mut components = BTreeMap::<ComponentName, ComponentDeployProperties>::new();
        for component_name in component_names {
            let properties = self.component_deploy_properties(&component_name).await?;
            components.insert(component_name, properties);
        }

        Ok(components)
    }

    pub async fn component_deploy_properties(
        &self,
        component_name: &ComponentName,
    ) -> anyhow::Result<ComponentDeployProperties> {
        let mut app_ctx = self.ctx.app_context_lock_mut().await?;
        let app_ctx = app_ctx.some_or_err_mut()?;

        let component = app_ctx.application.component(component_name);
        let linked_wasm_path = component.final_linked_wasm();
        if !component.component_type().is_deployable() {
            bail!("Component {component_name} is not deployable");
        }
        let files = component.files().clone();
        let env = resolve_env_vars(component_name, component.env())?;
        let dynamic_linking = app_component_dynamic_linking(app_ctx, component_name)?;

        Ok(ComponentDeployProperties {
            linked_wasm_path,
            files,
            dynamic_linking,
            env,
        })
    }

    pub async fn diffable_local_component(
        &self,
        component_name: &ComponentName,
        properties: &ComponentDeployProperties,
    ) -> anyhow::Result<diff::Component> {
        // TODO: atomic: cache it with a TaskResultMarker?
        let component_binary_hash = {
            log_action(
                "Calculating hash",
                format!(
                    "for component {} binary",
                    component_name.as_str().log_color_highlight()
                ),
            );
            let file = std::fs::File::open(&properties.linked_wasm_path)?;
            let mut component_hasher = blake3::Hasher::new();
            component_hasher
                .update_reader(&file)
                .context("Failed to hash component binary")?;
            component_hasher.finalize()
        };

        // TODO: atomic: cache it with a TaskResultMarker (handling local vs http)?
        let files: BTreeMap<String, diff::HashOf<diff::ComponentFile>> = {
            IfsFileManager::new(self.ctx.file_download_client().clone())
                .collect_file_hashes(component_name.as_str(), properties.files.as_slice())
                .await?
                .into_iter()
                .map(|file_hash| {
                    (
                        file_hash.target.path.to_abs_string(),
                        diff::ComponentFile {
                            hash: file_hash.hash.into(),
                            permissions: file_hash.target.permissions,
                        }
                        .into(),
                    )
                })
                .collect()
        };

        Ok(diff::Component {
            metadata: diff::ComponentMetadata {
                version: Some("TODO".to_string()), // TODO: atomic
                env: properties
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                dynamic_linking_wasm_rpc: dynamic_linking_to_diffable(&properties.dynamic_linking),
            }
            .into(),
            wasm_hash: component_binary_hash.into(),
            files_by_path: files,
            plugins_by_priority: Default::default(), // TODO: atomic: plugins
        })
    }

    pub async fn create_staged_component(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name: &ComponentName,
        component_deploy_properties: &ComponentDeployProperties,
    ) -> anyhow::Result<()> {
        log_action(
            "Creating",
            format!("component {}", component_name.0.log_color_highlight()),
        );
        let _indent = LogIndent::new();

        let component_stager = ComponentStager::new(
            self.ctx.clone(),
            component_name,
            component_deploy_properties,
            None,
        );

        let linked_wasm = component_stager.open_linked_wasm().await?;
        let agent_types: Vec<AgentType> = component_stager.agent_types().await?;

        // NOTE: do not drop until the component is created, keeps alive the temp archive
        let files = component_stager.all_files().await?;

        self.ctx
            .golem_clients()
            .await?
            .component
            .create_component(
                &environment.environment_id.0,
                &ComponentCreation {
                    component_name: component_name.clone(),
                    file_options: files
                        .as_ref()
                        .map(|files| files.file_options.clone())
                        .unwrap_or_default(),
                    dynamic_linking: component_stager.dynamic_linking(),
                    env: component_stager.env(),
                    agent_types,
                    plugins: Default::default(), // TODO: atomic, collect plugins from manifest
                },
                linked_wasm,
                OptionFuture::from(files.as_ref().map(|files| files.open_archive()))
                    .await
                    .transpose()?,
            )
            .await
            .map_service_error()?;

        Ok(())
    }

    pub async fn delete_staged_component(
        &self,
        component: &DeploymentPlanComponentEntry,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Deleting",
            format!("component {}", component.name.0.log_color_highlight()),
        );
        let _indent = LogIndent::new();

        self.ctx
            .golem_clients()
            .await?
            .component
            .delete_component(&component.id.0, component.revision.0)
            .await
            .map_service_error()?;

        Ok(())
    }

    pub async fn update_staged_component(
        &self,
        component: &DeploymentPlanComponentEntry,
        component_deploy_properties: &ComponentDeployProperties,
        diff: &diff::DiffForHashOf<diff::Component>,
    ) -> anyhow::Result<()> {
        log_action(
            "Updating",
            format!("component {}", component.name.0.log_color_highlight()),
        );
        let _indent = LogIndent::new();

        let component_stager = ComponentStager::new(
            self.ctx.clone(),
            &component.name,
            component_deploy_properties,
            Some(diff),
        );

        let linked_wasm = component_stager.open_linked_wasm_if_changed().await?;
        let agent_types = component_stager.agent_types_if_changed().await?;

        // NOTE: do not drop until the component is created, keeps alive the temp archive
        let changed_files = component_stager.changed_files().await?;

        self.ctx
            .golem_clients()
            .await?
            .component
            .update_component(
                &component.id.0,
                &ComponentUpdate {
                    current_revision: component.revision,
                    removed_files: changed_files.removed.clone(),
                    new_file_options: changed_files.merged_file_options(),
                    dynamic_linking: component_stager.dynamic_linking_if_changed(),
                    env: component_stager.env_if_changed(),
                    agent_types,
                    plugin_updates: Default::default(), // TODO: atomic, from diff
                },
                linked_wasm,
                changed_files.open_archive().await?,
            )
            .await
            .map_service_error()?;

        Ok(())
    }

    pub async fn get_component_revision_by_id(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
    ) -> anyhow::Result<ComponentDto> {
        self.ctx
            .caches()
            .component_revision
            .get_or_insert_simple(&(component_id.clone(), revision), {
                let ctx = self.ctx.clone();
                async move || {
                    ctx.golem_clients()
                        .await?
                        .component
                        .get_component_revision(&component_id.0, revision.0)
                        .await
                        .map_service_error()
                        .map_err(Arc::new)
                }
            })
            .await
            .map_err(|err| anyhow!(err))
    }
}

fn app_component_dynamic_linking(
    app_ctx: &mut ApplicationContext,
    component_name: &ComponentName,
) -> anyhow::Result<HashMap<String, DynamicLinkedInstance>> {
    let mut mapping = Vec::new();

    let wasm_rpc_deps = app_ctx
        .application
        .component_dependencies(component_name)
        .iter()
        .filter(|dep| dep.dep_type == DependencyType::DynamicWasmRpc)
        .filter_map(|dep| dep.as_dependent_app_component())
        .collect::<Vec<_>>();

    for wasm_rpc_dep in wasm_rpc_deps {
        mapping.push(app_ctx.component_stub_interfaces(&wasm_rpc_dep.name)?);
    }

    Ok(mapping
        .into_iter()
        .map(|stub_interfaces| {
            (
                stub_interfaces.stub_interface_name,
                DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                    targets: HashMap::from_iter(
                        stub_interfaces
                            .exported_interfaces_per_stub_resource
                            .into_iter()
                            .map(|(resource_name, interface_name)| {
                                (
                                    resource_name,
                                    WasmRpcTarget {
                                        interface_name,
                                        component_name: stub_interfaces
                                            .component_name
                                            .as_str()
                                            .to_string(),
                                    },
                                )
                            }),
                    ),
                }),
            )
        })
        .collect())
}

fn resolve_env_vars(
    component_name: &ComponentName,
    env: &BTreeMap<String, String>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let proc_env_vars = minijinja::value::Value::from(std::env::vars().collect::<HashMap<_, _>>());

    let minijinja_env = {
        let mut env = minijinja::Environment::new();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        env
    };

    let mut resolved_env = BTreeMap::new();
    let mut validation = ValidationBuilder::new();
    validation.with_context(
        vec![("component", component_name.to_string())],
        |validation| {
            for key in env.keys().sorted() {
                let value = env.get(key).unwrap();
                match minijinja_env.render_str(value, &proc_env_vars) {
                    Ok(resolved_value) => {
                        resolved_env.insert(key.clone(), resolved_value);
                    }
                    Err(err) => {
                        validation.with_context(
                            vec![
                                ("key", key.to_string()),
                                ("template", value.to_string()),
                                (
                                    "error",
                                    err.to_string().log_color_error_highlight().to_string(),
                                ),
                            ],
                            |validation| {
                                validation.add_error(
                                    "Failed to substitute environment variable".to_string(),
                                )
                            },
                        );
                    }
                };
            }
        },
    );

    to_anyhow(
        &format!(
            "Failed to prepare environment variables for component: {}",
            component_name.as_str().log_color_highlight()
        ),
        validation.build(resolved_env),
        None,
    )
}
