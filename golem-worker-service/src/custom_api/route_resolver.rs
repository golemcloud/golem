// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::RichRouteSecurity;
use super::api_definition_lookup::{ApiDefinitionLookupError, HttpApiDefinitionsLookup};
use super::model::RichCompiledRoute;
use super::openapi::HttpApiOpenApiSpec;
use crate::config::RouteResolverConfig;
use crate::custom_api::{
    OidcCallbackBehaviour, RichRouteBehaviour, RichSecuritySchemeRouteSecurity,
};
use golem_common::SafeDisplay;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::agent::http_files::HttpRequestTarget;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::SecuritySchemeId;
use golem_service_base::custom_api::router::Router;
use golem_service_base::custom_api::{
    CompiledRoutes, CorsOptions, PathSegment, RequestBodySchema, RouteMatch, RouteSecurity,
    SecuritySchemeDetails,
};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

pub struct ResolvedRouteEntry {
    pub domain: Domain,
    pub route: Arc<RichCompiledRoute>,
    pub captured_path_parameters: Vec<String>,
    pub request_target: HttpRequestTarget,
    pub openapi_spec: Option<Arc<HttpApiOpenApiSpec>>,
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
    domain_api_cache: Cache<Domain, (), DomainHttpApi, ()>,
    api_definition_lookup: Arc<dyn HttpApiDefinitionsLookup>,
}

impl RouteResolver {
    pub fn new(
        config: &RouteResolverConfig,
        api_definition_lookup: Arc<dyn HttpApiDefinitionsLookup>,
    ) -> Self {
        Self {
            domain_api_cache: Cache::new(
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

    pub async fn resolve_matching_route(
        &self,
        request: &poem::Request,
    ) -> Result<ResolvedRouteEntry, RouteResolverError> {
        let request_target = HttpRequestTarget::parse(
            request
                .uri()
                .path_and_query()
                .ok_or_else(|| RouteResolverError::MalformedPath("unsafe-path".into()))?
                .as_str(),
        )
        .map_err(RouteResolverError::MalformedPath)?;
        let domain = authority_from_request(request)
            .map_err(RouteResolverError::CouldNotGetDomainFromRequest)?;
        debug!("Resolving router for domain: {domain}");

        let domain_api = self.get_or_build_domain_api(&domain).await?;

        let path_segments: Vec<&str> = request_target
            .segments()
            .iter()
            .map(String::as_str)
            .collect();

        let (route_entry, captured_path_parameters) = domain_api
            .router
            .route(request.method(), &path_segments)
            .ok_or(RouteResolverError::NoMatchingRoute)?;

        debug!("Resolved route entry: {route_entry:?}");

        Ok(ResolvedRouteEntry {
            domain,
            captured_path_parameters,
            request_target,
            route: route_entry.clone(),
            openapi_spec: domain_api.openapi_spec.clone(),
        })
    }

    pub async fn invalidate_domain(&self, domain: &Domain) {
        self.domain_api_cache.remove(domain).await;
    }

    pub async fn clear_all(&self) {
        let keys = self.domain_api_cache.keys().await;
        for key in keys {
            self.domain_api_cache.remove(&key).await;
        }
    }

    pub async fn invalidate_domains_for_environment(&self, environment_id: EnvironmentId) {
        let entries = self.domain_api_cache.iter().await;
        for (domain, domain_api) in entries {
            if domain_api.environment_id == environment_id {
                self.domain_api_cache.remove(&domain).await;
            }
        }
    }

    async fn get_or_build_domain_api(
        &self,
        domain: &Domain,
    ) -> Result<DomainHttpApi, RouteResolverError> {
        self.domain_api_cache
            .get_or_insert_simple(domain, async || {
                self.fetch_and_build_domain_api(domain).await
            })
            .await
            .map_err(|_| RouteResolverError::CouldNotBuildRouter)
    }

    async fn fetch_and_build_domain_api(&self, domain: &Domain) -> Result<DomainHttpApi, ()> {
        let compiled_routes = self.api_definition_lookup.get(domain).await;

        let compiled_routes = match compiled_routes {
            Ok(value) => value,
            Err(ApiDefinitionLookupError::UnknownSite(_)) => {
                return Ok(DomainHttpApi {
                    environment_id: EnvironmentId(uuid::Uuid::nil()),
                    router: Router::new(),
                    mounts: Arc::new(Vec::new()),
                    openapi_spec: None,
                });
            }
            Err(ApiDefinitionLookupError::InternalError(err)) => {
                tracing::warn!("Failed to build router for domain {domain}: {err:?}");
                return Err(());
            }
        };

        let environment_id = compiled_routes.environment_id;
        let finalized_routes = match Self::finalize_routes(compiled_routes).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("Failed to finalize routes for domain {domain}: {err:?}");
                return Err(());
            }
        };

        let openapi_spec = match HttpApiOpenApiSpec::from_routes(&finalized_routes, domain) {
            Ok(spec) => Some(Arc::new(spec)),
            Err(e) => {
                tracing::warn!("Failed to build openapi spec for http api: {e}");
                None
            }
        };

        let (mounts, concrete_routes): (Vec<_>, Vec<_>) = finalized_routes
            .into_iter()
            .partition(|route| matches!(route.route_match, RouteMatch::MountPrefix));
        let router = build_router(concrete_routes);

        Ok(DomainHttpApi {
            environment_id,
            router,
            mounts: Arc::new(mounts),
            openapi_spec,
        })
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
            route.route_match.validate(&route.path, &route.behavior)?;
            let security = compile_route_security(&security_schemes, route.security)?;

            let enriched = RichCompiledRoute {
                account_id: compiled_routes.account_id,
                account_email: compiled_routes.account_email.clone(),
                environment_id: compiled_routes.environment_id,
                deployment_revision: compiled_routes.deployment_revision,
                route_id: route.route_id,
                route_match: route.route_match,
                path: route.path,
                body: route.body,
                behavior: route.behavior.into(),
                security,
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

            // This route could be ambiguous with other agent routes,
            // in which case it will fail to add to the router.
            // This is too late to show an error to the user, so just continue as best we can in that case.
            let callback_route = RichCompiledRoute {
                account_id: compiled_routes.account_id,
                account_email: compiled_routes.account_email.clone(),
                environment_id: compiled_routes.environment_id,
                deployment_revision: compiled_routes.deployment_revision,
                // TODO: Have some helper for synthethic vs user defined routes
                route_id: -1,
                route_match: golem_common::model::agent::HttpMethod::Get(
                    golem_common::model::Empty {},
                )
                .into(),
                path: redirect_url_path_segments,
                body: RequestBodySchema::Unused,
                behavior: RichRouteBehaviour::OidcCallback(OidcCallbackBehaviour {
                    security_scheme: scheme.clone(),
                }),
                security: RichRouteSecurity::None,
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
    crate::mcp::resolve_effective_host(request.headers())
        .map(Domain)
        .ok_or("No host header provided".to_string())
}

fn compile_route_security(
    security_schemes: &HashMap<SecuritySchemeId, Arc<SecuritySchemeDetails>>,
    security: RouteSecurity,
) -> Result<RichRouteSecurity, String> {
    match security {
        RouteSecurity::None => Ok(RichRouteSecurity::None),
        RouteSecurity::SessionFromHeader(inner) => Ok(RichRouteSecurity::SessionFromHeader(inner)),
        RouteSecurity::SecurityScheme(inner) => {
            let security_scheme_id = inner.security_scheme_id;

            let security_scheme = security_schemes
                .get(&security_scheme_id)
                .ok_or(format!("Security scheme {security_scheme_id} not found"))?
                .clone();

            Ok(RichRouteSecurity::SecurityScheme(
                RichSecuritySchemeRouteSecurity { security_scheme },
            ))
        }
    }
}

fn build_router(routes: Vec<RichCompiledRoute>) -> Router<Arc<RichCompiledRoute>> {
    let mut router = Router::new();

    for route in routes {
        let route_id = route.route_id;
        let RouteMatch::Method { method, .. } = &route.route_match else {
            continue;
        };
        let method = method
            .clone()
            .try_into()
            .expect("finalized route has a valid concrete method");
        if !router.add_route(method, route.path.clone(), Arc::new(route)) {
            tracing::warn!("Failed to add route with route_id {route_id}");
        }
    }

    router
}

#[derive(Clone)]
struct DomainHttpApi {
    environment_id: EnvironmentId,
    router: Router<Arc<RichCompiledRoute>>,
    #[allow(dead_code)]
    mounts: Arc<Vec<RichCompiledRoute>>,
    openapi_spec: Option<Arc<HttpApiOpenApiSpec>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::HttpMethod;
    use golem_common::model::component::ComponentId;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_service_base::custom_api::{CompiledRoute, RouteBehaviour, WebhookCallbackBehaviour};
    use test_r::test;

    struct LiteralLookup(Vec<String>);

    #[async_trait::async_trait]
    impl HttpApiDefinitionsLookup for LiteralLookup {
        async fn get(&self, _: &Domain) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
            Ok(CompiledRoutes {
                account_id: AccountId(uuid::Uuid::nil()),
                account_email: AccountEmail::new("test@example.com"),
                environment_id: EnvironmentId(uuid::Uuid::nil()),
                deployment_revision: DeploymentRevision::INITIAL,
                security_schemes: HashMap::new(),
                routes: vec![CompiledRoute {
                    route_id: 7,
                    route_match: HttpMethod::Get(Empty {}).into(),
                    path: self
                        .0
                        .iter()
                        .cloned()
                        .map(|value| PathSegment::Literal { value })
                        .collect(),
                    body: RequestBodySchema::Unused,
                    behavior: RouteBehaviour::WebhookCallback(WebhookCallbackBehaviour {
                        component_id: ComponentId(uuid::Uuid::nil()),
                    }),
                    security: RouteSecurity::None,
                    cors: CorsOptions {
                        allowed_patterns: vec![],
                    },
                }],
            })
        }
    }

    struct UnexpectedLookup;

    #[async_trait::async_trait]
    impl HttpApiDefinitionsLookup for UnexpectedLookup {
        async fn get(&self, _: &Domain) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
            panic!("Unsafe paths must be rejected before deployment lookup");
        }
    }

    #[test]
    async fn unsafe_paths_are_terminal_before_deployment_lookup() {
        let resolver =
            RouteResolver::new(&RouteResolverConfig::default(), Arc::new(UnexpectedLookup));
        for target in [
            "//",
            "/a//b",
            "/a%2Fb",
            "/a/%2e%2e",
            "/%ff",
            "/%zz",
            "/%00",
            "/a|b",
            "/[1]",
            "/{name}",
            "/a\\b",
            "/café",
        ] {
            let request = poem::Request::builder()
                .uri(target.parse().unwrap())
                .header("host", "example.com")
                .finish();
            assert!(
                matches!(
                    resolver.resolve_matching_route(&request).await,
                    Err(RouteResolverError::MalformedPath(_))
                ),
                "{target}"
            );
        }
    }

    #[test]
    async fn resolved_target_preserves_raw_path_and_decoded_boundaries() {
        for (target, expected_segments, expected_query, trailing_slash) in [
            ("/", vec![], None, false),
            ("/a?", vec!["a"], Some(""), false),
            (
                "/a%20b/%252f?x=+&x=%2f",
                vec!["a b", "%2f"],
                Some("x=+&x=%2f"),
                false,
            ),
        ] {
            let resolver = RouteResolver::new(
                &RouteResolverConfig::default(),
                Arc::new(LiteralLookup(
                    expected_segments.iter().map(|s| s.to_string()).collect(),
                )),
            );
            let request = poem::Request::builder()
                .uri(target.parse().unwrap())
                .header("host", "example.com")
                .finish();
            let resolved = resolver.resolve_matching_route(&request).await.unwrap();
            assert_eq!(resolved.route.route_id, 7, "{target}");
            assert_eq!(
                resolved.request_target.path(),
                target.split('?').next().unwrap()
            );
            assert_eq!(resolved.request_target.query(), expected_query);
            assert_eq!(resolved.request_target.segments(), expected_segments);
            assert_eq!(resolved.request_target.trailing_slash(), trailing_slash);
        }
    }
}
