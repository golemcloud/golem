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

use std::collections::HashMap;

use golem_wasm_ast::analysis::AnalysedExport;
use serde::{Deserialize, Serialize};

use crate::model::component_metadata::{DynamicLinkedInstance, LinearMemory};
use crate::model::{ComponentId, ComponentType, ComponentVersion, InitialComponentFile};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileSystemComponentMetadata {
    pub component_id: ComponentId,
    pub version: ComponentVersion,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub component_name: String,
    pub wasm_filename: String,

    #[serde(default)]
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,

    #[serde(default)]
    pub env: HashMap<String, String>,
}
