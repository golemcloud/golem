use crate::gateway_binding::{
    GatewayRequestDetails,ResolvedBinding, ResolvedGatewayBinding,
    ResolvedWorkerBinding, RibInputTypeMismatch, RibInputValueResolver, StaticBinding,
};
use crate::gateway_execution::file_server_binding_handler::{
    FileServerBindingHandler, FileServerBindingResult,
};
use crate::gateway_execution::gateway_session::{GatewaySessionStore};
use crate::gateway_execution::to_response::ToResponse;
use crate::gateway_middleware::{Cors as CorsPreflight, Middlewares};
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use async_trait::async_trait;
use rib::{RibInput, RibResult};
use std::fmt::Debug;
use std::sync::Arc;
use http::StatusCode;
use crate::gateway_execution::auth_call_back_binding_handler::AuthCallBackBindingHandler;
use crate::gateway_execution::to_response_failure::ToResponseFailure;
use crate::gateway_security::SecuritySchemeInternal;

#[async_trait]
pub trait GatewayBindingExecutor<Namespace, Response> {
    async fn execute_binding(
        &self,
        binding: &ResolvedGatewayBinding<Namespace>,
        session: GatewaySessionStore,
    ) -> Response
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
    pub auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
}

impl<N> DefaultGatewayBindingExecutor<N> {
    pub fn new(
        evaluator: Arc<dyn WorkerServiceRibInterpreter + Sync + Send>,
        file_server_binding_handler: Arc<dyn FileServerBindingHandler<N> + Sync + Send>,
        auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
    ) -> Self {
        Self {
            evaluator,
            file_server_binding_handler,
            auth_call_back_binding_handler
        }
    }

    async fn resolve_rib_inputs<R>(
        &self,
        request_details: &GatewayRequestDetails,
        resolved_worker_binding: &ResolvedWorkerBinding<N>,
    ) -> Result<(RibInput, RibInput), R>
    where
        RibInputTypeMismatch: ToResponse<R>,
    {
        let request_rib_input = request_details
            .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
            .map_err(|err| err.to_failed_response(&StatusCode::BAD_REQUEST))?;

        let worker_rib_input = resolved_worker_binding
            .worker_detail
            .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
            .map_err(|err| err.to_failed_response(&StatusCode::BAD_REQUEST))?;

        Ok((request_rib_input, worker_rib_input))
    }

    async fn get_rib_result(
        &self,
        request_rib_input: RibInput,
        worker_rib_input: RibInput,
        resolved_worker_binding: &ResolvedWorkerBinding<N>,
    ) -> Result<RibResult, EvaluationError> {
        let rib_input = request_rib_input.merge(worker_rib_input);
        self.evaluator
            .evaluate(
                resolved_worker_binding.worker_detail.worker_name.as_deref(),
                &resolved_worker_binding
                    .worker_detail
                    .component_id
                    .component_id,
                &resolved_worker_binding.worker_detail.idempotency_key,
                &resolved_worker_binding
                    .compiled_response_mapping
                    .response_mapping_compiled,
                &rib_input,
            )
            .await
    }

    async fn handle_worker_binding<R>(
        &self,
        binding: &ResolvedGatewayBinding<N>,
        resolved_binding: &ResolvedWorkerBinding<N>,
        session_store: &GatewaySessionStore,
    ) -> R
    where
        RibResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        RibInputTypeMismatch: ToResponse<R>,
    {
        match self
            .resolve_rib_inputs(&binding.request_details, resolved_binding)
            .await
        {
            Ok((request_rib_input, worker_rib_input)) => {
                match self
                    .get_rib_result(request_rib_input, worker_rib_input, resolved_binding)
                    .await
                {
                    Ok(result) => {
                        result.to_response(&binding.request_details, session_store)
                    }
                    Err(err) => {
                        err.to_failed_response(&StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }
            }
            Err(err_response) => err_response,
        }
    }

    async fn handle_file_server_binding<R>(
        &self,
        binding: &ResolvedGatewayBinding<N>,
        resolved_binding: &ResolvedWorkerBinding<N>,
        session_store: &GatewaySessionStore,
    ) -> R
    where
        FileServerBindingResult: ToResponse<R>,
        RibResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        RibInputTypeMismatch: ToResponse<R>,
    {
        match self
            .resolve_rib_inputs(&binding.request_details, resolved_binding)
            .await
        {
            Ok((request_rib_input, worker_rib_input)) => {
                match self
                    .get_rib_result(request_rib_input, worker_rib_input, resolved_binding)
                    .await
                {
                    Ok(worker_response) => self
                        .file_server_binding_handler
                        .handle_file_server_binding_result(
                            &resolved_binding.namespace,
                            &resolved_binding.worker_detail,
                            worker_response,
                        )
                        .await
                        .to_response(&binding.request_details, session_store),
                    Err(err) => {
                        err.to_failed_response(&StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }
            }
            Err(err_response) => err_response,
        }
    }
}

#[async_trait]
impl<N: Send + Sync, R: Debug + Send + Sync> GatewayBindingExecutor<N, R>
    for DefaultGatewayBindingExecutor<N>
{
    async fn execute_binding(
        &self,
        binding: &ResolvedGatewayBinding<N>,
        session: GatewaySessionStore,
    ) -> R
    where
        RibResult: ToResponse<R>,
        EvaluationError: ToResponse<R>,
        RibInputTypeMismatch: ToResponse<R>,
        FileServerBindingResult: ToResponse<R>,
        CorsPreflight: ToResponse<R>,
        SecuritySchemeInternal: ToResponse<R>,
    {
        match &binding.resolved_binding {
            ResolvedBinding::Worker(resolved_binding) => {
                let mut response = self.handle_worker_binding::<R>(binding, resolved_binding, &session)
                    .await;
                resolved_binding.middlewares.process(response);
                response
            }
            ResolvedBinding::FileServer(resolved_binding) => {
                self.handle_file_server_binding::<R>(binding, resolved_binding)
                    .await
            }
            ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                cors_preflight
                    .clone()
                    .to_response(&binding.request_details, &Middlewares::default())
            }

            ResolvedBinding::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
                auth_call_back
                    .clone()
                    .to_response(&binding.request_details, &Middlewares::default())
            }
        }


    }
}
