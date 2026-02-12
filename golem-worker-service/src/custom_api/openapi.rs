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

use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour};
use golem_common::base_model::agent::{AgentMethod, AgentType, ElementSchema};
use golem_common::model::agent::DataSchema;
use golem_common::model::security_scheme::SecuritySchemeId;
use golem_service_base::custom_api::{
    MethodParameter, PathSegment, PathSegmentType, QueryOrHeaderType, RequestBodySchema,
    RouteBehaviour, SecuritySchemeDetails,
};
use golem_service_base::model::SafeIndex;
use golem_wasm::analysis::{AnalysedType, NameTypePair};
use indexmap::IndexMap;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const GOLEM_DISABLED_EXTENSION: &str = "x-golem-disabled";

pub trait HttpApiRoute {
    fn security_scheme_missing(&self) -> bool;
    fn security_scheme(&self) -> &Option<Arc<SecuritySchemeDetails>>;
    fn method(&self) -> String;
    fn path(&self) -> &Vec<PathSegment>;
    fn binding(&self) -> &RichRouteBehaviour;
    fn request_body_schema(&self) -> &RequestBodySchema;
    fn associated_agent_method(&self) -> Option<&AgentMethod>;
}

pub struct RouteWithAgentType {
    pub agent_type: AgentType,
    pub details: RichCompiledRoute,
}

impl HttpApiRoute for RouteWithAgentType {
    fn security_scheme_missing(&self) -> bool {
        false
    }
    fn security_scheme(&self) -> &Option<Arc<SecuritySchemeDetails>> {
        &self.details.security_scheme
    }
    fn method(&self) -> String {
        self.details.method.to_string()
    }
    fn path(&self) -> &Vec<PathSegment> {
        &self.details.path
    }
    fn binding(&self) -> &RichRouteBehaviour {
        &self.details.behavior
    }
    fn request_body_schema(&self) -> &RequestBodySchema {
        &self.details.body
    }

    fn associated_agent_method(&self) -> Option<&AgentMethod> {
        match &self.details.behavior {
            RichRouteBehaviour::CallAgent(call_agent_behaviour) => {
                let method_name = &call_agent_behaviour.method_name;
                self.agent_type
                    .methods
                    .iter()
                    .find(|m| m.name == *method_name)
            }
            _ => None,
        }
    }
}

pub struct HttpApiDefinitionOpenApiSpec(pub openapiv3::OpenAPI);

impl HttpApiDefinitionOpenApiSpec {
    pub async fn from_routes<'a, T, I>(
        routes: I,
        security_schemes: &HashMap<SecuritySchemeId, SecuritySchemeDetails>,
    ) -> Result<Self, String>
    where
        T: 'a + HttpApiRoute + ?Sized,
        I: IntoIterator<Item = &'a T>,
    {
        let mut open_api = create_base_openapi(
            "foo", // TODO;
            "bar", // TODO;
            security_schemes,
        );

        let mut paths = BTreeMap::new();
        for route in routes {
            add_route_to_paths(route, &mut paths, security_schemes).await?;
        }

        open_api.paths.paths = paths
            .into_iter()
            .map(|(k, v)| (k, openapiv3::ReferenceOr::Item(v)))
            .collect();

        Ok(HttpApiDefinitionOpenApiSpec(open_api))
    }
}

fn create_base_openapi(
    http_api_definition_name: &str,
    http_api_definition_version: &str,
    security_schemes: &HashMap<SecuritySchemeId, SecuritySchemeDetails>,
) -> openapiv3::OpenAPI {
    let mut open_api = openapiv3::OpenAPI {
        openapi: "3.0.0".to_string(),
        info: openapiv3::Info {
            title: http_api_definition_name.to_string(),
            description: None,
            terms_of_service: None,
            contact: None,
            license: None,
            version: http_api_definition_version.to_string(),
            extensions: Default::default(),
        },
        ..Default::default()
    };

    let mut components = openapiv3::Components::default();
    for security_scheme in security_schemes.values() {
        let openid_config_url = format!(
            "{}/.well-known/openid-configuration",
            security_scheme.provider_type.issuer_url().url()
        );

        let scheme = openapiv3::SecurityScheme::OpenIDConnect {
            open_id_connect_url: openid_config_url,
            description: Some(format!(
                "OpenID Connect provider for {}",
                security_scheme.name
            )),
            extensions: Default::default(),
        };

        components.security_schemes.insert(
            security_scheme.name.0.clone(),
            openapiv3::ReferenceOr::Item(scheme),
        );
    }

    open_api.components = Some(openapiv3::Components::default());
    open_api
}

fn get_query_variable_and_types(
    method_params: Option<&Vec<MethodParameter>>,
) -> Vec<(String, &QueryOrHeaderType)> {
    method_params
        .map(|params| {
            params
                .iter()
                .filter_map(|p| match p {
                    MethodParameter::Query {
                        query_parameter_name,
                        parameter_type,
                    } => Some((query_parameter_name.clone(), parameter_type)),
                    MethodParameter::Path { .. } => None,
                    MethodParameter::Header { .. } => None,
                    MethodParameter::JsonObjectBodyField { .. } => None,
                    MethodParameter::UnstructuredBinaryBody => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn get_header_variable_and_types(
    method_params: Option<&Vec<MethodParameter>>,
) -> Vec<(String, &QueryOrHeaderType)> {
    method_params
        .map(|params| {
            params
                .iter()
                .filter_map(|p| match p {
                    MethodParameter::Header {
                        header_name,
                        parameter_type,
                    } => Some((header_name.clone(), parameter_type)),
                    MethodParameter::Path { .. } => None,
                    MethodParameter::Query { .. } => None,
                    MethodParameter::JsonObjectBodyField { .. } => None,
                    MethodParameter::UnstructuredBinaryBody => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn get_full_path_and_variables(
    agent_method: Option<&AgentMethod>,
    path_segments: &Vec<PathSegment>,
    method_params: Option<&Vec<MethodParameter>>,
) -> (String, Vec<(String, PathSegmentType)>) {
    if (agent_method.is_none()) {
        return (
            path_segments
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join("/"),
            Vec::new(),
        );
    }
    let method_path_indices_vec: Vec<(SafeIndex, &PathSegmentType)> = method_params
        .map(|params| {
            params
                .iter()
                .filter_map(|p| match p {
                    MethodParameter::Path {
                        path_segment_index,
                        parameter_type,
                    } => Some((*path_segment_index, parameter_type)),
                    MethodParameter::Query { .. } => None,
                    MethodParameter::Header { .. } => None,
                    MethodParameter::JsonObjectBodyField { .. } => None,
                    MethodParameter::UnstructuredBinaryBody => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let method_path_indices: HashMap<SafeIndex, &PathSegmentType> =
        method_path_indices_vec.into_iter().collect();

    let input_parameter_names: Option<Vec<String>> =
        agent_method.and_then(|x| match &x.input_schema {
            DataSchema::Tuple(named_element_schemas) => Some(
                named_element_schemas
                    .elements
                    .iter()
                    .map(|e| e.name.clone())
                    .collect(),
            ),

            DataSchema::Multimodal(_) => None,
        });

    let mut path_params_and_types: Vec<(String, PathSegmentType)> = Vec::new();

    let mut path_segment_string: Vec<String> = Vec::new();

    for (idx, segment) in path_segments.iter().enumerate() {
        match segment {
            PathSegment::Literal { value } => {
                path_segment_string.push(value.to_string());
                // No parameter to extract, just a literal segment
            }
            PathSegment::Variable => {
                // This segment is a parameter, we need to find its type
                if let Some(param_type) = method_path_indices.get(&SafeIndex::new(idx as u32)) {
                    let name = if let Some(input_names) = &input_parameter_names {
                        input_names.get(idx).cloned().unwrap()
                    } else {
                        panic!("Variable segment must have a parameter name in the input schema");
                    };

                    path_segment_string.push(name.clone());

                    path_params_and_types.push((name.clone(), param_type.clone().clone()));
                }
            }

            PathSegment::CatchAll => {
                if let Some(param_type) = method_path_indices.get(&SafeIndex::new(idx as u32)) {
                    let name = if let Some(input_names) = &input_parameter_names {
                        input_names.get(idx).cloned().unwrap()
                    } else {
                        panic!("Catch-all segment must have a parameter name in the input schema");
                    };

                    path_segment_string.push(name.clone());

                    path_params_and_types.push((name.clone(), param_type.clone().clone()));
                }
            }
        }
    }

    (
        path_segment_string
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("/"),
        path_params_and_types,
    )
}

async fn add_route_to_paths<T: HttpApiRoute + ?Sized>(
    route: &T,
    paths: &mut BTreeMap<String, openapiv3::PathItem>,
    security_schemes: &HashMap<SecuritySchemeId, SecuritySchemeDetails>,
) -> Result<(), String> {
    let agent_method = route.associated_agent_method();
    let method_params = match route.binding() {
        RichRouteBehaviour::CallAgent(call_agent_behaviour) => {
            Some(&call_agent_behaviour.method_parameters)
        }
        RichRouteBehaviour::CorsPreflight(_) => None,
        RichRouteBehaviour::WebhookCallback(_) => None,
        RichRouteBehaviour::OpenApiSpec(_) => None,
        RichRouteBehaviour::OidcCallback(_) => None,
    };

    let (path_str, path_params_raw) =
        get_full_path_and_variables(agent_method, route.path(), method_params);

    let path_item = paths.entry(path_str.clone()).or_default();

    let mut operation = openapiv3::Operation::default();

    let query_params_raw = get_query_variable_and_types(method_params);

    let header_params_raw = get_header_variable_and_types(method_params);

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

type ParameterTuple = (String, openapiv3::Schema);

fn add_parameters(
    operation: &mut openapiv3::Operation,
    path_params_raw: Vec<(String, PathSegmentType)>,
    query_params_raw: Vec<(String, &QueryOrHeaderType)>,
    header_params_raw: Vec<(String, &QueryOrHeaderType)>,
) {
    let (path_params, query_params, header_params) =
        get_parameters(path_params_raw, query_params_raw, header_params_raw);

    for (name, schema) in path_params {
        operation
            .parameters
            .push(openapiv3::ReferenceOr::Item(create_path_parameter(
                &name, schema,
            )));
    }
    for (name, schema) in query_params {
        operation
            .parameters
            .push(openapiv3::ReferenceOr::Item(create_query_parameter(
                &name, schema,
            )));
    }
    for (name, schema) in header_params {
        operation
            .parameters
            .push(openapiv3::ReferenceOr::Item(create_header_parameter(
                &name, schema,
            )));
    }
}

fn create_schema_from_path_segment_type(path_segment_type: PathSegmentType) -> openapiv3::Schema {
    let analysed_type = AnalysedType::from(path_segment_type);
    create_schema_from_analysed_type(&analysed_type)
}

fn create_schema_from_query_or_header_type(
    query_or_header_type: &QueryOrHeaderType,
) -> openapiv3::Schema {
    let analysed_type = AnalysedType::from(query_or_header_type.clone());
    create_schema_from_analysed_type(&analysed_type)
}

fn get_parameters(
    path_params_raw: Vec<(String, PathSegmentType)>,
    query_params_raw: Vec<(String, &QueryOrHeaderType)>,
    header_params_raw: Vec<(String, &QueryOrHeaderType)>,
) -> (
    Vec<ParameterTuple>,
    Vec<ParameterTuple>,
    Vec<ParameterTuple>,
) {
    let mut path_params = Vec::new();
    let mut query_params = Vec::new();
    let mut header_params = Vec::new();

    for (name, path_segment_type) in path_params_raw.iter() {
        let schema = create_schema_from_path_segment_type(path_segment_type.clone());
        path_params.push((name.clone(), schema));
    }

    for (name, query_or_header_type) in query_params_raw.iter() {
        let schema = create_schema_from_query_or_header_type(query_or_header_type);
        query_params.push((name.clone(), schema));
    }

    for (name, query_or_header_type) in header_params_raw.iter() {
        let schema = create_schema_from_query_or_header_type(query_or_header_type);
        header_params.push((name.clone(), schema));
    }

    (path_params, query_params, header_params)
}

fn create_path_parameter(name: &str, schema: openapiv3::Schema) -> openapiv3::Parameter {
    openapiv3::Parameter::Path {
        parameter_data: openapiv3::ParameterData {
            name: name.to_string(),
            description: Some(format!("Path parameter: {name}")),
            required: true,
            deprecated: None,
            explode: Some(false),
            format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(
                schema,
            )),
            example: None,
            examples: Default::default(),
            extensions: Default::default(),
        },
        style: openapiv3::PathStyle::Simple,
    }
}

fn create_query_parameter(name: &str, schema: openapiv3::Schema) -> openapiv3::Parameter {
    openapiv3::Parameter::Query {
        parameter_data: openapiv3::ParameterData {
            name: name.to_string(),
            description: Some(format!("Query parameter: {name}")),
            required: true,
            deprecated: None,
            explode: Some(false),
            format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(
                schema,
            )),
            example: None,
            examples: Default::default(),
            extensions: Default::default(),
        },
        style: openapiv3::QueryStyle::Form,
        allow_empty_value: Some(false),
        allow_reserved: false,
    }
}

fn create_header_parameter(name: &str, schema: openapiv3::Schema) -> openapiv3::Parameter {
    openapiv3::Parameter::Header {
        parameter_data: openapiv3::ParameterData {
            name: name.to_string(),
            description: Some(format!("Header parameter: {name}")),
            required: true,
            deprecated: None,
            explode: Some(false),
            format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(
                schema,
            )),
            example: None,
            examples: Default::default(),
            extensions: Default::default(),
        },
        style: openapiv3::HeaderStyle::Simple,
    }
}

fn add_request_body(operation: &mut openapiv3::Operation, request_body_schema: &RequestBodySchema) {
    let request_body = create_request_body(request_body_schema);

    if let Some(rb) = request_body {
        operation.request_body = Some(openapiv3::ReferenceOr::Item(rb));
    }
}

fn create_request_body(request_body_schema: &RequestBodySchema) -> Option<openapiv3::RequestBody> {
    match request_body_schema {
        RequestBodySchema::Unused => None,
        RequestBodySchema::JsonBody { expected_type } => {
            let schema = create_schema_from_analysed_type(expected_type);
            Some(openapiv3::RequestBody {
                description: Some("JSON body".to_string()),
                content: {
                    let mut content = IndexMap::new();
                    content.insert(
                        "application/json".to_string(),
                        openapiv3::MediaType {
                            schema: Some(openapiv3::ReferenceOr::Item(schema)),
                            ..Default::default()
                        },
                    );
                    content
                },
                required: true,
                extensions: Default::default(),
            })
        }

        RequestBodySchema::UnrestrictedBinary => Some(openapiv3::RequestBody {
            description: Some("Unrestricted binary body".to_string()),
            content: {
                let mut content = IndexMap::new();
                content.insert(
                    "*/*".to_string(),
                    openapiv3::MediaType {
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
                    openapiv3::MediaType {
                        ..Default::default()
                    },
                );
            }
            Some(openapiv3::RequestBody {
                description: Some("Restricted binary body".to_string()),
                content,
                required: true,
                extensions: Default::default(),
            })
        }
    }
}

fn add_responses<T: HttpApiRoute + ?Sized>(operation: &mut openapiv3::Operation, route: &T) {
    let (response_schema, headers, explicit_status) =
        determine_response_schema_content_type_headers(route);

    let mut response = openapiv3::Response {
        description: "Response".to_string(),
        ..Default::default()
    };

    match response_schema {
        DeterminedResponseBodySchema::Known {
            schema,
            content_type,
        } => {
            let mut content = IndexMap::new();
            content.insert(
                content_type.clone(),
                openapiv3::MediaType {
                    schema: Some(openapiv3::ReferenceOr::Item(*schema)),
                    ..Default::default()
                },
            );
            response.content = content;
        }
        DeterminedResponseBodySchema::Unknown => {
            let mut content = IndexMap::new();
            content.insert(
                "*/*".to_string(),
                openapiv3::MediaType {
                    ..Default::default()
                },
            );
            response.content = content;
        }
        DeterminedResponseBodySchema::NoBody => {
            response.content = IndexMap::new();
        }
    }

    // Add headers if any
    if let Some(headers_map) = headers {
        let mut response_headers = IndexMap::new();
        for (name, schema) in headers_map {
            let description = format!("Response header: {}", name);
            response_headers.insert(
                name.clone(),
                openapiv3::ReferenceOr::Item(openapiv3::Header {
                    description: Some(description),
                    required: true,
                    deprecated: None,
                    format: openapiv3::ParameterSchemaOrContent::Schema(
                        openapiv3::ReferenceOr::Item(schema),
                    ),
                    example: None,
                    examples: Default::default(),
                    extensions: Default::default(),
                    style: openapiv3::HeaderStyle::Simple,
                }),
            );
        }
        response.headers = response_headers;
    }

    // Insert response: explicit status if given, otherwise default
    if let Some(status) = explicit_status {
        operation.responses.responses.insert(
            openapiv3::StatusCode::Code(status),
            openapiv3::ReferenceOr::Item(response),
        );
    } else {
        operation.responses.default = Some(openapiv3::ReferenceOr::Item(response));
    }
}

enum DeterminedResponseBodySchema {
    Unknown,
    Known {
        schema: Box<openapiv3::Schema>,
        content_type: String,
    },
    NoBody,
}

fn determine_response_schema_content_type_headers<T: HttpApiRoute + ?Sized>(
    route: &T,
) -> (
    DeterminedResponseBodySchema,
    Option<IndexMap<String, openapiv3::Schema>>,
    Option<u16>, // status code
) {
    match route.binding() {
        RichRouteBehaviour::CallAgent(call_agent_behaviour) => {
            let response = &call_agent_behaviour.expected_agent_response;

            match response {
                DataSchema::Tuple(named_element_schemas) => {
                    match named_element_schemas.elements.len() {
                        0 => (DeterminedResponseBodySchema::NoBody, None, Some(204)),
                        1 => {
                            let element_schema = &named_element_schemas.elements[0];
                            let schema = &element_schema.schema;

                            match schema {
                                ElementSchema::ComponentModel(inner) => {
                                    let schema =
                                        create_schema_from_analysed_type(&inner.element_type);
                                    (
                                        DeterminedResponseBodySchema::Known {
                                            schema: Box::new(schema),
                                            content_type: "application/json".to_string(),
                                        },
                                        None,
                                        Some(200),
                                    )
                                }

                                _ => {
                                    todo!()
                                }
                            }
                        }
                        _ => (DeterminedResponseBodySchema::Unknown, None, Some(200)),
                    }
                }

                _ => todo!("response schema other than tuple not supported yet"),
            }
        }

        RichRouteBehaviour::CorsPreflight(_) => {
            let mut headers = IndexMap::new();
            let string_schema = openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                    openapiv3::StringType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Empty,
                        pattern: None,
                        enumeration: vec![],
                        min_length: None,
                        max_length: None,
                    },
                )),
            };

            headers.insert(
                "Access-Control-Allow-Origin".to_string(),
                string_schema.clone(),
            );
            headers.insert(
                "Access-Control-Allow-Headers".to_string(),
                string_schema.clone(),
            );

            (
                DeterminedResponseBodySchema::NoBody,
                Some(headers),
                Some(200),
            )
        }

        RichRouteBehaviour::OpenApiSpec(_) => {
            let schema = create_schema_from_analysed_type(&AnalysedType::Str(
                golem_wasm::analysis::TypeStr {},
            ));
            (
                DeterminedResponseBodySchema::Known {
                    schema: Box::new(schema),
                    content_type: "text/html".to_string(),
                },
                None,
                Some(200),
            )
        }

        RichRouteBehaviour::WebhookCallback(_) => {
            (DeterminedResponseBodySchema::NoBody, None, Some(200))
        }

        RichRouteBehaviour::OidcCallback(_) => {
            (DeterminedResponseBodySchema::NoBody, None, Some(200))
        }
    }
}

fn add_security<T: HttpApiRoute + ?Sized>(operation: &mut openapiv3::Operation, route: &T) {
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
    path_item: &mut openapiv3::PathItem,
    method: String,
    operation: openapiv3::Operation,
) -> Result<(), String> {
    match method.as_str() {
        "GET" => path_item.get = Some(operation),
        "POST" => path_item.post = Some(operation),
        "PUT" => path_item.put = Some(operation),
        "DELETE" => path_item.delete = Some(operation),
        "PATCH" => path_item.patch = Some(operation),
        "OPTIONS" => path_item.options = Some(operation),
        "HEAD" => path_item.head = Some(operation),
        "TRACE" => path_item.trace = Some(operation),
        _ => return Err(format!("Unsupported HTTP method: {:?}", method)), // Well, I don't know what to do here
    }
    Ok(())
}

fn create_integer_schema(
    format: openapiv3::IntegerFormat,
    min: Option<i64>,
    max: Option<i64>,
) -> openapiv3::Schema {
    let schema_data = openapiv3::SchemaData::default();
    openapiv3::Schema {
        schema_data,
        schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
            openapiv3::IntegerType {
                format: openapiv3::VariantOrUnknownOrEmpty::Item(format),
                minimum: min,
                maximum: max,
                multiple_of: None,
                exclusive_minimum: false,
                exclusive_maximum: false,
                enumeration: vec![],
            },
        )),
    }
}

fn create_schema_from_analysed_type(analysed_type: &AnalysedType) -> openapiv3::Schema {
    use golem_wasm::analysis::AnalysedType;

    match analysed_type {
        AnalysedType::Bool(_) => openapiv3::Schema {
            schema_data: Default::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Boolean(
                openapiv3::BooleanType::default(),
            )),
        },
        AnalysedType::U8(_) => {
            create_integer_schema(openapiv3::IntegerFormat::Int32, Some(0), Some(255))
        }
        AnalysedType::U16(_) => {
            create_integer_schema(openapiv3::IntegerFormat::Int32, Some(0), Some(65535))
        }
        AnalysedType::U32(_) => {
            create_integer_schema(openapiv3::IntegerFormat::Int32, Some(0), None)
        }
        AnalysedType::U64(_) => {
            create_integer_schema(openapiv3::IntegerFormat::Int64, Some(0), None)
        }
        AnalysedType::S8(_) => {
            create_integer_schema(openapiv3::IntegerFormat::Int32, Some(-128), Some(127))
        }
        AnalysedType::S16(_) => {
            create_integer_schema(openapiv3::IntegerFormat::Int32, Some(-32768), Some(32767))
        }
        AnalysedType::S32(_) => create_integer_schema(openapiv3::IntegerFormat::Int32, None, None),
        AnalysedType::S64(_) => create_integer_schema(openapiv3::IntegerFormat::Int64, None, None),

        AnalysedType::F32(_) => {
            let sd = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data: sd,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Number(
                    openapiv3::NumberType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(
                            openapiv3::NumberFormat::Float,
                        ),
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        minimum: None,
                        maximum: None,
                        enumeration: vec![],
                    },
                )),
            }
        }
        AnalysedType::F64(_) => {
            let sd = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data: sd,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Number(
                    openapiv3::NumberType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(
                            openapiv3::NumberFormat::Double,
                        ),
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        minimum: None,
                        maximum: None,
                        enumeration: vec![],
                    },
                )),
            }
        }
        AnalysedType::Str(_) => openapiv3::Schema {
            schema_data: Default::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                openapiv3::StringType {
                    format: openapiv3::VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: vec![],
                    min_length: None,
                    max_length: None,
                },
            )),
        },
        AnalysedType::List(type_list) => {
            let items = openapiv3::ReferenceOr::Item(Box::new(create_schema_from_analysed_type(
                &type_list.inner,
            )));
            openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Array(
                    openapiv3::ArrayType {
                        items: Some(items),
                        min_items: None,
                        max_items: None,
                        unique_items: false,
                    },
                )),
            }
        }
        AnalysedType::Tuple(type_tuple) => {
            let min_items = Some(type_tuple.items.len());
            let max_items = Some(type_tuple.items.len());

            let items = openapiv3::ReferenceOr::Item(Box::new(openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(
                    Default::default(),
                )),
            }));
            let array_schema = openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Array(
                    openapiv3::ArrayType {
                        items: Some(items),
                        min_items,
                        max_items,
                        unique_items: false,
                    },
                )),
            };

            let schema_data = openapiv3::SchemaData {
                description: Some("Tuple type".to_string()),
                ..Default::default()
            };
            openapiv3::Schema {
                schema_data,
                schema_kind: array_schema.schema_kind,
            }
        }
        AnalysedType::Record(type_record) => {
            let mut properties = IndexMap::new();
            let mut required = Vec::new();
            for field in &type_record.fields {
                let field_schema = create_schema_from_analysed_type(&field.typ);
                let is_nullable = field_schema.schema_data.nullable;
                properties.insert(
                    field.name.clone(),
                    openapiv3::ReferenceOr::Item(Box::new(field_schema)),
                );
                if !is_nullable {
                    required.push(field.name.clone());
                }
            }
            openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(
                    openapiv3::ObjectType {
                        properties,
                        required,
                        additional_properties: None,
                        min_properties: None,
                        max_properties: None,
                    },
                )),
            }
        }
        AnalysedType::Variant(type_variant) => {
            let mut one_of = Vec::new();
            for case in &type_variant.cases {
                let case_name = &case.name;
                if let Some(case_type) = &case.typ {
                    let case_schema = create_schema_from_analysed_type(case_type);
                    let mut properties = IndexMap::new();
                    properties.insert(
                        case_name.clone(),
                        openapiv3::ReferenceOr::Item(Box::new(case_schema)),
                    );
                    let required = vec![case_name.clone()];
                    let schema = openapiv3::Schema {
                        schema_data: Default::default(),
                        schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(
                            openapiv3::ObjectType {
                                properties,
                                required,
                                additional_properties: None,
                                min_properties: None,
                                max_properties: None,
                            },
                        )),
                    };
                    one_of.push(openapiv3::ReferenceOr::Item(schema));
                } else {
                    let schema = openapiv3::Schema {
                        schema_data: Default::default(),
                        schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                            openapiv3::StringType {
                                format: openapiv3::VariantOrUnknownOrEmpty::Empty,
                                pattern: None,
                                enumeration: vec![],
                                min_length: None,
                                max_length: None,
                            },
                        )),
                    };
                    one_of.push(openapiv3::ReferenceOr::Item(schema));
                }
            }
            openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::OneOf { one_of },
            }
        }
        AnalysedType::Enum(type_enum) => {
            let enum_values: Vec<Option<String>> =
                type_enum.cases.iter().map(|c| Some(c.clone())).collect();
            openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                    openapiv3::StringType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Empty,
                        pattern: None,
                        enumeration: enum_values,
                        min_length: None,
                        max_length: None,
                    },
                )),
            }
        }
        AnalysedType::Option(type_option) => {
            let mut schema = create_schema_from_analysed_type(&type_option.inner);
            schema.schema_data.nullable = true;
            schema
        }
        AnalysedType::Result(type_result) => {
            let ok_type = match &type_result.ok {
                Some(b) => &**b,
                None => &AnalysedType::Str(golem_wasm::analysis::TypeStr {}),
            };
            let err_type = match &type_result.err {
                Some(b) => &**b,
                None => &AnalysedType::Str(golem_wasm::analysis::TypeStr {}),
            };
            let ok_schema = create_schema_from_analysed_type(ok_type);
            let err_schema = create_schema_from_analysed_type(err_type);

            let mut ok_properties = IndexMap::new();
            ok_properties.insert(
                "ok".to_string(),
                openapiv3::ReferenceOr::Item(Box::new(ok_schema)),
            );
            let ok_required = vec!["ok".to_string()];
            let ok_object_schema = openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(
                    openapiv3::ObjectType {
                        properties: ok_properties,
                        required: ok_required,
                        additional_properties: None,
                        min_properties: None,
                        max_properties: None,
                    },
                )),
            };

            let mut err_properties = IndexMap::new();
            err_properties.insert(
                "err".to_string(),
                openapiv3::ReferenceOr::Item(Box::new(err_schema)),
            );
            let err_required = vec!["err".to_string()];
            let err_object_schema = openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(
                    openapiv3::ObjectType {
                        properties: err_properties,
                        required: err_required,
                        additional_properties: None,
                        min_properties: None,
                        max_properties: None,
                    },
                )),
            };

            openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::OneOf {
                    one_of: vec![
                        openapiv3::ReferenceOr::Item(ok_object_schema),
                        openapiv3::ReferenceOr::Item(err_object_schema),
                    ],
                },
            }
        }
        AnalysedType::Flags(type_flags) => {
            let enum_values: Vec<Option<String>> =
                type_flags.names.iter().map(|n| Some(n.clone())).collect();
            let items_schema = openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                    openapiv3::StringType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Empty,
                        pattern: None,
                        enumeration: enum_values,
                        min_length: None,
                        max_length: None,
                    },
                )),
            };
            openapiv3::Schema {
                schema_data: openapiv3::SchemaData {
                    description: Some("Flags type - array of flag names".to_string()),
                    ..Default::default()
                },
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Array(
                    openapiv3::ArrayType {
                        items: Some(openapiv3::ReferenceOr::Item(Box::new(items_schema))),
                        min_items: Some(0),
                        max_items: Some(type_flags.names.len()),
                        unique_items: true,
                    },
                )),
            }
        }
        AnalysedType::Chr(_) => openapiv3::Schema {
            schema_data: openapiv3::SchemaData {
                description: Some("Unicode character".to_string()),
                ..Default::default()
            },
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                openapiv3::StringType {
                    format: openapiv3::VariantOrUnknownOrEmpty::Empty,
                    pattern: Some("^.{1}$".to_string()),
                    enumeration: vec![],
                    min_length: Some(1),
                    max_length: Some(1),
                },
            )),
        },
        AnalysedType::Handle(_) => unimplemented!(),
    }
}
