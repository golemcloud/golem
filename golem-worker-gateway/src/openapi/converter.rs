// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use crate::openapi::schema::WitTypeConverter;
use golem_api_grpc::proto::golem::rib::{RibInputType, RibOutputType};
use golem_worker_service::api::path::{AllPathPatterns, PathPattern};
use openapiv3::{
    Components, Info, MediaType, Parameter, ParameterData, ParameterSchemaOrContent,
    RequestBody, Response, Responses, Schema, SchemaKind, Server, Type,
};

/// Converts API Definitions to OpenAPI Specifications
pub struct ApiDefinitionConverter {
    type_converter: WitTypeConverter,
}

impl ApiDefinitionConverter {
    pub fn new() -> Self {
        Self {
            type_converter: WitTypeConverter::new(),
        }
    }

    /// Convert an API Definition to an OpenAPI Specification
    pub fn convert(&self, api: &ApiDefinition) -> Result<OpenAPI> {
        let mut openapi = OpenAPI {
            openapi: "3.0.3".to_string(),
            info: Info {
                title: format!("API Definition {}", api.id.as_ref().map_or("", |id| &id.value)),
                version: api.version.clone(),
                ..Default::default()
            },
            servers: vec![Server {
                url: "/".to_string(),
                ..Default::default()
            }],
            paths: Default::default(),
            components: Some(Components {
                schemas: self.collect_common_schemas(api)?,
                ..Default::default()
            }),
            ..Default::default()
        };

        // Convert routes to paths
        if let Some(http_api) = &api.http_api {
            for route in &http_api.routes {
                self.add_route_to_openapi(&mut openapi, route)?;
            }
        }

        Ok(openapi)
    }

    fn collect_common_schemas(&self, api: &ApiDefinition) -> Result<HashMap<String, ReferenceOr<Schema>>> {
        let mut schemas = HashMap::new();
        
        if let Some(http_api) = &api.http_api {
            for route in &http_api.routes {
                if let Some(binding) = &route.binding {
                    // Collect input types
                    if let Some(input_type) = &binding.worker_name_rib_input {
                        for (name, type_info) in &input_type.types {
                            if let Some(Kind::Record(_)) = type_info.kind {
                                let schema = self.type_converter.convert_wit_type(type_info)?;
                                schemas.insert(
                                    name.to_string(),
                                    ReferenceOr::Item(schema),
                                );
                            }
                        }
                    }

                    // Collect output types
                    if let Some(output_type) = &binding.response_rib_output {
                        if let Some(type_info) = &output_type.type_ {
                            if let Some(Kind::Record(_)) = type_info.kind {
                                let schema = self.type_converter.convert_wit_type(type_info)?;
                                schemas.insert(
                                    "Response".to_string(),
                                    ReferenceOr::Item(schema),
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(schemas)
    }

    fn add_route_to_openapi(&self, openapi: &mut OpenAPI, route: &CompiledHttpRoute) -> Result<()> {
        let method = self.convert_http_method(&route.method)?;
        let mut operation = self.create_operation(route)?;

        // Add security schemes if authentication is required
        if let Some(middleware) = &route.middleware {
            if let Some(auth) = &middleware.http_authentication {
                self.add_security_scheme(auth, &mut operation)?;
            }

            // Add CORS headers if CORS is configured
            if let Some(cors) = &middleware.cors {
                self.add_cors_headers(cors, &mut operation)?;
            }
        }

        // Add operation metadata
        operation.summary = Some(format!("{} {}", method, route.path));
        operation.description = Some(format!(
            "Endpoint: {} {}\nBinding Type: {}\nSecurity: {}\nCORS: {}",
            method,
            route.path,
            if route.binding.is_some() { "Worker" } else { "Static" },
            if route.middleware.as_ref().and_then(|m| m.http_authentication.as_ref()).is_some() {
                "Required"
            } else {
                "None"
            },
            if route.middleware.as_ref().and_then(|m| m.cors.as_ref()).is_some() {
                "Enabled"
            } else {
                "Disabled"
            }
        ));

        // Add the operation to the path
        let path_item = openapi
            .paths
            .paths
            .entry(route.path.clone())
            .or_insert_with(|| PathItem::default());

        match method.as_str() {
            "get" => path_item.get = Some(operation),
            "post" => path_item.post = Some(operation),
            "put" => path_item.put = Some(operation),
            "delete" => path_item.delete = Some(operation),
            "options" => path_item.options = Some(operation),
            "head" => path_item.head = Some(operation),
            "patch" => path_item.patch = Some(operation),
            "trace" => path_item.trace = Some(operation),
            _ => return Err(OpenApiError::ConversionError(format!("Unsupported HTTP method: {}", method))),
        }

        Ok(())
    }

    fn convert_http_method(&self, method: &HttpMethod) -> Result<String> {
        Ok(match method {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Delete => "delete",
            HttpMethod::Options => "options",
            HttpMethod::Head => "head",
            HttpMethod::Patch => "patch",
            HttpMethod::Trace => "trace",
            HttpMethod::Connect => return Err(OpenApiError::ConversionError("CONNECT method not supported in OpenAPI".to_string())),
        }.to_string())
    }

    fn create_operation(&self, route: &CompiledHttpRoute) -> Result<Operation> {
        let mut operation = Operation::default();

        // Add path parameters
        let mut parameters = self.extract_path_parameters(&route.path)?;
        
        // Add query parameters
        if let Some(binding) = &route.binding {
            if let Some(input_type) = &binding.worker_name_rib_input {
                let query_params = self.extract_query_parameters(input_type)?;
                parameters.extend(query_params);
            }
        }
        
        operation.parameters = Some(parameters);

        // Add request body schema if this is a binding that accepts input
        if let Some(binding) = &route.binding {
            if let Some(input_type) = &binding.worker_name_rib_input {
                self.add_request_body(input_type, &mut operation)?;
            }

            // Add response schema
            if let Some(output_type) = &binding.response_rib_output {
                self.add_response(output_type, &mut operation)?;
            }
        }

        Ok(operation)
    }

    fn extract_path_parameters(&self, path: &str) -> Result<Vec<Parameter>> {
        let patterns = AllPathPatterns::parse(path)
            .map_err(|e| OpenApiError::ConversionError(format!("Failed to parse path: {}", e)))?;

        let mut parameters = Vec::new();
        for pattern in patterns.patterns {
            match pattern {
                PathPattern::Parameter(name) => {
                    parameters.push(Parameter::Path {
                        parameter_data: ParameterData {
                            name: name.to_string(),
                            description: None,
                            required: true,
                            deprecated: None,
                            format: ParameterSchemaOrContent::Schema(Schema {
                                schema_kind: SchemaKind::Type(Type::String(Default::default())),
                                schema_data: Default::default(),
                            }),
                            example: None,
                            examples: Default::default(),
                            explode: None,
                            extensions: Default::default(),
                        },
                        style: Default::default(),
                    });
                }
                PathPattern::MultiSegment(name) => {
                    parameters.push(Parameter::Path {
                        parameter_data: ParameterData {
                            name: name.to_string(),
                            description: Some("Multi-segment path parameter".to_string()),
                            required: true,
                            deprecated: None,
                            format: ParameterSchemaOrContent::Schema(Schema {
                                schema_kind: SchemaKind::Type(Type::Array(openapiv3::ArrayType {
                                    items: Box::new(ReferenceOr::Item(Schema {
                                        schema_kind: SchemaKind::Type(Type::String(Default::default())),
                                        schema_data: Default::default(),
                                    })),
                                    min_items: None,
                                    max_items: None,
                                    unique_items: false,
                                })),
                                schema_data: Default::default(),
                            }),
                            example: None,
                            examples: Default::default(),
                            explode: None,
                            extensions: Default::default(),
                        },
                        style: Default::default(),
                    });
                }
                _ => {}
            }
        }

        Ok(parameters)
    }

    fn extract_query_parameters(&self, input_type: &RibInputType) -> Result<Vec<Parameter>> {
        let mut parameters = Vec::new();

        for (name, type_info) in &input_type.types {
            // Skip fields that are part of the request body
            if matches!(type_info.kind, Some(Kind::Record(_))) {
                continue;
            }

            parameters.push(Parameter::Query {
                parameter_data: ParameterData {
                    name: name.clone(),
                    description: None,
                    required: true, // We can make this configurable if needed
                    deprecated: None,
                    format: ParameterSchemaOrContent::Schema(
                        self.type_converter.convert_wit_type(type_info)?,
                    ),
                    example: None,
                    examples: Default::default(),
                    explode: None,
                    extensions: Default::default(),
                },
                allow_empty_value: false,
                style: Default::default(),
                allow_reserved: false,
            });
        }

        Ok(parameters)
    }

    fn add_security_scheme(&self, auth: &SecurityWithProviderMetadata, operation: &mut Operation) -> Result<()> {
        // Add security scheme to components
        let components = operation.security.get_or_insert_with(Default::default);
        
        // Map the auth provider to an OpenAPI security scheme
        let scheme = match auth {
            SecurityWithProviderMetadata::Google(_) => SecurityScheme::OAuth2 {
                flows: Default::default(), // Configure OAuth2 flows as needed
            },
            SecurityWithProviderMetadata::Microsoft(_) => SecurityScheme::OAuth2 {
                flows: Default::default(), // Configure OAuth2 flows as needed
            },
            _ => return Err(OpenApiError::ConversionError("Unsupported auth provider".to_string())),
        };

        components.push(scheme);
        Ok(())
    }

    fn add_cors_headers(&self, cors: &CorsPreflight, operation: &mut Operation) -> Result<()> {
        // Add CORS headers to the response headers
        if let Some(response) = operation.responses.responses.get_mut("200") {
            if let ReferenceOr::Item(response) = response {
                // Add CORS headers
                if let Some(allow_origin) = &cors.allow_origin {
                    response.headers.insert(
                        "Access-Control-Allow-Origin".to_string(),
                        ReferenceOr::Item(Header {
                            description: Some("CORS allowed origin".to_string()),
                            required: false,
                            deprecated: false,
                            allow_empty_value: false,
                            schema: Schema {
                                schema_kind: SchemaKind::Type(Type::String(Default::default())),
                                schema_data: Default::default(),
                            },
                            example: Some(serde_json::Value::String(allow_origin.clone())),
                            examples: Default::default(),
                        }),
                    );
                }
                // Add other CORS headers similarly
            }
        }
        Ok(())
    }

    fn add_request_body(&self, input_type: &RibInputType, operation: &mut Operation) -> Result<()> {
        let schema = self.type_converter.convert_input_type(input_type)?;
        
        operation.request_body = Some(ReferenceOr::Item(RequestBody {
            description: Some("Request payload".to_string()),
            content: {
                let mut map = HashMap::new();
                map.insert(
                    "application/json".to_string(),
                    MediaType {
                        schema: Some(ReferenceOr::Item(schema)),
                        ..Default::default()
                    },
                );
                map
            },
            required: true,
        }));
        
        Ok(())
    }

    fn add_response(&self, output_type: &RibOutputType, operation: &mut Operation) -> Result<()> {
        let schema = self.type_converter.convert_output_type(output_type)?;
        
        operation.responses = Responses {
            default: None,
            responses: {
                let mut map = HashMap::new();
                map.insert(
                    "200".to_string(),
                    ReferenceOr::Item(Response {
                        description: "Successful response".to_string(),
                        content: {
                            let mut content_map = HashMap::new();
                            content_map.insert(
                                "application/json".to_string(),
                                MediaType {
                                    schema: Some(ReferenceOr::Item(schema)),
                                    ..Default::default()
                                },
                            );
                            content_map
                        },
                        ..Default::default()
                    }),
                );

                // Add error responses
                map.insert(
                    "400".to_string(),
                    ReferenceOr::Item(Response {
                        description: "Bad Request - Invalid input".to_string(),
                        ..Default::default()
                    }),
                );
                map.insert(
                    "401".to_string(),
                    ReferenceOr::Item(Response {
                        description: "Unauthorized - Authentication required".to_string(),
                        ..Default::default()
                    }),
                );
                map.insert(
                    "403".to_string(),
                    ReferenceOr::Item(Response {
                        description: "Forbidden - Insufficient permissions".to_string(),
                        ..Default::default()
                    }),
                );
                map.insert(
                    "500".to_string(),
                    ReferenceOr::Item(Response {
                        description: "Internal Server Error".to_string(),
                        ..Default::default()
                    }),
                );
                map
            },
        };
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::apidefinition::{
        ApiDefinition, ApiDefinitionId, CompiledGatewayBinding, CompiledHttpApiDefinition,
        CompiledHttpRoute, CorsPreflight, Middleware, SecurityWithProviderMetadata,
    };
    use golem_api_grpc::proto::golem::rib::{RibInputType, RibOutputType};
    use golem_api_grpc::proto::golem::wasm::ast::{Type, type_::Kind};

    #[test]
    fn test_convert_basic_api() {
        let converter = ApiDefinitionConverter::new();
        
        let api = ApiDefinition {
            id: Some(ApiDefinitionId {
                value: "test-api".to_string(),
            }),
            version: "1.0.0".to_string(),
            http_api: Some(CompiledHttpApiDefinition {
                routes: vec![
                    CompiledHttpRoute {
                        method: HttpMethod::Get as i32,
                        path: "/hello".to_string(),
                        binding: None,
                        middleware: None,
                    },
                ],
            }),
            ..Default::default()
        };

        let result = converter.convert(&api);
        assert!(result.is_ok());
        
        let openapi = result.unwrap();
        assert_eq!(openapi.openapi, "3.0.3");
        assert!(openapi.paths.paths.contains_key("/hello"));
    }

    #[test]
    fn test_path_parameter_extraction() {
        let converter = ApiDefinitionConverter::new();
        
        // Test single parameter
        let params = converter.extract_path_parameters("/users/{id}").unwrap();
        assert_eq!(params.len(), 1);
        if let Parameter::Path { parameter_data, .. } = &params[0] {
            assert_eq!(parameter_data.name, "id");
            assert!(parameter_data.required);
        } else {
            panic!("Expected path parameter");
        }

        // Test multi-segment parameter
        let params = converter.extract_path_parameters("/files/{path..}").unwrap();
        assert_eq!(params.len(), 1);
        if let Parameter::Path { parameter_data, .. } = &params[0] {
            assert_eq!(parameter_data.name, "path");
            assert!(parameter_data.required);
            if let ParameterSchemaOrContent::Schema(schema) = &parameter_data.format {
                assert!(matches!(schema.schema_kind, SchemaKind::Type(Type::Array(_))));
            } else {
                panic!("Expected array schema");
            }
        } else {
            panic!("Expected path parameter");
        }

        // Test multiple parameters
        let params = converter.extract_path_parameters("/users/{userId}/posts/{postId}").unwrap();
        assert_eq!(params.len(), 2);
        if let Parameter::Path { parameter_data, .. } = &params[0] {
            assert_eq!(parameter_data.name, "userId");
        }
        if let Parameter::Path { parameter_data, .. } = &params[1] {
            assert_eq!(parameter_data.name, "postId");
        }
    }

    #[test]
    fn test_full_api_definition_conversion() {
        let converter = ApiDefinitionConverter::new();
        
        // Create a complex API Definition with various routes and bindings
        let api = ApiDefinition {
            id: Some(ApiDefinitionId {
                value: "test-complex-api".to_string(),
            }),
            version: "1.0.0".to_string(),
            http_api: Some(CompiledHttpApiDefinition {
                routes: vec![
                    // Route with Rib binding and complex types
                    CompiledHttpRoute {
                        method: HttpMethod::Post as i32,
                        path: "/users/{userId}/documents/{path..}".to_string(),
                        binding: Some(CompiledGatewayBinding {
                            worker_name_rib_input: Some(RibInputType {
                                types: {
                                    let mut map = HashMap::new();
                                    map.insert("userId".to_string(), Type {
                                        kind: Some(Kind::String(Default::default())),
                                    });
                                    map.insert("document".to_string(), Type {
                                        kind: Some(Kind::Record(Default::default())),
                                    });
                                    map
                                },
                            }),
                            response_rib_output: Some(RibOutputType {
                                type_: Some(Type {
                                    kind: Some(Kind::Result(Box::new(Default::default()))),
                                }),
                            }),
                            ..Default::default()
                        }),
                        middleware: Some(Middleware {
                            cors: Some(CorsPreflight {
                                allow_origin: Some("http://localhost:3000".to_string()),
                                allow_methods: vec!["POST".to_string()],
                                allow_headers: vec!["content-type".to_string()],
                                ..Default::default()
                            }),
                            http_authentication: Some(SecurityWithProviderMetadata::Google(Default::default())),
                            ..Default::default()
                        }),
                    }),
                    // Static file server route
                    CompiledHttpRoute {
                        method: HttpMethod::Get as i32,
                        path: "/static/{file..}".to_string(),
                        binding: Some(CompiledGatewayBinding {
                            worker_name_rib_input: None,
                            response_rib_output: None,
                            ..Default::default()
                        }),
                        middleware: None,
                    }),
                ],
            }),
            ..Default::default()
        };

        let result = converter.convert(&api);
        assert!(result.is_ok());
        
        let openapi = result.unwrap();
        
        // Verify OpenAPI version
        assert_eq!(openapi.openapi, "3.0.3");
        
        // Verify paths
        let paths = &openapi.paths.paths;
        assert!(paths.contains_key("/users/{userId}/documents/{path}"));
        assert!(paths.contains_key("/static/{file}"));
        
        // Verify complex path parameters
        if let Some(path_item) = paths.get("/users/{userId}/documents/{path}") {
            if let Some(post) = &path_item.post {
                // Check parameters
                if let Some(parameters) = &post.parameters {
                    assert_eq!(parameters.len(), 2);
                    for param in parameters {
                        match param {
                            Parameter::Path { parameter_data, .. } => {
                                assert!(parameter_data.required);
                                match parameter_data.name.as_str() {
                                    "userId" => {
                                        if let ParameterSchemaOrContent::Schema(schema) = &parameter_data.format {
                                            assert!(matches!(schema.schema_kind, SchemaKind::Type(Type::String(_))));
                                        }
                                    }
                                    "path" => {
                                        if let ParameterSchemaOrContent::Schema(schema) = &parameter_data.format {
                                            assert!(matches!(schema.schema_kind, SchemaKind::Type(Type::Array(_))));
                                        }
                                    }
                                    _ => panic!("Unexpected parameter name"),
                                }
                            }
                            _ => panic!("Expected path parameter"),
                        }
                    }
                }

                // Check request body
                if let Some(request_body) = &post.request_body {
                    let content = &request_body.content;
                    assert!(content.contains_key("application/json"));
                }

                // Check security
                assert!(post.security.is_some());

                // Check CORS headers in responses
                if let Some(responses) = &post.responses.responses.get("200") {
                    if let ReferenceOr::Item(response) = responses {
                        assert!(response.headers.contains_key("Access-Control-Allow-Origin"));
                    }
                }
            }
        }
    }

    #[test]
    fn test_rib_type_conversion() -> Result<()> {
        let mut input_types = HashMap::new();
        
        // Basic types
        input_types.insert("string_field".to_string(), Type {
            kind: Some(Kind::String(Default::default())),
        });
        input_types.insert("i32_field".to_string(), Type {
            kind: Some(Kind::I32(Default::default())),
        });
        input_types.insert("i64_field".to_string(), Type {
            kind: Some(Kind::I64(Default::default())),
        });
        input_types.insert("f32_field".to_string(), Type {
            kind: Some(Kind::F32(Default::default())),
        });
        input_types.insert("f64_field".to_string(), Type {
            kind: Some(Kind::F64(Default::default())),
        });
        input_types.insert("bool_field".to_string(), Type {
            kind: Some(Kind::Bool(Default::default())),
        });

        // Complex types
        input_types.insert("array_field".to_string(), Type {
            kind: Some(Kind::List(Box::new(Type {
                kind: Some(Kind::String(Default::default())),
            }))),
        });
        input_types.insert("option_field".to_string(), Type {
            kind: Some(Kind::Option(Box::new(Type {
                kind: Some(Kind::I32(Default::default())),
            }))),
        });
        input_types.insert("result_field".to_string(), Type {
            kind: Some(Kind::Result(Box::new(Default::default()))), // ok: string, err: string
        });
        input_types.insert("tuple_field".to_string(), Type {
            kind: Some(Kind::Tuple(Default::default())), // (string, i32)
        });
        input_types.insert("map_field".to_string(), Type {
            kind: Some(Kind::Map(Box::new(Default::default()))), // string -> string
        });

        let api = ApiDefinition {
            id: Some(ApiDefinitionId {
                value: "test-api".to_string(),
            }),
            version: "1.0.0".to_string(),
            http_api: Some(CompiledHttpApiDefinition {
                routes: vec![CompiledHttpRoute {
                    method: 1,
                    path: "/api/test".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        worker_name_rib_input: Some(RibInputType {
                            types: input_types,
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
            }),
            ..Default::default()
        };

        let converter = ApiDefinitionConverter::new();
        let openapi = converter.convert(&api)?;
        
        let path = openapi.paths.get("/api/test").expect("Path should exist");
        let get = path.get.as_ref().expect("GET operation should exist");
        let parameters = get.parameters.as_ref().expect("Parameters should exist");

        // Verify each type conversion
        for param in parameters {
            if let Parameter::Query { parameter_data, .. } = param {
                match parameter_data.name.as_str() {
                    "string_field" => assert_matches!(
                        parameter_data.format,
                        ParameterSchemaOrContent::Schema(Schema::Type(Type::String(_)))
                    ),
                    "i32_field" => assert_matches!(
                        parameter_data.format,
                        ParameterSchemaOrContent::Schema(Schema::Type(Type::Integer(_)))
                    ),
                    "array_field" => assert_matches!(
                        parameter_data.format,
                        ParameterSchemaOrContent::Schema(Schema::Array { .. })
                    ),
                    "option_field" => {
                        assert!(!parameter_data.required);
                    },
                    "map_field" => assert_matches!(
                        parameter_data.format,
                        ParameterSchemaOrContent::Schema(Schema::Object { .. })
                    ),
                    _ => {}
                }
            }
        }

        Ok(())
    }

    #[test]
    fn test_file_server_binding() -> Result<()> {
        let api = ApiDefinition {
            id: Some(ApiDefinitionId {
                value: "test-api".to_string(),
            }),
            version: "1.0.0".to_string(),
            http_api: Some(CompiledHttpApiDefinition {
                routes: vec![CompiledHttpRoute {
                    method: 1,
                    path: "/static/{path..}".to_string(),
                    binding: Some(CompiledGatewayBinding {
                        binding_type: GatewayBindingType::StaticBinding as i32,
                        static_binding: Some(StaticBinding {
                            root_dir: "static".to_string(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
            }),
            ..Default::default()
        };

        let converter = ApiDefinitionConverter::new();
        let openapi = converter.convert(&api)?;
        
        let path = openapi.paths.get("/static/{path}").expect("Path should exist");
        let get = path.get.as_ref().expect("GET operation should exist");
        
        // Verify path parameter
        let parameters = get.parameters.as_ref().expect("Parameters should exist");
        assert_eq!(parameters.len(), 1);
        
        // Verify response content types
        let responses = get.responses.as_ref().expect("Responses should exist");
        let ok_response = responses.get("200").expect("200 response should exist");
        let content = ok_response.content.as_ref().expect("Content should exist");
        
        // Should support common file types
        assert!(content.contains_key("application/json"));
        assert!(content.contains_key("text/html"));
        assert!(content.contains_key("image/*"));
        assert!(content.contains_key("application/octet-stream"));

        Ok(())
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use axum::{
        Router,
        routing::{get, post},
        extract::{Path, Query},
        response::IntoResponse,
        Json,
    };
    use serde::{Deserialize, Serialize};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use reqwest;

    #[derive(Debug, Serialize, Deserialize)]
    struct User {
        id: String,
        name: String,
        age: i32,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct UserQuery {
        name: Option<String>,
        min_age: Option<i32>,
    }

    async fn get_user(Path(id): Path<String>) -> impl IntoResponse {
        Json(User {
            id,
            name: "Test User".to_string(),
            age: 30,
        })
    }

    async fn search_users(Query(query): Query<UserQuery>) -> impl IntoResponse {
        let user = User {
            id: "1".to_string(),
            name: query.name.unwrap_or_else(|| "Default User".to_string()),
            age: query.min_age.unwrap_or(0),
        };
        Json(vec![user])
    }

    #[tokio::test]
    async fn test_openapi_with_real_server() -> Result<()> {
        // Create API Definition
        let api = ApiDefinition {
            id: Some(ApiDefinitionId {
                value: "test-api".to_string(),
            }),
            version: "1.0.0".to_string(),
            http_api: Some(CompiledHttpApiDefinition {
                routes: vec![
                    CompiledHttpRoute {
                        method: 0, // GET
                        path: "/api/users/{id}".to_string(),
                        binding: Some(CompiledGatewayBinding {
                            worker_name_rib_input: Some(RibInputType {
                                types: {
                                    let mut map = HashMap::new();
                                    map.insert("id".to_string(), Type {
                                        kind: Some(Kind::String(Default::default())),
                                    });
                                    map
                                },
                            }),
                            response_rib_output: Some(RibOutputType {
                                type_: Some(Type {
                                    kind: Some(Kind::Record(Default::default())),
                                }),
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    CompiledHttpRoute {
                        method: 0, // GET
                        path: "/api/users".to_string(),
                        binding: Some(CompiledGatewayBinding {
                            worker_name_rib_input: Some(RibInputType {
                                types: {
                                    let mut map = HashMap::new();
                                    map.insert("name".to_string(), Type {
                                        kind: Some(Kind::Option(Box::new(Type {
                                            kind: Some(Kind::String(Default::default())),
                                        }))),
                                    });
                                    map.insert("min_age".to_string(), Type {
                                        kind: Some(Kind::Option(Box::new(Type {
                                            kind: Some(Kind::I32(Default::default())),
                                        }))),
                                    });
                                    map
                                },
                            }),
                            response_rib_output: Some(RibOutputType {
                                type_: Some(Type {
                                    kind: Some(Kind::List(Box::new(Type {
                                        kind: Some(Kind::Record(Default::default())),
                                    }))),
                                }),
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        };

        // Convert to OpenAPI
        let converter = ApiDefinitionConverter::new();
        let openapi = converter.convert(&api)?;

        // Create test server
        let app = Router::new()
            .route("/api/users/:id", get(get_user))
            .route("/api/users", get(search_users));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Test the endpoints using a generated client
        let client = reqwest::Client::new();

        // Test get user by id
        let response = client
            .get(&format!("http://{}/api/users/123", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let user: User = response.json().await.unwrap();
        assert_eq!(user.id, "123");

        // Test search users with query parameters
        let response = client
            .get(&format!("http://{}/api/users", addr))
            .query(&[("name", "Test"), ("min_age", "25")])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let users: Vec<User> = response.json().await.unwrap();
        assert!(!users.is_empty());
        assert!(users[0].age >= 25);

        Ok(())
    }
}
