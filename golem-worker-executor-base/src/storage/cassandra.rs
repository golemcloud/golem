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
use std::fmt::Debug;
use std::time::Instant;
use std::{num::NonZeroUsize, sync::Arc};

#[derive(Debug, Clone)]
pub struct CassandraSession {
    pub session: Arc<Session>,
    pub keyspace: String,
    pub set_tracing: bool,
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

    pub async fn create_schema(&self) -> Result<(), String> {
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
                );"#,
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
                );"#,
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
                );"#,
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
            cassandra: self.clone(),
        }
    }
}

pub struct CassandraLabelledApi {
    svc_name: &'static str,
    api_name: &'static str,
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

    pub async fn perform_query(
        &self,
        cmd_name: &'static str,
        query: String,
        values: impl SerializeRow,
    ) -> Result<(), QueryError> {
        let start = Instant::now();
        self.record(
            start,
            cmd_name,
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, values)
                .await,
        )
        .map(|_| ())
    }

    pub async fn perform_batch(
        &self,
        cmd_name: &'static str,
        query: String,
        values: Vec<impl SerializeRow>,
    ) -> Result<(), QueryError> {
        let mut batch: Batch = Batch::new(BatchType::Logged);

        let start = Instant::now();
        for _ in 1..=values.len() {
            batch.append_statement(self.statement(&query).await);
        }

        let mut batch: Batch = self.cassandra.session.prepare_batch(&batch).await?;

        batch.set_tracing(self.cassandra.set_tracing);

        self.record(
            start,
            cmd_name,
            self.cassandra.session.batch(&batch, &values).await,
        )
        .map(|_| ())
    }

    pub async fn maybe_row<RowT, T, F>(
        &self,
        cmd_name: &'static str,
        query: String,
        values: impl SerializeRow,
        map_to_value: F,
    ) -> Result<Option<T>, QueryError>
    where
        RowT: FromRow,
        T: Debug,
        F: FnOnce(RowT) -> T,
    {
        let start = Instant::now();

        self.record(
            start,
            cmd_name,
            self.cassandra
                .session
                .execute_unpaged(&self.statement(&query).await, values)
                .await?
                .maybe_first_row_typed::<RowT>()
                .map_err(|e| QueryError::InvalidMessage(e.to_string()))
                .map(|opt_row| opt_row.map(map_to_value)),
        )
    }

    pub async fn get_rows<RowT, T, F>(
        &self,
        cmd_name: &'static str,
        query: String,
        values: impl SerializeRow,
        mut map_to_value: F,
    ) -> Result<Vec<T>, QueryError>
    where
        RowT: FromRow,
        T: Debug,
        F: FnMut(RowT) -> T,
    {
        let start = Instant::now();

        let mut rows = self
            .cassandra
            .session
            .execute_iter(self.statement(&query).await, &values)
            .await?
            .into_typed::<RowT>();

        let mut result = Vec::new();
        while let Some(row) = rows.next().await {
            match row {
                Ok(row) => result.push(map_to_value(row)),
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
}
