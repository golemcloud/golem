// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Runtime agent reflection and schema-free invocation.

pub use super::tool_reflection::{
    DynamicToolClient, ReflectedToolClient, ReflectedToolCustomError, ToolArgument,
    ToolArgumentKind, ToolClientCommandDefinition, ToolClientDefinition,
    ToolClientDefinitionBuilder, ToolCommand, ToolNode, ToolReflectionError, ToolType,
    TypedToolClient, TypedToolCommand, TypedUnitToolCommand, get_all_tool_types, get_tool_type,
};

use crate::bindings::golem::agent::{common as wire_common, host};
use crate::permissions::ScopeCard;
use crate::schema::render::{
    RenderError, from_json_value, to_json_value, to_reflection_json_schema,
};
use crate::schema::validation::validate_value;
use crate::schema::{
    MetadataEnvelope, NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
};
use crate::{AgentId, ScheduledTime, Uuid};
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

/// A graph-backed view of one schema root.
#[derive(Clone, Debug)]
pub struct SchemaRef {
    graph: Arc<SchemaGraph>,
    root: SchemaType,
    root_field_defaults: Arc<[Option<SchemaValue>]>,
}

impl SchemaRef {
    pub fn new(graph: SchemaGraph) -> Self {
        let root = graph.root.clone();
        Self {
            graph: Arc::new(graph),
            root,
            root_field_defaults: Arc::new([]),
        }
    }

    pub(crate) fn with_root(graph: Arc<SchemaGraph>, root: SchemaType) -> Self {
        Self {
            graph,
            root,
            root_field_defaults: Arc::new([]),
        }
    }

    /// Attaches per-field defaults to a record root. They apply only to the JSON surface: JSON
    /// packing fills an omitted field with its default, and the rendered JSON Schema neither
    /// requires such a field nor hides its default. The value surface keeps the plain record.
    pub(crate) fn with_root_field_defaults(mut self, defaults: Vec<Option<SchemaValue>>) -> Self {
        debug_assert!(
            matches!(&self.root, SchemaType::Record { fields, .. } if fields.len() == defaults.len())
        );
        if defaults.iter().any(Option::is_some) {
            self.root_field_defaults = defaults.into();
        }
        self
    }

    #[cfg(feature = "json")]
    fn root_record_with_defaults(&self) -> Option<&[NamedFieldType]> {
        match &self.root {
            SchemaType::Record { fields, .. }
                if !self.root_field_defaults.is_empty()
                    && fields.len() == self.root_field_defaults.len() =>
            {
                Some(fields)
            }
            _ => None,
        }
    }

    #[cfg(feature = "json")]
    fn decode_json(&self, value: &serde_json::Value) -> Result<SchemaValue, RenderError> {
        let (Some(fields), Some(object)) = (self.root_record_with_defaults(), value.as_object())
        else {
            return from_json_value(&self.graph, &self.root, value);
        };
        let omitted = fields
            .iter()
            .zip(self.root_field_defaults.iter())
            .map(|(field, default)| {
                default
                    .as_ref()
                    .filter(|_| !object.contains_key(&field.name))
            })
            .collect::<Vec<_>>();
        if omitted.iter().all(Option::is_none) {
            return from_json_value(&self.graph, &self.root, value);
        }
        let mut present = self.root.clone();
        if let SchemaType::Record { fields, .. } = &mut present {
            let mut index = 0;
            fields.retain(|_| {
                let keep = omitted[index].is_none();
                index += 1;
                keep
            });
        }
        let SchemaValue::Record { fields: decoded } =
            from_json_value(&self.graph, &present, value)?
        else {
            unreachable!("a record schema decodes to a record value")
        };
        let mut decoded = decoded.into_iter();
        let fields = omitted
            .into_iter()
            .map(|default| match default {
                Some(default) => default.clone(),
                None => decoded.next().expect("decoded present field"),
            })
            .collect();
        Ok(SchemaValue::Record { fields })
    }

    #[cfg(test)]
    pub(crate) fn shares_definition_pool_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.graph, &other.graph)
    }

    pub fn graph(&self) -> &SchemaGraph {
        &self.graph
    }

    pub fn root(&self) -> &SchemaType {
        &self.root
    }

    pub fn contains_stream(&self) -> bool {
        super::schema_type_contains_stream(&self.root, &self.graph.defs)
    }

    pub fn validate_value(&self, value: &SchemaValue) -> Result<(), GolemReflectError> {
        validate_value(&self.graph, &self.root, value).map_err(|errors| {
            GolemReflectError::InvalidSchemaValue {
                issues: errors.into_iter().map(|error| error.to_string()).collect(),
            }
        })
    }

    #[cfg(feature = "json")]
    pub fn validate_json(
        &self,
        value: &serde_json::Value,
    ) -> Result<SchemaValue, GolemReflectError> {
        self.pack_json(value)
    }

    #[cfg(feature = "json")]
    pub fn pack_json(&self, value: &serde_json::Value) -> Result<SchemaValue, GolemReflectError> {
        let packed = self.decode_json(value)?;
        self.validate_value(&packed)?;
        Ok(packed)
    }

    #[cfg(feature = "json")]
    pub fn unpack_json(&self, value: &SchemaValue) -> Result<serde_json::Value, GolemReflectError> {
        self.validate_value(value)?;
        self.unpack_checked_json(value)
    }

    #[cfg(feature = "json")]
    pub(crate) fn unpack_checked_json(
        &self,
        value: &SchemaValue,
    ) -> Result<serde_json::Value, GolemReflectError> {
        Ok(to_json_value(&self.graph, &self.root, value)?)
    }

    #[cfg(feature = "json")]
    pub fn to_json_schema(&self, include_draft_marker: bool) -> serde_json::Value {
        let mut schema = to_reflection_json_schema(&self.graph, &self.root, include_draft_marker);
        if let Some(fields) = self.root_record_with_defaults() {
            for (field, default) in fields.iter().zip(self.root_field_defaults.iter()) {
                let Some(default) = default else {
                    continue;
                };
                if let Some(required) = schema
                    .get_mut("required")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    required.retain(|name| name.as_str() != Some(field.name.as_str()));
                }
                if let Ok(default) = to_json_value(&self.graph, &field.body, default)
                    && let Some(property) = schema
                        .get_mut("properties")
                        .and_then(|properties| properties.get_mut(&field.name))
                        .and_then(serde_json::Value::as_object_mut)
                {
                    property.insert("default".to_string(), default);
                }
            }
        }
        schema
    }
}

/// A closed, structured error surface shared by reflection and invocation.
#[derive(Debug)]
pub enum GolemReflectError {
    AgentTypeNotFound(String),
    MethodNotFound {
        agent_type: String,
        method: String,
    },
    InvalidAgentId(String),
    InvalidInput(String),
    InvalidType(String),
    KnownEphemeralBinding(String),
    InvalidSchemaValue {
        issues: Vec<String>,
    },
    MalformedRemoteOutput(String),
    SchemaRender(RenderError),
    SchemaEncode(String),
    SchemaDecode(String),
    ProtocolError(String),
    Denied(String),
    RemoteNotFound(String),
    RemoteInternalError(String),
    RemoteAgent(RemoteAgentError),
    Conflict(String),
    /// The awaited invocation was cancelled before it started.
    Cancelled(String),
    /// A cancellation found the invocation already running.
    CannotCancelRunning(String),
}

#[derive(Debug)]
pub enum RemoteAgentError {
    InvalidInput(String),
    InvalidMethod(String),
    InvalidType(String),
    InvalidAgentId(String),
    Custom(Box<TypedSchemaValue>),
    MalformedCustomError(String),
}

impl Display for GolemReflectError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentTypeNotFound(name) => write!(f, "agent type `{name}` was not found"),
            Self::MethodNotFound { agent_type, method } => {
                write!(f, "agent type `{agent_type}` has no method `{method}`")
            }
            Self::InvalidAgentId(message) => write!(f, "invalid agent id: {message}"),
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::InvalidType(message) => write!(f, "invalid type: {message}"),
            Self::KnownEphemeralBinding(name) => {
                write!(
                    f,
                    "cannot bind an existing identity to ephemeral agent type `{name}`"
                )
            }
            Self::InvalidSchemaValue { issues } => {
                write!(f, "schema value is invalid: {}", issues.join("; "))
            }
            Self::MalformedRemoteOutput(message) => {
                write!(f, "malformed remote output: {message}")
            }
            Self::SchemaRender(error) => write!(f, "schema rendering failed: {error}"),
            Self::SchemaEncode(message) => write!(f, "schema encoding failed: {message}"),
            Self::SchemaDecode(message) => write!(f, "schema decoding failed: {message}"),
            Self::ProtocolError(message) => write!(f, "RPC protocol error: {message}"),
            Self::Denied(message) => write!(f, "RPC access denied: {message}"),
            Self::RemoteNotFound(message) => write!(f, "remote target not found: {message}"),
            Self::RemoteInternalError(message) => {
                write!(f, "remote internal error: {message}")
            }
            Self::RemoteAgent(error) => write!(f, "remote agent error: {error}"),
            Self::Conflict(message) => write!(f, "agent conflict: {message}"),
            Self::Cancelled(message) => write!(f, "invocation cancelled: {message}"),
            Self::CannotCancelRunning(message) => {
                write!(f, "cannot cancel a running invocation: {message}")
            }
        }
    }
}

impl Display for RemoteAgentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::InvalidMethod(message) => write!(f, "invalid method: {message}"),
            Self::InvalidType(message) => write!(f, "invalid type: {message}"),
            Self::InvalidAgentId(message) => write!(f, "invalid agent id: {message}"),
            Self::Custom(_) => write!(f, "custom typed error"),
            Self::MalformedCustomError(message) => {
                write!(f, "malformed custom error: {message}")
            }
        }
    }
}

impl std::error::Error for GolemReflectError {}
impl std::error::Error for RemoteAgentError {}

impl From<RenderError> for GolemReflectError {
    fn from(value: RenderError) -> Self {
        Self::SchemaRender(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentMode {
    Durable,
    Ephemeral,
}

#[derive(Clone, Debug)]
pub struct AgentMethod {
    agent_type_name: String,
    raw: wire_common::AgentMethod,
    input: SchemaRef,
    output: Option<SchemaRef>,
}

impl AgentMethod {
    pub fn name(&self) -> &str {
        &self.raw.name
    }

    pub fn description(&self) -> &str {
        &self.raw.description
    }

    pub fn prompt_hint(&self) -> Option<&str> {
        self.raw.prompt_hint.as_deref()
    }

    pub fn input(&self) -> &SchemaRef {
        &self.input
    }

    pub fn output(&self) -> Option<&SchemaRef> {
        self.output.as_ref()
    }

    pub fn raw_wit(&self) -> &wire_common::AgentMethod {
        &self.raw
    }

    pub fn agent_type_name(&self) -> &str {
        &self.agent_type_name
    }

    pub fn http_endpoints(&self) -> &[wire_common::HttpEndpointDetails] {
        &self.raw.http_endpoint
    }

    pub fn read_only(&self) -> Option<&wire_common::ReadOnlyConfig> {
        self.raw.read_only.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct AgentType {
    raw: Arc<wire_common::AgentType>,
    graph: Arc<SchemaGraph>,
    constructor_input: SchemaRef,
    methods: Arc<[AgentMethod]>,
    config_schemas: Arc<[(Vec<String>, SchemaRef)]>,
    mode: AgentMode,
}

impl AgentType {
    fn from_registered(registered: host::RegisteredAgentType) -> Result<Self, GolemReflectError> {
        let raw = Arc::new(registered.agent_type);
        let decoder = crate::schema::wit::GraphDecoder::new(&raw.schema)
            .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?;
        let graph = Arc::new(SchemaGraph {
            defs: decoder
                .decode_defs()
                .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?,
            root: decode_root(&decoder, raw.schema.root)?,
        });
        let constructor_input = input_schema_ref(&decoder, &raw.constructor.input_schema, &graph)?;
        let methods = raw
            .methods
            .iter()
            .map(|method| {
                Ok(AgentMethod {
                    agent_type_name: raw.type_name.clone(),
                    raw: method.clone(),
                    input: input_schema_ref(&decoder, &method.input_schema, &graph)?,
                    output: output_schema_ref(&decoder, &method.output_schema, &graph)?,
                })
            })
            .collect::<Result<Vec<_>, GolemReflectError>>()?;
        let config_schemas = raw
            .config
            .iter()
            .map(|declaration| {
                Ok((
                    declaration.path.clone(),
                    SchemaRef::with_root(
                        graph.clone(),
                        decode_root(&decoder, declaration.value_type)?,
                    ),
                ))
            })
            .collect::<Result<Vec<_>, GolemReflectError>>()?;
        let mode = match raw.mode {
            wire_common::AgentMode::Durable => AgentMode::Durable,
            wire_common::AgentMode::Ephemeral => AgentMode::Ephemeral,
        };
        Ok(Self {
            raw,
            graph,
            constructor_input,
            methods: methods.into(),
            config_schemas: config_schemas.into(),
            mode,
        })
    }

    pub fn name(&self) -> &str {
        &self.raw.type_name
    }

    pub fn description(&self) -> &str {
        &self.raw.description
    }

    pub fn source_language(&self) -> &str {
        &self.raw.source_language
    }

    pub fn mode(&self) -> AgentMode {
        self.mode
    }

    pub fn schema_graph(&self) -> &SchemaGraph {
        &self.graph
    }

    pub fn constructor_input(&self) -> &SchemaRef {
        &self.constructor_input
    }

    pub fn methods(&self) -> &[AgentMethod] {
        &self.methods
    }

    pub fn method(&self, name: &str) -> Result<AgentMethod, GolemReflectError> {
        self.methods
            .iter()
            .find(|method| method.name() == name)
            .cloned()
            .ok_or_else(|| GolemReflectError::MethodNotFound {
                agent_type: self.name().to_string(),
                method: name.to_string(),
            })
    }

    pub fn raw_wit(&self) -> &wire_common::AgentType {
        &self.raw
    }

    pub fn constructor(&self) -> &wire_common::AgentConstructor {
        &self.raw.constructor
    }

    pub fn dependencies(&self) -> &[wire_common::AgentDependency] {
        &self.raw.dependencies
    }

    pub fn http_mount(&self) -> Option<&wire_common::HttpMountDetails> {
        self.raw.http_mount.as_ref()
    }

    pub fn snapshotting(&self) -> &wire_common::Snapshotting {
        &self.raw.snapshotting
    }

    pub fn config(&self) -> &[wire_common::AgentConfigDeclaration] {
        &self.raw.config
    }

    fn config_schema(&self, path: &[String]) -> Result<SchemaRef, GolemReflectError> {
        let declaration = self
            .raw
            .config
            .iter()
            .find(|declaration| declaration.path == path)
            .ok_or_else(|| {
                GolemReflectError::InvalidInput(format!("unknown config path `{}`", path.join(".")))
            })?;
        if declaration.source == wire_common::AgentConfigSource::Secret {
            return Err(GolemReflectError::InvalidInput(format!(
                "secret config field `{}` cannot be overridden over RPC",
                path.join(".")
            )));
        }
        self.config_schemas
            .iter()
            .find(|(candidate, _)| candidate == path)
            .map(|(_, schema)| schema.clone())
            .ok_or_else(|| {
                GolemReflectError::InvalidInput(format!("unknown config path `{}`", path.join(".")))
            })
    }

    fn validate_config(
        &self,
        config: Vec<AgentConfigValue>,
    ) -> Result<Vec<AgentConfigValue>, GolemReflectError> {
        config
            .into_iter()
            .map(|entry| {
                let schema = self.config_schema(&entry.path)?;
                schema.validate_value(entry.value.value())?;
                Ok(AgentConfigValue {
                    path: entry.path,
                    value: TypedSchemaValue::new(
                        SchemaGraph {
                            defs: schema.graph.defs.clone(),
                            root: schema.root.clone(),
                        },
                        entry.value.value().clone(),
                    ),
                })
            })
            .collect()
    }

    #[cfg(feature = "json")]
    pub fn pack_config_json(
        &self,
        config: Vec<(Vec<String>, serde_json::Value)>,
    ) -> Result<Vec<AgentConfigValue>, GolemReflectError> {
        config
            .into_iter()
            .map(|(path, json)| {
                let schema = self.config_schema(&path)?;
                Ok(AgentConfigValue {
                    path,
                    value: TypedSchemaValue::new(
                        SchemaGraph {
                            defs: schema.graph.defs.clone(),
                            root: schema.root.clone(),
                        },
                        schema.pack_json(&json)?,
                    ),
                })
            })
            .collect()
    }

    pub fn client(&self) -> ReflectedAgentClientFactory {
        ReflectedAgentClientFactory {
            agent_type: self.clone(),
        }
    }

    /// Gets the ID of the keyed agent of this type and constructor value, or `None` when no
    /// such agent exists. Never creates the agent.
    pub fn lookup_agent_id_value(
        &self,
        constructor: SchemaValue,
    ) -> Result<Option<AgentId>, GolemReflectError> {
        self.constructor_input.validate_value(&constructor)?;
        super::identity::lookup(self.name(), &constructor)
    }

    #[cfg(feature = "json")]
    pub fn lookup_agent_id_json(
        &self,
        constructor: &serde_json::Value,
    ) -> Result<Option<AgentId>, GolemReflectError> {
        self.lookup_agent_id_value(self.constructor_input.pack_json(constructor)?)
    }
}

pub fn get_all_agent_types() -> Result<Vec<AgentType>, GolemReflectError> {
    host::get_all_agent_types()
        .into_iter()
        .map(AgentType::from_registered)
        .collect()
}

pub fn get_agent_type(name: &str) -> Result<AgentType, GolemReflectError> {
    let registered = host::get_agent_type(name)
        .ok_or_else(|| GolemReflectError::AgentTypeNotFound(name.to_string()))?;
    AgentType::from_registered(registered)
}

/// Gets the deployed agent type that the type tag of `agent_id` names.
pub fn get_agent_type_for(agent_id: &AgentId) -> Result<AgentType, GolemReflectError> {
    let registered = host::get_agent_type_by_agent_id(agent_id.into())
        .ok_or_else(|| GolemReflectError::AgentTypeNotFound(agent_id.uuid.to_string()))?;
    AgentType::from_registered(registered)
}

/// Options of a `create` call.
#[derive(Clone, Debug, Default)]
pub struct CreateOptions<C = ()> {
    /// Creates the keyed agent of the constructor parameters, and fails with
    /// [`GolemReflectError::Conflict`] when it already exists. Otherwise the new agent has no key
    /// and a new ID.
    pub keyed: bool,
    /// Overrides of the agent type's local configuration values.
    pub config: C,
    /// Environment variables added to the ones the manifest defines.
    pub env: Vec<(String, String)>,
}

impl<C> CreateOptions<C> {
    fn try_map_config<D>(
        self,
        f: impl FnOnce(C) -> Result<D, GolemReflectError>,
    ) -> Result<CreateOptions<D>, GolemReflectError> {
        Ok(CreateOptions {
            keyed: self.keyed,
            config: f(self.config)?,
            env: self.env,
        })
    }
}

#[derive(Clone, Debug)]
pub struct AgentConfigValue {
    pub path: Vec<String>,
    pub value: TypedSchemaValue,
}

#[derive(Clone, Debug)]
pub struct ReflectedAgentClientFactory {
    agent_type: AgentType,
}

impl ReflectedAgentClientFactory {
    /// Connects to the keyed agent of the constructor value, and creates it when it is missing.
    /// For an ephemeral type, each invocation runs on a new one-shot instance.
    pub fn upsert_value(
        &self,
        constructor: SchemaValue,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        self.agent_type
            .constructor_input
            .validate_value(&constructor)?;
        let transport = RpcTransport::upsert(self.agent_type.name(), &constructor)?;
        Ok(self.client(transport))
    }

    #[cfg(feature = "json")]
    pub fn upsert_json(
        &self,
        constructor: &serde_json::Value,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        self.upsert_value(self.agent_type.constructor_input.pack_json(constructor)?)
    }

    /// Creates a durable agent of the constructor value. With `keyed`, it creates the keyed
    /// agent and fails with [`GolemReflectError::Conflict`] when it already exists.
    pub async fn create_value(
        &self,
        constructor: SchemaValue,
        options: CreateOptions<Vec<AgentConfigValue>>,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        if self.agent_type.mode != AgentMode::Durable {
            return Err(GolemReflectError::InvalidType(
                "create is available only for durable agent types".to_string(),
            ));
        }
        self.agent_type
            .constructor_input
            .validate_value(&constructor)?;
        let options = options.try_map_config(|config| {
            super::identity::encode_config(self.agent_type.validate_config(config)?)
        })?;
        let transport = RpcTransport::create(self.agent_type.name(), &constructor, options).await?;
        Ok(self.client(transport))
    }

    #[cfg(feature = "json")]
    pub async fn create_json(
        &self,
        constructor: &serde_json::Value,
        options: CreateOptions<Vec<(Vec<String>, serde_json::Value)>>,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        let constructor = self.agent_type.constructor_input.pack_json(constructor)?;
        let options = options.try_map_config(|config| self.agent_type.pack_config_json(config))?;
        self.create_value(constructor, options).await
    }

    /// Connects to the existing agent with the given ID. Never creates the agent; an unknown
    /// ID or the ID of an agent of another type is [`GolemReflectError::RemoteNotFound`].
    pub fn get_by_id(&self, agent_id: &AgentId) -> Result<ReflectedAgentClient, GolemReflectError> {
        if self.agent_type.mode == AgentMode::Ephemeral {
            return Err(GolemReflectError::KnownEphemeralBinding(
                self.agent_type.name().to_string(),
            ));
        }
        let transport = RpcTransport::get_by_id(self.agent_type.name(), agent_id)?;
        Ok(self.client(transport))
    }

    fn client(&self, transport: RpcTransport) -> ReflectedAgentClient {
        ReflectedAgentClient {
            agent_type: self.agent_type.clone(),
            transport: ReflectedTransport::Host(Rc::new(transport)),
        }
    }
}

#[derive(Clone)]
pub struct ReflectedAgentClient {
    agent_type: AgentType,
    transport: ReflectedTransport,
}

impl std::fmt::Debug for ReflectedAgentClient {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReflectedAgentClient")
            .field("agent_type", &self.agent_type.name())
            .finish_non_exhaustive()
    }
}

impl ReflectedAgentClient {
    /// The ID of the agent this client connects to. On a client of `upsert` it creates the
    /// agent when it is missing, without invoking a method.
    pub async fn agent_id(&self) -> Result<AgentId, GolemReflectError> {
        self.transport.agent_id().await
    }

    pub fn method(&self, name: &str) -> Result<ReflectedAgentMethod, GolemReflectError> {
        Ok(ReflectedAgentMethod {
            definition: self.agent_type.method(name)?,
            transport: self.transport.clone(),
        })
    }
}

#[derive(Clone)]
pub struct ReflectedAgentMethod {
    definition: AgentMethod,
    transport: ReflectedTransport,
}

#[derive(Clone)]
enum ReflectedTransport {
    Host(Rc<RpcTransport>),
    #[cfg(test)]
    Test(Rc<TestReflectedTransport>),
}

#[cfg(test)]
struct TestReflectedTransport {
    value: Option<SchemaValue>,
}

#[cfg(test)]
impl TestReflectedTransport {
    fn completion(&self) -> InvocationOutput<Option<SchemaValue>> {
        InvocationOutput {
            metadata: InvocationMetadata {
                agent_id: AgentId::new(Uuid::nil()),
                invocation_id: OwnedInvocationId::new(
                    AgentId::new(Uuid::nil()),
                    InvocationId(Uuid::nil()),
                ),
            },
            value: self.value.clone(),
        }
    }

    async fn invoke_and_await(
        &self,
        _method: &str,
        _input: SchemaValue,
        _scope_card: Option<&ScopeCard>,
    ) -> AwaitedResult {
        Ok(self.completion())
    }
}

type AwaitedResult = Result<InvocationOutput<Option<SchemaValue>>, GolemReflectError>;

impl ReflectedTransport {
    async fn agent_id(&self) -> Result<AgentId, GolemReflectError> {
        match self {
            Self::Host(transport) => transport.agent_id().await,
            #[cfg(test)]
            Self::Test(_) => Ok(AgentId::new(Uuid::nil())),
        }
    }

    async fn invoke_and_await(
        &self,
        method: &str,
        input: SchemaValue,
        scope_card: Option<&ScopeCard>,
    ) -> AwaitedResult {
        match self {
            Self::Host(transport) => transport.invoke_and_await(method, input, scope_card).await,
            #[cfg(test)]
            Self::Test(transport) => transport.invoke_and_await(method, input, scope_card).await,
        }
    }

    async fn trigger(
        &self,
        method: &str,
        input: SchemaValue,
    ) -> Result<host::Invocation, GolemReflectError> {
        match self {
            Self::Host(transport) => transport.trigger(method, input).await,
            #[cfg(test)]
            Self::Test(_) => panic!("test reflected transport does not support trigger"),
        }
    }

    async fn schedule(
        &self,
        at: ScheduledTime,
        method: &str,
        input: SchemaValue,
    ) -> Result<host::Invocation, GolemReflectError> {
        match self {
            Self::Host(transport) => transport.schedule(at, method, input).await,
            #[cfg(test)]
            Self::Test(_) => panic!("test reflected transport does not support scheduling"),
        }
    }
}

/// Decodes the output of an invocation of `method` and checks it against its declared schema.
fn checked_output_decoder(
    output: Option<SchemaRef>,
    method: String,
) -> impl Fn(WireOutput) -> Result<Option<SchemaValue>, GolemReflectError> + 'static {
    move |wire| {
        let value = wire
            .map(crate::decode_schema_value)
            .transpose()
            .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?;
        validate_declared_output(output.as_ref(), value.as_ref(), &method)?;
        Ok(value)
    }
}

fn validate_declared_output(
    output: Option<&SchemaRef>,
    value: Option<&SchemaValue>,
    method: &str,
) -> Result<(), GolemReflectError> {
    match (output, value) {
        (Some(schema), Some(value)) => schema
            .validate_value(value)
            .map_err(|error| GolemReflectError::MalformedRemoteOutput(error.to_string())),
        (None, None) => Ok(()),
        _ => Err(GolemReflectError::MalformedRemoteOutput(format!(
            "method `{method}` returned an unexpected unit/value shape"
        ))),
    }
}

impl ReflectedAgentMethod {
    pub fn definition(&self) -> &AgentMethod {
        &self.definition
    }

    pub async fn invoke_value(
        &self,
        input: SchemaValue,
    ) -> Result<InvocationOutput<Option<SchemaValue>>, GolemReflectError> {
        self.definition.input.validate_value(&input)?;
        self.invoke_checked_value(input, None).await
    }

    /// Like [`invoke_value`](Self::invoke_value), and lends `scope_card` to the
    /// callee for the duration of the invocation.
    pub async fn invoke_value_with_scope_card(
        &self,
        input: SchemaValue,
        scope_card: &ScopeCard,
    ) -> Result<InvocationOutput<Option<SchemaValue>>, GolemReflectError> {
        self.definition.input.validate_value(&input)?;
        self.invoke_checked_value(input, Some(scope_card)).await
    }

    async fn invoke_checked_value(
        &self,
        input: SchemaValue,
        scope_card: Option<&ScopeCard>,
    ) -> Result<InvocationOutput<Option<SchemaValue>>, GolemReflectError> {
        let invocation = self
            .transport
            .invoke_and_await(&self.definition.raw.name, input, scope_card)
            .await?;
        validate_declared_output(
            self.definition.output.as_ref(),
            invocation.value.as_ref(),
            &self.definition.raw.name,
        )?;
        Ok(invocation)
    }

    #[cfg(feature = "json")]
    pub async fn invoke_json(
        &self,
        input: &serde_json::Value,
    ) -> Result<InvocationOutput<Option<serde_json::Value>>, GolemReflectError> {
        self.invoke_json_scoped(input, None).await
    }

    /// Like [`invoke_json`](Self::invoke_json), and lends `scope_card` to the
    /// callee for the duration of the invocation.
    #[cfg(feature = "json")]
    pub async fn invoke_json_with_scope_card(
        &self,
        input: &serde_json::Value,
        scope_card: &ScopeCard,
    ) -> Result<InvocationOutput<Option<serde_json::Value>>, GolemReflectError> {
        self.invoke_json_scoped(input, Some(scope_card)).await
    }

    #[cfg(feature = "json")]
    async fn invoke_json_scoped(
        &self,
        input: &serde_json::Value,
        scope_card: Option<&ScopeCard>,
    ) -> Result<InvocationOutput<Option<serde_json::Value>>, GolemReflectError> {
        let invocation = self
            .invoke_checked_value(self.definition.input.pack_json(input)?, scope_card)
            .await?;
        let value = match (&self.definition.output, invocation.value) {
            (Some(schema), Some(value)) => Some(
                schema
                    .unpack_checked_json(&value)
                    .map_err(|error| GolemReflectError::MalformedRemoteOutput(error.to_string()))?,
            ),
            _ => None,
        };
        Ok(InvocationOutput {
            metadata: invocation.metadata,
            value,
        })
    }

    /// Triggers the method and returns the handle of the invocation once the target accepted
    /// it, without waiting for its result.
    pub async fn trigger_value(
        &self,
        input: SchemaValue,
    ) -> Result<Invocation<Option<SchemaValue>>, GolemReflectError> {
        self.reject_non_awaited_streams("trigger")?;
        self.definition.input.validate_value(&input)?;
        let raw = self
            .transport
            .trigger(&self.definition.raw.name, input)
            .await?;
        Invocation::from_raw(raw, self.decoder())
    }

    /// Schedules the method for `at` and returns the handle of the invocation once the target
    /// accepted it.
    pub async fn schedule_value(
        &self,
        at: ScheduledTime,
        input: SchemaValue,
    ) -> Result<Invocation<Option<SchemaValue>>, GolemReflectError> {
        self.reject_non_awaited_streams("schedule")?;
        self.definition.input.validate_value(&input)?;
        let raw = self
            .transport
            .schedule(at, &self.definition.raw.name, input)
            .await?;
        Invocation::from_raw(raw, self.decoder())
    }

    /// Rebuilds the handle of an invocation of this method from its ID. Makes no remote call.
    pub fn invocation(&self, id: &OwnedInvocationId) -> Invocation<Option<SchemaValue>> {
        Invocation::from_id(id, self.decoder())
    }

    fn decoder(
        &self,
    ) -> impl Fn(WireOutput) -> Result<Option<SchemaValue>, GolemReflectError> + 'static {
        checked_output_decoder(
            self.definition.output.clone(),
            self.definition.raw.name.clone(),
        )
    }

    fn reject_non_awaited_streams(&self, operation: &str) -> Result<(), GolemReflectError> {
        if self.definition.input.contains_stream()
            || self
                .definition
                .output
                .as_ref()
                .is_some_and(SchemaRef::contains_stream)
        {
            return Err(GolemReflectError::InvalidType(format!(
                "{operation} is unavailable for streaming method `{}`",
                self.definition.raw.name
            )));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct DynamicAgentClient {
    transport: Rc<RpcTransport>,
    agent_id: AgentId,
}

impl DynamicAgentClient {
    /// Connects to the existing agent with the given ID, whatever its type. Never creates the
    /// agent.
    pub fn from_agent_id(agent_id: &AgentId) -> Result<Self, GolemReflectError> {
        let transport = RpcTransport::get_by_id_of_any_type(agent_id)?;
        Ok(Self {
            transport: Rc::new(transport),
            agent_id: agent_id.clone(),
        })
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub fn method(&self, name: impl Into<String>) -> DynamicAgentMethod {
        DynamicAgentMethod {
            name: name.into(),
            transport: self.transport.clone(),
        }
    }
}

#[derive(Clone)]
pub struct DynamicAgentMethod {
    name: String,
    transport: Rc<RpcTransport>,
}

impl DynamicAgentMethod {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn invoke_value(
        &self,
        input: SchemaValue,
    ) -> Result<InvocationOutput<Option<SchemaValue>>, GolemReflectError> {
        self.transport
            .invoke_and_await(&self.name, input, None)
            .await
    }

    /// Like [`invoke_value`](Self::invoke_value), and lends `scope_card` to the
    /// callee for the duration of the invocation.
    pub async fn invoke_value_with_scope_card(
        &self,
        input: SchemaValue,
        scope_card: &ScopeCard,
    ) -> Result<InvocationOutput<Option<SchemaValue>>, GolemReflectError> {
        self.transport
            .invoke_and_await(&self.name, input, Some(scope_card))
            .await
    }

    /// Triggers the method and returns the handle of the invocation once the target accepted
    /// it. The handle's result is not checked against a declared schema.
    pub async fn trigger_value(
        &self,
        input: SchemaValue,
    ) -> Result<Invocation<Option<SchemaValue>>, GolemReflectError> {
        let raw = self.transport.trigger(&self.name, input).await?;
        Invocation::from_raw(raw, Self::decode)
    }

    /// Schedules the method for `at` and returns the handle of the invocation once the target
    /// accepted it.
    pub async fn schedule_value(
        &self,
        at: ScheduledTime,
        input: SchemaValue,
    ) -> Result<Invocation<Option<SchemaValue>>, GolemReflectError> {
        let raw = self.transport.schedule(at, &self.name, input).await?;
        Invocation::from_raw(raw, Self::decode)
    }

    /// Rebuilds the handle of an invocation from its ID. Makes no remote call.
    pub fn invocation(id: &OwnedInvocationId) -> Invocation<Option<SchemaValue>> {
        Invocation::from_id(id, Self::decode)
    }

    fn decode(wire: WireOutput) -> Result<Option<SchemaValue>, GolemReflectError> {
        wire.map(crate::decode_schema_value)
            .transpose()
            .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))
    }
}

/// The UUID assigned to an accepted invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InvocationId(pub Uuid);

impl Display for InvocationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl std::str::FromStr for InvocationId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// Identifies an invocation and its agent. Its text form is `<agent-id>.<uuid>`, where
/// `<agent-id>` is the agent ID as a UUID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OwnedInvocationId {
    agent_id: AgentId,
    id: InvocationId,
}

impl OwnedInvocationId {
    pub fn new(agent_id: AgentId, id: InvocationId) -> Self {
        Self { agent_id, id }
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub fn id(&self) -> InvocationId {
        self.id
    }

    fn from_wire(id: crate::schema::wit::wire::InvocationId) -> Result<Self, GolemReflectError> {
        let agent_id = match id.owner {
            crate::schema::wit::wire::InvocationOwner::Agent(agent_id) => AgentId::from(agent_id),
            crate::schema::wit::wire::InvocationOwner::Tool(tool) => {
                return Err(GolemReflectError::ProtocolError(format!(
                    "the host returned an agent invocation owned by tool `{tool}`"
                )));
            }
        };
        Ok(Self::new(
            agent_id,
            InvocationId(crate::wire_uuid_to_schema(id.id)),
        ))
    }
}

impl From<&OwnedInvocationId> for crate::schema::wit::wire::InvocationId {
    fn from(id: &OwnedInvocationId) -> Self {
        Self {
            owner: crate::schema::wit::wire::InvocationOwner::Agent((&id.agent_id).into()),
            id: crate::schema_uuid_to_wire(id.id.0),
        }
    }
}

impl Display for OwnedInvocationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.agent_id.uuid, self.id)
    }
}

impl std::str::FromStr for OwnedInvocationId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid =
            || format!("invalid invocation id: expected <agent-uuid>.<uuid>, got {value}");
        let (agent_id, id) = value.split_once('.').ok_or_else(invalid)?;
        let agent_id = Uuid::parse_str(agent_id).map_err(|_| invalid())?;
        let id = Uuid::parse_str(id).map_err(|_| invalid())?;
        Ok(Self::new(AgentId::new(agent_id), InvocationId(id)))
    }
}

#[derive(Clone, Debug)]
pub struct InvocationMetadata {
    pub agent_id: AgentId,
    pub invocation_id: OwnedInvocationId,
}

impl From<crate::golem_agentic::golem::agent::host::InvocationMetadata> for InvocationMetadata {
    fn from(metadata: crate::golem_agentic::golem::agent::host::InvocationMetadata) -> Self {
        Self::from_host(metadata.agent_id, metadata.invocation_id)
    }
}

impl InvocationMetadata {
    fn from_host(
        agent_id: crate::schema::wit::wire::AgentId,
        invocation_id: crate::schema::wit::wire::InvocationId,
    ) -> Self {
        let agent_id = AgentId::from(agent_id);
        Self {
            invocation_id: OwnedInvocationId::new(
                agent_id.clone(),
                InvocationId(crate::wire_uuid_to_schema(invocation_id.id)),
            ),
            agent_id,
        }
    }
}

/// The result of an awaited invocation, with the final identity of the invocation.
#[derive(Clone, Debug)]
pub struct InvocationOutput<T> {
    pub metadata: InvocationMetadata,
    pub value: T,
}

/// The lifecycle state of an accepted invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvocationStatus {
    /// Accepted and waiting for its turn
    Pending,
    /// Accepted and waiting for its scheduled time
    Scheduled,
    /// Started and executing
    Running,
    /// Started and waiting, for example for a promise or a timer
    Waiting,
    /// Started and waiting to retry after a failure
    Retrying,
    /// Started, and its agent was interrupted by an operator
    Interrupted,
    /// Finished with a result
    Completed,
    /// Finished with a failure
    Failed,
    /// Withdrawn before it started
    Cancelled,
    /// Recorded its result, then its agent failed during the invocation's background work
    BackgroundFailed,
}

impl From<host::InvocationStatus> for InvocationStatus {
    fn from(status: host::InvocationStatus) -> Self {
        match status {
            host::InvocationStatus::Pending => Self::Pending,
            host::InvocationStatus::Scheduled => Self::Scheduled,
            host::InvocationStatus::Running => Self::Running,
            host::InvocationStatus::Waiting => Self::Waiting,
            host::InvocationStatus::Retrying => Self::Retrying,
            host::InvocationStatus::Interrupted => Self::Interrupted,
            host::InvocationStatus::Completed => Self::Completed,
            host::InvocationStatus::Failed => Self::Failed,
            host::InvocationStatus::Cancelled => Self::Cancelled,
            host::InvocationStatus::BackgroundFailed => Self::BackgroundFailed,
        }
    }
}

type WireOutput = Option<crate::schema::wit::wire::SchemaValueTree>;
type DecodeOutput<T> = Rc<dyn Fn(WireOutput) -> Result<T, GolemReflectError>>;

/// The handle of an accepted invocation of an agent method, returned by `trigger` and
/// `schedule`.
///
/// The ID only locates the invocation; it is not authority. Only the caller that started the
/// invocation, or a caller with the applicable management permission, can observe or cancel
/// it. Store [`id`](Self::id) to await the invocation from a later invocation of the same
/// agent, and rebuild the handle with [`get`](Self::get). Dropping the handle, or an
/// unfinished [`await_result`](Self::await_result), only detaches: the invocation keeps
/// running.
///
/// `trigger` and `schedule` reject a method whose input or output contains a stream, and they
/// never lend a scope card to the callee. Use an awaited call for a streaming method or to lend
/// a scope card.
pub struct Invocation<T> {
    id: OwnedInvocationId,
    raw: host::Invocation,
    decode: DecodeOutput<T>,
}

impl<T> std::fmt::Debug for Invocation<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invocation")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl<T: crate::schema::wit::direct::FromWire + 'static> Invocation<T> {
    /// Rebuilds the handle of the invocation with the given ID. Makes no remote call.
    pub fn get(id: &OwnedInvocationId) -> Self {
        Self::from_id(id, |output| {
            crate::schema::wit::direct::decode_result_payload::<T>(output)
                .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))
        })
    }
}

impl<T> Invocation<T> {
    fn from_raw(
        raw: host::Invocation,
        decode: impl Fn(WireOutput) -> Result<T, GolemReflectError> + 'static,
    ) -> Result<Self, GolemReflectError> {
        Ok(Self {
            id: OwnedInvocationId::from_wire(raw.id())?,
            raw,
            decode: Rc::new(decode),
        })
    }

    fn from_id(
        id: &OwnedInvocationId,
        decode: impl Fn(WireOutput) -> Result<T, GolemReflectError> + 'static,
    ) -> Self {
        Self {
            id: id.clone(),
            raw: host::Invocation::get(&id.into()),
            decode: Rc::new(decode),
        }
    }

    /// The handle of an invocation that a generated client triggered or scheduled.
    #[doc(hidden)]
    #[cfg(feature = "export_golem_agentic")]
    pub fn __from_agentic(
        raw: crate::golem_agentic::golem::agent::host::Invocation,
        decode: impl Fn(WireOutput) -> Result<T, GolemReflectError> + 'static,
    ) -> Result<Self, GolemReflectError> {
        // Both bindings import the same `invocation` resource, so its handle moves between them.
        let raw = unsafe { host::Invocation::from_handle(raw.take_handle()) };
        Self::from_raw(raw, decode)
    }

    /// The handle of the same invocation, with `f` applied to its decoded result.
    #[doc(hidden)]
    pub fn __map_output<U>(
        self,
        f: impl Fn(T) -> Result<U, GolemReflectError> + 'static,
    ) -> Invocation<U>
    where
        T: 'static,
    {
        let decode = self.decode;
        Invocation {
            id: self.id,
            raw: self.raw,
            decode: Rc::new(move |output| f(decode(output)?)),
        }
    }

    /// The ID of the invocation.
    pub fn id(&self) -> &OwnedInvocationId {
        &self.id
    }

    /// The current lifecycle state of the invocation. Fails with
    /// [`GolemReflectError::RemoteNotFound`] when its agent or the invocation no longer exists.
    pub async fn status(&self) -> Result<InvocationStatus, GolemReflectError> {
        self.raw
            .status()
            .await
            .map(InvocationStatus::from)
            .map_err(rpc_error_to_reflect)
    }

    /// Awaits the result of the invocation, without starting, resuming or resubmitting it.
    /// Fails with [`GolemReflectError::Cancelled`] when the invocation was cancelled before it
    /// started.
    pub async fn await_result(&self) -> Result<T, GolemReflectError> {
        let output = self.raw.await_().await.map_err(rpc_error_to_reflect)?;
        (self.decode)(output)
    }

    /// Withdraws the invocation if it has not started yet, and returns `true`. Returns `false`
    /// when it already finished or was already cancelled, and fails with
    /// [`GolemReflectError::CannotCancelRunning`] when it is running.
    pub async fn cancel(&self) -> Result<bool, GolemReflectError> {
        self.raw.cancel().await.map_err(rpc_error_to_reflect)
    }
}

struct RpcTransport {
    raw: host::WasmRpc,
}

impl RpcTransport {
    fn upsert(type_name: &str, constructor: &SchemaValue) -> Result<Self, GolemReflectError> {
        let raw = super::identity::upsert(type_name, constructor)?;
        Ok(Self { raw })
    }

    async fn create(
        type_name: &str,
        constructor: &SchemaValue,
        options: CreateOptions<Vec<wire_common::TypedAgentConfigValue>>,
    ) -> Result<Self, GolemReflectError> {
        let raw = super::identity::create(type_name, constructor, options).await?;
        Ok(Self { raw })
    }

    fn get_by_id(type_name: &str, agent_id: &AgentId) -> Result<Self, GolemReflectError> {
        let raw = super::identity::get_by_id(type_name, agent_id)?;
        Ok(Self { raw })
    }

    /// An ID whose type tag names no deployed agent type is
    /// [`GolemReflectError::RemoteNotFound`], like an unknown ID.
    fn get_by_id_of_any_type(agent_id: &AgentId) -> Result<Self, GolemReflectError> {
        let registered = host::get_agent_type_by_agent_id(agent_id.into()).ok_or_else(|| {
            GolemReflectError::RemoteNotFound(format!("agent `{}` was not found", agent_id.uuid))
        })?;
        Self::get_by_id(&registered.agent_type.type_name, agent_id)
    }

    async fn agent_id(&self) -> Result<AgentId, GolemReflectError> {
        super::identity::agent_id(&self.raw).await
    }

    async fn invoke_and_await(
        &self,
        method: &str,
        input: SchemaValue,
        scope_card: Option<&ScopeCard>,
    ) -> AwaitedResult {
        let input = crate::encode_schema_value_async(&input)
            .await
            .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
        let completed = self
            .raw
            .invoke_and_await(method.to_string(), input, scope_card.map(ScopeCard::raw))
            .await
            .map_err(rpc_error_to_reflect)?;
        let value = completed
            .result
            .map(crate::decode_schema_value)
            .transpose()
            .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?;
        Ok(InvocationOutput {
            metadata: self.metadata(completed.metadata),
            value,
        })
    }

    async fn trigger(
        &self,
        method: &str,
        input: SchemaValue,
    ) -> Result<host::Invocation, GolemReflectError> {
        let input = crate::encode_schema_value_async(&input)
            .await
            .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
        self.raw
            .trigger(method.to_string(), input)
            .await
            .map_err(rpc_error_to_reflect)
    }

    async fn schedule(
        &self,
        at: ScheduledTime,
        method: &str,
        input: SchemaValue,
    ) -> Result<host::Invocation, GolemReflectError> {
        let input = crate::encode_schema_value_async(&input)
            .await
            .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
        self.raw
            .schedule(at, method.to_string(), input)
            .await
            .map_err(rpc_error_to_reflect)
    }

    fn metadata(&self, raw: host::InvocationMetadata) -> InvocationMetadata {
        InvocationMetadata::from_host(raw.agent_id, raw.invocation_id)
    }
}

fn input_schema_ref(
    decoder: &crate::schema::wit::GraphDecoder<'_>,
    input: &wire_common::InputSchema,
    graph: &Arc<SchemaGraph>,
) -> Result<SchemaRef, GolemReflectError> {
    let wire_common::InputSchema::Parameters(fields) = input;
    let fields = fields
        .iter()
        .filter(|field| matches!(field.source, wire_common::FieldSource::UserSupplied))
        .map(|field| {
            Ok(NamedFieldType {
                name: field.name.clone(),
                body: decode_root(decoder, field.schema)?,
                metadata: crate::schema::wit::decode_metadata(&field.metadata),
            })
        })
        .collect::<Result<Vec<_>, GolemReflectError>>()?;
    Ok(SchemaRef::with_root(
        graph.clone(),
        SchemaType::Record {
            fields,
            metadata: MetadataEnvelope::default(),
        },
    ))
}

fn output_schema_ref(
    decoder: &crate::schema::wit::GraphDecoder<'_>,
    output: &wire_common::OutputSchema,
    graph: &Arc<SchemaGraph>,
) -> Result<Option<SchemaRef>, GolemReflectError> {
    match output {
        wire_common::OutputSchema::Unit => Ok(None),
        wire_common::OutputSchema::Single(root) => Ok(Some(SchemaRef::with_root(
            graph.clone(),
            decode_root(decoder, *root)?,
        ))),
    }
}

fn decode_root(
    decoder: &crate::schema::wit::GraphDecoder<'_>,
    root: i32,
) -> Result<SchemaType, GolemReflectError> {
    decoder
        .decode_type_at(root)
        .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))
}

#[doc(hidden)]
pub fn rpc_error_to_reflect(error: host::RpcError) -> GolemReflectError {
    match error {
        host::RpcError::ProtocolError(message) => GolemReflectError::ProtocolError(message),
        host::RpcError::Denied(message) => GolemReflectError::Denied(message),
        host::RpcError::NotFound(message) => GolemReflectError::RemoteNotFound(message),
        host::RpcError::RemoteInternalError(message) => {
            GolemReflectError::RemoteInternalError(message)
        }
        host::RpcError::RemoteAgentError(error) => {
            GolemReflectError::RemoteAgent(remote_agent_error(error))
        }
        host::RpcError::Conflict(message) => GolemReflectError::Conflict(message),
        host::RpcError::Cancelled(message) => GolemReflectError::Cancelled(message),
        host::RpcError::CannotCancelRunning(message) => {
            GolemReflectError::CannotCancelRunning(message)
        }
    }
}

#[doc(hidden)]
pub fn agentic_rpc_error_to_reflect(
    error: crate::golem_agentic::golem::agent::host::RpcError,
) -> GolemReflectError {
    use crate::golem_agentic::golem::agent::host::RpcError;
    match error {
        RpcError::ProtocolError(message) => GolemReflectError::ProtocolError(message),
        RpcError::Denied(message) => GolemReflectError::Denied(message),
        RpcError::NotFound(message) => GolemReflectError::RemoteNotFound(message),
        RpcError::RemoteInternalError(message) => GolemReflectError::RemoteInternalError(message),
        RpcError::RemoteAgentError(error) => {
            GolemReflectError::RemoteAgent(agentic_remote_agent_error(error))
        }
        RpcError::Conflict(message) => GolemReflectError::Conflict(message),
        RpcError::Cancelled(message) => GolemReflectError::Cancelled(message),
        RpcError::CannotCancelRunning(message) => GolemReflectError::CannotCancelRunning(message),
    }
}

#[doc(hidden)]
pub fn agent_error_to_reflect(error: wire_common::AgentError) -> GolemReflectError {
    match error {
        wire_common::AgentError::InvalidInput(message) => GolemReflectError::InvalidInput(message),
        wire_common::AgentError::InvalidMethod(message) => {
            GolemReflectError::RemoteAgent(RemoteAgentError::InvalidMethod(message))
        }
        wire_common::AgentError::InvalidType(message) => GolemReflectError::InvalidType(message),
        wire_common::AgentError::InvalidAgentId(message) => {
            GolemReflectError::InvalidAgentId(message)
        }
        wire_common::AgentError::CustomError(value) => {
            GolemReflectError::RemoteAgent(decode_custom_error(value))
        }
    }
}

#[doc(hidden)]
pub fn agentic_agent_error_to_reflect(
    error: crate::golem_agentic::golem::agent::common::AgentError,
) -> GolemReflectError {
    use crate::golem_agentic::golem::agent::common::AgentError;
    match error {
        AgentError::InvalidInput(message) => GolemReflectError::InvalidInput(message),
        AgentError::InvalidMethod(message) => {
            GolemReflectError::RemoteAgent(RemoteAgentError::InvalidMethod(message))
        }
        AgentError::InvalidType(message) => GolemReflectError::InvalidType(message),
        AgentError::InvalidAgentId(message) => GolemReflectError::InvalidAgentId(message),
        AgentError::CustomError(value) => {
            GolemReflectError::RemoteAgent(decode_custom_error(value))
        }
    }
}

fn remote_agent_error(error: wire_common::AgentError) -> RemoteAgentError {
    match error {
        wire_common::AgentError::InvalidInput(message) => RemoteAgentError::InvalidInput(message),
        wire_common::AgentError::InvalidMethod(message) => RemoteAgentError::InvalidMethod(message),
        wire_common::AgentError::InvalidType(message) => RemoteAgentError::InvalidType(message),
        wire_common::AgentError::InvalidAgentId(message) => {
            RemoteAgentError::InvalidAgentId(message)
        }
        wire_common::AgentError::CustomError(value) => decode_custom_error(value),
    }
}

fn agentic_remote_agent_error(
    error: crate::golem_agentic::golem::agent::common::AgentError,
) -> RemoteAgentError {
    use crate::golem_agentic::golem::agent::common::AgentError;
    match error {
        AgentError::InvalidInput(message) => RemoteAgentError::InvalidInput(message),
        AgentError::InvalidMethod(message) => RemoteAgentError::InvalidMethod(message),
        AgentError::InvalidType(message) => RemoteAgentError::InvalidType(message),
        AgentError::InvalidAgentId(message) => RemoteAgentError::InvalidAgentId(message),
        AgentError::CustomError(value) => decode_custom_error(value),
    }
}

fn decode_custom_error(value: crate::schema::wit::wire::TypedSchemaValue) -> RemoteAgentError {
    match crate::decode_typed_schema_value(&value) {
        Ok(value) => RemoteAgentError::Custom(Box::new(value)),
        Err(error) => RemoteAgentError::MalformedCustomError(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentMethod, GolemReflectError, MethodOnlyAgentClientDefinition, ReflectedAgentMethod,
        ReflectedTransport, SchemaRef, TestReflectedTransport, checked_output_decoder,
        validate_declared_output,
    };
    use crate::bindings::golem::agent::common as wire_common;
    use crate::schema::render::from_json_value;
    use crate::schema::schema_type::{NumericBound, NumericRestrictions};
    use crate::schema::validation::{is_equivalent_cross_graph, validate_graph, validate_value};
    use crate::schema::{
        BinaryRestrictions, MetadataEnvelope, NamedFieldType, PermissionCardSpec, QuantitySpec,
        QuotaTokenSpec, ResultSpec, SchemaGraph, SchemaType, SchemaTypeDef, SchemaValue,
        TextRestrictions, TypeId, VariantCaseType, VariantValuePayload,
    };
    use serde_json::{Value, json};
    use std::collections::HashSet;
    use std::rc::Rc;
    use test_r::test;

    #[test]
    fn invocation_id_text_form_is_agent_uuid_dot_invocation_uuid() {
        let agent_id = crate::AgentId::new(
            crate::Uuid::parse_str("4a2b9c1d-0e3f-8a4b-9c5d-6e7f8a9b0c1d").unwrap(),
        );
        let uuid = crate::Uuid::parse_str("0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c").unwrap();
        let id = super::OwnedInvocationId::new(agent_id.clone(), super::InvocationId(uuid));
        let text = id.to_string();
        assert_eq!(
            text,
            "4a2b9c1d-0e3f-8a4b-9c5d-6e7f8a9b0c1d.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c"
        );
        let parsed: super::OwnedInvocationId = text.parse().unwrap();
        assert_eq!(parsed.agent_id(), &agent_id);
        assert_eq!(parsed.id(), super::InvocationId(uuid));
        assert_eq!(
            parsed
                .id()
                .to_string()
                .parse::<super::InvocationId>()
                .unwrap(),
            parsed.id()
        );
        assert!(
            ".0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c"
                .parse::<super::OwnedInvocationId>()
                .is_err()
        );
        assert!(
            r#"shop.v2/cart("a.b").0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c"#
                .parse::<super::OwnedInvocationId>()
                .is_err()
        );
        assert!(
            "agent-0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c"
                .parse::<super::OwnedInvocationId>()
                .is_err()
        );
    }

    #[test]
    fn schema_ref_packs_validates_and_unpacks_canonical_json() {
        let schema = SchemaRef::new(SchemaGraph::anonymous(SchemaType::record(vec![
            NamedFieldType {
                name: "name".to_string(),
                body: SchemaType::string(),
                metadata: MetadataEnvelope::default(),
            },
            NamedFieldType {
                name: "enabled".to_string(),
                body: SchemaType::option(SchemaType::bool()),
                metadata: MetadataEnvelope::default(),
            },
        ])));

        let packed = schema
            .pack_json(&json!({ "name": "demo", "enabled": true }))
            .expect("pack canonical JSON");
        assert_eq!(
            packed,
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("demo".to_string()),
                    SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::Bool(true)))
                    }
                ]
            }
        );
        assert_eq!(
            schema.unpack_json(&packed).expect("unpack canonical JSON"),
            json!({ "name": "demo", "enabled": true })
        );
        assert_eq!(schema.to_json_schema(true)["type"], json!("object"));
        assert_eq!(
            schema
                .pack_json(&json!({ "name": "demo" }))
                .expect("decode omitted option"),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("demo".to_string()),
                    SchemaValue::Option { inner: None },
                ]
            }
        );
        assert_eq!(schema.to_json_schema(false)["required"], json!(["name"]));
    }

    #[test]
    fn reflection_json_schema_rejects_unrepresentable_leaves() {
        for ty in [
            SchemaType::secret(Default::default()),
            SchemaType::future(None),
            SchemaType::stream(None),
        ] {
            let schema = SchemaRef::new(SchemaGraph::anonymous(ty));
            assert_eq!(schema.to_json_schema(false)["not"], json!({}));
        }
    }

    fn conformance_field(name: &str, body: SchemaType) -> NamedFieldType {
        NamedFieldType {
            name: name.to_string(),
            body,
            metadata: MetadataEnvelope::default(),
        }
    }

    fn conformance_schema(name: &str, id: &str) -> SchemaRef {
        let root = match name {
            "s64" => SchemaType::s64(),
            "constrained-s64" => SchemaType::S64 {
                restrictions: Some(NumericRestrictions {
                    min: Some(NumericBound::Signed(-9_007_199_254_740_993)),
                    max: Some(NumericBound::Signed(9_007_199_254_740_993)),
                    unit: None,
                }),
                metadata: MetadataEnvelope::default(),
            },
            "u64" => SchemaType::u64(),
            "binary" => SchemaType::binary(BinaryRestrictions::default()),
            "duration" => SchemaType::duration(),
            "quantity" => SchemaType::quantity(QuantitySpec {
                base_unit: "m".to_string(),
                allowed_suffixes: vec![],
                min: None,
                max: None,
            }),
            "tool-input" => SchemaType::record(vec![
                conformance_field("pattern", SchemaType::string()),
                conformance_field("paths", SchemaType::list(SchemaType::string())),
                conformance_field("ignoreCase", SchemaType::option(SchemaType::bool())),
            ]),
            "config-entry" => SchemaType::record(vec![
                conformance_field("path", SchemaType::list(SchemaType::string())),
                conformance_field("value", SchemaType::s64()),
            ]),
            "constrained-u32" => SchemaType::U32 {
                restrictions: Some(NumericRestrictions {
                    min: Some(NumericBound::Unsigned(2)),
                    max: Some(NumericBound::Unsigned(10)),
                    unit: None,
                }),
                metadata: MetadataEnvelope::default(),
            },
            "constrained-f64" => SchemaType::F64 {
                restrictions: Some(NumericRestrictions {
                    min: Some(NumericBound::float(-1.5).unwrap()),
                    max: Some(NumericBound::float(2.5).unwrap()),
                    unit: None,
                }),
                metadata: MetadataEnvelope::default(),
            },
            "constrained-text" => SchemaType::text(TextRestrictions {
                languages: Some(vec!["en".to_string(), "de".to_string()]),
                min_length: Some(2),
                max_length: Some(8),
                regex: Some("^[a-z]+$".to_string()),
            }),
            "constrained-binary" => SchemaType::binary(BinaryRestrictions {
                mime_types: Some(vec![
                    "image/png".to_string(),
                    "application/octet-stream".to_string(),
                ]),
                min_bytes: Some(2),
                max_bytes: Some(4),
            }),
            "result" => SchemaType::result(ResultSpec {
                ok: Some(Box::new(SchemaType::string())),
                err: Some(Box::new(SchemaType::u32())),
            }),
            "custom-error" => SchemaType::result(ResultSpec {
                ok: Some(Box::new(SchemaType::string())),
                err: Some(Box::new(SchemaType::record(vec![
                    conformance_field("code", SchemaType::string()),
                    conformance_field("retryable", SchemaType::bool()),
                ]))),
            }),
            "optional-record" => {
                let id = TypeId::new("conformance.optional");
                return SchemaRef::new(SchemaGraph {
                    defs: vec![SchemaTypeDef {
                        id: id.clone(),
                        name: None,
                        body: SchemaType::option(SchemaType::string()),
                    }],
                    root: SchemaType::record(vec![
                        conformance_field("direct", SchemaType::option(SchemaType::string())),
                        conformance_field("referenced", SchemaType::ref_to(id)),
                    ]),
                });
            }
            other => panic!("{id}: unknown conformance fixture {other}"),
        };
        SchemaRef::new(SchemaGraph::anonymous(root))
    }

    fn conformance_json_pointer<'a>(value: &'a Value, pointer: &str, id: &str) -> &'a Value {
        if pointer.is_empty() {
            value
        } else {
            value
                .pointer(pointer)
                .unwrap_or_else(|| panic!("{id}: missing JSON pointer {pointer} in {value}"))
        }
    }

    fn assert_conformance_subset(actual: &Value, expected: &Value, id: &str) {
        match expected {
            Value::Object(expected) => {
                let actual = actual
                    .as_object()
                    .unwrap_or_else(|| panic!("{id}: expected JSON object, got {actual}"));
                for (key, expected) in expected {
                    assert_conformance_subset(
                        actual
                            .get(key)
                            .unwrap_or_else(|| panic!("{id}: missing key {key} in {actual:?}")),
                        expected,
                        id,
                    );
                }
            }
            _ => assert_eq!(actual, expected, "{id}"),
        }
    }

    fn assert_conformance_semantic(fixture: &str, expected: &Value, id: &str) {
        match fixture {
            "unsupported-leaves" => {
                let types = [
                    SchemaType::secret(Default::default()),
                    SchemaType::quota_token(QuotaTokenSpec::default()),
                    SchemaType::permission_card(PermissionCardSpec::default()),
                    SchemaType::future(None),
                    SchemaType::stream(None),
                ];
                assert_eq!(
                    types.len(),
                    expected["count"]
                        .as_u64()
                        .unwrap_or_else(|| panic!("{id}: missing unsupported-leaves count"))
                        as usize,
                    "{id}"
                );
                for ty in types {
                    assert_conformance_subset(
                        &SchemaRef::new(SchemaGraph::anonymous(ty)).to_json_schema(false),
                        &expected["schema"],
                        id,
                    );
                }
            }
            "all-kinds" => assert_eq!(
                serde_json::to_value(vec![
                    "ref",
                    "bool",
                    "s8",
                    "s16",
                    "s32",
                    "s64",
                    "u8",
                    "u16",
                    "u32",
                    "u64",
                    "f32",
                    "f64",
                    "char",
                    "string",
                    "record",
                    "variant",
                    "enum",
                    "flags",
                    "tuple",
                    "list",
                    "fixed-list",
                    "map",
                    "option",
                    "result",
                    "text",
                    "binary",
                    "path",
                    "url",
                    "datetime",
                    "duration",
                    "quantity",
                    "union",
                    "secret",
                    "quota-token",
                    "permission-card",
                    "future",
                    "stream",
                ])
                .unwrap(),
                expected["names"],
                "{id}"
            ),
            "all-restrictions" => assert_eq!(
                serde_json::to_value(vec![
                    "numeric-minimum",
                    "numeric-maximum",
                    "numeric-unit",
                    "text-languages",
                    "text-min-length",
                    "text-max-length",
                    "text-regex",
                    "binary-mime-types",
                    "binary-min-bytes",
                    "binary-max-bytes",
                    "path-direction",
                    "path-kind",
                    "path-mime-types",
                    "path-extensions",
                    "url-schemes",
                    "url-hosts",
                    "quantity-base-unit",
                    "quantity-suffixes",
                    "quantity-minimum",
                    "quantity-maximum",
                    "union-prefix",
                    "union-suffix",
                    "union-regex",
                    "union-field",
                ])
                .unwrap(),
                expected["names"],
                "{id}"
            ),
            "graph" => {
                let referenced = conformance_schema("optional-record", id);
                let inline = SchemaRef::new(SchemaGraph::anonymous(SchemaType::record(vec![
                    conformance_field("direct", SchemaType::option(SchemaType::string())),
                    conformance_field("referenced", SchemaType::option(SchemaType::string())),
                ])));
                assert!(validate_graph(referenced.graph()).is_ok(), "{id}");
                assert!(
                    is_equivalent_cross_graph(
                        referenced.graph(),
                        referenced.root(),
                        inline.graph(),
                        inline.root(),
                    ),
                    "{id}"
                );
            }
            other => panic!("{id}: unknown semantic conformance fixture {other}"),
        }
    }

    fn conformance_corpus() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../test-data/reflection-conformance/v1.json"
        ))
        .expect("valid reflection conformance corpus")
    }

    fn run_conformance_cases(operation: &str) {
        let corpus = conformance_corpus();
        let cases = corpus["cases"].as_array().expect("cases array");
        for case in cases {
            let id = case["id"].as_str().expect("case id");
            let case_operation = case["operation"]
                .as_str()
                .unwrap_or_else(|| panic!("{id}: missing operation"));
            if case_operation != operation {
                continue;
            }
            let fixture = case["fixture"]
                .as_str()
                .unwrap_or_else(|| panic!("{id}: missing fixture"));
            let expected = &case["expected"];
            match operation {
                "roundtrip" => {
                    let schema = conformance_schema(fixture, id);
                    let packed = schema
                        .pack_json(&case["input"])
                        .unwrap_or_else(|error| panic!("{id}: {error}"));
                    assert_eq!(
                        schema
                            .unpack_json(&packed)
                            .unwrap_or_else(|error| panic!("{id}: {error}")),
                        *expected,
                        "{id}",
                    );
                }
                "reject" => {
                    let schema = conformance_schema(fixture, id);
                    let inputs = case["inputs"]
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| vec![case["input"].clone()]);
                    for input in inputs {
                        assert!(
                            schema.pack_json(&input).is_err(),
                            "{id}: direct packing accepted {input}"
                        );
                        let actual = match from_json_value(schema.graph(), schema.root(), &input) {
                            Err(_) => "invalid-json",
                            Ok(value)
                                if validate_value(schema.graph(), schema.root(), &value)
                                    .is_err() =>
                            {
                                "constraint-violation"
                            }
                            Ok(value) => panic!("{id} accepted {input} as {value:?}"),
                        };
                        assert_eq!(
                            actual,
                            expected["kind"]
                                .as_str()
                                .unwrap_or_else(|| panic!("{id}: missing reject kind")),
                            "{id}"
                        );
                    }
                }
                "json-schema" => {
                    let schema = conformance_schema(fixture, id);
                    let rendered = schema.to_json_schema(false);
                    let path = case["path"]
                        .as_str()
                        .unwrap_or_else(|| panic!("{id}: missing path"));
                    assert_conformance_subset(
                        conformance_json_pointer(&rendered, path, id),
                        expected,
                        id,
                    );
                }
                "semantic" => assert_conformance_semantic(fixture, expected, id),
                operation => panic!("unknown requested conformance operation {operation}"),
            }
        }
    }

    #[test]
    fn reflection_conformance_corpus_integrity() {
        let corpus = conformance_corpus();
        assert_eq!(corpus["version"], "1.0.0");
        let cases = corpus["cases"].as_array().expect("cases array");
        let recognized = HashSet::from(["roundtrip", "reject", "json-schema", "semantic"]);
        let mut case_ids = HashSet::new();
        for case in cases {
            let id = case["id"].as_str().expect("case id");
            assert!(case_ids.insert(id), "duplicate case ID {id}");
            let operation = case["operation"]
                .as_str()
                .unwrap_or_else(|| panic!("{id}: missing operation"));
            assert!(
                recognized.contains(operation),
                "{id}: unknown operation {operation}"
            );
        }
        let declared_ids = corpus["caseIds"].as_array().expect("declared case IDs");
        let declared: HashSet<_> = declared_ids
            .iter()
            .map(|id| id.as_str().expect("declared case ID"))
            .collect();
        assert_eq!(
            declared.len(),
            declared_ids.len(),
            "duplicate declared case ID"
        );
        assert_eq!(case_ids, declared, "missing or unknown conformance cases");
    }

    #[test]
    fn reflection_conformance_roundtrip_cases() {
        run_conformance_cases("roundtrip");
    }

    #[test]
    fn reflection_conformance_reject_cases() {
        run_conformance_cases("reject");
    }

    #[test]
    fn reflection_conformance_json_schema_cases() {
        run_conformance_cases("json-schema");
    }

    #[test]
    fn reflection_conformance_semantic_cases() {
        run_conformance_cases("semantic");
    }

    #[test]
    fn schema_ref_reports_json_and_packed_value_failures() {
        let schema = SchemaRef::new(SchemaGraph::anonymous(SchemaType::variant(vec![
            VariantCaseType {
                name: "ready".to_string(),
                payload: None,
                metadata: MetadataEnvelope::default(),
            },
        ])));

        assert!(matches!(
            schema.pack_json(&json!("missing")),
            Err(GolemReflectError::SchemaRender(_))
        ));
        assert!(matches!(
            schema.validate_value(&SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: None,
            })),
            Err(GolemReflectError::InvalidSchemaValue { .. })
        ));
    }

    #[test]
    fn malformed_declared_outputs_are_distinct_remote_errors() {
        let output = SchemaRef::new(SchemaGraph::anonymous(SchemaType::string()));
        assert!(
            validate_declared_output(
                Some(&output),
                Some(&SchemaValue::String("ok".to_string())),
                "read",
            )
            .is_ok()
        );
        for value in [Some(SchemaValue::U32(7)), None] {
            assert!(matches!(
                validate_declared_output(Some(&output), value.as_ref(), "read"),
                Err(GolemReflectError::MalformedRemoteOutput(_))
            ));
        }
        assert!(matches!(
            validate_declared_output(None, Some(&SchemaValue::Bool(true)), "unit"),
            Err(GolemReflectError::MalformedRemoteOutput(_))
        ));
    }

    #[test]
    async fn reflected_public_completion_paths_apply_the_same_output_policy() {
        let output = SchemaRef::new(SchemaGraph::anonymous(SchemaType::string()));
        let input = SchemaRef::new(SchemaGraph::anonymous(SchemaType::record(vec![])));
        for (name, declared, value) in [
            ("missing_declared", Some(output.clone()), None),
            ("unexpected_unit", None, Some(SchemaValue::Bool(true))),
            (
                "incompatible",
                Some(output.clone()),
                Some(SchemaValue::U32(7)),
            ),
        ] {
            let method = ReflectedAgentMethod {
                definition: AgentMethod {
                    agent_type_name: "InvalidOutput".to_string(),
                    raw: wire_common::AgentMethod {
                        name: name.to_string(),
                        description: String::new(),
                        http_endpoint: vec![],
                        prompt_hint: None,
                        input_schema: wire_common::InputSchema::Parameters(vec![]),
                        output_schema: if declared.is_some() {
                            wire_common::OutputSchema::Single(0)
                        } else {
                            wire_common::OutputSchema::Unit
                        },
                        read_only: None,
                    },
                    input: input.clone(),
                    output: declared,
                },
                transport: ReflectedTransport::Test(Rc::new(TestReflectedTransport {
                    value: value.clone(),
                })),
            };
            let empty = SchemaValue::Record { fields: vec![] };

            let awaited = method
                .invoke_value(empty.clone())
                .await
                .expect_err("awaited completion must reject malformed output");
            let handle =
                checked_output_decoder(method.definition.output.clone(), name.to_string())(
                    value
                        .as_ref()
                        .map(|value| crate::encode_schema_value(value).expect("value encodes")),
                )
                .expect_err("a handle's result must reject malformed output");

            assert!(matches!(
                awaited,
                GolemReflectError::MalformedRemoteOutput(_)
            ));
            assert!(matches!(
                handle,
                GolemReflectError::MalformedRemoteOutput(_)
            ));
            assert_eq!(awaited.to_string(), handle.to_string());
            let json_error = method.invoke_json(&json!({})).await.unwrap_err();
            assert!(matches!(
                json_error,
                GolemReflectError::MalformedRemoteOutput(_)
            ));
            assert_eq!(awaited.to_string(), json_error.to_string());
        }
    }

    #[test]
    async fn reflected_json_invocation_keeps_input_and_output_restrictions() {
        let constrained = conformance_schema("constrained-u32", "");
        let mut method = ReflectedAgentMethod {
            definition: AgentMethod {
                agent_type_name: "Restricted".to_string(),
                raw: wire_common::AgentMethod {
                    name: "read".to_string(),
                    description: String::new(),
                    http_endpoint: vec![],
                    prompt_hint: None,
                    input_schema: wire_common::InputSchema::Parameters(vec![]),
                    output_schema: wire_common::OutputSchema::Single(0),
                    read_only: None,
                },
                input: constrained.clone(),
                output: Some(constrained.clone()),
            },
            transport: ReflectedTransport::Test(Rc::new(TestReflectedTransport {
                value: Some(SchemaValue::U32(7)),
            })),
        };
        for invalid in [json!(1), json!(11)] {
            assert!(matches!(
                method.invoke_json(&invalid).await,
                Err(GolemReflectError::InvalidSchemaValue { .. })
            ));
        }
        assert!(matches!(
            method.invoke_value(SchemaValue::U32(1)).await,
            Err(GolemReflectError::InvalidSchemaValue { .. })
        ));
        assert!(matches!(
            method.invoke_json(&json!("wrong shape")).await,
            Err(GolemReflectError::SchemaRender(_))
        ));
        assert!(matches!(
            constrained.unpack_json(&SchemaValue::U32(11)),
            Err(GolemReflectError::InvalidSchemaValue { .. })
        ));
        assert_eq!(
            method.invoke_json(&json!(2)).await.unwrap().value,
            Some(json!(7))
        );
        method.transport = ReflectedTransport::Test(Rc::new(TestReflectedTransport {
            value: Some(SchemaValue::U32(11)),
        }));
        assert!(matches!(
            method.invoke_json(&json!(10)).await,
            Err(GolemReflectError::MalformedRemoteOutput(_))
        ));

        method.definition.output = None;
        method.transport =
            ReflectedTransport::Test(Rc::new(TestReflectedTransport { value: None }));
        assert_eq!(method.invoke_json(&json!(2)).await.unwrap().value, None);

        let optional_secret = SchemaRef::new(SchemaGraph::anonymous(SchemaType::option(
            SchemaType::secret(Default::default()),
        )));
        method.definition.input = optional_secret.clone();
        method.definition.output = Some(optional_secret);
        method.transport = ReflectedTransport::Test(Rc::new(TestReflectedTransport {
            value: Some(SchemaValue::Option { inner: None }),
        }));
        assert_eq!(
            method.invoke_json(&json!(null)).await.unwrap().value,
            Some(json!(null))
        );
        assert!(matches!(
            method
                .invoke_json(&json!({ "secretId": "untrusted" }))
                .await,
            Err(GolemReflectError::SchemaRender(_))
        ));

        use crate::schema::wit::{GuestSecretHandle, wire};
        let secret = GuestSecretHandle::new(unsafe { wire::Secret::from_handle(71) });
        method.definition.input = constrained;
        method.transport = ReflectedTransport::Test(Rc::new(TestReflectedTransport {
            value: Some(SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::Secret(secret.clone()))),
            }),
        }));
        assert!(matches!(
            method.invoke_json(&json!(2)).await,
            Err(GolemReflectError::MalformedRemoteOutput(_))
        ));
        assert!(secret.is_present());
        assert_eq!(secret.take().unwrap().take_handle(), 71);
    }

    #[test]
    fn schema_ref_detects_nested_streams() {
        let scalar = SchemaRef::new(SchemaGraph::anonymous(SchemaType::string()));
        let streaming = SchemaRef::new(SchemaGraph::anonymous(SchemaType::record(vec![
            NamedFieldType {
                name: "items".to_string(),
                body: SchemaType::stream(Some(SchemaType::string())),
                metadata: MetadataEnvelope::default(),
            },
        ])));

        assert!(!scalar.contains_stream());
        assert!(streaming.contains_stream());
    }

    #[test]
    fn caller_owned_method_only_client_is_lifecycle_free() {
        let definition = MethodOnlyAgentClientDefinition::builder()
            .method_only()
            .method::<String, u64>("lookup")
            .expect("method schema")
            .unit_method::<u32>("invalidate")
            .expect("unit method schema")
            .build();

        assert!(definition.data.type_name.is_none());
        assert_eq!(definition.data.methods.len(), 2);
        assert_eq!(definition.data.methods[0].name, "lookup");
        assert_eq!(definition.data.methods[1].name, "invalidate");
    }

    #[test]
    fn full_client_rejects_a_constructor_value_with_the_wrong_shape() {
        let definition = MethodOnlyAgentClientDefinition::builder()
            .durable::<String>("Counter")
            .build();
        let constructor = definition
            .data
            .constructor
            .as_ref()
            .expect("full client constructor schema");

        assert!(matches!(
            constructor.validate_value(&SchemaValue::U64(1)),
            Err(GolemReflectError::InvalidSchemaValue { .. })
        ));
    }
}

/// Typed method definition retained by a caller-owned client.
#[derive(Clone, Debug)]
pub struct AgentClientMethodDefinition {
    pub name: String,
    pub input: SchemaRef,
    pub output: Option<SchemaRef>,
}

#[derive(Clone, Debug)]
pub struct AgentClientDefinitionBuilder<State> {
    type_name: Option<String>,
    constructor: Option<SchemaRef>,
    methods: Vec<AgentClientMethodDefinition>,
    state: PhantomData<State>,
}

#[derive(Clone, Debug)]
pub struct UnselectedAgentClient;

#[derive(Clone, Debug)]
pub struct MethodOnlyAgentClient;

#[derive(Clone, Debug)]
pub struct NoAgentClientConfig;

impl super::ConfigSchema for NoAgentClientConfig {
    type RpcType = ();

    fn describe_config(_path: &[String]) -> Vec<super::ExtendedAgentConfigDeclaration> {
        Vec::new()
    }

    fn describe_wire_config(
        _path: &[String],
        _builder: &mut crate::schema::wit::direct::WireSchemaBuilder,
    ) -> Vec<crate::golem_agentic::golem::agent::common::AgentConfigDeclaration> {
        Vec::new()
    }

    fn load(_path: &[String]) -> Result<Self, super::ConfigValueError> {
        Ok(Self)
    }
}

#[derive(Clone, Debug)]
pub struct DurableAgentClient;

#[derive(Clone, Debug)]
pub struct EphemeralAgentClient;

#[derive(Clone, Debug)]
pub struct FullAgentClient<Id, Config, Mode>(PhantomData<(Id, Config, Mode)>);

impl AgentClientDefinitionBuilder<UnselectedAgentClient> {
    fn new() -> Self {
        Self {
            type_name: None,
            constructor: None,
            methods: Vec::new(),
            state: PhantomData,
        }
    }

    pub fn method_only(self) -> AgentClientDefinitionBuilder<MethodOnlyAgentClient> {
        AgentClientDefinitionBuilder {
            type_name: None,
            constructor: None,
            methods: self.methods,
            state: PhantomData,
        }
    }

    pub fn durable<Id>(
        self,
        type_name: impl Into<String>,
    ) -> AgentClientDefinitionBuilder<FullAgentClient<Id, NoAgentClientConfig, DurableAgentClient>>
    where
        Id: crate::IntoSchema,
    {
        AgentClientDefinitionBuilder {
            type_name: Some(type_name.into()),
            constructor: Some(SchemaRef::new(
                crate::schema::try_into_schema_graph::<Id>()
                    .expect("fully defined client identity must have a valid schema"),
            )),
            methods: self.methods,
            state: PhantomData,
        }
    }

    pub fn ephemeral<Id>(
        self,
        type_name: impl Into<String>,
    ) -> AgentClientDefinitionBuilder<FullAgentClient<Id, NoAgentClientConfig, EphemeralAgentClient>>
    where
        Id: crate::IntoSchema,
    {
        AgentClientDefinitionBuilder {
            type_name: Some(type_name.into()),
            constructor: Some(SchemaRef::new(
                crate::schema::try_into_schema_graph::<Id>()
                    .expect("fully defined client identity must have a valid schema"),
            )),
            methods: self.methods,
            state: PhantomData,
        }
    }
}

impl<Id, Mode> AgentClientDefinitionBuilder<FullAgentClient<Id, NoAgentClientConfig, Mode>> {
    pub fn config<C>(self) -> AgentClientDefinitionBuilder<FullAgentClient<Id, C, Mode>>
    where
        C: super::ConfigSchema,
    {
        AgentClientDefinitionBuilder {
            type_name: self.type_name,
            constructor: self.constructor,
            methods: self.methods,
            state: PhantomData,
        }
    }
}

impl<State> AgentClientDefinitionBuilder<State> {
    pub fn method<I, O>(mut self, name: impl Into<String>) -> Result<Self, GolemReflectError>
    where
        I: crate::IntoSchema,
        O: crate::IntoSchema,
    {
        let input = crate::schema::try_into_schema_graph::<I>()
            .map_err(|error| GolemReflectError::InvalidType(error.to_string()))?;
        let output = crate::schema::try_into_schema_graph::<O>()
            .map_err(|error| GolemReflectError::InvalidType(error.to_string()))?;
        self.methods.push(AgentClientMethodDefinition {
            name: name.into(),
            input: SchemaRef::new(input),
            output: Some(SchemaRef::new(output)),
        });
        Ok(self)
    }

    pub fn unit_method<I>(mut self, name: impl Into<String>) -> Result<Self, GolemReflectError>
    where
        I: crate::IntoSchema,
    {
        let input = crate::schema::try_into_schema_graph::<I>()
            .map_err(|error| GolemReflectError::InvalidType(error.to_string()))?;
        self.methods.push(AgentClientMethodDefinition {
            name: name.into(),
            input: SchemaRef::new(input),
            output: None,
        });
        Ok(self)
    }

    fn finish(self) -> AgentClientDefinitionData {
        AgentClientDefinitionData {
            type_name: self.type_name,
            constructor: self.constructor,
            methods: self.methods.into(),
        }
    }
}

impl AgentClientDefinitionBuilder<MethodOnlyAgentClient> {
    pub fn build(self) -> MethodOnlyAgentClientDefinition {
        MethodOnlyAgentClientDefinition {
            data: self.finish(),
        }
    }
}

impl<Id, Config, Mode> AgentClientDefinitionBuilder<FullAgentClient<Id, Config, Mode>> {
    pub fn build(self) -> FullAgentClientDefinition<Id, Config, Mode> {
        FullAgentClientDefinition {
            data: self.finish(),
            state: PhantomData,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MethodOnlyAgentClientDefinition {
    data: AgentClientDefinitionData,
}

#[derive(Clone, Debug)]
struct AgentClientDefinitionData {
    type_name: Option<String>,
    constructor: Option<SchemaRef>,
    methods: Arc<[AgentClientMethodDefinition]>,
}

impl MethodOnlyAgentClientDefinition {
    pub fn builder() -> AgentClientDefinitionBuilder<UnselectedAgentClient> {
        AgentClientDefinitionBuilder::new()
    }

    /// Connects to the existing agent with the given ID, whatever its type. Never creates the
    /// agent.
    pub fn get_by_id(&self, agent_id: &AgentId) -> Result<TypedAgentClient, GolemReflectError> {
        Ok(TypedAgentClient {
            definition: self.data.clone(),
            transport: Rc::new(RpcTransport::get_by_id_of_any_type(agent_id)?),
        })
    }
}

#[derive(Clone, Debug)]
pub struct FullAgentClientDefinition<Id, Config, Mode> {
    data: AgentClientDefinitionData,
    state: PhantomData<(Id, Config, Mode)>,
}

fn encode_rpc_config<Config>(
    config: <Config as super::ConfigSchema>::RpcType,
) -> Vec<wire_common::TypedAgentConfigValue>
where
    Config: super::ConfigSchema,
{
    super::IntoRpcConfigParam::into_rpc_param(config, &[])
        .into_iter()
        .map(|value| wire_common::TypedAgentConfigValue {
            path: value.path,
            value: value.value,
        })
        .collect()
}

impl<Id, Config, Mode> FullAgentClientDefinition<Id, Config, Mode>
where
    Id: crate::IntoSchema,
{
    fn type_name(&self) -> &str {
        self.data
            .type_name
            .as_deref()
            .expect("full client has a type name")
    }

    fn constructor_value(&self, constructor: &Id) -> Result<SchemaValue, GolemReflectError> {
        let constructor = constructor.to_value();
        self.data
            .constructor
            .as_ref()
            .expect("full client has a constructor schema")
            .validate_value(&constructor)?;
        Ok(constructor)
    }

    fn client(&self, transport: RpcTransport) -> TypedAgentClient {
        TypedAgentClient {
            definition: self.data.clone(),
            transport: Rc::new(transport),
        }
    }

    /// Connects to the keyed agent of the constructor parameters, and creates it when it is
    /// missing. For an ephemeral type, each invocation runs on a new one-shot instance.
    pub fn upsert(&self, constructor: &Id) -> Result<TypedAgentClient, GolemReflectError> {
        let constructor = self.constructor_value(constructor)?;
        Ok(self.client(RpcTransport::upsert(self.type_name(), &constructor)?))
    }
}

impl<Id, Config> FullAgentClientDefinition<Id, Config, DurableAgentClient>
where
    Id: crate::IntoSchema,
    Config: super::ConfigSchema,
{
    /// Creates a durable agent. With `keyed`, it creates the keyed agent of the constructor
    /// parameters and fails with [`GolemReflectError::Conflict`] when it already exists.
    pub async fn create(
        &self,
        constructor: &Id,
        options: CreateOptions<<Config as super::ConfigSchema>::RpcType>,
    ) -> Result<TypedAgentClient, GolemReflectError> {
        let constructor = self.constructor_value(constructor)?;
        let options = options.try_map_config(|config| Ok(encode_rpc_config::<Config>(config)))?;
        Ok(self.client(RpcTransport::create(self.type_name(), &constructor, options).await?))
    }

    /// Connects to the existing agent of this type with the given ID. Never creates the agent;
    /// an unknown ID or the ID of an agent of another type is
    /// [`GolemReflectError::RemoteNotFound`].
    pub fn get_by_id(&self, agent_id: &AgentId) -> Result<TypedAgentClient, GolemReflectError> {
        Ok(self.client(RpcTransport::get_by_id(self.type_name(), agent_id)?))
    }
}

#[derive(Clone)]
pub struct TypedAgentClient {
    definition: AgentClientDefinitionData,
    transport: Rc<RpcTransport>,
}

impl TypedAgentClient {
    /// The ID of the agent this client connects to. On a client of `upsert` it creates the
    /// agent when it is missing, without invoking a method.
    pub async fn agent_id(&self) -> Result<AgentId, GolemReflectError> {
        self.transport.agent_id().await
    }

    pub fn method<I, O>(&self, name: &str) -> Result<TypedAgentMethod<I, O>, GolemReflectError>
    where
        I: crate::IntoSchema,
        O: crate::FromSchema,
    {
        let definition = self
            .definition
            .methods
            .iter()
            .find(|method| method.name == name)
            .cloned()
            .ok_or_else(|| GolemReflectError::MethodNotFound {
                agent_type: self
                    .definition
                    .type_name
                    .clone()
                    .unwrap_or_else(|| "<method-only client>".to_string()),
                method: name.to_string(),
            })?;
        Ok(TypedAgentMethod {
            definition,
            transport: self.transport.clone(),
            marker: PhantomData,
        })
    }
}

pub struct TypedAgentMethod<I, O> {
    definition: AgentClientMethodDefinition,
    transport: Rc<RpcTransport>,
    marker: PhantomData<fn(I) -> O>,
}

impl<I, O> TypedAgentMethod<I, O>
where
    I: crate::IntoSchema,
    O: crate::FromSchema,
{
    pub async fn invoke(
        &self,
        input: &I,
    ) -> Result<InvocationOutput<Option<O>>, GolemReflectError> {
        self.invoke_scoped(input, None).await
    }

    /// Like [`invoke`](Self::invoke), and lends `scope_card` to the callee for
    /// the duration of the invocation.
    pub async fn invoke_with_scope_card(
        &self,
        input: &I,
        scope_card: &ScopeCard,
    ) -> Result<InvocationOutput<Option<O>>, GolemReflectError> {
        self.invoke_scoped(input, Some(scope_card)).await
    }

    #[doc(hidden)]
    pub async fn invoke_scoped(
        &self,
        input: &I,
        scope_card: Option<&ScopeCard>,
    ) -> Result<InvocationOutput<Option<O>>, GolemReflectError> {
        let input = input.to_value();
        self.definition.input.validate_value(&input)?;
        let invocation = self
            .transport
            .invoke_and_await(&self.definition.name, input, scope_card)
            .await?;
        validate_declared_output(
            self.definition.output.as_ref(),
            invocation.value.as_ref(),
            &self.definition.name,
        )?;
        let value = invocation
            .value
            .as_ref()
            .map(O::from_value)
            .transpose()
            .map_err(|error| GolemReflectError::MalformedRemoteOutput(error.to_string()))?;
        Ok(InvocationOutput {
            metadata: invocation.metadata,
            value,
        })
    }

    /// Triggers the method and returns the handle of the invocation once the target accepted
    /// it, without waiting for its result.
    pub async fn trigger(&self, input: &I) -> Result<Invocation<Option<O>>, GolemReflectError>
    where
        O: 'static,
    {
        self.reject_non_awaited_streams("trigger")?;
        let input = input.to_value();
        self.definition.input.validate_value(&input)?;
        let raw = self.transport.trigger(&self.definition.name, input).await?;
        Invocation::from_raw(raw, self.decoder())
    }

    /// Schedules the method for `at` and returns the handle of the invocation once the target
    /// accepted it.
    pub async fn schedule(
        &self,
        at: ScheduledTime,
        input: &I,
    ) -> Result<Invocation<Option<O>>, GolemReflectError>
    where
        O: 'static,
    {
        self.reject_non_awaited_streams("schedule")?;
        let input = input.to_value();
        self.definition.input.validate_value(&input)?;
        let raw = self
            .transport
            .schedule(at, &self.definition.name, input)
            .await?;
        Invocation::from_raw(raw, self.decoder())
    }

    /// Rebuilds the handle of an invocation of this method from its ID. Makes no remote call.
    pub fn invocation(&self, id: &OwnedInvocationId) -> Invocation<Option<O>>
    where
        O: 'static,
    {
        Invocation::from_id(id, self.decoder())
    }

    fn decoder(&self) -> impl Fn(WireOutput) -> Result<Option<O>, GolemReflectError> + 'static
    where
        O: 'static,
    {
        let checked =
            checked_output_decoder(self.definition.output.clone(), self.definition.name.clone());
        move |wire| {
            checked(wire)?
                .as_ref()
                .map(O::from_value)
                .transpose()
                .map_err(|error| GolemReflectError::MalformedRemoteOutput(error.to_string()))
        }
    }

    fn reject_non_awaited_streams(&self, operation: &str) -> Result<(), GolemReflectError> {
        if self.definition.input.contains_stream()
            || self
                .definition
                .output
                .as_ref()
                .is_some_and(SchemaRef::contains_stream)
        {
            return Err(GolemReflectError::InvalidType(format!(
                "{operation} is unavailable for streaming method `{}`",
                self.definition.name
            )));
        }
        Ok(())
    }
}
