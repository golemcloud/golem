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
    AgentFilesystemBehaviour, CallAgentBehaviour, CompiledInputSchema, CompiledOutputSchema,
    CompiledSchema, ConstructorParameter, CorsPreflightBehaviour, CorsPreflightMethodPolicy,
    HttpRouterBehaviour, MethodParameter, OriginPattern, QueryOrHeaderType, RouteMatch,
    RouterFileIndexEntry, RouterMethod, SecuritySchemeRouteSecurity,
    SessionFromHeaderRouteSecurity, WebhookCallbackBehaviour,
};
use golem_api_grpc::proto;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::security_scheme::{Provider, SecuritySchemeName};
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
            name: SecuritySchemeName(value.name),
            provider_type,
            client_id: ClientId::new(value.client_id),
            client_secret: ClientSecret::new(value.client_secret),
            redirect_url: RedirectUrl::new(value.redirect_url)
                .map_err(|e| format!("Failed parsing redirect url: {e}"))?,
            scopes: value.scopes.into_iter().map(Scope::new).collect(),
        })
    }
}

impl From<SecuritySchemeDetails>
    for golem_api_grpc::proto::golem::customapi::SecuritySchemeDetails
{
    fn from(value: SecuritySchemeDetails) -> Self {
        Self {
            id: Some(value.id.into()),
            name: value.name.0,
            provider: Some(value.provider_type.into()),
            client_id: value.client_id.deref().clone(),
            client_secret: value.client_secret.secret().clone(),
            redirect_url: value.redirect_url.deref().clone(),
            scopes: value.scopes.iter().map(|s| s.deref().clone()).collect(),
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
            body: value.body.ok_or("Missing body")?.try_into()?,
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
            body: Some(value.body.into()),
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
                phantom: call_agent.phantom,
                method_name: call_agent.method_name,
                method_input: call_agent
                    .method_input
                    .ok_or("Missing method_input")?
                    .try_into()?,
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
                use proto::golem::customapi::route_behaviour::open_api_spec::Format;

                let format = Format::try_from(open_api_spec.format)
                    .map_err(|_| "Invalid OpenApiSpec.format".to_string())?;

                let format = match format {
                    Format::Unspecified => {
                        return Err("OpenApiSpec.format missing".to_string());
                    }
                    Format::Json => OpenApiSpecFormat::Json,
                    Format::Yaml => OpenApiSpecFormat::Yaml,
                };

                Ok(RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour { format }))
            }
            Kind::HttpRouter(value) => Ok(RouteBehaviour::HttpRouter(HttpRouterBehaviour {
                component_id: value
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                component_revision: value.component_revision.try_into()?,
                agent_type: AgentTypeName(value.agent_type),
                constructor_input: value
                    .constructor_input
                    .ok_or("Missing constructor_input")?
                    .try_into()?,
                handler: value.handler.map(TryInto::try_into).transpose()?,
                openapi_provider: value.openapi_provider.map(TryInto::try_into).transpose()?,
                static_bindings: value
                    .static_bindings
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                file_index: decode_file_index(value.file_index)?,
            })),
            Kind::AgentFilesystem(value) => {
                Ok(RouteBehaviour::AgentFilesystem(AgentFilesystemBehaviour {
                    component_id: value
                        .component_id
                        .ok_or("Missing component_id")?
                        .try_into()?,
                    component_revision: value.component_revision.try_into()?,
                    agent_type: AgentTypeName(value.agent_type),
                    constructor_input: value
                        .constructor_input
                        .ok_or("Missing constructor_input")?
                        .try_into()?,
                    constructor_parameters: value
                        .constructor_parameters
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    filesystem_bindings: value
                        .filesystem_bindings
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                }))
            }
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
            let sha256 = value
                .sha256
                .try_into()
                .map_err(|_| "Router file sha256 must be exactly 32 bytes")?;
            Ok(RouterFileIndexEntry {
                path: value.path,
                blob_key: golem_common::model::agent::AgentFileContentHash(
                    golem_common::model::diff::Hash::from(blake3::Hash::from_bytes(blob_key)),
                ),
                size: value.size,
                sha256,
            })
        })
        .collect()
}

impl From<RouteBehaviour> for proto::golem::customapi::RouteBehaviour {
    fn from(value: RouteBehaviour) -> Self {
        use proto::golem::customapi::route_behaviour::Kind;

        match value {
            RouteBehaviour::CallAgent(CallAgentBehaviour {
                component_id,
                component_revision,
                agent_type,
                agent_mode,
                constructor_input,
                constructor_parameters,
                phantom,
                method_name,
                method_input,
                method_parameters,
                expected_agent_response,
                method_description,
                read_only,
            }) => Self {
                kind: Some(Kind::CallAgent(
                    proto::golem::customapi::route_behaviour::CallAgent {
                        component_id: Some(component_id.into()),
                        component_revision: component_revision.into(),
                        agent_type: agent_type.0,
                        agent_mode: proto::golem::component::AgentMode::from(agent_mode) as i32,
                        constructor_input: Some(constructor_input.into()),
                        constructor_parameters: constructor_parameters
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                        phantom,
                        method_name,
                        method_input: Some(method_input.into()),
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
            RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour { format }) => {
                use proto::golem::customapi::route_behaviour::open_api_spec::Format;

                Self {
                    kind: Some(Kind::OpenApiSpec(
                        proto::golem::customapi::route_behaviour::OpenApiSpec {
                            format: match format {
                                OpenApiSpecFormat::Json => Format::Json,
                                OpenApiSpecFormat::Yaml => Format::Yaml,
                            }
                            .into(),
                        },
                    )),
                }
            }
            RouteBehaviour::HttpRouter(value) => Self { kind: Some(Kind::HttpRouter(
                proto::golem::customapi::route_behaviour::HttpRouter {
                    component_id: Some(value.component_id.into()), component_revision: value.component_revision.into(),
                    agent_type: value.agent_type.0, constructor_input: Some(value.constructor_input.into()),
                    handler: value.handler.map(Into::into), openapi_provider: value.openapi_provider.map(Into::into),
                    static_bindings: value.static_bindings.into_iter().map(Into::into).collect(),
                    file_index: value.file_index.into_iter().map(|entry| proto::golem::customapi::route_behaviour::RouterFileIndexEntry {
                        path: entry.path, blob_key: entry.blob_key.0.as_blake3_hash().as_bytes().to_vec(), size: entry.size, sha256: entry.sha256.to_vec()
                    }).collect(),
                })) },
            RouteBehaviour::AgentFilesystem(value) => Self { kind: Some(Kind::AgentFilesystem(
                proto::golem::customapi::route_behaviour::AgentFilesystem {
                    component_id: Some(value.component_id.into()), component_revision: value.component_revision.into(),
                    agent_type: value.agent_type.0, constructor_input: Some(value.constructor_input.into()),
                    constructor_parameters: value.constructor_parameters.into_iter().map(Into::into).collect(),
                    filesystem_bindings: value.filesystem_bindings.into_iter().map(Into::into).collect(),
                })) },
        }
    }
}

impl TryFrom<proto::golem::customapi::RouteSecurity> for RouteSecurity {
    type Error = String;

    fn try_from(value: proto::golem::customapi::RouteSecurity) -> Result<Self, Self::Error> {
        use proto::golem::customapi::route_security::Kind;

        match value.kind.ok_or("RouteSecurity.kind missing")? {
            Kind::None(_) => Ok(RouteSecurity::None),
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
                inner: Box::new(opt.inner.ok_or("Option.inner missing")?.try_into()?),
            }),

            Kind::List(list) => Ok(QueryOrHeaderType::List {
                name: list.name,
                owner: list.owner,
                inner: Box::new(list.inner.ok_or("List.inner missing")?.try_into()?),
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

            QueryOrHeaderType::Option { name, owner, inner } => {
                Kind::Option(proto::golem::customapi::query_or_header_type::Option {
                    name,
                    owner,
                    inner: Some((*inner).into()),
                })
            }

            QueryOrHeaderType::List { name, owner, inner } => {
                Kind::List(proto::golem::customapi::query_or_header_type::List {
                    name,
                    owner,
                    inner: Some((*inner).into()),
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
    use golem_common::model::agent::{AgentFileContentHash, FileMapping, HttpMethod};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::schema::{InputSchema, OutputSchema, SchemaGraph, SchemaType};
    use test_r::test;

    fn input() -> CompiledInputSchema {
        CompiledInputSchema {
            graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
            input_schema: InputSchema::Parameters(vec![]),
        }
    }

    fn router() -> RouteBehaviour {
        RouteBehaviour::HttpRouter(HttpRouterBehaviour {
            component_id: ComponentId(uuid::Uuid::from_u128(17)),
            component_revision: ComponentRevision::try_from(23u64).unwrap(),
            agent_type: AgentTypeName("site".into()),
            constructor_input: input(),
            handler: None,
            openapi_provider: Some(RouterMethod {
                method_name: "schema".into(),
                input: input(),
                output: CompiledOutputSchema {
                    graph: SchemaGraph::anonymous(SchemaType::string()),
                    output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
                },
            }),
            static_bindings: FileMapping::compile_list([
                ("/favicon", "/assets/icon"),
                ("/*", "/assets/$1"),
            ])
            .unwrap(),
            file_index: vec![RouterFileIndexEntry {
                path: "/assets/%2e$icon".into(),
                blob_key: AgentFileContentHash(golem_common::model::diff::Hash::from(
                    blake3::hash(b"blob"),
                )),
                size: 4_294_967_301,
                sha256: [37; 32],
            }],
        })
    }

    fn route(behavior: RouteBehaviour) -> CompiledRoute {
        let mut path = vec![PathSegment::Literal {
            value: "site".into(),
        }];
        if matches!(behavior, RouteBehaviour::AgentFilesystem(_)) {
            path.push(PathSegment::Variable {
                display_name: "id".into(),
            });
        }
        CompiledRoute {
            route_id: 19,
            route_match: RouteMatch::MountPrefix,
            path,
            body: RequestBodySchema::Unused,
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
    fn mounted_dispatch_binary_and_protobuf_roundtrip() {
        let filesystem = RouteBehaviour::AgentFilesystem(AgentFilesystemBehaviour {
            component_id: ComponentId(uuid::Uuid::from_u128(42)),
            component_revision: ComponentRevision::try_from(31u64).unwrap(),
            agent_type: AgentTypeName("files".into()),
            constructor_input: input(),
            constructor_parameters: vec![ConstructorParameter::Path {
                path_segment_index: 0u32.into(),
                parameter_type: PathSegmentType::U64,
            }],
            filesystem_bindings: FileMapping::compile_list([
                ("/*", "/public/$1"),
                ("/fallback", "/fallback"),
            ])
            .unwrap(),
        });
        let mut filesystem_route = route(filesystem);
        filesystem_route
            .route_match
            .validate(&filesystem_route.path, &filesystem_route.behavior)
            .unwrap();
        let RouteBehaviour::AgentFilesystem(filesystem) = &mut filesystem_route.behavior else {
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
        let RouteBehaviour::AgentFilesystem(filesystem) = &mut filesystem_route.behavior else {
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
        let RouteBehaviour::AgentFilesystem(filesystem) = &mut filesystem_route.behavior else {
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
                method: HttpMethod::Custom(golem_common::model::agent::CustomHttpMethod {
                    value: "ANY".into(),
                }),
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
        invalid.route_match = HttpMethod::Get(golem_common::model::Empty {}).into();
        let encoded: proto::golem::customapi::CompiledRoute = invalid.into();
        assert!(CompiledRoute::try_from(encoded).is_err());
        let encoded: proto::golem::customapi::CompiledRoute =
            route(RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                format: OpenApiSpecFormat::Json,
            }))
            .into();
        assert!(CompiledRoute::try_from(encoded).is_err());

        for path in [
            "/", "/assets/", "relative", "/a//b", "/a/../b", "/a\\b", "/a\u{7f}",
        ] {
            let mut invalid = router();
            let RouteBehaviour::HttpRouter(router) = &mut invalid else {
                unreachable!()
            };
            router.file_index[0].path = path.into();
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
            let Some(proto::golem::customapi::route_behaviour::Kind::HttpRouter(router)) =
                invalid.behavior.as_mut().unwrap().kind.as_mut()
            else {
                unreachable!()
            };
            if duplicate_mapping {
                router
                    .static_bindings
                    .push(router.static_bindings[0].clone());
            } else {
                router.file_index.push(router.file_index[0].clone());
            }
            assert!(CompiledRoute::try_from(invalid).is_err());
        }
        for hash in [true, false] {
            let mut invalid = valid.clone();
            let Some(proto::golem::customapi::route_behaviour::Kind::HttpRouter(router)) =
                invalid.behavior.as_mut().unwrap().kind.as_mut()
            else {
                unreachable!()
            };
            if hash {
                router.file_index[0].sha256.pop();
            } else {
                router.file_index[0].blob_key.push(0);
            }
            assert!(CompiledRoute::try_from(invalid).is_err());
        }
        let any: proto::golem::customapi::RouteMatch =
            RouteMatch::from(HttpMethod::Any(golem_common::model::Empty {})).into();
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
            method: HttpMethod::Get(golem_common::model::Empty {}),
            trailing_slash: true,
        };
        assert!(
            trailing
                .validate(
                    &[],
                    &RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                        format: OpenApiSpecFormat::Json
                    })
                )
                .is_err()
        );
    }

    #[test]
    fn mounted_dispatch_rejects_constructor_schema_root_mismatch() {
        let mut invalid = router();
        let RouteBehaviour::HttpRouter(router) = &mut invalid else {
            unreachable!()
        };
        router.constructor_input.graph = SchemaGraph::anonymous(SchemaType::string());

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
            let RouteBehaviour::HttpRouter(router) = &mut invalid else {
                unreachable!()
            };
            let method = router.openapi_provider.as_mut().unwrap();
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
