// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use assert2::check;
use bytes::Bytes;
use golem_common::model::AccountId;
use golem_worker_executor_base::storage::keyvalue::memory::InMemoryKeyValueStorage;
use golem_worker_executor_base::storage::keyvalue::{KeyValueStorage, KeyValueStorageNamespace};

pub(crate) trait GetKeyValueStorage {
    fn get_key_value_storage(&self) -> &dyn KeyValueStorage;
}

struct InMemoryKeyValueStorageWrapper {
    kvs: InMemoryKeyValueStorage,
}

impl GetKeyValueStorage for InMemoryKeyValueStorageWrapper {
    fn get_key_value_storage(&self) -> &dyn KeyValueStorage {
        &self.kvs
    }
}

pub(crate) async fn init() -> impl GetKeyValueStorage {
    let kvs = InMemoryKeyValueStorage::new();
    InMemoryKeyValueStorageWrapper { kvs }
}

pub fn ns() -> KeyValueStorageNamespace {
    KeyValueStorageNamespace::Worker
}

pub fn ns2() -> KeyValueStorageNamespace {
    KeyValueStorageNamespace::UserDefined {
        account_id: AccountId::generate(),
        bucket: "test-bucket".to_string(),
    }
}

// TODO: wrap all tests in macro

// TODO: namespace separation tests
// TODO: edge cases

#[tokio::test]
#[tracing::instrument]
async fn get_set_get() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let key = "key";
    let value = "value".as_bytes();

    let result1 = kvs
        .get("test", "api", "entity", ns.clone(), key)
        .await
        .unwrap();
    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();
    let result2 = kvs.get("test", "api", "entity", ns, key).await.unwrap();
    assert_eq!(result1, None);
    assert_eq!(result2, Some(value.into()));
}

#[tokio::test]
#[tracing::instrument]
async fn get_set_get_many() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let key1 = "key1";
    let key2 = "key2";
    let key3 = "key3";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = kvs
        .get_many(
            "test",
            "api",
            "entity",
            ns.clone(),
            vec![key1.to_string(), key2.to_string(), key3.to_string()],
        )
        .await
        .unwrap();
    kvs.set_many(
        "test",
        "api",
        "entity",
        ns.clone(),
        &[(key1, value1), (key2, value2)],
    )
    .await
    .unwrap();
    let result2 = kvs
        .get_many(
            "test",
            "api",
            "entity",
            ns,
            vec![key1.to_string(), key2.to_string(), key3.to_string()],
        )
        .await
        .unwrap();
    assert_eq!(result1, vec![None, None, None]);
    assert_eq!(
        result2,
        vec![Some(value1.into()), Some(value2.into()), None]
    );
}

#[tokio::test]
#[tracing::instrument]
async fn set_if_not_exists() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let key = "key";
    let value1 = "value".as_bytes();
    let value2 = "value2".as_bytes();

    let result1 = kvs
        .set_if_not_exists("test", "api", "entity", ns.clone(), key, value1)
        .await
        .unwrap();
    let result2 = kvs
        .set_if_not_exists("test", "api", "entity", ns.clone(), key, value2)
        .await
        .unwrap();
    let result3 = kvs.get("test", "api", "entity", ns, key).await.unwrap();
    assert_eq!(result1, true);
    assert_eq!(result2, false);
    assert_eq!(result3, Some(value1.into()));
}

#[tokio::test]
#[tracing::instrument]
async fn del() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let key = "key";
    let value = "value".as_bytes();

    let _ = kvs.del("test", "api", ns.clone(), key).await.unwrap(); // deleting non-existing key must succeed
    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();
    let result1 = kvs
        .get("test", "api", "entity", ns.clone(), key)
        .await
        .unwrap();
    let _ = kvs.del("test", "api", ns.clone(), key).await.unwrap();
    let result2 = kvs.get("test", "api", "entity", ns, key).await.unwrap();

    assert_eq!(result1, Some(value.into()));
    assert_eq!(result2, None);
}

#[tokio::test]
#[tracing::instrument]
async fn del_many() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let key1 = "key";
    let key2 = "key2";
    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    let _ = kvs
        .del_many(
            "test",
            "api",
            ns.clone(),
            vec![key1.to_string(), key2.to_string()],
        )
        .await
        .unwrap(); // deleting non-existing key must succeed
    kvs.set("test", "api", "entity", ns.clone(), key1, value1)
        .await
        .unwrap();
    kvs.set("test", "api", "entity", ns.clone(), key2, value2)
        .await
        .unwrap();
    let result1 = kvs
        .get("test", "api", "entity", ns.clone(), key1)
        .await
        .unwrap();
    let result2 = kvs
        .get("test", "api", "entity", ns.clone(), key2)
        .await
        .unwrap();
    let _ = kvs
        .del_many(
            "test",
            "api",
            ns.clone(),
            vec![key1.to_string(), key2.to_string()],
        )
        .await
        .unwrap();
    let result3 = kvs
        .get("test", "api", "entity", ns.clone(), key1)
        .await
        .unwrap();
    let result4 = kvs
        .get("test", "api", "entity", ns.clone(), key2)
        .await
        .unwrap();

    assert_eq!(result1, Some(value1.into()));
    assert_eq!(result2, Some(value2.into()));
    assert_eq!(result3, None);
    assert_eq!(result4, None);
}

#[tokio::test]
#[tracing::instrument]
async fn exists_set_exists() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let key = "key";
    let value = "value".as_bytes();

    let result1 = kvs.exists("test", "api", ns.clone(), key).await.unwrap();
    kvs.set("test", "api", "entity", ns.clone(), key, value)
        .await
        .unwrap();
    let result2 = kvs.exists("test", "api", ns, key).await.unwrap();
    assert_eq!(result1, false);
    assert_eq!(result2, true);
}

#[tokio::test]
#[tracing::instrument]
async fn keys() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let key1 = "key1";
    let key2 = "key2";

    let keys1 = kvs.keys("test", "api", ns.clone()).await.unwrap();
    kvs.set(
        "test",
        "api",
        "entity",
        ns.clone(),
        key1,
        "value1".as_bytes(),
    )
    .await
    .unwrap();
    kvs.set(
        "test",
        "api",
        "entity",
        ns.clone(),
        key2,
        "value2".as_bytes(),
    )
    .await
    .unwrap();
    let keys2 = kvs.keys("test", "api", ns.clone()).await.unwrap();
    kvs.del("test", "api", ns.clone(), key1).await.unwrap();
    let keys3 = kvs.keys("test", "api", ns).await.unwrap();

    assert_eq!(keys1, Vec::<String>::new());
    assert_eq!(keys2, vec![key1.to_string(), key2.to_string()]);
    assert_eq!(keys3, vec![key2.to_string()]);
}

#[tokio::test]
#[tracing::instrument]
async fn sets() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let set1 = "set1";
    let set2 = "set2";

    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();

    let s11 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s21 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.add_to_set("test", "api", "entity", ns.clone(), set1, value1)
        .await
        .unwrap();
    kvs.add_to_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap();
    kvs.add_to_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap();

    kvs.add_to_set("test", "api", "entity", ns.clone(), set2, value3)
        .await
        .unwrap();
    kvs.add_to_set("test", "api", "entity", ns.clone(), set2, value3)
        .await
        .unwrap();
    kvs.add_to_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap();

    let s12 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s22 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.remove_from_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap();
    kvs.remove_from_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap();

    let s13 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s23 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.remove_from_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap(); // can remove non-existing value
    kvs.remove_from_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap(); // can remove non-existing value

    let s14 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s24 = kvs
        .members_of_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    check!(s11 == Vec::<Vec<u8>>::new());
    check!(s21 == Vec::<Vec<u8>>::new());
    check!(s12.len() == 2);
    check!(s12.contains(&value1.to_vec().into()));
    check!(s12.contains(&value2.to_vec().into()));
    check!(s22.len() == 2);
    check!(s22.contains(&value2.to_vec().into()));
    check!(s22.contains(&value3.to_vec().into()));
    check!(s13.len() == 1);
    check!(s13.contains(&value1.to_vec().into()));
    check!(s23.len() == 1);
    check!(s23.contains(&value3.to_vec().into()));
    check!(s14.len() == 1);
    check!(s14.contains(&value1.to_vec().into()));
    check!(s24.len() == 1);
    check!(s24.contains(&value3.to_vec().into()));
}

#[tokio::test]
#[tracing::instrument]
async fn sorted_sets() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let set1 = "set1";
    let set2 = "set2";

    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();
    let value4 = "value4".as_bytes();

    let s11 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s21 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 4.0, value4)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 1.0, value1)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 2.0, value2)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 2.0, value2)
        .await
        .unwrap();

    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 4.0, value4)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 3.0, value3)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 3.0, value3)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 2.0, value2)
        .await
        .unwrap();

    let s12 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s22 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.remove_from_sorted_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap();
    kvs.remove_from_sorted_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap();

    let s13 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s23 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    kvs.remove_from_sorted_set("test", "api", "entity", ns.clone(), set1, value2)
        .await
        .unwrap(); // can remove non-existing value
    kvs.remove_from_sorted_set("test", "api", "entity", ns.clone(), set2, value2)
        .await
        .unwrap(); // can remove non-existing value

    let s14 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();
    let s24 = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set2)
        .await
        .unwrap();

    assert_eq!(s11, Vec::<(f64, Bytes)>::new());
    assert_eq!(s21, Vec::<(f64, Bytes)>::new());

    assert_eq!(
        s12,
        vec![
            (1.0, value1.into()),
            (2.0, value2.into()),
            (4.0, value4.into())
        ]
    );
    assert_eq!(
        s22,
        vec![
            (2.0, value2.into()),
            (3.0, value3.into()),
            (4.0, value4.into())
        ]
    );

    assert_eq!(s13, vec![(1.0, value1.into()), (4.0, value4.into())]);
    assert_eq!(s23, vec![(3.0, value3.into()), (4.0, value4.into())]);

    assert_eq!(s14, vec![(1.0, value1.into()), (4.0, value4.into())]);
    assert_eq!(s24, vec![(3.0, value3.into()), (4.0, value4.into())]);
}

#[tokio::test]
#[tracing::instrument]
async fn add_to_sorted_set_updates_score() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let set1 = "set1";

    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();

    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 1.0, value1)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 2.0, value2)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 3.0, value2)
        .await
        .unwrap();

    let result = kvs
        .get_sorted_set("test", "api", "entity", ns.clone(), set1)
        .await
        .unwrap();

    assert_eq!(result, vec![(1.0, value1.into()), (3.0, value2.into())]);
}

#[tokio::test]
#[tracing::instrument]
async fn query_sorted_set() {
    let test = init().await;
    let kvs = test.get_key_value_storage();
    let ns = ns();

    let set1 = "set1";
    let set2 = "set2";

    let value1 = "value1".as_bytes();
    let value2 = "value2".as_bytes();
    let value3 = "value3".as_bytes();
    let value4 = "value4".as_bytes();

    let result1 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set1, 0.0, 4.0)
        .await
        .unwrap();

    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 1.0, value1)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 2.0, value2)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set1, 3.0, value3)
        .await
        .unwrap();
    kvs.add_to_sorted_set("test", "api", "entity", ns.clone(), set2, 4.0, value4)
        .await
        .unwrap();

    let result2 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set1, 0.0, 4.0)
        .await
        .unwrap();
    let result3 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set1, 1.0, 3.0)
        .await
        .unwrap();
    let result4 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set1, 1.5, 3.2)
        .await
        .unwrap();
    let result5 = kvs
        .query_sorted_set("test", "api", "entity", ns.clone(), set2, 4.0, 4.0)
        .await
        .unwrap();

    assert_eq!(result1, Vec::<(f64, Bytes)>::new());
    assert_eq!(
        result2,
        vec![
            (1.0, value1.into()),
            (2.0, value2.into()),
            (3.0, value3.into())
        ]
    );
    assert_eq!(
        result3,
        vec![
            (1.0, value1.into()),
            (2.0, value2.into()),
            (3.0, value3.into())
        ]
    );
    assert_eq!(result4, vec![(2.0, value2.into()), (3.0, value3.into())]);
    assert_eq!(result5, vec![(4.0, value4.into())]);
}
