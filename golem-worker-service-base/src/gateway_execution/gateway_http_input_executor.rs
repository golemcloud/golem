// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_binding::{
    HttpRequestDetails, ResolvedBinding, ResolvedWorkerBinding, RibInputTypeMismatch,
    RibInputValueResolver, StaticBinding,
};
use crate::gateway_execution::auth_call_back_binding_handler::{
    AuthCallBackBindingHandler, AuthCallBackResult,
};
use crate::gateway_execution::file_server_binding_handler::{
    FileServerBindingHandler, FileServerBindingResult,
};
use crate::gateway_execution::gateway_session::{GatewaySession, GatewaySessionStore, SessionId};
use crate::gateway_execution::to_response::ToHttpResponse;
use crate::gateway_execution::to_response_failure::ToHttpResponseFromSafeDisplay;
use crate::gateway_middleware::{
    HttpCors as CorsPreflight, HttpMiddlewares, MiddlewareError, MiddlewareSuccess,
};
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use crate::gateway_security::{IdentityProvider, SecuritySchemeWithProviderMetadata};
use async_trait::async_trait;
use http::StatusCode;
use rib::{RibInput, RibResult};
use std::sync::Arc;

// Response is type parameterised here, mainly to support
// other protocols.
// Every error and result-types involved in the workflow
// need to have an instance of ToResponse<ResponseType> where
// ResponseType depends on the protocol
// The workflow doesn't need to be changed for each protocol
#[async_trait]
pub trait GatewayHttpInputExecutor<Namespace> {
    async fn execute_binding(&self, input: &GatewayHttpInput<Namespace>) -> poem::Response
    where
        EvaluationError: ToHttpResponseFromSafeDisplay,
        RibInputTypeMismatch: ToHttpResponseFromSafeDisplay,
        MiddlewareError: ToHttpResponseFromSafeDisplay,
        RibResult: ToHttpResponse,
        FileServerBindingResult: ToHttpResponse,
        CorsPreflight: ToHttpResponse,
        AuthCallBackResult: ToHttpResponse;
}

// A product of actual request input (contained in the ResolvedGatewayBinding)
// and other details and resolvers that are needed to process the input.
pub struct GatewayHttpInput<Namespace> {
    pub http_request_details: HttpRequestDetails,
    pub resolved_gateway_binding: ResolvedBinding<Namespace>,
    pub session_store: Arc<dyn GatewaySession + Send + Sync>,
    pub identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
}

impl<Namespace: Clone> GatewayHttpInput<Namespace> {
    pub fn new(
        http_request_details: &HttpRequestDetails,
        resolved_gateway_binding: ResolvedBinding<Namespace>,
        session_store: GatewaySessionStore,
        identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
    ) -> Self {
        GatewayHttpInput {
            http_request_details: http_request_details.clone(),
            resolved_gateway_binding,
            session_store,
            identity_provider,
        }
    }
}

pub struct DefaultGatewayInputExecutor<Namespace> {
    pub evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
    pub file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
    pub auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
}

impl<Namespace: Clone> DefaultGatewayInputExecutor<Namespace> {
    pub fn new(
        evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
        file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
        auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
    ) -> Self {
        Self {
            evaluator,
            file_server_binding_handler,
            auth_call_back_binding_handler,
        }
    }

    async fn resolve_rib_inputs(
        &self,
        session_id: Option<SessionId>,
        session_store: &GatewaySessionStore,
        request_details: &mut HttpRequestDetails,
        resolved_worker_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> Result<(RibInput, RibInput), poem::Response>
    where
        RibInputTypeMismatch: ToHttpResponseFromSafeDisplay,
    {
        if let Some(session_id) = session_id {
            let result = request_details
                .inject_auth_details(&session_id, session_store)
                .await;

            if let Err(err_response) = result {
                return Err(poem::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(err_response));
            }
        }

        let rib_input_from_request_details = request_details
            .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
            .map_err(|err| err.to_response_from_safe_display(|_| StatusCode::BAD_REQUEST))?;

        let rib_input_from_worker_details = resolved_worker_binding
            .worker_detail
            .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
            .map_err(|err| err.to_response_from_safe_display(|_| StatusCode::BAD_REQUEST))?;

        Ok((
            rib_input_from_request_details,
            rib_input_from_worker_details,
        ))
    }

    async fn get_rib_result(
        &self,
        request_rib_input: RibInput,
        worker_rib_input: RibInput,
        resolved_worker_binding: &ResolvedWorkerBinding<Namespace>,
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
                resolved_worker_binding.namespace.clone(),
            )
            .await
    }

    async fn handle_worker_binding(
        &self,
        session_id: Option<SessionId>,
        session_store: &GatewaySessionStore,
        request_details: &mut HttpRequestDetails,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> poem::Response
    where
        RibResult: ToHttpResponse,
        EvaluationError: ToHttpResponseFromSafeDisplay,
        RibInputTypeMismatch: ToHttpResponseFromSafeDisplay,
    {
        match self
            .resolve_rib_inputs(session_id, session_store, request_details, resolved_binding)
            .await
        {
            Ok((rib_input_from_request_details, rib_input_from_worker_details)) => {
                match self
                    .get_rib_result(
                        rib_input_from_request_details,
                        rib_input_from_worker_details,
                        resolved_binding,
                    )
                    .await
                {
                    Ok(result) => result.to_response(request_details, session_store).await,
                    Err(err) => {
                        err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR)
                    }
                }
            }
            Err(err_response) => err_response,
        }
    }

    async fn handle_file_server_binding(
        &self,
        session_id: Option<SessionId>,
        session_store: &GatewaySessionStore,
        request_details: &mut HttpRequestDetails,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> poem::Response
    where
        RibResult: ToHttpResponse,
        EvaluationError: ToHttpResponseFromSafeDisplay,
        RibInputTypeMismatch: ToHttpResponseFromSafeDisplay,
        FileServerBindingResult: ToHttpResponse,
    {
        match self
            .resolve_rib_inputs(session_id, session_store, request_details, resolved_binding)
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
                            .to_response(request_details, session_store)
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

    async fn handle_http_auth_call_binding(
        &self,
        security_scheme_with_metadata: &SecuritySchemeWithProviderMetadata,
        input: &GatewayHttpInput<Namespace>,
    ) -> poem::Response
    where
        AuthCallBackResult: ToHttpResponse,
    {
        let http_request = &input.http_request_details;
        let authorisation_result = self
            .auth_call_back_binding_handler
            .handle_auth_call_back(
                http_request,
                security_scheme_with_metadata,
                &input.session_store,
                &input.identity_provider,
            )
            .await;

        authorisation_result
            .to_response(&input.http_request_details, &input.session_store)
            .await
    }

    // If redirects, it responds with Err(response)
    // or else it returns the session_id to continue with the rest.
    async fn redirect_or_continue(
        input: &GatewayHttpInput<Namespace>,
        middlewares: &HttpMiddlewares,
    ) -> Result<Option<SessionId>, poem::Response>
    where
        MiddlewareError: ToHttpResponseFromSafeDisplay,
    {
        let input_middleware_result = middlewares.process_middleware_in(input).await;

        match input_middleware_result {
            Ok(incoming_middleware_result) => match incoming_middleware_result {
                MiddlewareSuccess::Redirect(response) => Err(response),
                MiddlewareSuccess::PassThrough { session_id } => Ok(session_id),
            },

            Err(err) => Err(err.to_response_from_safe_display(|error| match error {
                MiddlewareError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
                MiddlewareError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            })),
        }
    }
}

#[async_trait]
impl<Namespace: Send + Sync + Clone> GatewayHttpInputExecutor<Namespace>
    for DefaultGatewayInputExecutor<Namespace>
{
    async fn execute_binding(&self, input: &GatewayHttpInput<Namespace>) -> poem::Response
    where
        RibResult: ToHttpResponse,
        EvaluationError: ToHttpResponseFromSafeDisplay,
        RibInputTypeMismatch: ToHttpResponseFromSafeDisplay,
        MiddlewareError: ToHttpResponseFromSafeDisplay,
        FileServerBindingResult: ToHttpResponse, // FileServerBindingResult can be a direct response in a file server endpoint
        CorsPreflight: ToHttpResponse, // Cors can be a direct response in a cors preflight endpoint
        AuthCallBackResult: ToHttpResponse, // AuthCallBackResult can be a direct response in auth callback endpoint
    {
        let binding = &input.resolved_gateway_binding;
        let middleware_opt = &input.http_request_details.http_middlewares;
        let mut request_details = input.http_request_details.clone();

        let middleware_response = if let Some(middleware) = middleware_opt {
            Self::redirect_or_continue(input, middleware).await
        } else {
            Ok(None)
        };

        match middleware_response {
            Err(redirect_response) => redirect_response,
            Ok(session) => match &binding {
                ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                    cors_preflight
                        .clone()
                        .to_response(&input.http_request_details, &input.session_store)
                        .await
                }

                ResolvedBinding::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
                    self.handle_http_auth_call_binding(
                        &auth_call_back.security_scheme_with_metadata,
                        input,
                    )
                    .await
                }

                ResolvedBinding::Worker(resolved_worker_binding) => {
                    let mut response = self
                        .handle_worker_binding(
                            session,
                            &input.session_store,
                            &mut request_details,
                            resolved_worker_binding,
                        )
                        .await;

                    if let Some(middleware) = middleware_opt {
                        let result = middleware.process_middleware_out(&mut response).await;
                        match result {
                            Ok(_) => response,
                            Err(err) => err.to_response_from_safe_display(|_| {
                                StatusCode::INTERNAL_SERVER_ERROR
                            }),
                        }
                    } else {
                        response
                    }
                }

                ResolvedBinding::FileServer(resolved_file_server_binding) => {
                    self.handle_file_server_binding(
                        session,
                        &input.session_store,
                        &mut request_details,
                        resolved_file_server_binding,
                    )
                    .await
                }
            },
        }
    }
}
