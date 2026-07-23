use super::{
    AgentConfigSource, AgentHttpAuthDetails, AgentInvocationMode, AgentMode, AgentPrincipal,
    CachePolicy, CachePolicyTtl, CorsOptions, CustomHttpMethod, GolemUserPrincipal, HeaderVariable,
    HttpEndpointDetails, HttpMethod, HttpMountDetails, LiteralSegment, OidcPrincipal, PathSegment,
    PathVariable, Principal, QueryVariable, ReadOnlyConfig, RegisteredAgentType,
    RegisteredAgentTypeImplementer, Snapshotting, SnapshottingConfig, SnapshottingEveryNInvocation,
    SnapshottingPeriodic, SystemVariable, SystemVariableSegment,
};
use crate::model::Empty;

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
                    Value::NoCache(golem_api_grpc::proto::golem::common::Empty {})
                }
                CachePolicy::UntilWrite(_) => {
                    Value::UntilWrite(golem_api_grpc::proto::golem::common::Empty {})
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
            cors_options: value
                .cors_options
                .ok_or_else(|| "Missing field: cors_options".to_string())?
                .try_into()?,
            webhook_suffix: value
                .webhook_suffix
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<HttpMountDetails> for golem_api_grpc::proto::golem::component::HttpMountDetails {
    fn from(value: HttpMountDetails) -> Self {
        Self {
            path_prefix: value.path_prefix.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            phantom_agent: value.phantom_agent,
            cors_options: Some(value.cors_options.into()),
            webhook_suffix: value.webhook_suffix.into_iter().map(Into::into).collect(),
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
                    Value::Anonymous(golem_api_grpc::proto::golem::common::Empty {})
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
                    Value::Disabled(golem_api_grpc::proto::golem::common::Empty {})
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
                    Value::Default(golem_api_grpc::proto::golem::common::Empty {})
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
