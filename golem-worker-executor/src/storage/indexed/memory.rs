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

use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageMetaNamespace, IndexedStorageNamespace,
    ScanCursor,
};
use async_trait::async_trait;
use golem_common::model::AgentId;
use regex::Regex;
use std::collections::BTreeMap;
use std::ops::Bound::Included;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug)]
pub struct InMemoryIndexedStorage {
    data: scc::HashMap<String, BTreeMap<u64, Vec<u8>>>,
    #[cfg(test)]
    read_count: AtomicU64,
}

impl Default for InMemoryIndexedStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryIndexedStorage {
    pub fn new() -> Self {
        Self {
            data: scc::HashMap::new(),
            #[cfg(test)]
            read_count: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn read_count(&self) -> u64 {
        self.read_count.load(Ordering::Relaxed)
    }

    fn composite_key(namespace: IndexedStorageNamespace, key: &str) -> String {
        match namespace {
            IndexedStorageNamespace::OpLog {
                agent_id:
                    AgentId {
                        component_id,
                        agent_id: agent_name,
                    },
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/oplog/{component_id}/{agent_name}/{key}")
            }
            IndexedStorageNamespace::StagedOpLog {
                agent_id:
                    AgentId {
                        component_id,
                        agent_id: agent_name,
                    },
                agent_mode,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/staged-oplog/{component_id}/{agent_name}/{key}")
            }
            IndexedStorageNamespace::CompressedOpLog {
                agent_id:
                    AgentId {
                        component_id,
                        agent_id: agent_name,
                    },
                agent_mode,
                level,
            } => {
                let mode = super::agent_mode_prefix(agent_mode);
                format!("{mode}/compressed-oplog/{level}/{component_id}/{agent_name}/{key}")
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn match_key(
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
    ) -> Box<dyn Fn(&str) -> Option<String> + Send + Sync> {
        let prefix = prefix.unwrap_or("");
        match namespace {
            IndexedStorageMetaNamespace::Oplog { agent_mode } => {
                let mode = super::agent_mode_prefix(agent_mode);
                let pattern: String = format!(
                    r"^{mode}/oplog/([^/]+)/([^/]+)/({}.*)$",
                    regex::escape(prefix)
                );
                let regex = Regex::new(&pattern).unwrap();

                Box::new(move |key| {
                    regex
                        .captures(key)
                        .map(|caps| caps.get(3).unwrap().as_str().to_string())
                })
            }
            IndexedStorageMetaNamespace::CompressedOplog { agent_mode, level } => {
                let mode = super::agent_mode_prefix(agent_mode);
                let pattern: String = format!(
                    r"^{mode}/compressed-oplog/{level}/([^/]+)/([^/]+)/({}.*)$",
                    regex::escape(prefix)
                );
                let regex = Regex::new(&pattern).unwrap();

                Box::new(move |key| {
                    regex
                        .captures(key)
                        .map(|caps| caps.get(3).unwrap().as_str().to_string())
                })
            }
        }
    }
}

#[async_trait]
impl IndexedStorage for InMemoryIndexedStorage {
    async fn number_of_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
    ) -> Result<u8, IndexedStorageError> {
        Ok(0)
    }

    async fn wait_for_replicas(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<u8, IndexedStorageError> {
        Ok(0)
    }

    async fn exists(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<bool, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self.data.contains_async(&composite_key).await)
    }

    async fn scan(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageMetaNamespace,
        prefix: Option<&str>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<String>), IndexedStorageError> {
        let mut result = Vec::new();
        let matcher = Self::match_key(namespace, prefix);
        let mut idx = 0;
        let mut has_more = false;

        self.data
            .iter_async(|key, _| {
                idx += 1;
                if idx > cursor {
                    if let Some(matched) = matcher(key) {
                        result.push(matched);

                        if (result.len() as u64) == count {
                            has_more = true;
                            false
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                } else {
                    true
                }
            })
            .await;

        if has_more {
            Ok((idx, result))
        } else {
            Ok((0, result))
        }
    }

    async fn append(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
        value: Vec<u8>,
    ) -> Result<(), IndexedStorageError> {
        let primary_oplog_insert = matches!(
            &namespace,
            IndexedStorageNamespace::OpLog { .. } | IndexedStorageNamespace::StagedOpLog { .. }
        );
        let composite_key = Self::composite_key(namespace, key);
        let mut entry = self
            .data
            .entry_async(composite_key.clone())
            .await
            .or_default();
        if let std::collections::btree_map::Entry::Vacant(e) = entry.entry(id) {
            e.insert(value.to_vec());
            Ok(())
        } else if primary_oplog_insert {
            Err(IndexedStorageError::Conflict(
                "Key already exists".to_string(),
            ))
        } else {
            Err(IndexedStorageError::Other("Key already exists".to_string()))
        }
    }

    async fn publish_staged(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        agent_id: &AgentId,
        agent_mode: golem_common::model::agent::AgentMode,
        stage_key: &str,
        target_key: &str,
        expected_last_id: u64,
    ) -> Result<bool, IndexedStorageError> {
        let stage = Self::composite_key(
            IndexedStorageNamespace::StagedOpLog {
                agent_id: agent_id.clone(),
                agent_mode,
            },
            stage_key,
        );
        let target = Self::composite_key(
            IndexedStorageNamespace::OpLog {
                agent_id: agent_id.clone(),
                agent_mode,
            },
            target_key,
        );
        if self.data.contains_async(&target).await {
            return Ok(false);
        }
        let staged = self
            .data
            .read_async(&stage, |_, entries| entries.clone())
            .await
            .ok_or_else(|| IndexedStorageError::Other("staged oplog is missing".to_string()))?;
        if expected_last_id == 0
            || staged.len() as u64 != expected_last_id
            || staged.keys().copied().ne(1..=expected_last_id)
        {
            return Err(IndexedStorageError::Other(
                "staged oplog is empty, gapped, or has an unexpected tip".to_string(),
            ));
        }
        if self.data.insert_async(target, staged).await.is_err() {
            return Ok(false);
        }
        self.data.remove_async(&stage).await;
        Ok(true)
    }

    async fn length(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<u64, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| entry.len() as u64)
            .await
            .unwrap_or_default())
    }

    async fn delete(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<(), IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        self.data.remove_async(&composite_key).await;
        Ok(())
    }

    async fn read(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        start_id: u64,
        end_id: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, IndexedStorageError> {
        #[cfg(test)]
        self.read_count.fetch_add(1, Ordering::Relaxed);

        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let mut result = Vec::new();
                for (id, value) in entry.range((Included(start_id), Included(end_id))) {
                    result.push((*id, value.clone()));
                }
                result
            })
            .await
            .unwrap_or_default())
    }

    async fn first(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let first = entry.first_key_value();
                first.map(|(id, value)| (*id, value.clone()))
            })
            .await
            .flatten())
    }

    async fn last(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                let last = entry.last_key_value();
                last.map(|(id, value)| (*id, value.clone()))
            })
            .await
            .flatten())
    }

    async fn closest(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        _entity_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        id: u64,
    ) -> Result<Option<(u64, Vec<u8>)>, IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        Ok(self
            .data
            .read_async(&composite_key, |_, entry| {
                entry
                    .keys()
                    .find(|k| **k >= id)
                    .map(|key| (*key, entry[key].clone()))
            })
            .await
            .flatten())
    }

    async fn drop_prefix(
        &self,
        _svc_name: &'static str,
        _api_name: &'static str,
        namespace: IndexedStorageNamespace,
        key: &str,
        last_dropped_id: u64,
    ) -> Result<(), IndexedStorageError> {
        let composite_key = Self::composite_key(namespace, key);
        self.data
            .update_async(&composite_key, |_, entry| {
                entry.retain(|k, _| *k > last_dropped_id);
            })
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::storage::indexed::{
        IndexedStorage, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
        IndexedStorageNamespace,
    };
    use assert2::check;
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;

    fn test_agent_id() -> AgentId {
        use std::sync::OnceLock;
        static WORKER_ID: OnceLock<AgentId> = OnceLock::new();

        WORKER_ID
            .get_or_init(|| AgentId {
                component_id: ComponentId::new(),
                agent_id: "worker".to_string(),
            })
            .clone()
    }

    fn staged_namespace() -> IndexedStorageNamespace {
        IndexedStorageNamespace::StagedOpLog {
            agent_id: test_agent_id(),
            agent_mode: golem_common::model::agent::AgentMode::Durable,
        }
    }

    #[test]
    async fn staged_publication_is_atomic_validated_and_hidden() {
        let storage = super::InMemoryIndexedStorage::new();
        for (id, value) in [(1, b"one"), (2, b"two")] {
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    staged_namespace(),
                    "stage",
                    id,
                    value.to_vec(),
                )
                .await
                .unwrap();
        }
        assert_eq!(
            storage
                .scan(
                    "test",
                    "scan",
                    IndexedStorageMetaNamespace::Oplog {
                        agent_mode: golem_common::model::agent::AgentMode::Durable,
                    },
                    None,
                    0,
                    100,
                )
                .await
                .unwrap()
                .1,
            Vec::<String>::new()
        );
        assert!(
            storage
                .publish_staged(
                    "test",
                    "publish",
                    &test_agent_id(),
                    golem_common::model::agent::AgentMode::Durable,
                    "stage",
                    "target",
                    2
                )
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .read(
                    "test",
                    "read",
                    "entry",
                    IndexedStorageNamespace::OpLog {
                        agent_id: test_agent_id(),
                        agent_mode: golem_common::model::agent::AgentMode::Durable
                    },
                    "target",
                    1,
                    2
                )
                .await
                .unwrap(),
            vec![(1, b"one".to_vec()), (2, b"two".to_vec())]
        );
        assert!(
            !storage
                .exists("test", "exists", staged_namespace(), "stage")
                .await
                .unwrap()
        );

        for (key, pairs, tip) in [
            ("empty", vec![], 1),
            ("gap", vec![(1, b"one".to_vec()), (3, b"three".to_vec())], 3),
            ("tip", vec![(1, b"one".to_vec())], 2),
        ] {
            for (id, value) in pairs {
                storage
                    .append(
                        "test",
                        "append",
                        "entry",
                        staged_namespace(),
                        key,
                        id,
                        value,
                    )
                    .await
                    .unwrap();
            }
            assert!(
                storage
                    .publish_staged(
                        "test",
                        "publish",
                        &test_agent_id(),
                        golem_common::model::agent::AgentMode::Durable,
                        key,
                        key,
                        tip
                    )
                    .await
                    .is_err()
            );
            assert!(
                !storage
                    .exists(
                        "test",
                        "exists",
                        IndexedStorageNamespace::OpLog {
                            agent_id: test_agent_id(),
                            agent_mode: golem_common::model::agent::AgentMode::Durable
                        },
                        key
                    )
                    .await
                    .unwrap()
            );
        }
    }

    #[test]
    async fn staged_publication_never_overwrites_and_has_one_concurrent_winner() {
        let storage = std::sync::Arc::new(super::InMemoryIndexedStorage::new());
        for stage in ["first", "second"] {
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    staged_namespace(),
                    stage,
                    1,
                    stage.as_bytes().to_vec(),
                )
                .await
                .unwrap();
        }
        let a = {
            let storage = storage.clone();
            tokio::spawn(async move {
                storage
                    .publish_staged(
                        "test",
                        "publish",
                        &test_agent_id(),
                        golem_common::model::agent::AgentMode::Durable,
                        "first",
                        "target",
                        1,
                    )
                    .await
                    .unwrap()
            })
        };
        let b = {
            let storage = storage.clone();
            tokio::spawn(async move {
                storage
                    .publish_staged(
                        "test",
                        "publish",
                        &test_agent_id(),
                        golem_common::model::agent::AgentMode::Durable,
                        "second",
                        "target",
                        1,
                    )
                    .await
                    .unwrap()
            })
        };
        assert_ne!(a.await.unwrap(), b.await.unwrap());
        let before = storage
            .read(
                "test",
                "read",
                "entry",
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                "target",
                1,
                1,
            )
            .await
            .unwrap();
        assert!(
            !storage
                .publish_staged(
                    "test",
                    "publish",
                    &test_agent_id(),
                    golem_common::model::agent::AgentMode::Durable,
                    if before[0].1 == b"first" {
                        "second"
                    } else {
                        "first"
                    },
                    "target",
                    1
                )
                .await
                .unwrap()
        );
        assert_eq!(
            storage
                .read(
                    "test",
                    "read",
                    "entry",
                    IndexedStorageNamespace::OpLog {
                        agent_id: test_agent_id(),
                        agent_mode: golem_common::model::agent::AgentMode::Durable
                    },
                    "target",
                    1,
                    1
                )
                .await
                .unwrap(),
            before
        );

        storage
            .append(
                "test",
                "append",
                "entry",
                staged_namespace(),
                "third",
                1,
                b"staged".to_vec(),
            )
            .await
            .unwrap();
        let publish = {
            let storage = storage.clone();
            tokio::spawn(async move {
                storage
                    .publish_staged(
                        "test",
                        "publish",
                        &test_agent_id(),
                        golem_common::model::agent::AgentMode::Durable,
                        "third",
                        "ordinary-race",
                        1,
                    )
                    .await
                    .unwrap()
            })
        };
        let append = {
            let storage = storage.clone();
            tokio::spawn(async move {
                storage
                    .append(
                        "test",
                        "append",
                        "entry",
                        IndexedStorageNamespace::OpLog {
                            agent_id: test_agent_id(),
                            agent_mode: golem_common::model::agent::AgentMode::Durable,
                        },
                        "ordinary-race",
                        1,
                        b"ordinary".to_vec(),
                    )
                    .await
                    .is_ok()
            })
        };
        assert_ne!(publish.await.unwrap(), append.await.unwrap());
    }

    #[test]
    async fn closest_exact_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            1,
            &100,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            2,
            &200,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            3,
            &300,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            4,
            &400,
        )
        .await
        .unwrap();

        let result = api
            .closest(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                3,
            )
            .await
            .unwrap();

        check!(result == Some((3, 300)));
    }

    #[test]
    async fn closest_no_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            1,
            &100,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            2,
            &200,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            3,
            &300,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            4,
            &400,
        )
        .await
        .unwrap();

        let result: Option<(u64, i32)> = api
            .closest(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                5,
            )
            .await
            .unwrap();

        check!(result == None);
    }

    #[test]
    async fn closest_match() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            30,
            &300,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            40,
            &400,
        )
        .await
        .unwrap();

        let result = api
            .closest(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                33,
            ) // 40 is the closest that is <= 33
            .await
            .unwrap();

        check!(result == Some((40, 400)));
    }

    #[test]
    async fn read() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            30,
            &300,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            40,
            &400,
        )
        .await
        .unwrap();

        let result = api
            .read(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                20,
                40,
            )
            .await
            .unwrap();

        check!(result == vec![(20, 200), (30, 300), (40, 400)]);
    }

    #[test]
    async fn read_wider() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            30,
            &300,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            40,
            &400,
        )
        .await
        .unwrap();

        let result = api
            .read(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                1,
                100,
            )
            .await
            .unwrap();

        check!(result == vec![(10, 100), (20, 200), (30, 300), (40, 400)]);
    }

    #[test]
    async fn first() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
        )
        .await
        .unwrap();

        let result = api
            .first(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
            )
            .await
            .unwrap();

        check!(result == Some((10, 100)));
    }

    #[test]
    async fn last() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            10,
            &100,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            20,
            &200,
        )
        .await
        .unwrap();

        let result = api
            .last(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
            )
            .await
            .unwrap();

        check!(result == Some((20, 200)));
    }

    #[test]
    async fn drop_prefix() {
        let storage = super::InMemoryIndexedStorage::new();
        let api = storage.with_entity("test", "test", "test");
        let key = "key";

        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            1,
            &100,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            2,
            &200,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            3,
            &300,
        )
        .await
        .unwrap();
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            key,
            4,
            &400,
        )
        .await
        .unwrap();

        storage
            .with("test", "test")
            .drop_prefix(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                2,
            )
            .await
            .unwrap();

        let result = api
            .read(
                IndexedStorageNamespace::OpLog {
                    agent_id: test_agent_id(),
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                key,
                1,
                4,
            )
            .await
            .unwrap();

        check!(result == vec![(3, 300), (4, 400)]);
    }
}
