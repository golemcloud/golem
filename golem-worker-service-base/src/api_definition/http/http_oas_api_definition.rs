use async_trait::async_trait;
use openapiv3::OpenAPI;
use poem_openapi::types::ParseFromJSON;
use poem_openapi::{registry, types};

use crate::api_definition::http::HttpApiDefinition;
use crate::api_definition::{ApiDefinitionId, ApiVersion};
use internal::*;

pub fn get_api_definition(openapi: OpenAPI) -> Result<HttpApiDefinition, String> {
    let api_definition_id = ApiDefinitionId(get_root_extension(
        &openapi,
        GOLEM_API_DEFINITION_ID_EXTENSION,
    )?);

    let api_definition_version =
        ApiVersion(get_root_extension(&openapi, GOLEM_API_DEFINITION_VERSION)?);

    let routes = get_routes(openapi.paths)?;

    Ok(HttpApiDefinition {
        id: api_definition_id,
        version: api_definition_version,
        routes,
        draft: true,
    })
}

// Used to extract the OpenAPI spec from JSON Body in Poem OpenAPI endpoints.
pub struct JsonOpenApiDefinition(pub openapiv3::OpenAPI);

impl types::Type for JsonOpenApiDefinition {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        "OpenApiDefinition".into()
    }

    fn schema_ref() -> registry::MetaSchemaRef {
        registry::MetaSchemaRef::Inline(Box::new(registry::MetaSchema::ANY))
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

#[async_trait]
impl ParseFromJSON for JsonOpenApiDefinition {
    fn parse_from_json(value: Option<serde_json::Value>) -> types::ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(JsonOpenApiDefinition(openapi)),
                Err(e) => Err(types::ParseError::<Self>::custom(format!(
                    "Failed to parse OpenAPI: {}",
                    e
                ))),
            },

            _ => Err(poem_openapi::types::ParseError::<Self>::custom(
                "OpenAPI spec missing".to_string(),
            )),
        }
    }
}

mod internal {
    use crate::api_definition::http::{AllPathPatterns, HttpResponseMapping, MethodPattern, Route};
    use crate::expression::Expr;
    use crate::worker_binding::{GolemWorkerBinding, ResponseMapping};
    use golem_common::model::ComponentId;
    use openapiv3::{OpenAPI, PathItem, Paths, ReferenceOr};
    use serde_json::Value;

    use crate::expression;
    use uuid::Uuid;

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

    pub(crate) fn get_routes(paths: Paths) -> Result<Vec<Route>, String> {
        let mut routes: Vec<Route> = vec![];

        for (path, path_item) in paths.iter() {
            match path_item {
                ReferenceOr::Item(item) => {
                    let path_pattern = get_path_pattern(path)?;

                    for (str, _) in item.iter() {
                        let route = get_route_from_path_item(str, item, &path_pattern)?;
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
        path_item: &PathItem,
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

        let worker_bridge_info = path_item
            .extensions
            .get(GOLEM_WORKER_BRIDGE_EXTENSION)
            .ok_or(format!(
                "No {} extension found",
                GOLEM_WORKER_BRIDGE_EXTENSION
            ))?;

        let binding = GolemWorkerBinding {
            worker_id: get_worker_id_expr(worker_bridge_info)?,
            function_name: get_function_name(worker_bridge_info)?,
            function_params: get_function_params_expr(worker_bridge_info)?,
            component: get_component_id(worker_bridge_info)?,
            response: get_response_mapping(worker_bridge_info)?,
        };

        Ok(Route {
            path: path_pattern.clone(),
            method,
            binding,
        })
    }

    pub(crate) fn get_component_id(worker_bridge_info: &Value) -> Result<ComponentId, String> {
        let component_id = worker_bridge_info
            .get("component-id")
            .ok_or("No component-id found")?
            .as_str()
            .ok_or("component-id is not a string")?;
        Ok(ComponentId(
            Uuid::parse_str(component_id).map_err(|err| err.to_string())?,
        ))
    }

    pub(crate) fn get_response_mapping(
        worker_bridge_info: &Value,
    ) -> Result<Option<ResponseMapping>, String> {
        let response = {
            let response_mapping_optional = worker_bridge_info.get("response");

            match response_mapping_optional {
                None => Ok(None),
                Some(Value::Null) => Ok(None),
                Some(Value::String(expr)) => expression::from_string(expr)
                    .map_err(|err| err.to_string())
                    .map(Some),
                _ => Err(
                    "Invalid response mapping type. It should be a string representing expression"
                        .to_string(),
                ),
            }
        };

        match response? {
            Some(response_mapping_expr) => {
                // Validating
                let _ =
                    HttpResponseMapping::try_from(&ResponseMapping(response_mapping_expr.clone()))
                        .map_err(|err| err.to_string())?;

                Ok(Some(ResponseMapping(response_mapping_expr)))
            }
            None => Ok(None),
        }
    }

    pub(crate) fn get_function_params_expr(
        worker_bridge_info: &Value,
    ) -> Result<Vec<Expr>, String> {
        let function_params = worker_bridge_info
            .get("function-params")
            .ok_or("No function-params found")?
            .as_array()
            .ok_or("function-params is not an array")?;
        let mut exprs = vec![];
        for param in function_params {
            match param {
                Value::String(function_param_expr_str) => {
                    let function_param_expr = expression::from_string(function_param_expr_str)
                        .map_err(|err| err.to_string())?;
                    exprs.push(function_param_expr);
                }
                _ => return Err(
                    "Invalid function param type. It should be a string representing expression"
                        .to_string(),
                ),
            }
        }
        Ok(exprs)
    }

    pub(crate) fn get_function_name(worker_bridge_info: &Value) -> Result<String, String> {
        let function_name = worker_bridge_info
            .get("function-name")
            .ok_or("No function-name found")?
            .as_str()
            .ok_or("function-name is not a string")?;
        Ok(function_name.to_string())
    }

    pub(crate) fn get_worker_id_expr(worker_bridge_info: &Value) -> Result<Expr, String> {
        let worker_id = worker_bridge_info
            .get("worker-id")
            .ok_or("No worker-id found")?
            .as_str()
            .ok_or("worker-id is not a string")?;

        expression::from_string(worker_id).map_err(|err| err.to_string())
    }

    pub(crate) fn get_path_pattern(path: &str) -> Result<AllPathPatterns, String> {
        AllPathPatterns::parse(path).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_definition::http::{AllPathPatterns, MethodPattern, Route};
    use crate::expression::{Expr, InnerNumber};
    use crate::worker_binding::{GolemWorkerBinding, ResponseMapping};
    use golem_common::model::ComponentId;
    use openapiv3::PathItem;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn test_get_route_from_path_item() {
        let path_item = PathItem {
            extensions: vec![("x-golem-worker-bridge".to_string(), json!({
                "worker-id": "worker-${request.body.user}",
                "function-name": "test",
                "function-params": ["${request}"],
                "component-id": "00000000-0000-0000-0000-000000000000",
                "response": "${{headers : {ContentType: 'json', user-id: 'foo'}, body: worker.response, status: 200}}"
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
                binding: GolemWorkerBinding {
                    worker_id: Expr::Concat(vec![
                        Expr::Literal("worker-".to_string()),
                        Expr::SelectField(
                            Box::new(Expr::SelectField(
                                Box::new(Expr::Request()),
                                "body".to_string()
                            )),
                            "user".to_string()
                        )
                    ]),
                    function_name: "test".to_string(),
                    function_params: vec![Expr::Request()],
                    component: ComponentId(Uuid::nil()),
                    response: Some(ResponseMapping(Expr::Record(
                        vec![
                            (
                                "headers".to_string(),
                                Box::new(Expr::Record(vec![
                                    (
                                        "ContentType".to_string(),
                                        Box::new(Expr::Literal("json".to_string())),
                                    ),
                                    (
                                        "user-id".to_string(),
                                        Box::new(Expr::Literal("foo".to_string())),
                                    )
                                ])),
                            ),
                            (
                                "body".to_string(),
                                Box::new(Expr::SelectField(
                                    Box::new(Expr::Worker()),
                                    "response".to_string(),
                                )),
                            ),
                            (
                                "status".to_string(),
                                Box::new(Expr::Number(InnerNumber::UnsignedInteger(200)))
                            ),
                        ]
                        .into_iter()
                        .collect()
                    )))
                }
            })
        );
    }
}
