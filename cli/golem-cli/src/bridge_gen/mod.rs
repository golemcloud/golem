// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Bridge SDK generators for the Rust and TypeScript client targets.
//!
//! The internal walkers in [`rust`] and [`typescript`] operate on the
//! schema layer ([`golem_common::schema::SchemaType`] /
//! [`golem_common::schema::graph::SchemaGraph`]) — [`type_naming::TypeNaming`]
//! keys generated names by [`SchemaType`](golem_common::schema::schema_type::SchemaType)
//! structural identity, with named legacy composites carried as
//! [`SchemaType::Ref`](golem_common::schema::schema_type::SchemaType::Ref)
//! against a shared graph imported via
//! [`analysed_type_to_schema_graph`](golem_common::schema::adapters::analysed_type::analysed_type_to_schema_graph).
//!
//! The public entry point ([`BridgeGenerator::new`]) still takes a legacy
//! [`AgentType`] from the agent declaration; the generator converts each
//! reachable [`AnalysedType`](golem_wasm::analysis::AnalysedType) into a
//! [`SchemaType`](golem_common::schema::schema_type::SchemaType) at the call
//! site of the walker. The string templates the generators emit continue to
//! reference the legacy `golem_wasm::analysis::AnalysedType` /
//! `golem_wasm::Value` surface plus the `IntoValue` / `FromValue` traits —
//! that is the SDK contract this generator targets. When the walker needs to
//! embed a legacy `AnalysedType` literal it projects the schema body back
//! via [`schema_type_to_analysed_type`](golem_common::schema::adapters::analysed_type::schema_type_to_analysed_type)
//! at the emission point.

pub mod parameter_naming;
pub mod rust;
pub mod type_naming;
pub mod typescript;

use camino::Utf8Path;
use golem_common::model::agent::{AgentType, AgentTypeName};
use heck::ToKebabCase;

pub trait BridgeGenerator {
    fn new(agent_type: AgentType, target_path: &Utf8Path, testing: bool) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn generate(&mut self) -> anyhow::Result<()>;
}

pub fn bridge_client_directory_name(agent_type_name: &AgentTypeName) -> String {
    format!("{}-client", agent_type_name.as_str().to_kebab_case())
}
