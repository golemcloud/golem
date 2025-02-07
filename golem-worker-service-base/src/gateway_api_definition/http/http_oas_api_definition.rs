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

use crate::gateway_api_definition::http::http_api_definition_request::HttpApiDefinitionRequest;
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use internal::*;
use openapiv3::OpenAPI;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ParseFromYAML, ParseResult};
use serde_json::Value;
use std::borrow::Cow;
use serde::{Serialize, Deserialize};

pub struct OpenApiHttpApiDefinitionRequest(pub OpenAPI);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub terms_of_service: Option<String>,
    pub contact_name: Option<String>,
    pub contact_url: Option<String>,
    pub contact_email: Option<String>,
    pub license_name: Option<String>,
    pub license_url: Option<String>,
}

impl Default for OpenApiMetadata {
    fn default() -> Self {
        Self {
            title: Some("OpenAPI Definition".to_string()),
            description: Some("API definition exported from Golem".to_string()),
            terms_of_service: None,
            contact_name: Some("Golem Cloud".to_string()),
            contact_url: None,
            contact_email: None,
            license_name: Some("Apache 2.0".to_string()),
            license_url: Some("https://www.apache.org/licenses/LICENSE-2.0".to_string()),
        }
    }
}

impl OpenApiHttpApiDefinitionRequest {
    pub fn to_http_api_definition_request(&self) -> Result<HttpApiDefinitionRequest, String> {
        let open_api = &self.0;
        let api_definition_id = ApiDefinitionId(get_root_extension_str(
            open_api,
            GOLEM_API_DEFINITION_ID_EXTENSION,
        )?);

        let api_definition_version = ApiVersion(get_root_extension_str(
            open_api,
            GOLEM_API_DEFINITION_VERSION,
        )?);

        let security = get_global_security(open_api);

        let routes = get_routes(&open_api.paths)?;

        Ok(HttpApiDefinitionRequest {
            id: api_definition_id,
            version: api_definition_version,
            routes,
            draft: true,
            security,
            metadata: None,
        })
    }

    pub fn from_http_api_definition_request(def: &crate::gateway_api_definition::http::HttpApiDefinitionRequest) -> Result<Self, String> {
        let mut open_api = openapiv3::OpenAPI::default();
        open_api.openapi = "3.0.0".to_string();
        
        // Get OpenAPI metadata from component metadata or use defaults
        let api_metadata = def.metadata
            .as_ref()
            .and_then(|m| m.openapi_metadata.as_ref())
            .cloned()
            .unwrap_or_default();
        
        // Enhanced Info object using metadata
        open_api.info = openapiv3::Info {
            title: api_metadata.title.unwrap_or_else(|| "OpenAPI Definition".to_string()),
            description: api_metadata.description,
            terms_of_service: api_metadata.terms_of_service,
            contact: Some(openapiv3::Contact {
                name: api_metadata.contact_name,
                url: api_metadata.contact_url,
                email: api_metadata.contact_email,
                extensions: Default::default(),
            }),
            license: Some(openapiv3::License {
                name: api_metadata.license_name.unwrap_or_else(|| "Apache 2.0".to_string()),
                url: api_metadata.license_url,
                extensions: Default::default(),
            }),
            version: def.version.0.clone(),
            extensions: Default::default(),
        };
        
        // Add Golem extensions
        open_api.extensions.insert(
            "x-golem-api-definition-id".to_string(), 
            serde_json::Value::String(def.id.0.clone())
        );
        open_api.extensions.insert(
            "x-golem-api-definition-version".to_string(), 
            serde_json::Value::String(def.version.0.clone())
        );
        
        let mut paths: std::collections::BTreeMap<String, openapiv3::PathItem> = std::collections::BTreeMap::new();

        // Components section for reusable schemas
        let mut components = openapiv3::Components::default();
        components.schemas = Default::default();
        open_api.components = Some(components);

        for route in &def.routes {
            let path_str = route.path.to_string();
            let mut operation = internal::get_operation_from_route(route)?;
            
            // Add default 200 response if none exists
            if operation.responses.responses.is_empty() {
                let mut response = openapiv3::Response::default();
                response.description = "Successful response".to_string();
                
                // Add default content type and schema
                let mut content = indexmap::IndexMap::new();
                let mut media = openapiv3::MediaType::default();
                media.schema = Some(openapiv3::ReferenceOr::Item(openapiv3::Schema {
                    schema_data: Default::default(),
                    schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Object(Default::default())),
                }));
                content.insert("application/json".to_string(), media);
                response.content = content;
                
                operation.responses.responses.insert(
                    openapiv3::StatusCode::Code(200),
                    openapiv3::ReferenceOr::Item(response),
                );
            }
            
            // Extract path parameters from the path and add them to operation parameters
            let params = extract_path_parameters(&path_str);
            for param_name in params {
                let parameter = openapiv3::Parameter::Path {
                    parameter_data: openapiv3::ParameterData {
                        name: param_name.clone(),
                        description: Some(format!("Path parameter: {}", param_name)),
                        required: true,
                        deprecated: None,
                        explode: Some(false),
                        format: openapiv3::ParameterSchemaOrContent::Schema(openapiv3::ReferenceOr::Item(
                            openapiv3::Schema {
                                schema_data: Default::default(),
                                schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(Default::default())),
                            }
                        )),
                        example: None,
                        examples: Default::default(),
                        extensions: Default::default(),
                    },
                    style: openapiv3::PathStyle::Simple,
                };
                operation.parameters.push(openapiv3::ReferenceOr::Item(parameter));
            }

            let path_item = paths.entry(path_str.clone()).or_insert_with(|| openapiv3::PathItem::default());
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
        }
        
        open_api.paths = openapiv3::Paths {
            paths: paths.into_iter()
                .map(|(k, v)| (k, openapiv3::ReferenceOr::Item(v)))
                .collect(),
            extensions: Default::default()
        };

        // Set the security field from def.security
        open_api.security = Some(
            def.security.as_ref().map_or(Vec::new(), |sec_refs| {
                sec_refs.iter().map(|sec_ref| {
                    let mut req: openapiv3::SecurityRequirement = openapiv3::SecurityRequirement::new();
                    req.insert(sec_ref.security_scheme_identifier.to_string(), Vec::<String>::new());
                    req
                }).collect()
            })
        );

        let has_cors_preflight = def.routes.iter().any(|route| {
            route.method == crate::gateway_api_definition::http::MethodPattern::Options &&
            matches!(route.binding, crate::gateway_binding::GatewayBinding::Static(_))
        });
        open_api.extensions.insert("x-golem-cors-preflight".to_string(), serde_json::Value::Bool(has_cors_preflight));

        Ok(OpenApiHttpApiDefinitionRequest(open_api))
    }
}

fn extract_path_parameters(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    for segment in path.split('/') {
        if segment.starts_with('{') && segment.ends_with('}') {
            params.push(segment[1..segment.len()-1].to_string());
        }
    }
    params
}

impl ParseFromJSON for OpenApiHttpApiDefinitionRequest {
    fn parse_from_json(value: Option<serde_json::Value>) -> ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(OpenApiHttpApiDefinitionRequest(openapi)),
                Err(e) => Err(ParseError::<Self>::custom(format!(
                    "Failed to parse OpenAPI: {}",
                    e
                ))),
            },

            _ => Err(ParseError::<Self>::custom(
                "OpenAPI spec missing".to_string(),
            )),
        }
    }
}

impl ParseFromYAML for OpenApiHttpApiDefinitionRequest {
    fn parse_from_yaml(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(OpenApiHttpApiDefinitionRequest(openapi)),
                Err(e) => Err(ParseError::<Self>::custom(format!(
                    "Failed to parse OpenAPI: {}",
                    e
                ))),
            },

            _ => Err(ParseError::<Self>::custom(
                "OpenAPI spec missing".to_string(),
            )),
        }
    }
}

impl poem_openapi::types::Type for OpenApiHttpApiDefinitionRequest {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "OpenApiDefinition".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema {
            title: Some("API definition in OpenAPI format".to_string()),
            description: Some("API definition in OpenAPI format with required custom extensions"),
            ..MetaSchema::new("OpenAPI+WorkerBridgeCustomExtension")
        }))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

mod internal {
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, RouteRequest};
    use golem_common::model::{ComponentId, GatewayBindingType};
    use openapiv3::{OpenAPI, Operation, Paths, ReferenceOr};
    use rib::Expr;
    use serde_json::Value;

    use crate::gateway_binding::{
        GatewayBinding, HttpHandlerBinding, ResponseMapping, StaticBinding, WorkerBinding,
    };
    use crate::gateway_middleware::{CorsPreflightExpr, HttpCors};
    use crate::gateway_security::{SecuritySchemeIdentifier, SecuritySchemeReference};
    use golem_service_base::model::VersionedComponentId;
    use uuid::Uuid;

    pub(crate) const GOLEM_API_DEFINITION_ID_EXTENSION: &str = "x-golem-api-definition-id";
    pub(crate) const GOLEM_API_DEFINITION_VERSION: &str = "x-golem-api-definition-version";

    // Legacy extension for worker bridge
    pub(crate) const GOLEM_WORKER_GATEWAY_EXTENSION_LEGACY: &str = "x-golem-worker-bridge";

    pub(crate) const GOLEM_API_GATEWAY_BINDING: &str = "x-golem-api-gateway-binding";

    pub(crate) fn get_global_security(open_api: &OpenAPI) -> Option<Vec<SecuritySchemeReference>> {
        open_api.security.as_ref().and_then(|requirements| {
            let global_security: Vec<_> = requirements
                .iter()
                .flat_map(|x| {
                    x.keys().map(|key| SecuritySchemeReference {
                        security_scheme_identifier: SecuritySchemeIdentifier::new(key.clone()),
                    })
                })
                .collect();

            if global_security.is_empty() {
                None
            } else {
                Some(global_security)
            }
        })
    }
    pub(crate) fn get_root_extension_str(
        open_api: &OpenAPI,
        key_name: &str,
    ) -> Result<String, String> {
        get_root_extension_value(open_api, key_name)
            .ok_or(format!("{} not found in the open API spec", key_name))?
            .as_str()
            .ok_or(format!("Invalid value for {}", key_name))
            .map(|x| x.to_string())
    }

    pub(crate) fn get_root_extension_value(open_api: &OpenAPI, key_name: &str) -> Option<Value> {
        open_api
            .extensions
            .iter()
            .find(|(key, __)| key.to_lowercase() == key_name)
            .map(|(_, v)| v.clone())
    }

    pub(crate) fn get_routes(paths: &Paths) -> Result<Vec<RouteRequest>, String> {
        let mut routes: Vec<RouteRequest> = vec![];

        for (path, path_item) in paths.iter() {
            match path_item {
                ReferenceOr::Item(item) => {
                    let path_pattern = get_path_pattern(path)?;

                    for (method, method_operation) in item.iter() {
                        let route =
                            get_route_from_path_item(method, method_operation, &path_pattern)?;
                        routes.push(route);
                    }
                }
                ReferenceOr::Reference { reference: _ } => {
                    return Err(
                        "Reference not supported yet when extracting worker bridge extension info"
                            .to_string(),
                    )
                }
            };
        }

        Ok(routes)
    }

    pub(crate) fn get_route_from_path_item(
        method: &str,
        method_operation: &Operation,
        path_pattern: &AllPathPatterns,
    ) -> Result<RouteRequest, String> {
        let method_res = match method {
            "get" => Ok(MethodPattern::Get),
            "post" => Ok(MethodPattern::Post),
            "put" => Ok(MethodPattern::Put),
            "delete" => Ok(MethodPattern::Delete),
            "options" => Ok(MethodPattern::Options),
            "head" => Ok(MethodPattern::Head),
            "patch" => Ok(MethodPattern::Patch),
            "trace" => Ok(MethodPattern::Trace),
            "connect" => Ok(MethodPattern::Connect),
            _ => Err("Other methods not supported".to_string()),
        };

        let method = method_res?;

        let security = method_operation
            .security
            .clone()
            .and_then(|x| x.clone().first().cloned());

        // Custom scopes to be supported later
        // Multiple security schemes to be supported later
        let security_name = security.and_then(|x| x.first().map(|x| x.0.clone()));

        let security = security_name.map(|x| SecuritySchemeReference {
            security_scheme_identifier: SecuritySchemeIdentifier::new(x),
        });

        let worker_gateway_info_optional = method_operation
            .extensions
            // TO keep backward compatibility with the old extension
            .get(GOLEM_WORKER_GATEWAY_EXTENSION_LEGACY)
            .or(method_operation.extensions.get(GOLEM_API_GATEWAY_BINDING));

        match worker_gateway_info_optional {
            Some(worker_gateway_info) => {
                let binding_type = get_binding_type(worker_gateway_info)?;

                match (&binding_type, &method) {
                    (GatewayBindingType::CorsPreflight, MethodPattern::Options) => {
                        let binding = get_cors_static_binding(worker_gateway_info)?;

                        Ok(RouteRequest {
                            method,
                            path: path_pattern.clone(),
                            binding: GatewayBinding::static_binding(binding),
                            security,
                            cors: None
                        })
                    }

                    (GatewayBindingType::Default, _) => {
                        let binding = get_worker_binding(worker_gateway_info)?;

                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::Default(binding),
                            security,
                            cors: None
                        })
                    }
                    (GatewayBindingType::FileServer, _) => {
                        let binding = get_worker_binding(worker_gateway_info)?;

                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::Default(binding),
                            security,
                            cors: None
                        })
                    }
                    (GatewayBindingType::HttpHandler, _) => {
                        let binding = get_http_handler_binding(worker_gateway_info)?;

                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::HttpHandler(binding),
                            security,
                            cors: None
                        })
                    }
                    (GatewayBindingType::CorsPreflight, method) => {
                        Err(format!("cors-preflight binding type is supported only for 'options' method, but found method '{}'", method))
                    },
                    (GatewayBindingType::SwaggerUi, _) => {
                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::SwaggerUi,
                            security,
                            cors: None
                        })
                    }
                }
            }

            None => {
                if method == MethodPattern::Options {
                    let binding = StaticBinding::from_http_cors(HttpCors::default());

                    Ok(RouteRequest {
                        path: path_pattern.clone(),
                        method,
                        binding: GatewayBinding::static_binding(binding),
                        security,
                        cors: None,
                    })
                } else {
                    Err(format!(
                        "No {} extension found",
                        GOLEM_WORKER_GATEWAY_EXTENSION_LEGACY
                    ))
                }
            }
        }
    }

    pub(crate) fn get_worker_binding(
        gateway_binding_value: &Value,
    ) -> Result<WorkerBinding, String> {
        let binding = WorkerBinding {
            worker_name: get_worker_id_expr(gateway_binding_value)?,
            component_id: get_component_id(gateway_binding_value)?,
            idempotency_key: get_idempotency_key(gateway_binding_value)?,
            response_mapping: get_response_mapping(gateway_binding_value)?,
        };

        Ok(binding)
    }

    pub(crate) fn get_http_handler_binding(
        gateway_binding_value: &Value,
    ) -> Result<HttpHandlerBinding, String> {
        let binding = HttpHandlerBinding {
            worker_name: get_worker_id_expr(gateway_binding_value)?,
            component_id: get_component_id(gateway_binding_value)?,
            idempotency_key: get_idempotency_key(gateway_binding_value)?,
        };

        Ok(binding)
    }

    pub(crate) fn get_cors_static_binding(
        worker_gateway_info: &Value,
    ) -> Result<StaticBinding, String> {
        match worker_gateway_info {
            Value::Object(map) => match map.get("response") {
                Some(value) => {
                    let rib_expr_text = value
                        .as_str()
                        .ok_or("response is not a Rib expression string")?;

                    let rib = rib::from_string(rib_expr_text).map_err(|err| err.to_string())?;

                    let cors_preflight =
                        HttpCors::from_cors_preflight_expr(&CorsPreflightExpr(rib))?;

                    Ok(StaticBinding::from_http_cors(cors_preflight))
                }

                None => Ok(StaticBinding::from_http_cors(HttpCors::default())),
            },
            _ => Err("Invalid schema for cors binding".to_string()),
        }
    }

    pub(crate) fn get_component_id(
        gateway_binding_value: &Value,
    ) -> Result<VersionedComponentId, String> {
        let comp_val = gateway_binding_value.get("component-id").ok_or("No component-id found")?;
        let component_id_str: String = serde_json::from_value(comp_val.clone()).map_err(|e| e.to_string())?;
        let version_val = gateway_binding_value.get("component-version").ok_or("No component-version found")?;
        let version: u64 = serde_json::from_value(version_val.clone()).map_err(|e| e.to_string())?;
        let component_id = ComponentId(Uuid::parse_str(&component_id_str).map_err(|err| err.to_string())?);
        Ok(VersionedComponentId { component_id, version })
    }

    pub(crate) fn get_binding_type(
        worker_gateway_info: &Value,
    ) -> Result<GatewayBindingType, String> {
        let binding_type_optional: Option<GatewayBindingType> = worker_gateway_info
            .get("binding-type")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|err| format!("Invalid schema for binding-type. {}", err))?;

        Ok(binding_type_optional.unwrap_or(GatewayBindingType::Default))
    }

    pub(crate) fn get_response_mapping(
        gateway_binding_value: &Value,
    ) -> Result<ResponseMapping, String> {
        let response_val = gateway_binding_value.get("response").ok_or("No response mapping found. It should be a JSON representation of the expression")?;
        let response = serde_json::from_value(response_val.clone()).map_err(|e| e.to_string())?;
        Ok(ResponseMapping(response))
    }

    pub(crate) fn get_worker_id_expr(
        gateway_binding_value: &Value,
    ) -> Result<Option<Expr>, String> {
        if let Some(val) = gateway_binding_value.get("worker-name") {
            let expr: Expr = serde_json::from_value(val.clone()).map_err(|e| e.to_string())?;
            Ok(Some(expr))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn get_idempotency_key(
        gateway_binding_value: &Value,
    ) -> Result<Option<Expr>, String> {
        if let Some(val) = gateway_binding_value.get("idempotency-key") {
            let expr: Expr = serde_json::from_value(val.clone()).map_err(|e| e.to_string())?;
            Ok(Some(expr))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn get_path_pattern(path: &str) -> Result<AllPathPatterns, String> {
        AllPathPatterns::parse(path)
    }

    pub(crate) fn get_operation_from_route(route: &crate::gateway_api_definition::http::RouteRequest) -> Result<openapiv3::Operation, String> {
        let mut operation = openapiv3::Operation::default();
        let mut binding_info = serde_json::Map::new();

        match &route.binding {
            GatewayBinding::Default(worker_binding) => {
                binding_info.insert("binding-type".to_string(), serde_json::Value::String("default".to_string()));
                binding_info.insert("worker-name".to_string(), serde_json::to_value(&worker_binding.worker_name).map_err(|e| e.to_string())?);
                binding_info.insert("component-id".to_string(), serde_json::to_value(&worker_binding.component_id).map_err(|e| e.to_string())?);
                if let Some(key) = &worker_binding.idempotency_key {
                    binding_info.insert("idempotency-key".to_string(), serde_json::to_value(key).map_err(|e| e.to_string())?);
                }
                binding_info.insert("response".to_string(), serde_json::to_value(&worker_binding.response_mapping).map_err(|e| e.to_string())?);
            },
            GatewayBinding::HttpHandler(http_handler_binding) => {
                binding_info.insert("binding-type".to_string(), serde_json::Value::String("http-handler".to_string()));
                binding_info.insert("worker-name".to_string(), serde_json::to_value(&http_handler_binding.worker_name).map_err(|e| e.to_string())?);
                binding_info.insert("component-id".to_string(), serde_json::to_value(&http_handler_binding.component_id).map_err(|e| e.to_string())?);
                if let Some(key) = &http_handler_binding.idempotency_key {
                    binding_info.insert("idempotency-key".to_string(), serde_json::to_value(key).map_err(|e| e.to_string())?);
                }
            },
            GatewayBinding::Static(static_binding) => {
                binding_info.insert("binding-type".to_string(), serde_json::Value::String("cors-preflight".to_string()));
                binding_info.insert("response".to_string(), serde_json::to_value(&static_binding).map_err(|e| e.to_string())?);
            },
            GatewayBinding::FileServer(worker_binding) => {
                binding_info.insert("binding-type".to_string(), serde_json::Value::String("file-server".to_string()));
                binding_info.insert("worker-name".to_string(), serde_json::to_value(&worker_binding.worker_name).map_err(|e| e.to_string())?);
                binding_info.insert("component-id".to_string(), serde_json::to_value(&worker_binding.component_id).map_err(|e| e.to_string())?);
                if let Some(key) = &worker_binding.idempotency_key {
                    binding_info.insert("idempotency-key".to_string(), serde_json::to_value(key).map_err(|e| e.to_string())?);
                }
                binding_info.insert("response".to_string(), serde_json::to_value(&worker_binding.response_mapping).map_err(|e| e.to_string())?);
            },
            GatewayBinding::SwaggerUi => {
                binding_info.insert("binding-type".to_string(), serde_json::Value::String("swagger-ui".to_string()));
            }
        }

        operation.extensions.insert("x-golem-api-gateway-binding".to_string(), serde_json::Value::Object(binding_info));
        Ok(operation)
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, RouteRequest};
    use crate::gateway_binding::{GatewayBinding, StaticBinding};
    use crate::gateway_middleware::HttpCors;

    use openapiv3::Operation;

    use serde_json::json;
    use indexmap::IndexMap;

    #[test]
    fn test_get_route_with_cors_preflight_binding() {
        let path_item = Operation {
            extensions: vec![(
                "x-golem-api-gateway-binding".to_string(),
                json!({
                    "binding-type": "cors-preflight",
                    "response" : "{Access-Control-Allow-Origin: \"apple.com\"}"
                }),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let path_pattern = AllPathPatterns::parse("/test").unwrap();

        let result = get_route_from_path_item("options", &path_item, &path_pattern);

        let expected = expected_route_with_cors_preflight_binding(&path_pattern);
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_get_route_with_cors_preflight_binding_default_response() {
        let path_item = Operation {
            extensions: vec![(
                "x-golem-api-gateway-binding".to_string(),
                json!({
                    "binding-type": "cors-preflight"
                }),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let path_pattern = AllPathPatterns::parse("/test").unwrap();

        let result = get_route_from_path_item("options", &path_item, &path_pattern);

        let expected = expected_route_with_cors_preflight_binding_default(&path_pattern);
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_get_route_with_no_binding_with_options_method() {
        let path_item = Operation::default();

        let path_pattern = AllPathPatterns::parse("/test").unwrap();

        let result = get_route_from_path_item("options", &path_item, &path_pattern);

        let expected = expected_route_with_cors_preflight_binding_default(&path_pattern);
        assert_eq!(result, Ok(expected));
    }

    fn expected_route_with_cors_preflight_binding_default(
        path_pattern: &AllPathPatterns,
    ) -> RouteRequest {
        RouteRequest {
            path: path_pattern.clone(),
            method: MethodPattern::Options,
            binding: GatewayBinding::static_binding(StaticBinding::from_http_cors(
                HttpCors::default(),
            )),
            security: None,
            cors: None,
        }
    }

    fn expected_route_with_cors_preflight_binding(path_pattern: &AllPathPatterns) -> RouteRequest {
        let mut cors_preflight = HttpCors::default();
        cors_preflight.set_allow_origin("apple.com").unwrap();
        RouteRequest {
            path: path_pattern.clone(),
            method: MethodPattern::Options,
            binding: GatewayBinding::static_binding(StaticBinding::from_http_cors(cors_preflight)),
            security: None,
            cors: None,
        }
    }

    #[test]
    fn test_conversion_preserves_security_and_cors() {
        use openapiv3::{OpenAPI, Info, Operation, PathItem, ReferenceOr};
        use std::collections::BTreeMap;
        use serde_json::json;
        use crate::gateway_api_definition::http::MethodPattern;

        // Build an OpenAPI spec with global security and both GET and OPTIONS operations
        let mut open_api = OpenAPI::default();
        open_api.info = Info {
            title: "Test API".into(),
            version: "1.0".into(),
            ..Default::default()
        };
        open_api.extensions.insert("x-golem-api-definition-id".to_string(), json!("test_export"));
        open_api.extensions.insert("x-golem-api-definition-version".to_string(), json!("1.0"));

        // Set global security: [{"api_key": []}]
        let mut global_sec = IndexMap::new();
        global_sec.insert("api_key".to_string(), Vec::<String>::new());
        open_api.security = Some(vec![global_sec]);

        // Create a GET operation with default binding and operation-level security
        let mut get_op = Operation::default();
        get_op.extensions.insert("x-golem-api-gateway-binding".to_string(), json!({
            "binding-type": "default",
            "worker-name": "workerA",
            "component-id": "550e8400-e29b-41d4-a716-446655440000",
            "component-version": 1,
            "response": "{dummy: \"data\"}"
        }));
        let mut op_sec = IndexMap::new();
        op_sec.insert("api_key".to_string(), Vec::<String>::new());
        get_op.security = Some(vec![op_sec]);

        // Create an OPTIONS operation with cors-preflight binding and a custom allow origin
        let mut options_op = Operation::default();
        options_op.extensions.insert("x-golem-api-gateway-binding".to_string(), json!({
            "binding-type": "cors-preflight",
            "response": "{Access-Control-Allow-Origin: \"example.com\"}"
        }));

        // Assemble the path item with both operations
        let mut path_item = PathItem::default();
        path_item.get = Some(get_op);
        path_item.options = Some(options_op);

        let mut paths = BTreeMap::new();
        paths.insert("/test".to_string(), ReferenceOr::Item(path_item));
        open_api.paths = openapiv3::Paths {
            paths: paths.into_iter()
                .map(|(k, v)| (k, v))  // No need for extra ReferenceOr wrapping
                .collect(),
            extensions: Default::default()
        };

        // Convert the OpenAPI spec into our internal representation
        let openapi_req = OpenApiHttpApiDefinitionRequest(open_api);
        let api_def_req = openapi_req.to_http_api_definition_request().unwrap();

        // Verify that the global security is preserved
        assert!(api_def_req.security.is_some(), "Global security should be preserved.");
        let global_sec_preserved = api_def_req.security.unwrap();
        let has_api_key = global_sec_preserved.into_iter().any(|sec| sec.security_scheme_identifier.to_string() == "api_key");
        assert!(has_api_key, "Global security should include 'api_key'.");

        // Verify that both GET and OPTIONS routes are correctly converted
        let mut found_get = false;
        let mut found_options = false;
        for route in api_def_req.routes {
            if route.method == MethodPattern::Get {
                found_get = true;
                if let crate::gateway_binding::GatewayBinding::Default(_) = route.binding {
                    // Expected default binding
                } else {
                    panic!("GET route should use default binding.");
                }
                assert!(route.security.is_some(), "GET route should have security set.");
                let sec_ref = route.security.unwrap();
                assert_eq!(sec_ref.security_scheme_identifier.to_string(), "api_key", "GET route security scheme should be 'api_key'.");
            } else if route.method == MethodPattern::Options {
                found_options = true;
                if let crate::gateway_binding::GatewayBinding::Static(sb) = route.binding {
                    if let Some(cors) = sb.get_cors_preflight() {
                        assert_eq!(cors.get_allow_origin(), "example.com", "OPTIONS route should have custom allow origin 'example.com'.");
                    } else {
                        panic!("OPTIONS route should have CORS preflight configuration.");
                    }
                } else {
                    panic!("OPTIONS route should use cors-preflight binding.");
                }
            }
        }
        assert!(found_get, "GET route not found.");
        assert!(found_options, "OPTIONS route not found.");
    }
}
