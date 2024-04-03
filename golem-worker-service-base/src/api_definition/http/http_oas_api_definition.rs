use openapiv3::OpenAPI;
use serde_json;

use crate::api_definition::http::HttpApiDefinition;
use crate::api_definition::{ApiDefinitionId, ApiVersion};
use internal::*;

pub fn get_api_definition_from_oas(open_api: &str) -> Result<HttpApiDefinition, String> {
    let openapi: OpenAPI = serde_json::from_str(open_api).map_err(|e| e.to_string())?;

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
    })
}

mod internal {
    use crate::api_definition::http::{AllPathPatterns, HttpResponseMapping, MethodPattern, Route};
    use crate::expression::Expr;
    use crate::worker_binding::{GolemWorkerBinding, ResponseMapping};
    use golem_common::model::TemplateId;
    use openapiv3::{OpenAPI, PathItem, Paths, ReferenceOr};
    use serde_json::Value;

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
            template: get_template_id(worker_bridge_info)?,
            response: get_response_mapping(worker_bridge_info)?,
        };

        Ok(Route {
            path: path_pattern.clone(),
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

    pub(crate) fn get_response_mapping(
        worker_bridge_info: &Value,
    ) -> Result<Option<ResponseMapping>, String> {
        let response = worker_bridge_info.get("response");
        match response {
            Some(response) => {
                let response_mapping_expr =
                    Expr::from_json_value(response).map_err(|err| err.to_string())?;

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
            exprs.push(Expr::from_json_value(param).map_err(|err| err.to_string())?);
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
            .ok_or("No worker-id found")?;
        Expr::from_json_value(worker_id).map_err(|err| err.to_string())
    }

    pub(crate) fn get_path_pattern(path: &str) -> Result<AllPathPatterns, String> {
        AllPathPatterns::from(path).map_err(|err| err.to_string())
    }
}
