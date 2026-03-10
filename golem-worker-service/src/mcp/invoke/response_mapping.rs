use base64::Engine;
use golem_common::base_model::agent::{
    BinaryReference, BinarySource, ElementValue,
    TextReference, TextSource, UnstructuredBinaryElementValue,
    UnstructuredTextElementValue,
};
use golem_wasm::json::ValueAndTypeJsonExtensions;
use rmcp::ErrorData;
use serde_json::json;

// Mapping from ElementValue to the JSON format expected by MCP clients 
// (based on the schema they learned from initialization)
// This is used for both resources and tools.
pub fn element_value_to_mcp_json(element: &ElementValue) -> Result<serde_json::Value, ErrorData> {
    match element {
        ElementValue::ComponentModel(component_model_value) => {
            component_model_value.value.to_json_value().map_err(|e| {
                ErrorData::internal_error(
                    format!("Failed to serialize component model response: {e}"),
                    None,
                )
            })
        }
        ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => match value {
            TextReference::Inline(TextSource { data, .. }) => Ok(json!({ "data": data })),
            TextReference::Url(url) => Ok(json!({ "data": url.value })),
        },
        ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
            match value {
                BinaryReference::Inline(BinarySource { data, binary_type }) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
                    Ok(json!({
                        "data": b64,
                        "mimeType": binary_type.mime_type,
                    }))
                }
                BinaryReference::Url(url) => Ok(json!({ "data": url.value })),
            }
        }
    }
}
