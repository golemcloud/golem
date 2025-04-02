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

use crate::api::api_definition::HttpApiDefinitionResponseData;
use crate::api::RouteResponseData;
use crate::gateway_api_definition::http::oas_api_definition::OpenApiHttpApiDefinition;
use crate::gateway_api_definition::http::MethodPattern;
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_middleware::HttpCors;
use golem_common::model::GatewayBindingType;
use golem_wasm_ast::analysis::AnalysedType;
use rib::RibInputTypeInfo;
use serde::{Deserialize, Serialize};

// Constants for OpenAPI extensions
const GOLEM_API_DEFINITION_ID_EXTENSION: &str = "x-golem-api-definition-id";
const GOLEM_API_DEFINITION_VERSION: &str = "x-golem-api-definition-version";
const GOLEM_API_GATEWAY_BINDING: &str = "x-golem-api-gateway-binding";

// OpenApiHttpApiDefinitionResponse had id, version and open api schema as yaml string
// OpenApiHttpApiDefinitionResponse is a wrapper around OpenApiHttpApiDefinition
// OpenApiHttpApiDefinition struct is defined using crate openapiv3 as OPENAPI+GOLEMEXTENSIONS
// openapiv3 does not have Json(T) trait, so we convert to yaml string and wrap it
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Object)]
pub struct OpenApiHttpApiDefinitionResponse {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub openapi_yaml: String,
}

// OpenApiHttpApiDefinitionResponse implementation
impl OpenApiHttpApiDefinitionResponse {
    pub fn from_http_api_definition_response_data(
        response_data: &HttpApiDefinitionResponseData,
    ) -> Result<Self, String> {
        let openapi =
            OpenApiHttpApiDefinition::from_http_api_definition_response_data(response_data)?;
        let openapi_yaml = serde_yaml::to_string(&openapi)
            .map_err(|e| format!("Failed to serialize OpenAPI to YAML: {}", e))?;

        Ok(Self {
            id: response_data.id.clone(),
            version: response_data.version.clone(),
            openapi_yaml,
        })
    }
}

// Convert HttpApiDefinitionResponseData to OpenHttpApiDefinition
impl OpenApiHttpApiDefinition {
    pub fn from_http_api_definition_response_data(
        response_data: &HttpApiDefinitionResponseData,
    ) -> Result<Self, String> {
        let mut open_api = create_base_openapi(response_data);
        let mut paths = std::collections::BTreeMap::new();
        let mut security_schemes = indexmap::IndexMap::new();

        // Process each route and build paths
        for route in &response_data.routes {
            process_route(route, &mut paths, &mut security_schemes)?;
        }

        // Set paths and security in OpenAPI spec
        finalize_openapi(&mut open_api, paths, security_schemes, response_data);

        Ok(OpenApiHttpApiDefinition(open_api))
    }
}

// Helper function: Base OpenAPI structure
fn create_base_openapi(response_data: &HttpApiDefinitionResponseData) -> openapiv3::OpenAPI {
    let mut open_api = openapiv3::OpenAPI {
        openapi: "3.0.0".to_string(),
        info: openapiv3::Info {
            title: response_data.id.0.clone(),
            description: None,
            terms_of_service: None,
            contact: None,
            license: None,
            version: response_data.version.0.clone(),
            extensions: Default::default(),
        },
        ..Default::default()
    };

    // Add Golem extensions
    open_api.extensions.insert(
        GOLEM_API_DEFINITION_ID_EXTENSION.to_string(),
        serde_json::Value::String(response_data.id.0.clone()),
    );
    open_api.extensions.insert(
        GOLEM_API_DEFINITION_VERSION.to_string(),
        serde_json::Value::String(response_data.version.0.clone()),
    );

    // Initialize components
    open_api.components = Some(openapiv3::Components::default());

    open_api
}

// Helper function: Handles each route in the HttpApiDefinitionResponseData
fn process_route(
    route: &RouteResponseData,
    paths: &mut std::collections::BTreeMap<String, openapiv3::PathItem>,
    security_schemes: &mut indexmap::IndexMap<
        String,
        openapiv3::ReferenceOr<openapiv3::SecurityScheme>,
    >,
) -> Result<(), String> {
    let path_str = route.path.to_string();
    let path_item = paths.entry(path_str.clone()).or_default();

    let operation = create_operation(route, security_schemes)?;

    // Add operation to path item based on method
    add_operation_to_path_item(path_item, &route.method, operation)?;

    Ok(())
}

// Helper function: Creates an operation for a route
fn create_operation(
    route: &RouteResponseData,
    security_schemes: &mut indexmap::IndexMap<
        String,
        openapiv3::ReferenceOr<openapiv3::SecurityScheme>,
    >,
) -> Result<openapiv3::Operation, String> {
    let mut operation = openapiv3::Operation::default();

    // Add path parameters
    add_path_parameters(&mut operation, route);

    // Add request body
    add_request_body(&mut operation, route);

    // Add responses
    add_responses(&mut operation, route);

    // Add binding info
    add_binding_info(&mut operation, route);

    // Add security
    add_security(&mut operation, route, security_schemes);

    Ok(operation)
}

// Helper function: Adds path parameters to the operation
fn add_path_parameters(operation: &mut openapiv3::Operation, route: &RouteResponseData) {
    // Extract path parameters from the route path
    let params = extract_path_parameters(&route.path.to_string());
    for param_name in params {
        let param_type = determine_parameter_type(route, &param_name);
        let parameter = create_path_parameter(&param_name, param_type);
        operation
            .parameters
            .push(openapiv3::ReferenceOr::Item(parameter));
    }
}

// Helper function: Adds request body to the operation
fn add_request_body(operation: &mut openapiv3::Operation, route: &RouteResponseData) {
    // Only add request body if response_mapping_input is present
    if let Some(response_mapping_input) = &route.binding.response_mapping_input {
        if !response_mapping_input.types.is_empty() {
            let request_body = create_request_body(route, response_mapping_input);
            operation.request_body = Some(openapiv3::ReferenceOr::Item(request_body));
        }
    }
}

// Helper function: Adds responses to the operation
fn add_responses(operation: &mut openapiv3::Operation, route: &RouteResponseData) {
    let default_status = get_default_status_code(&route.method);
    let status_codes = determine_status_codes(route, default_status);

    for status_code in status_codes {
        let response = create_response(status_code, route);
        operation.responses.responses.insert(
            openapiv3::StatusCode::Code(status_code),
            openapiv3::ReferenceOr::Item(response),
        );
    }
}

// Helper function: Adds binding info to the operation
fn add_binding_info(operation: &mut openapiv3::Operation, route: &RouteResponseData) {
    let binding_info = create_binding_info(route);
    operation.extensions.insert(
        GOLEM_API_GATEWAY_BINDING.to_string(),
        serde_json::Value::Object(binding_info),
    );
}

// Helper function: Adds security to the operation
fn add_security(
    operation: &mut openapiv3::Operation,
    route: &RouteResponseData,
    security_schemes: &mut indexmap::IndexMap<
        String,
        openapiv3::ReferenceOr<openapiv3::SecurityScheme>,
    >,
) {
    if let Some(security_ref) = &route.security {
        // Add security scheme if not already present
        if !security_schemes.contains_key::<str>(security_ref) {
            let security_scheme = create_security_scheme(security_ref);
            security_schemes.insert(
                security_ref.to_string(),
                openapiv3::ReferenceOr::Item(security_scheme),
            );
        }

        // Add security requirement to operation
        let mut req = indexmap::IndexMap::new();
        req.insert(security_ref.to_string(), Vec::<String>::new());
        operation.security = Some(vec![req]);
    }
}

// Helper function: Finalizes OpenAPI specification
fn finalize_openapi(
    open_api: &mut openapiv3::OpenAPI,
    paths: std::collections::BTreeMap<String, openapiv3::PathItem>,
    security_schemes: indexmap::IndexMap<String, openapiv3::ReferenceOr<openapiv3::SecurityScheme>>,
    response_data: &HttpApiDefinitionResponseData,
) {
    // Set paths
    open_api.paths = openapiv3::Paths {
        paths: paths
            .into_iter()
            .map(|(k, v)| (k, openapiv3::ReferenceOr::Item(v)))
            .collect(),
        extensions: Default::default(),
    };

    // Add security schemes to components if any exist
    if !security_schemes.is_empty() {
        open_api.components.as_mut().unwrap().security_schemes = security_schemes;
    }

    // Set global security if needed
    set_global_security(open_api, response_data);
}

// Helper function: Extracts path parameters from the route path
// Todo: Query parameters should be handled here
fn extract_path_parameters(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    for segment in path.split('/') {
        if segment.starts_with('{') && segment.ends_with('}') {
            params.push(segment[1..segment.len() - 1].to_string());
        }
    }
    params
}

// Helper function to determine parameter type
fn determine_parameter_type(route: &RouteResponseData, param_name: &str) -> openapiv3::Schema {
    use golem_wasm_ast::analysis::AnalysedType;

    // Check worker_name_input first, then check request key and then look for path field within the request
    if let Some(worker_name_input) = &route.binding.worker_name_input {
        if let Some(AnalysedType::Record(request_record)) = worker_name_input.types.get("request") {
            if let Some(path_field) = request_record
                .fields
                .iter()
                .find(|field| field.name == "path")
            {
                if let AnalysedType::Record(path_record) = &path_field.typ {
                    if let Some(param_type) = path_record
                        .fields
                        .iter()
                        .find(|field| field.name == param_name)
                    {
                        return create_schema_from_analysed_type(&param_type.typ);
                    }
                }
            }
        }
    }

    // Check response_mapping_input, request key and then look for path field within the request
    if let Some(response_mapping_input) = &route.binding.response_mapping_input {
        if let Some(AnalysedType::Record(request_record)) =
            response_mapping_input.types.get("request")
        {
            if let Some(path_field) = request_record
                .fields
                .iter()
                .find(|field| field.name == "path")
            {
                if let AnalysedType::Record(path_record) = &path_field.typ {
                    if let Some(param_type) = path_record
                        .fields
                        .iter()
                        .find(|field| field.name == param_name)
                    {
                        return create_schema_from_analysed_type(&param_type.typ);
                    }
                }
            }
        }
    }

    // Default to string if no type information is available
    openapiv3::Schema {
        schema_data: openapiv3::SchemaData {
            nullable: false,
            ..Default::default()
        },
        schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
            openapiv3::StringType::default(),
        )),
    }
}

// Helper function: Creates a path parameter
fn create_path_parameter(param_name: &str, param_type: openapiv3::Schema) -> openapiv3::Parameter {
    openapiv3::Parameter::Path {
        parameter_data: openapiv3::ParameterData {
            name: param_name.to_string(),
            description: Some(format!("Path parameter: {}", param_name)),
            required: true,
            deprecated: None,
            explode: Some(false),
            format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(
                param_type,
            )),
            example: None,
            examples: Default::default(),
            extensions: Default::default(),
        },
        style: openapiv3::PathStyle::Simple,
    }
}

// Helper function: Creates a request body
fn create_request_body(
    _route: &RouteResponseData,
    response_mapping_input: &RibInputTypeInfo,
) -> openapiv3::RequestBody {
    let body_schema = determine_request_body_schema(response_mapping_input);
    let media_type = openapiv3::MediaType {
        schema: Some(openapiv3::ReferenceOr::Item(body_schema)),
        ..Default::default()
    };

    let mut content = indexmap::IndexMap::new();
    content.insert("application/json".to_string(), media_type);

    // If we have response_mapping_input, body is always required
    let required = true;

    openapiv3::RequestBody {
        description: Some("Request payload".to_string()),
        content,
        required,
        extensions: Default::default(),
    }
}

// Helper function: Determines request body schema
fn determine_request_body_schema(response_mapping_input: &RibInputTypeInfo) -> openapiv3::Schema {
    // Check request key and then look for body field within the request
    if let Some(AnalysedType::Record(request_record)) = response_mapping_input.types.get("request")
    {
        if let Some(body_field) = request_record.fields.iter().find(|f| f.name == "body") {
            return create_schema_from_analysed_type(&body_field.typ);
        }
    }

    // Fallback to default object schema if no body is found
    create_default_object_schema()
}

// Helper function: Gets default status code based on method
fn get_default_status_code(method: &MethodPattern) -> u16 {
    match method {
        MethodPattern::Get => 200,
        MethodPattern::Post => 201,
        MethodPattern::Put => 204,
        MethodPattern::Delete => 204,
        MethodPattern::Options => 200,
        MethodPattern::Head => 200,
        MethodPattern::Patch => 200,
        MethodPattern::Trace => 200,
        _ => 200,
    }
}

// Helper function: Determines status codes to include
fn determine_status_codes(route: &RouteResponseData, default_status: u16) -> Vec<u16> {
    use golem_wasm_ast::analysis::AnalysedType;

    // Check if response mapping output has a variant or result type
    let has_multiple_responses =
        if let Some(response_mapping_output) = &route.binding.response_mapping_output {
            matches!(
                &response_mapping_output.analysed_type,
                AnalysedType::Variant(_) | AnalysedType::Result(_)
            )
        } else {
            false
        };

    if has_multiple_responses {
        // Include common 2xx, 4xx and 5xx status codes
        vec![200, 201, 204, 400, 404, 500]
    } else {
        // Just use the default status code
        vec![default_status]
    }
}

// Helper function: Create response
fn create_response(status_code: u16, route: &RouteResponseData) -> openapiv3::Response {
    let mut response = openapiv3::Response {
        description: get_status_description(status_code),
        ..Default::default()
    };

    // Only add content for non-204 responses
    if status_code != 204 {
        let mut content = indexmap::IndexMap::new();
        let media = openapiv3::MediaType {
            schema: Some(openapiv3::ReferenceOr::Item(determine_response_schema(
                route,
            ))),
            ..Default::default()
        };
        content.insert("application/json".to_string(), media);
        response.content = content;
    }

    response
}

// Helper function: Gets status code description
fn get_status_description(status_code: u16) -> String {
    match status_code {
        204 => "No Content".to_string(),
        201 => "Created".to_string(),
        200 => "OK".to_string(),
        400 => "Bad Request".to_string(),
        404 => "Not Found".to_string(),
        500 => "Internal Server Error".to_string(),
        _ => "Response".to_string(),
    }
}

// Helper function: Determines response schema
fn determine_response_schema(route: &RouteResponseData) -> openapiv3::Schema {
    // We check if it has both status and body fields, then use the body field
    if let Some(response_mapping_output) = &route.binding.response_mapping_output {
        if let AnalysedType::Record(record) = &response_mapping_output.analysed_type {
            let has_status = record.fields.iter().any(|f| f.name == "status");
            let has_body = record.fields.iter().any(|f| f.name == "body");

            return match (has_status, has_body) {
                (true, true) => record
                    .fields
                    .iter()
                    .find(|f| f.name == "body")
                    .map(|body_field| create_schema_from_analysed_type(&body_field.typ))
                    .unwrap_or_else(create_default_object_schema),
                (true, false) => create_default_object_schema(),
                _ => create_schema_from_analysed_type(&response_mapping_output.analysed_type),
            };
        }
        create_schema_from_analysed_type(&response_mapping_output.analysed_type)
    } else {
        create_default_object_schema()
    }
}

// Helper function: Creates binding info
// fileserver, default, http-handler and swagger-ui binding info are handled by common binding info
// cors-preflight binding info is handled by cors-preflight binding info
fn create_binding_info(route: &RouteResponseData) -> serde_json::Map<String, serde_json::Value> {
    let mut binding_info = serde_json::Map::new();

    match route.binding.binding_type {
        Some(GatewayBindingType::Default) => {
            binding_info.insert(
                "binding-type".to_string(),
                serde_json::Value::String("default".to_string()),
            );
            add_common_binding_info(&mut binding_info, route);
        }
        Some(GatewayBindingType::HttpHandler) => {
            binding_info.insert(
                "binding-type".to_string(),
                serde_json::Value::String("http-handler".to_string()),
            );
            add_common_binding_info(&mut binding_info, route);
        }
        Some(GatewayBindingType::FileServer) => {
            binding_info.insert(
                "binding-type".to_string(),
                serde_json::Value::String("file-server".to_string()),
            );
            add_common_binding_info(&mut binding_info, route);
        }
        Some(GatewayBindingType::CorsPreflight) => {
            add_cors_preflight_binding_info(&mut binding_info, route);
        }
        Some(GatewayBindingType::SwaggerUi) => {
            binding_info.insert(
                "binding-type".to_string(),
                serde_json::Value::String("swagger-ui".to_string()),
            );
        }
        None => {
            binding_info.insert(
                "binding-type".to_string(),
                serde_json::Value::String("default".to_string()),
            );
        }
    }

    binding_info
}

// Helper function: Adds common binding info
fn add_common_binding_info(
    binding_info: &mut serde_json::Map<String, serde_json::Value>,
    route: &RouteResponseData,
) {
    if let Some(worker_name) = &route.binding.worker_name {
        binding_info.insert(
            "worker-name".to_string(),
            serde_json::Value::String(worker_name.clone()),
        );
    }

    if let Some(versioned_component) = &route.binding.component {
        binding_info.insert(
            "component-name".to_string(),
            serde_json::Value::String(versioned_component.name.clone()),
        );
        binding_info.insert(
            "component-version".to_string(),
            serde_json::Value::Number(serde_json::Number::from(versioned_component.version)),
        );
    }

    if let Some(key) = &route.binding.idempotency_key {
        binding_info.insert(
            "idempotency-key".to_string(),
            serde_json::Value::String(key.clone()),
        );
    }

    if let Some(response) = &route.binding.response {
        binding_info.insert(
            "response".to_string(),
            serde_json::Value::String(response.clone()),
        );
    }
}

// Helper function: Adds cors-preflight binding info
// Converts the cors-preflight binding info to a response string
fn add_cors_preflight_binding_info(
    binding_info: &mut serde_json::Map<String, serde_json::Value>,
    route: &RouteResponseData,
) {
    binding_info.insert(
        "binding-type".to_string(),
        serde_json::Value::String("cors-preflight".to_string()),
    );

    if let Some(cors) = &route.binding.cors_preflight {
        let formatted_response = create_cors_response(cors);
        binding_info.insert(
            "response".to_string(),
            serde_json::Value::String(formatted_response),
        );
    } else {
        binding_info.insert(
            "response".to_string(),
            serde_json::Value::String("{\n}".to_string()),
        );
    }
}

// Helper function: Creates a CORS response
fn create_cors_response(cors: &HttpCors) -> String {
    let mut headers = vec![
        format!(
            "Access-Control-Allow-Headers: \"{}\"",
            cors.get_allow_headers()
        ),
        format!(
            "Access-Control-Allow-Methods: \"{}\"",
            cors.get_allow_methods()
        ),
        format!(
            "Access-Control-Allow-Origin: \"{}\"",
            cors.get_allow_origin()
        ),
    ];

    if let Some(expose_headers) = cors.get_expose_headers() {
        headers.push(format!(
            "Access-Control-Expose-Headers: \"{}\"",
            expose_headers
        ));
    }

    if let Some(allow_credentials) = cors.get_allow_credentials() {
        headers.push(format!(
            "Access-Control-Allow-Credentials: {}",
            allow_credentials
        ));
    }
    if let Some(max_age) = cors.get_max_age() {
        headers.push(format!("Access-Control-Max-Age: {}u64", max_age));
    }

    let mut response_lines = vec!["{".to_string()];
    for (i, header) in headers.iter().enumerate() {
        let comma = if i < headers.len() - 1 { "," } else { "" };
        response_lines.push(format!("  {}{}", header, comma));
    }
    response_lines.push("}".to_string());

    response_lines.join("\n")
}

// Helper function: Creates a security scheme
fn create_security_scheme(security_ref: &str) -> openapiv3::SecurityScheme {
    openapiv3::SecurityScheme::APIKey {
        location: openapiv3::APIKeyLocation::Header,
        name: "Authorization".to_string(),
        description: Some(format!("API key security scheme for {}", security_ref)),
        extensions: indexmap::IndexMap::new(),
    }
}

// Helper function: Sets global security
fn set_global_security(
    open_api: &mut openapiv3::OpenAPI,
    response_data: &HttpApiDefinitionResponseData,
) {
    let mut seen_requirements = std::collections::HashSet::new();
    let mut global_security = Vec::new();

    for route in &response_data.routes {
        if let Some(security_ref) = &route.security {
            if !seen_requirements.contains::<str>(security_ref) {
                let mut req = indexmap::IndexMap::new();
                req.insert(security_ref.to_string(), Vec::<String>::new());
                global_security.push(req);
                seen_requirements.insert(security_ref.to_string());
            }
        }
    }

    if !global_security.is_empty() {
        open_api.security = Some(global_security);
    }
}

// Helper function: Adds operation to path item
fn add_operation_to_path_item(
    path_item: &mut openapiv3::PathItem,
    method: &MethodPattern,
    operation: openapiv3::Operation,
) -> Result<(), String> {
    match method {
        MethodPattern::Get => {
            path_item.get = Some(operation);
        }
        MethodPattern::Post => {
            path_item.post = Some(operation);
        }
        MethodPattern::Put => {
            path_item.put = Some(operation);
        }
        MethodPattern::Delete => {
            path_item.delete = Some(operation);
        }
        MethodPattern::Options => {
            path_item.options = Some(operation);
        }
        MethodPattern::Head => {
            path_item.head = Some(operation);
        }
        MethodPattern::Patch => {
            path_item.patch = Some(operation);
        }
        MethodPattern::Trace => {
            path_item.trace = Some(operation);
        }
        MethodPattern::Connect => {
            return Err("CONNECT method is not supported in OpenAPI v3 specification".to_string());
        }
    }
    Ok(())
}

// Helper function: Creates a default object schema
// Used as fallback in case type or fields are missing
fn create_default_object_schema() -> openapiv3::Schema {
    openapiv3::Schema {
        schema_data: Default::default(),
        schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
    }
}

// Macro: Generate the body of the match arm
macro_rules! create_integer_schema_body {
    ($format:expr, $min:expr, $max:expr) => {{
        let schema_data = openapiv3::SchemaData::default();
        openapiv3::Schema {
            schema_data,
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType {
                    format: openapiv3::VariantOrUnknownOrEmpty::Item($format),
                    minimum: $min,
                    maximum: $max,
                    multiple_of: None,
                    exclusive_minimum: false,
                    exclusive_maximum: false,
                    enumeration: vec![],
                },
            )),
        }
    }};
}

// Helper function: Converts AnalysedType to OpenAPI Schema
fn create_schema_from_analysed_type(
    analysed_type: &golem_wasm_ast::analysis::AnalysedType,
) -> openapiv3::Schema {
    use golem_wasm_ast::analysis::AnalysedType;

    match analysed_type {
        // Handle boolean type
        AnalysedType::Bool(_) => openapiv3::Schema {
            schema_data: Default::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Boolean(
                openapiv3::BooleanType::default(),
            )),
        },

        // Handle integer types using the macro for the body
        AnalysedType::U8(_) => {
            create_integer_schema_body!(openapiv3::IntegerFormat::Int32, Some(0), Some(255))
        }
        AnalysedType::U16(_) => {
            create_integer_schema_body!(openapiv3::IntegerFormat::Int32, Some(0), Some(65535))
        }
        AnalysedType::U32(_) => {
            create_integer_schema_body!(openapiv3::IntegerFormat::Int32, Some(0), None)
        }
        AnalysedType::U64(_) => {
            create_integer_schema_body!(openapiv3::IntegerFormat::Int64, Some(0), None)
        }
        AnalysedType::S8(_) => {
            create_integer_schema_body!(openapiv3::IntegerFormat::Int32, Some(-128), Some(127))
        }
        AnalysedType::S16(_) => {
            create_integer_schema_body!(openapiv3::IntegerFormat::Int32, Some(-32768), Some(32767))
        }
        AnalysedType::S32(_) => {
            create_integer_schema_body!(openapiv3::IntegerFormat::Int32, None, None)
        }
        AnalysedType::S64(_) => {
            create_integer_schema_body!(openapiv3::IntegerFormat::Int64, None, None)
        }

        AnalysedType::F32(_) => {
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
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
            let schema_data = openapiv3::SchemaData::default();
            openapiv3::Schema {
                schema_data,
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
            // Convert tuple to an array with anyOf for each position
            let min_items = Some(type_tuple.items.len());
            let max_items = Some(type_tuple.items.len());

            // We can represent tuples as arrays in OpenAPI
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

            // Add description indicating this is a tuple
            // Initialize schema_data directly with description
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
            let mut properties = indexmap::IndexMap::new();
            let mut required = Vec::new();

            for field in &type_record.fields {
                let field_schema = create_schema_from_analysed_type(&field.typ);
                let is_nullable = field_schema.schema_data.nullable;
                properties.insert(
                    field.name.clone(),
                    openapiv3::ReferenceOr::Item(Box::new(field_schema)),
                );
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
        }
        AnalysedType::Variant(type_variant) => {
            // For variants, create a discriminated union using oneOf
            let mut one_of = Vec::new();

            for case in &type_variant.cases {
                let case_name = &case.name;

                if let Some(case_type) = &case.typ {
                    // If the variant case has an associated type, create a schema for it
                    let case_schema = create_schema_from_analysed_type(case_type);

                    let mut properties = indexmap::IndexMap::new();
                    properties.insert(
                        case_name.clone(),
                        openapiv3::ReferenceOr::Item(Box::new(case_schema)),
                    );

                    // Use vec! macro for initialization
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
        }
        AnalysedType::Enum(type_enum) => {
            // Convert Vec<String> to Vec<Option<String>> for enumeration
            let enum_values: Vec<Option<String>> = type_enum
                .cases
                .iter()
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
        }
        AnalysedType::Option(type_option) => {
            // For Option, use nullable property
            let mut schema = create_schema_from_analysed_type(&type_option.inner);
            schema.schema_data.nullable = true;
            schema
        }
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
            ok_properties.insert(
                "ok".to_string(),
                openapiv3::ReferenceOr::Item(Box::new(ok_schema)),
            );

            // Use vec! macro for initialization
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

            // Create a schema for error case
            let mut err_properties = indexmap::IndexMap::new();
            err_properties.insert(
                "err".to_string(),
                openapiv3::ReferenceOr::Item(Box::new(err_schema)),
            );

            // Use vec! macro for initialization
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
        // Handle any other cases with a generic object type
        _ => create_default_object_schema(),
    }
}
