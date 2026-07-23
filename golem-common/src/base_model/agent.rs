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

use crate::base_model::AgentId;
use crate::base_model::account::{AccountEmail, AccountId};
use crate::base_model::application::ApplicationId;
use crate::base_model::component::{ComponentId, ComponentRevision};
use crate::base_model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
use crate::base_model::diff::Hash as DiffHash;
use crate::base_model::environment::EnvironmentId;
use crate::model::Empty;
use crate::schema::AgentTypeSchema;
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

/// Content hash of an agent file. All files with identical content share the same hash.
#[derive(Copy, Debug, Clone, PartialEq, Eq, std::hash::Hash, Serialize, Deserialize)]
pub struct AgentFileContentHash(pub DiffHash);

impl std::fmt::Display for AgentFileContentHash {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for AgentFileContentHash {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        let bytes: [u8; 32] = *self.0.as_blake3_hash().as_bytes();
        desert_rust::BinarySerializer::serialize(&bytes, context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for AgentFileContentHash {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        let bytes = <[u8; 32] as desert_rust::BinaryDeserializer>::deserialize(context)?;
        Ok(AgentFileContentHash(DiffHash::new(
            blake3::Hash::from_bytes(bytes),
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RegisteredAgentTypeImplementer {
    pub component_id: ComponentId,
    pub component_revision: ComponentRevision,
    pub component_name: String,
    pub account_id: AccountId,
    pub account_email: AccountEmail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RegisteredAgentType {
    pub agent_type: AgentTypeSchema,
    pub implemented_by: RegisteredAgentTypeImplementer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
/// RegisteredAgentType with deployment specific information
/// Deployment related information can only be safely used if it is information of the _currently deployed_ component revision.
pub struct DeployedRegisteredAgentType {
    pub agent_type: AgentTypeSchema,
    pub implemented_by: RegisteredAgentTypeImplementer,
    pub webhook_prefix_authority_and_path: Option<String>,
}

impl From<DeployedRegisteredAgentType> for RegisteredAgentType {
    fn from(value: DeployedRegisteredAgentType) -> Self {
        Self {
            agent_type: value.agent_type,
            implemented_by: value.implemented_by,
        }
    }
}

/// Result of resolving an agent type by names, bundling the agent type
/// with the environment it belongs to.
#[derive(Clone)]
pub struct ResolvedAgentType {
    pub registered_agent_type: RegisteredAgentType,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    // Present only when resolving at the current deployment (no explicit deployment
    // revision requested).
    pub current_deployment_revision: Option<CurrentDeploymentRevision>,
}

/// Event received from the registry service when any registry state changes.
#[derive(Debug, Clone)]
pub enum RegistryInvalidationEvent {
    /// The client's cursor is older than the server's retention window.
    CursorExpired { event_id: u64 },
    /// A deployment changed for an environment.
    DeploymentChanged {
        event_id: u64,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        current_deployment_revision: CurrentDeploymentRevision,
    },
    /// Domain registrations changed (created/deleted).
    DomainRegistrationChanged {
        event_id: u64,
        environment_id: EnvironmentId,
        domains: Vec<String>,
    },
    /// An account's tokens were invalidated.
    AccountTokensInvalidated {
        event_id: u64,
        account_id: AccountId,
    },
    /// Environment sharing permissions changed.
    EnvironmentPermissionsChanged {
        event_id: u64,
        environment_id: EnvironmentId,
        grantee_account_id: AccountId,
    },
    /// A security scheme was created, updated, or deleted.
    SecuritySchemeChanged {
        event_id: u64,
        environment_id: EnvironmentId,
    },
    /// An environment retry policy was created, updated, or deleted.
    RetryPolicyChanged {
        event_id: u64,
        environment_id: EnvironmentId,
    },
    /// A resource definition was created, updated, or deleted.
    ResourceDefinitionChanged {
        event_id: u64,
        environment_id: EnvironmentId,
        resource_definition_id: crate::base_model::quota::ResourceDefinitionId,
        resource_name: crate::base_model::quota::ResourceName,
    },
    /// An agent secret was created, updated, or deleted.
    AgentSecretChanged {
        event_id: u64,
        environment_id: EnvironmentId,
    },
    /// An application was deleted. Subscribers should flush any cache entries
    /// keyed on the application's name so that a subsequent same-name recreation
    /// resolves through a fresh lookup rather than returning stale identifiers
    /// for the now-orphaned environments/components.
    ApplicationDeleted {
        event_id: u64,
        application_id: ApplicationId,
        account_id: AccountId,
        /// Human-readable application name (matches AgentResolutionCache key).
        app_name: String,
        /// All non-deleted environment UUIDs under this application at deletion time.
        environment_ids: Vec<EnvironmentId>,
    },
    /// An environment was deleted. Subscribers should flush any cache entries
    /// scoped to this environment so that a same-name recreation does not serve
    /// stale resolutions pointing at the deleted environment's components.
    EnvironmentDeleted {
        event_id: u64,
        environment_id: EnvironmentId,
        /// Human-readable application name (matches AgentResolutionCache key).
        app_name: String,
        /// Human-readable environment name (matches AgentResolutionCache key).
        env_name: String,
    },
    /// A permission card was revoked or deleted.
    CardRevoked { event_id: u64, card_ids: Vec<Uuid> },
}

impl RegistryInvalidationEvent {
    pub fn event_id(&self) -> u64 {
        match self {
            Self::CursorExpired { event_id } => *event_id,
            Self::DeploymentChanged { event_id, .. } => *event_id,
            Self::DomainRegistrationChanged { event_id, .. } => *event_id,
            Self::AccountTokensInvalidated { event_id, .. } => *event_id,
            Self::EnvironmentPermissionsChanged { event_id, .. } => *event_id,
            Self::SecuritySchemeChanged { event_id, .. } => *event_id,
            Self::RetryPolicyChanged { event_id, .. } => *event_id,
            Self::ResourceDefinitionChanged { event_id, .. } => *event_id,
            Self::AgentSecretChanged { event_id, .. } => *event_id,
            Self::ApplicationDeleted { event_id, .. } => *event_id,
            Self::EnvironmentDeleted { event_id, .. } => *event_id,
            Self::CardRevoked { event_id, .. } => *event_id,
        }
    }
}

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[repr(i32)]
pub enum AgentMode {
    Durable = 0,
    Ephemeral = 1,
}

impl Display for AgentMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentMode::Durable => write!(f, "Durable"),
            AgentMode::Ephemeral => write!(f, "Ephemeral"),
        }
    }
}

impl FromStr for AgentMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "durable" => Ok(AgentMode::Durable),
            "ephemeral" => Ok(AgentMode::Ephemeral),
            _ => Err(format!("Unknown agent mode: {s}")),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentInvocationMode {
    Await,
    Schedule,
    Lookup,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryType {
    pub mime_type: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextDescriptor {
    pub restrictions: Option<Vec<TextType>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextType {
    pub language_code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct Url {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextSource {
    pub data: String,
    pub text_type: Option<TextType>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum TextReference {
    Url(Url),
    Inline(TextSource),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryDescriptor {
    pub restrictions: Option<Vec<BinaryType>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinarySource {
    pub data: Vec<u8>,
    pub binary_type: BinaryType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum BinaryReference {
    Url(Url),
    Inline(BinarySource),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ReadOnlyConfig {
    pub cache_policy: CachePolicy,
    pub uses_principal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum CachePolicy {
    NoCache(Empty),
    UntilWrite(Empty),
    Ttl(CachePolicyTtl),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct CachePolicyTtl {
    pub duration_nanos: u64,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Deserialize,
    Serialize,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(poem_openapi::NewType, desert_rust::BinaryCodec)
)]
#[repr(transparent)]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AgentTypeName(pub String);

impl Display for AgentTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for AgentTypeName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

impl AgentTypeName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct HttpMountDetails {
    pub path_prefix: Vec<PathSegment>,
    pub auth_details: Option<AgentHttpAuthDetails>,
    pub phantom_agent: bool,
    pub cors_options: CorsOptions,
    pub webhook_suffix: Vec<PathSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct HttpEndpointDetails {
    pub http_method: HttpMethod,
    pub path_suffix: Vec<PathSegment>,
    pub header_vars: Vec<HeaderVariable>,
    pub query_vars: Vec<QueryVariable>,
    pub auth_details: Option<AgentHttpAuthDetails>,
    pub cors_options: CorsOptions,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum HttpMethod {
    Get(Empty),
    Head(Empty),
    Post(Empty),
    Put(Empty),
    Delete(Empty),
    Connect(Empty),
    Options(Empty),
    Trace(Empty),
    Patch(Empty),
    Custom(CustomHttpMethod),
}

#[cfg(feature = "full")]
impl TryFrom<HttpMethod> for http::Method {
    type Error = anyhow::Error;

    fn try_from(value: HttpMethod) -> Result<Self, Self::Error> {
        match value {
            HttpMethod::Get(_) => Ok(http::Method::GET),
            HttpMethod::Head(_) => Ok(http::Method::HEAD),
            HttpMethod::Post(_) => Ok(http::Method::POST),
            HttpMethod::Put(_) => Ok(http::Method::PUT),
            HttpMethod::Delete(_) => Ok(http::Method::DELETE),
            HttpMethod::Connect(_) => Ok(http::Method::CONNECT),
            HttpMethod::Options(_) => Ok(http::Method::OPTIONS),
            HttpMethod::Trace(_) => Ok(http::Method::TRACE),
            HttpMethod::Patch(_) => Ok(http::Method::PATCH),
            HttpMethod::Custom(custom) => {
                let converted = http::Method::from_bytes(custom.value.as_bytes())?;
                Ok(converted)
            }
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct CustomHttpMethod {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct CorsOptions {
    pub allowed_patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PathSegment {
    Literal(LiteralSegment),
    SystemVariable(SystemVariableSegment),
    PathVariable(PathVariable),
    RemainingPathVariable(PathVariable),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct LiteralSegment {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct SystemVariableSegment {
    pub value: SystemVariable,
}

#[derive(
    Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
pub enum SystemVariable {
    AgentType,
    AgentVersion,
}

impl Display for SystemVariable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SystemVariable::AgentType => "AgentType",
            SystemVariable::AgentVersion => "AgentVersion",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PathVariable {
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct HeaderVariable {
    pub header_name: String,
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct QueryVariable {
    pub query_param_name: String,
    pub variable_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum Snapshotting {
    Disabled(Empty),
    Enabled(SnapshottingConfig),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(
    feature = "full",
    oai(discriminator_name = "configType", one_of = true)
)]
#[serde(tag = "configType")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum SnapshottingConfig {
    Default(Empty),
    Periodic(SnapshottingPeriodic),
    EveryNInvocation(SnapshottingEveryNInvocation),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SnapshottingPeriodic {
    pub duration_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SnapshottingEveryNInvocation {
    pub count: u16,
}

#[derive(
    Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[repr(i32)]
pub enum AgentConfigSource {
    Local = 0,
    Secret = 1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentHttpAuthDetails {
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum Principal {
    Oidc(OidcPrincipal),
    Agent(AgentPrincipal),
    GolemUser(GolemUserPrincipal),
    Anonymous(Empty),
}

impl Principal {
    pub fn anonymous() -> Self {
        Self::Anonymous(Empty {})
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
// Meaning of the various claims: https://openid.net/specs/openid-connect-core-1_0.html#StandardClaims
pub struct OidcPrincipal {
    pub sub: String,
    pub issuer: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub email_verified: Option<bool>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub picture: Option<String>,
    pub preferred_username: Option<String>,
    pub claims: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentPrincipal {
    pub agent_id: AgentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct GolemUserPrincipal {
    pub account_id: AccountId,
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, OutputSchema,
    };
    use crate::schema::graph::SchemaGraph;

    fn mock_method(name: &str, read_only: Option<ReadOnlyConfig>) -> AgentMethodSchema {
        AgentMethodSchema {
            name: name.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(Vec::new()),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only,
        }
    }

    fn mock_agent_type(mode: AgentMode, methods: Vec<AgentMethodSchema>) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: AgentTypeName("Test".to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(Vec::new()),
            },
            methods,
            dependencies: Vec::new(),
            mode,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: Vec::new(),
        }
    }

    #[test]
    fn validate_accepts_durable_agent_with_read_only_methods() {
        let agent = mock_agent_type(
            AgentMode::Durable,
            vec![mock_method(
                "get",
                Some(ReadOnlyConfig {
                    cache_policy: CachePolicy::UntilWrite(Empty {}),
                    uses_principal: false,
                }),
            )],
        );
        assert!(agent.validate().is_ok());
    }

    #[test]
    fn validate_accepts_ephemeral_agent_without_read_only_methods() {
        let agent = mock_agent_type(AgentMode::Ephemeral, vec![mock_method("do", None)]);
        assert!(agent.validate().is_ok());
    }

    #[test]
    fn validate_rejects_ephemeral_agent_with_read_only_method() {
        let agent = mock_agent_type(
            AgentMode::Ephemeral,
            vec![mock_method(
                "get",
                Some(ReadOnlyConfig {
                    cache_policy: CachePolicy::UntilWrite(Empty {}),
                    uses_principal: false,
                }),
            )],
        );
        let result = agent.validate();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("ephemeral"));
        assert!(msg.contains("read-only"));
        assert!(msg.contains("Test"));
        assert!(msg.contains("get"));
    }

    // -----------------------------------------------------------------------
    // Read-only / cache-policy codec round-trip (issue #3393)
    //
    // Read-only metadata is carried inside `AgentMethodSchema.read_only` and
    // uses every cache-policy variant. The same `AgentMethodSchema` value must
    // travel unchanged through every codec we expose: `serde`, `desert_rust`
    // (BinaryCodec), and `poem_openapi`.
    //
    // Also covers the schema-migration case: a payload without the `read_only`
    // field must deserialize with `read_only == None`.
    // -----------------------------------------------------------------------

    fn all_cache_policies() -> Vec<CachePolicy> {
        vec![
            CachePolicy::NoCache(Empty {}),
            CachePolicy::UntilWrite(Empty {}),
            CachePolicy::Ttl(CachePolicyTtl {
                duration_nanos: 1_234_567,
            }),
        ]
    }

    fn all_read_only_methods() -> Vec<AgentMethodSchema> {
        let mut out = vec![mock_method("not-read-only", None)];
        for policy in all_cache_policies() {
            for uses_principal in [false, true] {
                out.push(mock_method(
                    "ro",
                    Some(ReadOnlyConfig {
                        cache_policy: policy.clone(),
                        uses_principal,
                    }),
                ));
            }
        }
        out
    }

    #[test]
    fn agent_method_read_only_roundtrips_through_serde_json() {
        for method in all_read_only_methods() {
            let json = serde_json::to_string(&method).expect("serde_json::to_string");
            let back: AgentMethodSchema =
                serde_json::from_str(&json).expect("serde_json::from_str");
            assert_eq!(back, method, "serde JSON round-trip mismatch");
        }
    }

    #[test]
    fn agent_method_read_only_roundtrips_through_desert() {
        for method in all_read_only_methods() {
            let bytes =
                desert_rust::serialize_to_byte_vec(&method).expect("desert_rust serialization");
            let back: AgentMethodSchema =
                desert_rust::deserialize(&bytes).expect("desert_rust deserialization");
            assert_eq!(back, method, "desert (BinaryCodec) round-trip mismatch");
        }
    }

    #[test]
    fn agent_method_read_only_roundtrips_through_poem_openapi() {
        use poem_openapi::types::{ParseFromJSON, ToJSON};

        for method in all_read_only_methods() {
            let json = ToJSON::to_json(&method);
            let back = <AgentMethodSchema as ParseFromJSON>::parse_from_json(json)
                .expect("poem_openapi parse_from_json");
            assert_eq!(back, method, "poem_openapi round-trip mismatch");
        }
    }

    /// A payload missing the `read_only` field must deserialize with
    /// `read_only == None`, verifying the `#[serde(default)]` and
    /// `desert(evolution)` contracts.
    #[test]
    fn agent_method_without_read_only_field_defaults_to_none_via_serde() {
        let method = mock_method("legacy", None);
        let mut json = serde_json::to_value(&method).expect("to_value");
        json.as_object_mut().unwrap().remove("read_only");
        json.as_object_mut().unwrap().remove("readOnly");
        let back: AgentMethodSchema =
            serde_json::from_value(json).expect("payload without read_only must deserialize");
        assert_eq!(back.name, "legacy");
        assert!(
            back.read_only.is_none(),
            "missing read_only field must default to None"
        );
    }
}
