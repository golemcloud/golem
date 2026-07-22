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
//! The internal walkers in [`rust`] and [`typescript`] operate directly on
//! the schema layer ([`golem_common::schema::SchemaType`] /
//! [`golem_common::schema::graph::SchemaGraph`]). The public entry point
//! ([`BridgeGenerator::new`]) takes a schema-native
//! [`AgentTypeSchema`](golem_common::schema::AgentTypeSchema); the agent's own
//! [`SchemaGraph`](golem_common::schema::graph::SchemaGraph) is adopted as the
//! ref-resolution graph and [`type_naming::TypeNaming`] keys generated names by
//! [`SchemaType`](golem_common::schema::schema_type::SchemaType) structural
//! identity. The generators emit schema-native `SchemaValue` (`{kind,value}`)
//! encode/decode code; there is no longer any dependency on the legacy
//! `AnalysedType` / `IntoValue` / `FromValue` surface.

pub mod moonbit;
pub mod parameter_naming;
pub mod rust;
pub mod scala;
pub mod tool_common;
pub mod type_naming;
pub mod typescript;

use camino::Utf8Path;
use golem_common::model::agent::AgentTypeName;
use golem_common::schema::AgentTypeSchema;
use heck::ToKebabCase;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BridgeMode {
    External,
    Guest,
}

impl BridgeMode {
    pub fn id(&self) -> &'static str {
        match self {
            BridgeMode::External => "external",
            BridgeMode::Guest => "internal",
        }
    }
}

impl Display for BridgeMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

pub trait BridgeGenerator {
    fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn generate(&mut self) -> anyhow::Result<()>;
}

pub fn tool_bridge_client_directory_name(tool_name: &str) -> String {
    format!("{}-tool-guest-client", tool_name.to_kebab_case())
}

pub fn bridge_client_directory_name(agent_type_name: &AgentTypeName, mode: BridgeMode) -> String {
    match mode {
        BridgeMode::External => format!("{}-client", agent_type_name.as_str().to_kebab_case()),
        BridgeMode::Guest => format!("{}-guest-client", agent_type_name.as_str().to_kebab_case()),
    }
}
