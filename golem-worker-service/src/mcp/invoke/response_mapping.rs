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

use base64::Engine;
use golem_common::base_model::agent::{
    BinaryReference, BinarySource, ElementValue, TextReference, TextSource,
    UnstructuredBinaryElementValue, UnstructuredTextElementValue,
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
            TextReference::Inline(TextSource { data, text_type }) => {
                let mut obj = serde_json::Map::new();
                obj.insert("data".to_string(), json!(data));
                if let Some(tt) = text_type {
                    obj.insert("languageCode".to_string(), json!(tt.language_code));
                }
                Ok(serde_json::Value::Object(obj))
            }
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
