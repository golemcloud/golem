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

pub mod api_definition;
pub mod api_domain;
pub mod application;
pub mod certificate;
pub mod environment;

use crate::model::api_definition::ApiDefinitionId;
use crate::model::plugin::{ComponentTransformerDefinition, OplogProcessorDefinition, PluginScope};
use crate::model::{ComponentId, Revision};
use crate::{declare_structs, declare_unions};
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[cfg(feature = "poem")]
#[derive(Debug, Clone, Serialize, Deserialize, poem_openapi::Object)]
pub struct Page<
    T: poem_openapi::types::Type + poem_openapi::types::ParseFromJSON + poem_openapi::types::ToJSON,
> {
    pub values: Vec<T>,
}

declare_structs! {
    pub struct CreateTokenRequest {
        pub expires_at: chrono::DateTime<Utc>,
    }

    pub struct CreatePluginRequest {
        pub name: String,
        pub version: String,
        pub description: String,
        pub icon: Vec<u8>,
        pub homepage: String,
        pub specs: PluginTypeSpecificCreation,
        pub scope: PluginScope,
    }

    pub struct WebFlowAuthorizeUrlResponse {
        pub url: String,
        pub state: String,
    }

    pub struct CreateApiDeploymentRequest {
        pub api_definitions: Vec<ApiDefinitionId>,
    }

    pub struct UpdateApiDeploymentRequest {
        pub api_definitions: Vec<ApiDefinitionId>,
    }

    pub struct ComponentReference {
        name: String,
        revision: Revision,
        id: ComponentId
    }
}

declare_unions! {
    pub enum PluginTypeSpecificCreation {
        ComponentTransformer(ComponentTransformerDefinition),
        OplogProcessor(OplogProcessorDefinition),
    }
}
