// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_api_definition::http::oas_api_definition::OpenApiHttpApiDefinition;
use crate::gateway_api_definition::http::{
    CompiledHttpApiDefinition, CompiledRoute, MethodPattern,
};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_binding::{
    GatewayBindingCompiled, ResponseMappingCompiled, StaticBinding, WorkerNameCompiled,
};
use crate::gateway_middleware::{CorsPreflightExpr, HttpCors};
use crate::service::gateway::BoxConversionContext;
use golem_common::model::component::VersionedComponentId;
use golem_common::model::GatewayBindingType;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_ast::analysis::NameTypePair;
use http::StatusCode;
use rib::RibInputTypeInfo;
use serde::{Deserialize, Serialize};

// Constants for OpenAPI extensions
const GOLEM_API_DEFINITION_ID_EXTENSION: &str = "x-golem-api-definition-id";
const GOLEM_API_DEFINITION_VERSION: &str = "x-golem-api-definition-version";
const GOLEM_API_GATEWAY_BINDING: &str = "x-golem-api-gateway-binding";

// OpenApiHttpApiDefinitionResponse is a wrapper id, version and open api schema as yaml string
// OpenApiHttpApiDefinition struct is defined using crate openapiv3 as OPENAPI+GOLEMEXTENSIONS
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, poem_openapi::Object)]
pub struct OpenApiHttpApiDefinitionResponse {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub openapi_yaml: String,
}

// OpenApiHttpApiDefinitionResponse implementation
impl OpenApiHttpApiDefinitionResponse {
    pub async fn from_compiled_http_api_definition(
        compiled_api_definition: &CompiledHttpApiDefinition,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<Self, String> {
        let openapi = OpenApiHttpApiDefinition::from_compiled_http_api_definition(
            compiled_api_definition,
            conversion_ctx,
        )
        .await?;
        let openapi_yaml = serde_yaml::to_string(&openapi)
            .map_err(|e| format!("Failed to serialize OpenAPI to YAML: {e}"))?;

        Ok(Self {
            id: compiled_api_definition.id.clone(),
            version: compiled_api_definition.version.clone(),
            openapi_yaml,
        })
    }
}

// Convert CompiledHttpApiDefinition to OpenHttpApiDefinition
impl OpenApiHttpApiDefinition {
    pub async fn from_compiled_http_api_definition(
        compiled_api_definition: &CompiledHttpApiDefinition,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<Self, String> {
        let mut open_api = create_base_openapi(compiled_api_definition);
        let mut paths = std::collections::BTreeMap::new();
        let mut security_schemes = indexmap::IndexMap::new();

        // Process each route and build paths
        for route in &compiled_api_definition.routes {
            // Skip auth callback routes, they shouldn't be exposed in OpenAPI
            if route.as_auth_callback_route().is_none() {
                process_route(route, &mut paths, &mut security_schemes, conversion_ctx).await?;
            }
        }

        // Set paths and security in OpenAPI spec
        finalize_openapi(
            &mut open_api,
            paths,
            security_schemes,
            compiled_api_definition,
        );

        Ok(OpenApiHttpApiDefinition(open_api))
    }
}

// Helper function: Base OpenAPI structure
fn create_base_openapi(compiled_api_definition: &CompiledHttpApiDefinition) -> openapiv3::OpenAPI {
    let mut open_api = openapiv3::OpenAPI {
        openapi: "3.0.0".to_string(),
        info: openapiv3::Info {
            title: compiled_api_definition.id.0.clone(),
            description: None,
            terms_of_service: None,
            contact: None,
            license: None,
            version: compiled_api_definition.version.0.clone(),
            extensions: Default::default(),
        },
        ..Default::default()
    };

    // Add Golem extensions
    open_api.extensions.insert(
        GOLEM_API_DEFINITION_ID_EXTENSION.to_string(),
        serde_json::Value::String(compiled_api_definition.id.0.clone()),
    );
    open_api.extensions.insert(
        GOLEM_API_DEFINITION_VERSION.to_string(),
        serde_json::Value::String(compiled_api_definition.version.0.clone()),
    );

    // Initialize components
    open_api.components = Some(openapiv3::Components::default());

    open_api
}

// Helper function: Handles each route in the CompiledHttpApiDefinition
async fn process_route(
    route: &CompiledRoute,
    paths: &mut std::collections::BTreeMap<String, openapiv3::PathItem>,
    security_schemes: &mut indexmap::IndexMap<
        String,
        openapiv3::ReferenceOr<openapiv3::SecurityScheme>,
    >,
    conversion_ctx: &BoxConversionContext<'_>,
) -> Result<(), String> {
    let path_str = route.path.to_string(); // AllPathPatterns to String
    let path_item = paths.entry(path_str.clone()).or_default();

    let operation = create_operation(route, security_schemes, conversion_ctx).await?;

    // Add operation to path item based on method
    add_operation_to_path_item(path_item, &route.method, operation)?;

    Ok(())
}

// Helper function: Adds path, query and header parameters to the operation
fn add_parameters(operation: &mut openapiv3::Operation, route: &CompiledRoute) {
    let (path_parameters, query_parameters, header_parameters) = get_parameters(route);

    // Add path parameters
    for (param_name, param_type) in path_parameters {
        let parameter = create_path_parameter(&param_name, param_type);
        operation
            .parameters
            .push(openapiv3::ReferenceOr::Item(parameter));
    }

    // Add query parameters
    for (param_name, param_type) in query_parameters {
        let parameter = create_query_parameter(&param_name, param_type);
        operation
            .parameters
            .push(openapiv3::ReferenceOr::Item(parameter));
    }

    // Add header parameters
    for (param_name, param_type) in header_parameters {
        let parameter = create_header_parameter(&param_name, param_type);
        operation
            .parameters
            .push(openapiv3::ReferenceOr::Item(parameter));
    }
}

// Helper function: Creates an operation for a route
async fn create_operation(
    route: &CompiledRoute,
    security_schemes: &mut indexmap::IndexMap<
        String,
        openapiv3::ReferenceOr<openapiv3::SecurityScheme>,
    >,
    conversion_ctx: &BoxConversionContext<'_>,
) -> Result<openapiv3::Operation, String> {
    let mut operation = openapiv3::Operation::default();

    add_parameters(&mut operation, route);
    add_request_body(&mut operation, route);
    add_responses(&mut operation, route);
    add_binding_info(&mut operation, route, conversion_ctx).await?;
    add_security(&mut operation, route, security_schemes);

    Ok(operation)
}

// Define a type alias for the parameter tuple
type ParameterTuple = (String, openapiv3::Schema);

// Helper function: Extracts parameters from a record and adds them to the appropriate collection
fn extract_parameters_from_record(
    record: &golem_wasm_ast::analysis::AnalysedType,
    path_parameters: &mut Vec<ParameterTuple>,
    query_parameters: &mut Vec<ParameterTuple>,
    header_parameters: &mut Vec<ParameterTuple>,
) {
    if let AnalysedType::Record(request_record) = record {
        // Check for path field
        if let Some(path_field) = request_record
            .fields
            .iter()
            .find(|field| field.name == "path")
        {
            if let AnalysedType::Record(path_record) = &path_field.typ {
                for field in &path_record.fields {
                    let schema = create_schema_from_analysed_type(&field.typ);
                    path_parameters.push((field.name.clone(), schema));
                }
            }
        }
        // Check for query field
        if let Some(query_field) = request_record
            .fields
            .iter()
            .find(|field| field.name == "query")
        {
            if let AnalysedType::Record(query_record) = &query_field.typ {
                for field in &query_record.fields {
                    let schema = create_schema_from_analysed_type(&field.typ);
                    query_parameters.push((field.name.clone(), schema));
                }
            }
        }
        // Check for headers field
        if let Some(headers_field) = request_record
            .fields
            .iter()
            .find(|field| field.name == "headers")
        {
            if let AnalysedType::Record(headers_record) = &headers_field.typ {
                for field in &headers_record.fields {
                    let schema = create_schema_from_analysed_type(&field.typ);
                    header_parameters.push((field.name.clone(), schema));
                }
            }
        }
    }
}

// Helper function: Gets path and query parameters with their types from route
// Returns three separate lists: one for path parameters, one for query parameters, and one for header parameters
fn get_parameters(
    route: &CompiledRoute,
) -> (
    Vec<ParameterTuple>,
    Vec<ParameterTuple>,
    Vec<ParameterTuple>,
) {
    let mut path_parameters = Vec::new();
    let mut query_parameters = Vec::new();
    let mut header_parameters = Vec::new();

    // Extract parameters based on binding type
    match &route.binding {
        GatewayBindingCompiled::Worker(worker_binding) => {
            // WorkerBindingCompiled doesn't have worker_name_compiled field
            // Check response_mapping_input
            if let Some(request_record) = worker_binding
                .response_compiled
                .rib_input
                .types
                .get("request")
            {
                extract_parameters_from_record(
                    request_record,
                    &mut path_parameters,
                    &mut query_parameters,
                    &mut header_parameters,
                );
            }
        }
        GatewayBindingCompiled::FileServer(file_server_binding) => {
            // Check worker_name_compiled for FileServer
            if let Some(worker_name_compiled) = &file_server_binding.worker_name_compiled {
                if let Some(request_record) = worker_name_compiled
                    .rib_input_type_info
                    .types
                    .get("request")
                {
                    extract_parameters_from_record(
                        request_record,
                        &mut path_parameters,
                        &mut query_parameters,
                        &mut header_parameters,
                    );
                }
            }

            // Check response_mapping_input
            if let Some(request_record) = file_server_binding
                .response_compiled
                .rib_input
                .types
                .get("request")
            {
                extract_parameters_from_record(
                    request_record,
                    &mut path_parameters,
                    &mut query_parameters,
                    &mut header_parameters,
                );
            }
        }
        GatewayBindingCompiled::HttpHandler(http_handler_binding) => {
            // Check worker_name_compiled for HttpHandler
            if let Some(worker_name_compiled) = &http_handler_binding.worker_name_compiled {
                if let Some(request_record) = worker_name_compiled
                    .rib_input_type_info
                    .types
                    .get("request")
                {
                    extract_parameters_from_record(
                        request_record,
                        &mut path_parameters,
                        &mut query_parameters,
                        &mut header_parameters,
                    );
                }
            }
        }
        // Other binding types don't have parameters
        _ => {}
    }

    (path_parameters, query_parameters, header_parameters)
}

// Helper function: Creates a path parameter
fn create_path_parameter(param_name: &str, param_type: openapiv3::Schema) -> openapiv3::Parameter {
    let parameter_data = openapiv3::ParameterData {
        name: param_name.to_string(),
        description: Some(format!("Path parameter: {param_name}")),
        required: true,
        deprecated: None,
        explode: Some(false),
        format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(
            param_type,
        )),
        example: None,
        examples: Default::default(),
        extensions: Default::default(),
    };

    openapiv3::Parameter::Path {
        parameter_data,
        style: openapiv3::PathStyle::Simple,
    }
}

// Helper function: Creates a query parameter
fn create_query_parameter(param_name: &str, param_type: openapiv3::Schema) -> openapiv3::Parameter {
    let parameter_data = openapiv3::ParameterData {
        name: param_name.to_string(),
        description: Some(format!("Query parameter: {param_name}")),
        required: true,
        deprecated: None,
        explode: Some(false),
        format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(
            param_type,
        )),
        example: None,
        examples: Default::default(),
        extensions: Default::default(),
    };

    openapiv3::Parameter::Query {
        parameter_data,
        style: openapiv3::QueryStyle::Form,
        allow_empty_value: Some(false),
        allow_reserved: false,
    }
}

// Helper function: Creates a header parameter
fn create_header_parameter(
    param_name: &str,
    param_type: openapiv3::Schema,
) -> openapiv3::Parameter {
    let parameter_data = openapiv3::ParameterData {
        name: param_name.to_string(),
        description: Some(format!("Header parameter: {param_name}")),
        required: true,
        deprecated: None,
        explode: Some(false),
        format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(
            param_type,
        )),
        example: None,
        examples: Default::default(),
        extensions: Default::default(),
    };

    openapiv3::Parameter::Header {
        parameter_data,
        style: openapiv3::HeaderStyle::Simple,
    }
}

// Helper function: Adds request body to the operation
fn add_request_body(operation: &mut openapiv3::Operation, route: &CompiledRoute) {
    // Only add request body if we have a binding that contains response_mapping
    match &route.binding {
        GatewayBindingCompiled::Worker(worker_binding) => {
            if let Some(request_body) =
                create_request_body(route, &worker_binding.response_compiled.rib_input)
            {
                operation.request_body = Some(openapiv3::ReferenceOr::Item(request_body));
            }
        }
        GatewayBindingCompiled::FileServer(file_server_binding) => {
            if let Some(request_body) =
                create_request_body(route, &file_server_binding.response_compiled.rib_input)
            {
                operation.request_body = Some(openapiv3::ReferenceOr::Item(request_body));
            }
        }
        _ => {}
    }
}

// Helper function: Determines response schema
fn determine_response_schema_and_content_type(
    route: &CompiledRoute,
) -> (Option<openapiv3::Schema>, String) {
    if let GatewayBindingCompiled::Worker(worker_binding) = &route.binding {
        if let Some(output_info) = &worker_binding.response_compiled.rib_output {
            if let AnalysedType::Record(record) = &output_info.analysed_type {
                let (headers_opt, body_opt, status_opt) = extract_response_fields(&record.fields);
                let content_type = "application/json".to_string();
                if headers_opt.is_some() || body_opt.is_some() || status_opt.is_some() {
                    let schema = body_opt.map(|body| create_schema_from_analysed_type(&body));
                    return (schema, content_type);
                } else {
                    // No structured response fields, use entire record
                    let schema = Some(create_schema_from_analysed_type(&output_info.analysed_type));
                    return (schema, content_type);
                }
            }
            // Not a record, use type as-is
            let schema = Some(create_schema_from_analysed_type(&output_info.analysed_type));
            return (schema, "application/json".to_string());
        }
    }
    (None, "application/json".to_string())
}

// Helper function for extracting headers, body and status from response record fields
fn extract_response_fields(
    record_name_type_pairs: &[NameTypePair],
) -> (
    Option<AnalysedType>,
    Option<AnalysedType>,
    Option<AnalysedType>,
) {
    let mut headers_opt = None;
    let mut body_opt = None;
    let mut status_opt = None;

    for field in record_name_type_pairs {
        match field.name.as_str() {
            "headers" => headers_opt = Some(field.typ.clone()),
            "body" => body_opt = Some(field.typ.clone()),
            "status" => status_opt = Some(field.typ.clone()),
            _ => {}
        }
    }

    (headers_opt, body_opt, status_opt)
}

// Helper function: Adds responses to the operation
fn add_responses(operation: &mut openapiv3::Operation, route: &CompiledRoute) {
    let default_status = get_default_status_code(&route.method);
    let (response_schema_opt, content_type) = determine_response_schema_and_content_type(route);
    let response = create_response_with_schema(default_status, response_schema_opt, &content_type);

    // Once as specific response and another as default to cover other status codes
    operation.responses.responses.insert(
        openapiv3::StatusCode::Code(default_status),
        openapiv3::ReferenceOr::Item(response.clone()),
    );

    operation.responses.default = Some(openapiv3::ReferenceOr::Item(response));
}

// Helper function: Create response with optional schema
fn create_response_with_schema(
    status_code: u16,
    response_schema: Option<openapiv3::Schema>,
    content_type: &str,
) -> openapiv3::Response {
    let mut response = openapiv3::Response {
        description: get_status_description(status_code),
        ..Default::default()
    };

    // Only add content if we have a response schema and it's not a 204 response
    if status_code != 204 {
        if let Some(schema) = response_schema {
            let mut content = indexmap::IndexMap::new();

            // Add the primary content type with the specific schema
            let media = openapiv3::MediaType {
                schema: Some(openapiv3::ReferenceOr::Item(schema)),
                ..Default::default()
            };
            content.insert(content_type.to_string(), media);

            // Add */* content type with string schema for other possible responses
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

            let string_media = openapiv3::MediaType {
                schema: Some(openapiv3::ReferenceOr::Item(string_schema)),
                ..Default::default()
            };
            content.insert("*/*".to_string(), string_media);

            response.content = content;
        }
    }

    response
}

// Helper function: Adds binding info to the operation
async fn add_binding_info(
    operation: &mut openapiv3::Operation,
    route: &CompiledRoute,
    conversion_ctx: &BoxConversionContext<'_>,
) -> Result<(), String> {
    let binding_info = create_binding_info(route, conversion_ctx).await?;
    operation.extensions.insert(
        GOLEM_API_GATEWAY_BINDING.to_string(),
        serde_json::Value::Object(binding_info),
    );
    Ok(())
}

// Helper function: Adds security to the operation
fn add_security(
    operation: &mut openapiv3::Operation,
    route: &CompiledRoute,
    security_schemes: &mut indexmap::IndexMap<
        String,
        openapiv3::ReferenceOr<openapiv3::SecurityScheme>,
    >,
) {
    if let Some(security_middleware) = route.get_security_middleware() {
        let security_ref = security_middleware
            .security_scheme_with_metadata
            .security_scheme
            .scheme_identifier()
            .to_string();

        // Add security scheme if not already present
        if !security_schemes.contains_key::<str>(&security_ref) {
            let security_scheme = create_security_scheme(&security_ref);
            security_schemes.insert(
                security_ref.clone(),
                openapiv3::ReferenceOr::Item(security_scheme),
            );
        }

        // Add security requirement to operation
        let mut req = indexmap::IndexMap::new();
        req.insert(security_ref, Vec::<String>::new());
        operation.security = Some(vec![req]);
    }
}

// Helper function: Finalizes OpenAPI specification
fn finalize_openapi(
    open_api: &mut openapiv3::OpenAPI,
    paths: std::collections::BTreeMap<String, openapiv3::PathItem>,
    security_schemes: indexmap::IndexMap<String, openapiv3::ReferenceOr<openapiv3::SecurityScheme>>,
    compiled_api_definition: &CompiledHttpApiDefinition,
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
    set_global_security(open_api, compiled_api_definition);
}

// Helper function: Creates a request body
fn create_request_body(
    _route: &CompiledRoute,
    response_mapping_input: &RibInputTypeInfo,
) -> Option<openapiv3::RequestBody> {
    if let Some(body_schema) = determine_request_body_schema(response_mapping_input) {
        let media_type = openapiv3::MediaType {
            schema: Some(openapiv3::ReferenceOr::Item(body_schema)),
            ..Default::default()
        };

        let mut content = indexmap::IndexMap::new();
        // Request body is always JSON
        content.insert("application/json".to_string(), media_type);

        Some(openapiv3::RequestBody {
            description: Some("Request payload".to_string()),
            content,
            required: true,
            extensions: Default::default(),
        })
    } else {
        // No request body schema found, return None
        None
    }
}

// Helper function: Determines request body schema
fn determine_request_body_schema(
    response_mapping_input: &RibInputTypeInfo,
) -> Option<openapiv3::Schema> {
    // Check request key and then look for body field within the request
    if let Some(AnalysedType::Record(request_record)) = response_mapping_input.types.get("request")
    {
        if let Some(body_field) = request_record.fields.iter().find(|f| f.name == "body") {
            return Some(create_schema_from_analysed_type(&body_field.typ));
        }
    }
    // No body field found, return None to indicate no request body
    None
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

// Helper function: Gets status code description
// Limited to 200, 201, 204 from possible MethodPattern
fn get_status_description(status_code: u16) -> String {
    StatusCode::from_u16(status_code)
        .map(|code| code.canonical_reason().unwrap_or("Unknown").to_string())
        .unwrap_or_else(|_| "Unknown".to_string())
}

// Helper function: Converts GatewayBindingCompiled to GatewayBindingType for automatic kebab-case serialization
fn get_binding_type_from_compiled(binding: &GatewayBindingCompiled) -> GatewayBindingType {
    match binding {
        GatewayBindingCompiled::Worker(_) => GatewayBindingType::Default,
        GatewayBindingCompiled::FileServer(_) => GatewayBindingType::FileServer,
        GatewayBindingCompiled::HttpHandler(_) => GatewayBindingType::HttpHandler,
        GatewayBindingCompiled::Static(static_binding) => match static_binding {
            StaticBinding::HttpCorsPreflight(_) => GatewayBindingType::CorsPreflight,
            StaticBinding::HttpAuthCallBack(_) => GatewayBindingType::CorsPreflight,
        },
        GatewayBindingCompiled::SwaggerUi(_) => GatewayBindingType::SwaggerUi,
    }
}

#[derive(Default)]
struct ExtractedBindingData<'a> {
    component_id: Option<&'a VersionedComponentId>,
    worker_name: Option<&'a WorkerNameCompiled>,
    response: Option<&'a ResponseMappingCompiled>,
    cors_preflight: Option<&'a HttpCors>,
}

fn extract_binding_data(binding: &GatewayBindingCompiled) -> ExtractedBindingData {
    match binding {
        GatewayBindingCompiled::Worker(w) => ExtractedBindingData {
            component_id: Some(&w.component_id),
            worker_name: None, // WorkerBindingCompiled doesn't have worker_name_compiled
            response: Some(&w.response_compiled),
            ..Default::default()
        },
        GatewayBindingCompiled::FileServer(w) => ExtractedBindingData {
            component_id: Some(&w.component_id),
            worker_name: w.worker_name_compiled.as_ref(),
            response: Some(&w.response_compiled),
            ..Default::default()
        },
        GatewayBindingCompiled::HttpHandler(h) => ExtractedBindingData {
            component_id: Some(&h.component_id),
            worker_name: h.worker_name_compiled.as_ref(),
            ..Default::default()
        },
        GatewayBindingCompiled::Static(StaticBinding::HttpCorsPreflight(cors)) => {
            ExtractedBindingData {
                cors_preflight: Some(cors),
                ..Default::default()
            }
        }
        _ => ExtractedBindingData::default(), // SwaggerUi, AuthCallBack need no extra fields
    }
}

async fn create_binding_info(
    route: &CompiledRoute,
    conversion_ctx: &BoxConversionContext<'_>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut binding_info = serde_json::Map::new();

    // Get binding type and serialize it to kebab-case string automatically
    let binding_type = get_binding_type_from_compiled(&route.binding);
    let binding_type_str = serde_json::to_value(&binding_type)
        .map_err(|e| format!("Failed to serialize binding type: {e}"))?
        .as_str()
        .unwrap()
        .to_string();

    binding_info.insert(
        "binding-type".to_string(),
        serde_json::Value::String(binding_type_str),
    );

    // Extract all binding data in one place
    let data = extract_binding_data(&route.binding);

    // Add component info
    if let Some(component_id) = data.component_id {
        let component_view = conversion_ctx
            .component_by_id(&component_id.component_id)
            .await?;

        binding_info.insert(
            "component-name".to_string(),
            serde_json::Value::String(component_view.name.0),
        );
        binding_info.insert(
            "component-version".to_string(),
            serde_json::Value::Number(serde_json::Number::from(component_id.version)),
        );
    }

    if let Some(worker_name) = data.worker_name {
        binding_info.insert(
            "worker-name".to_string(),
            serde_json::Value::String(worker_name.worker_name.to_string()),
        );
    }

    // Add response
    if let Some(response) = data.response {
        binding_info.insert(
            "response".to_string(),
            serde_json::Value::String(response.response_mapping_expr.to_string()),
        );
    }

    // Add CORS preflight response for Cors Binding
    if let Some(cors) = data.cors_preflight {
        let cors_expr = CorsPreflightExpr::from_cors(cors);
        binding_info.insert(
            "response".to_string(),
            serde_json::Value::String(cors_expr.0.to_string()),
        );
    }

    Ok(binding_info)
}

// Helper function: Creates a security scheme
fn create_security_scheme(security_ref: &str) -> openapiv3::SecurityScheme {
    openapiv3::SecurityScheme::APIKey {
        location: openapiv3::APIKeyLocation::Header,
        name: "Authorization".to_string(),
        description: Some(format!("API key security scheme for {security_ref}")),
        extensions: indexmap::IndexMap::new(),
    }
}

// Helper function: Sets global security
fn set_global_security(
    open_api: &mut openapiv3::OpenAPI,
    compiled_api_definition: &CompiledHttpApiDefinition,
) {
    let mut seen_requirements = std::collections::HashSet::new();
    let mut global_security = Vec::new();

    for route in &compiled_api_definition.routes {
        if let Some(security_middleware) = route.get_security_middleware() {
            let security_ref = security_middleware
                .security_scheme_with_metadata
                .security_scheme
                .scheme_identifier()
                .to_string();
            if !seen_requirements.contains::<str>(&security_ref) {
                let mut req = indexmap::IndexMap::new();
                req.insert(security_ref.clone(), Vec::<String>::new());
                global_security.push(req);
                seen_requirements.insert(security_ref);
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

// Helper function: Creates an integer schema
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

        // Handle integer types using the function for the body
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
            // Flags are represented as an array of strings where each string is a flag name
            let enum_values: Vec<Option<String>> = type_flags
                .names
                .iter()
                .map(|name| Some(name.clone()))
                .collect();

            // Create an array schema where items are string enums
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
        AnalysedType::Chr(_) => {
            // Chr is represented as a string with one character
            openapiv3::Schema {
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
            }
        }
        // Handle(_) => todo!()
        // This will not trigger for any case since Handle(_) is todo!()
        _ => todo!(),
    }
}