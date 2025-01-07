// Copyright 2024-2025 Golem Cloud
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

use crate::gateway_api_definition::http::CompiledHttpApiDefinition;
use crate::gateway_binding::{
    DefaultGatewayBindingResolver, ErrorOrRedirect, GatewayBindingResolver, GatewayRequestDetails,
    HttpRequestDetails, ResolvedBinding, ResolvedWorkerBinding, RibInputValueResolver,
    StaticBinding,
};
use crate::gateway_execution::api_definition_lookup::ApiDefinitionsLookup;
use crate::gateway_execution::auth_call_back_binding_handler::{
    AuthCallBackBindingHandler, AuthCallBackResult,
};
use crate::gateway_execution::file_server_binding_handler::FileServerBindingHandler;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_execution::to_response::ToHttpResponse;
use crate::gateway_execution::to_response_failure::ToHttpResponseFromSafeDisplay;
use crate::gateway_request::http_request::InputHttpRequest;
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use crate::gateway_security::{IdentityProvider, SecuritySchemeWithProviderMetadata};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use http::StatusCode;
use poem::Body;
use rib::{RibInput, RibResult};
use std::sync::Arc;
use tracing::error;

#[async_trait]
pub trait GatewayHttpInputExecutor {
    async fn execute_http_request(&self, input: poem::Request) -> poem::Response;
}

pub struct DefaultGatewayInputExecutor<Namespace> {
    pub evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
    pub file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
    pub auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
    pub api_definition_lookup_service: Arc<
        dyn ApiDefinitionsLookup<
                InputHttpRequest,
                ApiDefinition = CompiledHttpApiDefinition<Namespace>,
            > + Sync
            + Send,
    >,
    pub gateway_session_store: GatewaySessionStore,
    pub identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
}

impl<Namespace: Clone> DefaultGatewayInputExecutor<Namespace> {
    pub fn new(
        evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
        file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
        auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
        api_definition_lookup_service: Arc<
            dyn ApiDefinitionsLookup<
                    InputHttpRequest,
                    ApiDefinition = CompiledHttpApiDefinition<Namespace>,
                > + Sync
                + Send,
        >,
        gateway_session_store: GatewaySessionStore,
        identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
    ) -> Self {
        Self {
            evaluator,
            file_server_binding_handler,
            auth_call_back_binding_handler,
            api_definition_lookup_service,
            gateway_session_store,
            identity_provider,
        }
    }

    pub async fn execute(
        &self,
        http_request_details: &HttpRequestDetails,
        binding: ResolvedBinding<Namespace>,
    ) -> poem::Response {
        let middleware_opt = &http_request_details.http_middlewares;
        let mut request_details = http_request_details.clone();

        match &binding {
            ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                cors_preflight
                    .clone()
                    .to_response(http_request_details, &self.gateway_session_store)
                    .await
            }

            ResolvedBinding::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
                self.handle_http_auth_call_binding(
                    &auth_call_back.security_scheme_with_metadata,
                    http_request_details,
                )
                .await
            }

            ResolvedBinding::Worker(resolved_worker_binding) => {
                let mut response = self
                    .handle_worker_binding(
                        &self.gateway_session_store,
                        &mut request_details,
                        resolved_worker_binding,
                    )
                    .await;

                if let Some(middleware) = middleware_opt {
                    let result = middleware.process_middleware_out(&mut response).await;
                    match result {
                        Ok(_) => response,
                        Err(err) => {
                            err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR)
                        }
                    }
                } else {
                    response
                }
            }

            ResolvedBinding::FileServer(resolved_file_server_binding) => {
                self.handle_file_server_binding(
                    &self.gateway_session_store,
                    &mut request_details,
                    resolved_file_server_binding,
                )
                .await
            }
        }
    }

    async fn resolve_rib_inputs(
        &self,
        request_details: &HttpRequestDetails,
        resolved_worker_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> Result<(RibInput, RibInput), poem::Response> {
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
        session_store: &GatewaySessionStore,
        request_details: &mut HttpRequestDetails,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> poem::Response {
        match self
            .resolve_rib_inputs(request_details, resolved_binding)
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
        session_store: &GatewaySessionStore,
        request_details: &mut HttpRequestDetails,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> poem::Response {
        match self
            .resolve_rib_inputs(request_details, resolved_binding)
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
        http_request: &HttpRequestDetails,
    ) -> poem::Response
    where
        AuthCallBackResult: ToHttpResponse,
    {
        let authorisation_result = self
            .auth_call_back_binding_handler
            .handle_auth_call_back(
                http_request,
                security_scheme_with_metadata,
                &self.gateway_session_store,
                &self.identity_provider,
            )
            .await;

        authorisation_result
            .to_response(http_request, &self.gateway_session_store)
            .await
    }
}

#[async_trait]
impl<Namespace: Send + Sync + Clone + 'static> GatewayHttpInputExecutor
    for DefaultGatewayInputExecutor<Namespace>
{
    async fn execute_http_request(&self, request: poem::Request) -> poem::Response {
        let input_http_request_result = InputHttpRequest::from_request(request).await;

        match input_http_request_result {
            Ok(input_http_request) => {
                let possible_api_definitions = match self
                    .api_definition_lookup_service
                    .get(&input_http_request)
                    .await
                {
                    Ok(api_defs) => api_defs,
                    Err(api_defs_lookup_error) => {
                        error!(
                            "API request host: {} - error: {}",
                            input_http_request.host, api_defs_lookup_error
                        );
                        return poem::Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Body::from_string("Internal error".to_string()));
                    }
                };

                let resolver = DefaultGatewayBindingResolver::new(
                    input_http_request,
                    &self.gateway_session_store,
                    &self.identity_provider,
                );

                match resolver
                    .resolve_gateway_binding(possible_api_definitions)
                    .await
                {
                    Ok(resolved_gateway_binding) => {
                        let GatewayRequestDetails::Http(request) =
                            resolved_gateway_binding.request_details;

                        let response: poem::Response = self
                            .execute(&request, resolved_gateway_binding.resolved_binding)
                            .await;

                        response
                    }

                    Err(ErrorOrRedirect::Error(error)) => {
                        error!(
                            "Failed to resolve the API definition; error: {}",
                            error.to_safe_string()
                        );

                        error.to_http_response()
                    }

                    Err(ErrorOrRedirect::Redirect(response)) => response,
                }
            }
            Err(response) => response.into(),
        }
    }
}
