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

use crate::custom_api::openapi::HttpApiOpenApiSpec;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::agent::BinarySource;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::custom_api::{
    CallAgentBehaviour, CorsOptions, CorsPreflightBehaviour, SecuritySchemeDetails,
    SessionFromHeaderRouteSecurity, WebhookCallbackBehaviour, OpenApiSpecBehaviour
};
use golem_service_base::custom_api::{PathSegment, RequestBodySchema, RouteBehaviour, RouteId};
use http::Method;
use http::{HeaderName, StatusCode};
use openidconnect::Scope;
use openidconnect::core::CoreIdTokenClaims;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct OidcSession {
    pub subject: String,
    pub issuer: String,

    pub email: Option<String>,
    pub name: Option<String>,
    pub email_verified: Option<bool>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub picture: Option<String>,
    pub preferred_username: Option<String>,

    pub claims: CoreIdTokenClaims,
    pub scopes: HashSet<Scope>,
    pub expires_at: DateTime<Utc>,
}

impl OidcSession {
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }

    pub fn scopes(&self) -> &HashSet<Scope> {
        &self.scopes
    }
}

#[derive(Debug)]
pub struct RichCompiledRoute {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub route_id: RouteId,
    pub method: Method,
    pub path: Vec<PathSegment>,
    pub body: RequestBodySchema,
    pub behavior: RichRouteBehaviour,
    pub security: RichRouteSecurity,
    pub cors: CorsOptions,
}

#[derive(Debug)]
pub enum RichRouteBehaviour {
    CallAgent(CallAgentBehaviour),
    CorsPreflight(CorsPreflightBehaviour),
    WebhookCallback(WebhookCallbackBehaviour),
    OpenApiSpec(OpenApiSpecBehaviour),
    OidcCallback(OidcCallbackBehaviour),
}

impl From<RouteBehaviour> for RichRouteBehaviour {
    fn from(value: RouteBehaviour) -> Self {
        match value {
            RouteBehaviour::CallAgent(inner) => Self::CallAgent(inner),
            RouteBehaviour::CorsPreflight(inner) => Self::CorsPreflight(inner),
            RouteBehaviour::WebhookCallback(inner) => Self::WebhookCallback(inner),
            RouteBehaviour::OpenApiSpec(inner) => Self::OpenApiSpec(inner),
        }
    }
}

#[derive(Debug)]
pub struct OidcCallbackBehaviour {
    pub security_scheme: Arc<SecuritySchemeDetails>,
}

#[derive(Debug, Clone)]
pub enum RichRouteSecurity {
    None,
    SessionFromHeader(SessionFromHeaderRouteSecurity),
    SecurityScheme(RichSecuritySchemeRouteSecurity),
}

#[derive(Debug, Clone)]
pub struct RichSecuritySchemeRouteSecurity {
    pub security_scheme: Arc<SecuritySchemeDetails>,
}

#[derive(Debug)]
pub struct RouteExecutionResult {
    pub status: StatusCode,
    pub headers: HashMap<HeaderName, String>,
    pub body: ResponseBody,
}

pub enum ResponseBody {
    NoBody,
    ComponentModelJsonBody { body: golem_wasm::ValueAndType },
    UnstructuredBinaryBody { body: BinarySource },
    OpenApiSchema { body: HttpApiOpenApiSpec },
}

impl fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResponseBody::NoBody => f.debug_struct("NoBody").finish(),
            ResponseBody::ComponentModelJsonBody { body } => f
                .debug_struct("ComponentModelJsonBody")
                .field("body", body)
                .finish(),
            ResponseBody::UnstructuredBinaryBody { .. } => f.write_str("UnstructuredBinaryBody"),
            ResponseBody::OpenApiSchema { body } => f
                .debug_struct("OpenApiSchema")
                .field("body", &body.0)
                .finish(),
        }
    }
}

pub enum ParsedRequestBody {
    Unused,
    JsonBody(golem_wasm::Value),
    // Always Some initially, will be None after being consumed by handler code
    UnstructuredBinary(Option<BinarySource>),
}

impl fmt::Debug for ParsedRequestBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParsedRequestBody::Unused => f.write_str("Unused"),
            ParsedRequestBody::JsonBody(value) => f.debug_tuple("JsonBody").field(value).finish(),
            ParsedRequestBody::UnstructuredBinary(_) => f.write_str("UnstructuredBinary"),
        }
    }
}
