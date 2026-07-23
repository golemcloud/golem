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

pub use crate::golem_agentic::golem::agent::common::Principal;
pub type InputStream = wit_bindgen::StreamReader<u8>;
pub use agent::*;
pub use agent_config::*;
pub use agent_initiator::*;
pub use agent_registry::*;
pub use async_utils::*;
pub use errors::*;
pub use extended_agent_type::*;
pub use extended_tool_type::*;
pub use http::*;
pub use multimodal::*;
pub use resolved_agent::*;
pub use schema::*;
pub use tool_client::*;
pub use tool_impl::{OutputStream, new_tool_stdout};
pub use tool_literal::*;
pub use tool_refinement::*;
pub use tool_registry::{
    ToolInvokeFuture, ToolInvoker, get_all_tools, get_extended_tool_by_name, get_tool_by_name,
    get_tool_invoker_by_name, register_tool, register_tool_invoker,
};
pub use unstructured_binary::*;
pub use unstructured_text::*;
pub use webhook::*;
pub use wit_bindgen::spawn_local;

#[derive(Debug)]
pub struct EphemeralInvocationResult<T> {
    pub metadata: crate::golem_agentic::golem::agent::host::InvocationMetadata,
    pub value: T,
}

mod agent;
mod agent_config;
mod agent_impl;
mod agent_initiator;
mod agent_registry;
mod async_utils;
mod errors;
mod extended_agent_type;
mod extended_tool_type;
mod http;
mod multimodal;
mod principal_serde;
mod resolved_agent;
mod schema;
pub mod snapshot_auto;
mod tool_client;
mod tool_impl;
mod tool_literal;
mod tool_refinement;
mod tool_registry;
mod unstructured_binary;
mod unstructured_text;
mod webhook;
