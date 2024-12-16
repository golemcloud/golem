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

use golem_api_grpc::proto::golem::apidefinition::{HttpApiDefinition, HttpRoute};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use poem_openapi::Object;

/// Represents an OpenAPI Specification document.
#[derive(Serialize, Deserialize, Object)]
pub struct OpenApiSpec {
    pub openapi: String,
    pub info: ApiInfo,
    pub paths: HashMap<String, HashMap<String, PathItem>>,
    pub components: Option<Components>,
    pub security: Option<Vec<SecurityRequirement>>,
    pub cors: Option<HashMap<String, String>>, // Added for CORs configuration
}

/// Contains metadata about the API.
#[derive(Serialize, Deserialize, Object)]
pub struct ApiInfo {
    pub title: String,
    pub version: String,
}

/// Describes a path item in OpenAPI.
#[derive(Serialize, Deserialize, Object)]
pub struct PathItem {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub parameters: Option<Vec<Parameter>>,
    pub request_body: Option<RequestBody>,
    pub responses: HashMap<String, Response>,
}

/// Describes a parameter for a path item.
#[derive(Serialize, Deserialize, Object)]
pub struct Parameter {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub schema: serde_json::Value,
}

/// Represents a request body in OpenAPI.
#[derive(Serialize, Deserialize, Object)]
pub struct RequestBody {
    pub description: Option<String>,
    pub content: HashMap<String, MediaType>,
}

/// Describes a response for a path item.
#[derive(Serialize, Deserialize, Object)]
pub struct Response {
    pub description: String,
    pub content: Option<HashMap<String, MediaType>>,
}

/// Defines the media type schema.
#[derive(Serialize, Deserialize, Object)]
pub struct MediaType {
    pub schema: serde_json::Value,
}

/// Holds all components (schemas, security, etc.).
#[derive(Serialize, Deserialize, Object)]
pub struct Components {
    pub schemas: HashMap<String, serde_json::Value>,
}

/// Represents a security requirement in OpenAPI.
#[derive(Serialize, Deserialize, Object)]
pub struct SecurityRequirement {
    pub name: String,
    pub scopes: Option<Vec<String>>,
}

/// Represents an API route.
#[derive(Clone)]
pub struct Route {
    pub path: String,
    pub method: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub parameters: Vec<ParameterDefinition>,
    pub request_body: Option<RequestBodyDefinition>,
    pub responses: Vec<ResponseDefinition>,
}

/// Represents a parameter definition for a route.
#[derive(Clone)]
pub struct ParameterDefinition {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    pub schema: Value,
}

/// Represents a request body definition for a route.
#[derive(Clone)]
pub struct RequestBodyDefinition {
    pub description: Option<String>,
    pub content: HashMap<String, Value>,
}

/// Represents a response definition for a route.
#[derive(Clone)]
pub struct ResponseDefinition {
    pub status: String,
    pub description: String,
    pub content: Option<HashMap<String, Value>>,
}

///

/// Generate an OpenAPI specification from a HttpApiDefinition.
///
/// # Arguments
/// * `http_api_definition` - The HttpApiDefinition containing API metadata.
/// * `version` - The API version extracted or provided externally.
///
/// # Returns
/// * `OpenApiSpec` - The OpenAPI Specification derived from the HttpApiDefinition.
pub fn generate_openapi(http_api_definition: &HttpApiDefinition, version: &str) -> OpenApiSpec {
    OpenApiSpec {
        openapi: "3.0.0".to_string(),
        info: ApiInfo {
            title: "Generated API".to_string(),
            version: version.to_string(),
        },
        paths: map_paths(&http_api_definition.routes),
        components: None, // Add logic to map components if required
        security: None,   // Add logic to map security schemes if required
        cors: None,
    }
}

/// Maps API routes to OpenAPI paths.
fn map_paths(routes: &Vec<HttpRoute>) -> HashMap<String, HashMap<String, PathItem>> {
    let mut paths = HashMap::new();

    for route in routes {
        let converted_route = convert_http_route_to_route(route);
        let methods = paths
            .entry(converted_route.path.clone())
            .or_insert_with(HashMap::new);
        methods.insert(
            converted_route.method.clone(),
            PathItem {
                summary: converted_route.summary.clone(),
                description: converted_route.description.clone(),
                parameters: Some(map_parameters(&converted_route.parameters)),
                request_body: converted_route.request_body.as_ref().map(map_request_body),
                responses: map_responses(&converted_route.responses),
            },
        );
    }

    paths
}

/// Converts a HttpRoute to a Route with enriched details.
fn convert_http_route_to_route(http_route: &HttpRoute) -> Route {
    Route {
        path: http_route.path.clone(),
        method: match http_route.method {
            0 => "GET".to_string(),
            1 => "CONNECT".to_string(),
            2 => "POST".to_string(),
            3 => "DELETE".to_string(),
            4 => "PUT".to_string(),
            5 => "PATCH".to_string(),
            6 => "OPTIONS".to_string(),
            7 => "TRACE".to_string(),
            8 => "HEAD".to_string(),
            _ => "UNKNOWN".to_string(),
        },
        summary: None,                         // Populate if available
        description: None,
        parameters: vec![], // Add conversion logic for parameters
        request_body: None,
        responses: vec![], // Add conversion logic for responses
    }
}

/// Maps route parameters to OpenAPI parameters.
fn map_parameters(parameters: &Vec<ParameterDefinition>) -> Vec<Parameter> {
    parameters
        .iter()
        .map(|param| Parameter {
            name: param.name.clone(),
            description: param.description.clone(),
            required: param.required,
            schema: param.schema.clone(),
        })
        .collect()
}

/// Maps a request body definition to OpenAPI request body.
fn map_request_body(request_body: &RequestBodyDefinition) -> RequestBody {
    RequestBody {
        description: request_body.description.clone(),
        content: request_body
            .content
            .iter()
            .map(|(content_type, schema)| {
                (
                    content_type.clone(),
                    MediaType {
                        schema: schema.clone(),
                    },
                )
            })
            .collect(),
    }
}

/// Maps response definitions to OpenAPI responses.
fn map_responses(responses: &Vec<ResponseDefinition>) -> HashMap<String, Response> {
    responses
        .iter()
        .map(|response| {
            (
                response.status.clone(),
                Response {
                    description: response.description.clone(),
                    content: response.content.as_ref().map(|content| {
                        content
                            .iter()
                            .map(|(content_type, schema)| {
                                (
                                    content_type.clone(),
                                    MediaType {
                                        schema: schema.clone(),
                                    },
                                )
                            })
                            .collect()
                    }),
                },
            )
        })
        .collect()
}


#[cfg(test)]
mod tests {

    #[test]
    fn test_generate_openapi_with_empty_definition() {
        let http_api_definition = super::HttpApiDefinition {
            routes: vec![], // Empty routes for testing
        };

        // Pass version explicitly
        let openapi = super::generate_openapi(&http_api_definition, "1.0.0");

        // Validate the OpenAPI fields
        assert_eq!(openapi.openapi, "3.0.0");
        assert_eq!(openapi.info.title, "Generated API");
        assert_eq!(openapi.info.version, "1.0.0"); // Ensure version matches
        assert!(openapi.paths.is_empty());
        assert!(openapi.components.is_none());
        assert!(openapi.security.is_none());
    }

    #[test]
    fn test_map_paths_empty() {
        let routes = vec![];
        let paths = super::map_paths(&routes);
        assert!(paths.is_empty());
    }

    #[test]
    fn test_map_parameters_empty() {
        let parameters = vec![];
        let mapped_parameters = super::map_parameters(&parameters);
        assert!(mapped_parameters.is_empty());
    }

    #[test]
    fn test_map_responses_empty() {
        let responses = vec![];
        let mapped_responses = super::map_responses(&responses);
        assert!(mapped_responses.is_empty());
    }

    #[test]
    fn test_map_request_body_empty() {
        let request_body = super::RequestBodyDefinition {
            description: None,
            content: super::HashMap::new(),
        };
        let mapped_request_body = super::map_request_body(&request_body);
        assert!(mapped_request_body.description.is_none());
        assert!(mapped_request_body.content.is_empty());
    }

    #[test]
    fn test_map_paths_with_single_route() {
        let http_route = super::HttpRoute {
            path: "/test".to_string(),
            method: golem_api_grpc::proto::golem::apidefinition::HttpMethod::Get as i32, // Use enum variant for method
            binding: None,                  // No binding for simplicity
            middleware: None,               // No middleware for this test
        };

        let routes = vec![http_route];
        let paths = super::map_paths(&routes);

        // Assert the structure of the paths map
        assert_eq!(paths.len(), 1);
        assert!(paths.contains_key("/test"));

        let methods = paths.get("/test").unwrap();
        assert_eq!(methods.len(), 1);
        assert!(methods.contains_key("GET")); // Check for the correct method
    }

    #[test]
    fn test_openapi_with_security_and_cors() {
        let mut cors_config = super::HashMap::new();
        cors_config.insert("Access-Control-Allow-Origin".to_string(), "*".to_string());
        cors_config.insert("Access-Control-Allow-Methods".to_string(), "GET, POST".to_string());

        let openapi = super::OpenApiSpec {
            openapi: "3.0.0".to_string(),
            info: super::ApiInfo {
                title: "Test API".to_string(),
                version: "1.0.0".to_string(),
            },
            paths: super::HashMap::new(),
            components: None,
            security: Some(vec![super::SecurityRequirement {
                name: "api_key".to_string(),
                scopes: None,
            }]),
            cors: Some(cors_config),
        };

        assert_eq!(openapi.openapi, "3.0.0");
        assert!(openapi.security.is_some());
        assert!(openapi.cors.is_some());
    }

    #[test]
    fn test_map_parameters_with_multiple_parameters() {
        let params = vec![
            super::ParameterDefinition {
                name: "param1".to_string(),
                description: Some("A test parameter".to_string()),
                required: true,
                schema: serde_json::json!({"type": "string"}),
            },
            super::ParameterDefinition {
                name: "param2".to_string(),
                description: None,
                required: false,
                schema: serde_json::json!({"type": "integer"}),
            },
        ];

        let mapped_params = super::map_parameters(&params);

        // Validate mapped parameters
        assert_eq!(mapped_params.len(), 2);
        assert_eq!(mapped_params[0].name, "param1");
        assert!(mapped_params[0].description.is_some());
        assert!(mapped_params[1].description.is_none());
        assert_eq!(mapped_params[1].required, false);
    }

    #[test]
    fn test_map_request_body_with_content() {
        let request_body = super::RequestBodyDefinition {
            description: Some("Request body description".to_string()),
            content: super::HashMap::from([(
                "application/json".to_string(),
                serde_json::json!({"type": "object"}),
            )]),
        };

        let mapped_body = super::map_request_body(&request_body);

        // Validate the request body mapping
        assert_eq!(
            mapped_body.description,
            Some("Request body description".to_string())
        );
        assert!(mapped_body.content.contains_key("application/json"));
        assert_eq!(
            mapped_body.content.get("application/json").unwrap().schema,
            serde_json::json!({"type": "object"})
        );
    }

    #[test]
    fn test_map_responses_with_multiple_responses() {
        let responses = vec![
            super::ResponseDefinition {
                status: "200".to_string(),
                description: "OK".to_string(),
                content: Some(super::HashMap::from([(
                    "application/json".to_string(),
                    serde_json::json!({"type": "object"}),
                )])),
            },
            super::ResponseDefinition {
                status: "404".to_string(),
                description: "Not Found".to_string(),
                content: None,
            },
        ];

        let mapped_responses = super::map_responses(&responses);

        // Validate response mappings
        assert_eq!(mapped_responses.len(), 2);
        assert_eq!(mapped_responses.get("200").unwrap().description, "OK");
        assert!(mapped_responses.get("404").unwrap().content.is_none());
    }

}
