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

use crate::command::api::resource_definition::ResourceDefinitionSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::AnyhowMapServiceError;
use crate::log::log_error;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::text::resource_definition::{
    ResourceDefinitionCreateView, ResourceDefinitionDeleteView, ResourceDefinitionGetView,
    ResourceDefinitionUpdateView,
};
use anyhow::bail;
use golem_client::api::ResourcesClient;
use golem_common::model::quota::{
    EnforcementAction, ResourceDefinitionCreation, ResourceDefinitionId,
    ResourceDefinitionRevision, ResourceDefinitionUpdate, ResourceLimit, ResourceName,
};
use std::sync::Arc;

pub struct ResourceDefinitionCommandHandler {
    ctx: Arc<Context>,
}

impl ResourceDefinitionCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(
        &self,
        command: ResourceDefinitionSubcommand,
    ) -> anyhow::Result<()> {
        match command {
            ResourceDefinitionSubcommand::Create {
                name,
                limit,
                enforcement_action,
                unit,
                units,
            } => {
                self.cmd_create(name, limit, enforcement_action.into(), unit, units)
                    .await
            }
            ResourceDefinitionSubcommand::Update {
                id,
                current_revision,
                limit,
                enforcement_action,
                unit,
                units,
            } => {
                self.cmd_update(
                    id,
                    current_revision,
                    limit,
                    enforcement_action.map(Into::into),
                    unit,
                    units,
                )
                .await
            }
            ResourceDefinitionSubcommand::Delete {
                id,
                current_revision,
            } => self.cmd_delete(id, current_revision).await,
            ResourceDefinitionSubcommand::Get { id } => self.cmd_get(id).await,
            ResourceDefinitionSubcommand::List => self.cmd_list().await,
        }
    }

    async fn cmd_create(
        &self,
        name: String,
        limit: String,
        enforcement_action: EnforcementAction,
        unit: String,
        units: String,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let limit: ResourceLimit = match serde_json::from_str(&limit) {
            Ok(l) => l,
            Err(err) => {
                log_error(format!("Malformed resource limit JSON: {err}"));
                bail!(NonSuccessfulExit);
            }
        };

        let clients = self.ctx.golem_clients().await?;
        let result = clients
            .resources
            .create_resource(
                &environment.environment_id.0,
                &ResourceDefinitionCreation {
                    name: ResourceName(name),
                    limit,
                    enforcement_action,
                    unit,
                    units,
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&ResourceDefinitionCreateView(result));

        Ok(())
    }

    async fn cmd_update(
        &self,
        id: ResourceDefinitionId,
        current_revision: ResourceDefinitionRevision,
        limit: Option<String>,
        enforcement_action: Option<EnforcementAction>,
        unit: Option<String>,
        units: Option<String>,
    ) -> anyhow::Result<()> {
        let limit: Option<ResourceLimit> = match limit.map(|l| serde_json::from_str(&l)).transpose()
        {
            Ok(l) => l,
            Err(err) => {
                log_error(format!("Malformed resource limit JSON: {err}"));
                bail!(NonSuccessfulExit);
            }
        };

        let clients = self.ctx.golem_clients().await?;
        let result = clients
            .resources
            .update_resource(
                &id.0,
                &ResourceDefinitionUpdate {
                    current_revision,
                    limit,
                    enforcement_action,
                    unit,
                    units,
                },
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&ResourceDefinitionUpdateView(result));

        Ok(())
    }

    async fn cmd_delete(
        &self,
        id: ResourceDefinitionId,
        current_revision: ResourceDefinitionRevision,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        let resource = clients
            .resources
            .get_resource(&id.0)
            .await
            .map_service_error()?;

        clients
            .resources
            .delete_resource(&id.0, current_revision.get())
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&ResourceDefinitionDeleteView(resource));

        Ok(())
    }

    async fn cmd_get(&self, id: ResourceDefinitionId) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;
        let result = clients
            .resources
            .get_resource(&id.0)
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&ResourceDefinitionGetView(result));

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
            .resources
            .list_environment_resources(&environment.environment_id.0)
            .await
            .map_service_error()?
            .values;

        self.ctx.log_handler().log_view(&results);

        Ok(())
    }
}
