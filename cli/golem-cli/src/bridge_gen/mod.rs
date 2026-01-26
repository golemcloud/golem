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
