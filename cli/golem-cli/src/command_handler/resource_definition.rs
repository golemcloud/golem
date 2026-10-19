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

use crate::command::resource_definition::ResourceDefinitionSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::error::service::{MapServiceError, ServiceError};
use crate::log::log_error;
use crate::model::create_action::CreateAction;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::resource_definition::{
    ResourceDefinitionCreateView, ResourceDefinitionDeleteView, ResourceDefinitionGetView,
    ResourceDefinitionListView, ResourceDefinitionUpdateView,
};
use anyhow::bail;
use golem_client::api::ResourcesClient;
use golem_common::base_model::api;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::{
    EnforcementAction, ResourceDefinition, ResourceDefinitionCreation, ResourceDefinitionId,
    ResourceDefinitionUpdate, ResourceLimit, ResourceName,
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
                update_existing,
            } => {
                self.cmd_create(
                    name,
                    limit,
                    enforcement_action.into(),
                    unit,
                    units,
                    update_existing,
                )
                .await
            }
            ResourceDefinitionSubcommand::Update {
                name,
                id,
                limit,
                enforcement_action,
                unit,
                units,
            } => {
                self.cmd_update(
                    name,
                    id,
                    limit,
                    enforcement_action.map(Into::into),
                    unit,
                    units,
                )
                .await
            }
            ResourceDefinitionSubcommand::Delete { name, id } => self.cmd_delete(name, id).await,
            ResourceDefinitionSubcommand::Get { name, id } => self.cmd_get(name, id).await,
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
        update_existing: bool,
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

        let existing = if update_existing {
            self.find_resource_definition_by_name(&environment.environment_id, &name)
                .await?
        } else {
            None
        };

        let clients = self.ctx.golem_clients().await?;

        let (action, result) = match existing {
            Some(existing) => (
                CreateAction::Updated,
                clients
                    .resources
                    .update_resource(
                        &existing.id.0,
                        &ResourceDefinitionUpdate {
                            current_revision: existing.revision,
                            limit: Some(limit),
                            enforcement_action: Some(enforcement_action),
                            unit: Some(unit),
                            units: Some(units),
                        },
                    )
                    .await
                    .map_service_error()?,
            ),
            None => {
                let result = clients
                    .resources
                    .create_resource(
                        &environment.environment_id.0,
                        &ResourceDefinitionCreation {
                            name: ResourceName(name.clone()),
                            limit,
                            enforcement_action,
                            unit,
                            units,
                        },
                    )
                    .await;
                match result {
                    Ok(result) => (CreateAction::Created, result),
                    Err(err) => {
                        let err: ServiceError = err.into();
                        if !update_existing
                            && err.is_already_exists(
                                api::error_code::RESOURCE_DEFINITION_ALREADY_EXISTS,
                            )
                        {
                            log_error(format!(
                                "Resource definition '{name}' already exists. Use --update-existing to update it"
                            ));
                            bail!(NonSuccessfulExit);
                        }
                        return Err(err.into());
                    }
                }
            }
        };

        self.ctx
            .log_handler()
            .log_output(ResourceDefinitionCreateView {
                action,
                resource_definition: result,
            })?;

        Ok(())
    }

    async fn find_resource_definition_by_name(
        &self,
        environment_id: &EnvironmentId,
        name: &str,
    ) -> anyhow::Result<Option<ResourceDefinition>> {
        let clients = self.ctx.golem_clients().await?;

        Ok(clients
            .resources
            .get_environment_resource(&environment_id.0, name)
            .await
            .map_service_error_not_found_as_opt()?)
    }

    async fn resolve_resource_definition(
        &self,
        name: Option<String>,
        id: Option<ResourceDefinitionId>,
    ) -> anyhow::Result<ResourceDefinition> {
        let clients = self.ctx.golem_clients().await?;

        if let Some(name) = name {
            let environment = self
                .ctx
                .environment_handler()
                .resolve_environment(EnvironmentResolveMode::Any)
                .await?;

            let resource = self
                .find_resource_definition_by_name(&environment.environment_id, &name)
                .await?;

            let Some(resource) = resource else {
                log_error(format!(
                    "Resource definition '{name}' not found in environment"
                ));
                bail!(NonSuccessfulExit);
            };

            Ok(resource)
        } else if let Some(id) = id {
            let resource = clients
                .resources
                .get_resource(&id.0)
                .await
                .map_service_error()?;

            Ok(resource)
        } else {
            log_error("Either name or --id must be provided");
            bail!(NonSuccessfulExit);
        }
    }

    async fn cmd_update(
        &self,
        name: Option<String>,
        id: Option<ResourceDefinitionId>,
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

        let resource = self.resolve_resource_definition(name, id).await?;

        let clients = self.ctx.golem_clients().await?;
        let result = clients
            .resources
            .update_resource(
                &resource.id.0,
                &ResourceDefinitionUpdate {
                    current_revision: resource.revision,
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
            .log_output(ResourceDefinitionUpdateView(result))?;

        Ok(())
    }

    async fn cmd_delete(
        &self,
        name: Option<String>,
        id: Option<ResourceDefinitionId>,
    ) -> anyhow::Result<()> {
        let resource = self.resolve_resource_definition(name, id).await?;

        let clients = self.ctx.golem_clients().await?;
        clients
            .resources
            .delete_resource(&resource.id.0, resource.revision.get())
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_output(ResourceDefinitionDeleteView::from(resource))?;

        Ok(())
    }

    async fn cmd_get(
        &self,
        name: Option<String>,
        id: Option<ResourceDefinitionId>,
    ) -> anyhow::Result<()> {
        let result = self.resolve_resource_definition(name, id).await?;

        self.ctx
            .log_handler()
            .log_output(ResourceDefinitionGetView(result))?;

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

        self.ctx
            .log_handler()
            .log_output(ResourceDefinitionListView { resources: results })?;

        Ok(())
    }
}
