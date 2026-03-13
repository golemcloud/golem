// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::command::api::agent_secret::AgentSecretSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::log_error;
use crate::model::environment::{EnvironmentReference, EnvironmentResolveMode};
use crate::model::text::agent_secret::AgentSecretCreateView;
use anyhow::bail;
use golem_client::api::AgentSecretsClient;
use golem_client::model::AgentSecretUpdate;
use golem_common::model::agent_secret::{
    AgentSecretCreation, AgentSecretId, AgentSecretPath, AgentSecretRevision,
};
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_wasm::analysis::AnalysedType;
use std::sync::Arc;

pub struct AgentSecretCommandHandler {
    ctx: Arc<Context>,
}

impl AgentSecretCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: AgentSecretSubcommand) -> anyhow::Result<()> {
        match command {
            AgentSecretSubcommand::Create {
                environment,
                path,
                secret_type,
                secret_value,
            } => {
                self.cmd_create(environment, path, secret_type, secret_value)
                    .await
            }
            AgentSecretSubcommand::UpdateValue {
                id,
                current_revision,
                secret_value,
            } => {
                self.cmd_update_value(id, current_revision, secret_value)
                    .await
            }
            AgentSecretSubcommand::Delete {
                id,
                current_revision,
            } => self.cmd_delete(id, current_revision).await,
            AgentSecretSubcommand::List { environment } => self.cmd_list(environment).await,
        }
    }

    async fn cmd_create(
        &self,
        environment_reference: Option<EnvironmentReference>,
        path: AgentSecretPath,
        secret_type: String,
        secret_value: Option<String>,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_opt_environment_reference(
                EnvironmentResolveMode::Any,
                environment_reference.as_ref(),
            )
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let secret_type: AnalysedType = match serde_json::from_str(&secret_type) {
            Ok(res) => res,
            Err(err) => {
                log_error(format!("Malformed secret type provided: {err}"));
                bail!(NonSuccessfulExit);
            }
        };

        let secret_value: Option<serde_json::Value> =
            match secret_value.map(|sv| serde_json::from_str(&sv)).transpose() {
                Ok(res) => res,
                Err(err) => {
                    log_error(format!("Secret value is not valid json: {err}"));
                    bail!(NonSuccessfulExit);
                }
            };

        let result = clients
            .agent_secrets
            .create_agent_secret(
                &environment.environment_id.0,
                &AgentSecretCreation {
                    path,
                    secret_type,
                    secret_value,
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&AgentSecretCreateView(result));

        Ok(())
    }

    async fn cmd_update_value(
        &self,
        id: AgentSecretId,
        current_revision: AgentSecretRevision,
        secret_value: Option<String>,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let secret_value: Option<serde_json::Value> =
            match secret_value.map(|sv| serde_json::from_str(&sv)).transpose() {
                Ok(res) => res,
                Err(err) => {
                    log_error(format!("Secret value is not valid json: {err}"));
                    bail!(NonSuccessfulExit);
                }
            };

        let result = clients
            .agent_secrets
            .update_agent_secret(
                &id.0,
                &AgentSecretUpdate {
                    current_revision,
                    secret_value: OptionalFieldUpdate::update_from_option(secret_value),
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&AgentSecretCreateView(result));

        Ok(())
    }

    async fn cmd_delete(
        &self,
        id: AgentSecretId,
        current_revision: AgentSecretRevision,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .agent_secrets
            .delete_agent_secret(&id.0, current_revision.into())
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&AgentSecretCreateView(result));

        Ok(())
    }

    async fn cmd_list(
        &self,
        environment_reference: Option<EnvironmentReference>,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_opt_environment_reference(
                EnvironmentResolveMode::Any,
                environment_reference.as_ref(),
            )
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let results = clients
            .agent_secrets
            .get_environment_agent_secrets(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values;

        self.ctx.log_handler().log_view(&results);

        Ok(())
    }
}
