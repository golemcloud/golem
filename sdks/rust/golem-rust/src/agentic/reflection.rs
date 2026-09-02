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

use crate::bindings::golem::agent::{common as wire_common, host};
use crate::schema::render::{
    RenderError, from_json_value, to_json_schema_with_config, to_json_value,
};
use crate::schema::validation::validate_value;
use crate::schema::{
    MetadataEnvelope, NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
};
use crate::{AgentId, ComponentId, ScheduledTime, Uuid};
use std::cell::RefCell;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll};

/// A graph-backed view of one schema root.
#[derive(Clone, Debug)]
pub struct SchemaRef {
    graph: Arc<SchemaGraph>,
    root: SchemaType,
}

impl SchemaRef {
    pub fn new(graph: SchemaGraph) -> Self {
        let root = graph.root.clone();
        Self {
            graph: Arc::new(graph),
            root,
        }
    }

    fn with_root(graph: Arc<SchemaGraph>, root: SchemaType) -> Self {
        Self { graph, root }
    }

    pub fn graph(&self) -> &SchemaGraph {
        &self.graph
    }

    pub fn root(&self) -> &SchemaType {
        &self.root
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
        let packed = from_json_value(&self.graph, &self.root, value)?;
        self.validate_value(&packed)?;
        Ok(packed)
    }

    #[cfg(feature = "json")]
    pub fn unpack_json(&self, value: &SchemaValue) -> Result<serde_json::Value, GolemReflectError> {
        self.validate_value(value)?;
        Ok(to_json_value(&self.graph, &self.root, value)?)
    }

    #[cfg(feature = "json")]
    pub fn to_json_schema(&self, include_draft_marker: bool) -> serde_json::Value {
        to_json_schema_with_config(
            &self.graph,
            &self.root,
            crate::schema::render::JsonSchemaConfig {
                include_draft_marker,
            },
        )
    }
}

/// A closed, structured error surface shared by reflection and invocation.
#[derive(Debug)]
pub enum GolemReflectError {
    AgentTypeNotFound(String),
    MethodNotFound { agent_type: String, method: String },
    InvalidAgentId(String),
    InvalidInput(String),
    InvalidType(String),
    KnownEphemeralBinding(String),
    InvalidSchemaValue { issues: Vec<String> },
    SchemaRender(RenderError),
    SchemaEncode(String),
    SchemaDecode(String),
    ProtocolError(String),
    Denied(String),
    RemoteNotFound(String),
    RemoteInternalError(String),
    RemoteAgent(RemoteAgentError),
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
    component_id: ComponentId,
    graph: Arc<SchemaGraph>,
    constructor_input: SchemaRef,
    methods: Arc<[AgentMethod]>,
    mode: AgentMode,
}

impl AgentType {
    fn from_registered(registered: host::RegisteredAgentType) -> Result<Self, GolemReflectError> {
        let component_id = crate::wire_component_id_to_schema(registered.implemented_by);
        let raw = Arc::new(registered.agent_type);
        let graph = Arc::new(
            crate::decode_schema_graph(&raw.schema)
                .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?,
        );
        let constructor_input =
            input_schema_ref(&raw.schema, &raw.constructor.input_schema, &graph)?;
        let methods = raw
            .methods
            .iter()
            .map(|method| {
                Ok(AgentMethod {
                    agent_type_name: raw.type_name.clone(),
                    raw: method.clone(),
                    input: input_schema_ref(&raw.schema, &method.input_schema, &graph)?,
                    output: output_schema_ref(&raw.schema, &method.output_schema, &graph)?,
                })
            })
            .collect::<Result<Vec<_>, GolemReflectError>>()?;
        let mode = match raw.mode {
            wire_common::AgentMode::Durable => AgentMode::Durable,
            wire_common::AgentMode::Ephemeral => AgentMode::Ephemeral,
        };
        Ok(Self {
            raw,
            component_id,
            graph,
            constructor_input,
            methods: methods.into(),
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

    pub fn component_id(&self) -> &ComponentId {
        &self.component_id
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

    pub fn client(&self) -> ReflectedAgentClientFactory {
        ReflectedAgentClientFactory {
            agent_type: self.clone(),
        }
    }

    pub fn agent_id_value(
        &self,
        constructor: SchemaValue,
        phantom_id: Option<Uuid>,
    ) -> Result<AgentId, GolemReflectError> {
        self.constructor_input.validate_value(&constructor)?;
        make_agent_id_value(
            self.component_id.clone(),
            self.name(),
            constructor,
            phantom_id,
        )
    }

    #[cfg(feature = "json")]
    pub fn agent_id_json(
        &self,
        constructor: &serde_json::Value,
        phantom_id: Option<Uuid>,
    ) -> Result<AgentId, GolemReflectError> {
        self.agent_id_value(self.constructor_input.pack_json(constructor)?, phantom_id)
    }

    pub fn bind(&self, agent_id: &AgentId) -> Result<ReflectedAgentClient, GolemReflectError> {
        if self.mode == AgentMode::Ephemeral {
            return Err(GolemReflectError::KnownEphemeralBinding(
                self.name().to_string(),
            ));
        }
        let parts = agent_id.parts()?;
        if parts.type_name != self.name() {
            return Err(GolemReflectError::InvalidType(format!(
                "reflected type `{}` cannot bind identity for `{}`",
                self.name(),
                parts.type_name
            )));
        }
        let transport = RpcTransport::create(
            agent_id.component_id.clone(),
            parts.type_name,
            parts.constructor_value,
            parts.phantom_id,
            Vec::new(),
        )?;
        Ok(ReflectedAgentClient {
            agent_type: self.clone(),
            transport: Rc::new(transport),
            reusable_identity: Some(agent_id.clone()),
        })
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

pub fn get_agent_type_for(agent_id: &AgentId) -> Result<AgentType, GolemReflectError> {
    let raw_id = crate::schema_agent_id_to_wire(agent_id.clone());
    let registered = host::get_agent_type_by_agent_id(&raw_id)
        .ok_or_else(|| GolemReflectError::AgentTypeNotFound(agent_id.agent_id.clone()))?;
    AgentType::from_registered(registered)
}

#[derive(Clone, Debug)]
pub struct AgentIdParts {
    pub component_id: ComponentId,
    pub type_name: String,
    pub constructor_value: SchemaValue,
    pub constructor_schema: SchemaGraph,
    pub phantom_id: Option<Uuid>,
}

pub trait AgentIdExt: Sized {
    fn from_value(
        component_id: ComponentId,
        type_name: impl Into<String>,
        constructor_value: SchemaValue,
        phantom_id: Option<Uuid>,
    ) -> Result<Self, GolemReflectError>;

    fn from_schema<T: crate::IntoSchema>(
        component_id: ComponentId,
        type_name: impl Into<String>,
        constructor: &T,
        phantom_id: Option<Uuid>,
    ) -> Result<Self, GolemReflectError>;

    fn parts(&self) -> Result<AgentIdParts, GolemReflectError>;
    fn dynamic_client(&self) -> Result<DynamicAgentClient, GolemReflectError>;
    fn client(
        &self,
        definition: &AgentClientDefinition,
    ) -> Result<TypedAgentClient, GolemReflectError>;
    fn reflected_client(
        &self,
        agent_type: &AgentType,
    ) -> Result<ReflectedAgentClient, GolemReflectError>;
}

impl AgentIdExt for AgentId {
    fn from_value(
        component_id: ComponentId,
        type_name: impl Into<String>,
        constructor_value: SchemaValue,
        phantom_id: Option<Uuid>,
    ) -> Result<Self, GolemReflectError> {
        make_agent_id_value(
            component_id,
            &type_name.into(),
            constructor_value,
            phantom_id,
        )
    }

    fn from_schema<T: crate::IntoSchema>(
        component_id: ComponentId,
        type_name: impl Into<String>,
        constructor: &T,
        phantom_id: Option<Uuid>,
    ) -> Result<Self, GolemReflectError> {
        Self::from_value(component_id, type_name, constructor.to_value(), phantom_id)
    }

    fn parts(&self) -> Result<AgentIdParts, GolemReflectError> {
        let (type_name, typed, phantom_id) =
            host::parse_agent_id(&self.agent_id).map_err(agent_error_to_reflect)?;
        let typed = crate::decode_typed_schema_value(&typed)
            .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?;
        Ok(AgentIdParts {
            component_id: self.component_id.clone(),
            type_name,
            constructor_value: typed.value().clone(),
            constructor_schema: typed.graph().clone(),
            phantom_id: phantom_id.map(crate::wire_uuid_to_schema),
        })
    }

    fn dynamic_client(&self) -> Result<DynamicAgentClient, GolemReflectError> {
        DynamicAgentClient::from_agent_id(self)
    }

    fn client(
        &self,
        definition: &AgentClientDefinition,
    ) -> Result<TypedAgentClient, GolemReflectError> {
        definition.bind(self)
    }

    fn reflected_client(
        &self,
        agent_type: &AgentType,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        agent_type.bind(self)
    }
}

pub fn make_agent_id_value(
    component_id: ComponentId,
    type_name: &str,
    constructor_value: SchemaValue,
    phantom_id: Option<Uuid>,
) -> Result<AgentId, GolemReflectError> {
    let encoded = crate::encode_schema_value(&constructor_value)
        .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
    let agent_id = host::make_agent_id(
        type_name,
        encoded,
        phantom_id.map(crate::schema_uuid_to_wire),
    )
    .map_err(agent_error_to_reflect)?;
    Ok(AgentId::new(component_id, agent_id))
}

#[derive(Clone, Debug)]
pub struct AgentConfigValue {
    pub path: Vec<String>,
    pub value: TypedSchemaValue,
}

#[derive(Clone, Debug)]
pub struct ReflectedPhantomClient {
    pub agent_id: AgentId,
    pub phantom_id: Uuid,
    pub client: ReflectedAgentClient,
}

#[derive(Clone, Debug)]
pub enum NewPhantomClient {
    Durable(ReflectedPhantomClient),
    Ephemeral(ReflectedAgentClient),
}

#[derive(Clone, Debug)]
pub struct ReflectedAgentClientFactory {
    agent_type: AgentType,
}

impl ReflectedAgentClientFactory {
    pub fn get_value(
        &self,
        constructor: SchemaValue,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        if self.agent_type.mode != AgentMode::Durable {
            return Err(GolemReflectError::InvalidType(
                "get is available only for durable agent types".to_string(),
            ));
        }
        self.create(constructor, None, Vec::new(), true)
    }

    pub fn get_value_with_config(
        &self,
        constructor: SchemaValue,
        config: Vec<AgentConfigValue>,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        if self.agent_type.mode != AgentMode::Durable {
            return Err(GolemReflectError::InvalidType(
                "get_with_config is available only for durable agent types".to_string(),
            ));
        }
        self.create(constructor, None, config, true)
    }

    #[cfg(feature = "json")]
    pub fn get_json(
        &self,
        constructor: &serde_json::Value,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        self.get_value(self.agent_type.constructor_input.pack_json(constructor)?)
    }

    pub fn get_phantom_value(
        &self,
        constructor: SchemaValue,
        phantom_id: Uuid,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        self.create(constructor, Some(phantom_id), Vec::new(), true)
    }

    pub fn get_phantom_value_with_config(
        &self,
        constructor: SchemaValue,
        phantom_id: Uuid,
        config: Vec<AgentConfigValue>,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        self.create(constructor, Some(phantom_id), config, true)
    }

    pub fn new_phantom_value(
        &self,
        constructor: SchemaValue,
    ) -> Result<NewPhantomClient, GolemReflectError> {
        if self.agent_type.mode == AgentMode::Ephemeral {
            return Ok(NewPhantomClient::Ephemeral(self.create(
                constructor,
                None,
                Vec::new(),
                false,
            )?));
        }
        let phantom_id = Uuid::new_v4();
        let agent_id = self
            .agent_type
            .agent_id_value(constructor.clone(), Some(phantom_id))?;
        let client = self.create(constructor, Some(phantom_id), Vec::new(), true)?;
        Ok(NewPhantomClient::Durable(ReflectedPhantomClient {
            agent_id,
            phantom_id,
            client,
        }))
    }

    pub fn new_phantom_value_with_config(
        &self,
        constructor: SchemaValue,
        config: Vec<AgentConfigValue>,
    ) -> Result<NewPhantomClient, GolemReflectError> {
        if self.agent_type.mode == AgentMode::Ephemeral {
            return Ok(NewPhantomClient::Ephemeral(self.create(
                constructor,
                None,
                config,
                false,
            )?));
        }
        let phantom_id = Uuid::new_v4();
        let agent_id = self
            .agent_type
            .agent_id_value(constructor.clone(), Some(phantom_id))?;
        let client = self.create(constructor, Some(phantom_id), config, true)?;
        Ok(NewPhantomClient::Durable(ReflectedPhantomClient {
            agent_id,
            phantom_id,
            client,
        }))
    }

    #[cfg(feature = "json")]
    pub fn new_phantom_json(
        &self,
        constructor: &serde_json::Value,
    ) -> Result<NewPhantomClient, GolemReflectError> {
        self.new_phantom_value(self.agent_type.constructor_input.pack_json(constructor)?)
    }

    fn create(
        &self,
        constructor: SchemaValue,
        phantom_id: Option<Uuid>,
        config: Vec<AgentConfigValue>,
        reusable: bool,
    ) -> Result<ReflectedAgentClient, GolemReflectError> {
        self.agent_type
            .constructor_input
            .validate_value(&constructor)?;
        let identity = reusable
            .then(|| {
                self.agent_type
                    .agent_id_value(constructor.clone(), phantom_id)
            })
            .transpose()?;
        let transport = RpcTransport::create(
            self.agent_type.component_id.clone(),
            self.agent_type.name().to_string(),
            constructor,
            phantom_id,
            config,
        )?;
        Ok(ReflectedAgentClient {
            agent_type: self.agent_type.clone(),
            transport: Rc::new(transport),
            reusable_identity: identity,
        })
    }
}

#[derive(Clone)]
pub struct ReflectedAgentClient {
    agent_type: AgentType,
    transport: Rc<RpcTransport>,
    reusable_identity: Option<AgentId>,
}

impl std::fmt::Debug for ReflectedAgentClient {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReflectedAgentClient")
            .field("agent_type", &self.agent_type.name())
            .field("reusable_identity", &self.reusable_identity)
            .finish_non_exhaustive()
    }
}

impl ReflectedAgentClient {
    pub fn agent_id(&self) -> Option<&AgentId> {
        self.reusable_identity.as_ref()
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
    transport: Rc<RpcTransport>,
}

impl ReflectedAgentMethod {
    pub fn definition(&self) -> &AgentMethod {
        &self.definition
    }

    pub async fn invoke_value(
        &self,
        input: SchemaValue,
    ) -> Result<Invocation<Option<SchemaValue>>, GolemReflectError> {
        self.definition.input.validate_value(&input)?;
        let invocation = self
            .transport
            .invoke_and_await(&self.definition.raw.name, input)
            .await?;
        if let (Some(output), Some(value)) = (&self.definition.output, &invocation.value) {
            output.validate_value(value)?;
        } else if self.definition.output.is_some() != invocation.value.is_some() {
            return Err(GolemReflectError::InvalidType(format!(
                "method `{}` returned an unexpected unit/value shape",
                self.definition.raw.name
            )));
        }
        Ok(invocation)
    }

    #[cfg(feature = "json")]
    pub async fn invoke_json(
        &self,
        input: &serde_json::Value,
    ) -> Result<Invocation<Option<serde_json::Value>>, GolemReflectError> {
        let invocation = self
            .invoke_value(self.definition.input.pack_json(input)?)
            .await?;
        let value = match (&self.definition.output, invocation.value) {
            (Some(schema), Some(value)) => Some(schema.unpack_json(&value)?),
            _ => None,
        };
        Ok(Invocation {
            metadata: invocation.metadata,
            value,
        })
    }

    pub fn trigger_value(
        &self,
        input: SchemaValue,
    ) -> Result<InvocationMetadata, GolemReflectError> {
        self.definition.input.validate_value(&input)?;
        self.transport.trigger(&self.definition.raw.name, input)
    }

    pub fn pending_value(
        &self,
        input: SchemaValue,
    ) -> Result<PendingInvocation, GolemReflectError> {
        self.definition.input.validate_value(&input)?;
        self.transport.pending(&self.definition.raw.name, input)
    }

    pub fn schedule_value(
        &self,
        at: ScheduledTime,
        input: SchemaValue,
    ) -> Result<ScheduledInvocation, GolemReflectError> {
        self.definition.input.validate_value(&input)?;
        self.transport
            .schedule(at, &self.definition.raw.name, input)
    }
}

#[derive(Clone)]
pub struct DynamicAgentClient {
    transport: Rc<RpcTransport>,
    reusable_identity: Option<AgentId>,
}

impl DynamicAgentClient {
    pub fn from_agent_id(agent_id: &AgentId) -> Result<Self, GolemReflectError> {
        let parts = agent_id.parts()?;
        let transport = RpcTransport::create(
            agent_id.component_id.clone(),
            parts.type_name,
            parts.constructor_value,
            parts.phantom_id,
            Vec::new(),
        )?;
        Ok(Self {
            transport: Rc::new(transport),
            reusable_identity: Some(agent_id.clone()),
        })
    }

    /// Construct a raw one-shot invocation address. No reusable identity is
    /// guaranteed before invocation; final identity comes from metadata.
    pub fn ephemeral(
        component_id: ComponentId,
        type_name: impl Into<String>,
        constructor: SchemaValue,
    ) -> Result<Self, GolemReflectError> {
        Ok(Self {
            transport: Rc::new(RpcTransport::create(
                component_id,
                type_name.into(),
                constructor,
                None,
                Vec::new(),
            )?),
            reusable_identity: None,
        })
    }

    pub fn agent_id(&self) -> Option<&AgentId> {
        self.reusable_identity.as_ref()
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
    ) -> Result<Invocation<Option<SchemaValue>>, GolemReflectError> {
        self.transport.invoke_and_await(&self.name, input).await
    }

    pub fn trigger_value(
        &self,
        input: SchemaValue,
    ) -> Result<InvocationMetadata, GolemReflectError> {
        self.transport.trigger(&self.name, input)
    }

    pub fn pending_value(
        &self,
        input: SchemaValue,
    ) -> Result<PendingInvocation, GolemReflectError> {
        self.transport.pending(&self.name, input)
    }

    pub fn schedule_value(
        &self,
        at: ScheduledTime,
        input: SchemaValue,
    ) -> Result<ScheduledInvocation, GolemReflectError> {
        self.transport.schedule(at, &self.name, input)
    }
}

#[derive(Clone, Debug)]
pub struct InvocationMetadata {
    pub agent_id: AgentId,
    pub idempotency_key: String,
}

#[derive(Clone, Debug)]
pub struct Invocation<T> {
    pub metadata: InvocationMetadata,
    pub value: T,
}

pub struct CancellationToken {
    raw: host::CancellationToken,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.raw.cancel();
    }
}

impl std::fmt::Debug for CancellationToken {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationToken").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ScheduledInvocation {
    pub metadata: InvocationMetadata,
    pub cancellation_token: CancellationToken,
}

type PendingResult = Result<Invocation<Option<SchemaValue>>, GolemReflectError>;

pub struct PendingInvocation {
    pub metadata: InvocationMetadata,
    raw: Rc<host::FutureInvokeResult>,
    component_id: ComponentId,
    state: RefCell<Option<Pin<Box<dyn Future<Output = PendingResult>>>>>,
}

impl PendingInvocation {
    pub fn cancel(&self) {
        self.raw.cancel();
    }

    pub async fn get(self) -> PendingResult {
        self.await
    }
}

impl Future for PendingInvocation {
    type Output = PendingResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.state.borrow().is_none() {
            let raw = this.raw.clone();
            let component_id = this.component_id.clone();
            let metadata = this.metadata.clone();
            *this.state.borrow_mut() = Some(Box::pin(async move {
                let result = raw.get().await.map_err(rpc_error_to_reflect)?;
                let value = result
                    .map(crate::decode_schema_value)
                    .transpose()
                    .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?;
                Ok(Invocation {
                    metadata: InvocationMetadata {
                        agent_id: AgentId::new(component_id, metadata.agent_id.agent_id.clone()),
                        idempotency_key: metadata.idempotency_key,
                    },
                    value,
                })
            }));
        }
        let mut state = this.state.borrow_mut();
        state
            .as_mut()
            .expect("pending invocation future initialized")
            .as_mut()
            .poll(cx)
    }
}

struct RpcTransport {
    component_id: ComponentId,
    raw: host::WasmRpc,
}

impl RpcTransport {
    fn create(
        component_id: ComponentId,
        type_name: String,
        constructor: SchemaValue,
        phantom_id: Option<Uuid>,
        config: Vec<AgentConfigValue>,
    ) -> Result<Self, GolemReflectError> {
        let constructor = crate::encode_schema_value(&constructor)
            .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
        let config = config
            .into_iter()
            .map(|value| {
                Ok(wire_common::TypedAgentConfigValue {
                    path: value.path,
                    value: crate::encode_typed_schema_value_owned(value.value)
                        .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?,
                })
            })
            .collect::<Result<Vec<_>, GolemReflectError>>()?;
        let raw = host::WasmRpc::create(
            &type_name,
            constructor,
            phantom_id.map(crate::schema_uuid_to_wire),
            config,
        )
        .map_err(rpc_error_to_reflect)?;
        Ok(Self { component_id, raw })
    }

    async fn invoke_and_await(
        &self,
        method: &str,
        input: SchemaValue,
    ) -> Result<Invocation<Option<SchemaValue>>, GolemReflectError> {
        let input = crate::encode_schema_value_async(&input)
            .await
            .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
        let result = self
            .raw
            .invoke_and_await(method, input, None)
            .map_err(rpc_error_to_reflect)?;
        let value = result
            .result
            .map(crate::decode_schema_value)
            .transpose()
            .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?;
        Ok(Invocation {
            metadata: self.metadata(result.metadata),
            value,
        })
    }

    fn trigger(
        &self,
        method: &str,
        input: SchemaValue,
    ) -> Result<InvocationMetadata, GolemReflectError> {
        let input = crate::encode_schema_value(&input)
            .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
        let metadata = self
            .raw
            .invoke(method, input, None)
            .map_err(rpc_error_to_reflect)?;
        Ok(self.metadata(metadata))
    }

    fn pending(
        &self,
        method: &str,
        input: SchemaValue,
    ) -> Result<PendingInvocation, GolemReflectError> {
        let input = crate::encode_schema_value(&input)
            .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
        let pending = self.raw.async_invoke_and_await(method, input, None);
        let metadata = self.metadata(pending.metadata);
        Ok(PendingInvocation {
            metadata,
            raw: Rc::new(pending.future),
            component_id: self.component_id.clone(),
            state: RefCell::new(None),
        })
    }

    fn schedule(
        &self,
        at: ScheduledTime,
        method: &str,
        input: SchemaValue,
    ) -> Result<ScheduledInvocation, GolemReflectError> {
        let input = crate::encode_schema_value(&input)
            .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
        let receipt = self
            .raw
            .schedule_cancelable_invocation(at, method, input, None)
            .map_err(rpc_error_to_reflect)?;
        Ok(ScheduledInvocation {
            metadata: self.metadata(receipt.metadata),
            cancellation_token: CancellationToken {
                raw: receipt.cancellation_token,
            },
        })
    }

    fn metadata(&self, raw: host::InvocationMetadata) -> InvocationMetadata {
        InvocationMetadata {
            agent_id: AgentId::new(self.component_id.clone(), raw.agent_id),
            idempotency_key: raw.idempotency_key,
        }
    }
}

fn input_schema_ref(
    wire_graph: &crate::schema::wit::wire::SchemaGraph,
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
                body: decode_root(wire_graph, field.schema)?,
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
    wire_graph: &crate::schema::wit::wire::SchemaGraph,
    output: &wire_common::OutputSchema,
    graph: &Arc<SchemaGraph>,
) -> Result<Option<SchemaRef>, GolemReflectError> {
    match output {
        wire_common::OutputSchema::Unit => Ok(None),
        wire_common::OutputSchema::Single(root) => Ok(Some(SchemaRef::with_root(
            graph.clone(),
            decode_root(wire_graph, *root)?,
        ))),
    }
}

fn decode_root(
    wire_graph: &crate::schema::wit::wire::SchemaGraph,
    root: i32,
) -> Result<SchemaType, GolemReflectError> {
    let mut rooted = wire_graph.clone();
    rooted.root = root;
    crate::decode_schema_graph(&rooted)
        .map(|graph| graph.root)
        .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))
}

fn rpc_error_to_reflect(error: host::RpcError) -> GolemReflectError {
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
    }
}

fn agent_error_to_reflect(error: wire_common::AgentError) -> GolemReflectError {
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

fn decode_custom_error(value: crate::schema::wit::wire::TypedSchemaValue) -> RemoteAgentError {
    match crate::decode_typed_schema_value(&value) {
        Ok(value) => RemoteAgentError::Custom(Box::new(value)),
        Err(error) => RemoteAgentError::MalformedCustomError(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentClientDefinition, GolemReflectError, SchemaRef};
    use crate::schema::{
        MetadataEnvelope, NamedFieldType, SchemaGraph, SchemaType, SchemaValue, VariantCaseType,
        VariantValuePayload,
    };
    use serde_json::json;
    use test_r::test;

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
    fn caller_owned_contract_can_be_partial_and_lifecycle_free() {
        let definition = AgentClientDefinition::builder()
            .method::<String, u64>("lookup")
            .expect("method schema")
            .unit_method::<u32>("invalidate")
            .expect("unit method schema")
            .build();

        assert!(definition.type_name.is_none());
        assert_eq!(definition.methods.len(), 2);
        assert_eq!(definition.methods[0].name, "lookup");
        assert_eq!(definition.methods[1].name, "invalidate");
    }
}

/// Typed Level 2 method contract retained by a caller-owned definition.
#[derive(Clone, Debug)]
pub struct AgentClientMethodDefinition {
    pub name: String,
    pub input: SchemaRef,
    pub output: Option<SchemaRef>,
}

#[derive(Clone, Debug, Default)]
pub struct AgentClientDefinitionBuilder {
    type_name: Option<String>,
    methods: Vec<AgentClientMethodDefinition>,
}

impl AgentClientDefinitionBuilder {
    pub fn type_name(mut self, type_name: impl Into<String>) -> Self {
        self.type_name = Some(type_name.into());
        self
    }

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

    pub fn build(self) -> AgentClientDefinition {
        AgentClientDefinition {
            type_name: self.type_name,
            methods: self.methods.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentClientDefinition {
    type_name: Option<String>,
    methods: Arc<[AgentClientMethodDefinition]>,
}

impl AgentClientDefinition {
    pub fn builder() -> AgentClientDefinitionBuilder {
        AgentClientDefinitionBuilder::default()
    }

    pub fn bind(&self, agent_id: &AgentId) -> Result<TypedAgentClient, GolemReflectError> {
        let parts = agent_id.parts()?;
        if let Some(expected) = &self.type_name
            && expected != &parts.type_name
        {
            return Err(GolemReflectError::InvalidType(format!(
                "client contract expects `{expected}`, identity is `{}`",
                parts.type_name
            )));
        }
        let transport = RpcTransport::create(
            agent_id.component_id.clone(),
            parts.type_name,
            parts.constructor_value,
            parts.phantom_id,
            Vec::new(),
        )?;
        Ok(TypedAgentClient {
            definition: self.clone(),
            transport: Rc::new(transport),
        })
    }
}

#[derive(Clone)]
pub struct TypedAgentClient {
    definition: AgentClientDefinition,
    transport: Rc<RpcTransport>,
}

impl TypedAgentClient {
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
                    .unwrap_or_else(|| "<caller contract>".to_string()),
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
    pub async fn invoke(&self, input: &I) -> Result<Invocation<Option<O>>, GolemReflectError> {
        let input = input.to_value();
        self.definition.input.validate_value(&input)?;
        let invocation = self
            .transport
            .invoke_and_await(&self.definition.name, input)
            .await?;
        if let (Some(schema), Some(value)) = (&self.definition.output, &invocation.value) {
            schema.validate_value(value)?;
        } else if self.definition.output.is_some() != invocation.value.is_some() {
            return Err(GolemReflectError::InvalidType(format!(
                "method `{}` returned an unexpected unit/value shape",
                self.definition.name
            )));
        }
        let value = invocation
            .value
            .as_ref()
            .map(O::from_value)
            .transpose()
            .map_err(|error| GolemReflectError::InvalidType(error.to_string()))?;
        Ok(Invocation {
            metadata: invocation.metadata,
            value,
        })
    }

    pub fn trigger(&self, input: &I) -> Result<InvocationMetadata, GolemReflectError> {
        let input = input.to_value();
        self.definition.input.validate_value(&input)?;
        self.transport.trigger(&self.definition.name, input)
    }

    pub fn pending(&self, input: &I) -> Result<TypedPendingInvocation<O>, GolemReflectError> {
        let input = input.to_value();
        self.definition.input.validate_value(&input)?;
        Ok(TypedPendingInvocation {
            inner: self.transport.pending(&self.definition.name, input)?,
            output: self.definition.output.clone(),
            marker: PhantomData,
        })
    }

    pub fn schedule(
        &self,
        at: ScheduledTime,
        input: &I,
    ) -> Result<ScheduledInvocation, GolemReflectError> {
        let input = input.to_value();
        self.definition.input.validate_value(&input)?;
        self.transport.schedule(at, &self.definition.name, input)
    }
}

pub struct TypedPendingInvocation<O> {
    inner: PendingInvocation,
    output: Option<SchemaRef>,
    marker: PhantomData<fn() -> O>,
}

impl<O> TypedPendingInvocation<O> {
    pub fn metadata(&self) -> &InvocationMetadata {
        &self.inner.metadata
    }

    pub fn cancel(&self) {
        self.inner.cancel();
    }
}

impl<O: crate::FromSchema> Future for TypedPendingInvocation<O> {
    type Output = Result<Invocation<Option<O>>, GolemReflectError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(invocation)) => {
                if let (Some(schema), Some(value)) = (&this.output, &invocation.value) {
                    if let Err(error) = schema.validate_value(value) {
                        return Poll::Ready(Err(error));
                    }
                } else if this.output.is_some() != invocation.value.is_some() {
                    return Poll::Ready(Err(GolemReflectError::InvalidType(
                        "pending invocation returned an unexpected unit/value shape".to_string(),
                    )));
                }
                let value = match invocation.value.as_ref().map(O::from_value).transpose() {
                    Ok(value) => value,
                    Err(error) => {
                        return Poll::Ready(Err(GolemReflectError::InvalidType(error.to_string())));
                    }
                };
                Poll::Ready(Ok(Invocation {
                    metadata: invocation.metadata,
                    value,
                }))
            }
        }
    }
}
