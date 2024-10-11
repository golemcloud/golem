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

use crate::storage::{
    cassandra::CassandraSession,
    keyvalue::{KeyValueStorage, KeyValueStorageNamespace},
};
use async_trait::async_trait;
use bytes::Bytes;
use scylla::{
    prepared_statement::PreparedStatement, query::Query, serialize::row::SerializeRow,
    transport::errors::QueryError, FromRow,
};
use serde::Deserialize;
use std::fmt::Debug;
use std::{collections::HashMap, iter::repeat};

#[derive(Debug)]
pub struct CassandraKeyValueStorage {
    session: CassandraSession,
}

impl CassandraKeyValueStorage {
    pub async fn create_schema(&self) -> Result<(), String> {
        self.session.session.query_unpaged(
            Query::new(
                format!("CREATE KEYSPACE IF NOT EXISTS {} WITH REPLICATION = {{ 'class' : 'SimpleStrategy', 'replication_factor' : 1 }};", self.session.keyspace),
            ),
            &[],
        ).await
            .map_err(|e| e.to_string())?;

        self.session
            .session
            .query_unpaged(
                Query::new(format!(
                    r#"
                CREATE TABLE IF NOT EXISTS {}.kv_store (
                    namespace TEXT,
                    key TEXT,
                    value BLOB,
                    PRIMARY KEY (namespace, key)
                );"#,
                    self.session.keyspace
                )),
                &[],
            )
            .await
            .map_err(|e| e.to_string())?;

        self.session
            .session
            .query_unpaged(
                Query::new(format!(
                    r#"
                CREATE TABLE IF NOT EXISTS {}.kv_sets (
                    namespace TEXT,
                    key TEXT,
                    value BLOB,
                    PRIMARY KEY ((namespace, key), value)
                );"#,
                    self.session.keyspace
                )),
                &[],
            )
            .await
            .map_err(|e| e.to_string())?;

        self.session
            .session
            .query_unpaged(
                Query::new(format!(
                    r#"
                CREATE TABLE IF NOT EXISTS {}.kv_sorted_sets (
                    namespace TEXT,
                    key TEXT,
                    score DOUBLE,
                    value BLOB,
                    PRIMARY KEY ((namespace, key), score, value)
                );"#,
                    self.session.keyspace
                )),
                &[],
            )
            .await
            .map_err(|e| e.to_string())
            .map(|_| ())
    }

    pub fn new(session: CassandraSession) -> Self {
        Self { session }
    }

    fn to_string(&self, ns: KeyValueStorageNamespace) -> String {
        match ns {
            KeyValueStorageNamespace::Worker => "worker".to_string(),
            KeyValueStorageNamespace::Promise => "promise".to_string(),
            KeyValueStorageNamespace::Schedule => "schedule".to_string(),
            KeyValueStorageNamespace::UserDefined { account_id, bucket } => {
                format!("user-defined:{account_id}:{bucket}")
            }
        }
    }

    async fn statement(&self, query_text: &str) -> PreparedStatement {
        let mut statement = self.session.session.prepare(query_text).await.unwrap();
        statement.set_tracing(self.session.set_tracing);
        statement
    }

    async fn maybe_row<RowT, T, F>(
        &self,
        query: String,
        values: impl SerializeRow,
        map_to_value: F,
    ) -> Result<Option<T>, QueryError>
    where
        RowT: FromRow,
        T: Debug,
        F: FnOnce(RowT) -> T,
    {
        self.session
            .session
            .execute_unpaged(&self.statement(&query).await, values)
            .await?
            .maybe_first_row_typed::<RowT>()
            .map_err(|e| QueryError::InvalidMessage(e.to_string()))
            .map(|opt_row| opt_row.map(map_to_value))
    }
}

#[derive(FromRow, Debug, Deserialize)]
struct ValueRow {
    value: Vec<u8>,
}

impl ValueRow {
    fn into_bytes(self) -> Bytes {
        Bytes::from(self.value)
    }
}

#[derive(FromRow, Debug, Deserialize)]
struct KeyValueRow {
    key: String,
    value: Vec<u8>,
}

impl KeyValueRow {
    fn into_pair(self) -> (String, Bytes) {
        (self.key, Bytes::from(self.value))
    }
}
#[derive(FromRow, Debug, Deserialize)]
struct ScoreValueRow {
    score: f64,
    value: Vec<u8>,
}

impl ScoreValueRow {
    fn into_pair(self) -> (f64, Bytes) {
        (self.score, Bytes::from(self.value))
    }
}

#[async_trait]
impl KeyValueStorage for CassandraKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let query = format!(
            "INSERT INTO {}.kv_store (namespace, key, value) VALUES (?, ?, ?);",
            self.session.keyspace.clone()
        );
        self.session
            .with(svc_name, api_name)
            .perform_query("set", query, (&self.to_string(namespace), key, value))
            .await
            .map_err(|e| e.to_string())
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), String> {
        let query = format!(
            "INSERT INTO {}.kv_store (namespace, key, value) VALUES (?, ?, ?)",
            self.session.keyspace
        );
        let namespace = self.to_string(namespace);
        let values = pairs
            .iter()
            .map(|(field_key, field_value)| (&namespace, *field_key, *field_value))
            .collect::<Vec<_>>();

        self.session
            .with(svc_name, api_name)
            .perform_batch("set_many", query, values)
            .await
            .map_err(|e| e.to_string())
    }

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, String> {
        let exists_query = format!(
            "SELECT value FROM {}.kv_store WHERE namespace = ? AND key = ? LIMIT 1;",
            self.session.keyspace
        );
        let namespace = self.to_string(namespace);
        let not_exists = self
            .maybe_row(exists_query, (&namespace, key), |r: ValueRow| r)
            .await
            .map_or(true, |opt| opt.is_none());

        let insert_query = format!(
            "INSERT INTO {}.kv_store (namespace, key, value) VALUES (?, ?, ?) IF NOT EXISTS;",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .perform_query("set_if_not_exists", insert_query, (&namespace, key, value))
            .await
            .map_err(|e| e.to_string())
            .map(|_| not_exists)
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, String> {
        let query = format!(
            "SELECT value FROM {}.kv_store WHERE namespace = ? AND key = ?;",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .maybe_row(
                "get",
                query,
                (self.to_string(namespace), key),
                |row: ValueRow| row.into_bytes(),
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, String> {
        let placeholders: String = repeat("?").take(keys.len()).collect::<Vec<_>>().join(", ");
        let query = format!(
            "SELECT key, value FROM {}.kv_store WHERE namespace = ? AND key IN ({});",
            self.session.keyspace, placeholders
        );

        let parameters: Vec<String> = vec![self.to_string(namespace)]
            .into_iter()
            .chain(keys)
            .collect();

        let result = self
            .session
            .with(svc_name, api_name)
            .get_rows("get_many", query, &parameters, |row: KeyValueRow| {
                row.into_pair()
            })
            .await
            .map_err(|e| e.to_string())
            .unwrap();

        let mut result_map = result.into_iter().collect::<HashMap<String, Bytes>>();

        let keys = parameters[1..].to_vec();
        let values = keys
            .into_iter()
            .map(|key| result_map.remove(&key))
            .collect::<Vec<Option<Bytes>>>();

        Ok(values)
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), String> {
        let query = format!(
            "DELETE FROM {}.kv_store WHERE namespace = ? AND key = ?;",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .perform_query("del", query, (&self.to_string(namespace), key))
            .await
            .map_err(|e| e.to_string())
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Vec<String>,
    ) -> Result<(), String> {
        let placeholders: String = repeat("?").take(keys.len()).collect::<Vec<_>>().join(", ");
        let query = format!(
            "DELETE FROM {}.kv_store WHERE namespace = ? AND key IN ({});",
            self.session.keyspace, placeholders
        );

        let parameters: Vec<String> = vec![self.to_string(namespace)]
            .into_iter()
            .chain(keys)
            .collect();

        self.session
            .with(svc_name, api_name)
            .perform_query("del_many", query, &parameters)
            .await
            .map_err(|e| e.to_string())
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, String> {
        let query = format!(
            "SELECT value FROM {}.kv_store WHERE namespace = ? AND key = ? LIMIT 1;",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .maybe_row(
                "exists",
                query,
                (self.to_string(namespace), key),
                |row: ValueRow| row,
            )
            .await
            .map(|opt| opt.is_some())
            .map_err(|e| e.to_string())
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, String> {
        let query = format!(
            "SELECT key FROM {}.kv_store WHERE namespace = ?;",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .get_rows(
                "keys",
                query,
                (self.to_string(namespace),),
                |row: (String,)| row.0,
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let query = format!(
            "INSERT INTO {}.kv_sets (namespace, key, value) VALUES (?, ?, ?);",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .perform_query(
                "add_to_set",
                query,
                (&self.to_string(namespace), key, value),
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let query = format!(
            "DELETE FROM {}.kv_sets WHERE namespace = ? AND key = ? AND value = ?;",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .perform_query(
                "remove_from_set",
                query,
                (&self.to_string(namespace), key, value),
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, String> {
        let query = format!(
            "SELECT value FROM {}.kv_sets WHERE namespace = ? AND key = ?;",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .get_rows(
                "members_of_set",
                query,
                (&self.to_string(namespace), key),
                |row: ValueRow| row.into_bytes(),
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), String> {
        self.remove_from_sorted_set(
            svc_name,
            api_name,
            entity_name,
            namespace.clone(),
            key,
            value,
        )
        .await?;
        let insert_statement = format!(
            "INSERT INTO {}.kv_sorted_sets (namespace, key, score, value) VALUES (?, ?, ?, ?);",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .perform_query(
                "add_to_sorted_set",
                insert_statement,
                (&self.to_string(namespace), key, score, value),
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), String> {
        let get_score = format!(
            "SELECT score, value FROM {}.kv_sorted_sets WHERE namespace = ? AND key = ? AND value = ?  ALLOW FILTERING;",
            self.session.keyspace
        );
        let namespace = self.to_string(namespace);
        match self
            .maybe_row(get_score, (&namespace, key, value), |row: ScoreValueRow| {
                row.score
            })
            .await
            .map_err(|e| e.to_string())?
        {
            None => Ok(()),
            Some(score) => {
                let delete_statement = format!("DELETE FROM {}.kv_sorted_sets WHERE namespace = ? AND key = ? AND score = ? AND value = ?;", self.session.keyspace);
                self.session
                    .with(svc_name, api_name)
                    .perform_query(
                        "remove_from_sorted_set",
                        delete_statement,
                        (&namespace, key, score, value),
                    )
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        let query = format!(
            "SELECT score, value FROM {}.kv_sorted_sets WHERE namespace = ? AND key = ? ORDER BY score ASC;",
            self.session.keyspace
        );

        self.session
            .with(svc_name, api_name)
            .get_rows(
                "get_sorted_set",
                query,
                (&self.to_string(namespace), key),
                |row: ScoreValueRow| row.into_pair(),
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        _entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, String> {
        let query = format!(
            "SELECT score, value FROM {}.kv_sorted_sets WHERE namespace = ? AND key = ? AND score >= ? AND score <= ? ORDER BY score ASC;",
            self.session.keyspace
        );
        self.session
            .with(svc_name, api_name)
            .get_rows(
                "query_sorted_set",
                query,
                (&self.to_string(namespace), key, min, max),
                |row: ScoreValueRow| row.into_pair(),
            )
            .await
            .map_err(|e| e.to_string())
    }
}
