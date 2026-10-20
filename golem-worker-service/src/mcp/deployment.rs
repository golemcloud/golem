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

use crate::mcp::agent_mcp_capability::McpAgentCapability;
use crate::mcp::agent_mcp_prompt::{AgentMcpPrompt, PromptRegistry};
use crate::mcp::agent_mcp_resource::{AgentMcpResource, ResourceRegistry};
use crate::mcp::agent_mcp_server::GolemAgentMcpServer;
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::invoke;
use golem_common::schema::AgentTypeKind;
use golem_service_base::mcp::CompiledMcp;
use rmcp::ErrorData;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::Tool;
use std::sync::Arc;

/// A compiled MCP deployment and the tools, resources and prompts it serves. It does not change
/// after it is built; every request served with the deployment shares one value.
pub struct McpDeployment {
    pub compiled: CompiledMcp,
    capabilities: Result<McpCapabilities, ErrorData>,
}

/// The tools, resources and prompts of one deployment.
pub struct McpCapabilities {
    pub(crate) router: ToolRouter<GolemAgentMcpServer>,
    pub(crate) tools: Box<[Tool]>,
    pub(crate) resources: ResourceRegistry,
    pub(crate) prompts: PromptRegistry,
}

impl McpDeployment {
    /// Builds the deployment's capabilities.
    pub fn new(compiled: CompiledMcp) -> Self {
        let capabilities = McpCapabilities::of(&compiled);
        Self {
            compiled,
            capabilities,
        }
    }

    /// The deployment's capabilities, or why they could not be built.
    pub fn capabilities(&self) -> Result<&McpCapabilities, ErrorData> {
        self.capabilities.as_ref().map_err(Clone::clone)
    }
}

impl McpCapabilities {
    fn of(compiled: &CompiledMcp) -> Result<Self, ErrorData> {
        let capabilities = agent_capabilities_from_deployment(compiled);
        let router = capabilities
            .tools
            .into_iter()
            .fold(ToolRouter::new(), |router, tool| router.with_route(tool));
        let tools = router
            .list_all()
            .into_iter()
            .map(Ok)
            .chain(
                compiled
                    .tools
                    .iter()
                    .map(invoke::native_tool::tool_metadata),
            )
            .collect::<Result<Box<[_]>, _>>()?;
        let resources = capabilities.resources.into_iter().fold(
            ResourceRegistry::default(),
            |mut registry, resource| {
                registry.insert(resource);
                registry
            },
        );
        let prompts = capabilities.prompts.into_iter().fold(
            PromptRegistry::default(),
            |mut registry, prompt| {
                registry.insert(prompt);
                registry
            },
        );
        Ok(Self {
            router,
            tools,
            resources,
            prompts,
        })
    }
}

/// The tools, resources and prompts that a deployment's agent types export.
pub(super) struct AgentCapabilities {
    pub(super) tools: Vec<AgentMcpTool>,
    pub(super) resources: Vec<AgentMcpResource>,
    pub(super) prompts: Vec<AgentMcpPrompt>,
}

pub(super) fn agent_capabilities_from_deployment(compiled_mcp: &CompiledMcp) -> AgentCapabilities {
    let domain = &compiled_mcp.domain;
    let mut tools = vec![];
    let mut resources = vec![];
    let mut prompts = vec![];

    let account_id = compiled_mcp.account_id;
    let account_email = compiled_mcp.account_email.clone();
    let environment_id = compiled_mcp.environment_id;

    tracing::info!(
        "Found {} registered agent types for domain {}: {:?}",
        compiled_mcp.registered_agent_types.len(),
        domain.0,
        compiled_mcp
            .registered_agent_types
            .iter()
            .map(|rat| rat.agent_type.type_name.0.clone())
            .collect::<Vec<_>>()
    );

    for registered_agent_type in &compiled_mcp.registered_agent_types {
        if registered_agent_type.agent_type.kind == AgentTypeKind::HttpRouter {
            continue;
        }

        tracing::debug!(
            "Processing agent type {} for domain {}: implemented by component {}, methods: {:?}",
            registered_agent_type.agent_type.type_name.0,
            domain.0,
            registered_agent_type.implemented_by.component_id.0,
            registered_agent_type
                .agent_type
                .methods
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>()
        );

        let agent_type = &registered_agent_type.agent_type;
        let component_id = registered_agent_type.implemented_by.component_id;
        // One `SchemaGraph` per agent type, shared by its constructor and
        // methods; `SchemaType::Ref` bodies resolve against it both while
        // rendering MCP schemas and while converting to the legacy invoke
        // types.
        let schema_graph = Arc::new(agent_type.schema.clone());

        if let Some(prompt_hint) = &agent_type.constructor.prompt_hint {
            prompts.push(AgentMcpPrompt::from_constructor_hint(
                &agent_type.type_name,
                &agent_type.description,
                prompt_hint,
            ));
        }

        for method in &agent_type.methods {
            // Validate the method for MCP export before advertising anything.
            // If the capability cannot be exported, invoking it would always
            // fail, so skip both the tool/resource and its prompt.
            let agent_method_mcp = McpAgentCapability::from_agent_method(
                &account_id,
                &account_email,
                &environment_id,
                &agent_type.type_name,
                agent_type.mode,
                schema_graph.clone(),
                method,
                &agent_type.constructor,
                component_id,
            );

            match agent_method_mcp {
                Ok(McpAgentCapability::Tool(agent_mcp_tool)) => {
                    tools.push(*agent_mcp_tool);
                }
                Ok(McpAgentCapability::Resource(agent_mcp_resource)) => {
                    resources.push(*agent_mcp_resource);
                }
                Err(e) => {
                    tracing::warn!(
                        "Skipping method {} of agent type {} for domain {}: {:#}",
                        method.name,
                        agent_type.type_name.0,
                        domain.0,
                        e
                    );
                    continue;
                }
            }

            if let Some(prompt_hint) = &method.prompt_hint {
                prompts.push(AgentMcpPrompt::from_method_hint(
                    &agent_type.type_name,
                    &schema_graph,
                    method,
                    &agent_type.constructor,
                    prompt_hint,
                ));
            }
        }
    }

    tracing::info!(
        "Found {} tools, {} resources, and {} prompts for domain {}",
        tools.len(),
        resources.len(),
        prompts.len(),
        domain.0
    );

    AgentCapabilities {
        tools,
        resources,
        prompts,
    }
}
