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

pub fn get_mcp_schema(data_schema: &DataSchema) -> McpSchema {
    match data_schema {
        DataSchema::Tuple(schemas) => McpSchema::from_named_element_schemas(&schemas.elements),

        DataSchema::Multimodal(schemas) => {
            McpSchema::from_multimodal_element_schemas(&schemas.elements)
        }
    }
}
