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
use super::model::RichCompiledRoute;
use super::router::Router;
use crate::config::RouteResolverConfig;
use crate::custom_api::{OidcCallbackBehaviour, RichRouteBehaviour};
use golem_common::SafeDisplay;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::domain_registration::Domain;
use golem_service_base::custom_api::{CompiledRoute, CompiledRoutes, CorsOptions, PathSegment, RequestBodySchema};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

pub struct ResolvedRouteEntry {
    pub domain: Domain,
    pub route: Arc<RichCompiledRoute>,
    pub captured_path_parameters: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RouteResolverError {
    #[error("Could not get domain from request: {0}")]
    CouldNotGetDomainFromRequest(String),
    #[error("Could not build router for domain")]
    CouldNotBuildRouter,
    #[error("No matching route for request")]
    NoMatchingRoute,
    #[error("Could not decode request path: {0}")]
    MalformedPath(String),
}

impl SafeDisplay for RouteResolverError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

pub struct RouteResolver {
    router_cache: Cache<Domain, (), Router<Arc<RichCompiledRoute>>, ()>,
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

    pub async fn get_routes_for_domain(
        &self,
        domain: &Domain,
    ) -> Result<Vec<CompiledRoute>, RouteResolverError> {
        self.api_definition_lookup
            .get(domain)
            .await
            .map(|compiled_routes| compiled_routes.routes)
            .map_err(|e| match e {
                ApiDefinitionLookupError::UnknownSite(_) => RouteResolverError::NoMatchingRoute,
                _ => RouteResolverError::CouldNotBuildRouter,
            })
    }

    pub async fn resolve_matching_route(
        &self,
        request: &poem::Request,
    ) -> Result<ResolvedRouteEntry, RouteResolverError> {
        let domain = authority_from_request(request)
            .map_err(RouteResolverError::CouldNotGetDomainFromRequest)?;
        debug!("Resolving router for domain: {domain}");

        let router = self.get_or_build_router(&domain).await?;

        let decoded_path = urlencoding::decode(request.uri().path())
            .map_err(|err| RouteResolverError::MalformedPath(err.to_string()))?;

        let path_segments: Vec<&str> = split_path(&decoded_path).collect();

        let (route_entry, captured_path_parameters) = router
            .route(request.method(), &path_segments)
            .ok_or(RouteResolverError::NoMatchingRoute)?;

        debug!("Resolved route entry: {route_entry:?}");

        Ok(ResolvedRouteEntry {
            domain,
            captured_path_parameters,
            route: route_entry.clone(),
        })
    }

    async fn get_or_build_router(
        &self,
        domain: &Domain,
    ) -> Result<Router<Arc<RichCompiledRoute>>, RouteResolverError> {
        self.router_cache
            .get_or_insert_simple(domain, async || self.build_router_for_domain(domain).await)
            .await
            .map_err(|_| RouteResolverError::CouldNotBuildRouter)
    }

    async fn build_router_for_domain(
        &self,
        domain: &Domain,
    ) -> Result<Router<Arc<RichCompiledRoute>>, ()> {
        let compiled_routes = self.api_definition_lookup.get(domain).await;

        let compiled_routes = match compiled_routes {
            Ok(value) => value,
            Err(ApiDefinitionLookupError::UnknownSite(_)) => return Ok(Router::new()),
            Err(ApiDefinitionLookupError::InternalError(err)) => {
                tracing::warn!("Failed to build router for domain {domain}: {err:?}");
                return Err(());
            }
        };

        let finalized_routes = match Self::finalize_routes(compiled_routes).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("Failed to finalize routes for domain {domain}: {err:?}");
                return Err(());
            }
        };

        Ok(Router::build_router(finalized_routes))
    }

    async fn finalize_routes(
        compiled_routes: CompiledRoutes,
    ) -> Result<Vec<RichCompiledRoute>, String> {
        let security_schemes: HashMap<_, _> = compiled_routes
            .security_schemes
            .into_iter()
            .map(|(id, details)| (id, Arc::new(details)))
            .collect();

        let mut enriched_routes = Vec::with_capacity(compiled_routes.routes.len());

        for route in compiled_routes.routes {
            let security_scheme = if let Some(security_scheme_id) = route.security_scheme {
                let security_scheme = security_schemes
                    .get(&security_scheme_id)
                    .ok_or(format!("Security scheme {security_scheme_id} not found"))?
                    .clone();
                Some(security_scheme)
            } else {
                None
            };

            let enriched = RichCompiledRoute {
                account_id: compiled_routes.account_id,
                environment_id: compiled_routes.environment_id,
                route_id: route.route_id,
                method: route
                    .method
                    .try_into()
                    .map_err(|e| format!("Failed converting HttpMethod to http::Method: {e}"))?,
                path: route.path,
                body: route.body,
                behavior: route.behavior.into(),
                security_scheme,
                cors: route.cors,
            };

            enriched_routes.push(enriched);
        }

        // add synthethic oidc callback routes
        for scheme in security_schemes.values() {
            let redirect_url_path_segments: Vec<PathSegment> = scheme
                .redirect_url
                .url()
                .path_segments()
                .ok_or_else(|| "Failed splitting security scheme redirect url".to_string())?
                .map(|s| PathSegment::Literal {
                    value: s.to_string(),
                })
                .collect();

            let callback_route = RichCompiledRoute {
                account_id: compiled_routes.account_id,
                environment_id: compiled_routes.environment_id,
                // TODO: Have some helper for synthethic vs user defined routes
                route_id: -1,
                method: http::Method::GET,
                path: redirect_url_path_segments,
                body: RequestBodySchema::Unused,
                behavior: RichRouteBehaviour::OidcCallback(OidcCallbackBehaviour {
                    security_scheme: scheme.clone(),
                }),
                security_scheme: None,
                cors: CorsOptions {
                    allowed_patterns: Vec::new(),
                },
            };

            enriched_routes.push(callback_route);
        }

        Ok(enriched_routes)
    }
}

fn authority_from_request(request: &poem::Request) -> Result<Domain, String> {
    request
        .header(http::header::HOST)
        .map(|h| Domain(h.to_string()))
        .ok_or("No host header provided".to_string())
}

fn split_path(s: &str) -> impl Iterator<Item = &str> {
    s.trim_matches('/').split('/')
}
