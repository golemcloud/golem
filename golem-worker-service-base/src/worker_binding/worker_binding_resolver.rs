use crate::api_definition::http::{HttpApiDefinition, VarInfo};
use crate::evaluator::Evaluator;
use crate::evaluator::*;
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

use golem_service_base::model::{Id, WorkerId};

use crate::worker_binding::{RequestDetails, ResponseMapping};
use crate::worker_bridge_execution::to_response::ToResponse;

// TODO; It will be better if worker binding resolver
// able to deal with only one API definition
// as the first stage resolution can take place (based on host, input request (route resolution)
// up the stage
#[async_trait]
pub trait WorkerBindingResolver<ApiDefinition> {
    async fn resolve(
        &self,
        api_definitions: Vec<ApiDefinition>,
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
    pub response_mapping: ResponseMapping,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerDetail {
    pub component_id: ComponentId,
    pub worker_name: String,
    pub idempotency_key: Option<IdempotencyKey>,
}

impl WorkerDetail {
    pub fn to_type_annotated_value(&self) -> TypeAnnotatedValue {
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
                    TypeAnnotatedValue::Str(self.worker_name.clone()),
                ),
            ],
        };

        let optional_idempotency_key =
            self.idempotency_key
                .clone()
                .map(|x| TypeAnnotatedValue::Record {
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
        symbol_fetch: &Arc<dyn ComponentElementsFetch + Sync + Send>,
    ) -> R
    where
        ExprEvaluationResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        MetadataFetchError: ToResponse<R>,
    {
        let worker_name = match Id::try_from(self.worker_detail.worker_name.clone()) {
            Ok(worker_name) => worker_name,
            Err(err) => {
                return EvaluationError::Message(err.to_string()).to_response(&self.request_details)
            }
        };

        let worker_id = WorkerId {
            component_id: self.worker_detail.component_id.clone(),
            worker_name,
        };

        internal::get_response(
            self,
            &worker_id,
            evaluator,
            symbol_fetch,
            internal::CachePresence::Present,
        )
        .await
    }
}

#[async_trait]
impl WorkerBindingResolver<HttpApiDefinition> for InputHttpRequest {
    async fn resolve(
        &self,
        api_definition: Vec<HttpApiDefinition>,
    ) -> Result<ResolvedWorkerBinding, WorkerBindingResolutionError> {
        let default_evaluator = DefaultEvaluator::noop();

        let routes = api_definition
            .iter()
            .flat_map(|x| x.routes.clone())
            .collect::<Vec<_>>();

        let api_request = self;
        let router = router::build(routes);
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
                .evaluate(expr, &request_evaluation_context)
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

        let worker_detail = WorkerDetail {
            component_id: component_id.clone(),
            worker_name,
            idempotency_key,
        };

        let resolved_binding = ResolvedWorkerBinding {
            worker_detail,
            request_details,
            response_mapping: binding.response.clone(),
        };

        Ok(resolved_binding)
    }
}

mod internal {
    use crate::evaluator::{
        EvaluationContext, EvaluationError, Evaluator, ExprEvaluationResult, MetadataFetchError,
        ComponentElementsFetch,
    };
    use crate::worker_binding::ResolvedWorkerBinding;
    use crate::worker_bridge_execution::to_response::ToResponse;
    use golem_service_base::model::WorkerId;
    use std::sync::Arc;

    pub(crate) enum CachePresence {
        Present,
        Absent,
    }

    impl CachePresence {
        fn is_present(&self) -> bool {
            match self {
                CachePresence::Present => true,
                CachePresence::Absent => false,
            }
        }
    }
    pub(crate) async fn get_response<R>(
        resolved_worker_binding: &ResolvedWorkerBinding,
        worker_id: &WorkerId,
        evaluator: &Arc<dyn Evaluator + Sync + Send>,
        component_elements_fetch: &Arc<dyn ComponentElementsFetch + Sync + Send>,
        cache_presence: CachePresence,
    ) -> R
    where
        ExprEvaluationResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        MetadataFetchError: ToResponse<R>,
    {
        let functions_available = component_elements_fetch
            .get_component_elements(worker_id.component_id.clone())
            .await;

        match functions_available {
            Ok(component_elements) => {
                let evaluation_context = EvaluationContext::from_all(
                    &resolved_worker_binding.worker_detail,
                    &resolved_worker_binding.request_details,
                    component_elements,
                );

                match evaluation_context {
                    Ok(context) => {
                        let result = evaluator
                            .evaluate(
                                &resolved_worker_binding.response_mapping.clone().0,
                                &context,
                            )
                            .await;

                        match result {
                            Ok(worker_response) => worker_response
                                .to_response(&resolved_worker_binding.request_details),
                            Err(err) => match err {
                                EvaluationError::FunctionInvokeError(_)
                                    if cache_presence.is_present() =>
                                {
                                    component_elements_fetch
                                        .invalidate_cached_component_elements(&worker_id.component_id);
                                    Box::pin(get_response(
                                        resolved_worker_binding,
                                        worker_id,
                                        evaluator,
                                        component_elements_fetch,
                                        CachePresence::Absent,
                                    ))
                                    .await
                                }

                                _ => err.to_response(&resolved_worker_binding.request_details),
                            },
                        }
                    }
                    Err(err) => MetadataFetchError(err)
                        .to_response(&resolved_worker_binding.request_details),
                }
            }
            Err(err) => err.to_response(&resolved_worker_binding.request_details),
        }
    }
}
