// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::{CompiledRoute, CompiledRoutes};
use super::{CorsOptions, SecuritySchemeDetails};
use super::{PathSegment, PathSegmentType, RequestBodySchema, RouteBehaviour};
use crate::custom_api::{
    CallAgentBehaviour, ConstructorParameter, CorsPreflightBehaviour, MethodParameter,
    OriginPattern, QueryOrHeaderType, WebhookCallbackBehaviour,
};
use golem_api_grpc::proto;
use golem_common::model::agent::{AgentTypeName, HttpMethod};
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_wasm::analysis::TypeEnum;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::collections::{BTreeSet, HashMap};
use std::ops::Deref;

impl TryFrom<proto::golem::customapi::SecuritySchemeDetails> for SecuritySchemeDetails {
    type Error = String;

    fn try_from(
        value: proto::golem::customapi::SecuritySchemeDetails,
    ) -> Result<Self, Self::Error> {
        let id = value.id.ok_or("id field missing")?.try_into()?;

        let provider_type = value
            .provider()
            .try_into()
            .map_err(|e| format!("invalid provider: {e}"))?;

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
            provider: golem_api_grpc::proto::golem::registry::SecuritySchemeProvider::from(
                value.provider_type,
            )
            .into(),
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
        Ok(Self {
            route_id: value.route_id,
            method: value.method.ok_or("Missing method")?.try_into()?,
            path: value
                .path
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            body: value.body.ok_or("Missing body")?.try_into()?,
            behavior: value.behavior.ok_or("Missing behavior")?.try_into()?,
            security_scheme: value.security_scheme.map(TryInto::try_into).transpose()?,
            cors: value.cors.ok_or("Missing cors")?.try_into()?,
        })
    }
}

impl From<CompiledRoute> for proto::golem::customapi::CompiledRoute {
    fn from(value: CompiledRoute) -> Self {
        Self {
            route_id: value.route_id,
            method: Some(value.method.into()),
            path: value.path.into_iter().map(Into::into).collect(),
            body: Some(value.body.into()),
            behavior: Some(value.behavior.into()),
            security_scheme: value.security_scheme.map(Into::into),
            cors: Some(value.cors.into()),
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
                constructor_parameters: call_agent
                    .constructor_parameters
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                phantom: call_agent.phantom,
                method_name: call_agent.method_name,
                method_parameters: call_agent
                    .method_parameters
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                expected_agent_response: call_agent
                    .expected_agent_response
                    .ok_or("Missing expected_agent_response")?
                    .try_into()?,
            })),
            Kind::CorsPreflight(cors_preflight) => {
                Ok(RouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
                    allowed_origins: cors_preflight
                        .allowed_origins
                        .into_iter()
                        .map(OriginPattern)
                        .collect(),
                    allowed_methods: cors_preflight
                        .allowed_methods
                        .into_iter()
                        .map(HttpMethod::try_from)
                        .collect::<Result<BTreeSet<_>, _>>()?,
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
        }
    }
}

impl From<RouteBehaviour> for proto::golem::customapi::RouteBehaviour {
    fn from(value: RouteBehaviour) -> Self {
        use proto::golem::customapi::route_behaviour::Kind;

        match value {
            RouteBehaviour::CallAgent(CallAgentBehaviour {
                component_id,
                component_revision,
                agent_type,
                constructor_parameters,
                phantom,
                method_name,
                method_parameters,
                expected_agent_response,
            }) => Self {
                kind: Some(Kind::CallAgent(
                    proto::golem::customapi::route_behaviour::CallAgent {
                        component_id: Some(component_id.into()),
                        component_revision: component_revision.into(),
                        agent_type: agent_type.0,
                        constructor_parameters: constructor_parameters
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                        phantom,
                        method_name,
                        method_parameters: method_parameters.into_iter().map(Into::into).collect(),
                        expected_agent_response: Some(expected_agent_response.into()),
                    },
                )),
            },
            RouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
                allowed_origins,
                allowed_methods,
            }) => Self {
                kind: Some(Kind::CorsPreflight(
                    proto::golem::customapi::route_behaviour::CorsPreflight {
                        allowed_origins: allowed_origins.into_iter().map(|ao| ao.0).collect(),
                        allowed_methods: allowed_methods
                            .into_iter()
                            .map(proto::golem::component::HttpMethod::from)
                            .collect(),
                    },
                )),
            },
            RouteBehaviour::WebhookCallback(WebhookCallbackBehaviour {
                component_id,
            }) => Self {
                kind: Some(Kind::WebhookCallback(
                    proto::golem::customapi::route_behaviour::WebhookCallback {
                        component_id: Some(component_id.into()),
                    },
                )),
            },
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
                expected_type: (&body.expected_type.ok_or("JsonBody.expected_type missing")?)
                    .try_into()?,
            }),

            Kind::UnrestrictedBinary(_) => Ok(RequestBodySchema::UnrestrictedBinary),

            Kind::RestrictedBinary(body) => Ok(RequestBodySchema::RestrictedBinary {
                allowed_mime_types: body.allowed_mime_types,
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

            RequestBodySchema::JsonBody { expected_type } => Self {
                kind: Some(Kind::JsonBody(
                    proto::golem::customapi::request_body_schema::JsonBody {
                        expected_type: Some((&expected_type).into()),
                    },
                )),
            },

            RequestBodySchema::UnrestrictedBinary => Self {
                kind: Some(Kind::UnrestrictedBinary(
                    proto::golem::customapi::request_body_schema::UnrestrictedBinary {},
                )),
            },

            RequestBodySchema::RestrictedBinary { allowed_mime_types } => Self {
                kind: Some(Kind::RestrictedBinary(
                    proto::golem::customapi::request_body_schema::RestrictedBinary {
                        allowed_mime_types,
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

                Ok(PathSegmentType::Enum(TypeEnum {
                    owner: inner.owner,
                    name: inner.name,
                    cases: inner.names,
                }))
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
                    inner: Some(golem_wasm::protobuf::TypeEnum {
                        owner: inner.owner,
                        name: inner.name,
                        names: inner.cases,
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
            Kind::Literal(lit) => Ok(PathSegment::Literal { value: lit.value }),

            Kind::Variable(_) => Ok(PathSegment::Variable),

            Kind::CatchAll(_) => Ok(PathSegment::CatchAll),
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

            PathSegment::Variable => Kind::Variable(golem_api_grpc::proto::golem::common::Empty {}),

            PathSegment::CatchAll => Kind::CatchAll(golem_api_grpc::proto::golem::common::Empty {}),
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
