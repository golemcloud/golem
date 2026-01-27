use crate::command::GolemCliCommand;
use crate::mcp::security;
use clap::CommandFactory;
use clap::{Arg, Command};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone)]
pub struct Mcptool {
    pub tool_name: String,
    pub summary: Option<String>,
    pub input_schema: ToolInputSchema,
}

#[derive(Debug, Clone)]
pub struct ToolInputSchema {
    pub type_: String,
    pub properties: Option<Value>,
    pub required: Option<Vec<String>>,
}

pub struct CommandInfo;

impl CommandInfo {
    /// Extract all Golem CLI commands as MCP tools
    pub fn extract_all_tools(&self) -> Vec<Mcptool> {
        let root = GolemCliCommand::command();
        let mut tools = Vec::new();

        self.walk_commands(&root, &mut tools, Vec::new());

        tools
    }

    /// Recursively traverse subcommands
    fn walk_commands(&self, cmd: &Command, tools: &mut Vec<Mcptool>, path: Vec<String>) {
        let mut current_path = path.clone();
        let cmd_name = cmd.get_name().to_string();

        // Skip help and version commands
        if cmd_name == "help" || cmd_name == "version" {
            return;
        }

        if !(path.is_empty() && cmd_name == "golem-cli") {
            current_path.push(cmd_name.clone());
        }

        let subcommands: Vec<_> = cmd.get_subcommands().collect();
        let is_leaf = subcommands.is_empty();

        if !current_path.is_empty() && is_leaf {
            let command_path = current_path.join(" ");
            if !security::is_sensitive_command(&command_path) {
                tools.push(self.convert_to_mcp(cmd, &current_path));
            }
        }

        // Recurse into each subcommand
        for sub in subcommands {
            self.walk_commands(sub, tools, current_path.clone());
        }
    }

    /// Convert a Clap Command into an MCP tool
    fn convert_to_mcp(&self, cmd: &Command, path: &[String]) -> Mcptool {
        let tool_name = path.join("-");

        let summary = cmd
            .get_about()
            .map(|s| s.to_string())
            .or_else(|| cmd.get_long_about().map(|s| s.to_string()));

        let input_schema = self.extract_input_schema(cmd);

        Mcptool {
            tool_name,
            summary,
            input_schema,
        }
    }

    fn extract_input_schema(&self, cmd: &Command) -> ToolInputSchema {
        let mut properties = Map::new();
        let mut required = Vec::new();
        let mut positional_args = Vec::new(); // <- FIXED (outside loop)

        for arg in cmd.get_arguments() {
            // Skip global flags (e.g. --format, --profile)
            if arg.is_global_set() {
                continue;
            }

            if self.is_positional(arg) {
                positional_args.push(arg.get_id().to_string());

                if arg.is_required_set() {
                    required.push(arg.get_id().to_string());
                }

                // DO NOT add positional to properties
                continue;
            }

            // Normal flag / key-value args
            let arg_id = arg.get_id().to_string();
            let arg_type = self.infer_type(arg);

            let mut prop = Map::new();
            prop.insert("type".into(), json!(arg_type));

            if arg_type == "array" {
                prop.insert("items".into(), json!({"type": "string"}));
            }

            if let Some(help) = arg.get_help() {
                prop.insert("description".into(), json!(help.to_string()));
            }

            let values = arg.get_possible_values();
            if !values.is_empty() {
                let list: Vec<String> = values.iter().map(|v| v.get_name().to_string()).collect();
                prop.insert("enum".into(), json!(list));
            }
            properties.insert(arg_id.clone(), Value::Object(prop));

            if arg.is_required_set() {
                required.push(arg_id);
            }
        }

        // Add positional args into properties
        if !positional_args.is_empty() {
            let mut pos_schema = Map::new();
            pos_schema.insert("type".into(), json!("array"));
            pos_schema.insert("items".into(), json!({"type": "string"}));
            pos_schema.insert("description".into(), json!("Positional arguments in order"));
            properties.insert("positional_args".into(), Value::Object(pos_schema));
        }

        ToolInputSchema {
            type_: "object".into(),
            properties: Some(Value::Object(properties)),
            required: if required.is_empty() {
                None
            } else {
                Some(required)
            },
        }
    }

    fn infer_type(&self, arg: &Arg) -> &'static str {
        // Boolean flags
        if matches!(
            arg.get_action(),
            clap::ArgAction::SetTrue | clap::ArgAction::SetFalse
        ) {
            return "boolean";
        }

        // Multi-value args become arrays in JSON schema
        if matches!(arg.get_action(), clap::ArgAction::Append) {
            return "array";
        }

        // Numeric possible values → treat as number
        let values = arg.get_possible_values();
        if !values.is_empty() && values.iter().all(|v| v.get_name().parse::<f64>().is_ok()) {
            return "number";
        }

        "string"
    }

    fn is_positional(&self, arg: &Arg) -> bool {
        arg.get_long().is_none() && arg.get_short().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tools() {
        let extractor = CommandInfo;
        let tools = extractor.extract_all_tools();

        // Should have tools
        assert!(!tools.is_empty(), "Should extract tools");
        assert!(tools.iter().any(|t| t.tool_name == "build"));
        assert!(tools.iter().any(|t| t.tool_name == "agent-invoke"));

        // Should NOT have parent commands
        assert!(!tools.iter().any(|t| t.tool_name == "component"));
    }

    #[test]
    fn test_tool_has_description() {
        let extractor = CommandInfo;
        let tools = extractor.extract_all_tools();

        let app_new = tools
            .iter()
            .find(|t| t.tool_name == "build")
            .unwrap();
        assert!(app_new.summary.is_some());
    }

    #[test]
    fn test_tool_has_schema() {
        let extractor = CommandInfo;
        let tools = extractor.extract_all_tools();

        for tool in &tools {
            assert_eq!(tool.input_schema.type_, "object");
        }
    }

    #[test]
    fn test_extract_all_tools() {
        let extractor = CommandInfo;
        let tools = extractor.extract_all_tools();

        let tool_names: Vec<String> = tools.iter().map(|t| t.tool_name.clone()).collect();

        println!("Tools ({}) = {:?}", tool_names.len(), tool_names);

        assert!(!tool_names.is_empty());
    }
}
