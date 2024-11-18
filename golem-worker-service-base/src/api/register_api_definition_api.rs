use crate::gateway_api_definition::http::{
    AllPathPatterns, CompiledHttpApiDefinition, CompiledRoute, MethodPattern, Route, RouteRequest,
};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_api_deployment::ApiSite;
use crate::gateway_binding::{
    GatewayBinding, GatewayBindingCompiled, StaticBinding, WorkerBinding, WorkerBindingCompiled,
};
use crate::gateway_middleware::{Cors, CorsPreflightExpr, HttpMiddleware, Middleware};
use crate::gateway_security::{
    ProviderName, SecurityScheme, SecuritySchemeIdentifier, SecuritySchemeReference,
    SecuritySchemeWithProviderMetadata,
};
use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;
use golem_common::model::GatewayBindingType;
use golem_service_base::model::VersionedComponentId;
use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl, Scope};
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
    pub security: Option<SecuritySchemeReferenceData>,
    pub routes: Vec<RouteRequestData>,
    #[serde(default)]
    pub draft: bool,
}

// Mostly this data structures that represents the actual incoming request
// exist due to the presence of complicated Expr data type in gateway_api_definition::ApiDefinition.
// Consider them to be otherwise same
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct HttpApiDefinitionRequestData {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<RouteRequestData>,
    #[serde(default)]
    pub draft: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

// HttpApiDefinitionResponse is a trimmed down version of CompiledHttpApiDefinition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct HttpApiDefinitionResponseData {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub security: Option<SecuritySchemeReferenceData>,
    pub routes: Vec<RouteWithTypeInfo>,
    #[serde(default)]
    pub draft: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl<Namespace> From<CompiledHttpApiDefinition<Namespace>> for HttpApiDefinitionResponseData {
    fn from(value: CompiledHttpApiDefinition<Namespace>) -> Self {
        let routes = value.routes.into_iter().map(|route| route.into()).collect();

        Self {
            id: value.id,
            version: value.version,
            routes,
            security: value.security.map(|s| SecuritySchemeReferenceData::from(s)),
            draft: value.draft,
            created_at: Some(value.created_at),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteRequestData {
    pub method: MethodPattern,
    pub path: String,
    pub binding: GatewayBindingData,
    pub security: Option<SecuritySchemeReferenceData>,
}

impl TryFrom<RouteRequestData> for RouteRequest {
    type Error = String;
    fn try_from(value: RouteRequestData) -> Result<Self, String> {
        let path = AllPathPatterns::parse(value.path.as_str())?;
        let binding = GatewayBinding::try_from(value.binding)?;
        let security = value.security.map(|s| SecuritySchemeReference::from(s));
        Ok(Self {
            method: value.method,
            path,
            binding,
            security,
        })
    }
}

impl TryFrom<Route> for RouteRequestData {
    type Error = String;
    fn try_from(value: Route) -> Result<Self, String> {
        let method = value.method;
        let path = value.path.to_string();
        let binding = GatewayBindingData::try_from(value.binding)?;
        let security = value.security.map(|s| SecuritySchemeReferenceData::from(s));
        Ok(Self {
            method,
            path,
            binding,
            security,
        })
    }
}

impl TryFrom<RouteRequest> for RouteRequestData {
    type Error = String;

    fn try_from(value: RouteRequest) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = GatewayBindingData::try_from(value.binding)?;
        let security = value.security.map(|s| SecuritySchemeReferenceData::from(s));
        Ok(Self {
            method: value.method,
            path,
            binding,
            security,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteWithTypeInfo {
    pub method: MethodPattern,
    pub path: String,
    pub security: Option<SecuritySchemeReferenceData>,
    pub binding: GatewayBindingWithTypeInfo,
}

impl From<CompiledRoute> for RouteWithTypeInfo {
    fn from(value: CompiledRoute) -> Self {
        let method = value.method;
        let path = value.path.to_string();
        let binding = value.binding.into();
        let security = value.security.map(|s| SecuritySchemeReferenceData::from(s));
        Self {
            method,
            path,
            binding,
            security,
        }
    }
}

// GatewayBindingData is a user exposed structure of GatewayBinding
// GatewayBindingData is flattened here only to keep the REST API backward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GatewayBindingData {
    #[oai(rename = "bindingType")]
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
    pub expose_headers: Option<String>,
    //  For binding type - cors-middleware
    pub max_age: Option<u64>,
    //  For binding type - cors-middleware
    pub allow_credentials: Option<bool>,
}

impl GatewayBindingData {
    pub fn from_worker_binding(
        worker_binding: WorkerBinding,
        binding_type: GatewayBindingType,
    ) -> Result<Self, String> {
        let response: String =
            rib::to_string(&worker_binding.response_mapping.0).map_err(|e| e.to_string())?;

        let worker_id = worker_binding
            .worker_name
            .map(|expr| rib::to_string(&expr).map_err(|e| e.to_string()))
            .transpose()?;

        let idempotency_key = if let Some(key) = &worker_binding.idempotency_key {
            Some(rib::to_string(key).map_err(|e| e.to_string())?)
        } else {
            None
        };

        let mut cors = None;
        let mut auth = None;

        let middleware = if let Some(middlewares) = worker_binding.middleware {
            for m in middlewares.0.iter() {
                match m {
                    Middleware::Http(http_middleware) => match http_middleware {
                        HttpMiddleware::AddCorsHeaders(cors0) => {
                            cors = Some(cors0.clone());
                        }
                        HttpMiddleware::AuthenticateRequest(http_authorizer) => {
                            auth = Some(SecuritySchemeReferenceData::from(
                                http_authorizer.security_scheme.clone(),
                            ))
                        }
                    },
                }
            }

            Some(MiddlewareData { cors, auth })
        } else {
            None
        };

        Ok(Self {
            binding_type: Some(binding_type),
            component_id: Some(worker_binding.component_id),
            worker_name: worker_id,
            idempotency_key,
            response: Some(response),
            allow_origin: None,
            allow_methods: None,
            allow_headers: None,
            expose_headers: None,
            max_age: None,
            allow_credentials: None,
            middleware,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct MiddlewareData {
    pub cors: Option<Cors>,
    pub auth: Option<SecuritySchemeReferenceData>,
}

// Security Scheme data that's exposed to the users of API definition registration
// and deployment. Here we don't care any other part other than specifying the
// name of the security scheme. It is expected that this scheme is already registered with golem.
// Probably scopes are needed here as this is dynamic to each operation.
// Even provider name is not needed as golem can look up the provider type from the database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct SecuritySchemeReferenceData {
    security_scheme: String,
    // Additional scope support can go in future
}

impl From<SecuritySchemeWithProviderMetadata> for SecuritySchemeReferenceData {
    fn from(value: SecuritySchemeWithProviderMetadata) -> Self {
        Self {
            security_scheme: value.security_scheme.scheme_identifier().to_string(),
        }
    }
}

impl From<SecuritySchemeReference> for SecuritySchemeReferenceData {
    fn from(value: SecuritySchemeReference) -> Self {
        Self {
            security_scheme: value.security_scheme_identifier.to_string(),
        }
    }
}

impl From<SecuritySchemeReferenceData> for SecuritySchemeReference {
    fn from(value: SecuritySchemeReferenceData) -> Self {
        Self {
            security_scheme_identifier: SecuritySchemeIdentifier::new(value.security_scheme),
        }
    }
}

// GolemWorkerBindingWithTypeInfo is a subset of CompiledGolemWorkerBinding
// that it doesn't expose internal details such as byte code to be exposed
// to the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GatewayBindingWithTypeInfo {
    pub component_id: Option<VersionedComponentId>, // Optional to keep it backward compatible
    pub worker_name: Option<String>,
    pub idempotency_key: Option<String>,
    pub response: Option<String>, // Optional to keep it backward compatible
    #[oai(rename = "bindingType")]
    pub worker_binding_type: Option<GatewayBindingType>,
    pub response_mapping_input: Option<RibInputTypeInfo>,
    pub worker_name_input: Option<RibInputTypeInfo>,
    pub idempotency_key_input: Option<RibInputTypeInfo>,
    pub cors_preflight: Option<Cors>,
}

impl GatewayBindingWithTypeInfo {
    pub fn from_worker_binding_compiled(
        worker_binding: WorkerBindingCompiled,
        binding_type: GatewayBindingType,
    ) -> Self {
        GatewayBindingWithTypeInfo {
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
                    .response_mapping_expr
                    .to_string(),
            ),
            worker_binding_type: Some(binding_type),
            response_mapping_input: Some(worker_binding.response_compiled.rib_input),
            worker_name_input: worker_binding
                .worker_name_compiled
                .map(|compiled| compiled.rib_input_type_info),
            idempotency_key_input: worker_binding
                .idempotency_key_compiled
                .map(|idempotency_key_compiled| idempotency_key_compiled.rib_input),
            cors_preflight: None,
        }
    }
}

impl From<GatewayBindingCompiled> for GatewayBindingWithTypeInfo {
    fn from(value: GatewayBindingCompiled) -> Self {
        let gateway_binding = value.clone();

        match gateway_binding {
            GatewayBindingCompiled::FileServer(worker_binding) => {
                GatewayBindingWithTypeInfo::from_worker_binding_compiled(
                    worker_binding,
                    GatewayBindingType::FileServer,
                )
            }
            GatewayBindingCompiled::Worker(worker_binding) => {
                GatewayBindingWithTypeInfo::from_worker_binding_compiled(
                    worker_binding,
                    GatewayBindingType::Default,
                )
            }
            GatewayBindingCompiled::Static(static_binding) => GatewayBindingWithTypeInfo {
                component_id: None,
                worker_name: None,
                idempotency_key: None,
                response: None,
                worker_binding_type: None, // TODO; Remove worker_binding_type to not expose worker_function
                response_mapping_input: None,
                worker_name_input: None,
                idempotency_key_input: None,
                cors_preflight: static_binding.get_cors_preflight(),
            },
        }
    }
}

impl<N> From<crate::gateway_api_deployment::ApiDeployment<N>> for ApiDeployment {
    fn from(value: crate::gateway_api_deployment::ApiDeployment<N>) -> Self {
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

impl TryFrom<crate::gateway_api_definition::http::HttpApiDefinition>
    for HttpApiDefinitionRequestData
{
    type Error = String;

    fn try_from(
        value: crate::gateway_api_definition::http::HttpApiDefinition,
    ) -> Result<Self, Self::Error> {
        let mut routes = Vec::new();
        for route in value.routes {
            let v = RouteRequestData::try_from(route)?;
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

        for route_data in self.routes {
            let v = RouteRequest::try_from(route_data)?;
            routes.push(v);
        }

        Ok(
            crate::gateway_api_definition::http::HttpApiDefinitionRequest {
                id: self.id,
                version: self.version,
                security: self.security.map(|s| SecuritySchemeReference::from(s)),
                routes,
                draft: self.draft,
            },
        )
    }
}

impl TryFrom<GatewayBinding> for GatewayBindingData {
    type Error = String;

    fn try_from(value: GatewayBinding) -> Result<Self, Self::Error> {
        match value {
            GatewayBinding::Default(worker_binding) => {
                GatewayBindingData::from_worker_binding(worker_binding, GatewayBindingType::Default)
            }

            GatewayBinding::FileServer(worker_binding) => GatewayBindingData::from_worker_binding(
                worker_binding,
                GatewayBindingType::FileServer,
            ),

            GatewayBinding::Static(StaticBinding::HttpCorsPreflight(cors)) => Ok(Self {
                binding_type: Some(GatewayBindingType::CorsPreflight),
                component_id: None,
                worker_name: None,
                idempotency_key: None,
                response: None,
                allow_origin: Some(cors.get_allow_origin()),
                allow_methods: Some(cors.get_allow_methods()),
                allow_headers: Some(cors.get_allow_headers()),
                expose_headers: cors.get_expose_headers(),
                max_age: cors.get_max_age(),
                allow_credentials: cors.get_allow_credentials(),
                middleware: None,
            }),
            GatewayBinding::Static(StaticBinding::HttpAuthCallBack(auth)) => {
                unimplemented!("AuthCallBack is not supported in GatewayBindingData")
            }
        }
    }
}

impl TryFrom<GatewayBindingData> for GatewayBinding {
    type Error = String;

    fn try_from(gateway_binding_data: GatewayBindingData) -> Result<Self, Self::Error> {
        let v = gateway_binding_data.clone().binding_type;

        match v {
            Some(GatewayBindingType::Default) | Some(GatewayBindingType::FileServer) | None => {
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
                    response_mapping: response,
                    middleware: if middlewares.is_empty() {
                        None
                    } else {
                        Some(crate::gateway_middleware::Middlewares(middlewares))
                    },
                };

                if v == Some(GatewayBindingType::FileServer) {
                    Ok(GatewayBinding::FileServer(worker_binding))
                } else {
                    Ok(GatewayBinding::Default(worker_binding))
                }
            }

            Some(GatewayBindingType::CorsPreflight) => {
                let response_mapping = gateway_binding_data.response;

                match response_mapping {
                    Some(expr_str) => {
                        let expr = rib::from_string(expr_str).map_err(|e| e.to_string())?;
                        let cors_preflight_expr = CorsPreflightExpr(expr);
                        let cors = Cors::from_cors_preflight_expr(&cors_preflight_expr)?;
                        Ok(GatewayBinding::Static(StaticBinding::from_http_cors(cors)))
                    }
                    None => {
                        let cors = Cors::default();
                        Ok(GatewayBinding::Static(StaticBinding::from_http_cors(cors)))
                    }
                }
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

        let security = value
            .security
            .map(|x| {
                golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata::try_from(
                    x,
                )
            })
            .transpose()?;

        let definition = grpc_apidefinition::HttpApiDefinition { security, routes };

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

impl TryFrom<grpc_apidefinition::v1::ApiDefinitionRequest>
    for crate::gateway_api_definition::http::HttpApiDefinitionRequest
{
    type Error = String;

    fn try_from(value: grpc_apidefinition::v1::ApiDefinitionRequest) -> Result<Self, Self::Error> {
        let routes: Vec<RouteRequest> = match value.definition.ok_or("definition is missing")? {
            grpc_apidefinition::v1::api_definition_request::Definition::Http(http) => http
                .routes
                .into_iter()
                .map(crate::gateway_api_definition::http::RouteRequest::try_from)
                .collect::<Result<Vec<crate::gateway_api_definition::http::RouteRequest>, String>>(
                )?,
        };

        let id = value.id.ok_or("Api Definition ID is missing")?;

        let result = crate::gateway_api_definition::http::HttpApiDefinitionRequest {
            id: ApiDefinitionId(id.value),
            version: ApiVersion(value.version),
            routes: routes,
            draft: value.draft,
            security: None, //
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
        let security = value
            .security
            .map(|x| {
                golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata::try_from(
                    x,
                )
            })
            .transpose()?;

        let result = grpc_apidefinition::HttpRoute {
            method: method as i32,
            path,
            binding: Some(binding),
            security,
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

        let security = value
            .security
            .map(|x| {
                golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata::try_from(
                    x,
                )
            })
            .transpose()?;

        Ok(Self {
            method,
            path,
            binding: Some(binding),
            security,
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute> for CompiledRoute {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute,
    ) -> Result<Self, Self::Error> {
        let method = MethodPattern::try_from(value.method)?;
        let path = AllPathPatterns::parse(value.path.as_str()).map_err(|e| e.to_string())?;
        let binding = value.binding.ok_or("binding is missing")?.try_into()?;
        let security = value
            .security
            .map(|x| SecuritySchemeWithProviderMetadata::try_from(x))
            .transpose()?;

        Ok(CompiledRoute {
            method,
            path,
            binding,
            security,
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

impl TryFrom<grpc_apidefinition::HttpRoute> for crate::gateway_api_definition::http::RouteRequest {
    type Error = String;

    fn try_from(value: grpc_apidefinition::HttpRoute) -> Result<Self, Self::Error> {
        let path = AllPathPatterns::parse(value.path.as_str()).map_err(|e| e.to_string())?;
        let binding = value.binding.ok_or("binding is missing")?.try_into()?;

        let method: MethodPattern = value.method.try_into()?;

        let security = value
            .security
            .map(|x| {
                let security_scheme_result = x
                    .security_scheme
                    .ok_or("Missing security scheme".to_string());
                security_scheme_result.map(|x| SecuritySchemeReference {
                    security_scheme_identifier: SecuritySchemeIdentifier::new(x.scheme_identifier),
                })
            })
            .transpose()?;

        let result = crate::gateway_api_definition::http::RouteRequest {
            method,
            path,
            binding,
            security,
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
