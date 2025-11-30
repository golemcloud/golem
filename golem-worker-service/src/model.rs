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

// use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
// use crate::gateway_api_deployment::ApiSite;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{HttpApiDefinitionId, RouteMethod};
use golem_common::model::worker::WorkerMetadataDto;
use golem_common::model::ScanCursor;
use golem_service_base::custom_api::path_pattern::AllPathPatterns;
use golem_service_base::custom_api::HttpCors;
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::custom_api::{
    FileServerBindingCompiled, GatewayBindingCompiled, HttpCorsBindingCompiled,
    HttpHandlerBindingCompiled, WorkerBindingCompiled,
};
use poem_openapi::Object;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Object)]
pub struct WorkersMetadataResponse {
    pub workers: Vec<WorkerMetadataDto>,
    pub cursor: Option<ScanCursor>,
}

#[derive(Debug, Clone)]
pub enum HttpMiddleware {
    Cors(HttpCors),
    AuthenticateRequest(SecuritySchemeDetails),
}

#[derive(Debug, Clone)]
pub struct SwaggerHtml(pub String);

#[derive(Debug, Clone)]
pub struct SwaggerUiBinding {
    pub swagger_html: Arc<SwaggerHtml>,
}

#[derive(Debug, Clone)]
pub enum RichGatewayBindingCompiled {
    HttpCorsPreflight(Box<HttpCorsBindingCompiled>),
    HttpAuthCallBack(Box<SecuritySchemeDetails>),
    Worker(Box<WorkerBindingCompiled>),
    FileServer(Box<FileServerBindingCompiled>),
    HttpHandler(Box<HttpHandlerBindingCompiled>),
    SwaggerUi(SwaggerUiBinding),
}

impl RichGatewayBindingCompiled {
    pub fn from_compiled_binding(
        binding: GatewayBindingCompiled,
        precomputed_swagger_ui_htmls: &HashMap<HttpApiDefinitionId, Arc<SwaggerHtml>>,
    ) -> Result<Self, String> {
        match binding {
            GatewayBindingCompiled::FileServer(inner) => {
                Ok(RichGatewayBindingCompiled::FileServer(inner))
            }
            GatewayBindingCompiled::HttpCorsPreflight(inner) => {
                Ok(RichGatewayBindingCompiled::HttpCorsPreflight(inner))
            }
            GatewayBindingCompiled::Worker(inner) => Ok(RichGatewayBindingCompiled::Worker(inner)),
            GatewayBindingCompiled::HttpHandler(inner) => {
                Ok(RichGatewayBindingCompiled::HttpHandler(inner))
            }
            GatewayBindingCompiled::SwaggerUi(inner) => {
                let swagger_html = precomputed_swagger_ui_htmls
                    .get(&inner.http_api_definition_id)
                    .ok_or("no precomputed swagger html".to_string())?
                    .clone();
                Ok(RichGatewayBindingCompiled::SwaggerUi(SwaggerUiBinding {
                    swagger_html,
                }))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RichCompiledRoute {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub method: RouteMethod,
    pub path: AllPathPatterns,
    pub binding: RichGatewayBindingCompiled,
    pub middlewares: Vec<HttpMiddleware>,
}

impl RichCompiledRoute {
    pub fn get_security_middleware(&self) -> Option<SecuritySchemeDetails> {
        for middleware in &self.middlewares {
            if let HttpMiddleware::AuthenticateRequest(security) = middleware {
                return Some(security.clone());
            }
        }
        None
    }
}
