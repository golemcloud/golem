use std::collections::HashMap;
use std::sync::Arc;
use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_wasm_rpc::TypeAnnotatedValue;
use http::StatusCode;
use poem::{Body, EndpointExt, Response};
use serde_json::Value;
use tracing::error;
use uuid::Uuid;
use golem_common::model::ComponentId;
use crate::api_definition::http::{HttpApiDefinition, VarInfo};
use crate::http::http_request::router;
use crate::http::InputHttpRequest;
use crate::http::router::RouterPattern;
use crate::evaluator::{EvaluatorInputContext};
use crate::evaluator::Evaluator;
use crate::primitive::GetPrimitive;

use crate::worker_binding::{RequestDetails, ResponseMapping};
use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest, WorkerRequestExecutor, WorkerResponse};
use crate::worker_bridge_execution::to_response::ToResponse;

// For any input request type, there should be a way to resolve the
// worker binding component, which is then used to form the worker request
// resolved binding is always kept along with the request as binding may refer
// to request details
pub trait WorkerBindingResolver<ApiDefinition> {
    fn resolve(&self, api_specification: &ApiDefinition) -> Option<ResolvedWorkerBinding>;
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkerBinding {
    pub worker_request: WorkerRequest,
    pub request_details: RequestDetails,
    pub response_mapping: Option<ResponseMapping>
}

impl ResolvedWorkerBinding {
    pub async fn execute_with<A>(&self, executor: &Arc<dyn WorkerRequestExecutor>) -> A where WorkerResponse: ToResponse<A> {
        let worker_request = &self.worker_request;
        let worker_response = executor.execute(worker_request.clone()).await.map_err(|err| err.to_string());

        match worker_response {
            Ok(worker_response) => {
                worker_response.to_response(&self.response_mapping.clone(), &self.request_details);
            }
            Err(e) => {
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from_string(
                        format!("API request error {}", e).to_string(),
                    ))
            }
        }
    }
}

impl WorkerBindingResolver<HttpApiDefinition> for InputHttpRequest {
    fn resolve(&self, api_definition: &HttpApiDefinition) -> Option<ResolvedWorkerBinding> {
        let api_request = self;
        let router = router::build(api_definition.routes.clone());
        let path: Vec<&str> = RouterPattern::split(&api_request.input_path.base_path).collect();
        let request_query_variables = self.input_path.query_components().unwrap_or_default();
        let request_body = &self.req_body;
        let headers = &self.headers;

        let router::RouteEntry {
            path_params,
            query_params,
            binding,
        } = router.check_path(&api_request.req_method, &path)?;

        let zipped_path_params: HashMap<VarInfo, &str> = {
            path_params
                .iter()
                .map(|(var, index)| (var.clone(), path[*index]))
                .collect()
        };

        let request_details =
            RequestDetails::from(&zipped_path_params, &request_query_variables, query_params, request_body, headers)?;

        let request_evaluation_context = EvaluatorInputContext::from_request_data(&request_details);

        let worker_name: String =
            binding
            .worker_id
            .evaluate(&request_evaluation_context)
            .map_err(|err| err.to_string())?
            .get_value()
            .ok_or("Worker id is not a text value".to_string())?.get_primitive().ok_or("Worker id is not a primitive".to_string())?.as_string();

        let function_name =
            &binding.function_name
            .evaluate(&request_evaluation_context)
            .map_err(|err| err.to_string())?
                .get_value()
                .ok_or("Worker id is not a text value".to_string())?.get_primitive().ok_or("Worker id is not a primitive".to_string())?.as_string();

        let mut function_params: Vec<Value> = vec![];

        for expr in &binding
            .function_params
        {
            let type_annotated_value = expr
                .evaluate(&request_evaluation_context)
                .map_err(|err| err.to_string())?
                .get_value()
                .ok_or("Failed to evaluate Route expression".to_string())?;

            let json = get_json_from_typed_value(&type_annotated_value);

            function_params.push(json);
        }

        let component_id_text: String = binding
            .worker_id
            .evaluate(&request_evaluation_context)
            .map_err(|err| err.to_string())?
            .get_value()
            .ok_or("Worker id is not a text value".to_string())?.get_primitive().ok_or("Worker id is not a primitive".to_string())?.as_string();

        let component_id = ComponentId(Uuid::parse_str(&component_id_text).map_err(|err| err.to_string())?);


        let worker_request = WorkerRequest {
            component_id,
            worker_name,
            function_name: function_name.to_string(),
            function_params
        };

        let resolved_binding = ResolvedWorkerBinding {
            worker_request,
            request_details,
            response_mapping: binding.response.clone()
        };

        Some(resolved_binding)
    }
}