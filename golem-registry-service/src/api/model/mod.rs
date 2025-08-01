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

/// Organization of api types is as follows:
/// - domain types that are reused in the http api / clients -> golem_common::model::*
/// - api specific types that are reused by the clients -> golem_common::api::*
/// - general server-side only utilities -> golem_service_base::api::*
/// - types specific to this api that are not reused by the client -> golem_registry_service::api::model::*

pub mod components;
pub mod login;

use golem_common_next::model::{AccountId, ApplicationId, EnvironmentId, ProjectId};
use poem_openapi::types::{ParseFromJSON, ToJSON};
use poem_openapi::{Enum, Object};
use golem_common_next::model::plugin::PluginScope;
use golem_service_base_next::poem::TempFileUpload;

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
