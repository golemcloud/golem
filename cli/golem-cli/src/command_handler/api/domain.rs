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

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::model::text::http_api_domain::{DomainRegistrationNewView, HttpApiDomainListView};

use crate::command::api::domain::ApiDomainSubcommand;
use crate::error::NonSuccessfulExit;
use crate::log::log_error;
use crate::log::{log_action, log_warn_action, logln, LogColorize};
use crate::model::environment::EnvironmentResolveMode;
use anyhow::bail;
use golem_client::api::ApiDomainClient;
use golem_client::model::{DomainRegistration, DomainRegistrationCreation};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use std::sync::Arc;

pub struct ApiDomainCommandHandler {
    ctx: Arc<Context>,
}

impl ApiDomainCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiDomainSubcommand) -> anyhow::Result<()> {
        match command {
            ApiDomainSubcommand::List => self.cmd_list().await,
            ApiDomainSubcommand::Register { domain } => self.cmd_register(domain).await,
            ApiDomainSubcommand::Delete { domain } => self.cmd_delete(domain).await,
        }
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let domains = self.list_domains(&environment.environment_id).await?;

        self.ctx
            .log_handler()
            .log_view(&HttpApiDomainListView(domains));

        Ok(())
    }

    async fn cmd_register(&self, domain: String) -> anyhow::Result<()> {
        let domain = Domain(domain);

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        log_action(
            "Creating",
            format!(
                "domain registration {} for {}",
                domain.0.log_color_highlight(),
                environment.text_format(),
            ),
        );

        let domain = self
            .ctx
            .golem_clients()
            .await?
            .api_domain
            .create_domain_registration(
                &environment.environment_id.0,
                &DomainRegistrationCreation {
                    domain: domain.clone(),
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&DomainRegistrationNewView(domain));

        Ok(())
    }

    async fn cmd_delete(&self, domain: String) -> anyhow::Result<()> {
        let domain = Domain(domain);

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let domains = self.list_domains(&environment.environment_id).await?;

        let Some(domain_to_delete) = domains.iter().find(|d| d.domain == domain) else {
            log_error(format!(
                "Domain {} not found",
                domain.0.log_color_highlight()
            ));
            logln("");

            if domains.is_empty() {
                logln(format!(
                    "No domains are registered yet for {}",
                    environment.text_format()
                ));
            } else {
                logln(
                    format!(
                        "Currently registered domains for {}:",
                        environment.text_format()
                    )
                    .log_color_help_group()
                    .to_string(),
                );
                for domain in domains {
                    logln(format!("- {}", domain.domain.0))
                }
            }

            bail!(NonSuccessfulExit);
        };

        clients
            .api_domain
            .delete_domain_registrations(&domain_to_delete.id.0)
            .await
            .map_service_error()?;

        log_warn_action(
            "Deleting",
            format!(
                "domain registration {} from {}",
                domain.0.log_color_highlight(),
                environment.text_format()
            ),
        );
        log_action("Deleted", "domain {}");

        Ok(())
    }

    pub async fn register_missing_domain(
        &self,
        environment_id: &EnvironmentId,
        domain: &Domain,
    ) -> anyhow::Result<()> {
        if !self
            .ctx
            .interactive_handler()
            .confirm_register_missing_domain(domain)?
        {
            bail!(NonSuccessfulExit);
        }

        let _ = self.register(environment_id, domain).await;

        Ok(())
    }

    async fn register(
        &self,
        environment_id: &EnvironmentId,
        domain: &Domain,
    ) -> anyhow::Result<DomainRegistration> {
        self.ctx
            .golem_clients()
            .await?
            .api_domain
            .create_domain_registration(
                &environment_id.0,
                &DomainRegistrationCreation {
                    domain: domain.clone(),
                },
            )
            .await
            .map_service_error()
    }

    async fn list_domains(
        &self,
        environment_id: &EnvironmentId,
    ) -> anyhow::Result<Vec<DomainRegistration>> {
        Ok(self
            .ctx
            .golem_clients()
            .await?
            .api_domain
            .list_environment_domain_registrations(&environment_id.0)
            .await
            .map_service_error()?
            .values)
    }
}
