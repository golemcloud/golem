use crate::gateway_binding::{
    ResolvedBinding, ResolvedGatewayBinding, RibInputTypeMismatch, RibInputValueResolver,
    StaticBinding,
};
use crate::gateway_execution::file_server_binding_handler::{
    FileServerBindingHandler, FileServerBindingResult,
};
use crate::gateway_execution::to_response::ToResponse;
use crate::gateway_middleware::{Cors as CorsPreflight, Middlewares};
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use async_trait::async_trait;
use golem_common::model::GatewayBindingType;
use rib::RibResult;
use std::fmt::Debug;
use std::sync::Arc;

#[async_trait]
pub trait GatewayBindingExecutor<Namespace, Response> {
    async fn execute_binding(&self, binding: &ResolvedGatewayBinding<Namespace>) -> Response
    where
        RibResult: ToResponse<Response>,
        EvaluationError: ToResponse<Response>,
        RibInputTypeMismatch: ToResponse<Response>,
        FileServerBindingResult: ToResponse<Response>,
        CorsPreflight: ToResponse<Response>;
}

pub struct DefaultGatewayBindingExecutor<Namespace> {
    pub evaluator: Arc<dyn WorkerServiceRibInterpreter + Sync + Send>,
    pub file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
}

impl<Namespace> DefaultGatewayBindingExecutor<Namespace> {
    pub fn new(
        evaluator: Arc<dyn WorkerServiceRibInterpreter + Sync + Send>,
        file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
    ) -> Self {
        Self {
            evaluator,
            file_server_binding_handler,
        }
    }
}

#[async_trait]
impl<N: Send + Sync, R: Debug + Send + Sync> GatewayBindingExecutor<N, R>
    for DefaultGatewayBindingExecutor<N>
{
    async fn execute_binding(&self, binding: &ResolvedGatewayBinding<N>) -> R
    where
        RibResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        RibInputTypeMismatch: ToResponse<R>,
        FileServerBindingResult: ToResponse<R>,
        CorsPreflight: ToResponse<R>,
    {
        let resolved_binding = &binding.resolved_binding;
        let request_details = &binding.request_details;

        match resolved_binding {
            ResolvedBinding::Worker(resolved_worker_binding) => {
                let request_rib_input = request_details.resolve_rib_input_value(
                    &resolved_worker_binding.compiled_response_mapping.rib_input,
                );

                let worker_rib_input = resolved_worker_binding
                    .worker_detail
                    .resolve_rib_input_value(
                        &resolved_worker_binding.compiled_response_mapping.rib_input,
                    );

                match (request_rib_input, worker_rib_input) {
                    (Ok(request_rib_input), Ok(worker_rib_input)) => {
                        let rib_input = request_rib_input.merge(worker_rib_input);
                        let result = self
                            .evaluator
                            .evaluate(
                                resolved_worker_binding.worker_detail.worker_name.as_deref(),
                                &resolved_worker_binding
                                    .worker_detail
                                    .component_id
                                    .component_id,
                                &resolved_worker_binding.worker_detail.idempotency_key,
                                &resolved_worker_binding
                                    .compiled_response_mapping
                                    .response_mapping_compiled
                                    .clone(),
                                &rib_input,
                            )
                            .await;

                        match result {
                            Ok(worker_response) => {
                                match resolved_worker_binding.worker_binding_type {
                                    GatewayBindingType::Default => worker_response.to_response(
                                        &binding.request_details,
                                        &resolved_worker_binding.middlewares,
                                    ),
                                    GatewayBindingType::FileServer => self
                                        .file_server_binding_handler
                                        .handle_file_server_binding_result(
                                            &resolved_worker_binding.namespace,
                                            &resolved_worker_binding.worker_detail,
                                            worker_response,
                                        )
                                        .await
                                        .to_response(
                                            &binding.request_details,
                                            &resolved_worker_binding.middlewares,
                                        ),
                                    GatewayBindingType::CorsPreflight => {
                                        EvaluationError(
                                            "Cors preflight is not supported".to_string(),
                                        )
                                        .to_response(
                                            &binding.request_details,
                                            &resolved_worker_binding.middlewares,
                                        ) //TODO; remove this as it is an invalid state (IFS PR driven changes)
                                    }
                                }
                            }
                            Err(err) => err.to_response(
                                &binding.request_details,
                                &resolved_worker_binding.middlewares,
                            ),
                        }
                    }
                    (Err(err), _) => err.to_response(
                        &binding.request_details,
                        &resolved_worker_binding.middlewares,
                    ),
                    (_, Err(err)) => err.to_response(
                        &binding.request_details,
                        &resolved_worker_binding.middlewares,
                    ),
                }
            }

            ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                let cors_preflight = cors_preflight.clone();
                cors_preflight.to_response(&binding.request_details, &Middlewares::default())
            }
        }
    }
}
