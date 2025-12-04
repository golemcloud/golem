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
use crate::log::log_warn_action;
use crate::model::environment::EnvironmentResolveMode;
use golem_client::api::ApiDomainClient;
use golem_client::model::DomainRegistrationCreation;
use golem_common::model::domain_registration::Domain;
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

        let domains = self
            .ctx
            .golem_clients()
            .await?
            .api_domain
            .list_environment_domain_registrations(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values;

        self.ctx
            .log_handler()
            .log_view(&HttpApiDomainListView(domains));

        Ok(())
    }

    async fn cmd_register(&self, domain: String) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let domain = self
            .ctx
            .golem_clients()
            .await?
            .api_domain
            .create_domain_registration(
                &environment.environment_id.0,
                &DomainRegistrationCreation {
                    domain: Domain(domain),
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&DomainRegistrationNewView(domain));

        Ok(())
    }

    async fn cmd_delete(&self, _domain: String) -> anyhow::Result<()> {
        let _environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        // TODO: atomic: missing client: get domain registration by domain?

        log_warn_action("Deleted", "domain");

        Ok(())
    }
}
