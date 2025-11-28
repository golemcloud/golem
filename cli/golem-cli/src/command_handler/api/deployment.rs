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

use crate::command::api::deployment::ApiDeploymentSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};
use crate::model::api::{ApiDefinitionId, ApiDeployment};
use crate::model::app::{HttpApiDefinitionName, HttpApiDeploymentSite, WithSource};
use crate::model::app_raw::HttpApiDeployment;
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::text::fmt::{log_error, log_warn};
use anyhow::bail;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ApiDeploymentCommandHandler {
    ctx: Arc<Context>,
}

impl ApiDeploymentCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiDeploymentSubcommand) -> anyhow::Result<()> {
        match command {
            ApiDeploymentSubcommand::Get { project, site } => self.cmd_get(project, site).await,
            ApiDeploymentSubcommand::List {
                project,
                definition,
            } => self.cmd_list(project, definition).await,
        }
    }

    async fn cmd_get(&self, project: ProjectOptionalFlagArg, site: String) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        let Some(result) = self.api_deployment(project.as_ref(), &site).await? else {
            bail!("Not found!");
        };

        self.ctx.log_handler().log_view(&result);

        Ok(())
    }

    async fn cmd_list(
        &self,
        project: ProjectOptionalFlagArg,
        definition: Option<ApiDefinitionId>,
    ) -> anyhow::Result<()> {
        let id = definition.as_ref().map(|id| id.0.as_str());

        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let result: Vec<ApiDeployment> = clients
            .api_deployment
            .list_deployments(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project.as_ref())
                    .await?
                    .0,
                id,
            )
            .await
            .map_service_error()?
            .into_iter()
            .map(ApiDeployment::from)
            .collect::<Vec<_>>();

        self.ctx.log_handler().log_view(&result);

        Ok(())
    }

    async fn api_deployment(
        &self,
        environment: Option<&ResolvedEnvironmentIdentity>,
        site: &str,
    ) -> anyhow::Result<Option<ApiDeployment>> {
        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .api_deployment
            .get_deployment(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                site,
            )
            .await
            .map_service_error_not_found_as_opt()?
            .map(ApiDeployment::from);

        Ok(result)
    }

    async fn create_or_update_api_deployment(
        &self,
        environment: Option<&ResolvedEnvironmentIdentity>,
        site: &HttpApiDeploymentSite,
        api_deployment: &DiffableHttpApiDeployment,
    ) -> anyhow::Result<ApiDeployment> {
        let clients = self.ctx.golem_clients().await?;

        let result: ApiDeployment = clients
            .api_deployment
            .deploy(&ApiDeploymentRequestCloud {
                project_id: self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                api_definitions: api_deployment
                    .definitions()
                    .map(|(name, version)| ApiDefinitionInfoCloud {
                        id: name.to_string(),
                        version: version.to_string(),
                    })
                    .collect::<Vec<_>>(),
                site: ApiSiteCloud {
                    host: site.host.clone(),
                    subdomain: site.subdomain.clone(),
                },
            })
            .await
            .map_service_error()?
            .into();

        Ok(result)
    }

    async fn undeploy_api_definition(
        &self,
        environment: Option<&ResolvedEnvironmentIdentity>,
        site: &HttpApiDeploymentSite,
        id: &str,
        version: &str,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .api_deployment
            .undeploy_api(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                &site.to_string(),
                id,
                version,
            )
            .await
            .map_service_error()
            .map(|_| ())
    }

    pub async fn undeploy_api_from_all_sites_for_redeploy(
        &self,
        environment: Option<&ResolvedEnvironmentIdentity>,
        api_definition_name: &HttpApiDefinitionName,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let targets: Vec<(HttpApiDeploymentSite, String)> = clients
            .api_deployment
            .list_deployments(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                Some(api_definition_name.as_str()),
            )
            .await
            .map_service_error()?
            .into_iter()
            .filter_map(|dep| {
                dep.api_definitions
                    .into_iter()
                    .find_map(|def| (def.id == api_definition_name.as_str()).then_some(def.version))
                    .map(|version| {
                        (
                            HttpApiDeploymentSite {
                                host: dep.site.host,
                                subdomain: dep.site.subdomain,
                            },
                            version,
                        )
                    })
            })
            .collect();

        if targets.is_empty() {
            log_warn(format!(
                "No deployments found using HTTP API: {}",
                api_definition_name.as_str().log_color_highlight()
            ));
            return Ok(());
        }

        if !self
            .ctx
            .interactive_handler()
            .confirm_undeploy_api_from_sites_for_redeploy(
                api_definition_name.as_str(),
                targets.as_slice(),
            )?
        {
            bail!(NonSuccessfulExit)
        }

        log_warn_action("Undeploying", "HTTP API {} for redeploy");
        let _indent = LogIndent::new();
        for (site, version) in targets {
            log_warn_action(
                "Undeploying",
                format!(
                    "HTTP API definition {}@{} from {} for redeploy",
                    api_definition_name.as_str().log_color_highlight(),
                    version.log_color_highlight(),
                    site.to_string().log_color_highlight()
                ),
            );
            self.undeploy_api_definition(project, &site, api_definition_name.as_str(), &version)
                .await?;
            log_action(
                "Undeployed",
                format!(
                    "HTTP API definition {}@{} from {}",
                    api_definition_name.as_str().log_color_highlight(),
                    version.log_color_highlight(),
                    site.to_string().log_color_highlight()
                ),
            );
        }

        Ok(())
    }

    async fn manifest_api_deployments(
        &self,
    ) -> anyhow::Result<BTreeMap<HttpApiDeploymentSite, Vec<WithSource<Vec<HttpApiDefinitionName>>>>>
    {
        let profile = self.ctx.profile_name().clone();

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        Ok(app_ctx
            .application
            .http_api_deployments(&profile)
            .cloned()
            .unwrap_or_default())
    }

    async fn merge_manifest_api_deployments(
        &self,
    ) -> anyhow::Result<BTreeMap<HttpApiDeploymentSite, HttpApiDeployment>> {
        Ok(self
            .manifest_api_deployments()
            .await?
            .into_iter()
            .map(|(site, definitions)| {
                (
                    site.clone(),
                    HttpApiDeployment {
                        host: site.host.clone(),
                        subdomain: site.subdomain.clone(),
                        definitions: definitions
                            .into_iter()
                            .flat_map(|d| d.value.into_iter().map(|d| d.into_string()))
                            .collect(),
                    },
                )
            })
            .collect())
    }
}
