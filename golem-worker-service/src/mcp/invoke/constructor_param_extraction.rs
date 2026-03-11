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

use golem_common::base_model::agent::{
    ComponentModelElementSchema, ComponentModelElementValue, DataSchema, ElementSchema,
    NamedElementSchema,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use rmcp::model::JsonObject;

pub fn extract_constructor_input_values(
    args_map: &JsonObject,
    schema: &DataSchema,
) -> Result<Vec<ComponentModelElementValue>, String> {
    match schema {
        DataSchema::Tuple(named_schemas) => {
            let mut params = Vec::new();

            for NamedElementSchema {
                name,
                schema: elem_schema,
            } in &named_schemas.elements
            {
                match elem_schema {
                    ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
                        let json_value = match args_map.get(name) {
                            Some(value) => value.clone(),
                            None => {
                                if matches!(element_type, AnalysedType::Option(_)) {
                                    serde_json::Value::Null
                                } else {
                                    return Err(format!("Missing parameter: {}", name));
                                }
                            }
                        };

                        let value_and_type =
                            golem_wasm::ValueAndType::parse_with_type(&json_value, element_type)
                                .map_err(|errs| {
                                    format!(
                                        "Failed to parse parameter '{}': {}",
                                        name,
                                        errs.join(", ")
                                    )
                                })?;

                        params.push(ComponentModelElementValue {
                            value: value_and_type,
                        });
                    }
                    ElementSchema::UnstructuredText(_) => {
                        return Err(format!(
                            "MCP cannot support unstructured-text constructor parameters like '{}'",
                            name
                        ));
                    }

                    ElementSchema::UnstructuredBinary(_) => {
                        return Err(format!(
                            "MCP cannot support unstructured-binary constructor parameters like '{}'",
                            name
                        ));
                    }
                }
            }

            Ok(params)
        }
        DataSchema::Multimodal(_) => {
            Err("MCP does not support multimodal constructor schemas".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::agent::{NamedElementSchemas, TextDescriptor};
    use golem_wasm::analysis::analysed_type::{str, u32};
    use serde_json::json;
    use test_r::test;

    fn string_schema(name: &str) -> NamedElementSchema {
        NamedElementSchema {
            name: name.to_string(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: str(),
            }),
        }
    }

    fn u32_schema(name: &str) -> NamedElementSchema {
        NamedElementSchema {
            name: name.to_string(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: u32(),
            }),
        }
    }

    #[test]
    fn extracts_string_param() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![string_schema("name")],
        });
        let args: JsonObject = json!({"name": "alice"}).as_object().unwrap().clone();
        let result = extract_constructor_input_values(&args, &schema).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn extracts_multiple_params() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![string_schema("name"), u32_schema("age")],
        });
        let args: JsonObject = json!({"name": "alice", "age": 30})
            .as_object()
            .unwrap()
            .clone();
        let result = extract_constructor_input_values(&args, &schema).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn error_on_missing_required_param() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![string_schema("name")],
        });
        let args: JsonObject = json!({}).as_object().unwrap().clone();
        let err = extract_constructor_input_values(&args, &schema).unwrap_err();
        assert!(err.contains("Missing parameter: name"), "got: {err}");
    }

    #[test]
    fn rejects_unstructured_text_constructor() {
        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "desc".to_string(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            }],
        });
        let args: JsonObject = json!({"desc": "hello"}).as_object().unwrap().clone();
        let err = extract_constructor_input_values(&args, &schema).unwrap_err();
        assert!(err.contains("unstructured-text"), "got: {err}");
    }

    #[test]
    fn rejects_multimodal_schema() {
        let schema = DataSchema::Multimodal(NamedElementSchemas { elements: vec![] });
        let args: JsonObject = json!({}).as_object().unwrap().clone();
        let err = extract_constructor_input_values(&args, &schema).unwrap_err();
        assert!(err.contains("multimodal"), "got: {err}");
    }
}
