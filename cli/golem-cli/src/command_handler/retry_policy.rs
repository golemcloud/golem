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

use crate::command::api::retry_policy::RetryPolicySubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::AnyhowMapServiceError;
use crate::log::log_error;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::text::retry_policy::{
    RetryPolicyCreateView, RetryPolicyDeleteView, RetryPolicyGetView, RetryPolicyUpdateView,
};
use anyhow::bail;
use golem_client::api::RetryPoliciesClient;
use golem_common::model::retry_policy::{
    RetryPolicyCreation, RetryPolicyDto, RetryPolicyId, RetryPolicyUpdate,
};
use std::sync::Arc;

pub struct RetryPolicyCommandHandler {
    ctx: Arc<Context>,
}

impl RetryPolicyCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: RetryPolicySubcommand) -> anyhow::Result<()> {
        match command {
            RetryPolicySubcommand::Create {
                name,
                priority,
                predicate_json,
                policy_json,
            } => {
                self.cmd_create(name, priority, predicate_json, policy_json)
                    .await
            }
            RetryPolicySubcommand::List => self.cmd_list().await,
            RetryPolicySubcommand::Get { name, id } => self.cmd_get(name, id).await,
            RetryPolicySubcommand::Update {
                name,
                id,
                priority,
                predicate_json,
                policy_json,
            } => {
                self.cmd_update(name, id, priority, predicate_json, policy_json)
                    .await
            }
            RetryPolicySubcommand::Delete { name, id } => self.cmd_delete(name, id).await,
        }
    }

    async fn cmd_create(
        &self,
        name: String,
        priority: u32,
        predicate_json: String,
        policy_json: String,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .retry_policies
            .create_retry_policy(
                &environment.environment_id.0,
                &RetryPolicyCreation {
                    name,
                    priority,
                    predicate_json,
                    policy_json,
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&RetryPolicyCreateView(result));

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
            .retry_policies
            .list_environment_retry_policies(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values;

        self.ctx.log_handler().log_view(&results);

        Ok(())
    }

    async fn resolve_retry_policy(
        &self,
        name: Option<String>,
        id: Option<RetryPolicyId>,
    ) -> anyhow::Result<RetryPolicyDto> {
        if let Some(name) = name {
            let environment = self
                .ctx
                .environment_handler()
                .resolve_environment(EnvironmentResolveMode::Any)
                .await?;

            let clients = self.ctx.golem_clients().await?;

            let Some(result) = clients
                .retry_policies
                .list_environment_retry_policies(&environment.environment_id.0)
                .await
                .map_service_error()?
                .values
                .into_iter()
                .find(|p| p.name == name)
            else {
                log_error(format!("Retry policy '{}' not found in environment", name));
                bail!(NonSuccessfulExit);
            };

            Ok(result)
        } else if let Some(id) = id {
            let clients = self.ctx.golem_clients().await?;

            let result = clients
                .retry_policies
                .get_retry_policy(&id.0)
                .await
                .map_service_error()?;

            Ok(result)
        } else {
            bail!("Either name or --id must be specified");
        }
    }

    async fn cmd_get(&self, name: Option<String>, id: Option<RetryPolicyId>) -> anyhow::Result<()> {
        let result = self.resolve_retry_policy(name, id).await?;

        self.ctx.log_handler().log_view(&RetryPolicyGetView(result));

        Ok(())
    }

    async fn cmd_update(
        &self,
        name: Option<String>,
        id: Option<RetryPolicyId>,
        priority: Option<u32>,
        predicate_json: Option<String>,
        policy_json: Option<String>,
    ) -> anyhow::Result<()> {
        let current = self.resolve_retry_policy(name, id).await?;

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .retry_policies
            .update_retry_policy(
                &current.id.0,
                &RetryPolicyUpdate {
                    current_revision: current.revision,
                    priority,
                    predicate_json,
                    policy_json,
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&RetryPolicyUpdateView(result));

        Ok(())
    }

    async fn cmd_delete(
        &self,
        name: Option<String>,
        id: Option<RetryPolicyId>,
    ) -> anyhow::Result<()> {
        let current = self.resolve_retry_policy(name, id).await?;

        let clients = self.ctx.golem_clients().await?;

        let result = clients
            .retry_policies
            .delete_retry_policy(&current.id.0, current.revision.into())
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&RetryPolicyDeleteView(result));

        Ok(())
    }
}
