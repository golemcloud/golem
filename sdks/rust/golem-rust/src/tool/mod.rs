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

#[cfg(not(any(
    feature = "export_golem_agentic",
    feature = "export_golem_tool_middleware"
)))]
pub use crate::bindings::golem::agent::common::Principal;
#[cfg(any(
    feature = "export_golem_agentic",
    feature = "export_golem_tool_middleware"
))]
pub use crate::golem_agentic::golem::agent::common::Principal;
pub use crate::schema::tool::Tool;
pub use tool_middleware::{
    InputStream, InvocationResult, MonomorphicToolMiddlewareScope, ToolInvokeError, ToolMiddleware,
    ToolMiddlewareScope, UnderlyingTool, decode_result_empty, decode_result_stdout_only,
    decode_result_value, decode_result_with_stdout,
};
#[doc(hidden)]
pub use tool_middleware::{ToolMiddlewareInvokeFuture, ToolMiddlewareInvokeFutureFor};
#[doc(hidden)]
pub use tool_middleware_registry::{
    ToolMiddlewareInvoker, get_all_tool_middlewares, get_tool_middleware_by_name,
    get_tool_middleware_invoker_by_name, register_tool_middleware,
};

pub(crate) use crate::schema::tool::wit::wire;

mod tool_middleware;
#[cfg(any(
    feature = "export_golem_tool_middleware",
    feature = "export_golem_agentic_tool_middleware"
))]
mod tool_middleware_impl;
mod tool_middleware_registry;

#[doc(hidden)]
pub trait ToolUnderlying: Sized {
    fn __golem_from_underlying(underlying: UnderlyingTool) -> Self;

    fn __golem_tool_descriptor() -> Tool;
}
