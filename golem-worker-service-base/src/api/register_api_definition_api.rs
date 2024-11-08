use crate::api;
use crate::gateway_api_definition::http::{
    AllPathPatterns, CompiledHttpApiDefinition, CompiledRoute, MethodPattern,
};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_api_deployment::http::ApiSite;
use crate::gateway_binding::{
    GatewayBinding, GatewayBindingCompiled, StaticBinding, WorkerBinding,
};
use crate::gateway_middleware::{CorsPreflight, HttpMiddleware, Middleware};
use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;
use golem_service_base::model::VersionedComponentId;
use poem_openapi::*;
use rib::RibInputTypeInfo;
use serde::{Deserialize, Serialize};
use std::result::Result;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ApiDeploymentRequest {
    pub api_definitions: Vec<ApiDefinitionInfo>,
    pub site: ApiSite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ApiDeployment {
    pub api_definitions: Vec<ApiDefinitionInfo>,
    pub site: ApiSite,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ApiDefinitionInfo {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
}

// Mostly this data structures that represents the actual incoming request
// exist due to the presence of complicated Expr data type in gateway_api_definition::ApiDefinition.
// Consider them to be otherwise same
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct HttpApiDefinitionRequest {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<RouteData>,
    #[serde(default)]
    pub draft: bool,
}

// Mostly this data structures that represents the actual incoming request
// exist due to the presence of complicated Expr data type in gateway_api_definition::ApiDefinition.
// Consider them to be otherwise same
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct HttpApiDefinition {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<RouteData>,
    #[serde(default)]
    pub draft: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

// HttpApiDefinitionWithTypeInfo is CompiledHttpApiDefinition minus rib-byte-code
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct HttpApiDefinitionWithTypeInfo {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<RouteWithTypeInfo>,
    #[serde(default)]
    pub draft: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<CompiledHttpApiDefinition> for HttpApiDefinitionWithTypeInfo {
    fn from(value: CompiledHttpApiDefinition) -> Self {
        let routes = value.routes.into_iter().map(|route| route.into()).collect();

        Self {
            id: value.id,
            version: value.version,
            routes,
            draft: value.draft,
            created_at: Some(value.created_at),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteData {
    pub method: MethodPattern,
    pub path: String,
    pub binding: GatewayBindingData,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteWithTypeInfo {
    pub method: MethodPattern,
    pub path: String,
    pub binding: GatewayBindingWithTypeInfo,
}

impl From<CompiledRoute> for RouteWithTypeInfo {
    fn from(value: CompiledRoute) -> Self {
        let method = value.method;
        let path = value.path.to_string();
        let binding = value.binding.into();
        Self {
            method,
            path,
            binding,
        }
    }
}

// GatewayBindingData is a user exposed structure of GatewayBinding
// GatewayBindingData is flattened here only to keep the REST API backward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GatewayBindingData {
    pub binding_type: Option<GatewayBindingType>, // descriminator to keep backward compatibility

    // WORKER
    // For binding type - worker
    // Optional only to keep backward compatibility
    pub component_id: Option<VersionedComponentId>,
    // For binding type - worker
    pub worker_name: Option<String>,
    // For binding type - worker
    pub idempotency_key: Option<String>,
    // For binding type - worker
    // Optional only to keep backward compatibility
    pub response: Option<String>,
    // For binding type - worker
    // Optional only to keep backward compatibility
    pub middleware: Option<MiddlewareData>,

    // CORS
    //  For binding type - cors-middleware
    // Optional only to keep backward compatibility
    pub allow_origin: Option<String>,
    //  For binding type - cors-middleware
    // Optional only to keep backward compatibility
    pub allow_methods: Option<String>,
    //  For binding type - cors-middleware
    // Optional only to keep backward compatibility
    pub allow_headers: Option<String>,
    //  For binding type - cors-middleware
    // Optional only to keep backward compatibility
    pub expose_headers: Option<String>,
    //  For binding type - cors-middleware
    // Optional only to keep backward compatibility
    pub max_age: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct MiddlewareData {
    pub cors: Option<CorsPreflight>,
}

// This is only to keep backward compatibility we introduce an optional discriminator
// and it's absence will result in default binding which GolemWorkerBinding
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Enum)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub enum GatewayBindingType {
    Cors,
    Default,
}

impl From<golem_api_grpc::proto::golem::apidefinition::GatewayBindingType> for GatewayBindingType {
    fn from(value: grpc_apidefinition::GatewayBindingType) -> Self {
        match value {
            grpc_apidefinition::GatewayBindingType::Cors => GatewayBindingType::Cors,
            grpc_apidefinition::GatewayBindingType::Default => GatewayBindingType::Default,
        }
    }
}

impl From<GatewayBindingType> for golem_api_grpc::proto::golem::apidefinition::GatewayBindingType {
    fn from(value: GatewayBindingType) -> Self {
        match value {
            GatewayBindingType::Cors => grpc_apidefinition::GatewayBindingType::Cors,
            GatewayBindingType::Default => grpc_apidefinition::GatewayBindingType::Default,
        }
    }
}

// GatewayBindingWithTypeInfo is a subset of GatewayBindingCompiled
// that it doesn't expose internal details such as rib byte code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GatewayBindingWithTypeInfo {
    pub component_id: Option<VersionedComponentId>, // Optional to keep it backward compatible
    pub worker_name: Option<String>,
    pub idempotency_key: Option<String>,
    pub response: Option<String>, // Optional to keep it backward compatible
    pub response_mapping_input: Option<RibInputTypeInfo>,
    pub worker_name_input: Option<RibInputTypeInfo>,
    pub idempotency_key_input: Option<RibInputTypeInfo>,
    pub cors_preflight: Option<CorsPreflight>,
}

impl From<GatewayBindingCompiled> for GatewayBindingWithTypeInfo {
    fn from(value: GatewayBindingCompiled) -> Self {
        let gateway_binding = value.clone();

        match gateway_binding {
            GatewayBindingCompiled::Worker(worker_binding) => GatewayBindingWithTypeInfo {
                component_id: Some(worker_binding.component_id),
                worker_name: worker_binding
                    .worker_name_compiled
                    .clone()
                    .map(|compiled| compiled.worker_name.to_string()),
                idempotency_key: worker_binding.idempotency_key_compiled.clone().map(
                    |idempotency_key_compiled| idempotency_key_compiled.idempotency_key.to_string(),
                ),
                response: Some(
                    worker_binding
                        .response_compiled
                        .response_rib_expr
                        .to_string(),
                ),
                response_mapping_input: Some(worker_binding.response_compiled.rib_input),
                worker_name_input: worker_binding
                    .worker_name_compiled
                    .map(|compiled| compiled.rib_input_type_info),
                idempotency_key_input: worker_binding
                    .idempotency_key_compiled
                    .map(|idempotency_key_compiled| idempotency_key_compiled.rib_input),
                cors_preflight: None,
            },
            GatewayBindingCompiled::Static(static_binding) => GatewayBindingWithTypeInfo {
                component_id: None,
                worker_name: None,
                idempotency_key: None,
                response: None,
                response_mapping_input: None,
                worker_name_input: None,
                idempotency_key_input: None,
                cors_preflight: static_binding.get_cors_preflight(),
            },
        }
    }
}

impl<N> From<crate::gateway_api_deployment::http::ApiDeployment<N>> for ApiDeployment {
    fn from(value: crate::gateway_api_deployment::http::ApiDeployment<N>) -> Self {
        let api_definitions = value
            .api_definition_keys
            .into_iter()
            .map(|key| ApiDefinitionInfo {
                id: key.id,
                version: key.version,
            })
            .collect();

        Self {
            api_definitions,
            site: value.site,
            created_at: Some(value.created_at),
        }
    }
}

impl TryFrom<crate::gateway_api_definition::http::HttpApiDefinition> for HttpApiDefinition {
    type Error = String;

    fn try_from(
        value: crate::gateway_api_definition::http::HttpApiDefinition,
    ) -> Result<Self, Self::Error> {
        let mut routes = Vec::new();
        for route in value.routes {
            let v = RouteData::try_from(route)?;
            routes.push(v);
        }

        Ok(Self {
            id: value.id,
            version: value.version,
            routes,
            draft: value.draft,
            created_at: Some(value.created_at),
        })
    }
}

impl TryInto<crate::gateway_api_definition::http::HttpApiDefinitionRequest>
    for HttpApiDefinitionRequest
{
    type Error = String;

    fn try_into(
        self,
    ) -> Result<crate::gateway_api_definition::http::HttpApiDefinitionRequest, Self::Error> {
        let mut routes = Vec::new();

        for route in self.routes {
            let v = route.try_into()?;
            routes.push(v);
        }

        Ok(
            crate::gateway_api_definition::http::HttpApiDefinitionRequest {
                id: self.id,
                version: self.version,
                routes,
                draft: self.draft,
            },
        )
    }
}

impl TryFrom<crate::gateway_api_definition::http::Route> for RouteData {
    type Error = String;

    fn try_from(value: crate::gateway_api_definition::http::Route) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = GatewayBindingData::try_from(value.binding)?;

        Ok(Self {
            method: value.method,
            path,
            binding,
        })
    }
}

impl TryInto<crate::gateway_api_definition::http::Route> for RouteData {
    type Error = String;

    fn try_into(self) -> Result<crate::gateway_api_definition::http::Route, Self::Error> {
        let path = AllPathPatterns::parse(self.path.as_str())?;
        let binding = self.binding.try_into()?;

        Ok(crate::gateway_api_definition::http::Route {
            method: self.method,
            path,
            binding,
        })
    }
}

impl TryFrom<GatewayBinding> for GatewayBindingData {
    type Error = String;

    fn try_from(value: crate::gateway_binding::GatewayBinding) -> Result<Self, Self::Error> {
        match value {
            GatewayBinding::Worker(worker_binding) => {
                let response: String =
                    rib::to_string(&worker_binding.response.0).map_err(|e| e.to_string())?;

                let worker_id = worker_binding
                    .worker_name
                    .map(|expr| rib::to_string(&expr).map_err(|e| e.to_string()))
                    .transpose()?;

                let idempotency_key = if let Some(key) = &worker_binding.idempotency_key {
                    Some(rib::to_string(key).map_err(|e| e.to_string())?)
                } else {
                    None
                };

                let middleware = worker_binding.middleware.and_then(|x| {
                    x.0.iter().find_map(|m| {
                        m.get_cors().map(|c| MiddlewareData {
                            cors: Some(c.clone()),
                        })
                    })
                });

                Ok(Self {
                    binding_type: Some(api::GatewayBindingType::Default),
                    component_id: Some(worker_binding.component_id),
                    worker_name: worker_id,
                    idempotency_key,
                    response: Some(response),
                    allow_origin: None,
                    allow_methods: None,
                    allow_headers: None,
                    expose_headers: None,
                    max_age: None,
                    middleware,
                })
            }

            GatewayBinding::Static(plugin) => match plugin {
                StaticBinding::Middleware(middleware) => {
                    let cors = middleware.get_cors().clone();

                    Ok(Self {
                        binding_type: Some(api::GatewayBindingType::Cors),
                        component_id: None,
                        worker_name: None,
                        idempotency_key: None,
                        response: None,
                        allow_origin: cors.as_ref().map(|c| c.allow_origin.clone()),
                        allow_methods: cors.as_ref().map(|c| c.allow_methods.clone()),
                        allow_headers: cors.as_ref().map(|c| c.allow_headers.clone()),
                        expose_headers: cors.as_ref().and_then(|c| c.expose_headers.clone()),
                        max_age: cors.as_ref().and_then(|c| c.max_age),
                        middleware: None,
                    })
                }
            },
        }
    }
}

impl TryFrom<GatewayBindingData> for GatewayBinding {
    type Error = String;

    fn try_from(gateway_binding_data: GatewayBindingData) -> Result<Self, Self::Error> {
        let v = gateway_binding_data.binding_type;

        match v {
            Some(GatewayBindingType::Default) | None => {
                let response = gateway_binding_data
                    .response
                    .ok_or("Missing response field in binding")?;
                let component_id = gateway_binding_data
                    .component_id
                    .ok_or("Missing componentId field in binding")?;

                let response: crate::gateway_binding::ResponseMapping = {
                    let r = rib::from_string(response.as_str()).map_err(|e| e.to_string())?;
                    crate::gateway_binding::ResponseMapping(r)
                };

                let worker_name = gateway_binding_data
                    .worker_name
                    .map(|name| rib::from_string(name.as_str()).map_err(|e| e.to_string()))
                    .transpose()?;

                let idempotency_key = if let Some(key) = &gateway_binding_data.idempotency_key {
                    Some(rib::from_string(key).map_err(|e| e.to_string())?)
                } else {
                    None
                };

                let mut middlewares = Vec::new();
                if let Some(middle_ware_daa) = gateway_binding_data.middleware {
                    if let Some(cors) = middle_ware_daa.cors {
                        middlewares.push(Middleware::http(HttpMiddleware::cors(cors)));
                    }
                }

                let worker_binding = WorkerBinding {
                    component_id,
                    worker_name,
                    idempotency_key,
                    response,
                    middleware: if middlewares.is_empty() {
                        None
                    } else {
                        Some(crate::gateway_middleware::Middlewares(middlewares))
                    },
                };

                Ok(GatewayBinding::Worker(worker_binding))
            }

            Some(GatewayBindingType::Cors) => {
                let cors_preflight = CorsPreflight::from_parameters(
                    gateway_binding_data.allow_origin,
                    gateway_binding_data.allow_methods,
                    gateway_binding_data.allow_headers,
                    gateway_binding_data.expose_headers,
                    gateway_binding_data.max_age,
                );

                Ok(GatewayBinding::Static(StaticBinding::from_http_middleware(
                    HttpMiddleware::cors(cors_preflight),
                )))
            }
        }
    }
}

impl TryFrom<crate::gateway_api_definition::http::HttpApiDefinition>
    for grpc_apidefinition::ApiDefinition
{
    type Error = String;

    fn try_from(
        value: crate::gateway_api_definition::http::HttpApiDefinition,
    ) -> Result<Self, Self::Error> {
        let routes = value
            .routes
            .into_iter()
            .map(grpc_apidefinition::HttpRoute::try_from)
            .collect::<Result<Vec<grpc_apidefinition::HttpRoute>, String>>()?;

        let id = value.id.0;

        let definition = grpc_apidefinition::HttpApiDefinition { routes };

        let created_at = prost_types::Timestamp::from(SystemTime::from(value.created_at));

        let result = grpc_apidefinition::ApiDefinition {
            id: Some(grpc_apidefinition::ApiDefinitionId { value: id }),
            version: value.version.0,
            definition: Some(grpc_apidefinition::api_definition::Definition::Http(
                definition,
            )),
            draft: value.draft,
            created_at: Some(created_at),
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::ApiDefinition>
    for crate::gateway_api_definition::http::HttpApiDefinition
{
    type Error = String;

    fn try_from(value: grpc_apidefinition::ApiDefinition) -> Result<Self, Self::Error> {
        let routes = match value.definition.ok_or("definition is missing")? {
            grpc_apidefinition::api_definition::Definition::Http(http) => http
                .routes
                .into_iter()
                .map(crate::gateway_api_definition::http::Route::try_from)
                .collect::<Result<Vec<crate::gateway_api_definition::http::Route>, String>>()?,
        };

        let id = value.id.ok_or("Api Definition ID is missing")?;
        let created_at = value
            .created_at
            .ok_or("Created At is missing")
            .and_then(|t| SystemTime::try_from(t).map_err(|_| "Failed to convert timestamp"))?;

        let result = crate::gateway_api_definition::http::HttpApiDefinition {
            id: ApiDefinitionId(id.value),
            version: ApiVersion(value.version),
            routes,
            draft: value.draft,
            created_at: created_at.into(),
        };

        Ok(result)
    }
}

impl TryFrom<grpc_apidefinition::v1::ApiDefinitionRequest>
    for crate::gateway_api_definition::http::HttpApiDefinitionRequest
{
    type Error = String;

    fn try_from(value: grpc_apidefinition::v1::ApiDefinitionRequest) -> Result<Self, Self::Error> {
        let routes = match value.definition.ok_or("definition is missing")? {
            grpc_apidefinition::v1::api_definition_request::Definition::Http(http) => http
                .routes
                .into_iter()
                .map(crate::gateway_api_definition::http::Route::try_from)
                .collect::<Result<Vec<crate::gateway_api_definition::http::Route>, String>>()?,
        };

        let id = value.id.ok_or("Api Definition ID is missing")?;

        let result = crate::gateway_api_definition::http::HttpApiDefinitionRequest {
            id: ApiDefinitionId(id.value),
            version: ApiVersion(value.version),
            routes,
            draft: value.draft,
        };

        Ok(result)
    }
}

impl TryFrom<crate::gateway_api_definition::http::Route> for grpc_apidefinition::HttpRoute {
    type Error = String;

    fn try_from(value: crate::gateway_api_definition::http::Route) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = value.binding.into();
        let method: grpc_apidefinition::HttpMethod = value.method.into();

        let result = grpc_apidefinition::HttpRoute {
            method: method as i32,
            path,
            binding: Some(binding),
        };

        Ok(result)
    }
}

impl TryFrom<CompiledRoute> for golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute {
    type Error = String;

    fn try_from(value: CompiledRoute) -> Result<Self, Self::Error> {
        let method = value.method as i32;
        let path = value.path.to_string();
        let binding = golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding::from(
            value.binding,
        );
        Ok(Self {
            method,
            path,
            binding: Some(binding),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute> for CompiledRoute {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute,
    ) -> Result<Self, Self::Error> {
        let method = MethodPattern::try_from(value.method)?;
        let path = AllPathPatterns::parse(value.path.as_str())?;
        let binding = value.binding.ok_or("binding is missing".to_string())?;

        let binding = crate::gateway_binding::GatewayBindingCompiled::try_from(binding)?;

        Ok(CompiledRoute {
            method,
            path,
            binding,
        })
    }
}

impl From<MethodPattern> for grpc_apidefinition::HttpMethod {
    fn from(value: MethodPattern) -> Self {
        match value {
            MethodPattern::Get => grpc_apidefinition::HttpMethod::Get,
            MethodPattern::Post => grpc_apidefinition::HttpMethod::Post,
            MethodPattern::Put => grpc_apidefinition::HttpMethod::Put,
            MethodPattern::Delete => grpc_apidefinition::HttpMethod::Delete,
            MethodPattern::Patch => grpc_apidefinition::HttpMethod::Patch,
            MethodPattern::Head => grpc_apidefinition::HttpMethod::Head,
            MethodPattern::Options => grpc_apidefinition::HttpMethod::Options,
            MethodPattern::Trace => grpc_apidefinition::HttpMethod::Trace,
            MethodPattern::Connect => grpc_apidefinition::HttpMethod::Connect,
        }
    }
}

impl TryFrom<grpc_apidefinition::HttpRoute> for crate::gateway_api_definition::http::Route {
    type Error = String;

    fn try_from(value: grpc_apidefinition::HttpRoute) -> Result<Self, Self::Error> {
        let path = AllPathPatterns::parse(value.path.as_str())?;
        let binding_proto: golem_api_grpc::proto::golem::apidefinition::GatewayBinding =
            value.binding.ok_or("Missing binding type")?;
        let binding = GatewayBinding::try_from(binding_proto)?;

        let method: MethodPattern = value.method.try_into()?;

        let result = crate::gateway_api_definition::http::Route {
            method,
            path,
            binding,
        };

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use crate::gateway_api_definition::http::MethodPattern;
    use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;
    use test_r::test;

    #[test]
    fn test_method_pattern() {
        for method in 0..8 {
            let method_pattern: MethodPattern = method.try_into().unwrap();
            let method_grpc: grpc_apidefinition::HttpMethod = method_pattern.into();
            assert_eq!(method, method_grpc as i32);
        }
    }
}
