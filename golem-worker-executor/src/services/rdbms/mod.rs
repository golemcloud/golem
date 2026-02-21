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

pub(crate) mod metrics;
pub mod mysql;
pub mod postgres;
pub(crate) mod sqlx_common;

use crate::services::golem_config::RdbmsConfig;
use crate::services::rdbms::mysql::MysqlType;
use crate::services::rdbms::postgres::PostgresType;
use async_trait::async_trait;
use golem_common::model::oplog::types::{
    SerializableDbColumn, SerializableDbResult, SerializableDbValue, SerializableRdbmsError,
};
use golem_common::model::WorkerId;
use golem_common::model::{RdbmsPoolKey, TransactionId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;

pub trait RdbmsType: Debug + Display + Default + PartialEq + Clone + Send {
    type DbColumn: Clone
        + Send
        + Sync
        + PartialEq
        + Debug
        + Into<SerializableDbColumn>
        + TryFrom<SerializableDbColumn, Error = String>
        + 'static;
    type DbValue: Clone
        + Send
        + Sync
        + PartialEq
        + Debug
        + Into<SerializableDbValue>
        + TryFrom<SerializableDbValue, Error = String>
        + 'static;

    fn durability_connection_interface() -> &'static str;
    fn durability_transaction_interface() -> &'static str;
    fn durability_result_stream_interface() -> &'static str;
}

#[derive(Clone)]
pub struct RdbmsStatus {
    pub pools: HashMap<RdbmsPoolKey, HashSet<WorkerId>>,
}

impl Display for RdbmsStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for (key, workers) in self.pools.iter() {
            writeln!(f, "{}: {}", key, workers.iter().join(", "))?;
        }

        Ok(())
    }
}

#[async_trait]
pub trait DbTransaction<T: RdbmsType> {
    fn transaction_id(&self) -> TransactionId;

    async fn execute(&self, statement: &str, params: Vec<T::DbValue>) -> Result<u64, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query(
        &self,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<DbResult<T>, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query_stream(
        &self,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn pre_commit(&self) -> Result<(), RdbmsError>;

    async fn pre_rollback(&self) -> Result<(), RdbmsError>;

    async fn commit(&self) -> Result<(), RdbmsError>;

    async fn rollback(&self) -> Result<(), RdbmsError>;

    async fn rollback_if_open(&self) -> Result<(), RdbmsError>;
}

#[async_trait]
pub trait Rdbms<T: RdbmsType>: Send + Sync {
    async fn create(&self, address: &str, worker_id: &WorkerId)
        -> Result<RdbmsPoolKey, RdbmsError>;

    async fn exists(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool;

    async fn remove(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool;

    async fn execute(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<u64, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query_stream(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<DbResult<T>, RdbmsError>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn begin_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
    ) -> Result<Arc<dyn DbTransaction<T> + Send + Sync>, RdbmsError>;

    async fn get_transaction_status(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        transaction_id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, RdbmsError>;

    async fn cleanup_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        transaction_id: &TransactionId,
    ) -> Result<(), RdbmsError>;

    async fn status(&self) -> RdbmsStatus;
}

pub trait RdbmsService: Send + Sync {
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType>>;
    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType>>;
}

pub trait RdbmsTypeService<T: RdbmsType> {
    fn rdbms_type_service(&self) -> Arc<dyn Rdbms<T>>;
}

impl RdbmsTypeService<MysqlType> for dyn RdbmsService {
    fn rdbms_type_service(&self) -> Arc<dyn Rdbms<MysqlType>> {
        self.mysql()
    }
}

impl RdbmsTypeService<PostgresType> for dyn RdbmsService {
    fn rdbms_type_service(&self) -> Arc<dyn Rdbms<PostgresType>> {
        self.postgres()
    }
}

#[derive(Clone)]
pub struct RdbmsServiceDefault {
    mysql: Arc<dyn Rdbms<MysqlType> + Send + Sync>,
    postgres: Arc<dyn Rdbms<PostgresType> + Send + Sync>,
}

impl RdbmsServiceDefault {
    pub fn new(config: RdbmsConfig) -> Self {
        Self {
            mysql: MysqlType::new_rdbms(config),
            postgres: PostgresType::new_rdbms(config),
        }
    }
}

impl Default for RdbmsServiceDefault {
    fn default() -> Self {
        Self::new(RdbmsConfig::default())
    }
}

impl RdbmsService for RdbmsServiceDefault {
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType>> {
        self.mysql.clone()
    }

    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType>> {
        self.postgres.clone()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbRow<T: 'static> {
    pub values: Vec<T>,
}

#[async_trait]
pub trait DbResultStream<T: RdbmsType> {
    async fn get_columns(&self) -> Result<Vec<T::DbColumn>, RdbmsError>;

    async fn get_next(&self) -> Result<Option<Vec<DbRow<T::DbValue>>>, RdbmsError>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbResult<T: RdbmsType + 'static> {
    pub columns: Vec<T::DbColumn>,
    pub rows: Vec<DbRow<T::DbValue>>,
}

impl<T: RdbmsType> DbResult<T> {
    pub fn new(columns: Vec<T::DbColumn>, rows: Vec<DbRow<T::DbValue>>) -> Self {
        Self { columns, rows }
    }

    pub fn empty() -> Self {
        Self::new(vec![], vec![])
    }

    #[allow(dead_code)]
    pub async fn from(
        result_set: Arc<dyn DbResultStream<T> + Send + Sync>,
    ) -> Result<DbResult<T>, RdbmsError> {
        let columns = result_set.get_columns().await?;
        let mut rows: Vec<DbRow<T::DbValue>> = vec![];

        while let Some(vs) = result_set.get_next().await? {
            rows.extend(vs);
        }
        Ok(DbResult::new(columns, rows))
    }
}

impl<T: RdbmsType> From<DbResult<T>> for SerializableDbResult {
    fn from(value: DbResult<T>) -> Self {
        let columns = value.columns.into_iter().map(|c| c.into()).collect();
        let rows = value
            .rows
            .into_iter()
            .map(|r| r.values.into_iter().map(|v| v.into()).collect())
            .collect();
        SerializableDbResult { columns, rows }
    }
}

impl<T: RdbmsType> TryFrom<SerializableDbResult> for DbResult<T> {
    type Error = String;

    fn try_from(value: SerializableDbResult) -> Result<Self, Self::Error> {
        let columns = value
            .columns
            .into_iter()
            .map(|c| c.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        let rows = value
            .rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|v| v.try_into())
                    .collect::<Result<Vec<_>, _>>()
                    .map(|values| DbRow { values })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(DbResult { columns, rows })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RdbmsError {
    ConnectionFailure(String),
    QueryParameterFailure(String),
    QueryExecutionFailure(String),
    QueryResponseFailure(String),
    Other(String),
}

impl RdbmsError {
    pub fn connection_failure<E: Display>(error: E) -> RdbmsError {
        Self::ConnectionFailure(error.to_string())
    }

    pub fn query_execution_failure<E: Display>(error: E) -> RdbmsError {
        Self::QueryExecutionFailure(error.to_string())
    }

    pub fn query_response_failure<E: Display>(error: E) -> RdbmsError {
        Self::QueryResponseFailure(error.to_string())
    }

    pub fn other_response_failure<E: Display>(error: E) -> RdbmsError {
        Self::Other(error.to_string())
    }
}

impl Display for RdbmsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RdbmsError::ConnectionFailure(msg) => write!(f, "ConnectionFailure: {msg}"),
            RdbmsError::QueryParameterFailure(msg) => write!(f, "QueryParameterFailure: {msg}"),
            RdbmsError::QueryExecutionFailure(msg) => write!(f, "QueryExecutionFailure: {msg}"),
            RdbmsError::QueryResponseFailure(msg) => write!(f, "QueryResponseFailure: {msg}"),
            RdbmsError::Other(msg) => write!(f, "Other: {msg}"),
        }
    }
}

impl From<RdbmsError> for SerializableRdbmsError {
    fn from(value: RdbmsError) -> SerializableRdbmsError {
        match value {
            RdbmsError::ConnectionFailure(msg) => SerializableRdbmsError::ConnectionFailure(msg),
            RdbmsError::QueryParameterFailure(msg) => {
                SerializableRdbmsError::QueryParameterFailure(msg)
            }
            RdbmsError::QueryExecutionFailure(msg) => {
                SerializableRdbmsError::QueryExecutionFailure(msg)
            }
            RdbmsError::QueryResponseFailure(msg) => {
                SerializableRdbmsError::QueryResponseFailure(msg)
            }
            RdbmsError::Other(msg) => SerializableRdbmsError::Other(msg),
        }
    }
}

impl From<SerializableRdbmsError> for RdbmsError {
    fn from(value: SerializableRdbmsError) -> RdbmsError {
        match value {
            SerializableRdbmsError::ConnectionFailure(msg) => RdbmsError::ConnectionFailure(msg),
            SerializableRdbmsError::QueryParameterFailure(msg) => {
                RdbmsError::QueryParameterFailure(msg)
            }
            SerializableRdbmsError::QueryExecutionFailure(msg) => {
                RdbmsError::QueryExecutionFailure(msg)
            }
            SerializableRdbmsError::QueryResponseFailure(msg) => {
                RdbmsError::QueryResponseFailure(msg)
            }
            SerializableRdbmsError::Other(msg) => RdbmsError::Other(msg),
        }
    }
}

impl From<WorkerExecutorError> for RdbmsError {
    fn from(value: WorkerExecutorError) -> Self {
        Self::other_response_failure(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RdbmsTransactionStatus {
    InProgress,
    Committed,
    RolledBack,
    NotFound,
}

impl Display for RdbmsTransactionStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RdbmsTransactionStatus::InProgress => write!(f, "InProgress"),
            RdbmsTransactionStatus::Committed => write!(f, "Committed"),
            RdbmsTransactionStatus::RolledBack => write!(f, "RolledBack"),
            RdbmsTransactionStatus::NotFound => write!(f, "NotFound"),
        }
    }
}

impl FromStr for RdbmsTransactionStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "InProgress" => Ok(RdbmsTransactionStatus::InProgress),
            "Committed" => Ok(RdbmsTransactionStatus::Committed),
            "RolledBack" => Ok(RdbmsTransactionStatus::RolledBack),
            "NotFound" => Ok(RdbmsTransactionStatus::NotFound),
            _ => Err(format!("Unknown transaction status: {s}")),
        }
    }
}
