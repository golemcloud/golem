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

use async_trait::async_trait;
use futures::future::BoxFuture;
use golem_common::base_model::domain_registration::Domain;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::mcp::CompiledMcp;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

#[async_trait]
pub trait McpCapabilityLookup: Send + Sync {
    async fn get(&self, domain: &Domain) -> Result<CompiledMcp, McpCapabilitiesLookupError>;
    async fn invalidate(&self, domain: &Domain);
    async fn invalidate_all(&self);
}

#[derive(Debug, thiserror::Error)]
pub enum McpCapabilitiesLookupError {
    #[error("No mcp capabilities found for site {0}")]
    UnknownSite(Domain),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(McpCapabilitiesLookupError, RegistryServiceError);

impl SafeDisplay for McpCapabilitiesLookupError {
    fn to_safe_string(&self) -> String {
        match self {
            McpCapabilitiesLookupError::InternalError(_) => "Internal error".to_string(),
            McpCapabilitiesLookupError::UnknownSite(_) => "Unknown authority".to_string(),
        }
    }
}

/// TTL-based in-memory cache for compiled MCP lookups, consistent with
/// how HTTP uses `RouteResolver`'s `domain_api_cache`.
const MCP_CACHE_MAX_CAPACITY: usize = 1024;
/// Domains without an active MCP deployment are cached too, because the public listener looks
/// up the host of every request that is not for an HTTP API deployment. They have their own
/// cache, so requests for many unknown hosts cannot evict the deployments.
const MCP_NEGATIVE_CACHE_MAX_CAPACITY: usize = 4096;
const MCP_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const MCP_CACHE_EVICTION_PERIOD: Duration = Duration::from_secs(60);

// One initial attempt and two retries for invalidation races. Persistent churn
// fails the lookup rather than serving a result from before an invalidation.
const MAX_LOOKUP_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct McpCacheKey {
    domain: Domain,
    generation: u64,
    domain_generation: u64,
}

struct CachedMcp {
    created_at: Instant,
    deployment: Option<CompiledMcp>,
}

type CachedMcpCache = Cache<McpCacheKey, (), Arc<CachedMcp>, String>;

type FetchCompiledMcp = Arc<
    dyn Fn(Domain) -> BoxFuture<'static, Result<CompiledMcp, RegistryServiceError>> + Send + Sync,
>;

pub struct RegistryServiceMcpCapabilityLookup {
    fetch: FetchCompiledMcp,
    ttl: Duration,
    cache: CachedMcpCache,
    negative_cache: CachedMcpCache,
    /// Fences lookups that started before `invalidate_all`.
    generation: AtomicU64,
    /// Fences lookups of one domain that started before `invalidate` of that domain. Only
    /// domains of MCP deployments are invalidated one by one, which bounds the map.
    domain_generations: Mutex<HashMap<Domain, u64>>,
}

impl RegistryServiceMcpCapabilityLookup {
    pub fn new(registry_service_client: Arc<dyn RegistryService>) -> Self {
        Self::with_fetch(Arc::new(move |domain| {
            let registry_service_client = registry_service_client.clone();
            Box::pin(async move {
                registry_service_client
                    .get_active_compiled_mcps_for_domain(&domain)
                    .await
            })
        }))
    }

    fn with_fetch(fetch: FetchCompiledMcp) -> Self {
        Self::with_fetch_and_ttl(fetch, MCP_CACHE_TTL)
    }

    fn with_fetch_and_ttl(fetch: FetchCompiledMcp, ttl: Duration) -> Self {
        let cache = |capacity, name| {
            Cache::new(
                Some(capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl,
                    period: MCP_CACHE_EVICTION_PERIOD,
                },
                name,
            )
        };
        Self {
            fetch,
            ttl,
            cache: cache(MCP_CACHE_MAX_CAPACITY, "mcp_capability_lookup"),
            negative_cache: cache(
                MCP_NEGATIVE_CACHE_MAX_CAPACITY,
                "mcp_capability_lookup_negative",
            ),
            generation: AtomicU64::new(0),
            domain_generations: Mutex::new(HashMap::new()),
        }
    }

    fn key(&self, domain: &Domain) -> McpCacheKey {
        // A lookup compares its key before and after the fill, so a key that races an
        // invalidation only causes a retry.
        let generation = self.generation.load(Ordering::SeqCst);
        let domain_generation = self
            .domain_generations
            .lock()
            .unwrap()
            .get(domain)
            .copied()
            .unwrap_or_default();
        McpCacheKey {
            domain: domain.clone(),
            generation,
            domain_generation,
        }
    }

    async fn remove_cached(&self, predicate: impl Fn(&McpCacheKey) -> bool) {
        for cache in [&self.cache, &self.negative_cache] {
            for key in cache.keys().await {
                if predicate(&key) {
                    cache.remove_if_cached(&key, |_| true).await;
                }
            }
        }
    }

    async fn lookup(&self, key: &McpCacheKey) -> Result<Arc<CachedMcp>, String> {
        if let Some(cached) = self.cache.try_get(key).await {
            return Ok(cached);
        }
        // Lookups fill the negative cache, so the entry of a pending or unknown domain can only
        // take the place of another unknown domain. Found deployments move to the main cache.
        let fetch = self.fetch.clone();
        let domain = key.domain.clone();
        let cached = self
            .negative_cache
            .get_or_insert_simple_spawned(key, move || async move {
                let deployment = match fetch(domain).await {
                    Ok(compiled) => Some(compiled),
                    Err(RegistryServiceError::NotFound(_)) => None,
                    Err(error) => return Err(error.to_string()),
                };
                Ok(Arc::new(CachedMcp {
                    created_at: Instant::now(),
                    deployment,
                }))
            })
            .await?;
        if cached.deployment.is_some() {
            let found = cached.clone();
            let _ = self
                .cache
                .get_or_insert_simple(key, async || Ok(found))
                .await;
            self.negative_cache
                .remove_if_cached(key, |value| Arc::ptr_eq(value, &cached))
                .await;
        }
        Ok(cached)
    }

    async fn remove_if_same(&self, key: &McpCacheKey, cached: &Arc<CachedMcp>) {
        for cache in [&self.cache, &self.negative_cache] {
            cache
                .remove_if_cached(key, |value| Arc::ptr_eq(value, cached))
                .await;
        }
    }
}

#[async_trait]
impl McpCapabilityLookup for RegistryServiceMcpCapabilityLookup {
    async fn invalidate(&self, domain: &Domain) {
        *self
            .domain_generations
            .lock()
            .unwrap()
            .entry(domain.clone())
            .or_default() += 1;
        self.remove_cached(|key| &key.domain == domain).await;
    }

    async fn invalidate_all(&self) {
        // Lookups started before this call fill entries of an older generation.
        // Those entries are unreachable for later lookups and expire normally.
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.remove_cached(|key| key.generation < generation).await;
    }

    async fn get(&self, domain: &Domain) -> Result<CompiledMcp, McpCapabilitiesLookupError> {
        for _ in 0..MAX_LOOKUP_ATTEMPTS {
            let key = self.key(domain);
            let result = self.lookup(&key).await;
            if self.key(domain) != key {
                continue;
            }
            let cached = match result {
                Ok(cached) => cached,
                Err(error) => {
                    return Err(McpCapabilitiesLookupError::InternalError(anyhow::anyhow!(
                        "Failed to get compiled MCP for domain {}: {error}",
                        domain.0
                    )));
                }
            };
            // The caches evict entries that are idle, not entries that are old. A domain that is
            // looked up often must still see its deployment change within the TTL.
            if cached.created_at.elapsed() >= self.ttl {
                self.remove_if_same(&key, &cached).await;
                continue;
            }
            return match &cached.deployment {
                Some(compiled) => Ok(compiled.clone()),
                None => Err(McpCapabilitiesLookupError::UnknownSite(domain.clone())),
            };
        }
        Err(McpCapabilitiesLookupError::InternalError(anyhow::anyhow!(
            "MCP deployments for domain {} kept changing during the lookup",
            domain.0
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::application::ApplicationName;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use std::sync::atomic::AtomicUsize;
    use test_r::test;
    use tokio::sync::{mpsc, oneshot};

    fn compiled(domain: &Domain) -> CompiledMcp {
        CompiledMcp {
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.test"),
            application_name: ApplicationName("app".into()),
            environment_id: EnvironmentId::new(),
            environment_name: EnvironmentName("env".into()),
            deployment_revision: DeploymentRevision::INITIAL,
            domain: domain.clone(),
            security_scheme_name: None,
            security_scheme: None,
            registered_agent_types: vec![],
            tools: vec![],
        }
    }

    /// A registry that answers from a shared, mutable set of MCP domains and counts its calls.
    fn registry(
        domains: Arc<Mutex<Vec<Domain>>>,
        failing: Arc<Mutex<bool>>,
        calls: Arc<AtomicUsize>,
    ) -> RegistryServiceMcpCapabilityLookup {
        registry_with_ttl(domains, failing, calls, MCP_CACHE_TTL)
    }

    fn registry_with_ttl(
        domains: Arc<Mutex<Vec<Domain>>>,
        failing: Arc<Mutex<bool>>,
        calls: Arc<AtomicUsize>,
        ttl: Duration,
    ) -> RegistryServiceMcpCapabilityLookup {
        RegistryServiceMcpCapabilityLookup::with_fetch_and_ttl(
            Arc::new(move |domain: Domain| {
                calls.fetch_add(1, Ordering::SeqCst);
                let result = if *failing.lock().unwrap() {
                    Err(RegistryServiceError::internal_client_error(
                        "registry unavailable",
                    ))
                } else if domains.lock().unwrap().contains(&domain) {
                    Ok(compiled(&domain))
                } else {
                    Err(RegistryServiceError::NotFound(domain.0.clone()))
                };
                Box::pin(async move { result })
            }),
            ttl,
        )
    }

    #[test]
    async fn unknown_domains_are_cached_until_invalidation() {
        let domains = Arc::new(Mutex::new(vec![]));
        let calls = Arc::new(AtomicUsize::new(0));
        let lookup = registry(domains.clone(), Arc::new(Mutex::new(false)), calls.clone());
        let domain = Domain("tools.example.com".into());

        for _ in 0..3 {
            assert!(matches!(
                lookup.get(&domain).await,
                Err(McpCapabilitiesLookupError::UnknownSite(_))
            ));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        domains.lock().unwrap().push(domain.clone());
        lookup.invalidate_all().await;

        assert_eq!(lookup.get(&domain).await.ok().unwrap().domain, domain);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    async fn registry_failures_are_not_cached() {
        let domain = Domain("tools.example.com".into());
        let failing = Arc::new(Mutex::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        let lookup = registry(
            Arc::new(Mutex::new(vec![domain.clone()])),
            failing.clone(),
            calls.clone(),
        );

        assert!(matches!(
            lookup.get(&domain).await,
            Err(McpCapabilitiesLookupError::InternalError(_))
        ));
        *failing.lock().unwrap() = false;
        assert_eq!(lookup.get(&domain).await.ok().unwrap().domain, domain);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn a_fill_started_before_invalidation_cannot_hide_a_new_deployment() {
        type Reply = oneshot::Sender<Result<CompiledMcp, RegistryServiceError>>;
        let (requests_tx, mut requests) = mpsc::unbounded_channel::<Reply>();
        let lookup = Arc::new(RegistryServiceMcpCapabilityLookup::with_fetch(Arc::new(
            move |_domain: Domain| {
                let (tx, rx) = oneshot::channel();
                requests_tx.send(tx).unwrap();
                Box::pin(async move { rx.await.unwrap() })
            },
        )));
        let domain = Domain("tools.example.com".into());

        let stale_lookup = {
            let lookup = lookup.clone();
            let domain = domain.clone();
            tokio::spawn(async move { lookup.get(&domain).await })
        };
        let stale_fill = requests.recv().await.unwrap();

        // The MCP deployment appears while the first lookup is in flight.
        lookup.invalidate_all().await;
        assert!(
            stale_fill
                .send(Err(RegistryServiceError::NotFound(domain.0.clone())))
                .is_ok()
        );

        let fresh_fill = requests.recv().await.unwrap();
        assert!(fresh_fill.send(Ok(compiled(&domain))).is_ok());
        assert_eq!(stale_lookup.await.unwrap().ok().unwrap().domain, domain);

        // The stale "not found" fill must not answer later lookups either.
        assert_eq!(lookup.get(&domain).await.ok().unwrap().domain, domain);
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn a_fill_started_before_invalidating_its_domain_cannot_answer_later_lookups() {
        type Reply = oneshot::Sender<Result<CompiledMcp, RegistryServiceError>>;
        let (requests_tx, mut requests) = mpsc::unbounded_channel::<Reply>();
        let lookup = Arc::new(RegistryServiceMcpCapabilityLookup::with_fetch(Arc::new(
            move |_domain: Domain| {
                let (tx, rx) = oneshot::channel();
                requests_tx.send(tx).unwrap();
                Box::pin(async move { rx.await.unwrap() })
            },
        )));
        let domain = Domain("tools.example.com".into());
        let mut old = compiled(&domain);
        old.deployment_revision = DeploymentRevision::INITIAL;
        let mut new = compiled(&domain);
        new.deployment_revision = DeploymentRevision::INITIAL.next().unwrap();

        let stale_lookup = {
            let lookup = lookup.clone();
            let domain = domain.clone();
            tokio::spawn(async move { lookup.get(&domain).await })
        };
        let stale_fill = requests.recv().await.unwrap();

        lookup.invalidate(&domain).await;
        assert!(stale_fill.send(Ok(old)).is_ok());

        let fresh_fill = requests.recv().await.unwrap();
        assert!(fresh_fill.send(Ok(new.clone())).is_ok());
        assert_eq!(
            stale_lookup
                .await
                .unwrap()
                .ok()
                .unwrap()
                .deployment_revision,
            new.deployment_revision
        );
        assert_eq!(
            lookup.get(&domain).await.ok().unwrap().deployment_revision,
            new.deployment_revision
        );
    }

    #[test]
    async fn unknown_domains_do_not_take_the_place_of_deployments() {
        let deployed: Vec<Domain> = (0..MCP_CACHE_MAX_CAPACITY)
            .map(|index| Domain(format!("tools-{index}.example.com")))
            .collect();
        let calls = Arc::new(AtomicUsize::new(0));
        let lookup = registry(
            Arc::new(Mutex::new(deployed.clone())),
            Arc::new(Mutex::new(false)),
            calls.clone(),
        );

        for domain in &deployed {
            assert!(lookup.get(domain).await.is_ok());
        }
        for index in 0..(MCP_CACHE_MAX_CAPACITY + 10) {
            let unknown = Domain(format!("unknown-{index}.example.com"));
            assert!(lookup.get(&unknown).await.is_err());
        }

        let fetches = calls.load(Ordering::SeqCst);
        for domain in &deployed {
            assert!(lookup.get(domain).await.is_ok());
        }
        assert_eq!(calls.load(Ordering::SeqCst), fetches);
    }

    #[test]
    async fn cached_results_expire_after_the_ttl_even_when_used() {
        let ttl = Duration::from_millis(500);
        let domains = Arc::new(Mutex::new(vec![]));
        let calls = Arc::new(AtomicUsize::new(0));
        let lookup = registry_with_ttl(
            domains.clone(),
            Arc::new(Mutex::new(false)),
            calls.clone(),
            ttl,
        );
        let domain = Domain("tools.example.com".into());

        assert!(lookup.get(&domain).await.is_err());
        domains.lock().unwrap().push(domain.clone());
        assert!(lookup.get(&domain).await.is_err());
        tokio::time::sleep(ttl).await;

        assert_eq!(lookup.get(&domain).await.ok().unwrap().domain, domain);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
