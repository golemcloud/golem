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

use golem_common::base_model::agent::{
    AgentMethod, ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
};
use golem_wasm::analysis::AnalysedType;
use rmcp::model::JsonObject;
use serde_json::{Map, Value, json};

pub trait GetMcpSchema {
    fn get_mcp_schema(&self) -> JsonObject;
}

impl GetMcpSchema for DataSchema {
    fn get_mcp_schema(&self) -> JsonObject {
        match self {
            DataSchema::Tuple(schemas) => {
                let properties = get_json_schema(&schemas.elements);
                json!({
                    "type": "object",
                    "properties": properties,
                })
                .as_object()
                .unwrap()
                .clone()
            }
            DataSchema::Multimodal(_) => {
                todo!("Multimodal schema is not supported in this example")
            }
        }
    }
}

pub trait GetMcpToolSchema {
    fn get_mcp_tool_schema(&self) -> McpToolSchema;
}

pub struct McpToolSchema {
    pub input_schema: JsonObject,
    pub output_schema: Option<JsonObject>,
}

impl McpToolSchema {
    pub fn merge_input_schema(&mut self, input_schema: JsonObject) {
        let mut new_properties = input_schema
            .get("properties")
            .and_then(|props| props.as_object())
            .cloned()
            .unwrap_or_default();

        if let Some(existing_properties) = self
            .input_schema
            .get("properties")
            .and_then(|props| props.as_object())
        {
            for (key, value) in existing_properties {
                new_properties.insert(key.clone(), value.clone());
            }
        }

        self.input_schema = json!({
            "type": "object",
            "properties": new_properties,
        })
        .as_object()
        .unwrap()
        .clone();
    }
}

impl GetMcpToolSchema for AgentMethod {
    fn get_mcp_tool_schema(&self) -> McpToolSchema {
        let input_schema: JsonObject = self.input_schema.get_mcp_schema();
        let output_schema: JsonObject = self.output_schema.get_mcp_schema();

        McpToolSchema {
            input_schema,
            output_schema: Some(output_schema),
        }
    }
}

fn get_json_schema(schemas: &Vec<NamedElementSchema>) -> Map<String, Value> {
    let mut properties = Map::new();

    for NamedElementSchema { name, schema } in schemas {
        let json_schema = match schema {
            ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
                match element_type {
                    AnalysedType::Str(_) => json!({"type": "string"}),
                    AnalysedType::U32(_) => json!({"type": "integer"}),
                    AnalysedType::Bool(_) => json!({"type": "boolean"}),
                    _ => {
                        todo!("Unsupported component model element type in schema mapping")
                    }
                }
            }
            _ => todo!("Unsupported component model element type in schema mapping"),
        };
        properties.insert(name.clone(), json_schema);
    }

    properties
}
