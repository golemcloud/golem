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

use crate::mcp::schema::mcp_schema::McpSchema;
use golem_common::base_model::agent::DataSchema;
use golem_common::model::agent::ElementSchema;

pub fn get_input_mcp_schema(data_schema: &DataSchema) -> McpSchema {
    match data_schema {
        DataSchema::Tuple(schemas) => McpSchema::from_named_element_schemas(&schemas.elements),

        DataSchema::Multimodal(schemas) => {
            McpSchema::from_multimodal_element_schemas(&schemas.elements)
        }
    }
}

pub fn get_output_mcp_schema(data_schema: &DataSchema) -> Option<McpSchema> {
    match data_schema {
        DataSchema::Tuple(schemas) => {
            // This in reality will be just "{result_value: T}"
            if schemas.elements.len() == 1 {
                // If the output schema is structured (i.e. component model), we can represent it as MCP schema,
                // otherwise we will just return None and let clients handle it as unstructured output. This is also in accordance
                // with the MCP protocol, and probably the main reason why protocol says it's optional
                // Setting any schema for any unstructured content is either imposing bad user experience, or indeterministic results
                // Also says, `OutputSchema` is for structured output here: https://modelcontextprotocol.io/specification/2025-11-25/server/tools#output-schema
                // although optional
                let is_structured =
                    matches!(schemas.elements[0].schema, ElementSchema::ComponentModel(_));

                if is_structured {
                    Some(McpSchema::from_named_element_schemas(&schemas.elements))
                } else {
                    None
                }
            } else {
                None
            }
        }

        // This is decided after testing with several MCP clients, and the actual MCP protocol also considers mainly just `inputSchema`
        // If we set `outputSchema` (similar to input schema for multimodal), clients find it difficult to render the output
        // and behaves inconsistently. Example: If the output multimodal schema is represented using `rmcp::model::JsonObject` (tool output schema) instead of `None`,
        // then clients prefer to not render the image (and simply emit base64), or actual decode and fail due to large size b64, and at times succeeds,
        // or worse case, it can go in circles (it decodes to image, but finds the output schema to be b64 and decides to encode it again)
        // https://modelcontextprotocol.io/specification/2025-11-25/server/tools#listing-tools
        DataSchema::Multimodal(_) => None,
    }
}
// for mcp output schema
