// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

pub mod parameter_parsing;
mod protobuf;
pub mod router;

use crate::model::SafeIndex;
use base64::Engine;
use desert_rust::BinaryCodec;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{
    AgentFileContentHash, AgentMode, AgentTypeName, HttpMethod, ReadOnlyConfig,
};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::{Provider, SecuritySchemeId, SecuritySchemeName};
use golem_common::model::security_scheme::{SecuritySchemeLogin, SecuritySchemeRevision};
use golem_common::model::{AgentFingerprint, AgentId, OplogIndex, PromiseId};
use golem_common::schema::{InputSchema, MetadataEnvelope, OutputSchema, SchemaGraph, SchemaType};
pub use golem_schema::http::HttpRouteMode as AgentRouteMode;
use golem_schema::http::{FileMapping, FileResponseHeader};
use hmac::{Hmac, Mac};
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use sha2::Sha256;
use std::collections::{BTreeSet, HashMap};
use std::fmt;

pub type RouteId = i32;

#[derive(Debug, Clone, PartialEq, Eq, Hash, BinaryCodec)]
#[desert(evolution())]
pub enum PathSegment {
    Literal { value: String },
    Variable { display_name: String },
    CatchAll { display_name: String },
}

impl PathSegment {
    pub fn literal_value(&self) -> Option<&str> {
        match self {
            Self::Literal { value } => Some(value),
            _ => None,
        }
    }
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathSegment::Literal { value } => f.write_str(value),
            PathSegment::Variable { display_name } => f.write_str(&format!("{{{display_name}}}")),
            PathSegment::CatchAll { display_name } => f.write_str(&format!("{{*{display_name}}}")),
        }
    }
}

/// reduced version of SchemaType for types that can be parsed from query params and headers.
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
    Uuid,
    Enum(Vec<String>),
}

impl From<&PathSegmentType> for SchemaType {
    fn from(value: &PathSegmentType) -> Self {
        let metadata = MetadataEnvelope::default();
        match value {
            PathSegmentType::Str => SchemaType::String { metadata },
            PathSegmentType::Chr => SchemaType::Char { metadata },
            PathSegmentType::F64 => SchemaType::F64 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::F32 => SchemaType::F32 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::U64 => SchemaType::U64 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::S64 => SchemaType::S64 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::U32 => SchemaType::U32 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::S32 => SchemaType::S32 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::U16 => SchemaType::U16 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::S16 => SchemaType::S16 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::U8 => SchemaType::U8 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::S8 => SchemaType::S8 {
                restrictions: None,
                metadata,
            },
            PathSegmentType::Bool => SchemaType::Bool { metadata },
            PathSegmentType::Uuid => SchemaType::Uuid { metadata },
            PathSegmentType::Enum(cases) => SchemaType::Enum {
                cases: cases.clone(),
                metadata,
            },
        }
    }
}

impl TryFrom<SchemaType> for PathSegmentType {
    type Error = String;

    fn try_from(value: SchemaType) -> Result<Self, Self::Error> {
        match value {
            SchemaType::String { .. } => Ok(PathSegmentType::Str),
            SchemaType::Char { .. } => Ok(PathSegmentType::Chr),
            SchemaType::F64 { .. } => Ok(PathSegmentType::F64),
            SchemaType::F32 { .. } => Ok(PathSegmentType::F32),
            SchemaType::U64 { .. } => Ok(PathSegmentType::U64),
            SchemaType::S64 { .. } => Ok(PathSegmentType::S64),
            SchemaType::U32 { .. } => Ok(PathSegmentType::U32),
            SchemaType::S32 { .. } => Ok(PathSegmentType::S32),
            SchemaType::U16 { .. } => Ok(PathSegmentType::U16),
            SchemaType::S16 { .. } => Ok(PathSegmentType::S16),
            SchemaType::U8 { .. } => Ok(PathSegmentType::U8),
            SchemaType::S8 { .. } => Ok(PathSegmentType::S8),
            SchemaType::Bool { .. } => Ok(PathSegmentType::Bool),
            SchemaType::Uuid { .. } => Ok(PathSegmentType::Uuid),
            SchemaType::Enum { cases, .. } => Ok(PathSegmentType::Enum(cases)),
            _ => Err("Unsupported schema type for path segment".into()),
        }
    }
}

/// reduced version of SchemaType for types that can be parsed from headers and query params.
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
        inner: Box<QueryOrHeaderType>,
    },
    List {
        name: Option<String>,
        owner: Option<String>,
        inner: Box<PathSegmentType>,
    },
    Map {
        key: PathSegmentType,
        value: PathSegmentType,
    },
}

impl QueryOrHeaderType {
    pub fn is_map(&self) -> bool {
        match self {
            Self::Map { .. } => true,
            Self::Option { inner, .. } => inner.is_map(),
            _ => false,
        }
    }
}

impl From<QueryOrHeaderType> for SchemaType {
    fn from(value: QueryOrHeaderType) -> Self {
        let metadata = MetadataEnvelope::default();
        match value {
            QueryOrHeaderType::Primitive(inner) => (&inner).into(),
            QueryOrHeaderType::Option { inner, .. } => SchemaType::Option {
                inner: Box::new(SchemaType::from(*inner)),
                metadata,
            },
            QueryOrHeaderType::List { inner, .. } => SchemaType::List {
                element: Box::new(SchemaType::from(&*inner)),
                metadata,
            },
            QueryOrHeaderType::Map { key, value } => SchemaType::Map {
                key: Box::new(SchemaType::from(&key)),
                value: Box::new(SchemaType::from(&value)),
                metadata,
            },
        }
    }
}

impl TryFrom<SchemaType> for QueryOrHeaderType {
    type Error = String;

    fn try_from(value: SchemaType) -> Result<Self, Self::Error> {
        match value {
            SchemaType::Option { inner, .. } => Ok(Self::Option {
                name: None,
                owner: None,
                inner: Box::new(Self::try_from(*inner)?),
            }),
            SchemaType::List { element, .. } => Ok(Self::List {
                name: None,
                owner: None,
                inner: Box::new(PathSegmentType::try_from(*element)?),
            }),
            SchemaType::Map { key, value, .. } => Ok(Self::Map {
                key: PathSegmentType::try_from(*key)?,
                value: PathSegmentType::try_from(*value)?,
            }),
            other => Ok(Self::Primitive(
                PathSegmentType::try_from(other)
                    .map_err(|_| "Unsupported schema type for query or header")?,
            )),
        }
    }
}

/// A self-contained schema carrier persisted in a compiled route.
///
/// `graph.root` is the exact root being parsed/rendered/invoked and `graph.defs`
/// carry the per-agent named-type definitions, so any `SchemaType::Ref` inside
/// `root` resolves at runtime without an `AgentTypeSchema` lookup (the worker
/// custom-API runtime has no access to the agent type after deployment).
#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct CompiledSchema {
    pub graph: SchemaGraph,
}

/// Self-contained constructor/method input schema persisted in a compiled route.
#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct CompiledInputSchema {
    pub graph: SchemaGraph,
    pub input_schema: InputSchema,
}

impl CompiledInputSchema {
    fn validate(&self) -> Result<(), String> {
        let expected = SchemaType::record(
            self.input_schema
                .fields()
                .iter()
                .map(|field| golem_common::schema::NamedFieldType {
                    name: field.name.clone(),
                    body: field.schema.clone(),
                    metadata: field.metadata.clone(),
                })
                .collect(),
        );
        if self.graph.root != expected {
            return Err(
                "Compiled input graph root must describe the positional input schema".into(),
            );
        }
        golem_common::schema::validation::validate_graph(&self.graph)
            .map_err(|errors| format!("Invalid compiled input graph: {errors:?}"))
    }
}

/// Self-contained method output schema persisted in a compiled route.
#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct CompiledOutputSchema {
    pub graph: SchemaGraph,
    pub output_schema: OutputSchema,
}

impl CompiledOutputSchema {
    fn validate(&self) -> Result<(), String> {
        let expected = match &self.output_schema {
            OutputSchema::Unit => SchemaType::record(vec![]),
            OutputSchema::Single(schema) => (**schema).clone(),
        };
        if self.graph.root != expected {
            return Err("Compiled output graph root must describe the output schema".into());
        }
        golem_common::schema::validation::validate_graph(&self.graph)
            .map_err(|errors| format!("Invalid compiled output graph: {errors:?}"))
    }
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub enum RequestBodySchema {
    Unused,
    /// JSON request body whose `expected.graph.root` is the record of the
    /// remaining (non-path/query/header) input fields.
    JsonBody {
        expected: CompiledSchema,
    },
    /// Raw binary request body; `expected.graph.root` is an unstructured binary
    /// carrier — either the canonical `variant { inline, url }` wrapper or a
    /// bare `SchemaType::Binary` rich scalar (its restrictions carry the allowed
    /// MIME types).
    BinaryBody {
        expected: CompiledSchema,
    },
    /// Raw text request body; `expected.graph.root` is an unstructured text
    /// carrier — either the canonical `variant { inline, url }` wrapper or a
    /// bare `SchemaType::Text` rich scalar (its restrictions carry the allowed
    /// languages).
    TextBody {
        expected: CompiledSchema,
    },
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
        argument_name: String,
        path_segment_index: SafeIndex,
        parameter_type: PathSegmentType,
    },
    Query {
        argument_name: String,
        query_parameter_name: String,
        parameter_type: QueryOrHeaderType,
    },
    Header {
        argument_name: String,
        header_name: String,
        parameter_type: QueryOrHeaderType,
    },
    JsonObjectBodyField {
        argument_name: String,
        field_name: String,
    },
    UnstructuredBinaryBody {
        argument_name: String,
    },
    UnstructuredTextBody {
        argument_name: String,
    },
}

impl MethodParameter {
    pub fn argument_name(&self) -> &str {
        match self {
            Self::Path { argument_name, .. }
            | Self::Query { argument_name, .. }
            | Self::Header { argument_name, .. }
            | Self::JsonObjectBodyField { argument_name, .. }
            | Self::UnstructuredBinaryBody { argument_name }
            | Self::UnstructuredTextBody { argument_name } => argument_name,
        }
    }
}

#[derive(Debug)]
pub struct CompiledRoutes {
    pub account_id: AccountId,
    pub account_email: AccountEmail,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub security_schemes: HashMap<SecuritySchemeId, SecuritySchemeDetails>,
    pub routes: Vec<CompiledRoute>,
}

#[derive(Debug)]
pub struct CompiledRoute {
    pub route_id: RouteId,
    pub route_match: RouteMatch,
    pub path: Vec<PathSegment>,
    pub behavior: RouteBehaviour,
    pub security: RouteSecurity,
    pub cors: CorsOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum RouteMatch {
    Method {
        method: HttpMethod,
        trailing_slash: bool,
    },
    MountPrefix,
}

impl From<HttpMethod> for RouteMatch {
    fn from(method: HttpMethod) -> Self {
        Self::Method {
            method,
            trailing_slash: false,
        }
    }
}

impl RouteMatch {
    pub fn method(&self) -> Option<&HttpMethod> {
        match self {
            Self::Method { method, .. } => Some(method),
            Self::MountPrefix => None,
        }
    }

    pub fn validate(&self, path: &[PathSegment], behavior: &RouteBehaviour) -> Result<(), String> {
        let mount_behavior = matches!(behavior, RouteBehaviour::Mount(_));
        if matches!(self, Self::MountPrefix) != mount_behavior {
            return Err("MountPrefix must be paired with Mount behavior".into());
        }
        if let Self::Method {
            method,
            trailing_slash,
        } = self
        {
            http::Method::try_from(method.clone()).map_err(|error| error.to_string())?;
            if *trailing_slash && path.is_empty() {
                return Err("Root path cannot have a trailing-slash flag".into());
            }
        } else if path
            .iter()
            .any(|segment| matches!(segment, PathSegment::CatchAll { .. }))
        {
            return Err("MountPrefix cannot contain a catch-all".into());
        }
        if let RouteBehaviour::Mount(mount) = behavior {
            mount.files.agent().constructor_input.validate()?;
            for target in mount.handler.iter().chain(mount.openapi_provider.iter()) {
                let MountTarget::AgentMethod(target) = target;
                target.agent.constructor_input.validate()?;
                target.method.input.validate()?;
                target.method.output.validate()?;
                if target.method.method_name.is_empty() {
                    return Err("Mount method name must not be empty".into());
                }
            }
            if let (Some(handler), Some(provider)) = (&mount.handler, &mount.openapi_provider) {
                let (MountTarget::AgentMethod(handler), MountTarget::AgentMethod(provider)) =
                    (handler, provider);
                if handler.agent.component_id == provider.agent.component_id
                    && handler.agent.agent_type == provider.agent.agent_type
                    && handler.method.method_name == provider.method.method_name
                {
                    return Err("Mount handler and provider must be distinct methods".into());
                }
            }
            FileMapping::validate_list(&mount.file_bindings)?;
            golem_common::schema::agent::http::validate_file_response_headers(
                &mount.file_response_headers,
                !mount.file_bindings.is_empty(),
            )
            .map_err(|error| error.to_string())?;
            match &mount.files {
                MountFileSource::Router { file_index, .. } => {
                    if path
                        .iter()
                        .any(|segment| !matches!(segment, PathSegment::Literal { .. }))
                    {
                        return Err("HttpRouter mount must be literal".into());
                    }
                    let mut paths = BTreeSet::new();
                    for entry in file_index {
                        let path = entry
                            .path
                            .strip_prefix('/')
                            .ok_or("Router file path must be absolute")?;
                        if path.chars().any(char::is_control)
                            || path.split('/').any(|part| {
                                part.is_empty()
                                    || part == "."
                                    || part == ".."
                                    || part.contains('\\')
                            })
                        {
                            return Err("Router file path must be canonical and name a file".into());
                        }
                        if !paths.insert(&entry.path) {
                            return Err("Duplicate router file index path".into());
                        }
                    }
                }
                MountFileSource::Live { agent } => {
                    let captures = path
                        .iter()
                        .filter(|segment| matches!(segment, PathSegment::Variable { .. }))
                        .count();
                    let mut bound = BTreeSet::new();
                    for ConstructorParameter::Path {
                        path_segment_index, ..
                    } in &agent.constructor_parameters
                    {
                        let index: usize = (*path_segment_index).into();
                        if index >= captures || !bound.insert(index) {
                            return Err(
                        "Filesystem constructor parameters must bind distinct mount captures"
                            .into(),
                    );
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub enum RouteBehaviour {
    CallAgent(CallAgentBehaviour),
    CorsPreflight(CorsPreflightBehaviour),
    WebhookCallback(WebhookCallbackBehaviour),
    OpenApiSpec(OpenApiSpecBehaviour),
    Mount(MountBehaviour),
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct MountBehaviour {
    pub handler: Option<MountTarget>,
    pub openapi_provider: Option<MountTarget>,
    pub files: MountFileSource,
    pub file_bindings: Vec<FileMapping>,
    pub file_response_headers: Vec<FileResponseHeader>,
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub enum MountTarget {
    AgentMethod(MountAgentMethod),
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct MountAgentMethod {
    pub agent: MountAgent,
    pub method: RouterMethod,
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct MountAgent {
    pub component_id: ComponentId,
    /// Revision at which the mount policy was selected, not a required executor version.
    pub component_revision: ComponentRevision,
    pub agent_type: AgentTypeName,
    pub constructor_input: CompiledInputSchema,
    pub constructor_parameters: Vec<ConstructorParameter>,
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub enum MountFileSource {
    Router {
        agent: MountAgent,
        file_index: Vec<RouterFileIndexEntry>,
    },
    Live {
        agent: MountAgent,
    },
}

impl MountFileSource {
    pub fn agent(&self) -> &MountAgent {
        match self {
            Self::Router { agent, .. } | Self::Live { agent } => agent,
        }
    }
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct RouterMethod {
    pub method_name: String,
    pub input: CompiledInputSchema,
    pub output: CompiledOutputSchema,
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct RouterFileIndexEntry {
    pub path: String,
    /// BLAKE3 digest computed during upload, used for blob lookup and the immutable HTTP ETag.
    pub blob_key: AgentFileContentHash,
    pub size: u64,
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct CallAgentBehaviour {
    pub route_mode: AgentRouteMode,
    /// Number of captured variables in the declared base path, excluding DS session and slot.
    pub base_path_variables: u32,
    pub durable_streams: Option<DurableStreamRoutePolicy>,
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub agent_type: AgentTypeName,
    pub agent_mode: AgentMode,
    /// Self-contained schema of the agent constructor input. Used by the runtime
    /// to build the `TypedSchemaValue` for the agent id from the constructor
    /// parameters.
    pub constructor_input: CompiledInputSchema,
    pub constructor_parameters: Vec<ConstructorParameter>,
    pub phantom_selection: PhantomSelection,
    pub method_name: String,
    /// Self-contained schema of the agent method input. Used by the runtime to
    /// reconstruct the full positional `SchemaValue::Record` from the bound
    /// HTTP method parameters, injecting auto-injected fields in declaration
    /// order.
    pub method_input: CompiledInputSchema,
    pub body: RequestBodySchema,
    pub method_parameters: Vec<MethodParameter>,
    pub expected_agent_response: CompiledOutputSchema,
    #[desert(default)]
    pub method_description: Option<String>,
    /// The read-only configuration carried over from the `AgentMethod` this route
    /// resolves to. Used by the HTTP layer to emit cache-friendly headers and
    /// handle `If-None-Match` revalidation for `GET`/`HEAD` requests.
    #[desert(default)]
    pub read_only: Option<ReadOnlyConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum PhantomSelection {
    Policy { phantom: bool },
    Original,
    Path { index: SafeIndex },
    Query { name: String, optional: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct DurableStreamRoutePolicy {
    /// The original base route ID, shared by every endpoint in this route family.
    pub family_id: RouteId,
    pub slots: Vec<DurableStreamSlot>,
    pub allow_external_writes: bool,
    pub allow_stream_delete: bool,
    pub allow_invocation_delete: bool,
    pub load: Option<DurableStreamRouteLoadPolicy>,
}

impl DurableStreamRoutePolicy {
    pub fn slot_by_public_name(&self, name: &str) -> Option<&DurableStreamSlot> {
        self.slots.iter().find(|slot| slot.public_name == name)
    }

    pub fn slot_by_canonical_name(&self, name: &str) -> Option<&DurableStreamSlot> {
        self.slots.iter().find(|slot| slot.canonical_name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct DurableStreamSlot {
    pub canonical_name: String,
    pub public_name: String,
    pub direction: DurableStreamSlotDirection,
    pub content_type: String,
    pub representation: DurableStreamRepresentation,
}

impl DurableStreamSlot {
    pub fn writable(&self) -> bool {
        self.direction == DurableStreamSlotDirection::Input
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BinaryCodec)]
pub enum DurableStreamSlotDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BinaryCodec)]
pub enum DurableStreamRepresentation {
    Json,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct DurableStreamRouteLoadPolicy {
    pub max_concurrent_readers_per_stream: Option<u32>,
    pub max_append_requests_per_second_per_stream: Option<u32>,
}
/// Request headers for durable-stream session creation, conditional reads,
/// closing appends and producer-tracked appends.
pub const DURABLE_STREAM_REQUEST_HEADERS: &[&str] = &[
    "content-type",
    "if-none-match",
    "stream-ttl",
    "stream-expires-at",
    "stream-forked-from",
    "stream-fork-offset",
    "stream-fork-sub-offset",
    "stream-closed",
    "producer-id",
    "producer-epoch",
    "producer-seq",
];

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct CorsPreflightBehaviour {
    #[desert(default)]
    pub method_policies: Vec<CorsPreflightMethodPolicy>,
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct CorsPreflightMethodPolicy {
    pub method: HttpMethod,
    #[desert(default)]
    pub allowed_origins: BTreeSet<OriginPattern>,
    #[desert(default)]
    pub allowed_headers: BTreeSet<String>,
}

pub fn cors_allowed_request_headers(
    body: &RequestBodySchema,
    method_parameters: &[MethodParameter],
    session_header: Option<&str>,
) -> BTreeSet<String> {
    let mut headers = BTreeSet::new();
    for parameter in method_parameters {
        if let MethodParameter::Header { header_name, .. } = parameter {
            headers.insert(header_name.trim().to_ascii_lowercase());
        }
    }
    if !matches!(body, RequestBodySchema::Unused) {
        headers.insert("content-type".into());
    }
    if let Some(header) = session_header {
        headers.insert(header.trim().to_ascii_lowercase());
    }
    headers
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct WebhookCallbackBehaviour {
    pub component_id: ComponentId,
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct OpenApiSpecBehaviour {
    pub format: OpenApiSpecFormat,
    pub scheme: golem_common::model::http_api_deployment::HttpApiDeploymentScheme,
}

#[derive(Debug, Clone, Copy, BinaryCodec)]
#[desert(evolution())]
pub enum OpenApiSpecFormat {
    Json,
    Yaml,
}

#[derive(Debug, Clone)]
pub enum RouteSecurity {
    None,
    SessionFromHeader(SessionFromHeaderRouteSecurity),
    SecurityScheme(SecuritySchemeRouteSecurity),
    Unavailable,
}

#[derive(Debug, Clone, BinaryCodec)]
#[desert(evolution())]
pub struct SessionFromHeaderRouteSecurity {
    pub header_name: String,
}

#[derive(Debug, Clone)]
pub struct SecuritySchemeRouteSecurity {
    pub security_scheme_id: SecuritySchemeId,
}

#[derive(Debug, Clone)]
pub struct SecuritySchemeDetails {
    pub id: SecuritySchemeId,
    pub revision: SecuritySchemeRevision,
    pub name: SecuritySchemeName,
    pub provider_type: Provider,
    pub client_id: ClientId,
    pub client_secret: ClientSecret,
    pub redirect_url: RedirectUrl,
    pub scopes: Vec<Scope>,
    pub login: SecuritySchemeLogin,
}

pub const PKCE_ENDPOINT_PREFIX: &str = "/.golem/oidc";

pub fn pkce_authorization_path(security_scheme_id: &SecuritySchemeId) -> String {
    format!("{PKCE_ENDPOINT_PREFIX}/{security_scheme_id}/authorize")
}

pub fn pkce_token_path(security_scheme_id: &SecuritySchemeId) -> String {
    format!("{PKCE_ENDPOINT_PREFIX}/{security_scheme_id}/token")
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

#[derive(Clone, Debug, Eq, PartialEq, Hash, BinaryCodec)]
/// A PromiseId, base64-url encoded as the final segment of a webhook callback URL.
/// Includes a checksum that verifies that the worker executor created the webhook for an agent
/// of the route's environment. The checksum covers the agent's fingerprint, so a webhook of a
/// deleted agent does not complete a promise of the agent re-created with the same ID.
pub struct AgentWebhookId {
    pub agent_id: AgentId,
    pub fingerprint: AgentFingerprint,
    pub oplog_idx: OplogIndex,
    pub checksum: [u8; 32],
}

impl AgentWebhookId {
    pub fn from_promise_id(
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
        hmac_key: &[u8],
    ) -> Self {
        Self {
            agent_id: promise_id.agent_id,
            fingerprint: promise_id.fingerprint,
            oplog_idx: promise_id.oplog_idx,
            checksum: Self::checksum(environment_id, promise_id, hmac_key),
        }
    }

    pub fn into_promise_id(self) -> PromiseId {
        PromiseId {
            agent_id: self.agent_id,
            fingerprint: self.fingerprint,
            oplog_idx: self.oplog_idx,
        }
    }

    /// Whether the worker executor created this webhook for an agent of `environment_id`.
    pub fn verify_checksum(&self, environment_id: EnvironmentId, hmac_key: &[u8]) -> bool {
        let promise_id = self.clone().into_promise_id();
        Self::mac(environment_id, &promise_id, hmac_key)
            .verify_slice(&self.checksum)
            .is_ok()
    }

    fn checksum(
        environment_id: EnvironmentId,
        promise_id: &PromiseId,
        hmac_key: &[u8],
    ) -> [u8; 32] {
        Self::mac(environment_id, promise_id, hmac_key)
            .finalize()
            .into_bytes()
            .into()
    }

    fn mac(environment_id: EnvironmentId, promise_id: &PromiseId, hmac_key: &[u8]) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(hmac_key).unwrap();
        mac.update(b"agent-webhook-id:v2");
        mac.update(environment_id.0.as_bytes());
        let promise_id_bytes = desert_rust::serialize_to_byte_vec(promise_id)
            .expect("PromiseId serialization must not fail");
        mac.update(&promise_id_bytes);
        mac
    }

    pub fn to_base64_url(&self) -> String {
        let bytes = desert_rust::serialize_to_byte_vec(self)
            .expect("AgentWebhookId serialization must not fail");
        // FIXME: base62 would be better as this is used in urls
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    pub fn from_base64_url(s: &str) -> anyhow::Result<Self> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s)?;
        Ok(desert_rust::deserialize(&bytes)?)
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

    #[test]
    fn webhook_id_checksum_covers_the_fingerprint() {
        let hmac_key = b"webhook-test-key";
        let environment_id = EnvironmentId(uuid::Uuid::from_u128(1));
        let promise_id = PromiseId {
            agent_id: "00000007-0000-8000-8000-000000000001".parse().unwrap(),
            fingerprint: AgentFingerprint(uuid::Uuid::from_u128(2)),
            oplog_idx: OplogIndex::from_u64(7),
        };
        let webhook_id = AgentWebhookId::from_promise_id(environment_id, &promise_id, hmac_key);

        let decoded = AgentWebhookId::from_base64_url(&webhook_id.to_base64_url()).unwrap();
        assert!(decoded.verify_checksum(environment_id, hmac_key));
        assert_eq!(decoded.into_promise_id(), promise_id);

        let mut of_new_incarnation = webhook_id.clone();
        of_new_incarnation.fingerprint = AgentFingerprint(uuid::Uuid::from_u128(3));
        assert!(!of_new_incarnation.verify_checksum(environment_id, hmac_key));

        let other_environment = EnvironmentId(uuid::Uuid::from_u128(4));
        assert!(!webhook_id.verify_checksum(other_environment, hmac_key));

        let mut of_other_agent = webhook_id.clone();
        of_other_agent.agent_id = "00000007-0000-8000-8000-000000000002".parse().unwrap();
        assert!(!of_other_agent.verify_checksum(environment_id, hmac_key));
    }

    #[test]
    fn durable_stream_policy_resolves_public_and_canonical_slot_names() {
        let policy = DurableStreamRoutePolicy {
            family_id: 37,
            slots: vec![
                DurableStreamSlot {
                    canonical_name: "input".into(),
                    public_name: "requests".into(),
                    direction: DurableStreamSlotDirection::Input,
                    content_type: "application/json".into(),
                    representation: DurableStreamRepresentation::Json,
                },
                DurableStreamSlot {
                    canonical_name: "$result".into(),
                    public_name: "responses".into(),
                    direction: DurableStreamSlotDirection::Output,
                    content_type: "application/vnd.golem.binary".into(),
                    representation: DurableStreamRepresentation::Bytes,
                },
            ],
            allow_external_writes: true,
            allow_stream_delete: true,
            allow_invocation_delete: true,
            load: None,
        };

        assert_eq!(
            policy
                .slot_by_public_name("requests")
                .map(|slot| slot.canonical_name.as_str()),
            Some("input")
        );
        assert_eq!(
            policy
                .slot_by_public_name("responses")
                .map(|slot| slot.canonical_name.as_str()),
            Some("$result")
        );
        assert_eq!(
            policy
                .slot_by_canonical_name("$result")
                .map(|slot| slot.public_name.as_str()),
            Some("responses")
        );
        assert!(policy.slot_by_public_name("input").is_none());
        assert!(policy.slot_by_canonical_name("responses").is_none());
    }
}
