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

use super::quota_lease::QuotaLease;
use super::quota_service::{QuotaError, QuotaService};
use super::resource_definition_fetcher::{FetchError, ResourceDefinitionFetcher};
use crate::model::Pod;
use crate::shard_manager_config::QuotaServiceConfig;
use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::resource_definition::{
    EnforcementAction, ResourceCapacityLimit, ResourceConcurrencyLimit, ResourceDefinition,
    ResourceDefinitionId, ResourceDefinitionRevision, ResourceLimit, ResourceName,
    ResourceRateLimit, TimePeriod,
};
use golem_service_base::model::quota_lease::{LeaseEpoch, QuotaAllocation};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio::sync::RwLock;

fn test_config() -> QuotaServiceConfig {
    QuotaServiceConfig {
        lease_duration: Duration::from_secs(60),
        definition_staleness_ttl: Duration::from_secs(300),
        min_executors: 2,
        exhausted_retry_after: Duration::from_secs(30),
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
    async fn fetch_by_id(
        &self,
        id: ResourceDefinitionId,
    ) -> Result<ResourceDefinition, FetchError> {
        self.definitions
            .read()
            .await
            .values()
            .find(|def| def.id == id)
            .cloned()
            .ok_or(FetchError::NotFound)
    }

    async fn resolve_by_name(
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

    async fn invalidate(&self, _environment_id: EnvironmentId, _name: ResourceName) {}

    async fn invalidate_all(&self) {}
}

fn assert_unlimited(lease: &QuotaLease) {
    assert!(
        matches!(lease, QuotaLease::Unlimited { .. }),
        "expected Unlimited, got: {lease:?}"
    );
}

impl QuotaLease {
    fn epoch(&self) -> LeaseEpoch {
        match self {
            QuotaLease::Bounded { epoch, .. } => *epoch,
            QuotaLease::Unlimited { .. } => panic!("epoch() called on Unlimited lease"),
        }
    }

    fn id(&self) -> ResourceDefinitionId {
        match self {
            QuotaLease::Bounded {
                resource_definition_id,
                ..
            } => *resource_definition_id,
            QuotaLease::Unlimited { .. } => panic!("id() called on Unlimited lease"),
        }
    }

    fn allocation(&self) -> &QuotaAllocation {
        match self {
            QuotaLease::Bounded { allocation, .. } => allocation,
            QuotaLease::Unlimited { .. } => panic!("allocation() called on Unlimited lease"),
        }
    }
}

fn env_id() -> EnvironmentId {
    EnvironmentId(uuid::Uuid::new_v4())
}

fn test_pod() -> Pod {
    Pod::new("localhost".to_string(), 9000)
}

fn test_pod_2() -> Pod {
    Pod::new("localhost".to_string(), 9001)
}

#[test]
async fn acquire_lease_returns_lease_for_existing_resource() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def.clone()).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let lease = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    assert_eq!(lease.id(), def.id);
    assert_eq!(lease.epoch(), LeaseEpoch::initial());
}

#[test]
async fn release_lease_fails_for_unknown_resource() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let svc = QuotaService::new(test_config(), fetcher);

    let result = svc
        .release_lease(
            ResourceDefinitionId(uuid::Uuid::new_v4()),
            test_pod(),
            LeaseEpoch::initial(),
            0,
        )
        .await;
    assert!(matches!(result, Err(QuotaError::LeaseNotFound { .. })));
}

#[test]
async fn acquire_lease_increments_epoch_on_repeated_calls() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();
    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();

    assert_eq!(l1.epoch(), LeaseEpoch::initial());
    assert_eq!(l2.epoch(), LeaseEpoch::initial().next());
}

#[test]
async fn acquire_lease_tracks_separate_epochs_per_pod() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod_2())
        .await
        .unwrap();

    // Both pods get initial epoch independently.
    assert_eq!(l1.epoch(), LeaseEpoch::initial());
    assert_eq!(l2.epoch(), LeaseEpoch::initial());
}

#[test]
async fn renew_lease_succeeds_with_correct_epoch() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();

    let l2 = svc
        .renew_lease(id, pod.clone(), l1.epoch(), 0)
        .await
        .unwrap();

    assert_eq!(l2.id(), id);
    assert_eq!(l2.epoch(), l1.epoch().next());
}

#[test]
async fn renew_lease_rejects_stale_epoch() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();
    let l2 = svc
        .renew_lease(id, pod.clone(), l1.epoch(), 0)
        .await
        .unwrap();

    let result = svc.renew_lease(id, pod.clone(), l1.epoch(), 0).await;
    assert!(matches!(result, Err(QuotaError::StaleEpoch { .. })));

    let l3 = svc
        .renew_lease(id, pod.clone(), l2.epoch(), 0)
        .await
        .unwrap();
    assert_eq!(l3.epoch(), l2.epoch().next());
}

#[test]
async fn renew_lease_fails_for_unknown_pod() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let _ = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    let result = svc
        .renew_lease(id, test_pod_2(), LeaseEpoch::initial(), 0)
        .await;
    assert!(matches!(result, Err(QuotaError::LeaseNotFound { .. })));
}

#[test]
async fn acquire_lease_returns_unlimited_for_missing_resource() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let svc = QuotaService::new(test_config(), fetcher);

    let lease = svc
        .acquire_lease(env_id(), ResourceName("missing".into()), test_pod())
        .await
        .unwrap();
    assert_unlimited(&lease);
}

#[test]
async fn renew_lease_fails_for_unknown_resource() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let svc = QuotaService::new(test_config(), fetcher);

    let result = svc
        .renew_lease(
            ResourceDefinitionId(uuid::Uuid::new_v4()),
            test_pod(),
            LeaseEpoch::initial(),
            0,
        )
        .await;
    assert!(matches!(result, Err(QuotaError::LeaseNotFound { .. })));
}

#[test]
async fn renew_lease_fails_for_tombstoned_resource() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();

    // Tombstone via CDC.
    fetcher.remove(env, "tokens").await;
    svc.on_resource_definition_changed(id).await;

    let result = svc.renew_lease(id, pod, l1.epoch(), 0).await;
    assert!(matches!(result, Err(QuotaError::LeaseNotFound { .. })));
}

#[test]
async fn cdc_tombstones_deleted_resource() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    fetcher.remove(env, "tokens").await;
    svc.on_resource_definition_changed(id).await;

    // Acquiring with the same name re-fetches — returns unlimited.
    let lease = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    assert_unlimited(&lease);
}

#[test]
async fn cdc_for_tombstoned_id_is_noop() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    fetcher.remove(env, "tokens").await;
    svc.on_resource_definition_changed(id).await;

    // Repeated CDC for same tombstoned id is a no-op.
    svc.on_resource_definition_changed(id).await;
}

#[test]
async fn release_lease_succeeds_with_correct_epoch() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();

    svc.release_lease(id, pod, l1.epoch(), 0).await.unwrap();
}

#[test]
async fn release_lease_removes_pod_from_leases() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();

    svc.release_lease(id, pod.clone(), l1.epoch(), 0)
        .await
        .unwrap();

    let result = svc.renew_lease(id, pod, l1.epoch(), 0).await;
    assert!(matches!(result, Err(QuotaError::LeaseNotFound { .. })));
}

#[test]
async fn release_lease_rejects_stale_epoch() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();
    let l2 = svc
        .renew_lease(id, pod.clone(), l1.epoch(), 0)
        .await
        .unwrap();

    let result = svc.release_lease(id, pod.clone(), l1.epoch(), 0).await;
    assert!(matches!(result, Err(QuotaError::StaleEpoch { .. })));

    svc.release_lease(id, pod, l2.epoch(), 0).await.unwrap();
}

#[test]
async fn release_lease_allows_re_acquire() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();

    svc.release_lease(id, pod.clone(), l1.epoch(), 0)
        .await
        .unwrap();

    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod)
        .await
        .unwrap();
    assert_eq!(l2.epoch(), LeaseEpoch::initial());
}

#[test]
async fn cdc_refreshes_definition_in_live_entry() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let rev1 = rev0.next().unwrap();
    let v1 = make_definition_with_id(id, env, "tokens", rev0);
    fetcher.put(v1).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    let v2 = make_definition_with_id(id, env, "tokens", rev1);
    fetcher.put(v2).await;

    svc.on_resource_definition_changed(id).await;

    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    assert_eq!(l2.id(), id);
    assert!(l2.epoch() > l1.epoch());
}

#[test]
async fn cursor_expired_refreshes_live_entries() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let rev1 = rev0.next().unwrap();
    let v1 = make_definition_with_id(id, env, "tokens", rev0);
    fetcher.put(v1).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    let v2 = make_definition_with_id(id, env, "tokens", rev1);
    fetcher.put(v2).await;

    svc.on_cursor_expired().await;

    // Lease should reflect updated definition after refresh.
    let lease = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(lease.id(), id);
}

#[test]
async fn cursor_expired_clears_definition_cache() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let svc = QuotaService::new(test_config(), fetcher.clone());

    // First acquire: resource doesn't exist — gets unlimited.
    let r1 = svc
        .acquire_lease(env, ResourceName("missing".into()), test_pod())
        .await
        .unwrap();
    assert_unlimited(&r1);

    let def = make_definition(env, "missing");
    fetcher.put(def.clone()).await;

    svc.on_cursor_expired().await;

    // Cache was cleared, so re-fetches successfully.
    let lease = svc
        .acquire_lease(env, ResourceName("missing".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(lease.id(), def.id);
}

#[test]
async fn cdc_handles_name_replaced_with_new_id() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let id1 = ResourceDefinitionId(uuid::Uuid::new_v4());
    let rev0 = ResourceDefinitionRevision::INITIAL;
    let v1 = make_definition_with_id(id1, env, "tokens", rev0);
    fetcher.put(v1).await;

    let svc = QuotaService::new(test_config(), fetcher.clone());
    let _ = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    // Replace with a new id under the same name.
    let id2 = ResourceDefinitionId(uuid::Uuid::new_v4());
    let v2 = make_definition_with_id(id2, env, "tokens", rev0);
    fetcher.put(v2).await;

    // CDC for old id: tombstones id1.
    // In production, the registry_event_subscriber also invalidates the fetcher cache.
    fetcher.invalidate(env, ResourceName("tokens".into())).await;
    svc.on_resource_definition_changed(id1).await;

    // Next acquire resolves to new id.
    let lease = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(lease.id(), id2);
    assert_eq!(lease.epoch(), LeaseEpoch::initial());
}

#[test]
async fn single_pod_gets_fair_share_not_full_budget() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    // min_executors=2, so single pod gets 100/2 = 50.
    let svc = QuotaService::new(test_config(), fetcher);
    let lease = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();

    assert_eq!(*lease.allocation(), QuotaAllocation::Budget { amount: 50 });
}

#[test]
async fn two_pods_each_get_fair_share() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    // min_executors=2.
    let svc = QuotaService::new(test_config(), fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    // First pod: 100 / max(1, 2) = 50.
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 50 });

    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod_2())
        .await
        .unwrap();
    // Second pod: 50 remaining / max(2, 2) = 25.
    assert_eq!(*l2.allocation(), QuotaAllocation::Budget { amount: 25 });
}

#[test]
async fn renew_returns_unused_and_reallocates() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    // Gets 50 (100 / min_executors=2).
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 50 });

    // Pod used 10, returning 40 unused.
    let l2 = svc
        .renew_lease(l1.id(), test_pod(), l1.epoch(), 40)
        .await
        .unwrap();

    // Remaining was 50, +40 returned = 90. Single pod: 90 / max(1,2) = 45.
    assert_eq!(*l2.allocation(), QuotaAllocation::Budget { amount: 45 });
}

#[test]
async fn release_returns_unused_to_pool() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 50 });

    // Release with 30 unused.
    svc.release_lease(id, test_pod(), l1.epoch(), 30)
        .await
        .unwrap();

    // Remaining was 50 + 30 returned = 80. New pod: 80 / max(1, 2) = 40.
    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod_2())
        .await
        .unwrap();
    assert_eq!(*l2.allocation(), QuotaAllocation::Budget { amount: 40 });
}

#[test]
async fn unused_capped_at_allocated() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    let svc = QuotaService::new(test_config(), fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 50 });

    // Executor claims 200 unused — capped at allocated (50).
    // Remaining was 50, +50 returned = 100. 100 / max(1,2) = 50.
    let l2 = svc
        .renew_lease(l1.id(), test_pod(), l1.epoch(), 200)
        .await
        .unwrap();
    assert_eq!(*l2.allocation(), QuotaAllocation::Budget { amount: 50 });
}

#[test]
async fn exhausted_when_no_remaining_budget() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    // min_executors=1 so first pod takes everything.
    let config = QuotaServiceConfig {
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 100 });

    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod_2())
        .await
        .unwrap();
    assert!(matches!(l2.allocation(), QuotaAllocation::Exhausted { .. }));
}

fn make_concurrency_definition(
    env_id: EnvironmentId,
    name: &str,
    value: u64,
) -> ResourceDefinition {
    ResourceDefinition {
        id: ResourceDefinitionId(uuid::Uuid::new_v4()),
        revision: ResourceDefinitionRevision::INITIAL,
        environment_id: env_id,
        name: ResourceName(name.to_string()),
        limit: ResourceLimit::Concurrency(ResourceConcurrencyLimit { value }),
        enforcement_action: EnforcementAction::Reject,
        unit: "slot".to_string(),
        units: "slots".to_string(),
    }
}

#[test]
async fn concurrency_expired_lease_returns_slots() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_concurrency_definition(env, "workers", 10);
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        lease_duration: Duration::from_millis(50),
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("workers".into()), pod.clone())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 10 });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Expired concurrency lease returns all slots.
    let l2 = svc
        .acquire_lease(env, ResourceName("workers".into()), test_pod_2())
        .await
        .unwrap();
    assert_eq!(*l2.allocation(), QuotaAllocation::Budget { amount: 10 });
}

#[test]
async fn concurrency_release_returns_all_slots() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_concurrency_definition(env, "workers", 10);
    let id = def.id;
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("workers".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 10 });

    // Release with unused=0 — but concurrency always returns all slots.
    svc.release_lease(id, test_pod(), l1.epoch(), 0)
        .await
        .unwrap();

    let l2 = svc
        .acquire_lease(env, ResourceName("workers".into()), test_pod_2())
        .await
        .unwrap();
    assert_eq!(*l2.allocation(), QuotaAllocation::Budget { amount: 10 });
}

#[test]
async fn capacity_expired_lease_loses_budget() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        lease_duration: Duration::from_millis(50),
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 100 });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Expired capacity lease loses budget — assume executor consumed it all.
    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod_2())
        .await
        .unwrap();
    assert!(matches!(l2.allocation(), QuotaAllocation::Exhausted { .. }));
}

#[test]
async fn capacity_release_returns_only_reported_unused() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    let id = def.id;
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 100 });

    // Executor consumed 60, returning 40 unused.
    svc.release_lease(id, test_pod(), l1.epoch(), 40)
        .await
        .unwrap();

    let l2 = svc
        .acquire_lease(env, ResourceName("tokens".into()), test_pod_2())
        .await
        .unwrap();
    // Only the 40 unused was returned, not the full 100.
    assert_eq!(*l2.allocation(), QuotaAllocation::Budget { amount: 40 });
}

fn make_rate_definition(
    env_id: EnvironmentId,
    name: &str,
    value: u64,
    period: TimePeriod,
    max: u64,
) -> ResourceDefinition {
    ResourceDefinition {
        id: ResourceDefinitionId(uuid::Uuid::new_v4()),
        revision: ResourceDefinitionRevision::INITIAL,
        environment_id: env_id,
        name: ResourceName(name.to_string()),
        limit: ResourceLimit::Rate(ResourceRateLimit { value, period, max }),
        enforcement_action: EnforcementAction::Throttle,
        unit: "request".to_string(),
        units: "requests".to_string(),
    }
}

#[test]
async fn rate_limit_allocates_from_pool() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_rate_definition(env, "api-calls", 100, TimePeriod::Second, 100);
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);

    // Pool starts at max=100. Single executor gets 100.
    let lease = svc
        .acquire_lease(env, ResourceName("api-calls".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*lease.allocation(), QuotaAllocation::Budget { amount: 100 });
}

#[test]
async fn rate_limit_divides_pool_across_executors() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_rate_definition(env, "api-calls", 100, TimePeriod::Minute, 100);
    fetcher.put(def).await;

    // min_executors=2, so single pod gets half the pool (100/2 = 50).
    let svc = QuotaService::new(test_config(), fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("api-calls".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 50 });

    let l2 = svc
        .acquire_lease(env, ResourceName("api-calls".into()), test_pod_2())
        .await
        .unwrap();
    // Remaining 50 / max(2, 2) = 25.
    assert_eq!(*l2.allocation(), QuotaAllocation::Budget { amount: 25 });
}

#[test]
async fn rate_limit_renew_returns_unused_to_pool() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_rate_definition(env, "api-calls", 90, TimePeriod::Second, 90);
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);

    // Pod 1 acquires — gets full pool (90).
    let l1 = svc
        .acquire_lease(env, ResourceName("api-calls".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 90 });

    // Pod 2 acquires — pool is 0, gets Exhausted.
    let l2 = svc
        .acquire_lease(env, ResourceName("api-calls".into()), test_pod_2())
        .await
        .unwrap();
    assert!(matches!(l2.allocation(), QuotaAllocation::Exhausted { .. }));

    // Pod 1 renews returning 45 unused. Pool refills from rate + returned.
    // 45 returned, remaining = 45, divided by 2 active = 22.
    let l1_renewed = svc
        .renew_lease(l1.id(), test_pod(), l1.epoch(), 45)
        .await
        .unwrap();
    assert_eq!(
        *l1_renewed.allocation(),
        QuotaAllocation::Budget { amount: 22 }
    );
}

#[test]
async fn rate_limit_refills_after_full_period() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    // 100/second with max=100.
    let def = make_rate_definition(env, "api-calls", 100, TimePeriod::Second, 100);
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("api-calls".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 100 });

    // Consume all tokens, wait less than a full period — no refill.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let l2 = svc
        .renew_lease(l1.id(), test_pod(), l1.epoch(), 0)
        .await
        .unwrap();
    assert!(matches!(l2.allocation(), QuotaAllocation::Exhausted { .. }));

    // Wait for a full period to elapse from the start.
    tokio::time::sleep(Duration::from_millis(600)).await;

    let l3 = svc
        .renew_lease(l1.id(), test_pod(), l2.epoch(), 0)
        .await
        .unwrap();
    assert_eq!(*l3.allocation(), QuotaAllocation::Budget { amount: 100 });
}

#[test]
async fn rate_limit_no_partial_refill() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    // 1000/second with max=1000.
    let def = make_rate_definition(env, "api-calls", 1000, TimePeriod::Second, 1000);
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);

    let l1 = svc
        .acquire_lease(env, ResourceName("api-calls".into()), test_pod())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 1000 });

    // Wait 500ms — less than 1 full period. Should get nothing.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let l2 = svc
        .renew_lease(l1.id(), test_pod(), l1.epoch(), 0)
        .await
        .unwrap();
    // retry_after should hint at the time remaining until next full period (~500ms).
    match l2.allocation() {
        QuotaAllocation::Exhausted { retry_after } => {
            assert!(
                *retry_after >= Duration::from_millis(400)
                    && *retry_after <= Duration::from_millis(600),
                "expected retry_after ~500ms, got {retry_after:?}"
            );
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
async fn rate_limit_exhausted_retry_after_uses_config_fallback_not_period() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    // Capacity limit — no time-based refill, uses config fallback.
    let config = QuotaServiceConfig {
        min_executors: 1,
        exhausted_retry_after: Duration::from_secs(42),
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();

    let l2 = svc
        .renew_lease(l1.id(), pod.clone(), l1.epoch(), 0)
        .await
        .unwrap();
    match l2.allocation() {
        QuotaAllocation::Exhausted { retry_after } => {
            assert_eq!(*retry_after, Duration::from_secs(42));
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
}

#[test]
async fn capacity_limit_does_not_refill() {
    let fetcher = Arc::new(InMemoryFetcher::new());
    let env = env_id();
    let def = make_definition(env, "tokens");
    fetcher.put(def).await;

    let config = QuotaServiceConfig {
        min_executors: 1,
        ..test_config()
    };
    let svc = QuotaService::new(config, fetcher);
    let pod = test_pod();

    let l1 = svc
        .acquire_lease(env, ResourceName("tokens".into()), pod.clone())
        .await
        .unwrap();
    assert_eq!(*l1.allocation(), QuotaAllocation::Budget { amount: 100 });

    // Wait and renew with 0 unused — capacity does not refill.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let l2 = svc
        .renew_lease(l1.id(), pod.clone(), l1.epoch(), 0)
        .await
        .unwrap();
    assert!(matches!(l2.allocation(), QuotaAllocation::Exhausted { .. }));
}
