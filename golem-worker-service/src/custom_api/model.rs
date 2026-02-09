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

use super::error::RequestHandlerError;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::agent::BinarySource;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::custom_api::{
    CallAgentBehaviour, CorsOptions, CorsPreflightBehaviour, SecuritySchemeDetails,
};
use golem_service_base::custom_api::{PathSegment, RequestBodySchema, RouteBehaviour, RouteId};
use http::{HeaderMap, Method};
use http::{HeaderName, StatusCode};
use openidconnect::Scope;
use openidconnect::core::CoreIdTokenClaims;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

const COOKIE_HEADER_NAMES: [&str; 2] = ["cookie", "Cookie"];

pub struct RichRequest {
    pub underlying: poem::Request,
    pub request_id: Uuid,
    pub authenticated_session: Option<OidcSession>,

    parsed_cookies: OnceLock<HashMap<String, String>>,
    parsed_query_params: OnceLock<HashMap<String, Vec<String>>>,
}

impl RichRequest {
    pub fn new(underlying: poem::Request) -> RichRequest {
        RichRequest {
            underlying,
            request_id: Uuid::new_v4(),
            authenticated_session: None,
            parsed_cookies: OnceLock::new(),
            parsed_query_params: OnceLock::new(),
        }
    }

    pub fn origin(&self) -> Result<Option<&str>, RequestHandlerError> {
        match self.underlying.headers().get("Origin") {
            Some(header) => {
                let result =
                    header
                        .to_str()
                        .map_err(|_| RequestHandlerError::HeaderIsNotAscii {
                            header_name: "Origin".to_string(),
                        })?;
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }

    pub fn headers(&self) -> &HeaderMap {
        self.underlying.headers()
    }

    pub fn query_params(&self) -> &HashMap<String, Vec<String>> {
        self.parsed_query_params.get_or_init(|| {
            let mut params: HashMap<String, Vec<String>> = HashMap::new();

            if let Some(q) = self.underlying.uri().query() {
                for (key, value) in url::form_urlencoded::parse(q.as_bytes()).into_owned() {
                    params.entry(key).or_default().push(value);
                }
            }

            params
        })
    }

    pub fn get_single_param(&self, name: &'static str) -> Result<&str, RequestHandlerError> {
        match self.query_params().get(name).map(|qp| qp.as_slice()) {
            Some([single]) => Ok(single),
            None | Some([]) => Err(RequestHandlerError::MissingValue { expected: name }),
            _ => Err(RequestHandlerError::TooManyValues { expected: name }),
        }
    }

    pub fn cookies(&self) -> &HashMap<String, String> {
        self.parsed_cookies.get_or_init(|| {
            let mut map = HashMap::new();
            for header_name in COOKIE_HEADER_NAMES.iter() {
                if let Some(value) = self.underlying.header(header_name) {
                    for part in value.split(';') {
                        let mut kv = part.splitn(2, '=');
                        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                            map.insert(k.trim().to_string(), v.trim().to_string());
                        }
                    }
                }
            }
            map
        })
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies().get(name).map(|s| s.as_str())
    }

    pub fn set_authenticated_session(&mut self, session: OidcSession) {
        self.authenticated_session = Some(session);
    }

    pub fn authenticated_session(&self) -> Option<&OidcSession> {
        self.authenticated_session.as_ref()
    }
}

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
    pub security_scheme: Option<Arc<SecuritySchemeDetails>>,
    pub cors: CorsOptions,
}

#[derive(Debug)]
pub enum RichRouteBehaviour {
    CallAgent(CallAgentBehaviour),
    CorsPreflight(CorsPreflightBehaviour),
    OidcCallback(OidcCallbackBehaviour),
}

impl From<RouteBehaviour> for RichRouteBehaviour {
    fn from(value: RouteBehaviour) -> Self {
        match value {
            RouteBehaviour::CallAgent(inner) => Self::CallAgent(inner),
            RouteBehaviour::CorsPreflight(inner) => Self::CorsPreflight(inner),
        }
    }
}

#[derive(Debug)]
pub struct OidcCallbackBehaviour {
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
