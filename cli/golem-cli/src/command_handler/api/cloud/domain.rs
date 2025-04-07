// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
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
use crate::model::ProjectName;
use golem_cloud_client::api::ApiDomainClient;
use golem_cloud_client::model::DomainRequest;

use crate::log::log_warn_action;
use std::sync::Arc;

pub struct ApiCloudDomainCommandHandler {
    ctx: Arc<Context>,
}

impl ApiCloudDomainCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&mut self, command: ApiDomainSubcommand) -> anyhow::Result<()> {
        match command {
            ApiDomainSubcommand::Get { project_name } => self.cmd_get(project_name).await,
            ApiDomainSubcommand::New {
                project_name,
                domain_name,
            } => self.cmd_new(project_name, domain_name).await,
            ApiDomainSubcommand::Delete {
                project_name,
                domain_name,
            } => self.cmd_delete(project_name, domain_name).await,
        }
    }

    async fn cmd_get(&self, project_name: ProjectName) -> anyhow::Result<()> {
        let domains = self
            .ctx
            .golem_clients_cloud()
            .await?
            .api_domain
            .get_domains(
                &self
                    .ctx
                    .cloud_project_handler()
                    .select_project(None, &project_name)
                    .await?
                    .project_id
                    .0,
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&ApiDomainListView(domains));

        Ok(())
    }

    async fn cmd_new(&self, project_name: ProjectName, domain_name: String) -> anyhow::Result<()> {
        let domain = self
            .ctx
            .golem_clients_cloud()
            .await?
            .api_domain
            .create_or_update_domain(&DomainRequest {
                project_id: self
                    .ctx
                    .cloud_project_handler()
                    .select_project(None, &project_name)
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
        project_name: ProjectName,
        domain_name: String,
    ) -> anyhow::Result<()> {
        self.ctx
            .golem_clients_cloud()
            .await?
            .api_domain
            .delete_domain(
                &self
                    .ctx
                    .cloud_project_handler()
                    .select_project(None, &project_name)
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
