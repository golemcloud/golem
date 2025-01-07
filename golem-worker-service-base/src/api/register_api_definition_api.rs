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

use crate::gateway_api_definition::http::{
    AllPathPatterns, CompiledHttpApiDefinition, CompiledRoute, MethodPattern, Route, RouteRequest,
};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_api_deployment::ApiSite;
use crate::gateway_binding::{
    GatewayBinding, GatewayBindingCompiled, StaticBinding, WorkerBinding, WorkerBindingCompiled,
};
use crate::gateway_middleware::{CorsPreflightExpr, HttpCors, HttpMiddleware, HttpMiddlewares};
use crate::gateway_security::{
    Provider, SecurityScheme, SecuritySchemeIdentifier, SecuritySchemeReference,
    SecuritySchemeWithProviderMetadata,
};
use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;
use golem_common::model::GatewayBindingType;
use golem_service_base::model::VersionedComponentId;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use poem_openapi::*;
use rib::{RibInputTypeInfo, RibOutputTypeInfo};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
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
    pub security: Option<Vec<String>>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct SecuritySchemeData {
    pub provider_type: Provider,
    pub scheme_identifier: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

impl TryFrom<SecuritySchemeData> for SecurityScheme {
    type Error = String;

    fn try_from(value: SecuritySchemeData) -> Result<Self, Self::Error> {
        let provider_type = value.provider_type;
        let scheme_identifier = value.scheme_identifier;
        let client_id = ClientId::new(value.client_id);
        let client_secret = ClientSecret::new(value.client_secret);
        let redirect_url = RedirectUrl::new(value.redirect_url).map_err(|e| e.to_string())?;
        let scopes = value.scopes.into_iter().map(Scope::new).collect();

        Ok(SecurityScheme::new(
            provider_type,
            SecuritySchemeIdentifier::new(scheme_identifier),
            client_id,
            client_secret,
            redirect_url,
            scopes,
        ))
    }
}

impl From<SecuritySchemeWithProviderMetadata> for SecuritySchemeData {
    fn from(value: SecuritySchemeWithProviderMetadata) -> Self {
        let provider_type = value.security_scheme.provider_type();
        let scheme_identifier = value.security_scheme.scheme_identifier().to_string();
        let client_id = value.security_scheme.client_id().to_string();
        let client_secret = value.security_scheme.client_secret().secret().to_string();
        let redirect_url = value.security_scheme.redirect_url().to_string();
        let scopes = value
            .security_scheme
            .scopes()
            .iter()
            .map(|scope| scope.to_string())
            .collect();

        Self {
            provider_type,
            scheme_identifier,
            client_id,
            client_secret,
            redirect_url,
            scopes,
        }
    }
}

// HttpApiDefinitionResponse is a trimmed down version of CompiledHttpApiDefinition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct HttpApiDefinitionResponseData {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<RouteResponseData>,
    #[serde(default)]
    pub draft: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl<Namespace> TryFrom<CompiledHttpApiDefinition<Namespace>> for HttpApiDefinitionResponseData {
    type Error = String;
    fn try_from(value: CompiledHttpApiDefinition<Namespace>) -> Result<Self, String> {
        let mut routes = vec![];

        for route in value.routes {
            // We shouldn't expose auth call back binding to users
            // as it is giving away the internal details of the call back system that enables security.
            if !route.binding.is_static_auth_call_back_binding() {
                let route_with_type_info = RouteResponseData::try_from(route)?;
                routes.push(route_with_type_info);
            }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteRequestData {
    pub method: MethodPattern,
    pub path: String,
    pub binding: GatewayBindingData,
    pub cors: Option<HttpCors>,
    pub security: Option<String>,
}

impl TryFrom<RouteRequestData> for RouteRequest {
    type Error = String;
    fn try_from(value: RouteRequestData) -> Result<Self, String> {
        let path = AllPathPatterns::parse(value.path.as_str())?;
        let binding = GatewayBinding::try_from(value.binding.clone())?;

        let security = value.security.map(|s| SecuritySchemeReference {
            security_scheme_identifier: SecuritySchemeIdentifier::new(s),
        });

        Ok(Self {
            method: value.method,
            path,
            binding,
            security,
            cors: value.cors,
        })
    }
}

impl TryFrom<Route> for RouteRequestData {
    type Error = String;
    fn try_from(value: Route) -> Result<Self, String> {
        let method = value.method.clone();
        let path = value.path.to_string();
        let binding = GatewayBindingData::try_from(value.binding.clone())?;
        let security = value.middlewares.clone().and_then(|middlewares| {
            middlewares.get_http_authentication_middleware().map(|x| {
                x.security_scheme_with_metadata
                    .security_scheme
                    .scheme_identifier()
                    .to_string()
            })
        });

        let cors = value
            .middlewares
            .and_then(|middlewares| middlewares.get_cors_middleware());

        Ok(Self {
            method,
            path,
            binding,
            security,
            cors,
        })
    }
}

impl TryFrom<RouteRequest> for RouteRequestData {
    type Error = String;

    fn try_from(value: RouteRequest) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = GatewayBindingData::try_from(value.binding)?;
        let security = value
            .security
            .map(|s| s.security_scheme_identifier.to_string());

        let cors = value.cors;

        Ok(Self {
            method: value.method,
            path,
            binding,
            security,
            cors,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct RouteResponseData {
    pub method: MethodPattern,
    pub path: String,
    pub security: Option<String>,
    pub binding: GatewayBindingResponseData,
}

impl TryFrom<CompiledRoute> for RouteResponseData {
    type Error = String;
    fn try_from(value: CompiledRoute) -> Result<Self, String> {
        let method = value.method;
        let path = value.path.to_string();
        let security = value.middlewares.and_then(|middlewares| {
            middlewares
                .get_http_authentication_middleware()
                .map(|http_authentication_middleware| {
                    http_authentication_middleware
                        .security_scheme_with_metadata
                        .security_scheme
                        .scheme_identifier()
                        .to_string()
                })
        });

        Ok(Self {
            method,
            path,
            security,
            binding: GatewayBindingResponseData::try_from(value.binding)?,
        })
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

    // CORS binding type
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
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct MiddlewareData {
    pub cors: Option<HttpCors>,
    pub auth: Option<SecuritySchemeReferenceData>,
}

impl From<HttpMiddlewares> for MiddlewareData {
    fn from(value: HttpMiddlewares) -> Self {
        let mut cors = None;
        let mut auth = None;

        for i in value.0.iter() {
            match i {
                HttpMiddleware::AddCorsHeaders(cors0) => cors = Some(cors0.clone()),
                HttpMiddleware::AuthenticateRequest(auth0) => {
                    let security_scheme_reference = SecuritySchemeReferenceData::from(
                        auth0.security_scheme_with_metadata.clone(),
                    );
                    auth = Some(security_scheme_reference)
                }
            }
        }

        MiddlewareData { cors, auth }
    }
}

// Security-scheme that's exposed to the users of API definition registration
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
pub struct GatewayBindingResponseData {
    pub component_id: Option<VersionedComponentId>, // Optional to keep it backward compatible
    pub worker_name: Option<String>,                // If bindingType is Default or FileServer
    pub idempotency_key: Option<String>,            // If bindingType is Default or FileServer
    pub response: Option<String>, // Optional to keep it backward compatible. If bindingType is Default or FileServer
    #[oai(rename = "bindingType")]
    pub binding_type: Option<GatewayBindingType>,
    pub response_mapping_input: Option<RibInputTypeInfo>, // If bindingType is Default or FileServer
    pub worker_name_input: Option<RibInputTypeInfo>,      // If bindingType is Default or FileServer
    pub idempotency_key_input: Option<RibInputTypeInfo>, // If bindingType is Default or FilerServer
    pub cors_preflight: Option<HttpCors>, // If bindingType is CorsPreflight (internally, a static binding)
    pub response_mapping_output: Option<RibOutputTypeInfo>, // If bindingType is Default or FileServer
}

impl GatewayBindingResponseData {
    pub fn from_worker_binding_compiled(
        worker_binding: WorkerBindingCompiled,
        binding_type: GatewayBindingType,
    ) -> Self {
        GatewayBindingResponseData {
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
            binding_type: Some(binding_type),
            response_mapping_input: Some(worker_binding.response_compiled.rib_input),
            worker_name_input: worker_binding
                .worker_name_compiled
                .map(|compiled| compiled.rib_input_type_info),
            idempotency_key_input: worker_binding
                .idempotency_key_compiled
                .map(|idempotency_key_compiled| idempotency_key_compiled.rib_input),
            cors_preflight: None,
            response_mapping_output: worker_binding.response_compiled.rib_output,
        }
    }
}

impl TryFrom<GatewayBindingCompiled> for GatewayBindingResponseData {
    type Error = String;

    fn try_from(value: GatewayBindingCompiled) -> Result<Self, String> {
        let gateway_binding = value.clone();

        match gateway_binding {
            GatewayBindingCompiled::FileServer(worker_binding) => {
                Ok(GatewayBindingResponseData::from_worker_binding_compiled(
                    worker_binding,
                    GatewayBindingType::FileServer,
                ))
            }
            GatewayBindingCompiled::Worker(worker_binding) => {
                Ok(GatewayBindingResponseData::from_worker_binding_compiled(
                    worker_binding,
                    GatewayBindingType::Default,
                ))
            }
            GatewayBindingCompiled::Static(static_binding) => {
                let binding_type = match static_binding.deref() {
                    StaticBinding::HttpCorsPreflight(_) => GatewayBindingType::CorsPreflight,
                    StaticBinding::HttpAuthCallBack(_) => {
                        return Err(
                            "Auth call back static binding not to be exposed to users".to_string()
                        )
                    }
                };

                Ok(GatewayBindingResponseData {
                    component_id: None,
                    worker_name: None,
                    idempotency_key: None,
                    response: None,
                    binding_type: Some(binding_type),
                    response_mapping_input: None,
                    worker_name_input: None,
                    idempotency_key_input: None,
                    cors_preflight: static_binding.get_cors_preflight(),
                    response_mapping_output: None,
                })
            }
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
            // We shouldn't expose auth call back binding to users
            // as it is giving away the internal details of the call back system that enables security.
            if !route.binding.is_security_binding() {
                let v = RouteRequestData::try_from(route)?;
                routes.push(v);
            }
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

        for route_request_data in self.routes {
            let v = RouteRequest::try_from(route_request_data)?;
            routes.push(v);
        }

        Ok(
            crate::gateway_api_definition::http::HttpApiDefinitionRequest {
                id: self.id,
                version: self.version,
                security: self
                    .security
                    .map(|x| x.into_iter().map(SecuritySchemeReference::new).collect()),
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

            GatewayBinding::Static(static_binding) => match static_binding.deref() {
                StaticBinding::HttpCorsPreflight(cors) => Ok(GatewayBindingData {
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
                }),

                StaticBinding::HttpAuthCallBack(_) => {
                    Err("Auth call back static binding not to be exposed to users".to_string())
                }
            },
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

                let worker_binding = WorkerBinding {
                    component_id,
                    worker_name,
                    idempotency_key,
                    response_mapping: response,
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
                        let cors = HttpCors::from_cors_preflight_expr(&cors_preflight_expr)?;
                        Ok(GatewayBinding::static_binding(
                            StaticBinding::from_http_cors(cors),
                        ))
                    }
                    None => {
                        let cors = HttpCors::default();
                        Ok(GatewayBinding::static_binding(
                            StaticBinding::from_http_cors(cors),
                        ))
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

impl TryFrom<grpc_apidefinition::v1::ApiDefinitionRequest>
    for crate::gateway_api_definition::http::HttpApiDefinitionRequest
{
    type Error = String;

    fn try_from(value: grpc_apidefinition::v1::ApiDefinitionRequest) -> Result<Self, Self::Error> {
        let mut global_securities = vec![];
        let mut route_requests = vec![];

        match value.definition.ok_or("definition is missing")? {
            grpc_apidefinition::v1::api_definition_request::Definition::Http(http) => {
                for route in http.routes {
                    let route_request =
                        crate::gateway_api_definition::http::RouteRequest::try_from(route)?;
                    if let Some(security) = &route_request.security {
                        global_securities.push(security.clone());
                    }

                    route_requests.push(route_request);
                }
            }
        };

        let id = value.id.ok_or("Api Definition ID is missing")?;

        let security = if global_securities.is_empty() {
            None
        } else {
            Some(global_securities)
        };

        let result = crate::gateway_api_definition::http::HttpApiDefinitionRequest {
            id: ApiDefinitionId(id.value),
            version: ApiVersion(value.version),
            routes: route_requests,
            draft: value.draft,
            security,
        };

        Ok(result)
    }
}

impl TryFrom<crate::gateway_api_definition::http::Route> for grpc_apidefinition::HttpRoute {
    type Error = String;

    fn try_from(value: crate::gateway_api_definition::http::Route) -> Result<Self, Self::Error> {
        let path = value.path.to_string();
        let binding = grpc_apidefinition::GatewayBinding::try_from(value.binding)?;
        let method: grpc_apidefinition::HttpMethod = value.method.into();
        let middlewares = value.middlewares.clone();
        let middleware_proto = middlewares
            .map(golem_api_grpc::proto::golem::apidefinition::Middleware::try_from)
            .transpose()?;

        let result = grpc_apidefinition::HttpRoute {
            method: method as i32,
            path,
            binding: Some(binding),
            middleware: middleware_proto,
        };

        Ok(result)
    }
}

impl TryFrom<CompiledRoute> for golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute {
    type Error = String;

    fn try_from(value: CompiledRoute) -> Result<Self, Self::Error> {
        let method = value.method as i32;
        let path = value.path.to_string();
        let binding =
            golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding::try_from(
                value.binding,
            )?;

        let middleware_proto = value
            .middlewares
            .map(golem_api_grpc::proto::golem::apidefinition::Middleware::try_from)
            .transpose()?;

        Ok(Self {
            method,
            path,
            binding: Some(binding),
            middleware: middleware_proto,
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
        let binding_proto = value.binding.ok_or("binding is missing")?;
        let binding = GatewayBindingCompiled::try_from(binding_proto)?;
        let middlewares = value
            .middleware
            .map(HttpMiddlewares::try_from)
            .transpose()?;

        Ok(CompiledRoute {
            method,
            path,
            binding,
            middlewares,
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
        let binding = value.binding.ok_or("binding is missing")?;
        let method: MethodPattern = value.method.try_into()?;

        let gateway_binding = GatewayBinding::try_from(binding)?;
        let security = value.middleware.clone().and_then(|x| x.http_authentication);

        let security = security.and_then(|x| {
            x.security_scheme.map(|x| SecuritySchemeReference {
                security_scheme_identifier: SecuritySchemeIdentifier::new(x.scheme_identifier),
            })
        });

        let cors = value.middleware.and_then(|x| x.cors);

        let cors = cors.map(HttpCors::try_from).transpose()?;

        let result = crate::gateway_api_definition::http::RouteRequest {
            method,
            path,
            binding: gateway_binding,
            security,
            cors,
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
