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

mod component;
pub mod plugin;

use bincode::{Decode, Encode};
pub use component::*;
use golem_common::base_model::ComponentVersion;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::{ComponentFilePathWithPermissionsList, ComponentType};
use golem_service_base::model::ComponentName;
use golem_service_base::poem::TempFileUpload;
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::{Multipart, Object};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct UpdatePayload {
    pub component_type: Option<ComponentType>,
    pub component: Upload,
    pub files_permissions: Option<ComponentFilePathWithPermissionsList>,
    pub files: Option<TempFileUpload>,
    pub dynamic_linking: Option<JsonField<DynamicLinking>>,
    pub env: Option<JsonField<ComponentEnv>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct ComponentEnv {
    pub key_values: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct DynamicLinking {
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
pub struct ComponentSearch {
    pub components: Vec<ComponentSearchParameters>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ComponentSearchParameters {
    pub name: ComponentName,
    pub version: Option<ComponentVersion>,
}
