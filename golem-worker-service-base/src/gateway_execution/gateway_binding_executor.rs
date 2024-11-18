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
use crate::gateway_execution::to_response_failure::ToResponseFromSafeDisplay;
use crate::gateway_middleware::{
    Cors as CorsPreflight, HttpRequestAuthentication, MiddlewareIn, MiddlewareInError,
    MiddlewareOut, MiddlewareOutError, MiddlewareSuccess, Middlewares,
};
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use crate::gateway_security::SecuritySchemeWithProviderMetadata;
use async_trait::async_trait;
use http::StatusCode;
use rib::{RibInput, RibResult};
use std::fmt::Debug;
use std::sync::Arc;

// Response is type parameterised here, mainly to support
// other protocols.
// Every error and result-types involved in the workflow
// need to have an instance of ToResponse<ResponseType> where
// ResponseType depends on the protocol
// The workflow doesn't need to be changed for each protocol
#[async_trait]
pub trait GatewayBindingExecutor<Namespace, Response> {
    async fn execute_binding(
        &self,
        binding: &ResolvedGatewayBinding<Namespace>,
        session: GatewaySessionStore,
    ) -> Response
    where
        EvaluationError: ToResponseFromSafeDisplay<Response>,
        RibInputTypeMismatch: ToResponseFromSafeDisplay<Response>,
        MiddlewareInError: ToResponseFromSafeDisplay<Response>,
        MiddlewareOutError: ToResponseFromSafeDisplay<Response>,
        RibResult: ToResponse<Response>,
        FileServerBindingResult: ToResponse<Response>,
        CorsPreflight: ToResponse<Response>,
        AuthCallBackResult: ToResponse<Response>,
        HttpRequestAuthentication: MiddlewareIn<Response>,
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
        RibInputTypeMismatch: ToResponseFromSafeDisplay<R>,
    {
        let request_rib_input = request_details
            .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
            .map_err(|err| err.to_response_from_safe_display(|_| StatusCode::BAD_REQUEST))?;

        let worker_rib_input = resolved_worker_binding
            .worker_detail
            .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
            .map_err(|err| err.to_response_from_safe_display(|_| StatusCode::BAD_REQUEST))?;

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
        EvaluationError: ToResponseFromSafeDisplay<R>,
        RibInputTypeMismatch: ToResponseFromSafeDisplay<R>,
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
                    Err(err) => {
                        err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR)
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
        EvaluationError: ToResponseFromSafeDisplay<R>,
        RibInputTypeMismatch: ToResponseFromSafeDisplay<R>,
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
                    Err(err) => {
                        err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }
            }
            Err(err_response) => err_response,
        }
    }

    async fn handle_http_auth_call_binding<R>(
        &self,
        binding: &ResolvedGatewayBinding<N>,
        security_scheme_with_metadata: &SecuritySchemeWithProviderMetadata,
        session_store: &GatewaySessionStore,
    ) -> R
    where
        AuthCallBackResult: ToResponse<R>,
    {
        match &binding.request_details {
            GatewayRequestDetails::Http(http_request) => {
                let authorisation_result = self
                    .auth_call_back_binding_handler
                    .handle_auth_call_back(
                        http_request,
                        security_scheme_with_metadata,
                        session_store,
                    )
                    .await;

                authorisation_result
                    .to_response(&binding.request_details, session_store)
                    .await
            }
        }
    }

    async fn redirect_or_continue<R>(
        request_details: &GatewayRequestDetails,
        session: &GatewaySessionStore,
        middlewares: Middlewares,
    ) -> Option<R>
    where
        HttpRequestAuthentication: MiddlewareIn<R>,
        CorsPreflight: MiddlewareOut<R>,
        MiddlewareInError: ToResponseFromSafeDisplay<R>,
    {
        let input_middleware_result = middlewares
            .process_middleware_in(session, request_details)
            .await;

        match input_middleware_result {
            Ok(incoming_middleware_result) => match incoming_middleware_result {
                MiddlewareSuccess::Redirect(response) => Some(response),
                MiddlewareSuccess::PassThrough => None,
            },

            Err(err) => {
                Some(err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR))
            }
        }
    }
}

#[async_trait]
impl<N: Send + Sync + Clone, R: Debug + Send + Sync> GatewayBindingExecutor<N, R>
    for DefaultGatewayBindingExecutor<N>
{
    async fn execute_binding(
        &self,
        binding: &ResolvedGatewayBinding<N>,
        session: GatewaySessionStore,
    ) -> R
    where
        RibResult: ToResponse<R>,
        EvaluationError: ToResponseFromSafeDisplay<R>,
        RibInputTypeMismatch: ToResponseFromSafeDisplay<R>,
        MiddlewareInError: ToResponseFromSafeDisplay<R>,
        MiddlewareOutError: ToResponseFromSafeDisplay<R>,
        FileServerBindingResult: ToResponse<R>, // FileServerBindingResult can be a direct response in a file server endpoint
        CorsPreflight: ToResponse<R>, // Cors can be a direct response in a cors preflight endpoint
        AuthCallBackResult: ToResponse<R>, // AuthCallBackResult can be a direct response in auth callback endpoint
        HttpRequestAuthentication: MiddlewareIn<R>, // HttpAuthorizer can authorise input
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
                    &auth_call_back.security_scheme,
                    &session,
                )
                .await
            }

            ResolvedBinding::Worker(resolved_worker_binding) => {
                let result = Self::redirect_or_continue(
                    &binding.request_details,
                    &session,
                    resolved_worker_binding.middlewares.clone(),
                )
                .await;

                match result {
                    Some(r) => r,
                    None => {
                        let mut response = self
                            .handle_worker_binding::<R>(binding, resolved_worker_binding, &session)
                            .await;

                        let middleware_out_result = resolved_worker_binding
                            .middlewares
                            .process_middleware_out(&session, &mut response)
                            .await;

                        match middleware_out_result {
                            Ok(_) => response,
                            Err(err) => err.to_response_from_safe_display(|_| {
                                StatusCode::INTERNAL_SERVER_ERROR
                            }),
                        }
                    }
                }
            }

            ResolvedBinding::FileServer(resolved_file_server_binding) => {
                let result = Self::redirect_or_continue(
                    &binding.request_details,
                    &session,
                    resolved_file_server_binding.middlewares.clone(),
                )
                .await;

                match result {
                    Some(r) => r,
                    None => {
                        self.handle_file_server_binding::<R>(
                            binding,
                            resolved_file_server_binding,
                            &session,
                        )
                        .await
                    }
                }
            }
        }
    }
}
