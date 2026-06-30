// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agentic::agent_impl::Component;
use crate::agentic::tool_registry::{get_all_tools, get_tool_by_name};
use crate::golem_agentic::exports::golem::tool::guest::{
    Guest, InvocationResult, Tool, ToolError, TypedSchemaValue,
};
use crate::golem_agentic::golem::agent::common::Principal;
use crate::wasip2::io::streams::InputStream;

impl Guest for Component {
    fn discover_tools() -> Result<Vec<Tool>, ToolError> {
        Ok(get_all_tools())
    }

    fn get_tool(name: String) -> Result<Tool, ToolError> {
        get_tool_by_name(&name).ok_or(ToolError::InvalidToolName(name))
    }

    fn invoke(
        tool_name: String,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<InputStream>,
        _principal: Principal,
    ) -> Result<InvocationResult, ToolError> {
        // Tool invocation dispatch is not wired yet; metadata discovery is available.
        Err(ToolError::InvalidToolName(tool_name))
    }
}
