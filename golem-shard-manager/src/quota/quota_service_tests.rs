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

use super::quota_service::{QuotaError, QuotaService};
use super::resource_definition_fetcher::{FetchError, ResourceDefinitionFetcher};
use crate::shard_manager_config::QuotaServiceConfig;
use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::resource_definition::{
    EnforcementAction, ResourceCapacityLimit, ResourceDefinition, ResourceDefinitionId,
    ResourceDefinitionRevision, ResourceLimit, ResourceName,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio::sync::RwLock;

fn test_config() -> QuotaServiceConfig {
    QuotaServiceConfig {
        definition_cache_max_capacity: 1024,
        definition_cache_ttl: Duration::from_secs(300),
        definition_cache_eviction_period: Duration::from_secs(60),
    }
}

fn short_ttl_config() -> QuotaServiceConfig {
    QuotaServiceConfig {
        definition_cache_max_capacity: 1024,
        definition_cache_ttl: Duration::from_millis(50),
        definition_cache_eviction_period: Duration::from_millis(25),
    }
}

fn make_definition(env_id: EnvironmentId, name: &str) -> ResourceDefinition {
    ResourceDefinition {
        id: ResourceDefinitionId(uuid::Uuid::new_v4()),
        revision: ResourceDefinitionRevision::INITIAL,
        environment_id: env_id,
        name: ResourceName(name.to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Reject,
        unit: "token".to_string(),
        units: "tokens".to_string(),
    }
}

fn make_definition_with_id(
    id: ResourceDefinitionId,
    env_id: EnvironmentId,
    name: &str,
    revision: ResourceDefinitionRevision,
) -> ResourceDefinition {
    ResourceDefinition {
        id,
        revision,
        environment_id: env_id,
        name: ResourceName(name.to_string()),
        limit: ResourceLimit::Capacity(ResourceCapacityLimit { value: 100 }),
        enforcement_action: EnforcementAction::Reject,
        unit: "token".to_string(),
        units: "tokens".to_string(),
    }
}

struct InMemoryFetcher {
    definitions: RwLock<HashMap<(EnvironmentId, ResourceName), ResourceDefinition>>,
}

impl InMemoryFetcher {
    fn new() -> Self {
        Self {
            definitions: RwLock::new(HashMap::new()),
        }
    }

    async fn put(&self, def: ResourceDefinition) {
        let key = (def.environment_id, def.name.clone());
        self.definitions.write().await.insert(key, def);
    }

    async fn remove(&self, env_id: EnvironmentId, name: &str) {
        self.definitions
            .write()
            .await
            .remove(&(env_id, ResourceName(name.to_string())));
    }
}

#[async_trait]
impl ResourceDefinitionFetcher for InMemoryFetcher {
    async fn get_by_id(&self, id: ResourceDefinitionId) -> Result<ResourceDefinition, FetchError> {
        self.definitions
            .read()
            .await
            .values()
            .find(|def| def.id == id)
            .cloned()
            .ok_or(FetchError::NotFound)
    }

    async fn get_by_name(
        &self,
        environment_id: EnvironmentId,
        name: ResourceName,
    ) -> Result<ResourceDefinition, FetchError> {
        self.definitions
            .read()
            .await
            .get(&(environment_id, name))
            .cloned()
            .ok_or(FetchError::NotFound)
    }
}

fn assert_not_found(result: Result<ResourceDefinition, QuotaError>) {
    assert!(
        matches!(
            result,
            Err(QuotaError::ResourceDefinitionNotFoundById(_))
                | Err(QuotaError::ResourceDefinitionNotFoundByName { .. })
        ),
        "expected not found, got: {result:?}"
    );
}

fn env_id() -> EnvironmentId {
    EnvironmentId(uuid::Uuid::new_v4())
}

#[test]
async fn get_by_id_returns_cached_definition() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def.clone()).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    let result = svc.get_by_id(id).await;
    assert_eq!(result.unwrap(), def);
}

#[test]
async fn get_by_id_returns_not_found_for_unknown_id() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let svc = QuotaService::new(test_config(), fetcher);

    let result = svc
        .get_by_id(ResourceDefinitionId(uuid::Uuid::new_v4()))
        .await;
    assert_not_found(result);
}

#[test]
async fn get_by_id_returns_not_found_for_tombstoned_entry() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    // Remove and trigger CDC to tombstone.
    fetcher.remove(env, "tokens").await;
    svc.on_resource_definition_changed(env, id, ResourceName("tokens".into()))
        .await;

    let result = svc.get_by_id(id).await;
    assert_not_found(result);
}

#[test]
async fn get_by_id_refreshes_stale_entry() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let rev1 = rev0.next().unwrap();
    let v1 = make_definition_with_id(id, env, "tokens", rev0);
    fetcher.put(v1.clone()).await;

    let svc = QuotaService::new(short_ttl_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    // Update in fetcher.
    let v2 = make_definition_with_id(id, env, "tokens", rev1);
    fetcher.put(v2.clone()).await;

    // Not stale yet — returns v1.
    let r1 = svc.get_by_id(id).await;
    assert_eq!(r1.unwrap(), v1);

    // Wait for TTL to expire.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stale — refreshes by id, returns v2.
    let r2 = svc.get_by_id(id).await;
    assert_eq!(r2.unwrap(), v2);
}

#[test]
async fn get_by_id_tombstones_on_stale_refresh_not_found() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(short_ttl_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    // Remove from fetcher.
    fetcher.remove(env, "tokens").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stale refresh discovers NotFound — tombstones.
    let result = svc.get_by_id(id).await;
    assert_not_found(result);
}

#[test]
async fn get_or_fetch_returns_definition_on_cache_miss() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def.clone()).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let result = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    assert_eq!(result.unwrap(), def);
}

#[test]
async fn get_or_fetch_returns_cached_value_on_second_call() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def.clone()).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    // Remove from fetcher — should still return cached value.
    fetcher.remove(env, "tokens").await;
    let result = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    assert_eq!(result.unwrap(), def);
}

#[test]
async fn get_or_fetch_returns_not_found_when_missing() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let svc = QuotaService::new(test_config(), fetcher);

    let result = svc
        .get_or_fetch(env_id(), ResourceName("missing".into()))
        .await;

    assert_not_found(result);
}

#[test]
async fn cache_ttl_eviction_causes_refetch() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let rev1 = rev0.next().unwrap();
    let v1 = make_definition_with_id(id, env, "tokens", rev0);
    fetcher.put(v1.clone()).await;

    let svc = QuotaService::new(short_ttl_config(), fetcher.clone());
    let r1 = svc.get_or_fetch(env, ResourceName("tokens".into())).await;
    assert_eq!(r1.unwrap(), v1);

    let v2 = make_definition_with_id(id, env, "tokens", rev1);
    fetcher.put(v2.clone()).await;

    // Wait for TTL + eviction period to expire the cache entry.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let r2 = svc.get_or_fetch(env, ResourceName("tokens".into())).await;
    assert_eq!(r2.unwrap(), v2);
}

#[test]
async fn cdc_event_tombstones_deleted_resource() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    // Remove and trigger CDC — will tombstone the entry.
    fetcher.remove(env, "tokens").await;
    svc.on_resource_definition_changed(env, id, ResourceName("tokens".into()))
        .await;

    // get_by_id returns NotFound for tombstoned entries.
    let result = svc.get_by_id(id).await;
    assert_not_found(result);

    // get_or_fetch re-fetches by name (cache was invalidated by CDC).
    let result2 = svc.get_or_fetch(env, ResourceName("tokens".into())).await;
    assert_not_found(result2);
}

#[test]
async fn cdc_event_refreshes_live_entry() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let rev1 = rev0.next().unwrap();
    let v1 = make_definition_with_id(id, env, "tokens", rev0);
    fetcher.put(v1).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    let v2 = make_definition_with_id(id, env, "tokens", rev1);
    fetcher.put(v2.clone()).await;

    svc.on_resource_definition_changed(env, id, ResourceName("tokens".into()))
        .await;

    // get_by_id returns the refreshed definition.
    let result = svc.get_by_id(id).await;
    assert_eq!(result.unwrap(), v2);
}

#[test]
async fn cdc_event_for_tombstoned_id_is_noop() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    // Remove from fetcher, then trigger CDC — will tombstone.
    fetcher.remove(env, "tokens").await;
    svc.on_resource_definition_changed(env, id, ResourceName("tokens".into()))
        .await;

    // Subsequent CDC events for the same id are no-ops (not live).
    svc.on_resource_definition_changed(env, id, ResourceName("tokens".into()))
        .await;

    let result = svc.get_by_id(id).await;
    assert_not_found(result);
}

#[test]
async fn cdc_invalidates_definition_cache() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let rev1 = rev0.next().unwrap();
    let v1 = make_definition_with_id(id, env, "tokens", rev0);
    fetcher.put(v1.clone()).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let r1 = svc.get_or_fetch(env, ResourceName("tokens".into())).await;
    assert_eq!(r1.unwrap(), v1);

    // Update in fetcher — cache still has old value.
    let v2 = make_definition_with_id(id, env, "tokens", rev1);
    fetcher.put(v2.clone()).await;

    let r2 = svc.get_or_fetch(env, ResourceName("tokens".into())).await;
    assert_eq!(r2.unwrap(), v1);

    // CDC invalidates the definition cache, next fetch gets v2.
    svc.on_resource_definition_changed(env, id, ResourceName("tokens".into()))
        .await;

    let r3 = svc.get_or_fetch(env, ResourceName("tokens".into())).await;
    assert_eq!(r3.unwrap(), v2);
}

// --- cursor expired ---

#[test]
async fn cursor_expired_refreshes_all_live_entries() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let rev1 = rev0.next().unwrap();
    let v1 = make_definition_with_id(id, env, "tokens", rev0);
    fetcher.put(v1).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    let v2 = make_definition_with_id(id, env, "tokens", rev1);
    fetcher.put(v2.clone()).await;

    svc.on_cursor_expired().await;

    let result = svc.get_by_id(id).await;
    assert_eq!(result.unwrap(), v2);
}

#[test]
async fn cursor_expired_clears_definition_cache() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let svc = QuotaService::new(test_config(), fetcher.clone());

    // Cache a not-found.
    let _ = svc.get_or_fetch(env, ResourceName("missing".into())).await;

    let def = make_definition(env, "missing");
    fetcher.put(def.clone()).await;

    svc.on_cursor_expired().await;

    // Cache was cleared, so re-fetches successfully.
    let result = svc.get_or_fetch(env, ResourceName("missing".into())).await;
    assert_eq!(result.unwrap(), def);
}

// --- name replaced (delete + recreate with same name, different id) ---

#[test]
async fn cdc_handles_name_replaced_with_new_id() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id1 = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let v1 = make_definition_with_id(id1, env, "tokens", rev0);
    fetcher.put(v1).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc.get_or_fetch(env, ResourceName("tokens".into())).await;

    // Replace with a new id under the same name.
    let id2 = ResourceDefinitionId(uuid::Uuid::new_v4());
    let v2 = make_definition_with_id(id2, env, "tokens", rev0);
    fetcher.put(v2.clone()).await;

    // CDC for old id: tombstones id1, invalidates "tokens" in definition cache.
    svc.on_resource_definition_changed(env, id1, ResourceName("tokens".into()))
        .await;

    // Next get_or_fetch resolves to v2 via cache miss.
    let result = svc.get_or_fetch(env, ResourceName("tokens".into())).await;
    assert_eq!(result.unwrap(), v2);

    // Old id is tombstoned.
    assert_not_found(svc.get_by_id(id1).await);
}
