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
use crate::command_handler::Handlers;
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
}

