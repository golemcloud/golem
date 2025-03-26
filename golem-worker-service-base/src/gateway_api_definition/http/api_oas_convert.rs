// Copyright 2024-2025 Golem Cloud
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

use crate::gateway_api_definition::http::oas_api_definition::OpenApiHttpApiDefinition;
use golem_common::model::GatewayBindingType;
use serde::{Serialize, Deserialize};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::api::api_definition::HttpApiDefinitionResponseData;
use golem_wasm_ast::analysis::AnalysedType;

// Add constants from oas_api_definition.rs
const GOLEM_API_DEFINITION_ID_EXTENSION: &str = "x-golem-api-definition-id";
const GOLEM_API_DEFINITION_VERSION: &str = "x-golem-api-definition-version";
const GOLEM_API_GATEWAY_BINDING: &str = "x-golem-api-gateway-binding";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Object)]
pub struct OpenApiHttpApiDefinitionResponse {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub openapi_yaml: String,
}

impl OpenApiHttpApiDefinitionResponse {
    pub fn from_http_api_definition_response_data(
        response_data: &HttpApiDefinitionResponseData
    ) -> Result<Self, String> {
        let openapi = OpenApiHttpApiDefinition::from_http_api_definition_response_data(response_data)?;
        let openapi_yaml = serde_yaml::to_string(&openapi)
            .map_err(|e| format!("Failed to serialize OpenAPI to YAML: {}", e))?;
            
        Ok(Self {
            id: response_data.id.clone(),
            version: response_data.version.clone(),
            openapi_yaml,
        })
    }
}

// Function moved from oas_api_definition.rs
impl OpenApiHttpApiDefinition {
    pub fn from_http_api_definition_response_data(response_data: &HttpApiDefinitionResponseData) -> Result<Self, String> {
        let mut open_api = openapiv3::OpenAPI::default();
        open_api.openapi = "3.0.0".to_string();
        
        open_api.info = openapiv3::Info {
            title: response_data.id.0.clone(),
            description: None,
            terms_of_service: None,
            contact: None,
            license: None,
            version: response_data.version.0.clone(),
            extensions: Default::default(),
        };
        
        open_api.extensions.insert(
            GOLEM_API_DEFINITION_ID_EXTENSION.to_string(), 
            serde_json::Value::String(response_data.id.0.clone())
        );
        open_api.extensions.insert(
            GOLEM_API_DEFINITION_VERSION.to_string(), 
            serde_json::Value::String(response_data.version.0.clone())
        );
        
        let mut paths = std::collections::BTreeMap::new();
        
        // Create components section if it doesn't exist
        if open_api.components.is_none() {
            open_api.components = Some(openapiv3::Components::default());
        }
        
        // Collect all security schemes from routes
        let mut security_schemes = indexmap::IndexMap::new();
        
        for route in &response_data.routes {
            let path_str = route.path.to_string();
            let path_item = paths.entry(path_str.clone()).or_insert_with(|| openapiv3::PathItem::default());
            
            let mut operation = openapiv3::Operation::default();
            
            // Add path parameters
            let params = extract_path_parameters(&path_str);
            for param_name in params {
                // Check if we have parameter type information available from worker_name_input or response_mapping_input
                let param_type = if let (Some(worker_name_input), true) = (&route.binding.worker_name_input, route.binding.worker_name_input.is_some()) {
                    // Check if this path parameter is used in the worker name expression
                    if let Some(analysed_type) = worker_name_input.types.get(&format!("request.path.{}", param_name)) {
                        create_schema_from_analysed_type(analysed_type)
                    } else {
                        // Default to string if not found
                        openapiv3::Schema {
                            schema_data: Default::default(),
                            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(Default::default())),
                        }
                    }
                } else if let (Some(response_mapping_input), true) = (&route.binding.response_mapping_input, route.binding.response_mapping_input.is_some()) {
                    // Check if this path parameter is used in the response mapping expression
                    if let Some(analysed_type) = response_mapping_input.types.get(&format!("request.path.{}", param_name)) {
                        create_schema_from_analysed_type(analysed_type)
                    } else {
                        // Default to string if not found
                        openapiv3::Schema {
                            schema_data: Default::default(),
                            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(Default::default())),
                        }
                    }
                } else {
                    // Default to string if no type info available
                    openapiv3::Schema {
                        schema_data: Default::default(),
                        schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(Default::default())),
                    }
                };

                let parameter = openapiv3::Parameter::Path {
                    parameter_data: openapiv3::ParameterData {
                        name: param_name.clone(),
                        description: Some(format!("Path parameter: {}", param_name)),
                        required: true,
                        deprecated: None,
                        explode: Some(false),
                        format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(param_type)),
                        example: None,
                        examples: Default::default(),
                        extensions: Default::default(),
                    },
                    style: openapiv3::PathStyle::Simple,
                };
                operation.parameters.push(openapiv3::ReferenceOr::Item(parameter));
            }
            
            // Add request body based on responseMappingInput
            if let (Some(response_mapping_input), true) = (&route.binding.response_mapping_input, route.binding.response_mapping_input.is_some()) {
                // If types is not empty, we may need a request body
                if !response_mapping_input.types.is_empty() {
                    // Check if the response uses request.body
                    let uses_request_body = if let Some(response) = &route.binding.response {
                        response.contains("request.body")
                    } else {
                        false
                    };
                    
                    // If the response uses request.body or if request structure exists, create a request body
                    if uses_request_body || response_mapping_input.types.contains_key("request") {
                        // Extract the schema from the nested structure in "request" -> fields -> "body"
                        let body_schema = if let Some(request_type) = response_mapping_input.types.get("request") {
                            if let golem_wasm_ast::analysis::AnalysedType::Record(request_record) = request_type {
                                if let Some(body_field) = request_record.fields.iter().find(|f| f.name == "body") {
                                    // Now we have the body field type
                                    // Create a schema that accurately represents the request body structure
                                    create_schema_from_analysed_type(&body_field.typ)
                                } else {
                                    // Default to object if body field not found in request record
                                    openapiv3::Schema {
                                        schema_data: Default::default(),
                                        schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
                                    }
                                }
                            } else {
                                // Default to object if request is not a record
                                openapiv3::Schema {
                                    schema_data: Default::default(),
                                    schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
                                }
                            }
                        } else if let Some(body_type) = response_mapping_input.types.get("request.body") {
                            // Directly get request.body if available
                            create_schema_from_analysed_type(body_type)
                        } else {
                            // Default to object if neither structure found
                            openapiv3::Schema {
                                schema_data: Default::default(),
                                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
                            }
                        };
                        
                        let mut media_type = openapiv3::MediaType::default();
                        media_type.schema = Some(openapiv3::ReferenceOr::Item(body_schema));
                        
                        let mut content = indexmap::IndexMap::new();
                        content.insert("application/json".to_string(), media_type);
                        
                        // Determine if request body should be required
                        // POST/PUT/PATCH typically require a body, for others like GET/DELETE it's based on usage
                        let required = match route.method {
                            crate::gateway_api_definition::http::MethodPattern::Post |
                            crate::gateway_api_definition::http::MethodPattern::Put |
                            crate::gateway_api_definition::http::MethodPattern::Patch => true,
                            _ => uses_request_body // For GET/DELETE/etc, only require if explicitly used
                        };
                        
                        let request_body = openapiv3::RequestBody {
                            description: Some("Request payload".to_string()),
                            content,
                            required,
                            extensions: Default::default(),
                        };
                        
                        operation.request_body = Some(openapiv3::ReferenceOr::Item(request_body));
                        
                    }
                }
            }

            // Add default response based on method
            let default_status = match route.method {
                crate::gateway_api_definition::http::MethodPattern::Get => 200,
                crate::gateway_api_definition::http::MethodPattern::Post => 201,
                crate::gateway_api_definition::http::MethodPattern::Put => {
                    match route.binding.binding_type {
                        Some(GatewayBindingType::Default) | 
                        Some(GatewayBindingType::FileServer) => {
                            if let Some(response) = &route.binding.response {
                                if response.trim().is_empty() { 204 } else { 200 }
                            } else {
                                204
                            }
                        },
                        _ => 204
                    }
                },
                crate::gateway_api_definition::http::MethodPattern::Delete => 204,
                crate::gateway_api_definition::http::MethodPattern::Options => 200,
                crate::gateway_api_definition::http::MethodPattern::Head => 200,
                crate::gateway_api_definition::http::MethodPattern::Patch => {
                    match route.binding.binding_type {
                        Some(GatewayBindingType::Default) | 
                        Some(GatewayBindingType::FileServer) => {
                            if let Some(response) = &route.binding.response {
                                if response.trim().is_empty() { 204 } else { 200 }
                            } else {
                                200
                            }
                        },
                        _ => 200
                    }
                },
                crate::gateway_api_definition::http::MethodPattern::Trace => 200,
                _ => 200,
            };
            
            // Check if response expression might include multiple status codes
            let might_have_multiple_status_codes = if let (Some(GatewayBindingType::Default) | Some(GatewayBindingType::FileServer), Some(response)) = 
                (&route.binding.binding_type, &route.binding.response) {
                // Simple heuristic to detect if the response might have multiple status codes
                // TODO: This is a simple heuristic and might not be 100% accurate
                response.contains("status:") && 
                    (response.contains("match") || 
                     response.contains("if") || 
                     response.contains("?"))
            } else {
                false
            };
            
            // Define status codes to include in responses
            let status_codes = if might_have_multiple_status_codes {
                // Common status codes for success/error responses
                vec![200, 201, 204, 400, 404, 500]
            } else {
                vec![default_status]
            };
            
            // Create responses for each status code
            for status_code in &status_codes {
                let mut response = openapiv3::Response::default();
                response.description = match *status_code {
                    204 => "No Content".to_string(),
                    201 => "Created".to_string(),
                    200 => "OK".to_string(),
                    400 => "Bad Request".to_string(),
                    404 => "Not Found".to_string(),
                    500 => "Internal Server Error".to_string(),
                    _ => "Response".to_string()
                };
                
                if status_code != &204 {
                    let mut content = indexmap::IndexMap::new();
                    let mut media = openapiv3::MediaType::default();
                    
                    // Check if we have response body type information available from response_mapping_output
                    let response_schema = if let (Some(response_mapping_output), true) = (&route.binding.response_mapping_output, route.binding.response_mapping_output.is_some()) {
                        // Check if the response is a record with status and body fields
                        if let AnalysedType::Record(record) = &response_mapping_output.analysed_type {
                            let has_status = record.fields.iter().any(|f| f.name == "status");
                            let has_body = record.fields.iter().any(|f| f.name == "body");
                            
                            match (has_status, has_body) {
                                // If both status and body exist, use only the body's type
                                (true, true) => {
                                    if let Some(body_field) = record.fields.iter().find(|f| f.name == "body") {
                                        create_schema_from_analysed_type(&body_field.typ)
                                    } else {
                                        // Fallback to empty object if body field not found
                                        openapiv3::Schema {
                                            schema_data: Default::default(),
                                            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
                                        }
                                    }
                                },
                                // If only status exists, use empty object
                                (true, false) => {
                                    openapiv3::Schema {
                                        schema_data: Default::default(),
                                        schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
                                    }
                                },
                                // If neither exists, use the whole response type
                                _ => create_schema_from_analysed_type(&response_mapping_output.analysed_type)
                            }
                        } else {
                            // If not a record, use the whole response type
                            create_schema_from_analysed_type(&response_mapping_output.analysed_type)
                        }
                    } else {
                        // Default to object if no type info available
                        openapiv3::Schema {
                            schema_data: Default::default(),
                            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
                        }
                    };
                    
                    media.schema = Some(openapiv3::ReferenceOr::Item(response_schema));
                    content.insert("application/json".to_string(), media);
                    response.content = content;
                }
                
                operation.responses.responses.insert(
                    openapiv3::StatusCode::Code(status_code.clone()),
                    openapiv3::ReferenceOr::Item(response),
                );
            }

            // Create binding info based on route.binding
            let mut binding_info = serde_json::Map::new();
            
            match route.binding.binding_type {
                Some(GatewayBindingType::Default) => {
                    binding_info.insert("binding-type".to_string(), serde_json::Value::String("default".to_string()));
                    
                    if let Some(worker_name) = &route.binding.worker_name {
                        binding_info.insert("worker-name".to_string(), serde_json::Value::String(worker_name.clone()));
                    }
                    
                    if let Some(component_id) = &route.binding.component {
                        binding_info.insert(
                            "component-id".to_string(), 
                            serde_json::Value::String(component_id.name.clone())
                        );
                        binding_info.insert(
                            "component-version".to_string(), 
                            serde_json::Value::Number(serde_json::Number::from(component_id.version))
                        );
                    }
                    
                    if let Some(key) = &route.binding.idempotency_key {
                        binding_info.insert("idempotency-key".to_string(), serde_json::Value::String(key.clone()));
                    }
                    
                    if let Some(response) = &route.binding.response {
                        binding_info.insert("response".to_string(), serde_json::Value::String(response.clone()));
                    }
                },
                Some(GatewayBindingType::HttpHandler) => {
                    binding_info.insert("binding-type".to_string(), serde_json::Value::String("http-handler".to_string()));
                    
                    if let Some(worker_name) = &route.binding.worker_name {
                        binding_info.insert("worker-name".to_string(), serde_json::Value::String(worker_name.clone()));
                    }
                    
                    if let Some(component_id) = &route.binding.component {
                        binding_info.insert(
                            "component-id".to_string(), 
                            serde_json::Value::String(component_id.name.clone())
                        );
                        binding_info.insert(
                            "component-version".to_string(), 
                            serde_json::Value::Number(serde_json::Number::from(component_id.version))
                        );
                    }
                    
                    if let Some(key) = &route.binding.idempotency_key {
                        binding_info.insert("idempotency-key".to_string(), serde_json::Value::String(key.clone()));
                    }
                },
                Some(GatewayBindingType::CorsPreflight) => {
                    binding_info.insert("binding-type".to_string(), serde_json::Value::String("cors-preflight".to_string()));
                    
                    if let Some(cors) = &route.binding.cors_preflight {
                        let mut response_lines = Vec::new();
                        
                        // Opening brace
                        response_lines.push("{".to_string());
                        
                        // Create a list of all headers to be included
                        let mut headers = Vec::new();
                        headers.push(format!("Access-Control-Allow-Headers: \"{}\"", cors.get_allow_headers()));
                        headers.push(format!("Access-Control-Allow-Methods: \"{}\"", cors.get_allow_methods()));
                        headers.push(format!("Access-Control-Allow-Origin: \"{}\"", cors.get_allow_origin()));
                        
                        if let Some(expose_headers) = cors.get_expose_headers() {
                            headers.push(format!("Access-Control-Expose-Headers: \"{}\"", expose_headers));
                        }
                        
                        if let Some(allow_credentials) = cors.get_allow_credentials() {
                            headers.push(format!("Access-Control-Allow-Credentials: {}", allow_credentials));
                        }
                        
                        if let Some(max_age) = cors.get_max_age() {
                            headers.push(format!("Access-Control-Max-Age: {}u64", max_age));
                        }
                        
                        // Add all headers with appropriate indentation and commas
                        for (i, header) in headers.iter().enumerate() {
                            let comma = if i < headers.len() - 1 { "," } else { "" };
                            response_lines.push(format!("  {}{}", header, comma));
                        }
                        
                        // Closing brace
                        response_lines.push("}".to_string());
                        
                        // Join with newlines 
                        let formatted_response = response_lines.join("\n");
                        binding_info.insert("response".to_string(), serde_json::Value::String(formatted_response));
                    } else {
                        // Empty JSON object as placeholder 
                        binding_info.insert("response".to_string(), serde_json::Value::String("{\n}".to_string()));
                    }
                },
                Some(GatewayBindingType::FileServer) => {
                    binding_info.insert("binding-type".to_string(), serde_json::Value::String("file-server".to_string()));
                    
                    if let Some(worker_name) = &route.binding.worker_name {
                        binding_info.insert("worker-name".to_string(), serde_json::Value::String(worker_name.clone()));
                    }
                    
                    if let Some(component_id) = &route.binding.component {
                        binding_info.insert(
                            "component-id".to_string(), 
                            serde_json::Value::String(component_id.name.clone())
                        );
                        binding_info.insert(
                            "component-version".to_string(), 
                            serde_json::Value::Number(serde_json::Number::from(component_id.version))
                        );
                    }
                    
                    if let Some(key) = &route.binding.idempotency_key {
                        binding_info.insert("idempotency-key".to_string(), serde_json::Value::String(key.clone()));
                    }
                    
                    if let Some(response) = &route.binding.response {
                        binding_info.insert("response".to_string(), serde_json::Value::String(response.clone()));
                    }
                },
                Some(GatewayBindingType::SwaggerUi) => {
                    binding_info.insert("binding-type".to_string(), serde_json::Value::String("swagger-ui".to_string()));
                },
                None => {
                    // Default to a simple binding type
                    binding_info.insert("binding-type".to_string(), serde_json::Value::String("default".to_string()));
                }
            }

            // Add binding info to operation
            operation.extensions.insert(GOLEM_API_GATEWAY_BINDING.to_string(), serde_json::Value::Object(binding_info));
            
            // Add security if present
            if let Some(security_ref) = &route.security {
                let mut req = indexmap::IndexMap::new();
                req.insert(security_ref.clone(), Vec::<String>::new());
                operation.security = Some(vec![req]);
            }
            
            // There is no CORS information in RouteResponseData, so we'll skip this
            // TODO: If CORS information becomes available in RouteResponseData, handle it here
            
            // Add operation to path item based on method
            match route.method {
                crate::gateway_api_definition::http::MethodPattern::Get => { path_item.get = Some(operation); },
                crate::gateway_api_definition::http::MethodPattern::Post => { path_item.post = Some(operation); },
                crate::gateway_api_definition::http::MethodPattern::Put => { path_item.put = Some(operation); },
                crate::gateway_api_definition::http::MethodPattern::Delete => { path_item.delete = Some(operation); },
                crate::gateway_api_definition::http::MethodPattern::Options => { path_item.options = Some(operation); },
                crate::gateway_api_definition::http::MethodPattern::Head => { path_item.head = Some(operation); },
                crate::gateway_api_definition::http::MethodPattern::Patch => { path_item.patch = Some(operation); },
                crate::gateway_api_definition::http::MethodPattern::Trace => { path_item.trace = Some(operation); },
                crate::gateway_api_definition::http::MethodPattern::Connect => {
                    return Err("CONNECT method is not supported in OpenAPI v3 specification".to_string());
                },
            }

            // Collect security schemes from route
            if let Some(security_ref) = &route.security {
                // Add security scheme if not already present
                if !security_schemes.contains_key::<str>(security_ref) {
                    let security_scheme = openapiv3::SecurityScheme::APIKey {
                        location: openapiv3::APIKeyLocation::Header,
                        name: "Authorization".to_string(),
                        description: Some(format!("API key security scheme for {}", security_ref)),
                        extensions: indexmap::IndexMap::new(), // Add empty extensions
                    };
                    // Wrap in ReferenceOr::Item
                    security_schemes.insert(security_ref.clone(), openapiv3::ReferenceOr::Item(security_scheme));
                }
            }
        }
        
        open_api.paths = openapiv3::Paths {
            paths: paths.into_iter()
                .map(|(k, v)| (k, openapiv3::ReferenceOr::Item(v)))
                .collect(),
            extensions: Default::default()
        };
        
        // Add security schemes to components
        if !security_schemes.is_empty() {
            open_api.components.as_mut().unwrap().security_schemes = security_schemes;
        }

        // Handle global security
        let mut global_security = Vec::new();
        
        // Collect all unique security requirements
        let mut seen_requirements = std::collections::HashSet::new();
        for route in &response_data.routes {
            if let Some(security_ref) = &route.security {
                if !seen_requirements.contains::<str>(security_ref) {
                    let mut req = indexmap::IndexMap::new();
                    req.insert(security_ref.clone(), Vec::<String>::new());
                    global_security.push(req);
                    seen_requirements.insert(security_ref.clone());
                }
            }
        }
        
        // Set global security if any requirements were found
        if !global_security.is_empty() {
            // Wrap the Vec in Some
            open_api.security = Some(global_security);
        }

        Ok(OpenApiHttpApiDefinition(open_api))
    }
}

// Helper functions moved from oas_api_definition.rs
fn extract_path_parameters(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    for segment in path.split('/') {
        if segment.starts_with('{') && segment.ends_with('}') {
            params.push(segment[1..segment.len()-1].to_string());
        }
    }
    params
}

// Helper function to convert AnalysedType to OpenAPI Schema
fn create_schema_from_analysed_type(analysed_type: &golem_wasm_ast::analysis::AnalysedType) -> openapiv3::Schema {
    use golem_wasm_ast::analysis::AnalysedType;

    match analysed_type {
        AnalysedType::Bool(_) => openapiv3::Schema {
            schema_data: Default::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Boolean(openapiv3::BooleanType::default())),
        },
        AnalysedType::U8(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                    openapiv3::IntegerType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32),
                        minimum: Some(0),
                        maximum: Some(255),
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::U16(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                    openapiv3::IntegerType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32),
                        minimum: Some(0),
                        maximum: Some(65535),
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::U32(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                    openapiv3::IntegerType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32),
                        minimum: Some(0),
                        maximum: None, // Too large to represent as i64
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::U64(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                    openapiv3::IntegerType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int64),
                        minimum: Some(0),
                        maximum: None,
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::S8(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                    openapiv3::IntegerType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32),
                        minimum: Some(-128),
                        maximum: Some(127),
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::S16(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                    openapiv3::IntegerType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32),
                        minimum: Some(-32768),
                        maximum: Some(32767),
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::S32(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                    openapiv3::IntegerType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32),
                        minimum: None,
                        maximum: None,
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::S64(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                    openapiv3::IntegerType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int64),
                        minimum: None,
                        maximum: None,
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::F32(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Number(
                    openapiv3::NumberType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::NumberFormat::Float),
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        minimum: None,
                        maximum: None,
                        enumeration: vec![],
                    },
                )),
            }
        },
        AnalysedType::F64(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Number(
                    openapiv3::NumberType {
                        format: openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::NumberFormat::Double),
                        multiple_of: None,
                        exclusive_minimum: false,
                        exclusive_maximum: false,
                        minimum: None,
                        maximum: None,
                        enumeration: vec![],
                    },
                )),
            }
        },
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
            let items = openapiv3::ReferenceOr::Item(Box::new(create_schema_from_analysed_type(&type_list.inner)));
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
        },
        AnalysedType::Tuple(type_tuple) => {
            // Convert tuple to an array with anyOf for each position
            let min_items = Some(type_tuple.items.len());
            let max_items = Some(type_tuple.items.len());
            
            // We can represent tuples as arrays in OpenAPI
            let items = openapiv3::ReferenceOr::Item(Box::new(openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
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
            
            // Add description indicating this is a tuple
            let mut schema_data = openapiv3::SchemaData::default();
            schema_data.description = Some("Tuple type".to_string());
            
            openapiv3::Schema {
                schema_data,
                schema_kind: array_schema.schema_kind,
            }
        },
        AnalysedType::Record(type_record) => {
            let mut properties = indexmap::IndexMap::new();
            let mut required = Vec::new();
            
            for field in &type_record.fields {
                let field_schema = create_schema_from_analysed_type(&field.typ);
                let is_nullable = field_schema.schema_data.nullable;
                properties.insert(field.name.clone(), openapiv3::ReferenceOr::Item(Box::new(field_schema)));
                // Only add to required list if the field is not nullable
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
        },
        AnalysedType::Variant(type_variant) => {
            // For variants, create a discriminated union using oneOf
            let mut one_of = Vec::new();
            
            for case in &type_variant.cases {
                let case_name = &case.name;
                
                if let Some(case_type) = &case.typ {
                    // If the variant case has an associated type, create a schema for it
                    let case_schema = create_schema_from_analysed_type(case_type);
                    
                    let mut properties = indexmap::IndexMap::new();
                    properties.insert(case_name.clone(), openapiv3::ReferenceOr::Item(Box::new(case_schema)));
                    
                    let mut required = Vec::new();
                    required.push(case_name.clone());
                    
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
                    // If the variant case has no associated type, create a simple string enum
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
        },
        AnalysedType::Enum(type_enum) => {
            // Convert Vec<String> to Vec<Option<String>> for enumeration
            let enum_values: Vec<Option<String>> = type_enum.cases.iter()
                .map(|case| Some(case.clone()))
                .collect();
                
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
        },
        AnalysedType::Option(type_option) => {
            // For Option, use nullable property
            let mut schema = create_schema_from_analysed_type(&type_option.inner);
            schema.schema_data.nullable = true;
            schema
        },
        AnalysedType::Result(type_result) => {
            // Handle Option<Box<AnalysedType>> correctly by unwrapping
            let ok_type = match &type_result.ok {
                Some(boxed_type) => &**boxed_type,
                None => &AnalysedType::Str(golem_wasm_ast::analysis::TypeStr {}),
            };
            
            let err_type = match &type_result.err {
                Some(boxed_type) => &**boxed_type,
                None => &AnalysedType::Str(golem_wasm_ast::analysis::TypeStr {}),
            };
            
            // For Result, use oneOf with success and error schemas
            let ok_schema = create_schema_from_analysed_type(ok_type);
            let err_schema = create_schema_from_analysed_type(err_type);
            
            // Create a schema for success case
            let mut ok_properties = indexmap::IndexMap::new();
            ok_properties.insert("ok".to_string(), openapiv3::ReferenceOr::Item(Box::new(ok_schema)));
            
            let mut ok_required = Vec::new();
            ok_required.push("ok".to_string());
            
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
            
            // Create a schema for error case
            let mut err_properties = indexmap::IndexMap::new();
            err_properties.insert("err".to_string(), openapiv3::ReferenceOr::Item(Box::new(err_schema)));
            
            let mut err_required = Vec::new();
            err_required.push("err".to_string());
            
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
                    ] 
                },
            }
        },
        // Handle any other cases with a generic object type
        _ => openapiv3::Schema {
            schema_data: Default::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
        },
    }
}

