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

//! Stub host implementation of `golem:tool/host@0.1.0`.
//!
//! The interface is wired into the linker for every component shape so that any
//! agent or tool component may import it and instantiate. No runtime behavior is
//! implemented yet: discovery and invocation operations return a
//! not-implemented error. The tool registry, the (agent, tool) instance model,
//! and the middleware chain are layered on in later steps.

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::InputStream;
use crate::preview2::golem::tool::host::{
    Host, HostFutureInvokeResult, HostToolRpc, InvocationResult, RegisteredTool, RpcError,
    TypedSchemaValue,
};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::io::poll::Pollable;

const NOT_IMPLEMENTED: &str = "golem:tool/host is not yet implemented";

/// Host-side resource table entry backing the `golem:tool/host.tool-rpc`
/// resource. A placeholder until the tool runtime is implemented.
pub struct ToolRpcEntry;

/// Host-side resource table entry backing the
/// `golem:tool/host.future-invoke-result` resource. A placeholder until the tool
/// runtime is implemented.
pub struct FutureInvokeResultEntry;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_tools(&mut self) -> anyhow::Result<Vec<RegisteredTool>> {
        self.observe_function_call("golem::tool::host", "get-all-tools");
        Ok(Vec::new())
    }

    async fn get_tool(&mut self, _name: String) -> anyhow::Result<Option<RegisteredTool>> {
        self.observe_function_call("golem::tool::host", "get-tool");
        Ok(None)
    }
}

impl<Ctx: WorkerCtx> HostToolRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, _tool_name: String) -> anyhow::Result<Resource<ToolRpcEntry>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "new");
        Err(anyhow!(NOT_IMPLEMENTED))
    }

    async fn invoke_and_await(
        &mut self,
        _self_: Resource<ToolRpcEntry>,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<Resource<InputStream>>,
    ) -> anyhow::Result<Result<InvocationResult, RpcError>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "invoke-and-await");
        Ok(Err(RpcError::RemoteInternalError(
            NOT_IMPLEMENTED.to_string(),
        )))
    }

    async fn invoke(
        &mut self,
        _self_: Resource<ToolRpcEntry>,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<Resource<InputStream>>,
    ) -> anyhow::Result<Result<(), RpcError>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "invoke");
        Ok(Err(RpcError::RemoteInternalError(
            NOT_IMPLEMENTED.to_string(),
        )))
    }

    async fn async_invoke_and_await(
        &mut self,
        _self_: Resource<ToolRpcEntry>,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<Resource<InputStream>>,
    ) -> anyhow::Result<Resource<FutureInvokeResultEntry>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "async-invoke-and-await");
        Err(anyhow!(NOT_IMPLEMENTED))
    }

    async fn drop(&mut self, rep: Resource<ToolRpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::tool-rpc", "drop");
        let _ = self.table().delete(rep);
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn subscribe(
        &mut self,
        _self_: Resource<FutureInvokeResultEntry>,
    ) -> anyhow::Result<Resource<Pollable>> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "subscribe");
        Err(anyhow!(NOT_IMPLEMENTED))
    }

    async fn get(
        &mut self,
        _self_: Resource<FutureInvokeResultEntry>,
    ) -> anyhow::Result<Option<Result<InvocationResult, RpcError>>> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "get");
        Ok(Some(Err(RpcError::RemoteInternalError(
            NOT_IMPLEMENTED.to_string(),
        ))))
    }

    async fn cancel(&mut self, _self_: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "cancel");
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "drop");
        let _ = self.table().delete(rep);
        Ok(())
    }
}
