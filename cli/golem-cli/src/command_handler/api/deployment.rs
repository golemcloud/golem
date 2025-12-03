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
use crate::model::environment::{EnvironmentResolveMode, ResolvedEnvironmentIdentity};
use crate::model::text::http_api_deployment::HttpApiDeploymentGetView;
use anyhow::bail;
use golem_client::api::ApiDeploymentClient;
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_deployment::{HttpApiDeployment, HttpApiDeploymentRevision};
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

        let Some(_revision) = revision else {
            return Ok(Some(deployment));
        };

        // TODO: atomic: missing client method
        todo!()
    }

    // TODO: atomic:
    /*
    async fn manifest_api_deployments(
        &self,
        environment: &ResolvedEnvironmentIdentity,
    ) -> anyhow::Result<BTreeMap<Domain, Vec<WithSource<Vec<HttpApiDefinitionName>>>>> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        Ok(app_ctx
            .application
            .http_api_deployments(&environment.environment_name)
            .cloned()
            .unwrap_or_default())
    }
    */
}
