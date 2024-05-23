use crate::api_definition::http::{HttpApiDefinition, VarInfo};
use crate::evaluator::{
    DefaultEvaluator, EvaluationContext, EvaluationError, EvaluationResult, MetadataFetchError,
};
use crate::evaluator::{Evaluator, WorkerMetadataFetcher};
use crate::http::http_request::router;
use crate::http::router::RouterPattern;
use crate::http::InputHttpRequest;
use crate::merge::Merge;
use crate::primitive::GetPrimitive;
use async_trait::async_trait;
use golem_common::model::{ComponentId, IdempotencyKey};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use crate::worker_binding::{RequestDetails, ResponseMapping};
use crate::worker_bridge_execution::to_response::ToResponse;

#[async_trait]
pub trait WorkerBindingResolver<ApiDefinition> {
    async fn resolve(
        &self,
        api_specification: &ApiDefinition,
    ) -> Result<ResolvedWorkerBinding, WorkerBindingResolutionError>;
}

#[derive(Debug)]
pub struct WorkerBindingResolutionError(pub String);

impl<A: AsRef<str>> From<A> for WorkerBindingResolutionError {
    fn from(message: A) -> Self {
        WorkerBindingResolutionError(message.as_ref().to_string())
    }
}

impl Display for WorkerBindingResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Worker binding resolution error: {}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkerBinding {
    pub worker_detail: WorkerDetail,
    pub request_details: RequestDetails,
    pub response_mapping: Option<ResponseMapping>,
}

#[derive(Debug, Clone)]
pub struct WorkerDetail {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub idempotency_key: Option<IdempotencyKey>,
}

impl WorkerDetail {
    pub fn to_type_annotated_value(self) -> TypeAnnotatedValue {
        let mut required = TypeAnnotatedValue::Record {
            typ: vec![
                ("component_id".to_string(), AnalysedType::Str),
                ("name".to_string(), AnalysedType::Str),
            ],
            value: vec![
                (
                    "component_id".to_string(),
                    TypeAnnotatedValue::Str(self.component_id.0.to_string()),
                ),
                (
                    "name".to_string(),
                    TypeAnnotatedValue::Str(self.worker_name),
                ),
            ],
        };

        let optional_idempotency_key = self.idempotency_key.map(|x| TypeAnnotatedValue::Record {
            // Idempotency key can exist in header of the request in which case users can refer to it as
            // request.headers.idempotency-key. In order to keep some consistency, we are keeping the same key name here,
            // if it exists as part of the API definition
            typ: vec![("idempotency-key".to_string(), AnalysedType::Str)],
            value: vec![(
                "idempotency-key".to_string(),
                TypeAnnotatedValue::Str(x.to_string()),
            )],
        });

        if let Some(idempotency_key) = optional_idempotency_key {
            required = required.merge(&idempotency_key).clone();
        }

        required
    }
}

impl ResolvedWorkerBinding {
    pub async fn execute_with<R>(
        &self,
        evaluator: &Arc<dyn Evaluator + Sync + Send>,
        worker_metadata_fetcher: &Arc<dyn WorkerMetadataFetcher + Sync + Send>,
    ) -> R
    where
        EvaluationResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        MetadataFetchError: ToResponse<R>,
    {
        let functions_available = worker_metadata_fetcher
            .get_worker_metadata(&self.worker_detail.component_id)
            .await;

        match functions_available {
            Ok(functions) => {
                let runtime = EvaluationContext::from_all(
                    &self.worker_detail,
                    &self.request_details,
                    functions,
                );

                let result = evaluator
                    .evaluate(&self.response_mapping.clone().unwrap().0, &runtime)
                    .await;

                match result {
                    Ok(worker_response) => worker_response.to_response(&self.request_details),
                    Err(err) => err.to_response(&self.request_details),
                }
            }
            Err(err) => err.to_response(&self.request_details),
        }
    }
}

#[async_trait]
impl WorkerBindingResolver<HttpApiDefinition> for InputHttpRequest {
    async fn resolve(
        &self,
        api_definition: &HttpApiDefinition,
    ) -> Result<ResolvedWorkerBinding, WorkerBindingResolutionError> {
        let default_evaluator = DefaultEvaluator::noop();

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
        } = router
            .check_path(&api_request.req_method, &path)
            .ok_or("Failed to resolve route")?;

        let zipped_path_params: HashMap<VarInfo, &str> = {
            path_params
                .iter()
                .map(|(var, index)| (var.clone(), path[*index]))
                .collect()
        };

        let request_details = RequestDetails::from(
            &zipped_path_params,
            &request_query_variables,
            query_params,
            request_body,
            headers,
        )
        .map_err(|err| format!("Failed to fetch input request details {}", err.join(", ")))?;

        let request_evaluation_context = EvaluationContext::from_request_data(&request_details);

        let worker_name: String = default_evaluator
            .evaluate(&binding.worker_name, &request_evaluation_context)
            .await
            .map_err(|err| err.to_string())?
            .get_value()
            .ok_or("Failed to evaluate worker name expression".to_string())?
            .get_primitive()
            .ok_or("Worker name is not a String".to_string())?
            .as_string();

        let component_id = &binding.component_id;

        let idempotency_key = if let Some(expr) = &binding.idempotency_key {
            let idempotency_key_value = default_evaluator
                .evaluate(&expr, &request_evaluation_context)
                .await
                .map_err(|err| err.to_string())?;

            let idempotency_key = idempotency_key_value
                .get_primitive()
                .ok_or("Idempotency Key is not a string")?
                .as_string();

            Some(IdempotencyKey::new(idempotency_key))
        } else {
            headers
                .get("idempotency-key")
                .and_then(|h| h.to_str().ok())
                .map(|value| IdempotencyKey::new(value.to_string()))
        };

        let worker_request = WorkerDetail {
            component_id: component_id.clone(),
            worker_name,
            idempotency_key,
        };

        let resolved_binding = ResolvedWorkerBinding {
            worker_detail: worker_request,
            request_details,
            response_mapping: binding.response.clone(),
        };

        Ok(resolved_binding)
    }
}
