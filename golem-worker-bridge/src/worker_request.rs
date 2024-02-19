use golem_common::model::TemplateId;
use serde_json::Value;

use crate::api_request_route_resolver::ResolvedRoute;
use crate::evaluator::{Evaluator, Primitive};

// Every resolved route definition can be mapped to a worker request
#[derive(PartialEq, Debug, Clone)]
pub struct ResolvedRouteAsWorkerRequest {
    pub resolved_route: ResolvedRoute,
    pub template: TemplateId,
    pub worker_id: String,
    pub function: String,
    pub function_params: Value,
}

impl ResolvedRouteAsWorkerRequest {
    // A worker-request can bef formed a route with resolved variables (through incoming http request)
    pub fn from_resolved_route(
        resolved_route: ResolvedRoute,
    ) -> Result<ResolvedRouteAsWorkerRequest, String> {
        let worker_id: Value = resolved_route
            .route_definition
            .binding
            .worker_id
            .evaluate(&resolved_route.resolved_variables)
            .map_err(|err| err.to_string())?;

        let function_name = Primitive::new(&resolved_route.route_definition.binding.function_name)
            .evaluate(&resolved_route.resolved_variables)
            .map_err(|err| err.to_string())?;

        let mut function_params: Vec<Value> = vec![];

        for expr in &resolved_route.route_definition.binding.function_params {
            let json = expr
                .evaluate(&resolved_route.resolved_variables)
                .map_err(|err| err.to_string())?;

            function_params.push(json);
        }

        let worker_id_str = worker_id.as_str().ok_or(format!(
            "Worker id is not evaluated to a valid string. {}",
            worker_id
        ))?;

        Ok(ResolvedRouteAsWorkerRequest {
            resolved_route: resolved_route.clone(),
            worker_id: worker_id_str.to_string(),
            template: resolved_route.route_definition.binding.template.clone(),
            function: function_name,
            function_params: Value::Array(function_params),
        })
    }
}
