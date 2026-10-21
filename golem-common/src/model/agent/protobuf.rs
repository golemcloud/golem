use super::AgentMetadataDto;
use super::{
    AgentConfigEntryDto, ExportedResourceMetadata, FailedUpdate, PendingUpdate,
    SnapshotAssistedUpdateMetadata, SuccessfulUpdate, TypedAgentConfigEntry, UpdateRecord,
};
use super::{
    AgentConfigSource, AgentHttpAuthDetails, AgentInvocationMode, AgentMode, AgentPrincipal,
    CachePolicy, CachePolicyTtl, CorsOptions, CustomHttpMethod, DurableStreamInputSlotSource,
    DurableStreamOutputSlotSource, DurableStreamRouteLoadOptions, DurableStreamRouteOptions,
    DurableStreamSlotOptions, DurableStreamSlotSource, FileResponseHeader, GolemUserPrincipal,
    HeaderVariable, HttpEndpointDetails, HttpMethod, HttpMountDetails, LiteralSegment,
    OidcPrincipal, PathSegment, PathVariable, PhantomIdBinding, PhantomIdBindingDetails, Principal,
    QueryVariable, ReadOnlyConfig, RegisteredAgentType, RegisteredAgentTypeImplementer,
    Snapshotting, SnapshottingConfig, SnapshottingEveryNInvocation, SnapshottingPeriodic,
    SystemVariable, SystemVariableSegment,
};
use super::{AgentUpdateMode, RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget};
use crate::base_model::AgentFingerprint;
use crate::base_model::agent::{ExactFileMapping, FileMapping, SubtreeFileMapping};
use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::model::Empty;
use crate::model::oplog::{AgentResourceId, OplogErrorKind};
use crate::model::regions::OplogRegion;
use crate::model::{AgentResourceDescription, OplogIndex};
use std::collections::HashSet;

impl From<golem_api_grpc::proto::golem::component::AgentMode> for AgentMode {
    fn from(value: golem_api_grpc::proto::golem::component::AgentMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::component::AgentMode::Durable => AgentMode::Durable,
            golem_api_grpc::proto::golem::component::AgentMode::Ephemeral => AgentMode::Ephemeral,
        }
    }
}

impl From<AgentMode> for golem_api_grpc::proto::golem::component::AgentMode {
    fn from(value: AgentMode) -> Self {
        match value {
            AgentMode::Durable => golem_api_grpc::proto::golem::component::AgentMode::Durable,
            AgentMode::Ephemeral => golem_api_grpc::proto::golem::component::AgentMode::Ephemeral,
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::AgentInvocationMode> for AgentInvocationMode {
    fn from(value: golem_api_grpc::proto::golem::worker::AgentInvocationMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await => {
                AgentInvocationMode::Await
            }
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule => {
                AgentInvocationMode::Schedule
            }
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup => {
                AgentInvocationMode::Lookup
            }
        }
    }
}

impl From<AgentInvocationMode> for golem_api_grpc::proto::golem::worker::AgentInvocationMode {
    fn from(value: AgentInvocationMode) -> Self {
        match value {
            AgentInvocationMode::Await => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await
            }
            AgentInvocationMode::Schedule => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule
            }
            AgentInvocationMode::Lookup => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ReadOnlyConfig> for ReadOnlyConfig {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ReadOnlyConfig,
    ) -> Result<Self, Self::Error> {
        Ok(ReadOnlyConfig {
            cache_policy: value
                .cache_policy
                .ok_or_else(|| "Missing field: cache_policy".to_string())?
                .try_into()?,
            uses_principal: value.uses_principal,
        })
    }
}

impl From<ReadOnlyConfig> for golem_api_grpc::proto::golem::component::ReadOnlyConfig {
    fn from(value: ReadOnlyConfig) -> Self {
        golem_api_grpc::proto::golem::component::ReadOnlyConfig {
            cache_policy: Some(value.cache_policy.into()),
            uses_principal: value.uses_principal,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::CachePolicy> for CachePolicy {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::CachePolicy,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::cache_policy::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::NoCache(_) => Ok(Self::NoCache(Empty {})),
            Value::UntilWrite(_) => Ok(Self::UntilWrite(Empty {})),
            Value::TtlNanos(nanos) => Ok(Self::Ttl(CachePolicyTtl {
                duration_nanos: nanos,
            })),
        }
    }
}

impl From<CachePolicy> for golem_api_grpc::proto::golem::component::CachePolicy {
    fn from(value: CachePolicy) -> Self {
        use golem_api_grpc::proto::golem::component::cache_policy::Value;

        Self {
            value: Some(match value {
                CachePolicy::NoCache(_) => {
                    Value::NoCache(golem_schema::proto::golem::common::Empty {})
                }
                CachePolicy::UntilWrite(_) => {
                    Value::UntilWrite(golem_schema::proto::golem::common::Empty {})
                }
                CachePolicy::Ttl(ttl) => Value::TtlNanos(ttl.duration_nanos),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::RegisteredAgentTypeImplementer>
    for RegisteredAgentTypeImplementer
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::RegisteredAgentTypeImplementer,
    ) -> Result<Self, Self::Error> {
        Ok(RegisteredAgentTypeImplementer {
            component_id: value
                .component_id
                .ok_or_else(|| "Missing component_id field".to_string())?
                .try_into()?,
            component_revision: value.component_revision.try_into()?,
            component_name: value.component_name,
            account_id: value
                .account_id
                .ok_or_else(|| "Missing account_id field".to_string())?
                .try_into()?,
            account_email: crate::model::account::AccountEmail::new(value.account_email),
        })
    }
}

impl From<RegisteredAgentTypeImplementer>
    for golem_api_grpc::proto::golem::registry::RegisteredAgentTypeImplementer
{
    fn from(value: RegisteredAgentTypeImplementer) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            component_revision: value.component_revision.into(),
            component_name: value.component_name,
            account_id: Some(value.account_id.into()),
            account_email: value.account_email.into_inner(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::RegisteredAgentType> for RegisteredAgentType {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::RegisteredAgentType,
    ) -> Result<Self, Self::Error> {
        Ok(RegisteredAgentType {
            agent_type: value
                .agent_type
                .ok_or_else(|| "Missing agent_type field".to_string())?
                .try_into()?,
            implemented_by: value
                .implemented_by
                .ok_or_else(|| "Missing implemented_by field".to_string())?
                .try_into()?,
        })
    }
}

impl From<RegisteredAgentType> for golem_api_grpc::proto::golem::registry::RegisteredAgentType {
    fn from(value: RegisteredAgentType) -> Self {
        Self {
            agent_type: Some(value.agent_type.into()),
            implemented_by: Some(value.implemented_by.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HttpMountDetails> for HttpMountDetails {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HttpMountDetails,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            path_prefix: value
                .path_prefix
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            auth_details: value.auth_details.map(TryInto::try_into).transpose()?,
            phantom_agent: value.phantom_agent,
            phantom_id_binding: value
                .phantom_id_binding
                .map(TryInto::try_into)
                .transpose()?,
            cors_options: value
                .cors_options
                .ok_or_else(|| "Missing field: cors_options".to_string())?
                .try_into()?,
            webhook_suffix: value
                .webhook_suffix
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            static_bindings: value
                .static_bindings
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            filesystem_bindings: value
                .filesystem_bindings
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            file_response_headers: value
                .file_response_headers
                .into_iter()
                .map(Into::into)
                .collect(),
            openapi_provider_method: value.openapi_provider_method,
        })
    }
}

impl From<HttpMountDetails> for golem_api_grpc::proto::golem::component::HttpMountDetails {
    fn from(value: HttpMountDetails) -> Self {
        Self {
            path_prefix: value.path_prefix.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            phantom_agent: value.phantom_agent,
            phantom_id_binding: value.phantom_id_binding.map(Into::into),
            cors_options: Some(value.cors_options.into()),
            webhook_suffix: value.webhook_suffix.into_iter().map(Into::into).collect(),
            static_bindings: value.static_bindings.into_iter().map(Into::into).collect(),
            filesystem_bindings: value
                .filesystem_bindings
                .into_iter()
                .map(Into::into)
                .collect(),
            file_response_headers: value
                .file_response_headers
                .into_iter()
                .map(Into::into)
                .collect(),
            openapi_provider_method: value.openapi_provider_method,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PhantomIdBinding> for PhantomIdBinding {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PhantomIdBinding,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::phantom_id_binding::Source;
        match value.source.ok_or("Missing PhantomIdBinding.source")? {
            Source::Path(value) => Ok(Self::Path(PhantomIdBindingDetails {
                name: value.name,
                optional: value.optional,
            })),
            Source::Query(value) => Ok(Self::Query(PhantomIdBindingDetails {
                name: value.name,
                optional: value.optional,
            })),
        }
    }
}

impl From<PhantomIdBinding> for golem_api_grpc::proto::golem::component::PhantomIdBinding {
    fn from(value: PhantomIdBinding) -> Self {
        use golem_api_grpc::proto::golem::component::{
            PhantomIdBindingDetails, phantom_id_binding::Source,
        };
        Self {
            source: Some(match value {
                PhantomIdBinding::Path(value) => Source::Path(PhantomIdBindingDetails {
                    name: value.name,
                    optional: value.optional,
                }),
                PhantomIdBinding::Query(value) => Source::Query(PhantomIdBindingDetails {
                    name: value.name,
                    optional: value.optional,
                }),
            }),
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::FileResponseHeader> for FileResponseHeader {
    fn from(value: golem_api_grpc::proto::golem::component::FileResponseHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl From<FileResponseHeader> for golem_api_grpc::proto::golem::component::FileResponseHeader {
    fn from(value: FileResponseHeader) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::FileMapping> for FileMapping {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::FileMapping,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::file_mapping::Value;
        match value.value.ok_or("Missing FileMapping.value")? {
            Value::Exact(mapping) => Ok(Self::Exact(ExactFileMapping {
                public_path: mapping.public_path,
                file_path: mapping.file_path,
            })),
            Value::Subtree(mapping) => Ok(Self::Subtree(SubtreeFileMapping {
                public_prefix: mapping.public_prefix,
                filesystem_root: mapping.filesystem_root,
            })),
        }
    }
}

impl From<FileMapping> for golem_api_grpc::proto::golem::component::FileMapping {
    fn from(value: FileMapping) -> Self {
        use golem_api_grpc::proto::golem::component::{self as proto, file_mapping::Value};
        Self {
            value: Some(match value {
                FileMapping::Exact(mapping) => Value::Exact(proto::ExactFileMapping {
                    public_path: mapping.public_path,
                    file_path: mapping.file_path,
                }),
                FileMapping::Subtree(mapping) => Value::Subtree(proto::SubtreeFileMapping {
                    public_prefix: mapping.public_prefix,
                    filesystem_root: mapping.filesystem_root,
                }),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HttpEndpointDetails> for HttpEndpointDetails {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HttpEndpointDetails,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            http_method: value
                .http_method
                .ok_or_else(|| "Missing field: http_method".to_string())?
                .try_into()?,
            path_suffix: value
                .path_suffix
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            header_vars: value
                .header_vars
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            query_vars: value
                .query_vars
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            auth_details: value.auth_details.map(TryInto::try_into).transpose()?,
            cors_options: value
                .cors_options
                .ok_or_else(|| "Missing field: cors_options".to_string())?
                .try_into()?,
            durable_streams: value.durable_streams.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<HttpEndpointDetails> for golem_api_grpc::proto::golem::component::HttpEndpointDetails {
    fn from(value: HttpEndpointDetails) -> Self {
        Self {
            http_method: Some(value.http_method.into()),
            path_suffix: value.path_suffix.into_iter().map(Into::into).collect(),
            header_vars: value.header_vars.into_iter().map(Into::into).collect(),
            query_vars: value.query_vars.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            cors_options: Some(value.cors_options.into()),
            durable_streams: value.durable_streams.map(Into::into),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::DurableStreamRouteOptions>
    for DurableStreamRouteOptions
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::DurableStreamRouteOptions,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            slots: value
                .slots
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            allow_external_writes: value.allow_external_writes,
            allow_stream_delete: value.allow_stream_delete,
            allow_invocation_delete: value.allow_invocation_delete,
            load: value.load.map(Into::into),
        })
    }
}

impl From<DurableStreamRouteOptions>
    for golem_api_grpc::proto::golem::component::DurableStreamRouteOptions
{
    fn from(value: DurableStreamRouteOptions) -> Self {
        Self {
            slots: value.slots.into_iter().map(Into::into).collect(),
            allow_external_writes: value.allow_external_writes,
            allow_stream_delete: value.allow_stream_delete,
            allow_invocation_delete: value.allow_invocation_delete,
            load: value.load.map(Into::into),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::DurableStreamSlotOptions>
    for DurableStreamSlotOptions
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::DurableStreamSlotOptions,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            source: value
                .source
                .ok_or_else(|| "Missing field: durable_stream_slot_options.source".to_string())?
                .try_into()?,
            name: value.name,
            content_type: value.content_type,
        })
    }
}

impl From<DurableStreamSlotOptions>
    for golem_api_grpc::proto::golem::component::DurableStreamSlotOptions
{
    fn from(value: DurableStreamSlotOptions) -> Self {
        Self {
            source: Some(value.source.into()),
            name: value.name,
            content_type: value.content_type,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::DurableStreamSlotSource>
    for DurableStreamSlotSource
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::DurableStreamSlotSource,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::durable_stream_slot_source::Value;

        match value
            .value
            .ok_or_else(|| "Missing oneof: durable_stream_slot_source.value".to_string())?
        {
            Value::Input(name) => Ok(Self::Input(DurableStreamInputSlotSource { name })),
            Value::Output(name) => Ok(Self::Output(DurableStreamOutputSlotSource { name })),
        }
    }
}

impl From<DurableStreamSlotSource>
    for golem_api_grpc::proto::golem::component::DurableStreamSlotSource
{
    fn from(value: DurableStreamSlotSource) -> Self {
        use golem_api_grpc::proto::golem::component::durable_stream_slot_source::Value;

        Self {
            value: Some(match value {
                DurableStreamSlotSource::Input(source) => Value::Input(source.name),
                DurableStreamSlotSource::Output(source) => Value::Output(source.name),
            }),
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::DurableStreamRouteLoadOptions>
    for DurableStreamRouteLoadOptions
{
    fn from(value: golem_api_grpc::proto::golem::component::DurableStreamRouteLoadOptions) -> Self {
        Self {
            max_concurrent_readers_per_stream: value.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: value
                .max_append_requests_per_second_per_stream,
        }
    }
}

impl From<DurableStreamRouteLoadOptions>
    for golem_api_grpc::proto::golem::component::DurableStreamRouteLoadOptions
{
    fn from(value: DurableStreamRouteLoadOptions) -> Self {
        Self {
            max_concurrent_readers_per_stream: value.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: value
                .max_append_requests_per_second_per_stream,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HttpMethod> for HttpMethod {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HttpMethod,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::StandardHttpMethod;
        use golem_api_grpc::proto::golem::component::http_method::Value;

        match value
            .value
            .ok_or_else(|| "Missing oneof: value".to_string())?
        {
            Value::Standard(inner) => {
                let typed =
                    golem_api_grpc::proto::golem::component::StandardHttpMethod::try_from(inner)
                        .unwrap_or_default();
                match typed {
                    StandardHttpMethod::Get => Ok(Self::Get(Empty {})),
                    StandardHttpMethod::Head => Ok(Self::Head(Empty {})),
                    StandardHttpMethod::Post => Ok(Self::Post(Empty {})),
                    StandardHttpMethod::Put => Ok(Self::Put(Empty {})),
                    StandardHttpMethod::Delete => Ok(Self::Delete(Empty {})),
                    StandardHttpMethod::Connect => Ok(Self::Connect(Empty {})),
                    StandardHttpMethod::Options => Ok(Self::Options(Empty {})),
                    StandardHttpMethod::Trace => Ok(Self::Trace(Empty {})),
                    StandardHttpMethod::Patch => Ok(Self::Patch(Empty {})),
                    StandardHttpMethod::Unspecified => {
                        Err("Unknown http method variant".to_string())
                    }
                }
            }
            Value::Custom(c) => Ok(HttpMethod::Custom(CustomHttpMethod { value: c })),
            Value::Any(_) => Ok(HttpMethod::Any(Empty {})),
        }
    }
}

impl From<HttpMethod> for golem_api_grpc::proto::golem::component::HttpMethod {
    fn from(value: HttpMethod) -> Self {
        use golem_api_grpc::proto::golem::component::StandardHttpMethod;
        use golem_api_grpc::proto::golem::component::http_method::Value;

        Self {
            value: Some(match value {
                HttpMethod::Get(_) => Value::Standard(StandardHttpMethod::Get.into()),
                HttpMethod::Head(_) => Value::Standard(StandardHttpMethod::Head.into()),
                HttpMethod::Post(_) => Value::Standard(StandardHttpMethod::Post.into()),
                HttpMethod::Put(_) => Value::Standard(StandardHttpMethod::Put.into()),
                HttpMethod::Delete(_) => Value::Standard(StandardHttpMethod::Delete.into()),
                HttpMethod::Connect(_) => Value::Standard(StandardHttpMethod::Connect.into()),
                HttpMethod::Options(_) => Value::Standard(StandardHttpMethod::Options.into()),
                HttpMethod::Trace(_) => Value::Standard(StandardHttpMethod::Trace.into()),
                HttpMethod::Patch(_) => Value::Standard(StandardHttpMethod::Patch.into()),
                HttpMethod::Custom(c) => Value::Custom(c.value),
                HttpMethod::Any(_) => Value::Any(golem_schema::proto::golem::common::Empty {}),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::CorsOptions> for CorsOptions {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::CorsOptions,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            allowed_patterns: value.allowed_patterns,
        })
    }
}

impl From<CorsOptions> for golem_api_grpc::proto::golem::component::CorsOptions {
    fn from(value: CorsOptions) -> Self {
        Self {
            allowed_patterns: value.allowed_patterns,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PathSegment> for PathSegment {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PathSegment,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::path_segment::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::Literal(v) => Ok(Self::Literal(v.try_into()?)),
            Value::SystemVariable(v) => Ok(Self::SystemVariable(v.try_into()?)),
            Value::PathVariable(v) => Ok(Self::PathVariable(v.try_into()?)),
            Value::RemainingPathVariable(v) => Ok(Self::RemainingPathVariable(v.try_into()?)),
        }
    }
}

impl From<PathSegment> for golem_api_grpc::proto::golem::component::PathSegment {
    fn from(value: PathSegment) -> Self {
        use golem_api_grpc::proto::golem::component::path_segment::Value;

        Self {
            value: Some(match value {
                PathSegment::Literal(v) => Value::Literal(v.into()),
                PathSegment::SystemVariable(v) => Value::SystemVariable(v.into()),
                PathSegment::PathVariable(v) => Value::PathVariable(v.into()),
                PathSegment::RemainingPathVariable(v) => Value::RemainingPathVariable(v.into()),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::LiteralSegment> for LiteralSegment {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::LiteralSegment,
    ) -> Result<Self, Self::Error> {
        Ok(Self { value: value.value })
    }
}

impl From<LiteralSegment> for golem_api_grpc::proto::golem::component::LiteralSegment {
    fn from(value: LiteralSegment) -> Self {
        Self { value: value.value }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::SystemVariableSegment>
    for SystemVariableSegment
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::SystemVariableSegment,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            value: value.value().try_into()?,
        })
    }
}

impl From<SystemVariableSegment>
    for golem_api_grpc::proto::golem::component::SystemVariableSegment
{
    fn from(value: SystemVariableSegment) -> Self {
        Self {
            value: golem_api_grpc::proto::golem::component::SystemVariable::from(value.value)
                .into(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::SystemVariable> for SystemVariable {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::SystemVariable,
    ) -> Result<Self, Self::Error> {
        match value {
            golem_api_grpc::proto::golem::component::SystemVariable::AgentType => {
                Ok(Self::AgentType)
            }
            golem_api_grpc::proto::golem::component::SystemVariable::AgentVersion => {
                Ok(Self::AgentVersion)
            }
            golem_api_grpc::proto::golem::component::SystemVariable::Unspecified => {
                Err("Unknown SystemVariable variant".to_string())
            }
        }
    }
}

impl From<SystemVariable> for golem_api_grpc::proto::golem::component::SystemVariable {
    fn from(value: SystemVariable) -> Self {
        match value {
            SystemVariable::AgentType => Self::AgentType,
            SystemVariable::AgentVersion => Self::AgentVersion,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PathVariable> for PathVariable {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PathVariable,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            variable_name: value.variable_name,
        })
    }
}

impl From<PathVariable> for golem_api_grpc::proto::golem::component::PathVariable {
    fn from(value: PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HeaderVariable> for HeaderVariable {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HeaderVariable,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        })
    }
}

impl From<HeaderVariable> for golem_api_grpc::proto::golem::component::HeaderVariable {
    fn from(value: HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::QueryVariable> for QueryVariable {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::QueryVariable,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        })
    }
}

impl From<QueryVariable> for golem_api_grpc::proto::golem::component::QueryVariable {
    fn from(value: QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentHttpAuthDetails>
    for AgentHttpAuthDetails
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentHttpAuthDetails,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            required: value.required,
        })
    }
}

impl From<AgentHttpAuthDetails> for golem_api_grpc::proto::golem::component::AgentHttpAuthDetails {
    fn from(value: AgentHttpAuthDetails) -> Self {
        Self {
            required: value.required,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Principal> for Principal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Principal,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::principal::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::Oidc(v) => Ok(Self::Oidc(v.try_into()?)),
            Value::Agent(v) => Ok(Self::Agent(v.try_into()?)),
            Value::GolemUser(v) => Ok(Self::GolemUser(v.try_into()?)),
            Value::Anonymous(_) => Ok(Self::Anonymous(Empty {})),
        }
    }
}

impl From<Principal> for golem_api_grpc::proto::golem::component::Principal {
    fn from(value: Principal) -> Self {
        use golem_api_grpc::proto::golem::component::principal::Value;

        Self {
            value: Some(match value {
                Principal::Oidc(v) => Value::Oidc(v.into()),
                Principal::Agent(v) => Value::Agent(v.into()),
                Principal::GolemUser(v) => Value::GolemUser(v.into()),
                Principal::Anonymous(_) => {
                    Value::Anonymous(golem_schema::proto::golem::common::Empty {})
                }
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::OidcPrincipal> for OidcPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::OidcPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
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
        })
    }
}

impl From<OidcPrincipal> for golem_api_grpc::proto::golem::component::OidcPrincipal {
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

impl TryFrom<golem_api_grpc::proto::golem::component::AgentPrincipal> for AgentPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_id: value
                .agent_id
                .ok_or_else(|| "Missing field: agent_id".to_string())?
                .try_into()?,
        })
    }
}

impl From<AgentPrincipal> for golem_api_grpc::proto::golem::component::AgentPrincipal {
    fn from(value: AgentPrincipal) -> Self {
        Self {
            agent_id: Some(value.agent_id.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::GolemUserPrincipal> for GolemUserPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::GolemUserPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            account_id: value
                .account_id
                .ok_or_else(|| "Missing field: account_id".to_string())?
                .try_into()?,
        })
    }
}

impl From<GolemUserPrincipal> for golem_api_grpc::proto::golem::component::GolemUserPrincipal {
    fn from(value: GolemUserPrincipal) -> Self {
        Self {
            account_id: Some(value.account_id.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Snapshotting> for Snapshotting {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Snapshotting,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::snapshotting::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::Disabled(_) => Ok(Self::Disabled(Empty {})),
            Value::Enabled(config) => Ok(Self::Enabled(config.try_into()?)),
        }
    }
}

impl From<Snapshotting> for golem_api_grpc::proto::golem::component::Snapshotting {
    fn from(value: Snapshotting) -> Self {
        use golem_api_grpc::proto::golem::component::snapshotting::Value;

        Self {
            value: Some(match value {
                Snapshotting::Disabled(_) => {
                    Value::Disabled(golem_schema::proto::golem::common::Empty {})
                }
                Snapshotting::Enabled(config) => Value::Enabled(config.into()),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::SnapshottingConfig> for SnapshottingConfig {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::SnapshottingConfig,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::snapshotting_config::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::Default(_) => Ok(Self::Default(Empty {})),
            Value::PeriodicNanos(nanos) => Ok(Self::Periodic(SnapshottingPeriodic {
                duration_nanos: nanos,
            })),
            Value::EveryNInvocation(n) => {
                Ok(Self::EveryNInvocation(SnapshottingEveryNInvocation {
                    count: n as u16,
                }))
            }
        }
    }
}

impl From<SnapshottingConfig> for golem_api_grpc::proto::golem::component::SnapshottingConfig {
    fn from(value: SnapshottingConfig) -> Self {
        use golem_api_grpc::proto::golem::component::snapshotting_config::Value;

        Self {
            value: Some(match value {
                SnapshottingConfig::Default(_) => {
                    Value::Default(golem_schema::proto::golem::common::Empty {})
                }
                SnapshottingConfig::Periodic(periodic) => {
                    Value::PeriodicNanos(periodic.duration_nanos)
                }
                SnapshottingConfig::EveryNInvocation(every_n) => {
                    Value::EveryNInvocation(every_n.count as u32)
                }
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentConfigSource> for AgentConfigSource {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentConfigSource,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::AgentConfigSource as GrpcAgentConfigSource;
        match value {
            GrpcAgentConfigSource::Local => Ok(Self::Local),
            GrpcAgentConfigSource::Secret => Ok(Self::Secret),
            GrpcAgentConfigSource::Unspecified => Err("unknown agent config source".to_string()),
        }
    }
}

impl From<AgentConfigSource> for golem_api_grpc::proto::golem::component::AgentConfigSource {
    fn from(value: AgentConfigSource) -> Self {
        match value {
            AgentConfigSource::Local => Self::Local,
            AgentConfigSource::Secret => Self::Secret,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::AgentMetadata> for AgentMetadataDto {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AgentMetadata,
    ) -> Result<Self, Self::Error> {
        let mut exported_resource_instances = Vec::new();

        for desc in value.owned_resources {
            exported_resource_instances.push(ExportedResourceMetadata {
                key: AgentResourceId(desc.resource_id),
                description: AgentResourceDescription {
                    created_at: desc.created_at.ok_or("Missing created_at")?.into(),
                    resource_owner: desc.resource_owner,
                    resource_name: desc.resource_name,
                },
            });
        }
        Ok(Self {
            agent_id: value.agent_id.ok_or("Missing agent_id")?.try_into()?,
            owner_kind: crate::model::agent::OwnerKind::try_from(value.owner_kind)?,
            environment_id: value
                .environment_id
                .ok_or("Missing environment_id")?
                .try_into()?,
            created_by: value.created_by.ok_or("Missing account_id")?.try_into()?,
            env: value.env,
            config: value
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
            status: value.status.try_into()?,
            component_revision: value.component_revision.try_into()?,
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value
                .updates
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
            created_at: value.created_at.ok_or("Missing created_at")?.into(),
            last_error: value.last_error,
            last_error_kind: value
                .last_error_kind
                .map(|kind| {
                    golem_api_grpc::proto::golem::worker::OplogErrorKind::try_from(kind)
                        .map_err(|_| format!("Invalid oplog error kind: {kind}"))
                        .map(|kind| match kind {
                            golem_api_grpc::proto::golem::worker::OplogErrorKind::Invocation => {
                                OplogErrorKind::Invocation
                            }
                            golem_api_grpc::proto::golem::worker::OplogErrorKind::Recovery => {
                                OplogErrorKind::Recovery
                            }
                        })
                })
                .transpose()?,
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            exported_resource_instances,
            active_plugins: value
                .active_plugins
                .into_iter()
                .map(EnvironmentPluginGrantId::try_from)
                .collect::<Result<HashSet<_, _>, _>>()?,
            skipped_regions: value
                .skipped_regions
                .into_iter()
                .map(OplogRegion::from)
                .collect::<Vec<_>>(),
            deleted_regions: value
                .deleted_regions
                .into_iter()
                .map(OplogRegion::from)
                .collect::<Vec<_>>(),
            last_oplog_index: OplogIndex::from_u64(value.oplog_idx),
            fingerprint: value
                .fingerprint
                .map(|uuid| AgentFingerprint(uuid.into()))
                .unwrap_or_default(),
        })
    }
}

impl TryFrom<AgentMetadataDto> for golem_api_grpc::proto::golem::worker::AgentMetadata {
    type Error = String;

    fn try_from(value: AgentMetadataDto) -> Result<Self, Self::Error> {
        let mut owned_resources = Vec::new();
        for instance in value.exported_resource_instances {
            owned_resources.push(golem_api_grpc::proto::golem::worker::ResourceDescription {
                resource_id: instance.key.0,
                resource_name: instance.description.resource_name,
                resource_owner: instance.description.resource_owner,
                created_at: Some(instance.description.created_at.into()),
            });
        }

        Ok(Self {
            agent_id: Some(value.agent_id.into()),
            owner_kind: value.owner_kind.into(),
            environment_id: Some(value.environment_id.into()),
            created_by: Some(value.created_by.into()),
            env: value.env,
            config: value
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            status: value.status.into(),
            component_revision: value.component_revision.into(),
            retry_count: value.retry_count,
            pending_invocation_count: value.pending_invocation_count,
            updates: value.updates.into_iter().map(Into::into).collect(),
            created_at: Some(value.created_at.into()),
            last_error: value.last_error,
            last_error_kind: value.last_error_kind.map(|kind| match kind {
                OplogErrorKind::Invocation => {
                    golem_api_grpc::proto::golem::worker::OplogErrorKind::Invocation as i32
                }
                OplogErrorKind::Recovery => {
                    golem_api_grpc::proto::golem::worker::OplogErrorKind::Recovery as i32
                }
            }),
            component_size: value.component_size,
            total_linear_memory_size: value.total_linear_memory_size,
            owned_resources,
            active_plugins: value
                .active_plugins
                .into_iter()
                .map(|id| id.into())
                .collect(),
            skipped_regions: value
                .skipped_regions
                .into_iter()
                .map(|region| region.into())
                .collect(),
            deleted_regions: value
                .deleted_regions
                .into_iter()
                .map(|region| region.into())
                .collect(),
            oplog_idx: u64::from(value.last_oplog_index),
            fingerprint: Some(value.fingerprint.0.into()),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::UpdateRecord> for UpdateRecord {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::UpdateRecord,
    ) -> Result<Self, Self::Error> {
        let mode = value.mode().into();
        let timestamp = value.timestamp.ok_or("Missing timestamp")?.into();
        let target_revision = value.target_revision.try_into()?;
        let pending_update_index = value.pending_update_index.map(OplogIndex::from_u64);
        let snapshot_assisted_details = value
            .snapshot_assisted_details
            .map(TryInto::try_into)
            .transpose()?;
        match value.update.ok_or("Missing update field")? {
            golem_api_grpc::proto::golem::worker::update_record::Update::Failed(failed) => {
                Ok(Self::FailedUpdate(FailedUpdate {
                    timestamp,
                    target_revision,
                    details: { failed.details },
                    pending_update_index,
                    mode,
                    snapshot_assisted_details,
                }))
            }
            golem_api_grpc::proto::golem::worker::update_record::Update::Pending(_) => {
                Ok(Self::PendingUpdate(PendingUpdate {
                    timestamp,
                    target_revision,
                    pending_update_index,
                    mode,
                    snapshot_assisted_details,
                }))
            }
            golem_api_grpc::proto::golem::worker::update_record::Update::Successful(_) => {
                Ok(Self::SuccessfulUpdate(SuccessfulUpdate {
                    timestamp,
                    target_revision,
                    pending_update_index,
                    mode,
                    snapshot_assisted_details,
                }))
            }
        }
    }
}

impl From<UpdateRecord> for golem_api_grpc::proto::golem::worker::UpdateRecord {
    fn from(value: UpdateRecord) -> Self {
        match value {
            UpdateRecord::FailedUpdate(FailedUpdate {
                timestamp,
                target_revision,
                details,
                pending_update_index,
                mode,
                snapshot_assisted_details,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_revision: target_revision.into(),
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Failed(
                        golem_api_grpc::proto::golem::worker::FailedUpdate { details },
                    ),
                ),
                pending_update_index: pending_update_index.map(Into::into),
                mode: golem_api_grpc::proto::golem::worker::UpdateMode::from(mode) as i32,
                snapshot_assisted_details: snapshot_assisted_details.map(Into::into),
            },
            UpdateRecord::PendingUpdate(PendingUpdate {
                timestamp,
                target_revision,
                pending_update_index,
                mode,
                snapshot_assisted_details,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_revision: target_revision.into(),
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Pending(
                        golem_api_grpc::proto::golem::worker::PendingUpdate {},
                    ),
                ),
                pending_update_index: pending_update_index.map(Into::into),
                mode: golem_api_grpc::proto::golem::worker::UpdateMode::from(mode) as i32,
                snapshot_assisted_details: snapshot_assisted_details.map(Into::into),
            },
            UpdateRecord::SuccessfulUpdate(SuccessfulUpdate {
                timestamp,
                target_revision,
                pending_update_index,
                mode,
                snapshot_assisted_details,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_revision: target_revision.into(),
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Successful(
                        golem_api_grpc::proto::golem::worker::SuccessfulUpdate {},
                    ),
                ),
                pending_update_index: pending_update_index.map(Into::into),
                mode: golem_api_grpc::proto::golem::worker::UpdateMode::from(mode) as i32,
                snapshot_assisted_details: snapshot_assisted_details.map(Into::into),
            },
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::SnapshotAssistedUpdateMetadata>
    for SnapshotAssistedUpdateMetadata
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::SnapshotAssistedUpdateMetadata,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            source_component_revision: value.source_component_revision.try_into()?,
            source_revision_start_index: OplogIndex::from_u64(value.source_revision_start_index),
            snapshot_index: OplogIndex::from_u64(value.snapshot_index),
            snapshot_revision: value.snapshot_revision.try_into()?,
            filesystem_snapshot: value.filesystem_snapshot,
        })
    }
}

impl From<SnapshotAssistedUpdateMetadata>
    for golem_api_grpc::proto::golem::worker::SnapshotAssistedUpdateMetadata
{
    fn from(value: SnapshotAssistedUpdateMetadata) -> Self {
        Self {
            source_component_revision: value.source_component_revision.into(),
            source_revision_start_index: value.source_revision_start_index.into(),
            snapshot_index: value.snapshot_index.into(),
            snapshot_revision: value.snapshot_revision.into(),
            filesystem_snapshot: value.filesystem_snapshot,
        }
    }
}

impl From<golem_api_grpc::proto::golem::common::RevertToOplogIndex> for RevertToOplogIndex {
    fn from(value: golem_api_grpc::proto::golem::common::RevertToOplogIndex) -> Self {
        Self {
            last_oplog_index: OplogIndex::from_u64(value.last_oplog_index as u64),
        }
    }
}

impl From<RevertToOplogIndex> for golem_api_grpc::proto::golem::common::RevertToOplogIndex {
    fn from(value: RevertToOplogIndex) -> Self {
        Self {
            last_oplog_index: u64::from(value.last_oplog_index) as i64,
        }
    }
}

impl From<golem_api_grpc::proto::golem::common::RevertLastInvocations> for RevertLastInvocations {
    fn from(value: golem_api_grpc::proto::golem::common::RevertLastInvocations) -> Self {
        Self {
            number_of_invocations: value.number_of_invocations as u64,
        }
    }
}

impl From<RevertLastInvocations> for golem_api_grpc::proto::golem::common::RevertLastInvocations {
    fn from(value: RevertLastInvocations) -> Self {
        Self {
            number_of_invocations: value.number_of_invocations as i64,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::common::RevertWorkerTarget> for RevertWorkerTarget {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::common::RevertWorkerTarget,
    ) -> Result<Self, Self::Error> {
        match value.target {
            Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertToOplogIndex(target)) => {
                Ok(RevertWorkerTarget::RevertToOplogIndex(target.into()))
            }
            Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertLastInvocations(target)) => {
                Ok(RevertWorkerTarget::RevertLastInvocations(target.into()))
            }
            None => Err("Missing field: target".to_string()),
        }
    }
}

impl From<RevertWorkerTarget> for golem_api_grpc::proto::golem::common::RevertWorkerTarget {
    fn from(value: RevertWorkerTarget) -> Self {
        match value {
            RevertWorkerTarget::RevertToOplogIndex(target) => Self {
                target: Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertToOplogIndex(target.into())),
            },
            RevertWorkerTarget::RevertLastInvocations(target) => Self {
                target: Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertLastInvocations(target.into())),
            },
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::UpdateMode> for AgentUpdateMode {
    fn from(value: golem_api_grpc::proto::golem::worker::UpdateMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::UpdateMode::Automatic => {
                AgentUpdateMode::Automatic
            }
            golem_api_grpc::proto::golem::worker::UpdateMode::Manual => AgentUpdateMode::Manual,
        }
    }
}

impl From<AgentUpdateMode> for golem_api_grpc::proto::golem::worker::UpdateMode {
    fn from(value: AgentUpdateMode) -> Self {
        match value {
            AgentUpdateMode::Automatic => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
            }
            AgentUpdateMode::Manual => golem_api_grpc::proto::golem::worker::UpdateMode::Manual,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::AgentConfigEntryDto> for AgentConfigEntryDto {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AgentConfigEntryDto,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            path: value.path,
            value: serde_json::from_str::<serde_json::Value>(&value.value)
                .map_err(|e| e.to_string())?
                .into(),
        })
    }
}

impl From<AgentConfigEntryDto> for golem_api_grpc::proto::golem::worker::AgentConfigEntryDto {
    fn from(value: AgentConfigEntryDto) -> Self {
        Self {
            path: value.path,
            value: value.value.to_string(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::TypedAgentConfigEntry>
    for TypedAgentConfigEntry
{
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::worker::TypedAgentConfigEntry,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            path: value.path,
            value: value
                .value
                .ok_or_else(|| "Missing field: value".to_string())?
                .try_into()?,
        })
    }
}

impl TryFrom<TypedAgentConfigEntry>
    for golem_api_grpc::proto::golem::worker::TypedAgentConfigEntry
{
    type Error = String;

    fn try_from(value: TypedAgentConfigEntry) -> Result<Self, Self::Error> {
        Ok(Self {
            path: value.path,
            value: Some(value.value.try_into()?),
        })
    }
}
