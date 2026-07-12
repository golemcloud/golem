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

use anyhow::{anyhow, bail};
use golem_common::base_model::Empty;
use golem_common::model::agent::{AgentMode, AgentTypeName, Snapshotting};
use golem_common::schema::agent::{
    AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField,
    OutputSchema,
};
use golem_common::schema::tool::canonical::{CanonicalInputField, CanonicalSurfaceRef};
use golem_common::schema::tool::{CommandIndex, CommandNode, Tool};
use heck::ToUpperCamelCase;

pub(crate) fn synthetic_agent_type(
    tool: &Tool,
    tool_name: &str,
) -> anyhow::Result<AgentTypeSchema> {
    let root_doc = tool
        .commands
        .nodes
        .first()
        .map(|n| n.doc.summary.clone())
        .unwrap_or_default();
    let mut methods = Vec::new();
    for (index, node) in tool.commands.nodes.iter().enumerate() {
        let Some(body) = &node.body else { continue };
        let path = command_path(tool, index)?;
        let method_name = if index == 0 {
            tool_name.to_string()
        } else {
            path[1..].join("-")
        };
        let fields = tool
            .canonical_input_fields(index)
            .into_iter()
            .map(|field| NamedField::user_supplied(field.name, field.type_));
        methods.push(AgentMethodSchema {
            name: method_name.clone(),
            description: node.doc.summary.clone(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(fields),
            output_schema: body
                .result
                .as_ref()
                .map(|result| OutputSchema::Single(Box::new(result.type_.clone())))
                .unwrap_or(OutputSchema::Unit),
            http_endpoint: vec![],
            read_only: None,
        });
        if !body.errors.is_empty() {
            methods.push(AgentMethodSchema {
                name: format!("{method_name}-errors"),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(body.errors.iter().filter_map(|case| {
                    case.payload
                        .clone()
                        .map(|payload| NamedField::user_supplied(case.name.clone(), payload))
                })),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![],
                read_only: None,
            });
        }
    }
    Ok(AgentTypeSchema {
        type_name: AgentTypeName(tool_name.to_upper_camel_case()),
        description: root_doc,
        source_language: String::new(),
        schema: tool.schema.clone(),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
        },
        methods,
        dependencies: vec![],
        mode: AgentMode::Ephemeral,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    })
}

pub(crate) fn global_surfaces(tool: &Tool, command_index: usize) -> Vec<CanonicalSurfaceRef> {
    tool.canonical_input_surfaces(command_index)
        .into_iter()
        .filter(|surface| {
            matches!(
                surface,
                CanonicalSurfaceRef::GlobalOption { .. } | CanonicalSurfaceRef::GlobalFlag { .. }
            )
        })
        .collect()
}

pub(crate) fn command_path(tool: &Tool, command_index: usize) -> anyhow::Result<Vec<String>> {
    fn visit(nodes: &[CommandNode], current: usize, target: usize, path: &mut Vec<String>) -> bool {
        path.push(nodes[current].name.clone());
        if current == target {
            return true;
        }
        for child in &nodes[current].subcommands {
            if let Some(child) = child.as_usize()
                && child < nodes.len()
                && visit(nodes, child, target, path)
            {
                return true;
            }
        }
        path.pop();
        false
    }
    let mut path = Vec::new();
    if !tool.commands.nodes.is_empty() && visit(&tool.commands.nodes, 0, command_index, &mut path) {
        Ok(path)
    } else {
        bail!("command node {command_index} is not reachable from root")
    }
}

pub(crate) fn idx_to_usize(index: CommandIndex) -> anyhow::Result<usize> {
    index
        .as_usize()
        .ok_or_else(|| anyhow!("negative command index {}", index.0))
}

pub(crate) fn field_names(field: &CanonicalInputField) -> Vec<String> {
    std::iter::once(field.name.clone())
        .chain(field.aliases.iter().cloned())
        .collect()
}
