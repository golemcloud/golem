use crate::api_definition::{
    ApiDefinition, ApiDefinitionId, GolemWorkerBinding, MethodPattern, PathPattern,
    ResponseMapping, Route, Version,
};
use crate::expr::Expr;
use golem_common::model::TemplateId;
use openapiv3::{OpenAPI, PathItem, ReferenceOr};
use serde_json;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

pub fn get_api_definition(open_api: &str) -> Result<ApiDefinition, String> {
    let openapi: OpenAPI = serde_json::from_str(open_api).map_err(|e| e.to_string())?;
    let version = Version(openapi.info.version);
    let id = ApiDefinitionId(
        openapi
            .extensions
            .iter()
            .find(|(key, _)| key.to_lowercase() == "x-api-definition-id")
            .map(|(_, value)| value)
            .ok_or("No X-API-Definition-Id found")?
            .as_str()
            .ok_or("X-API-Definition-Id is not a string")?
            .to_string(),
    );

    let mut routes: Vec<Route> = vec![];

    for (path, path_item) in openapi.paths.iter() {
        match path_item {
            ReferenceOr::Item(item) => {
                let path_pattern = get_path_pattern(path)?;

                for (str, op) in item.iter() {
                    let route = get_route_from_path_item(str, item, &path_pattern)?;
                    routes.push(route);
                }
            }
            ReferenceOr::Reference { reference: _ } => Err(
                "Reference not supported when extracting worker bridge extension info".to_string(),
            ),
        };
    }

    Ok(ApiDefinition {
        id,
        version,
        routes,
    })
}

fn get_route_from_path_item(
    method: &str,
    path_item: &PathItem,
    path_pattern: &PathPattern,
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
        .get("x-worker-bridge")
        .ok_or("No x-worker-bridge extension found")?;

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

fn get_template_id(worker_bridge_info: &Value) -> Result<TemplateId, String> {
    let template_id = worker_bridge_info
        .get("template-id")
        .ok_or("No template-id found")?
        .as_str()
        .ok_or("template-id is not a string")?;
    Ok(TemplateId(
        Uuid::parse_str(template_id).map_err(|err| err.to_string())?,
    ))
}

fn get_response_mapping(worker_bridge_info: &Value) -> Result<Option<ResponseMapping>, String> {
    let response = worker_bridge_info.get("response");
    match response {
        Some(response) => Ok(Some(ResponseMapping {
            status: Expr::from_json_value(response.get("status").ok_or("No status found")?)
                .map_err(|err| err.to_string())?,
            headers: {
                let mut header_map = HashMap::new();

                let header_iter = response
                    .get("headers")
                    .ok_or("No headers found")?
                    .as_object()
                    .ok_or("headers is not an object")?
                    .iter();

                for (header_name, value) in header_iter {
                    let value_str = value.as_str().ok_or("Header value is not a string")?;
                    header_map.insert(
                        header_name.clone(),
                        Expr::from_primitive_string(value_str).map_err(|err| err.to_string())?,
                    );
                }

                header_map
            },
            body: Expr::from_json_value(response.get("body").ok_or("No body found")?)
                .map_err(|err| err.to_string())?,
        })),
        None => Ok(None),
    }
}

fn get_function_params_expr(worker_bridge_info: &Value) -> Result<Vec<Expr>, String> {
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

fn get_function_name(worker_bridge_info: &Value) -> Result<String, String> {
    let function_name = worker_bridge_info
        .get("function-name")
        .ok_or("No function-name found")?
        .as_str()
        .ok_or("function-name is not a string")?;
    Ok(function_name.to_string())
}

fn get_worker_id_expr(worker_bridge_info: &Value) -> Result<Expr, String> {
    let worker_id = worker_bridge_info
        .get("worker-id")
        .ok_or("No worker-id found")?;
    Expr::from_json_value(worker_id).map_err(|err| err.to_string())
}

fn get_path_pattern(path: &str) -> Result<PathPattern, String> {
    PathPattern::from(path).map_err(|err| err.to_string())
}
