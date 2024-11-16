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
pub mod types;

#[cfg(test)]
mod tests;

use crate::services::rdbms::mysql::MysqlType;
use crate::services::rdbms::postgres::PostgresType;
use crate::services::rdbms::types::{DbResultSet, DbValue, Error};
use async_trait::async_trait;
use golem_common::model::WorkerId;
use itertools::Itertools;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::Arc;

lazy_static! {
    static ref MASK_ADDRESS_REGEX: Regex = Regex::new(r"(?i)([a-z]+)://([^:]+):([^@]+)@")
        .expect("Failed to compile mask address regex");
}

pub trait RdbmsType {}

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
pub trait Rdbms<T: RdbmsType> {
    async fn create(&self, worker_id: &WorkerId, address: &str) -> Result<RdbmsPoolKey, Error>;

    fn exists(&self, worker_id: &WorkerId, key: &RdbmsPoolKey) -> bool;

    fn remove(&self, worker_id: &WorkerId, key: &RdbmsPoolKey) -> bool;

    async fn execute(
        &self,
        worker_id: &WorkerId,
        key: &RdbmsPoolKey,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<u64, Error>;

    async fn query(
        &self,
        worker_id: &WorkerId,
        key: &RdbmsPoolKey,
        statement: &str,
        params: Vec<DbValue>,
    ) -> Result<Arc<dyn DbResultSet + Send + Sync>, Error>;

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
    pub fn new(
        mysql: Arc<dyn Rdbms<MysqlType> + Send + Sync>,
        postgres: Arc<dyn Rdbms<PostgresType> + Send + Sync>,
    ) -> Self {
        Self { mysql, postgres }
    }
}

impl Default for RdbmsServiceDefault {
    fn default() -> Self {
        let config = RdbmsConfig::default();
        Self::new(
            MysqlType::new_rdbms(config),
            PostgresType::new_rdbms(config),
        )
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

#[derive(Clone, Copy, Debug, Default)]
pub struct RdbmsConfig {
    pub pool: RdbmsPoolConfig,
    pub query: RdbmsQueryConfig,
}

#[derive(Clone, Copy, Debug)]
pub struct RdbmsQueryConfig {
    pub query_batch: usize,
}

impl Default for RdbmsQueryConfig {
    fn default() -> Self {
        Self { query_batch: 50 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RdbmsPoolConfig {
    pub max_connections: u32,
    pub eviction_ttl: std::time::Duration,
    pub eviction_period: std::time::Duration,
}

impl Default for RdbmsPoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
            eviction_ttl: std::time::Duration::from_secs(10 * 60),
            eviction_period: std::time::Duration::from_secs(2 * 60),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RdbmsPoolKey {
    pub address: String,
}

impl RdbmsPoolKey {
    pub fn new(address: String) -> Self {
        Self { address }
    }

    pub fn masked_address(&self) -> String {
        MASK_ADDRESS_REGEX
            .replace_all(self.address.as_str(), "$1://$2:*****@")
            .to_string()
    }
}

impl Display for RdbmsPoolKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.masked_address())
    }
}
