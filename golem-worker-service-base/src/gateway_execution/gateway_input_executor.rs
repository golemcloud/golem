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
use crate::gateway_security::{IdentityProviderResolver, SecuritySchemeWithProviderMetadata};
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
pub trait GatewayInputExecutor<Namespace, Response> {
    async fn execute_binding(&self, input: &Input<Namespace>) -> Response
    where
        EvaluationError: ToResponseFromSafeDisplay<Response>,
        RibInputTypeMismatch: ToResponseFromSafeDisplay<Response>,
        MiddlewareInError: ToResponseFromSafeDisplay<Response>,
        MiddlewareOutError: ToResponseFromSafeDisplay<Response>,
        RibResult: ToResponse<Response>,
        FileServerBindingResult: ToResponse<Response>,
        CorsPreflight: ToResponse<Response>,
        AuthCallBackResult: ToResponse<Response>,
        HttpRequestAuthentication: MiddlewareIn<Namespace, Response>,
        CorsPreflight: MiddlewareOut<Response>;
}

// A product of actual request input (contained in the ResolvedGatewayBinding)
// and other details and resolvers that are needed to process the input.
pub struct Input<Namespace> {
    pub resolved_gateway_binding: ResolvedGatewayBinding<Namespace>,
    pub session_store: GatewaySessionStore,
    pub identity_provider_resolver: Arc<dyn IdentityProviderResolver + Send + Sync>,
}

impl<Namespace: Clone> Input<Namespace> {
    pub fn new(
        resolved_gateway_binding: &ResolvedGatewayBinding<Namespace>,
        session_store: &GatewaySessionStore,
        identity_provider_resolver: Arc<dyn IdentityProviderResolver + Send + Sync>,
    ) -> Self {
        Input {
            resolved_gateway_binding: resolved_gateway_binding.clone(),
            session_store: session_store.clone(),
            identity_provider_resolver,
        }
    }
}

pub struct DefaultGatewayBindingExecutor<Namespace> {
    pub evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
    pub file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
    pub auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
}

impl<Namespace: Clone> DefaultGatewayBindingExecutor<Namespace> {
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

    async fn resolve_rib_inputs<R>(
        &self,
        request_details: &GatewayRequestDetails,
        resolved_worker_binding: &ResolvedWorkerBinding<Namespace>,
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

    async fn handle_worker_binding<R>(
        &self,
        request_details: &GatewayRequestDetails,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
        session_store: &GatewaySessionStore,
    ) -> R
    where
        RibResult: ToResponse<R>,
        EvaluationError: ToResponseFromSafeDisplay<R>,
        RibInputTypeMismatch: ToResponseFromSafeDisplay<R>,
    {
        match self
            .resolve_rib_inputs(&request_details, resolved_binding)
            .await
        {
            Ok((request_rib_input, worker_rib_input)) => {
                match self
                    .get_rib_result(request_rib_input, worker_rib_input, resolved_binding)
                    .await
                {
                    Ok(result) => result.to_response(&request_details, session_store).await,
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
        request_details: &GatewayRequestDetails,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
        session_store: &GatewaySessionStore,
    ) -> R
    where
        FileServerBindingResult: ToResponse<R>,
        RibResult: ToResponse<R>,
        EvaluationError: ToResponseFromSafeDisplay<R>,
        RibInputTypeMismatch: ToResponseFromSafeDisplay<R>,
    {
        match self
            .resolve_rib_inputs(&request_details, resolved_binding)
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
                            .to_response(&request_details, session_store)
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

    async fn handle_http_auth_call_binding<Response>(
        &self,
        security_scheme_with_metadata: &SecuritySchemeWithProviderMetadata,
        input: &Input<Namespace>,
    ) -> Response
    where
        AuthCallBackResult: ToResponse<Response>,
    {
        match &input.resolved_gateway_binding.request_details {
            GatewayRequestDetails::Http(http_request) => {
                let authorisation_result = self
                    .auth_call_back_binding_handler
                    .handle_auth_call_back(
                        &http_request,
                        security_scheme_with_metadata,
                        &input.session_store,
                        &input.identity_provider_resolver,
                    )
                    .await;

                authorisation_result
                    .to_response(
                        &input.resolved_gateway_binding.request_details,
                        &input.session_store,
                    )
                    .await
            }
        }
    }

    async fn redirect_or_continue<Namespace, Response>(
        input: &Input<Namespace>,
        middlewares: &Middlewares,
    ) -> Option<Response>
    where
        HttpRequestAuthentication: MiddlewareIn<Namespace, Response>,
        CorsPreflight: MiddlewareOut<Response>,
        MiddlewareInError: ToResponseFromSafeDisplay<Response>,
    {
        let input_middleware_result = middlewares.process_middleware_in(input).await;

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
impl<Namespace: Send + Sync + Clone, Response: Debug + Send + Sync>
    GatewayInputExecutor<Namespace, Response> for DefaultGatewayBindingExecutor<Namespace>
{
    async fn execute_binding(&self, input: &Input<Namespace>) -> Response
    where
        RibResult: ToResponse<Response>,
        EvaluationError: ToResponseFromSafeDisplay<Response>,
        RibInputTypeMismatch: ToResponseFromSafeDisplay<Response>,
        MiddlewareInError: ToResponseFromSafeDisplay<Response>,
        MiddlewareOutError: ToResponseFromSafeDisplay<Response>,
        FileServerBindingResult: ToResponse<Response>, // FileServerBindingResult can be a direct response in a file server endpoint
        CorsPreflight: ToResponse<Response>, // Cors can be a direct response in a cors preflight endpoint
        AuthCallBackResult: ToResponse<Response>, // AuthCallBackResult can be a direct response in auth callback endpoint
        HttpRequestAuthentication: MiddlewareIn<Namespace, Response>, // HttpAuthorizer can authorise input
        CorsPreflight: MiddlewareOut<Response>, // CorsPreflight can be a middleware in other endpoints
    {
        let binding = &input.resolved_gateway_binding.resolved_binding;
        let request_details = &input.resolved_gateway_binding.request_details;
        match &binding {
            ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                cors_preflight
                    .clone()
                    .to_response(&request_details, &input.session_store)
                    .await
            }

            ResolvedBinding::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
                self.handle_http_auth_call_binding(&auth_call_back.security_scheme, &input)
                    .await
            }

            ResolvedBinding::Worker(resolved_worker_binding) => {
                let result =
                    Self::redirect_or_continue(&input, &resolved_worker_binding.middlewares).await;

                match result {
                    Some(r) => r,
                    None => {
                        let mut response = self
                            .handle_worker_binding::<Response>(
                                &request_details,
                                resolved_worker_binding,
                                &input.session_store,
                            )
                            .await;

                        let middleware_out_result = resolved_worker_binding
                            .middlewares
                            .process_middleware_out(&input.session_store, &mut response)
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
                let result =
                    Self::redirect_or_continue(&input, &resolved_file_server_binding.middlewares)
                        .await;

                match result {
                    Some(r) => r,
                    None => {
                        self.handle_file_server_binding::<Response>(
                            request_details,
                            resolved_file_server_binding,
                            &input.session_store,
                        )
                        .await
                    }
                }
            }
        }
    }
}
