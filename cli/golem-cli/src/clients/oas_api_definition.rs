use golem_client::model::HttpApiDefinition;
use openapiv3::OpenAPI;
use serde_json;

use internal::*;

use crate::model::{ApiDefinitionId, ApiDefinitionVersion};

pub fn get_api_definition_from_oas(open_api: &str) -> Result<HttpApiDefinition, String> {
    let openapi: OpenAPI = serde_json::from_str(open_api).map_err(|e| e.to_string())?;

    let api_definition_id = ApiDefinitionId(get_root_extension(
        &openapi,
        GOLEM_API_DEFINITION_ID_EXTENSION,
    )?);

    let api_definition_version =
        ApiDefinitionVersion(get_root_extension(&openapi, GOLEM_API_DEFINITION_VERSION)?);

    let routes = get_routes(openapi.paths)?;

    Ok(HttpApiDefinition {
        id: api_definition_id.0,
        version: api_definition_version.0,
        routes,
    })
}

mod internal {
    use golem_client::model::{GolemWorkerBinding, MethodPattern, Route};
    use openapiv3::{OpenAPI, PathItem, Paths, ReferenceOr};
    use serde_json::Value;

    use uuid::Uuid;

    use crate::model::TemplateId;

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
                    for (str, _) in item.iter() {
                        let route = get_route_from_path_item(str, item, path)?;
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
        path: &str,
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
            worker_id: get_worker_id(worker_bridge_info)?,
            function_name: get_function_name(worker_bridge_info)?,
            function_params: get_function_params(worker_bridge_info)?,
            template: get_template_id(worker_bridge_info)?.0,
            response: get_response(worker_bridge_info),
        };

        Ok(Route {
            path: path.to_string(),
            method,
            binding,
        })
    }

    pub(crate) fn get_template_id(worker_bridge_info: &Value) -> Result<TemplateId, String> {
        let template_id = worker_bridge_info
            .get("template-id")
            .ok_or("No template-id found")?
            .as_str()
            .ok_or("template-id is not a string")?;
        Ok(TemplateId(
            Uuid::parse_str(template_id).map_err(|err| err.to_string())?,
        ))
    }

    pub(crate) fn get_response(worker_bridge_info: &Value) -> Option<Value> {
        worker_bridge_info.get("response").cloned()
    }

    pub(crate) fn get_function_params(worker_bridge_info: &Value) -> Result<Vec<Value>, String> {
        let function_params = worker_bridge_info
            .get("function-params")
            .ok_or("No function-params found")?
            .as_array()
            .ok_or("function-params is not an array")?;
        Ok(function_params.clone())
    }

    pub(crate) fn get_function_name(worker_bridge_info: &Value) -> Result<String, String> {
        let function_name = worker_bridge_info
            .get("function-name")
            .ok_or("No function-name found")?
            .as_str()
            .ok_or("function-name is not a string")?;
        Ok(function_name.to_string())
    }

    pub(crate) fn get_worker_id(worker_bridge_info: &Value) -> Result<Value, String> {
        let worker_id = worker_bridge_info
            .get("worker-id")
            .ok_or("No worker-id found")?;
        Ok(worker_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use golem_client::model::{GolemWorkerBinding, Route};
    use uuid::Uuid;

    use super::get_api_definition_from_oas;

    #[test]
    fn parse_openapi() {
        let str = r#"
        {
            "openapi": "3.0.0",
            "x-golem-api-definition-id": "shopping",
            "x-golem-api-definition-version": "0.0.1",
            "info": {
                "title": "Sample API",
                "version": "1.0.2"
            },
            "servers": [],
            "paths": {
                "/{user-id}/get-cart-contents": {
                "x-golem-worker-bridge": {
                    "worker-id": "worker-${request.path.user-id}",
                    "function-name": "golem:it/api/get-cart-contents",
                    "function-params": [],
                    "template-id": "9fff66ee-13d9-4bd3-902b-8786aa863007",
                    "response" : {
                    "status": "201",
                    "body": {
                        "name" : "${worker.response[0][0].name}",
                        "price" : "${worker.response[0][0].price}",
                        "quantity" : "${worker.response[0][0].quantity}"
                    },
                    "headers": {}
                    }
                },
                "get": {
                    "summary": "Get Cart Contents",
                    "description": "Get the contents of a user's cart",
                    "parameters": [
                    {
                        "name": "user-id",
                        "in": "path",
                        "required": true,
                        "schema": {
                        "type": "string"
                        }
                    }
                    ],
                    "responses": {
                    "200": {
                        "description": "OK",
                        "content":{
                        "application/json": {
                            "schema": {
                            "$ref": "/components/schemas/CartItem"
                            }
                        }
                        }
                    },
                    "404": {
                        "description": "Contents not found"
                    }
                    }
                }
                }
            }
        }"#;

        match get_api_definition_from_oas(str) {
            Err(e) => panic!("{}", format!("failed: {e}")),
            Ok(result) => {
                let expected_body = serde_json::Value::Object(serde_json::Map::from_iter(vec![
                    (
                        "name".to_string(),
                        serde_json::Value::String("${worker.response[0][0].name}".to_string()),
                    ),
                    (
                        "price".to_string(),
                        serde_json::Value::String("${worker.response[0][0].price}".to_string()),
                    ),
                    (
                        "quantity".to_string(),
                        serde_json::Value::String("${worker.response[0][0].quantity}".to_string()),
                    ),
                ]));
                let expected_response =
                    serde_json::Value::Object(serde_json::Map::from_iter(vec![
                        ("body".to_string(), expected_body),
                        (
                            "headers".to_string(),
                            serde_json::Value::Object(serde_json::Map::from_iter(vec![])),
                        ),
                        (
                            "status".to_string(),
                            serde_json::Value::String("201".to_string()),
                        ),
                    ]));
                assert_eq!(result.id, "shopping");
                assert_eq!(result.version, "0.0.1");
                assert_eq!(
                    result.routes,
                    vec![Route {
                        method: golem_client::model::MethodPattern::Get,
                        path: "/{user-id}/get-cart-contents".to_string(),
                        binding: GolemWorkerBinding {
                            template: Uuid::parse_str("9fff66ee-13d9-4bd3-902b-8786aa863007")
                                .unwrap(),
                            worker_id: serde_json::Value::String(
                                "worker-${request.path.user-id}".to_string()
                            ),
                            function_name: "golem:it/api/get-cart-contents".to_string(),
                            function_params: vec![],
                            response: Some(expected_response)
                        }
                    }]
                );
            }
        }
    }
}
