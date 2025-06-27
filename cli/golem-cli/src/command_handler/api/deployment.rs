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
use crate::model::ProjectRefAndId;
use anyhow::bail;
use golem_client::api::ApiDeploymentClient;
use golem_client::model::{
    ApiDefinitionInfo as ApiDefinitionInfoCloud, ApiDeploymentRequest as ApiDeploymentRequestCloud,
    ApiSite as ApiSiteCloud,
};
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
            ApiDeploymentSubcommand::Deploy {
                host_or_site,
                update_or_redeploy,
            } => self.cmd_deploy(host_or_site, update_or_redeploy).await,
            ApiDeploymentSubcommand::Get { project, site } => self.cmd_get(project, site).await,
            ApiDeploymentSubcommand::List {
                project,
                definition,
            } => self.cmd_list(project, definition).await,
            ApiDeploymentSubcommand::Delete { project, site } => {
                self.cmd_delete(project, site).await
            }
        }
    }

    async fn cmd_deploy(
        &self,
        host_or_site: Option<String>,
        update_or_redeploy: UpdateOrRedeployArgs,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(None)
            .await?;

        let api_deployments = self.manifest_api_deployments().await?;
        let api_deployments = match &host_or_site {
            Some(host_or_site) => api_deployments
                .into_iter()
                .filter(|(site, _)| {
                    &site.host == host_or_site || site.subdomain.as_ref() == Some(host_or_site)
                })
                .collect::<BTreeMap<_, _>>(),
            None => api_deployments,
        };

        if api_deployments.is_empty() {
            if host_or_site.is_some() {
                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.some_or_err()?;

                logln("");
                log_error("No matching HTTP API deployment found");
                logln("");
                app_ctx.log_dynamic_help(&DynamicHelpSections::show_api_definitions())?;
                bail!(NonSuccessfulExit)
            } else {
                log_warn_action("Skipping", "deploying, no deployments are defined");
                return Ok(());
            }
        }

        let latest_api_definition_versions = self
            .deploy_required_api_definitions(
                project.as_ref(),
                &update_or_redeploy,
                api_deployments.values().map(|dep| &dep.value),
            )
            .await?;

        let _indent: Option<LogIndent> = (api_deployments.len() > 1).then(|| {
            log_action("Deploying", "matching HTTP API deployments");
            LogIndent::new()
        });

        for (site, deployment) in &api_deployments {
            self.deploy_api_deployment(
                project.as_ref(),
                HttpApiDeployMode::Matching,
                &latest_api_definition_versions,
                site,
                deployment,
            )
            .await?;
        }

        Ok(())
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

    async fn cmd_delete(
        &self,
        project: ProjectOptionalFlagArg,
        site: String,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        let clients = self.ctx.golem_clients().await?;

        clients
            .api_deployment
            .delete_deployment(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project.as_ref())
                    .await?
                    .0,
                &site,
            )
            .await
            .map(|_| ())
            .map_service_error()?;

        log_warn_action("Deleted", format!("site {}", site.log_color_highlight()));

        Ok(())
    }

    pub async fn deploy(
        &self,
        project: Option<&ProjectRefAndId>,
        deploy_mode: HttpApiDeployMode,
        latest_api_definition_versions: &BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        let api_deployments = self.manifest_api_deployments().await?;

        if !api_deployments.is_empty() {
            log_action("Deploying", "HTTP API deployments");

            for (site, api_deployment) in api_deployments {
                let _indent = LogIndent::new();
                self.deploy_api_deployment(
                    project,
                    deploy_mode,
                    latest_api_definition_versions,
                    &site,
                    &api_deployment,
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn deploy_api_deployment(
        &self,
        project: Option<&ProjectRefAndId>,
        deploy_mode: HttpApiDeployMode,
        latest_api_definition_versions: &BTreeMap<String, String>,
        site: &HttpApiDeploymentSite,
        api_definition: &WithSource<HttpApiDeployment>,
    ) -> anyhow::Result<()> {
        let site_as_str = site.to_string();

        let skip_by_api_def_filter = match deploy_mode {
            HttpApiDeployMode::All => false,
            HttpApiDeployMode::Matching => !api_definition
                .value
                .definitions
                .iter()
                .any(|api_def| latest_api_definition_versions.contains_key(api_def)),
        };

        if skip_by_api_def_filter {
            log_warn_action(
                "Skipping",
                format!(
                    "deploying HTTP API deployment {}, not matched by definition selection",
                    site.to_string().log_color_highlight()
                ),
            );
            return Ok(());
        }

        let server_diffable_api_deployment = self
            .api_deployment(project, &site_as_str)
            .await?
            .map(DiffableHttpApiDeployment::from_server)
            .transpose()?;
        let manifest_diffable_api_deployment = DiffableHttpApiDeployment::from_manifest(
            &api_definition.value,
            latest_api_definition_versions,
        )?;

        match server_diffable_api_deployment {
            Some(server_diffable_api_deployment) => {
                if server_diffable_api_deployment != manifest_diffable_api_deployment {
                    log_warn_action(
                        "Found",
                        format!(
                            "changes in HTTP API deployment {}",
                            site.to_string().log_color_highlight()
                        ),
                    );

                    {
                        let _indent = self.ctx.log_handler().nested_text_view_indent();
                        log_deploy_diff(
                            &server_diffable_api_deployment,
                            &manifest_diffable_api_deployment,
                        )?;
                    }

                    let plan =
                        server_diffable_api_deployment.plan(&manifest_diffable_api_deployment);

                    if !plan.delete.is_empty() {
                        let steps = {
                            plan.delete
                                .iter()
                                .map(|(name, version)| {
                                    format!("{} {}@{}", "Undeploy".log_color_warn(), name, version)
                                })
                                .chain(
                                    plan.add
                                        .iter()
                                        .map(|(name, version)| format!("Deploy {name}@{version}")),
                                )
                                .collect::<Vec<_>>()
                        };

                        if !self
                            .ctx
                            .interactive_handler()
                            .confirm_deployment_installation_changes(&site_as_str, &steps)?
                        {
                            bail!(NonSuccessfulExit);
                        }
                    }

                    if !plan.delete.is_empty() {
                        for (name, version) in plan.delete {
                            log_warn_action(
                                "Undeploying",
                                format!(
                                    "HTTP API definition {}@{} from {}",
                                    name.log_color_highlight(),
                                    version.log_color_highlight(),
                                    site_as_str.log_color_highlight()
                                ),
                            );
                            self.undeploy_api_definition(project, site, &name, &version)
                                .await?;
                            log_action(
                                "Undeployed",
                                format!(
                                    "HTTP API definition {}@{} from {}",
                                    name.log_color_highlight(),
                                    version.log_color_highlight(),
                                    site_as_str.log_color_highlight()
                                ),
                            );
                        }
                    }

                    log_action(
                        "Updating",
                        format!(
                            "HTTP API deployment {}",
                            site.to_string().log_color_highlight()
                        ),
                    );

                    let _indent = LogIndent::new();
                    let result: ApiDeployment = self
                        .create_or_update_api_deployment(
                            project,
                            site,
                            &manifest_diffable_api_deployment,
                        )
                        .await?;

                    self.ctx.log_handler().log_view(&result);
                } else {
                    log_skipping_up_to_date(format!(
                        "deploying HTTP API deployment {}",
                        site.to_string().log_color_highlight()
                    ));
                }
            }
            None => {
                log_action(
                    "Creating",
                    format!(
                        "new HTTP API deployment for {}",
                        site.to_string().log_color_highlight()
                    ),
                );

                let result: ApiDeployment = self
                    .create_or_update_api_deployment(
                        project,
                        site,
                        &manifest_diffable_api_deployment,
                    )
                    .await?;

                self.ctx.log_handler().log_view(&result);
            }
        }

        Ok(())
    }

    async fn deploy_required_api_definitions<'a, I: Iterator<Item = &'a HttpApiDeployment>>(
        &self,
        project: Option<&ProjectRefAndId>,
        update_or_redeploy: &UpdateOrRedeployArgs,
        api_deployments: I,
    ) -> anyhow::Result<BTreeMap<String, String>> {
        let used_definition_names = api_deployments
            .flat_map(|deployment| deployment.definitions.clone())
            .map(|name| name.as_str().split('@').next().unwrap().into())
            .collect::<BTreeSet<HttpApiDefinitionName>>();

        if used_definition_names.is_empty() {
            return Ok(BTreeMap::new());
        }

        let used_definitions = {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;
            app_ctx
                .application
                .http_api_definitions()
                .iter()
                .filter(|(name, _)| used_definition_names.contains(name))
                .map(|(name, version)| (name.clone(), version.clone()))
                .collect::<BTreeMap<_, _>>()
        };

        let latest_components = self
            .ctx
            .api_definition_handler()
            .deploy_required_components(project, update_or_redeploy, used_definition_names)
            .await?;

        log_action("Deploying", "required HTTP API definitions");
        let _indent = LogIndent::new();

        let mut latest_api_definition_versions = BTreeMap::new();
        for (name, definition) in &used_definitions {
            let version = self
                .ctx
                .api_definition_handler()
                .deploy_api_definition(
                    project,
                    HttpApiDeployMode::Matching,
                    update_or_redeploy,
                    &latest_components,
                    name,
                    definition,
                )
                .await?;
            if let Some(version) = version {
                latest_api_definition_versions.insert(name.to_string(), version);
            }
        }

        Ok(latest_api_definition_versions)
    }

    async fn api_deployment(
        &self,
        project: Option<&ProjectRefAndId>,
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
        project: Option<&ProjectRefAndId>,
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
        project: Option<&ProjectRefAndId>,
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
        project: Option<&ProjectRefAndId>,
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
