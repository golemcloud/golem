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

use super::component::ComponentRevision;
use super::ComponentId;
use crate::declare_transparent_newtypes;
use crate::model::diff;

pub use crate::base_model::plugin_registration::*;

declare_transparent_newtypes! {
    pub struct WasmContentHash(pub diff::Hash);
}

impl PluginRegistrationDto {
    fn oplog_processor(&self) -> Option<&OplogProcessorPluginSpec> {
        match &self.spec {
            PluginSpecDto::OplogProcessor(inner) => Some(inner),
        }
    }

    pub fn oplog_processor_component_id(&self) -> Option<ComponentId> {
        self.oplog_processor().map(|inner| inner.component_id)
    }

    pub fn oplog_processor_component_revision(&self) -> Option<ComponentRevision> {
        self.oplog_processor().map(|inner| inner.component_revision)
    }

    pub fn typ_as_str(&self) -> &'static str {
        match &self.spec {
            PluginSpecDto::OplogProcessor(_) => "oplog processor",
        }
    }
}
