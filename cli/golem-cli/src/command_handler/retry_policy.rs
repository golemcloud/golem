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

use crate::command::retry_policy::RetryPolicySubcommand;
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
use golem_common::model::UntypedJsonBody;
use golem_common::model::retry_policy::RetryPolicyDto;
use golem_common::model::retry_policy::{
    Predicate, RetryPolicy, RetryPolicyCreation, RetryPolicyId, RetryPolicyUpdate,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
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
                predicate,
                policy,
            } => self.cmd_create(name, priority, predicate, policy).await,
            RetryPolicySubcommand::List => self.cmd_list().await,
            RetryPolicySubcommand::Get { name, id } => self.cmd_get(name, id).await,
            RetryPolicySubcommand::Update {
                name,
                id,
                priority,
                predicate,
                policy,
            } => self.cmd_update(name, id, priority, predicate, policy).await,
            RetryPolicySubcommand::Delete { name, id } => self.cmd_delete(name, id).await,
        }
    }

    async fn cmd_create(
        &self,
        name: String,
        priority: u32,
        predicate: String,
        policy: String,
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
                    predicate: parse_and_validate_predicate(&predicate)?,
                    policy: parse_and_validate_policy(&policy)?,
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
        predicate: Option<String>,
        policy: Option<String>,
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
                    predicate: predicate
                        .as_deref()
                        .map(parse_and_validate_predicate)
                        .transpose()?,
                    policy: policy
                        .as_deref()
                        .map(parse_and_validate_policy)
                        .transpose()?,
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

fn parse_and_validate_predicate(value: &str) -> anyhow::Result<UntypedJsonBody> {
    parse_and_validate_yaml_or_json_as_untyped_json_body::<Predicate>("predicate", value)
}

fn parse_and_validate_policy(value: &str) -> anyhow::Result<UntypedJsonBody> {
    parse_and_validate_yaml_or_json_as_untyped_json_body::<RetryPolicy>("policy", value)
}

fn parse_and_validate_yaml_or_json_as_untyped_json_body<T: Serialize + DeserializeOwned>(
    kind: &'static str,
    value: &str,
) -> anyhow::Result<UntypedJsonBody> {
    let raw = serde_yaml::from_str::<serde_json::Value>(value)
        .map_err(|e| anyhow::anyhow!("Invalid {kind} YAML/JSON: {e}"))?;
    let typed = serde_json::from_value::<T>(raw)
        .map_err(|e| anyhow::anyhow!("Invalid {kind} value: {e}"))?;
    serde_json::to_value(typed)
        .map_err(|e| anyhow::anyhow!("Failed to encode {kind} as JSON: {e}"))
        .map(UntypedJsonBody)
}
