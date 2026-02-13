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
use golem_common::base_model::agent::{AgentMethod, AgentType, ElementSchema, HttpMethod};
use golem_common::model::agent::DataSchema;
use golem_service_base::custom_api::{
    MethodParameter, PathSegment, PathSegmentType, QueryOrHeaderType, RequestBodySchema,
    SecuritySchemeDetails,
};
use golem_service_base::model::SafeIndex;
use golem_wasm::analysis::AnalysedType;
use indexmap::IndexMap;
use openapiv3::{
    AdditionalProperties, ArrayType, BooleanType, Components, Header, HeaderStyle, Info,
    IntegerFormat, IntegerType, MediaType, NumberFormat, NumberType, ObjectType, OpenAPI,
    Operation, Parameter, ParameterData, ParameterSchemaOrContent, PathItem, PathStyle, QueryStyle,
    ReferenceOr, RequestBody, Response, Schema, SchemaData, SchemaKind, SecurityScheme, StatusCode,
    StringFormat, StringType, Type, VariantOrUnknownOrEmpty,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const GOLEM_DISABLED_EXTENSION: &str = "x-golem-disabled";

pub trait HttpApiRoute {
    fn security_scheme_missing(&self) -> bool;
    fn security_scheme(&self) -> &Option<Arc<SecuritySchemeDetails>>;
    fn method(&self) -> &str;
    fn path(&self) -> &Vec<PathSegment>;
    fn binding(&self) -> &RichRouteBehaviour;
    fn request_body_schema(&self) -> &RequestBodySchema;
    fn associated_agent_method(&self) -> Option<&AgentMethod>;
}

pub struct RichCompiledRouteWithAgentType<'a> {
    pub agent_type: &'a AgentType,
    pub details: &'a RichCompiledRoute,
}

impl<'a> HttpApiRoute for RichCompiledRouteWithAgentType<'a> {
    fn security_scheme_missing(&self) -> bool {
        false
    }
    fn security_scheme(&self) -> &Option<Arc<SecuritySchemeDetails>> {
        &self.details.security_scheme
    }
    fn method(&self) -> &str {
        self.details.method.as_str()
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
    agent_method: Option<&'a AgentMethod>,
    path_segments: &'a [PathSegment],
    method_params: Option<&'a Vec<MethodParameter>>,
) -> (String, Vec<(&'a str, &'a PathSegmentType)>) {
    if path_segments
        .iter()
        .all(|segment| matches!(segment, PathSegment::Literal { .. }))
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

    let method_path_indices: HashMap<SafeIndex, &'a PathSegmentType> = method_params
        .map(|params| {
            params
                .iter()
                .filter_map(|p| match p {
                    MethodParameter::Path {
                        path_segment_index,
                        parameter_type,
                    } => Some((*path_segment_index, parameter_type)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();

    let input_parameter_names = agent_method.and_then(|x| match &x.input_schema {
        DataSchema::Tuple(named_element_schemas) => Some(&named_element_schemas.elements),
        DataSchema::Multimodal(_) => None,
    });

    let mut path_params_and_types: Vec<(&'a str, &'a PathSegmentType)> = Vec::new();
    let mut path_segment_string: Vec<String> = Vec::new();

    for (idx, segment) in path_segments.iter().enumerate() {
        match segment {
            PathSegment::Literal { value } => {
                path_segment_string.push(value.to_string());
            }

            PathSegment::Variable | PathSegment::CatchAll => {
                if let Some(param_type) = method_path_indices.get(&SafeIndex::new(idx as u32)) {
                    let name = input_parameter_names
                        .and_then(|elements| elements.get(idx))
                        .map(|e| e.name.as_str())
                        .expect("Path segment must have parameter name");

                    path_segment_string.push(format!("{{{}}}", name));
                    path_params_and_types.push((name, *param_type));
                }
            }
        }
    }

    (path_segment_string.join("/"), path_params_and_types)
}

fn add_route_to_paths<T: HttpApiRoute + ?Sized>(
    route: &T,
    paths: &mut BTreeMap<String, PathItem>,
) -> Result<(), String> {
    let method_params = match route.binding() {
        RichRouteBehaviour::CallAgent(behaviour) => Some(&behaviour.method_parameters),
        _ => None,
    };

    let (path_str, path_params_raw) =
        get_full_path_and_variables(route.associated_agent_method(), route.path(), method_params);

    let path_item = paths.entry(path_str).or_default();
    let mut operation = Operation::default();

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
    let (status_code_and_response_schemas, headers_map) =
        determine_agent_response_with_status_code(route);

    for (status_code, response_schema) in status_code_and_response_schemas {
        let mut response = Response {
            description: format!("Response {}", status_code),
            ..Default::default()
        };

        match response_schema {
            DeterminedResponseBodySchema::Known {
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
            DeterminedResponseBodySchema::Unknown => {
                let mut content = IndexMap::new();
                content.insert(
                    "*/*".to_string(),
                    MediaType {
                        ..Default::default()
                    },
                );
                response.content = content;
            }
            DeterminedResponseBodySchema::NoBody => {
                response.content = IndexMap::new();
            }
        }

        let mut response_headers = IndexMap::new();

        for (name, schema) in headers_map.iter() {
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

enum DeterminedResponseBodySchema {
    Unknown,
    Known {
        schema: Box<Schema>,
        content_type: String,
    },
    NoBody,
}

fn determine_agent_response_with_status_code<T: HttpApiRoute + ?Sized>(
    route: &T,
) -> (
    IndexMap<u16, DeterminedResponseBodySchema>, // u16 for status code
    IndexMap<String, Schema>,
) {
    let mut responses = IndexMap::new();
    let mut headers = IndexMap::new();

    match route.binding() {
        RichRouteBehaviour::CallAgent(call_agent_behaviour) => {
            match &call_agent_behaviour.expected_agent_response {
                DataSchema::Tuple(named_elements) => match named_elements.elements.len() {
                    0 => {
                        responses.insert(204, DeterminedResponseBodySchema::NoBody);
                    }
                    1 => {
                        let element_schema = &named_elements.elements[0].schema;

                        match element_schema {
                            // based on response_mapping logic in the actual request_handler
                            ElementSchema::ComponentModel(typ) => {
                                if let AnalysedType::Option(opt) = &typ.element_type {
                                    let inner_schema = create_schema_from_analysed_type(&opt.inner);
                                    responses.insert(
                                        200,
                                        DeterminedResponseBodySchema::Known {
                                            schema: Box::new(inner_schema),
                                            content_type: "application/json".to_string(),
                                        },
                                    );
                                    responses.insert(404, DeterminedResponseBodySchema::NoBody);
                                } else if let AnalysedType::Result(res) = &typ.element_type {
                                    if let Some(ok_type) = &res.ok {
                                        let ok_schema = create_schema_from_analysed_type(ok_type);
                                        responses.insert(
                                            200,
                                            DeterminedResponseBodySchema::Known {
                                                schema: Box::new(ok_schema),
                                                content_type: "application/json".to_string(),
                                            },
                                        );
                                    } else {
                                        responses
                                            .insert(200, DeterminedResponseBodySchema::Unknown);
                                    }

                                    if let Some(err_type) = &res.err {
                                        let err_schema = create_schema_from_analysed_type(err_type);
                                        responses.insert(
                                            500,
                                            DeterminedResponseBodySchema::Known {
                                                schema: Box::new(err_schema),
                                                content_type: "application/json".to_string(),
                                            },
                                        );
                                        responses.insert(500, DeterminedResponseBodySchema::NoBody);
                                    } else {
                                        responses
                                            .insert(500, DeterminedResponseBodySchema::Unknown);
                                    }
                                } else {
                                    let schema =
                                        create_schema_from_analysed_type(&typ.element_type);
                                    responses.insert(
                                        200,
                                        DeterminedResponseBodySchema::Known {
                                            schema: Box::new(schema),
                                            content_type: "application/json".to_string(),
                                        },
                                    );
                                }
                            }
                            ElementSchema::UnstructuredBinary(binary_descriptor) => {
                                let content_type = binary_descriptor
                                    .restrictions
                                    .as_ref()
                                    .and_then(|types| types.first())
                                    .map(|bt| bt.mime_type.clone())
                                    .unwrap_or_else(|| "application/octet-stream".to_string());

                                let schema = Schema {
                                    schema_data: SchemaData::default(),
                                    schema_kind: SchemaKind::Type(Type::String(StringType {
                                        format: VariantOrUnknownOrEmpty::Item(StringFormat::Binary),
                                        pattern: None,
                                        enumeration: Vec::new(),
                                        min_length: None,
                                        max_length: None,
                                    })),
                                };
                                responses.insert(
                                    200,
                                    DeterminedResponseBodySchema::Known {
                                        schema: Box::new(schema),
                                        content_type,
                                    },
                                );
                            }
                            _ => {
                                responses.insert(200, DeterminedResponseBodySchema::Unknown);
                            }
                        }
                    }
                    _ => {
                        responses.insert(200, DeterminedResponseBodySchema::Unknown);
                    }
                },
                DataSchema::Multimodal(_) => {
                    responses.insert(200, DeterminedResponseBodySchema::Unknown);
                }
            }
        }
        RichRouteBehaviour::CorsPreflight(cors_preflight_behaviour) => {
            responses.insert(204, DeterminedResponseBodySchema::NoBody);

            let string_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: vec![],
                    min_length: None,
                    max_length: None,
                })),
            };

            headers.insert(
                "Access-Control-Allow-Origin".to_string(),
                string_schema.clone(),
            );

            headers.insert(
                "Access-Control-Allow-Headers".to_string(),
                string_schema.clone(),
            );

            let allowed_methods_enum: Vec<Option<String>> = cors_preflight_behaviour
                .allowed_methods
                .iter()
                .map(|m| {
                    Some(match m {
                        HttpMethod::Get(_) => "GET".to_string(),
                        HttpMethod::Head(_) => "HEAD".to_string(),
                        HttpMethod::Post(_) => "POST".to_string(),
                        HttpMethod::Put(_) => "PUT".to_string(),
                        HttpMethod::Delete(_) => "DELETE".to_string(),
                        HttpMethod::Connect(_) => "CONNECT".to_string(),
                        HttpMethod::Options(_) => "OPTIONS".to_string(),
                        HttpMethod::Trace(_) => "TRACE".to_string(),
                        HttpMethod::Patch(_) => "PATCH".to_string(),
                        HttpMethod::Custom(custom) => custom.value.to_uppercase(),
                    })
                })
                .collect();

            headers.insert(
                "Access-Control-Allow-Methods".to_string(),
                Schema {
                    schema_data: Default::default(),
                    schema_kind: SchemaKind::Type(Type::String(StringType {
                        format: VariantOrUnknownOrEmpty::Empty,
                        pattern: None,
                        enumeration: allowed_methods_enum,
                        min_length: None,
                        max_length: None,
                    })),
                },
            );
        }
        RichRouteBehaviour::WebhookCallback(_) => {
            // based on runtime behaviour of webhook
            responses.insert(204, DeterminedResponseBodySchema::NoBody);
            responses.insert(404, DeterminedResponseBodySchema::NoBody);
        }
        RichRouteBehaviour::OpenApiSpec(_) => {
            let schema = create_schema_from_analysed_type(&AnalysedType::Str(
                golem_wasm::analysis::TypeStr {},
            ));

            responses.insert(
                200,
                DeterminedResponseBodySchema::Known {
                    schema: Box::new(schema),
                    content_type: "text/plain".to_string(),
                },
            );
        }
        RichRouteBehaviour::OidcCallback(_) => {
            let string_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: vec![],
                    min_length: None,
                    max_length: None,
                })),
            };

            headers.insert("Set-Cookie".to_string(), string_schema.clone());
            headers.insert("Location".to_string(), string_schema);
        }
    }

    (responses, headers)
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

fn create_integer_schema(format: IntegerFormat, min: Option<i64>, max: Option<i64>) -> Schema {
    let schema_data = SchemaData::default();
    Schema {
        schema_data,
        schema_kind: SchemaKind::Type(Type::Integer(IntegerType {
            format: VariantOrUnknownOrEmpty::Item(format),
            minimum: min,
            maximum: max,
            multiple_of: None,
            exclusive_minimum: false,
            exclusive_maximum: false,
            enumeration: vec![],
        })),
    }
}

fn create_schema_from_analysed_type(analysed_type: &AnalysedType) -> Schema {
    use golem_wasm::analysis::AnalysedType;

    match analysed_type {
        AnalysedType::Bool(_) => Schema {
            schema_data: Default::default(),
            schema_kind: SchemaKind::Type(Type::Boolean(BooleanType::default())),
        },
        AnalysedType::U8(_) => create_integer_schema(IntegerFormat::Int32, Some(0), Some(255)),
        AnalysedType::U16(_) => create_integer_schema(IntegerFormat::Int32, Some(0), Some(65535)),
        AnalysedType::U32(_) => create_integer_schema(IntegerFormat::Int32, Some(0), None),
        AnalysedType::U64(_) => create_integer_schema(IntegerFormat::Int64, Some(0), None),
        AnalysedType::S8(_) => create_integer_schema(IntegerFormat::Int32, Some(-128), Some(127)),
        AnalysedType::S16(_) => {
            create_integer_schema(IntegerFormat::Int32, Some(-32768), Some(32767))
        }
        AnalysedType::S32(_) => create_integer_schema(IntegerFormat::Int32, None, None),
        AnalysedType::S64(_) => create_integer_schema(IntegerFormat::Int64, None, None),

        AnalysedType::F32(_) => {
            let sd = SchemaData::default();
            Schema {
                schema_data: sd,
                schema_kind: SchemaKind::Type(Type::Number(NumberType {
                    format: VariantOrUnknownOrEmpty::Item(NumberFormat::Float),
                    multiple_of: None,
                    exclusive_minimum: false,
                    exclusive_maximum: false,
                    minimum: None,
                    maximum: None,
                    enumeration: vec![],
                })),
            }
        }
        AnalysedType::F64(_) => {
            let sd = SchemaData::default();
            Schema {
                schema_data: sd,
                schema_kind: SchemaKind::Type(Type::Number(NumberType {
                    format: VariantOrUnknownOrEmpty::Item(NumberFormat::Double),
                    multiple_of: None,
                    exclusive_minimum: false,
                    exclusive_maximum: false,
                    minimum: None,
                    maximum: None,
                    enumeration: vec![],
                })),
            }
        }
        AnalysedType::Str(_) => Schema {
            schema_data: Default::default(),
            schema_kind: SchemaKind::Type(Type::String(StringType {
                format: VariantOrUnknownOrEmpty::Empty,
                pattern: None,
                enumeration: vec![],
                min_length: None,
                max_length: None,
            })),
        },
        AnalysedType::List(type_list) => {
            let items =
                ReferenceOr::Item(Box::new(create_schema_from_analysed_type(&type_list.inner)));
            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Array(ArrayType {
                    items: Some(items),
                    min_items: None,
                    max_items: None,
                    unique_items: false,
                })),
            }
        }
        AnalysedType::Tuple(type_tuple) => {
            let min_items = Some(type_tuple.items.len());
            let max_items = Some(type_tuple.items.len());

            let items = ReferenceOr::Item(Box::new(Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(Default::default())),
            }));
            let array_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Array(ArrayType {
                    items: Some(items),
                    min_items,
                    max_items,
                    unique_items: false,
                })),
            };

            let schema_data = SchemaData {
                description: Some("Tuple type".to_string()),
                ..Default::default()
            };
            Schema {
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
                    ReferenceOr::Item(Box::new(field_schema)),
                );
                if !is_nullable {
                    required.push(field.name.clone());
                }
            }
            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                    properties,
                    required,
                    additional_properties: None,
                    min_properties: None,
                    max_properties: None,
                })),
            }
        }
        AnalysedType::Variant(type_variant) => {
            let mut one_of = Vec::new();
            for case in &type_variant.cases {
                let case_name = &case.name;
                if let Some(case_type) = &case.typ {
                    let case_schema = create_schema_from_analysed_type(case_type);
                    let mut properties = IndexMap::new();
                    properties.insert(case_name.clone(), ReferenceOr::Item(Box::new(case_schema)));
                    let required = vec![case_name.clone()];
                    let schema = Schema {
                        schema_data: Default::default(),
                        schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                            properties,
                            required,
                            additional_properties: None,
                            min_properties: None,
                            max_properties: None,
                        })),
                    };
                    one_of.push(ReferenceOr::Item(schema));
                } else {
                    let schema = Schema {
                        schema_data: Default::default(),
                        schema_kind: SchemaKind::Type(Type::String(StringType {
                            format: VariantOrUnknownOrEmpty::Empty,
                            pattern: None,
                            enumeration: vec![Some(case_name.clone())],
                            min_length: None,
                            max_length: None,
                        })),
                    };

                    one_of.push(ReferenceOr::Item(schema));
                }
            }
            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::OneOf { one_of },
            }
        }
        AnalysedType::Enum(type_enum) => {
            let enum_values: Vec<Option<String>> =
                type_enum.cases.iter().map(|c| Some(c.clone())).collect();
            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: enum_values,
                    min_length: None,
                    max_length: None,
                })),
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
            ok_properties.insert("ok".to_string(), ReferenceOr::Item(Box::new(ok_schema)));
            let ok_required = vec!["ok".to_string()];
            let ok_object_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                    properties: ok_properties,
                    required: ok_required,
                    additional_properties: Some(AdditionalProperties::Any(false)),
                    min_properties: None,
                    max_properties: None,
                })),
            };

            let mut err_properties = IndexMap::new();
            err_properties.insert("err".to_string(), ReferenceOr::Item(Box::new(err_schema)));
            let err_required = vec!["err".to_string()];
            let err_object_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                    properties: err_properties,
                    required: err_required,
                    additional_properties: Some(AdditionalProperties::Any(false)),
                    min_properties: None,
                    max_properties: None,
                })),
            };

            Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::OneOf {
                    one_of: vec![
                        ReferenceOr::Item(ok_object_schema),
                        ReferenceOr::Item(err_object_schema),
                    ],
                },
            }
        }
        AnalysedType::Flags(type_flags) => {
            let enum_values: Vec<Option<String>> =
                type_flags.names.iter().map(|n| Some(n.clone())).collect();
            let items_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: enum_values,
                    min_length: None,
                    max_length: None,
                })),
            };
            Schema {
                schema_data: SchemaData {
                    description: Some("Flags type - array of flag names".to_string()),
                    ..Default::default()
                },
                schema_kind: SchemaKind::Type(Type::Array(ArrayType {
                    items: Some(ReferenceOr::Item(Box::new(items_schema))),
                    min_items: Some(0),
                    max_items: Some(type_flags.names.len()),
                    unique_items: true,
                })),
            }
        }
        AnalysedType::Chr(_) => Schema {
            schema_data: SchemaData {
                description: Some("Unicode character".to_string()),
                ..Default::default()
            },
            schema_kind: SchemaKind::Type(Type::String(StringType {
                format: VariantOrUnknownOrEmpty::Empty,
                pattern: Some("^.{1}$".to_string()),
                enumeration: vec![],
                min_length: Some(1),
                max_length: Some(1),
            })),
        },
        AnalysedType::Handle(_) => Schema {
            schema_data: SchemaData {
                description: Some("Opaque handle identifier".to_string()),
                ..Default::default()
            },
            schema_kind: SchemaKind::Type(Type::String(StringType {
                format: VariantOrUnknownOrEmpty::Empty,
                pattern: None,
                enumeration: vec![],
                min_length: None,
                max_length: None,
            })),
        },
    }
}
