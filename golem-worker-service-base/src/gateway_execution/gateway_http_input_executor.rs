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

use super::auth_call_back_binding_handler::AuthorisationSuccess;
use super::file_server_binding_handler::FileServerBindingSuccess;
use super::http_handler_binding_handler::{HttpHandlerBindingHandler, HttpHandlerBindingResult};
use super::request::{
    authority_from_request, split_resolved_route_entry, RichRequest, SplitResolvedRouteEntryResult,
};
use super::to_response::GatewayHttpResult;
use super::WorkerDetail;
use crate::gateway_api_deployment::ApiSiteString;
use crate::gateway_binding::{
    resolve_gateway_binding, GatewayBindingCompiled, HttpHandlerBindingCompiled,
    IdempotencyKeyCompiled, InvocationContextCompiled, ResponseMappingCompiled, StaticBinding,
    WorkerBindingCompiled, WorkerNameCompiled,
};
use crate::gateway_execution::api_definition_lookup::{
    ApiDefinitionLookupError, HttpApiDefinitionsLookup,
};
use crate::gateway_execution::auth_call_back_binding_handler::AuthCallBackBindingHandler;
use crate::gateway_execution::file_server_binding_handler::FileServerBindingHandler;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_execution::to_response::{GatewayHttpError, ToHttpResponse};
use crate::gateway_execution::to_response_failure::ToHttpResponseFromSafeDisplay;
use crate::gateway_middleware::{HttpMiddlewares, MiddlewareError, MiddlewareSuccess};
use crate::gateway_rib_interpreter::WorkerServiceRibInterpreter;
use crate::gateway_security::{IdentityProvider, SecuritySchemeWithProviderMetadata};
use crate::http_invocation_context::{extract_request_attributes, invocation_context_from_request};
use crate::service::gateway::api_deployment::ApiDeploymentError;
use async_trait::async_trait;
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId, TraceId,
};
use golem_common::model::IdempotencyKey;
use golem_common::SafeDisplay;
use golem_service_base::headers::TraceContextHeaders;
use golem_service_base::model::VersionedComponentId;
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::ValueAndType;
use http::StatusCode;
use poem::Body;
use rib::{RibInput, RibInputTypeInfo, RibResult};
use serde_json::Value;
use std::collections::HashMap;
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

    async fn handle_worker_binding(
        &self,
        namespace: &Namespace,
        request: &mut RichRequest,
        binding: &WorkerBindingCompiled,
    ) -> GatewayHttpResult<RibResult> {
        let mut rib_input: serde_json::Map<String, Value> = serde_json::Map::new();

        // phase 1. we only have the request details available
        {
            let request_value = request
                .as_json_with_body()
                .await
                .map_err(GatewayHttpError::BadRequest)?;
            rib_input.insert("request".to_string(), request_value);
        }

        let worker_detail = self
            .get_worker_detail(
                request,
                &rib_input,
                &binding.worker_name_compiled,
                &binding.idempotency_key_compiled,
                &binding.component_id,
                &binding.invocation_context_compiled,
            )
            .await?;

        // phase 2. we have both the request and the worker details available
        {
            let worker_value: Value = worker_detail.as_json();
            rib_input.insert("worker".to_string(), worker_value);
        }

        self.get_response_script_result(
            namespace,
            &binding.response_compiled,
            &rib_input,
            &worker_detail,
        )
        .await
    }

    async fn handle_http_handler_binding(
        &self,
        namespace: &Namespace,
        request: &mut RichRequest,
        binding: &HttpHandlerBindingCompiled,
    ) -> GatewayHttpResult<HttpHandlerBindingResult> {
        let mut rib_input: serde_json::Map<String, Value> = serde_json::Map::new();

        {
            let request_value = request.as_json().map_err(GatewayHttpError::BadRequest)?;
            rib_input.insert("request".to_string(), request_value);
        }

        let worker_detail = self
            .get_worker_detail(
                request,
                &rib_input,
                &binding.worker_name_compiled,
                &binding.idempotency_key_compiled,
                &binding.component_id,
                &None,
            )
            .await?;

        let incoming_http_request = request
            .as_wasi_http_input()
            .await
            .map_err(GatewayHttpError::BadRequest)?;

        let result = self
            .http_handler_binding_handler
            .handle_http_handler_binding(namespace, &worker_detail, incoming_http_request)
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
            let request_value = request
                .as_json_with_body()
                .await
                .map_err(GatewayHttpError::BadRequest)?;
            rib_input.insert("request".to_string(), request_value);
        }

        let worker_detail = self
            .get_worker_detail(
                request,
                &rib_input,
                &binding.worker_name_compiled,
                &binding.idempotency_key_compiled,
                &binding.component_id,
                &None,
            )
            .await?;

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
                &worker_detail,
            )
            .await?;

        self.file_server_binding_handler
            .handle_file_server_binding_result(namespace, &worker_detail, response_script_result)
            .await
            .map_err(GatewayHttpError::FileServerBindingError)
    }

    async fn handle_http_auth_callback_binding(
        &self,
        security_scheme_with_metadata: &SecuritySchemeWithProviderMetadata,
        request: &RichRequest,
    ) -> GatewayHttpResult<AuthorisationSuccess> {
        self.auth_call_back_binding_handler
            .handle_auth_call_back(
                &request.query_params(),
                security_scheme_with_metadata,
                &self.gateway_session_store,
                &self.identity_provider,
            )
            .await
            .map_err(GatewayHttpError::AuthorisationError)
    }

    async fn evaluate_worker_name_rib_script(
        &self,
        script: &WorkerNameCompiled,
        request_value: &serde_json::Map<String, Value>,
    ) -> GatewayHttpResult<String> {
        let rib_input: RibInput = resolve_rib_input(request_value, &script.rib_input_type_info)
            .await
            .map_err(GatewayHttpError::BadRequest)?;

        let result = rib::interpret_pure(&script.compiled_worker_name, &rib_input)
            .await
            .map_err(GatewayHttpError::RibInterpretPureError)?
            .get_literal()
            .ok_or(GatewayHttpError::BadRequest(
                "Worker name is not a Rib expression that resolves to String".to_string(),
            ))?
            .as_string();

        Ok(result)
    }

    async fn evaluate_idempotency_key_rib_script(
        &self,
        script: &IdempotencyKeyCompiled,
        request_value: &serde_json::Map<String, Value>,
    ) -> GatewayHttpResult<IdempotencyKey> {
        let rib_input: RibInput = resolve_rib_input(request_value, &script.rib_input)
            .await
            .map_err(GatewayHttpError::BadRequest)?;

        let value = rib::interpret_pure(&script.compiled_idempotency_key, &rib_input)
            .await
            .map_err(GatewayHttpError::RibInterpretPureError)?
            .get_literal()
            .ok_or(GatewayHttpError::BadRequest(
                "Idempotency key is not a Rib expression that resolves to String".to_string(),
            ))?
            .as_string();

        Ok(IdempotencyKey::new(value))
    }

    async fn evaluate_invocation_context_rib_script(
        &self,
        script: &InvocationContextCompiled,
        request_value: &serde_json::Map<String, Value>,
    ) -> GatewayHttpResult<(Option<TraceId>, HashMap<String, ValueAndType>)> {
        let rib_input: RibInput = resolve_rib_input(request_value, &script.rib_input)
            .await
            .map_err(GatewayHttpError::BadRequest)?;

        let value = rib::interpret_pure(&script.compiled_invocation_context, &rib_input)
            .await
            .map_err(GatewayHttpError::RibInterpretPureError)?
            .get_record()
            .ok_or(GatewayHttpError::BadRequest(
                "Invocation context must be a Rib expression that resolves to record".to_string(),
            ))?;
        let record: HashMap<String, ValueAndType> = HashMap::from_iter(value);

        let trace_id = record
            .get("trace_id")
            .or(record.get("trace-id"))
            .map(to_attribute_value)
            .transpose()?
            .map(TraceId::from_attribute_value)
            .transpose()
            .map_err(|err| GatewayHttpError::BadRequest(format!("Invalid Trace ID: {err}")))?;

        Ok((trace_id, record))
    }

    fn materialize_user_invocation_context(
        record: HashMap<String, ValueAndType>,
        parent: Option<Arc<InvocationContextSpan>>,
        request_attributes: HashMap<String, AttributeValue>,
    ) -> GatewayHttpResult<Arc<InvocationContextSpan>> {
        let span_id = record
            .get("span_id")
            .or(record.get("span-id"))
            .map(to_attribute_value)
            .transpose()?
            .map(SpanId::from_attribute_value)
            .transpose()
            .map_err(|err| GatewayHttpError::BadRequest(format!("Invalid Span ID: {err}")))?;

        let span = InvocationContextSpan::local()
            .span_id(span_id)
            .parent(parent)
            .with_attributes(request_attributes)
            .build();

        for (key, value) in record {
            if key != "span_id" && key != "span-id" && key != "trace_id" && key != "trace-id" {
                span.set_attribute(key, to_attribute_value(&value)?);
            }
        }

        Ok(span)
    }

    async fn get_worker_detail(
        &self,
        request: &RichRequest,
        request_value: &serde_json::Map<String, Value>,
        worker_name_compiled: &Option<WorkerNameCompiled>,
        idempotency_key_compiled: &Option<IdempotencyKeyCompiled>,
        component_id: &VersionedComponentId,
        invocation_context_compiled: &Option<InvocationContextCompiled>,
    ) -> GatewayHttpResult<WorkerDetail> {
        let worker_name = if let Some(worker_name_compiled) = worker_name_compiled {
            let result = self
                .evaluate_worker_name_rib_script(worker_name_compiled, request_value)
                .await?;
            Some(result)
        } else {
            None
        };

        // We prefer to take idempotency key from the rib script,
        // if that is not available we fall back to our custom header.
        // If neither are available, the worker-executor will later generate an idempotency key.
        let idempotency_key = if let Some(idempotency_key_compiled) = idempotency_key_compiled {
            let result = self
                .evaluate_idempotency_key_rib_script(idempotency_key_compiled, request_value)
                .await?;
            Some(result)
        } else {
            request
                .underlying
                .headers()
                .get("idempotency-key")
                .and_then(|h| h.to_str().ok())
                .map(|value| IdempotencyKey::new(value.to_string()))
        };

        let invocation_context = if let Some(invocation_context_compiled) =
            invocation_context_compiled
        {
            let request_attributes = extract_request_attributes(&request.underlying);

            let trace_context_headers = TraceContextHeaders::parse(request.underlying.headers());

            let (user_defined_trace_id, user_defined_span) = self
                .evaluate_invocation_context_rib_script(invocation_context_compiled, request_value)
                .await?;

            match (trace_context_headers, &user_defined_trace_id) {
                (Some(ctx), None) => {
                    // Trace context found in headers and not overridden, starting a new span in it
                    let mut ctx = InvocationContextStack::new(
                        ctx.trace_id,
                        InvocationContextSpan::external_parent(ctx.parent_id),
                        ctx.trace_states,
                    );
                    let user_defined_span = Self::materialize_user_invocation_context(
                        user_defined_span,
                        Some(ctx.spans.first().clone()),
                        request_attributes,
                    )?;
                    ctx.push(user_defined_span);
                    ctx
                }
                (_, Some(trace_id)) => {
                    // Forced a new trace, ignoring the trace context in the headers
                    let user_defined_span = Self::materialize_user_invocation_context(
                        user_defined_span,
                        None,
                        request_attributes,
                    )?;
                    InvocationContextStack::new(trace_id.clone(), user_defined_span, Vec::new())
                }
                (None, _) => {
                    // No trace context in headers, starting a new trace
                    let user_defined_span = Self::materialize_user_invocation_context(
                        user_defined_span,
                        None,
                        request_attributes,
                    )?;
                    InvocationContextStack::new(
                        user_defined_trace_id.unwrap_or_else(TraceId::generate),
                        user_defined_span,
                        Vec::new(),
                    )
                }
            }
        } else {
            invocation_context_from_request(&request.underlying)
        };

        Ok(WorkerDetail {
            component_id: component_id.clone(),
            worker_name,
            idempotency_key,
            invocation_context,
        })
    }

    async fn get_response_script_result(
        &self,
        namespace: &Namespace,
        compiled_response_mapping: &ResponseMappingCompiled,
        request_value: &serde_json::Map<String, Value>,
        worker_detail: &WorkerDetail,
    ) -> GatewayHttpResult<RibResult> {
        let rib_input = resolve_rib_input(request_value, &compiled_response_mapping.rib_input)
            .await
            .map_err(GatewayHttpError::BadRequest)?;

        self.evaluator
            .evaluate(
                worker_detail.worker_name.as_deref(),
                &worker_detail.component_id.component_id,
                &worker_detail.idempotency_key,
                worker_detail.invocation_context.clone(),
                &compiled_response_mapping.response_mapping_compiled,
                &rib_input,
                namespace.clone(),
            )
            .await
            .map_err(GatewayHttpError::EvaluationError)
    }

    async fn maybe_apply_middlewares_in(
        &self,
        mut request: RichRequest,
        middlewares: &Option<HttpMiddlewares>,
    ) -> Result<RichRequest, poem::Response> {
        if let Some(middlewares) = middlewares {
            let input_middleware_result = middlewares
                .process_middleware_in(
                    &request,
                    &self.gateway_session_store,
                    &self.identity_provider,
                )
                .await;

            let input_middleware_result = match input_middleware_result {
                Ok(MiddlewareSuccess::PassThrough {
                    session_id: session_id_opt,
                }) => {
                    if let Some(session_id) = session_id_opt.as_ref() {
                        let result = request
                            .add_auth_details(session_id, &self.gateway_session_store)
                            .await;

                        if let Err(err_response) = result {
                            Err(MiddlewareError::InternalError(err_response))
                        } else {
                            Ok(MiddlewareSuccess::PassThrough {
                                session_id: session_id_opt,
                            })
                        }
                    } else {
                        Ok(MiddlewareSuccess::PassThrough {
                            session_id: session_id_opt,
                        })
                    }
                }
                other => other,
            };

            match input_middleware_result {
                Ok(MiddlewareSuccess::Redirect(response)) => Err(response)?,
                Ok(MiddlewareSuccess::PassThrough { .. }) => Ok(request),
                Err(err) => {
                    let response = err.to_response_from_safe_display(|error| match error {
                        MiddlewareError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
                        MiddlewareError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
                    });
                    Err(response)?
                }
            }
        } else {
            Ok(request)
        }
    }
}

#[async_trait]
impl<Namespace: Send + Sync + Clone + 'static> GatewayHttpInputExecutor
    for DefaultGatewayInputExecutor<Namespace>
{
    async fn execute_http_request(&self, request: poem::Request) -> poem::Response {
        let authority = match authority_from_request(&request) {
            Ok(success) => success,
            Err(err) => {
                return poem::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::from_string(err));
            }
        };

        let possible_api_definitions = self
            .api_definition_lookup_service
            .get(&ApiSiteString(authority.clone()))
            .await;

        let possible_api_definitions = match possible_api_definitions {
            Ok(api_defs) => api_defs,
            Err(api_defs_lookup_error) => {
                error!(
                    "API request host: {} - error: {}",
                    authority,
                    api_defs_lookup_error.to_safe_string()
                );

                return api_defs_lookup_error
                    .to_response_from_safe_display(get_status_code_from_api_lookup_error);
            }
        };

        let resolved_route_entry = if let Some(resolved_route_entry) =
            resolve_gateway_binding(possible_api_definitions, &request).await
        {
            resolved_route_entry
        } else {
            return poem::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from_string("Route not found".to_string()));
        };

        let SplitResolvedRouteEntryResult {
            namespace,
            binding,
            middlewares,
            rich_request,
        } = split_resolved_route_entry(request, resolved_route_entry);

        let mut rich_request = match self
            .maybe_apply_middlewares_in(rich_request, &middlewares)
            .await
        {
            Ok(req) => req,
            Err(resp) => {
                tracing::debug!("Middleware short-circuited the request handling");
                return resp;
            }
        };

        match binding {
            GatewayBindingCompiled::Static(StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                cors_preflight
                    .clone()
                    .to_response(&rich_request, &self.gateway_session_store)
                    .await
            }

            GatewayBindingCompiled::Static(StaticBinding::HttpAuthCallBack(auth_call_back)) => {
                let result = self
                    .handle_http_auth_callback_binding(
                        &auth_call_back.security_scheme_with_metadata,
                        &rich_request,
                    )
                    .await;

                result
                    .to_response(&rich_request, &self.gateway_session_store)
                    .await
            }

            GatewayBindingCompiled::Worker(resolved_worker_binding) => {
                let result = self
                    .handle_worker_binding(&namespace, &mut rich_request, &resolved_worker_binding)
                    .await;

                let response = result
                    .to_response(&rich_request, &self.gateway_session_store)
                    .await;

                maybe_apply_middlewares_out(response, &middlewares).await
            }

            GatewayBindingCompiled::HttpHandler(http_handler_binding) => {
                let result = self
                    .handle_http_handler_binding(
                        &namespace,
                        &mut rich_request,
                        &http_handler_binding,
                    )
                    .await;

                let response = result
                    .to_response(&rich_request, &self.gateway_session_store)
                    .await;

                maybe_apply_middlewares_out(response, &middlewares).await
            }

            GatewayBindingCompiled::FileServer(resolved_file_server_binding) => {
                let result = self
                    .handle_file_server_binding(
                        &namespace,
                        &mut rich_request,
                        &resolved_file_server_binding,
                    )
                    .await;

                let response = result
                    .to_response(&rich_request, &self.gateway_session_store)
                    .await;

                maybe_apply_middlewares_out(response, &middlewares).await
            }
        }
    }
}

async fn resolve_rib_input(
    input: &serde_json::Map<String, Value>,
    required_types: &RibInputTypeInfo,
) -> Result<RibInput, String> {
    let mut result_map: HashMap<String, ValueAndType> = HashMap::new();

    for (key, analysed_type) in required_types.types.iter() {
        let input_value = input
            .get(key)
            .ok_or(format!("Required input not available: {key}"))?;

        let parsed_value = TypeAnnotatedValue::parse_with_type(
            input_value,
            analysed_type,
        ).map_err(|err| format!("Input {key} doesn't match the requirements for rib expression to execute: {}. Requirements. {:?}", err.join(", "), analysed_type))?;

        let converted_value = parsed_value.try_into().map_err(|err| {
            format!("Internal error converting between value representations: {err}")
        })?;

        result_map.insert(key.clone(), converted_value);
    }

    Ok(RibInput { input: result_map })
}

async fn maybe_apply_middlewares_out(
    mut response: poem::Response,
    middlewares: &Option<HttpMiddlewares>,
) -> poem::Response {
    if let Some(middlewares) = middlewares {
        let result = middlewares.process_middleware_out(&mut response).await;
        match result {
            Ok(_) => response,
            Err(err) => err.to_response_from_safe_display(|_| StatusCode::INTERNAL_SERVER_ERROR),
        }
    } else {
        response
    }
}

fn to_attribute_value(value: &ValueAndType) -> GatewayHttpResult<AttributeValue> {
    match &value.value {
        golem_wasm_rpc::Value::String(value) => Ok(AttributeValue::String(value.clone())),
        _ => Err(GatewayHttpError::BadRequest(
            "Invocation context values must be string".to_string(),
        )),
    }
}

fn get_status_code_from_api_lookup_error<Namespace>(
    error: &ApiDefinitionLookupError<Namespace>,
) -> StatusCode {
    match &error {
        ApiDefinitionLookupError::ApiDeploymentError(err) => {
            // In the context of APIDefinitionLookup (which occurs for an actual incoming request),
            // we have a different set of http response status code
            // for API deployment errors
            match &err {
                ApiDeploymentError::ApiDeploymentNotFound(_, _) => StatusCode::NOT_FOUND,

                ApiDeploymentError::ApiDefinitionNotFound(_, _) => StatusCode::NOT_FOUND,

                ApiDeploymentError::ApiDeploymentConflict(_) => StatusCode::INTERNAL_SERVER_ERROR,

                ApiDeploymentError::ComponentConstraintCreateError(_) => {
                    StatusCode::INTERNAL_SERVER_ERROR
                }

                ApiDeploymentError::ApiDefinitionsConflict(_) => StatusCode::INTERNAL_SERVER_ERROR,
                ApiDeploymentError::InternalRepoError(_) => StatusCode::INTERNAL_SERVER_ERROR,
                ApiDeploymentError::InternalConversionError { .. } => {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }
        ApiDefinitionLookupError::UnknownSite(_) => StatusCode::NOT_FOUND,
    }
}
