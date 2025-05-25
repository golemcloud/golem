// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
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
use super::WorkerDetails;
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
use golem_common::model::component::VersionedComponentId;
use golem_common::model::invocation_context::{
    AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId, TraceId,
};
use golem_common::model::IdempotencyKey;
use golem_common::SafeDisplay;
use golem_service_base::headers::TraceContextHeaders;
use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeRecord};
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{IntoValue, IntoValueAndType, ValueAndType};
use http::StatusCode;
use poem::Body;
use rib::{RibInput, RibInputTypeInfo, RibResult, TypeName};
use std::collections::HashMap;
use std::str::FromStr;
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
        namespace: Namespace,
        request: &mut RichRequest,
        binding: WorkerBindingCompiled,
    ) -> GatewayHttpResult<RibResult> {
        let WorkerBindingCompiled {
            response_compiled,
            component_id,
            worker_name_compiled,
            idempotency_key_compiled,
            invocation_context_compiled,
        } = binding;

        let worker_detail = self
            .get_worker_details(
                request,
                worker_name_compiled,
                idempotency_key_compiled,
                component_id,
                invocation_context_compiled,
            )
            .await?;

        self.get_response_script_result(namespace, response_compiled, request, worker_detail)
            .await
    }

    async fn handle_http_handler_binding(
        &self,
        namespace: &Namespace,
        request: &mut RichRequest,
        binding: HttpHandlerBindingCompiled,
    ) -> GatewayHttpResult<HttpHandlerBindingResult> {
        let HttpHandlerBindingCompiled {
            component_id,
            worker_name_compiled,
            idempotency_key_compiled,
            ..
        } = binding;

        let worker_detail = self
            .get_worker_details(
                request,
                worker_name_compiled,
                idempotency_key_compiled,
                component_id,
                None,
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
        namespace: Namespace,
        request: &mut RichRequest,
        binding: WorkerBindingCompiled, // TODO make separate type
    ) -> GatewayHttpResult<FileServerBindingSuccess> {
        let WorkerBindingCompiled {
            component_id: version_component_id,
            worker_name_compiled,
            idempotency_key_compiled,
            response_compiled,
            ..
        } = binding;

        let component_id = version_component_id.component_id.clone();

        let worker_detail = self
            .get_worker_details(
                request,
                worker_name_compiled,
                idempotency_key_compiled,
                version_component_id,
                None,
            )
            .await?;

        let worker_name = worker_detail.worker_name.clone();
        let worker_name = worker_name.as_deref();

        let response_script_result = self
            .get_response_script_result(
                namespace.clone(),
                response_compiled,
                request,
                worker_detail,
            )
            .await?;

        self.file_server_binding_handler
            .handle_file_server_binding_result(
                namespace,
                worker_name,
                &component_id,
                response_script_result,
            )
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
        script: WorkerNameCompiled,
        request: &mut RichRequest,
    ) -> GatewayHttpResult<String> {
        let WorkerNameCompiled {
            compiled_worker_name,
            rib_input_type_info,
            ..
        } = script;

        let rib_input: RibInput = resolve_rib_input(request, &rib_input_type_info).await?;

        let result = rib::interpret_pure(compiled_worker_name, rib_input)
            .await
            .map_err(|err| GatewayHttpError::RibInterpretPureError(err.to_string()))?
            .get_literal()
            .ok_or(GatewayHttpError::BadRequest(
                "Worker name is not a Rib expression that resolves to String".to_string(),
            ))?
            .as_string();

        Ok(result)
    }

    async fn evaluate_idempotency_key_rib_script(
        &self,
        script: IdempotencyKeyCompiled,
        request: &mut RichRequest,
    ) -> GatewayHttpResult<IdempotencyKey> {
        let IdempotencyKeyCompiled {
            compiled_idempotency_key,
            rib_input,
            ..
        } = script;

        let rib_input: RibInput = resolve_rib_input(request, &rib_input).await?;

        let value = rib::interpret_pure(compiled_idempotency_key, rib_input)
            .await
            .map_err(|err| GatewayHttpError::RibInterpretPureError(err.to_string()))?
            .get_literal()
            .ok_or(GatewayHttpError::BadRequest(
                "Idempotency key is not a Rib expression that resolves to String".to_string(),
            ))?
            .as_string();

        Ok(IdempotencyKey::new(value))
    }

    async fn evaluate_invocation_context_rib_script(
        &self,
        script: InvocationContextCompiled,
        request: &mut RichRequest,
    ) -> GatewayHttpResult<(Option<TraceId>, HashMap<String, ValueAndType>)> {
        let InvocationContextCompiled {
            compiled_invocation_context,
            rib_input,
            ..
        } = script;

        let rib_input: RibInput = resolve_rib_input(request, &rib_input).await?;

        let value = rib::interpret_pure(compiled_invocation_context, rib_input)
            .await
            .map_err(|err| GatewayHttpError::RibInterpretPureError(err.to_string()))?
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

    async fn get_worker_details(
        &self,
        request: &mut RichRequest,
        worker_name_compiled: Option<WorkerNameCompiled>,
        idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
        component_id: VersionedComponentId,
        invocation_context_compiled: Option<InvocationContextCompiled>,
    ) -> GatewayHttpResult<WorkerDetails> {
        let worker_name = if let Some(worker_name_compiled) = worker_name_compiled {
            let result = self
                .evaluate_worker_name_rib_script(worker_name_compiled, request)
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
                .evaluate_idempotency_key_rib_script(idempotency_key_compiled, request)
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
                .evaluate_invocation_context_rib_script(invocation_context_compiled, request)
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

        Ok(WorkerDetails {
            component_id: component_id.component_id,
            worker_name,
            idempotency_key,
            invocation_context,
        })
    }

    async fn get_response_script_result(
        &self,
        namespace: Namespace,
        compiled_response_mapping: ResponseMappingCompiled,
        request: &mut RichRequest,
        worker_detail: WorkerDetails,
    ) -> GatewayHttpResult<RibResult> {
        let WorkerDetails {
            invocation_context,
            component_id,
            worker_name,
            idempotency_key,
        } = worker_detail;

        let ResponseMappingCompiled {
            response_mapping_compiled,
            rib_input,
            ..
        } = compiled_response_mapping;

        let rib_input = resolve_rib_input(request, &rib_input).await?;

        self.evaluator
            .evaluate(
                worker_name,
                component_id,
                idempotency_key,
                invocation_context,
                response_mapping_compiled,
                rib_input,
                namespace,
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
                        MiddlewareError::CorsError(_) => StatusCode::FORBIDDEN,
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
                    .handle_worker_binding(namespace, &mut rich_request, resolved_worker_binding)
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
                        http_handler_binding,
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
                        namespace,
                        &mut rich_request,
                        resolved_file_server_binding,
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
    rich_request: &mut RichRequest,
    required_types: &RibInputTypeInfo,
) -> Result<RibInput, GatewayHttpError> {
    let mut values: Vec<golem_wasm_rpc::Value> = vec![];
    let mut types: Vec<NameTypePair> = vec![];

    let request_analysed_type = required_types.types.get("request");

    match request_analysed_type {
        Some(AnalysedType::Record(type_record)) => {
            for record in type_record.fields.iter() {
                let field_name = record.name.as_str();

                types.push(NameTypePair {
                    name: field_name.to_string(),
                    typ: record.typ.clone(),
                });

                match field_name {
                    "body" => {
                        let body = rich_request.request_body().await.map_err(|err| {
                            GatewayHttpError::BadRequest(format!(
                                "invalid http request body. {}",
                                err
                            ))
                        })?;

                        let body_value = TypeAnnotatedValue::parse_with_type(body, &record.typ)
                            .map_err(|err| {
                                GatewayHttpError::BadRequest(format!(
                                    "invalid http request body\n{}\nexpected request body: {}",
                                    err.join("\n"),
                                    TypeName::try_from(record.typ.clone())
                                        .map(|x| x.to_string())
                                        .unwrap_or_else(|_| format!("{:?}", &record.typ))
                                ))
                            })?;

                        let converted_value =
                            ValueAndType::try_from(body_value).map_err(|err| {
                                error!("internal value conversion error: {}", err);
                                GatewayHttpError::InternalError(
                                    "internal value conversion error".to_string(),
                                )
                            })?;

                        values.push(converted_value.value);
                    }
                    "headers" | "header" => {
                        let header_values = get_wasm_rpc_value_for_primitives(
                            &record.typ,
                            rich_request,
                            &|request, key| {
                                request
                                    .headers()
                                    .get(key)
                                    .map(|x| x.to_str().unwrap().to_string())
                                    .ok_or(format!("missing header: {}", &key))
                            },
                        )
                        .map_err(|err| {
                            GatewayHttpError::BadRequest(format!(
                                "invalid http request header. {}",
                                err
                            ))
                        })?;

                        values.push(header_values);
                    }
                    "query" => {
                        let query_value = get_wasm_rpc_value_for_primitives(
                            &record.typ,
                            rich_request,
                            &|request, key| {
                                request
                                    .query_params()
                                    .get(key)
                                    .map(|x| x.to_string())
                                    .ok_or(format!("Missing query parameter: {}", key))
                            },
                        )
                        .map_err(|err| {
                            GatewayHttpError::BadRequest(format!(
                                "invalid http request query. {}",
                                err
                            ))
                        })?;

                        values.push(query_value);
                    }
                    "path" => {
                        let path_values = get_wasm_rpc_value_for_primitives(
                            &record.typ,
                            rich_request,
                            &|request, key| {
                                request
                                    .path_params()
                                    .get(key)
                                    .map(|x| x.to_string())
                                    .ok_or(format!("Missing path parameter: {}", key))
                            },
                        )
                        .map_err(|err| {
                            GatewayHttpError::BadRequest(format!(
                                "invalid http request path. {}",
                                err
                            ))
                        })?;

                        values.push(path_values);
                    }

                    "auth" => {
                        let auth_data =
                            rich_request
                                .auth_data()
                                .ok_or(GatewayHttpError::BadRequest(
                                    "missing auth data".to_string(),
                                ))?;

                        let auth_value =
                            TypeAnnotatedValue::parse_with_type(auth_data, &record.typ).map_err(
                                |err| {
                                    GatewayHttpError::BadRequest(format!(
                                        "invalid auth data\n{}\nexpected auth: {}",
                                        err.join("\n"),
                                        TypeName::try_from(record.typ.clone())
                                            .map(|x| x.to_string())
                                            .unwrap_or_else(|_| format!("{:?}", &record.typ))
                                    ))
                                },
                            )?;

                        let converted_value =
                            ValueAndType::try_from(auth_value).map_err(|err| {
                                error!("internal value conversion error: {}", err);
                                GatewayHttpError::InternalError(
                                    "internal value conversion error".to_string(),
                                )
                            })?;

                        values.push(converted_value.value);
                    }
                    field_name => {
                        // This is already type checked during API registration,
                        // however we still fail if we happen to have other inputs
                        // at this stage instead of silently ignoring them.
                        return Err(GatewayHttpError::InternalError(format!(
                            "invalid rib script with unknown input: request.{}",
                            field_name
                        )));
                    }
                }
            }

            let mut result_map: HashMap<String, ValueAndType> = HashMap::new();

            result_map.insert(
                "request".to_string(),
                ValueAndType::new(
                    golem_wasm_rpc::Value::Record(values),
                    AnalysedType::Record(TypeRecord { fields: types }),
                ),
            );

            Ok(RibInput { input: result_map })
        }

        Some(_) => Err(GatewayHttpError::InternalError(
            "invalid rib script with unsupported type for `request`".to_string(),
        )),

        None => Ok(RibInput::default()),
    }
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

                ApiDeploymentError::ApiDefinitionNotFound(_, _, _) => StatusCode::NOT_FOUND,

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

/// Map against the required types and get `wasm_rpc::Value` from http request
/// # Parameters
/// - `analysed_type: &AnalysedType`
///   - RibInput requirement follows a pseudo form like `{request : {headers: record-type, query: record-type, path: record-type, body: analysed-type}}`).
///   - The `analysed_type` here is the type of headers, query, or path (and not body). i.e, `record-type` in the above pseudo form.
///   - This `record-type` is expected to have primitive field types. Example for a Rib `request.path.user-id` `user-id` is some primitive and `path` should be hence a record.
///   - This analysed doesn't handle (or shouldn't correspond to) the `body` field because it can be anything and not a record of primitives
/// - `request: RichRequest`
///   - The incoming request from the client
///   - `fetch_input: &FnOnce(RichRequest) -> String`, making sure we fetch anything out of the request only if it is needed
///
fn get_wasm_rpc_value_for_primitives<F>(
    required_type: &AnalysedType,
    request: &RichRequest,
    fetch_key_value: &F,
) -> Result<golem_wasm_rpc::Value, String>
where
    F: Fn(&RichRequest, &String) -> Result<String, String>,
{
    let mut header_values: Vec<golem_wasm_rpc::Value> = vec![];

    if let AnalysedType::Record(record_type) = required_type {
        for field in record_type.fields.iter() {
            let typ = &field.typ;

            let header_value = fetch_key_value(request, &field.name)?;

            let value_and_type = match typ {
                AnalysedType::Str(_) => {
                    parse_to_value::<String>(field.name.clone(), header_value, "string")?
                }
                AnalysedType::Bool(_) => {
                    parse_to_value::<bool>(field.name.clone(), header_value, "bool")?
                }
                AnalysedType::U8(_) => {
                    parse_to_value::<u8>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::U16(_) => {
                    parse_to_value::<u16>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::U32(_) => {
                    parse_to_value::<u32>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::U64(_) => {
                    parse_to_value::<u64>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::S8(_) => {
                    parse_to_value::<i8>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::S16(_) => {
                    parse_to_value::<i16>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::S32(_) => {
                    parse_to_value::<i32>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::S64(_) => {
                    parse_to_value::<i64>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::F32(_) => {
                    parse_to_value::<f32>(field.name.clone(), header_value, "number")?
                }
                AnalysedType::F64(_) => {
                    parse_to_value::<f64>(field.name.clone(), header_value, "number")?
                }
                _ => {
                    return Err(format!("Invalid type: {}", field.name));
                }
            };

            header_values.push(value_and_type);
        }
    }

    Ok(golem_wasm_rpc::Value::Record(header_values))
}

fn parse_to_value<T: FromStr + IntoValue + Sized>(
    field_name: String,
    field_value: String,
    type_name: &str,
) -> Result<golem_wasm_rpc::Value, String> {
    let value = field_value.parse::<T>().map_err(|_| {
        format!(
            "Invalid value for key {}. Expected {}, Found {}",
            field_name, type_name, field_value
        )
    })?;
    Ok(value.into_value_and_type().value)
}
