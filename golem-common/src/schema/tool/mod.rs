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

//! Compatibility layer around [`golem_schema::schema::tool`].
//!
//! The native tool model, its validation, and the canonical input model live
//! in `golem-schema` so the Rust SDK and the Golem services share one
//! implementation. This module re-exports them and keeps the platform-specific
//! durable discovery model.

use crate::model::component::ComponentId;
use crate::model::tool::{RegisteredTool, ToolSource};
use serde::{Deserialize, Serialize};

pub use golem_schema::schema::tool::*;

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct DiscoveredTool {
    pub definition: Tool,
    pub implemented_by: ComponentId,
}

impl From<RegisteredTool> for DiscoveredTool {
    fn from(value: RegisteredTool) -> Self {
        let RegisteredTool {
            definition, source, ..
        } = value;
        let implemented_by = match source {
            ToolSource::Component { component_id, .. } => component_id,
        };

        Self {
            definition,
            implemented_by,
        }
    }
}

#[cfg(test)]
mod tests;
