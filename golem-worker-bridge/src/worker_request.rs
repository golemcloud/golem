use golem_common::model::TemplateId;

use crate::api_request_route_resolver::ResolvedRoute;
use crate::evaluator::{Evaluator, Primitive};
use golem_api_grpc::proto::golem::worker::val::Val;
use crate::api_spec::ResponseMapping;
use crate::typed_json::ValueTyped;
use crate::worker::WorkerName;

#[derive(PartialEq, Debug, Clone)]
pub struct GolemWorkerRequest {
    pub template: TemplateId,
    pub worker_id: String,
    pub function: String,
    pub function_params: Val,
    pub response_mapping: Option<ResponseMapping>,
}

impl GolemWorkerRequest {
    pub fn from_resolved_route(
        resolved_route: &ResolvedRoute,
    ) -> Result<GolemWorkerRequest, String> {
        let worker_id: ValueTyped = resolved_route
            .route_definition
            .binding
            .worker_id
            .evaluate(&resolved_route.resolved_variables)
            .map_err(|err| err.to_string())?;

        let function_name = Primitive::new(&resolved_route.route_definition.binding.function_name)
            .evaluate(&resolved_route.resolved_variables)
            .map_err(|err| err.to_string())?;

        // TODO; Once we make use of golem_common::Val directly, we don't need this conversion to JSON
        let mut function_params: Vec<serde_json::Value> = vec![];

        for expr in &resolved_route.route_definition.binding.function_params {
            let variant = expr
                .evaluate(&resolved_route.resolved_variables)
                .map_err(|err| err.to_string())?;

            let json = variant.convert_to_json();

            function_params.push(json);
        }

        Ok(GolemWorkerRequest {
            worker_id,
            template: resolved_route.route_definition.binding.template.clone(),
            function: function_name,
            function_params: Val,
            response_mapping: resolved_route.route_definition.binding.response.clone(),
        })
    }

    pub fn get_worker_name(&self) -> Result<WorkerName, String> {
        let worker_name_string = self
            .worker_id
            .get_primitive_string()
            .ok_or("Evaluated worker id is a complex string")?;

        Ok(WorkerName(worker_name_string))
    }
}
