// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

use crate::custom_api::openapi::core::{HttpApiRoute, RichCompiledRouteWithAgentType};
use crate::custom_api::openapi::response_schema::{
    ResponseBodyOpenApiSchema, get_agent_response_schema,
};
use crate::custom_api::openapi::schema_mapping::create_schema_from_analysed_type;
use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour};
use golem_common::base_model::agent::{AgentMethod, AgentType};
use golem_common::model::agent::{AgentConstructor, DataSchema, NamedElementSchema};
use golem_service_base::custom_api::{
    ConstructorParameter, MethodParameter, PathSegment, PathSegmentType, QueryOrHeaderType,
    RequestBodySchema, SecuritySchemeDetails,
};
use golem_service_base::model::SafeIndex;
use golem_wasm::analysis::AnalysedType;
use indexmap::IndexMap;
use openapiv3::{
    Components, Header, HeaderStyle, Info, MediaType, OpenAPI, Operation, Parameter, ParameterData,
    ParameterSchemaOrContent, PathItem, PathStyle, QueryStyle, ReferenceOr, RequestBody, Response,
    Schema, SecurityScheme, StatusCode,
};
use std::collections::BTreeMap;
use std::sync::Arc;

const GOLEM_DISABLED_EXTENSION: &str = "x-golem-disabled";

pub async fn generate_open_api_spec<'a>(
    spec_details: &[(&'a AgentType, &'a RichCompiledRoute)],
) -> Result<HttpApiOpenApiSpec, String> {
    let routes: Vec<_> = spec_details
        .iter()
        .map(|(agent_type, rich_route)| RichCompiledRouteWithAgentType {
            agent_type,
            details: rich_route,
        })
        .collect();

    HttpApiOpenApiSpec::from_routes(&routes)
}

pub struct HttpApiOpenApiSpec(pub OpenAPI);

impl HttpApiOpenApiSpec {
    pub fn from_routes<'a, T, I>(routes: I) -> Result<Self, String>
    where
        T: 'a + HttpApiRoute + ?Sized,
        I: IntoIterator<Item = &'a T>,
    {
        let routes_vec: Vec<&T> = routes.into_iter().collect();

        let security_scheme_details = get_security_scheme_details(&routes_vec);

        let mut open_api = create_base_openapi(&security_scheme_details);

        let mut paths = BTreeMap::new();

        for route in &routes_vec {
            add_route_to_paths(*route, &mut paths)?;
        }

        open_api.paths.paths = paths
            .into_iter()
            .map(|(k, v)| (k, ReferenceOr::Item(v)))
            .collect();

        Ok(HttpApiOpenApiSpec(open_api))
    }
}

fn get_security_scheme_details<T>(routes: &[&T]) -> Vec<Arc<SecuritySchemeDetails>>
where
    T: HttpApiRoute + ?Sized,
{
    let mut schemes = Vec::new();

    for route in routes {
        if let Some(scheme_details) = route.security_scheme() {
            schemes.push(scheme_details.clone());
        }
    }

    schemes
}

fn create_base_openapi(security_schemes: &Vec<Arc<SecuritySchemeDetails>>) -> OpenAPI {
    let mut open_api = OpenAPI {
        openapi: "3.0.0".to_string(),
        info: Info {
            title: "".to_string(),
            description: None,
            terms_of_service: None,
            contact: None,
            license: None,
            version: "".to_string(),
            extensions: Default::default(),
        },
        ..Default::default()
    };

    let mut components = Components::default();
    for security_scheme in security_schemes {
        let openid_config_url = format!(
            "{}/.well-known/openid-configuration",
            security_scheme.provider_type.issuer_url().url()
        );

        let scheme = SecurityScheme::OpenIDConnect {
            open_id_connect_url: openid_config_url,
            description: Some(format!(
                "OpenID Connect provider for {}",
                security_scheme.name
            )),
            extensions: Default::default(),
        };

        components
            .security_schemes
            .insert(security_scheme.name.0.clone(), ReferenceOr::Item(scheme));
    }

    open_api.components = Some(components);
    open_api
}

fn get_query_variable_and_types(
    method_params: Option<&Vec<MethodParameter>>,
) -> Vec<(&str, &QueryOrHeaderType)> {
    method_params
        .into_iter()
        .flat_map(|params| {
            params.iter().filter_map(|p| {
                if let MethodParameter::Query {
                    query_parameter_name,
                    parameter_type,
                } = p
                {
                    Some((query_parameter_name.as_str(), parameter_type))
                } else {
                    None
                }
            })
        })
        .collect()
}

fn get_header_variable_and_types(
    method_params: Option<&Vec<MethodParameter>>,
) -> Vec<(&str, &QueryOrHeaderType)> {
    method_params
        .into_iter()
        .flat_map(|params| {
            params.iter().filter_map(|p| {
                if let MethodParameter::Header {
                    header_name,
                    parameter_type,
                } = p
                {
                    Some((header_name.as_str(), parameter_type))
                } else {
                    None
                }
            })
        })
        .collect()
}

fn get_full_path_and_variables<'a>(
    agent_constructor: &'a AgentConstructor,
    agent_method: Option<&'a AgentMethod>,
    path_segments: &'a [PathSegment],
    constructor_parameter: Option<&'a Vec<ConstructorParameter>>,
    method_params: Option<&'a Vec<MethodParameter>>,
) -> (String, Vec<(&'a str, &'a PathSegmentType)>) {
    if path_segments
        .iter()
        .all(|s| matches!(s, PathSegment::Literal { .. }))
    {
        return (
            path_segments
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("/"),
            Vec::new(),
        );
    }

    let input_path_variable_types =
        collect_path_variable_types(constructor_parameter, method_params);
    let agent_input_params = collect_agent_input_params(agent_constructor, agent_method);
    let with_names = zip_params_with_path_variables(agent_input_params, input_path_variable_types);

    build_path_and_variables(path_segments, with_names)
}

fn collect_path_variable_types<'a>(
    constructor_parameter: Option<&'a Vec<ConstructorParameter>>,
    method_params: Option<&'a Vec<MethodParameter>>,
) -> Vec<(&'a SafeIndex, &'a PathSegmentType)> {
    let mut types = Vec::new();

    let constructor_types = constructor_parameter
        .map(|params| {
            params
                .iter()
                .map(|p| match p {
                    ConstructorParameter::Path {
                        parameter_type,
                        path_segment_index,
                    } => (path_segment_index, parameter_type),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let method_types = method_params
        .map(|params| {
            params
                .iter()
                .filter_map(|p| match p {
                    MethodParameter::Path {
                        parameter_type,
                        path_segment_index,
                    } => Some((path_segment_index, parameter_type)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    types.extend(constructor_types);
    types.extend(method_types);
    types
}

fn collect_agent_input_params<'a>(
    agent_constructor: &'a AgentConstructor,
    agent_method: Option<&'a AgentMethod>,
) -> Vec<&'a NamedElementSchema> {
    let mut inputs = match &agent_constructor.input_schema {
        DataSchema::Tuple(tuple) => tuple.elements.iter().collect(),
        DataSchema::Multimodal(_) => vec![],
    };

    if let Some(method) = agent_method {
        let method_inputs = match &method.input_schema {
            DataSchema::Tuple(tuple) => tuple.elements.iter().collect(),
            DataSchema::Multimodal(_) => vec![],
        };
        inputs.extend(method_inputs);
    }

    inputs
}

fn zip_params_with_path_variables<'a>(
    params: Vec<&'a NamedElementSchema>,
    path_variables: Vec<(&'a SafeIndex, &'a PathSegmentType)>,
) -> Vec<(&'a str, &'a SafeIndex, &'a PathSegmentType)> {
    params
        .iter()
        .zip(path_variables.iter().map(|(idx, ty)| (idx, ty)))
        .map(|(param, (idx, ty))| (param.name.as_str(), *idx, *ty))
        .collect()
}

fn build_path_and_variables<'a>(
    path_segments: &'a [PathSegment],
    with_names: Vec<(&'a str, &'a SafeIndex, &'a PathSegmentType)>,
) -> (String, Vec<(&'a str, &'a PathSegmentType)>) {
    let mut path_params_and_types = Vec::new();
    let mut path_segment_string = Vec::new();
    let mut path_variable_index = SafeIndex::new(0);

    for segment in path_segments.iter() {
        match segment {
            PathSegment::Literal { value } => path_segment_string.push(value.to_string()),

            PathSegment::Variable | PathSegment::CatchAll => {
                let (name, _, var_type) = with_names
                    .iter()
                    .find(|(_, index, _)| *index == &path_variable_index)
                    .expect("Failed to find path variable index in agent parameters");

                path_segment_string.push(format!("{{{}}}", name));
                path_params_and_types.push((*name, *var_type));
                path_variable_index += 1;
            }
        }
    }

    let full_path = format!("/{}", path_segment_string.join("/"));

    (full_path, path_params_and_types)
}

fn add_route_to_paths<T: HttpApiRoute + ?Sized>(
    route: &T,
    paths: &mut BTreeMap<String, PathItem>,
) -> Result<(), String> {
    let constructor_parameters = match route.binding() {
        RichRouteBehaviour::CallAgent(behaviour) => Some(&behaviour.constructor_parameters),
        _ => None,
    };

    let method_parameters = match route.binding() {
        RichRouteBehaviour::CallAgent(behaviour) => Some(&behaviour.method_parameters),
        _ => None,
    };

    let (path_str, path_params_raw) = get_full_path_and_variables(
        route.agent_constructor(),
        route.associated_agent_method(),
        route.path(),
        constructor_parameters,
        method_parameters,
    );

    let path_item = paths.entry(path_str).or_default();
    let mut operation = Operation::default();

    let query_params_raw = get_query_variable_and_types(method_parameters);
    let header_params_raw = get_header_variable_and_types(method_parameters);

    add_parameters(
        &mut operation,
        path_params_raw,
        query_params_raw,
        header_params_raw,
    );
    add_request_body(&mut operation, route.request_body_schema());
    add_responses(&mut operation, route);
    add_security(&mut operation, route);
    add_operation_to_path_item(path_item, route.method(), operation)?;

    Ok(())
}

struct ParameterSchema<'a> {
    name: &'a str,
    schema: Schema,
}
struct Parameters<'a> {
    path_params: Vec<ParameterSchema<'a>>,
    query_params: Vec<ParameterSchema<'a>>,
    header_params: Vec<ParameterSchema<'a>>,
}

fn add_parameters(
    operation: &mut Operation,
    path_params_raw: Vec<(&str, &PathSegmentType)>,
    query_params_raw: Vec<(&str, &QueryOrHeaderType)>,
    header_params_raw: Vec<(&str, &QueryOrHeaderType)>,
) {
    let params = get_parameters(path_params_raw, query_params_raw, header_params_raw);

    for ParameterSchema { name, schema } in params.path_params {
        operation
            .parameters
            .push(ReferenceOr::Item(create_path_parameter(name, schema)));
    }
    for ParameterSchema { name, schema } in params.query_params {
        operation
            .parameters
            .push(ReferenceOr::Item(create_query_parameter(name, schema)));
    }
    for ParameterSchema { name, schema } in params.header_params {
        operation
            .parameters
            .push(ReferenceOr::Item(create_header_parameter(name, schema)));
    }
}

fn create_schema_from_path_segment_type(path_segment_type: &PathSegmentType) -> Schema {
    let analysed_type = AnalysedType::from(path_segment_type);
    create_schema_from_analysed_type(&analysed_type)
}

fn create_schema_from_query_or_header_type(query_or_header_type: &QueryOrHeaderType) -> Schema {
    let analysed_type = AnalysedType::from(query_or_header_type.clone());
    create_schema_from_analysed_type(&analysed_type)
}

fn get_parameters<'a>(
    path_params_raw: Vec<(&'a str, &PathSegmentType)>,
    query_params_raw: Vec<(&'a str, &QueryOrHeaderType)>,
    header_params_raw: Vec<(&'a str, &QueryOrHeaderType)>,
) -> Parameters<'a> {
    let mut path_params = Vec::new();
    let mut query_params = Vec::new();
    let mut header_params = Vec::new();

    for (name, path_segment_type) in path_params_raw {
        let schema = create_schema_from_path_segment_type(path_segment_type);
        path_params.push(ParameterSchema { name, schema });
    }

    for (name, query_or_header_type) in query_params_raw {
        let schema = create_schema_from_query_or_header_type(query_or_header_type);
        query_params.push(ParameterSchema { name, schema });
    }

    for (name, query_or_header_type) in header_params_raw {
        let schema = create_schema_from_query_or_header_type(query_or_header_type);
        header_params.push(ParameterSchema { name, schema });
    }

    Parameters {
        path_params,
        query_params,
        header_params,
    }
}

fn create_path_parameter(name: &str, schema: Schema) -> Parameter {
    Parameter::Path {
        parameter_data: ParameterData {
            name: name.to_string(),
            description: Some(format!("Path parameter: {name}")),
            required: true,
            deprecated: None,
            explode: Some(false),
            format: ParameterSchemaOrContent::Schema(ReferenceOr::Item(schema)),
            example: None,
            examples: Default::default(),
            extensions: Default::default(),
        },
        style: PathStyle::Simple,
    }
}

fn create_query_parameter(name: &str, schema: Schema) -> Parameter {
    let required = !schema.schema_data.nullable;

    Parameter::Query {
        parameter_data: ParameterData {
            name: name.to_string(),
            description: Some(format!("Query parameter: {name}")),
            required,
            deprecated: None,
            explode: Some(false),
            format: ParameterSchemaOrContent::Schema(ReferenceOr::Item(schema)),
            example: None,
            examples: Default::default(),
            extensions: Default::default(),
        },
        style: QueryStyle::Form,
        allow_empty_value: Some(false),
        allow_reserved: false,
    }
}

fn create_header_parameter(name: &str, schema: Schema) -> Parameter {
    let required = !schema.schema_data.nullable;

    Parameter::Header {
        parameter_data: ParameterData {
            name: name.to_string(),
            description: Some(format!("Header parameter: {name}")),
            required,
            deprecated: None,
            explode: Some(false),
            format: ParameterSchemaOrContent::Schema(ReferenceOr::Item(schema)),
            example: None,
            examples: Default::default(),
            extensions: Default::default(),
        },
        style: HeaderStyle::Simple,
    }
}

fn add_request_body(operation: &mut Operation, request_body_schema: &RequestBodySchema) {
    let request_body = create_request_body(request_body_schema);

    if let Some(rb) = request_body {
        operation.request_body = Some(ReferenceOr::Item(rb));
    }
}

fn create_request_body(request_body_schema: &RequestBodySchema) -> Option<RequestBody> {
    match request_body_schema {
        RequestBodySchema::Unused => None,
        RequestBodySchema::JsonBody { expected_type } => {
            let schema = create_schema_from_analysed_type(expected_type);
            Some(RequestBody {
                description: Some("JSON body".to_string()),
                content: {
                    let mut content = IndexMap::new();
                    content.insert(
                        "application/json".to_string(),
                        MediaType {
                            schema: Some(ReferenceOr::Item(schema)),
                            ..Default::default()
                        },
                    );
                    content
                },
                required: true,
                extensions: Default::default(),
            })
        }

        RequestBodySchema::UnrestrictedBinary => Some(RequestBody {
            description: Some("Unrestricted binary body".to_string()),
            content: {
                let mut content = IndexMap::new();
                content.insert(
                    "*/*".to_string(),
                    MediaType {
                        ..Default::default()
                    },
                );
                content
            },
            required: true,
            extensions: Default::default(),
        }),

        RequestBodySchema::RestrictedBinary { allowed_mime_types } => {
            let mut content = IndexMap::new();
            for mime in allowed_mime_types {
                content.insert(
                    mime.clone(),
                    MediaType {
                        ..Default::default()
                    },
                );
            }
            Some(RequestBody {
                description: Some("Restricted binary body".to_string()),
                content,
                required: true,
                extensions: Default::default(),
            })
        }
    }
}

fn add_responses<T: HttpApiRoute + ?Sized>(operation: &mut Operation, route: &T) {
    let agent_response_schema = get_agent_response_schema(route);

    for (status_code, response_schema) in agent_response_schema.body_and_status_codes {
        let mut response = Response {
            description: format!("Response {}", status_code),
            ..Default::default()
        };

        match response_schema {
            ResponseBodyOpenApiSchema::Known {
                schema,
                content_type,
            } => {
                let mut content = IndexMap::new();
                content.insert(
                    content_type,
                    MediaType {
                        schema: Some(ReferenceOr::Item(*schema)),
                        ..Default::default()
                    },
                );
                response.content = content;
            }
            ResponseBodyOpenApiSchema::Unknown => {
                let mut content = IndexMap::new();
                content.insert(
                    "*/*".to_string(),
                    MediaType {
                        ..Default::default()
                    },
                );
                response.content = content;
            }
            ResponseBodyOpenApiSchema::NoBody => {
                response.content = IndexMap::new();
            }
        }

        let mut response_headers = IndexMap::new();

        for (name, schema) in agent_response_schema.headers.iter() {
            let description = format!("Response header: {}", name);
            response_headers.insert(
                name.clone(),
                ReferenceOr::Item(Header {
                    description: Some(description),
                    required: true,
                    deprecated: None,
                    format: ParameterSchemaOrContent::Schema(ReferenceOr::Item(schema.clone())),
                    example: None,
                    examples: Default::default(),
                    extensions: Default::default(),
                    style: HeaderStyle::Simple,
                }),
            );
        }
        response.headers = response_headers;

        operation
            .responses
            .responses
            .insert(StatusCode::Code(status_code), ReferenceOr::Item(response));
    }
}

fn add_security<T: HttpApiRoute + ?Sized>(operation: &mut Operation, route: &T) {
    if route.security_scheme_missing() {
        operation.extensions.insert(
            GOLEM_DISABLED_EXTENSION.to_string(),
            serde_json::json!({ "reason": "security_scheme_missing" }),
        );
        return;
    }

    if let Some(security_schema_details) = route.security_scheme() {
        let scopes_vec: Vec<String> = security_schema_details
            .scopes
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut requirement: indexmap::IndexMap<String, Vec<String>> = indexmap::IndexMap::new();
        requirement.insert(security_schema_details.name.0.clone(), scopes_vec);

        operation.security = Some(vec![requirement]);
    }
}

fn add_operation_to_path_item(
    path_item: &mut PathItem,
    method: &str,
    operation: Operation,
) -> Result<(), String> {
    match method {
        "GET" => path_item.get = Some(operation),
        "POST" => path_item.post = Some(operation),
        "PUT" => path_item.put = Some(operation),
        "DELETE" => path_item.delete = Some(operation),
        "PATCH" => path_item.patch = Some(operation),
        "OPTIONS" => path_item.options = Some(operation),
        "HEAD" => path_item.head = Some(operation),
        "TRACE" => path_item.trace = Some(operation),
        _ => {}
    }
    Ok(())
}
