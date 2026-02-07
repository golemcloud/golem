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

use crate::command::api::security_scheme::ApiSecuritySchemeSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::log_error;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::text::http_api_security::{
    HttpSecuritySchemeCreateView, HttpSecuritySchemeGetView,
};
use anyhow::bail;
use golem_client::api::ApiSecurityClient;
use golem_client::model::SecuritySchemeCreation;
use golem_common::model::security_scheme::{Provider, SecuritySchemeName};
use std::sync::Arc;

pub struct ApiSecuritySchemeCommandHandler {
    ctx: Arc<Context>,
}

impl ApiSecuritySchemeCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiSecuritySchemeSubcommand) -> anyhow::Result<()> {
        match command {
            ApiSecuritySchemeSubcommand::Create {
                security_scheme_name,
                provider_type,
                client_id,
                client_secret,
                scope,
                redirect_url,
            } => {
                self.cmd_create(
                    security_scheme_name,
                    provider_type,
                    client_id,
                    client_secret,
                    scope,
                    redirect_url,
                )
                .await
            }
            ApiSecuritySchemeSubcommand::Get {
                security_scheme_name,
            } => self.cmd_get(security_scheme_name).await,
            ApiSecuritySchemeSubcommand::List => self.cmd_list().await,
        }
    }

    async fn cmd_create(
        &self,
        security_scheme_name: SecuritySchemeName,
        provider_type: Provider,
        client_id: String,
        client_secret: String,
        scopes: Vec<String>,
        redirect_url: String,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .api_security
            .create_security_scheme(
                &environment.environment_id.0,
                &SecuritySchemeCreation {
                    name: security_scheme_name,
                    provider_type,
                    client_id,
                    client_secret,
                    redirect_url,
                    scopes,
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&HttpSecuritySchemeCreateView(result));

        Ok(())
    }

    async fn cmd_get(&self, security_scheme_name: SecuritySchemeName) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        // TODO: atomic: missing client method to get by name
        let Some(result) = clients
            .api_security
            .get_environment_security_schemes(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values
            .into_iter()
            .find(|s| s.name == security_scheme_name)
        else {
            log_error(format!(
                "HTTP API Security Scheme {} not found.",
                security_scheme_name.0
            ));
            bail!(NonSuccessfulExit);
        };

        self.ctx
            .log_handler()
            .log_view(&HttpSecuritySchemeGetView(result));

        Ok(())
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let results = clients
            .api_security
            .get_environment_security_schemes(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values;

        self.ctx.log_handler().log_view(&results);

        Ok(())
    }
}
