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

//! Organization of api types is as follows:
//! - domain types that are reused in the http api / clients -> golem_common::model::*
//! - api specific types that are reused by the clients -> golem_common::api::*
//! - general server-side only utilities -> golem_service_base::api::*
//! - types specific to this api that are not reused by the client -> golem_registry_service::api::model::*

use golem_common::model::agent::AgentType;
use golem_common::model::component::ComponentName;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::login::TokenWithSecret;
use golem_common::model::plugin::{PluginInstallationAction, PluginScope};
use golem_common::model::{
    ComponentFilePath, ComponentFilePermissions, ComponentType, ComponentVersion, Empty,
};
use golem_service_base::poem::TempFileUpload;
use poem_openapi::payload::Json;
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::{ApiResponse, Multipart, Object, Union};
use std::collections::HashMap;

#[derive(Debug, poem_openapi::Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateLibraryPluginRequest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: poem_openapi::types::Binary<Vec<u8>>,
    pub homepage: String,
    pub scope: PluginScope,
    pub wasm: TempFileUpload,
}

#[derive(Debug, poem_openapi::Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateAppPluginRequest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: poem_openapi::types::Binary<Vec<u8>>,
    pub homepage: String,
    pub scope: PluginScope,
    pub wasm: TempFileUpload,
}

#[derive(Debug, Clone, ApiResponse)]
pub enum WebFlowPollResponse {
    /// OAuth flow has completed
    #[oai(status = 200)]
    Completed(Json<TokenWithSecret>),
    /// OAuth flow is pending
    #[oai(status = 202)]
    Pending(Json<Empty>),
}

#[derive(Debug, Clone, ApiResponse)]
pub enum WebFlowCallbackResponse {
    /// Redirect to the given URL specified in the web flow start
    #[oai(status = 302)]
    Redirect(Json<Empty>, #[oai(header = "Location")] String),
    /// OAuth flow has completed
    #[oai(status = 200)]
    Success(Json<Empty>),
}

#[derive(Clone, Debug, Object)]
pub struct PreviousVersionComponentFileSource {
    /// path in the filesystem of the previous component version
    path_in_previous_version: String,
}

#[derive(Clone, Debug, Object)]
pub struct ArchiveComponentFileSource {
    /// path in the archive that was uploaded as part of this request
    path_in_archive: String,
}

#[derive(Clone, Debug, Union)]
#[oai(one_of = true)]
pub enum ComponentFileSource {
    PreviousVersion(PreviousVersionComponentFileSource),
    Archive(ArchiveComponentFileSource),
}

#[derive(Clone, Debug, Object)]
pub struct ComponentFileOptions {
    /// Path of the file in the uploaded archive
    pub source: ArchiveComponentFileSource,
    pub permissions: ComponentFilePermissions,
}

#[derive(Clone, Debug, Object)]
pub struct ComponentFileOptionsForUpdate {
    /// Path of the file in the uploaded archive
    pub source: ComponentFileSource,
    pub permissions: ComponentFilePermissions,
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateComponentRequest {
    pub component_name: ComponentName,
    pub component: Upload,
    pub component_type: Option<ComponentType>,
    pub files: Option<JsonField<HashMap<ComponentFilePath, ComponentFileOptions>>>,
    pub files_archive: Option<TempFileUpload>,
    pub dynamic_linking: Option<JsonField<HashMap<String, DynamicLinkedInstance>>>,
    pub env: Option<JsonField<HashMap<String, String>>>,
    pub agent_types: Option<JsonField<Vec<AgentType>>>,
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct UpdateComponentRequest {
    pub previous_version: ComponentVersion,
    pub component_type: Option<ComponentType>,
    pub component: Option<Upload>,
    pub files: Option<JsonField<HashMap<ComponentFilePath, ComponentFileOptionsForUpdate>>>,
    pub files_archive: Option<TempFileUpload>,
    pub dynamic_linking: Option<JsonField<HashMap<String, DynamicLinkedInstance>>>,
    pub env: Option<JsonField<HashMap<String, String>>>,
    pub agent_types: Option<JsonField<Vec<AgentType>>>,
    pub plugin_installation_actions: Option<JsonField<Vec<PluginInstallationAction>>>,
}
