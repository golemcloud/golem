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

use golem_common::api::component::{
    CreateAppPluginRequestMetadata, CreateComponentRequestMetadata,
    CreateLibraryPluginRequestMetadata, UpdateComponentRequestMetadata,
};
use golem_common::model::Empty;
use golem_common::model::auth::TokenWithSecret;
use golem_service_base::poem::TempFileUpload;
use poem_openapi::payload::Json;
use poem_openapi::types::Binary;
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::{ApiResponse, Multipart};

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

#[derive(Debug, poem_openapi::Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateLibraryPluginRequest {
    pub metadata: JsonField<CreateLibraryPluginRequestMetadata>,
    pub icon: Binary<Vec<u8>>,
    pub plugin_wasm: TempFileUpload,
}

#[derive(Debug, poem_openapi::Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateAppPluginRequest {
    pub metadata: JsonField<CreateAppPluginRequestMetadata>,
    pub icon: Binary<Vec<u8>>,
    pub plugin_wasm: TempFileUpload,
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct CreateComponentRequest {
    pub metadata: JsonField<CreateComponentRequestMetadata>,
    pub component_wasm: Upload,
    pub files: Option<TempFileUpload>,
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct UpdateComponentRequest {
    pub metadata: JsonField<UpdateComponentRequestMetadata>,
    pub new_component_wasm: Option<Upload>,
    pub new_files: Option<TempFileUpload>,
}

#[derive(Debug, Clone, ApiResponse)]
pub enum WebFlowCallbackSuccess {
    /// Redirect to the given URL specified in the web flow start
    #[oai(status = 302)]
    Redirect(Json<Empty>, #[oai(header = "Location")] String),
    /// OAuth flow has completed
    #[oai(status = 200)]
    Success(Json<Empty>),
}

#[derive(Debug, Clone, ApiResponse)]
pub enum WebFlowPoll {
    /// OAuth flow has completed
    #[oai(status = 200)]
    Completed(Json<TokenWithSecret>),
    /// OAuth flow is pending
    #[oai(status = 202)]
    Pending(Json<Empty>),
}
