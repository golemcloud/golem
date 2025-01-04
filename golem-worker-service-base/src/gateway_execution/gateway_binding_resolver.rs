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

use crate::gateway_api_definition::http::{CompiledHttpApiDefinition, VarInfo};
use crate::gateway_binding::{
    GatewayBindingCompiled, HttpRequestDetails, RibInputTypeMismatch, StaticBinding,
};
use crate::gateway_binding::{GatewayRequestDetails, ResponseMappingCompiled};
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_execution::router::RouterPattern;
use crate::gateway_execution::to_response_failure::ToHttpResponseFromSafeDisplay;
use crate::gateway_middleware::{MiddlewareError, MiddlewareSuccess};
use crate::gateway_request::http_request::{router, InputHttpRequest};
use crate::gateway_security::{IdentityProvider, OpenIdClient};
use async_trait::async_trait;
use golem_common::model::IdempotencyKey;
use golem_common::SafeDisplay;
use golem_service_base::model::VersionedComponentId;
use http::StatusCode;
use openidconnect::{CsrfToken, Nonce};
use poem::Body;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

// Every type of request (example: InputHttpRequest (which corresponds to a Route)) can have an instance of this resolver,
// which will resolve the gateway binding equired for that request.
#[async_trait]
pub trait GatewayBindingResolver<Namespace, ApiDefinition> {
    async fn resolve_gateway_binding(
        &self,
        api_definitions: Vec<ApiDefinition>,
    ) -> Result<ResolvedGatewayBinding<Namespace>, ErrorOrRedirect>;
}

#[derive(Debug)]
pub enum ErrorOrRedirect {
    Error(GatewayBindingResolverError),
    Redirect(poem::Response),
}

impl ErrorOrRedirect {
    pub fn internal(err: String) -> Self {
        ErrorOrRedirect::Error(GatewayBindingResolverError::Internal(err))
    }

    pub fn route_not_found() -> Self {
        ErrorOrRedirect::Error(GatewayBindingResolverError::RouteNotFound)
    }

    pub fn rib_input_type_mismatch(err: RibInputTypeMismatch) -> Self {
        ErrorOrRedirect::Error(GatewayBindingResolverError::RibInputTypeMismatch(err))
    }
}

#[derive(Debug)]
pub enum GatewayBindingResolverError {
    RibInputTypeMismatch(RibInputTypeMismatch),
    Internal(String),
    RouteNotFound,
    MiddlewareError(MiddlewareError),
}

impl SafeDisplay for GatewayBindingResolverError {
    fn to_safe_string(&self) -> String {
        match self {
            GatewayBindingResolverError::RibInputTypeMismatch(err) => {
                format!("Input type mismatch: {}", err)
            }
            GatewayBindingResolverError::Internal(err) => format!("Internal: {}", err),
            GatewayBindingResolverError::RouteNotFound => "RouteNotFound".to_string(),
            GatewayBindingResolverError::MiddlewareError(err) => err.to_safe_string(),
        }
    }
}

impl GatewayBindingResolverError {
    pub fn to_http_response(self) -> poem::Response {
        match self {
            GatewayBindingResolverError::Internal(str) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(str)),
            GatewayBindingResolverError::RouteNotFound => poem::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from_string("Route not found".to_string())),
            GatewayBindingResolverError::RibInputTypeMismatch(_) => poem::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from_string("Rib input type mismatch".to_string())),
            GatewayBindingResolverError::MiddlewareError(error) => error
                .to_response_from_safe_display(|error| match error {
                    MiddlewareError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
                    MiddlewareError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
                }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedGatewayBinding<Namespace> {
    pub request_details: GatewayRequestDetails,
    pub resolved_binding: ResolvedBinding<Namespace>,
}

#[derive(Clone, Debug)]
pub enum ResolvedBinding<Namespace> {
    Static(StaticBinding),
    Worker(ResolvedWorkerBinding<Namespace>),
    FileServer(ResolvedWorkerBinding<Namespace>),
}

#[derive(Clone, Debug)]
pub struct AuthParams {
    pub client: OpenIdClient,
    pub csrf_state: CsrfToken,
    pub nonce: Nonce,
    pub original_uri: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerDetail {
    pub component_id: VersionedComponentId,
    pub worker_name: Option<String>,
    pub idempotency_key: Option<IdempotencyKey>,
}

impl WorkerDetail {
    pub fn as_json(&self) -> Value {
        let mut worker_detail_content = HashMap::new();
        worker_detail_content.insert(
            "component_id".to_string(),
            Value::String(self.component_id.component_id.0.to_string()),
        );

        if let Some(worker_name) = &self.worker_name {
            worker_detail_content
                .insert("name".to_string(), Value::String(worker_name.to_string()));
        }

        if let Some(idempotency_key) = &self.idempotency_key {
            worker_detail_content.insert(
                "idempotency_key".to_string(),
                Value::String(idempotency_key.value.clone()),
            );
        }

        let map = serde_json::Map::from_iter(worker_detail_content);

        Value::Object(map)
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkerBinding<Namespace> {
    pub worker_detail: WorkerDetail,
    pub compiled_response_mapping: ResponseMappingCompiled,
    pub namespace: Namespace,
}

impl<Namespace> ResolvedGatewayBinding<Namespace> {
    pub fn get_worker_detail(&self) -> Option<WorkerDetail> {
        match &self.resolved_binding {
            ResolvedBinding::Worker(resolved_worker_binding) => {
                Some(resolved_worker_binding.worker_detail.clone())
            }
            _ => None,
        }
    }
    pub fn from_static_binding(
        request_details: &GatewayRequestDetails,
        static_binding: &StaticBinding,
    ) -> ResolvedGatewayBinding<Namespace> {
        ResolvedGatewayBinding {
            request_details: request_details.clone(),
            resolved_binding: ResolvedBinding::Static(static_binding.clone()),
        }
    }

    pub fn from_resolved_worker_binding(
        request_details: &GatewayRequestDetails,
        resolved_worker_binding: ResolvedWorkerBinding<Namespace>,
    ) -> ResolvedGatewayBinding<Namespace> {
        ResolvedGatewayBinding {
            request_details: request_details.clone(),
            resolved_binding: ResolvedBinding::Worker(resolved_worker_binding),
        }
    }
}

pub struct DefaultGatewayBindingResolver {
    input: InputHttpRequest,
    gateway_session_store: GatewaySessionStore,
    identity_provider: Arc<dyn IdentityProvider + Sync + Send>,
}

impl DefaultGatewayBindingResolver {
    pub fn new(
        input: InputHttpRequest,
        gateway_session_store: &GatewaySessionStore,
        identity_provider: &Arc<dyn IdentityProvider + Sync + Send>,
    ) -> Self {
        DefaultGatewayBindingResolver {
            input,
            gateway_session_store: Arc::clone(gateway_session_store),
            identity_provider: Arc::clone(identity_provider),
        }
    }
}

#[async_trait]
impl<Namespace: Clone + Send + Sync + 'static>
    GatewayBindingResolver<Namespace, CompiledHttpApiDefinition<Namespace>>
    for DefaultGatewayBindingResolver
{
    async fn resolve_gateway_binding(
        &self,
        compiled_api_definitions: Vec<CompiledHttpApiDefinition<Namespace>>,
    ) -> Result<ResolvedGatewayBinding<Namespace>, ErrorOrRedirect> {
        let compiled_routes = compiled_api_definitions
            .iter()
            .flat_map(|x| x.routes.iter().map(|y| (x.namespace.clone(), y.clone())))
            .collect::<Vec<_>>();

        let api_request = self;
        let router = router::build(compiled_routes);

        let path: Vec<&str> =
            RouterPattern::split(&api_request.input.api_input_path.base_path).collect();
        let request_query_variables = self
            .input
            .api_input_path
            .query_components()
            .unwrap_or_default();
        let request_body = &self.input.req_body;
        let headers = &self.input.headers;

        let router::RouteEntry {
            path_params,
            query_params,
            namespace,
            binding,
            middlewares,
        } = router
            .check_path(&api_request.input.req_method, &path)
            .ok_or(ErrorOrRedirect::route_not_found())?;

        let zipped_path_params: HashMap<VarInfo, String> = {
            path_params
                .iter()
                .map(|param| match param {
                    router::PathParamExtractor::Single { var_info, index } => {
                        (var_info.clone(), path[*index].to_string())
                    }
                    router::PathParamExtractor::AllFollowing { var_info, index } => {
                        let value = path[*index..].join("/");
                        (var_info.clone(), value)
                    }
                })
                .collect()
        };

        let mut http_request_details = HttpRequestDetails::from_input_http_request(
            &self.input.scheme,
            &self.input.host,
            &self.input.api_input_path,
            &zipped_path_params,
            &request_query_variables,
            query_params,
            request_body,
            headers.clone(),
            middlewares,
        )
        .map_err(|err| {
            ErrorOrRedirect::internal(format!(
                "Failed to fetch input request details {}",
                err.join(", ")
            ))
        })?;

        if let Some(middlewares) = middlewares {
            let middleware_result = internal::redirect_or_continue(
                &mut http_request_details,
                middlewares,
                &self.gateway_session_store,
                &self.identity_provider,
            )
            .await
            .map_err(|err| {
                ErrorOrRedirect::Error(GatewayBindingResolverError::MiddlewareError(err))
            })?;

            if let MiddlewareSuccess::Redirect(response) = middleware_result {
                return Err(ErrorOrRedirect::Redirect(response));
            }
        }

        match binding {
            GatewayBindingCompiled::FileServer(worker_binding) => internal::get_resolved_binding(
                worker_binding,
                &http_request_details,
                namespace,
                headers,
            )
            .await
            .map(|resolved_binding| ResolvedGatewayBinding {
                request_details: GatewayRequestDetails::Http(http_request_details),
                resolved_binding: ResolvedBinding::FileServer(resolved_binding),
            }),
            GatewayBindingCompiled::Worker(worker_binding) => internal::get_resolved_binding(
                worker_binding,
                &http_request_details,
                namespace,
                headers,
            )
            .await
            .map(|resolved_binding| ResolvedGatewayBinding {
                request_details: GatewayRequestDetails::Http(http_request_details),
                resolved_binding: ResolvedBinding::Worker(resolved_binding),
            }),
            GatewayBindingCompiled::Static(static_binding) => {
                Ok(ResolvedGatewayBinding::from_static_binding(
                    &GatewayRequestDetails::Http(http_request_details),
                    static_binding,
                ))
            }
        }
    }
}

mod internal {
    use crate::gateway_binding::{
        ErrorOrRedirect, HttpRequestDetails, ResolvedWorkerBinding, RibInputValueResolver,
        WorkerBindingCompiled, WorkerDetail,
    };
    use crate::gateway_execution::gateway_session::GatewaySessionStore;
    use crate::gateway_middleware::{HttpMiddlewares, MiddlewareError, MiddlewareSuccess};
    use crate::gateway_security::IdentityProvider;
    use golem_common::model::IdempotencyKey;
    use http::HeaderMap;
    use std::sync::Arc;

    pub async fn redirect_or_continue(
        input: &mut HttpRequestDetails,
        middlewares: &HttpMiddlewares,
        session_store: &GatewaySessionStore,
        identity_provider: &Arc<dyn IdentityProvider + Sync + Send>,
    ) -> Result<MiddlewareSuccess, MiddlewareError> {
        let input_middleware_result = middlewares
            .process_middleware_in(input, session_store, identity_provider)
            .await;

        match input_middleware_result {
            Ok(incoming_middleware_result) => match incoming_middleware_result {
                MiddlewareSuccess::Redirect(response) => Ok(MiddlewareSuccess::Redirect(response)),
                MiddlewareSuccess::PassThrough { session_id } => {
                    if let Some(session_id) = &session_id {
                        let result = input.inject_auth_details(session_id, session_store).await;

                        if let Err(err_response) = result {
                            return Err(MiddlewareError::InternalError(err_response));
                        }
                    }

                    Ok(MiddlewareSuccess::PassThrough { session_id })
                }
            },

            Err(err) => Err(err),
        }
    }

    pub async fn get_resolved_binding<Namespace: Clone>(
        binding: &WorkerBindingCompiled,
        http_request_details: &HttpRequestDetails,
        namespace: &Namespace,
        headers: &HeaderMap,
    ) -> Result<ResolvedWorkerBinding<Namespace>, ErrorOrRedirect> {
        let worker_name_opt = if let Some(worker_name_compiled) = &binding.worker_name_compiled {
            let resolve_rib_input = http_request_details
                .resolve_rib_input_value(&worker_name_compiled.rib_input_type_info)
                .map_err(ErrorOrRedirect::rib_input_type_mismatch)?;

            let worker_name = rib::interpret_pure(
                &worker_name_compiled.compiled_worker_name,
                &resolve_rib_input,
            )
            .await
            .map_err(|err| {
                ErrorOrRedirect::internal(format!(
                    "Failed to evaluate worker name rib expression. {}",
                    err
                ))
            })?
            .get_literal()
            .ok_or(ErrorOrRedirect::internal(
                "Worker name is not a Rib expression that resolves to String".to_string(),
            ))?
            .as_string();

            Some(worker_name)
        } else {
            None
        };

        let component_id = &binding.component_id;

        let idempotency_key =
            if let Some(idempotency_key_compiled) = &binding.idempotency_key_compiled {
                let resolve_rib_input = http_request_details
                    .resolve_rib_input_value(&idempotency_key_compiled.rib_input)
                    .map_err(ErrorOrRedirect::rib_input_type_mismatch)?;

                let idempotency_key_value = rib::interpret_pure(
                    &idempotency_key_compiled.compiled_idempotency_key,
                    &resolve_rib_input,
                )
                .await
                .map_err(|err| ErrorOrRedirect::internal(err.to_string()))?;

                let idempotency_key = idempotency_key_value
                    .get_literal()
                    .ok_or(ErrorOrRedirect::internal(
                        "Idempotency Key is not a string".to_string(),
                    ))?
                    .as_string();

                Some(IdempotencyKey::new(idempotency_key))
            } else {
                headers
                    .get("idempotency-key")
                    .and_then(|h| h.to_str().ok())
                    .map(|value| IdempotencyKey::new(value.to_string()))
            };

        let worker_detail = WorkerDetail {
            component_id: component_id.clone(),
            worker_name: worker_name_opt,
            idempotency_key,
        };

        let resolved_binding = ResolvedWorkerBinding {
            worker_detail,
            compiled_response_mapping: binding.response_compiled.clone(),
            namespace: namespace.clone(),
        };

        Ok(resolved_binding)
    }
}
