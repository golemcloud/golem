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

use rmcp::model::JsonObject;
use serde_json::json;
use golem_common::base_model::agent::{AgentMethod, ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema};
use golem_wasm::analysis::AnalysedType;

pub trait McpToolSchemaMapper {
    fn get_schema(&self) -> McpToolSchema;
}

pub struct McpToolSchema {
    pub input_schema: JsonObject,
    pub output_schema: Option<JsonObject>,
}

impl McpToolSchemaMapper for AgentMethod {
    fn get_schema(&self) -> McpToolSchema {
        let input_schema: JsonObject = get_mcp_tool_schema(&self.input_schema);
        let output_schema: JsonObject = get_mcp_tool_schema(&self.output_schema);


        McpToolSchema {
            input_schema,
            output_schema: Some(output_schema),
        }
    }
}

fn get_mcp_tool_schema(data_schema: &DataSchema) -> JsonObject {
    let mut properties = serde_json::Map::new();

    match data_schema {
        DataSchema::Tuple(element_schemas) => {
            for NamedElementSchema { name, schema } in &element_schemas.elements {
                // For simplicity, we treat schema as a string describing the type
                // In a real implementation, this would be more complex and handle nested structures
                let json_schema = match schema {
                    ElementSchema::ComponentModel(ComponentModelElementSchema {element_type}) =>
                        match element_type {
                            AnalysedType::Str(_) => json!({"type": "string"}),
                            AnalysedType::U32(_) => json!({"type": "integer"}),
                            AnalysedType::Bool(_) => json!({"type": "boolean"}),
                            _ => todo!("Unsupported component model element type in schema mapping"),
                        }
                    _ => todo!("Unsupported component model element type in schema mapping"),
                };
                properties.insert(name.clone(), json_schema);
            }
        }
        DataSchema::Multimodal(_) => {}
    }

    json!({
            "type": "object",
            "properties": properties,
        })
        .as_object()
        .unwrap()
        .clone()

}