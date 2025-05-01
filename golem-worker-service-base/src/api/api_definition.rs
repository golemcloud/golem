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
    AllPathPatterns, CompiledHttpApiDefinition, CompiledRoute, MethodPattern, RouteRequest,
};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_api_deployment::ApiSite;
use crate::gateway_binding::{
    GatewayBinding, GatewayBindingCompiled, HttpHandlerBinding, HttpHandlerBindingCompiled,
    StaticBinding, WorkerBinding, WorkerBindingCompiled,
};
use crate::gateway_middleware::{CorsPreflightExpr, HttpCors, HttpMiddleware, HttpMiddlewares};
use crate::gateway_security::{
    Provider, SecurityScheme, SecuritySchemeIdentifier, SecuritySchemeReference,
    SecuritySchemeWithProviderMetadata,
};
use crate::service::gateway::BoxConversionContext;
use golem_common::model::component::VersionedComponentId;
use golem_common::model::GatewayBindingType;
use golem_service_base::model::ComponentName;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use poem_openapi::*;
use rib::{RibInputTypeInfo, RibOutputTypeInfo};
use serde::{Deserialize, Serialize};
use std::result::Result;

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

impl HttpApiDefinitionRequest {
    pub async fn into_core(
        self,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<crate::gateway_api_definition::http::HttpApiDefinitionRequest, String> {
        let mut routes = Vec::new();

        for route_request_data in self.routes {
            let method = route_request_data.method.clone();
            let path = route_request_data.path.clone();

            match route_request_data.into_route_request(conversion_ctx).await {
                Ok(v) => {
                    routes.push(v);
                }
                Err(error) => {
                    Err(format!("Error in endpoint {method} {path}: {error}"))?;
                }
            }
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

impl HttpApiDefinitionResponseData {
    pub async fn from_compiled_http_api_definition<Namespace>(
        value: CompiledHttpApiDefinition<Namespace>,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<Self, String> {
        let mut routes = vec![];

        for route in value.routes {
            // We shouldn't expose auth call back binding to users
            // as it is giving away the internal details of the call back system that enables security.
            if !route.binding.is_static_auth_call_back_binding() {
                let route_with_type_info =
                    RouteResponseData::from_compiled_route(route, conversion_ctx).await?;
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
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct RouteRequestData {
    pub method: MethodPattern,
    pub path: String,
    pub binding: GatewayBindingData,
    pub security: Option<String>,
}

impl RouteRequestData {
    pub async fn into_route_request(
        self,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<RouteRequest, String> {
        let path = AllPathPatterns::parse(self.path.as_str())?;
        let binding = self.binding.into_gateway_binding(conversion_ctx).await?;

        let security = self.security.map(|s| SecuritySchemeReference {
            security_scheme_identifier: SecuritySchemeIdentifier::new(s),
        });

        Ok(RouteRequest {
            method: self.method,
            path,
            binding,
            security,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct RouteResponseData {
    pub method: MethodPattern,
    pub path: String,
    pub security: Option<String>,
    pub binding: GatewayBindingResponseData,
}

impl RouteResponseData {
    pub async fn from_compiled_route(
        value: CompiledRoute,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<Self, String> {
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
            binding: GatewayBindingResponseData::from_gateway_binding_compiled(
                value.binding,
                conversion_ctx,
            )
            .await?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GatewayBindingComponent {
    name: String,
    /// Version of the component. If not provided the latest version is used.
    /// Note that the version is only used to typecheck the various rib scripts and prevent component updates.
    /// During runtime, the actual version of the worker or the latest version (in case no worker was found) is used.
    version: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ResolvedGatewayBindingComponent {
    name: String,
    version: u64,
}

// GatewayBindingData is a user exposed structure of GatewayBinding
// GatewayBindingData is flattened here only to keep the REST API backward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GatewayBindingData {
    #[oai(rename = "bindingType")]
    pub binding_type: Option<GatewayBindingType>, // descriminator to keep backward compatibility
    // For binding type - worker/default and file-server
    // Optional only to keep backward compatibility
    pub component: Option<GatewayBindingComponent>,
    // worker-name is optional to keep backward compatibility
    // this is not required anymore with first class worker support in rib
    // which is embedded in response field
    pub worker_name: Option<String>,
    // For binding type - worker/default
    pub idempotency_key: Option<String>,
    // For binding type - worker/default and fileserver, this is required
    // For binding type cors-preflight, this is optional otherwise default cors-preflight settings
    // is used
    pub response: Option<String>,
    // For binding type - worker/default
    pub invocation_context: Option<String>,
}

impl GatewayBindingData {
    pub async fn into_gateway_binding(
        self,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<GatewayBinding, String> {
        let v = self.binding_type.clone();

        match v {
            Some(GatewayBindingType::Default) | Some(GatewayBindingType::FileServer) | None => {
                let response = self.response.ok_or("Missing response field in binding")?;
                let component = self.component.ok_or("Missing component field in binding")?;
                let component_name = ComponentName(component.name);

                let component_view = conversion_ctx.component_by_name(&component_name).await?;

                let response: crate::gateway_binding::ResponseMapping = {
                    let r = rib::from_string(response.as_str()).map_err(|e| e.to_string())?;
                    crate::gateway_binding::ResponseMapping(r)
                };

                let worker_name = self
                    .worker_name
                    .map(|name| rib::from_string(name.as_str()).map_err(|e| e.to_string()))
                    .transpose()?;

                let idempotency_key = if let Some(key) = &self.idempotency_key {
                    Some(rib::from_string(key).map_err(|e| e.to_string())?)
                } else {
                    None
                };

                let invocation_context = if let Some(invocation_context) = self.invocation_context {
                    Some(rib::from_string(invocation_context).map_err(|e| e.to_string())?)
                } else {
                    None
                };

                let worker_binding = WorkerBinding {
                    component_id: VersionedComponentId {
                        component_id: component_view.id,
                        version: component.version.unwrap_or(component_view.latest_version),
                    },
                    worker_name,
                    idempotency_key,
                    response_mapping: response,
                    invocation_context,
                };

                if v == Some(GatewayBindingType::FileServer) {
                    Ok(GatewayBinding::FileServer(worker_binding))
                } else {
                    Ok(GatewayBinding::Default(worker_binding))
                }
            }

            Some(GatewayBindingType::HttpHandler) => {
                let component = self.component.ok_or("Missing component field in binding")?;
                let component_name = ComponentName(component.name);

                let component_view = conversion_ctx.component_by_name(&component_name).await?;

                let worker_name = self
                    .worker_name
                    .map(|name| rib::from_string(name.as_str()).map_err(|e| e.to_string()))
                    .transpose()?;

                let idempotency_key = if let Some(key) = &self.idempotency_key {
                    Some(rib::from_string(key).map_err(|e| e.to_string())?)
                } else {
                    None
                };

                let binding = HttpHandlerBinding {
                    component_id: VersionedComponentId {
                        component_id: component_view.id,
                        version: component.version.unwrap_or(component_view.latest_version),
                    },
                    worker_name,
                    idempotency_key,
                };

                Ok(GatewayBinding::HttpHandler(binding))
            }

            Some(GatewayBindingType::CorsPreflight) => {
                let response_mapping = self.response;

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
                HttpMiddleware::Cors(cors0) => cors = Some(cors0.clone()),
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
    pub component: Option<ResolvedGatewayBindingComponent>, // Allowed only if bindingType is Default, FileServer or HttpServer
    pub worker_name: Option<String>, // Allowed only if bindingType is Default or FileServer
    pub idempotency_key: Option<String>, // Allowed only if bindingType is Default or FileServer
    pub invocation_context: Option<String>, // Allowed only if bindingType is Default or FileServer
    pub response: Option<String>,    // Allowed only if bindingType is Default or FileServer
    #[oai(rename = "bindingType")]
    pub binding_type: Option<GatewayBindingType>,
    pub response_mapping_input: Option<RibInputTypeInfo>, // If bindingType is Default or FileServer
    pub worker_name_input: Option<RibInputTypeInfo>,      // If bindingType is Default or FileServer
    pub idempotency_key_input: Option<RibInputTypeInfo>, // If bindingType is Default or FilerServer
    pub cors_preflight: Option<HttpCors>, // If bindingType is CorsPreflight (internally, a static binding)
    pub response_mapping_output: Option<RibOutputTypeInfo>, // If bindingType is Default or FileServer
}

impl GatewayBindingResponseData {
    pub async fn from_gateway_binding_compiled(
        value: GatewayBindingCompiled,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<Self, String> {
        let gateway_binding = value.clone();

        match gateway_binding {
            GatewayBindingCompiled::FileServer(worker_binding) => {
                Self::from_worker_binding_compiled(
                    worker_binding,
                    GatewayBindingType::FileServer,
                    conversion_ctx,
                )
                .await
            }
            GatewayBindingCompiled::Worker(worker_binding) => {
                Self::from_worker_binding_compiled(
                    worker_binding,
                    GatewayBindingType::Default,
                    conversion_ctx,
                )
                .await
            }
            GatewayBindingCompiled::HttpHandler(http_handler_binding) => {
                Self::from_http_handler_binding_compiled(
                    http_handler_binding,
                    GatewayBindingType::HttpHandler,
                    conversion_ctx,
                )
                .await
            }
            GatewayBindingCompiled::Static(static_binding) => {
                let binding_type = match static_binding {
                    StaticBinding::HttpCorsPreflight(_) => GatewayBindingType::CorsPreflight,
                    StaticBinding::HttpAuthCallBack(_) => {
                        return Err(
                            "Auth call back static binding not to be exposed to users".to_string()
                        )
                    }
                };

                Ok(GatewayBindingResponseData {
                    component: None,
                    worker_name: None,
                    idempotency_key: None,
                    invocation_context: None,
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

    async fn from_worker_binding_compiled(
        worker_binding: WorkerBindingCompiled,
        binding_type: GatewayBindingType,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<Self, String> {
        let component_view = conversion_ctx
            .component_by_id(&worker_binding.component_id.component_id)
            .await?;

        Ok(GatewayBindingResponseData {
            component: Some(ResolvedGatewayBindingComponent {
                name: component_view.name.0,
                version: worker_binding.component_id.version,
            }),
            worker_name: worker_binding
                .worker_name_compiled
                .as_ref()
                .map(|compiled| compiled.worker_name.to_string()),
            idempotency_key: worker_binding.idempotency_key_compiled.as_ref().map(
                |idempotency_key_compiled| idempotency_key_compiled.idempotency_key.to_string(),
            ),
            invocation_context: worker_binding.invocation_context_compiled.as_ref().map(
                |invocation_context_compiled| {
                    invocation_context_compiled.invocation_context.to_string()
                },
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
        })
    }

    async fn from_http_handler_binding_compiled(
        http_handler_binding: HttpHandlerBindingCompiled,
        binding_type: GatewayBindingType,
        conversion_ctx: &BoxConversionContext<'_>,
    ) -> Result<Self, String> {
        let component_view = conversion_ctx
            .component_by_id(&http_handler_binding.component_id.component_id)
            .await?;

        Ok(GatewayBindingResponseData {
            component: Some(ResolvedGatewayBindingComponent {
                name: component_view.name.0,
                version: http_handler_binding.component_id.version,
            }),
            worker_name: http_handler_binding
                .worker_name_compiled
                .as_ref()
                .map(|compiled| compiled.worker_name.to_string()),
            idempotency_key: http_handler_binding.idempotency_key_compiled.as_ref().map(
                |idempotency_key_compiled| idempotency_key_compiled.idempotency_key.to_string(),
            ),
            invocation_context: None,
            response: None,
            binding_type: Some(binding_type),
            response_mapping_input: None,
            worker_name_input: http_handler_binding
                .worker_name_compiled
                .map(|compiled| compiled.rib_input_type_info),
            idempotency_key_input: http_handler_binding
                .idempotency_key_compiled
                .map(|idempotency_key_compiled| idempotency_key_compiled.rib_input),
            cors_preflight: None,
            response_mapping_output: None,
        })
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

#[cfg(test)]
mod tests {
    use crate::{
        gateway_api_definition::http::MethodPattern,
        service::gateway::{ComponentView, ConversionContext},
    };
    use assert2::check;
    use async_trait::async_trait;
    use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;
    use golem_common::model::ComponentId;
    use golem_service_base::model::ComponentName;
    use test_r::test;
    use uuid::uuid;

    struct TestConversionContext;

    #[async_trait]
    impl ConversionContext for TestConversionContext {
        async fn component_by_name(&self, name: &ComponentName) -> Result<ComponentView, String> {
            if name.0 == "test-component" {
                Ok(ComponentView {
                    name: ComponentName("test-component".to_string()),
                    id: ComponentId(uuid!("0b6d9cd8-f373-4e29-8a5a-548e61b868a5")),
                    latest_version: 1,
                })
            } else {
                Err("unknown component name".to_string())
            }
        }
        async fn component_by_id(
            &self,
            _component_id: &ComponentId,
        ) -> Result<ComponentView, String> {
            unimplemented!()
        }
    }

    #[test]
    fn test_method_pattern() {
        for method in 0..8 {
            let method_pattern: MethodPattern = method.try_into().unwrap();
            let method_grpc: grpc_apidefinition::HttpMethod = method_pattern.into();
            assert_eq!(method, method_grpc as i32);
        }
    }

    #[test]
    async fn method_is_not_case_sensitive() {
        let yaml_string = r#"
          id: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
          version: 0.0.1
          draft: true
          routes:
          - method: post
            path: /good/syntax
            binding:
              component:
                name: test-component
                version: 0
              response: |
                  { status: 200, body: "x" }
          - method: GET
            path: /bad/syntax
            binding:
              component:
                name: test-component
                version: 0
              response: |
                  { status: 200, body: "x" }
        "#;

        let api: super::HttpApiDefinitionRequest = serde_yaml::from_str(yaml_string).unwrap();
        let result: Result<crate::gateway_api_definition::http::HttpApiDefinitionRequest, String> =
            api.into_core(&TestConversionContext.boxed()).await;

        check!(result.is_ok(), "Expected success");
    }

    #[test]
    async fn rib_syntax_error_reporting() {
        let yaml_string = r#"
          id: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
          version: 0.0.1
          draft: true
          routes:
          - method: Get
            path: /good/syntax
            binding:
              component:
                name: test-component
                version: 0
              response: |
                  let email = "user@test.com";
                  let temp_worker = instance("__accounts_proxy");
                  let user = temp_worker.get-user-name(email);
                  { status: 200, body: user }
          - method: Get
            path: /bad/syntax
            binding:
              component:
                name: test-component
                version: 0
              response: |
                  let email = "user@test.com";
                  lett temp_worker = instance("__accounts_proxy");
                  let user = temp_worker.get-user-name(email);
                  { status: 200, body: user }
        "#;

        let api: super::HttpApiDefinitionRequest = serde_yaml::from_str(yaml_string).unwrap();
        let result: Result<crate::gateway_api_definition::http::HttpApiDefinitionRequest, String> =
            api.into_core(&TestConversionContext.boxed()).await;

        let err = result.expect_err("Expected error");

        check!(
            err.contains("Parse error at line: 2"),
            "Error contains the rib line number"
        );
        check!(
            err.contains("/bad/syntax"),
            "Error contains the failing endpoint path"
        );
        check!(
            err.contains("GET"),
            "Error contains the failing endpoint's method"
        );
        check!(
            !err.contains("/good/syntax"),
            "Error does not contain the correct endpoint's path"
        );
    }

    #[test]
    async fn test_version_is_optional() {
        let yaml_string = r#"
          id: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
          version: 0.0.1
          draft: true
          routes:
          - method: post
            path: /good/syntax
            binding:
              component:
                name: test-component
              response: |
                  { status: 200, body: "x" }
          - method: GET
            path: /bad/syntax
            binding:
              component:
                name: test-component
                version: 0
              response: |
                  { status: 200, body: "x" }
        "#;

        let api: super::HttpApiDefinitionRequest = serde_yaml::from_str(yaml_string).unwrap();
        let result = api.into_core(&TestConversionContext.boxed()).await.unwrap();

        let post_route = result
            .routes
            .iter()
            .find(|r| r.method == MethodPattern::Post)
            .unwrap();
        let post_route_component_version = post_route.binding.get_component_id().unwrap().version;
        assert_eq!(post_route_component_version, 1)
    }
}
