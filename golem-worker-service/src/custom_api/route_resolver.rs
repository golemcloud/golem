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
use std::net::IpAddr;
use std::sync::Arc;
use tracing::debug;

pub struct ResolvedRouteEntry {
    pub domain: Domain,
    pub public_scheme: String,
    pub public_authority: String,
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
    #[error("No deployment for domain")]
    UnknownSite,
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
    trusted_ingress_addresses: Vec<IpAddr>,
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
            trusted_ingress_addresses: config.trusted_ingress_addresses.clone(),
        }
    }

    pub async fn resolve_matching_route(
        &self,
        request: &poem::Request,
    ) -> Result<ResolvedRouteEntry, RouteResolverError> {
        self.resolve_matching_route_for_method(request, request.method())
            .await
    }

    pub async fn resolve_matching_route_for_method(
        &self,
        request: &poem::Request,
        method: &http::Method,
    ) -> Result<ResolvedRouteEntry, RouteResolverError> {
        let request_target = HttpRequestTarget::parse(
            request
                .uri()
                .path_and_query()
                .ok_or_else(|| RouteResolverError::MalformedPath("unsafe-path".into()))?
                .as_str(),
        )
        .map_err(RouteResolverError::MalformedPath)?;
        let public_origin = self
            .public_origin(request)
            .map_err(RouteResolverError::CouldNotGetDomainFromRequest)?;
        let domain = Domain(public_origin.authority.clone());
        debug!("Resolving router for domain: {domain}");

        let domain_api = self.get_or_build_domain_api(&domain).await?;
        if !domain_api.domain_exists {
            return Err(RouteResolverError::UnknownSite);
        }

        let path_segments: Vec<&str> = request_target
            .segments()
            .iter()
            .map(String::as_str)
            .collect();

        let (route_entry, captured_path_parameters) = domain_api
            .select(method, &path_segments, request_target.trailing_slash())
            .ok_or(RouteResolverError::NoMatchingRoute)?;

        debug!("Resolved route entry: {route_entry:?}");

        Ok(ResolvedRouteEntry {
            domain,
            public_scheme: public_origin.scheme,
            public_authority: public_origin.authority,
            captured_path_parameters,
            request_target,
            route: route_entry.clone(),
            openapi_spec: domain_api.openapi_spec.clone(),
        })
    }

    pub(crate) fn public_origin(
        &self,
        request: &poem::Request,
    ) -> Result<super::http_envelope::Origin, String> {
        let trusted = request
            .remote_addr()
            .as_socket_addr()
            .is_some_and(|address| self.trusted_ingress_addresses.contains(&address.ip()));
        super::http_envelope::origin_from_request(request, trusted)
            .map_err(|_| "Invalid request origin".to_string())
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
                    domain_exists: false,
                    environment_id: EnvironmentId(uuid::Uuid::nil()),
                    reserved: [Router::new(), Router::new()],
                    typed: [Router::new(), Router::new()],
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
            .filter(|route| !matches!(route.behavior, RichRouteBehaviour::CorsPreflight(_)))
            .partition(|route| matches!(route.route_match, RouteMatch::MountPrefix));
        let (reserved, typed): (Vec<_>, Vec<_>) = concrete_routes.into_iter().partition(|route| {
            matches!(
                route.behavior,
                RichRouteBehaviour::OidcCallback(_)
                    | RichRouteBehaviour::WebhookCallback(_)
                    | RichRouteBehaviour::OpenApiSpec(_)
            )
        });
        let mut mounts: Vec<_> = mounts.into_iter().map(Arc::new).collect();
        mounts.sort_by(|a, b| mount_specificity(&b.path).cmp(mount_specificity(&a.path)));

        Ok(DomainHttpApi {
            domain_exists: true,
            environment_id,
            reserved: build_router(reserved)?,
            typed: build_router(typed)?,
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
            let security = compile_route_security(&security_schemes, route.security);

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

        let mut callbacks = security_schemes.values().collect::<Vec<_>>();
        callbacks.sort_by_key(|scheme| scheme.id);
        for scheme in callbacks {
            // Schemes are mutable independently of the selected deployment.
            let target = match HttpRequestTarget::parse(scheme.redirect_url.url().path()) {
                Ok(target) => target,
                Err(error) => {
                    tracing::warn!(scheme_id = %scheme.id, error = %error, "Ignoring invalid OIDC callback path");
                    continue;
                }
            };
            let redirect_url_path_segments: Vec<PathSegment> = target
                .segments()
                .iter()
                .cloned()
                .map(|value| PathSegment::Literal { value })
                .collect();

            let callback_route = RichCompiledRoute {
                account_id: compiled_routes.account_id,
                account_email: compiled_routes.account_email.clone(),
                environment_id: compiled_routes.environment_id,
                deployment_revision: compiled_routes.deployment_revision,
                // TODO: Have some helper for synthethic vs user defined routes
                route_id: -1,
                route_match: RouteMatch::Method {
                    method: golem_common::model::agent::HttpMethod::Get(
                        golem_common::model::Empty {},
                    ),
                    trailing_slash: target.trailing_slash(),
                },
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

fn compile_route_security(
    security_schemes: &HashMap<SecuritySchemeId, Arc<SecuritySchemeDetails>>,
    security: RouteSecurity,
) -> RichRouteSecurity {
    match security {
        RouteSecurity::None => RichRouteSecurity::None,
        RouteSecurity::Unavailable => RichRouteSecurity::Unavailable,
        RouteSecurity::SessionFromHeader(inner) => RichRouteSecurity::SessionFromHeader(inner),
        RouteSecurity::SecurityScheme(inner) => {
            match security_schemes.get(&inner.security_scheme_id) {
                Some(security_scheme) => {
                    RichRouteSecurity::SecurityScheme(RichSecuritySchemeRouteSecurity {
                        security_scheme: security_scheme.clone(),
                    })
                }
                None => RichRouteSecurity::Unavailable,
            }
        }
    }
}

fn build_router(routes: Vec<RichCompiledRoute>) -> Result<[Router<Arc<RichCompiledRoute>>; 2], ()> {
    let mut routers = [Router::new(), Router::new()];

    for route in routes {
        let RouteMatch::Method {
            method,
            trailing_slash,
        } = &route.route_match
        else {
            continue;
        };
        let method = method
            .clone()
            .try_into()
            .expect("finalized route has a valid concrete method");
        let callback = matches!(route.behavior, RichRouteBehaviour::OidcCallback(_));
        if !routers[usize::from(*trailing_slash)].add_route(
            method,
            route.path.clone(),
            Arc::new(route),
        ) {
            if callback {
                tracing::warn!("Ignoring conflicting OIDC callback binding");
                continue;
            }
            return Err(());
        }
    }

    Ok(routers)
}

fn mount_specificity(path: &[PathSegment]) -> impl Iterator<Item = bool> + '_ {
    path.iter()
        .map(|segment| matches!(segment, PathSegment::Literal { .. }))
}

#[derive(Clone)]
struct DomainHttpApi {
    domain_exists: bool,
    environment_id: EnvironmentId,
    reserved: [Router<Arc<RichCompiledRoute>>; 2],
    typed: [Router<Arc<RichCompiledRoute>>; 2],
    mounts: Arc<Vec<Arc<RichCompiledRoute>>>,
    openapi_spec: Option<Arc<HttpApiOpenApiSpec>>,
}

impl DomainHttpApi {
    fn select(
        &self,
        method: &http::Method,
        path: &[&str],
        trailing_slash: bool,
    ) -> Option<(&Arc<RichCompiledRoute>, Vec<String>)> {
        let index = usize::from(trailing_slash);
        self.reserved[index]
            .route(method, path)
            .or_else(|| self.typed[index].route(method, path))
            .or_else(|| {
                self.mounts.iter().find_map(|mount| {
                    if matches!(mount.behavior, RichRouteBehaviour::AgentFilesystem(_))
                        && method != http::Method::GET
                        && method != http::Method::HEAD
                    {
                        return None;
                    }
                    if mount.path.len() > path.len() {
                        return None;
                    }
                    let mut captures = Vec::new();
                    for (pattern, segment) in mount.path.iter().zip(path) {
                        match pattern {
                            PathSegment::Literal { value } if value == segment => {}
                            PathSegment::Variable { .. } => captures.push((*segment).to_string()),
                            _ => return None,
                        }
                    }
                    Some((mount, captures))
                })
            })
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use golem_common::model::Empty;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::HttpMethod;
    use golem_common::model::component::ComponentId;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_service_base::custom_api::{CompiledRoute, RouteBehaviour, WebhookCallbackBehaviour};
    use test_r::test;

    pub(in crate::custom_api) fn test_route(
        id: i32,
        path: &str,
        method: Option<&str>,
        kind: &str,
    ) -> CompiledRoute {
        use golem_common::model::agent::{AgentMode, AgentTypeName};
        use golem_common::model::component::ComponentRevision;
        use golem_common::schema::{InputSchema, OutputSchema, SchemaGraph, SchemaType};
        use golem_service_base::custom_api::{
            AgentFilesystemBehaviour, CallAgentBehaviour, CompiledInputSchema,
            CompiledOutputSchema, HttpRouterBehaviour, OpenApiSpecBehaviour, OpenApiSpecFormat,
        };
        let input = || CompiledInputSchema {
            graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
            input_schema: InputSchema::Parameters(vec![]),
        };
        let component_id = ComponentId(uuid::Uuid::nil());
        let component_revision = ComponentRevision::INITIAL;
        let agent_type = AgentTypeName(format!("agent{id}"));
        let behavior = match kind {
            "router" => RouteBehaviour::HttpRouter(HttpRouterBehaviour {
                component_id,
                component_revision,
                agent_type,
                constructor_input: input(),
                handler: None,
                openapi_provider_method: None,
                static_bindings: vec![],
                file_index: vec![],
            }),
            "filesystem" => RouteBehaviour::AgentFilesystem(AgentFilesystemBehaviour {
                component_id,
                component_revision,
                agent_type,
                constructor_input: input(),
                constructor_parameters: vec![],
                filesystem_bindings: vec![],
            }),
            "reserved" => RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                format: OpenApiSpecFormat::Json,
            }),
            _ => RouteBehaviour::CallAgent(CallAgentBehaviour {
                route_mode: golem_service_base::custom_api::AgentRouteMode::Rest,
                base_path_variables: 0,
                component_id,
                component_revision,
                agent_type,
                agent_mode: AgentMode::Durable,
                constructor_input: input(),
                constructor_parameters: vec![],
                phantom: false,
                method_name: "run".into(),
                method_input: input(),
                method_parameters: path
                    .split('/')
                    .filter(|segment| segment.starts_with('{'))
                    .enumerate()
                    .map(
                        |(index, _)| golem_service_base::custom_api::MethodParameter::Path {
                            path_segment_index: golem_service_base::model::SafeIndex::new(
                                index as u32,
                            ),
                            parameter_type: golem_service_base::custom_api::PathSegmentType::Str,
                        },
                    )
                    .collect(),
                expected_agent_response: CompiledOutputSchema {
                    graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                    output_schema: OutputSchema::Unit,
                },
                method_description: None,
                read_only: None,
            }),
        };
        CompiledRoute {
            route_id: id,
            route_match: method
                .map(|method| RouteMatch::Method {
                    method: HttpMethod::Custom(golem_common::model::agent::CustomHttpMethod {
                        value: method.into(),
                    }),
                    trailing_slash: path.len() > 1 && path.ends_with('/'),
                })
                .unwrap_or(RouteMatch::MountPrefix),
            path: path
                .trim_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|segment| {
                    if let Some(name) = segment.strip_prefix("{*").and_then(|s| s.strip_suffix('}'))
                    {
                        PathSegment::CatchAll {
                            display_name: name.into(),
                        }
                    } else if let Some(name) =
                        segment.strip_prefix('{').and_then(|s| s.strip_suffix('}'))
                    {
                        PathSegment::Variable {
                            display_name: name.into(),
                        }
                    } else {
                        PathSegment::Literal {
                            value: segment.into(),
                        }
                    }
                })
                .collect(),
            body: RequestBodySchema::Unused,
            behavior,
            security: RouteSecurity::None,
            cors: CorsOptions {
                allowed_patterns: vec![],
            },
        }
    }

    struct RoutesLookup(std::sync::Mutex<Option<CompiledRoutes>>);

    #[async_trait::async_trait]
    impl HttpApiDefinitionsLookup for RoutesLookup {
        async fn get(&self, _: &Domain) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
            Ok(self.0.lock().unwrap().take().unwrap())
        }
    }

    pub(in crate::custom_api) fn test_resolver(routes: Vec<CompiledRoute>) -> RouteResolver {
        RouteResolver::new(
            &RouteResolverConfig::default(),
            Arc::new(RoutesLookup(std::sync::Mutex::new(Some(CompiledRoutes {
                account_id: AccountId(uuid::Uuid::nil()),
                account_email: AccountEmail::new("test@example.com"),
                environment_id: EnvironmentId(uuid::Uuid::nil()),
                deployment_revision: DeploymentRevision::INITIAL,
                security_schemes: HashMap::new(),
                routes,
            })))),
        )
    }

    #[test]
    async fn shared_route_selection_corpus() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();
        for (id, expected) in [
            ("route-typed-404-terminal", Some(0)),
            ("route-concrete-method-only", Some(100)),
            ("route-literal-dead-end", Some(1)),
            ("route-root-empty-suffix", Some(100)),
            ("route-mount-root-trailing-slash", Some(100)),
            ("route-prefix-not-segment", None),
            ("route-child-exhaustion-no-parent", Some(101)),
            ("route-live-post-ineligible", Some(100)),
            ("route-plain-options-any", Some(100)),
            ("route-reserved-openapi", Some(200)),
            ("route-typed-parameter-before-literal-mount", Some(0)),
            ("route-typed-trailing-slash-distinct", Some(1)),
        ] {
            let case = corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == id)
                .unwrap();
            let input = &case["input"];
            let mut routes = Vec::new();
            for (key, base, kind) in [
                ("typed", 0, "typed"),
                ("mounts", 100, "router"),
                ("reserved", 200, "reserved"),
            ] {
                if let Some(entries) = input[key].as_array() {
                    for (index, entry) in entries.iter().enumerate() {
                        routes.push(test_route(
                            base + index as i32,
                            entry["path"].as_str().unwrap(),
                            entry["method"].as_str(),
                            entry["kind"].as_str().unwrap_or(kind),
                        ));
                    }
                }
            }
            // Input order must not determine specificity.
            routes.reverse();
            let resolver = test_resolver(routes);
            let request = poem::Request::builder()
                .uri(input["target"].as_str().unwrap().parse().unwrap())
                .method(input["method"].as_str().unwrap().parse().unwrap())
                .header("host", "example.com")
                .finish();
            let result = resolver.resolve_matching_route(&request).await;
            assert_eq!(
                result.as_ref().ok().map(|r| r.route.route_id),
                expected,
                "{id}"
            );
            if id == "route-literal-dead-end" || id == "route-typed-parameter-before-literal-mount"
            {
                assert_eq!(
                    result.unwrap().captured_path_parameters,
                    vec!["fixed"],
                    "{id}"
                );
            }
        }
    }

    #[test]
    async fn literal_mount_precedes_longer_parameter_mount_and_reserved_precedes_typed() {
        let resolver = test_resolver(vec![
            test_route(1, "/a/{id}/tail", None, "filesystem"),
            test_route(2, "/a/fixed", None, "router"),
            test_route(3, "/{*all}", Some("GET"), "typed"),
            test_route(4, "/openapi.json", Some("GET"), "reserved"),
        ]);
        for (method, path, expected) in [
            ("HEAD", "/a/fixed/tail/file", 2),
            ("GET", "/openapi.json", 4),
            ("GET", "/a/fixed/tail/file", 3),
            ("HEAD", "/a/other/tail/file", 1),
        ] {
            let request = poem::Request::builder()
                .uri(path.parse().unwrap())
                .method(method.parse().unwrap())
                .header("host", "example.com")
                .finish();
            let result = resolver.resolve_matching_route(&request).await.unwrap();
            assert_eq!(result.route.route_id, expected);
            if expected == 1 {
                assert_eq!(result.captured_path_parameters, vec!["other"]);
            }
        }
    }

    #[test]
    async fn oidc_callback_uses_shared_decoding_and_trailing_flag() {
        use golem_common::model::security_scheme::{Provider, SecuritySchemeName};
        for (raw, segments, trailing) in [
            ("/", vec![], false),
            ("/sign%20in/%252f/", vec!["sign in", "%2f"], true),
        ] {
            let scheme = SecuritySchemeDetails {
                id: SecuritySchemeId::new(),
                name: SecuritySchemeName("test".into()),
                provider_type: Provider::Google(Empty {}),
                client_id: openidconnect::ClientId::new("test".into()),
                client_secret: openidconnect::ClientSecret::new("test".into()),
                redirect_url: openidconnect::RedirectUrl::new(format!("https://example.com{raw}"))
                    .unwrap(),
                scopes: vec![],
            };
            let mut duplicate = scheme.clone();
            duplicate.id = SecuritySchemeId::new();
            let mut invalid = scheme.clone();
            invalid.id = SecuritySchemeId::new();
            invalid.redirect_url =
                openidconnect::RedirectUrl::new("https://example.com/bad%2fpath".into()).unwrap();
            let compiled = CompiledRoutes {
                account_id: AccountId(uuid::Uuid::nil()),
                account_email: AccountEmail::new("test@example.com"),
                environment_id: EnvironmentId(uuid::Uuid::nil()),
                deployment_revision: DeploymentRevision::INITIAL,
                security_schemes: HashMap::from([
                    (scheme.id, scheme),
                    (duplicate.id, duplicate),
                    (invalid.id, invalid),
                ]),
                routes: vec![test_route(5, "/health", Some("GET"), "typed")],
            };
            let routers =
                build_router(RouteResolver::finalize_routes(compiled).await.unwrap()).unwrap();
            assert_eq!(
                routers[0]
                    .route(&http::Method::GET, &["health"])
                    .unwrap()
                    .0
                    .route_id,
                5
            );
            assert!(routers[0].route(&http::Method::HEAD, &["health"]).is_none());
            assert!(
                routers[usize::from(trailing)]
                    .route(&http::Method::GET, &segments)
                    .is_some()
            );
            assert!(
                routers[usize::from(!trailing)]
                    .route(&http::Method::GET, &segments)
                    .is_none()
            );
        }
    }

    struct ChangingSecurityLookup(std::sync::atomic::AtomicBool);

    #[async_trait::async_trait]
    impl HttpApiDefinitionsLookup for ChangingSecurityLookup {
        async fn get(&self, domain: &Domain) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
            let mut routes = LiteralLookup(vec!["protected".into()]).get(domain).await?;
            let id = SecuritySchemeId(uuid::Uuid::nil());
            routes.routes[0].security = RouteSecurity::SecurityScheme(
                golem_service_base::custom_api::SecuritySchemeRouteSecurity {
                    security_scheme_id: id,
                },
            );
            if self.0.load(std::sync::atomic::Ordering::SeqCst) {
                routes.security_schemes.insert(
                    id,
                    SecuritySchemeDetails {
                        id,
                        name: golem_common::model::security_scheme::SecuritySchemeName(
                            "current".into(),
                        ),
                        provider_type: golem_common::model::security_scheme::Provider::Google(
                            Empty {},
                        ),
                        client_id: openidconnect::ClientId::new("test".into()),
                        client_secret: openidconnect::ClientSecret::new("test".into()),
                        redirect_url: openidconnect::RedirectUrl::new(
                            "https://example.com/callback".into(),
                        )
                        .unwrap(),
                        scopes: vec![],
                    },
                );
            }
            Ok(routes)
        }
    }

    #[test]
    async fn security_invalidation_reloads_current_policy_without_changing_selected_route() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let lookup = Arc::new(ChangingSecurityLookup(AtomicBool::new(false)));
        let resolver = RouteResolver::new(&RouteResolverConfig::default(), lookup.clone());
        for available in [false, true, false] {
            lookup.0.store(available, Ordering::SeqCst);
            resolver
                .invalidate_domains_for_environment(EnvironmentId(uuid::Uuid::nil()))
                .await;
            let request = poem::Request::builder()
                .uri("/protected".parse().unwrap())
                .header("host", "example.com")
                .finish();
            let route = resolver.resolve_matching_route(&request).await.unwrap();
            assert_eq!(route.route.route_id, 7);
            assert_eq!(
                matches!(route.route.security, RichRouteSecurity::SecurityScheme(_)),
                available
            );
            assert_eq!(
                matches!(route.route.security, RichRouteSecurity::Unavailable),
                !available
            );
        }
    }

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

    fn request_from_peer(
        headers: &[(&str, &str)],
        peer: std::net::IpAddr,
        version: http::Version,
        uri: &str,
        scheme: &'static str,
    ) -> poem::Request {
        let mut builder = http::Request::builder().uri(uri).version(version);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let (parts, _) = builder.body(()).unwrap().into_parts();
        let parts = poem::RequestParts::from((
            parts,
            poem::web::LocalAddr(poem::Addr::socket(([127, 0, 0, 1], 9006).into())),
            poem::web::RemoteAddr(poem::Addr::socket((peer, 1234).into())),
            scheme.parse::<http::uri::Scheme>().unwrap(),
        ));
        poem::Request::from_parts(parts, poem::Body::empty())
    }

    #[test]
    async fn canonical_origin_policy_is_enforced_through_route_resolution() {
        let trusted: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        let untrusted: std::net::IpAddr = "10.0.0.2".parse().unwrap();
        let config = RouteResolverConfig {
            trusted_ingress_addresses: vec![trusted],
            ..Default::default()
        };
        let cases = [
            (
                "untrusted-forwarding-ignored",
                untrusted,
                vec![
                    ("host", "Direct.EXAMPLE:8080"),
                    ("x-forwarded-proto", "https"),
                    ("x-forwarded-host", "public.example"),
                ],
                http::Version::HTTP_11,
                "/",
                Ok(("http", "direct.example:8080")),
            ),
            (
                "trusted-forwarding",
                trusted,
                vec![
                    ("host", "internal"),
                    ("x-forwarded-proto", "HTTPS"),
                    ("x-forwarded-host", "Public.EXAMPLE:443"),
                ],
                http::Version::HTTP_11,
                "/",
                Ok(("https", "public.example:443")),
            ),
            (
                "trusted-partial-forwarding",
                trusted,
                vec![("host", "internal"), ("x-forwarded-proto", "https")],
                http::Version::HTTP_11,
                "/",
                Err(()),
            ),
            (
                "trusted-forwarding-chain",
                trusted,
                vec![
                    ("host", "internal"),
                    ("x-forwarded-proto", "https"),
                    ("x-forwarded-host", "public.example, proxy"),
                ],
                http::Version::HTTP_11,
                "/",
                Err(()),
            ),
            (
                "duplicate-host",
                untrusted,
                vec![("host", "one.example"), ("host", "two.example")],
                http::Version::HTTP_11,
                "/",
                Err(()),
            ),
            (
                "h2-authority-host-conflict",
                untrusted,
                vec![("host", "other.example")],
                http::Version::HTTP_2,
                "https://example.com/",
                Err(()),
            ),
        ];
        for (name, peer, headers, version, uri, expected) in cases {
            let resolver = RouteResolver::new(&config, Arc::new(LiteralLookup(Vec::new())));
            let request = request_from_peer(&headers, peer, version, uri, "http");
            let result = resolver.resolve_matching_route(&request).await;
            match expected {
                Ok((scheme, authority)) => {
                    let resolved = result.unwrap_or_else(|error| panic!("{name}: {error}"));
                    assert_eq!(resolved.public_scheme, scheme, "{name}");
                    assert_eq!(resolved.public_authority, authority, "{name}");
                    assert_eq!(resolved.domain.0, authority, "{name}");
                }
                Err(()) => assert!(
                    matches!(
                        result,
                        Err(RouteResolverError::CouldNotGetDomainFromRequest(_))
                    ),
                    "{name}"
                ),
            }
        }
    }
}
