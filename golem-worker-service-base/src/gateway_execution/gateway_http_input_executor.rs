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
use crate::gateway_api_deployment::ApiSiteString;
use crate::gateway_binding::{
    resolve_gateway_binding, ErrorOrRedirect, HttpRequestDetails, ResolvedBinding, ResolvedHttpHandlerBinding, ResolvedWorkerBinding, StaticBinding
};
use crate::gateway_execution::api_definition_lookup::HttpApiDefinitionsLookup;
use crate::gateway_execution::auth_call_back_binding_handler::{
    AuthCallBackBindingHandler, AuthCallBackResult,
};
use crate::gateway_execution::file_server_binding_handler::FileServerBindingHandler;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_execution::http_handler_binding_handler::HttpHandlerBindingError;
use crate::gateway_execution::to_response::{GatewayHttpError, ToHttpResponse};
use crate::gateway_execution::to_response_failure::ToHttpResponseFromSafeDisplay;
use crate::gateway_middleware::HttpMiddlewares;
use crate::gateway_request::http_request::{ErrorResponse, InputHttpRequest};
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use crate::gateway_security::{IdentityProvider, SecuritySchemeWithProviderMetadata};
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::SafeDisplay;
use http::{request, StatusCode};
use poem::Body;
use poem_openapi::error::AuthorizationError;
use rib::{RibInput, RibInputTypeInfo, RibResult};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::error;

use super::file_server_binding_handler::{FileServerBindingResult, FileServerBindingSuccess};
use super::http_handler_binding_handler::{HttpHandlerBindingHandler, HttpHandlerBindingResult};
use super::to_response::GatewayHttpResult;

#[async_trait]
pub trait GatewayHttpInputExecutor {
    async fn execute_http_request(&self, input: poem::Request) -> poem::Response;
}

pub struct DefaultGatewayInputExecutor<Namespace> {
    pub evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
    pub file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
    pub auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
    pub http_handler_binding_handler: Arc<dyn HttpHandlerBindingHandler<Namespace> + Sync + Send>,
    pub api_definition_lookup_service: Arc<dyn HttpApiDefinitionsLookup<Namespace> + Sync + Send>,
    pub gateway_session_store: GatewaySessionStore,
    pub identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
}

impl<Namespace: Clone> DefaultGatewayInputExecutor<Namespace> {
    pub fn new(
        evaluator: Arc<dyn WorkerServiceRibInterpreter<Namespace> + Sync + Send>,
        file_server_binding_handler: Arc<dyn FileServerBindingHandler<Namespace> + Sync + Send>,
        auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler + Sync + Send>,
        http_handler_binding_handler: Arc<dyn HttpHandlerBindingHandler<Namespace> + Sync + Send>,
        api_definition_lookup_service: Arc<dyn HttpApiDefinitionsLookup<Namespace> + Sync + Send>,
        gateway_session_store: GatewaySessionStore,
        identity_provider: Arc<dyn IdentityProvider + Send + Sync>,
    ) -> Self {
        Self {
            evaluator,
            file_server_binding_handler,
            auth_call_back_binding_handler,
            http_handler_binding_handler,
            api_definition_lookup_service,
            gateway_session_store,
            identity_provider,
        }
    }

    async fn execute(
        &self,
        http_request_details: &mut HttpRequestDetails,
        middlewares: Option<HttpMiddlewares>,
        binding: ResolvedBinding<Namespace>,
    ) -> poem::Response {
        match &binding {
            ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                cors_preflight
                    .clone()
                    .to_response(http_request_details, &self.gateway_session_store)
                    .await
            }

            ResolvedBinding::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
                let result = self.handle_http_auth_call_binding(
                    &auth_call_back.security_scheme_with_metadata,
                    http_request_details,
                )
                .await;

                result.to_response(http_request_details, &self.gateway_session_store).await
            }

            ResolvedBinding::Worker(resolved_worker_binding) => {
                let result = self
                    .handle_worker_binding(
                        http_request_details,
                        resolved_worker_binding,
                    )
                    .await;

                let mut response = result.to_response(http_request_details, &self.gateway_session_store).await;

                if let Some(middlewares) = middlewares {
                    let result = middlewares.process_middleware_out(&mut response).await;
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

            ResolvedBinding::HttpHandler(http_handler_binding) => {
                let result = self.handle_http_handler_binding(http_request_details, http_handler_binding).await;
                let mut response = result.to_response(http_request_details, &self.gateway_session_store).await;

                if let Some(middlewares) = middlewares {
                    let result = middlewares.process_middleware_out(&mut response).await;
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
                let result = self.handle_file_server_binding(
                    http_request_details,
                    resolved_file_server_binding,
                )
                .await;

                result.to_response(http_request_details, &self.gateway_session_store).await
            }
        }
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
        request_details: &mut HttpRequestDetails,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> GatewayHttpResult<RibResult> {
        let (rib_input_from_request_details, rib_input_from_worker_details) =
            resolve_response_mapping_rib_inputs(request_details, resolved_binding).await?;

        self
            .get_rib_result(
                rib_input_from_request_details,
                rib_input_from_worker_details,
                resolved_binding,
            )
            .await
            .map_err(GatewayHttpError::EvaluationError)
    }

    async fn handle_http_handler_binding(
        &self,
        request_details: &mut HttpRequestDetails,
        http_handler_binding: &ResolvedHttpHandlerBinding<Namespace>,
    ) -> GatewayHttpResult<HttpHandlerBindingResult> {
        let inner_request = &mut request_details.underlying;
        let incoming_http_request = {
            use golem_common::virtual_exports::http_incoming_handler as hic;

            let headers = {
                let mut acc = Vec::new();
                for (header_name, header_value) in inner_request.headers().iter() {
                    let header_bytes: Vec<u8> = header_value.as_bytes().into();
                    acc.push((
                        header_name.clone().to_string(),
                        Bytes::from(header_bytes),
                    ));
                }
                hic::HttpFields(acc)
            };

            let body_bytes = inner_request
                .take_body()
                .into_bytes()
                .await
                .map_err(|e| GatewayHttpError::BadRequest(format!("Failed reading request body: ${e}")))?;

            let body = hic::HttpBodyAndTrailers {
                content: hic::HttpBodyContent(Bytes::from(body_bytes)),
                trailers: None,
            };

            let authority = authority_from_request(&inner_request)?;

            let path_and_query = path_and_query_from_request(&inner_request)?;

            hic::IncomingHttpRequest {
                scheme: inner_request.scheme().clone().into(),
                authority,
                path_and_query,
                method: hic::HttpMethod::from_http_method(inner_request.method().into()),
                headers,
                body: Some(body),
            }
        };

        let result = self
            .http_handler_binding_handler
            .handle_http_handler_binding(
                &http_handler_binding.namespace,
                &http_handler_binding.worker_detail,
                incoming_http_request,
            )
            .await;

        match result {
            Ok(_) => tracing::debug!("http handler binding successful"),
            Err(ref e) => tracing::warn!("http handler binding failed: {e:?}"),
        }

        Ok(result)

        // result
        //     .to_response(&request_details, &self.gateway_session_store)
        //     .await
    }

    async fn handle_file_server_binding(
        &self,
        request_details: &mut HttpRequestDetails,
        resolved_binding: &ResolvedWorkerBinding<Namespace>,
    ) -> GatewayHttpResult<FileServerBindingSuccess> {
        let (request_rib_input, worker_rib_input) = resolve_response_mapping_rib_inputs(request_details, resolved_binding).await?;

        let worker_response = self
            .get_rib_result(request_rib_input, worker_rib_input, resolved_binding)
            .await
            .map_err(GatewayHttpError::EvaluationError)?;

        self.file_server_binding_handler
            .handle_file_server_binding_result(
                &resolved_binding.namespace,
                &resolved_binding.worker_detail,
                worker_response,
            )
            .await
            .map_err(GatewayHttpError::FileServerBindingError)
    }

    async fn handle_http_auth_call_binding(
        &self,
        security_scheme_with_metadata: &SecuritySchemeWithProviderMetadata,
        http_request: &HttpRequestDetails,
    ) -> GatewayHttpResult<AuthCallBackResult>
    where
        AuthCallBackResult: ToHttpResponse,
    {
        let url = http_request.url()
            .map_err(|e| GatewayHttpError::BadRequest(format!("Failed converting url {e}")))?;

        let authorisation_result = self
            .auth_call_back_binding_handler
            .handle_auth_call_back(
                &url,
                security_scheme_with_metadata,
                &self.gateway_session_store,
                &self.identity_provider,
            )
            .await;

        Ok(authorisation_result)

        // authorisation_result
        //     .to_response(http_request, &self.gateway_session_store)
        //     .await
    }
}

#[async_trait]
impl<Namespace: Send + Sync + Clone + 'static> GatewayHttpInputExecutor
    for DefaultGatewayInputExecutor<Namespace>
{
    async fn execute_http_request(&self, request: poem::Request) -> poem::Response {

        //         let host_header = self.underlying.header(http::header::HOST).map(|h| h.to_string())
        //     .ok_or("No host header provided".to_string())?;
        // Ok(ApiSiteString(host_header))

        // let api_site_string = match request.api_site_string() {
        //     Ok(ass) => ass,
        //     Err(e) => {
        //         return poem::Response::builder().status(StatusCode::BAD_REQUEST).body(Body::from_string(e))
        //     },
        // };

        let authority = authority_from_request(&request)?;

        let possible_api_definitions = self
            .api_definition_lookup_service
            .get(&ApiSiteString(authority))
            .await;

        let possible_api_definitions = match possible_api_definitions {
            Ok(api_defs) => api_defs,
            Err(api_defs_lookup_error) => {
                error!(
                    "API request host: {} - error: {}",
                    authority, api_defs_lookup_error
                );
                return poem::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from_string("Internal error".to_string()));
            }
        };

        match resolve_gateway_binding(
            &self.gateway_session_store,
            &self.identity_provider,
            possible_api_definitions,
            request,
        )
            .await
        {
            Ok(resolved_gateway_binding) => {
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
}

async fn resolve_response_mapping_rib_inputs<Namespace>(
    request_details: &mut HttpRequestDetails,
    resolved_worker_binding: &ResolvedWorkerBinding<Namespace>,
) -> GatewayHttpResult<(RibInput, RibInput)> {
    let rib_input_from_request_details = request_details
        .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
        .await
        .map_err(GatewayHttpError::RibInputTypeMismatch)?;

    let rib_input_from_worker_details = resolved_worker_binding
        .worker_detail
        .resolve_rib_input_value(&resolved_worker_binding.compiled_response_mapping.rib_input)
        .map_err(GatewayHttpError::RibInputTypeMismatch)?;

    Ok((
        rib_input_from_request_details,
        rib_input_from_worker_details,
    ))
}

fn authority_from_request(request: &poem::Request) -> GatewayHttpResult<String> {
    request.header(http::header::HOST).map(|h| h.to_string())
        .ok_or(GatewayHttpError::BadRequest("No host header provided".to_string()))
}

fn path_and_query_from_request(request: &poem::Request) -> GatewayHttpResult<String> {
    request.uri().path_and_query().map(|paq| paq.to_string())
        .ok_or(GatewayHttpError::BadRequest("No path and query provided".to_string()))
}
