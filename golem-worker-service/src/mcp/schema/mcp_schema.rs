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

use crate::mcp::schema::internal::McpSchemaInternal;
use golem_common::base_model::agent::DataSchema;
use rmcp::model::JsonObject;

pub type McpSchema = JsonObject;

pub trait GetMcpSchema {
    fn get_mcp_schema(&self) -> McpSchemaInternal;
}

impl GetMcpSchema for DataSchema {
    fn get_mcp_schema(&self) -> McpSchemaInternal {
        match self {
            DataSchema::Tuple(schemas) => {
                McpSchemaInternal::from_named_element_schemas(&schemas.elements)
            }
            DataSchema::Multimodal(_) => {
                todo!("Multimodal schema is not supported in this example")
            }
        }
    }
}
