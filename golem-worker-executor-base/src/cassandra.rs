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

use bytes::Bytes;
use futures::StreamExt;

use golem_common::config::CassandraConfig;
use golem_common::metrics::db::{record_db_failure, record_db_success};
use scylla::batch::{Batch, BatchType};
use scylla::prepared_statement::PreparedStatement;
use scylla::query::Query;
use scylla::serialize::row::SerializeRow;
use scylla::transport::errors::QueryError;
use scylla::FromRow;
use scylla::{transport::session::PoolSize, Session, SessionBuilder};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Debug;
use std::iter::repeat;
use std::time::Instant;
use std::{num::NonZeroUsize, sync::Arc};

#[derive(Debug, Clone)]
pub struct CassandraSession {
    pub session: Arc<Session>,
    keyspace: String,
    set_tracing: bool,
}

impl CassandraSession {
    pub fn new(session: Arc<Session>, set_tracing: bool, keyspace: &str) -> Self {
        CassandraSession {
            session,
            keyspace: String::from(keyspace),
            set_tracing,
        }
    }

    pub async fn configured(config: &CassandraConfig) -> Result<Self, String> {
        let mut session_builder = SessionBuilder::new()
            .known_nodes_addr(config.nodes.iter())
            .pool_size(PoolSize::PerHost(
                NonZeroUsize::new(config.pool_size_per_host).unwrap(),
            ))
            .use_keyspace(&config.keyspace, false);

        if let (Some(username), Some(password)) =
            (config.username.as_ref(), config.password.as_ref())
        {
            session_builder = session_builder.user(username, password);
        }

        let session = session_builder.build().await.map_err(|e| e.to_string())?;

        Ok(CassandraSession {
            session: Arc::new(session),
            keyspace: config.keyspace.clone(),
            set_tracing: config.tracing,
        })
    }

    pub async fn create_docker_schema(&self) -> Result<(), String> {
        self.session.query_unpaged(
            Query::new(
                format!("CREATE KEYSPACE IF NOT EXISTS {} WITH REPLICATION = {{ 'class' : 'SimpleStrategy', 'replication_factor' : 1 }};", self.keyspace),
            ),
            &[],
        ).await
            .map_err(|e| e.to_string())?;

        self.session
            .query_unpaged(
                Query::new(format!(
                    r#"
            CREATE TABLE IF NOT EXISTS {}.kv_store (
                namespace TEXT,
                key TEXT,
                value BLOB,
                PRIMARY KEY (namespace, key)
            );
        "#,
                    self.keyspace
                )),
                &[],
            )
            .await
            .map_err(|e| e.to_string())?;

        self.session
            .query_unpaged(
                Query::new(format!(
                    r#"
            CREATE TABLE IF NOT EXISTS {}.kv_sets (
                namespace TEXT,
                key TEXT,
                value BLOB,
                PRIMARY KEY ((namespace, key), value)
            );
        "#,
                    self.keyspace
                )),
                &[],
            )
            .await
            .map_err(|e| e.to_string())?;

        self.session
            .query_unpaged(
                Query::new(format!(
                    r#"
                CREATE TABLE IF NOT EXISTS {}.kv_sorted_sets (
                    namespace TEXT,
                    key TEXT,
                    score DOUBLE,
                    value BLOB,
                    PRIMARY KEY ((namespace, key), score, value)
                );
        "#,
                    self.keyspace
                )),
                &[],
            )
            .await
            .map_err(|e| e.to_string())
            .map(|_| ())
    }

    pub fn with(&self, svc_name: &'static str, api_name: &'static str) -> CassandraLabelledApi {
        CassandraLabelledApi {
            svc_name,
            api_name,
            keyspace: self.keyspace.clone(),
            cassandra: self.clone(),
        }
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

pub struct CassandraLabelledApi {
    svc_name: &'static str,
    api_name: &'static str,
    keyspace: String,
    pub cassandra: CassandraSession,
}

impl CassandraLabelledApi {
    fn record<R: Debug>(
        &self,
        start: Instant,
        cmd_name: &'static str,
        result: Result<R, QueryError>,
    ) -> Result<R, QueryError> {
        let end = Instant::now();
        match result {
            Ok(result) => {
                record_db_success(
                    "cassandra",
                    self.svc_name,
                    self.api_name,
                    cmd_name,
                    end.duration_since(start),
                );
                Ok(result)
            }
            Err(err) => {
                record_db_failure("cassandra", self.svc_name, self.api_name, cmd_name);
                Err(err)
            }
        }
    }

    async fn statement(&self, query_text: &str) -> PreparedStatement {
        let mut statement = self.cassandra.session.prepare(query_text).await.unwrap();
        statement.set_tracing(self.cassandra.set_tracing);
        statement
    }

    pub async fn set(&self, namespace: &str, key: &str, value: &[u8]) -> Result<(), QueryError> {
        let query = format!(
            "INSERT INTO {}.kv_store (namespace, key, value) VALUES (?, ?, ?);",
            self.keyspace
        );

        let start = Instant::now();
        self.record(
            start,
            "set",
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, (namespace, key, value))
                .await,
        )
        .map(|_| ())
    }

    pub async fn set_many(
        &self,
        namespace: &str,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), QueryError> {
        let query = format!(
            "INSERT INTO {}.kv_store (namespace, key, value) VALUES (?, ?, ?)",
            self.keyspace
        );
        let mut batch: Batch = Batch::new(BatchType::Logged);

        let start = Instant::now();
        for _ in 1..=pairs.len() {
            batch.append_statement(self.statement(&query).await);
        }

        let mut batch: Batch = self.cassandra.session.prepare_batch(&batch).await?;

        batch.set_tracing(self.cassandra.set_tracing);

        let values = pairs
            .iter()
            .map(|(field_key, field_value)| (namespace, *field_key, *field_value))
            .collect::<Vec<_>>();

        self.record(
            start,
            "set_many",
            self.cassandra.session.batch(&batch, &values).await,
        )
        .map(|_| ())
    }

    pub async fn set_if_not_exists(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<bool, QueryError> {
        let existing = self
            .cassandra
            .session
            .execute_unpaged(
                &self
                    .statement(&format!(
                        "SELECT key FROM {}.kv_store WHERE namespace = ? AND key = ?;",
                        self.keyspace
                    ))
                    .await,
                (namespace, key),
            )
            .await?
            .maybe_first_row_typed::<(String,)>()
            .map_err(|e| QueryError::InvalidMessage(e.to_string()))?;

        let query = format!(
            "INSERT INTO {}.kv_store (namespace, key, value) VALUES (?, ?, ?) IF NOT EXISTS;",
            self.keyspace
        );

        let start = Instant::now();
        self.record(
            start,
            "set_if_not_exists",
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, (namespace, key, value))
                .await,
        )
        .map(|_| existing.is_none())
    }

    pub async fn get(&self, namespace: &str, key: &str) -> Result<Option<Bytes>, QueryError> {
        let query = format!(
            "SELECT value FROM {}.kv_store WHERE namespace = ? AND key = ?;",
            self.keyspace
        );

        let start = Instant::now();

        self.record(
            start,
            "get",
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, (namespace, key))
                .await?
                .maybe_first_row_typed::<ValueRow>()
                .map_err(|e| QueryError::InvalidMessage(e.to_string()))
                .map(|opt_row| opt_row.map(|row| row.into_bytes())),
        )
    }

    pub async fn get_many(
        &self,
        namespace: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Bytes>>, QueryError> {
        let placeholders: String = repeat("?").take(keys.len()).collect::<Vec<_>>().join(", ");
        let query = format!(
            "SELECT key, value FROM {}.kv_store WHERE namespace = ? AND key IN ({});",
            self.keyspace, placeholders
        );

        let start = Instant::now();
        let parameters: Vec<String> = vec![namespace.to_string()]
            .into_iter()
            .chain(keys)
            .collect();
        let mut rows = self
            .cassandra
            .session
            .execute_iter(self.statement(&query).await, &parameters)
            .await?
            .into_typed::<KeyValueRow>();

        let keys = parameters[1..].to_vec();

        let mut result = Vec::new();
        while let Some(row) = rows.next().await {
            match row {
                Ok(row) => result.push(row.into_pair()),
                Err(err) => {
                    return self.record(
                        start,
                        "get_many",
                        Err(QueryError::InvalidMessage(err.to_string())),
                    )
                }
            }
        }
        let result = self.record(start, "get_many", Ok(result)).unwrap();

        let mut result_map = result.into_iter().collect::<HashMap<String, Bytes>>();

        let values = keys
            .into_iter()
            .map(|key| result_map.remove(&key))
            .collect::<Vec<Option<Bytes>>>();

        Ok(values)
    }

    pub async fn del(&self, namespace: &str, key: &str) -> Result<(), QueryError> {
        let query = format!(
            "DELETE FROM {}.kv_store WHERE namespace = ? AND key = ?;",
            self.keyspace
        );

        let start = Instant::now();
        self.record(
            start,
            "del",
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, (namespace, key))
                .await,
        )
        .map(|_| ())
    }

    pub async fn del_many(&self, namespace: &str, keys: Vec<String>) -> Result<(), QueryError> {
        let placeholders: String = repeat("?").take(keys.len()).collect::<Vec<_>>().join(", ");
        let query = format!(
            "DELETE FROM {}.kv_store WHERE namespace = ? AND key IN ({});",
            self.keyspace, placeholders
        );

        let start = Instant::now();
        let parameters: Vec<String> = vec![namespace.to_string()]
            .into_iter()
            .chain(keys)
            .collect();

        self.record(
            start,
            "del_many",
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, &parameters)
                .await,
        )
        .map(|_| ())
    }

    pub async fn exists(&self, namespace: &str, key: &str) -> Result<bool, QueryError> {
        let query = format!(
            "SELECT value FROM {}.kv_store WHERE namespace = ? AND key = ? LIMIT 1;",
            self.keyspace
        );

        let start = Instant::now();
        let rows = self
            .record(
                start,
                "exists",
                self.cassandra
                    .session
                    .execute_unpaged(&self.statement(&query).await, (namespace, key))
                    .await,
            )?
            .rows;
        Ok(rows.map_or(false, |rows| !rows.is_empty()))
    }

    pub async fn keys(&self, namespace: &str) -> Result<Vec<String>, QueryError> {
        let query = format!(
            "SELECT key FROM {}.kv_store WHERE namespace = ?;",
            self.keyspace
        );
        let mut result = Vec::new();

        let start = Instant::now();
        let mut rows = self
            .cassandra
            .session
            .execute_iter(self.statement(&query).await, &(namespace,))
            .await?
            .into_typed::<(String,)>();

        while let Some(row) = rows.next().await {
            match row {
                Ok(row) => result.push(row.0),
                Err(err) => return Err(QueryError::InvalidMessage(err.to_string())),
            }
        }
        self.record(start, "keys", Ok(result))
    }

    pub async fn add_to_set(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), QueryError> {
        let query = format!(
            "INSERT INTO {}.kv_sets (namespace, key, value) VALUES (?, ?, ?);",
            self.keyspace
        );

        let start = Instant::now();
        self.record(
            start,
            "add_to_set",
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, (namespace, key, value))
                .await,
        )
        .map(|_| ())
    }

    pub async fn remove_from_set(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), QueryError> {
        let query = format!(
            "DELETE FROM {}.kv_sets WHERE namespace = ? AND key = ? AND value = ?;",
            self.keyspace
        );

        let start = Instant::now();
        self.record(
            start,
            "del",
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, (namespace, key, value))
                .await,
        )
        .map(|_| ())
    }

    pub async fn members_of_set(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Vec<Bytes>, QueryError> {
        let query = format!(
            "SELECT value FROM {}.kv_sets WHERE namespace = ? AND key = ?;",
            self.keyspace
        );

        let start = Instant::now();
        let mut rows = self
            .cassandra
            .session
            .execute_iter(self.statement(&query).await, (namespace, key))
            .await?
            .into_typed::<ValueRow>();

        let mut result = Vec::new();
        while let Some(row) = rows.next().await {
            match row {
                Ok(row) => result.push(row.into_bytes()),
                Err(err) => {
                    return self.record(
                        start,
                        "members_of_set",
                        Err(QueryError::InvalidMessage(err.to_string())),
                    )
                }
            }
        }
        self.record(start, "members_of_set", Ok(result))
    }

    pub async fn add_to_sorted_set(
        &self,
        namespace: &str,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), QueryError> {
        self.remove_from_sorted_set(namespace, key, value).await?;
        let insert_statement = format!(
            "INSERT INTO {}.kv_sorted_sets (namespace, key, score, value) VALUES (?, ?, ?, ?);",
            self.keyspace
        );

        let start = Instant::now();
        self.record(
            start,
            "add_to_sorted_set",
            self.cassandra
                .session
                .execute_unpaged(
                    &self.statement(&insert_statement).await,
                    (namespace, key, score, value),
                )
                .await,
        )
        .map(|_| ())
    }

    pub async fn remove_from_sorted_set(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), QueryError> {
        let get_score = format!(
            "SELECT SCORE FROM {}.kv_sorted_sets WHERE namespace = ? AND key = ? AND value = ?  ALLOW FILTERING;",
            self.keyspace
        );
        let start = Instant::now();
        match self
            .cassandra
            .session
            .execute_unpaged(&self.statement(&get_score).await, &(namespace, key, value))
            .await?
            .maybe_first_row_typed::<(f64,)>()
        {
            Ok(None) => self.record(start, "remove_from_sorted_set", Ok(())),
            Ok(Some((score,))) => {
                let delete_statement = format!("DELETE FROM {}.kv_sorted_sets WHERE namespace = ? AND key = ? AND score = ? AND value = ?;", self.keyspace);
                self.record(
                    start,
                    "remove_from_sorted_set",
                    self.cassandra
                        .session
                        .execute_unpaged(
                            &self.statement(&delete_statement).await,
                            &(namespace, key, score, value),
                        )
                        .await,
                )
                .map(|_| ())
            }
            Err(err) => self.record(
                start,
                "remove_from_sorted_set",
                Err(QueryError::InvalidMessage(err.to_string())),
            ),
        }
    }

    async fn execute_query(
        &self,
        statement: String,
        values: impl SerializeRow,
        cmd_name: &'static str,
    ) -> Result<Vec<(f64, Bytes)>, QueryError> {
        let start = Instant::now();

        let mut rows = self
            .cassandra
            .session
            .execute_iter(self.statement(&statement).await, values)
            .await?
            .into_typed::<ScoreValueRow>();

        let mut result = Vec::new();
        while let Some(row) = rows.next().await {
            match row {
                Ok(row) => result.push(row.into_pair()),
                Err(err) => {
                    return self.record(
                        start,
                        cmd_name,
                        Err(QueryError::InvalidMessage(err.to_string())),
                    )
                }
            }
        }
        self.record(start, cmd_name, Ok(result))
    }

    pub async fn get_sorted_set(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, QueryError> {
        let query = format!(
            "SELECT score, value FROM {}.kv_sorted_sets WHERE namespace = ? AND key = ? ORDER BY score ASC;",
            self.keyspace
        );

        self.execute_query(query, (namespace, key), "get_sorted_set")
            .await
    }

    pub async fn query_sorted_set(
        &self,
        namespace: &str,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, QueryError> {
        let query = format!(
            "SELECT score, value FROM {}.kv_sorted_sets WHERE namespace = ? AND key = ? AND score >= ? AND score <= ? ORDER BY score ASC;",
            self.keyspace
        );
        self.execute_query(query, (namespace, key, min, max), "query_sorted_set")
            .await
    }
}
