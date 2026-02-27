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

use crate::app::context::{to_anyhow, BuildContext};

use crate::app::build::extract_agent_type::extract_and_store_agent_types;
use crate::command::component::ComponentSubcommand;
use crate::command::shared_args::{ComponentTemplateName, OptionalComponentNames, PostDeployArgs};
use crate::command_handler::component::ifs::IfsFileManager;
use crate::command_handler::component::staging::ComponentStager;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::{HintError, NonSuccessfulExit, ShowClapHelpTarget};
use crate::log::{log_action, log_error, log_warn_action, logln, LogColorize, LogIndent};
use crate::model::app::BuildConfig;
use crate::model::app::{ApplicationComponentSelectMode, DynamicHelpSections};
use crate::model::component::{
    ComponentDeployProperties, ComponentNameMatchKind, ComponentRevisionSelection, ComponentView,
    SelectedComponents,
};
use crate::model::deploy::{DeployConfig, TryUpdateAllWorkersResult};
use crate::model::environment::{
    EnvironmentReference, EnvironmentResolveMode, ResolvedEnvironmentIdentity,
};
use crate::model::text::component::ComponentGetView;
use crate::model::text::fmt::log_text_view;
use crate::model::text::help::ComponentNameHelp;
use crate::model::text::plugin::PluginNameAndVersion;
use crate::model::worker::AgentUpdateMode;
use crate::validation::ValidationBuilder;
use anyhow::{anyhow, bail, Context as AnyhowContext};
use futures_util::future::OptionFuture;
use golem_client::api::ComponentClient;
use golem_client::model::{ComponentCreation, ComponentDto};
use golem_common::cache::SimpleCache;
use golem_common::model::agent::AgentType;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{
    ComponentId, ComponentName, ComponentRevision, ComponentUpdate,
};

use crate::app_template::add_component_by_template;
use crate::model::GuestLanguage;
use golem_common::model::deployment::DeploymentPlanComponentEntry;
use golem_common::model::diff;
use golem_common::model::environment::EnvironmentName;
use itertools::Itertools;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

pub mod ifs;
mod staging;

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
            ComponentSubcommand::List => self.cmd_list().await,
            ComponentSubcommand::Get {
                component_name,
                revision,
            } => self.cmd_get(component_name.component_name, revision).await,

            ComponentSubcommand::UpdateAgents {
                component_name,
                update_mode,
                r#await,
                disable_wakeup,
            } => {
                self.cmd_update_workers(
                    component_name.component_name,
                    update_mode,
                    r#await,
                    disable_wakeup,
                )
                .await
            }
            ComponentSubcommand::RedeployAgents { component_name } => {
                self.cmd_redeploy_workers(component_name.component_name)
                    .await
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
        component_name: Option<ComponentName>,
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
                    .application()
                    .component_names()
                    .map(|name| name.to_string())
                    .collect::<HashSet<_>>(),
                app_ctx.application().application_name().clone(),
            )
        };

        let Some((template, component_name)) = ({
            match (template, component_name) {
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
            log_error("Both TEMPLATE and COMPONENT_NAME are required in non-interactive mode");
            logln("");
            self.ctx
                .app_handler()
                .log_templates_help(None, None, self.ctx.dev_mode());
            logln("");
            bail!(HintError::ShowClapHelp(ShowClapHelpTarget::ComponentNew));
        };

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

        match add_component_by_template(
            common_template,
            Some(component_template),
            &PathBuf::from("."),
            &application_name,
            &component_name,
            Some(self.ctx.template_sdk_overrides()),
        ) {
            Ok(()) => {
                log_action(
                    "Added",
                    format!(
                        "new app component {}",
                        component_name.0.log_color_highlight()
                    ),
                );
            }
            Err(error) => {
                bail!("Failed to create new app component: {}", error)
            }
        }

        Ok(())
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

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let show_sensitive = self.ctx.show_sensitive();

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let components = environment
            .with_current_deployment_revision_or_default_warn(
                |current_deployment_revision| async move {
                    Ok(self
                        .ctx
                        .golem_clients()
                        .await?
                        .component
                        .get_deployment_components(
                            &environment.environment_id.0,
                            current_deployment_revision.into(),
                        )
                        .await?
                        .values
                        .into_iter()
                        .map(|component| ComponentView::new_wit_style(show_sensitive, component))
                        .collect::<Vec<_>>())
                },
            )
            .await?;

        self.ctx.log_handler().log_view(&components);

        Ok(())
    }

    async fn cmd_get(
        &self,
        component_name: Option<ComponentName>,
        revision: Option<ComponentRevision>,
    ) -> anyhow::Result<()> {
        let selected_components = self
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        if revision.is_some() && selected_components.component_names.len() > 1 {
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
                .resolve_component(
                    &selected_components.environment,
                    component_name,
                    revision.map(|revision| revision.into()),
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
            if revision.is_some() && selected_components.component_names.len() == 1 {
                let current = self
                    .get_current_deployed_server_component_by_name(
                        &selected_components.environment,
                        &selected_components.component_names[0],
                    )
                    .await;
                if let Ok(Some(current)) = current {
                    log_error(format!(
                        "Component revision not found, current deployed revision: {}",
                        current.revision.to_string().log_color_highlight()
                    ));
                } else {
                    log_error("Component revision not found");
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
        update_mode: AgentUpdateMode,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        let components = self.components_for_deploy_args(component_name).await?;
        self.update_workers_by_components(&components, update_mode, await_update, disable_wakeup)
            .await?;

        Ok(())
    }

    async fn cmd_redeploy_workers(
        &self,
        component_name: Option<ComponentName>,
    ) -> anyhow::Result<()> {
        let components = self.components_for_deploy_args(component_name).await?;
        self.redeploy_workers_by_components(&components).await?;

        Ok(())
    }

    async fn components_for_deploy_args(
        &self,
        component_name: Option<ComponentName>,
    ) -> anyhow::Result<Vec<ComponentDto>> {
        let clients = self.ctx.golem_clients().await?;

        let selected_component_names = self
            .opt_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await?;
        let current_deployment = environment.current_deployment_or_err()?;

        let mut components = Vec::with_capacity(selected_component_names.component_names.len());
        for component_name in &selected_component_names.component_names {
            match clients
                .component
                .get_deployment_component(
                    &environment.environment_id.0,
                    current_deployment.revision.into(),
                    &component_name.0,
                )
                .await
                .map_service_error_not_found_as_opt()?
            {
                Some(component) => {
                    components.push(component);
                }
                None => {
                    log_error(format!(
                        "Component {} is not deployed!",
                        component_name.0.log_color_highlight()
                    ));
                    bail!(NonSuccessfulExit);
                }
            }
        }
        Ok(components)
    }

    async fn cmd_diagnose(&self, component_names: OptionalComponentNames) -> anyhow::Result<()> {
        self.ctx
            .app_handler()
            .diagnose(
                component_names.component_name,
                &ApplicationComponentSelectMode::CurrentDir,
            )
            .await
    }

    async fn cmd_manifest_trace(
        &self,
        _component_names: OptionalComponentNames,
    ) -> anyhow::Result<()> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        let component_names = app_ctx
            .application()
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
                    .application()
                    .component(&component_name)
                    .layer_properties()
                    .with_compacted_traces(),
            )
        }

        Ok(())
    }

    pub async fn update_workers_by_components(
        &self,
        components: &[ComponentDto],
        update: AgentUpdateMode,
        await_updates: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        if components.is_empty() {
            return Ok(());
        }

        log_action("Updating", format!("existing agents using {update} mode"));
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
                    disable_wakeup,
                )
                .await?;
            update_results.extend(result);
        }

        self.ctx.log_handler().log_view(&update_results);

        if !update_results.failed.is_empty() {
            bail!(NonSuccessfulExit)
        } else {
            Ok(())
        }
    }

    pub async fn redeploy_workers_by_components(
        &self,
        components: &[ComponentDto],
    ) -> anyhow::Result<()> {
        if components.is_empty() {
            return Ok(());
        }

        log_action("Redeploying", "existing agents");
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
        component_name: Option<&ComponentName>,
        allow_no_matches: bool,
    ) -> anyhow::Result<SelectedComponents> {
        fn non_empty<'a>(name: &'a str, value: &'a str) -> anyhow::Result<&'a str> {
            if value.is_empty() {
                log_error(format!("Missing {name} part in component name!"));
                logln("");
                log_text_view(&ComponentNameHelp);
                bail!(NonSuccessfulExit);
            }
            Ok(value)
        }

        fn validated<'a, T>(name: &'a str, value: &'a str) -> anyhow::Result<T>
        where
            T: FromStr<Err = String>,
        {
            let value = non_empty(name, value)?;
            match T::from_str(value) {
                Ok(value) => Ok(value),
                Err(err) => {
                    log_error(format!(
                        "Invalid {name} part in component name, value: {value}, error: {err}",
                        name = name.log_color_highlight(),
                        value = value.log_color_error_highlight(),
                        err = err.log_color_error_highlight()
                    ));
                    logln("");
                    log_text_view(&ComponentNameHelp);
                    bail!(NonSuccessfulExit);
                }
            }
        }

        fn validated_account(value: &str) -> anyhow::Result<String> {
            Ok(non_empty("account", value)?.to_string())
        }

        fn validated_application(value: &str) -> anyhow::Result<ApplicationName> {
            validated("application", value)
        }

        fn validated_environment(value: &str) -> anyhow::Result<EnvironmentName> {
            validated("environment", value)
        }

        fn validated_component(value: &str) -> anyhow::Result<ComponentName> {
            Ok(ComponentName(non_empty("component", value)?.to_string()))
        }

        self.ctx.silence_app_context_init().await;

        let (environment_reference, component_name): (
            Option<EnvironmentReference>,
            Option<ComponentName>,
        ) = {
            match component_name {
                Some(component_name) => {
                    let segments = component_name.0.split("/").collect::<Vec<_>>();
                    match segments.len() {
                        1 => (None, Some(validated_component(segments[0])?)),
                        2 => (
                            Some(EnvironmentReference::Environment {
                                environment_name: validated_environment(segments[0])?,
                            }),
                            Some(validated_component(segments[1])?),
                        ),
                        3 => (
                            Some(EnvironmentReference::ApplicationEnvironment {
                                application_name: validated_application(segments[0])?,
                                environment_name: validated_environment(segments[1])?,
                            }),
                            Some(validated_component(segments[2])?),
                        ),
                        4 => (
                            Some(EnvironmentReference::AccountApplicationEnvironment {
                                account_email: validated_account(segments[0])?,
                                application_name: validated_application(segments[1])?,
                                environment_name: validated_environment(segments[2])?,
                            }),
                            Some(validated_component(segments[3])?),
                        ),
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
                None => (None, None),
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

        let environment = self
            .ctx
            .environment_handler()
            .resolve_opt_environment_reference(
                EnvironmentResolveMode::Any,
                environment_reference.as_ref(),
            )
            .await?;

        Ok(SelectedComponents {
            environment,
            component_names: selected_component_names,
        })
    }

    pub async fn component_by_name_with_auto_deploy(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_match_kind: ComponentNameMatchKind,
        component_name: &ComponentName,
        component_revision_selection: Option<ComponentRevisionSelection<'_>>,
        post_deploy_args: Option<&PostDeployArgs>,
        repl_bridge_sdk_target: Option<GuestLanguage>,
        skip_build: bool,
    ) -> anyhow::Result<ComponentDto> {
        if post_deploy_args.is_some_and(|da| da.is_any_set(self.ctx.deploy_args())) {
            self.ctx
                .app_handler()
                .deploy(DeployConfig {
                    plan: false,
                    stage: false,
                    approve_staging_steps: false,
                    force_build: None,
                    post_deploy_args: post_deploy_args
                        .cloned()
                        .unwrap_or_else(PostDeployArgs::none),
                    repl_bridge_sdk_target,
                    skip_build,
                })
                .await?;
        }

        match self
            .resolve_component(environment, component_name, component_revision_selection)
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
                        .deploy(DeployConfig {
                            plan: false,
                            stage: false,
                            approve_staging_steps: false,
                            force_build: None,
                            post_deploy_args: PostDeployArgs::none(),
                            repl_bridge_sdk_target,
                            skip_build,
                        })
                        .await?;

                    let environment = self
                        .ctx
                        .environment_handler()
                        .resolve_environment(EnvironmentResolveMode::ManifestOnly)
                        .await?;

                    self.ctx
                        .component_handler()
                        .resolve_component(&environment, component_name, None)
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

    pub async fn resolve_component(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name: &ComponentName,
        component_revision_selection: Option<ComponentRevisionSelection<'_>>,
    ) -> anyhow::Result<Option<ComponentDto>> {
        let component = self
            .get_current_deployed_server_component_by_name(environment, component_name)
            .await?;

        match (component, component_revision_selection) {
            (Some(component), Some(component_revision_selection)) => {
                let revision = match component_revision_selection {
                    ComponentRevisionSelection::ByWorkerName(worker_name) => self
                        .ctx
                        .worker_handler()
                        .worker_metadata(component.id.0, &component.component_name, worker_name)
                        .await
                        .ok()
                        .map(|worker_metadata| worker_metadata.component_revision),
                    ComponentRevisionSelection::ByExplicitRevision(revision) => Some(revision),
                };

                match revision {
                    Some(revision) => {
                        let clients = self.ctx.golem_clients().await?;

                        let component = clients
                            .component
                            .get_component_revision(&component.id.0, revision.into())
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

    pub async fn deployable_manifest_components(
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

        let agent_types = extract_and_store_agent_types(
            &BuildContext::new(app_ctx, &BuildConfig::new()),
            component_name,
        )
        .await?;
        let component = app_ctx.application().component(component_name);
        let wasm_path = component.final_wasm();

        if !component.component_type().is_deployable() {
            bail!("Component {component_name} is not deployable");
        }
        let files = component.files().clone();
        let plugins = component.plugins().clone();
        let env = resolve_env_vars(component_name, component.env())?;
        let config_vars = component.config_vars().clone();

        Ok(ComponentDeployProperties {
            wasm_path,
            agent_types,
            files,
            plugins,
            env,
            config_vars,
        })
    }

    pub async fn diffable_local_component(
        &self,
        environment: &ResolvedEnvironmentIdentity,
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
            let file = std::fs::File::open(&properties.wasm_path)?;
            let mut component_hasher = blake3::Hasher::new();
            component_hasher
                .update_reader(&file)
                .context("Failed to hash component binary")?;
            component_hasher.finalize()
        };

        // TODO: atomic: cache it with a TaskResultMarker (handling local vs http)?
        let files_by_path: BTreeMap<String, diff::HashOf<diff::ComponentFile>> = {
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

        let plugins_by_grant_id = {
            if properties.plugins.is_empty() {
                BTreeMap::new()
            } else {
                let plugin_grants = self
                    .ctx
                    .environment_handler()
                    .plugin_grants(environment)
                    .await?;

                let mut plugins_by_grant_id = BTreeMap::new();

                for (priority, plugin) in properties.plugins.iter().enumerate() {
                    // TODO: atomic: cannot lookup by account email
                    let Some(server_plugin) = plugin_grants.get(&PluginNameAndVersion {
                        name: plugin.name.clone(),
                        version: plugin.version.clone(),
                    }) else {
                        log_error(format!(
                            "Plugin {}/{} for component {} not found.",
                            plugin.name,
                            plugin.version,
                            component_name.0.log_color_highlight()
                        ));
                        logln("");
                        logln("Check if the plugin is registered and granted for the application environment!");
                        bail!(NonSuccessfulExit);
                    };
                    plugins_by_grant_id.insert(
                        server_plugin.id.0,
                        diff::PluginInstallation {
                            priority: priority as i32,
                            name: plugin.name.clone(),
                            version: plugin.version.clone(),
                            grant_id: server_plugin.id.0,
                            parameters: Default::default(),
                        },
                    );
                }

                plugins_by_grant_id
            }
        };

        Ok(diff::Component {
            metadata: diff::ComponentMetadata {
                version: Some("".to_string()), // TODO: atomic
                env: properties
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                config_vars: properties
                    .config_vars
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            }
            .into(),
            wasm_hash: component_binary_hash.into(),
            files_by_path,
            plugins_by_grant_id,
            // FIXME: agent-config
            local_agent_config_ordered_by_agent_and_key: Vec::new(),
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
            component_deploy_properties,
            self.ctx
                .environment_handler()
                .plugin_grants(environment)
                .await?,
            None,
        );

        let wasm = component_stager.open_wasm().await?;
        let agent_types: Vec<AgentType> = component_stager.agent_types().clone();

        // NOTE: do not drop until the component is created, keeps alive the temp archive
        let files = component_stager.all_files().await?;

        let component = self
            .ctx
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
                    env: component_stager.env(),
                    config_vars: component_stager.config_vars(),
                    // FIXME: agent-config
                    local_agent_config: Vec::new(),
                    agent_types,
                    plugins: component_stager.plugins(),
                },
                wasm,
                OptionFuture::from(files.as_ref().map(|files| files.open_archive()))
                    .await
                    .transpose()?,
            )
            .await
            .map_service_error()?;

        log_action(
            "Created",
            format!(
                "component revision: {} {}",
                component_name.0.log_color_highlight(),
                component.revision.to_string().log_color_highlight()
            ),
        );

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
            .delete_component(&component.id.0, component.revision.into())
            .await
            .map_service_error()?;

        log_action(
            "Deleted",
            format!(
                "component revision: {} {}",
                component.name.0.log_color_highlight(),
                component.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn update_staged_component(
        &self,
        environment: &ResolvedEnvironmentIdentity,
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
            component_deploy_properties,
            self.ctx
                .environment_handler()
                .plugin_grants(environment)
                .await?,
            Some(diff),
        );

        let wasm = component_stager.open_wasm_if_changed().await?;
        let agent_types = component_stager.agent_types_if_changed().cloned();

        // NOTE: do not drop until the component is created, keeps alive the temp archive
        let changed_files = component_stager.changed_files().await?;

        let component = self
            .ctx
            .golem_clients()
            .await?
            .component
            .update_component(
                &component.id.0,
                &ComponentUpdate {
                    current_revision: component.revision,
                    removed_files: changed_files.removed.clone(),
                    new_file_options: changed_files.merged_file_options(),
                    config_vars: component_stager.config_vars_if_changed(),
                    // FIXME: local-agent-config
                    local_agent_config: None,
                    env: component_stager.env_if_changed(),
                    agent_types,
                    plugin_updates: component_stager.plugins_if_changed(),
                },
                wasm,
                changed_files.open_archive().await?,
            )
            .await
            .map_service_error()?;

        log_action(
            "Created",
            format!(
                "component revision: {} {}",
                component.component_name.0.log_color_highlight(),
                component.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn get_current_deployed_server_component_by_name(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name: &ComponentName,
    ) -> anyhow::Result<Option<ComponentDto>> {
        environment
            .with_current_deployment_revision_or_default_warn(
                |current_deployment_revision| async move {
                    self.ctx
                        .golem_clients()
                        .await?
                        .component
                        .get_deployment_component(
                            &environment.environment_id.0,
                            current_deployment_revision.get(),
                            component_name.0.as_str(),
                        )
                        .await
                        .map_service_error_not_found_as_opt()
                },
            )
            .await
    }

    pub async fn get_component_revision_by_id(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
    ) -> anyhow::Result<ComponentDto> {
        self.ctx
            .caches()
            .component_revision
            .get_or_insert_simple(&(*component_id, revision), {
                let ctx = self.ctx.clone();
                async move || {
                    ctx.golem_clients()
                        .await?
                        .component
                        .get_component_revision(&component_id.0, revision.into())
                        .await
                        .map_service_error()
                        .map_err(Arc::new)
                }
            })
            .await
            .map_err(|err| anyhow!(err))
    }
}

fn resolve_env_vars(
    component_name: &ComponentName,
    env: &BTreeMap<String, String>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let proc_env_map: HashMap<String, String> = std::env::vars().collect();
    let proc_env_vars = minijinja::value::Value::from(proc_env_map.clone());

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
                        let missing_env_vars =
                            missing_env_vars(&minijinja_env, value, &proc_env_map, &err);
                        let error_message = if missing_env_vars.is_empty() {
                            format!(
                                "Failed to substitute environment variable(s) for {}",
                                key.log_color_highlight()
                            )
                        } else {
                            format!(
                                "Failed to substitute environment variable(s){}for {}",
                                if missing_env_vars.is_empty() {
                                    "".to_string()
                                } else {
                                    format!(
                                        " ({}) ",
                                        missing_env_vars
                                            .iter()
                                            .map(|key| key.log_color_highlight())
                                            .join(", ")
                                    )
                                },
                                key.log_color_highlight()
                            )
                        };
                        let mut context = vec![
                            ("key", key.to_string()),
                            ("template", value.to_string()),
                            (
                                "error",
                                err.to_string().log_color_error_highlight().to_string(),
                            ),
                        ];
                        if !missing_env_vars.is_empty() {
                            context.push(("missing", missing_env_vars.join(", ")));
                        }
                        validation.with_context(context, |validation| {
                            validation.add_error(error_message)
                        });
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

fn missing_env_vars(
    minijinja_env: &minijinja::Environment,
    template: &str,
    env_vars: &HashMap<String, String>,
    err: &minijinja::Error,
) -> Vec<String> {
    fn is_known_var(
        var: &str,
        env_vars: &HashMap<String, String>,
        global_vars: &HashSet<String>,
    ) -> bool {
        if env_vars.contains_key(var) || global_vars.contains(var) {
            return true;
        }

        if let Some((root, _)) = var.split_once('.') {
            return env_vars.contains_key(root) || global_vars.contains(root);
        }

        false
    }

    if err.kind() != minijinja::ErrorKind::UndefinedError {
        return Vec::new();
    }

    let Ok(template) = minijinja_env.template_from_str(template) else {
        return Vec::new();
    };

    let global_vars: HashSet<String> = minijinja_env
        .globals()
        .map(|(name, _)| name.to_string())
        .collect();

    let mut missing: Vec<String> = template
        .undeclared_variables(true)
        .into_iter()
        .filter(|var| !is_known_var(var, env_vars, &global_vars))
        .collect();

    missing.sort();
    missing
}
