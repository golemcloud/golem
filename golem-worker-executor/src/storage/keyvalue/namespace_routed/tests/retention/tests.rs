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
use test_r::test;

#[test]
fn the_oplog_retention_uses_persistent_storage() {
    let storage = NamespaceRoutedKeyValueStorage::new(
        Arc::new(InMemoryKeyValueStorage::new()),
        Arc::new(InMemoryKeyValueStorage::new()),
    );
    assert!(Arc::ptr_eq(
        storage.backend_for_namespace(&KeyValueStorageNamespace::AgentOplogRetention {
            owned_agent_id: golem_common::model::OwnedAgentId::new(
                golem_common::model::environment::EnvironmentId(uuid::Uuid::from_u128(1)),
                &AgentId::from_parts(7, 2),
            ),
        }),
        &storage.persistent,
    ));
}
