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

use golem_common_next::model::agent::AgentTypes;
use golem_common_next::model::{ComponentFilePath, ComponentFilePermissions, ComponentType};
use golem_service_base_next::poem::TempFileUpload;
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::types::{ParseFromJSON, ParseResult};
use poem_openapi::{Object, Union};
use std::collections::HashMap;

#[derive(Clone, Debug, Object)]
pub struct ComponentFilePathWithPermissionsList {
    pub values: Vec<ComponentFilePathWithPermissions>,
}

impl poem_openapi::types::ParseFromMultipartField for ComponentFilePathWithPermissionsList {
    async fn parse_from_multipart(field: Option<poem::web::Field>) -> ParseResult<Self> {
        String::parse_from_multipart(field)
            .await
            .map_err(|err| err.propagate::<ComponentFilePathWithPermissionsList>())
            .and_then(|s| ParseFromJSON::parse_from_json_string(&s))
    }
}

#[derive(Clone, Debug, Object)]
pub struct ComponentFilePathWithPermissions {
    pub path: ComponentFilePath,
    pub permissions: ComponentFilePermissions,
}

#[derive(Clone, Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct ComponentEnv {
    pub key_values: HashMap<String, String>,
}

#[derive(Clone, Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct DynamicLinking {
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
}

#[derive(Clone, Debug, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum DynamicLinkedInstance {
    WasmRpc(DynamicLinkedWasmRpc),
}

#[derive(Debug, Clone, Object)]
#[oai(rename_all = "camelCase")]
pub struct WasmRpcTarget {
    pub interface_name: String,
    pub component_name: String,
    pub component_type: ComponentType,
}

#[derive(Debug, Clone, Object)]
pub struct DynamicLinkedWasmRpc {
    /// Maps resource names within the dynamic linked interface to target information
    pub targets: HashMap<String, WasmRpcTarget>,
}

#[derive(poem_openapi::Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateComponentRequest {
    pub component: Upload,
    pub component_type: Option<ComponentType>,
    pub files_permissions: Option<ComponentFilePathWithPermissionsList>,
    pub files: Option<TempFileUpload>,
    pub dynamic_linking: Option<JsonField<DynamicLinking>>,
    pub env: Option<JsonField<ComponentEnv>>,
    pub agent_types: Option<JsonField<AgentTypes>>,
}
