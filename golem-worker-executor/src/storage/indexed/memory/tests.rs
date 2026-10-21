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

use test_r::test;

use crate::storage::indexed::{
    IndexedStorage, IndexedStorageError, IndexedStorageLabelledApi, IndexedStorageMetaNamespace,
    IndexedStorageNamespace, ScanResume,
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

fn primary_namespace() -> IndexedStorageNamespace {
    IndexedStorageNamespace::OpLog {
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
                None,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        storage
            .scan_stable(
                "test",
                "scan",
                IndexedStorageMetaNamespace::Oplog {
                    agent_mode: golem_common::model::agent::AgentMode::Durable,
                },
                None,
                None,
                100,
            )
            .await
            .unwrap()
            .1,
        Vec::<String>::new()
    );
    assert!(
        storage
            .move_if_absent(
                "test",
                "publish",
                staged_namespace(),
                "stage",
                primary_namespace(),
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
                    None,
                )
                .await
                .unwrap();
        }
        assert!(
            storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged_namespace(),
                    key,
                    primary_namespace(),
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
                None,
            )
            .await
            .unwrap();
    }
    let a = {
        let storage = storage.clone();
        tokio::spawn(async move {
            storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged_namespace(),
                    "first",
                    primary_namespace(),
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
                .move_if_absent(
                    "test",
                    "publish",
                    staged_namespace(),
                    "second",
                    primary_namespace(),
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
            .move_if_absent(
                "test",
                "publish",
                staged_namespace(),
                if before[0].1 == b"first" {
                    "second"
                } else {
                    "first"
                },
                primary_namespace(),
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
            None,
        )
        .await
        .unwrap();
    let publish = {
        let storage = storage.clone();
        tokio::spawn(async move {
            storage
                .move_if_absent(
                    "test",
                    "publish",
                    staged_namespace(),
                    "third",
                    primary_namespace(),
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
                    None,
                )
                .await
                .is_ok()
        })
    };
    assert_ne!(publish.await.unwrap(), append.await.unwrap());
}

#[test]
async fn scan_stable_pages_in_order_and_hands_a_key_back_once() {
    let storage = super::InMemoryIndexedStorage::new();
    let api = storage.with_entity("test", "test", "test");
    let agent_mode = golem_common::model::agent::AgentMode::Durable;

    for key in ["k1", "k2", "k3", "k4", "k5"] {
        api.append(
            IndexedStorageNamespace::OpLog {
                agent_id: test_agent_id(),
                agent_mode,
            },
            key,
            1,
            &100,
            None,
        )
        .await
        .unwrap();
    }

    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut resume = None;
    for _ in 0..8 {
        let (next, page) = storage
            .with("test", "test")
            .scan_stable(
                IndexedStorageMetaNamespace::Oplog { agent_mode },
                None,
                resume,
                2,
            )
            .await
            .unwrap();
        pages.push(page);
        match next {
            Some(next) => resume = Some(next),
            None => break,
        }
    }

    // The short page is what ends the walk, so five keys cost three pages and not a fourth,
    // and no key appears in two of them.
    let expected: Vec<Vec<String>> = vec![
        vec!["k1".to_string(), "k2".to_string()],
        vec!["k3".to_string(), "k4".to_string()],
        vec!["k5".to_string()],
    ];
    check!(pages == expected);
}

#[test]
async fn a_scan_gives_the_key_of_an_agent_whose_name_holds_a_slash() {
    // The oplogs write each agent under its `to_redis_key`, and a scan of a component uses the
    // component id as the prefix.
    let storage = super::InMemoryIndexedStorage::new();
    let api = storage.with_entity("test", "test", "test");
    let agent_mode = golem_common::model::agent::AgentMode::Durable;
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: r#"counter("a/b")"#.to_string(),
    };
    let key = agent_id.to_redis_key();
    let prefix = agent_id.component_id.0.to_string();
    api.append(
        IndexedStorageNamespace::OpLog {
            agent_id: agent_id.clone(),
            agent_mode,
        },
        &key,
        1,
        &100,
        None,
    )
    .await
    .unwrap();
    api.append(
        IndexedStorageNamespace::CompressedOpLog {
            agent_id: agent_id.clone(),
            agent_mode,
            level: 1,
        },
        &key,
        1,
        &100,
        None,
    )
    .await
    .unwrap();

    let scan = storage.with("test", "test");
    let (_, scanned) = scan
        .scan_stable(
            IndexedStorageMetaNamespace::Oplog { agent_mode },
            Some(&prefix),
            None,
            10,
        )
        .await
        .unwrap();
    let (_, scanned_compressed) = scan
        .scan_stable(
            IndexedStorageMetaNamespace::CompressedOplog {
                agent_mode,
                level: 1,
            },
            Some(&prefix),
            None,
            10,
        )
        .await
        .unwrap();

    check!((scanned, scanned_compressed) == (vec![key.clone()], vec![key]));
}

#[test]
async fn scan_stable_rejects_marker_containing_nul() {
    let storage = super::InMemoryIndexedStorage::new();
    let result = storage
        .with("test", "test")
        .scan_stable(
            IndexedStorageMetaNamespace::Oplog {
                agent_mode: golem_common::model::agent::AgentMode::Durable,
            },
            None,
            Some(ScanResume::Marker("invalid\0marker".to_string())),
            2,
        )
        .await;

    assert!(matches!(result, Err(IndexedStorageError::InvalidResume(_))));
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
            None,
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

/// An unfenced append racing the delete of an emptied key lands either before the delete,
/// which then keeps the key, or after it, into a new key: an acknowledged entry is never
/// removed.
#[test]
fn an_unfenced_append_racing_delete_empty_is_never_lost() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .build()
        .unwrap();
    runtime.block_on(async {
        let storage = std::sync::Arc::new(super::InMemoryIndexedStorage::new());
        for round in 0..5000u64 {
            let key = format!("delete-empty-race-{round}");
            storage
                .append(
                    "test",
                    "append",
                    "entry",
                    primary_namespace(),
                    &key,
                    1,
                    b"one".to_vec(),
                    None,
                )
                .await
                .unwrap();
            storage
                .drop_prefix("test", "drop_prefix", primary_namespace(), &key, 1, None)
                .await
                .unwrap();

            let append = tokio::spawn({
                let storage = storage.clone();
                let key = key.clone();
                async move {
                    storage
                        .append(
                            "test",
                            "append",
                            "entry",
                            primary_namespace(),
                            &key,
                            2,
                            b"two".to_vec(),
                            None,
                        )
                        .await
                }
            });
            let delete = tokio::spawn({
                let storage = storage.clone();
                let key = key.clone();
                async move {
                    storage
                        .delete_empty_with_epoch(
                            "test",
                            "delete_empty",
                            primary_namespace(),
                            &key,
                            None,
                        )
                        .await
                }
            });
            append.await.unwrap().unwrap();
            delete.await.unwrap().unwrap();

            check!(
                storage
                    .length("test", "length", primary_namespace(), &key)
                    .await
                    .unwrap()
                    == 1,
                "round {round}: an acknowledged append was removed"
            );
        }
    });
}
