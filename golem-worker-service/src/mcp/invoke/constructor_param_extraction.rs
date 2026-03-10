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
