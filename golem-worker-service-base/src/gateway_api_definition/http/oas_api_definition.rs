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

use crate::gateway_api_definition::http::HttpApiDefinitionRequest;
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::service::gateway::BoxConversionContext;
use internal::*;
use openapiv3::OpenAPI;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ParseFromYAML, ParseResult};
use serde_json::Value;
use std::borrow::Cow;

pub struct OpenApiHttpApiDefinition(pub OpenAPI);

impl OpenApiHttpApiDefinition {
    // Convert an open api spec with golem extensions into a golem http api definition.
    // This may make api calls to resolve component names used in the definition
    pub async fn to_http_api_definition_request(
        &self,
        conversion_context: &BoxConversionContext<'_>,
    ) -> Result<HttpApiDefinitionRequest, String> {
        let open_api = &self.0;

        let api_definition_id = ApiDefinitionId(get_root_extension_str(
            open_api,
            GOLEM_API_DEFINITION_ID_EXTENSION,
        )?);

        let api_definition_version = ApiVersion(get_root_extension_str(
            open_api,
            GOLEM_API_DEFINITION_VERSION,
        )?);

        let routes = get_routes(&open_api.paths, conversion_context).await?;

        Ok(HttpApiDefinitionRequest {
            id: api_definition_id,
            version: api_definition_version,
            routes,
            draft: true,
        })
    }
}

impl ParseFromJSON for OpenApiHttpApiDefinition {
    fn parse_from_json(value: Option<serde_json::Value>) -> ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(OpenApiHttpApiDefinition(openapi)),
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

impl ParseFromYAML for OpenApiHttpApiDefinition {
    fn parse_from_yaml(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(OpenApiHttpApiDefinition(openapi)),
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

impl poem_openapi::types::Type for OpenApiHttpApiDefinition {
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

    use crate::gateway_binding::{
        GatewayBinding, HttpHandlerBinding, ResponseMapping, StaticBinding, WorkerBinding,
    };
    use crate::gateway_middleware::{CorsPreflightExpr, HttpCors};
    use crate::gateway_security::{SecuritySchemeIdentifier, SecuritySchemeReference};
    use crate::service::gateway::BoxConversionContext;
    use golem_common::model::component::VersionedComponentId;
    use golem_common::model::GatewayBindingType;
    use golem_service_base::model::ComponentName;
    use openapiv3::{OpenAPI, Operation, Paths, ReferenceOr};
    use rib::Expr;
    use serde_json::Value;

    pub(super) const GOLEM_API_DEFINITION_ID_EXTENSION: &str = "x-golem-api-definition-id";
    pub(super) const GOLEM_API_DEFINITION_VERSION: &str = "x-golem-api-definition-version";

    // Legacy extension for worker bridge
    pub(super) const GOLEM_WORKER_GATEWAY_EXTENSION_LEGACY: &str = "x-golem-worker-bridge";

    pub(super) const GOLEM_API_GATEWAY_BINDING: &str = "x-golem-api-gateway-binding";

    pub(super) fn get_root_extension_str(
        open_api: &OpenAPI,
        key_name: &str,
    ) -> Result<String, String> {
        get_root_extension_value(open_api, key_name)
            .ok_or(format!("{} not found in the open API spec", key_name))?
            .as_str()
            .ok_or(format!("Invalid value for {}", key_name))
            .map(|x| x.to_string())
    }

    pub(super) fn get_root_extension_value(open_api: &OpenAPI, key_name: &str) -> Option<Value> {
        open_api
            .extensions
            .iter()
            .find(|(key, __)| key.to_lowercase() == key_name)
            .map(|(_, v)| v.clone())
    }

    pub(super) async fn get_routes(
        paths: &Paths,
        ctx: &BoxConversionContext<'_>,
    ) -> Result<Vec<RouteRequest>, String> {
        let mut routes: Vec<RouteRequest> = vec![];

        for (path, path_item) in paths.iter() {
            match path_item {
                ReferenceOr::Item(item) => {
                    let path_pattern = get_path_pattern(path)?;

                    for (method, method_operation) in item.iter() {
                        let route =
                            get_route_from_path_item(method, method_operation, &path_pattern, ctx)
                                .await?;
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

    pub(super) async fn get_route_from_path_item(
        method: &str,
        method_operation: &Operation,
        path_pattern: &AllPathPatterns,
        ctx: &BoxConversionContext<'_>,
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
                        })
                    }

                    (GatewayBindingType::Default, _) => {
                        let binding = get_worker_binding(worker_gateway_info, ctx).await?;

                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::Default(binding),
                            security,
                        })
                    }
                    (GatewayBindingType::FileServer, _) => {
                        let binding = get_worker_binding(worker_gateway_info, ctx).await?;

                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::Default(binding),
                            security,
                        })
                    }
                    (GatewayBindingType::HttpHandler, _) => {
                        let binding = get_http_handler_binding(worker_gateway_info, ctx).await?;

                        Ok(RouteRequest {
                            path: path_pattern.clone(),
                            method,
                            binding: GatewayBinding::HttpHandler(binding),
                            security,
                        })
                    }
                    (GatewayBindingType::CorsPreflight, method) => {
                        Err(format!("cors-preflight binding type is supported only for 'options' method, but found method '{}'", method))
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

    pub(super) async fn get_worker_binding(
        gateway_binding_value: &Value,
        ctx: &BoxConversionContext<'_>,
    ) -> Result<WorkerBinding, String> {
        let component_name = get_component_name(gateway_binding_value)?;
        let component_version = get_component_version(gateway_binding_value)?;

        let component_view = ctx.component_by_name(&component_name).await?;

        let binding = WorkerBinding {
            worker_name: get_worker_id_expr(gateway_binding_value)?,
            component_id: VersionedComponentId {
                component_id: component_view.id,
                version: component_version.unwrap_or(component_view.latest_version),
            },
            idempotency_key: get_idempotency_key(gateway_binding_value)?,
            response_mapping: get_response_mapping(gateway_binding_value)?,
            invocation_context: get_invocation_context(gateway_binding_value)?,
        };

        Ok(binding)
    }

    pub(super) async fn get_http_handler_binding(
        gateway_binding_value: &Value,
        ctx: &BoxConversionContext<'_>,
    ) -> Result<HttpHandlerBinding, String> {
        let component_name = get_component_name(gateway_binding_value)?;
        let component_version = get_component_version(gateway_binding_value)?;

        let component_view = ctx.component_by_name(&component_name).await?;

        let binding = HttpHandlerBinding {
            worker_name: get_worker_id_expr(gateway_binding_value)?,
            component_id: VersionedComponentId {
                component_id: component_view.id,
                version: component_version.unwrap_or(component_view.latest_version),
            },
            idempotency_key: get_idempotency_key(gateway_binding_value)?,
        };

        Ok(binding)
    }

    pub(super) fn get_cors_static_binding(
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

    pub(super) fn get_component_name(
        gateway_binding_value: &Value,
    ) -> Result<ComponentName, String> {
        let result = gateway_binding_value
            .get("component-name")
            .ok_or("No component-name found")?
            .as_str()
            .ok_or("component-name is not a string")?;

        Ok(ComponentName(result.to_string()))
    }

    pub(super) fn get_component_version(
        gateway_binding_value: &Value,
    ) -> Result<Option<u64>, String> {
        if let Some(component_version) = gateway_binding_value.get("component-version") {
            let converted = component_version
                .as_u64()
                .ok_or("component-version is not a u64")?;
            Ok(Some(converted))
        } else {
            Ok(None)
        }
    }

    pub(super) fn get_binding_type(
        worker_gateway_info: &Value,
    ) -> Result<GatewayBindingType, String> {
        let binding_type_optional: Option<GatewayBindingType> = worker_gateway_info
            .get("binding-type")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|err| format!("Invalid schema for binding-type. {}", err))?;

        Ok(binding_type_optional.unwrap_or(GatewayBindingType::Default))
    }

    pub(super) fn get_response_mapping(
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

    pub(super) fn get_worker_id_expr(
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

    pub(super) fn get_idempotency_key(
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

    pub(super) fn get_invocation_context(
        gateway_binding_value: &Value,
    ) -> Result<Option<Expr>, String> {
        if let Some(key) = gateway_binding_value.get("invocation-context") {
            let key_expr = key.as_str().ok_or("invocation-context is not a string")?;
            Ok(Some(
                rib::from_string(key_expr).map_err(|err| err.to_string())?,
            ))
        } else {
            Ok(None)
        }
    }

    pub(super) fn get_path_pattern(path: &str) -> Result<AllPathPatterns, String> {
        AllPathPatterns::parse(path)
    }
}

#[cfg(test)]
mod tests {
    use golem_common::model::ComponentId;
    use golem_service_base::model::ComponentName;
    use test_r::test;

    use super::*;
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, RouteRequest};
    use crate::gateway_binding::{GatewayBinding, StaticBinding};
    use crate::gateway_middleware::HttpCors;
    use crate::service::gateway::{ComponentView, ConversionContext};
    use async_trait::async_trait;
    use golem_common::model::component::VersionedComponentId;
    use openapiv3::Operation;
    use serde_json::json;
    use uuid::uuid;

    struct TestConversionCtx;

    #[async_trait]
    impl ConversionContext for TestConversionCtx {
        async fn component_by_name(&self, name: &ComponentName) -> Result<ComponentView, String> {
            if name.0 == "foobar" {
                Ok(ComponentView {
                    name: ComponentName("foobar".to_string()),
                    id: ComponentId(uuid!("15d70aa5-2e23-4ee3-b65c-4e1d702836a3")),
                    latest_version: 1,
                })
            } else {
                Err("unknown component name".to_string())
            }
        }
        async fn component_by_id(
            &self,
            _component_id: &ComponentId,
        ) -> Result<ComponentView, String> {
            unimplemented!()
        }
    }

    #[test]
    async fn test_get_route_with_cors_preflight_binding() {
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

        let result = get_route_from_path_item(
            "options",
            &path_item,
            &path_pattern,
            &TestConversionCtx.boxed(),
        )
        .await;

        let expected = expected_route_with_cors_preflight_binding(&path_pattern);
        assert_eq!(result, Ok(expected));
    }

    #[test]
    async fn test_get_route_with_cors_preflight_binding_default_response() {
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

        let result = get_route_from_path_item(
            "options",
            &path_item,
            &path_pattern,
            &TestConversionCtx.boxed(),
        )
        .await;

        let expected = expected_route_with_cors_preflight_binding_default(&path_pattern);
        assert_eq!(result, Ok(expected));
    }

    #[test]
    async fn test_get_route_with_no_binding_with_options_method() {
        let path_item = Operation::default();

        let path_pattern = AllPathPatterns::parse("/test").unwrap();

        let result = get_route_from_path_item(
            "options",
            &path_item,
            &path_pattern,
            &TestConversionCtx.boxed(),
        )
        .await;

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
        }
    }

    #[test]
    async fn test_optional_component_version() {
        let path_item = Operation {
            extensions: vec![(
                "x-golem-api-gateway-binding".to_string(),
                json!({
                    "binding-type": "default",
                    "response" : "{}",
                    "workerName": "{}",
                    "component-name": "foobar"
                }),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let path_pattern = AllPathPatterns::parse("/test").unwrap();

        let result =
            get_route_from_path_item("get", &path_item, &path_pattern, &TestConversionCtx.boxed())
                .await
                .unwrap();

        let component_id = result.binding.get_component_id().unwrap();
        assert_eq!(
            component_id,
            VersionedComponentId {
                component_id: ComponentId(uuid!("15d70aa5-2e23-4ee3-b65c-4e1d702836a3")),
                version: 1
            }
        )
    }
}
