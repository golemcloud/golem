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

use super::*;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use golem_common::model::AgentId;
use test_r::test;

#[test]
fn snapshot_rejections_use_persistent_storage_not_status_cache() {
    let storage = NamespaceRoutedKeyValueStorage::new(
        Arc::new(InMemoryKeyValueStorage::new()),
        Arc::new(InMemoryKeyValueStorage::new()),
    );
    let agent_id = golem_common::model::OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId(uuid::Uuid::from_u128(1)),
        &AgentId::from_parts(7, 1),
    );
    assert!(Arc::ptr_eq(
        storage.backend_for_namespace(&KeyValueStorageNamespace::AgentRejectedPeriodicSnapshots {
            owned_agent_id: agent_id.clone(),
        }),
        &storage.persistent,
    ));
    assert!(Arc::ptr_eq(
        storage.backend_for_namespace(&KeyValueStorageNamespace::AgentStatus {
            owned_agent_id: agent_id,
            fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::nil()),
        }),
        &storage.cache,
    ));
}

#[test]
fn the_status_baseline_uses_persistent_storage() {
    let storage = NamespaceRoutedKeyValueStorage::new(
        Arc::new(InMemoryKeyValueStorage::new()),
        Arc::new(InMemoryKeyValueStorage::new()),
    );
    let agent_id = golem_common::model::OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId(uuid::Uuid::from_u128(1)),
        &AgentId::from_parts(7, 1),
    );
    assert!(Arc::ptr_eq(
        storage.backend_for_namespace(&KeyValueStorageNamespace::AgentStatusCheckpoint {
            owned_agent_id: agent_id,
            fingerprint: golem_common::model::AgentFingerprint(uuid::Uuid::nil()),
        }),
        &storage.persistent,
    ));
}

/// A write with an expiry to a namespace of the persistent backend fails, also when the
/// namespace is the status baseline. A write with an expiry to a derived cache succeeds.
#[test]
async fn an_expiry_is_refused_for_a_namespace_with_no_expiry() {
    let storage = NamespaceRoutedKeyValueStorage::new(
        Arc::new(InMemoryKeyValueStorage::new()),
        Arc::new(InMemoryKeyValueStorage::new()),
    );
    let owned_agent_id = golem_common::model::OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId(uuid::Uuid::from_u128(1)),
        &AgentId::from_parts(7, 2),
    );
    let fingerprint = golem_common::model::AgentFingerprint(uuid::Uuid::nil());
    let baseline = KeyValueStorageNamespace::AgentStatusCheckpoint {
        owned_agent_id: owned_agent_id.clone(),
        fingerprint,
    };
    let cache = KeyValueStorageNamespace::AgentStatus {
        owned_agent_id,
        fingerprint,
    };
    let expiry = Duration::from_secs(60);
    let sets: [(&str, &[u8]); 1] = [("core", b"v")];
    let set = |namespace| {
        storage.set_with_expiry("test", "set", "entity", namespace, "core", b"v", expiry)
    };
    let mutate = |namespace| {
        storage.compare_and_mutate_many(
            "test",
            "mutate",
            "entity",
            namespace,
            "core",
            None,
            &sets,
            &[],
            expiry,
        )
    };

    assert!(set(baseline.clone()).await.is_err());
    assert!(mutate(baseline).await.is_err());
    assert!(set(cache.clone()).await.is_ok());
    assert!(mutate(cache).await.is_ok());
}
