use crate::command::app::AppSubcommand;
use crate::command::shared_args::{AppOptionalComponentNames, BuildArgs, ForceBuildArg};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::model::app::AppBuildStep;
use crate::model::ComponentName;
use rust_mcp_sdk::macros::{mcp_tool, JsonSchema};
use rust_mcp_sdk::schema::{schema_utils::CallToolError, CallToolResult, TextContent};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

//*********************//
//  BuildAppTool      //
//*********************//
#[mcp_tool(
    name = "app:build",
    description = "Build one or more components in the application"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
pub struct BuildAppTool {
    /// Project root directory (required)
    pub project_root: String,

    /// Component names to build (empty = all components)
    #[serde(default)]
    pub components: Vec<String>,

    /// Skip modification time checks
    #[serde(default)]
    pub force_build: bool,

    /// Build steps to run (gen-rpc, componentize, link, add-metada)
    #[serde(default)]
    pub steps: Vec<String>,
}

impl BuildAppTool {
    pub async fn call_tool(&self, ctx: Arc<Context>) -> Result<CallToolResult, CallToolError> {
        let component_names: Vec<ComponentName> = self
            .components
            .iter()
            .map(|name| ComponentName::from(name.as_str()))
            .collect();

        let build_steps: Vec<AppBuildStep> = self
            .steps
            .iter()
            .filter_map(|step| Self::parse_build_step(step))
            .collect();

        if !self.steps.is_empty() && build_steps.is_empty() {
            return Err(CallToolError::from_message(
                "Invalid build steps. Valid options: gen-rpc, componentize, link, add-metadata",
            ));
        }

        let subcommand = AppSubcommand::Build {
            component_name: AppOptionalComponentNames {
                component_name: component_names,
            },
            build: BuildArgs {
                step: build_steps,
                force_build: ForceBuildArg {
                    force_build: self.force_build,
                },
            },
        };

        let result = ctx.app_handler().handle_command(subcommand).await;

        match result {
            Ok(_) => {
                let response = "Build completed successfully".to_string();
                Ok(CallToolResult::text_content(vec![TextContent::from(
                    response,
                )]))
            }
            Err(e) => {
                let response = format!("Build failed: {}", e);
                Err(CallToolError::from_message(response))
            }
        }
    }

    fn parse_build_step(step: &str) -> Option<AppBuildStep> {
        match step.to_lowercase().as_str() {
            "gen-rpc" | "genrpc" | "gen_rpc" => Some(AppBuildStep::GenRpc),
            "componentize" => Some(AppBuildStep::Componentize),
            "link" => Some(AppBuildStep::Link),
            "add-metadata" | "addmetadata" | "add_metadata" => Some(AppBuildStep::AddMetadata),
            _ => None,
        }
    }
}

//*********************//
//  DeployAppTool     //
//*********************//
#[mcp_tool(
    name = "app:deploy",
    description = "Deploy one or more components in the application"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
pub struct DeployAppTool {
    /// Component names to deploy (empty = all components)
    #[serde(default)]
    pub components: Vec<String>,

    /// Skip modification time checks
    #[serde(default)]
    pub force_build: bool,

    /// Update existing agents instead of recreating
    #[serde(default)]
    pub update_agents: bool,
}

impl DeployAppTool {
    pub fn call_tool(&self) -> Result<CallToolResult, CallToolError> {
        Ok(CallToolResult::text_content(vec![TextContent::from(
            "Deploy tool called".to_string(),
        )]))
    }
}

//*********************//
//  CleanAppTool      //
//*********************//
#[mcp_tool(
    name = "app:clean",
    description = "Clean all components in the application or by selection"
)]
#[derive(Debug, Deserialize, Serialize, JsonSchema, Clone)]
pub struct CleanAppTool {
    /// Component names to clean (empty = all components)
    #[serde(default)]
    pub components: Vec<String>,
}

impl CleanAppTool {
    pub fn call_tool(&self) -> Result<CallToolResult, CallToolError> {
        Ok(CallToolResult::text_content(vec![TextContent::from(
            "Clean tool called".to_string(),
        )]))
    }
}

// Generate enum from tools using tool_box macro
rust_mcp_sdk::tool_box!(AppTools, [BuildAppTool, DeployAppTool, CleanAppTool]);
