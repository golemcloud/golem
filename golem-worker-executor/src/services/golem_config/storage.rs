// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use golem_common::SafeDisplay;
use golem_common::config::{DbPostgresConfig, DbSqliteConfig, RedisConfig};
use golem_common::model::RetryConfig;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn default_scheduler_storage_retry() -> RetryConfig {
    RetryConfig::max_attempts_3()
}

pub(super) fn default_indexed_storage_retry() -> RetryConfig {
    RetryConfig::max_attempts_3()
}

pub fn default_key_value_storage_retry() -> RetryConfig {
    // Sized to outlast an AWS-side switchover rather than a momentary blip, because those are the
    // outages this policy exists to hide. Aurora promotes a reader to writer in high single-digit
    // to mid-tens of seconds, worst case around a minute; ElastiCache Multi-AZ promotes a replica
    // in around thirty. Roughly 93 seconds of backoff across these attempts covers both with
    // margin, and the delay is capped so the tail stays responsive once the new writer answers.
    //
    // The other half of this budget is `DbPostgresConfig::acquire_timeout`: an attempt against an
    // endpoint that blackholes packets, rather than one that refuses fast, costs that timeout
    // before the backoff below even starts.
    RetryConfig {
        max_attempts: 15,
        min_delay: Duration::from_millis(200),
        max_delay: Duration::from_secs(10),
        multiplier: 2.0,
        max_jitter_factor: Some(0.15),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum KeyValueStorageConfig {
    Redis(RedisConfig),
    Postgres(KeyValueStoragePostgresConfig),
    NamespaceRouted(KeyValueStorageNamespaceRoutedConfig),
    Sqlite(DbSqliteConfig),
    MultiSqlite(KeyValueStorageMultiSqliteConfig),
    InMemory(KeyValueStorageInMemoryConfig),
}

impl SafeDisplay for KeyValueStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            KeyValueStorageConfig::Redis(inner) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageConfig::Postgres(inner) => {
                let _ = writeln!(&mut result, "postgres:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageConfig::NamespaceRouted(inner) => {
                let _ = writeln!(&mut result, "namespace-routed:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageConfig::Sqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageConfig::MultiSqlite(inner) => {
                let _ = writeln!(&mut result, "multi-sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageConfig::InMemory(inner) => {
                let _ = writeln!(&mut result, "in-memory:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyValueStoragePostgresConfig {
    #[serde(flatten)]
    pub postgres: DbPostgresConfig,
}

impl SafeDisplay for KeyValueStoragePostgresConfig {
    fn to_safe_string(&self) -> String {
        self.postgres.to_safe_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyValueStorageNamespaceRoutedConfig {
    pub cache: KeyValueStorageInnerConfig,
    pub persistent: KeyValueStorageInnerConfig,
}

impl SafeDisplay for KeyValueStorageNamespaceRoutedConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "cache:");
        let _ = writeln!(&mut result, "{}", self.cache.to_safe_string_indented());
        let _ = writeln!(&mut result, "persistent:");
        let _ = writeln!(&mut result, "{}", self.persistent.to_safe_string_indented());
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum KeyValueStorageInnerConfig {
    Redis(RedisConfig),
    Postgres(KeyValueStoragePostgresConfig),
    Sqlite(DbSqliteConfig),
    MultiSqlite(KeyValueStorageMultiSqliteConfig),
    InMemory(KeyValueStorageInMemoryConfig),
}

impl SafeDisplay for KeyValueStorageInnerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            KeyValueStorageInnerConfig::Redis(inner) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageInnerConfig::Postgres(inner) => {
                let _ = writeln!(&mut result, "postgres:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageInnerConfig::Sqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageInnerConfig::MultiSqlite(inner) => {
                let _ = writeln!(&mut result, "multi-sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageInnerConfig::InMemory(inner) => {
                let _ = writeln!(&mut result, "in-memory:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyValueStorageMultiSqliteConfig {
    pub root_dir: PathBuf,
    pub max_connections: u32,
    pub foreign_keys: bool,
}

impl SafeDisplay for KeyValueStorageMultiSqliteConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "root dir: {}", self.root_dir.display());
        let _ = writeln!(&mut result, "max connections: {}", self.max_connections);
        let _ = writeln!(&mut result, "foreign keys: {}", self.foreign_keys);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyValueStorageInMemoryConfig {}

impl SafeDisplay for KeyValueStorageInMemoryConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum SchedulerStorageConfig {
    Postgres(SchedulerStoragePostgresConfig),
    Sqlite(DbSqliteConfig),
    InMemory(SchedulerStorageInMemoryConfig),
}

impl SafeDisplay for SchedulerStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            SchedulerStorageConfig::Postgres(inner) => {
                let _ = writeln!(&mut result, "postgres:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            SchedulerStorageConfig::Sqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            SchedulerStorageConfig::InMemory(inner) => {
                let _ = writeln!(&mut result, "in-memory:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

impl Default for SchedulerStorageConfig {
    fn default() -> Self {
        Self::Sqlite(DbSqliteConfig {
            database: "../data/worker-executor-scheduler-storage.db".to_string(),
            max_connections: 10,
            foreign_keys: false,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulerStoragePostgresConfig {
    #[serde(flatten)]
    pub postgres: DbPostgresConfig,
}

impl SafeDisplay for SchedulerStoragePostgresConfig {
    fn to_safe_string(&self) -> String {
        self.postgres.to_safe_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SchedulerStorageInMemoryConfig {}

impl SafeDisplay for SchedulerStorageInMemoryConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum IndexedStorageConfig {
    KVStoreRedis(IndexedStorageKVStoreRedisConfig),
    Redis(RedisConfig),
    Postgres(IndexedStoragePostgresConfig),
    KVStoreSqlite(IndexedStorageKVStoreSqliteConfig),
    KVStoreMultiSqlite(IndexedStorageKVStoreMultiSqliteConfig),
    Sqlite(DbSqliteConfig),
    MultiSqlite(IndexedStorageMultiSqliteConfig),
    InMemory(IndexedStorageInMemoryConfig),
}

impl SafeDisplay for IndexedStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            IndexedStorageConfig::KVStoreRedis(inner) => {
                let _ = writeln!(&mut result, "redis kv-store:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::Redis(inner) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::Postgres(inner) => {
                let _ = writeln!(&mut result, "postgres:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::KVStoreSqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite kv-store:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::KVStoreMultiSqlite(inner) => {
                let _ = writeln!(&mut result, "multi-sqlite kv-store:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::Sqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::MultiSqlite(inner) => {
                let _ = writeln!(&mut result, "multi-sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::InMemory(inner) => {
                let _ = writeln!(&mut result, "in-memory:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageKVStoreRedisConfig {}

impl SafeDisplay for IndexedStorageKVStoreRedisConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageKVStoreSqliteConfig {}

impl SafeDisplay for IndexedStorageKVStoreSqliteConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageKVStoreMultiSqliteConfig {}

impl SafeDisplay for IndexedStorageKVStoreMultiSqliteConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageMultiSqliteConfig {
    pub root_dir: PathBuf,
    pub max_connections: u32,
    pub foreign_keys: bool,
}

impl SafeDisplay for IndexedStorageMultiSqliteConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "root dir: {}", self.root_dir.display());
        let _ = writeln!(&mut result, "max connections: {}", self.max_connections);
        let _ = writeln!(&mut result, "foreign keys: {}", self.foreign_keys);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageInMemoryConfig {}

impl SafeDisplay for IndexedStorageInMemoryConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStoragePostgresConfig {
    #[serde(flatten)]
    pub postgres: DbPostgresConfig,
    #[serde(default = "default_indexed_storage_postgres_drop_prefix_delete_batch_size")]
    pub drop_prefix_delete_batch_size: u64,
    #[serde(default)]
    pub max_concurrent_ops: Option<u32>,
}

impl SafeDisplay for IndexedStoragePostgresConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "{}", self.postgres.to_safe_string_indented());
        let _ = writeln!(
            &mut result,
            "drop prefix delete batch size: {}",
            self.drop_prefix_delete_batch_size
        );
        let _ = writeln!(
            &mut result,
            "max concurrent ops: {:?}",
            self.max_concurrent_ops
        );
        result
    }
}

impl IndexedStoragePostgresConfig {
    pub fn new(postgres: DbPostgresConfig) -> Self {
        Self {
            postgres,
            drop_prefix_delete_batch_size:
                default_indexed_storage_postgres_drop_prefix_delete_batch_size(),
            max_concurrent_ops: None,
        }
    }
}

fn default_indexed_storage_postgres_drop_prefix_delete_batch_size() -> u64 {
    1024
}

impl Default for KeyValueStorageConfig {
    fn default() -> Self {
        Self::default_namespace_routed()
    }
}

impl KeyValueStorageConfig {
    pub fn default_redis() -> Self {
        Self::Redis(RedisConfig::default())
    }

    pub fn default_namespace_routed() -> Self {
        Self::NamespaceRouted(KeyValueStorageNamespaceRoutedConfig {
            cache: KeyValueStorageInnerConfig::Redis(RedisConfig::default()),
            persistent: KeyValueStorageInnerConfig::Sqlite(DbSqliteConfig {
                database: "../data/worker-executor-key-value-storage.db".to_string(),
                max_connections: 10,
                foreign_keys: false,
            }),
        })
    }
}

impl Default for IndexedStorageConfig {
    fn default() -> Self {
        Self::Sqlite(DbSqliteConfig {
            database: "../data/worker-executor-indexed-storage.db".to_string(),
            max_connections: 10,
            foreign_keys: false,
        })
    }
}
