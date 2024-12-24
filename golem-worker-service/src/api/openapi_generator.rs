// openapi_generator.rs
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

use crate::api::definition::types::{ApiDefinition, BindingType, Route};
use crate::api::openapi::{
    Components, MediaType, OpenAPISpec, Operation, Parameter, ParameterLocation, PathItem,
    Response, Schema,
};
use golem_worker_service_base::api::rib_to_openapi::{OpenApiSchema, RibToOpenApi};
use std::collections::{BTreeMap, HashMap};

pub struct OpenApiGenerator;

impl OpenApiGenerator {
    /// Generates an OpenAPI specification from an ApiDefinition.
    pub fn generate(api_definition: &ApiDefinition) -> OpenAPISpec {
        OpenAPISpec {
            openapi: "3.0.0".to_string(),
            info: crate::api::openapi::Info {
                title: api_definition.name.clone(),
                version: api_definition.version.clone(),
                description: Some(api_definition.description.clone()),
            },
            paths: Self::generate_paths(&api_definition.routes),
            components: Some(Self::generate_components(&api_definition.routes)),
            security: Some(vec![HashMap::from([("bearerAuth".to_string(), vec![])])]),
        }
    }

    /// Converts routes into OpenAPI paths.
    fn generate_paths(routes: &[Route]) -> HashMap<String, PathItem> {
        routes
            .iter()
            .map(|route| {
                let mut path_item = PathItem {
                    get: Some(Operation {
                        summary: Some(route.description.clone()),
                        description: Some(route.description.clone()),
                        operation_id: Some(format!("{}_{}", route.method, route.component_name)),
                        parameters: Some(Self::generate_parameters(route)),
                        request_body: None,
                        responses: Self::generate_responses(route),
                        security: Some(vec![HashMap::from([("bearerAuth".to_string(), vec![])])]),
                        tags: Some(vec![route.component_name.clone()]),
                    }),
                    post: None,
                    put: None,
                    delete: None,
                    options: None,
                    patch: None,
                    trace: None,
                    parameters: None,
                };

                // Add CORS headers to the responses
                if let Some(responses) = path_item.get.as_mut().map(|op| &mut op.responses) {
                    if let Some(cors_response) = responses.get_mut("200") {
                        cors_response.headers = Some(HashMap::from([
                            (
                                "Access-Control-Allow-Origin".to_string(),
                                Parameter {
                                    name: "Access-Control-Allow-Origin".to_string(),
                                    r#in: ParameterLocation::Header,
                                    description: Some("CORS origin".to_string()),
                                    required: Some(false),
                                    schema: Schema::String {
                                        format: None,
                                        enum_values: None,
                                        default: None,
                                        pattern: None,
                                        min_length: None,
                                        max_length: None,
                                    },
                                    style: Some("simple".to_string()),
                                    explode: Some(false),
                                    example: Some(
                                        "https://example.com".to_string().parse().unwrap(),
                                    ),
                                },
                            ),
                            (
                                "Access-Control-Allow-Methods".to_string(),
                                Parameter {
                                    name: "Access-Control-Allow-Methods".to_string(),
                                    r#in: ParameterLocation::Header,
                                    description: Some("CORS methods".to_string()),
                                    required: Some(false),
                                    schema: Schema::String {
                                        format: None,
                                        enum_values: None,
                                        default: None,
                                        pattern: None,
                                        min_length: None,
                                        max_length: None,
                                    },
                                    style: Some("simple".to_string()),
                                    explode: Some(false),
                                    example: Some(
                                        "GET, POST, PUT, DELETE".to_string().parse().unwrap(),
                                    ),
                                },
                            ),
                        ]));
                    }
                }
                (route.path.clone(), path_item)
            })
            .collect()
    }

    /// Converts route parameters into OpenAPI parameters.
    fn generate_parameters(route: &Route) -> Vec<Parameter> {
        if let BindingType::Default { input_type, .. } = &route.binding {
            if let OpenApiSchema::Object {
                properties,
                required,
                ..
            } = RibToOpenApi::convert_type(input_type).unwrap_or(OpenApiSchema::Object {
                properties: BTreeMap::new(),
                required: Some(vec![]),
                additional_properties: None,
            }) {
                properties
                    .iter()
                    .map(|(name, schema)| Parameter {
                        name: name.clone(),
                        r#in: if required.as_ref().unwrap_or(&vec![]).contains(name) {
                            ParameterLocation::Path
                        } else {
                            ParameterLocation::Query
                        },
                        description: None,
                        required: Some(required.as_ref().unwrap_or(&vec![]).contains(name)),
                        schema: Self::convert_openapi_schema(schema),
                        style: Some("form".to_string()),
                        explode: Some(true),
                        example: None,
                    })
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    }

    /// Generates responses for a route.
    fn generate_responses(route: &Route) -> HashMap<String, Response> {
        let mut responses = HashMap::new();

        // Success response
        responses.insert(
            "200".to_string(),
            Response {
                description: "Successful operation".to_string(),
                content: Some(HashMap::from([(
                    "application/json".to_string(),
                    MediaType {
                        schema: match &route.binding {
                            BindingType::Default { output_type, .. } => {
                                Self::convert_openapi_schema(
                                    &RibToOpenApi::convert_type(output_type)
                                        .expect("Failed to convert output type"),
                                )
                            }
                            _ => Schema::String {
                                format: None,
                                enum_values: None,
                                default: None,
                                pattern: None,
                                min_length: None,
                                max_length: None,
                            },
                        },
                        example: None,
                    },
                )])),
                headers: None,
            },
        );

        // Standard error responses
        responses.insert(
            "400".to_string(),
            Response {
                description: "Bad request".to_string(),
                content: None,
                headers: None,
            },
        );

        responses
    }

    /// Generates reusable components for OpenAPI.
    fn generate_components(routes: &[Route]) -> Components {
        let mut schemas = HashMap::new();
        for route in routes {
            if let BindingType::Default { output_type, .. } = &route.binding {
                if let Some(schema) = RibToOpenApi::convert_type(output_type) {
                    schemas.insert(
                        route.component_name.clone(),
                        Self::convert_openapi_schema(&schema),
                    );
                }
            }
        }
        Components {
            schemas: Some(schemas),
            responses: None,
            parameters: None,
            security_schemes: None,
        }
    }

    /// Converts `OpenApiSchema` into `Schema` for OpenAPI compatibility.
    fn convert_openapi_schema(openapi_schema: &OpenApiSchema) -> Schema {
        match openapi_schema {
            OpenApiSchema::Boolean => Schema::Boolean { default: None },
            OpenApiSchema::Integer { format } => Schema::Integer {
                format: format.clone(),
                default: None,
                minimum: None,
                maximum: None,
            },
            OpenApiSchema::Number { format } => Schema::Number {
                format: format.clone(),
                default: None,
                minimum: None,
                maximum: None,
            },
            OpenApiSchema::String {
                format,
                enum_values,
            } => Schema::String {
                format: format.clone(),
                enum_values: enum_values.clone(),
                default: None,
                pattern: None,
                min_length: None,
                max_length: None,
            },
            OpenApiSchema::Array { items } => Schema::Array {
                items: Box::new(Self::convert_openapi_schema(items)),
            },
            OpenApiSchema::Object {
                properties,
                required,
                additional_properties,
            } => Schema::Object {
                properties: properties
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::convert_openapi_schema(value)))
                    .collect(),
                required: required.clone(),
                additional_properties: additional_properties
                    .as_ref()
                    .map(|schema| Box::new(Self::convert_openapi_schema(schema))),
            },
            OpenApiSchema::Enum { values } => Schema::String {
                format: None,
                enum_values: Some(values.clone()),
                default: None,
                pattern: None,
                min_length: None,
                max_length: None,
            },
            OpenApiSchema::Ref { reference } => Schema::Ref {
                reference: reference.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::definition::types::{ApiDefinition, BindingType, HttpMethod, Route};
    use golem_wasm_ast::analysis::{model, AnalysedType, NameTypePair, TypeRecord};

    fn create_route() -> Route {
        Route {
            path: "/example".to_string(),
            method: HttpMethod::Get,
            description: "Example route".to_string(),
            component_name: "ExampleComponent".to_string(),
            binding: BindingType::Default {
                input_type: AnalysedType::Str(model::TypeStr {}),
                output_type: AnalysedType::Bool(model::TypeBool {}),
                function_name: "example_function".to_string(),
            },
        }
    }

    #[test]
    fn test_generate_openapi_basic() {
        // Define a basic API definition with a single route
        let api_definition = ApiDefinition {
            id: "test_api".to_string(),
            name: "Test API".to_string(),
            version: "1.0.0".to_string(),
            description: "A test API definition".to_string(),
            routes: vec![create_route()],
        };

        // Generate the OpenAPI specification
        let openapi = OpenApiGenerator::generate(&api_definition);

        // Validate basic OpenAPI structure
        assert_eq!(openapi.openapi, "3.0.0");
        assert_eq!(openapi.info.title, "Test API");
        assert_eq!(openapi.info.version, "1.0.0");
        assert_eq!(
            openapi.info.description.as_deref(),
            Some("A test API definition")
        );
        assert!(openapi.paths.contains_key("/example"));
    }

    #[test]
    fn test_generate_paths() {
        // Define a basic route
        let route = create_route();

        // Generate paths for the route
        let paths = OpenApiGenerator::generate_paths(&[route]);

        // Validate the generated path exists
        assert!(paths.contains_key("/example"));

        // Validate the properties of the generated path
        let path_item = paths.get("/example").unwrap();
        assert!(path_item.get.is_some());
        let operation = path_item.get.as_ref().unwrap();
        assert_eq!(operation.summary.as_deref(), Some("Example route"));
        assert_eq!(operation.description.as_deref(), Some("Example route"));
        assert_eq!(
            operation.operation_id.as_deref(),
            Some("GET_ExampleComponent")
        );
        assert!(operation.parameters.is_some());
    }

    #[test]
    fn test_generate_responses() {
        // Define a basic route with output type
        let route = create_route();

        // Generate responses for the route
        let responses = OpenApiGenerator::generate_responses(&route);

        // Validate the success response
        assert!(responses.contains_key("200"));
        let response = responses.get("200").unwrap();
        assert_eq!(response.description, "Successful operation");
        assert!(response.content.is_some());
        let content = response.content.as_ref().unwrap();
        assert!(content.contains_key("application/json"));

        // Validate the error response
        assert!(responses.contains_key("400"));
        let error_response = responses.get("400").unwrap();
        assert_eq!(error_response.description, "Bad request");
        assert!(error_response.content.is_none());
    }

    #[test]
    fn test_generate_components() {
        // Define routes with output types
        let routes = vec![
            Route {
                path: "/example1".to_string(),
                method: HttpMethod::Get,
                description: "Example route 1".to_string(),
                component_name: "ExampleComponent1".to_string(),
                binding: BindingType::Default {
                    input_type: AnalysedType::Str(model::TypeStr {}),
                    output_type: AnalysedType::Bool(model::TypeBool {}),
                    function_name: "example_function1".to_string(),
                },
            },
            Route {
                path: "/example2".to_string(),
                method: HttpMethod::Post,
                description: "Example route 2".to_string(),
                component_name: "ExampleComponent2".to_string(),
                binding: BindingType::Default {
                    input_type: AnalysedType::U32(model::TypeU32 {}),
                    output_type: AnalysedType::Str(model::TypeStr {}),
                    function_name: "example_function2".to_string(),
                },
            },
        ];

        // Generate components from the routes
        let components = OpenApiGenerator::generate_components(&routes);

        // Validate the components include the correct schemas
        assert!(components
            .schemas
            .as_ref()
            .unwrap()
            .contains_key("ExampleComponent1"));
        assert!(components
            .schemas
            .as_ref()
            .unwrap()
            .contains_key("ExampleComponent2"));
    }

    #[test]
    fn test_generate_parameters() {
        // Define a route with record input type
        let route = Route {
            path: "/example".to_string(),
            method: HttpMethod::Get,
            description: "Example route".to_string(),
            component_name: "ExampleComponent".to_string(),
            binding: BindingType::Default {
                input_type: AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "param1".to_string(),
                            typ: AnalysedType::Str(model::TypeStr {}),
                        },
                        NameTypePair {
                            name: "param2".to_string(),
                            typ: AnalysedType::U32(model::TypeU32 {}),
                        },
                    ],
                }),
                output_type: AnalysedType::Bool(model::TypeBool {}),
                function_name: "example_function".to_string(),
            },
        };

        // Generate parameters for the route
        let parameters = OpenApiGenerator::generate_parameters(&route);

        // Validate the generated parameters
        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters[0].name, "param1");
        assert_eq!(parameters[1].name, "param2");
    }

    #[test]
    fn test_generate_parameters_with_nested_object() {
        // Define a route with a nested object input type
        let route = Route {
            path: "/example".to_string(),
            method: HttpMethod::Get,
            description: "Example route with nested object".to_string(),
            component_name: "NestedComponent".to_string(),
            binding: BindingType::Default {
                input_type: AnalysedType::Record(TypeRecord {
                    fields: vec![NameTypePair {
                        name: "nested_object".to_string(),
                        typ: AnalysedType::Record(TypeRecord {
                            fields: vec![NameTypePair {
                                name: "inner_field".to_string(),
                                typ: AnalysedType::U32(model::TypeU32 {}),
                            }],
                        }),
                    }],
                }),
                output_type: AnalysedType::Bool(model::TypeBool {}),
                function_name: "example_function".to_string(),
            },
        };

        // Generate parameters for the route
        let parameters = OpenApiGenerator::generate_parameters(&route);

        // Validate the generated parameter includes the nested object
        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters[0].name, "nested_object");
    }

    /// Test for generating paths with multiple HTTP methods
    #[test]
    fn test_generate_paths_with_multiple_methods() {
        let routes = vec![
            Route {
                path: "/example".to_string(),
                method: HttpMethod::Get,
                description: "GET example".to_string(),
                component_name: "ExampleComponent".to_string(),
                binding: BindingType::Default {
                    input_type: AnalysedType::Str(model::TypeStr {}),
                    output_type: AnalysedType::Bool(model::TypeBool {}),
                    function_name: "example_get".to_string(),
                },
            },
            Route {
                path: "/example".to_string(),
                method: HttpMethod::Post,
                description: "POST example".to_string(),
                component_name: "ExampleComponent".to_string(),
                binding: BindingType::Default {
                    input_type: AnalysedType::Str(model::TypeStr {}),
                    output_type: AnalysedType::Bool(model::TypeBool {}),
                    function_name: "example_post".to_string(),
                },
            },
        ];

        let paths = OpenApiGenerator::generate_paths(&routes);

        assert!(paths.contains_key("/example"));

        if let Some(PathItem { get, post, .. }) = paths.get("/example") {
            assert!(get.is_some(), "GET method should be present");
            assert!(post.is_some(), "POST method should be present");
        } else {
            panic!("Expected PathItem for /example");
        }
    }

    /// Test for generating paths with CORS headers
    #[test]
    fn test_generate_paths_with_cors_headers() {
        let routes = vec![Route {
            path: "/example".to_string(),
            method: HttpMethod::Get,
            description: "GET example with CORS".to_string(),
            component_name: "ExampleComponent".to_string(),
            binding: BindingType::Default {
                input_type: AnalysedType::Str(model::TypeStr {}),
                output_type: AnalysedType::Bool(model::TypeBool {}),
                function_name: "example_cors".to_string(),
            },
        }];

        let paths = OpenApiGenerator::generate_paths(&routes);

        if let Some(PathItem { get, .. }) = paths.get("/example") {
            if let Some(operation) = get {
                let responses = &operation.responses;
                if let Some(response) = responses.get("200") {
                    if let Some(headers) = &response.headers {
                        assert!(
                            headers.contains_key("Access-Control-Allow-Origin"),
                            "CORS header missing"
                        );
                    } else {
                        panic!("Expected headers in response");
                    }
                } else {
                    panic!("Expected 200 response");
                }
            } else {
                panic!("Expected GET operation");
            }
        } else {
            panic!("Expected PathItem for /example");
        }
    }

    /// Test for generating components with complex schemas
    #[test]
    fn test_generate_components_with_complex_schemas() {
        let routes = vec![Route {
            path: "/complex".to_string(),
            method: HttpMethod::Get,
            description: "Complex schema route".to_string(),
            component_name: "ComplexComponent".to_string(),
            binding: BindingType::Default {
                input_type: AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "field1".to_string(),
                            typ: AnalysedType::Str(model::TypeStr {}),
                        },
                        NameTypePair {
                            name: "field2".to_string(),
                            typ: AnalysedType::List(model::TypeList {
                                inner: Box::new(AnalysedType::U32(model::TypeU32 {})),
                            }),
                        },
                    ],
                }),
                output_type: AnalysedType::Bool(model::TypeBool {}),
                function_name: "complex_function".to_string(),
            },
        }];

        let components = OpenApiGenerator::generate_components(&routes);

        assert!(
            components.schemas.is_some(),
            "Expected schemas in components"
        );

        if let Some(schemas) = components.schemas {
            assert!(
                schemas.contains_key("ComplexComponent"),
                "Missing ComplexComponent schema"
            );
        } else {
            panic!("Expected schemas in components");
        }
    }

    /// Test for generating parameters with nested structures
    #[test]
    fn test_generate_parameters_with_nested_structures() {
        let route = Route {
            path: "/nested".to_string(),
            method: HttpMethod::Get,
            description: "Nested parameters route".to_string(),
            component_name: "NestedComponent".to_string(),
            binding: BindingType::Default {
                input_type: AnalysedType::Record(TypeRecord {
                    fields: vec![NameTypePair {
                        name: "nested".to_string(),
                        typ: AnalysedType::Record(TypeRecord {
                            fields: vec![NameTypePair {
                                name: "inner_field".to_string(),
                                typ: AnalysedType::U64(model::TypeU64 {}),
                            }],
                        }),
                    }],
                }),
                output_type: AnalysedType::Bool(model::TypeBool {}),
                function_name: "nested_function".to_string(),
            },
        };

        let parameters = OpenApiGenerator::generate_parameters(&route);

        assert_eq!(parameters.len(), 1, "Expected one parameter");
        assert_eq!(
            parameters[0].name, "nested",
            "Expected parameter name 'nested'"
        );
    }
}
