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

use crate::agentic::InputStream;
#[cfg(feature = "export_golem_agentic")]
use crate::agentic::agent_impl::Component;
#[cfg(feature = "export_golem_agentic")]
use crate::agentic::tool_registry::{get_all_tools, get_tool_by_name, get_tool_invoker_by_name};
#[cfg(feature = "export_golem_agentic")]
use crate::golem_agentic::exports::golem::tool::guest::{
    Guest, InvocationResult, Tool, ToolError, TypedSchemaValue,
};
#[cfg(feature = "export_golem_agentic")]
use crate::golem_agentic::golem::agent::common::Principal;
use crate::golem_agentic::golem::tool::host::ToolStdoutWriter;
use crate::golem_agentic::golem::tool::host::{ByteStreamFailure, StreamWriteError};
use std::cell::RefCell;
use std::rc::Rc;

/// Writable stdout passed to tool implementations.
///
pub struct OutputStream {
    writer: Rc<RefCell<Option<ToolStdoutWriter>>>,
}

impl Clone for OutputStream {
    fn clone(&self) -> Self {
        Self {
            writer: Rc::clone(&self.writer),
        }
    }
}

impl OutputStream {
    #[doc(hidden)]
    pub fn new(writer: ToolStdoutWriter) -> Self {
        Self {
            writer: Rc::new(RefCell::new(Some(writer))),
        }
    }

    /// Writes a non-empty chunk directly to the host-owned output attachment.
    pub async fn write(&mut self, bytes: Vec<u8>) -> Result<(), StreamWriteError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let Some(writer) = self.writer.borrow_mut().take() else {
            return Err(StreamWriteError::ConcurrentOperation);
        };
        let result = writer.write(bytes).await;
        *self.writer.borrow_mut() = Some(writer);
        result
    }

    pub async fn write_all(&mut self, bytes: Vec<u8>) -> Result<(), StreamWriteError> {
        self.write(bytes).await
    }

    pub async fn write_one(&mut self, byte: u8) -> Result<(), StreamWriteError> {
        self.write(vec![byte]).await
    }

    pub async fn finish(self) -> Result<(), StreamWriteError> {
        let Some(writer) = self.writer.borrow_mut().take() else {
            return Err(StreamWriteError::ConcurrentOperation);
        };
        writer.finish().await
    }

    pub async fn fail(self, reason: ByteStreamFailure) -> Result<(), StreamWriteError> {
        let Some(writer) = self.writer.borrow_mut().take() else {
            return Err(StreamWriteError::ConcurrentOperation);
        };
        writer.fail(reason).await
    }
}

impl Guest for Component {
    fn discover_tools() -> Result<Vec<Tool>, ToolError> {
        Ok(get_all_tools())
    }

    fn get_tool(name: String) -> Result<Tool, ToolError> {
        get_tool_by_name(&name).ok_or(ToolError::InvalidToolName(name))
    }

    async fn invoke(
        tool_name: String,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
        stdout: Option<ToolStdoutWriter>,
        principal: Principal,
    ) -> Result<InvocationResult, ToolError> {
        let invoker = get_tool_invoker_by_name(&tool_name)
            .ok_or_else(|| ToolError::InvalidToolName(tool_name.clone()))?;
        invoker(command_path, input, stdin, stdout, principal).await
    }
}
