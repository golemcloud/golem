use crate::gateway_api_definition::http::HttpApiDefinitionRequest;
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use internal::*;
use openapiv3::OpenAPI;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ParseFromYAML, ParseResult};
use serde_json::Value;
use std::borrow::Cow;

pub struct OpenApiDefinitionRequest(pub OpenAPI);

impl OpenApiDefinitionRequest {
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

        let global_security = get_global_security(open_api);

        let routes = get_routes(&open_api.paths)?;

        Ok(HttpApiDefinitionRequest {
            id: api_definition_id,
            version: api_definition_version,
            routes,
            draft: true,
            security: global_security,
        })
    }
}

impl ParseFromJSON for OpenApiDefinitionRequest {
    fn parse_from_json(value: Option<serde_json::Value>) -> ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(OpenApiDefinitionRequest(openapi)),
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

impl ParseFromYAML for OpenApiDefinitionRequest {
    fn parse_from_yaml(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(OpenApiDefinitionRequest(openapi)),
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

impl poem_openapi::types::Type for OpenApiDefinitionRequest {
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
    use crate::gateway_api_definition::http::{
        AllPathPatterns, MethodPattern, Route, RouteRequest,
    };
    use golem_common::model::{ComponentId, GatewayBindingType};
    use openapiv3::{OpenAPI, Operation, Paths, ReferenceOr};
    use rib::Expr;
    use serde_json::Value;

    use crate::gateway_binding::{GatewayBinding, ResponseMapping, StaticBinding, WorkerBinding};
    use crate::gateway_middleware::{
        Cors, CorsPreflightExpr, HttpMiddleware, Middleware, Middlewares,
    };
    use crate::gateway_security::{SecuritySchemeIdentifier, SecuritySchemeReference};
    use golem_service_base::model::VersionedComponentId;
    use uuid::Uuid;

    pub(crate) const GOLEM_API_DEFINITION_ID_EXTENSION: &str = "x-golem-api-definition-id";
    pub(crate) const GOLEM_API_DEFINITION_VERSION: &str = "x-golem-api-definition-version";

    // Legacy extension for worker bridge
    pub(crate) const GOLEM_WORKER_GATEWAY_EXTENSION_LEGACY: &str = "x-golem-worker-bridge";

    pub(crate) const GOLEM_API_GATEWAY_BINDING: &str = "x-golem-api-gateway-binding";

    pub(crate) fn get_global_security(open_api: &OpenAPI) -> Option<SecuritySchemeReference> {
        let global_security = open_api.security.clone().and_then(|x| x.first());
        let global_security_name = global_security.and_then(|x| x.first().map(|x| x.0.to_string()));

        global_security_name.map(|x| SecuritySchemeReference {
            security_scheme_identifier: SecuritySchemeIdentifier::new(x),
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
            _ => Err("Other methods not supported".to_string()),
        };

        let method = method_res?;

        let security = method_operation.security.clone().and_then(|x| x.first());

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
                            binding: GatewayBinding::Static(binding),
                            security
                        })
                    }

                    (GatewayBindingType::Default, _) => {
                        let binding = get_gateway_binding(worker_gateway_info)?;

                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::Default(binding),
                            security
                        })
                    }
                    (GatewayBindingType::FileServer, _) => {
                        let binding = get_gateway_binding(worker_gateway_info)?;

                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::Default(binding),
                            security
                        })
                    }


                    (GatewayBindingType::CorsPreflight, method) => {
                        Err(format!("cors-preflight binding type is supported only for 'options' method, but found method '{}'", method))
                    }
                }
            }

            None => {
                if method == MethodPattern::Options {
                    let binding = StaticBinding::from_http_cors(Cors::default());

                    Ok(RouteRequest {
                        path: path_pattern.clone(),
                        method,
                        binding: GatewayBinding::Static(binding),
                        security,
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

    pub(crate) fn get_gateway_binding(
        gateway_binding_value: &Value,
    ) -> Result<WorkerBinding, String> {
        let http_middlewares = get_middleware(gateway_binding_value)?;
        let middlewares = http_middlewares
            .into_iter()
            .map(Middleware::http)
            .collect::<Vec<_>>();

        let binding_middleware = if middlewares.is_empty() {
            None
        } else {
            Some(Middlewares(middlewares))
        };

        let binding = WorkerBinding {
            worker_name: get_worker_id_expr(gateway_binding_value)?,
            component_id: get_component_id(gateway_binding_value)?,
            idempotency_key: get_idempotency_key(gateway_binding_value)?,
            response_mapping: get_response_mapping(gateway_binding_value)?,
            middleware: binding_middleware,
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

                    let cors_preflight = Cors::from_cors_preflight_expr(&CorsPreflightExpr(rib))?;

                    Ok(StaticBinding::from_http_cors(cors_preflight))
                }

                None => Ok(StaticBinding::from_http_cors(Cors::default())),
            },
            _ => Err("Invalid schema for cors binding".to_string()),
        }
    }

    pub(crate) fn get_component_id(
        gateway_binding_value: &Value,
    ) -> Result<VersionedComponentId, String> {
        let component_id_str = gateway_binding_value
            .get("component-id")
            .ok_or("No component-id found")?
            .as_str()
            .ok_or("component-id is not a string")?;

        let version = gateway_binding_value
            .get("component-version")
            .ok_or("No component-version found")?
            .as_u64()
            .ok_or("component-version is not a u64")?;

        let component_id =
            ComponentId(Uuid::parse_str(component_id_str).map_err(|err| err.to_string())?);

        Ok(VersionedComponentId {
            component_id,
            version,
        })
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

    pub(crate) fn get_middleware(
        worker_gateway_info: &Value,
    ) -> Result<Vec<HttpMiddleware>, String> {
        let mut middlewares = vec![];
        if let Some(middleware_value) = worker_gateway_info.get("middlewares") {
            // Users hardly need to specify the auth middleware in an open API spec as this is not a standard
            // pattern. Even cors is not needed in the standard pattern as it gets auto injected if there
            // is a preflight endpoint that corresponds to a static binding of cors.
            match middleware_value {
                Value::Object(map) => {
                    let cors_preflight: Option<Cors> = map
                        .get("cors")
                        .map(|json_value| serde_json::from_value(json_value.clone()))
                        .transpose()
                        .map_err(|err| format!("Invalid schema for Cors {}", err))?;

                    if let Some(cors_preflight) = cors_preflight {
                        middlewares.push(HttpMiddleware::cors(cors_preflight));
                    }
                }
                _ => return Err(
                    "Invalid response mapping type. It should be a string representing expression"
                        .to_string(),
                ),
            }
        }

        Ok(middlewares)
    }

    pub(crate) fn get_response_mapping(
        gateway_binding_value: &Value,
    ) -> Result<ResponseMapping, String> {
        let response = {
            let response_mapping_optional = gateway_binding_value.get("response").ok_or(
                "No response mapping found. It should be a string representing expression"
                    .to_string(),
            )?;

            match response_mapping_optional {
                Value::String(expr) => rib::from_string(expr).map_err(|err| err.to_string()),
                _ => Err(
                    "Invalid response mapping type. It should be a string representing expression"
                        .to_string(),
                ),
            }
        }?;

        Ok(ResponseMapping(response.clone()))
    }

    pub(crate) fn get_worker_id_expr(
        gateway_binding_value: &Value,
    ) -> Result<Option<Expr>, String> {
        let worker_id_str_opt = gateway_binding_value
            .get("worker-name")
            .map(|json_value| json_value.as_str().ok_or("worker-name is not a string"))
            .transpose()?;

        let worker_id_expr_opt = worker_id_str_opt
            .map(|worker_id| rib::from_string(worker_id).map_err(|err| err.to_string()))
            .transpose()?;

        Ok(worker_id_expr_opt)
    }

    pub(crate) fn get_idempotency_key(
        gateway_binding_value: &Value,
    ) -> Result<Option<Expr>, String> {
        if let Some(key) = gateway_binding_value.get("idempotency-key") {
            let key_expr = key.as_str().ok_or("idempotency-key is not a string")?;
            Ok(Some(
                rib::from_string(key_expr).map_err(|err| err.to_string())?,
            ))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn get_path_pattern(path: &str) -> Result<AllPathPatterns, String> {
        AllPathPatterns::parse(path)
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::gateway_api_definition::http::{
        AllPathPatterns, MethodPattern, Route, RouteRequest,
    };
    use crate::gateway_binding::{GatewayBinding, ResponseMapping, StaticBinding, WorkerBinding};
    use crate::gateway_middleware::{Cors, HttpMiddleware, Middleware, Middlewares};
    use golem_common::model::ComponentId;
    use openapiv3::Operation;
    use rib::Expr;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn test_get_route_from_path_with_worker_binding_with_middleware() {
        let path_item = Operation {
            extensions: vec![("x-golem-api-gateway-binding".to_string(), json!({
                "worker-name": "let x: str = request.body.user; \"worker-${x}\"",
                "component-id": "00000000-0000-0000-0000-000000000000",
                "component-version": 0,
                "idempotency-key": "\"test-key\"",
                "response": "${{headers : {ContentType: \"json\", user-id: \"foo\"}, body: worker.response, status: 200}}",
                "middlewares": {
                    "cors" : {
                        "allowHeaders": "Content-Type, Authorization",
                        "allowMethods": "GET, POST, PUT, DELETE, OPTIONS",
                        "allowOrigin": "*",
                        "allowCredentials": true
                    }
                }
            }))]
                .into_iter()
                .collect(),
            ..Default::default()
        };

        let path_pattern = AllPathPatterns::parse("/test").unwrap();

        let result = get_route_from_path_item("get", &path_item, &path_pattern);

        let expected = expected_route_with_middleware(&path_pattern);
        assert_eq!(result, Ok(expected));
    }

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
            binding: GatewayBinding::Static(StaticBinding::from_http_cors(Cors::default())),
            security: None,
        }
    }

    fn expected_route_with_cors_preflight_binding(path_pattern: &AllPathPatterns) -> RouteRequest {
        let mut cors_preflight = Cors::default();
        cors_preflight.set_allow_origin("apple.com").unwrap();
        RouteRequest {
            path: path_pattern.clone(),
            method: MethodPattern::Options,
            binding: GatewayBinding::Static(StaticBinding::from_http_cors(cors_preflight)),
            security: None,
        }
    }

    fn expected_route_with_middleware(path_pattern: &AllPathPatterns) -> RouteRequest {
        RouteRequest {
            path: path_pattern.clone(),
            method: MethodPattern::Get,
            security: None,
            binding: GatewayBinding::Default(WorkerBinding {
                worker_name: Some(Expr::expr_block(vec![
                    Expr::let_binding_with_type(
                        "x",
                        rib::TypeName::Str,
                        Expr::select_field(
                            Expr::select_field(Expr::identifier("request"), "body"),
                            "user",
                        ),
                    ),
                    Expr::concat(vec![Expr::literal("worker-"), Expr::identifier("x")]),
                ])),
                component_id: golem_service_base::model::VersionedComponentId {
                    component_id: ComponentId(Uuid::nil()),
                    version: 0,
                },
                idempotency_key: Some(Expr::literal("test-key")),
                response_mapping: ResponseMapping(Expr::record(
                    vec![
                        (
                            "headers".to_string(),
                            Expr::record(vec![
                                ("ContentType".to_string(), Expr::literal("json")),
                                ("user-id".to_string(), Expr::literal("foo")),
                            ]),
                        ),
                        (
                            "body".to_string(),
                            Expr::select_field(Expr::identifier("worker"), "response"),
                        ),
                        ("status".to_string(), Expr::untyped_number(200f64)),
                    ]
                    .into_iter()
                    .collect(),
                )),
                middleware: Some(Middlewares(vec![Middleware::Http(HttpMiddleware::cors(
                    Cors::from_parameters(
                        Some("*".to_string()),
                        Some("GET, POST, PUT, DELETE, OPTIONS".to_string()),
                        Some("Content-Type, Authorization".to_string()),
                        None,
                        Some(true),
                        None,
                    )
                    .unwrap(),
                ))])),
            }),
        }
    }
}
