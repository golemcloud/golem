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

use crate::gateway_api_definition::http::{CompiledHttpApiDefinition, QueryInfo, VarInfo};
use crate::gateway_api_deployment::ApiSiteString;
use crate::gateway_binding::{
    resolve_gateway_binding, GatewayBindingCompiled, HttpHandlerBindingCompiled, HttpRequestDetails, IdempotencyKeyCompiled, RequestBodyValue, RequestHeaderValues, RequestPathValues, RequestQueryValues, ResolvedBinding, ResponseMappingCompiled, StaticBinding, WorkerBindingCompiled, WorkerNameCompiled
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
use crate::gateway_request::http_request::router::PathParamExtractor;
use crate::gateway_request::http_request::{ErrorResponse, InputHttpRequest};
use crate::gateway_rib_interpreter::{EvaluationError, WorkerServiceRibInterpreter};
use crate::gateway_security::{IdentityProvider, SecuritySchemeWithProviderMetadata};
use async_trait::async_trait;
use bytes::Bytes;
use golem_service_base::model::VersionedComponentId;
use golem_common::model::IdempotencyKey;
use golem_common::SafeDisplay;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::ValueAndType;
use serde_json::Value;
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
use super::WorkerDetail;

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

    // async fn execute(
    //     &self,
    //     http_request_details: &mut HttpRequestDetails,
    //     middlewares: Option<HttpMiddlewares>,
    //     binding: ResolvedBinding<Namespace>,
    // ) -> poem::Response {
    //     match &binding {
    //         ResolvedBinding::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
    //             cors_preflight
    //                 .clone()
    //                 .to_response(http_request_details, &self.gateway_session_store)
    //                 .await
    //         }

    //         ResolvedBinding::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
    //             let result = self.handle_http_auth_call_binding(
    //                 &auth_call_back.security_scheme_with_metadata,
    //                 http_request_details,
    //             )
    //             .await;

    //             result.to_response(http_request_details, &self.gateway_session_store).await
    //         }

    //         ResolvedBinding::Worker(resolved_worker_binding) => {
    //             let result = self
    //                 .handle_worker_binding(
    //                     http_request_details,
    //                     resolved_worker_binding,
    //                 )
    //                 .await;

    //             let mut response = result.to_response(http_request_details, &self.gateway_session_store).await;

    //             if let Some(middlewares) = middlewares {
    //                 let result = middlewares.process_middleware_out(&mut response).await;
    //                 match result {
    //                     Ok(_) => response,
    //                     Err(err) => {
    //                         err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR)
    //                     }
    //                 }
    //             } else {
    //                 response
    //             }
    //         }

    //         ResolvedBinding::HttpHandler(http_handler_binding) => {
    //             let result = self.handle_http_handler_binding(http_request_details, http_handler_binding).await;
    //             let mut response = result.to_response(http_request_details, &self.gateway_session_store).await;

    //             if let Some(middlewares) = middlewares {
    //                 let result = middlewares.process_middleware_out(&mut response).await;
    //                 match result {
    //                     Ok(_) => response,
    //                     Err(err) => {
    //                         err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR)
    //                     }
    //                 }
    //             } else {
    //                 response
    //             }
    //         }

    //         ResolvedBinding::FileServer(resolved_file_server_binding) => {
    //             let result = self.handle_file_server_binding(
    //                 http_request_details,
    //                 resolved_file_server_binding,
    //             )
    //             .await;

    //             result.to_response(http_request_details, &self.gateway_session_store).await
    //         }
    //     }
    // }

    async fn evaluate_worker_name_rib_script(
        &self,
        script: &WorkerNameCompiled,
        request_value: &serde_json::Map<String, Value>,
    ) -> String {
        let rib_input: RibInput = resolve_rib_input(request_value, &script.rib_input_type_info).await.unwrap();

        rib::interpret_pure(
            &script.compiled_worker_name,
            &rib_input,
        )
        .await
        .unwrap()
        .get_literal()
        .unwrap()
        .as_string()
    }

    async fn evaluate_idempotency_key_rib_script(
        &self,
        script: &IdempotencyKeyCompiled,
        request_value: &serde_json::Map<String, Value>
    ) -> IdempotencyKey {
        let rib_input: RibInput = resolve_rib_input(request_value, &script.rib_input).await.unwrap();

        let value = rib::interpret_pure(
            &script.compiled_idempotency_key,
            &rib_input,
        )
        .await
        .unwrap()
        .get_literal()
        .unwrap()
        .as_string();

        IdempotencyKey::new(value)
    }

    async fn get_worker_detail(
        &self,
        request: &RichRequest,
        request_value: &serde_json::Map<String, Value>,
        worker_name_compiled: &Option<WorkerNameCompiled>,
        idempotency_key_compiled: &Option<IdempotencyKeyCompiled>,
        component_id: &VersionedComponentId
    ) -> WorkerDetail {
        let worker_name = if let Some(worker_name_compiled) = worker_name_compiled {
            let result = self.evaluate_worker_name_rib_script(worker_name_compiled, request_value).await;
            Some(result)
        } else {
            None
        };

        // We prefer to take idempotency key from the rib script,
        // if that is not available we fall back to our custom header.
        // If neither are available, the worker-executor will later generate an idempotency key.
        let idempotency_key = if let Some(idempotency_key_compiled) = idempotency_key_compiled {
            let result = self.evaluate_idempotency_key_rib_script(idempotency_key_compiled, request_value).await;
            Some(result)
        } else {
            request.underlying
                .headers()
                .get("idempotency-key")
                .and_then(|h| h.to_str().ok())
                .map(|value| IdempotencyKey::new(value.to_string()))
        };

        WorkerDetail {
            component_id: component_id.clone(),
            worker_name,
            idempotency_key
        }
    }

    async fn get_response_script_result(
        &self,
        namespace: &Namespace,
        compiled_response_mapping: &ResponseMappingCompiled,
        request_value: &serde_json::Map<String, Value>,
        worker_detail: &WorkerDetail
    ) -> Result<RibResult, EvaluationError> {
        let rib_input = resolve_rib_input(&request_value, &compiled_response_mapping.rib_input).await.unwrap();

        self.evaluator
            .evaluate(
                worker_detail.worker_name.as_deref(),
                &worker_detail
                    .component_id
                    .component_id,
                &worker_detail.idempotency_key,
                &compiled_response_mapping.response_mapping_compiled,
                &rib_input,
                namespace.clone(),
            )
            .await
    }


    async fn handle_worker_binding(
        &self,
        namespace: &Namespace,
        request: &mut RichRequest,
        binding: &WorkerBindingCompiled,
    ) -> GatewayHttpResult<RibResult> {
        let mut rib_input: serde_json::Map<String, Value> = serde_json::Map::new();

        // phase 1. we only have the request details available
        {
            let request_value = request.as_json_with_body().await.unwrap();
            rib_input.insert("request".to_string(), request_value);
        }

        let worker_detail = self.get_worker_detail(
            request,
            &rib_input,
            &binding.worker_name_compiled,
            &binding.idempotency_key_compiled,
            &binding.component_id
        ).await;

        // phase 2. we have both the request and the worker details available
        {
            let worker_value: Value = worker_detail.as_json();
            rib_input.insert("worker".to_string(), worker_value);
        }

        self
            .get_response_script_result(
                namespace,
                &binding.response_compiled,
                &rib_input,
                &worker_detail
            )
            .await
            .map_err(GatewayHttpError::EvaluationError)
    }

    async fn handle_http_handler_binding(
        &self,
        namespace: &Namespace,
        request: &mut RichRequest,
        binding: &HttpHandlerBindingCompiled,
    ) -> GatewayHttpResult<HttpHandlerBindingResult> {
        let mut rib_input: serde_json::Map<String, Value> = serde_json::Map::new();

        {
            let request_value = request.as_json().unwrap();
            rib_input.insert("request".to_string(), request_value);
        }

        let worker_detail = self.get_worker_detail(
            request,
            &rib_input,
            &binding.worker_name_compiled,
            &binding.idempotency_key_compiled,
            &binding.component_id
        ).await;

        let incoming_http_request = request.as_wasi_http_input().await.unwrap();

        let result = self
            .http_handler_binding_handler
            .handle_http_handler_binding(
                namespace,
                &worker_detail,
                incoming_http_request,
            )
            .await;

        match result {
            Ok(_) => tracing::debug!("http handler binding successful"),
            Err(ref e) => tracing::warn!("http handler binding failed: {e:?}"),
        }

        Ok(result)
    }

    async fn handle_file_server_binding(
        &self,
        namespace: &Namespace,
        request: &mut RichRequest,
        binding: &WorkerBindingCompiled, // TODO make separate type
    ) -> GatewayHttpResult<FileServerBindingSuccess> {
        let mut rib_input: serde_json::Map<String, Value> = serde_json::Map::new();

        // phase 1. we only have the request details available
        {
            let request_value = request.as_json_with_body().await.unwrap();
            rib_input.insert("request".to_string(), request_value);
        }

        let worker_detail = self.get_worker_detail(
            request,
            &rib_input,
            &binding.worker_name_compiled,
            &binding.idempotency_key_compiled,
            &binding.component_id
        ).await;

        // phase 2. we have both the request and the worker details available
        {
            let worker_value: Value = worker_detail.as_json();
            rib_input.insert("worker".to_string(), worker_value);
        }

        let response_script_result = self
            .get_response_script_result(
                namespace,
                &binding.response_compiled,
                &rib_input,
                &worker_detail
            )
            .await
            .map_err(GatewayHttpError::EvaluationError)?;

        self.file_server_binding_handler
            .handle_file_server_binding_result(
                &namespace,
                &worker_detail,
                response_script_result,
            )
            .await
            .map_err(GatewayHttpError::FileServerBindingError)
    }

    async fn handle_http_auth_call_binding(
        &self,
        security_scheme_with_metadata: &SecuritySchemeWithProviderMetadata,
        request: &RichRequest,
    ) -> GatewayHttpResult<AuthCallBackResult>
    where
        AuthCallBackResult: ToHttpResponse,
    {
        let url = request.url()
            .map_err(|e| GatewayHttpError::BadRequest(format!("Failed getting url: {e}")))?;

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

        let resolved_gateway_binding = if let Some(resolved_gateway_binding) = resolve_gateway_binding(possible_api_definitions, &request).await {
            resolved_gateway_binding
        } else {
            return poem::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from_string("Route not found".to_string()))
        };

        let rich_request = RichRequest(request);

        match resolved_gateway_binding.route_entry.binding {
            GatewayBindingCompiled::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                cors_preflight
                    .clone()
                    .to_response(http_request_details, &self.gateway_session_store)
                    .await
            }

            GatewayBindingCompiled::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
                let result = self.handle_http_auth_call_binding(
                    &auth_call_back.security_scheme_with_metadata,
                    &rich_request,
                )
                .await;

                result.to_response(http_request_details, &self.gateway_session_store).await
            }

            GatewayBindingCompiled::Worker(resolved_worker_binding) => {
                let result = self
                    .handle_worker_binding(
                        &mut rich_request,
                        &resolved_worker_binding,
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

            GatewayBindingCompiled::HttpHandler(http_handler_binding) => {
                let result = self.handle_http_handler_binding(&mut rich_request, &http_handler_binding).await;
                let mut response = result.to_response(&rich_request, &self.gateway_session_store).await;

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

            GatewayBindingCompiled::FileServer(resolved_file_server_binding) => {
                let result = self.handle_file_server_binding(
                    &mut rich_request,
                    &resolved_file_server_binding,
                )
                .await;

                result.to_response(http_request_details, &self.gateway_session_store).await
            }
        }

        // match resolved_gateway_binding.

    //     match resolve_gateway_binding(possible_api_definitions, request)
    //         .await
    //     {
    //         Ok(resolved_gateway_binding) => {
    //             let response: poem::Response = self
    //                 .execute(&request, resolved_gateway_binding.resolved_binding)
    //                 .await;

    //             response
    //         }

    //         Err(ErrorOrRedirect::Error(error)) => {
    //             error!(
    //                 "Failed to resolve the API definition; error: {}",
    //                 error.to_safe_string()
    //             );

    //             error.to_http_response()
    //         }

    //         Err(ErrorOrRedirect::Redirect(response)) => response,
    //     }
    // }
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

async fn resolve_rib_input(
    input: &serde_json::Map<String, Value>,
    required_types: &RibInputTypeInfo,
) -> Result<RibInput, String> {

    let mut result_map: HashMap<String, ValueAndType> = HashMap::new();

    for (key, analysed_type) in required_types.types.iter() {
        let input_value = input.get(key).ok_or(format!("Required input not available: {key}"))?;

        let parsed_value = TypeAnnotatedValue::parse_with_type(
            input_value,
            &analysed_type
        ).map_err(|err| format!("Input {key} doesn't match the requirements for rib expression to execute: {}. Requirements. {:?}", err.join(", "), analysed_type))?;

        let converted_value = parsed_value.try_into().map_err(|err| format!("Internal error converting between value representations: {err}"))?;

        result_map.insert(key.clone(), converted_value);
    }

    Ok(RibInput {
        input: result_map,
    })
}

fn authority_from_request(request: &poem::Request) -> GatewayHttpResult<String> {
    request.header(http::header::HOST).map(|h| h.to_string())
        .ok_or(GatewayHttpError::BadRequest("No host header provided".to_string()))
}

fn path_and_query_from_request(request: &poem::Request) -> GatewayHttpResult<String> {
    request.uri().path_and_query().map(|paq| paq.to_string())
        .ok_or(GatewayHttpError::BadRequest("No path and query provided".to_string()))
}

struct RichRequest {
    pub underlying: poem::Request,
    pub path_segments: Vec<String>,
    pub path_param_extractors: Vec<PathParamExtractor>,
    pub query_info: Vec<QueryInfo>,
    pub request_custom_params: Option<HashMap<String, Value>>,
}

impl RichRequest {

    pub fn url(&self) -> Result<url::Url, String> {
        url::Url::parse(&self.underlying.uri().to_string()).map_err(|e| format!("Failed parsing url: {e}"))
    }

    fn authority(&self) -> Result<String, String> {
        self.underlying.header(http::header::HOST).map(|h| h.to_string())
            .ok_or("No host header provided".to_string())
    }

    fn path_and_query(&self) -> Result<String, String> {
        self.underlying.uri().path_and_query().map(|paq| paq.to_string())
            .ok_or("No path and query provided".to_string())
    }

    fn request_path_values(&self) -> RequestPathValues {
        use crate::gateway_request::http_request::router;

        let path_param_values: HashMap<VarInfo, String> = self.path_param_extractors
            .iter()
            .map(|param| match param {
                router::PathParamExtractor::Single { var_info, index } => {
                    (var_info.clone(), self.path_segments[*index].clone())
                }
                router::PathParamExtractor::AllFollowing { var_info, index } => {
                    let value = self.path_segments[*index..].join("/");
                    (var_info.clone(), value)
                }
            })
            .collect();

        RequestPathValues::from(path_param_values)
    }

    fn request_query_values(&self) -> Result<RequestQueryValues, String> {
        let query_key_values = self.underlying.uri().query().map(query_components_from_str).unwrap_or_default();

        RequestQueryValues::from(&query_key_values, &self.query_info)
            .map_err(|e| format!("Failed to extract query values, missing: [{}]", e.join(",")))
    }

    fn request_header_values(&self) -> Result<RequestHeaderValues, String> {
        RequestHeaderValues::from(self.underlying.headers())
            .map_err(|e| format!("Found malformed headers: [{}]", e.join(",")))
    }

    /// consumes the body of the underlying request
    async fn request_body_value(&mut self) -> Result<RequestBodyValue, String> {
        let body = self.underlying.take_body();

        let json_request_body: Value = if body.is_empty() {
            Value::Null
        } else {
            match body.into_json().await {
                Ok(json_request_body) => json_request_body,
                Err(err) => {
                    tracing::error!("Failed reading http request body as json: {}", err);
                    Err(format!("Request body parse error: {err}"))?
                }
            }
        };

        Ok(RequestBodyValue(json_request_body))
    }

    fn as_basic_json_hashmap(&self) -> Result<serde_json::Map<String, Value>, String> {
        let typed_path_values = self.request_path_values();
        let typed_query_values = self.request_query_values()?;
        let typed_header_values = self.request_header_values()?;
        // let typed_body_value: RequestBodyValue = self.request_body_value().await?;

        let mut path_values = serde_json::Map::new();

        for field in typed_path_values.0.fields.into_iter() {
            path_values.insert(field.name, field.value);
        }

        for field in typed_query_values.0.fields.into_iter() {
            path_values.insert(field.name, field.value);
        }

        let merged_request_path_and_query = Value::Object(path_values);

        let mut header_records = serde_json::Map::new();

        for field in typed_header_values.0.fields.iter() {
            header_records.insert(field.name.clone(), field.value.clone());
        }

        let header_value = Value::Object(header_records);

        let mut basic = serde_json::Map::from_iter(vec![
            ("path".to_string(), merged_request_path_and_query),
            // ("body".to_string(), typed_body_value.0),
            ("headers".to_string(), header_value),
        ]);

        let custom = self.request_custom_params.clone().unwrap_or_default();

        for (key, value) in custom.iter() {
            basic.insert(key.clone(), value.clone());
        }

        Ok(basic)
    }


    fn as_json(&self) -> Result<Value, String> {
        Ok(Value::Object(self.as_basic_json_hashmap()?))
    }

    /// consumes the body of the underlying request
    async fn as_json_with_body(&mut self) -> Result<Value, String> {
        let mut basic = self.as_basic_json_hashmap()?;
        let body = self.request_body_value().await?;

        basic.insert("body".to_string(), body.0);

        Ok(Value::Object(basic))
    }

    async fn as_wasi_http_input(&self) -> Result<golem_common::virtual_exports::http_incoming_handler::IncomingHttpRequest, String> {
        use golem_common::virtual_exports::http_incoming_handler as hic;

        let headers = {
            let mut acc = Vec::new();
            for (header_name, header_value) in self.underlying.headers().iter() {
                let header_bytes: Vec<u8> = header_value.as_bytes().into();
                acc.push((
                    header_name.clone().to_string(),
                    Bytes::from(header_bytes),
                ));
            }
            hic::HttpFields(acc)
        };

        let body_bytes = self.underlying
            .take_body()
            .into_bytes()
            .await
            .map_err(|e| GatewayHttpError::BadRequest(format!("Failed reading request body: ${e}")))?;

        let body = hic::HttpBodyAndTrailers {
            content: hic::HttpBodyContent(Bytes::from(body_bytes)),
            trailers: None,
        };

        let authority = self.authority()?;

        let path_and_query = self.path_and_query()?;

        Ok(hic::IncomingHttpRequest {
            scheme: self.underlying.scheme().clone().into(),
            authority,
            path_and_query,
            method: hic::HttpMethod::from_http_method(self.underlying.method().into()),
            headers,
            body: Some(body),
        })
    }

}

fn query_components_from_str(query_path: &str) -> HashMap<String, String> {
    let mut query_components: HashMap<String, String> = HashMap::new();
    let query_parts = query_path.split('&').map(|x| x.trim());

    for part in query_parts {
        let key_value: Vec<&str> = part.split('=').map(|x| x.trim()).collect();

        if let (Some(key), Some(value)) = (key_value.first(), key_value.get(1)) {
            query_components.insert(key.to_string(), value.to_string());
        }
    }

    query_components
}
