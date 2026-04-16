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

pub mod parameter_naming;
pub mod rust;
pub mod type_naming;
pub mod typescript;

use camino::Utf8Path;
use golem_common::model::agent::{AgentType, AgentTypeName};
use heck::ToKebabCase;

/// A rule that adds derive macros to generated types whose names match a regex pattern.
///
/// Multiple rules can match the same type; their derives are merged and deduplicated.
///
/// # Examples
///
/// Add `PartialEq` to all types:
/// ```yaml
/// { pattern: ".*", derives: ["PartialEq"] }
/// ```
///
/// Add `Eq` and `Hash` only to `Uuid`:
/// ```yaml
/// { pattern: "^Uuid$", derives: ["Eq", "Hash"] }
/// ```
#[derive(Debug, Clone)]
pub struct DeriveRule {
    /// Regex pattern matched against the generated type name.
    pub pattern: String,
    /// Derive macros to add when the pattern matches (e.g., "PartialEq", "Eq", "Hash").
    pub derives: Vec<String>,
}

/// Configuration options for bridge SDK code generation.
#[derive(Debug, Clone, Default)]
pub struct BridgeGeneratorConfig {
    /// Rules for adding derive macros to generated types. Each rule pairs a regex
    /// pattern (matched against type names) with a list of derives to add.
    pub derive_rules: Vec<DeriveRule>,
}

pub trait BridgeGenerator {
    fn new(
        agent_type: AgentType,
        target_path: &Utf8Path,
        testing: bool,
        config: BridgeGeneratorConfig,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn generate(&mut self) -> anyhow::Result<()>;
}

pub fn bridge_client_directory_name(agent_type_name: &AgentTypeName) -> String {
    format!("{}-client", agent_type_name.as_str().to_kebab_case())
}
