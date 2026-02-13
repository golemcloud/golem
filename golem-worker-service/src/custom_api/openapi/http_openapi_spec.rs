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

use super::call_agent;
use crate::custom_api::openapi::response_schema::{
    ResponseBodyOpenApiSchema, get_route_response_schema,
};
use crate::custom_api::openapi::schema_mapping::{
    arbitrary_binary_schema, create_schema_from_analysed_type,
};
use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour, RichRouteSecurity};
use golem_service_base::custom_api::{
    PathSegment, PathSegmentType, QueryOrHeaderType, RequestBodySchema, SecuritySchemeDetails,
};
use golem_wasm::analysis::AnalysedType;
use indexmap::IndexMap;
use openapiv3::{
    Components, Header, HeaderStyle, Info, MediaType, OpenAPI, Operation, Parameter, ParameterData,
    ParameterSchemaOrContent, PathItem, PathStyle, QueryStyle, ReferenceOr, RequestBody, Response,
    Schema, SecurityScheme, StatusCode,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct HttpApiOpenApiSpec(pub OpenAPI);

impl HttpApiOpenApiSpec {
    pub fn from_routes(routes: &[RichCompiledRoute]) -> Result<Self, String> {
        let security_scheme_details = get_security_scheme_details(routes);

        let mut open_api = create_base_openapi(&security_scheme_details);

        let mut paths = BTreeMap::new();

        for route in routes {
            add_route_to_paths(route, &mut paths)?;
        }

        open_api.paths.paths = paths
            .into_iter()
            .map(|(k, v)| (k, ReferenceOr::Item(v)))
            .collect();

        Ok(HttpApiOpenApiSpec(open_api))
    }
}

fn get_security_scheme_details(
    compiled_routes: &[RichCompiledRoute],
) -> Vec<Arc<SecuritySchemeDetails>> {
    let mut schemes = Vec::new();

    for route in compiled_routes {
        if let RichRouteSecurity::SecurityScheme(details) = &route.security {
            schemes.push(details.security_scheme.clone());
        }
    }

    schemes
}

fn create_base_openapi(security_schemes: &Vec<Arc<SecuritySchemeDetails>>) -> OpenAPI {
    let mut open_api = OpenAPI {
        openapi: "3.0.0".to_string(),
        info: Info {
            title: "Managed api provided by Golem".to_string(),
            description: None,
            terms_of_service: None,
            contact: None,
            license: None,
            version: "1.0.0".to_string(),
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

fn render_full_path(path_segments: &[PathSegment]) -> String {
    let suffix = path_segments
        .iter()
        .map(|ps| match ps {
            PathSegment::Literal { value } => value.clone(),
            PathSegment::Variable { display_name } => format!("{{{display_name}}}"),
            PathSegment::CatchAll { display_name } => {
                // Note: this is the same as variable on purpose. The difference between both types is only communicated
                // in openapi in the variable type.
                format!("{{{display_name}}}")
            }
        })
        .collect::<Vec<String>>()
        .join("/");
    format!("/{suffix}")
}

fn add_route_to_paths(
    route: &RichCompiledRoute,
    paths: &mut BTreeMap<String, PathItem>,
) -> Result<(), String> {
    let path_str = render_full_path(&route.path);

    let path_item = paths.entry(path_str).or_default();
    let mut operation = Operation::default();

    let path_params_raw = match &route.behavior {
        RichRouteBehaviour::CallAgent(inner) => call_agent::get_path_variables_and_types(
            &route.path,
            &inner.constructor_parameters,
            &inner.method_parameters,
        ),
        RichRouteBehaviour::WebhookCallback(_) => vec![("promise-id", false, &PathSegmentType::Str)],
        _ => Vec::new(),
    };
    let query_params_raw = match &route.behavior {
        RichRouteBehaviour::CallAgent(inner) => {
            call_agent::get_query_variable_and_types(&inner.method_parameters)
        }
        _ => Vec::new(),
    };
    let header_params_raw = match &route.behavior {
        RichRouteBehaviour::CallAgent(inner) => {
            call_agent::get_header_variable_and_types(&inner.method_parameters)
        }
        _ => Vec::new(),
    };

    add_parameters(
        &mut operation,
        path_params_raw,
        query_params_raw,
        header_params_raw,
    );
    add_request_body(&mut operation, &route.body);
    add_responses(&mut operation, route);
    add_security(&mut operation, route);

    add_operation_to_path_item(path_item, route.method.as_str(), operation)?;

    Ok(())
}

fn add_parameters(
    operation: &mut Operation,
    path_params_raw: Vec<(&str, bool, &PathSegmentType)>,
    query_params_raw: Vec<(&str, &QueryOrHeaderType)>,
    header_params_raw: Vec<(&str, &QueryOrHeaderType)>,
) {
    for (name, is_catchall_segment, path_segment_type) in path_params_raw {
        let schema = create_schema_from_path_segment_type(path_segment_type, is_catchall_segment);
        operation
            .parameters
            .push(ReferenceOr::Item(create_path_parameter(name, schema)));
    }

    for (name, query_or_header_type) in query_params_raw {
        let schema = create_schema_from_query_or_header_type(query_or_header_type);
        operation
            .parameters
            .push(ReferenceOr::Item(create_query_parameter(name, schema)));
    }

    for (name, query_or_header_type) in header_params_raw {
        let schema = create_schema_from_query_or_header_type(query_or_header_type);
        operation
            .parameters
            .push(ReferenceOr::Item(create_header_parameter(name, schema)));
    }
}

fn create_schema_from_path_segment_type(
    path_segment_type: &PathSegmentType,
    is_catchall_segment: bool,
) -> Schema {
    let analysed_type = AnalysedType::from(path_segment_type);
    let mut schema = create_schema_from_analysed_type(&analysed_type);
    if is_catchall_segment {
        schema.schema_data.description = Some("Parameter represents the remaining path, including slashes.".to_string())
    }
    schema
}

fn create_schema_from_query_or_header_type(query_or_header_type: &QueryOrHeaderType) -> Schema {
    let analysed_type = AnalysedType::from(query_or_header_type.clone());
    create_schema_from_analysed_type(&analysed_type)
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
                        schema: Some(ReferenceOr::Item(arbitrary_binary_schema())),
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
                        schema: Some(ReferenceOr::Item(arbitrary_binary_schema())),
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

fn add_responses(operation: &mut Operation, route: &RichCompiledRoute) {
    let agent_response_schema = get_route_response_schema(route);

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
                        schema: Some(ReferenceOr::Item(arbitrary_binary_schema())),
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

fn add_security(operation: &mut Operation, route: &RichCompiledRoute) {
    if let RichRouteSecurity::SecurityScheme(inner) = &route.security {
        let scopes_vec: Vec<String> = inner
            .security_scheme
            .scopes
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut requirement: indexmap::IndexMap<String, Vec<String>> = indexmap::IndexMap::new();
        requirement.insert(inner.security_scheme.name.0.clone(), scopes_vec);

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
