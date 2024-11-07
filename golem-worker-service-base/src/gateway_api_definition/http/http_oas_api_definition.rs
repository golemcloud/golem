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
    pub fn to_http_api_definition(&self) -> Result<HttpApiDefinitionRequest, String> {
        let open_api = &self.0;
        let api_definition_id = ApiDefinitionId(get_root_extension(
            open_api,
            GOLEM_API_DEFINITION_ID_EXTENSION,
        )?);

        let api_definition_version =
            ApiVersion(get_root_extension(open_api, GOLEM_API_DEFINITION_VERSION)?);

        let routes = get_routes(&open_api.paths)?;

        Ok(HttpApiDefinitionRequest {
            id: api_definition_id,
            version: api_definition_version,
            routes,
            draft: true,
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
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
    use golem_common::model::ComponentId;
    use openapiv3::{OpenAPI, Operation, PathItem, Paths, ReferenceOr};
    use rib::Expr;
    use serde_json::Value;

    use golem_service_base::model::VersionedComponentId;
    use uuid::Uuid;
    use crate::gateway_binding::{ResponseMapping, WorkerBinding};

    pub(crate) const GOLEM_API_DEFINITION_ID_EXTENSION: &str = "x-golem-api-definition-id";
    pub(crate) const GOLEM_API_DEFINITION_VERSION: &str = "x-golem-api-definition-version";
    pub(crate) const GOLEM_WORKER_BRIDGE_EXTENSION: &str = "x-golem-worker-bridge";

    pub(crate) fn get_root_extension(open_api: &OpenAPI, key_name: &str) -> Result<String, String> {
        open_api
            .extensions
            .iter()
            .find(|(key, _)| key.to_lowercase() == key_name)
            .map(|(_, value)| value)
            .ok_or(format!("{} not found in the open API spec", key_name))?
            .as_str()
            .ok_or(format!("Invalid value for {}", key_name))
            .map(|x| x.to_string())
    }

    pub(crate) fn get_routes(paths: &Paths) -> Result<Vec<Route>, String> {
        let mut routes: Vec<Route> = vec![];

        for (path, path_item) in paths.iter() {
            match path_item {
                ReferenceOr::Item(item) => {
                    let path_pattern = get_path_pattern(path)?;

                    for (method, method_operation) in item.iter() {
                        let route = get_route_from_path_item(method, method_operation, &path_pattern)?;
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
    ) -> Result<Route, String> {
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

        let worker_bridge_info = method_operation
            .extensions
            .get(GOLEM_WORKER_BRIDGE_EXTENSION)
            .ok_or(format!(
                "No {} extension found",
                GOLEM_WORKER_BRIDGE_EXTENSION
            ))?;

        let binding = WorkerBinding {
            worker_name: get_worker_id_expr(worker_bridge_info)?,
            component_id: get_component_id(worker_bridge_info)?,
            idempotency_key: get_idempotency_key(worker_bridge_info)?,
            response: get_response_mapping(worker_bridge_info)?,
        };

        Ok(Route {
            path: path_pattern.clone(),
            method,
            binding,
        })
    }

    pub(crate) fn get_component_id(
        worker_bridge_info: &Value,
    ) -> Result<VersionedComponentId, String> {
        let component_id_str = worker_bridge_info
            .get("component-id")
            .ok_or("No component-id found")?
            .as_str()
            .ok_or("component-id is not a string")?;

        let version = worker_bridge_info
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

    pub(crate) fn get_response_mapping(
        worker_bridge_info: &Value,
    ) -> Result<ResponseMapping, String> {
        let response = {
            let response_mapping_optional = worker_bridge_info.get("response").ok_or(
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

    pub(crate) fn get_worker_id_expr(worker_bridge_info: &Value) -> Result<Option<Expr>, String> {
        let worker_id_str_opt = worker_bridge_info
            .get("worker-name")
            .map(|json_value| json_value.as_str().ok_or("worker-name is not a string"))
            .transpose()?;

        let worker_id_expr_opt = worker_id_str_opt
            .map(|worker_id| rib::from_string(worker_id).map_err(|err| err.to_string()))
            .transpose()?;

        Ok(worker_id_expr_opt)
    }

    pub(crate) fn get_idempotency_key(worker_bridge_info: &Value) -> Result<Option<Expr>, String> {
        if let Some(key) = worker_bridge_info.get("idempotency-key") {
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
    use crate::gateway_api_definition::http::{AllPathPatterns, MethodPattern, Route};
    use crate::gateway_binding::{ResponseMapping, WorkerBinding};
    use golem_common::model::ComponentId;
    use openapiv3::{Operation};
    use rib::Expr;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn test_get_route_from_path_item() {
        let path_item = Operation {
            extensions: vec![("x-golem-worker-bridge".to_string(), json!({
                "worker-name": "let x: str = request.body.user; \"worker-${x}\"",
                "component-id": "00000000-0000-0000-0000-000000000000",
                "component-version": 0,
                "idempotency-key": "\"test-key\"",
                "response": "${{headers : {ContentType: \"json\", user-id: \"foo\"}, body: worker.response, status: 200}}"
            }))]
                .into_iter()
                .collect(),
            ..Default::default()
        };

        let path_pattern = AllPathPatterns::parse("/test").unwrap();

        let result = get_route_from_path_item("get", &path_item, &path_pattern);
        assert_eq!(
            result,
            Ok(Route {
                path: path_pattern,
                method: MethodPattern::Get,
                binding: WorkerBinding {
                    worker_name: Some(Expr::expr_block(vec![
                        Expr::let_binding_with_type(
                            "x",
                            rib::TypeName::Str,
                            Expr::select_field(
                                Expr::select_field(Expr::identifier("request"), "body"),
                                "user"
                            )
                        ),
                        Expr::concat(vec![Expr::literal("worker-"), Expr::identifier("x"),])
                    ])),
                    component_id: golem_service_base::model::VersionedComponentId {
                        component_id: ComponentId(Uuid::nil()),
                        version: 0
                    },
                    idempotency_key: Some(Expr::literal("test-key")),
                    response: ResponseMapping(Expr::record(
                        vec![
                            (
                                "headers".to_string(),
                                Expr::record(vec![
                                    ("ContentType".to_string(), Expr::literal("json"),),
                                    ("user-id".to_string(), Expr::literal("foo"),)
                                ]),
                            ),
                            (
                                "body".to_string(),
                                Expr::select_field(Expr::identifier("worker"), "response",),
                            ),
                            ("status".to_string(), Expr::untyped_number(200f64)),
                        ]
                        .into_iter()
                        .collect()
                    ))
                }
            })
        );
    }
}
