use super::AgentMetadataDto;
use super::{
    AgentConfigEntryDto, ExportedResourceMetadata, FailedUpdate, PendingUpdate,
    SnapshotAssistedUpdateMetadata, SuccessfulUpdate, TypedAgentConfigEntry, UpdateRecord,
};
use super::{
    AgentConfigSource, AgentHttpAuthDetails, AgentIdBinding, AgentIdBindingDetails,
    AgentInvocationMode, AgentMode, CachePolicy, CachePolicyTtl, CorsOptions, HttpEndpointDetails,
    HttpMethod, HttpMountDetails, PathSegment, ReadOnlyConfig, RegisteredAgentType,
    RegisteredAgentTypeImplementer, Snapshotting, SnapshottingConfig, SnapshottingEveryNInvocation,
    SnapshottingPeriodic, SystemVariable, SystemVariableSegment,
};
use super::{AgentUpdateMode, RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget};
use crate::base_model::AgentFingerprint;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::caller::{
    AccountAuthentication, AccountPrincipal, AgentPrincipal, Caller, ClaimVisibility, Claims, Pair,
    Principal, PrincipalRef, PublicAccountPrincipal, PublicAgentPrincipal, PublicCaller,
    PublicClaims, PublicPrincipal, PublicTokenProvenance, PublicToolPrincipal, PublicUserPrincipal,
    TokenProvenance, ToolPrincipal, UserPrincipal,
};
use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::base_model::tool::ToolName;
use crate::model::oplog::{AgentResourceId, OplogErrorKind, ReplayDivergence};
use crate::model::regions::OplogRegion;
use crate::model::{AgentResourceDescription, OplogIndex};
use golem_schema::Empty;
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
        })
    }
}

impl From<ReadOnlyConfig> for golem_api_grpc::proto::golem::component::ReadOnlyConfig {
    fn from(value: ReadOnlyConfig) -> Self {
        golem_api_grpc::proto::golem::component::ReadOnlyConfig {
            cache_policy: Some(value.cache_policy.into()),
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
            type_id: value.type_id.try_into()?,
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
            type_id: value.type_id.into(),
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
            agent_id_binding: value.agent_id_binding.map(TryInto::try_into).transpose()?,
            create_route: value
                .create_route
                .map(|route| {
                    route
                        .path
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<Vec<_>, _>>()
                })
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
            agent_id_binding: value.agent_id_binding.map(Into::into),
            create_route: value.create_route.map(|path| {
                golem_api_grpc::proto::golem::component::HttpCreateRoute {
                    path: path.into_iter().map(Into::into).collect(),
                }
            }),
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

impl TryFrom<golem_api_grpc::proto::golem::component::AgentIdBinding> for AgentIdBinding {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentIdBinding,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::agent_id_binding::Source;
        match value.source.ok_or("Missing AgentIdBinding.source")? {
            Source::Path(value) => Ok(Self::Path(AgentIdBindingDetails { name: value.name })),
            Source::Query(value) => Ok(Self::Query(AgentIdBindingDetails { name: value.name })),
        }
    }
}

impl From<AgentIdBinding> for golem_api_grpc::proto::golem::component::AgentIdBinding {
    fn from(value: AgentIdBinding) -> Self {
        use golem_api_grpc::proto::golem::component::{
            AgentIdBindingDetails, agent_id_binding::Source,
        };
        Self {
            source: Some(match value {
                AgentIdBinding::Path(value) => {
                    Source::Path(AgentIdBindingDetails { name: value.name })
                }
                AgentIdBinding::Query(value) => {
                    Source::Query(AgentIdBindingDetails { name: value.name })
                }
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
            header_vars: value.header_vars.into_iter().map(Into::into).collect(),
            query_vars: value.query_vars.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(TryInto::try_into).transpose()?,
            cors_options: value
                .cors_options
                .ok_or_else(|| "Missing field: cors_options".to_string())?
                .try_into()?,
            durable_streams: value.durable_streams.map(TryInto::try_into).transpose()?,
            mode: value.mode.map(TryInto::try_into).transpose()?,
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
            mode: value
                .mode
                .map(|mode| golem_schema::proto::golem::http::HttpRouteMode::from(mode) as i32),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HttpMethod> for HttpMethod {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HttpMethod,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::http_method::Value;

        match value
            .value
            .ok_or_else(|| "Missing oneof: value".to_string())?
        {
            Value::Method(method) => Ok(Self::Method(method.try_into()?)),
            Value::Any(_) => Ok(HttpMethod::Any(Empty {})),
        }
    }
}

impl From<HttpMethod> for golem_api_grpc::proto::golem::component::HttpMethod {
    fn from(value: HttpMethod) -> Self {
        use golem_api_grpc::proto::golem::component::http_method::Value;

        Self {
            value: Some(match value {
                HttpMethod::Method(method) => Value::Method(method.into()),
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
            Value::Segment(v) => Ok(Self::Segment(v.try_into()?)),
            Value::SystemVariable(v) => Ok(Self::SystemVariable(v.try_into()?)),
        }
    }
}

impl From<PathSegment> for golem_api_grpc::proto::golem::component::PathSegment {
    fn from(value: PathSegment) -> Self {
        use golem_api_grpc::proto::golem::component::path_segment::Value;

        Self {
            value: Some(match value {
                PathSegment::Segment(v) => Value::Segment(v.into()),
                PathSegment::SystemVariable(v) => Value::SystemVariable(v.into()),
            }),
        }
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

fn missing(field: &str) -> String {
    format!("Missing field: {field}")
}

impl From<Pair> for golem_api_grpc::proto::golem::component::Pair {
    fn from(value: Pair) -> Self {
        Self {
            issuer: value.issuer,
            subject: value.subject,
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::Pair> for Pair {
    fn from(value: golem_api_grpc::proto::golem::component::Pair) -> Self {
        Pair::new(value.issuer, value.subject)
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Principal> for Principal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Principal,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::principal::Value;

        match value.value.ok_or_else(|| missing("value"))? {
            Value::User(v) => Ok(Self::User(v.try_into()?)),
            Value::Account(v) => Ok(Self::Account(v.try_into()?)),
            Value::Agent(v) => Ok(Self::Agent(v.try_into()?)),
            Value::Tool(v) => Ok(Self::Tool(v.try_into()?)),
            Value::Anonymous(_) => Ok(Self::Anonymous),
        }
    }
}

impl From<Principal> for golem_api_grpc::proto::golem::component::Principal {
    fn from(value: Principal) -> Self {
        use golem_api_grpc::proto::golem::component::principal::Value;

        Self {
            value: Some(match value {
                Principal::User(v) => Value::User(v.into()),
                Principal::Account(v) => Value::Account(v.into()),
                Principal::Agent(v) => Value::Agent(v.into()),
                Principal::Tool(v) => Value::Tool(v.into()),
                Principal::Anonymous => {
                    Value::Anonymous(golem_schema::proto::golem::common::Empty {})
                }
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::UserPrincipal> for UserPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::UserPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            pair: value.pair.ok_or_else(|| missing("pair"))?.into(),
            claims: Claims::from_json(value.claims)
                .map_err(|error| format!("Invalid field: claims: {error}"))?,
            authenticated_at: authenticated_at_from_protobuf(
                value
                    .authenticated_at
                    .ok_or_else(|| missing("authenticated_at"))?,
            )?,
        })
    }
}

/// The last millisecond of year 9999: the latest instant that a `Timestamp` can add to the epoch
/// without overflowing the date.
const LAST_REPRESENTABLE_MILLISECOND: u64 = 253_402_300_799_999;

/// A checked, millisecond-precision conversion of the time a user authenticated at. A value out
/// of range is refused instead of panicking, and sub-millisecond precision is dropped, because the
/// oplog keeps whole milliseconds and the decoded caller must be the record that is recorded.
fn authenticated_at_from_protobuf(
    value: prost_types::Timestamp,
) -> Result<crate::base_model::Timestamp, String> {
    let invalid = || "Invalid field: authenticated_at".to_string();
    let seconds = u64::try_from(value.seconds).map_err(|_| invalid())?;
    let nanos = u32::try_from(value.nanos).map_err(|_| invalid())?;
    if nanos >= 1_000_000_000 {
        return Err(invalid());
    }
    let millis = seconds
        .checked_mul(1000)
        .and_then(|millis| millis.checked_add(u64::from(nanos / 1_000_000)))
        .filter(|millis| *millis <= LAST_REPRESENTABLE_MILLISECOND)
        .ok_or_else(invalid)?;
    Ok(crate::base_model::Timestamp::from(millis))
}

impl From<UserPrincipal> for golem_api_grpc::proto::golem::component::UserPrincipal {
    fn from(value: UserPrincipal) -> Self {
        Self {
            pair: Some(value.pair.into()),
            claims: value.claims.as_json().to_string(),
            authenticated_at: Some(value.authenticated_at.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AccountAuthentication>
    for AccountAuthentication
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AccountAuthentication,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::AccountAuthentication as Proto;

        match value {
            Proto::Unspecified => Err("Unspecified field: authentication".to_string()),
            Proto::ApiToken => Ok(Self::ApiToken),
            Proto::BootstrapToken => Ok(Self::BootstrapToken),
            Proto::SignIn => Ok(Self::SignIn),
        }
    }
}

impl From<AccountAuthentication>
    for golem_api_grpc::proto::golem::component::AccountAuthentication
{
    fn from(value: AccountAuthentication) -> Self {
        match value {
            AccountAuthentication::ApiToken => Self::ApiToken,
            AccountAuthentication::BootstrapToken => Self::BootstrapToken,
            AccountAuthentication::SignIn => Self::SignIn,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AccountPrincipal> for AccountPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AccountPrincipal,
    ) -> Result<Self, Self::Error> {
        let authentication =
            golem_api_grpc::proto::golem::component::AccountAuthentication::try_from(
                value.authentication,
            )
            .map_err(|_| "Invalid field: authentication".to_string())?
            .try_into()?;
        Ok(Self {
            issuer: value.issuer,
            account_id: value
                .account_id
                .ok_or_else(|| missing("account_id"))?
                .try_into()?,
            authentication,
        })
    }
}

impl From<AccountPrincipal> for golem_api_grpc::proto::golem::component::AccountPrincipal {
    fn from(value: AccountPrincipal) -> Self {
        Self {
            issuer: value.issuer,
            account_id: Some(value.account_id.into()),
            authentication: golem_api_grpc::proto::golem::component::AccountAuthentication::from(
                value.authentication,
            ) as i32,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentPrincipal> for AgentPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(AgentPrincipal::of_agent(
            value.issuer,
            value
                .agent_id
                .ok_or_else(|| missing("agent_id"))?
                .try_into()?,
            AgentFingerprint(
                value
                    .fingerprint
                    .ok_or_else(|| missing("fingerprint"))?
                    .into(),
            ),
            AgentTypeName(value.agent_type),
            value
                .environment_id
                .ok_or_else(|| missing("environment_id"))?
                .try_into()?,
        ))
    }
}

impl From<AgentPrincipal> for golem_api_grpc::proto::golem::component::AgentPrincipal {
    fn from(value: AgentPrincipal) -> Self {
        Self {
            issuer: value.issuer,
            agent_id: Some(value.address.into()),
            fingerprint: Some(value.fingerprint.0.into()),
            agent_type: value.agent_type.0,
            environment_id: Some(value.environment_id.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ToolPrincipal> for ToolPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ToolPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            issuer: value.issuer,
            environment_id: value
                .environment_id
                .ok_or_else(|| missing("environment_id"))?
                .try_into()?,
            tool_name: ToolName::try_from(value.tool_name)?,
        })
    }
}

impl From<ToolPrincipal> for golem_api_grpc::proto::golem::component::ToolPrincipal {
    fn from(value: ToolPrincipal) -> Self {
        Self {
            issuer: value.issuer,
            environment_id: Some(value.environment_id.into()),
            tool_name: value.tool_name.to_string(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::TokenProvenance> for TokenProvenance {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::TokenProvenance,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.ok_or_else(|| missing("token id"))?.into(),
            maker: value.maker.map(|pair| PrincipalRef::new(pair.into())),
        })
    }
}

impl From<TokenProvenance> for golem_api_grpc::proto::golem::component::TokenProvenance {
    fn from(value: TokenProvenance) -> Self {
        Self {
            id: Some(value.id.into()),
            maker: value.maker.map(|maker| maker.into_pair().into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Caller> for Caller {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Caller,
    ) -> Result<Self, Self::Error> {
        let original: Principal = value
            .original
            .ok_or_else(|| missing("original"))?
            .try_into()?;
        Ok(Self {
            // An absent immediate principal is the same as the original one.
            immediate: match value.immediate {
                Some(immediate) => immediate.try_into()?,
                None => original.clone(),
            },
            original,
            tokens: value
                .tokens
                .into_iter()
                .map(TokenProvenance::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<Caller> for golem_api_grpc::proto::golem::component::Caller {
    fn from(value: Caller) -> Self {
        Self {
            // The immediate principal is left out when it is the same as the original one.
            immediate: (!value.original.same_record(&value.immediate))
                .then(|| value.immediate.into()),
            original: Some(value.original.into()),
            tokens: value.tokens.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PublicCaller> for PublicCaller {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PublicCaller,
    ) -> Result<Self, Self::Error> {
        let original: PublicPrincipal = value
            .original
            .ok_or_else(|| missing("original"))?
            .try_into()?;
        Ok(Self {
            // An absent immediate principal is the same as the original one.
            immediate: match value.immediate {
                Some(immediate) => immediate.try_into()?,
                None => original.clone(),
            },
            original,
            tokens: value
                .tokens
                .into_iter()
                .map(PublicTokenProvenance::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<PublicCaller> for golem_api_grpc::proto::golem::component::PublicCaller {
    fn from(value: PublicCaller) -> Self {
        Self {
            // The immediate principal is left out when it is the same as the original one, so
            // that shown claims cross the wire once.
            immediate: (value.original != value.immediate).then(|| value.immediate.into()),
            original: Some(value.original.into()),
            tokens: value.tokens.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PublicPrincipal> for PublicPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PublicPrincipal,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::public_principal::Value;

        match value.value.ok_or_else(|| missing("value"))? {
            Value::User(v) => Ok(Self::User(v.try_into()?)),
            Value::Account(v) => Ok(Self::Account(v.try_into()?)),
            Value::Agent(v) => Ok(Self::Agent(v.try_into()?)),
            Value::Tool(v) => Ok(Self::Tool(v.try_into()?)),
            Value::Anonymous(_) => Ok(Self::Anonymous(Empty {})),
        }
    }
}

impl From<PublicPrincipal> for golem_api_grpc::proto::golem::component::PublicPrincipal {
    fn from(value: PublicPrincipal) -> Self {
        use golem_api_grpc::proto::golem::component::public_principal::Value;

        Self {
            value: Some(match value {
                PublicPrincipal::User(v) => Value::User(v.into()),
                PublicPrincipal::Account(v) => Value::Account(v.into()),
                PublicPrincipal::Agent(v) => Value::Agent(v.into()),
                PublicPrincipal::Tool(v) => Value::Tool(v.into()),
                PublicPrincipal::Anonymous(_) => {
                    Value::Anonymous(golem_schema::proto::golem::common::Empty {})
                }
            }),
        }
    }
}

impl From<ClaimVisibility> for golem_api_grpc::proto::golem::workerexecutor::v1::ClaimVisibility {
    fn from(value: ClaimVisibility) -> Self {
        match value {
            ClaimVisibility::Hidden => Self::Hidden,
            ClaimVisibility::Shown => Self::Shown,
        }
    }
}

impl From<golem_api_grpc::proto::golem::workerexecutor::v1::ClaimVisibility> for ClaimVisibility {
    fn from(value: golem_api_grpc::proto::golem::workerexecutor::v1::ClaimVisibility) -> Self {
        use golem_api_grpc::proto::golem::workerexecutor::v1::ClaimVisibility as Proto;
        match value {
            Proto::Hidden => Self::Hidden,
            Proto::Shown => Self::Shown,
        }
    }
}

impl ClaimVisibility {
    /// The value of the `claims` field of the executor's oplog read requests.
    pub fn to_protobuf(self) -> i32 {
        golem_api_grpc::proto::golem::workerexecutor::v1::ClaimVisibility::from(self).into()
    }

    /// Reads the `claims` field of an oplog read request. An unknown value is refused, so a
    /// request never gets a visibility that its sender did not ask for.
    pub fn from_protobuf(value: i32) -> Result<Self, String> {
        golem_api_grpc::proto::golem::workerexecutor::v1::ClaimVisibility::try_from(value)
            .map(Self::from)
            .map_err(|_| format!("Invalid field: claims: unknown value {value}"))
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PublicUserPrincipal> for PublicUserPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PublicUserPrincipal,
    ) -> Result<Self, Self::Error> {
        let claims = value
            .claims
            .map(|text| {
                // The text must be one valid claim set. The check does not parse numbers.
                Claims::from_json(text)
                    .map(|claims| PublicClaims::from(&claims))
                    .map_err(|error| format!("Invalid field: claims: {error}"))
            })
            .transpose()?;
        Ok(Self {
            pair: value.pair.ok_or_else(|| missing("pair"))?.into(),
            authenticated_at: authenticated_at_from_protobuf(
                value
                    .authenticated_at
                    .ok_or_else(|| missing("authenticated_at"))?,
            )?,
            claims,
        })
    }
}

impl From<PublicUserPrincipal> for golem_api_grpc::proto::golem::component::PublicUserPrincipal {
    fn from(value: PublicUserPrincipal) -> Self {
        Self {
            pair: Some(value.pair.into()),
            authenticated_at: Some(value.authenticated_at.into()),
            claims: value.claims.map(|claims| claims.as_json().to_string()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PublicAccountPrincipal>
    for PublicAccountPrincipal
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PublicAccountPrincipal,
    ) -> Result<Self, Self::Error> {
        let authentication =
            golem_api_grpc::proto::golem::component::AccountAuthentication::try_from(
                value.authentication,
            )
            .map_err(|_| "Invalid field: authentication".to_string())?
            .try_into()?;
        Ok(Self {
            pair: value.pair.ok_or_else(|| missing("pair"))?.into(),
            account_id: value
                .account_id
                .ok_or_else(|| missing("account_id"))?
                .try_into()?,
            authentication,
        })
    }
}

impl From<PublicAccountPrincipal>
    for golem_api_grpc::proto::golem::component::PublicAccountPrincipal
{
    fn from(value: PublicAccountPrincipal) -> Self {
        Self {
            pair: Some(value.pair.into()),
            account_id: Some(value.account_id.into()),
            authentication: golem_api_grpc::proto::golem::component::AccountAuthentication::from(
                value.authentication,
            ) as i32,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PublicAgentPrincipal>
    for PublicAgentPrincipal
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PublicAgentPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            pair: value.pair.ok_or_else(|| missing("pair"))?.into(),
            agent_id: value
                .agent_id
                .ok_or_else(|| missing("agent_id"))?
                .try_into()?,
            agent_type: value.agent_type,
            environment_id: value
                .environment_id
                .ok_or_else(|| missing("environment_id"))?
                .try_into()?,
        })
    }
}

impl From<PublicAgentPrincipal> for golem_api_grpc::proto::golem::component::PublicAgentPrincipal {
    fn from(value: PublicAgentPrincipal) -> Self {
        Self {
            pair: Some(value.pair.into()),
            agent_id: Some(value.agent_id.into()),
            agent_type: value.agent_type,
            environment_id: Some(value.environment_id.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PublicToolPrincipal> for PublicToolPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PublicToolPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            pair: value.pair.ok_or_else(|| missing("pair"))?.into(),
            environment_id: value
                .environment_id
                .ok_or_else(|| missing("environment_id"))?
                .try_into()?,
            tool_name: value.tool_name,
        })
    }
}

impl From<PublicToolPrincipal> for golem_api_grpc::proto::golem::component::PublicToolPrincipal {
    fn from(value: PublicToolPrincipal) -> Self {
        Self {
            pair: Some(value.pair.into()),
            environment_id: Some(value.environment_id.into()),
            tool_name: value.tool_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PublicTokenProvenance>
    for PublicTokenProvenance
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PublicTokenProvenance,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.ok_or_else(|| missing("token id"))?.into(),
            maker: value.maker.map(Into::into),
        })
    }
}

impl From<PublicTokenProvenance>
    for golem_api_grpc::proto::golem::component::PublicTokenProvenance
{
    fn from(value: PublicTokenProvenance) -> Self {
        Self {
            id: Some(value.id.into()),
            maker: value.maker.map(Into::into),
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
            agent_type_name: value
                .agent_type_name
                .map(crate::model::agent::AgentTypeName),
            canonical_key: value.canonical_key,
            origin: crate::base_model::agent::AgentOrigin::try_from(value.origin)?,
            constructor_parameters: value.constructor_parameters,
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
            underlying_status: value
                .underlying_status
                .map(crate::model::AgentStatus::try_from)
                .transpose()?,
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
            agent_type_name: value.agent_type_name.map(|name| name.0),
            canonical_key: value.canonical_key,
            origin: value.origin.into(),
            constructor_parameters: value.constructor_parameters,
            component_id: None,
            environment_id: Some(value.environment_id.into()),
            created_by: Some(value.created_by.into()),
            env: value.env,
            config: value
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            status: value.status.into(),
            underlying_status: value.underlying_status.map(Into::into),
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
            state_oplog_idx: 0,
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
                    details: failed.details,
                    pending_update_index,
                    mode,
                    snapshot_assisted_details,
                    failure_class: crate::model::oplog::UpdateFailureClass::from_proto_number(
                        failed.failure_class,
                    )?,
                    divergence: failed
                        .divergence
                        .map(|divergence| ReplayDivergence::try_from(divergence).map(Box::new))
                        .transpose()?,
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
                failure_class,
                divergence,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_revision: target_revision.into(),
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Failed(
                        golem_api_grpc::proto::golem::worker::FailedUpdate {
                            details,
                            failure_class: failure_class.proto_number(),
                            divergence: divergence
                                .map(|divergence| (*divergence).into_proto_keeping_structure()),
                        },
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

impl TryFrom<golem_api_grpc::proto::golem::worker::AgentKey> for super::ParsedAgentKey {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AgentKey,
    ) -> Result<Self, Self::Error> {
        let parameters = value
            .parameters
            .ok_or_else(|| "Missing field: AgentKey.parameters".to_string())?
            .try_into()?;
        Self::try_new(super::AgentTypeName(value.agent_type_name), parameters)
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::AgentKey> for super::AgentKey {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AgentKey,
    ) -> Result<Self, Self::Error> {
        let constructor = super::ParsedAgentKey::try_from(value)?;
        Self::new(constructor.agent_type, constructor.parameters).map_err(|err| err.to_string())
    }
}

impl TryFrom<super::ParsedAgentKey> for golem_api_grpc::proto::golem::worker::AgentKey {
    type Error = String;

    fn try_from(value: super::ParsedAgentKey) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_type_name: value.agent_type.0,
            parameters: Some(value.parameters.try_into()?),
        })
    }
}

impl TryFrom<super::AgentKey> for golem_api_grpc::proto::golem::worker::AgentKey {
    type Error = String;

    fn try_from(value: super::AgentKey) -> Result<Self, Self::Error> {
        let (agent_type, parameters) = value.into_parts();
        Ok(Self {
            agent_type_name: agent_type.0,
            parameters: Some(parameters.try_into()?),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::AgentTarget> for super::AgentTarget {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AgentTarget,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::agent_target::Target;

        match value
            .target
            .ok_or_else(|| "Missing field: AgentTarget.target".to_string())?
        {
            Target::Key(key) => Ok(Self::Key(key.try_into()?)),
            Target::Id(id) => Ok(Self::Id(id.try_into()?)),
        }
    }
}

impl TryFrom<super::AgentTarget> for golem_api_grpc::proto::golem::worker::AgentTarget {
    type Error = String;

    fn try_from(value: super::AgentTarget) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::agent_target::Target;

        let target = match value {
            super::AgentTarget::Key(key) => Target::Key(key.try_into()?),
            super::AgentTarget::Id(id) => Target::Id(id.into()),
        };
        Ok(Self {
            target: Some(target),
        })
    }
}
