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

//! Round-trip conversion between the recursive in-memory agent schema types in
//! [`super`] and the flat, index-based `golem:agent/common@2.0.0` wire bindings
//! re-exported here as [`wire`].
//!
//! The agent wire form carries one [`wire::SchemaGraph`](crate::schema::wit::wire::SchemaGraph)
//! per agent type (and per dependency). The constructor / method / config
//! schema roots are `type-node-index` values into that shared graph. The
//! conversion uses [`GraphEncoder`] / [`GraphDecoder`] to fold the recursive
//! [`SchemaType`] roots into (and out of) that single flat pool.
//!
//! Non-schema structural fields (HTTP mount / endpoints, snapshotting,
//! principals, …) are byte-identical to the legacy `golem:agent@1.5.0` form;
//! they are converted between the `base_model` representation and the new wire
//! bindings by the `From` impls below, mirroring
//! [`crate::model::agent::conversions`].

use crate::base_model::Empty;
use crate::base_model::agent::{
    AgentConfigDeclaration, AgentConfigSource, AgentHttpAuthDetails, AgentMode, AgentPrincipal,
    CachePolicy, CachePolicyTtl, CorsOptions, CustomHttpMethod, GolemUserPrincipal, HeaderVariable,
    HttpEndpointDetails, HttpMethod, HttpMountDetails, LiteralSegment, OidcPrincipal, PathSegment,
    PathVariable, Principal, QueryVariable, ReadOnlyConfig, Snapshotting, SnapshottingConfig,
    SnapshottingEveryNInvocation, SnapshottingPeriodic, SystemVariable, SystemVariableSegment,
};
use crate::base_model::agent::AgentTypeName;
use crate::schema::adapters::analysed_type::{
    analysed_type_to_schema_type_inline, schema_type_to_analysed_type,
};
use crate::schema::adapters::error::SchemaAdapterError;
use crate::schema::agent::{
    AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema, AgentTypeSchema,
    AutoInjectedKind, FieldSource, InputSchema, NamedField, OutputSchema, RegisteredAgentTypeSchema,
};
use crate::schema::graph::SchemaGraph;
use crate::schema::wit::{
    DecodeError, EncodeError, GraphDecoder, GraphEncoder, decode_metadata, decode_typed,
    encode_metadata, encode_typed,
};

/// Generated `golem:agent/common@2.0.0` types used as the wire shape.
pub use super::bindings::golem::agent::common as wire;

// ============================================================
// Platform identity conversions (base_model <-> golem:core@2.0.0)
//
// The agent surface moved to `golem:core@2.0.0`, whose identity records are
// duplicated copies of the `@1.5.0` ones. `golem-wasm` only provides
// `uuid::Uuid` conversions for its `@1.5.0` re-exports, so the `@2.0.0`
// identity records are bridged to the canonical `base_model` identifiers here.
// ============================================================

use crate::base_model::account::AccountId;
use crate::base_model::component::ComponentId;
use crate::base_model::{AgentId, OplogIndex, PromiseId};

fn uuid_to_core2(value: uuid::Uuid) -> golem_wasm::golem_core_2_0_x::types::Uuid {
    let (high_bits, low_bits) = value.as_u64_pair();
    golem_wasm::golem_core_2_0_x::types::Uuid {
        high_bits,
        low_bits,
    }
}

fn uuid_from_core2(value: golem_wasm::golem_core_2_0_x::types::Uuid) -> uuid::Uuid {
    uuid::Uuid::from_u64_pair(value.high_bits, value.low_bits)
}

impl From<ComponentId> for golem_wasm::golem_core_2_0_x::types::ComponentId {
    fn from(value: ComponentId) -> Self {
        Self {
            uuid: uuid_to_core2(value.0),
        }
    }
}

impl From<golem_wasm::golem_core_2_0_x::types::ComponentId> for ComponentId {
    fn from(value: golem_wasm::golem_core_2_0_x::types::ComponentId) -> Self {
        ComponentId(uuid_from_core2(value.uuid))
    }
}

impl From<AccountId> for golem_wasm::golem_core_2_0_x::types::AccountId {
    fn from(value: AccountId) -> Self {
        Self {
            uuid: uuid_to_core2(value.0),
        }
    }
}

impl From<golem_wasm::golem_core_2_0_x::types::AccountId> for AccountId {
    fn from(value: golem_wasm::golem_core_2_0_x::types::AccountId) -> Self {
        AccountId(uuid_from_core2(value.uuid))
    }
}

impl From<AgentId> for golem_wasm::golem_core_2_0_x::types::AgentId {
    fn from(value: AgentId) -> Self {
        Self {
            component_id: value.component_id.into(),
            agent_id: value.agent_id,
        }
    }
}

impl From<golem_wasm::golem_core_2_0_x::types::AgentId> for AgentId {
    fn from(value: golem_wasm::golem_core_2_0_x::types::AgentId) -> Self {
        AgentId {
            component_id: value.component_id.into(),
            agent_id: value.agent_id,
        }
    }
}

impl From<PromiseId> for golem_wasm::golem_core_2_0_x::types::PromiseId {
    fn from(value: PromiseId) -> Self {
        Self {
            agent_id: value.agent_id.into(),
            oplog_idx: value.oplog_idx.into(),
        }
    }
}

impl From<golem_wasm::golem_core_2_0_x::types::PromiseId> for PromiseId {
    fn from(value: golem_wasm::golem_core_2_0_x::types::PromiseId) -> Self {
        PromiseId {
            agent_id: value.agent_id.into(),
            oplog_idx: OplogIndex::from_u64(value.oplog_idx),
        }
    }
}

/// Errors that can occur while converting between the agent schema layer and
/// the `golem:agent/common@2.0.0` wire bindings.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentWitError {
    /// Failed while flattening a recursive schema into the wire graph.
    Encode(EncodeError),
    /// Failed while reconstructing a recursive schema from the wire graph.
    Decode(DecodeError),
    /// Failed while bridging an agent-config `AnalysedType` value type.
    Adapter(SchemaAdapterError),
}

impl std::fmt::Display for AgentWitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentWitError::Encode(e) => write!(f, "agent schema encode error: {e}"),
            AgentWitError::Decode(e) => write!(f, "agent schema decode error: {e}"),
            AgentWitError::Adapter(e) => write!(f, "agent config type adapter error: {e}"),
        }
    }
}

impl std::error::Error for AgentWitError {}

impl From<EncodeError> for AgentWitError {
    fn from(e: EncodeError) -> Self {
        AgentWitError::Encode(e)
    }
}

impl From<DecodeError> for AgentWitError {
    fn from(e: DecodeError) -> Self {
        AgentWitError::Decode(e)
    }
}

impl From<SchemaAdapterError> for AgentWitError {
    fn from(e: SchemaAdapterError) -> Self {
        AgentWitError::Adapter(e)
    }
}

// ============================================================
// Schema-aware conversions (multi-root graph)
// ============================================================

/// Encode an [`AgentTypeSchema`] into the flat `golem:agent/common@2.0.0`
/// wire form. The agent's `defs` plus every constructor / method / config root
/// are folded into one shared [`wire::SchemaGraph`](crate::schema::wit::wire::SchemaGraph).
pub fn encode_agent_type(ty: &AgentTypeSchema) -> Result<wire::AgentType, AgentWitError> {
    let mut enc = GraphEncoder::new(&ty.schema.defs)?;
    let constructor = encode_constructor(&mut enc, &ty.constructor)?;
    let methods = ty
        .methods
        .iter()
        .map(|m| encode_method(&mut enc, m))
        .collect::<Result<Vec<_>, _>>()?;
    let config = ty
        .config
        .iter()
        .map(|c| encode_config_declaration(&mut enc, c))
        .collect::<Result<Vec<_>, _>>()?;
    let dependencies = ty
        .dependencies
        .iter()
        .map(encode_dependency)
        .collect::<Result<Vec<_>, _>>()?;
    let schema = enc.finish();
    Ok(wire::AgentType {
        type_name: ty.type_name.0.clone(),
        description: ty.description.clone(),
        source_language: ty.source_language.clone(),
        schema,
        constructor,
        methods,
        dependencies,
        mode: ty.mode.into(),
        http_mount: ty.http_mount.clone().map(Into::into),
        snapshotting: ty.snapshotting.clone().into(),
        config,
    })
}

/// Decode an `golem:agent/common@2.0.0` wire [`wire::AgentType`] into the
/// recursive [`AgentTypeSchema`].
pub fn decode_agent_type(w: &wire::AgentType) -> Result<AgentTypeSchema, AgentWitError> {
    let dec = GraphDecoder::new(&w.schema)?;
    let mut schema = SchemaGraph::empty();
    schema.defs = dec.decode_defs()?;
    let constructor = decode_constructor(&dec, &w.constructor)?;
    let methods = w
        .methods
        .iter()
        .map(|m| decode_method(&dec, m))
        .collect::<Result<Vec<_>, _>>()?;
    let config = w
        .config
        .iter()
        .map(|c| decode_config_declaration(&dec, &schema, c))
        .collect::<Result<Vec<_>, _>>()?;
    let dependencies = w
        .dependencies
        .iter()
        .map(decode_dependency)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AgentTypeSchema {
        type_name: AgentTypeName(w.type_name.clone()),
        description: w.description.clone(),
        source_language: w.source_language.clone(),
        schema,
        constructor,
        methods,
        dependencies,
        mode: w.mode.into(),
        http_mount: w.http_mount.clone().map(Into::into),
        snapshotting: w.snapshotting.clone().into(),
        config,
    })
}

/// Encode a [`RegisteredAgentTypeSchema`] into the wire form. The wire type
/// carries only the implementer's `component-id`; the component revision is
/// host-side metadata that does not cross this interface.
pub fn encode_registered_agent_type(
    rt: &RegisteredAgentTypeSchema,
) -> Result<wire::RegisteredAgentType, AgentWitError> {
    Ok(wire::RegisteredAgentType {
        agent_type: encode_agent_type(&rt.agent_type)?,
        implemented_by: rt.implemented_by.component_id.clone().into(),
    })
}

fn encode_dependency(d: &AgentDependencySchema) -> Result<wire::AgentDependency, AgentWitError> {
    let mut enc = GraphEncoder::new(&d.schema.defs)?;
    let constructor = encode_constructor(&mut enc, &d.constructor)?;
    let methods = d
        .methods
        .iter()
        .map(|m| encode_method(&mut enc, m))
        .collect::<Result<Vec<_>, _>>()?;
    let schema = enc.finish();
    Ok(wire::AgentDependency {
        type_name: d.type_name.clone(),
        description: d.description.clone(),
        schema,
        constructor,
        methods,
    })
}

fn decode_dependency(d: &wire::AgentDependency) -> Result<AgentDependencySchema, AgentWitError> {
    let dec = GraphDecoder::new(&d.schema)?;
    let mut schema = SchemaGraph::empty();
    schema.defs = dec.decode_defs()?;
    let constructor = decode_constructor(&dec, &d.constructor)?;
    let methods = d
        .methods
        .iter()
        .map(|m| decode_method(&dec, m))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AgentDependencySchema {
        type_name: d.type_name.clone(),
        description: d.description.clone(),
        schema,
        constructor,
        methods,
    })
}

fn encode_constructor(
    enc: &mut GraphEncoder,
    c: &AgentConstructorSchema,
) -> Result<wire::AgentConstructor, AgentWitError> {
    Ok(wire::AgentConstructor {
        name: c.name.clone(),
        description: c.description.clone(),
        prompt_hint: c.prompt_hint.clone(),
        input_schema: encode_input_schema(enc, &c.input_schema)?,
    })
}

fn decode_constructor(
    dec: &GraphDecoder,
    c: &wire::AgentConstructor,
) -> Result<AgentConstructorSchema, AgentWitError> {
    Ok(AgentConstructorSchema {
        name: c.name.clone(),
        description: c.description.clone(),
        prompt_hint: c.prompt_hint.clone(),
        input_schema: decode_input_schema(dec, &c.input_schema)?,
    })
}

fn encode_method(
    enc: &mut GraphEncoder,
    m: &AgentMethodSchema,
) -> Result<wire::AgentMethod, AgentWitError> {
    Ok(wire::AgentMethod {
        name: m.name.clone(),
        description: m.description.clone(),
        http_endpoint: m.http_endpoint.iter().cloned().map(Into::into).collect(),
        prompt_hint: m.prompt_hint.clone(),
        input_schema: encode_input_schema(enc, &m.input_schema)?,
        output_schema: encode_output_schema(enc, &m.output_schema)?,
        read_only: m.read_only.clone().map(Into::into),
    })
}

fn decode_method(
    dec: &GraphDecoder,
    m: &wire::AgentMethod,
) -> Result<AgentMethodSchema, AgentWitError> {
    Ok(AgentMethodSchema {
        name: m.name.clone(),
        description: m.description.clone(),
        prompt_hint: m.prompt_hint.clone(),
        input_schema: decode_input_schema(dec, &m.input_schema)?,
        output_schema: decode_output_schema(dec, &m.output_schema)?,
        http_endpoint: m.http_endpoint.iter().cloned().map(Into::into).collect(),
        read_only: m.read_only.clone().map(Into::into),
    })
}

fn encode_input_schema(
    enc: &mut GraphEncoder,
    s: &InputSchema,
) -> Result<wire::InputSchema, AgentWitError> {
    match s {
        InputSchema::Parameters(fields) => {
            let encoded = fields
                .iter()
                .map(|f| encode_named_field(enc, f))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(wire::InputSchema::Parameters(encoded))
        }
    }
}

fn decode_input_schema(
    dec: &GraphDecoder,
    s: &wire::InputSchema,
) -> Result<InputSchema, AgentWitError> {
    match s {
        wire::InputSchema::Parameters(fields) => {
            let decoded = fields
                .iter()
                .map(|f| decode_named_field(dec, f))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(InputSchema::Parameters(decoded))
        }
    }
}

fn encode_named_field(
    enc: &mut GraphEncoder,
    f: &NamedField,
) -> Result<wire::NamedField, AgentWitError> {
    let schema = enc.encode_type(&f.schema)?;
    Ok(wire::NamedField {
        name: f.name.clone(),
        source: encode_field_source(&f.source),
        schema,
        metadata: encode_metadata(&f.metadata),
    })
}

fn decode_named_field(
    dec: &GraphDecoder,
    f: &wire::NamedField,
) -> Result<NamedField, AgentWitError> {
    Ok(NamedField {
        name: f.name.clone(),
        source: decode_field_source(&f.source),
        schema: dec.decode_type_at(f.schema)?,
        metadata: decode_metadata(&f.metadata),
    })
}

fn encode_output_schema(
    enc: &mut GraphEncoder,
    o: &OutputSchema,
) -> Result<wire::OutputSchema, AgentWitError> {
    match o {
        OutputSchema::Unit => Ok(wire::OutputSchema::Unit),
        OutputSchema::Single(ty) => Ok(wire::OutputSchema::Single(enc.encode_type(ty)?)),
    }
}

fn decode_output_schema(
    dec: &GraphDecoder,
    o: &wire::OutputSchema,
) -> Result<OutputSchema, AgentWitError> {
    match o {
        wire::OutputSchema::Unit => Ok(OutputSchema::Unit),
        wire::OutputSchema::Single(idx) => {
            Ok(OutputSchema::Single(Box::new(dec.decode_type_at(*idx)?)))
        }
    }
}

fn encode_field_source(s: &FieldSource) -> wire::FieldSource {
    match s {
        FieldSource::UserSupplied => wire::FieldSource::UserSupplied,
        FieldSource::AutoInjected(k) => wire::FieldSource::AutoInjected(encode_auto_injected(k)),
    }
}

fn decode_field_source(s: &wire::FieldSource) -> FieldSource {
    match s {
        wire::FieldSource::UserSupplied => FieldSource::UserSupplied,
        wire::FieldSource::AutoInjected(k) => FieldSource::AutoInjected(decode_auto_injected(k)),
    }
}

fn encode_auto_injected(k: &AutoInjectedKind) -> wire::AutoInjectedKind {
    match k {
        AutoInjectedKind::Principal => wire::AutoInjectedKind::Principal,
    }
}

fn decode_auto_injected(k: &wire::AutoInjectedKind) -> AutoInjectedKind {
    match k {
        wire::AutoInjectedKind::Principal => AutoInjectedKind::Principal,
    }
}

fn encode_config_declaration(
    enc: &mut GraphEncoder,
    c: &AgentConfigDeclaration,
) -> Result<wire::AgentConfigDeclaration, AgentWitError> {
    let schema_ty = analysed_type_to_schema_type_inline(&c.value_type)?;
    let value_type = enc.encode_type(&schema_ty)?;
    Ok(wire::AgentConfigDeclaration {
        source: c.source.into(),
        path: c.path.clone(),
        value_type,
    })
}

fn decode_config_declaration(
    dec: &GraphDecoder,
    graph: &SchemaGraph,
    c: &wire::AgentConfigDeclaration,
) -> Result<AgentConfigDeclaration, AgentWitError> {
    let ty = dec.decode_type_at(c.value_type)?;
    let value_type = schema_type_to_analysed_type(graph, &ty)?;
    Ok(AgentConfigDeclaration {
        source: c.source.into(),
        path: c.path.clone(),
        value_type,
    })
}

// ============================================================
// Agent error
// ============================================================

/// Encode the canonical [`AgentError`](crate::model::agent::AgentError) into
/// its wire form. The `CustomError` payload is a self-contained typed value.
pub fn encode_agent_error(
    e: &crate::model::agent::AgentError,
) -> Result<wire::AgentError, AgentWitError> {
    use crate::model::agent::AgentError as M;
    Ok(match e {
        M::InvalidInput(s) => wire::AgentError::InvalidInput(s.clone()),
        M::InvalidMethod(s) => wire::AgentError::InvalidMethod(s.clone()),
        M::InvalidType(s) => wire::AgentError::InvalidType(s.clone()),
        M::InvalidAgentId(s) => wire::AgentError::InvalidAgentId(s.clone()),
        M::CustomError(typed) => wire::AgentError::CustomError(encode_typed(typed)?),
    })
}

/// Decode a wire [`wire::AgentError`] into the canonical
/// [`AgentError`](crate::model::agent::AgentError).
pub fn decode_agent_error(
    w: wire::AgentError,
) -> Result<crate::model::agent::AgentError, AgentWitError> {
    use crate::model::agent::AgentError as M;
    Ok(match w {
        wire::AgentError::InvalidInput(s) => M::InvalidInput(s),
        wire::AgentError::InvalidMethod(s) => M::InvalidMethod(s),
        wire::AgentError::InvalidType(s) => M::InvalidType(s),
        wire::AgentError::InvalidAgentId(s) => M::InvalidAgentId(s),
        wire::AgentError::CustomError(typed) => M::CustomError(decode_typed(&typed)?),
    })
}

// ============================================================
// Non-schema structural conversions (base_model <-> wire)
//
// These mirror `crate::model::agent::conversions`; the underlying WIT records
// are byte-identical to the legacy `golem:agent@1.5.0` form.
// ============================================================

impl From<wire::AgentMode> for AgentMode {
    fn from(value: wire::AgentMode) -> Self {
        match value {
            wire::AgentMode::Durable => Self::Durable,
            wire::AgentMode::Ephemeral => Self::Ephemeral,
        }
    }
}

impl From<AgentMode> for wire::AgentMode {
    fn from(value: AgentMode) -> Self {
        match value {
            AgentMode::Durable => wire::AgentMode::Durable,
            AgentMode::Ephemeral => wire::AgentMode::Ephemeral,
        }
    }
}

impl From<wire::ReadOnlyConfig> for ReadOnlyConfig {
    fn from(value: wire::ReadOnlyConfig) -> Self {
        Self {
            cache_policy: value.cache_policy.into(),
            uses_principal: value.uses_principal,
        }
    }
}

impl From<ReadOnlyConfig> for wire::ReadOnlyConfig {
    fn from(value: ReadOnlyConfig) -> Self {
        Self {
            cache_policy: value.cache_policy.into(),
            uses_principal: value.uses_principal,
        }
    }
}

impl From<wire::CachePolicy> for CachePolicy {
    fn from(value: wire::CachePolicy) -> Self {
        match value {
            wire::CachePolicy::NoCache => Self::NoCache(Empty {}),
            wire::CachePolicy::UntilWrite => Self::UntilWrite(Empty {}),
            wire::CachePolicy::Ttl(nanos) => Self::Ttl(CachePolicyTtl {
                duration_nanos: nanos,
            }),
        }
    }
}

impl From<CachePolicy> for wire::CachePolicy {
    fn from(value: CachePolicy) -> Self {
        match value {
            CachePolicy::NoCache(_) => Self::NoCache,
            CachePolicy::UntilWrite(_) => Self::UntilWrite,
            CachePolicy::Ttl(ttl) => Self::Ttl(ttl.duration_nanos),
        }
    }
}

impl From<HttpMountDetails> for wire::HttpMountDetails {
    fn from(value: HttpMountDetails) -> Self {
        Self {
            path_prefix: value.path_prefix.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            phantom_agent: value.phantom_agent,
            cors_options: value.cors_options.into(),
            webhook_suffix: value.webhook_suffix.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<wire::HttpMountDetails> for HttpMountDetails {
    fn from(value: wire::HttpMountDetails) -> Self {
        Self {
            path_prefix: value.path_prefix.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            phantom_agent: value.phantom_agent,
            cors_options: value.cors_options.into(),
            webhook_suffix: value.webhook_suffix.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<HttpEndpointDetails> for wire::HttpEndpointDetails {
    fn from(value: HttpEndpointDetails) -> Self {
        Self {
            http_method: value.http_method.into(),
            path_suffix: value.path_suffix.into_iter().map(Into::into).collect(),
            header_vars: value.header_vars.into_iter().map(Into::into).collect(),
            query_vars: value.query_vars.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            cors_options: value.cors_options.into(),
        }
    }
}

impl From<wire::HttpEndpointDetails> for HttpEndpointDetails {
    fn from(value: wire::HttpEndpointDetails) -> Self {
        Self {
            http_method: value.http_method.into(),
            path_suffix: value.path_suffix.into_iter().map(Into::into).collect(),
            header_vars: value.header_vars.into_iter().map(Into::into).collect(),
            query_vars: value.query_vars.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            cors_options: value.cors_options.into(),
        }
    }
}

impl From<HttpMethod> for wire::HttpMethod {
    fn from(value: HttpMethod) -> Self {
        match value {
            HttpMethod::Get(_) => Self::Get,
            HttpMethod::Head(_) => Self::Head,
            HttpMethod::Post(_) => Self::Post,
            HttpMethod::Put(_) => Self::Put,
            HttpMethod::Delete(_) => Self::Delete,
            HttpMethod::Connect(_) => Self::Connect,
            HttpMethod::Options(_) => Self::Options,
            HttpMethod::Trace(_) => Self::Trace,
            HttpMethod::Patch(_) => Self::Patch,
            HttpMethod::Custom(c) => Self::Custom(c.value),
        }
    }
}

impl From<wire::HttpMethod> for HttpMethod {
    fn from(value: wire::HttpMethod) -> Self {
        match value {
            wire::HttpMethod::Get => Self::Get(Empty {}),
            wire::HttpMethod::Head => Self::Head(Empty {}),
            wire::HttpMethod::Post => Self::Post(Empty {}),
            wire::HttpMethod::Put => Self::Put(Empty {}),
            wire::HttpMethod::Delete => Self::Delete(Empty {}),
            wire::HttpMethod::Connect => Self::Connect(Empty {}),
            wire::HttpMethod::Options => Self::Options(Empty {}),
            wire::HttpMethod::Trace => Self::Trace(Empty {}),
            wire::HttpMethod::Patch => Self::Patch(Empty {}),
            wire::HttpMethod::Custom(value) => Self::Custom(CustomHttpMethod { value }),
        }
    }
}

impl From<CorsOptions> for wire::CorsOptions {
    fn from(value: CorsOptions) -> Self {
        Self {
            allowed_patterns: value.allowed_patterns,
        }
    }
}

impl From<wire::CorsOptions> for CorsOptions {
    fn from(value: wire::CorsOptions) -> Self {
        Self {
            allowed_patterns: value.allowed_patterns,
        }
    }
}

impl From<PathSegment> for wire::PathSegment {
    fn from(value: PathSegment) -> Self {
        match value {
            PathSegment::Literal(v) => Self::Literal(v.value),
            PathSegment::SystemVariable(v) => Self::SystemVariable(v.value.into()),
            PathSegment::PathVariable(v) => Self::PathVariable(v.into()),
            PathSegment::RemainingPathVariable(v) => Self::RemainingPathVariable(v.into()),
        }
    }
}

impl From<wire::PathSegment> for PathSegment {
    fn from(value: wire::PathSegment) -> Self {
        match value {
            wire::PathSegment::Literal(value) => Self::Literal(LiteralSegment { value }),
            wire::PathSegment::SystemVariable(value) => Self::SystemVariable(SystemVariableSegment {
                value: value.into(),
            }),
            wire::PathSegment::PathVariable(v) => Self::PathVariable(v.into()),
            wire::PathSegment::RemainingPathVariable(v) => Self::RemainingPathVariable(v.into()),
        }
    }
}

impl From<SystemVariable> for wire::SystemVariable {
    fn from(value: SystemVariable) -> Self {
        match value {
            SystemVariable::AgentType => Self::AgentType,
            SystemVariable::AgentVersion => Self::AgentVersion,
        }
    }
}

impl From<wire::SystemVariable> for SystemVariable {
    fn from(value: wire::SystemVariable) -> Self {
        match value {
            wire::SystemVariable::AgentType => Self::AgentType,
            wire::SystemVariable::AgentVersion => Self::AgentVersion,
        }
    }
}

impl From<PathVariable> for wire::PathVariable {
    fn from(value: PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl From<wire::PathVariable> for PathVariable {
    fn from(value: wire::PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl From<HeaderVariable> for wire::HeaderVariable {
    fn from(value: HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<wire::HeaderVariable> for HeaderVariable {
    fn from(value: wire::HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<QueryVariable> for wire::QueryVariable {
    fn from(value: QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<wire::QueryVariable> for QueryVariable {
    fn from(value: wire::QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<AgentHttpAuthDetails> for wire::AuthDetails {
    fn from(value: AgentHttpAuthDetails) -> Self {
        Self {
            required: value.required,
        }
    }
}

impl From<wire::AuthDetails> for AgentHttpAuthDetails {
    fn from(value: wire::AuthDetails) -> Self {
        Self {
            required: value.required,
        }
    }
}

impl From<Principal> for wire::Principal {
    fn from(value: Principal) -> Self {
        match value {
            Principal::Oidc(inner) => Self::Oidc(inner.into()),
            Principal::Agent(inner) => Self::Agent(inner.into()),
            Principal::GolemUser(inner) => Self::GolemUser(inner.into()),
            Principal::Anonymous(_) => Self::Anonymous,
        }
    }
}

impl From<wire::Principal> for Principal {
    fn from(value: wire::Principal) -> Self {
        match value {
            wire::Principal::Oidc(inner) => Self::Oidc(inner.into()),
            wire::Principal::Agent(inner) => Self::Agent(inner.into()),
            wire::Principal::GolemUser(inner) => Self::GolemUser(inner.into()),
            wire::Principal::Anonymous => Self::Anonymous(Empty {}),
        }
    }
}

impl From<OidcPrincipal> for wire::OidcPrincipal {
    fn from(value: OidcPrincipal) -> Self {
        Self {
            sub: value.sub,
            issuer: value.issuer,
            email: value.email,
            name: value.name,
            email_verified: value.email_verified,
            given_name: value.given_name,
            family_name: value.family_name,
            picture: value.picture,
            preferred_username: value.preferred_username,
            claims: value.claims,
        }
    }
}

impl From<wire::OidcPrincipal> for OidcPrincipal {
    fn from(value: wire::OidcPrincipal) -> Self {
        Self {
            sub: value.sub,
            issuer: value.issuer,
            email: value.email,
            name: value.name,
            email_verified: value.email_verified,
            given_name: value.given_name,
            family_name: value.family_name,
            picture: value.picture,
            preferred_username: value.preferred_username,
            claims: value.claims,
        }
    }
}

impl From<AgentPrincipal> for wire::AgentPrincipal {
    fn from(value: AgentPrincipal) -> Self {
        Self {
            agent_id: value.agent_id.into(),
        }
    }
}

impl From<wire::AgentPrincipal> for AgentPrincipal {
    fn from(value: wire::AgentPrincipal) -> Self {
        Self {
            agent_id: value.agent_id.into(),
        }
    }
}

impl From<GolemUserPrincipal> for wire::GolemUserPrincipal {
    fn from(value: GolemUserPrincipal) -> Self {
        Self {
            account_id: value.account_id.into(),
        }
    }
}

impl From<wire::GolemUserPrincipal> for GolemUserPrincipal {
    fn from(value: wire::GolemUserPrincipal) -> Self {
        Self {
            account_id: value.account_id.into(),
        }
    }
}

impl From<wire::Snapshotting> for Snapshotting {
    fn from(value: wire::Snapshotting) -> Self {
        match value {
            wire::Snapshotting::Disabled => Self::Disabled(Empty {}),
            wire::Snapshotting::Enabled(config) => Self::Enabled(config.into()),
        }
    }
}

impl From<Snapshotting> for wire::Snapshotting {
    fn from(value: Snapshotting) -> Self {
        match value {
            Snapshotting::Disabled(_) => Self::Disabled,
            Snapshotting::Enabled(config) => Self::Enabled(config.into()),
        }
    }
}

impl From<wire::SnapshottingConfig> for SnapshottingConfig {
    fn from(value: wire::SnapshottingConfig) -> Self {
        match value {
            wire::SnapshottingConfig::Default => Self::Default(Empty {}),
            wire::SnapshottingConfig::Periodic(nanos) => Self::Periodic(SnapshottingPeriodic {
                duration_nanos: nanos,
            }),
            wire::SnapshottingConfig::EveryNInvocation(n) => {
                Self::EveryNInvocation(SnapshottingEveryNInvocation { count: n })
            }
        }
    }
}

impl From<SnapshottingConfig> for wire::SnapshottingConfig {
    fn from(value: SnapshottingConfig) -> Self {
        match value {
            SnapshottingConfig::Default(_) => Self::Default,
            SnapshottingConfig::Periodic(periodic) => Self::Periodic(periodic.duration_nanos),
            SnapshottingConfig::EveryNInvocation(every_n) => Self::EveryNInvocation(every_n.count),
        }
    }
}

impl From<wire::AgentConfigSource> for AgentConfigSource {
    fn from(value: wire::AgentConfigSource) -> Self {
        match value {
            wire::AgentConfigSource::Local => Self::Local,
            wire::AgentConfigSource::Secret => Self::Secret,
        }
    }
}

impl From<AgentConfigSource> for wire::AgentConfigSource {
    fn from(value: AgentConfigSource) -> Self {
        match value {
            AgentConfigSource::Local => Self::Local,
            AgentConfigSource::Secret => Self::Secret,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_model::Empty;
    use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
    use crate::schema::metadata::{MetadataEnvelope, TypeId};
    use crate::schema::schema_type::{NamedFieldType, SchemaType};
    use test_r::test;

    fn point_def() -> SchemaTypeDef {
        SchemaTypeDef {
            id: TypeId("myapp.point".to_string()),
            name: Some("Point".to_string()),
            body: SchemaType::record(vec![
                NamedFieldType {
                    name: "x".to_string(),
                    body: SchemaType::u32(),
                    metadata: MetadataEnvelope::default(),
                },
                NamedFieldType {
                    name: "y".to_string(),
                    body: SchemaType::u32(),
                    metadata: MetadataEnvelope::default(),
                },
            ]),
        }
    }

    fn point_ref() -> SchemaType {
        SchemaType::Ref {
            id: TypeId("myapp.point".to_string()),
            metadata: MetadataEnvelope::default(),
        }
    }

    fn sample_agent_type() -> AgentTypeSchema {
        let mut schema = SchemaGraph::empty();
        schema.defs = vec![point_def()];

        let constructor = AgentConstructorSchema {
            name: Some("create".to_string()),
            description: "constructor".to_string(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![
                NamedField::user_supplied("origin", point_ref()),
                NamedField::auto_injected(
                    "caller",
                    AutoInjectedKind::Principal,
                    SchemaType::string(),
                ),
            ]),
        };

        let method_single = AgentMethodSchema {
            name: "move-to".to_string(),
            description: "move".to_string(),
            prompt_hint: Some("hint".to_string()),
            input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
                "target",
                point_ref(),
            )]),
            output_schema: OutputSchema::Single(Box::new(point_ref())),
            http_endpoint: vec![],
            read_only: None,
        };

        let method_unit = AgentMethodSchema {
            name: "reset".to_string(),
            description: "reset".to_string(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        };

        let mut dep_schema = SchemaGraph::empty();
        dep_schema.defs = vec![SchemaTypeDef {
            id: TypeId("dep.item".to_string()),
            name: None,
            body: SchemaType::list(SchemaType::string()),
        }];
        let dependency = AgentDependencySchema {
            type_name: "helper".to_string(),
            description: Some("a dependency".to_string()),
            schema: dep_schema,
            constructor: AgentConstructorSchema {
                name: None,
                description: "dep ctor".to_string(),
                prompt_hint: None,
                input_schema: InputSchema::parameters(vec![NamedField::user_supplied(
                    "items",
                    SchemaType::Ref {
                        id: TypeId("dep.item".to_string()),
                        metadata: MetadataEnvelope::default(),
                    },
                )]),
            },
            methods: vec![],
        };

        let config = vec![AgentConfigDeclaration {
            source: AgentConfigSource::Local,
            path: vec!["api".to_string(), "key".to_string()],
            value_type: golem_wasm::analysis::analysed_type::str(),
        }];

        AgentTypeSchema {
            type_name: AgentTypeName("my-agent".to_string()),
            description: "an agent".to_string(),
            source_language: "rust".to_string(),
            schema,
            constructor,
            methods: vec![method_single, method_unit],
            dependencies: vec![dependency],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config,
        }
    }

    #[test]
    fn agent_type_round_trips_through_wire() {
        let original = sample_agent_type();
        let wire = encode_agent_type(&original).expect("encode");
        let decoded = decode_agent_type(&wire).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn agent_error_round_trips_through_wire() {
        let cases = [
            crate::model::agent::AgentError::InvalidInput("bad".to_string()),
            crate::model::agent::AgentError::InvalidMethod("nope".to_string()),
            crate::model::agent::AgentError::InvalidType("type".to_string()),
            crate::model::agent::AgentError::InvalidAgentId("id".to_string()),
        ];
        for original in cases {
            let wire = encode_agent_error(&original).expect("encode");
            let decoded = decode_agent_error(wire).expect("decode");
            assert_eq!(format!("{decoded}"), format!("{original}"));
        }
    }
}
