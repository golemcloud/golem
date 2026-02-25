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

use crate::mcp::GolemAgentMcpServer;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::base_model::agent::AgentMethod;
use rmcp::ErrorData;
use rmcp::handler::server::prompt::{GetPromptHandler, PromptContext};
use rmcp::handler::server::router::prompt::{IntoPromptRoute, PromptRoute};
use rmcp::model::{
    GetPromptResult, Prompt, PromptMessage, PromptMessageContent, PromptMessageRole,
};

#[allow(unused)]
#[derive(Clone)]
pub struct AgentMcpPrompt {
    pub agent_method: AgentMethod,
    pub raw_prompt: Prompt,
}

impl GetPromptHandler<GolemAgentMcpServer, ()> for AgentMcpPrompt {
    fn handle(
        self,
        context: PromptContext<'_, GolemAgentMcpServer>,
    ) -> BoxFuture<'_, Result<GetPromptResult, ErrorData>> {
        async move {
            let parameters = context
                .arguments
                .map(|x| {
                    x.iter()
                        .map(|(k, v)| format!("{}: {}", k, v))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "no parameters".to_string());

            let result = GetPromptResult {
                description: None,
                messages: vec![PromptMessage {
                    role: PromptMessageRole::User,
                    content: PromptMessageContent::Text {
                        text: format!(
                            "{}, call {} with the following parameters: {}",
                            "developer-given prompt", self.agent_method.name, parameters
                        ),
                    },
                }],
            };

            Ok(result)
        }
        .boxed()
    }
}

impl IntoPromptRoute<GolemAgentMcpServer, ()> for AgentMcpPrompt {
    fn into_prompt_route(self) -> PromptRoute<GolemAgentMcpServer> {
        PromptRoute::new(self.raw_prompt.clone(), self)
    }
}
