use crate::gateway_binding::{
    GatewayRequestDetails, ResolvedBinding, ResolvedGatewayBinding, ResolvedWorkerBinding,
    RibInputTypeMismatch, RibInputValueResolver, StaticBinding,
};
use crate::gateway_execution::auth_call_back_binding_handler::{
    AuthCallBackBindingHandler, AuthCallBackResult,
};
use crate::gateway_execution::file_server_binding_handler::{
    FileServerBindingHandler, FileServerBindingResult,
};
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_execution::to_response::ToResponse;
use crate::gateway_execution::to_response_failure::ToResponseFailure;
use crate::gateway_middleware::{
    Cors as CorsPreflight, HttpAuthorizer, MiddlewareIn, MiddlewareOut, MiddlewareResult,
    Middlewares,
};
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use crate::gateway_security::SecuritySchemeInternal;
use async_trait::async_trait;
use http::StatusCode;
use rib::{RibInput, RibResult};
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[async_trait]
pub trait GatewayBindingExecutor<Namespace, Response> {
    async fn execute_binding(
        &self,
        binding: &ResolvedGatewayBinding<Namespace>,
        session: GatewaySessionStore,
    ) -> Response
    where
        EvaluationError: ToResponseFailure<Response>,
        RibInputTypeMismatch: ToResponseFailure<Response>,
        RibResult: ToResponse<Response>,
        FileServerBindingResult: ToResponse<Response>,
        CorsPreflight: ToResponse<Response>,
        AuthCallBackResult: ToResponse<Response>,
        HttpAuthorizer: MiddlewareIn<Response>,
        CorsPreflight: MiddlewareOut<Response>;
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
            auth_call_back_binding_handler,
        }
    }

    async fn resolve_rib_inputs<R>(
        &self,
        request_details: &GatewayRequestDetails,
        resolved_worker_binding: &ResolvedWorkerBinding<N>,
    ) -> Result<(RibInput, RibInput), R>
    where
        RibInputTypeMismatch: ToResponseFailure<R>,
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
        EvaluationError: ToResponseFailure<R>,
        RibInputTypeMismatch: ToResponseFailure<R>,
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
                        result
                            .to_response(&binding.request_details, session_store)
                            .await
                    }
                    Err(err) => err.to_failed_response(&StatusCode::INTERNAL_SERVER_ERROR),
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
        EvaluationError: ToResponseFailure<R>,
        RibInputTypeMismatch: ToResponseFailure<R>,
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
                    Ok(worker_response) => {
                        self.file_server_binding_handler
                            .handle_file_server_binding_result(
                                &resolved_binding.namespace,
                                &resolved_binding.worker_detail,
                                worker_response,
                            )
                            .await
                            .to_response(&binding.request_details, session_store)
                            .await
                    }
                    Err(err) => err.to_failed_response(&StatusCode::INTERNAL_SERVER_ERROR),
                }
            }
            Err(err_response) => err_response,
        }
    }

    async fn handle_http_auth_call_binding<R>(
        &self,
        binding: &ResolvedGatewayBinding<N>,
        auth_call_back: &SecuritySchemeInternal,
        session_store: &GatewaySessionStore,
    ) -> R
    where
        AuthCallBackResult: ToResponse<R>,
    {
        match &binding.request_details {
            GatewayRequestDetails::Http(http_request) => {
                let authorisation_result = self
                    .auth_call_back_binding_handler
                    .handle_auth_call_back(&http_request, &auth_call_back, &session_store)
                    .await;

                authorisation_result
                    .to_response(&binding.request_details, session_store)
                    .await
            }
        }
    }

    async fn redirect_or_continue<R>(
        incoming_middleware_result: MiddlewareResult<R>,
        session: &GatewaySessionStore,
        continue_fn: impl FnOnce() -> Pin<Box<dyn Future<Output = Result<R, poem::Error>> + Send>>,
        middlewares: Middlewares, // To process any middleware-out if any
    ) -> R {
        match incoming_middleware_result {
            MiddlewareResult::Redirect(response) => response,
            MiddlewareResult::PassThrough => {
                let mut response = continue_fn().await?;

                middlewares
                    .process_middleware_out(session, &mut response)
                    .await?;

                Ok(response)
            }
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
        EvaluationError: ToResponseFailure<R>,
        RibInputTypeMismatch: ToResponseFailure<R>,
        FileServerBindingResult: ToResponse<R>, // FileServerBindingResult can be a direct response in a file server endpoint
        CorsPreflight: ToResponse<R>, // Cors can be a direct response in a cors preflight endpoint
        AuthCallBackResult: ToResponse<R>, // AuthCallBackResult can be a direct response in auth callback endpoint
        HttpAuthorizer: MiddlewareIn<R>,   // HttpAuthorizer can authorise input
        CorsPreflight: MiddlewareOut<R>,   // CorsPreflight can be a middleware in other endpoints
    {
        match &binding.resolved_binding {
            ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                cors_preflight
                    .clone()
                    .to_response(&binding.request_details, &session)
                    .await
            }

            ResolvedBinding::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
                self.handle_http_auth_call_binding(
                    binding,
                    &auth_call_back.scheme_internal,
                    &session,
                )
                .await
            }

            ResolvedBinding::Worker(resolved_worker_binding) => {
                let input_middleware_result = resolved_worker_binding
                    .middlewares
                    .process_middleware_in(&session, &binding.request_details)
                    .await?;

                Self::redirect_or_continue(
                    input_middleware_result,
                    &session,
                    || async {
                        let response = self
                            .handle_worker_binding::<R>(binding, resolved_worker_binding, &session)
                            .await;
                        Ok(response)
                    },
                    resolved_worker_binding.middlewares.clone(),
                )
                .await
            }

            ResolvedBinding::FileServer(resolved_file_server_binding) => {
                let input_middleware_result = resolved_file_server_binding
                    .middlewares
                    .process_middleware_in(&session, &binding.request_details);

                Self::redirect_or_continue(
                    input_middleware_result,
                    &session,
                    || async {
                        let response = self
                            .handle_file_server_binding::<R>(
                                binding,
                                resolved_file_server_binding,
                                &session,
                            )
                            .await;
                        Ok(response)
                    },
                    resolved_file_server_binding.middlewares.clone(),
                )
                .await
            }
        }
    }
}