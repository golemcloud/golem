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

use crate::services::golem_config::RdbmsConfig;
use crate::services::rdbms::ignite::types::{DbColumn, DbValue};
use crate::services::rdbms::ignite::{IGNITE, IgniteType};
use crate::services::rdbms::{
    DbResult, DbResultStream, DbRow, DbTransaction, Rdbms, RdbmsError, RdbmsStatus,
    RdbmsTransactionStatus,
};
use async_trait::async_trait;
use dashmap::DashMap;
use futures::StreamExt;
use golem_common::model::{AgentId, RdbmsPoolKey, TransactionId};
use ignite_client::{
    IgniteClient, IgniteClientConfig, IgniteError, IgniteValue, QueryStream as IgniteQueryStream,
    Transaction as IgniteTransaction,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use url::Url;

const PAGE_SIZE: usize = 100;

// ── Error conversion ──────────────────────────────────────────────────────────

impl From<IgniteError> for RdbmsError {
    fn from(value: IgniteError) -> Self {
        match value {
            IgniteError::Transport(e) => RdbmsError::ConnectionFailure(e.to_string()),
            IgniteError::Protocol(e) => RdbmsError::QueryExecutionFailure(e.to_string()),
            IgniteError::Pool(msg) => RdbmsError::ConnectionFailure(msg),
            IgniteError::TransactionFinished => {
                RdbmsError::Other("transaction already finalised".to_string())
            }
            IgniteError::CursorClosed => {
                RdbmsError::QueryResponseFailure("cursor already closed".to_string())
            }
            IgniteError::NoRows => {
                RdbmsError::QueryResponseFailure("query returned no rows".to_string())
            }
        }
    }
}

// ── Value conversions ─────────────────────────────────────────────────────────

fn ignite_to_service(v: IgniteValue) -> DbValue {
    match v {
        IgniteValue::Null => DbValue::Null,
        IgniteValue::Bool(v) => DbValue::Boolean(v),
        IgniteValue::Byte(v) => DbValue::Byte(v),
        IgniteValue::Short(v) => DbValue::Short(v),
        IgniteValue::Int(v) => DbValue::Int(v),
        IgniteValue::Long(v) => DbValue::Long(v),
        IgniteValue::Float(v) => DbValue::Float(v),
        IgniteValue::Double(v) => DbValue::Double(v),
        IgniteValue::Char(v) => DbValue::Char(v),
        IgniteValue::String(v) => DbValue::String(v),
        IgniteValue::Uuid(v) => DbValue::Uuid(v),
        IgniteValue::Date(ms) => DbValue::Date(ms),
        IgniteValue::Timestamp(ms, ns) => DbValue::Timestamp(ms, ns),
        IgniteValue::Time(ms) => DbValue::Time(ms),
        IgniteValue::Decimal(v) => DbValue::Decimal(v),
        IgniteValue::ByteArray(v) => DbValue::ByteArray(v),
        IgniteValue::RawObject(v) => DbValue::ByteArray(v),
    }
}

fn service_to_ignite(v: DbValue) -> IgniteValue {
    match v {
        DbValue::Null => IgniteValue::Null,
        DbValue::Boolean(v) => IgniteValue::Bool(v),
        DbValue::Byte(v) => IgniteValue::Byte(v),
        DbValue::Short(v) => IgniteValue::Short(v),
        DbValue::Int(v) => IgniteValue::Int(v),
        DbValue::Long(v) => IgniteValue::Long(v),
        DbValue::Float(v) => IgniteValue::Float(v),
        DbValue::Double(v) => IgniteValue::Double(v),
        DbValue::Char(v) => IgniteValue::Char(v),
        DbValue::String(v) => IgniteValue::String(v),
        DbValue::Uuid(v) => IgniteValue::Uuid(v),
        DbValue::Date(ms) => IgniteValue::Date(ms),
        DbValue::Timestamp(ms, ns) => IgniteValue::Timestamp(ms, ns),
        DbValue::Time(ms) => IgniteValue::Time(ms),
        DbValue::Decimal(v) => IgniteValue::Decimal(v),
        DbValue::ByteArray(v) => IgniteValue::ByteArray(v),
    }
}

// ── Address parsing ───────────────────────────────────────────────────────────

fn parse_config(address: &Url) -> Result<IgniteClientConfig, RdbmsError> {
    if address.scheme() != IGNITE {
        return Err(RdbmsError::ConnectionFailure(format!(
            "scheme '{}' in url is invalid, expected 'ignite'",
            address.scheme()
        )));
    }
    let host = address
        .host_str()
        .ok_or_else(|| RdbmsError::ConnectionFailure("missing host in address".to_string()))?;
    let port = address.port().unwrap_or(10800);
    let ignite_address = format!("{host}:{port}");

    let username = if address.username().is_empty() {
        None
    } else {
        Some(address.username().to_string())
    };
    let password = address.password().map(|p| p.to_string());

    let mut max_pool_size: usize = 10;
    let mut use_tls = false;
    let mut page_size: usize = 1024;
    for (key, value) in address.query_pairs() {
        match key.as_ref() {
            "pool_size" => max_pool_size = value.parse().unwrap_or(10),
            "tls" => use_tls = value == "true",
            "page_size" => page_size = value.parse().unwrap_or(1024),
            _ => {}
        }
    }

    Ok(IgniteClientConfig {
        address: ignite_address,
        username,
        password,
        max_pool_size,
        connect_timeout: Duration::from_secs(10),
        request_timeout: Duration::from_secs(30),
        page_size,
        use_tls,
        tls_accept_invalid_certs: false,
    })
}

// ── QueryResult → DbResult conversion ────────────────────────────────────────

fn convert_query_result(result: ignite_client::QueryResult) -> DbResult<IgniteType> {
    let columns: Vec<DbColumn> = result
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| DbColumn::new(i, c.name.clone()))
        .collect();

    let rows: Vec<DbRow<DbValue>> = result
        .rows
        .into_iter()
        .map(|row| DbRow {
            values: row
                .values()
                .iter()
                .cloned()
                .map(ignite_to_service)
                .collect(),
        })
        .collect();

    DbResult::new(columns, rows)
}

// ── IgniteResultStream ────────────────────────────────────────────────────────

struct IgniteResultStream {
    columns: Vec<DbColumn>,
    inner: Mutex<IgniteQueryStream>,
    done: AtomicBool,
}

impl IgniteResultStream {
    fn new(columns: Vec<DbColumn>, stream: IgniteQueryStream) -> Self {
        Self {
            columns,
            inner: Mutex::new(stream),
            done: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl DbResultStream<IgniteType> for IgniteResultStream {
    async fn get_columns(&self) -> Result<Vec<DbColumn>, RdbmsError> {
        Ok(self.columns.clone())
    }

    async fn get_next(&self) -> Result<Option<Vec<DbRow<DbValue>>>, RdbmsError> {
        if self.done.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let mut stream = self.inner.lock().await;
        let mut batch: Vec<DbRow<DbValue>> = Vec::with_capacity(PAGE_SIZE);
        for _ in 0..PAGE_SIZE {
            match stream.next().await {
                Some(Ok(row)) => batch.push(DbRow {
                    values: row
                        .values()
                        .iter()
                        .cloned()
                        .map(ignite_to_service)
                        .collect(),
                }),
                Some(Err(e)) => return Err(RdbmsError::from(e)),
                None => {
                    self.done.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
        if batch.is_empty() {
            Ok(None)
        } else {
            Ok(Some(batch))
        }
    }
}

// ── IgniteDbTransaction ───────────────────────────────────────────────────────

struct IgniteDbTransaction {
    transaction_id: TransactionId,
    inner: Mutex<Option<IgniteTransaction>>,
    /// Shared with IgniteRdbms so get_transaction_status() sees updates.
    tx_statuses: Arc<DashMap<TransactionId, RdbmsTransactionStatus>>,
}

impl IgniteDbTransaction {
    fn new(
        tx: IgniteTransaction,
        tx_statuses: Arc<DashMap<TransactionId, RdbmsTransactionStatus>>,
    ) -> (Self, TransactionId) {
        let id = TransactionId::generate();
        tx_statuses.insert(id.clone(), RdbmsTransactionStatus::InProgress);
        let txn = Self {
            transaction_id: id.clone(),
            inner: Mutex::new(Some(tx)),
            tx_statuses,
        };
        (txn, id)
    }
}

#[async_trait]
impl DbTransaction<IgniteType> for IgniteDbTransaction {
    fn transaction_id(&self) -> TransactionId {
        self.transaction_id.clone()
    }

    async fn execute(&self, statement: &str, params: Vec<DbValue>) -> Result<u64, RdbmsError>
    where
        DbValue: 'async_trait,
    {
        let ignite_params: Vec<IgniteValue> = params.into_iter().map(service_to_ignite).collect();
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| RdbmsError::Other("transaction already finalised".to_string()))?;
        let result = tx
            .execute(statement, ignite_params)
            .await
            .map_err(RdbmsError::from)?;
        let rows = result.rows_affected;
        Ok(if rows < 0 { 0u64 } else { rows as u64 })
    }

    async fn query(
        &self,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<DbResult<IgniteType>, RdbmsError>
    where
        DbValue: 'async_trait,
    {
        let ignite_params: Vec<IgniteValue> = params.into_iter().map(service_to_ignite).collect();
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| RdbmsError::Other("transaction already finalised".to_string()))?;
        let result = tx
            .query(statement, ignite_params)
            .await
            .map_err(RdbmsError::from)?;
        Ok(convert_query_result(result))
    }

    async fn query_stream(
        &self,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<Arc<dyn DbResultStream<IgniteType> + Send + Sync>, RdbmsError>
    where
        DbValue: 'async_trait,
    {
        let ignite_params: Vec<IgniteValue> = params.into_iter().map(service_to_ignite).collect();
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| RdbmsError::Other("transaction already finalised".to_string()))?;
        let qs = tx
            .query_stream(statement, ignite_params)
            .await
            .map_err(RdbmsError::from)?;
        let columns: Vec<DbColumn> = qs
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| DbColumn::new(i, c.name.clone()))
            .collect();
        Ok(Arc::new(IgniteResultStream::new(columns, qs)))
    }

    async fn pre_commit(&self) -> Result<(), RdbmsError> {
        Ok(())
    }

    async fn pre_rollback(&self) -> Result<(), RdbmsError> {
        Ok(())
    }

    async fn commit(&self) -> Result<(), RdbmsError> {
        let tx = self
            .inner
            .lock()
            .await
            .take()
            .ok_or_else(|| RdbmsError::Other("transaction already finalised".to_string()))?;
        tx.commit().await.map_err(RdbmsError::from)?;
        self.tx_statuses.insert(
            self.transaction_id.clone(),
            RdbmsTransactionStatus::Committed,
        );
        Ok(())
    }

    async fn rollback(&self) -> Result<(), RdbmsError> {
        let tx = self
            .inner
            .lock()
            .await
            .take()
            .ok_or_else(|| RdbmsError::Other("transaction already finalised".to_string()))?;
        tx.rollback().await.map_err(RdbmsError::from)?;
        self.tx_statuses.insert(
            self.transaction_id.clone(),
            RdbmsTransactionStatus::RolledBack,
        );
        Ok(())
    }

    async fn rollback_if_open(&self) -> Result<(), RdbmsError> {
        let tx = self.inner.lock().await.take();
        if let Some(tx) = tx {
            tx.rollback().await.map_err(RdbmsError::from)?;
            self.tx_statuses.insert(
                self.transaction_id.clone(),
                RdbmsTransactionStatus::RolledBack,
            );
        }
        Ok(())
    }
}

// ── IgniteRdbms ───────────────────────────────────────────────────────────────

type ClientPool = Arc<DashMap<RdbmsPoolKey, (Arc<IgniteClient>, HashSet<AgentId>)>>;

#[derive(Clone)]
pub(crate) struct IgniteRdbms {
    _config: RdbmsConfig,
    /// Pool key → (IgniteClient, set of worker IDs).
    clients: ClientPool,
    /// Transaction ID → status (for replay support).
    tx_statuses: Arc<DashMap<TransactionId, RdbmsTransactionStatus>>,
}

impl IgniteRdbms {
    pub(crate) fn new(config: RdbmsConfig) -> Self {
        Self {
            _config: config,
            clients: Arc::new(DashMap::new()),
            tx_statuses: Arc::new(DashMap::new()),
        }
    }

    fn get_client(&self, key: &RdbmsPoolKey) -> Option<Arc<IgniteClient>> {
        self.clients.get(key).map(|r| r.0.clone())
    }

    async fn get_or_create_client(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &AgentId,
    ) -> Result<Arc<IgniteClient>, RdbmsError> {
        if let Some(client) = self.get_client(key) {
            return Ok(client);
        }
        self.create(key.address.as_str(), worker_id).await?;
        self.get_client(key)
            .ok_or_else(|| RdbmsError::ConnectionFailure(format!("no client for {}", key)))
    }
}

#[async_trait]
impl Rdbms<IgniteType> for IgniteRdbms {
    async fn create(&self, address: &str, worker_id: &AgentId) -> Result<RdbmsPoolKey, RdbmsError> {
        let key = RdbmsPoolKey::from(address).map_err(RdbmsError::ConnectionFailure)?;
        if !self.clients.contains_key(&key) {
            let cfg = parse_config(&key.address)?;
            info!(pool_key = key.to_string(), "ignite: creating client");
            let client = Arc::new(IgniteClient::new(cfg));
            self.clients
                .entry(key.clone())
                .or_insert_with(|| (client, HashSet::new()))
                .1
                .insert(worker_id.clone());
        } else {
            self.clients
                .get_mut(&key)
                .unwrap()
                .1
                .insert(worker_id.clone());
        }
        Ok(key)
    }

    async fn exists(&self, key: &RdbmsPoolKey, worker_id: &AgentId) -> bool {
        self.clients
            .get(key)
            .map(|r| r.1.contains(worker_id))
            .unwrap_or(false)
    }

    async fn remove(&self, key: &RdbmsPoolKey, worker_id: &AgentId) -> bool {
        if let Some(mut entry) = self.clients.get_mut(key) {
            return entry.1.remove(worker_id);
        }
        false
    }

    async fn execute(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &AgentId,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<u64, RdbmsError>
    where
        DbValue: 'async_trait,
    {
        debug!(
            pool_key = key.to_string(),
            "ignite execute: {statement}, params: {}",
            params.len()
        );
        let client = self.get_or_create_client(key, worker_id).await?;
        let ignite_params: Vec<IgniteValue> = params.into_iter().map(service_to_ignite).collect();
        let result = client
            .execute(statement, ignite_params)
            .await
            .map_err(|e| {
                error!(pool_key = key.to_string(), "ignite execute error: {e}");
                RdbmsError::from(e)
            })?;
        let rows = result.rows_affected;
        Ok(if rows < 0 { 0u64 } else { rows as u64 })
    }

    async fn query_stream(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &AgentId,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<Arc<dyn DbResultStream<IgniteType> + Send + Sync>, RdbmsError>
    where
        DbValue: 'async_trait,
    {
        debug!(
            pool_key = key.to_string(),
            "ignite query_stream: {statement}, params: {}",
            params.len()
        );
        let client = self.get_or_create_client(key, worker_id).await?;
        let ignite_params: Vec<IgniteValue> = params.into_iter().map(service_to_ignite).collect();
        let qs = client
            .query_stream(statement, ignite_params)
            .await
            .map_err(|e| {
                error!(pool_key = key.to_string(), "ignite query_stream error: {e}");
                RdbmsError::from(e)
            })?;
        let columns: Vec<DbColumn> = qs
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| DbColumn::new(i, c.name.clone()))
            .collect();
        Ok(Arc::new(IgniteResultStream::new(columns, qs)))
    }

    async fn query(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &AgentId,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<DbResult<IgniteType>, RdbmsError>
    where
        DbValue: 'async_trait,
    {
        debug!(
            pool_key = key.to_string(),
            "ignite query: {statement}, params: {}",
            params.len()
        );
        let client = self.get_or_create_client(key, worker_id).await?;
        let ignite_params: Vec<IgniteValue> = params.into_iter().map(service_to_ignite).collect();
        let result = client.query(statement, ignite_params).await.map_err(|e| {
            error!(pool_key = key.to_string(), "ignite query error: {e}");
            RdbmsError::from(e)
        })?;
        Ok(convert_query_result(result))
    }

    async fn begin_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &AgentId,
    ) -> Result<Arc<dyn DbTransaction<IgniteType> + Send + Sync>, RdbmsError> {
        debug!(pool_key = key.to_string(), "ignite begin_transaction");
        let client = self.get_or_create_client(key, worker_id).await?;
        let tx = client.begin_transaction().await.map_err(|e| {
            error!(
                pool_key = key.to_string(),
                "ignite begin_transaction error: {e}"
            );
            RdbmsError::from(e)
        })?;
        let (txn, _id) = IgniteDbTransaction::new(tx, self.tx_statuses.clone());
        Ok(Arc::new(txn))
    }

    async fn get_transaction_status(
        &self,
        _key: &RdbmsPoolKey,
        _worker_id: &AgentId,
        transaction_id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, RdbmsError> {
        Ok(self
            .tx_statuses
            .get(transaction_id)
            .map(|r| r.clone())
            .unwrap_or(RdbmsTransactionStatus::NotFound))
    }

    async fn cleanup_transaction(
        &self,
        _key: &RdbmsPoolKey,
        _worker_id: &AgentId,
        transaction_id: &TransactionId,
    ) -> Result<(), RdbmsError> {
        self.tx_statuses.remove(transaction_id);
        Ok(())
    }

    async fn status(&self) -> RdbmsStatus {
        let mut pools = HashMap::new();
        for entry in self.clients.iter() {
            pools.insert(entry.key().clone(), entry.1.clone());
        }
        RdbmsStatus { pools }
    }
}
