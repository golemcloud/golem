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

use crate::app::yaml_edit::AppYamlEditor;
use crate::command::api::definition::ApiDefinitionSubcommand;
use crate::command::shared_args::{ProjectOptionalFlagArg, UpdateOrRedeployArgs};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::{
    log_action, log_skipping_up_to_date, log_warn_action, logln, LogColorize, LogIndent,
};
use crate::model::api::{ApiDefinitionId, ApiDefinitionVersion, HttpApiDeployMode};
use crate::model::app::{
    ApplicationComponentSelectMode, DynamicHelpSections, HttpApiDefinitionName, WithSource,
};
use crate::model::app_raw::HttpApiDefinition;
use crate::model::component::Component;
use crate::model::deploy_diff::api_definition::DiffableHttpApiDefinition;
use crate::model::text::api_definition::{
    ApiDefinitionGetView, ApiDefinitionNewView, ApiDefinitionUpdateView,
};
use crate::model::text::fmt::{log_deploy_diff, log_error, log_warn};
use crate::model::{ComponentName, ProjectRefAndId};
use anyhow::{bail, Context as AnyhowContext};
use golem_client::api::ApiDefinitionClient;
use golem_client::model::{HttpApiDefinitionRequest, HttpApiDefinitionResponseData};
use itertools::Itertools;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub struct ApiDefinitionCommandHandler {
    ctx: Arc<Context>,
}

impl ApiDefinitionCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiDefinitionSubcommand) -> anyhow::Result<()> {
        match command {
            ApiDefinitionSubcommand::Deploy {
                http_api_definition_name,
                update_or_redeploy,
            } => {
                self.cmd_deploy(http_api_definition_name, update_or_redeploy)
                    .await
            }
            ApiDefinitionSubcommand::Get {
                project,
                id,
                version,
            } => self.cmd_get(project, id, version).await,
            ApiDefinitionSubcommand::Delete {
                project,
                id,
                version,
            } => self.cmd_delete(project, id, version).await,
            ApiDefinitionSubcommand::List { project, id } => self.cmd_list(project, id).await,
        }
    }

    async fn cmd_deploy(
        &self,
        name: Option<HttpApiDefinitionName>,
        update_or_redeploy: UpdateOrRedeployArgs,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(None)
            .await?;

        if let Some(name) = name.as_ref() {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;
            if !app_ctx
                .application
                .http_api_definitions()
                .keys()
                .contains(name)
            {
                logln("");
                log_error(format!(
                    "HTTP API definition {} not found in the application manifest",
                    name.as_str().log_color_highlight()
                ));
                logln("");
                app_ctx.log_dynamic_help(&DynamicHelpSections::show_api_definitions())?;
                bail!(NonSuccessfulExit)
            }
        }

        let api_def_filter = name.as_ref().into_iter().cloned().collect::<BTreeSet<_>>();

        let lastest_used_components = self
            .deploy_required_components(project.as_ref(), &update_or_redeploy, api_def_filter)
            .await?;

        match &name {
            Some(name) => {
                let definition = {
                    let app_ctx = self.ctx.app_context_lock().await;
                    let app_ctx = app_ctx.some_or_err()?;
                    app_ctx
                        .application
                        .http_api_definitions()
                        .get(name)
                        .unwrap()
                        .clone()
                };

                self.deploy_api_definition(
                    project.as_ref(),
                    HttpApiDeployMode::All,
                    &update_or_redeploy,
                    &lastest_used_components,
                    name,
                    &definition,
                )
                .await?;

                Ok(())
            }
            None => {
                self.deploy(
                    project.as_ref(),
                    HttpApiDeployMode::All,
                    &update_or_redeploy,
                    &lastest_used_components,
                )
                .await?;

                Ok(())
            }
        }
    }

    async fn cmd_get(
        &self,
        project: ProjectOptionalFlagArg,
        api_def_id: ApiDefinitionId,
        version: ApiDefinitionVersion,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        match self
            .api_definition(project.as_ref(), &api_def_id.0, &version.0)
            .await?
        {
            Some(result) => {
                self.ctx
                    .log_handler()
                    .log_view(&ApiDefinitionGetView(result));
                Ok(())
            }
            None => {
                log_error("Not found");
                bail!(NonSuccessfulExit)
            }
        }
    }

    async fn cmd_list(
        &self,
        project: ProjectOptionalFlagArg,
        api_definition_id: Option<ApiDefinitionId>,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let definitions = clients
            .api_definition
            .list_definitions(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project.as_ref())
                    .await?
                    .0,
                api_definition_id.as_ref().map(|id| id.0.as_str()),
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&definitions);

        Ok(())
    }

    async fn cmd_delete(
        &self,
        project: ProjectOptionalFlagArg,
        api_def_id: ApiDefinitionId,
        version: ApiDefinitionVersion,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        let clients = self.ctx.golem_clients().await?;

        clients
            .api_definition
            .delete_definition(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project.as_ref())
                    .await?
                    .0,
                &api_def_id.0,
                &version.0,
            )
            .await
            .map_service_error()?;

        log_warn_action(
            "Deleted",
            format!(
                "API definition: {}/{}",
                api_def_id.0.log_color_highlight(),
                version.0.log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn deploy(
        &self,
        project: Option<&ProjectRefAndId>,
        deploy_mode: HttpApiDeployMode,
        update_or_redeploy: &UpdateOrRedeployArgs,
        latest_component_versions: &BTreeMap<String, Component>,
    ) -> anyhow::Result<BTreeMap<String, String>> {
        let api_definitions = {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;
            app_ctx.application.http_api_definitions().clone()
        };

        let mut latest_api_definition_versions = BTreeMap::new();

        if !api_definitions.is_empty() {
            log_action("Deploying", "HTTP API definitions");

            for (api_definition_name, api_definition) in api_definitions {
                let _indent = LogIndent::new();
                let version = self
                    .deploy_api_definition(
                        project,
                        deploy_mode,
                        update_or_redeploy,
                        latest_component_versions,
                        &api_definition_name,
                        &api_definition,
                    )
                    .await?;

                if let Some(version) = version {
                    latest_api_definition_versions.insert(api_definition_name.to_string(), version);
                }
            }
        }

        Ok(latest_api_definition_versions)
    }

    pub async fn deploy_api_definition(
        &self,
        project: Option<&ProjectRefAndId>,
        deploy_mode: HttpApiDeployMode,
        update_or_redeploy: &UpdateOrRedeployArgs,
        latest_component_versions: &BTreeMap<String, Component>,
        api_definition_name: &HttpApiDefinitionName,
        api_definition: &WithSource<HttpApiDefinition>,
    ) -> anyhow::Result<Option<String>> {
        let skip_by_component_filter = match deploy_mode {
            HttpApiDeployMode::All => false,
            HttpApiDeployMode::Matching => !api_definition.value.routes.iter().any(|route| {
                match &route.binding.component_name {
                    Some(component_name) => latest_component_versions.contains_key(component_name),
                    None => false,
                }
            }),
        };

        if skip_by_component_filter {
            log_warn_action(
                "Skipping",
                format!(
                    "deploying HTTP API definition {}, not matched by component selection",
                    api_definition_name.as_str().log_color_highlight()
                ),
            );
            return Ok(None);
        };

        let server_diffable_api_definition = self
            .api_definition(
                project,
                api_definition_name.as_str(),
                api_definition.value.version.as_str(),
            )
            .await?
            .map(DiffableHttpApiDefinition::from_server)
            .transpose()?;
        let manifest_api_definition = DiffableHttpApiDefinition::from_manifest(
            server_diffable_api_definition.as_ref(),
            api_definition_name,
            &api_definition.value,
            latest_component_versions,
        )?;

        match server_diffable_api_definition {
            Some(server_diffable_api_definition) => {
                if server_diffable_api_definition != manifest_api_definition {
                    log_warn_action(
                        "Found",
                        format!(
                            "changes in HTTP API definition {}@{}",
                            api_definition_name.as_str().log_color_highlight(),
                            manifest_api_definition
                                .0
                                .version
                                .as_str()
                                .log_color_highlight()
                        ),
                    );

                    {
                        let _indent = self.ctx.log_handler().nested_text_view_indent();
                        log_deploy_diff(&server_diffable_api_definition, &manifest_api_definition)?;
                    }

                    if server_diffable_api_definition.0.draft {
                        log_action(
                            "Updating",
                            format!(
                                "HTTP API definition {}",
                                api_definition_name.as_str().log_color_highlight()
                            ),
                        );

                        let result = self
                            .update_api_definition(project, &manifest_api_definition.0)
                            .await?;

                        self.ctx
                            .log_handler()
                            .log_view(&ApiDefinitionUpdateView(result));

                        Ok(Some(manifest_api_definition.0.version))
                    } else {
                        log_warn(
                            "The current version of the HTTP API is already deployed as non-draft.",
                        );

                        if update_or_redeploy.redeploy_http_api(self.ctx.update_or_redeploy()) {
                            self.ctx
                                .api_deployment_handler()
                                .undeploy_api_from_all_sites_for_redeploy(
                                    project,
                                    api_definition_name,
                                )
                                .await?;

                            log_action(
                                "Redeploying",
                                format!(
                                    "new HTTP API definition version {}@{}",
                                    api_definition_name.as_str().log_color_highlight(),
                                    manifest_api_definition
                                        .0
                                        .version
                                        .as_str()
                                        .log_color_highlight()
                                ),
                            );

                            let result = self
                                .update_api_definition(project, &manifest_api_definition.0)
                                .await?;

                            self.ctx
                                .log_handler()
                                .log_view(&ApiDefinitionNewView(result));

                            Ok(Some(manifest_api_definition.0.version))
                        } else {
                            match self
                                .ctx
                                .interactive_handler()
                                .select_new_api_definition_version(&manifest_api_definition.0)?
                            {
                                Some(new_version) => {
                                    let new_draft = true;
                                    let old_version = manifest_api_definition.0.version.clone();

                                    let manifest_api_definition = {
                                        let mut manifest_api_definition = manifest_api_definition;
                                        manifest_api_definition.0.version = new_version;
                                        manifest_api_definition.0.draft = new_draft;
                                        manifest_api_definition
                                    };

                                    {
                                        let app_ctx = self.ctx.app_context_lock().await;
                                        let app_ctx = app_ctx.some_or_err()?;

                                        let mut editor = AppYamlEditor::new(&app_ctx.application);
                                        editor.update_api_definition_version(
                                            api_definition_name,
                                            &manifest_api_definition.0.version,
                                        )?;
                                        editor.update_documents()?;
                                    }

                                    log_action(
                                        "Creating",
                                        format!(
                                            "new HTTP API definition version for {}, with version updated from {} to {}",
                                            api_definition_name.as_str().log_color_highlight(),
                                            old_version.log_color_highlight(),
                                            manifest_api_definition
                                                .0.version
                                                .as_str()
                                                .log_color_highlight()
                                        ),
                                    );

                                    let result = self
                                        .new_api_definition(project, &manifest_api_definition.0)
                                        .await?;

                                    self.ctx
                                        .log_handler()
                                        .log_view(&ApiDefinitionNewView(result));

                                    Ok(Some(manifest_api_definition.0.version))
                                }
                                None => {
                                    log_error(format!(
                                        "Please specify a new version for {} in {}",
                                        api_definition_name.as_str().log_color_highlight(),
                                        api_definition.source.log_color_highlight()
                                    ));
                                    bail!(NonSuccessfulExit)
                                }
                            }
                        }
                    }
                } else {
                    log_skipping_up_to_date(format!(
                        "deploying HTTP API definition {}",
                        api_definition_name.as_str().log_color_highlight()
                    ));
                    Ok(Some(server_diffable_api_definition.0.version))
                }
            }
            None => {
                log_action(
                    "Creating",
                    format!(
                        "new HTTP API definition version {}@{}",
                        api_definition_name.as_str().log_color_highlight(),
                        manifest_api_definition
                            .0
                            .version
                            .as_str()
                            .log_color_highlight()
                    ),
                );

                let result = self
                    .new_api_definition(project, &manifest_api_definition.0)
                    .await?;

                self.ctx
                    .log_handler()
                    .log_view(&ApiDefinitionNewView(result));

                Ok(Some(manifest_api_definition.0.version))
            }
        }
    }

    pub async fn deploy_required_components(
        &self,
        project: Option<&ProjectRefAndId>,
        update_or_redeploy: &UpdateOrRedeployArgs,
        api_defs_filter: BTreeSet<HttpApiDefinitionName>,
    ) -> anyhow::Result<BTreeMap<String, Component>> {
        let used_component_names = {
            {
                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.some_or_err()?;

                if api_defs_filter.is_empty() {
                    app_ctx
                        .application
                        .used_component_names_for_all_http_api_definition()
                } else {
                    api_defs_filter
                        .iter()
                        .flat_map(|name| {
                            app_ctx
                                .application
                                .used_component_names_for_http_api_definition(name)
                        })
                        .collect()
                }
            }
            .into_iter()
            .map(|component_name| ComponentName::from(component_name.to_string()))
            .collect::<Vec<_>>()
        };

        if used_component_names.is_empty() {
            return Ok(BTreeMap::new());
        }

        log_action("Deploying", "required components");
        let _indent = LogIndent::new();

        let latest_components = self
            .ctx
            .component_handler()
            .deploy(
                project,
                used_component_names,
                None,
                &ApplicationComponentSelectMode::All,
                update_or_redeploy,
            )
            .await?
            .into_iter()
            .map(|component| (component.component_name.0.clone(), component))
            .collect::<BTreeMap<_, _>>();

        Ok(latest_components)
    }

    async fn api_definition(
        &self,
        project: Option<&ProjectRefAndId>,
        name: &str,
        version: &str,
    ) -> anyhow::Result<Option<HttpApiDefinitionResponseData>> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .api_definition
            .get_definition(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                name,
                version,
            )
            .await
            .map_service_error_not_found_as_opt()
    }

    async fn update_api_definition(
        &self,
        project: Option<&ProjectRefAndId>,
        manifest_api_definition: &HttpApiDefinitionRequest,
    ) -> anyhow::Result<HttpApiDefinitionResponseData> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .api_definition
            .update_definition_json(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                &manifest_api_definition.id,
                &manifest_api_definition.version,
                // TODO: would be nice to share the model between oss and cloud instead of "re-encoding"
                &parse_api_definition(&serde_yaml::to_string(&manifest_api_definition)?)?,
            )
            .await
            .map_service_error()
    }

    async fn new_api_definition(
        &self,
        project: Option<&ProjectRefAndId>,
        api_definition: &HttpApiDefinitionRequest,
    ) -> anyhow::Result<HttpApiDefinitionResponseData> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .api_definition
            .create_definition_json(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                // TODO: would be nice to share the model between oss and cloud instead of "re-encoding"
                &parse_api_definition(&serde_yaml::to_string(&api_definition)?)?,
            )
            .await
            .map_service_error()
    }
}

fn parse_api_definition<T: DeserializeOwned>(input: &str) -> anyhow::Result<T> {
    serde_yaml::from_str(input).context("Failed to parse API definition")
}
