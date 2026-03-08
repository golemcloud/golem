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

use crate::model::cli_command_metadata::{CliArgMetadata, CliCommandMetadata};
use rmcp::model::Tool;
use serde_json::{Map, Value};
use std::sync::Arc;

/// Convert the CLI command tree into a flat list of MCP tools.
/// Each leaf command (no subcommands) becomes an MCP tool.
/// The tool name uses dot-separated path (e.g. "component.new", "agent.invoke").
pub fn cli_metadata_to_tools(metadata: &CliCommandMetadata) -> Vec<Tool> {
    let mut tools = Vec::new();
    collect_tools(metadata, &[], &mut tools);
    tools
}

fn collect_tools(metadata: &CliCommandMetadata, path: &[&str], tools: &mut Vec<Tool>) {
    if metadata.subcommands.is_empty() {
        // Leaf command — create an MCP tool
        if !path.is_empty() {
            let tool = create_tool(metadata, path);
            tools.push(tool);
        }
    } else {
        // Branch command — recurse into subcommands
        for sub in &metadata.subcommands {
            if sub.hidden {
                continue;
            }
            let mut new_path = path.to_vec();
            new_path.push(&sub.name);
            collect_tools(sub, &new_path, tools);
        }
    }
}

fn create_tool(metadata: &CliCommandMetadata, path: &[&str]) -> Tool {
    let tool_name = path.join(".");

    let description = metadata
        .about
        .clone()
        .or_else(|| metadata.long_about.clone())
        .unwrap_or_else(|| format!("Golem CLI command: {}", path.join(" ")));

    let input_schema = build_input_schema(metadata);

    Tool {
        name: tool_name.into(),
        title: None,
        description: Some(description.into()),
        input_schema: Arc::new(input_schema),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    }
}

fn build_input_schema(metadata: &CliCommandMetadata) -> Map<String, Value> {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for arg in &metadata.args {
        // Skip global/hidden args
        if arg.is_global || arg.is_hidden {
            continue;
        }
        // Skip the help flag
        if arg.id == "help" || arg.id == "version" {
            continue;
        }

        let prop_name = arg_to_property_name(arg);
        let prop_schema = arg_to_json_schema(arg);
        properties.insert(prop_name.clone(), prop_schema);

        if arg.is_required {
            required.push(Value::String(prop_name));
        }
    }

    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }

    // Also add global flags as optional parameters
    let mut global_props = Map::new();
    add_global_flag_properties(&mut global_props);
    if !global_props.is_empty() {
        // Merge global props into properties
        if let Some(Value::Object(props)) = schema.get_mut("properties") {
            for (k, v) in global_props {
                props.entry(k).or_insert(v);
            }
        }
    }

    schema
}

fn arg_to_property_name(arg: &CliArgMetadata) -> String {
    // Use the arg id (snake_case) as the property name
    arg.id.clone()
}

fn arg_to_json_schema(arg: &CliArgMetadata) -> Value {
    let mut schema = Map::new();

    // Determine type based on action and possible values
    if !arg.possible_values.is_empty() {
        // Enum type
        schema.insert("type".into(), Value::String("string".into()));
        let enum_values: Vec<Value> = arg
            .possible_values
            .iter()
            .filter(|v| !v.hidden)
            .map(|v| Value::String(v.name.clone()))
            .collect();
        schema.insert("enum".into(), Value::Array(enum_values));
    } else if arg.action.contains("SetTrue") || arg.action.contains("SetFalse") {
        schema.insert("type".into(), Value::String("boolean".into()));
    } else if arg.action.contains("Count") {
        schema.insert("type".into(), Value::String("integer".into()));
    } else if is_array_action(&arg.action) || has_multiple_values(arg) {
        // Array type for repeated args
        schema.insert("type".into(), Value::String("array".into()));
        let mut items = Map::new();
        items.insert("type".into(), Value::String("string".into()));
        schema.insert("items".into(), Value::Object(items));
    } else {
        schema.insert("type".into(), Value::String("string".into()));
    }

    // Add description
    if let Some(ref help) = arg.help {
        schema.insert("description".into(), Value::String(help.clone()));
    } else if let Some(ref long_help) = arg.long_help {
        schema.insert("description".into(), Value::String(long_help.clone()));
    }

    // Add default values
    if !arg.default_values.is_empty() {
        if arg.default_values.len() == 1 {
            schema.insert(
                "default".into(),
                Value::String(arg.default_values[0].clone()),
            );
        }
    }

    Value::Object(schema)
}

fn is_array_action(action: &str) -> bool {
    action.contains("Append")
}

fn has_multiple_values(arg: &CliArgMetadata) -> bool {
    if let Some(ref num_args) = arg.num_args {
        // e.g. "1..=18446744073709551615" or "0..=1"
        num_args.contains("..") && !num_args.ends_with("=1")
    } else {
        false
    }
}

fn add_global_flag_properties(props: &mut Map<String, Value>) {
    // Add commonly used global flags as optional properties
    let global_flags = vec![
        (
            "format",
            "Output format (text, json, yaml)",
            vec!["text", "json", "yaml"],
        ),
        ("profile", "Golem profile name to use", vec![]),
        ("environment", "Golem environment name to use", vec![]),
    ];

    for (name, desc, possible_values) in global_flags {
        let mut schema = Map::new();
        schema.insert("type".into(), Value::String("string".into()));
        schema.insert("description".into(), Value::String(desc.into()));
        if !possible_values.is_empty() {
            let values: Vec<Value> = possible_values
                .into_iter()
                .map(|v| Value::String(v.into()))
                .collect();
            schema.insert("enum".into(), Value::Array(values));
        }
        props.insert(name.into(), Value::Object(schema));
    }
}

/// Reconstruct CLI arguments from MCP tool call parameters.
/// Takes the tool name (dot-separated path) and the JSON parameters,
/// and returns a Vec of command-line argument strings.
pub fn tool_call_to_cli_args(
    tool_name: &str,
    params: &Map<String, Value>,
    metadata: &CliCommandMetadata,
) -> Vec<String> {
    let mut args = Vec::new();

    // Convert tool name path to subcommand args
    // e.g. "component.new" -> ["component", "new"]
    let subcommands: Vec<&str> = tool_name.split('.').collect();
    args.extend(subcommands.iter().map(|s| s.to_string()));

    // Find the metadata for this specific command
    let cmd_metadata = find_command_metadata(metadata, &subcommands);

    if let Some(cmd) = cmd_metadata {
        // Convert parameters to CLI flags
        for (key, value) in params {
            // Check if this is a global flag
            if matches!(key.as_str(), "format" | "profile" | "environment") {
                if let Value::String(s) = value {
                    args.push(format!("--{}", key));
                    args.push(s.clone());
                }
                continue;
            }

            // Find the arg metadata for this parameter
            if let Some(arg_meta) = cmd.args.iter().find(|a| a.id == *key) {
                add_arg_to_cli_args(&mut args, arg_meta, value);
            }
        }
    }

    args
}

fn find_command_metadata<'a>(
    metadata: &'a CliCommandMetadata,
    path: &[&str],
) -> Option<&'a CliCommandMetadata> {
    if path.is_empty() {
        return Some(metadata);
    }

    for sub in &metadata.subcommands {
        if sub.name == path[0] {
            if path.len() == 1 {
                return Some(sub);
            }
            return find_command_metadata(sub, &path[1..]);
        }
    }

    None
}

fn add_arg_to_cli_args(args: &mut Vec<String>, arg_meta: &CliArgMetadata, value: &Value) {
    if arg_meta.is_positional {
        // Positional arguments
        match value {
            Value::String(s) => args.push(s.clone()),
            Value::Array(arr) => {
                for v in arr {
                    if let Value::String(s) = v {
                        args.push(s.clone());
                    }
                }
            }
            Value::Number(n) => args.push(n.to_string()),
            Value::Bool(b) => args.push(b.to_string()),
            _ => {}
        }
    } else {
        // Named arguments (flags)
        let flag_name = if let Some(long) = arg_meta.long.first() {
            format!("--{}", long)
        } else if let Some(short) = arg_meta.short.first() {
            format!("-{}", short)
        } else {
            format!("--{}", arg_meta.id)
        };

        match value {
            Value::Bool(true) => {
                args.push(flag_name);
            }
            Value::Bool(false) => {
                // Don't add flag for false
            }
            Value::String(s) => {
                args.push(flag_name);
                args.push(s.clone());
            }
            Value::Number(n) => {
                args.push(flag_name);
                args.push(n.to_string());
            }
            Value::Array(arr) => {
                for v in arr {
                    args.push(flag_name.clone());
                    match v {
                        Value::String(s) => args.push(s.clone()),
                        Value::Number(n) => args.push(n.to_string()),
                        _ => args.push(v.to_string()),
                    }
                }
            }
            _ => {
                args.push(flag_name);
                args.push(value.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::GolemCliCommand;

    #[test]
    fn test_cli_metadata_to_tools_produces_tools() {
        let metadata = GolemCliCommand::collect_metadata();
        let tools = cli_metadata_to_tools(&metadata);
        assert!(!tools.is_empty(), "Should produce at least one tool");

        // Check that we have common commands
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(
            tool_names.contains(&"build"),
            "Should have 'build' tool, got: {:?}",
            tool_names
        );
        assert!(
            tool_names.contains(&"deploy"),
            "Should have 'deploy' tool, got: {:?}",
            tool_names
        );
    }

    #[test]
    fn test_tool_call_to_cli_args() {
        let metadata = GolemCliCommand::collect_metadata();
        let mut params = Map::new();
        params.insert("format".into(), Value::String("json".into()));

        let args = tool_call_to_cli_args("build", &params, &metadata);
        assert!(args.contains(&"build".to_string()));
        assert!(args.contains(&"--format".to_string()));
        assert!(args.contains(&"json".to_string()));
    }
}
