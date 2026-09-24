//! In-process logical-call observations, isolated from global Prometheus state.

use super::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Call = (&'static str, &'static str, &'static str);

#[derive(Debug)]
pub(crate) struct CountingKeyValueStorage {
    inner: Arc<dyn KeyValueStorage + Send + Sync>,
    calls: Mutex<BTreeMap<Call, usize>>,
}

impl CountingKeyValueStorage {
    pub(crate) fn new(inner: Arc<dyn KeyValueStorage + Send + Sync>) -> Self {
        Self {
            inner,
            calls: Mutex::new(BTreeMap::new()),
        }
    }

    pub(crate) fn reset(&self) {
        self.calls.lock().unwrap().clear();
    }

    pub(crate) fn calls(&self) -> BTreeMap<Call, usize> {
        self.calls.lock().unwrap().clone()
    }
}

macro_rules! counting_methods {
    ($(fn $name:ident($($arg:ident: $typ:ty),*) -> $result:ty;)*) => {
        #[async_trait]
        impl KeyValueStorage for CountingKeyValueStorage {
            $(async fn $name(&self, svc_name: &'static str, api_name: &'static str, $($arg: $typ),*) -> Result<$result, KeyValueStorageError> {
                *self.calls.lock().unwrap().entry((stringify!($name), svc_name, api_name)).or_default() += 1;
                self.inner.$name(svc_name, api_name, $($arg),*).await
            })*
        }
    };
}

counting_methods! {
    fn set(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, value: &[u8]) -> ();
    fn set_with_expiry(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, value: &[u8], expiry: Duration) -> ();
    fn set_many(entity_name: &'static str, namespace: KeyValueStorageNamespace, pairs: &[(&str, &[u8])]) -> ();
    fn compare_and_set_many(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, expected: Option<&[u8]>, deletes: &[&str], pairs: &[(&str, &[u8])]) -> bool;
    fn compare_and_mutate_many(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, expected: Option<&[u8]>, sets: &[(&str, &[u8])], deletions: &[&str], expiry: Duration) -> bool;
    fn set_if_not_exists(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, value: &[u8]) -> bool;
    fn get(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str) -> Option<Bytes>;
    fn get_many(entity_name: &'static str, namespace: KeyValueStorageNamespace, keys: Arc<[String]>) -> Vec<Option<Bytes>>;
    fn get_all(entity_name: &'static str, namespace: KeyValueStorageNamespace) -> Vec<(String, Bytes)>;
    fn del(namespace: KeyValueStorageNamespace, key: &str) -> ();
    fn del_many(namespace: KeyValueStorageNamespace, keys: Arc<[String]>) -> ();
    fn exists(namespace: KeyValueStorageNamespace, key: &str) -> bool;
    fn keys(namespace: KeyValueStorageNamespace) -> Vec<String>;
    fn add_to_set(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, value: &[u8]) -> ();
    fn remove_from_set(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, value: &[u8]) -> ();
    fn members_of_set(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str) -> Vec<Bytes>;
    fn add_to_sorted_set(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, score: f64, value: &[u8]) -> ();
    fn remove_from_sorted_set(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, value: &[u8]) -> ();
    fn get_sorted_set(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str) -> Vec<(f64, Bytes)>;
    fn query_sorted_set(entity_name: &'static str, namespace: KeyValueStorageNamespace, key: &str, min: f64, max: f64) -> Vec<(f64, Bytes)>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::keyvalue::{KeyValueStorageLabelledApi, memory::InMemoryKeyValueStorage};
    use golem_service_base::metrics::storage::LOGICAL_OPERATIONS_TOTAL;
    use test_r::test;

    #[test]
    async fn logical_storage_counters_count_once_and_distinguish_apis() {
        let storage = CountingKeyValueStorage::new(Arc::new(InMemoryKeyValueStorage::new()));
        let namespace = KeyValueStorageNamespace::Schedule;
        // A dedicated static service label isolates this test from other tests.
        let count = |api| {
            LOGICAL_OPERATIONS_TOTAL
                .with_label_values(&["keyvalue", "get", "logical-counter-test", api, "metadata"])
                .get()
        };
        let before = count("read_recovery");
        let other = count("lookup");
        for key in ["one", "two"] {
            assert!(
                storage
                    .with_entity("logical-counter-test", "read_recovery", "metadata")
                    .get::<u64>(namespace.clone(), key)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            storage
                .with_entity("logical-counter-test", "lookup", "metadata")
                .get_raw(namespace, "one")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(count("read_recovery") - before, 2);
        assert_eq!(count("lookup") - other, 1);
        assert_eq!(
            storage.calls()[&("get", "logical-counter-test", "read_recovery")],
            2
        );
        assert_eq!(storage.calls().len(), 2);
    }

    #[test]
    async fn logical_storage_counters_count_indexed_and_blob_delegation_once() {
        use crate::storage::indexed::{
            IndexedStorageLabelledApi, IndexedStorageNamespace, memory::InMemoryIndexedStorage,
        };
        use golem_common::model::{
            AgentId, agent::AgentMode, component::ComponentId, environment::EnvironmentId,
        };
        use golem_service_base::storage::blob::{
            BlobStorageLabelledApi, BlobStorageNamespace, memory::InMemoryBlobStorage,
        };
        use std::path::Path;

        let count = |kind, operation, entity| {
            LOGICAL_OPERATIONS_TOTAL
                .with_label_values(&[kind, operation, "logical-delegation-test", "lookup", entity])
                .get()
        };
        let indexed = InMemoryIndexedStorage::new();
        let ns = IndexedStorageNamespace::OpLog {
            agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: "counter".into(),
            },
            agent_mode: AgentMode::Durable,
        };
        let api = indexed.with_entity("logical-delegation-test", "lookup", "entry");
        let before = count("indexed", "append_many", "entry");
        api.append_many(&ns, "key", &[(1, &7u64), (3, &11u64)])
            .await
            .unwrap();
        assert_eq!(count("indexed", "append_many", "entry") - before, 1);
        let before = count("indexed", "first", "entry");
        assert_eq!(api.first_id(ns.clone(), "key").await.unwrap(), Some(1));
        assert_eq!(count("indexed", "first", "entry") - before, 1);
        let before = count("indexed", "read", "entry");
        assert_eq!(
            api.read::<u64>(ns, "key", 1, 3).await.unwrap(),
            vec![(1, 7), (3, 11)]
        );
        assert_eq!(count("indexed", "read", "entry") - before, 1);

        let blob = InMemoryBlobStorage::new();
        let ns = BlobStorageNamespace::InitialAgentFiles {
            environment_id: EnvironmentId::new(),
        };
        let api = blob.with("logical-delegation-test", "lookup");
        let before = count("blob", "put", "");
        api.put(ns.clone(), Path::new("value"), &37u64)
            .await
            .unwrap();
        assert_eq!(count("blob", "put", "") - before, 1);
        let before = count("blob", "get", "");
        assert_eq!(
            api.get::<u64>(ns.clone(), Path::new("value"))
                .await
                .unwrap(),
            Some(37)
        );
        assert_eq!(count("blob", "get", "") - before, 1);
        let before = count("blob", "get_range_stream", "");
        assert!(
            api.get_range_stream(ns, Path::new("value"), 0, 1)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(count("blob", "get_range_stream", "") - before, 1);
    }
}
