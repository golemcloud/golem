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

//! The MCP session layer: session records in a shared store, and the per-replica state (response
//! events, running calls, GET streams) of the requests this replica serves.

mod calls;
mod event_store;
mod middleware;
mod replica;
mod rules;
mod sharded;
pub mod store;
mod streams;
#[cfg(test)]
mod tests;

/// Session IDs shared by the tests of this module.
#[cfg(test)]
mod test_ids {
    use super::store::McpSessionId;

    /// The text form of [`session`].
    pub(super) const SESSION: &str = "0123456789abcdef0123456789abcdef";

    pub(super) fn session() -> McpSessionId {
        SESSION.parse().unwrap()
    }

    pub(super) fn other_session() -> McpSessionId {
        "fedcba9876543210fedcba9876543210".parse().unwrap()
    }
}

pub use middleware::McpSessionLayer;
pub use replica::McpReplica;
pub(crate) use replica::SSE_RETRY_MILLIS;

use crate::config::{SessionStoreConfig, with_session_store_schema};
use golem_common::SafeDisplay;
use golem_common::config::{DbPostgresConfig, DbSqliteConfig, RedisConfig};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::time::Duration;

/// The MCP session settings. They are part of the worker service's `mcp` section, so their paths
/// are `mcp.session_idle_timeout` and `mcp.session_storage`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpSessionConfig {
    /// A session ends this long after its last request, unless a call of the session still runs.
    #[serde(with = "humantime_serde")]
    pub session_idle_timeout: Duration,
    /// Where the session records are kept. When it is not set, the MCP store uses the backend
    /// connection of `gateway_session_storage`, and with it the gateway store's pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_storage: Option<McpSessionStorageConfig>,
}

impl McpSessionConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.session_idle_timeout.as_millis() == 0 {
            return Err("mcp.session_idle_timeout must be at least 1ms".into());
        }
        match &self.session_storage {
            Some(McpSessionStorageConfig::Sqlite(McpSqliteStorageConfig {
                cleanup_interval,
                ..
            }))
            | Some(McpSessionStorageConfig::Postgres(McpPostgresStorageConfig {
                cleanup_interval,
                ..
            })) if cleanup_interval.is_zero() => {
                Err("mcp.session_storage cleanup_interval must be non-zero".into())
            }
            _ => Ok(()),
        }
    }

    /// The backend the MCP store uses: `session_storage`, or else the backend connection of the
    /// gateway session store with its cleanup interval.
    pub fn effective_session_storage(
        &self,
        gateway: &SessionStoreConfig,
    ) -> McpSessionStorageConfig {
        self.session_storage
            .clone()
            .unwrap_or_else(|| match gateway {
                SessionStoreConfig::Redis(inner) => {
                    McpSessionStorageConfig::Redis(inner.redis_config.clone())
                }
                SessionStoreConfig::Sqlite(inner) => {
                    McpSessionStorageConfig::Sqlite(McpSqliteStorageConfig {
                        cleanup_interval: inner.cleanup_interval,
                        sqlite_config: inner.sqlite_config.clone(),
                    })
                }
                SessionStoreConfig::Postgres(inner) => {
                    McpSessionStorageConfig::Postgres(McpPostgresStorageConfig {
                        cleanup_interval: inner.cleanup_interval,
                        postgres_config: inner.postgres_config_with_schema(),
                    })
                }
            })
    }
}

impl Default for McpSessionConfig {
    fn default() -> Self {
        Self {
            session_idle_timeout: Duration::from_secs(5 * 60),
            session_storage: None,
        }
    }
}

impl SafeDisplay for McpSessionConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "session idle timeout: {:?}",
            self.session_idle_timeout
        );
        match &self.session_storage {
            Some(session_storage) => {
                let _ = writeln!(&mut result, "session storage:");
                let _ = writeln!(&mut result, "{}", session_storage.to_safe_string_indented());
            }
            None => {
                let _ = writeln!(&mut result, "session storage: the gateway session storage");
            }
        }
        result
    }
}

/// The backend of the MCP session store. A SQLite file or Postgres schema may be the one of
/// `gateway_session_storage`: both stores use one migration set, and an equal connection shares
/// the gateway store's pool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum McpSessionStorageConfig {
    Redis(RedisConfig),
    Sqlite(McpSqliteStorageConfig),
    Postgres(McpPostgresStorageConfig),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpSqliteStorageConfig {
    /// How often expired session rows are deleted.
    #[serde(with = "humantime_serde")]
    pub cleanup_interval: Duration,
    #[serde(flatten)]
    pub sqlite_config: DbSqliteConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpPostgresStorageConfig {
    /// How often expired session rows are deleted.
    #[serde(with = "humantime_serde")]
    pub cleanup_interval: Duration,
    /// When `schema` is not set, the store uses the gateway session store's default schema.
    #[serde(flatten)]
    pub postgres_config: DbPostgresConfig,
}

impl McpPostgresStorageConfig {
    pub fn postgres_config_with_schema(&self) -> DbPostgresConfig {
        with_session_store_schema(&self.postgres_config)
    }
}

impl SafeDisplay for McpSessionStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            McpSessionStorageConfig::Redis(redis) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", redis.to_safe_string_indented());
            }
            McpSessionStorageConfig::Sqlite(sqlite) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(
                    &mut result,
                    "  cleanup_interval: {:?}",
                    sqlite.cleanup_interval
                );
                let _ = writeln!(
                    &mut result,
                    "{}",
                    sqlite.sqlite_config.to_safe_string_indented()
                );
            }
            McpSessionStorageConfig::Postgres(postgres) => {
                let _ = writeln!(&mut result, "postgres:");
                let _ = writeln!(
                    &mut result,
                    "  cleanup_interval: {:?}",
                    postgres.cleanup_interval
                );
                let _ = writeln!(
                    &mut result,
                    "{}",
                    postgres
                        .postgres_config_with_schema()
                        .to_safe_string_indented()
                );
            }
        }
        result
    }
}
