use super::types::*;
use crate::api::definition::{
    types::{ApiDefinition, Route, HttpMethod, BindingType},
    patterns::{AllPathPatterns, PathPattern},
};
use std::collections::HashMap;
use heck::ToSnakeCase; 

pub struct OpenAPIConverter;

impl OpenAPIConverter {
    pub fn convert(api: &ApiDefinition) -> OpenAPISpec {
        let converter = OpenAPIConverter;
        OpenAPISpec {
            openapi: "3.0.0".to_string(),
            info: Info {
                title: "Golem API".to_string(),
                version: "1.0".to_string(),
                description: None,
            },
            paths: converter.convert_paths(&api.routes),
            components: Some(Self::create_components(&api.routes)),
            security: None,
        }
    }

    fn convert_paths(&self, routes: &[Route]) -> HashMap<String, PathItem> {
        let mut paths = HashMap::new();

        for route in routes {
            let operation = self.generate_operation(route);

            let path_item = PathItem {
                get: if route.method == HttpMethod::Get { Some(operation.clone()) } else { None },
                post: if route.method == HttpMethod::Post { Some(operation.clone()) } else { None },
                put: if route.method == HttpMethod::Put { Some(operation.clone()) } else { None },
                delete: if route.method == HttpMethod::Delete { Some(operation.clone()) } else { None },
                options: Some(Operation {
                    responses: {
                        let mut map = HashMap::new();
                        map.insert("200".to_string(), Response {
                            description: String::new(),
                            content: None,
                            headers: Some(Self::create_cors_headers("*")),
                        });
                        map
                    },
                       ..operation
                    }),
                parameters: None,
            };

            paths.insert(route.path.clone(), path_item);
        }

        paths
    }

    fn generate_operation(&self, route: &Route) -> Operation {
        match &route.binding {
            BindingType::Default { .. } => {
                Operation {
                    summary: Some(route.description.clone()),
                    description: None,
                    operation_id: Some(format!("{}_{}",
                        route.template_name.to_snake_case(),
                        route.method.to_string().to_lowercase())),
                    parameters: {
                        let mut params = Self::extract_path_parameters(&route.path).unwrap_or_default();
                        params.extend(Self::extract_query_parameters(route));
                        params.extend(Self::extract_header_parameters(route));
                        if params.is_empty() { None } else { Some(params) }
                    },
                    request_body: Self::create_request_body(route),
                    responses: {
                        let mut map = Self::create_responses(route, "*");
                        // Add CORS headers to all responses
                        for response in map.values_mut() {
                            response.headers = Some(Self::create_cors_headers("*"));
                        }
                        map
                    },
                    security: None,
                    tags: Some(vec![route.template_name.clone()]),
                }
            },
            BindingType::FileServer { .. } => {
                Operation {
                    summary: Some(route.description.clone()),
                    description: None,
                    operation_id: Some(format!("{}_{}",
                        route.template_name.to_snake_case(),
                        route.method.to_string().to_lowercase())),
                    parameters: {
                        let mut params = Self::extract_path_parameters(&route.path).unwrap_or_default();
                        params.extend(Self::extract_query_parameters(route));
                        params.extend(Self::extract_header_parameters(route));
                        if params.is_empty() { None } else { Some(params) }
                    },
                    request_body: Self::create_request_body(route),
                    responses: {
                        let mut map = Self::create_responses(route, "*");
                        // Add CORS headers to all responses
                        for response in map.values_mut() {
                            response.headers = Some(Self::create_cors_headers("*"));
                        }
                        map
                    },
                    security: None,
                    tags: Some(vec![route.template_name.clone()]),
                }
            },
            BindingType::SwaggerUI { .. } => {
                Operation {
                    summary: Some(route.description.clone()),
                    description: None,
                    operation_id: Some(format!("{}_{}",
                        route.template_name.to_snake_case(),
                        route.method.to_string().to_lowercase())),
                    parameters: {
                        let mut params = Self::extract_path_parameters(&route.path).unwrap_or_default();
                        params.extend(Self::extract_query_parameters(route));
                        params.extend(Self::extract_header_parameters(route));
                        if params.is_empty() { None } else { Some(params) }
                    },
                    request_body: Self::create_request_body(route),
                    responses: {
                        let mut map = Self::create_responses(route, "*");
                        // Add CORS headers to all responses
                        for response in map.values_mut() {
                            response.headers = Some(Self::create_cors_headers("*"));
                        }
                        map
                    },
                    security: None,
                    tags: Some(vec![route.template_name.clone()]),
                }
            },
            BindingType::Http => Operation {
                summary: Some(route.description.clone()),
                description: None,
                operation_id: Some(format!("{}_{}",
                    route.template_name.to_snake_case(),
                    route.method.to_string().to_lowercase())),
                parameters: {
                    let mut params = Self::extract_path_parameters(&route.path).unwrap_or_default();
                    params.extend(Self::extract_query_parameters(route));
                    params.extend(Self::extract_header_parameters(route));
                    if params.is_empty() { None } else { Some(params) }
                },
                request_body: Self::create_request_body(route),
                responses: {
                    let mut map = Self::create_responses(route, "*");
                    // Add CORS headers to all responses
                    for response in map.values_mut() {
                        response.headers = Some(Self::create_cors_headers("*"));
                    }
                    map
                },
                security: None,
                tags: Some(vec![route.template_name.clone()]),
            },
            BindingType::Worker => Operation {
                summary: Some(route.description.clone()),
                description: None,
                operation_id: Some(format!("{}_{}",
                    route.template_name.to_snake_case(),
                    route.method.to_string().to_lowercase())),
                parameters: {
                    let mut params = Self::extract_path_parameters(&route.path).unwrap_or_default();
                    params.extend(Self::extract_query_parameters(route));
                    params.extend(Self::extract_header_parameters(route));
                    if params.is_empty() { None } else { Some(params) }
                },
                request_body: Self::create_request_body(route),
                responses: {
                    let mut map = Self::create_responses(route, "*");
                    // Add CORS headers to all responses
                    for response in map.values_mut() {
                        response.headers = Some(Self::create_cors_headers("*"));
                    }
                    map
                },
                security: None,
                tags: Some(vec![route.template_name.clone()]),
            },
            BindingType::Proxy => Operation {
                summary: Some(route.description.clone()),
                description: None,
                operation_id: Some(format!("{}_{}",
                    route.template_name.to_snake_case(),
                    route.method.to_string().to_lowercase())),
                parameters: {
                    let mut params = Self::extract_path_parameters(&route.path).unwrap_or_default();
                    params.extend(Self::extract_query_parameters(route));
                    params.extend(Self::extract_header_parameters(route));
                    if params.is_empty() { None } else { Some(params) }
                },
                request_body: Self::create_request_body(route),
                responses: {
                    let mut map = Self::create_responses(route, "*");
                    // Add CORS headers to all responses
                    for response in map.values_mut() {
                        response.headers = Some(Self::create_cors_headers("*"));
                    }
                    map
                },
                security: None,
                tags: Some(vec![route.template_name.clone()]),
            },
        }
    }

    fn extract_path_parameters(path: &str) -> Option<Vec<Parameter>> {
        // Use the official parser to parse the path pattern
        let path_pattern = match AllPathPatterns::parse(path) {
            Ok(pattern) => pattern,
            Err(_) => return None
        };

        // Extract parameters from path patterns
        let params: Vec<Parameter> = path_pattern.path_patterns
            .iter()
            .filter_map(|pattern| match pattern {
                PathPattern::Var(info) => Some(Parameter {
                    name: info.key_name.clone(),
                    r#in: ParameterLocation::Path,
                    description: None, 
                    required: Some(true),
                    schema: if info.key_name.ends_with("_id") {
                        Schema::String {
                            format: Some("uuid".to_string()),
                            enum_values: None
                        }
                    } else {
                        Schema::String {
                            format: None,
                            enum_values: None
                        }
                    },
                    style: Some("simple".to_string()),
                    explode: Some(true),
                }),
                PathPattern::CatchAllVar(info) => Some(Parameter {
                    name: info.key_name.clone(), 
                    r#in: ParameterLocation::Path,
                    description: Some("Matches one or more path segments".to_string()),
                    required: Some(true),
                    schema: Schema::String {
                        format: None,
                        enum_values: None
                    },
                    style: Some("simple".to_string()),
                    explode: Some(true),
                }),
                _ => None
            })
            .collect();

        if params.is_empty() {
            None
        } else {
            Some(params)
        }
    }

    fn extract_query_parameters(route: &Route) -> Vec<Parameter> {
         let mut params = Vec::new();

        if route.path.contains("/workers") && route.method == HttpMethod::Get {
            params.push(
                Parameter {
                    name: "filter".to_string(),
                    r#in: ParameterLocation::Query,
                    schema: Schema::Array {
                        items: Box::new(Schema::String {
                            format: None,
                            enum_values: None
                        })
                    },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                     description: Some("Filter criteria for workers".to_string()),  // Added description
                }
            );
           params.push(
                Parameter {
                    name: "cursor".to_string(),
                    r#in: ParameterLocation::Query,
                    schema: Schema::String { format: None, enum_values: None },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
            params.push(
                Parameter {
                    name: "count".to_string(),
                    r#in: ParameterLocation::Query,
                    schema: Schema::Integer { format: Some("uint64".to_string()) },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
            params.push(
                Parameter {
                    name: "precise".to_string(),
                    r#in: ParameterLocation::Query,
                    schema: Schema::Boolean,
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
        }
        if route.path.contains("/invoke-and-await") || route.path.contains("/invoke") {
            params.push(
                Parameter {
                    name: "function".to_string(),
                    r#in: ParameterLocation::Query,
                    schema: Schema::String { format: None, enum_values: None },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(true),
                    description: None,
                }
            );
        }
        if route.path.contains("/interrupt") {
             params.push(
                Parameter {
                    name: "recovery-immediately".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::Boolean,
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
        }
        if route.path.contains("/oplog") {
            params.push(
                Parameter {
                    name: "from".to_string(),
                    r#in: ParameterLocation::Query,
                    schema: Schema::Integer { format: Some("uint64".to_string()) },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
             params.push(
                Parameter {
                    name: "count".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::Integer { format: Some("uint64".to_string()) },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(true),
                    description: None,
                }
            );
             params.push(
                Parameter {
                    name: "cursor".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::Ref {
                        reference: "#/components/schemas/OplogCursor".to_string()
                     },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
              params.push(
                Parameter {
                    name: "query".to_string(),
                    r#in: ParameterLocation::Query,
                    schema: Schema::String { format: None, enum_values: None },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
        }
         if route.path.contains("/download") {
              params.push(
                Parameter {
                    name: "version".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::Integer { format: Some("uint64".to_string()) },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
         }
        if route.path.contains("/components") && route.method == HttpMethod::Get {
              params.push(
                Parameter {
                    name: "component-name".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::String { format: None, enum_values: None },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
        }
        if route.path.contains("/api/definitions") && route.method == HttpMethod::Get {
             params.push(
                Parameter {
                    name: "api-definition-id".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::String { format: None, enum_values: None },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
        }
         if route.path.contains("/api/deployments") && route.method == HttpMethod::Get {
             params.push(
                Parameter {
                    name: "api-definition-id".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::String { format: None, enum_values: None },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(true),
                    description: None,
                }
            );
        }

        if route.path.contains("/upload") {
            params.push(
                Parameter {
                    name: "component_type".to_string(),
                    r#in: ParameterLocation::Query,
                    schema: Schema::Ref {
                        reference: "#/components/schemas/ComponentType".to_string()
                     },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: Some(
                        "Type of the new version of the component - if not specified, the type of the previous version is used.".to_string()
                    ),
                }
            );
        }
        if route.path.contains("/plugins") && route.method == HttpMethod::Get {
              params.push(
                Parameter {
                    name: "scope".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::Ref {
                        reference: "#/components/schemas/DefaultPluginScope".to_string()
                     },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
         }
         if route.path.contains("/activate-plugin") || route.path.contains("/deactivate-plugin"){
             params.push(
                Parameter {
                    name: "plugin-installation-id".to_string(),
                    r#in: ParameterLocation::Query,
                     schema: Schema::String { format: Some("uuid".to_string()), enum_values: None },
                    style: Some("form".to_string()),
                    explode: Some(true),
                    required: Some(true),
                    description: None,
                }
            );
         }
         params
    }


   fn extract_header_parameters(route: &Route) -> Vec<Parameter> {
        let mut params = Vec::new();
        if route.path.contains("/invoke-and-await") || route.path.contains("/invoke") {
            params.push(
                Parameter {
                    name: "Idempotency-Key".to_string(),
                    r#in: ParameterLocation::Header,
                    schema: Schema::String { format: None, enum_values: None },
                    style: Some("simple".to_string()),
                    explode: Some(true),
                    required: Some(false),
                    description: None,
                }
            );
        }
        params
    }

    fn create_request_body(route: &Route) -> Option<RequestBody> {
         match &route.binding {
            BindingType::Default { input_type, .. } => {
                 let schema = Self::wit_type_to_schema(input_type);

                // Check if route allows x-yaml
                let allows_yaml = route.path.starts_with("/v1/api/definitions")
                    && (route.method == HttpMethod::Put || route.method == HttpMethod::Post);

                 let mut content = HashMap::new();

                // JSON content type
                 if route.path.contains("/components") && route.method == HttpMethod::Post {

                    let mut properties = HashMap::new();

                    properties.insert("name".to_string(), Schema::String { format: None, enum_values: None });

                    properties.insert("componentType".to_string(), Schema::Ref {
                        reference: "#/components/schemas/ComponentType".to_string()
                    });


                    properties.insert("component".to_string(), Schema::String {
                        format: Some("binary".to_string()),
                        enum_values: None
                    });
                    properties.insert("filesPermissions".to_string(), Schema::Ref {
                        reference: "#/components/schemas/ComponentFilePathWithPermissionsList".to_string()
                    });
                      properties.insert("files".to_string(), Schema::String {
                        format: Some("binary".to_string()),
                        enum_values: None
                    });


                    content.insert(
                        "multipart/form-data".to_string(),
                        MediaType {
                            schema: Schema::Object {
                                properties,
                                required: Some(vec!["name".to_string(), "component".to_string()]),
                                additional_properties: None,
                            },
                            example: None,
                         },
                    );
                     Some(RequestBody {
                        description: None,
                        content,
                        required: Some(true),
                    })
                 } else if route.path.contains("/updates") && route.method == HttpMethod::Post {
                       let mut properties = HashMap::new();
                      properties.insert("componentType".to_string(), Schema::Ref {
                        reference: "#/components/schemas/ComponentType".to_string()
                    });
                      properties.insert("component".to_string(), Schema::String {
                        format: Some("binary".to_string()),
                        enum_values: None
                    });
                        properties.insert("filesPermissions".to_string(), Schema::Ref {
                        reference: "#/components/schemas/ComponentFilePathWithPermissionsList".to_string()
                    });
                      properties.insert("files".to_string(), Schema::String {
                        format: Some("binary".to_string()),
                        enum_values: None
                    });

                     content.insert(
                        "multipart/form-data".to_string(),
                        MediaType {
                           schema: Schema::Object {
                                properties,
                                required: Some(vec!["component".to_string()]),
                                additional_properties: None,
                            },
                            example: None,
                        },
                    );
                    Some(RequestBody {
                        description: None,
                        content,
                        required: Some(true),
                    })
                 } else if route.path.contains("/upload") && route.method == HttpMethod::Put {
                      content.insert(
                            "application/octet-stream".to_string(),
                            MediaType {
                                 schema: Schema::String {
                                        format: Some("binary".to_string()),
                                        enum_values: None
                                 },
                                example: None,
                            },
                        );
                         Some(RequestBody {
                            description: None,
                            content,
                            required: Some(true),
                        })
                 } else {
                    content.insert(
                        "application/json; charset=utf-8".to_string(),
                        MediaType {
                            schema: schema.clone(),
                            example: None,
                        },
                    );
                    if allows_yaml {
                        content.insert(
                            "application/x-yaml".to_string(),
                            MediaType {
                                schema,
                                example: None,
                            },
                        );
                    }
                      Some(RequestBody {
                        description: None,
                        content,
                        required: Some(true),
                    })
                }

            },
             _ => None,
        }
    }

    fn create_cors_headers(cors_allowed_origins: &str) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("Access-Control-Allow-Origin".to_string(), cors_allowed_origins.to_string());
        headers.insert("Access-Control-Allow-Methods".to_string(), "GET, POST, PUT, DELETE, OPTIONS".to_string());
        headers.insert("Access-Control-Allow-Headers".to_string(), "Content-Type, Authorization, Idempotency-Key".to_string());
        headers.insert("Access-Control-Max-Age".to_string(), "3600".to_string());
        headers
    }

   fn create_responses(route: &Route, cors_allowed_origins: &str) -> HashMap<String, Response> {
        let mut responses = HashMap::new();

        // Success response
        let response_schema = Self::get_response_schema(route);
         let content = if route.path.ends_with("/file-contents/{file_name}") && route.method == HttpMethod::Get {
                Some(HashMap::from([(
                     "application/octet-stream".to_string(),
                        MediaType {
                            schema: response_schema,
                            example: None,
                        }
                )]))
        } else {
             Some(HashMap::from([(
                "application/json; charset=utf-8".to_string(),
                MediaType {
                    schema: response_schema,
                    example: None,
                }
            )]))
        };

        responses.insert(
            "200".to_string(),
            Response {
                description: String::new(),
                content,
                headers: Some(Self::create_cors_headers(cors_allowed_origins)),
            }
        );


        // Standard error responses
        Self::add_error_responses(&mut responses, cors_allowed_origins);

        responses
    }

    fn add_error_responses(responses: &mut HashMap<String, Response>, cors_allowed_origins: &str) {
        let error_codes = ["400", "401", "403", "404", "409", "500"];
          let error_schemas = [
            "#/components/schemas/ErrorsBody",
            "#/components/schemas/ErrorBody",
            "#/components/schemas/ErrorBody",
            "#/components/schemas/ErrorBody",
            "#/components/schemas/ErrorBody",
            "#/components/schemas/GolemErrorBody"
        ];
        for (code, schema) in error_codes.iter().zip(error_schemas.iter()) {
            responses.insert(
                code.to_string(),
                Response {
                    description: String::new(),
                   content: Some(HashMap::from([(
                        "application/json; charset=utf-8".to_string(),
                        MediaType {
                            schema: Schema::Ref {
                                reference: schema.to_string()
                            },
                            example: None,
                        }
                    )])),
                    headers: Some(Self::create_cors_headers(cors_allowed_origins)),
                }
            );
        }
    }


    fn create_components(routes: &[Route]) -> Components {
        let mut components = Components {
            schemas: Some(HashMap::new()),
            responses: Some(HashMap::new()),
            parameters: Some(Self::create_common_parameters()),
            security_schemes: Some(HashMap::new()),
        };

        if let Some(schemas) = &mut components.schemas {
            // Add standard error schemas
            schemas.insert(
                "ErrorsBody".to_string(),
                Schema::Object {
                    properties: HashMap::from([
                        ("errors".to_string(), Schema::Array {
                            items: Box::new(Schema::String {
                                format: None,
                                enum_values: None
                            })
                        })
                    ]),
                    required: Some(vec!["errors".to_string()]),
                    additional_properties: None,
                }
            );

            schemas.insert(
                "ErrorBody".to_string(),
                Schema::Object {
                    properties: HashMap::from([
                        ("error".to_string(), Schema::String {
                            format: None,
                            enum_values: None
                        })
                    ]),
                    required: Some(vec!["error".to_string()]),
                    additional_properties: None,
                }
            );

            schemas.insert(
                "GolemErrorBody".to_string(),
                Schema::Object {
                    properties: HashMap::from([
                        ("golemError".to_string(), Schema::Ref {
                            reference: "#/components/schemas/GolemError".to_string()
                        })
                    ]),
                    required: Some(vec!["golemError".to_string()]),
                    additional_properties: None,
                }
            );

           // Add WorkersMetadataResponse
            schemas.insert(
                "WorkersMetadataResponse".to_string(),
                Schema::Object {
                    properties: HashMap::from([
                        ("workers".to_string(), Schema::Array {
                            items: Box::new(Schema::Ref {
                                reference: "#/components/schemas/WorkerMetadata".to_string()
                            })
                        }),
                        ("cursor".to_string(), Schema::String {  // Match yaml exactly
                            format: None,
                            enum_values: None
                        })
                    ]),
                    required: Some(vec!["workers".to_string()]),
                    additional_properties: None
                }
            );
             schemas.insert(
                "HttpApiDefinitionRequest".to_string(),
                 Schema::Object {
                      properties: HashMap::from([
                        ("id".to_string(), Schema::String { format: None, enum_values: None }),
                        ("version".to_string(), Schema::String { format: None, enum_values: None }),
                        ("security".to_string(), Schema::Array { items: Box::new(Schema::String { format: None, enum_values: None }) }),
                         ("routes".to_string(), Schema::Array {
                            items: Box::new(Schema::Ref {
                                reference: "#/components/schemas/RouteRequestData".to_string()
                            })
                         }),
                          ("draft".to_string(), Schema::Boolean)
                     ]),
                     required: Some(vec![
                        "id".to_string(),
                        "version".to_string(),
                        "routes".to_string(),
                        "draft".to_string()
                    ]),
                     additional_properties: None
                }
            );
             schemas.insert(
                "HttpApiDefinitionResponseData".to_string(),
                 Schema::Object {
                      properties: HashMap::from([
                        ("id".to_string(), Schema::String { format: None, enum_values: None }),
                        ("version".to_string(), Schema::String { format: None, enum_values: None }),
                         ("routes".to_string(), Schema::Array {
                            items: Box::new(Schema::Ref {
                                reference: "#/components/schemas/RouteResponseData".to_string()
                            })
                         }),
                          ("draft".to_string(), Schema::Boolean),
                        ("createdAt".to_string(), Schema::String { format: Some("date-time".to_string()), enum_values: None }),
                     ]),
                     required: Some(vec![
                        "id".to_string(),
                        "version".to_string(),
                        "routes".to_string(),
                         "draft".to_string(),
                    ]),
                     additional_properties: None
                }
            );
            // Add other schemas if necessary
            Self::collect_common_schemas(routes, schemas);
        }

        if let Some(security_schemes) = &mut components.security_schemes {
            security_schemes.insert(
                "bearerAuth".to_string(),
                SecurityScheme::Http {
                    scheme: "bearer".to_string(),
                    bearer_format: Some("JWT".to_string()),
                    description: Some("JWT Authorization header".to_string()),
                },
            );
        }


        components
    }

    fn create_common_parameters() -> HashMap<String, Parameter> {
        let mut params = HashMap::new();
         // Add the 'filter' parameter as requested
        params.insert(
            "filter".to_string(),
            Parameter {
                name: "filter".to_string(),
                r#in: ParameterLocation::Query,
                 schema: Schema::Array {
                    items: Box::new(Schema::String {
                        format: None,
                        enum_values: None
                    })
                },
                style: Some("form".to_string()),
                explode: Some(true),
                required: Some(false),
                 description: Some("Filter criteria".to_string()),
            }
        );
       // Add other common parameters similarly if needed
        // e.g. cursor, count, precise, etc. matching the YAML.

        params
    }


    fn wit_type_to_schema(wit_type: &str) -> Schema {
        match wit_type {
            "string" => Schema::String { format: None, enum_values: None },
            "i32" | "i64" => Schema::Integer { format: None },
            "f32" | "f64" => Schema::Number { format: None },
            "bool" => Schema::Boolean,
            t if t.starts_with("list<") => {
                 let inner_type = &t[5..t.len()-1];
                  Schema::Array {
                    items: Box::new(Self::wit_type_to_schema(inner_type)),
                }
            },
             t if t.starts_with("record{") => {
                Schema::Object {
                    properties: Self::parse_record_fields(t),
                    required: None,
                    additional_properties: None,
                }
            },
            _ => Schema::Ref {
                reference: format!("#/components/schemas/{}", wit_type),
            },
        }
    }


    fn parse_record_fields(record_type: &str) -> HashMap<String, Schema> {
          let mut properties = HashMap::new();
         if let Some(fields_str) = record_type
            .strip_prefix("record{")
            .and_then(|s| s.strip_suffix("}"))
        {
            for field in fields_str.split(',').map(str::trim) {
                if let Some((name, type_str)) = field.split_once(':') {
                    let name = name.trim().to_string();
                    let type_str = type_str.trim();
                   properties.insert(name, Self::wit_type_to_schema(type_str));
                }
            }
        }
        properties
    }

    fn collect_common_schemas(routes: &[Route], schemas: &mut HashMap<String, Schema>) {
        let mut type_set = std::collections::HashSet::new();

         for route in routes {
            match &route.binding {
                BindingType::Default { input_type, output_type, .. } => {
                    Self::extract_custom_types(input_type, &mut type_set);
                     Self::extract_custom_types(output_type, &mut type_set);
                }
                _ => {}
            }
        }
        for type_name in type_set {
            if !type_name.starts_with("record{") && !type_name.starts_with("list<")
                && type_name != "binary" && type_name != "string" && type_name != "i32" && type_name != "i64"
                && type_name != "f32" && type_name != "f64" && type_name != "bool" {
                 schemas.insert(
                    type_name.clone(),
                    Schema::Object {
                         properties: Self::parse_record_fields(&format!("record{{{}}}", type_name)),
                        required: None,
                        additional_properties: None,
                    }
                );
            }
        }
    }

    fn extract_custom_types(wit_type: &str, type_set: &mut std::collections::HashSet<String>) {
        match wit_type {
            "string" | "i32" | "i64" | "f32" | "f64" | "bool" | "binary" => {},
            t if t.starts_with("list<") => {
                let inner_type = &t[5..t.len()-1];
                Self::extract_custom_types(inner_type, type_set);
            },
            t if t.starts_with("record{") => {
                if let Some(fields_str) = t.strip_prefix("record{").and_then(|s| s.strip_suffix("}")) {
                    for field in fields_str.split(',').map(str::trim) {
                        if let Some((_, type_str)) = field.split_once(':') {
                            Self::extract_custom_types(type_str.trim(), type_set);
                        }
                    }
                }
            },
            t => {
                type_set.insert(t.to_string());
            }
        }
    }

    fn get_response_schema(route: &Route) -> Schema {
        match &route.binding {
            BindingType::Default { output_type, .. } => {
                if output_type == "binary" {
                    Schema::String {
                        format: Some("binary".to_string()),
                        enum_values: None,
                    }
                } else  {
                    Schema::Ref {
                        reference: format!("#/components/schemas/{}",
                            Self::get_response_type_name(route))
                    }
                }
            },
            BindingType::FileServer { .. } => Schema::String {
                format: Some("binary".to_string()),
                enum_values: None,
            },
            BindingType::SwaggerUI { .. } => Schema::String {
                format: Some("html".to_string()),
                enum_values: None,
            },
            BindingType::Http => Schema::Ref {
                reference: format!("#/components/schemas/{}",
                    Self::get_response_type_name(route))
            },
            BindingType::Worker => Schema::Ref {
                reference: format!("#/components/schemas/{}",
                    Self::get_response_type_name(route))
            },
            BindingType::Proxy => Schema::Ref {
                reference: format!("#/components/schemas/{}",
                    Self::get_response_type_name(route))
            },
        }
    }


    fn get_response_type_name(route: &Route) -> String {
        if route.path.ends_with("/workers") && route.method == HttpMethod::Get {
            "WorkersMetadataResponse".to_string()
        } else if route.path.ends_with("/complete") && route.method == HttpMethod::Post {
            "boolean".to_string()
        } else {
            match &route.binding {
                BindingType::Default { output_type, .. } => output_type.clone(),
                BindingType::FileServer { .. } => "binary".to_string(),
                BindingType::SwaggerUI { .. } => "html".to_string(),
                BindingType::Http => "HttpResponse".to_string(),
                BindingType::Worker => "WorkerResponse".to_string(),
                BindingType::Proxy => "ProxyResponse".to_string(),
            }
        }
    }
}
