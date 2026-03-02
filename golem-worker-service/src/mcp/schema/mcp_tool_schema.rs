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

use crate::mcp::schema::mcp_schema::McpSchema;
use crate::mcp::schema::mcp_schema_mapping::get_mcp_schema;
use golem_common::base_model::agent::AgentMethod;

pub struct McpToolSchema {
    pub input_schema: McpSchema,
    pub output_schema: Option<McpSchema>,
}

pub fn get_mcp_tool_schema(method: &AgentMethod) -> McpToolSchema {
    let input_schema = get_mcp_schema(&method.input_schema);
    let output_schema = get_mcp_schema(&method.output_schema);

    McpToolSchema {
        input_schema,
        output_schema: Some(output_schema),
    }
}
