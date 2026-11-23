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

use super::{
    CompiledRoute, CompiledRoutes, OpenApiSpecBehaviour, OpenApiSpecFormat, RouteSecurity,
};
use super::{CorsOptions, SecuritySchemeDetails};
use super::{PathSegment, PathSegmentType, RequestBodySchema, RouteBehaviour};
use crate::custom_api::{
    AgentSelection, CallAgentBehaviour, CompiledInputSchema, CompiledOutputSchema, CompiledSchema,
    ConstructorParameter, CorsPreflightBehaviour, CorsPreflightMethodPolicy, CreateAgentBehaviour,
    DurableStreamRepresentation, DurableStreamRouteLoadPolicy, DurableStreamRoutePolicy,
    DurableStreamSlot, DurableStreamSlotDirection, MethodParameter, MountAgent, MountAgentMethod,
    MountBehaviour, MountFileSource, MountTarget, OriginPattern, QueryOrHeaderType, RouteMatch,
    RouterFileIndexEntry, RouterMethod, SecuritySchemeRouteSecurity,
    SessionFromHeaderRouteSecurity, WebhookCallbackBehaviour,
};
use golem_api_grpc::proto;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::http_api_deployment::HttpApiDeploymentScheme;
use golem_common::model::security_scheme::{
    Provider, SecuritySchemeLogin, SecuritySchemeName, SecuritySchemeRevision,
};
use http::HeaderName;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::collections::HashMap;
use std::ops::Deref;

fn normalize_header_name(header_name: String) -> String {
    HeaderName::from_bytes(header_name.trim().as_bytes())
        .map(|header_name| header_name.as_str().to_string())
        .unwrap_or_else(|_| header_name.trim().to_ascii_lowercase())
}

impl TryFrom<proto::golem::customapi::SecuritySchemeDetails> for SecuritySchemeDetails {
    type Error = String;

    fn try_from(
        value: proto::golem::customapi::SecuritySchemeDetails,
    ) -> Result<Self, Self::Error> {
        let id = value.id.ok_or("id field missing")?.try_into()?;

        let provider_type: Provider = value
            .provider
            .ok_or("provider field missing")?
            .try_into()
            .map_err(|e: String| format!("invalid provider: {e}"))?;

        Ok(Self {
            id,
            revision: SecuritySchemeRevision::new(value.revision)
                .map_err(|e| format!("invalid security scheme revision: {e}"))?,
            name: SecuritySchemeName(value.name),
            provider_type,
            client_id: ClientId::new(value.client_id),
            client_secret: ClientSecret::new(value.client_secret),
            redirect_url: RedirectUrl::new(value.redirect_url)
                .map_err(|e| format!("Failed parsing redirect url: {e}"))?,
            scopes: value.scopes.into_iter().map(Scope::new).collect(),
            login: serde_json::from_str::<SecuritySchemeLogin>(&value.login_config)
                .map_err(|e| format!("invalid login config: {e}"))?,
        })
    }
}

impl From<SecuritySchemeDetails>
    for golem_api_grpc::proto::golem::customapi::SecuritySchemeDetails
{
    fn from(value: SecuritySchemeDetails) -> Self {
        Self {
            id: Some(value.id.into()),
            revision: value.revision.get(),
            name: value.name.0,
            provider: Some(value.provider_type.into()),
            client_id: value.client_id.deref().clone(),
            client_secret: value.client_secret.secret().clone(),
            redirect_url: value.redirect_url.deref().clone(),
            scopes: value.scopes.iter().map(|s| s.deref().clone()).collect(),
            login_config: serde_json::to_string(&value.login)
                .expect("security scheme login must serialize"),
        }
    }
}

impl TryFrom<proto::golem::customapi::CompiledRoutes> for CompiledRoutes {
    type Error = String;

    fn try_from(value: proto::golem::customapi::CompiledRoutes) -> Result<Self, Self::Error> {
        let account_id = value.account_id.ok_or("Missing account_id")?.try_into()?;
        let environment_id = value
            .environment_id
            .ok_or("Missing environment_id")?
            .try_into()?;

        let mut security_schemes = HashMap::new();
        for scheme in value.security_schemes {
            let scheme: SecuritySchemeDetails = scheme.try_into()?;
            security_schemes.insert(scheme.id, scheme);
        }

        let routes = value
            .routes
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            account_id,
            account_email: AccountEmail::new(value.account_email),
            environment_id,
            deployment_revision: value.deployment_revision.try_into()?,
            security_schemes,
            routes,
        })
    }
}

impl From<CompiledRoutes> for proto::golem::customapi::CompiledRoutes {
    fn from(value: CompiledRoutes) -> Self {
        Self {
            account_id: Some(value.account_id.into()),
            account_email: value.account_email.into_inner(),
            environment_id: Some(value.environment_id.into()),
            deployment_revision: value.deployment_revision.into(),
            security_schemes: value
                .security_schemes
                .into_values()
                .map(Into::into)
                .collect(),
            routes: value.routes.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::golem::customapi::CompiledRoute> for CompiledRoute {
    type Error = String;

    fn try_from(value: proto::golem::customapi::CompiledRoute) -> Result<Self, Self::Error> {
        let route_match: RouteMatch = value.route_match.ok_or("Missing route_match")?.try_into()?;
        let behavior: RouteBehaviour = value.behavior.ok_or("Missing behavior")?.try_into()?;
        let path = value
            .path
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        route_match.validate(&path, &behavior)?;
        Ok(Self {
            route_id: value.route_id,
            route_match,
            path,
            behavior,
            security: value.security.ok_or("Missing security")?.try_into()?,
            cors: value.cors.ok_or("Missing cors")?.try_into()?,
        })
    }
}

impl From<CompiledRoute> for proto::golem::customapi::CompiledRoute {
    fn from(value: CompiledRoute) -> Self {
        Self {
            route_id: value.route_id,
            route_match: Some(value.route_match.into()),
            path: value.path.into_iter().map(Into::into).collect(),
            behavior: Some(value.behavior.into()),
            security: Some(value.security.into()),
            cors: Some(value.cors.into()),
        }
    }
}

impl TryFrom<proto::golem::customapi::RouteMatch> for RouteMatch {
    type Error = String;

    fn try_from(value: proto::golem::customapi::RouteMatch) -> Result<Self, Self::Error> {
        use proto::golem::customapi::route_match::Kind;
        match value.kind.ok_or("RouteMatch.kind missing")? {
            Kind::Method(value) => {
                let method = value.method.ok_or("RouteMatch.Method.method missing")?;
                let method = golem_common::model::agent::HttpMethod::try_from(method)?;
                if matches!(method, golem_common::model::agent::HttpMethod::Any(_)) {
                    return Err("RouteMatch.Method cannot contain HttpMethod.Any".into());
                }
                Ok(Self::Method {
                    method,
                    trailing_slash: value.trailing_slash,
                })
            }
            Kind::MountPrefix(_) => Ok(Self::MountPrefix),
        }
    }
}

impl From<RouteMatch> for proto::golem::customapi::RouteMatch {
    fn from(value: RouteMatch) -> Self {
        use proto::golem::customapi::route_match::{Kind, Method, MountPrefix};
        Self {
            kind: Some(match value {
                RouteMatch::Method {
                    method,
                    trailing_slash,
                } => Kind::Method(Method {
                    method: Some(method.into()),
                    trailing_slash,
                }),
                RouteMatch::MountPrefix => Kind::MountPrefix(MountPrefix {}),
            }),
        }
    }
}

impl TryFrom<proto::golem::customapi::RouteBehaviour> for RouteBehaviour {
    type Error = String;

    fn try_from(value: proto::golem::customapi::RouteBehaviour) -> Result<Self, Self::Error> {
        use proto::golem::customapi::route_behaviour::Kind;

        match value.kind.ok_or("RouteBehaviour.kind missing")? {
            Kind::CallAgent(call_agent) => Ok(RouteBehaviour::CallAgent(CallAgentBehaviour {
                base_path_variables: call_agent.base_path_variables,
                durable_streams: call_agent
                    .durable_streams
                    .map(TryInto::try_into)
                    .transpose()?,
                route_mode: call_agent.route_mode.try_into()?,
                component_id: call_agent
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                component_revision: call_agent.component_revision.try_into()?,
                agent_type: AgentTypeName(call_agent.agent_type),
                agent_mode: proto::golem::component::AgentMode::try_from(call_agent.agent_mode)
                    .map_err(|err| format!("Invalid agent_mode: {err}"))?
                    .into(),
                constructor_input: call_agent
                    .constructor_input
                    .ok_or("Missing constructor_input")?
                    .try_into()?,
                constructor_parameters: call_agent
                    .constructor_parameters
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                agent_selection: call_agent
                    .agent_selection
                    .ok_or("Missing agent_selection")?
                    .try_into()?,
                method_name: call_agent.method_name,
                method_input: call_agent
                    .method_input
                    .ok_or("Missing method_input")?
                    .try_into()?,
                body: call_agent.body.ok_or("Missing body")?.try_into()?,
                method_parameters: call_agent
                    .method_parameters
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                expected_agent_response: call_agent
                    .expected_agent_response
                    .ok_or("Missing expected_agent_response")?
                    .try_into()?,
                method_description: call_agent.method_description,
                read_only: call_agent.read_only.map(TryInto::try_into).transpose()?,
            })),
            Kind::CorsPreflight(cors_preflight) => {
                Ok(RouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
                    method_policies: cors_preflight
                        .method_policies
                        .into_iter()
                        .map(|method_policy| {
                            Ok::<_, String>(CorsPreflightMethodPolicy {
                                method: method_policy
                                    .method
                                    .ok_or("Missing cors preflight method")?
                                    .try_into()?,
                                allowed_origins: method_policy
                                    .allowed_origins
                                    .into_iter()
                                    .map(OriginPattern)
                                    .collect(),
                                allowed_headers: method_policy
                                    .allowed_headers
                                    .into_iter()
                                    .map(normalize_header_name)
                                    .collect(),
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                }))
            }
            Kind::WebhookCallback(webhook_callback) => {
                Ok(RouteBehaviour::WebhookCallback(WebhookCallbackBehaviour {
                    component_id: webhook_callback
                        .component_id
                        .ok_or("Missing component_id")?
                        .try_into()?,
                }))
            }
            Kind::OpenApiSpec(open_api_spec) => {
                use proto::golem::customapi::route_behaviour::open_api_spec::{Format, Scheme};

                let format = Format::try_from(open_api_spec.format)
                    .map_err(|_| "Invalid OpenApiSpec.format".to_string())?;

                let format = match format {
                    Format::Unspecified => {
                        return Err("OpenApiSpec.format missing".to_string());
                    }
                    Format::Json => OpenApiSpecFormat::Json,
                    Format::Yaml => OpenApiSpecFormat::Yaml,
                };

                let scheme = match Scheme::try_from(open_api_spec.scheme)
                    .map_err(|_| "Invalid OpenApiSpec.scheme".to_string())?
                {
                    Scheme::Unspecified => return Err("OpenApiSpec.scheme missing".to_string()),
                    Scheme::Http => HttpApiDeploymentScheme::Http,
                    Scheme::Https => HttpApiDeploymentScheme::Https,
                };
                Ok(RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                    format,
                    scheme,
                }))
            }
            Kind::CreateAgent(value) => Ok(RouteBehaviour::CreateAgent(CreateAgentBehaviour {
                agent: value.agent.ok_or("Missing create agent")?.try_into()?,
                location: value
                    .location
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            })),
            Kind::Mount(value) => Ok(RouteBehaviour::Mount(MountBehaviour {
                handler: value.handler.map(TryInto::try_into).transpose()?,
                openapi_provider: value.openapi_provider.map(TryInto::try_into).transpose()?,
                files: value.files.ok_or("Missing mount files")?.try_into()?,
                file_bindings: value
                    .file_bindings
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                file_response_headers: value
                    .file_response_headers
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            })),
        }
    }
}

impl TryFrom<proto::golem::customapi::route_behaviour::MountAgent> for MountAgent {
    type Error = String;

    fn try_from(
        value: proto::golem::customapi::route_behaviour::MountAgent,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value
                .component_id
                .ok_or("Missing component_id")?
                .try_into()?,
            component_revision: value.component_revision.try_into()?,
            agent_type: AgentTypeName(value.agent_type),
            agent_selection: value
                .agent_selection
                .ok_or("Missing mount agent_selection")?
                .try_into()?,
            constructor_input: value
                .constructor_input
                .ok_or("Missing constructor_input")?
                .try_into()?,
            constructor_parameters: value
                .constructor_parameters
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<MountAgent> for proto::golem::customapi::route_behaviour::MountAgent {
    fn from(value: MountAgent) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            component_revision: value.component_revision.into(),
            agent_type: value.agent_type.0,
            agent_selection: Some(value.agent_selection.into()),
            constructor_input: Some(value.constructor_input.into()),
            constructor_parameters: value
                .constructor_parameters
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl TryFrom<proto::golem::customapi::route_behaviour::MountTarget> for MountTarget {
    type Error = String;

    fn try_from(
        value: proto::golem::customapi::route_behaviour::MountTarget,
    ) -> Result<Self, Self::Error> {
        use proto::golem::customapi::route_behaviour::mount_target::Kind;
        match value.kind.ok_or("Missing MountTarget.kind")? {
            Kind::AgentMethod(value) => Ok(Self::AgentMethod(MountAgentMethod {
                agent: value.agent.ok_or("Missing target agent")?.try_into()?,
                method: value.method.ok_or("Missing target method")?.try_into()?,
            })),
        }
    }
}

impl From<MountTarget> for proto::golem::customapi::route_behaviour::MountTarget {
    fn from(value: MountTarget) -> Self {
        use proto::golem::customapi::route_behaviour::{MountAgentMethod, mount_target::Kind};
        let MountTarget::AgentMethod(value) = value;
        Self {
            kind: Some(Kind::AgentMethod(MountAgentMethod {
                agent: Some(value.agent.into()),
                method: Some(value.method.into()),
            })),
        }
    }
}

impl TryFrom<proto::golem::customapi::route_behaviour::MountFileSource> for MountFileSource {
    type Error = String;

    fn try_from(
        value: proto::golem::customapi::route_behaviour::MountFileSource,
    ) -> Result<Self, Self::Error> {
        use proto::golem::customapi::route_behaviour::mount_file_source::Kind;
        match value.kind.ok_or("Missing MountFileSource.kind")? {
            Kind::Router(value) => Ok(Self::Router {
                agent: value.agent.ok_or("Missing file source agent")?.try_into()?,
                file_index: decode_file_index(value.file_index)?,
            }),
            Kind::Live(value) => Ok(Self::Live {
                agent: value.try_into()?,
            }),
        }
    }
}

impl From<MountFileSource> for proto::golem::customapi::route_behaviour::MountFileSource {
    fn from(value: MountFileSource) -> Self {
        use proto::golem::customapi::route_behaviour::{RouterFiles, mount_file_source::Kind};
        Self {
            kind: Some(match value {
                MountFileSource::Router { agent, file_index } => Kind::Router(RouterFiles {
                    agent: Some(agent.into()),
                    file_index: file_index
                        .into_iter()
                        .map(|entry| {
                            proto::golem::customapi::route_behaviour::RouterFileIndexEntry {
                                path: entry.path,
                                blob_key: entry.blob_key.0.as_blake3_hash().as_bytes().to_vec(),
                                size: entry.size,
                            }
                        })
                        .collect(),
                }),
                MountFileSource::Live { agent } => Kind::Live(agent.into()),
            }),
        }
    }
}

impl TryFrom<proto::golem::customapi::route_behaviour::RouterMethod> for RouterMethod {
    type Error = String;
    fn try_from(
        value: proto::golem::customapi::route_behaviour::RouterMethod,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            method_name: value.method_name,
            input: value
                .input
                .ok_or("RouterMethod.input missing")?
                .try_into()?,
            output: value
                .output
                .ok_or("RouterMethod.output missing")?
                .try_into()?,
        })
    }
}

impl From<RouterMethod> for proto::golem::customapi::route_behaviour::RouterMethod {
    fn from(value: RouterMethod) -> Self {
        Self {
            method_name: value.method_name,
            input: Some(value.input.into()),
            output: Some(value.output.into()),
        }
    }
}

fn decode_file_index(
    values: Vec<proto::golem::customapi::route_behaviour::RouterFileIndexEntry>,
) -> Result<Vec<RouterFileIndexEntry>, String> {
    values
        .into_iter()
        .map(|value| {
            let blob_key: [u8; 32] = value
                .blob_key
                .try_into()
                .map_err(|_| "Router file blob_key must be exactly 32 bytes")?;
            Ok(RouterFileIndexEntry {
                path: value.path,
                blob_key: golem_common::model::agent::AgentFileContentHash(
                    golem_common::model::diff::Hash::from(blake3::Hash::from_bytes(blob_key)),
                ),
                size: value.size,
            })
        })
        .collect()
}

impl TryFrom<proto::golem::customapi::route_behaviour::AgentSelection> for AgentSelection {
    type Error = String;

    fn try_from(
        value: proto::golem::customapi::route_behaviour::AgentSelection,
    ) -> Result<Self, Self::Error> {
        use proto::golem::customapi::route_behaviour::agent_selection::Selection;
        Ok(
            match value.selection.ok_or("Missing AgentSelection.selection")? {
                Selection::Key(_) => Self::Key,
                Selection::Fresh(_) => Self::Fresh,
                Selection::PathIndex(index) => Self::Path {
                    index: index.into(),
                },
                Selection::Query(query) => {
                    if query.is_empty()
                        || query.trim() != query
                        || query.chars().any(char::is_control)
                    {
                        return Err("Agent query selector name must be nonempty and contain no surrounding whitespace or control characters".into());
                    }
                    Self::Query { name: query }
                }
            },
        )
    }
}

impl From<AgentSelection> for proto::golem::customapi::route_behaviour::AgentSelection {
    fn from(value: AgentSelection) -> Self {
        use proto::golem::customapi::route_behaviour::agent_selection::Selection;
        Self {
            selection: Some(match value {
                AgentSelection::Key => Selection::Key(Default::default()),
                AgentSelection::Fresh => Selection::Fresh(Default::default()),
                AgentSelection::Path { index } => Selection::PathIndex(index.into()),
                AgentSelection::Query { name } => Selection::Query(name),
            }),
        }
    }
}

impl From<RouteBehaviour> for proto::golem::customapi::RouteBehaviour {
    fn from(value: RouteBehaviour) -> Self {
        use proto::golem::customapi::route_behaviour::Kind;

        match value {
            RouteBehaviour::CallAgent(CallAgentBehaviour {
                route_mode,
                base_path_variables,
                durable_streams,
                component_id,
                component_revision,
                agent_type,
                agent_mode,
                constructor_input,
                constructor_parameters,
                agent_selection,
                method_name,
                method_input,
                body,
                method_parameters,
                expected_agent_response,
                method_description,
                read_only,
            }) => Self {
                kind: Some(Kind::CallAgent(
                    proto::golem::customapi::route_behaviour::CallAgent {
                        base_path_variables,
                        durable_streams: durable_streams.map(Into::into),
                        route_mode: golem_schema::proto::golem::http::HttpRouteMode::from(route_mode) as i32,
                        component_id: Some(component_id.into()),
                        component_revision: component_revision.into(),
                        agent_type: agent_type.0,
                        agent_mode: proto::golem::component::AgentMode::from(agent_mode) as i32,
                        constructor_input: Some(constructor_input.into()),
                        constructor_parameters: constructor_parameters
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                        agent_selection: Some(agent_selection.into()),
                        method_name,
                        method_input: Some(method_input.into()),
                        body: Some(body.into()),
                        method_parameters: method_parameters.into_iter().map(Into::into).collect(),
                        expected_agent_response: Some(expected_agent_response.into()),
                        method_description,
                        read_only: read_only.map(Into::into),
                    },
                )),
            },
            RouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
                method_policies,
            }) => Self {
                kind: Some(Kind::CorsPreflight(
                    proto::golem::customapi::route_behaviour::CorsPreflight {
                        method_policies: method_policies
                            .into_iter()
                            .map(|method_policy| {
                                proto::golem::customapi::route_behaviour::cors_preflight::MethodPolicy {
                                    method: Some(method_policy.method.into()),
                                    allowed_origins: method_policy
                                        .allowed_origins
                                        .into_iter()
                                        .map(|origin| origin.0)
                                        .collect(),
                                    allowed_headers: method_policy
                                        .allowed_headers
                                        .into_iter()
                                        .collect(),
                                }
                            })
                            .collect(),
                    },
                )),
            },
            RouteBehaviour::WebhookCallback(WebhookCallbackBehaviour { component_id }) => Self {
                kind: Some(Kind::WebhookCallback(
                    proto::golem::customapi::route_behaviour::WebhookCallback {
                        component_id: Some(component_id.into()),
                    },
                )),
            },
            RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour { format, scheme }) => {
                use proto::golem::customapi::route_behaviour::open_api_spec::{Format, Scheme};

                Self {
                    kind: Some(Kind::OpenApiSpec(
                        proto::golem::customapi::route_behaviour::OpenApiSpec {
                            format: match format {
                                OpenApiSpecFormat::Json => Format::Json,
                                OpenApiSpecFormat::Yaml => Format::Yaml,
                            }
                            .into(),
                            scheme: match scheme {
                                HttpApiDeploymentScheme::Http => Scheme::Http,
                                HttpApiDeploymentScheme::Https => Scheme::Https,
                            }.into(),
                        },
                    )),
                }
            }
            RouteBehaviour::CreateAgent(value) => Self { kind: Some(Kind::CreateAgent(
                proto::golem::customapi::route_behaviour::CreateAgent {
                    agent: Some(value.agent.into()),
                    location: value.location.into_iter().map(Into::into).collect(),
                })) },
            RouteBehaviour::Mount(value) => Self { kind: Some(Kind::Mount(
                proto::golem::customapi::route_behaviour::Mount {
                    handler: value.handler.map(Into::into), openapi_provider: value.openapi_provider.map(Into::into),
                    files: Some(value.files.into()),
                    file_bindings: value.file_bindings.into_iter().map(Into::into).collect(),
                    file_response_headers: value.file_response_headers.into_iter().map(Into::into).collect(),
                })) },
        }
    }
}

impl TryFrom<proto::golem::customapi::route_behaviour::DurableStreamRoutePolicy>
    for DurableStreamRoutePolicy
{
    type Error = String;

    fn try_from(
        value: proto::golem::customapi::route_behaviour::DurableStreamRoutePolicy,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            family_id: value.family_id,
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

impl From<DurableStreamRoutePolicy>
    for proto::golem::customapi::route_behaviour::DurableStreamRoutePolicy
{
    fn from(value: DurableStreamRoutePolicy) -> Self {
        Self {
            family_id: value.family_id,
            slots: value.slots.into_iter().map(Into::into).collect(),
            allow_external_writes: value.allow_external_writes,
            allow_stream_delete: value.allow_stream_delete,
            allow_invocation_delete: value.allow_invocation_delete,
            load: value.load.map(Into::into),
        }
    }
}

impl TryFrom<proto::golem::customapi::route_behaviour::DurableStreamSlot> for DurableStreamSlot {
    type Error = String;

    fn try_from(
        value: proto::golem::customapi::route_behaviour::DurableStreamSlot,
    ) -> Result<Self, Self::Error> {
        use proto::golem::customapi::route_behaviour::{
            DurableStreamRepresentation as ProtoRepresentation,
            DurableStreamSlotDirection as ProtoDirection,
        };

        let direction = match ProtoDirection::try_from(value.direction) {
            Ok(ProtoDirection::Input) => DurableStreamSlotDirection::Input,
            Ok(ProtoDirection::Output) => DurableStreamSlotDirection::Output,
            _ => return Err("Invalid or missing durable stream slot direction".into()),
        };
        let representation = match ProtoRepresentation::try_from(value.representation) {
            Ok(ProtoRepresentation::Json) => DurableStreamRepresentation::Json,
            Ok(ProtoRepresentation::Bytes) => DurableStreamRepresentation::Bytes,
            _ => return Err("Invalid or missing durable stream representation".into()),
        };
        Ok(Self {
            canonical_name: value.canonical_name,
            public_name: value.public_name,
            direction,
            content_type: value.content_type,
            representation,
        })
    }
}

impl From<DurableStreamSlot> for proto::golem::customapi::route_behaviour::DurableStreamSlot {
    fn from(value: DurableStreamSlot) -> Self {
        use proto::golem::customapi::route_behaviour::{
            DurableStreamRepresentation as ProtoRepresentation,
            DurableStreamSlotDirection as ProtoDirection,
        };

        Self {
            canonical_name: value.canonical_name,
            public_name: value.public_name,
            direction: match value.direction {
                DurableStreamSlotDirection::Input => ProtoDirection::Input as i32,
                DurableStreamSlotDirection::Output => ProtoDirection::Output as i32,
            },
            content_type: value.content_type,
            representation: match value.representation {
                DurableStreamRepresentation::Json => ProtoRepresentation::Json as i32,
                DurableStreamRepresentation::Bytes => ProtoRepresentation::Bytes as i32,
            },
        }
    }
}

impl From<proto::golem::customapi::route_behaviour::DurableStreamRouteLoadPolicy>
    for DurableStreamRouteLoadPolicy
{
    fn from(value: proto::golem::customapi::route_behaviour::DurableStreamRouteLoadPolicy) -> Self {
        Self {
            max_concurrent_readers_per_stream: value.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: value
                .max_append_requests_per_second_per_stream,
        }
    }
}

impl From<DurableStreamRouteLoadPolicy>
    for proto::golem::customapi::route_behaviour::DurableStreamRouteLoadPolicy
{
    fn from(value: DurableStreamRouteLoadPolicy) -> Self {
        Self {
            max_concurrent_readers_per_stream: value.max_concurrent_readers_per_stream,
            max_append_requests_per_second_per_stream: value
                .max_append_requests_per_second_per_stream,
        }
    }
}

impl TryFrom<proto::golem::customapi::RouteSecurity> for RouteSecurity {
    type Error = String;

    fn try_from(value: proto::golem::customapi::RouteSecurity) -> Result<Self, Self::Error> {
        use proto::golem::customapi::route_security::Kind;

        match value.kind.ok_or("RouteSecurity.kind missing")? {
            Kind::None(_) => Ok(RouteSecurity::None),
            Kind::Unavailable(_) => Ok(RouteSecurity::Unavailable),
            Kind::SessionFromHeader(session_from_header) => Ok(RouteSecurity::SessionFromHeader(
                SessionFromHeaderRouteSecurity {
                    header_name: session_from_header.header_name,
                },
            )),
            Kind::SecurityScheme(security_scheme) => {
                Ok(RouteSecurity::SecurityScheme(SecuritySchemeRouteSecurity {
                    security_scheme_id: security_scheme
                        .security_scheme_id
                        .ok_or("Missing security_scheme_id")?
                        .try_into()?,
                }))
            }
        }
    }
}

impl From<RouteSecurity> for proto::golem::customapi::RouteSecurity {
    fn from(value: RouteSecurity) -> Self {
        use proto::golem::customapi::route_security::Kind;

        match value {
            RouteSecurity::None => Self {
                kind: Some(Kind::None(proto::golem::customapi::route_security::None {})),
            },
            RouteSecurity::Unavailable => Self {
                kind: Some(Kind::Unavailable(
                    proto::golem::customapi::route_security::Unavailable {},
                )),
            },
            RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity { header_name }) => {
                Self {
                    kind: Some(Kind::SessionFromHeader(
                        proto::golem::customapi::route_security::SessionFromHeader { header_name },
                    )),
                }
            }
            RouteSecurity::SecurityScheme(SecuritySchemeRouteSecurity { security_scheme_id }) => {
                Self {
                    kind: Some(Kind::SecurityScheme(
                        proto::golem::customapi::route_security::SecurityScheme {
                            security_scheme_id: Some(security_scheme_id.into()),
                        },
                    )),
                }
            }
        }
    }
}

impl TryFrom<proto::golem::customapi::ConstructorParameter> for ConstructorParameter {
    type Error = String;

    fn try_from(value: proto::golem::customapi::ConstructorParameter) -> Result<Self, Self::Error> {
        use proto::golem::customapi::constructor_parameter::Kind;

        match value.kind.ok_or("ConstructorParameter.kind missing")? {
            Kind::Path(path) => Ok(ConstructorParameter::Path {
                path_segment_index: path.path_segment_index.into(),
                parameter_type: path
                    .parameter_type
                    .ok_or("Missing parameter_type")?
                    .try_into()?,
            }),
        }
    }
}

impl From<ConstructorParameter> for proto::golem::customapi::ConstructorParameter {
    fn from(value: ConstructorParameter) -> Self {
        use proto::golem::customapi::constructor_parameter::Kind;

        match value {
            ConstructorParameter::Path {
                path_segment_index,
                parameter_type,
            } => Self {
                kind: Some(Kind::Path(
                    proto::golem::customapi::constructor_parameter::Path {
                        path_segment_index: path_segment_index.into(),
                        parameter_type: Some(parameter_type.into()),
                    },
                )),
            },
        }
    }
}

impl TryFrom<proto::golem::customapi::MethodParameter> for MethodParameter {
    type Error = String;

    fn try_from(value: proto::golem::customapi::MethodParameter) -> Result<Self, Self::Error> {
        use proto::golem::customapi::method_parameter::Kind;

        match value.kind.ok_or("MethodParameter.kind missing")? {
            Kind::Path(path) => Ok(MethodParameter::Path {
                path_segment_index: path.path_segment_index.into(),
                parameter_type: path
                    .parameter_type
                    .ok_or("Missing parameter_type")?
                    .try_into()?,
            }),

            Kind::Query(query) => Ok(MethodParameter::Query {
                query_parameter_name: query.query_parameter_name,
                parameter_type: query
                    .parameter_type
                    .ok_or("Missing parameter_type")?
                    .try_into()?,
            }),

            Kind::Header(header) => Ok(MethodParameter::Header {
                header_name: header.header_name,
                parameter_type: header
                    .parameter_type
                    .ok_or("Missing parameter_type")?
                    .try_into()?,
            }),

            Kind::JsonObjectBodyField(field) => Ok(MethodParameter::JsonObjectBodyField {
                field_index: field.field_index.into(),
            }),

            Kind::UnstructuredBinaryBody(_) => Ok(MethodParameter::UnstructuredBinaryBody),

            Kind::UnstructuredTextBody(_) => Ok(MethodParameter::UnstructuredTextBody),
        }
    }
}

impl From<MethodParameter> for proto::golem::customapi::MethodParameter {
    fn from(value: MethodParameter) -> Self {
        use proto::golem::customapi::method_parameter::Kind;

        match value {
            MethodParameter::Path {
                path_segment_index,
                parameter_type,
            } => Self {
                kind: Some(Kind::Path(
                    proto::golem::customapi::method_parameter::Path {
                        path_segment_index: path_segment_index.into(),
                        parameter_type: Some(parameter_type.into()),
                    },
                )),
            },

            MethodParameter::Query {
                query_parameter_name,
                parameter_type,
            } => Self {
                kind: Some(Kind::Query(
                    proto::golem::customapi::method_parameter::Query {
                        query_parameter_name,
                        parameter_type: Some(parameter_type.into()),
                    },
                )),
            },

            MethodParameter::Header {
                header_name,
                parameter_type,
            } => Self {
                kind: Some(Kind::Header(
                    proto::golem::customapi::method_parameter::Header {
                        header_name,
                        parameter_type: Some(parameter_type.into()),
                    },
                )),
            },

            MethodParameter::JsonObjectBodyField { field_index } => Self {
                kind: Some(Kind::JsonObjectBodyField(
                    proto::golem::customapi::method_parameter::JsonObjectBodyField {
                        field_index: field_index.into(),
                    },
                )),
            },

            MethodParameter::UnstructuredBinaryBody => Self {
                kind: Some(Kind::UnstructuredBinaryBody(
                    proto::golem::customapi::method_parameter::UnstructuredBinaryBody {},
                )),
            },

            MethodParameter::UnstructuredTextBody => Self {
                kind: Some(Kind::UnstructuredTextBody(
                    proto::golem::customapi::method_parameter::UnstructuredTextBody {},
                )),
            },
        }
    }
}

impl TryFrom<proto::golem::customapi::CompiledSchema> for CompiledSchema {
    type Error = String;

    fn try_from(value: proto::golem::customapi::CompiledSchema) -> Result<Self, Self::Error> {
        Ok(CompiledSchema {
            graph: value
                .graph
                .ok_or("CompiledSchema.graph missing")?
                .try_into()?,
        })
    }
}

impl From<CompiledSchema> for proto::golem::customapi::CompiledSchema {
    fn from(value: CompiledSchema) -> Self {
        Self {
            graph: Some(value.graph.into()),
        }
    }
}

impl TryFrom<proto::golem::customapi::CompiledInputSchema> for CompiledInputSchema {
    type Error = String;

    fn try_from(value: proto::golem::customapi::CompiledInputSchema) -> Result<Self, Self::Error> {
        Ok(CompiledInputSchema {
            graph: value
                .graph
                .ok_or("CompiledInputSchema.graph missing")?
                .try_into()?,
            input_schema: value
                .input_schema
                .ok_or("CompiledInputSchema.input_schema missing")?
                .try_into()?,
        })
    }
}

impl From<CompiledInputSchema> for proto::golem::customapi::CompiledInputSchema {
    fn from(value: CompiledInputSchema) -> Self {
        Self {
            graph: Some(value.graph.into()),
            input_schema: Some(value.input_schema.into()),
        }
    }
}

impl TryFrom<proto::golem::customapi::CompiledOutputSchema> for CompiledOutputSchema {
    type Error = String;

    fn try_from(value: proto::golem::customapi::CompiledOutputSchema) -> Result<Self, Self::Error> {
        Ok(CompiledOutputSchema {
            graph: value
                .graph
                .ok_or("CompiledOutputSchema.graph missing")?
                .try_into()?,
            output_schema: value
                .output_schema
                .ok_or("CompiledOutputSchema.output_schema missing")?
                .try_into()?,
        })
    }
}

impl From<CompiledOutputSchema> for proto::golem::customapi::CompiledOutputSchema {
    fn from(value: CompiledOutputSchema) -> Self {
        Self {
            graph: Some(value.graph.into()),
            output_schema: Some(value.output_schema.into()),
        }
    }
}

impl TryFrom<proto::golem::customapi::RequestBodySchema> for RequestBodySchema {
    type Error = String;

    fn try_from(value: proto::golem::customapi::RequestBodySchema) -> Result<Self, Self::Error> {
        use proto::golem::customapi::request_body_schema::Kind;

        match value.kind.ok_or("RequestBodySchema.kind missing")? {
            Kind::Unused(_) => Ok(RequestBodySchema::Unused),

            Kind::JsonBody(body) => Ok(RequestBodySchema::JsonBody {
                expected: body
                    .expected
                    .ok_or("JsonBody.expected missing")?
                    .try_into()?,
            }),

            Kind::BinaryBody(body) => Ok(RequestBodySchema::BinaryBody {
                expected: body
                    .expected
                    .ok_or("BinaryBody.expected missing")?
                    .try_into()?,
            }),

            Kind::TextBody(body) => Ok(RequestBodySchema::TextBody {
                expected: body
                    .expected
                    .ok_or("TextBody.expected missing")?
                    .try_into()?,
            }),
        }
    }
}

impl From<RequestBodySchema> for proto::golem::customapi::RequestBodySchema {
    fn from(value: RequestBodySchema) -> Self {
        use proto::golem::customapi::request_body_schema::Kind;

        match value {
            RequestBodySchema::Unused => Self {
                kind: Some(Kind::Unused(
                    proto::golem::customapi::request_body_schema::Unused {},
                )),
            },

            RequestBodySchema::JsonBody { expected } => Self {
                kind: Some(Kind::JsonBody(
                    proto::golem::customapi::request_body_schema::JsonBody {
                        expected: Some(expected.into()),
                    },
                )),
            },

            RequestBodySchema::BinaryBody { expected } => Self {
                kind: Some(Kind::BinaryBody(
                    proto::golem::customapi::request_body_schema::BinaryBody {
                        expected: Some(expected.into()),
                    },
                )),
            },

            RequestBodySchema::TextBody { expected } => Self {
                kind: Some(Kind::TextBody(
                    proto::golem::customapi::request_body_schema::TextBody {
                        expected: Some(expected.into()),
                    },
                )),
            },
        }
    }
}

impl TryFrom<proto::golem::customapi::PathSegmentType> for PathSegmentType {
    type Error = String;

    fn try_from(value: proto::golem::customapi::PathSegmentType) -> Result<Self, Self::Error> {
        use proto::golem::customapi::path_segment_type::{Kind, Primitive};

        match value.kind.ok_or("PathSegmentType.kind missing")? {
            Kind::Primitive(p) => match Primitive::try_from(p).unwrap_or_default() {
                Primitive::Str => Ok(PathSegmentType::Str),
                Primitive::Chr => Ok(PathSegmentType::Chr),
                Primitive::F64 => Ok(PathSegmentType::F64),
                Primitive::F32 => Ok(PathSegmentType::F32),
                Primitive::U64 => Ok(PathSegmentType::U64),
                Primitive::S64 => Ok(PathSegmentType::S64),
                Primitive::U32 => Ok(PathSegmentType::U32),
                Primitive::S32 => Ok(PathSegmentType::S32),
                Primitive::U16 => Ok(PathSegmentType::U16),
                Primitive::S16 => Ok(PathSegmentType::S16),
                Primitive::U8 => Ok(PathSegmentType::U8),
                Primitive::S8 => Ok(PathSegmentType::S8),
                Primitive::Bool => Ok(PathSegmentType::Bool),
                Primitive::Uuid => Ok(PathSegmentType::Uuid),
                Primitive::Unspecified => Err("Invalid PathSegmentType::Primitive".to_string()),
            },

            Kind::EnumType(e) => {
                let inner = e.inner.ok_or("PathSegmentType::Enum.inner missing")?;

                Ok(PathSegmentType::Enum(inner.names))
            }
        }
    }
}

impl From<PathSegmentType> for proto::golem::customapi::PathSegmentType {
    fn from(value: PathSegmentType) -> Self {
        use proto::golem::customapi::path_segment_type::{Kind, Primitive};

        let kind = match value {
            PathSegmentType::Str => Kind::Primitive(Primitive::Str.into()),
            PathSegmentType::Chr => Kind::Primitive(Primitive::Chr.into()),
            PathSegmentType::F64 => Kind::Primitive(Primitive::F64.into()),
            PathSegmentType::F32 => Kind::Primitive(Primitive::F32.into()),
            PathSegmentType::U64 => Kind::Primitive(Primitive::U64.into()),
            PathSegmentType::S64 => Kind::Primitive(Primitive::S64.into()),
            PathSegmentType::U32 => Kind::Primitive(Primitive::U32.into()),
            PathSegmentType::S32 => Kind::Primitive(Primitive::S32.into()),
            PathSegmentType::U16 => Kind::Primitive(Primitive::U16.into()),
            PathSegmentType::S16 => Kind::Primitive(Primitive::S16.into()),
            PathSegmentType::U8 => Kind::Primitive(Primitive::U8.into()),
            PathSegmentType::S8 => Kind::Primitive(Primitive::S8.into()),
            PathSegmentType::Bool => Kind::Primitive(Primitive::Bool.into()),
            PathSegmentType::Uuid => Kind::Primitive(Primitive::Uuid.into()),

            PathSegmentType::Enum(inner) => {
                Kind::EnumType(proto::golem::customapi::path_segment_type::Enum {
                    inner: Some(proto::golem::customapi::PathSegmentEnum {
                        owner: None,
                        name: None,
                        names: inner,
                    }),
                })
            }
        };

        Self { kind: Some(kind) }
    }
}

impl TryFrom<proto::golem::customapi::PathSegment> for PathSegment {
    type Error = String;

    fn try_from(value: proto::golem::customapi::PathSegment) -> Result<Self, Self::Error> {
        use proto::golem::customapi::path_segment::Kind;

        match value.kind.ok_or("PathSegment.kind missing")? {
            Kind::Literal(inner) => Ok(PathSegment::Literal { value: inner.value }),

            Kind::Variable(inner) => Ok(PathSegment::Variable {
                display_name: inner.display_name,
            }),

            Kind::CatchAll(inner) => Ok(PathSegment::CatchAll {
                display_name: inner.display_name,
            }),
        }
    }
}

impl From<PathSegment> for proto::golem::customapi::PathSegment {
    fn from(value: PathSegment) -> Self {
        use proto::golem::customapi::path_segment::Kind;

        let kind = match value {
            PathSegment::Literal { value } => {
                Kind::Literal(proto::golem::customapi::path_segment::Literal { value })
            }

            PathSegment::Variable { display_name } => {
                Kind::Variable(proto::golem::customapi::path_segment::Variable { display_name })
            }

            PathSegment::CatchAll { display_name } => {
                Kind::CatchAll(proto::golem::customapi::path_segment::CatchAll { display_name })
            }
        };

        Self { kind: Some(kind) }
    }
}

impl TryFrom<proto::golem::customapi::QueryOrHeaderType> for QueryOrHeaderType {
    type Error = String;

    fn try_from(value: proto::golem::customapi::QueryOrHeaderType) -> Result<Self, Self::Error> {
        use proto::golem::customapi::query_or_header_type::Kind;

        match value.kind.ok_or("QueryOrHeaderType.kind missing")? {
            Kind::Primitive(p) => Ok(QueryOrHeaderType::Primitive(
                p.inner.ok_or("Primitive.inner missing")?.try_into()?,
            )),

            Kind::Option(opt) => Ok(QueryOrHeaderType::Option {
                name: opt.name,
                owner: opt.owner,
                inner: Box::new((*opt.inner.ok_or("Option.inner missing")?).try_into()?),
            }),

            Kind::List(list) => Ok(QueryOrHeaderType::List {
                name: list.name,
                owner: list.owner,
                inner: Box::new(list.inner.ok_or("List.inner missing")?.try_into()?),
            }),
            Kind::Map(map) => Ok(QueryOrHeaderType::Map {
                key: map.key.ok_or("Map.key missing")?.try_into()?,
                value: map.value.ok_or("Map.value missing")?.try_into()?,
            }),
        }
    }
}

impl From<QueryOrHeaderType> for proto::golem::customapi::QueryOrHeaderType {
    fn from(value: QueryOrHeaderType) -> Self {
        use proto::golem::customapi::query_or_header_type::Kind;

        let kind = match value {
            QueryOrHeaderType::Primitive(inner) => {
                Kind::Primitive(proto::golem::customapi::query_or_header_type::Primitive {
                    inner: Some(inner.into()),
                })
            }

            QueryOrHeaderType::Option { name, owner, inner } => Kind::Option(Box::new(
                proto::golem::customapi::query_or_header_type::Option {
                    name,
                    owner,
                    inner: Some(Box::new((*inner).into())),
                },
            )),

            QueryOrHeaderType::List { name, owner, inner } => {
                Kind::List(proto::golem::customapi::query_or_header_type::List {
                    name,
                    owner,
                    inner: Some((*inner).into()),
                })
            }
            QueryOrHeaderType::Map { key, value } => {
                Kind::Map(proto::golem::customapi::query_or_header_type::Map {
                    key: Some(key.into()),
                    value: Some(value.into()),
                })
            }
        };

        Self { kind: Some(kind) }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::customapi::CorsOptions> for CorsOptions {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::customapi::CorsOptions,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            allowed_patterns: value
                .allowed_patterns
                .into_iter()
                .map(OriginPattern)
                .collect(),
        })
    }
}

impl From<CorsOptions> for golem_api_grpc::proto::golem::customapi::CorsOptions {
    fn from(value: CorsOptions) -> Self {
        Self {
            allowed_patterns: value.allowed_patterns.into_iter().map(|op| op.0).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::{AgentFileContentHash, HttpMethod};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::schema::{InputSchema, OutputSchema, SchemaGraph, SchemaType};
    use golem_schema::http::{FileMapping, FileResponseHeader};
    use test_r::test;

    #[test]
    fn query_parameter_recursive_wire_roundtrips() {
        use prost::Message;

        let scalars = [
            PathSegmentType::Str,
            PathSegmentType::Chr,
            PathSegmentType::F64,
            PathSegmentType::F32,
            PathSegmentType::U64,
            PathSegmentType::S64,
            PathSegmentType::U32,
            PathSegmentType::S32,
            PathSegmentType::U16,
            PathSegmentType::S16,
            PathSegmentType::U8,
            PathSegmentType::S8,
            PathSegmentType::Bool,
            PathSegmentType::Uuid,
            PathSegmentType::Enum(vec!["a".into(), "β".into()]),
        ];
        for scalar in scalars {
            for inner in [
                QueryOrHeaderType::Primitive(scalar.clone()),
                QueryOrHeaderType::List {
                    name: Some("items".into()),
                    owner: Some("owner".into()),
                    inner: Box::new(scalar.clone()),
                },
                QueryOrHeaderType::Map {
                    key: scalar.clone(),
                    value: scalar.clone(),
                },
            ] {
                let parameter = QueryOrHeaderType::Option {
                    name: Some("optional".into()),
                    owner: Some("owner".into()),
                    inner: Box::new(inner),
                };
                let wire: proto::golem::customapi::QueryOrHeaderType = parameter.clone().into();
                let decoded = proto::golem::customapi::QueryOrHeaderType::decode(
                    wire.encode_to_vec().as_slice(),
                )
                .unwrap();
                assert_eq!(QueryOrHeaderType::try_from(decoded).unwrap(), parameter);
                let bytes = desert_rust::serialize_to_byte_vec(&parameter).unwrap();
                let decoded: QueryOrHeaderType = desert_rust::deserialize(&bytes).unwrap();
                assert_eq!(decoded, parameter);
            }
        }

        // Missing recursive children must remain invalid after real byte transport.
        for child in [
            None,
            Some(Box::new(proto::golem::customapi::QueryOrHeaderType {
                kind: None,
            })),
        ] {
            let wire = proto::golem::customapi::QueryOrHeaderType {
                kind: Some(proto::golem::customapi::query_or_header_type::Kind::Option(
                    Box::new(proto::golem::customapi::query_or_header_type::Option {
                        name: None,
                        owner: None,
                        inner: child,
                    }),
                )),
            };
            let decoded =
                proto::golem::customapi::QueryOrHeaderType::decode(wire.encode_to_vec().as_slice())
                    .unwrap();
            assert!(QueryOrHeaderType::try_from(decoded).is_err());
        }
    }

    #[test]
    fn query_parameter_collection_contract_roundtrips() {
        use prost::Message;

        for schema in [
            SchemaType::list(SchemaType::u32()),
            SchemaType::option(SchemaType::list(SchemaType::string())),
            SchemaType::map(SchemaType::u32(), SchemaType::bool()),
            SchemaType::option(SchemaType::map(SchemaType::string(), SchemaType::s64())),
        ] {
            let parameter = QueryOrHeaderType::try_from(schema.clone()).unwrap();
            assert_eq!(SchemaType::from(parameter.clone()), schema);
            let bytes = desert_rust::serialize_to_byte_vec(&parameter).unwrap();
            let decoded: QueryOrHeaderType = desert_rust::deserialize(&bytes).unwrap();
            assert_eq!(decoded, parameter);
            let wire: proto::golem::customapi::QueryOrHeaderType = parameter.clone().into();
            let wire =
                proto::golem::customapi::QueryOrHeaderType::decode(wire.encode_to_vec().as_slice())
                    .unwrap();
            assert_eq!(QueryOrHeaderType::try_from(wire).unwrap(), parameter);
        }
        assert!(
            QueryOrHeaderType::try_from(SchemaType::list(SchemaType::list(SchemaType::string())))
                .is_err()
        );
        assert!(
            QueryOrHeaderType::try_from(SchemaType::map(
                SchemaType::string(),
                SchemaType::record(vec![])
            ))
            .is_err()
        );
        for map in [
            proto::golem::customapi::query_or_header_type::Map {
                key: None,
                value: Some(PathSegmentType::Str.into()),
            },
            proto::golem::customapi::query_or_header_type::Map {
                key: Some(PathSegmentType::Str.into()),
                value: None,
            },
        ] {
            assert!(
                QueryOrHeaderType::try_from(proto::golem::customapi::QueryOrHeaderType {
                    kind: Some(proto::golem::customapi::query_or_header_type::Kind::Map(
                        map
                    )),
                })
                .is_err()
            );
        }
    }

    #[test]
    fn agent_selection_roundtrips_and_rejects_missing_wire_selection() {
        for selection in [
            AgentSelection::Key,
            AgentSelection::Fresh,
            AgentSelection::Path { index: 3.into() },
            AgentSelection::Query {
                name: "instance".into(),
            },
        ] {
            let bytes = desert_rust::serialize_to_byte_vec(&selection).unwrap();
            let decoded: AgentSelection = desert_rust::deserialize(&bytes).unwrap();
            assert_eq!(decoded, selection);
            let proto: proto::golem::customapi::route_behaviour::AgentSelection =
                selection.clone().into();
            assert_eq!(AgentSelection::try_from(proto).unwrap(), selection);
        }
        assert!(
            AgentSelection::try_from(proto::golem::customapi::route_behaviour::AgentSelection {
                selection: None
            })
            .is_err()
        );
        let empty_query = AgentSelection::Query { name: "".into() };
        let proto: proto::golem::customapi::route_behaviour::AgentSelection = empty_query.into();
        assert!(AgentSelection::try_from(proto).is_err());
    }

    #[test]
    fn openapi_scheme_roundtrips_codecs_and_rejects_missing_or_unknown_wire_values() {
        use proto::golem::customapi::route_behaviour::{Kind, OpenApiSpec};
        for scheme in [
            HttpApiDeploymentScheme::Http,
            HttpApiDeploymentScheme::Https,
        ] {
            let behavior = OpenApiSpecBehaviour {
                format: OpenApiSpecFormat::Yaml,
                scheme,
            };
            let bytes = desert_rust::serialize_to_byte_vec(&behavior).unwrap();
            let decoded: OpenApiSpecBehaviour = desert_rust::deserialize(&bytes).unwrap();
            assert_eq!(decoded.scheme, scheme);
            let encoded: proto::golem::customapi::RouteBehaviour =
                RouteBehaviour::OpenApiSpec(decoded).into();
            let RouteBehaviour::OpenApiSpec(decoded) = RouteBehaviour::try_from(encoded).unwrap()
            else {
                panic!("wrong behavior")
            };
            assert_eq!(decoded.scheme, scheme);
            assert!(matches!(decoded.format, OpenApiSpecFormat::Yaml));
        }
        for scheme in [0, 3, -1] {
            let encoded = proto::golem::customapi::RouteBehaviour {
                kind: Some(Kind::OpenApiSpec(OpenApiSpec { format: 1, scheme })),
            };
            assert!(RouteBehaviour::try_from(encoded).is_err());
        }
    }

    #[test]
    fn unavailable_route_security_roundtrip_is_not_public() {
        let encoded: proto::golem::customapi::RouteSecurity = RouteSecurity::Unavailable.into();
        assert!(matches!(
            RouteSecurity::try_from(encoded).unwrap(),
            RouteSecurity::Unavailable
        ));
    }

    #[test]
    fn security_scheme_revision_and_login_roundtrip() {
        let details = SecuritySchemeDetails {
            id: golem_common::model::security_scheme::SecuritySchemeId::new(),
            revision: SecuritySchemeRevision::new(7).unwrap(),
            name: SecuritySchemeName("frontend-login".into()),
            provider_type: Provider::Google(golem_schema::Empty {}),
            client_id: ClientId::new("client".into()),
            client_secret: ClientSecret::new("secret".into()),
            redirect_url: RedirectUrl::new("https://api.example/callback".into()).unwrap(),
            scopes: vec![Scope::new("openid".into())],
            login: SecuritySchemeLogin::AuthorizationCodePkce(
                golem_common::model::security_scheme::AuthorizationCodePkceConfig {
                    redirect_uris: vec!["https://frontend.example/callback".into()],
                    origins: vec!["https://frontend.example".into()],
                },
            ),
        };
        let id = details.id;
        let encoded: proto::golem::customapi::SecuritySchemeDetails = details.into();
        let decoded = SecuritySchemeDetails::try_from(encoded).unwrap();

        assert_eq!(decoded.id, id);
        assert_eq!(decoded.revision, SecuritySchemeRevision::new(7).unwrap());
        assert!(matches!(
            decoded.login,
            SecuritySchemeLogin::AuthorizationCodePkce(ref config)
                if config.redirect_uris == ["https://frontend.example/callback"]
                    && config.origins == ["https://frontend.example"]
        ));
    }

    fn input() -> CompiledInputSchema {
        CompiledInputSchema {
            graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
            input_schema: InputSchema::Parameters(vec![]),
        }
    }

    fn id_agent(selection: AgentSelection) -> MountAgent {
        let constructor_input = if matches!(
            selection,
            AgentSelection::Path { .. } | AgentSelection::Query { .. }
        ) {
            CompiledInputSchema {
                graph: SchemaGraph::anonymous(SchemaType::record(vec![
                    golem_common::schema::NamedFieldType {
                        name: "owner".into(),
                        body: SchemaType::string(),
                        metadata: Default::default(),
                    },
                ])),
                input_schema: InputSchema::Parameters(vec![
                    golem_common::schema::NamedField::user_supplied("owner", SchemaType::string()),
                ]),
            }
        } else {
            input()
        };
        MountAgent {
            component_id: ComponentId(uuid::Uuid::from_u128(17)),
            component_revision: ComponentRevision::try_from(23u64).unwrap(),
            agent_type: AgentTypeName("files".into()),
            constructor_input,
            constructor_parameters: vec![],
            agent_selection: selection,
        }
    }

    #[test]
    fn call_agent_selections_roundtrip_and_reject_invalid_bindings() {
        use prost::Message;
        for (selection, session) in [
            (AgentSelection::Key, false),
            (AgentSelection::Fresh, false),
            (AgentSelection::Path { index: 0.into() }, false),
            (AgentSelection::Path { index: 1.into() }, false),
            (AgentSelection::Query { name: "id".into() }, false),
            (AgentSelection::Fresh, true),
        ] {
            let second_capture =
                matches!(selection, AgentSelection::Path { index } if usize::from(index) == 1);
            let durable_streams = session.then(|| DurableStreamRoutePolicy {
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
                        content_type: "application/octet-stream".into(),
                        representation: DurableStreamRepresentation::Bytes,
                    },
                ],
                allow_external_writes: true,
                allow_stream_delete: false,
                allow_invocation_delete: true,
                load: Some(DurableStreamRouteLoadPolicy {
                    max_concurrent_readers_per_stream: Some(7),
                    max_append_requests_per_second_per_stream: Some(11),
                }),
            });
            let agent = id_agent(selection);
            let call = CallAgentBehaviour {
                route_mode: if session {
                    crate::custom_api::AgentRouteMode::DurableStreams
                } else {
                    crate::custom_api::AgentRouteMode::Plain
                },
                base_path_variables: if second_capture { 2 } else { 1 },
                durable_streams: durable_streams.clone(),
                component_id: agent.component_id,
                component_revision: agent.component_revision,
                agent_type: agent.agent_type,
                agent_mode: if session {
                    golem_common::model::agent::AgentMode::Ephemeral
                } else {
                    golem_common::model::agent::AgentMode::Durable
                },
                constructor_input: agent.constructor_input,
                constructor_parameters: agent.constructor_parameters,
                agent_selection: agent.agent_selection,
                method_name: "get".into(),
                method_input: input(),
                body: RequestBodySchema::Unused,
                method_parameters: vec![],
                expected_agent_response: CompiledOutputSchema {
                    graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                    output_schema: OutputSchema::Unit,
                },
                method_description: None,
                read_only: None,
            };
            let mut compiled = route(RouteBehaviour::CallAgent(call));
            compiled.route_match =
                HttpMethod::Method(golem_schema::http::HttpMethod::Get(Default::default())).into();
            compiled.path.push(PathSegment::Variable {
                display_name: if second_capture || session {
                    "tenant"
                } else {
                    "id"
                }
                .into(),
            });
            if second_capture {
                compiled.path.extend([
                    PathSegment::Literal {
                        value: "agents".into(),
                    },
                    PathSegment::Variable {
                        display_name: "id".into(),
                    },
                    PathSegment::Literal {
                        value: "get".into(),
                    },
                ]);
            }
            if session {
                compiled.path.extend([
                    PathSegment::Literal {
                        value: "sessions".into(),
                    },
                    PathSegment::Variable {
                        display_name: "session".into(),
                    },
                    PathSegment::Literal {
                        value: "streams".into(),
                    },
                    PathSegment::Variable {
                        display_name: "slot".into(),
                    },
                ]);
            }
            let expected: proto::golem::customapi::CompiledRoute = compiled.into();
            let expected_bytes = expected.encode_to_vec();
            let decoded =
                proto::golem::customapi::CompiledRoute::decode(expected_bytes.as_slice()).unwrap();
            let mut decoded = CompiledRoute::try_from(decoded).unwrap();
            let binary = desert_rust::serialize_to_byte_vec(&decoded.behavior).unwrap();
            decoded.behavior = desert_rust::deserialize(&binary).unwrap();
            let RouteBehaviour::CallAgent(call) = &decoded.behavior else {
                unreachable!()
            };
            assert_eq!(call.durable_streams, durable_streams);
            if session {
                assert_eq!(call.agent_selection, AgentSelection::Fresh);
                assert_eq!(
                    call.agent_mode,
                    golem_common::model::agent::AgentMode::Ephemeral
                );
                assert_eq!(
                    call.route_mode,
                    crate::custom_api::AgentRouteMode::DurableStreams
                );
                assert_eq!(call.base_path_variables, 1);
                assert_eq!(
                    decoded
                        .path
                        .iter()
                        .filter(|segment| matches!(segment, PathSegment::Variable { .. }))
                        .count(),
                    3
                );
            }
            if second_capture {
                assert_eq!(
                    call.agent_selection,
                    AgentSelection::Path { index: 1.into() }
                );
                assert_eq!(call.base_path_variables, 2);
            }
            let actual: proto::golem::customapi::CompiledRoute = decoded.into();
            assert_eq!(actual, expected);
            assert_eq!(actual.encode_to_vec(), expected_bytes);
            let mut missing = expected.clone();
            let Some(proto::golem::customapi::route_behaviour::Kind::CallAgent(call)) =
                missing.behavior.as_mut().unwrap().kind.as_mut()
            else {
                unreachable!()
            };
            call.agent_selection = None;
            assert!(CompiledRoute::try_from(missing).is_err());

            let mut invalid = expected;
            let Some(proto::golem::customapi::route_behaviour::Kind::CallAgent(call)) =
                invalid.behavior.as_mut().unwrap().kind.as_mut()
            else {
                unreachable!()
            };
            call.agent_selection = Some(AgentSelection::Query { name: "id".into() }.into());
            call.method_parameters.push(
                MethodParameter::Query {
                    query_parameter_name: "id".into(),
                    parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::Str),
                }
                .into(),
            );
            assert!(CompiledRoute::try_from(invalid).is_err());
        }
    }

    #[test]
    fn id_mount_and_create_transport_roundtrip_and_validation() {
        use prost::Message;
        for (selection, location) in [
            (
                AgentSelection::Path { index: 0.into() },
                vec![
                    PathSegment::Literal {
                        value: "files".into(),
                    },
                    PathSegment::Variable {
                        display_name: "id".into(),
                    },
                ],
            ),
            (
                AgentSelection::Query { name: "id".into() },
                vec![PathSegment::Literal {
                    value: "files".into(),
                }],
            ),
            (AgentSelection::Query { name: "id".into() }, vec![]),
        ] {
            for create in [false, true] {
                let agent = id_agent(selection.clone());
                let mut compiled = route(if create {
                    RouteBehaviour::CreateAgent(CreateAgentBehaviour {
                        agent,
                        location: location.clone(),
                    })
                } else {
                    RouteBehaviour::Mount(MountBehaviour {
                        handler: None,
                        openapi_provider: None,
                        files: MountFileSource::Live { agent },
                        file_bindings: vec![],
                        file_response_headers: vec![],
                    })
                });
                compiled.path = if create {
                    vec![PathSegment::Literal {
                        value: "create".into(),
                    }]
                } else {
                    location.clone()
                };
                if create {
                    compiled.route_match = HttpMethod::Method(
                        golem_schema::http::HttpMethod::Post(Default::default()),
                    )
                    .into();
                }
                let expected: proto::golem::customapi::CompiledRoute = compiled.into();
                let bytes = expected.encode_to_vec();
                let wire =
                    proto::golem::customapi::CompiledRoute::decode(bytes.as_slice()).unwrap();
                let decoded = CompiledRoute::try_from(wire).unwrap();
                let binary = desert_rust::serialize_to_byte_vec(&decoded.behavior).unwrap();
                let behavior: RouteBehaviour = desert_rust::deserialize(&binary).unwrap();
                if let RouteBehaviour::CreateAgent(create) = &behavior {
                    assert_eq!(create.location, location);
                }
                decoded
                    .route_match
                    .validate(&decoded.path, &behavior)
                    .unwrap();
                let actual: proto::golem::customapi::CompiledRoute = decoded.into();
                assert_eq!(actual, expected);

                let mut missing = expected.clone();
                let agent = match missing.behavior.as_mut().unwrap().kind.as_mut().unwrap() {
                    proto::golem::customapi::route_behaviour::Kind::CreateAgent(value) => {
                        value.agent.as_mut().unwrap()
                    }
                    proto::golem::customapi::route_behaviour::Kind::Mount(value) => {
                        let proto::golem::customapi::route_behaviour::mount_file_source::Kind::Live(
                            agent,
                        ) = value.files.as_mut().unwrap().kind.as_mut().unwrap()
                        else {
                            unreachable!()
                        };
                        agent
                    }
                    _ => unreachable!(),
                };
                agent.agent_selection = None;
                assert!(CompiledRoute::try_from(missing).is_err());
            }
        }
    }

    #[test]
    fn id_selection_and_create_reject_malformed_shapes() {
        let literal = vec![PathSegment::Literal {
            value: "files".into(),
        }];
        let captured = vec![PathSegment::Variable {
            display_name: "id".into(),
        }];
        let parameter = ConstructorParameter::Path {
            path_segment_index: 0.into(),
            parameter_type: PathSegmentType::Str,
        };
        for selection in [
            AgentSelection::Path { index: 0.into() },
            AgentSelection::Query { name: "id".into() },
        ] {
            assert!(
                selection
                    .validate(&captured, std::slice::from_ref(&parameter))
                    .is_err()
            );
        }
        assert!(
            AgentSelection::Path { index: 1.into() }
                .validate(&captured, &[])
                .is_err()
        );
        assert!(
            AgentSelection::Path { index: 0.into() }
                .validate(&literal, &[])
                .is_err()
        );
        for name in ["", " id", "id\n"] {
            assert!(
                AgentSelection::Query { name: name.into() }
                    .validate(&literal, &[])
                    .is_err()
            );
        }
        let post: RouteMatch =
            HttpMethod::Method(golem_schema::http::HttpMethod::Post(Default::default())).into();
        let get: RouteMatch =
            HttpMethod::Method(golem_schema::http::HttpMethod::Get(Default::default())).into();
        for (selection, location) in [
            (AgentSelection::Key, literal.clone()),
            (AgentSelection::Fresh, literal.clone()),
            (AgentSelection::Path { index: 0.into() }, vec![]),
            (AgentSelection::Path { index: 0.into() }, literal.clone()),
            (AgentSelection::Path { index: 1.into() }, captured.clone()),
            (
                AgentSelection::Query { name: "id".into() },
                captured.clone(),
            ),
            (
                AgentSelection::Path { index: 0.into() },
                vec![captured[0].clone(), captured[0].clone()],
            ),
            (
                AgentSelection::Path { index: 0.into() },
                vec![PathSegment::CatchAll {
                    display_name: "id".into(),
                }],
            ),
        ] {
            let behavior = RouteBehaviour::CreateAgent(CreateAgentBehaviour {
                agent: id_agent(selection),
                location,
            });
            assert!(post.validate(&literal, &behavior).is_err());
        }
        let behavior = RouteBehaviour::CreateAgent(CreateAgentBehaviour {
            agent: id_agent(AgentSelection::Query { name: "id".into() }),
            location: literal.clone(),
        });
        assert!(get.validate(&literal, &behavior).is_err());
        assert!(
            RouteMatch::MountPrefix
                .validate(&literal, &behavior)
                .is_err()
        );
        assert!(post.validate(&captured, &behavior).is_err());
    }

    fn router() -> RouteBehaviour {
        let agent = MountAgent {
            component_id: ComponentId(uuid::Uuid::from_u128(17)),
            component_revision: ComponentRevision::try_from(23u64).unwrap(),
            agent_type: AgentTypeName("site".into()),
            agent_selection: AgentSelection::Key,
            constructor_input: input(),
            constructor_parameters: vec![],
        };
        RouteBehaviour::Mount(MountBehaviour {
            handler: None,
            openapi_provider: Some(MountTarget::AgentMethod(MountAgentMethod {
                agent: agent.clone(),
                method: RouterMethod {
                    method_name: "schema".into(),
                    input: input(),
                    output: CompiledOutputSchema {
                        graph: SchemaGraph::anonymous(SchemaType::string()),
                        output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
                    },
                },
            })),
            file_bindings: FileMapping::compile_list([
                ("/favicon", "/assets/icon"),
                ("/*", "/assets/$1"),
            ])
            .unwrap(),
            files: MountFileSource::Router {
                agent,
                file_index: vec![RouterFileIndexEntry {
                    path: "/assets/%2e$icon".into(),
                    blob_key: AgentFileContentHash(golem_common::model::diff::Hash::from(
                        blake3::hash(b"blob"),
                    )),
                    size: 4_294_967_301,
                }],
            },
            file_response_headers: vec![
                FileResponseHeader {
                    name: "content-security-policy".into(),
                    value: "default-src 'self'".into(),
                },
                FileResponseHeader {
                    name: "referrer-policy".into(),
                    value: "same-origin".into(),
                },
            ],
        })
    }

    fn route(behavior: RouteBehaviour) -> CompiledRoute {
        let mut path = vec![PathSegment::Literal {
            value: "site".into(),
        }];
        if matches!(&behavior, RouteBehaviour::Mount(mount) if matches!(mount.files, MountFileSource::Live { .. }))
        {
            path.push(PathSegment::Variable {
                display_name: "id".into(),
            });
        }
        CompiledRoute {
            route_id: 19,
            route_match: RouteMatch::MountPrefix,
            path,
            behavior,
            security: RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
                header_name: "x-session".into(),
            }),
            cors: CorsOptions {
                allowed_patterns: [OriginPattern("https://site.example".into())].into(),
            },
        }
    }

    #[test]
    fn mounted_file_headers_are_validated_on_compiled_route_ingress() {
        for live in [false, true] {
            let behavior = if live {
                RouteBehaviour::Mount(MountBehaviour {
                    handler: None,
                    openapi_provider: None,
                    files: MountFileSource::Live {
                        agent: MountAgent {
                            component_id: ComponentId(uuid::Uuid::from_u128(42)),
                            component_revision: ComponentRevision::try_from(31u64).unwrap(),
                            agent_type: AgentTypeName("files".into()),
                            agent_selection: AgentSelection::Key,
                            constructor_input: input(),
                            constructor_parameters: vec![],
                        },
                    },
                    file_bindings: FileMapping::compile_list([("/*", "/public/$1")]).unwrap(),
                    file_response_headers: vec![],
                })
            } else {
                router()
            };
            let encoded: proto::golem::customapi::CompiledRoute = route(behavior).into();
            for (headers, has_bindings, valid) in [
                (
                    vec![("Content-Security-Policy", "default-src 'self'")],
                    true,
                    true,
                ),
                (vec![("bad name", "value")], true, false),
                (vec![("X-Policy", "value\r\ninjected: value")], true, false),
                (vec![("Content-Length", "123")], true, false),
                (vec![("Access-Control-Allow-Origin", "*")], true, false),
                (vec![("X-Policy", "one"), ("x-policy", "two")], true, false),
                (vec![("Referrer-Policy", "no-referrer")], false, false),
                (vec![], false, true),
            ] {
                let mut candidate = encoded.clone();
                let headers = headers
                    .into_iter()
                    .map(
                        |(name, value)| golem_schema::proto::golem::http::FileResponseHeader {
                            name: name.into(),
                            value: value.into(),
                        },
                    )
                    .collect();
                match candidate.behavior.as_mut().unwrap().kind.as_mut().unwrap() {
                    proto::golem::customapi::route_behaviour::Kind::Mount(mount) => {
                        mount.file_response_headers = headers;
                        if !has_bindings {
                            mount.file_bindings.clear();
                        }
                    }
                    _ => unreachable!(),
                }
                let behavior: RouteBehaviour =
                    candidate.behavior.clone().unwrap().try_into().unwrap();
                let path = candidate
                    .path
                    .clone()
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(
                    RouteMatch::MountPrefix.validate(&path, &behavior).is_ok(),
                    valid
                );
                assert_eq!(CompiledRoute::try_from(candidate).is_ok(), valid);
            }
        }
    }

    #[test]
    fn mounted_dispatch_binary_and_protobuf_roundtrip() {
        let filesystem = RouteBehaviour::Mount(MountBehaviour {
            handler: None,
            openapi_provider: None,
            files: MountFileSource::Live {
                agent: MountAgent {
                    component_id: ComponentId(uuid::Uuid::from_u128(42)),
                    component_revision: ComponentRevision::try_from(31u64).unwrap(),
                    agent_type: AgentTypeName("files".into()),
                    agent_selection: AgentSelection::Key,
                    constructor_input: input(),
                    constructor_parameters: vec![ConstructorParameter::Path {
                        path_segment_index: 0u32.into(),
                        parameter_type: PathSegmentType::U64,
                    }],
                },
            },
            file_bindings: FileMapping::compile_list([
                ("/*", "/public/$1"),
                ("/fallback", "/fallback"),
            ])
            .unwrap(),
            file_response_headers: vec![
                FileResponseHeader {
                    name: "content-security-policy".into(),
                    value: "default-src 'none'".into(),
                },
                FileResponseHeader {
                    name: "referrer-policy".into(),
                    value: "no-referrer".into(),
                },
            ],
        });
        let mut filesystem_route = route(filesystem);
        filesystem_route
            .route_match
            .validate(&filesystem_route.path, &filesystem_route.behavior)
            .unwrap();
        let RouteBehaviour::Mount(MountBehaviour {
            files: MountFileSource::Live { agent: filesystem },
            ..
        }) = &mut filesystem_route.behavior
        else {
            unreachable!()
        };
        filesystem
            .constructor_parameters
            .push(ConstructorParameter::Path {
                path_segment_index: 0u32.into(),
                parameter_type: PathSegmentType::U64,
            });
        assert!(
            filesystem_route
                .route_match
                .validate(&filesystem_route.path, &filesystem_route.behavior)
                .is_err()
        );
        let RouteBehaviour::Mount(MountBehaviour {
            files: MountFileSource::Live { agent: filesystem },
            ..
        }) = &mut filesystem_route.behavior
        else {
            unreachable!()
        };
        filesystem.constructor_parameters = vec![ConstructorParameter::Path {
            path_segment_index: 1u32.into(),
            parameter_type: PathSegmentType::U64,
        }];
        assert!(
            filesystem_route
                .route_match
                .validate(&filesystem_route.path, &filesystem_route.behavior)
                .is_err()
        );
        let RouteBehaviour::Mount(MountBehaviour {
            files: MountFileSource::Live { agent: filesystem },
            ..
        }) = &mut filesystem_route.behavior
        else {
            unreachable!()
        };
        filesystem.constructor_parameters = vec![ConstructorParameter::Path {
            path_segment_index: 0u32.into(),
            parameter_type: PathSegmentType::U64,
        }];
        let filesystem = filesystem_route.behavior;
        for behavior in [router(), filesystem] {
            let expected: proto::golem::customapi::RouteBehaviour = behavior.into();
            let decoded: RouteBehaviour = expected.clone().try_into().unwrap();
            let bytes = desert_rust::serialize_to_byte_vec(&decoded).unwrap();
            let decoded: RouteBehaviour = desert_rust::deserialize(&bytes).unwrap();
            let actual: proto::golem::customapi::RouteBehaviour = decoded.into();
            assert_eq!(actual, expected);

            let expected: proto::golem::customapi::CompiledRoute =
                route(actual.try_into().unwrap()).into();
            let decoded: CompiledRoute = expected.clone().try_into().unwrap();
            let actual: proto::golem::customapi::CompiledRoute = decoded.into();
            assert_eq!(actual, expected);
        }
        for route_match in [
            RouteMatch::MountPrefix,
            RouteMatch::Method {
                method: HttpMethod::Method(golem_schema::http::HttpMethod::Custom(
                    golem_schema::http::CustomHttpMethod {
                        value: "ANY".into(),
                    },
                )),
                trailing_slash: true,
            },
        ] {
            let bytes = desert_rust::serialize_to_byte_vec(&route_match).unwrap();
            let decoded: RouteMatch = desert_rust::deserialize(&bytes).unwrap();
            assert_eq!(decoded, route_match);
            let encoded: proto::golem::customapi::RouteMatch = decoded.into();
            assert_eq!(RouteMatch::try_from(encoded).unwrap(), route_match);
        }
    }

    #[test]
    fn mounted_dispatch_rejects_invalid_pairings_and_index() {
        let mut invalid = route(router());
        invalid.route_match =
            HttpMethod::Method(golem_schema::http::HttpMethod::Get(Default::default())).into();
        let encoded: proto::golem::customapi::CompiledRoute = invalid.into();
        assert!(CompiledRoute::try_from(encoded).is_err());
        let encoded: proto::golem::customapi::CompiledRoute =
            route(RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                format: OpenApiSpecFormat::Json,
                scheme: HttpApiDeploymentScheme::Https,
            }))
            .into();
        assert!(CompiledRoute::try_from(encoded).is_err());

        for path in [
            "/", "/assets/", "relative", "/a//b", "/a/../b", "/a\\b", "/a\u{7f}",
        ] {
            let mut invalid = router();
            let RouteBehaviour::Mount(MountBehaviour {
                files: MountFileSource::Router { file_index, .. },
                ..
            }) = &mut invalid
            else {
                unreachable!()
            };
            file_index[0].path = path.into();
            // Same validation protects binary reload and protobuf ingress.
            let bytes = desert_rust::serialize_to_byte_vec(&invalid).unwrap();
            let decoded: RouteBehaviour = desert_rust::deserialize(&bytes).unwrap();
            assert!(
                RouteMatch::MountPrefix.validate(&[], &decoded).is_err(),
                "{path}"
            );
            let encoded: proto::golem::customapi::CompiledRoute = route(invalid).into();
            assert!(CompiledRoute::try_from(encoded).is_err(), "{path}");
        }

        let valid: proto::golem::customapi::CompiledRoute = route(router()).into();
        for duplicate_mapping in [true, false] {
            let mut invalid = valid.clone();
            let Some(proto::golem::customapi::route_behaviour::Kind::Mount(mount)) =
                invalid.behavior.as_mut().unwrap().kind.as_mut()
            else {
                unreachable!()
            };
            if duplicate_mapping {
                mount.file_bindings.push(mount.file_bindings[0].clone());
            } else {
                let Some(
                    proto::golem::customapi::route_behaviour::mount_file_source::Kind::Router(
                        router,
                    ),
                ) = mount.files.as_mut().unwrap().kind.as_mut()
                else {
                    unreachable!()
                };
                router.file_index.push(router.file_index[0].clone());
            }
            assert!(CompiledRoute::try_from(invalid).is_err());
        }
        for length in [31, 33] {
            let mut invalid = valid.clone();
            let Some(proto::golem::customapi::route_behaviour::Kind::Mount(mount)) =
                invalid.behavior.as_mut().unwrap().kind.as_mut()
            else {
                unreachable!()
            };
            let Some(proto::golem::customapi::route_behaviour::mount_file_source::Kind::Router(
                router,
            )) = mount.files.as_mut().unwrap().kind.as_mut()
            else {
                unreachable!()
            };
            router.file_index[0].blob_key.resize(length, 0);
            assert!(CompiledRoute::try_from(invalid).is_err());
        }
        let any: proto::golem::customapi::RouteMatch =
            RouteMatch::from(HttpMethod::Any(golem_schema::Empty {})).into();
        assert!(RouteMatch::try_from(any).is_err());

        for segment in [
            PathSegment::Variable {
                display_name: "id".into(),
            },
            PathSegment::CatchAll {
                display_name: "rest".into(),
            },
        ] {
            assert!(
                RouteMatch::MountPrefix
                    .validate(&[segment], &router())
                    .is_err()
            );
        }
        let trailing = RouteMatch::Method {
            method: HttpMethod::Method(golem_schema::http::HttpMethod::Get(Default::default())),
            trailing_slash: true,
        };
        assert!(
            trailing
                .validate(
                    &[],
                    &RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                        format: OpenApiSpecFormat::Json,
                        scheme: HttpApiDeploymentScheme::Https,
                    })
                )
                .is_err()
        );
    }

    #[test]
    fn mounted_dispatch_rejects_constructor_schema_root_mismatch() {
        let mut invalid = router();
        let RouteBehaviour::Mount(MountBehaviour {
            files: MountFileSource::Router { agent, .. },
            ..
        }) = &mut invalid
        else {
            unreachable!()
        };
        agent.constructor_input.graph = SchemaGraph::anonymous(SchemaType::string());

        let encoded: proto::golem::customapi::CompiledRoute = route(invalid).into();
        assert!(
            CompiledRoute::try_from(encoded).is_err(),
            "constructor input graph root must describe the complete positional input"
        );
    }

    #[test]
    fn mounted_dispatch_rejects_method_schema_root_mismatch() {
        for corrupt_input in [true, false] {
            let mut invalid = router();
            let RouteBehaviour::Mount(mount) = &mut invalid else {
                unreachable!()
            };
            let MountTarget::AgentMethod(target) = mount.openapi_provider.as_mut().unwrap();
            let method = &mut target.method;
            if corrupt_input {
                method.input.graph = SchemaGraph::anonymous(SchemaType::string());
            } else {
                method.output.graph = SchemaGraph::anonymous(SchemaType::u64());
            }

            let encoded: proto::golem::customapi::CompiledRoute = route(invalid).into();
            assert!(CompiledRoute::try_from(encoded).is_err());
        }
    }
}
