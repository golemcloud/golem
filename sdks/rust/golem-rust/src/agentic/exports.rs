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

//! Mandatory guest exports must not retain uninstalled capability runtimes.
//! Only implementation-macro constructors install these tables. In particular,
//! never initialize a table with runtime defaults: its function pointers would
//! keep that runtime alive even when no implementation is linked.

use super::InputStream;
use crate::golem_agentic::exports::golem::tool::tool_middleware_guest as middleware;
use crate::golem_agentic::exports::golem::{agent::guest as agent, tool::guest as tool};
use crate::golem_agentic::golem::agent::common::Principal;
use crate::golem_agentic::golem::tool::streams::ToolStdoutWriter;
use crate::load_snapshot::exports::golem::api::load_snapshot as load;
use crate::save_snapshot::exports::golem::api::save_snapshot as save;
use crate::schema::wit::wire::{SchemaValueTree, TypedSchemaValue};
use crate::tool_underlying_bindings::UnderlyingTool;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

pub(crate) type ExportFuture<T> = Pin<Box<dyn Future<Output = T>>>;

#[allow(clippy::type_complexity)]
pub(crate) struct AgentHooks {
    pub initialize:
        fn(String, SchemaValueTree, Principal) -> ExportFuture<Result<(), agent::AgentError>>,
    pub invoke: fn(
        String,
        SchemaValueTree,
        Principal,
    ) -> ExportFuture<Result<Option<SchemaValueTree>, agent::AgentError>>,
    pub get_definition: fn() -> agent::AgentType,
    pub discover: fn() -> Result<Vec<agent::AgentType>, agent::AgentError>,
    pub load: fn(load::Snapshot) -> ExportFuture<Result<(), String>>,
    pub save: fn() -> ExportFuture<save::Snapshot>,
}

#[allow(clippy::type_complexity)]
pub(crate) struct ToolHooks {
    pub discover: fn() -> Result<Vec<tool::Tool>, tool::ToolError>,
    pub get: fn(String) -> Result<tool::Tool, tool::ToolError>,
    pub invoke: fn(
        String,
        Vec<String>,
        TypedSchemaValue,
        Option<InputStream>,
        Option<ToolStdoutWriter>,
        Principal,
    ) -> ExportFuture<Result<tool::InvocationResult, tool::ToolError>>,
}

#[allow(clippy::type_complexity)]
pub(crate) struct MiddlewareHooks {
    pub discover: fn() -> Result<Vec<middleware::ToolMiddleware>, tool::ToolError>,
    pub get: fn(String) -> Result<middleware::ToolMiddleware, tool::ToolError>,
    pub invoke: fn(
        String,
        String,
        tool::Tool,
        TypedSchemaValue,
        Vec<String>,
        TypedSchemaValue,
        Option<InputStream>,
        Option<ToolStdoutWriter>,
        Principal,
        UnderlyingTool,
    ) -> ExportFuture<Result<tool::InvocationResult, tool::ToolError>>,
}

pub(crate) static AGENT: OnceLock<AgentHooks> = OnceLock::new();
pub(crate) static TOOL: OnceLock<ToolHooks> = OnceLock::new();
pub(crate) static MIDDLEWARE: OnceLock<MiddlewareHooks> = OnceLock::new();

#[doc(hidden)]
pub struct Component;

impl agent::Guest for Component {
    async fn initialize(
        agent_type: String,
        input: SchemaValueTree,
        principal: Principal,
    ) -> Result<(), agent::AgentError> {
        match AGENT.get() {
            Some(hooks) => (hooks.initialize)(agent_type, input, principal).await,
            None => Err(agent::AgentError::InvalidInput(
                "component has no agent implementation".into(),
            )),
        }
    }

    async fn invoke(
        method: String,
        input: SchemaValueTree,
        principal: Principal,
    ) -> Result<Option<SchemaValueTree>, agent::AgentError> {
        match AGENT.get() {
            Some(hooks) => (hooks.invoke)(method, input, principal).await,
            None => Err(agent::AgentError::InvalidInput(
                "component has no agent implementation".into(),
            )),
        }
    }

    fn get_definition() -> agent::AgentType {
        (AGENT
            .get()
            .expect("component has no agent implementation")
            .get_definition)()
    }

    fn discover_agent_types() -> Result<Vec<agent::AgentType>, agent::AgentError> {
        match AGENT.get() {
            Some(hooks) => (hooks.discover)(),
            None => Ok(Vec::new()),
        }
    }
}

impl load::Guest for Component {
    async fn load(snapshot: load::Snapshot) -> Result<(), String> {
        match AGENT.get() {
            Some(hooks) => (hooks.load)(snapshot).await,
            None => Err("component has no agent snapshot support".into()),
        }
    }
}

impl save::Guest for Component {
    async fn save() -> save::Snapshot {
        (AGENT
            .get()
            .expect("component has no agent snapshot support")
            .save)()
        .await
    }
}

impl tool::Guest for Component {
    fn discover_tools() -> Result<Vec<tool::Tool>, tool::ToolError> {
        match TOOL.get() {
            Some(hooks) => (hooks.discover)(),
            None => Ok(Vec::new()),
        }
    }

    fn get_tool(name: String) -> Result<tool::Tool, tool::ToolError> {
        match TOOL.get() {
            Some(hooks) => (hooks.get)(name),
            None => Err(tool::ToolError::InvalidToolName(name)),
        }
    }

    async fn invoke(
        name: String,
        path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
        stdout: Option<ToolStdoutWriter>,
        principal: Principal,
    ) -> Result<tool::InvocationResult, tool::ToolError> {
        match TOOL.get() {
            Some(hooks) => (hooks.invoke)(name, path, input, stdin, stdout, principal).await,
            None => Err(tool::ToolError::InvalidToolName(name)),
        }
    }
}

impl middleware::Guest for Component {
    fn discover_tool_middlewares() -> Result<Vec<middleware::ToolMiddleware>, tool::ToolError> {
        match MIDDLEWARE.get() {
            Some(hooks) => (hooks.discover)(),
            None => Ok(Vec::new()),
        }
    }

    fn get_tool_middleware(name: String) -> Result<middleware::ToolMiddleware, tool::ToolError> {
        match MIDDLEWARE.get() {
            Some(hooks) => (hooks.get)(name),
            None => Err(tool::ToolError::InvalidToolName(name)),
        }
    }

    async fn invoke_tool_middleware(
        name: String,
        tool_name: String,
        metadata: tool::Tool,
        parameters: TypedSchemaValue,
        path: Vec<String>,
        input: TypedSchemaValue,
        stdin: Option<InputStream>,
        stdout: Option<ToolStdoutWriter>,
        principal: Principal,
        wrapped: UnderlyingTool,
    ) -> Result<tool::InvocationResult, tool::ToolError> {
        match MIDDLEWARE.get() {
            Some(hooks) => {
                (hooks.invoke)(
                    name, tool_name, metadata, parameters, path, input, stdin, stdout, principal,
                    wrapped,
                )
                .await
            }
            None => Err(tool::ToolError::InvalidToolName(name)),
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) mod raw;
