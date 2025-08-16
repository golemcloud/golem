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

use crate::app::build::task_result_marker::{
    GetServerComponentHash, GetServerIfsFileHash, TaskResultMarker,
};
use crate::app::context::ApplicationContext;
use crate::app::yaml_edit::AppYamlEditor;
use crate::command::component::ComponentSubcommand;
use crate::command::shared_args::{
    BuildArgs, ComponentOptionalComponentNames, ComponentTemplateName, ForceBuildArg,
    UpdateOrRedeployArgs,
};
use crate::command_handler::component::ifs::IfsFileManager;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::{HintError, NonSuccessfulExit, ShowClapHelpTarget};
use crate::log::{
    log_action, log_skipping_up_to_date, log_warn_action, logln, LogColorize, LogIndent,
};
use crate::model::app::{
    AppComponentName, ApplicationComponentSelectMode, BuildProfileName, DynamicHelpSections,
};
use crate::model::app::{DependencyType, InitialComponentFile};
use crate::model::component::{Component, ComponentSelection, ComponentView};
use crate::model::deploy::TryUpdateAllWorkersResult;
use crate::model::deploy_diff::component::{DiffableComponent, DiffableComponentFile};
use crate::model::text::component::{ComponentCreateView, ComponentGetView, ComponentUpdateView};
use crate::model::text::fmt::{log_deploy_diff, log_error, log_text_view, log_warn};
use crate::model::text::help::ComponentNameHelp;
use crate::model::{
    AccountDetails, ComponentName, ComponentNameMatchKind, ComponentVersionSelection,
    ProjectRefAndId, ProjectReference, SelectedComponents, WorkerUpdateMode,
};
use anyhow::{anyhow, bail, Context as AnyhowContext};
use golem_client::api::ComponentClient;
use golem_client::model::ComponentQuery;
use golem_client::model::ComponentSearch as ComponentSearchCloud;
use golem_client::model::ComponentSearchParameters as ComponentSearchParametersCloud;
use golem_client::model::DynamicLinkedInstance as DynamicLinkedInstanceOss;
use golem_client::model::DynamicLinkedWasmRpc as DynamicLinkedWasmRpcOss;
use golem_client::model::DynamicLinking as DynamicLinkingOss;
use golem_client::model::{AgentTypes, ComponentEnv as ComponentEnvCloud};
use golem_common::model::agent::AgentType;
use golem_common::model::component_metadata::WasmRpcTarget;
use golem_common::model::{ComponentId, ComponentType};
use golem_templates::add_component_by_template;
use golem_templates::model::{GuestLanguage, PackageName};
use itertools::Itertools;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tracing::debug;
use url::Url;

pub mod ifs;
pub mod plugin;
pub mod plugin_installation;

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
            ComponentSubcommand::Deploy {
                component_name,
                force_build,
                update_or_redeploy,
            } => {
                self.cmd_deploy(component_name, force_build, update_or_redeploy)
                    .await
            }
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
                version,
            } => self.cmd_get(component_name.component_name, version).await,

            ComponentSubcommand::UpdateWorkers {
                component_name,
                update_mode,
                r#await,
            } => {
                self.cmd_update_workers(component_name.component_name, update_mode, r#await)
                    .await
            }
            ComponentSubcommand::RedeployWorkers { component_name } => {
                self.cmd_redeploy_workers(component_name.component_name)
                    .await
            }
            ComponentSubcommand::Plugin { subcommand } => {
                self.ctx
                    .component_plugin_handler()
                    .handle_command(subcommand)
                    .await
            }
            ComponentSubcommand::Diagnose { component_name } => {
                self.cmd_diagnose(component_name).await
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
        let existing_component_names = {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;
            app_ctx
                .application
                .component_names()
                .map(|name| name.to_string())
                .collect::<HashSet<_>>()
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
            self.ctx.app_handler().log_templates_help(None, None);
            logln("");
            bail!(HintError::ShowClapHelp(ShowClapHelpTarget::ComponentNew));
        };

        let component_name = AppComponentName::from(component_package_name.to_string_with_colon());

        if existing_component_names.contains(component_name.as_str()) {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;

            log_error(format!("Component {component_name} already exists"));
            logln("");
            app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?;
            bail!(NonSuccessfulExit)
        }

        let app_handler = self.ctx.app_handler();
        let (common_template, component_template) = app_handler.get_template(&template)?;

        // Unloading app context, so we can reload after the new component is created
        self.ctx.unload_app_context().await;

        match add_component_by_template(
            common_template,
            Some(component_template),
            &PathBuf::from("."),
            &component_package_name,
        ) {
            Ok(()) => {
                log_action(
                    "Added",
                    format!(
                        "new app component {}, loading application manifest...",
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

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        logln("");
        app_ctx.log_dynamic_help(&DynamicHelpSections::show_components())?;

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

    async fn cmd_deploy(
        &self,
        component_name: ComponentOptionalComponentNames,
        force_build: ForceBuildArg,
        update_or_redeploy: UpdateOrRedeployArgs,
    ) -> anyhow::Result<()> {
        self.deploy(
            self.ctx
                .cloud_project_handler()
                .opt_select_project(None)
                .await?
                .as_ref(),
            component_name.component_name,
            Some(force_build),
            &ApplicationComponentSelectMode::CurrentDir,
            &update_or_redeploy,
        )
        .await?;

        Ok(())
    }

    fn cmd_templates(&self, filter: Option<String>) {
        match filter {
            Some(filter) => {
                if let Some(language) = GuestLanguage::from_string(filter.clone()) {
                    self.ctx
                        .app_handler()
                        .log_templates_help(Some(language), None);
                } else {
                    self.ctx
                        .app_handler()
                        .log_templates_help(None, Some(&filter));
                }
            }
            None => self.ctx.app_handler().log_templates_help(None, None),
        }
    }

    async fn cmd_list(&self, component_name: Option<ComponentName>) -> anyhow::Result<()> {
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
                results
                    .into_iter()
                    .map(|meta| ComponentView::new(show_sensitive, Component::from(meta))),
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
                    .map(|meta| ComponentView::new(show_sensitive, Component::from(meta)))
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

        if component_views.is_empty() {
            bail!(NonSuccessfulExit)
        } else {
            self.ctx.log_handler().log_view(&component_views);
        }

        Ok(())
    }

    async fn cmd_get(
        &self,
        component_name: Option<ComponentName>,
        version: Option<u64>,
    ) -> anyhow::Result<()> {
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
                    selected_components.project.as_ref(),
                    component_name.into(),
                    version.map(|version| version.into()),
                )
                .await?;

            if let Some(component) = component {
                component_views.push(ComponentView::new(self.ctx.show_sensitive(), component));
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
    }

    async fn cmd_update_workers(
        &self,
        component_name: Option<ComponentName>,
        update_mode: WorkerUpdateMode,
        await_update: bool,
    ) -> anyhow::Result<()> {
        let components = self
            .components_for_update_or_redeploy(component_name)
            .await?;
        self.update_workers_by_components(&components, update_mode, await_update)
            .await?;

        Ok(())
    }

    async fn cmd_redeploy_workers(
        &self,
        component_name: Option<ComponentName>,
    ) -> anyhow::Result<()> {
        let components = self
            .components_for_update_or_redeploy(component_name)
            .await?;
        self.redeploy_workers_by_components(&components).await?;

        Ok(())
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
                component_name.map(|cn| cn.0.into()),
                target_component_name.map(|cn| cn.0.into()),
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

    pub async fn deploy(
        &self,
        project: Option<&ProjectRefAndId>,
        component_names: Vec<ComponentName>,
        force_build: Option<ForceBuildArg>,
        default_component_select_mode: &ApplicationComponentSelectMode,
        update_or_redeploy: &UpdateOrRedeployArgs,
    ) -> anyhow::Result<Vec<Component>> {
        self.ctx
            .app_handler()
            .build(
                component_names,
                force_build.map(|force_build| BuildArgs {
                    step: vec![],
                    force_build,
                }),
                default_component_select_mode,
            )
            .await?;

        let selected_component_names = {
            let app_ctx = self.ctx.app_context_lock().await;
            app_ctx
                .some_or_err()?
                .selected_component_names()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
        };
        let build_profile = self.ctx.build_profile().cloned();

        let plugin_installation_handler = self.ctx.plugin_installation_handler();

        let components = {
            log_action("Deploying", "components");
            let _indent = LogIndent::new();

            let mut components = Vec::with_capacity(selected_component_names.len());
            for component_name in &selected_component_names {
                let app_ctx = self.ctx.app_context_lock().await;
                if app_ctx
                    .some_or_err()?
                    .application
                    .component_properties(component_name, build_profile.as_ref())
                    .is_deployable()
                {
                    drop(app_ctx);

                    let component = self
                        .deploy_component(build_profile.as_ref(), project, component_name)
                        .await?;
                    let component = plugin_installation_handler
                        .apply_plugin_installation_changes(
                            component_name,
                            build_profile.as_ref(),
                            component,
                        )
                        .await?;
                    components.push(component);
                }
            }

            components
        };

        if let Some(update) = update_or_redeploy.update_workers {
            self.update_workers_by_components(&components, update, true)
                .await?;
        } else if update_or_redeploy.redeploy_workers(self.ctx.update_or_redeploy()) {
            self.redeploy_workers_by_components(&components).await?;
        }

        Ok(components)
    }

    async fn deploy_component(
        &self,
        build_profile: Option<&BuildProfileName>,
        project: Option<&ProjectRefAndId>,
        component_name: &AppComponentName,
    ) -> anyhow::Result<Component> {
        let server_component = self
            .component(
                project,
                (&ComponentName::from(component_name.as_str())).into(),
                None,
            )
            .await?;
        let deploy_properties = {
            let mut app_ctx = self.ctx.app_context_lock_mut().await?;
            let app_ctx = app_ctx.some_or_err_mut()?;
            component_deploy_properties(app_ctx, component_name, build_profile)?
        };
        let component_id = server_component
            .as_ref()
            .map(|c| c.versioned_component_id.component_id);

        let manifest_diffable_component = self
            .manifest_diffable_component(component_name, &deploy_properties)
            .await?;

        if let Some(server_component) = server_component {
            let server_diffable_component = self
                .server_diffable_component(project, &server_component)
                .await?;

            if server_diffable_component == manifest_diffable_component {
                log_skipping_up_to_date(format!(
                    "deploying component {}",
                    component_name.as_str().log_color_highlight()
                ));
                return Ok(server_component);
            } else {
                log_warn_action(
                    "Found",
                    format!(
                        "changes for component {}",
                        component_name.as_str().log_color_highlight()
                    ),
                );

                {
                    let _indent = self.ctx.log_handler().nested_text_view_indent();
                    log_deploy_diff(&server_diffable_component, &manifest_diffable_component)?;
                }
            }
        }

        let linked_wasm = File::open(&deploy_properties.linked_wasm_path)
            .await
            .with_context(|| {
                anyhow!(
                    "Failed to open component linked WASM at {}",
                    deploy_properties
                        .linked_wasm_path
                        .display()
                        .to_string()
                        .log_color_error_highlight()
                )
            })?;

        let ifs_files = {
            if !deploy_properties.files.is_empty() {
                Some(
                    IfsFileManager::new(self.ctx.file_download_client().clone())
                        .build_files_archive(deploy_properties.files.as_slice())
                        .await?,
                )
            } else {
                None
            }
        };
        let ifs_properties = ifs_files.as_ref().map(|f| &f.properties);
        let ifs_archive = {
            if let Some(files) = ifs_files.as_ref() {
                Some(File::open(&files.archive_path).await.with_context(|| {
                    anyhow!(
                        "Failed to open IFS archive: {}",
                        files.archive_path.display()
                    )
                })?)
            } else {
                None
            }
        };

        let agent_types: Option<Vec<AgentType>> = {
            let mut app_ctx = self.ctx.app_context_lock_mut().await?;
            let app_ctx = app_ctx.some_or_err_mut()?;
            if app_ctx.wit.is_agent(component_name) {
                let agent_types = app_ctx
                    .wit
                    .get_extracted_agent_types(component_name, &deploy_properties.linked_wasm_path)
                    .await?;

                debug!("Deploying agent type information for {component_name}: {agent_types:#?}");

                Some(agent_types)
            } else {
                None
            }
        };

        let component = match component_id {
            Some(component_id) => {
                log_action(
                    "Updating",
                    format!(
                        "component {}",
                        component_name.as_str().log_color_highlight()
                    ),
                );

                let clients = self.ctx.golem_clients().await?;

                let component = {
                    let component = clients
                        .component
                        .update_component(
                            &component_id,
                            Some(&deploy_properties.component_type),
                            linked_wasm,
                            ifs_properties,
                            ifs_archive,
                            deploy_properties.dynamic_linking.as_ref(),
                            deploy_properties
                                .env
                                .map(|env| ComponentEnvCloud { key_values: env })
                                .as_ref(),
                            agent_types.map(|types| AgentTypes { types }).as_ref(),
                        )
                        .await
                        .map_service_error()?;

                    Component::from(component)
                };

                self.ctx
                    .log_handler()
                    .log_view(&ComponentUpdateView(ComponentView::new(
                        self.ctx.show_sensitive(),
                        component.clone(),
                    )));
                component
            }
            None => {
                log_action(
                    "Creating",
                    format!(
                        "component {}",
                        component_name.as_str().log_color_highlight()
                    ),
                );
                let clients = self.ctx.golem_clients().await?;

                let component = {
                    let component = clients
                        .component
                        .create_component(
                            &ComponentQuery {
                                project_id: project.map(|p| p.project_id.0),
                                component_name: component_name.to_string(),
                            },
                            linked_wasm,
                            Some(&deploy_properties.component_type),
                            ifs_properties,
                            ifs_archive,
                            deploy_properties.dynamic_linking.as_ref(),
                            deploy_properties
                                .env
                                .map(|env| ComponentEnvCloud { key_values: env })
                                .as_ref(),
                            agent_types.map(|types| AgentTypes { types }).as_ref(),
                        )
                        .await
                        .map_service_error()?;
                    Component::from(component)
                };

                self.ctx
                    .log_handler()
                    .log_view(&ComponentCreateView(ComponentView::new(
                        self.ctx.show_sensitive(),
                        component.clone(),
                    )));

                component
            }
        };

        // We save the recently deployed hashes, so we don't have to download them
        TaskResultMarker::new(
            &self.ctx.task_result_marker_dir().await?,
            GetServerComponentHash {
                project_id: project.map(|p| &p.project_id),
                component_name: &manifest_diffable_component.component_name,
                component_version: component.versioned_component_id.version,
                component_hash: Some(&manifest_diffable_component.component_hash),
            },
        )?
        .success()?;

        Ok(component)
    }

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
                    selected_component_names.project.as_ref(),
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

    pub async fn update_workers_by_components(
        &self,
        components: &[Component],
        update: WorkerUpdateMode,
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
                    component.versioned_component_id.component_id,
                    update,
                    component.versioned_component_id.version,
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
        components: &[Component],
    ) -> anyhow::Result<()> {
        if components.is_empty() {
            return Ok(());
        }

        log_action("Redeploying", "existing workers");
        let _indent = LogIndent::new();

        for component in components {
            self.ctx
                .worker_handler()
                .redeploy_component_workers(
                    &component.component_name,
                    component.versioned_component_id.component_id,
                )
                .await?;
        }

        // TODO: json / yaml output?
        // TODO: unlike updating, redeploy is short-circuiting, should we normalize?
        // TODO: should we expose "delete-workers" too for development?
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
        component_name: Option<&ComponentName>,
        allow_no_matches: bool,
    ) -> anyhow::Result<SelectedComponents> {
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
                            .map(|cn| cn.as_str().into())
                            .collect::<Vec<_>>()
                    })
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
    }

    pub async fn component_by_name_with_auto_deploy(
        &self,
        project: Option<&ProjectRefAndId>,
        component_match_kind: ComponentNameMatchKind,
        component_name: &ComponentName,
        component_version_selection: Option<ComponentVersionSelection<'_>>,
    ) -> anyhow::Result<Component> {
        match self
            .component(project, component_name.into(), component_version_selection)
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
                        "Auto deploying",
                        format!(
                            "missing component {}",
                            component_name.0.log_color_highlight()
                        ),
                    );
                    self.ctx
                        .component_handler()
                        .deploy(
                            project,
                            vec![component_name.clone()],
                            None,
                            &ApplicationComponentSelectMode::CurrentDir,
                            &UpdateOrRedeployArgs::none(),
                        )
                        .await?;
                    self.ctx
                        .component_handler()
                        .component(project, component_name.into(), None)
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
    pub async fn component(
        &self,
        project: Option<&ProjectRefAndId>,
        component_name_or_id: ComponentSelection<'_>,
        component_version_selection: Option<ComponentVersionSelection<'_>>,
    ) -> anyhow::Result<Option<Component>> {
        let component = match component_name_or_id {
            ComponentSelection::Name(component_name) => {
                self.latest_component_by_name(project, component_name)
                    .await?
            }
            ComponentSelection::Id(component_id) => {
                self.latest_component_by_id(component_id).await?
            }
        };

        match (component, component_version_selection) {
            (Some(component), Some(component_version_selection)) => {
                let version = match component_version_selection {
                    ComponentVersionSelection::ByWorkerName(worker_name) => self
                        .ctx
                        .worker_handler()
                        .worker_metadata(
                            component.versioned_component_id.component_id,
                            &component.component_name,
                            worker_name,
                        )
                        .await
                        .ok()
                        .map(|worker_metadata| worker_metadata.component_version),
                    ComponentVersionSelection::ByExplicitVersion(version) => Some(version),
                };

                match version {
                    Some(version) => {
                        let clients = self.ctx.golem_clients().await?;

                        let component = clients
                            .component
                            .get_component_metadata(
                                &component.versioned_component_id.component_id,
                                &version.to_string(),
                            )
                            .await
                            .map_service_error()
                            .map(Component::from)?;

                        Ok(Some(component))
                    }
                    None => Ok(Some(component)),
                }
            }
            (Some(component), None) => Ok(Some(component)),
            (None, _) => Ok(None),
        }
    }

    pub async fn component_id_by_name(
        &self,
        project: Option<&ProjectRefAndId>,
        component_name: &ComponentName,
    ) -> anyhow::Result<Option<ComponentId>> {
        Ok(self
            .component(project, component_name.into(), None)
            .await?
            .map(|c| ComponentId(c.versioned_component_id.component_id)))
    }

    pub async fn latest_component_by_id(
        &self,
        component_id: uuid::Uuid,
    ) -> anyhow::Result<Option<Component>> {
        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .component
            .get_latest_component_metadata(&component_id)
            .await
            .map_service_error_not_found_as_opt()?
            .map(Component::from);

        Ok(result)
    }

    pub async fn latest_component_version_by_id(
        &self,
        component_id: uuid::Uuid,
    ) -> anyhow::Result<Option<u64>> {
        Ok(self
            .latest_component_by_id(component_id)
            .await?
            .map(|component| component.versioned_component_id.version))
    }

    pub async fn latest_component_by_name(
        &self,
        project: Option<&ProjectRefAndId>,
        component_name: &ComponentName,
    ) -> anyhow::Result<Option<Component>> {
        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .component
            .search_components(&ComponentSearchCloud {
                project_id: project.as_ref().map(|p| p.project_id.0),
                components: vec![
                    // TODO: should be the same as ComponentSearchParametersOss in the next release
                    ComponentSearchParametersCloud {
                        name: component_name.0.to_string(),
                        version: None,
                    },
                ],
            })
            .await
            .map_service_error()?
            .into_iter()
            .map(Component::from)
            .next();

        Ok(result)
    }

    pub async fn latest_components_by_app(
        &self,
        project: Option<&ProjectRefAndId>,
    ) -> anyhow::Result<BTreeMap<String, Component>> {
        let component_names = {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;
            app_ctx
                .application
                .component_names()
                .map(|component_name| component_name.as_str().into())
                .collect::<Vec<_>>()
        };

        self.latest_components_by_name(project, component_names)
            .await
    }

    // NOTE: the returned Map uses String as a key, so it is easy to use with all the different
    //       ComponentName types without cloning
    pub async fn latest_components_by_name(
        &self,
        project: Option<&ProjectRefAndId>,
        component_names: Vec<ComponentName>,
    ) -> anyhow::Result<BTreeMap<String, Component>> {
        let clients = self.ctx.golem_clients().await?;

        let results = clients
            .component
            .search_components(&ComponentSearchCloud {
                project_id: project.as_ref().map(|p| p.project_id.0),
                components: component_names
                    .into_iter()
                    .map(|component_name|
                        // TODO: should be the same as ComponentSearchParametersOss in the next release
                        ComponentSearchParametersCloud {
                            name: component_name.0,
                            version: None,
                        })
                    .collect(),
            })
            .await?
            .into_iter()
            .map(|component| (component.component_name.clone(), Component::from(component)))
            .collect();

        Ok(results)
    }

    // NOTE: all of this is naive for now (as in performance, streaming, parallelism)
    async fn manifest_diffable_component(
        &self,
        component_name: &AppComponentName,
        properties: &ComponentDeployProperties,
    ) -> anyhow::Result<DiffableComponent> {
        let component_hash = {
            log_action(
                "Calculating hash",
                format!(
                    "for local component {}",
                    component_name.as_str().log_color_highlight()
                ),
            );
            let file = std::fs::File::open(&properties.linked_wasm_path)?;
            let mut component_hasher = blake3::Hasher::new();
            component_hasher
                .update_reader(&file)
                .context("Failed to hash component")?;
            component_hasher.finalize().to_hex().to_string()
        };

        let files: BTreeMap<String, DiffableComponentFile> = {
            IfsFileManager::new(self.ctx.file_download_client().clone())
                .collect_file_hashes(component_name.as_str(), properties.files.as_slice())
                .await?
                .into_iter()
                .map(|file_hash| {
                    (
                        file_hash.target.path.to_rel_string(),
                        DiffableComponentFile {
                            hash: file_hash.hash_hex,
                            permissions: file_hash.target.permissions,
                        },
                    )
                })
                .collect()
        };

        DiffableComponent::from_manifest(
            self.ctx.show_sensitive(),
            component_name,
            component_hash,
            properties.component_type,
            files,
            properties.dynamic_linking.as_ref(),
            properties.env.as_ref(),
        )
    }

    // NOTE: all of this is naive for now (as in performance, streaming, parallelism)
    async fn server_diffable_component(
        &self,
        project: Option<&ProjectRefAndId>,
        component: &Component,
    ) -> anyhow::Result<DiffableComponent> {
        let component_hash = self
            .server_component_hash(
                project,
                &component.component_name,
                ComponentId(component.versioned_component_id.component_id),
                component.versioned_component_id.version,
            )
            .await?;

        let files: BTreeMap<String, DiffableComponentFile> = {
            if component.files.is_empty() {
                BTreeMap::new()
            } else {
                log_action(
                    "Calculating hashes",
                    format!(
                        "for server IFS files, component: {}",
                        &component.component_name.0.log_color_highlight()
                    ),
                );
                let _indent = LogIndent::new();

                let mut files = BTreeMap::new();
                for file in &component.files {
                    let target_path = file.path.to_rel_string();

                    let hash = self
                        .server_ifs_file_hash(
                            project,
                            &component.component_name,
                            ComponentId(component.versioned_component_id.component_id),
                            component.versioned_component_id.version,
                            &target_path,
                        )
                        .await?;
                    files.insert(
                        target_path,
                        DiffableComponentFile {
                            hash,
                            permissions: file.permissions,
                        },
                    );
                }

                files
            }
        };

        DiffableComponent::from_server(self.ctx.show_sensitive(), component, component_hash, files)
    }

    async fn server_component_hash(
        &self,
        project: Option<&ProjectRefAndId>,
        component_name: &ComponentName,
        component_id: ComponentId,
        component_version: u64,
    ) -> anyhow::Result<String> {
        let task_result_marker_dir = self.ctx.task_result_marker_dir().await?;

        let hash_result = |component_hash| GetServerComponentHash {
            project_id: project.map(|p| &p.project_id),
            component_name,
            component_version,
            component_hash,
        };

        let hash = TaskResultMarker::get_hash(&task_result_marker_dir, hash_result(None))?;

        match hash {
            Some(hash) => {
                debug!(
                    component_name = component_name.0,
                    component_id = ?component_id.0,
                    component_version,
                    hash,
                    "Found cached hash for server component"
                );
                Ok(hash)
            }
            None => {
                log_action(
                    "Calculating hash",
                    format!(
                        "for server component {}@{}",
                        component_name.0.log_color_highlight(),
                        component_version.to_string().log_color_highlight()
                    ),
                );

                // TODO: streaming
                let clients = self.ctx.golem_clients().await?;

                let component_bytes = clients
                    .component
                    .download_component(&component_id.0, Some(component_version))
                    .await?;

                let mut component_hasher = blake3::Hasher::new();
                component_hasher.update(&component_bytes);
                let hash = component_hasher.finalize().to_hex().to_string();

                TaskResultMarker::new(&task_result_marker_dir, hash_result(Some(&hash)))?
                    .success()?;

                Ok(hash)
            }
        }
    }

    async fn server_ifs_file_hash(
        &self,
        project: Option<&ProjectRefAndId>,
        component_name: &ComponentName,
        component_id: ComponentId,
        component_version: u64,
        target_path: &str,
    ) -> anyhow::Result<String> {
        let task_result_marker_dir = self.ctx.task_result_marker_dir().await?;

        let hash_result = |file_hash| GetServerIfsFileHash {
            project_id: project.map(|p| &p.project_id),
            component_name,
            component_version,
            target_path,
            file_hash,
        };

        let hash = TaskResultMarker::get_hash(&task_result_marker_dir, hash_result(None))?;

        match hash {
            Some(hash) => {
                debug!(
                    component_name = component_name.0,
                    component_id = ?component_id.0,
                    component_version,
                    hash,
                    "Found cached hash for server IFS file"
                );
                Ok(hash)
            }
            None => {
                log_action(
                    "Calculating hash",
                    format!(
                        "for server IFS file {}@{} - {}",
                        component_name.0.log_color_highlight(),
                        component_version.to_string().log_color_highlight(),
                        target_path.log_color_highlight()
                    ),
                );

                // TODO: streaming
                let clients = self.ctx.golem_clients().await?;

                let component_bytes = clients
                    .component
                    .download_component_file(
                        &component_id.0,
                        &component_version.to_string(),
                        target_path,
                    )
                    .await?;

                let mut component_hasher = blake3::Hasher::new();
                component_hasher.update(&component_bytes);
                let hash = component_hasher.finalize().to_hex().to_string();

                TaskResultMarker::new(&task_result_marker_dir, hash_result(Some(&hash)))?
                    .success()?;

                Ok(hash)
            }
        }
    }
}

struct ComponentDeployProperties {
    component_type: ComponentType,
    linked_wasm_path: PathBuf,
    files: Vec<InitialComponentFile>,
    dynamic_linking: Option<DynamicLinkingOss>,
    env: Option<HashMap<String, String>>,
}

fn component_deploy_properties(
    app_ctx: &mut ApplicationContext,
    component_name: &AppComponentName,
    build_profile: Option<&BuildProfileName>,
) -> anyhow::Result<ComponentDeployProperties> {
    let linked_wasm_path = app_ctx
        .application
        .component_linked_wasm(component_name, build_profile);
    let component_properties = app_ctx
        .application
        .component_properties(component_name, build_profile);
    let component_type = component_properties
        .component_type()
        .as_deployable_component_type()
        .ok_or_else(|| anyhow!("Component {component_name} is not deployable"))?;
    let files = component_properties.files.clone();
    let env = (!component_properties.env.is_empty()).then(|| component_properties.env.clone());
    let dynamic_linking = app_component_dynamic_linking(app_ctx, component_name)?;

    Ok(ComponentDeployProperties {
        component_type,
        linked_wasm_path,
        files,
        dynamic_linking,
        env,
    })
}

fn app_component_dynamic_linking(
    app_ctx: &mut ApplicationContext,
    component_name: &AppComponentName,
) -> anyhow::Result<Option<DynamicLinkingOss>> {
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

    if mapping.is_empty() {
        Ok(None)
    } else {
        Ok(Some(DynamicLinkingOss {
            dynamic_linking: HashMap::from_iter(mapping.into_iter().map(|stub_interfaces| {
                (
                    stub_interfaces.stub_interface_name,
                    DynamicLinkedInstanceOss::WasmRpc(DynamicLinkedWasmRpcOss {
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
                                            component_type: if stub_interfaces.is_ephemeral {
                                                ComponentType::Ephemeral
                                            } else {
                                                ComponentType::Durable
                                            },
                                        },
                                    )
                                }),
                        ),
                    }),
                )
            })),
        }))
    }
}
