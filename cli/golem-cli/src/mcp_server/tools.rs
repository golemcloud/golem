// Auto-generated tool definitions for 96 CLI commands
// Generated from golem-cli help output

use rmcp::model::Tool;
use std::sync::Arc;

/// Generate list of all CLI commands as MCP tools
pub fn generate_tool_list() -> Vec<Tool> {
    vec![
        Tool::new(
            "agent",
            "Execute agent command",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_cancel_invocation",
            "Cancel-invocation agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_delete",
            "Delete agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_file_contents",
            "File-contents agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_files",
            "Files agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_get",
            "Get agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_interrupt",
            "Interrupt agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_invoke",
            "Invoke agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_list",
            "List agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_new",
            "New agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_oplog",
            "Oplog agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_resume",
            "Resume agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_revert",
            "Revert agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_simulate_crash",
            "Simulate-crash agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_stream",
            "Stream agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "agent_update",
            "Update agents",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api",
            "Execute api command",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud",
            "Cloud apis",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud_certificate",
            "Certificate api cloud",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud_certificate_delete",
            "Delete api cloud certificate",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud_certificate_get",
            "Get api cloud certificate",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud_certificate_new",
            "New api cloud certificate",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud_domain",
            "Domain api cloud",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud_domain_delete",
            "Delete api cloud domain",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud_domain_get",
            "Get api cloud domain",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_cloud_domain_new",
            "New api cloud domain",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_definition",
            "Definition apis",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_definition_delete",
            "Delete api definition",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_definition_deploy",
            "Deploy api definition",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_definition_export",
            "Export api definition",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_definition_get",
            "Get api definition",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_definition_list",
            "List api definition",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_definition_swagger",
            "Swagger api definition",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_deploy",
            "Deploy apis",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_deployment",
            "Deployment apis",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_deployment_delete",
            "Delete api deployment",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_deployment_deploy",
            "Deploy api deployment",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_deployment_get",
            "Get api deployment",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_deployment_list",
            "List api deployment",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_security_scheme",
            "Security-scheme apis",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_security_scheme_create",
            "Create api security-scheme",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "api_security_scheme_get",
            "Get api security-scheme",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app",
            "Execute app command",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app_build",
            "Build apps",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app_clean",
            "Clean apps",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app_deploy",
            "Deploy apps",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app_diagnose",
            "Diagnose apps",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app_list_agent_types",
            "List-agent-types apps",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app_new",
            "New apps",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app_redeploy_agents",
            "Redeploy-agents apps",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "app_update_agents",
            "Update-agents apps",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud",
            "Execute cloud command",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_account",
            "Account clouds",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_account_delete",
            "Delete cloud account",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_account_get",
            "Get cloud account",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_account_new",
            "New cloud account",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_account_update",
            "Update cloud account",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project",
            "Project clouds",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_get_default",
            "Get-default cloud project",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_grant",
            "Grant cloud project",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_list",
            "List cloud project",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_new",
            "New cloud project",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_plugin",
            "Plugin cloud project",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_plugin_get",
            "Get cloud project plugin",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_plugin_install",
            "Install cloud project plugin",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_plugin_uninstall",
            "Uninstall cloud project plugin",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_plugin_update",
            "Update cloud project plugin",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_policy",
            "Policy cloud project",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_policy_get",
            "Get cloud project policy",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "cloud_project_policy_new",
            "New cloud project policy",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component",
            "Execute component command",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_add_dependency",
            "Add-dependency components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_build",
            "Build components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_clean",
            "Clean components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_deploy",
            "Deploy components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_diagnose",
            "Diagnose components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_get",
            "Get components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_list",
            "List components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_new",
            "New components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_plugin",
            "Plugin components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_plugin_get",
            "Get component plugin",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_plugin_install",
            "Install component plugin",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_plugin_uninstall",
            "Uninstall component plugin",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_plugin_update",
            "Update component plugin",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_redeploy_agents",
            "Redeploy-agents components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_templates",
            "Templates components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "component_update_agents",
            "Update-agents components",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "plugin",
            "Execute plugin command",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "plugin_get",
            "Get plugins",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "plugin_list",
            "List plugins",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "plugin_register",
            "Register plugins",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "plugin_unregister",
            "Unregister plugins",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "repl",
            "Execute repl command",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "server",
            "Execute server command",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "server_clean",
            "Clean servers",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
        Tool::new(
            "server_run",
            "Run servers",
            Arc::new(serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
        })
                .as_object().unwrap().clone())
        ),
    ]
}

/// Check if command is safe to expose via MCP
pub fn is_command_safe_to_expose(command: &str) -> bool {
    const UNSAFE_COMMANDS: &[&str] = &["cloud account grant", "cloud token", "profile"];
    !UNSAFE_COMMANDS.iter().any(|unsafe_cmd| command.starts_with(unsafe_cmd))
}

// Total tools exposed: 96
// Total tools filtered: 16
