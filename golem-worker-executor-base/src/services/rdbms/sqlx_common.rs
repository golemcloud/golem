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

use crate::services::golem_config::{RdbmsConfig, RdbmsPoolConfig};
use crate::services::rdbms::metrics::{record_rdbms_failure, record_rdbms_success};
use crate::services::rdbms::types::{DbColumn, DbResultSet, DbRow, DbValue, Error};
use crate::services::rdbms::{Rdbms, RdbmsPoolKey, RdbmsStatus, RdbmsType};
use async_trait::async_trait;
use dashmap::DashMap;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::WorkerId;
use sqlx::database::HasArguments;
use sqlx::{Database, Pool, Row};
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info};

#[derive(Clone)]
pub(crate) struct SqlxRdbms<DB>
where
    DB: Database,
{
    rdbms_type: &'static str,
    config: RdbmsConfig,
    pool_cache: Cache<RdbmsPoolKey, (), Arc<Pool<DB>>, Error>,
    pool_workers_cache: DashMap<RdbmsPoolKey, HashSet<WorkerId>>,
}

impl<DB> SqlxRdbms<DB>
where
    DB: Database,
    Pool<DB>: QueryExecutor,
    RdbmsPoolKey: PoolCreator<DB>,
{
    pub(crate) fn new(rdbms_type: &'static str, config: RdbmsConfig) -> Self {
        let cache_name: &'static str = format!("rdbms-{}-pools", rdbms_type).leak();
        let pool_cache = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::OlderThan {
                ttl: config.pool.eviction_ttl,
                period: config.pool.eviction_period,
            },
            cache_name,
        );
        let pool_workers_cache = DashMap::new();
        Self {
            rdbms_type,
            config,
            pool_cache,
            pool_workers_cache,
        }
    }

    async fn get_or_create(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
    ) -> Result<Arc<Pool<DB>>, Error> {
        let key_clone = key.clone();
        let pool_config = self.config.pool;
        let name = self.rdbms_type.to_string();
        let pool = self
            .pool_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    info!(
                        rdbms_type = name,
                        pool_key = key_clone.to_string(),
                        "create pool, connections: {}",
                        pool_config.max_connections
                    );
                    let result = key_clone.create_pool(&pool_config).await.map_err(|e| {
                        error!(
                            rdbms_type = name,
                            pool_key = key_clone.to_string(),
                            "create pool, connections: {}, error: {}",
                            pool_config.max_connections,
                            e
                        );
                        e
                    })?;
                    Ok(Arc::new(result))
                })
            })
            .await?;

        self.pool_workers_cache
            .entry(key.clone())
            .or_default()
            .insert(worker_id.clone());

        Ok(pool)
    }

    #[allow(dead_code)]
    pub(crate) async fn remove_pool(&self, key: &RdbmsPoolKey) -> Result<bool, Error> {
        let _ = self.pool_workers_cache.remove(key);
        let pool = self.pool_cache.try_get(key);
        if let Some(pool) = pool {
            self.pool_cache.remove(key);
            pool.close().await;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn record_metrics<R>(
        &self,
        name: &'static str,
        start: Instant,
        result: Result<R, Error>,
    ) -> Result<R, Error> {
        let end = Instant::now();
        match result {
            Ok(result) => {
                record_rdbms_success(self.rdbms_type, name, end.duration_since(start));
                Ok(result)
            }
            Err(err) => {
                record_rdbms_failure(self.rdbms_type, name);
                Err(err)
            }
        }
    }
}

#[async_trait]
impl<T, DB> Rdbms<T> for SqlxRdbms<DB>
where
    T: RdbmsType,
    DB: Database,
    Pool<DB>: QueryExecutor,
    RdbmsPoolKey: PoolCreator<DB>,
{
    async fn create(&self, address: &str, worker_id: &WorkerId) -> Result<RdbmsPoolKey, Error> {
        let start = Instant::now();

        let result = {
            let key = RdbmsPoolKey::from(address).map_err(Error::ConnectionFailure)?;
            info!(
                rdbms_type = self.rdbms_type,
                pool_key = key.to_string(),
                "create connection",
            );

            let _ = self.get_or_create(&key, worker_id).await?;

            Ok(key)
        };
        self.record_metrics("create", start, result)
    }

    fn remove(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool {
        match self.pool_workers_cache.get_mut(key) {
            Some(mut workers) => (*workers).remove(worker_id),
            None => false,
        }
    }

    fn exists(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool {
        self.pool_workers_cache
            .get(key)
            .is_some_and(|workers| workers.contains(worker_id))
    }

    async fn execute(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<u64, Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type,
            pool_key = key.to_string(),
            "execute - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let result = {
            let pool = self.get_or_create(key, worker_id).await?;
            pool.deref().execute(statement, params).await
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type,
                pool_key = key.to_string(),
                "execute - statement: {}, error: {}",
                statement,
                e
            );
            e
        });
        self.record_metrics("execute", start, result)
    }

    async fn query(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type,
            pool_key = key.to_string(),
            "query - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let result = {
            let pool = self.get_or_create(key, worker_id).await?;
            pool.deref()
                .query_stream(statement, params, self.config.query.query_batch)
                .await
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type,
                pool_key = key.to_string(),
                "query - statement: {}, error: {}",
                statement,
                e
            );
            e
        });
        self.record_metrics("query", start, result)
    }

    fn status(&self) -> RdbmsStatus {
        let pools: HashMap<RdbmsPoolKey, HashSet<WorkerId>> = self
            .pool_workers_cache
            .iter()
            .map(|kv| (kv.key().clone(), kv.value().clone()))
            .collect();
        RdbmsStatus { pools }
    }
}

#[async_trait]
pub(crate) trait PoolCreator<DB: Database> {
    async fn create_pool(&self, config: &RdbmsPoolConfig) -> Result<Pool<DB>, Error>;
}

#[async_trait]
pub(crate) trait QueryExecutor {
    async fn execute(&self, statement: &str, params: Vec<DbValue>) -> Result<u64, Error>;

    async fn query_stream(
        &self,
        statement: &str,
        params: Vec<DbValue>,
        batch: usize,
    ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error>;
}

#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct StreamDbResultSet<'q, DB: Database> {
    rdbms_type: &'static str,
    columns: Vec<DbColumn>,
    first_rows: Arc<async_mutex::Mutex<Option<Vec<DbRow>>>>,
    row_stream: Arc<async_mutex::Mutex<BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>>>>,
}

impl<'q, DB: Database> StreamDbResultSet<'q, DB>
where
    DB::Row: Row,
    DbRow: for<'a> TryFrom<&'a DB::Row, Error = String>,
    DbColumn: for<'a> TryFrom<&'a DB::Column, Error = String>,
{
    fn new(
        rdbms_type: &'static str,
        columns: Vec<DbColumn>,
        first_rows: Vec<DbRow>,
        row_stream: BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>>,
    ) -> Self {
        Self {
            rdbms_type,
            columns,
            first_rows: Arc::new(async_mutex::Mutex::new(Some(first_rows))),
            row_stream: Arc::new(async_mutex::Mutex::new(row_stream)),
        }
    }

    pub(crate) async fn create(
        rdbms_type: &'static str,
        stream: BoxStream<'q, Result<DB::Row, sqlx::Error>>,
        batch: usize,
    ) -> Result<StreamDbResultSet<'q, DB>, Error> {
        let mut row_stream: BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>> =
            Box::pin(stream.chunks(batch));

        let first: Option<Vec<Result<DB::Row, sqlx::Error>>> = row_stream.next().await;

        match first {
            Some(rows) if !rows.is_empty() => {
                let rows: Vec<DB::Row> = rows
                    .into_iter()
                    .collect::<Result<Vec<_>, sqlx::Error>>()
                    .map_err(|e| Error::QueryExecutionFailure(e.to_string()))?;

                let columns = rows[0]
                    .columns()
                    .iter()
                    .map(|c: &DB::Column| c.try_into())
                    .collect::<Result<Vec<_>, String>>()
                    .map_err(Error::QueryResponseFailure)?;

                let first_rows = rows
                    .iter()
                    .map(|r: &DB::Row| r.try_into())
                    .collect::<Result<Vec<_>, String>>()
                    .map_err(Error::QueryResponseFailure)?;

                Ok(StreamDbResultSet::new(
                    rdbms_type, columns, first_rows, row_stream,
                ))
            }
            _ => Ok(StreamDbResultSet::new(
                rdbms_type,
                vec![],
                vec![],
                row_stream,
            )),
        }
    }
}

#[async_trait]
impl<DB: Database> DbResultSet for StreamDbResultSet<'_, DB>
where
    DB::Row: Row,
    DbRow: for<'a> TryFrom<&'a DB::Row, Error = String>,
{
    async fn get_columns(&self) -> Result<Vec<DbColumn>, Error> {
        debug!(rdbms_type = self.rdbms_type, "get columns");
        Ok(self.columns.clone())
    }

    async fn get_next(&self) -> Result<Option<Vec<DbRow>>, Error> {
        let mut rows = self.first_rows.lock().await;
        if rows.is_some() {
            debug!(rdbms_type = self.rdbms_type, "get next - initial");
            let result = rows.clone();
            *rows = None;
            Ok(result)
        } else {
            debug!(rdbms_type = self.rdbms_type, "get next");
            let mut stream = self.row_stream.lock().await;
            let next = stream.next().await;

            if let Some(rows) = next {
                let mut values = Vec::with_capacity(rows.len());
                for row in rows.into_iter() {
                    let row = row.map_err(|e| Error::QueryResponseFailure(e.to_string()))?;
                    let value = (&row).try_into().map_err(Error::QueryResponseFailure)?;
                    values.push(value);
                }
                Ok(Some(values))
            } else {
                Ok(None)
            }
        }
    }
}

pub(crate) trait QueryParamsBinder<'q, DB: Database> {
    fn bind_params(
        self,
        params: Vec<DbValue>,
    ) -> Result<sqlx::query::Query<'q, DB, <DB as HasArguments<'q>>::Arguments>, Error>;
}
