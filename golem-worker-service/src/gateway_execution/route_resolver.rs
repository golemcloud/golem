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

use super::api_definition_lookup::{ApiDefinitionLookupError, HttpApiDefinitionsLookup};
use super::request::authority_from_request;
use crate::config::RouteResolverConfig;
use crate::gateway_router::{build_router, RouteEntry, Router, RouterPattern};
use crate::model::{HttpMiddleware, RichCompiledRoute, RichGatewayBindingCompiled, SwaggerHtml};
use crate::swagger_ui::generate_swagger_html;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::account::AccountId;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{HttpApiDefinitionId, RouteMethod};
use golem_common::model::security_scheme::SecuritySchemeId;
use golem_service_base::custom_api::openapi::HttpApiDefinitionOpenApiSpec;
use golem_service_base::custom_api::path_pattern::AllPathPatterns;
use golem_service_base::custom_api::HttpCors;
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::custom_api::{CompiledRoute, CompiledRoutes};
use golem_service_base::custom_api::{RouteBehaviour, SwaggerUiBindingCompiled};
use std::collections::HashMap;
use std::sync::Arc;

pub struct ResolvedRouteEntry {
    pub domain: Domain,
    pub path_segments: Vec<String>,
    pub route_entry: RouteEntry,
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayBindingResolverError {
    #[error("Could not get domain from request: {0}")]
    CouldNotGetDomainFromRequest(String),
    #[error("Could not build router for domain")]
    CouldNotBuildRouter,
    #[error("No matching route for request")]
    NoMatchingRoute,
}

pub struct RouteResolver {
    router_cache: Cache<Domain, (), Router<RouteEntry>, ()>,
    api_definition_lookup: Arc<dyn HttpApiDefinitionsLookup>,
}

impl RouteResolver {
    pub fn new(
        config: &RouteResolverConfig,
        api_definition_lookup: Arc<dyn HttpApiDefinitionsLookup>,
    ) -> Self {
        Self {
            router_cache: Cache::new(
                Some(config.router_cache_max_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl: config.router_cache_ttl,
                    period: config.router_cache_eviction_period,
                },
                "route_resolver_routers",
            ),
            api_definition_lookup,
        }
    }

    pub async fn resolve_route(
        &self,
        request: &poem::Request,
    ) -> Result<ResolvedRouteEntry, GatewayBindingResolverError> {
        let authority = authority_from_request(request)
            .map_err(GatewayBindingResolverError::CouldNotGetDomainFromRequest)?;

        let domain = Domain(authority);

        let router = self
            .router_cache
            .get_or_insert_simple(&domain, async || {
                self.build_router_for_domain(&domain).await
            })
            .await
            .map_err(|_| GatewayBindingResolverError::CouldNotBuildRouter)?;

        let decoded_path = urlencoding::decode(request.uri().path())
            .map_err(|_| GatewayBindingResolverError::NoMatchingRoute)?;
        let path_segments: Vec<&str> = RouterPattern::split(&decoded_path).collect();

        let route_entry = router
            .check_path(request.method(), &path_segments)
            .ok_or(GatewayBindingResolverError::NoMatchingRoute)?;

        Ok(ResolvedRouteEntry {
            domain,
            path_segments: path_segments.into_iter().map(|s| s.to_string()).collect(),
            route_entry: route_entry.clone(),
        })
    }

    async fn build_router_for_domain(&self, domain: &Domain) -> Result<Router<RouteEntry>, ()> {
        let compiled_routes = self.api_definition_lookup.get(domain).await;

        let compiled_routes = match compiled_routes {
            Ok(value) => value,
            Err(ApiDefinitionLookupError::UnknownSite(_)) => return Ok(Router::new()),
            Err(ApiDefinitionLookupError::InternalError(err)) => {
                tracing::warn!("Failed to build router for domain {domain}: {err:?}");
                return Err(());
            }
        };

        let finalized_routes = match Self::finalize_routes(domain, compiled_routes).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("Failed to finalize routes for domain {domain}: {err:?}");
                return Err(());
            }
        };

        Ok(build_router(finalized_routes))
    }

    async fn finalize_routes(
        domain: &Domain,
        compiled_routes: CompiledRoutes,
    ) -> Result<Vec<RichCompiledRoute>, String> {
        let swagger_ui_htmls = Self::precompute_swagger_ui_htmls(
            domain,
            &compiled_routes.routes,
            &compiled_routes.security_schemes,
        )
        .await?;

        let cors_routes: HashMap<AllPathPatterns, HttpCors> = compiled_routes
            .routes
            .iter()
            .filter_map(|r| match &r.binding {
                RouteBehaviour::HttpCorsPreflight(inner) => {
                    Some((r.path.clone(), inner.http_cors.clone()))
                }
                _ => None,
            })
            .collect();

        let mut transformed_routes = Vec::with_capacity(compiled_routes.routes.len());

        for route in compiled_routes.routes {
            let mut middlewares = Vec::new();

            if let Some(security_scheme_id) = route.security_scheme {
                let security_scheme = compiled_routes
                    .security_schemes
                    .get(&security_scheme_id)
                    .ok_or("Security scheme {security_scheme_id} not found".to_string())?;
                middlewares.push(HttpMiddleware::AuthenticateRequest(security_scheme.clone()));
            };

            if route.method != RouteMethod::Options {
                if let Some(cors) = cors_routes.get(&route.path) {
                    middlewares.push(HttpMiddleware::Cors(cors.clone()));
                }
            }

            let transformed = RichCompiledRoute {
                method: route.method,
                path: route.path,
                binding: RichGatewayBindingCompiled::from_compiled_binding(
                    route.binding,
                    &swagger_ui_htmls,
                )?,
                middlewares,
                account_id: compiled_routes.account_id,
                environment_id: compiled_routes.environment_id,
            };

            transformed_routes.push(transformed);
        }

        let auth_call_back_routes = Self::get_auth_call_back_routes(
            &compiled_routes.account_id,
            &compiled_routes.environment_id,
            compiled_routes.security_schemes,
        )?;

        for auth_call_back_route in auth_call_back_routes {
            let existing_route = transformed_routes.iter().find(|r| {
                r.method == auth_call_back_route.method && r.path == auth_call_back_route.path
            });
            if existing_route.is_none() {
                transformed_routes.push(auth_call_back_route);
            }
        }

        Ok(transformed_routes)
    }

    async fn precompute_swagger_ui_htmls(
        domain: &Domain,
        compiled_routes: &[CompiledRoute],
        security_schemes: &HashMap<SecuritySchemeId, SecuritySchemeDetails>,
    ) -> Result<HashMap<HttpApiDefinitionId, Arc<SwaggerHtml>>, String> {
        let definitions_that_need_ui: HashMap<HttpApiDefinitionId, SwaggerUiBindingCompiled> =
            compiled_routes
                .iter()
                .filter_map(|r| match &r.binding {
                    RouteBehaviour::SwaggerUi(inner) => {
                        Some((inner.http_api_definition_id, *inner.clone()))
                    }
                    _ => None,
                })
                .collect();

        let mut swagger_uis = HashMap::with_capacity(definitions_that_need_ui.len());
        for (_, swagger_ui_binding) in definitions_that_need_ui {
            let matching_routes: Vec<&CompiledRoute> = compiled_routes
                .iter()
                .filter(|cr| cr.http_api_definition_id == swagger_ui_binding.http_api_definition_id)
                .collect();

            let openapi_definition = HttpApiDefinitionOpenApiSpec::from_routes(
                &swagger_ui_binding.http_api_definition_name,
                &swagger_ui_binding.http_api_definition_version,
                matching_routes,
                security_schemes,
            )
            .await?;

            let swagger_html = generate_swagger_html(&domain.0, openapi_definition)?;
            swagger_uis.insert(
                swagger_ui_binding.http_api_definition_id,
                Arc::new(swagger_html),
            );
        }

        Ok(swagger_uis)
    }

    fn get_auth_call_back_routes(
        account_id: &AccountId,
        environment_id: &EnvironmentId,
        security_schemes: HashMap<SecuritySchemeId, SecuritySchemeDetails>,
    ) -> Result<Vec<RichCompiledRoute>, String> {
        let mut routes = vec![];

        for (_, scheme) in security_schemes {
            // In a security scheme, the auth-call-back (aka redirect_url) is full URL
            // and not just the relative path
            let redirect_url = scheme.redirect_url.clone();
            let path = redirect_url.url().path();
            let path = AllPathPatterns::parse(path)?;
            let method = RouteMethod::Get;
            let binding = RichGatewayBindingCompiled::HttpAuthCallBack(Box::new(scheme));

            let route = RichCompiledRoute {
                path,
                method,
                binding,
                middlewares: Vec::new(),
                account_id: *account_id,
                environment_id: *environment_id,
            };

            routes.push(route)
        }

        Ok(routes)
    }
}
