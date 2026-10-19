// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::golem_agentic::exports::golem::tool::guest::ToolError;
use crate::golem_agentic::golem::agent::common::AgentError;
use crate::schema::IntoTypedSchemaValue;
use crate::schema::wit::wire;

pub fn custom_error(msg: impl ToString) -> AgentError {
    AgentError::CustomError(wire::TypedSchemaValue {
        graph: wire::SchemaGraph {
            type_nodes: vec![wire::SchemaTypeNode {
                body: wire::SchemaTypeBody::StringType,
                metadata: wire::MetadataEnvelope {
                    doc: None,
                    aliases: vec![],
                    examples: vec![],
                    deprecated: None,
                    role: None,
                },
            }],
            defs: vec![],
            root: 0,
        },
        value: wire::SchemaValueTree {
            value_nodes: vec![wire::SchemaValueNode::StringValue(msg.to_string())],
            root: 0,
        },
    })
}

pub fn internal_error(msg: impl ToString) -> AgentError {
    custom_error(format!("Internal error: {}", msg.to_string()))
}

pub fn invalid_input_error(msg: impl ToString) -> AgentError {
    AgentError::InvalidInput(msg.to_string())
}

pub fn invalid_method_error(method_name: impl ToString) -> AgentError {
    AgentError::InvalidMethod(method_name.to_string())
}

pub fn invalid_tool_name(tool_name: impl ToString) -> ToolError {
    ToolError::InvalidToolName(tool_name.to_string())
}

pub fn invalid_command_path(path: Vec<String>) -> ToolError {
    ToolError::InvalidCommandPath(path)
}

pub fn invalid_input(msg: impl ToString) -> ToolError {
    ToolError::InvalidInput(msg.to_string())
}

pub fn constraint_violation(msg: impl ToString) -> ToolError {
    ToolError::ConstraintViolation(msg.to_string())
}

pub fn invalid_result(msg: impl ToString) -> ToolError {
    ToolError::InvalidResult(msg.to_string())
}

pub fn custom_tool_error<T: IntoTypedSchemaValue>(name: impl Into<String>, value: T) -> ToolError {
    let typed = value
        .into_typed_schema_value()
        .expect("failed to encode custom tool error");
    ToolError::CustomError(crate::schema::wit::wire::CustomToolError {
        name: name.into(),
        payload: crate::encode_typed_schema_value(&typed)
            .expect("failed to encode custom tool error"),
    })
}

#[cfg(test)]
mod tests {
    use super::{AgentError, custom_error, internal_error, wire};
    use test_r::test;

    #[test]
    fn string_agent_errors_have_a_self_contained_wire_graph() {
        for (error, expected) in [
            (custom_error("error: árvíz 🦀"), "error: árvíz 🦀"),
            (internal_error(173), "Internal error: 173"),
            (custom_error(""), ""),
        ] {
            let AgentError::CustomError(typed) = error else {
                panic!("expected custom error");
            };
            assert_eq!(typed.graph.root, 0);
            assert!(typed.graph.defs.is_empty());
            let [node] = typed.graph.type_nodes.as_slice() else {
                panic!("expected one type node");
            };
            assert!(matches!(node.body, wire::SchemaTypeBody::StringType));
            assert!(node.metadata.doc.is_none());
            assert!(node.metadata.aliases.is_empty());
            assert!(node.metadata.examples.is_empty());
            assert!(node.metadata.deprecated.is_none());
            assert!(node.metadata.role.is_none());
            assert_eq!(typed.value.root, 0);
            assert!(matches!(
                typed.value.value_nodes.as_slice(),
                [wire::SchemaValueNode::StringValue(value)] if value == expected
            ));
        }
    }
}
