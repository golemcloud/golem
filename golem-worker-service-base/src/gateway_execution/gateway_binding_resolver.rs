use crate::gateway_api_definition::http::{CompiledHttpApiDefinition, VarInfo};
use crate::gateway_binding::{GatewayBindingCompiled, RibInputValueResolver, StaticBinding};
use crate::gateway_binding::{GatewayRequestDetails, ResponseMappingCompiled};
use crate::gateway_execution::router::RouterPattern;
use crate::gateway_request::http_request::{router, InputHttpRequest};
use async_trait::async_trait;
use golem_common::model::IdempotencyKey;
use golem_service_base::model::VersionedComponentId;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Display;

use crate::gateway_middleware::Middlewares;
use golem_common::model::GatewayBindingType;

// Every type of request (example: InputHttpRequest (which corresponds to a Route)) can have an instance of this resolver,
// which will resolve the gateway binding equired for that request.
#[async_trait]
pub trait GatewayBindingResolver<Namespace, ApiDefinition> {
    async fn resolve_worker_binding(
        &self,
        api_definitions: Vec<ApiDefinition>,
    ) -> Result<ResolvedGatewayBinding<Namespace>, GatewayBindingResolverError>;
}

#[derive(Debug)]
pub struct GatewayBindingResolverError(pub String);

impl<A: AsRef<str>> From<A> for GatewayBindingResolverError {
    fn from(message: A) -> Self {
        GatewayBindingResolverError(message.as_ref().to_string())
    }
}

impl Display for GatewayBindingResolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Worker binding resolution error: {}", self.0)
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
    pub worker_binding_type: GatewayBindingType,
    pub namespace: Namespace,
    pub middlewares: Middlewares,
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
    pub fn from_static(
        request_details: &GatewayRequestDetails,
        static_binding: &StaticBinding,
    ) -> ResolvedGatewayBinding<Namespace> {
        ResolvedGatewayBinding {
            request_details: request_details.clone(),
            resolved_binding: ResolvedBinding::Static(static_binding.clone()),
        }
    }

    pub fn from_worker(
        request_details: &GatewayRequestDetails,
        resolved_worker_binding: ResolvedWorkerBinding<Namespace>,
    ) -> ResolvedGatewayBinding<Namespace> {
        ResolvedGatewayBinding {
            request_details: request_details.clone(),
            resolved_binding: ResolvedBinding::Worker(resolved_worker_binding),
        }
    }
}

#[async_trait]
impl<Namespace: Clone + Send + Sync + 'static>
    GatewayBindingResolver<Namespace, CompiledHttpApiDefinition<Namespace>> for InputHttpRequest
{
    async fn resolve_worker_binding(
        &self,
        compiled_api_definitions: Vec<CompiledHttpApiDefinition<Namespace>>,
    ) -> Result<ResolvedGatewayBinding<Namespace>, GatewayBindingResolverError> {
        let compiled_routes = compiled_api_definitions
            .iter()
            .flat_map(|x| x.routes.iter().map(|y| (x.namespace.clone(), y.clone())))
            .collect::<Vec<_>>();

        let api_request = self;
        let router = router::build(compiled_routes);

        let path: Vec<&str> = RouterPattern::split(&api_request.input_path.base_path).collect();
        let request_query_variables = self.input_path.query_components().unwrap_or_default();
        let request_body = &self.req_body;
        let headers = &self.headers;

        let router::RouteEntry {
            path_params,
            query_params,
            namespace,
            binding,
        } = router
            .check_path(&api_request.req_method, &path)
            .ok_or("Failed to resolve route")?;

        let zipped_path_params: HashMap<VarInfo, &str> = {
            path_params
                .iter()
                .map(|(var, index)| (var.clone(), path[*index]))
                .collect()
        };

        let http_request_details = GatewayRequestDetails::from(
            &zipped_path_params,
            &request_query_variables,
            query_params,
            request_body,
            headers,
        )
        .map_err(|err| format!("Failed to fetch input request details {}", err.join(", ")))?;

        match binding {
            GatewayBindingCompiled::Worker(binding) => {
                let worker_name_opt = if let Some(worker_name_compiled) =
                    &binding.worker_name_compiled
                {
                    let resolve_rib_input = http_request_details
                        .resolve_rib_input_value(&worker_name_compiled.rib_input_type_info)
                        .map_err(|err| {
                            format!(
                                "Failed to resolve rib input value from http request details {}",
                                err
                            )
                        })?;

                    let worker_name = rib::interpret_pure(
                        &worker_name_compiled.compiled_worker_name,
                        &resolve_rib_input,
                    )
                    .await
                    .map_err(|err| {
                        format!("Failed to evaluate worker name rib expression. {}", err)
                    })?
                    .get_literal()
                    .ok_or(
                        "Worker name is not a Rib expression that resolves to String".to_string(),
                    )?
                    .as_string();

                    Some(worker_name)
                } else {
                    None
                };

                let component_id = &binding.component_id;

                let idempotency_key = if let Some(idempotency_key_compiled) =
                    &binding.idempotency_key_compiled
                {
                    let resolve_rib_input = http_request_details
                        .resolve_rib_input_value(&idempotency_key_compiled.rib_input)
                        .map_err(|err| {
                            format!(
                                "Failed to resolve rib input value from http request details {} for idemptency key",
                                err
                            )
                        })?;

                    let idempotency_key_value = rib::interpret_pure(
                        &idempotency_key_compiled.compiled_idempotency_key,
                        &resolve_rib_input,
                    )
                    .await
                    .map_err(|err| err.to_string())?;

                    let idempotency_key = idempotency_key_value
                        .get_literal()
                        .ok_or("Idempotency Key is not a string")?
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
                    worker_binding_type: binding.worker_binding_type.clone(),
                    namespace: namespace.clone(),
                    middlewares: binding.middlewares.clone().unwrap_or_default(),
                };

                Ok(ResolvedGatewayBinding::from_worker(
                    &http_request_details,
                    resolved_binding,
                ))
            }
            GatewayBindingCompiled::Static(static_binding) => Ok(
                ResolvedGatewayBinding::from_static(&http_request_details, static_binding),
            ),
        }
    }
}
