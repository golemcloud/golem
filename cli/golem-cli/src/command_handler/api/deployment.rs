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
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};
use crate::model::environment::{EnvironmentResolveMode, ResolvedEnvironmentIdentity};
use crate::model::text::http_api_deployment::HttpApiDeploymentGetView;
use anyhow::{anyhow, bail};
use golem_client::api::ApiDeploymentClient;
use golem_client::model::{HttpApiDeploymentCreation, HttpApiDeploymentUpdate};
use golem_common::cache::SimpleCache;
use golem_common::model::deployment::DeploymentPlanHttpApiDeploymentEntry;
use golem_common::model::diff;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::http_api_definition::HttpApiDefinitionName;
use golem_common::model::http_api_deployment::{
    HttpApiDeployment, HttpApiDeploymentId, HttpApiDeploymentRevision,
};
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
            ApiDeploymentSubcommand::Get { domain } => self.cmd_get(Domain(domain)).await,
            ApiDeploymentSubcommand::List => self.cmd_list().await,
        }
    }

    async fn cmd_get(&self, domain: Domain) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let Some(result) = self
            .resolve_http_api_deployment(&environment, &domain, None)
            .await?
        else {
            bail!("Not found!");
        };

        self.ctx
            .log_handler()
            .log_view(&HttpApiDeploymentGetView(result));

        Ok(())
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .api_deployment
            .list_http_api_deployments_in_environment(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values;

        self.ctx.log_handler().log_view(&result);

        Ok(())
    }

    async fn resolve_http_api_deployment(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        domain: &Domain,
        revision: Option<&HttpApiDeploymentRevision>,
    ) -> anyhow::Result<Option<HttpApiDeployment>> {
        let clients = self.ctx.golem_clients().await?;

        let Some(deployment) = clients
            .api_deployment
            .get_http_api_deployment_in_environment(&environment.environment_id.0, &domain.0)
            .await
            .map_service_error_not_found_as_opt()?
        else {
            return Ok(None);
        };

        let Some(revision) = revision else {
            return Ok(Some(deployment));
        };

        clients
            .api_deployment
            .get_http_api_deployment_revision(&deployment.id.0, revision.0)
            .await
            .map_service_error_not_found_as_opt()
    }

    pub async fn deployable_manifest_api_deployments(
        &self,
        environment_name: &EnvironmentName,
    ) -> anyhow::Result<BTreeMap<Domain, Vec<HttpApiDefinitionName>>> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        Ok(app_ctx
            .application
            .http_api_deployments(environment_name)
            .map(|deployments| {
                deployments
                    .iter()
                    .map(|(name, sources)| {
                        (
                            name.clone(),
                            sources
                                .iter()
                                .flat_map(|source_and_value| source_and_value.value.iter())
                                .cloned()
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default())
    }

    pub async fn get_http_api_deployment_revision_by_id(
        &self,
        http_api_deployment_id: &HttpApiDeploymentId,
        revision: HttpApiDeploymentRevision,
    ) -> anyhow::Result<HttpApiDeployment> {
        self.ctx
            .caches()
            .http_api_deployment_revision
            .get_or_insert_simple(&(*http_api_deployment_id, revision), {
                let ctx = self.ctx.clone();
                async move || {
                    ctx.golem_clients()
                        .await?
                        .api_deployment
                        .get_http_api_deployment_revision(&http_api_deployment_id.0, revision.0)
                        .await
                        .map_service_error()
                        .map_err(Arc::new)
                }
            })
            .await
            .map_err(|err| anyhow!(err))
    }

    pub async fn create_staged_http_api_deployment(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        domain: &Domain,
        deployable_http_api_deployment: &[HttpApiDefinitionName],
    ) -> anyhow::Result<()> {
        log_action(
            "Creating",
            format!("HTTP API deployment {}", domain.0.log_color_highlight()),
        );
        let _indent = LogIndent::new();

        self.ctx
            .golem_clients()
            .await?
            .api_deployment
            .create_http_api_deployment(
                &environment.environment_id.0,
                &HttpApiDeploymentCreation {
                    domain: domain.clone(),
                    api_definitions: deployable_http_api_deployment.to_vec(),
                },
            )
            .await
            .map_service_error()?;

        Ok(())
    }

    pub async fn delete_staged_http_api_deployment(
        &self,
        http_api_deployment: &DeploymentPlanHttpApiDeploymentEntry,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Deleting",
            format!(
                "HTTP API deployment {}",
                http_api_deployment.domain.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        self.ctx
            .golem_clients()
            .await?
            .api_deployment
            .delete_http_api_deployment(&http_api_deployment.id.0, http_api_deployment.revision.0)
            .await
            .map_service_error()?;

        Ok(())
    }

    pub async fn update_staged_http_api_deployment(
        &self,
        http_api_deployment: &DeploymentPlanHttpApiDeploymentEntry,
        deployable_http_api_deployment: &[HttpApiDefinitionName],
        _diff: &diff::DiffForHashOf<diff::HttpApiDeployment>,
    ) -> anyhow::Result<()> {
        log_action(
            "Updating",
            format!(
                "HTTP API deployment {}",
                http_api_deployment.domain.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        self.ctx
            .golem_clients()
            .await?
            .api_deployment
            .update_http_api_deployment(
                &http_api_deployment.id.0,
                &HttpApiDeploymentUpdate {
                    current_revision: http_api_deployment.revision,
                    api_definitions: Some(deployable_http_api_deployment.to_vec()),
                },
            )
            .await
            .map_service_error()?;

        Ok(())
    }
}
