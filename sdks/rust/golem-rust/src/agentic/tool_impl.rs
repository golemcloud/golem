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

//! Placeholder implementation of the `golem:tool/guest@0.1.0` export.
//!
//! Every agentic component exports `golem:tool/guest` alongside
//! `golem:agent/guest`, mirroring the agent guest surface. The tool runtime is
//! not implemented yet, so this component currently exposes no tools: discovery
//! returns an empty list and lookups/invocations report the tool as unknown.

use crate::agentic::agent_impl::Component;
use crate::golem_agentic::exports::golem::tool::guest::{
    Guest, InvocationResult, Tool, ToolError, TypedSchemaValue,
};
use crate::golem_agentic::golem::agent::common::Principal;
use crate::golem_agentic::wasi::io::streams::InputStream;

impl Guest for Component {
    fn discover_tools() -> Result<Vec<Tool>, ToolError> {
        Ok(Vec::new())
    }

    fn get_tool(name: String) -> Result<Tool, ToolError> {
        Err(ToolError::InvalidToolName(name))
    }

    fn invoke(
        tool_name: String,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<InputStream>,
        _principal: Principal,
    ) -> Result<InvocationResult, ToolError> {
        Err(ToolError::InvalidToolName(tool_name))
    }
}
