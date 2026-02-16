// Copyright 2024-2025 Golem Cloud
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
pub use agent::*;
pub use agent_initiator::*;
pub use agent_registry::*;
pub use async_utils::*;
pub use errors::*;
pub use extended_agent_type::*;
pub use golem_wasm::agentic::unstructured_binary::*;
pub use golem_wasm::agentic::unstructured_text::*;
pub use http::*;
pub use multimodal::*;
pub use resolved_agent::*;
pub use schema::*;
pub use webhook::*;
pub use wit_utils::*;

mod agent;
mod agent_impl;
mod agent_initiator;
mod agent_registry;
mod async_utils;
mod errors;
mod extended_agent_type;
mod http;
mod multimodal;
mod principal_serde;
mod resolved_agent;
mod schema;
mod unstructured_binary;
mod unstructured_text;
mod webhook;
mod wit_utils;
