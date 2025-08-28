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
use crate::command::shared_args::{ProjectOptionalFlagArg, UpdateOrRedeployArgs};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::{
    log_action, log_skipping_up_to_date, log_warn_action, logln, LogColorize, LogIndent,
};
use crate::model::api::{ApiDefinitionId, ApiDeployment, HttpApiDeployMode};
use crate::model::app::{
    DynamicHelpSections, HttpApiDefinitionName, HttpApiDeploymentSite, WithSource,
};
use crate::model::app_raw::HttpApiDeployment;
use crate::model::deploy_diff::api_deployment::DiffableHttpApiDeployment;
use crate::model::text::fmt::{log_deploy_diff, log_error, log_warn};
use anyhow::bail;
use std::collections::{BTreeMap, BTreeSet};
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

        let result: Vec<ApiDeployment> = match id {
            Some(id) => clients
                .api_deployment
                .list_deployments(
                    &self
                        .ctx
                        .cloud_project_handler()
                        .selected_project_id_or_default(project.as_ref())
                        .await?
                        .0,
                    Some(id),
                )
                .await
                .map_service_error()?
                .into_iter()
                .map(ApiDeployment::from)
                .collect::<Vec<_>>(),
            None => {
                // TODO: update in cloud to allow listing without id
                log_error("API definition ID for Cloud is required");
                bail!(NonSuccessfulExit);
            }
        };

        self.ctx.log_handler().log_view(&result);

        Ok(())
    }

    async fn api_deployment(
        &self,
        app_id: Option<&AppIdentity>,
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
        app_id: Option<&AppIdentity>,
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
        app_id: Option<&AppIdentity>,
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
        app_id: Option<&AppIdentity>,
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
    ) -> anyhow::Result<BTreeMap<HttpApiDeploymentSite, WithSource<HttpApiDeployment>>> {
        let profile = self.ctx.profile_name().clone();

        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        Ok(app_ctx
            .application
            .http_api_deployments(&profile)
            .cloned()
            .unwrap_or_default())
    }
}
