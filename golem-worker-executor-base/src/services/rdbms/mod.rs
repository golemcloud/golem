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

pub(crate) mod metrics;
pub mod mysql;
pub mod postgres;
pub(crate) mod sqlx_common;

#[cfg(test)]
mod tests;

use crate::services::golem_config::RdbmsConfig;
use crate::services::rdbms::mysql::MysqlType;
use crate::services::rdbms::postgres::PostgresType;
use async_trait::async_trait;
use golem_common::model::WorkerId;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display};
use std::sync::Arc;
use url::Url;

pub trait RdbmsType {
    type DbColumn: Clone + Send + Sync + PartialEq + Debug;
    type DbValue: Clone + Send + Sync + PartialEq + Debug;
}

#[derive(Clone)]
pub struct RdbmsStatus {
    pools: HashMap<RdbmsPoolKey, HashSet<WorkerId>>,
}

impl Display for RdbmsStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (key, workers) in self.pools.iter() {
            writeln!(f, "{}: {}", key, workers.iter().join(", "))?;
        }

        Ok(())
    }
}

#[async_trait]
pub trait DbTransaction<T: RdbmsType> {
    async fn execute(&self, statement: &str, params: Vec<T::DbValue>) -> Result<u64, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query(&self, statement: &str, params: Vec<T::DbValue>) -> Result<DbResult<T>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn commit(&self) -> Result<(), Error>;

    async fn rollback(&self) -> Result<(), Error>;

    async fn rollback_if_open(&self) -> Result<(), Error>;
}

#[async_trait]
pub trait Rdbms<T: RdbmsType> {
    async fn create(&self, address: &str, worker_id: &WorkerId) -> Result<RdbmsPoolKey, Error>;

    fn exists(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool;

    fn remove(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool;

    async fn execute(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<u64, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query_stream(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<DbResult<T>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn begin_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
    ) -> Result<Arc<dyn DbTransaction<T> + Send + Sync>, Error>;

    fn status(&self) -> RdbmsStatus;
}

pub trait RdbmsService {
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType> + Send + Sync>;
    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType> + Send + Sync>;
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
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType> + Send + Sync> {
        self.mysql.clone()
    }

    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType> + Send + Sync> {
        self.postgres.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RdbmsPoolKey {
    pub address: Url,
}

impl RdbmsPoolKey {
    pub fn new(address: Url) -> Self {
        Self { address }
    }

    pub fn from(address: &str) -> Result<Self, String> {
        Ok(RdbmsPoolKey {
            address: Url::parse(address).map_err(|e| e.to_string())?,
        })
    }

    pub fn masked_address(&self) -> String {
        let mut output: String = self.address.scheme().to_string();
        output.push_str("://");

        let username = self.address.username();
        output.push_str(username);

        let password = self.address.password();
        if password.is_some() {
            output.push_str(":*****");
        }

        if let Some(h) = self.address.host_str() {
            if !username.is_empty() || password.is_some() {
                output.push('@');
            }

            output.push_str(h);

            if let Some(p) = self.address.port() {
                output.push(':');
                output.push_str(p.to_string().as_str());
            }
        }

        output.push_str(self.address.path());

        let query_pairs = self.address.query_pairs();

        if query_pairs.count() > 0 {
            output.push('?');
        }
        for (index, (key, value)) in query_pairs.enumerate() {
            let key = &*key;
            output.push_str(key);
            output.push('=');

            if key == "password" || key == "secret" {
                output.push_str("*****");
            } else {
                output.push_str(&value);
            }
            if index < query_pairs.count() - 1 {
                output.push('&');
            }
        }

        output
    }
}

impl Display for RdbmsPoolKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.masked_address())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbRow<V> {
    pub values: Vec<V>,
}

#[async_trait]
pub trait DbResultStream<T: RdbmsType> {
    async fn get_columns(&self) -> Result<Vec<T::DbColumn>, Error>;

    async fn get_next(&self) -> Result<Option<Vec<DbRow<T::DbValue>>>, Error>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbResult<T: RdbmsType> {
    pub columns: Vec<T::DbColumn>,
    pub rows: Vec<DbRow<T::DbValue>>,
}

impl<T: RdbmsType> DbResult<T> {
    pub fn new(columns: Vec<T::DbColumn>, rows: Vec<DbRow<T::DbValue>>) -> Self {
        Self { columns, rows }
    }

    pub fn empty() -> Self {
        Self {
            columns: vec![],
            rows: vec![],
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn from(
        result_set: Arc<dyn DbResultStream<T> + Send + Sync>,
    ) -> Result<DbResult<T>, Error> {
        let columns = result_set.get_columns().await?;
        let mut rows: Vec<DbRow<T::DbValue>> = vec![];

        while let Some(vs) = result_set.get_next().await? {
            rows.extend(vs);
        }
        Ok(DbResult::new(columns, rows))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    ConnectionFailure(String),
    QueryParameterFailure(String),
    QueryExecutionFailure(String),
    QueryResponseFailure(String),
    Other(String),
}

impl Error {
    pub(crate) fn connection_failure<E: Display>(error: E) -> Error {
        Self::ConnectionFailure(error.to_string())
    }

    pub(crate) fn query_execution_failure<E: Display>(error: E) -> Error {
        Self::QueryExecutionFailure(error.to_string())
    }

    pub(crate) fn query_response_failure<E: Display>(error: E) -> Error {
        Self::QueryResponseFailure(error.to_string())
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ConnectionFailure(msg) => write!(f, "ConnectionFailure: {}", msg),
            Error::QueryParameterFailure(msg) => write!(f, "QueryParameterFailure: {}", msg),
            Error::QueryExecutionFailure(msg) => write!(f, "QueryExecutionFailure: {}", msg),
            Error::QueryResponseFailure(msg) => write!(f, "QueryResponseFailure: {}", msg),
            Error::Other(msg) => write!(f, "Other: {}", msg),
        }
    }
}
