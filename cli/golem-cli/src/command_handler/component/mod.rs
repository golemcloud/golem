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

use crate::app::context::{BuildContext, validated_to_anyhow};

use crate::app::build::extract_component_metadata::extract_and_store_component_metadata;
use crate::command::component::ComponentSubcommand;
use crate::command::shared_args::{OptionalComponentNames, PostDeployArgs};
use crate::command_handler::Handlers;
use crate::command_handler::component::ifs::IfsFileManager;
use crate::command_handler::component::staging::ComponentStager;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::MapServiceError;
use crate::log::{LogColorize, LogIndent, log_action, log_error, log_warn_action, logln};
use crate::model::agent::AgentUpdateMode;
use crate::model::agent::action_result::{
    AgentDeleteAllView, AgentDeletionMeta, AgentRedeployResult, AgentRedeploymentMeta,
};
use crate::model::app::BuildConfig;
use crate::model::app::{ApplicationComponentSelectMode, DynamicHelpSections};
use crate::model::app_raw;
use crate::model::cascade::property::tool_bindings::ToolBindingState;
use crate::model::component::{
    AgentTypeManifestProvisionConfig, ComponentDeployProperties, ComponentNameMatchKind,
    ComponentRevisionSelection, ComponentView, DeployableManifestComponents,
    PendingRemoteInitialFile, SelectedComponents, ToolManifestDeploymentConfig,
    ToolManifestProvisionConfig, initial_permission_from_manifest_card,
    initial_permission_recipient_context,
};
use crate::model::component::{ComponentGetView, ComponentListView, ComponentManifestTraceView};
use crate::model::config::{collect_unused_leaf_paths, value_at_path};
use crate::model::deploy::{
    DeployConfig, TryUpdateAllWorkersView, UpdateStagedComponentError, UpdateStagedComponentResult,
};
use crate::model::environment::{
    EnvironmentReference, EnvironmentResolveMode, ResolvedEnvironmentIdentity,
};
use crate::model::help::ComponentNameHelp;
use crate::model::language::GuestLanguage;
use crate::model::plugin::PluginNameAndVersion;
use crate::model::text_format::log_text_view;
use crate::model::tool_deployment::{
    DiscoveredToolImplementation, ToolEntityPath, ToolImplementationSource, ToolValidationCode,
    ToolValidationIssue, ToolValidationPhase, add_tool_issues,
};
use crate::validation::ValidationBuilder;
use anyhow::{Context as AnyhowContext, anyhow, bail};
use futures_util::future::OptionFuture;
use golem_client::api::{ComponentClient, EnvironmentToolGrantsClient};
use golem_client::model::{ComponentCreation, ComponentDto};
use golem_common::cache::SimpleCache;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::AgentFileContentHash;
use golem_common::model::agent::{AgentConfigSource, AgentTypeName};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{
    AgentConfigEntryDto, AgentFilePath, ComponentId, ComponentName, ComponentRevision,
    ComponentUpdate, InitialAgentFile, InstalledPlugin, PluginPriority,
};
use golem_common::model::deployment::DeploymentPlanComponentEntry;
use golem_common::model::diff;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantWithDetails;
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::plugin_registration::PluginSpecDto;
use golem_common::model::tool::{
    RemoteToolDeployment, SecretKeyScope, ToolBindingInput, ToolName, ToolProvisionConfig,
};
use golem_common::model::tool_release::{ToolReleaseById, ToolReleaseReference};
use golem_common::schema::agent::AgentTypeSchema;
use golem_common::schema::tool::Tool;
use golem_common::schema::tool::validation::validate_tool;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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

    pub fn handle_command(
        &self,
        subcommand: ComponentSubcommand,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + '_>> {
        Box::pin(async move {
            match subcommand {
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
                    self.cmd_update_agents(
                        component_name.component_name,
                        update_mode,
                        r#await,
                        disable_wakeup,
                    )
                    .await
                }
                ComponentSubcommand::RedeployAgents { component_name } => {
                    self.cmd_redeploy_agents(component_name.component_name)
                        .await
                }
                ComponentSubcommand::ManifestTrace { component_name } => {
                    self.cmd_manifest_trace(component_name).await
                }
            }
        })
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
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
                        .list_deployment_components(
                            &environment.environment_id.0,
                            current_deployment_revision.into(),
                        )
                        .await?
                        .values
                        .into_iter()
                        .map(ComponentView::new)
                        .collect::<Vec<_>>())
                },
            )
            .await?;

        self.ctx
            .log_handler()
            .log_output(ComponentListView { components })?;

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
            logln(
                "Specify the requested component name or switch to an application directory with exactly one component!",
            );
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
                component_views.push(ComponentView::new(component));
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
                .log_output(ComponentGetView(component_view))?;
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

    async fn cmd_update_agents(
        &self,
        component_name: Option<ComponentName>,
        update_mode: AgentUpdateMode,
        await_update: bool,
        disable_wakeup: bool,
    ) -> anyhow::Result<()> {
        let components = self.components_for_deploy_args(component_name).await?;
        self.update_agents_by_components(&components, update_mode, await_update, disable_wakeup)
            .await?;

        Ok(())
    }

    async fn cmd_redeploy_agents(
        &self,
        component_name: Option<ComponentName>,
    ) -> anyhow::Result<()> {
        let components = self.components_for_deploy_args(component_name).await?;
        self.redeploy_agents_by_components(&components).await?;

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
            let _indent = self.ctx.log_handler().decorated_indent_primary();
            self.ctx
                .log_handler()
                .log_output(ComponentManifestTraceView {
                    component_name: component_name.clone(),
                    properties: app_ctx
                        .application()
                        .component(&component_name)
                        .layer_properties()
                        .with_compacted_traces(),
                })?
        }

        Ok(())
    }

    pub async fn update_agents_by_components(
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

        let mut update_results = TryUpdateAllWorkersView::default();
        for component in components {
            let result = self
                .ctx
                .agent_handler()
                .update_component_agents(
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

        let has_errors = !update_results.errors.is_empty();

        self.ctx.log_handler().log_output(update_results)?;

        if has_errors {
            bail!(NonSuccessfulExit);
        }

        Ok(())
    }

    pub async fn redeploy_agents_by_components(
        &self,
        components: &[ComponentDto],
    ) -> anyhow::Result<()> {
        if components.is_empty() {
            return Ok(());
        }

        log_action("Redeploying", "existing agents");
        let _indent = LogIndent::new();

        // TODO: unlike updating, redeploy is short-circuiting, should we normalize?
        let mut agents = Vec::new();
        for component in components {
            let redeployed = self
                .ctx
                .agent_handler()
                .redeploy_component_agents(&component.component_name, &component.id)
                .await?;
            let version = component.metadata.root_package_version().clone();
            for (agent_id, from_revision) in redeployed {
                let from_version = self
                    .component_version_at(&component.id, from_revision)
                    .await;
                agents.push(AgentRedeploymentMeta {
                    component_name: component.component_name.clone(),
                    agent_id,
                    from_revision,
                    revision: component.revision,
                    from_version,
                    version: version.clone(),
                });
            }
        }

        self.ctx.log_handler().log_output(AgentRedeployResult {
            redeployed: true,
            agents,
        })?;

        Ok(())
    }

    pub async fn delete_agents(&self, components: &[ComponentDto]) -> anyhow::Result<()> {
        if components.is_empty() {
            return Ok(());
        }

        log_action("Deleting", "existing agents");
        let _indent = LogIndent::new();

        // NOTE: for now we naively keep deleting in a loop until we do not find any more agents,
        //       we do so to help a bit with pending invocations or currently running worker creations,
        //       but this is not a 100% guarantee.
        let mut agents = Vec::new();
        let mut found_any = true;
        let mut first_round = true;
        while found_any {
            found_any = false;
            for component in components {
                let deleted = self
                    .ctx
                    .agent_handler()
                    .delete_component_agents(&component.component_name, &component.id, first_round)
                    .await?;
                if !deleted.is_empty() {
                    found_any = true;
                }
                for agent_id in deleted {
                    agents.push(AgentDeletionMeta {
                        component_name: component.component_name.clone(),
                        agent_id,
                    });
                }
            }
            first_round = false;
        }

        self.ctx.log_handler().log_output(AgentDeleteAllView {
            deleted: true,
            agents,
        })?;

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
            log_error(
                "No components were selected based on the current directory an no component was requested.",
            );
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
                    full_diff: false,
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
                            full_diff: false,
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
                    ComponentRevisionSelection::ByAgentId(agent_id) => self
                        .ctx
                        .agent_handler()
                        .agent_metadata(component.id.0, &component.component_name, agent_id)
                        .await?
                        .map(|worker_metadata| worker_metadata.component_revision),
                    ComponentRevisionSelection::ByExplicitRevision(revision) => Some(revision),
                };

                match revision {
                    Some(revision) => {
                        let component = self
                            .get_component_revision_by_id(&component.id, revision)
                            .await?;

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
        environment: &ResolvedEnvironmentIdentity,
    ) -> anyhow::Result<DeployableManifestComponents> {
        let (component_names, declared_agents) = {
            let app_ctx = self.ctx.app_context_lock().await;
            let app = app_ctx.some_or_err()?;
            (
                app.component_names().into_iter().collect::<Vec<_>>(),
                app.application()
                    .agent_ids()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            )
        };

        let mut components = BTreeMap::<ComponentName, ComponentDeployProperties>::new();
        for component_name in component_names {
            let properties = self.component_deploy_properties(&component_name).await?;
            components.insert(component_name, properties);
        }

        let mut exported_agents = HashMap::<AgentTypeName, Vec<ComponentName>>::new();
        for (component_name, properties) in &components {
            for agent_type in &properties.agent_types {
                exported_agents
                    .entry(agent_type.type_name.clone())
                    .or_default()
                    .push(component_name.clone());
            }
        }

        let unknown_declared_agents = declared_agents
            .into_iter()
            .filter(|declared_agent| !exported_agents.contains_key(declared_agent))
            .collect::<BTreeSet<_>>();

        let (
            remote_tool_deployments,
            diffable_remote_tool_deployments,
            published_tools,
            pending_remote_initial_files,
        ) = self
            .resolve_manifest_tool_deployments(
                environment,
                &mut components,
                &unknown_declared_agents,
            )
            .await?;

        Ok(DeployableManifestComponents {
            components,
            remote_tool_deployments,
            diffable_remote_tool_deployments,
            published_tools,
            pending_remote_initial_files,
        })
    }

    async fn resolve_manifest_tool_deployments(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        components: &mut BTreeMap<ComponentName, ComponentDeployProperties>,
        unknown_declared_agents: &BTreeSet<AgentTypeName>,
    ) -> anyhow::Result<(
        Vec<RemoteToolDeployment>,
        BTreeMap<String, diff::HashOf<diff::RemoteToolDeployment>>,
        BTreeSet<ToolName>,
        Vec<PendingRemoteInitialFile>,
    )> {
        let release_grants = self
            .ctx
            .golem_clients()
            .await?
            .environment_tool_grants
            .list_environment_tool_grants(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values;
        let plugin_grants = self
            .ctx
            .environment_handler()
            .plugin_grants(environment)
            .await?;
        let app_ctx = self.ctx.app_context_lock().await;
        let app = app_ctx.some_or_err()?.application();
        let mut issues = Vec::new();
        let mut implementations = BTreeMap::<ToolName, Vec<DiscoveredToolImplementation>>::new();

        for agent_name in unknown_declared_agents {
            issues.push(ToolValidationIssue::error(
                ToolValidationPhase::BindingReferences,
                ToolValidationCode::UnknownAgentReference,
                ToolEntityPath::agent(agent_name, "agents"),
                app.agent_declaration_source(agent_name)
                    .map(std::path::Path::to_path_buf),
                "Manifest declares an agent that is not exported by any component",
            ));
        }

        for (component_name, properties) in components.iter() {
            for definition in &properties.tools {
                let Some(raw_name) = definition.name() else {
                    issues.push(ToolValidationIssue::error(
                        ToolValidationPhase::StructuralMetadata,
                        ToolValidationCode::InvalidDefinition,
                        ToolEntityPath::tool("<missing>", "definition.commands"),
                        Some(app.component(component_name).source().to_path_buf()),
                        format!("Component '{component_name}' exports a tool without a root name"),
                    ));
                    continue;
                };
                let name = match ToolName::try_from(raw_name) {
                    Ok(name) => name,
                    Err(message) => {
                        issues.push(ToolValidationIssue::error(
                            ToolValidationPhase::StructuralMetadata,
                            ToolValidationCode::InvalidName,
                            ToolEntityPath::tool(raw_name, "definition.name"),
                            Some(app.component(component_name).source().to_path_buf()),
                            message,
                        ));
                        continue;
                    }
                };
                if let Err(errors) = validate_tool(definition) {
                    issues.push(ToolValidationIssue::error(
                        ToolValidationPhase::StructuralMetadata,
                        ToolValidationCode::InvalidDefinition,
                        ToolEntityPath::tool(&name, "definition"),
                        Some(app.component(component_name).source().to_path_buf()),
                        errors.into_iter().map(|error| error.to_string()).join("; "),
                    ));
                    continue;
                }
                implementations
                    .entry(name)
                    .or_default()
                    .push(DiscoveredToolImplementation {
                        definition: definition.clone(),
                        implementation: ToolImplementationSource::Component {
                            component_name: component_name.clone(),
                        },
                        diagnostic_source: Some(
                            app.component(component_name).source().to_path_buf(),
                        ),
                    });
            }
        }

        for (tool_name, declaration) in app.tool_declarations() {
            let Some(release_source) = declaration.value.source.as_ref() else {
                continue;
            };
            let grant = release_grants
                .iter()
                .find(|grant| match &release_source.registry {
                    app_raw::RegistrySubject::ById(reference) => {
                        grant.release.id == reference.release_id
                    }
                    app_raw::RegistrySubject::ByCoordinates(reference) => {
                        grant.release_owner.email.as_str() == reference.account
                            && grant.release.name.as_str() == reference.name
                            && grant.release.version == reference.version
                    }
                });
            let Some(grant) = grant else {
                issues.push(ToolValidationIssue::error(
                    ToolValidationPhase::DeclarationDiscoveryIdentity,
                    ToolValidationCode::ReleaseNotGranted,
                    ToolEntityPath::tool(tool_name, "tools.source.registry"),
                    Some(declaration.source.clone()),
                    "Published tool release was not found among the active grants for this environment",
                ));
                continue;
            };
            if &grant.release.name != tool_name {
                issues.push(ToolValidationIssue::error(
                    ToolValidationPhase::DeclarationDiscoveryIdentity,
                    ToolValidationCode::InvalidName,
                    ToolEntityPath::tool(tool_name, "tools.source.registry"),
                    Some(declaration.source.clone()),
                    format!(
                        "Declaration key must equal resolved published tool name '{}'",
                        grant.release.name
                    ),
                ));
            }
            if let Err(errors) = validate_tool(&grant.release.definition) {
                issues.push(ToolValidationIssue::error(
                    ToolValidationPhase::StructuralMetadata,
                    ToolValidationCode::InvalidDefinition,
                    ToolEntityPath::tool(tool_name, "definition"),
                    Some(declaration.source.clone()),
                    errors.into_iter().map(|error| error.to_string()).join("; "),
                ));
                continue;
            }
            implementations.entry(tool_name.clone()).or_default().push(
                DiscoveredToolImplementation {
                    definition: grant.release.definition.clone(),
                    implementation: ToolImplementationSource::RemoteRelease {
                        grant: Box::new(grant.clone()),
                    },
                    diagnostic_source: Some(declaration.source.clone()),
                },
            );
        }

        for (tool_name, declaration) in app.tool_declarations() {
            if declaration.value.source.is_none() && !implementations.contains_key(tool_name) {
                issues.push(ToolValidationIssue::error(
                    ToolValidationPhase::DeclarationDiscoveryIdentity,
                    ToolValidationCode::MissingImplementation,
                    ToolEntityPath::tool(tool_name, "tools"),
                    Some(declaration.source.clone()),
                    "Tool is declared but is not exported by any component",
                ));
            }
        }

        for (tool_name, sources) in &implementations {
            if !app.tool_declarations().contains_key(tool_name) {
                issues.push(ToolValidationIssue::error(
                    ToolValidationPhase::DeclarationDiscoveryIdentity,
                    ToolValidationCode::MissingDeclaration,
                    ToolEntityPath::tool(tool_name, "definition.name"),
                    sources
                        .first()
                        .and_then(|source| source.diagnostic_source.clone()),
                    "Discovered tool has no matching top-level declaration",
                ));
            }
            if sources.len() > 1 {
                issues.push(ToolValidationIssue::error(
                    ToolValidationPhase::DeclarationDiscoveryIdentity,
                    ToolValidationCode::DuplicateImplementation,
                    ToolEntityPath::tool(tool_name, "definition.name"),
                    None,
                    format!(
                        "Tool is exported by multiple components: {}",
                        sources
                            .iter()
                            .map(|source| match &source.implementation {
                                ToolImplementationSource::Component { component_name } =>
                                    component_name.as_str(),
                                ToolImplementationSource::RemoteRelease { .. } => "remote release",
                            })
                            .join(", ")
                    ),
                ));
            }
        }

        let agent_components = components
            .iter()
            .flat_map(|(component_name, properties)| {
                properties
                    .agent_types
                    .iter()
                    .map(move |agent| (agent.type_name.clone(), component_name.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let resolved_agents = app.resolve_agents(&agent_components)?;
        let environment_bindings = resolve_tool_binding_map(
            app.selected_environment()
                .tools_merge_mode
                .unwrap_or_default(),
            app.selected_environment().tools.clone().unwrap_or_default(),
        );

        validate_tool_binding_references(
            &mut issues,
            &environment_bindings,
            "environments.tools",
            None,
            app.selected_environment_source(),
            &implementations,
        );
        for agent_name in agent_components.keys() {
            if let Some(agent) = resolved_agents.agent(agent_name) {
                validate_tool_binding_references(
                    &mut issues,
                    agent.tool_bindings(),
                    "agents.tools",
                    Some(agent_name),
                    Some(agent.source()),
                    &implementations,
                );
            }
        }

        let local_owner = &environment.server_environment.owner_account_email;
        let mut configs_by_component =
            BTreeMap::<ComponentName, BTreeMap<ToolName, ToolManifestDeploymentConfig>>::new();
        let mut remote_tool_deployments = Vec::new();
        let mut diffable_remote_tool_deployments = BTreeMap::new();
        let mut pending_remote_initial_files = Vec::new();

        for (tool_name, sources) in &implementations {
            let Some(source) = sources.as_slice().first() else {
                continue;
            };
            if sources.len() != 1 || !app.tool_declarations().contains_key(tool_name) {
                continue;
            }
            let definition = &source.definition;
            let owner = source
                .implementation
                .release_grant()
                .map(|grant| &grant.release_owner.email)
                .unwrap_or(local_owner);
            let declaration_source = app
                .tool_declarations()
                .get(tool_name)
                .map(|declaration| declaration.source.clone());
            let environment_binding =
                environment_bindings
                    .get(tool_name.as_str())
                    .and_then(|state| {
                        resolve_tool_binding_input(
                            &mut issues,
                            tool_name,
                            definition,
                            owner,
                            state,
                            "environments.tools",
                            None,
                            app.selected_environment_source(),
                        )
                    });

            let mut agent_bindings = BTreeMap::new();
            for agent_name in agent_components.keys() {
                let Some(agent) = resolved_agents.agent(agent_name) else {
                    continue;
                };
                let Some(state) = agent.tool_bindings().get(tool_name.as_str()) else {
                    continue;
                };
                if let (Some(environment_version), Some(agent_version)) = (
                    environment_bindings
                        .get(tool_name.as_str())
                        .and_then(|binding| binding.version.as_ref()),
                    state.version.as_ref(),
                ) && environment_version != agent_version
                {
                    issues.push(ToolValidationIssue::error(
                        ToolValidationPhase::LocalResolution,
                        ToolValidationCode::EnvironmentAgentVersionMismatch,
                        ToolEntityPath::agent(agent_name, format!("tools.{tool_name}.version")),
                        Some(agent.source().to_path_buf()),
                        format!(
                            "Environment version '{environment_version}' does not match agent version '{agent_version}'"
                        ),
                    ));
                }
                if let Some(binding) = resolve_tool_binding_input(
                    &mut issues,
                    tool_name,
                    definition,
                    owner,
                    state,
                    "agents.tools",
                    Some(agent_name),
                    Some(agent.source()),
                ) {
                    validate_effective_tool_binding(
                        &mut issues,
                        tool_name,
                        environment_binding.as_ref(),
                        &binding,
                        agent_name,
                        agent.source(),
                    );
                    agent_bindings.insert(agent_name.clone(), binding);
                }
            }

            let provision = match match source.implementation.local_component_name() {
                Some(component_name) => app.resolve_tool_provision(tool_name, component_name),
                None => app.resolve_remote_tool_provision(tool_name),
            } {
                Ok(provision) => provision,
                Err(error) => {
                    issues.push(ToolValidationIssue::error(
                        ToolValidationPhase::LocalResolution,
                        ToolValidationCode::InvalidProvision,
                        ToolEntityPath::tool(tool_name, "tools"),
                        declaration_source,
                        format!("Failed to resolve tool provision properties: {error:#}"),
                    ));
                    continue;
                }
            };
            let mut files_valid = true;
            for file in &provision.properties.files {
                if let Err(error) = crate::model::app::InitialComponentFileSource::new(
                    &file.file.source_path,
                    &file.source,
                ) {
                    files_valid = false;
                    issues.push(ToolValidationIssue::error(
                        ToolValidationPhase::LocalResolution,
                        ToolValidationCode::InvalidProvision,
                        ToolEntityPath::tool(tool_name, "tools.files.sourcePath"),
                        Some(file.source.clone()),
                        format!(
                            "Invalid tool provision file source '{}': {error}",
                            file.file.source_path
                        ),
                    ));
                }
            }
            let materialization_component = source
                .implementation
                .local_component_name()
                .cloned()
                .unwrap_or_else(|| ComponentName(format!("remote-release-{tool_name}")));
            let config = resolve_json_value(
                &materialization_component,
                "tool config",
                provision
                    .properties
                    .config
                    .unwrap_or_else(|| serde_json::json!({})),
            );
            let env = resolve_env_vars(&materialization_component, &provision.properties.env);
            let plugins = resolve_plugin_parameters(
                &materialization_component,
                &provision.properties.plugins,
            );
            let (config, env, plugins) = match (config, env, plugins, files_valid) {
                (Ok(config), Ok(env), Ok(plugins), true) => (config, env, plugins),
                (config, env, plugins, _) => {
                    for error in [config.err(), env.err(), plugins.err()]
                        .into_iter()
                        .flatten()
                    {
                        issues.push(ToolValidationIssue::error(
                            ToolValidationPhase::LocalResolution,
                            ToolValidationCode::InvalidProvision,
                            ToolEntityPath::tool(tool_name, "tools"),
                            declaration_source.clone(),
                            format!("Failed to materialize tool provision properties: {error:#}"),
                        ));
                    }
                    continue;
                }
            };

            let manifest_config = ToolManifestDeploymentConfig {
                provision: ToolManifestProvisionConfig {
                    config: NormalizedJsonValue::new(config),
                    env,
                    files: provision.properties.files,
                    plugins,
                },
                environment_binding,
                agent_bindings,
            };

            if let Some(component_name) = source.implementation.local_component_name() {
                configs_by_component
                    .entry(component_name.clone())
                    .or_default()
                    .insert(tool_name.clone(), manifest_config);
            } else if let Some(grant) = source.implementation.release_grant() {
                let (provision, pending_files) = self
                    .materialize_remote_tool_provision(
                        tool_name,
                        &manifest_config.provision,
                        &plugin_grants,
                    )
                    .await?;
                pending_remote_initial_files.extend(pending_files);
                let request = RemoteToolDeployment {
                    name: tool_name.clone(),
                    release: ToolReleaseReference::ById(ToolReleaseById {
                        release_id: grant.release.id,
                    }),
                    provision: provision.clone(),
                    environment_binding: manifest_config.environment_binding.clone(),
                    agent_bindings: manifest_config.agent_bindings.clone(),
                };
                let bindings = effective_remote_tool_bindings(
                    &agent_components,
                    manifest_config.environment_binding.as_ref(),
                    &manifest_config.agent_bindings,
                );
                diffable_remote_tool_deployments.insert(
                    tool_name.to_string(),
                    diff::RemoteToolDeployment {
                        release_id: grant.release.id,
                        version: grant.release.version.clone(),
                        source_digest: grant.release.source_digest,
                        owner_account_id: grant.release_owner.id,
                        owner_account_email: grant.release_owner.email.clone(),
                        metadata_version: grant.release.metadata_version.clone(),
                        metadata_digest: grant.release.metadata_digest,
                        provision,
                        bindings,
                    }
                    .into(),
                );
                remote_tool_deployments.push(request);
            }
        }

        let published_tools = app
            .selected_published_tools()
            .map(ToolName::try_from)
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(anyhow::Error::msg)?;
        for tool_name in &published_tools {
            let local_count = implementations
                .get(tool_name)
                .into_iter()
                .flatten()
                .filter(|implementation| {
                    implementation
                        .implementation
                        .local_component_name()
                        .is_some()
                })
                .count();
            if local_count != 1 {
                issues.push(ToolValidationIssue::error(
                    ToolValidationPhase::DeclarationDiscoveryIdentity,
                    ToolValidationCode::MissingImplementation,
                    ToolEntityPath::tool(tool_name, "environments.publishTools"),
                    app.selected_environment_source().map(std::path::Path::to_path_buf),
                    format!("Published tool must resolve to exactly one local implementation, found {local_count}"),
                ));
            }
        }

        let mut validation = ValidationBuilder::new();
        add_tool_issues(&mut validation, issues);
        let configs_by_component = validated_to_anyhow(
            "Tool manifest validation failed",
            validation.build(configs_by_component),
            None,
        )?;

        for (component_name, tool_configs) in configs_by_component {
            components
                .get_mut(&component_name)
                .expect("implementing component must be deployable")
                .tool_deployment_configs = tool_configs;
        }

        let mut seen_initial_files = HashSet::new();
        pending_remote_initial_files.retain(|file| seen_initial_files.insert(file.content_hash));

        Ok((
            remote_tool_deployments,
            diffable_remote_tool_deployments,
            published_tools,
            pending_remote_initial_files,
        ))
    }

    async fn materialize_remote_tool_provision(
        &self,
        tool_name: &ToolName,
        provision: &ToolManifestProvisionConfig,
        plugin_grants: &HashMap<PluginNameAndVersion, EnvironmentPluginGrantWithDetails>,
    ) -> anyhow::Result<(ToolProvisionConfig, Vec<PendingRemoteInitialFile>)> {
        let plugins = provision
            .plugins
            .iter()
            .enumerate()
            .map(|(index, plugin)| {
                let grant = plugin_grants
                    .get(&PluginNameAndVersion {
                        name: plugin.name.clone(),
                        version: plugin.version.clone(),
                    })
                    .with_context(|| {
                        format!(
                            "Plugin {}/{} required by remote tool {} is not granted to this environment",
                            plugin.name, plugin.version, tool_name
                        )
                    })?;
                let PluginSpecDto::OplogProcessor(spec) = &grant.plugin.spec;
                Ok(InstalledPlugin {
                    environment_plugin_grant_id: grant.id,
                    priority: PluginPriority(index as i32),
                    parameters: plugin.parameters.clone().into_iter().collect(),
                    plugin_registration_id: grant.plugin.id,
                    plugin_name: grant.plugin.name.clone(),
                    plugin_version: grant.plugin.version.clone(),
                    oplog_processor_component_id: Some(spec.component_id),
                    oplog_processor_component_revision: Some(spec.component_revision),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let files = provision
            .files
            .iter()
            .map(|file| {
                crate::model::app::InitialComponentFileSource::new(
                    &file.file.source_path,
                    &file.source,
                )
                .map(|source| crate::model::app::InitialComponentFile {
                    source,
                    target: crate::model::app::CanonicalFilePathWithPermissions {
                        path: file.file.target_path.clone(),
                        permissions: file.file.permissions.unwrap_or_default(),
                    },
                })
                .map_err(anyhow::Error::msg)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let loaded_files = IfsFileManager::new(self.ctx.file_download_client().clone())
            .load_initial_files(&files)
            .await?;
        let mut initial_files = Vec::with_capacity(loaded_files.len());
        let mut pending_files = Vec::with_capacity(loaded_files.len());
        for file in loaded_files {
            let content_hash = AgentFileContentHash(diff::Hash::new(file.content_hash));
            let size = file.size;
            initial_files.push(InitialAgentFile {
                content_hash,
                path: AgentFilePath::from_abs_str(file.target.path.as_abs_str())
                    .map_err(anyhow::Error::msg)?,
                permissions: file.target.permissions,
                size,
            });
            pending_files.push(PendingRemoteInitialFile {
                content: file.content,
                content_hash,
                size,
            });
        }

        Ok((
            ToolProvisionConfig {
                config: provision.config.clone(),
                env: provision.env.clone(),
                plugins,
                files: initial_files,
            },
            pending_files,
        ))
    }

    pub async fn component_deploy_properties(
        &self,
        component_name: &ComponentName,
    ) -> anyhow::Result<ComponentDeployProperties> {
        let mut app_ctx = self.ctx.app_context_lock_mut().await?;
        let app_ctx = app_ctx.some_or_err_mut()?;

        let extracted_metadata = extract_and_store_component_metadata(
            &BuildContext::new(app_ctx, &BuildConfig::new()),
            component_name,
        )
        .await?;
        let agent_types = extracted_metadata.agent_types;
        let tools = extracted_metadata.tools;
        let component = app_ctx.application().component(component_name);
        let wasm_path = component.final_wasm();

        let mapping = agent_types
            .iter()
            .map(|agent_type| (agent_type.type_name.clone(), component_name.clone()))
            .collect::<BTreeMap<_, _>>();
        let resolved_agents = app_ctx.application().resolve_agents(&mapping)?;

        let mut agent_type_configs =
            BTreeMap::<AgentTypeName, AgentTypeManifestProvisionConfig>::new();
        let mut unused_config_by_agent = BTreeMap::<AgentTypeName, Vec<String>>::new();

        for agent_type in &agent_types {
            let Some(resolved_agent) = resolved_agents.agent(&agent_type.type_name) else {
                continue;
            };

            let unused_paths =
                collect_unused_agent_config_paths(agent_type, resolved_agent.config());
            if !unused_paths.is_empty() {
                unused_config_by_agent.insert(agent_type.type_name.clone(), unused_paths);
            }

            agent_type_configs.insert(
                agent_type.type_name.clone(),
                AgentTypeManifestProvisionConfig {
                    env: resolve_env_vars(component_name, resolved_agent.env())?,
                    config: resolve_config_values(
                        component_name,
                        &agent_type.type_name,
                        materialize_agent_config_entries(agent_type, resolved_agent.config()),
                    )?,
                    initial_card: resolved_agent
                        .initial_card()
                        .map(initial_permission_from_manifest_card)
                        .transpose()
                        .with_context(|| {
                            format!(
                                "Invalid initialCard for component {} and agent {}",
                                component_name.0, agent_type.type_name.0
                            )
                        })?,
                    files_source: component.source().to_path_buf(),
                    files: resolved_agent.files().to_vec(),
                    plugins: resolve_plugin_parameters(component_name, resolved_agent.plugins())?,
                },
            );
        }

        if !unused_config_by_agent.is_empty() {
            for (agent_id, unused_keys) in &unused_config_by_agent {
                log_warn_action(
                    "Ignoring unused config keys",
                    format!(
                        "for agent {}: {}",
                        agent_id.0.log_color_highlight(),
                        unused_keys.join(", ")
                    ),
                );
            }

            if !self
                .ctx
                .interactive_handler()
                .confirm_ignore_unused_agent_config(&unused_config_by_agent)?
            {
                bail!(NonSuccessfulExit);
            }
        }

        Ok(ComponentDeployProperties {
            wasm_path,
            agent_types,
            tools,
            agent_type_configs,
            tool_deployment_configs: BTreeMap::new(),
        })
    }

    pub async fn diffable_local_component(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        component_name: &ComponentName,
        properties: &ComponentDeployProperties,
    ) -> anyhow::Result<diff::Component> {
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

        let plugin_grants = self
            .ctx
            .environment_handler()
            .plugin_grants(environment)
            .await?;

        let ifs_manager = IfsFileManager::new(self.ctx.file_download_client().clone());

        let mut agent_type_provision_configs = BTreeMap::new();
        for (agent_type_name, manifest_config) in &properties.agent_type_configs {
            // Hash files for this agent type
            let resolved_files: Vec<crate::model::app::InitialComponentFile> = manifest_config
                .files
                .iter()
                .map(|f| {
                    crate::model::app::InitialComponentFileSource::new(
                        &f.source_path,
                        &manifest_config.files_source,
                    )
                    .map_err(|err| {
                        anyhow!(
                            "Failed to resolve source path '{}' for component {} and agent {}: {}",
                            f.source_path,
                            component_name.0,
                            agent_type_name.0,
                            err
                        )
                    })
                    .map(|source| crate::model::app::InitialComponentFile {
                        source,
                        target: crate::model::app::CanonicalFilePathWithPermissions {
                            path: f.target_path.clone(),
                            permissions: f.permissions.unwrap_or(
                                golem_common::model::component::AgentFilePermissions::ReadOnly,
                            ),
                        },
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;

            let file_hashes = ifs_manager
                .collect_file_hashes(
                    &format!("{}:{}", component_name.0, agent_type_name.0),
                    &resolved_files,
                )
                .await?;

            let files_by_path = file_hashes
                .into_iter()
                .map(|file_hash| {
                    (
                        file_hash.target.path.to_abs_string(),
                        diff::AgentFile {
                            hash: file_hash.hash.into(),
                            permissions: file_hash.target.permissions,
                        }
                        .into(),
                    )
                })
                .collect();

            // TODO: atomic: cannot lookup by account email
            // Look up plugin grants
            let plugins_by_grant_id = manifest_config
                .plugins
                .iter()
                .enumerate()
                .map(|(idx, p)| {
                    let grant = plugin_grants
                        .get(&PluginNameAndVersion {
                            name: p.name.clone(),
                            version: p.version.clone(),
                        })
                        .ok_or_else(|| {
                            anyhow!(
                                "Plugin {}/{} is not available in this environment. \
                                 Use 'golem plugin list' to see available plugins, \
                                 or grant the plugin to this environment first.",
                                p.name,
                                p.version
                            )
                        })?;
                    Ok((
                        grant.id.0,
                        diff::PluginInstallation {
                            priority: idx as i32,
                            name: p.name.clone(),
                            version: p.version.clone(),
                            grant_id: grant.id.0,
                            parameters: p
                                .parameters
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        },
                    ))
                })
                .collect::<anyhow::Result<_>>()?;

            let config = manifest_config
                .config
                .iter()
                .map(|c| (c.path.join("."), c.value.clone()))
                .collect();

            let provision_config = diff::AgentTypeProvisionConfig {
                env: manifest_config.env.clone(),
                config,
                files_by_path,
                plugins_by_grant_id,
                initial_permissions: {
                    let context = initial_permission_recipient_context(
                        environment,
                        component_name,
                        agent_type_name,
                    );
                    let initial_permission = manifest_config.to_initial_permission(&context);
                    diff::AgentTypeInitialPermission {
                        lower_positive: initial_permission.lower_bound.positive,
                        lower_negative: initial_permission.lower_bound.negative,
                        upper_positive: initial_permission.upper_bound.positive,
                        upper_negative: initial_permission.upper_bound.negative,
                    }
                },
            };

            agent_type_provision_configs.insert(agent_type_name.0.clone(), provision_config.into());
        }

        let tools_by_name = properties
            .tools
            .iter()
            .filter_map(|tool| tool.name().map(|name| (name, tool)))
            .collect::<BTreeMap<_, _>>();
        let mut tool_deployment_configs = BTreeMap::new();
        for (tool_name, manifest_config) in &properties.tool_deployment_configs {
            let definition = tools_by_name.get(tool_name.as_str()).ok_or_else(|| {
                anyhow!(
                    "Missing discovered definition for resolved tool {}",
                    tool_name.as_str().log_color_error_highlight()
                )
            })?;
            let resolved_files = manifest_config
                .provision
                .files
                .iter()
                .map(|file| {
                    crate::model::app::InitialComponentFileSource::new(
                        &file.file.source_path,
                        &file.source,
                    )
                    .map_err(|error| {
                        anyhow!(
                            "Failed to resolve source path '{}' for component {} and tool {}: {}",
                            file.file.source_path,
                            component_name.0,
                            tool_name,
                            error
                        )
                    })
                    .map(|source| crate::model::app::InitialComponentFile {
                        source,
                        target: crate::model::app::CanonicalFilePathWithPermissions {
                            path: file.file.target_path.clone(),
                            permissions: file.file.permissions.unwrap_or(
                                golem_common::model::component::AgentFilePermissions::ReadOnly,
                            ),
                        },
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let file_hashes = ifs_manager
                .collect_file_hashes(
                    &format!("{}:tool:{}", component_name.0, tool_name),
                    &resolved_files,
                )
                .await?;
            let files_by_path = file_hashes
                .into_iter()
                .map(|file_hash| {
                    (
                        file_hash.target.path.to_abs_string(),
                        diff::AgentFile {
                            hash: file_hash.hash.into(),
                            permissions: file_hash.target.permissions,
                        }
                        .into(),
                    )
                })
                .collect();
            let plugins_by_grant_id = manifest_config
                .provision
                .plugins
                .iter()
                .enumerate()
                .map(|(index, plugin)| {
                    let grant = plugin_grants
                        .get(&PluginNameAndVersion {
                            name: plugin.name.clone(),
                            version: plugin.version.clone(),
                        })
                        .ok_or_else(|| {
                            anyhow!(
                                "Plugin {}/{} is not available in this environment. Use 'golem plugin list' to see available plugins, or grant the plugin to this environment first.",
                                plugin.name,
                                plugin.version
                            )
                        })?;
                    Ok((
                        grant.id.0,
                        diff::PluginInstallation {
                            priority: index as i32,
                            name: plugin.name.clone(),
                            version: plugin.version.clone(),
                            grant_id: grant.id.0,
                            parameters: plugin.parameters.clone().into_iter().collect(),
                        },
                    ))
                })
                .collect::<anyhow::Result<_>>()?;

            tool_deployment_configs.insert(
                tool_name.as_str().to_string(),
                diff::ToolDeploymentConfig {
                    definition: (*definition).clone(),
                    config: manifest_config.provision.config.clone(),
                    env: manifest_config.provision.env.clone(),
                    files_by_path,
                    plugins_by_grant_id,
                    environment_binding: manifest_config.environment_binding.clone(),
                    agent_bindings: manifest_config
                        .agent_bindings
                        .iter()
                        .map(|(agent_name, binding)| (agent_name.0.clone(), binding.clone()))
                        .collect(),
                }
                .into(),
            );
        }

        Ok(diff::Component {
            wasm_hash: component_binary_hash.into(),
            agent_type_provision_configs,
            tool_deployment_configs,
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
        )?;

        let wasm = component_stager.open_wasm().await?;
        let agent_types: Vec<AgentTypeSchema> = component_stager.agent_types().clone();

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
                    agent_types,
                    agent_type_provision_configs: component_stager
                        .agent_type_provision_configs(environment, component_name)
                        .await?,
                    tools: component_stager.tools().clone(),
                    tool_deployment_configs: component_stager.tool_deployment_configs().await?,
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
        allow_incompatible_config: bool,
    ) -> UpdateStagedComponentResult<()> {
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
                .await
                .map_err(UpdateStagedComponentError::Other)?,
            Some(diff),
        )
        .map_err(UpdateStagedComponentError::Other)?;

        let wasm = component_stager
            .open_wasm_if_changed()
            .await
            .map_err(UpdateStagedComponentError::Other)?;
        let agent_types = component_stager.agent_types_if_changed().cloned();

        // NOTE: do not drop until the component is created, keeps alive the temp archive
        let changed_files = component_stager
            .changed_files()
            .await
            .map_err(UpdateStagedComponentError::Other)?;

        let component = self
            .ctx
            .golem_clients()
            .await
            .map_err(UpdateStagedComponentError::Other)?
            .component
            .update_component(
                &component.id.0,
                &ComponentUpdate {
                    current_revision: component.revision,
                    agent_types,
                    agent_type_provision_config_updates: component_stager
                        .agent_type_provision_config_updates(
                            environment,
                            &component.name,
                            &changed_files,
                        )
                        .await
                        .map_err(UpdateStagedComponentError::Other)?,
                    tools: component_stager.tools_if_changed().cloned(),
                    tool_deployment_config_updates: component_stager
                        .tool_deployment_config_updates_if_changed(&changed_files)
                        .await
                        .map_err(UpdateStagedComponentError::Other)?,
                    allow_incompatible_config,
                },
                wasm,
                changed_files
                    .open_archive()
                    .await
                    .map_err(UpdateStagedComponentError::Other)?,
            )
            .await
            .map_err(|err| UpdateStagedComponentError::Service(err.into()))?;

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
                    Ok(self
                        .ctx
                        .golem_clients()
                        .await?
                        .component
                        .get_deployment_component(
                            &environment.environment_id.0,
                            current_deployment_revision.get(),
                            component_name.0.as_str(),
                        )
                        .await
                        .map_service_error_not_found_as_opt()?)
                },
            )
            .await
    }

    /// Best-effort, cached lookup of a component's user-facing release version string at a given
    /// revision. Returns `None` if the revision can't be fetched or has no version set.
    pub async fn component_version_at(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
    ) -> Option<String> {
        self.get_component_revision_by_id(component_id, revision)
            .await
            .ok()
            .and_then(|component| component.metadata.root_package_version().clone())
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
                        .map_err(|err| Arc::new(err.into()))
                }
            })
            .await
            .map_err(|err| anyhow!(err))
    }
}

fn resolve_tool_binding_map(
    merge_mode: crate::model::cascade::property::map::MapMergeMode,
    bindings: indexmap::IndexMap<String, app_raw::ToolBinding>,
) -> BTreeMap<String, ToolBindingState> {
    match merge_mode {
        crate::model::cascade::property::map::MapMergeMode::Remove => BTreeMap::new(),
        crate::model::cascade::property::map::MapMergeMode::Upsert
        | crate::model::cascade::property::map::MapMergeMode::Replace => bindings
            .into_iter()
            .map(|(name, binding)| (name, ToolBindingState::from_binding(binding)))
            .collect(),
    }
}

fn validate_tool_binding_references(
    issues: &mut Vec<ToolValidationIssue>,
    bindings: &BTreeMap<String, ToolBindingState>,
    field_prefix: &str,
    agent_name: Option<&AgentTypeName>,
    source: Option<&std::path::Path>,
    implementations: &BTreeMap<ToolName, Vec<DiscoveredToolImplementation>>,
) {
    for raw_name in bindings.keys() {
        let field_path = format!("{field_prefix}.{raw_name}");
        let path = match agent_name {
            Some(agent_name) => ToolEntityPath::agent(agent_name, field_path),
            None => ToolEntityPath::tool(raw_name, field_path),
        };
        if raw_name == "middleware" {
            issues.push(ToolValidationIssue::error(
                ToolValidationPhase::BindingReferences,
                ToolValidationCode::ReservedMiddleware,
                path,
                source.map(std::path::Path::to_path_buf),
                "Tool middleware bindings are reserved for GOL-39",
            ));
            continue;
        }
        match ToolName::try_from(raw_name.as_str()) {
            Ok(tool_name) if implementations.contains_key(&tool_name) => {}
            Ok(_) => issues.push(ToolValidationIssue::error(
                ToolValidationPhase::BindingReferences,
                ToolValidationCode::UnknownToolReference,
                path,
                source.map(std::path::Path::to_path_buf),
                "Binding references a tool that is not implemented by this application",
            )),
            Err(message) => issues.push(ToolValidationIssue::error(
                ToolValidationPhase::BindingReferences,
                ToolValidationCode::InvalidName,
                path,
                source.map(std::path::Path::to_path_buf),
                message,
            )),
        }
    }
}

fn resolve_tool_binding_input(
    issues: &mut Vec<ToolValidationIssue>,
    tool_name: &ToolName,
    definition: &Tool,
    owner: &AccountEmail,
    state: &ToolBindingState,
    field_prefix: &str,
    agent_name: Option<&AgentTypeName>,
    source: Option<&std::path::Path>,
) -> Option<ToolBindingInput> {
    let entity_path = |field: &str| {
        let field_path = format!("{field_prefix}.{tool_name}.{field}");
        match agent_name {
            Some(agent_name) => ToolEntityPath::agent(agent_name, field_path),
            None => ToolEntityPath::tool(tool_name, field_path),
        }
    };

    if let Some(version) = &state.version
        && version != &definition.version
    {
        issues.push(ToolValidationIssue::error(
            ToolValidationPhase::LocalResolution,
            ToolValidationCode::VersionMismatch,
            entity_path("version"),
            source.map(std::path::Path::to_path_buf),
            format!(
                "Binding version '{version}' does not match local tool version '{}'",
                definition.version
            ),
        ));
    }

    let account = state.account.as_ref().map(AccountEmail::new);
    if let Some(account) = &account
        && account != owner
    {
        issues.push(ToolValidationIssue::error(
            ToolValidationPhase::LocalResolution,
            ToolValidationCode::AccountMismatch,
            entity_path("account"),
            source.map(std::path::Path::to_path_buf),
            format!(
                "Binding account '{}' does not match the local component owner '{}'",
                account, owner
            ),
        ));
    }

    let raw_parameters = serde_json::Value::Object(
        state
            .parameters
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    let renderer = crate::command_handler::template::EnvVarRenderer::new();
    let parameters = match renderer.render_json_value(&raw_parameters) {
        Ok(parameters) => parameters,
        Err(error) => {
            issues.push(ToolValidationIssue::error(
                ToolValidationPhase::BindingSemantics,
                ToolValidationCode::InvalidParameters,
                entity_path("parameters"),
                source.map(std::path::Path::to_path_buf),
                format!("Failed to resolve tool parameters: {error}"),
            ));
            raw_parameters
        }
    };

    let readable = resolve_secret_scope(
        issues,
        &state.secret_keys_readable,
        entity_path("secretKeysReadable"),
        source,
    );
    let requested_revealable = resolve_secret_scope(
        issues,
        &state.secret_keys_revealable,
        entity_path("secretKeysRevealable"),
        source,
    );
    let revealable = requested_revealable.intersection(&readable);
    if revealable != requested_revealable {
        issues.push(ToolValidationIssue::warning(
            ToolValidationPhase::BindingSemantics,
            ToolValidationCode::RevealableScopeNarrowed,
            entity_path("secretKeysRevealable"),
            source.map(std::path::Path::to_path_buf),
            "Revealable secret keys outside the readable scope were dropped",
        ));
    }

    Some(ToolBindingInput {
        version: Some(definition.version.clone()),
        parameters: NormalizedJsonValue::new(parameters),
        account: Some(account.unwrap_or_else(|| owner.clone())),
        secret_keys_readable: readable,
        secret_keys_revealable: revealable,
    })
}

fn validate_effective_tool_binding(
    issues: &mut Vec<ToolValidationIssue>,
    tool_name: &ToolName,
    environment: Option<&ToolBindingInput>,
    agent: &ToolBindingInput,
    agent_name: &AgentTypeName,
    source: &std::path::Path,
) {
    let Some(environment) = environment else {
        return;
    };

    let readable = environment
        .secret_keys_readable
        .intersection(&agent.secret_keys_readable);
    let requested_revealable = environment
        .secret_keys_revealable
        .intersection(&agent.secret_keys_revealable);
    if requested_revealable.intersection(&readable) != requested_revealable {
        issues.push(ToolValidationIssue::warning(
            ToolValidationPhase::BindingSemantics,
            ToolValidationCode::RevealableScopeNarrowed,
            ToolEntityPath::agent(
                agent_name,
                format!("tools.{tool_name}.secretKeysRevealable"),
            ),
            Some(source.to_path_buf()),
            "Effective revealable secret keys outside the environment-and-agent readable scope were dropped",
        ));
    }
}

fn effective_remote_tool_bindings(
    agent_components: &BTreeMap<AgentTypeName, ComponentName>,
    environment: Option<&ToolBindingInput>,
    agents: &BTreeMap<AgentTypeName, ToolBindingInput>,
) -> BTreeMap<AgentTypeName, diff::EffectiveToolBinding> {
    agent_components
        .keys()
        .filter_map(|agent_name| {
            let agent = agents.get(agent_name);
            diff::effective_tool_binding(environment, agent)
                .map(|(binding, _)| (agent_name.clone(), binding))
        })
        .collect()
}

fn resolve_secret_scope(
    issues: &mut Vec<ToolValidationIssue>,
    layers: &[app_raw::ManifestSecretKeyScope],
    path: ToolEntityPath,
    source: Option<&std::path::Path>,
) -> SecretKeyScope {
    layers.iter().fold(SecretKeyScope::All, |resolved, layer| {
        let next = match layer {
            app_raw::ManifestSecretKeyScope::All(value) if value == "*" => SecretKeyScope::All,
            app_raw::ManifestSecretKeyScope::All(value) => {
                issues.push(ToolValidationIssue::error(
                    ToolValidationPhase::BindingSemantics,
                    ToolValidationCode::InvalidSecretScope,
                    path.clone(),
                    source.map(std::path::Path::to_path_buf),
                    format!("Expected '*' or a list of secret paths, found '{value}'"),
                ));
                SecretKeyScope::All
            }
            app_raw::ManifestSecretKeyScope::Keys(paths) => {
                let mut canonical_paths = BTreeSet::new();
                for raw_path in paths {
                    if raw_path == "*" {
                        issues.push(ToolValidationIssue::error(
                            ToolValidationPhase::BindingSemantics,
                            ToolValidationCode::InvalidSecretScope,
                            path.clone(),
                            source.map(std::path::Path::to_path_buf),
                            "'*' must be used as the whole secret scope, not as a list entry",
                        ));
                        continue;
                    }
                    match crate::args::parse_agent_config_path(raw_path) {
                        Ok(segments) => {
                            canonical_paths.insert(
                                CanonicalAgentSecretPath::from_path_in_unknown_casing(&segments),
                            );
                        }
                        Err(error) => issues.push(ToolValidationIssue::error(
                            ToolValidationPhase::BindingSemantics,
                            ToolValidationCode::InvalidSecretScope,
                            path.clone(),
                            source.map(std::path::Path::to_path_buf),
                            format!("Invalid secret path '{raw_path}': {error}"),
                        )),
                    }
                }
                SecretKeyScope::Keys(canonical_paths)
            }
        };
        resolved.intersection(&next)
    })
}

fn resolve_json_value(
    component_name: &ComponentName,
    value_kind: &str,
    value: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    crate::command_handler::template::EnvVarRenderer::new()
        .render_json_value(&value)
        .with_context(|| {
            format!(
                "Failed to prepare {value_kind} for component {}",
                component_name.as_str().log_color_highlight()
            )
        })
}

fn resolve_env_vars(
    component_name: &ComponentName,
    env: &BTreeMap<String, String>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let renderer = crate::command_handler::template::EnvVarRenderer::new();

    let mut resolved_env = BTreeMap::new();
    let mut validation = ValidationBuilder::new();
    validation.with_context(
        vec![("component", component_name.to_string())],
        |validation| {
            for key in env.keys().sorted() {
                let value = env.get(key).unwrap();
                match renderer.render_str(value) {
                    Ok(resolved_value) => {
                        resolved_env.insert(key.clone(), resolved_value);
                    }
                    Err(err) => {
                        let missing_env_vars = renderer.missing_env_vars(value, &err);
                        let error_message = if missing_env_vars.is_empty() {
                            format!(
                                "Failed to substitute environment variable(s) for {}",
                                key.log_color_highlight()
                            )
                        } else {
                            format!(
                                "Failed to substitute environment variable(s) ({}) for {}",
                                missing_env_vars
                                    .iter()
                                    .map(|key| key.log_color_highlight())
                                    .join(", "),
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

    validated_to_anyhow(
        &format!(
            "Failed to prepare environment variables for component: {}",
            component_name.as_str().log_color_highlight()
        ),
        validation.build(resolved_env),
        None,
    )
}

fn resolve_plugin_parameters(
    component_name: &ComponentName,
    plugins: &[app_raw::PluginInstallation],
) -> anyhow::Result<Vec<app_raw::PluginInstallation>> {
    let renderer = crate::command_handler::template::EnvVarRenderer::new();

    let mut resolved_plugins = Vec::with_capacity(plugins.len());
    let mut validation = ValidationBuilder::new();
    validation.with_context(
        vec![("component", component_name.to_string())],
        |validation| {
            for plugin in plugins {
                validation.with_context(
                    vec![
                        ("plugin", plugin.name.clone()),
                        ("version", plugin.version.clone()),
                    ],
                    |validation| {
                        let mut resolved_parameters = HashMap::with_capacity(plugin.parameters.len());
                        for key in plugin.parameters.keys().sorted() {
                            let value = plugin.parameters.get(key).unwrap();
                            match renderer.render_str(value) {
                                Ok(resolved_value) => {
                                    resolved_parameters.insert(key.clone(), resolved_value);
                                }
                                Err(err) => {
                                    let missing_env_vars = renderer.missing_env_vars(value, &err);
                                    let error_message = if missing_env_vars.is_empty() {
                                        format!(
                                            "Failed to substitute environment variable(s) for plugin parameter {}",
                                            key.log_color_highlight()
                                        )
                                    } else {
                                        format!(
                                            "Failed to substitute environment variable(s) ({}) for plugin parameter {}",
                                            missing_env_vars
                                                .iter()
                                                .map(|key| key.log_color_highlight())
                                                .join(", "),
                                            key.log_color_highlight()
                                        )
                                    };
                                    let mut context = vec![
                                        ("key", key.to_string()),
                                        ("template", value.to_string()),
                                        (
                                            "error",
                                            err.to_string()
                                                .log_color_error_highlight()
                                                .to_string(),
                                        ),
                                    ];
                                    if !missing_env_vars.is_empty() {
                                        context.push(("missing", missing_env_vars.join(", ")));
                                    }
                                    validation.with_context(context, |validation| {
                                        validation.add_error(error_message)
                                    });
                                }
                            }
                        }
                        resolved_plugins.push(app_raw::PluginInstallation {
                            account: plugin.account.clone(),
                            name: plugin.name.clone(),
                            version: plugin.version.clone(),
                            parameters: resolved_parameters,
                        });
                    },
                );
            }
        },
    );

    validated_to_anyhow(
        &format!(
            "Failed to prepare plugin parameters for component: {}",
            component_name.as_str().log_color_highlight()
        ),
        validation.build(resolved_plugins),
        None,
    )
}

fn resolve_config_values(
    component_name: &ComponentName,
    agent_type_name: &AgentTypeName,
    entries: Vec<AgentConfigEntryDto>,
) -> anyhow::Result<Vec<AgentConfigEntryDto>> {
    let renderer = crate::command_handler::template::EnvVarRenderer::new();

    let mut resolved_entries = Vec::with_capacity(entries.len());
    let mut validation = ValidationBuilder::new();
    validation.with_context(
        vec![
            ("component", component_name.to_string()),
            ("agentType", agent_type_name.0.clone()),
        ],
        |validation| {
            for entry in entries {
                let raw_value: serde_json::Value = entry.value.clone().into();
                match renderer.render_json_value(&raw_value) {
                    Ok(resolved_value) => {
                        resolved_entries.push(AgentConfigEntryDto {
                            path: entry.path,
                            value: resolved_value.into(),
                        });
                    }
                    Err(err) => {
                        let template = raw_value.to_string();
                        let missing_env_vars = renderer.missing_env_vars(&template, &err);
                        let path = entry.path.join(".");
                        let error_message = if missing_env_vars.is_empty() {
                            format!(
                                "Failed to substitute environment variable(s) for config {}",
                                path.log_color_highlight()
                            )
                        } else {
                            format!(
                                "Failed to substitute environment variable(s) ({}) for config {}",
                                missing_env_vars
                                    .iter()
                                    .map(|key| key.log_color_highlight())
                                    .join(", "),
                                path.log_color_highlight()
                            )
                        };
                        let mut context = vec![
                            ("path", path),
                            ("template", template),
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
                }
            }
        },
    );

    validated_to_anyhow(
        &format!(
            "Failed to prepare config values for component: {}",
            component_name.as_str().log_color_highlight()
        ),
        validation.build(resolved_entries),
        None,
    )
}

fn materialize_agent_config_entries(
    agent_type: &AgentTypeSchema,
    config_root: Option<&serde_json::Value>,
) -> Vec<AgentConfigEntryDto> {
    let Some(config_root) = config_root else {
        return vec![];
    };

    agent_type
        .config
        .iter()
        .filter(|decl| decl.source == AgentConfigSource::Local)
        .filter_map(|decl| {
            value_at_path(config_root, &decl.path).map(|value| AgentConfigEntryDto {
                path: decl.path.clone(),
                value: value.clone().into(),
            })
        })
        .collect()
}

fn collect_unused_agent_config_paths(
    agent_type: &AgentTypeSchema,
    config_root: Option<&serde_json::Value>,
) -> Vec<String> {
    let Some(config_root) = config_root else {
        return vec![];
    };

    let declared_paths = agent_type
        .config
        .iter()
        .filter(|decl| decl.source == AgentConfigSource::Local)
        .map(|decl| decl.path.clone())
        .collect::<BTreeSet<_>>();

    let mut unused = collect_unused_leaf_paths(config_root, |path| {
        declared_paths
            .iter()
            .any(|declared_path| path.starts_with(declared_path))
    })
    .into_iter()
    .map(|path| path.join("."))
    .collect::<Vec<_>>();
    unused.sort();
    unused
}

#[cfg(test)]
mod tool_binding_tests {
    use super::{
        effective_remote_tool_bindings, resolve_secret_scope, validate_effective_tool_binding,
    };
    use crate::model::app_raw::ManifestSecretKeyScope;
    use crate::model::tool_deployment::{
        ToolEntityPath, ToolValidationCode, ToolValidationSeverity,
    };
    use golem_common::model::account::AccountEmail;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::agent_secret::CanonicalAgentSecretPath;
    use golem_common::model::component::ComponentName;
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::tool::{SecretKeyScope, ToolBindingInput, ToolName};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use test_r::test;

    fn keys(values: &[&str]) -> SecretKeyScope {
        SecretKeyScope::Keys(
            values
                .iter()
                .map(|value| CanonicalAgentSecretPath(vec![(*value).to_string()]))
                .collect(),
        )
    }

    fn binding(readable: SecretKeyScope, revealable: SecretKeyScope) -> ToolBindingInput {
        ToolBindingInput {
            version: Some("1.0.0".to_string()),
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            account: Some(AccountEmail::new("owner@example.com")),
            secret_keys_readable: readable,
            secret_keys_revealable: revealable,
        }
    }

    #[test]
    fn secret_scope_layers_are_canonicalized_deduplicated_and_intersected() {
        let mut issues = Vec::new();
        let scope = resolve_secret_scope(
            &mut issues,
            &[
                ManifestSecretKeyScope::All("*".to_string()),
                ManifestSecretKeyScope::Keys(vec![
                    "Credentials.GitHub".to_string(),
                    "credentials.github".to_string(),
                    "credentials.gitlab".to_string(),
                ]),
                ManifestSecretKeyScope::Keys(vec!["credentials.github".to_string()]),
            ],
            ToolEntityPath::tool("grep", "tools.grep.secretKeysReadable"),
            Some(Path::new("golem.yaml")),
        );

        assert!(issues.is_empty());
        assert_eq!(
            scope,
            SecretKeyScope::Keys(BTreeSet::from([CanonicalAgentSecretPath(vec![
                "credentials".to_string(),
                "github".to_string(),
            ])]))
        );
    }

    #[test]
    fn wildcard_inside_secret_scope_list_is_rejected() {
        let mut issues = Vec::new();
        let scope = resolve_secret_scope(
            &mut issues,
            &[ManifestSecretKeyScope::Keys(vec!["*".to_string()])],
            ToolEntityPath::tool("grep", "tools.grep.secretKeysReadable"),
            Some(Path::new("golem.yaml")),
        );

        assert_eq!(scope, SecretKeyScope::Keys(BTreeSet::new()));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, ToolValidationCode::InvalidSecretScope);
        assert_eq!(issues[0].source.as_deref(), Some(Path::new("golem.yaml")));
    }

    #[test]
    fn environment_and_agent_secret_policies_are_checked_as_one_effective_binding() {
        let environment = binding(SecretKeyScope::All, keys(&["github"]));
        let agent = binding(keys(&["gitlab"]), SecretKeyScope::All);
        let mut issues = Vec::new();

        validate_effective_tool_binding(
            &mut issues,
            &ToolName::try_from("grep").unwrap(),
            Some(&environment),
            &agent,
            &AgentTypeName("CoderAgent".to_string()),
            Path::new("agents.yaml"),
        );

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, ToolValidationCode::RevealableScopeNarrowed);
        assert_eq!(issues[0].severity, ToolValidationSeverity::Warning);
        assert_eq!(issues[0].source.as_deref(), Some(Path::new("agents.yaml")));
    }

    #[test]
    fn remote_diff_binding_matches_server_parameter_merge_and_scope_intersection() {
        let agent_name = AgentTypeName("CoderAgent".to_string());
        let mut environment = binding(SecretKeyScope::All, keys(&["github", "gitlab"]));
        environment.parameters =
            NormalizedJsonValue::new(serde_json::json!({"shared": "environment", "base": 1}));
        let mut agent = binding(keys(&["github"]), SecretKeyScope::All);
        agent.parameters =
            NormalizedJsonValue::new(serde_json::json!({"shared": "agent", "extra": 2}));

        let bindings = effective_remote_tool_bindings(
            &BTreeMap::from([(agent_name.clone(), ComponentName("component".to_string()))]),
            Some(&environment),
            &BTreeMap::from([(agent_name.clone(), agent)]),
        );
        let effective = bindings.get(&agent_name).unwrap();

        assert_eq!(
            effective.parameters.0,
            serde_json::json!({"base": 1, "extra": 2, "shared": "agent"})
        );
        assert_eq!(effective.secret_keys_readable, keys(&["github"]));
        assert_eq!(effective.secret_keys_revealable, keys(&["github"]));
    }
}
