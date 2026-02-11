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

pub mod openapi;
mod protobuf;

use crate::model::SafeIndex;
use base64::Engine;
use desert_rust::BinaryCodec;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentType, AgentTypeName, DataSchema, HttpMethod};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::{Provider, SecuritySchemeId, SecuritySchemeName};
use golem_common::model::{OplogIndex, PromiseId, WorkerId};
use golem_wasm::analysis::analysed_type;
use golem_wasm::analysis::{AnalysedType, TypeList, TypeOption};
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::collections::{BTreeSet, HashMap};
use std::fmt;

pub type RouteId = i32;

#[derive(Debug, Clone, PartialEq, Eq, Hash, BinaryCodec)]
#[desert(evolution())]
pub enum PathSegment {
    Literal { value: String },
    Variable,
    CatchAll,
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathSegment::Literal { value } => f.write_str(value),
            PathSegment::Variable => f.write_str("*"),
            PathSegment::CatchAll => f.write_str("**"),
        }
    }
}

/// reduced version of AnalysedType for types that can be parsed from query params and headers.
#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum PathSegmentType {
    Str,
    Chr,
    F64,
    F32,
    U64,
    S64,
    U32,
    S32,
    U16,
    S16,
    U8,
    S8,
    Bool,
    Enum(golem_wasm::analysis::TypeEnum),
}

impl From<PathSegmentType> for AnalysedType {
    fn from(value: PathSegmentType) -> Self {
        match value {
            PathSegmentType::Str => analysed_type::str(),
            PathSegmentType::Chr => analysed_type::chr(),
            PathSegmentType::F64 => analysed_type::f64(),
            PathSegmentType::F32 => analysed_type::f32(),
            PathSegmentType::U64 => analysed_type::u64(),
            PathSegmentType::S64 => analysed_type::s64(),
            PathSegmentType::U32 => analysed_type::u32(),
            PathSegmentType::S32 => analysed_type::s32(),
            PathSegmentType::U16 => analysed_type::u16(),
            PathSegmentType::S16 => analysed_type::s16(),
            PathSegmentType::U8 => analysed_type::u8(),
            PathSegmentType::S8 => analysed_type::s8(),
            PathSegmentType::Bool => analysed_type::bool(),
            PathSegmentType::Enum(inner) => AnalysedType::Enum(inner),
        }
    }
}

impl TryFrom<AnalysedType> for PathSegmentType {
    type Error = String;

    fn try_from(value: AnalysedType) -> Result<Self, Self::Error> {
        match value {
            AnalysedType::Str(_) => Ok(PathSegmentType::Str),
            AnalysedType::Chr(_) => Ok(PathSegmentType::Chr),
            AnalysedType::F64(_) => Ok(PathSegmentType::F64),
            AnalysedType::F32(_) => Ok(PathSegmentType::F32),
            AnalysedType::U64(_) => Ok(PathSegmentType::U64),
            AnalysedType::S64(_) => Ok(PathSegmentType::S64),
            AnalysedType::U32(_) => Ok(PathSegmentType::U32),
            AnalysedType::S32(_) => Ok(PathSegmentType::S32),
            AnalysedType::U16(_) => Ok(PathSegmentType::U16),
            AnalysedType::S16(_) => Ok(PathSegmentType::S16),
            AnalysedType::U8(_) => Ok(PathSegmentType::U8),
            AnalysedType::S8(_) => Ok(PathSegmentType::S8),
            AnalysedType::Bool(_) => Ok(PathSegmentType::Bool),
            AnalysedType::Enum(e) => Ok(PathSegmentType::Enum(e.clone())),
            _ => Err("Unsupported analyzed type for path segment".into()),
        }
    }
}

/// reduced version of AnalysedType for types that can be parsed from headers and query params.
/// * option maps to optional params
/// * list maps to
///     * repeated query params
///     * repeated headers and/or comma-seperated header value. See https://www.w3.org/Protocols/rfc2616/rfc2616-sec4.html#sec4.2
#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum QueryOrHeaderType {
    Primitive(PathSegmentType),
    Option {
        name: Option<String>,
        owner: Option<String>,
        inner: Box<PathSegmentType>,
    },
    List {
        name: Option<String>,
        owner: Option<String>,
        inner: Box<PathSegmentType>,
    },
}

impl From<QueryOrHeaderType> for AnalysedType {
    fn from(value: QueryOrHeaderType) -> Self {
        match value {
            QueryOrHeaderType::Primitive(inner) => inner.into(),
            QueryOrHeaderType::Option { name, owner, inner } => AnalysedType::Option(TypeOption {
                name,
                owner,
                inner: Box::new(AnalysedType::from(*inner)),
            }),
            QueryOrHeaderType::List { name, owner, inner } => AnalysedType::List(TypeList {
                name,
                owner,
                inner: Box::new(AnalysedType::from(*inner)),
            }),
        }
    }
}

impl TryFrom<AnalysedType> for QueryOrHeaderType {
    type Error = String;

    fn try_from(value: AnalysedType) -> Result<Self, Self::Error> {
        match value {
            AnalysedType::Option(TypeOption { name, owner, inner }) => Ok(Self::Option {
                name,
                owner,
                inner: Box::new(PathSegmentType::try_from(*inner)?),
            }),
            AnalysedType::List(TypeList { name, owner, inner }) => Ok(Self::List {
                name,
                owner,
                inner: Box::new(PathSegmentType::try_from(*inner)?),
            }),
            other => Ok(Self::Primitive(
                PathSegmentType::try_from(other)
                    .map_err(|_| "Unsupported analyzed type for query or header")?,
            )),
        }
    }
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub enum RequestBodySchema {
    Unused,
    JsonBody { expected_type: AnalysedType },
    UnrestrictedBinary,
    RestrictedBinary { allowed_mime_types: Vec<String> },
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub enum ConstructorParameter {
    Path {
        path_segment_index: SafeIndex,
        parameter_type: PathSegmentType,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum MethodParameter {
    Path {
        path_segment_index: SafeIndex,
        parameter_type: PathSegmentType,
    },
    Query {
        query_parameter_name: String,
        parameter_type: QueryOrHeaderType,
    },
    Header {
        header_name: String,
        parameter_type: QueryOrHeaderType,
    },
    JsonObjectBodyField {
        field_index: SafeIndex,
    },
    UnstructuredBinaryBody,
}

#[derive(Debug)]
pub struct CompiledRoutes {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub security_schemes: HashMap<SecuritySchemeId, SecuritySchemeDetails>,
    pub routes: Vec<CompiledRoute>,
}

#[derive(Debug)]
pub struct CompiledRoute {
    pub route_id: RouteId,
    pub method: HttpMethod,
    pub path: Vec<PathSegment>,
    // TODO: move this into the individual route behaviours
    pub body: RequestBodySchema,
    pub behavior: RouteBehaviour,
    pub security_scheme: Option<SecuritySchemeId>,
    pub cors: CorsOptions,
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub enum RouteBehaviour {
    CallAgent(CallAgentBehaviour),
    CorsPreflight(CorsPreflightBehaviour),
    WebhookCallback(WebhookCallbackBehaviour),
    OpenApiSpec(OpenApiSpecBehaviour),
}


// OpenAPI spec behaviour is more per component for now
// `open_api_spec` can also represent agent types across the deployment.
// However, it should still be keyed by agent-type since it decides the mount path
// and the structure enables us to have more fine-grained versions of open api spec.
#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct OpenApiSpecBehaviour {
    pub open_api_spec: Vec<RoutesPerAgent>,
}

#[derive(Debug, Clone, BinaryCodec)]
pub struct RoutesPerAgent {
    pub agent_type: AgentType,
    pub routes: Vec<HttpRouteDetails>,
}

// TODO; remove comment
// Anything that is in HttpRouteDetails can be correlated back to AgentType
// The following details can be extracted back from UnboundCompiledRoute
#[derive(Debug, Clone, BinaryCodec)]
pub struct HttpRouteDetails {
    pub route_id: RouteId,
    pub method: HttpMethod,
    pub path: Vec<PathSegment>,
    pub body: RequestBodySchema,
    pub behaviour: RouteBehaviour,
    pub security_scheme: Option<SecuritySchemeName>, // This is fully available in RichCompiledRoute
    pub cors: CorsOptions,
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct CallAgentBehaviour {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub agent_type: AgentTypeName,
    pub constructor_parameters: Vec<ConstructorParameter>,
    pub phantom: bool,
    pub method_name: String,
    pub method_parameters: Vec<MethodParameter>,
    pub expected_agent_response: DataSchema,
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct CorsPreflightBehaviour {
    pub allowed_origins: BTreeSet<OriginPattern>,
    pub allowed_methods: BTreeSet<HttpMethod>,
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct WebhookCallbackBehaviour {
    pub component_id: ComponentId,
}

#[derive(Debug, Clone)]
pub struct SecuritySchemeDetails {
    pub id: SecuritySchemeId,
    pub name: SecuritySchemeName,
    pub provider_type: Provider,
    pub client_id: ClientId,
    pub client_secret: ClientSecret,
    pub redirect_url: RedirectUrl,
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, BinaryCodec)]
#[desert(evolution())]
pub struct CorsOptions {
    pub allowed_patterns: Vec<OriginPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, BinaryCodec)]
#[desert(transparent)]
// Note: Wildcards are only considered during matching. When setting the allow-origin header
// always use the exact origin that made the request to avoid complications with
// presence of auth information.
// https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Access-Control-Allow-Origin#sect
pub struct OriginPattern(pub String);

impl OriginPattern {
    // match origin according to cors spec https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Access-Control-Allow-Origin
    pub fn matches(&self, origin: &str) -> bool {
        if self.0 == "*" {
            true
        } else {
            self.0 == origin
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
/// PromiseId (-component-id) to be base64-url encoded and used as the final segment in a webhook callback
pub struct AgentWebhookId {
    pub worker_name: String,
    pub oplog_idx: OplogIndex,
}

impl AgentWebhookId {
    pub fn into_promise_id(self, component_id: ComponentId) -> PromiseId {
        PromiseId {
            worker_id: WorkerId {
                component_id,
                worker_name: self.worker_name,
            },
            oplog_idx: self.oplog_idx,
        }
    }

    pub fn to_base64_url(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("AgentWebhookId serialization must not fail");
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    pub fn from_base64_url(s: &str) -> anyhow::Result<Self> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn test_origin_pattern_matches() {
        let wildcard = OriginPattern("*".to_string());
        assert!(wildcard.matches("https://example.com"));
        assert!(wildcard.matches("https://foo.bar"));

        let exact = OriginPattern("https://example.com".to_string());
        assert!(exact.matches("https://example.com"));
        assert!(!exact.matches("https://other.com"));
        assert!(!exact.matches("http://example.com")); // scheme matters
    }
}
