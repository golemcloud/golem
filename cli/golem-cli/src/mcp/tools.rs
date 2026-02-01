// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://github.com/golemcloud/golem/blob/main/LICENSE

use rust_mcp_sdk::macros::{mcp_tool, JsonSchema};
use serde::{Deserialize, Serialize};

#[mcp_tool(
    name = "golem_new",
    description = "Create a new Golem application in the specified directory"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemNewTool {
    pub application_name: Option<String>,
    pub language: Option<String>,
}

#[mcp_tool(
    name = "golem_build",
    description = "Build all or selected components in the application"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemBuildTool {
    pub component_name: Option<String>,
    pub force_build: Option<bool>,
}

#[mcp_tool(
    name = "golem_deploy",
    description = "Deploy the application to the configured environment"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemDeployTool {
    pub plan: Option<bool>,
    pub reset: Option<bool>,
    pub update_agents: Option<String>,
}

#[mcp_tool(
    name = "golem_clean",
    description = "Clean build artifacts for all or selected components"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemCleanTool {
    pub component_name: Option<String>,
}

#[mcp_tool(
    name = "golem_diagnose",
    description = "Run diagnostics to identify potential tooling problems"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemDiagnoseTool {
    pub component_name: Option<String>,
}

#[mcp_tool(
    name = "golem_update_agents",
    description = "Update existing agents to the latest component version"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemUpdateAgentsTool {
    pub component_name: Option<String>,
    pub update_mode: Option<String>,
    pub wait: Option<bool>,
}

#[mcp_tool(
    name = "golem_redeploy_agents",
    description = "Redeploy all agents using the latest component version"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemRedeployAgentsTool {
    pub component_name: Option<String>,
}

#[mcp_tool(
    name = "golem_list_agent_types",
    description = "List all deployed agent types in the application"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemListAgentTypesTool {}

#[mcp_tool(
    name = "golem_component_new",
    description = "Create a new component within the current application"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemComponentNewTool {
    pub component_name: Option<String>,
    pub template: Option<String>,
}

#[mcp_tool(
    name = "golem_component_list",
    description = "List all deployed component versions and their metadata"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemComponentListTool {}

#[mcp_tool(
    name = "golem_component_get",
    description = "Get metadata for a specific component revision"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemComponentGetTool {
    pub component_name: Option<String>,
    pub revision: Option<u64>,
}

#[mcp_tool(
    name = "golem_agent_new",
    description = "Create a new agent instance"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemAgentNewTool {
    pub agent_id: String,
    pub env: Option<Vec<String>>,
}

#[mcp_tool(
    name = "golem_agent_invoke",
    description = "Invoke a function on an agent"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemAgentInvokeTool {
    pub agent_id: String,
    pub function_name: String,
    pub args: Option<Vec<String>>,
}

#[mcp_tool(
    name = "golem_agent_get",
    description = "Get information about a specific agent"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemAgentGetTool {
    pub agent_id: String,
}

#[mcp_tool(
    name = "golem_agent_list",
    description = "List all agents for a component"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemAgentListTool {
    pub component_name: Option<String>,
    pub filter: Option<String>,
}

#[mcp_tool(
    name = "golem_agent_delete",
    description = "Delete an agent instance"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemAgentDeleteTool {
    pub agent_id: String,
}

#[mcp_tool(
    name = "golem_environment_list",
    description = "List all application environments"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemEnvironmentListTool {}

#[mcp_tool(
    name = "golem_profile_list",
    description = "List all configured CLI profiles"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemProfileListTool {}

#[mcp_tool(
    name = "golem_profile_switch",
    description = "Switch to a different CLI profile"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GolemProfileSwitchTool {
    pub profile_name: String,
}
