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

use super::ComponentReference;
use crate::model::api_definition::cors::CorsConfiguration;
use crate::model::api_definition::ApiDefinitionId;
use crate::model::api_definition::RouteMethod;
use crate::model::security_scheme::Provider;
use crate::{declare_enums, declare_structs};
use chrono::{DateTime, Utc};
use rib::{RibInputTypeInfo, RibOutputTypeInfo};

declare_enums! {
    pub enum GatewayBindingType {
        Default,
        FileServer,
        HttpHandler,
        CorsPreflight,
        SwaggerUi,
    }
}

declare_structs! {
    pub struct CreateHttpApiDefinitionRequest {
        pub routes: Vec<RouteRequestView>,
        pub version: String,
    }

    pub struct UpdateHttpApiDefinitionRequest {
        pub routes: Vec<RouteRequestView>,
        pub version: String,
    }

    pub struct HttpApiDefinitionResponseView {
        pub id: ApiDefinitionId,
        pub routes: Vec<RouteResponseView>,
        pub version: String,
        pub created_at: DateTime<Utc>,
    }

    pub struct GatewayBindingRequestView {
        pub binding_type: Option<GatewayBindingType>, // discriminator to keep backward compatibility
        // For binding type - worker/default and file-server
        // Optional only to keep backward compatibility
        pub component: Option<ComponentReference>,
        // For binding type - worker/default
        pub idempotency_key: Option<String>,
        // For binding type - worker/default and fileserver, this is required
        // For binding type cors-preflight, this is optional otherwise default cors-preflight settings
        // is used
        pub response: Option<String>,
        // For binding type - worker/default
        pub invocation_context: Option<String>,
    }

    pub struct GatewayBindingResponseView {
        pub component: Option<ComponentReference>, // Allowed only if bindingType is Default, FileServer or HttpServer
        pub worker_name: Option<String>, // Allowed only if bindingType is Default or FileServer
        pub idempotency_key: Option<String>, // Allowed only if bindingType is Default or FileServer
        pub invocation_context: Option<String>, // Allowed only if bindingType is Default or FileServer
        pub response: Option<String>,    // Allowed only if bindingType is Default or FileServer
        pub binding_type: Option<GatewayBindingType>,
        pub response_mapping_input: Option<RibInputTypeInfo>, // If bindingType is Default or FileServer
        pub worker_name_input: Option<RibInputTypeInfo>,      // If bindingType is Default or FileServer
        pub idempotency_key_input: Option<RibInputTypeInfo>, // If bindingType is Default or FilerServer
        pub cors_preflight: Option<CorsConfiguration>, // If bindingType is CorsPreflight (internally, a static binding)
        pub response_mapping_output: Option<RibOutputTypeInfo>, // If bindingType is Default or FileServer
        pub openapi_spec_json: Option<String>, // If bindingType is SwaggerUi
    }

    pub struct CreateSecuritySchemeRequest {
        pub provider_type: Provider,
        pub scheme_identifier: String,
        pub client_id: String,
        pub client_secret: String,
        pub redirect_url: String,
        pub scopes: Vec<String>,
    }

    pub struct SecuritySchemeResponseView {
        pub provider_type: Provider,
        pub scheme_identifier: String,
        pub client_id: String,
        pub client_secret: String,
        pub redirect_url: String,
        pub scopes: Vec<String>,
        pub created_at: DateTime<Utc>
    }

    pub struct RouteRequestView {
        pub method: RouteMethod,
        pub path: String,
        pub binding: GatewayBindingRequestView,
        pub security: Option<String>,
    }

    pub struct RouteResponseView {
        pub method: RouteMethod,
        pub path: String,
        pub binding: GatewayBindingResponseView,
        pub security: Option<String>,
    }

}
