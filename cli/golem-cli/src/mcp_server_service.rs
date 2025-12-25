// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;
use crate::context::Context;
use golem_common::model::agent::RegisteredAgentType;
use golem_client::model::ComponentDto;
use golem_common::model::component::{ComponentName, ComponentRevision};
use crate::command_handler::Handlers;
use crate::model::environment::EnvironmentResolveMode;
use rmcp_macros::{tool, tool_router};

pub struct Tools {
    ctx: Arc<Context>,
}

#[tool_router]
impl Tools {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    #[tool]
    async fn list_agent_types(&self) -> Result<Vec<RegisteredAgentType>, String> {
        self.ctx
            .app_handler()
            .cmd_list_agent_types()
            .await
            .map_err(|e: anyhow::Error| e.to_string())
    }

    #[tool]
    async fn list_components(&self) -> Result<Vec<ComponentDto>, String> {
        self.ctx
            .component_handler()
            .cmd_list_components()
            .await
            .map_err(|e: anyhow::Error| e.to_string())
    }

    #[tool]
    async fn get_component(
        &self,
        component_name: ComponentName,
        revision: Option<ComponentRevision>,
    ) -> Result<Option<golem_client::model::ComponentDto>, String> {
        let environment = self.ctx.environment_handler().resolve_environment(crate::model::environment::EnvironmentResolveMode::Any).await
            .map_err(|e: anyhow::Error| e.to_string())?;

        self.ctx
            .component_handler()
            .resolve_component(&environment, &component_name, revision.map(|r| r.into()))
            .await
            .map_err(|e: anyhow::Error| e.to_string())
    }

    #[tool]
    async fn describe_component(
        &self,
        component_name: ComponentName,
        revision: Option<ComponentRevision>,
    ) -> Result<Vec<golem_client::model::ComponentDto>, String> {
        self.ctx
            .component_handler()
            .cmd_describe_component(Some(component_name), revision)
            .await
            .map_err(|e: anyhow::Error| e.to_string())
            .map(|components| {
                components
                    .into_iter()
                    .map(|c| c.into())
                    .collect()
            })
    }

    #[tool]
    async fn new_component(
        &self,
        component_name: ComponentName,
        template: String,
    ) -> Result<(), String> {
        self.ctx
            .component_handler()
            .cmd_new_component(template, component_name)
            .await
            .map_err(|e: anyhow::Error| e.to_string())
    }

    #[tool]
    async fn update_component(
        &self,
        component_name: ComponentName,
        revision: Option<ComponentRevision>,
        update_mode: golem_cli::model::worker::AgentUpdateMode,
        await_update: bool,
    ) -> Result<String, String> {
        let component = self.ctx.component_handler().resolve_component(
            &self.ctx.environment_handler().resolve_environment(golem_cli::model::environment::EnvironmentResolveMode::Any).await.map_err(|e| e.to_string())?,
            &component_name,
            revision.map(|r| r.into()),
        ).await.map_err(|e| e.to_string())?;

        if let Some(component) = component {
            let result = self
                .ctx
                .worker_handler()
                .update_component_workers(
                    &component.name,
                    &golem_common::model::component::ComponentId(component.id.0),
                    update_mode,
                    component.revision,
                    await_update,
                )
                .await
                .map_err(|e: anyhow::Error| e.to_string())?;
            Ok(format!("{:?}", result))
        } else {
            Err(format!("Component {} not found", component_name))
        }
    }

    #[tool]
    async fn invoke_worker(
        &self,
        worker_name: golem_cli::model::worker::WorkerName,
        function_name: String,
        arguments: Vec<String>,
    ) -> Result<String, String> {
        let worker_name_match = self.ctx.worker_handler().match_worker_name(worker_name).await.map_err(|e| e.to_string())?;

        let component = self.ctx.component_handler().resolve_component(
            &worker_name_match.environment,
            &worker_name_match.component_name,
            Some((&worker_name_match.worker_name).into()),
        ).await.map_err(|e| e.to_string())?;

        if let Some(component) = component {
            let parsed_args = arguments
                .into_iter()
                .map(|s| serde_json::from_str(&s).map(golem_wasm::json::OptionallyValueAndTypeJson::Json).map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()?;

            let result = self
                .ctx
                .worker_handler()
                .invoke_worker(
                    &component,
                    &worker_name_match.worker_name,
                    &function_name,
                    parsed_args,
                    golem_common::model::IdempotencyKey::fresh(),
                    false,
                    None,
                )
                .await
                .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!("{:?}", result))
        } else {
            Err(format!("Component {} not found", worker_name_match.component_name))
        }
    }
}

