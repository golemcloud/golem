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

#[cfg(feature = "full")]
use crate::base_model::canonicalize_agent_path;
use crate::base_model::render_config_path;
use crate::declare_transparent_newtypes;
use std::fmt::Display;

declare_transparent_newtypes! {
    /// Canonical representation of an agent config path (segments are each camelCase)
    #[derive(Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    #[cfg_attr(feature = "full", oai(to_header = false))]
    pub struct CanonicalAgentConfigPath(pub Vec<String>);
}

impl CanonicalAgentConfigPath {
    #[cfg(feature = "full")]
    pub fn from_path_in_unknown_casing(value: &[String]) -> Self {
        Self(canonicalize_agent_path(value))
    }
}

impl Display for CanonicalAgentConfigPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", render_config_path(&self.0))
    }
}
