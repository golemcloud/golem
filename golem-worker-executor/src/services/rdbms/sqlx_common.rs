// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::services::golem_config::{RdbmsConfig, RdbmsPoolConfig, RdbmsQueryConfig};
use crate::services::rdbms::metrics::record_rdbms_metrics;
use crate::services::rdbms::{
    DbResult, DbResultStream, DbRow, DbTransaction, Error, Rdbms, RdbmsPoolKey, RdbmsStatus,
    RdbmsTransactionStatus, RdbmsType,
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::StreamExt;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::{TransactionId, WorkerId};
use itertools::Either;
use sqlx::pool::PoolConnection;
use sqlx::{Database, Describe, Execute, Pool, Row, TransactionManager};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::fmt::Formatter;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info};

#[derive(Clone)]
pub(crate) struct SqlxRdbms<T, DB>
where
    T: RdbmsType,
    DB: Database,
{
    rdbms_type: T,
    config: RdbmsConfig,
    pool_cache: Cache<RdbmsPoolKey, (), Arc<Pool<DB>>, Error>,
    pool_workers_cache: scc::HashMap<RdbmsPoolKey, HashSet<WorkerId>>,
}

impl<T, DB> SqlxRdbms<T, DB>
where
    T: RdbmsType
        + Sync
        + QueryExecutor<T, DB>
        + BeginTransactionSupport<T, DB>
        + TransactionSupport<T, DB>,
    DB: Database,
    RdbmsPoolKey: PoolCreator<DB>,
{
    pub(crate) fn new(config: RdbmsConfig) -> Self {
        let rdbms_type = T::default();
        let cache_name: &'static str = format!("rdbms-{rdbms_type}-pools").leak();
        let pool_cache = Cache::new(
            None,
            FullCacheEvictionMode::None,
            BackgroundEvictionMode::OlderThan {
                ttl: config.pool.eviction_ttl,
                period: config.pool.eviction_period,
            },
            cache_name,
        );
        let pool_workers_cache = scc::HashMap::new();
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
            .entry_async(key.clone())
            .await
            .or_default()
            .get_mut()
            .insert(worker_id.clone());

        Ok(pool)
    }

    #[allow(dead_code)]
    pub(crate) async fn remove_pool(&self, key: &RdbmsPoolKey) -> Result<bool, Error> {
        let _ = self.pool_workers_cache.remove_async(key).await;
        let pool = self.pool_cache.try_get(key).await;
        if let Some(pool) = pool {
            self.pool_cache.remove(key).await;
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
    T: RdbmsType
        + Sync
        + QueryExecutor<T, DB>
        + BeginTransactionSupport<T, DB>
        + TransactionSupport<T, DB>
        + 'static,
    DB: Database,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
    RdbmsPoolKey: PoolCreator<DB>,
{
    async fn create(&self, address: &str, worker_id: &WorkerId) -> Result<RdbmsPoolKey, Error> {
        let start = Instant::now();

        let result = match RdbmsPoolKey::from(address) {
            Ok(key) => {
                info!(
                    rdbms_type = self.rdbms_type.to_string(),
                    pool_key = key.to_string(),
                    "create connection",
                );
                self.get_or_create(&key, worker_id)
                    .await
                    .map(|_| key.clone())
                    .map_err(|e| {
                        error!(
                            rdbms_type = self.rdbms_type.to_string(),
                            pool_key = key.to_string(),
                            "create connection error: {}",
                            e
                        );
                        e
                    })
            }
            Err(e) => {
                error!(
                    rdbms_type = self.rdbms_type.to_string(),
                    "create connection error: {}", e
                );
                Err(Error::ConnectionFailure(e))
            }
        };
        self.record_metrics("create-connection", start, result)
    }

    async fn remove(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool {
        self.pool_workers_cache
            .update_async(key, |_, workers| workers.remove(worker_id))
            .await
            .is_some()
    }

    async fn exists(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool {
        self.pool_workers_cache
            .read_async(key, |_, workers| workers.contains(worker_id))
            .await
            .unwrap_or_default()
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

        let pool = self.get_or_create(key, worker_id).await;
        let result = match pool {
            Ok(pool) => T::execute(statement, params, pool.deref()).await,
            Err(e) => Err(e),
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

    async fn query_stream(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = key.to_string(),
            "query stream - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let pool = self.get_or_create(key, worker_id).await;
        let result = match pool {
            Ok(pool) => {
                T::query_stream(
                    statement,
                    params,
                    self.config.query.query_batch,
                    pool.deref(),
                )
                .await
            }
            Err(e) => Err(e),
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = key.to_string(),
                "query stream - statement: {}, error: {}",
                statement,
                e
            );
            e
        });
        self.record_metrics("query-stream", start, result)
    }

    async fn query(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<DbResult<T>, Error>
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

        let pool = self.get_or_create(key, worker_id).await;
        let result = match pool {
            Ok(pool) => T::query(statement, params, pool.deref()).await,
            Err(e) => Err(e),
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
            "begin transaction",
        );

        let pool = self.get_or_create(key, worker_id).await;
        let result = match pool {
            Ok(pool) => T::begin_transaction(key, pool, self.config.query)
                .await
                .map(|r| r as Arc<dyn DbTransaction<T> + Send + Sync>),
            Err(e) => Err(e),
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = key.to_string(),
                "begin transaction - error: {}",
                e
            );
            e
        });
        self.record_metrics("begin-transaction", start, result)
    }

    async fn get_transaction_status(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        transaction_id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = key.to_string(),
            transaction_id = transaction_id.to_string(),
            "get transaction status",
        );

        let pool = self.get_or_create(key, worker_id).await;
        let result = match pool {
            Ok(pool) => T::get_transaction_status(pool.deref(), transaction_id).await,
            Err(e) => Err(e),
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = key.to_string(),
                transaction_id = transaction_id.to_string(),
                "get transaction status - error: {}",
                e
            );
            e
        });
        self.record_metrics("get-transaction-status", start, result)
    }

    async fn cleanup_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        transaction_id: &TransactionId,
    ) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = key.to_string(),
            transaction_id = transaction_id.to_string(),
            "cleanup transaction",
        );

        let pool = self.get_or_create(key, worker_id).await;
        let result = match pool {
            Ok(pool) => T::cleanup_transaction(pool.deref(), transaction_id).await,
            Err(e) => Err(e),
        };

        let result = result.map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = key.to_string(),
                transaction_id = transaction_id.to_string(),
                "cleanup transaction - error: {}",
                e
            );
            e
        });
        self.record_metrics("cleanup-transaction", start, result)
    }

    async fn status(&self) -> RdbmsStatus {
        let mut pools = HashMap::new();

        self.pool_workers_cache
            .iter_async(|pool_key, worker_ids| {
                pools.insert(pool_key.clone(), worker_ids.clone());
                true
            })
            .await;

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
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync + 'c>, Error>
    where
        E: sqlx::Executor<'c, Database = DB>;
}

struct TransactionConnection<DB: Database> {
    connection: PoolConnection<DB>,
    open: bool,
}

impl<DB: Database> TransactionConnection<DB> {
    fn new(connection: PoolConnection<DB>, open: bool) -> Self {
        Self { connection, open }
    }
}

struct SqlxDbTransactionConnection<DB: Database>(
    Arc<async_mutex::Mutex<TransactionConnection<DB>>>,
);

impl<DB: Database> SqlxDbTransactionConnection<DB> {
    fn new(connection: PoolConnection<DB>, open: bool) -> Self {
        Self(Arc::new(async_mutex::Mutex::new(
            TransactionConnection::new(connection, open),
        )))
    }
}

impl<DB: Database> Clone for SqlxDbTransactionConnection<DB> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<DB: Database> Debug for SqlxDbTransactionConnection<DB> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlxDbTransactionConnection").finish()
    }
}

#[derive(Clone)]
pub struct SqlxDbTransaction<T, DB>
where
    T: RdbmsType,
    DB: Database,
{
    rdbms_type: T,
    transaction_id: TransactionId,
    pool_key: RdbmsPoolKey,
    tx_connection: SqlxDbTransactionConnection<DB>,
    pool: Arc<Pool<DB>>,
    query_config: RdbmsQueryConfig,
}

impl<T: RdbmsType, DB: Database> Debug for SqlxDbTransaction<T, DB> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlxDbTransaction")
            .field("rdbms_type", &self.rdbms_type)
            .field("transaction_id", &self.transaction_id)
            .field("pool_key", &self.pool_key)
            .field("query_config", &self.query_config)
            .finish()
    }
}

impl<T, DB> SqlxDbTransaction<T, DB>
where
    T: RdbmsType + Sync + QueryExecutor<T, DB> + TransactionSupport<T, DB>,
    DB: Database,
{
    pub(crate) fn new(
        transaction_id: TransactionId,
        pool_key: RdbmsPoolKey,
        connection: PoolConnection<DB>,
        pool: Arc<Pool<DB>>,
        query_config: RdbmsQueryConfig,
    ) -> Self {
        let rdbms_type = T::default();
        Self {
            rdbms_type,
            transaction_id,
            pool_key,
            tx_connection: SqlxDbTransactionConnection::new(connection, true),
            pool,
            query_config,
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

impl<'p, DB: Database> sqlx::Executor<'p> for SqlxDbTransactionConnection<DB>
where
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
{
    type Database = DB;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<DB::QueryResult, DB::Row>, sqlx::Error>>
    where
        'p: 'e,
        E: 'q + Execute<'q, Self::Database>,
    {
        use futures::TryStreamExt;
        let tx = self.clone();
        Box::pin(sqlx_core::try_stream! {
            let mut tx_conn = tx.0.lock().await;
            let mut s = tx_conn.connection.fetch_many(query);

            while let Some(v) = s.try_next().await? {
                r#yield!(v);
            }

            Ok(())
        })
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<DB::Row>, sqlx::Error>>
    where
        'p: 'e,
        E: 'q + Execute<'q, Self::Database>,
    {
        let tx = self.clone();

        Box::pin(async move {
            let mut tx_conn = tx.0.lock().await;
            tx_conn.connection.fetch_optional(query).await
        })
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [<Self::Database as Database>::TypeInfo],
    ) -> BoxFuture<'e, Result<<Self::Database as Database>::Statement<'q>, sqlx::Error>>
    where
        'p: 'e,
    {
        let tx = self.clone();

        Box::pin(async move {
            let mut tx_conn = tx.0.lock().await;
            tx_conn.connection.prepare_with(sql, parameters).await
        })
    }

    #[doc(hidden)]
    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, Result<Describe<Self::Database>, sqlx::Error>>
    where
        'p: 'e,
    {
        let tx = self.clone();

        Box::pin(async move {
            let mut tx_conn = tx.0.lock().await;
            tx_conn.connection.describe(sql).await
        })
    }
}

#[async_trait]
impl<T, DB> DbTransaction<T> for SqlxDbTransaction<T, DB>
where
    T: RdbmsType + Sync + QueryExecutor<T, DB> + TransactionSupport<T, DB>,
    DB: Database,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
{
    fn transaction_id(&self) -> TransactionId {
        self.transaction_id.clone()
    }

    async fn execute(&self, statement: &str, params: Vec<T::DbValue>) -> Result<u64, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            transaction_id = self.transaction_id.to_string(),
            "execute - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let mut tx_conn = self.tx_connection.0.lock().await;

        let result = T::execute(statement, params, tx_conn.connection.deref_mut())
            .await
            .map_err(|e| {
                error!(
                    rdbms_type = self.rdbms_type.to_string(),
                    pool_key = self.pool_key.to_string(),
                    transaction_id = self.transaction_id.to_string(),
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
            transaction_id = self.transaction_id.to_string(),
            "query - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let mut tx_conn = self.tx_connection.0.lock().await;
        let result = T::query(statement, params, tx_conn.connection.deref_mut())
            .await
            .map_err(|e| {
                error!(
                    rdbms_type = self.rdbms_type.to_string(),
                    pool_key = self.pool_key.to_string(),
                    transaction_id = self.transaction_id.to_string(),
                    "query - statement: {}, error: {}",
                    statement,
                    e
                );
                e
            });
        self.record_metrics("query", start, result)
    }

    async fn query_stream(
        &self,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait,
    {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            transaction_id = self.transaction_id.to_string(),
            "query stream - statement: {}, params count: {}",
            statement,
            params.len()
        );

        let result = T::query_stream(
            statement,
            params,
            self.query_config.query_batch,
            self.tx_connection.clone(),
        )
        .await
        .map_err(|e| {
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = self.pool_key.to_string(),
                "query stream - statement: {}, error: {}",
                statement,
                e
            );
            e
        });
        self.record_metrics("query-stream", start, result)
    }

    async fn pre_commit(&self) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            transaction_id = self.transaction_id.to_string(),
            "pre-commit transaction"
        );

        let result = T::pre_commit_transaction(self.pool.deref(), self)
            .await
            .map_err(|e| {
                error!(
                    rdbms_type = self.rdbms_type.to_string(),
                    pool_key = self.pool_key.to_string(),
                    transaction_id = self.transaction_id.to_string(),
                    "pre-commit transaction - error: {}",
                    e
                );
                e
            });
        self.record_metrics("pre-commit-transaction", start, result)
    }

    async fn pre_rollback(&self) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            transaction_id = self.transaction_id.to_string(),
            "pre-rollback transaction"
        );

        let result = T::pre_rollback_transaction(self.pool.deref(), self)
            .await
            .map_err(|e| {
                error!(
                    rdbms_type = self.rdbms_type.to_string(),
                    pool_key = self.pool_key.to_string(),
                    transaction_id = self.transaction_id.to_string(),
                    "pre-rollback transaction - error: {}",
                    e
                );
                e
            });
        self.record_metrics("pre-rollback-transaction", start, result)
    }

    async fn commit(&self) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            transaction_id = self.transaction_id.to_string(),
            "commit transaction"
        );

        let mut tx_conn = self.tx_connection.0.lock().await;
        let result = DB::TransactionManager::commit(tx_conn.connection.deref_mut()).await;

        if result.is_ok() {
            tx_conn.open = false;
        }

        let result = result.map_err(|e| {
            let e = Error::query_execution_failure(e);
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = self.pool_key.to_string(),
                transaction_id = self.transaction_id.to_string(),
                "commit transaction - error: {}",
                e
            );
            e
        });
        self.record_metrics("commit-transaction", start, result)
    }

    async fn rollback(&self) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            transaction_id = self.transaction_id.to_string(),
            "rollback transaction"
        );

        let mut tx_conn = self.tx_connection.0.lock().await;
        let result = DB::TransactionManager::rollback(tx_conn.connection.deref_mut()).await;

        if result.is_ok() {
            tx_conn.open = false;
        }

        let result = result.map_err(|e| {
            let e = Error::query_execution_failure(e);
            error!(
                rdbms_type = self.rdbms_type.to_string(),
                pool_key = self.pool_key.to_string(),
                transaction_id = self.transaction_id.to_string(),
                "rollback transaction - error: {}",
                e
            );
            e
        });
        self.record_metrics("rollback-transaction", start, result)
    }

    async fn rollback_if_open(&self) -> Result<(), Error> {
        let start = Instant::now();
        debug!(
            rdbms_type = self.rdbms_type.to_string(),
            pool_key = self.pool_key.to_string(),
            transaction_id = self.transaction_id.to_string(),
            "rollback transaction if open"
        );

        let mut tx_conn = self.tx_connection.0.lock().await;

        let result = if tx_conn.open {
            let result = DB::TransactionManager::rollback(&mut tx_conn.connection)
                .await
                .map_err(|e| {
                    let e = Error::query_execution_failure(e);
                    error!(
                        rdbms_type = self.rdbms_type.to_string(),
                        pool_key = self.pool_key.to_string(),
                        transaction_id = self.transaction_id.to_string(),
                        "rollback if open transaction - error: {}",
                        e
                    );
                    e
                });
            tx_conn.open = false;
            result
        } else {
            Ok(())
        };

        self.record_metrics("rollback-transaction-if-open", start, result)
            .or(Ok(()))
    }
}

impl<T, DB> Drop for SqlxDbTransaction<T, DB>
where
    T: RdbmsType,
    DB: Database,
{
    fn drop(&mut self) {
        if let Some(mut tx_conn) = self.tx_connection.0.try_lock() {
            if tx_conn.open {
                DB::TransactionManager::start_rollback(&mut tx_conn.connection);
                tx_conn.open = false;
            }
        }
    }
}

#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct SqlxDbResultStream<'q, T: RdbmsType, DB: Database> {
    rdbms_type: T,
    columns: Vec<T::DbColumn>,
    first_rows: Arc<async_mutex::Mutex<Option<Vec<DbRow<T::DbValue>>>>>,
    row_stream: Arc<async_mutex::Mutex<BoxStream<'q, Vec<Result<DB::Row, sqlx::Error>>>>>,
}

impl<'q, T, DB> SqlxDbResultStream<'q, T, DB>
where
    T: RdbmsType + Sync + 'static,
    DB: Database,
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
    ) -> Result<SqlxDbResultStream<'q, T, DB>, Error> {
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

                Ok(SqlxDbResultStream::new(
                    result.columns,
                    result.rows,
                    row_stream,
                ))
            }
            _ => Ok(SqlxDbResultStream::new(vec![], vec![], row_stream)),
        }
    }
}

#[async_trait]
impl<T, DB> DbResultStream<T> for SqlxDbResultStream<'_, T, DB>
where
    T: RdbmsType + Sync,
    DB: Database,
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
                let values: Vec<DbRow<T::DbValue>> = rows
                    .into_iter()
                    .map(|row| {
                        let row = row.map_err(Error::query_response_failure)?;
                        (&row).try_into().map_err(Error::QueryResponseFailure)
                    })
                    .collect::<Result<Vec<_>, Error>>()
                    .map_err(|e| {
                        error!(
                            rdbms_type = self.rdbms_type.to_string(),
                            "get next - error: {}", e
                        );
                        e
                    })?;
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
    T: RdbmsType + Sync,
    DB: Database,
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

#[async_trait]
pub(crate) trait BeginTransactionSupport<T: RdbmsType, DB: Database> {
    async fn begin_transaction(
        key: &RdbmsPoolKey,
        pool: Arc<Pool<DB>>,
        query_config: RdbmsQueryConfig,
    ) -> Result<Arc<SqlxDbTransaction<T, DB>>, Error>;
}

#[async_trait]
impl<T, DB> BeginTransactionSupport<T, DB> for T
where
    T: RdbmsType + Sync + QueryExecutor<T, DB> + GolemTransactionRepo<DB> + 'static,
    DB: Database,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
{
    async fn begin_transaction(
        key: &RdbmsPoolKey,
        pool: Arc<Pool<DB>>,
        query_config: RdbmsQueryConfig,
    ) -> Result<Arc<SqlxDbTransaction<T, DB>>, Error> {
        let mut connection = pool
            .deref()
            .acquire()
            .await
            .map_err(Error::connection_failure)?;

        let id = TransactionId::generate();

        <T as GolemTransactionRepo<DB>>::create_transaction(&id, pool.deref()).await?;

        DB::TransactionManager::begin(&mut connection, None)
            .await
            .map_err(Error::query_execution_failure)?;

        let db_transaction: Arc<SqlxDbTransaction<T, DB>> = Arc::new(SqlxDbTransaction::new(
            id,
            key.clone(),
            connection,
            pool,
            query_config,
        ));

        Ok(db_transaction)
    }
}

#[async_trait]
pub(crate) trait TransactionSupport<T: RdbmsType, DB: Database> {
    async fn pre_commit_transaction(
        pool: &Pool<DB>,
        db_transaction: &SqlxDbTransaction<T, DB>,
    ) -> Result<(), Error>;

    async fn pre_rollback_transaction(
        pool: &Pool<DB>,
        db_transaction: &SqlxDbTransaction<T, DB>,
    ) -> Result<(), Error>;

    async fn get_transaction_status(
        pool: &Pool<DB>,
        id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, Error>;

    async fn cleanup_transaction(pool: &Pool<DB>, id: &TransactionId) -> Result<(), Error>;
}

#[async_trait]
impl<T, DB> TransactionSupport<T, DB> for T
where
    T: RdbmsType + Sync + QueryExecutor<T, DB> + GolemTransactionRepo<DB> + 'static,
    DB: Database,
    for<'c> &'c mut <DB as Database>::Connection: sqlx::Executor<'c, Database = DB>,
{
    async fn pre_commit_transaction(
        _pool: &Pool<DB>,
        db_transaction: &SqlxDbTransaction<T, DB>,
    ) -> Result<(), Error> {
        let id = db_transaction.transaction_id();
        let updated = <T as GolemTransactionRepo<DB>>::update_transaction_status(
            &id,
            RdbmsTransactionStatus::Committed,
            db_transaction.tx_connection.clone(),
        )
        .await?;

        if updated {
            Ok(())
        } else {
            Err(Error::other_response_failure(
                "Pre-commit transaction status not updated, transaction not found",
            ))
        }
    }

    async fn pre_rollback_transaction(
        pool: &Pool<DB>,
        db_transaction: &SqlxDbTransaction<T, DB>,
    ) -> Result<(), Error> {
        let id = db_transaction.transaction_id();
        let updated = <T as GolemTransactionRepo<DB>>::update_transaction_status(
            &id,
            RdbmsTransactionStatus::RolledBack,
            pool,
        )
        .await?;

        if updated {
            Ok(())
        } else {
            Err(Error::other_response_failure(
                "Pre-rollback transaction status not updated, transaction not found",
            ))
        }
    }

    async fn get_transaction_status(
        pool: &Pool<DB>,
        id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, Error> {
        <T as GolemTransactionRepo<DB>>::get_transaction_status(id, pool).await
    }

    async fn cleanup_transaction(pool: &Pool<DB>, id: &TransactionId) -> Result<(), Error> {
        <T as GolemTransactionRepo<DB>>::delete_transaction(id, pool).await?;
        Ok(())
    }
}

#[async_trait]
pub(crate) trait GolemTransactionRepo<DB: Database> {
    async fn create_transaction_table<'c, E>(executor: E) -> Result<(), Error>
    where
        E: sqlx::Executor<'c, Database = DB>;

    async fn create_transaction<'c, E>(id: &TransactionId, executor: E) -> Result<(), Error>
    where
        E: sqlx::Executor<'c, Database = DB>;

    async fn delete_transaction<'c, E>(id: &TransactionId, executor: E) -> Result<bool, Error>
    where
        E: sqlx::Executor<'c, Database = DB>;

    async fn update_transaction_status<'c, E>(
        id: &TransactionId,
        status: RdbmsTransactionStatus,
        executor: E,
    ) -> Result<bool, Error>
    where
        E: sqlx::Executor<'c, Database = DB>;

    async fn get_transaction_status<'c, E>(
        id: &TransactionId,
        executor: E,
    ) -> Result<RdbmsTransactionStatus, Error>
    where
        E: sqlx::Executor<'c, Database = DB>;
}
