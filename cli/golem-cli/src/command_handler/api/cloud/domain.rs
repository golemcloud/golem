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

use crate::command::api::cloud::domain::ApiDomainSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::model::text::api_domain::{ApiDomainListView, ApiDomainNewView};
use crate::model::ProjectReference;
use golem_client::api::ApiDomainClient;
use golem_client::model::DomainRequest;

use crate::log::log_warn_action;
use std::sync::Arc;

pub struct ApiCloudDomainCommandHandler {
    ctx: Arc<Context>,
}

impl ApiCloudDomainCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiDomainSubcommand) -> anyhow::Result<()> {
        match command {
            ApiDomainSubcommand::Get { project } => self.cmd_get(project.project).await,
            ApiDomainSubcommand::New {
                project,
                domain_name,
            } => self.cmd_new(project.project, domain_name).await,
            ApiDomainSubcommand::Delete {
                project,
                domain_name,
            } => self.cmd_delete(project.project, domain_name).await,
        }
    }

    async fn cmd_get(&self, project_reference: ProjectReference) -> anyhow::Result<()> {
        let domains = self
            .ctx
            .golem_clients()
            .await?
            .api_domain
            .get_domains(
                &self
                    .ctx
                    .cloud_project_handler()
                    .select_project(&project_reference)
                    .await?
                    .project_id
                    .0,
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&ApiDomainListView(domains));

        Ok(())
    }

    async fn cmd_new(
        &self,
        project_reference: ProjectReference,
        domain_name: String,
    ) -> anyhow::Result<()> {
        let domain = self
            .ctx
            .golem_clients()
            .await?
            .api_domain
            .create_or_update_domain(&DomainRequest {
                project_id: self
                    .ctx
                    .cloud_project_handler()
                    .select_project(&project_reference)
                    .await?
                    .project_id
                    .0,
                domain_name,
            })
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&ApiDomainNewView(domain));

        Ok(())
    }

    async fn cmd_delete(
        &self,
        project_reference: ProjectReference,
        domain_name: String,
    ) -> anyhow::Result<()> {
        self.ctx
            .golem_clients()
            .await?
            .api_domain
            .delete_domain(
                &self
                    .ctx
                    .cloud_project_handler()
                    .select_project(&project_reference)
                    .await?
                    .project_id
                    .0,
                &domain_name,
            )
            .await
            .map_service_error()?;

        log_warn_action("Deleted", "domain");

        Ok(())
    }
}
