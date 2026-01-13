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

use std::fmt::{Display, Formatter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use golem_wasm_derive::{FromValue, IntoValue};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Deserialize,
    Serialize,
    IntoValue,
    FromValue,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::NewType, desert_rust::BinaryCodec))]
#[repr(transparent)]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AgentTypeName(pub String);

impl Display for AgentTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union, desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum DataValue {
    Tuple(ElementValues),
    Multimodal(NamedElementValues),
}


/// Identifies a deployed, instantiated agent.
///
/// AgentId is convertible to and from string, and is used as _worker names_.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentId {
    pub agent_type: AgentTypeName,
    pub parameters: DataValue,
    pub phantom_id: Option<Uuid>,
    wrapper_agent_type: String,
    as_string: String,
}
