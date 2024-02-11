use serde_json;
use openapiv3::OpenAPI;
use serde_json::Value;
use uuid::Uuid;
use golem_api_grpc::proto::golem::template::TemplateId;
use golem_common::model::TemplateId;
use crate::api_definition::{ApiDefinition, GolemWorkerBinding, MethodPattern, PathPattern, Route};
use crate::expr::Expr;

pub fn sample()  -> String {
    print!("Golem TimeLine");

    let json_data = r#"
openapi: 3.0.0
info:
  title: Sample API
  version: 1.0.0
paths:
  /users/{id}:
    x-worker-bridge:
      worker-id: "myworker"
      function-name: "golem:it/api/get-cart-contents"
      function-params: []
      template-id: "4d1b159d-51d3-4440-aa29-8edb235f62a3"
    get:
      summary: Get user by ID
      description: getUserById
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: integer
            format: int64
      responses:
        '200':
          description: OK
        '404':
          description: User not found
    "#;

    json_data.to_string()

}

pub fn get_api_definition(open_api: &str) -> Result<ApiDefinition, String> {
    let openapi: OpenAPI = serde_yaml::from_str(open_api).map_err(|e| e.to_string())?;

    let mut routes: Vec<Route> = vec![];

    openapi.paths.iter().for_each(|(path, path_item)| {
        println!("Path: {}", path);


        let worker_bridge_info_result: Result<&Value, String> = match path_item {
            openapiv3::ReferenceOr::Item(item) => {
                println!("extensions: {:?}", item.extensions);
                let worker_bridgge_info = item.extensions.get("x-worker-bridge").ok_or("No x-worker-bridge extension found")?;
                Ok(worker_bridgge_info)
            }
            openapiv3::ReferenceOr::Reference {
                reference,
            } => {
                Err("Reference not supported".to_string())
            }
        };

        let worker_bridge_info = worker_bridge_info_result?;
        let golem_binding = GolemWorkerBinding {
            worker_id: get_worker_id(worker_bridge_info)?,
            function_name: get_function_name(worker_bridge_info)?,
            function_params: get_function_params(worker_bridge_info)?,
            template: get_template_id(worker_bridge_info)?,
            response: None
        };

        let path_pattern = get_path_pattern(path)?;
        let method_res = match path_item {
            openapiv3::ReferenceOr::Item(item) => {
                match &item.get {
                    Some(_) => {
                        Ok(MethodPattern::Get)
                    }
                    None => {
                        Err("Other methods not supported".to_string())
                    }
                }
            }

            openapiv3::ReferenceOr::Reference {
                reference,
            } => {
                Err("Reference not supported".to_string())
            }
        };

        let method = method_res?;
        let route = Route {
            path: path_pattern,
            method: method,
            binding: golem_binding
        };

        routes.push(route);
    });

    Err("not yet implemented".to_string())


}

fn get_template_id(worker_bridge_info: &Value) -> Result<TemplateId, String> {
    let template_id = worker_bridge_info.get("template-id").ok_or("No template-id found")?.as_str().ok_or("template-id is not a string")?;
    Ok(TemplateId(Uuid::parse_str(template_id).map_err(|err| err.to_string())?))
}

fn get_function_params(worker_bridge_info: &Value) -> Result<Vec<Expr>, String> {
    let function_params = worker_bridge_info.get("function-params").ok_or("No function-params found")?.as_array().ok_or("function-params is not an array")?;
    let mut exprs = vec![];
    for param in function_params {
        exprs.push(Expr::from_json_value(param).map_err(|err| err.to_string())?);
    }
    Ok(exprs)
}

fn get_function_name(worker_bridge_info: &Value) -> Result<String, String> {
    let function_name = worker_bridge_info.get("function-name").ok_or("No function-name found")?.as_str().ok_or("function-name is not a string")?;
    Ok(function_name.to_string())
}

fn get_worker_id(worker_bridge_info: &Value) -> Result<Expr, String> {
    let worker_id = worker_bridge_info.get("worker-id").ok_or("No worker-id found")?;
    Expr::from_json_value(worker_id).map_err(|err| err.to_string())
}

fn get_path_pattern(path: &str) -> Result<PathPattern, String> {
    PathPattern::from(path).map_err(|err| err.to_string())
}