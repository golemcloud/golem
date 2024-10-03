use crate::api_definition::http::{CompiledHttpApiDefinition, VarInfo};
use crate::http::http_request::router;
use crate::http::router::RouterPattern;
use crate::http::InputHttpRequest;
use crate::worker_service_rib_interpreter::EvaluationError;
use crate::worker_service_rib_interpreter::WorkerServiceRibInterpreter;
use async_trait::async_trait;
use golem_common::model::IdempotencyKey;
use golem_service_base::model::VersionedComponentId;
use rib::RibInterpreterResult;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use crate::worker_binding::rib_input_value_resolver::RibInputValueResolver;
use crate::worker_binding::{RequestDetails, ResponseMappingCompiled, RibInputTypeMismatch};
use crate::worker_bridge_execution::to_response::ToResponse;

// Every type of request (example: InputHttpRequest (which corresponds to a Route)) can have an instance of this resolver,
// to resolve a single worker-binding is then executed with the help of worker_service_rib_interpreter, which internally
// calls the worker function.
#[async_trait]
pub trait RequestToWorkerBindingResolver<ApiDefinition> {
    async fn resolve_worker_binding(
        &self,
        api_definitions: Vec<ApiDefinition>,
    ) -> Result<ResolvedWorkerBindingFromRequest, WorkerBindingResolutionError>;
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
pub struct ResolvedWorkerBindingFromRequest {
    pub worker_detail: WorkerDetail,
    pub request_details: RequestDetails,
    pub compiled_response_mapping: ResponseMappingCompiled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerDetail {
    pub component_id: VersionedComponentId,
    pub worker_name: String,
    pub idempotency_key: Option<IdempotencyKey>,
}

impl WorkerDetail {
    pub fn as_json(&self) -> Value {
        let mut worker_detail_content = HashMap::new();
        worker_detail_content.insert(
            "component_id".to_string(),
            Value::String(self.component_id.component_id.0.to_string()),
        );
        worker_detail_content.insert("name".to_string(), Value::String(self.worker_name.clone()));
        if let Some(idempotency_key) = &self.idempotency_key {
            worker_detail_content.insert(
                "idempotency_key".to_string(),
                Value::String(idempotency_key.value.clone()),
            );
        }

        let map = serde_json::Map::from_iter(worker_detail_content);

        Value::Object(map)
    }
}

impl ResolvedWorkerBindingFromRequest {
    pub async fn interpret_response_mapping<R>(
        &self,
        evaluator: &Arc<dyn WorkerServiceRibInterpreter + Sync + Send>,
    ) -> R
    where
        RibInterpreterResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        RibInputTypeMismatch: ToResponse<R>,
    {
        let request_rib_input = self
            .request_details
            .resolve_rib_input_value(&self.compiled_response_mapping.rib_input);

        let worker_rib_input = self
            .worker_detail
            .resolve_rib_input_value(&self.compiled_response_mapping.rib_input);

        match (request_rib_input, worker_rib_input) {
            (Ok(request_rib_input), Ok(worker_rib_input)) => {
                let rib_input = request_rib_input.merge(worker_rib_input);
                let result = evaluator
                    .evaluate(
                        &self.worker_detail.worker_name,
                        &self.worker_detail.component_id.component_id,
                        &self.worker_detail.idempotency_key,
                        &self.compiled_response_mapping.compiled_response.clone(),
                        &rib_input,
                    )
                    .await;

                match result {
                    Ok(worker_response) => worker_response.to_response(&self.request_details),
                    Err(err) => err.to_response(&self.request_details),
                }
            }
            (Err(err), _) => err.to_response(&self.request_details),
            (_, Err(err)) => err.to_response(&self.request_details),
        }
    }
}

#[async_trait]
impl RequestToWorkerBindingResolver<CompiledHttpApiDefinition> for InputHttpRequest {
    async fn resolve_worker_binding(
        &self,
        compiled_api_definitions: Vec<CompiledHttpApiDefinition>,
    ) -> Result<ResolvedWorkerBindingFromRequest, WorkerBindingResolutionError> {
        let compiled_routes = compiled_api_definitions
            .iter()
            .flat_map(|x| x.routes.clone())
            .collect::<Vec<_>>();

        let api_request = self;
        let router = router::build(compiled_routes);
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

        let http_request_details = RequestDetails::from(
            &zipped_path_params,
            &request_query_variables,
            query_params,
            request_body,
            headers,
        )
        .map_err(|err| format!("Failed to fetch input request details {}", err.join(", ")))?;

        let resolve_rib_input = http_request_details
            .resolve_rib_input_value(&binding.worker_name_compiled.rib_input_type_info)
            .map_err(|err| {
                format!(
                    "Failed to resolve rib input value from http request details {}",
                    err
                )
            })?;

        // To evaluate worker-name, most probably
        let worker_name: String = rib::interpret_pure(
            &binding.worker_name_compiled.compiled_worker_name,
            &resolve_rib_input.value,
        )
        .await
        .map_err(|err| format!("Failed to evaluate worker name rib expression. {}", err))?
        .get_literal()
        .ok_or("Worker name is not a Rib expression that resolves to String".to_string())?
        .as_string();

        let component_id = &binding.component_id;

        let idempotency_key =
            if let Some(idempotency_key_compiled) = &binding.idempotency_key_compiled {
                let idempotency_key_value = rib::interpret_pure(
                    &idempotency_key_compiled.compiled_idempotency_key,
                    &resolve_rib_input.value,
                )
                .await
                .map_err(|err| err.to_string())?;

                let idempotency_key = idempotency_key_value
                    .get_literal()
                    .ok_or("Idempotency Key is not a string")?
                    .as_string();

                Some(IdempotencyKey::new(idempotency_key))
            } else {
                headers
                    .get("idempotency-key")
                    .and_then(|h| h.to_str().ok())
                    .map(|value| IdempotencyKey::new(value.to_string()))
            };

        let worker_detail = WorkerDetail {
            component_id: component_id.clone(),
            worker_name,
            idempotency_key,
        };

        let resolved_binding = ResolvedWorkerBindingFromRequest {
            worker_detail,
            request_details: http_request_details,
            compiled_response_mapping: binding.response_compiled.clone(),
        };

        Ok(resolved_binding)
    }
}
