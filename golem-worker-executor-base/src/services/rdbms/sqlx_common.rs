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
use crate::services::rdbms::metrics::record_rdbms_metrics;
use crate::services::rdbms::{
    DbResult, DbResultSet, DbRow, DbTransaction, Error, Rdbms, RdbmsPoolKey, RdbmsStatus, RdbmsType,
};
use async_dropper_simple::AsyncDrop;
use async_trait::async_trait;
use dashmap::DashMap;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::WorkerId;
use sqlx::pool::PoolConnection;
use sqlx::{Database, Pool, Row, TransactionManager};
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info};

#[derive(Clone)]
pub(crate) struct SqlxRdbms<T, DB>
where
    T: RdbmsType + Display,
    DB: Database,
{
    rdbms_type: T,
    config: RdbmsConfig,
    pool_cache: Cache<RdbmsPoolKey, (), Arc<Pool<DB>>, Error>,
    pool_workers_cache: DashMap<RdbmsPoolKey, HashSet<WorkerId>>,
}

impl<T, DB> SqlxRdbms<T, DB>
where
    T: RdbmsType + Display + Default + Send + Sync + QueryExecutor<T, DB>,
    DB: Database,
    RdbmsPoolKey: PoolCreator<DB>,
{
    pub(crate) fn new(config: RdbmsConfig) -> Self {
        let rdbms_type = T::default();
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
        let rdbms_type = self.rdbms_type.to_string();
        let pool = self
            .pool_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    info!(
                        rdbms_type = rdbms_type,
                        pool_key = key_clone.to_string(),
                        "create pool, connections: {}",
                        pool_config.max_connections
                    );
                    let result = key_clone.create_pool(&pool_config).await.map_err(|e| {
                        error!(
                            rdbms_type = rdbms_type,
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
        record_rdbms_metrics(&self.rdbms_type, name, start, result)
    }
}

#[async_trait]
impl<T, DB> Rdbms<T> for SqlxRdbms<T, DB>
where
    T: RdbmsType + Display + Default + Send + Sync + QueryExecutor<T, DB> + 'static,
    DB: Database,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
    RdbmsPoolKey: PoolCreator<DB>,
{
    async fn create(&self, address: &str, worker_id: &WorkerId) -> Result<RdbmsPoolKey, Error> {
        let start = Instant::now();

        let result = {
            let key = RdbmsPoolKey::from(address).map_err(Error::ConnectionFailure)?;
            info!(
                rdbms_type = self.rdbms_type.to_string(),
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
        params: Vec<T::DbValue>,
    ) -> Result<u64, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = key.to_string(),
            "execute - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let result = {
            let pool = self.get_or_create(key, worker_id).await?;

            T::execute(statement, params, pool.deref()).await
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
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
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultSet<T> + Send + Sync>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = key.to_string(),
            "query - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let result = {
            let pool = self.get_or_create(key, worker_id).await?;
            T::query_stream(
                statement,
                params,
                self.config.query.query_batch,
                pool.deref(),
            )
            .await
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = key.to_string(),
                "query - statement: {}, error: {}",
                statement,
                e
            );
            e
        });
        self.record_metrics("query", start, result)
    }

    async fn begin_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
    ) -> Result<Arc<dyn DbTransaction<T> + Send + Sync>, Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = key.to_string(),
            "begin",
        );

        let result = {
            let pool = self.get_or_create(key, worker_id).await?;

            let mut connection = pool
                .deref()
                .acquire()
                .await
                .map_err(Error::connection_failure)?;
            DB::TransactionManager::begin(&mut connection)
                .await
                .map_err(Error::query_execution_failure)?;

            let db_transaction: Arc<dyn DbTransaction<T> + Send + Sync> =
                Arc::new(SqlxDbTransaction::new(key.clone(), connection));

            Ok(db_transaction)
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = key.to_string(),
                "begin - error: {}",
                e
            );
            e
        });
        self.record_metrics("begin", start, result)
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
pub(crate) trait QueryExecutor<T: RdbmsType, DB: Database> {
    async fn execute<'c, E>(
        statement: &str,
        params: Vec<T::DbValue>,
        executor: E,
    ) -> Result<u64, Error>
    where
        E: sqlx::Executor<'c, Database = DB>;

    async fn query<'c, E>(
        statement: &str,
        params: Vec<T::DbValue>,
        executor: E,
    ) -> Result<DbResult<T>, Error>
    where
        E: sqlx::Executor<'c, Database = DB>;

    async fn query_stream<'c, E>(
        statement: &str,
        params: Vec<T::DbValue>,
        batch: usize,
        executor: E,
    ) -> Result<Arc<dyn DbResultSet<T> + Send + Sync + 'c>, Error>
    where
        E: sqlx::Executor<'c, Database = DB>;
}

#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct SqlxDbTransaction<T: RdbmsType, DB: Database> {
    rdbms_type: T,
    pool_key: RdbmsPoolKey,
    transaction: Arc<async_mutex::Mutex<(PoolConnection<DB>, bool)>>,
}

impl<T, DB> SqlxDbTransaction<T, DB>
where
    T: RdbmsType + Display + Default + Send + Sync + QueryExecutor<T, DB>,
    DB: Database,
{
    fn new(pool_key: RdbmsPoolKey, connection: PoolConnection<DB>) -> Self {
        let rdbms_type = T::default();
        Self {
            rdbms_type,
            pool_key,
            transaction: Arc::new(async_mutex::Mutex::new((connection, true))),
        }
    }

    fn record_metrics<R>(
        &self,
        name: &'static str,
        start: Instant,
        result: Result<R, Error>,
    ) -> Result<R, Error> {
        record_rdbms_metrics(&self.rdbms_type, name, start, result)
    }
}

#[async_trait]
impl<T, DB> DbTransaction<T> for SqlxDbTransaction<T, DB>
where
    T: RdbmsType + Display + Default + Send + Sync + QueryExecutor<T, DB>,
    DB: Database,
    for<'c> &'c mut <DB as sqlx::Database>::Connection: sqlx::Executor<'c, Database = DB>,
    RdbmsPoolKey: PoolCreator<DB>,
{
    async fn execute(&self, statement: &str, params: Vec<T::DbValue>) -> Result<u64, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            "execute - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let result = {
            let mut transaction = self.transaction.lock().await;
            T::execute(statement, params, transaction.0.deref_mut()).await
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = self.pool_key.to_string(),
                "execute - statement: {}, error: {}",
                statement,
                e
            );
            e
        });
        self.record_metrics("execute", start, result)
    }

    async fn query(&self, statement: &str, params: Vec<T::DbValue>) -> Result<DbResult<T>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            "query - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let result = {
            let mut transaction = self.transaction.lock().await;
            T::query(statement, params, transaction.0.deref_mut()).await
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = self.pool_key.to_string(),
                "query - statement: {}, error: {}",
                statement,
                e
            );
            e
        });
        self.record_metrics("query", start, result)
    }

    async fn commit(&self) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            "commit"
        );

        let mut transaction = self.transaction.lock().await;
        let result = DB::TransactionManager::commit(transaction.0.deref_mut()).await;
        if result.is_ok() {
            transaction.1 = false;
        }

        let result = result.map_err(|e| {
            let e = Error::query_execution_failure(e);
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = self.pool_key.to_string(),
                "commit - error: {}",
                e
            );
            e
        });
        self.record_metrics("commit", start, result)
    }

    async fn rollback(&self) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            "rollback"
        );

        let mut transaction = self.transaction.lock().await;
        let result = DB::TransactionManager::rollback(transaction.0.deref_mut()).await;
        if result.is_ok() {
            transaction.1 = false;
        }
        let result = result.map_err(|e| {
            let e = Error::query_execution_failure(e);
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = self.pool_key.to_string(),
                "rollback - error: {}",
                e
            );
            e
        });
        self.record_metrics("rollback", start, result)
    }

    async fn rollback_if_open(&self) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            "rollback if open"
        );

        let mut transaction = self.transaction.lock().await;

        if transaction.1 {
            DB::TransactionManager::start_rollback(&mut transaction.0);
        }

        self.record_metrics("rollback-if-open", start, Ok(()))
    }
}

#[async_trait]
impl<T, DB> AsyncDrop for SqlxDbTransaction<T, DB>
where
    T: RdbmsType + Display + Default + Send + Sync + QueryExecutor<T, DB>,
    DB: Database,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
    RdbmsPoolKey: PoolCreator<DB>,
{
    async fn async_drop(&mut self) {
        let _ = SqlxDbTransaction::rollback_if_open(self).await;
    }
}

#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct SqlxDbResultSet<'q, T: RdbmsType, DB: Database> {
    rdbms_type: T,
    columns: Vec<T::DbColumn>,
    first_rows: Arc<async_mutex::Mutex<Option<Vec<DbRow<T::DbValue>>>>>,
    row_stream: Arc<async_mutex::Mutex<BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>>>>,
}

impl<'q, T, DB> SqlxDbResultSet<'q, T, DB>
where
    T: RdbmsType + Display + Default + Send + Sync,
    DB: Database,
    DB::Row: Row,
    DbRow<T::DbValue>: for<'a> TryFrom<&'a DB::Row, Error = String>,
    T::DbColumn: for<'a> TryFrom<&'a DB::Column, Error = String>,
{
    fn new(
        columns: Vec<T::DbColumn>,
        first_rows: Vec<DbRow<T::DbValue>>,
        row_stream: BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>>,
    ) -> Self {
        let rdbms_type = T::default();
        Self {
            rdbms_type,
            columns,
            first_rows: Arc::new(async_mutex::Mutex::new(Some(first_rows))),
            row_stream: Arc::new(async_mutex::Mutex::new(row_stream)),
        }
    }

    pub(crate) async fn create(
        stream: BoxStream<'q, Result<DB::Row, sqlx::Error>>,
        batch: usize,
    ) -> Result<SqlxDbResultSet<'q, T, DB>, Error> {
        let mut row_stream: BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>> =
            Box::pin(stream.chunks(batch));

        let first: Option<Vec<Result<DB::Row, sqlx::Error>>> = row_stream.next().await;

        match first {
            Some(rows) if !rows.is_empty() => {
                let rows: Vec<DB::Row> = rows
                    .into_iter()
                    .collect::<Result<Vec<_>, sqlx::Error>>()
                    .map_err(Error::query_execution_failure)?;

                let result = create_db_result::<T, DB>(rows)?;

                Ok(SqlxDbResultSet::new(
                    result.columns,
                    result.rows,
                    row_stream,
                ))
            }
            _ => Ok(SqlxDbResultSet::new(vec![], vec![], row_stream)),
        }
    }
}

#[async_trait]
impl<T, DB> DbResultSet<T> for SqlxDbResultSet<'_, T, DB>
where
    T: RdbmsType + Display + Default + Send + Sync,
    DB: Database,
    DB::Row: Row,
    DbRow<T::DbValue>: for<'a> TryFrom<&'a DB::Row, Error = String>,
{
    async fn get_columns(&self) -> Result<Vec<T::DbColumn>, Error> {
        debug!(rdbms_type = self.rdbms_type.to_string(), "get columns");
        Ok(self.columns.clone())
    }

    async fn get_next(&self) -> Result<Option<Vec<DbRow<T::DbValue>>>, Error> {
        let mut rows = self.first_rows.lock().await;
        if rows.is_some() {
            debug!(
                rdbms_type = self.rdbms_type.to_string(),
                "get next - initial"
            );
            let result = rows.clone();
            *rows = None;
            Ok(result)
        } else {
            debug!(rdbms_type = self.rdbms_type.to_string(), "get next");
            let mut stream = self.row_stream.lock().await;
            let next = stream.next().await;

            if let Some(rows) = next {
                let mut values = Vec::with_capacity(rows.len());
                for row in rows.into_iter() {
                    let row = row.map_err(Error::query_response_failure)?;
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

pub(crate) trait QueryParamsBinder<'q, T: RdbmsType, DB: Database> {
    fn bind_params(
        self,
        params: Vec<T::DbValue>,
    ) -> Result<sqlx::query::Query<'q, DB, <DB as Database>::Arguments<'q>>, Error>;
}

pub(crate) fn create_db_result<T, DB>(rows: Vec<DB::Row>) -> Result<DbResult<T>, Error>
where
    T: RdbmsType + Send + Sync,
    DB: Database,
    DB::Row: Row,
    DbRow<T::DbValue>: for<'a> TryFrom<&'a DB::Row, Error = String>,
    T::DbColumn: for<'a> TryFrom<&'a DB::Column, Error = String>,
{
    if rows.is_empty() {
        Ok(DbResult::empty())
    } else {
        let columns = rows[0]
            .columns()
            .iter()
            .map(|c: &DB::Column| c.try_into())
            .collect::<Result<Vec<_>, String>>()
            .map_err(Error::QueryResponseFailure)?;

        let values = rows
            .iter()
            .map(|r: &DB::Row| r.try_into())
            .collect::<Result<Vec<_>, String>>()
            .map_err(Error::QueryResponseFailure)?;

        Ok(DbResult::new(columns, values))
    }
}
