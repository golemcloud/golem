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

#[cfg(feature = "kubernetes")]
use crate::config::HealthCheckMode::K8s;
use golem_common::SafeDisplay;
use golem_common::config::{
    ConfigExample, ConfigLoader, DbPostgresConfig, DbSqliteConfig, HasConfigExamples,
};
use golem_common::model::{Empty, RetryConfig};
use golem_common::tracing::TracingConfig;
use golem_service_base::clients::registry::GrpcRegistryServiceConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardManagerConfig {
    pub tracing: TracingConfig,
    pub persistence: PersistenceConfig,
    pub worker_executors: WorkerExecutorServiceConfig,
    pub health_check: HealthCheckConfig,
    pub http_port: u16,
    #[serde(with = "humantime_serde")]
    pub runtime_metrics_sampling_interval: Duration,
    pub grpc: GrpcApiConfig,
    pub number_of_shards: usize,
    pub rebalance_threshold: f64,
    #[serde(with = "humantime_serde")]
    pub shard_lease_duration: Duration,
    pub registry_service: GrpcRegistryServiceConfig,
    pub resource_definition_fetcher: ResourceDefinitionFetcherConfig,
    pub quota: QuotaServiceConfig,
}

impl SafeDisplay for ShardManagerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "persistence:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.persistence.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "worker executors:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.worker_executors.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "healthcheck:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.health_check.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "HTTP port: {}", self.http_port);
        let _ = writeln!(
            &mut result,
            "runtime metrics sampling interval: {}s",
            self.runtime_metrics_sampling_interval.as_secs()
        );

        let _ = writeln!(&mut result, "grpc:");
        let _ = writeln!(&mut result, "{}", self.grpc.to_safe_string_indented());

        let _ = writeln!(&mut result, "number of shards: {}", self.number_of_shards);
        let _ = writeln!(
            &mut result,
            "rebalance threshold: {}",
            self.rebalance_threshold
        );
        let _ = writeln!(
            &mut result,
            "shard lease duration: {:?}",
            self.shard_lease_duration
        );
        let _ = writeln!(&mut result, "registry service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.registry_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "resource definition fetcher:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.resource_definition_fetcher.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "quota:");
        let _ = writeln!(&mut result, "{}", self.quota.to_safe_string_indented());
        result
    }
}

impl Default for ShardManagerConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("shard-manager"),
            persistence: PersistenceConfig::default(),
            worker_executors: WorkerExecutorServiceConfig::default(),
            health_check: HealthCheckConfig::default(),
            http_port: 8081,
            runtime_metrics_sampling_interval: Duration::from_secs(5),
            grpc: GrpcApiConfig::default(),
            number_of_shards: 1024,
            rebalance_threshold: 0.1,
            shard_lease_duration: Duration::from_secs(60),
            registry_service: GrpcRegistryServiceConfig::default(),
            resource_definition_fetcher: ResourceDefinitionFetcherConfig::default(),
            quota: QuotaServiceConfig::default(),
        }
    }
}

impl HasConfigExamples<ShardManagerConfig> for ShardManagerConfig {
    fn examples() -> Vec<ConfigExample<ShardManagerConfig>> {
        let etcd_example: ConfigExample<ShardManagerConfig> = (
            "with etcd persistence (distributed mode)",
            Self {
                persistence: PersistenceConfig::Etcd(EtcdConfig::default()),
                ..Self::default()
            },
        );

        #[cfg(feature = "kubernetes")]
        {
            vec![
                (
                    "with k8s healthcheck",
                    Self {
                        health_check: HealthCheckConfig {
                            delay: Duration::from_secs(1),
                            mode: K8s(HealthCheckK8sConfig {
                                namespace: "namespace".to_string(),
                            }),
                            silent: false,
                        },
                        ..Self::default()
                    },
                ),
                etcd_example,
            ]
        }

        #[cfg(not(feature = "kubernetes"))]
        {
            vec![etcd_example]
        }
    }
}

/// Where the shard manager persists its state, which also selects the deployment mode.
///
/// * `Postgres` / `Sqlite` - **local mode**: a single shard manager instance, with the shard
///   lease state and the quota state in one SQL database.
/// * `Etcd` - **distributed mode**: the shard lease state lives in etcd behind a
///   compare-and-swap on the key's `mod_revision`. Quota state is not durable in this mode.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum PersistenceConfig {
    Postgres(DbPostgresConfig),
    Sqlite(DbSqliteConfig),
    Etcd(EtcdConfig),
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self::Sqlite(DbSqliteConfig {
            database: "golem_shard_manager.db".to_string(),
            ..Default::default()
        })
    }
}

impl SafeDisplay for PersistenceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            PersistenceConfig::Postgres(postgres) => {
                let _ = writeln!(&mut result, "postgres:");
                let _ = writeln!(&mut result, "{}", postgres.to_safe_string_indented());
            }
            PersistenceConfig::Sqlite(sqlite) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", sqlite.to_safe_string_indented());
            }
            PersistenceConfig::Etcd(etcd) => {
                let _ = writeln!(&mut result, "etcd:");
                let _ = writeln!(&mut result, "{}", etcd.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EtcdConfig {
    /// Client URLs of the etcd cluster.
    ///
    /// TLS is not configurable, so only `http://` endpoints are accepted; the shard manager
    /// refuses to start otherwise.
    ///
    /// An environment override must be bracketed, otherwise it is read as a single string and
    /// fails to deserialize:
    /// `GOLEM__PERSISTENCE__CONFIG__ENDPOINTS=["http://a:2379","http://b:2379"]`
    pub endpoints: Vec<String>,
    /// Defaulted, so that selecting etcd by environment variable does not also require setting
    /// every timeout: figment merges `GOLEM__PERSISTENCE__CONFIG__*` over the *default* variant's
    /// map, which is SQLite's, so these two would otherwise be missing rather than inherited.
    #[serde(with = "humantime_serde", default = "default_etcd_connect_timeout")]
    pub connect_timeout: Duration,
    #[serde(with = "humantime_serde", default = "default_etcd_request_timeout")]
    pub request_timeout: Duration,
    /// How long etcd holds this replica's leadership lease without a renewal; renewed at TTL/3.
    #[serde(with = "humantime_serde", default = "default_leader_lease_ttl")]
    pub leader_lease_ttl: Duration,
}

fn default_etcd_connect_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_leader_lease_ttl() -> Duration {
    Duration::from_secs(10)
}

fn default_etcd_request_timeout() -> Duration {
    Duration::from_secs(5)
}

impl Default for EtcdConfig {
    fn default() -> Self {
        Self {
            endpoints: vec!["http://localhost:2379".to_string()],
            connect_timeout: default_etcd_connect_timeout(),
            request_timeout: default_etcd_request_timeout(),
            leader_lease_ttl: default_leader_lease_ttl(),
        }
    }
}

impl SafeDisplay for EtcdConfig {
    fn to_safe_string(&self) -> String {
        let Self {
            endpoints,
            connect_timeout,
            request_timeout,
            leader_lease_ttl,
        } = self;

        let mut result = String::new();
        let _ = writeln!(&mut result, "endpoints: {}", endpoints.join(", "));
        let _ = writeln!(&mut result, "connect timeout: {connect_timeout:?}");
        let _ = writeln!(&mut result, "request timeout: {request_timeout:?}");
        let _ = writeln!(&mut result, "leader lease ttl: {leader_lease_ttl:?}");
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceDefinitionFetcherConfig {
    pub cache_max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub cache_eviction_period: Duration,
}

impl SafeDisplay for ResourceDefinitionFetcherConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "cache max capacity: {}",
            self.cache_max_capacity
        );
        let _ = writeln!(&mut result, "cache ttl: {:?}", self.cache_ttl);
        let _ = writeln!(
            &mut result,
            "cache eviction period: {:?}",
            self.cache_eviction_period
        );
        result
    }
}

impl Default for ResourceDefinitionFetcherConfig {
    fn default() -> Self {
        Self {
            cache_max_capacity: 1024,
            cache_ttl: Duration::from_secs(5 * 60),
            cache_eviction_period: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuotaServiceConfig {
    #[serde(with = "humantime_serde")]
    pub lease_duration: Duration,
    #[serde(with = "humantime_serde")]
    pub definition_staleness_ttl: Duration,
    /// Minimum number of executors to plan for when dividing budget.
    /// Prevents the first executor from taking the entire quota.
    pub min_executors: u64,
}

impl SafeDisplay for QuotaServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "lease duration: {:?}", self.lease_duration);
        let _ = writeln!(
            &mut result,
            "definition staleness ttl: {:?}",
            self.definition_staleness_ttl
        );
        let _ = writeln!(&mut result, "min executors: {}", self.min_executors);
        result
    }
}

impl Default for QuotaServiceConfig {
    fn default() -> Self {
        Self {
            lease_duration: Duration::from_secs(60),
            definition_staleness_ttl: Duration::from_secs(5 * 60),
            min_executors: 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcApiConfig {
    pub port: u16,
    pub tls: GrpcServerTlsConfig,
}

impl SafeDisplay for GrpcApiConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "port: {}", self.port);

        let _ = writeln!(&mut result, "tls:");
        let _ = writeln!(&mut result, "{}", self.tls.to_safe_string_indented());

        result
    }
}

impl Default for GrpcApiConfig {
    fn default() -> Self {
        Self {
            port: 9092,
            tls: GrpcServerTlsConfig::disabled(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerExecutorServiceConfig {
    #[serde(with = "humantime_serde")]
    pub assign_shards_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub health_check_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub revoke_shards_timeout: Duration,
    pub retries: RetryConfig,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
}

impl SafeDisplay for WorkerExecutorServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "assign shards timeout: {:?}",
            self.assign_shards_timeout
        );
        let _ = writeln!(
            &mut result,
            "health check timeout: {:?}",
            self.health_check_timeout
        );
        let _ = writeln!(
            &mut result,
            "revoke shards timeout: {:?}",
            self.revoke_shards_timeout
        );
        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        result
    }
}

impl Default for WorkerExecutorServiceConfig {
    fn default() -> Self {
        Self {
            assign_shards_timeout: Duration::from_secs(5),
            health_check_timeout: Duration::from_secs(2),
            revoke_shards_timeout: Duration::from_secs(5),
            retries: RetryConfig::max_attempts_5(),
            client_config: GrpcClientConfig {
                connect_timeout: Duration::from_secs(10),
                ..Default::default()
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(with = "humantime_serde")]
    pub delay: Duration,
    pub mode: HealthCheckMode,
    pub silent: bool,
}

impl SafeDisplay for HealthCheckConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "delay: {:?}", self.delay);
        let _ = writeln!(&mut result, "mode:");
        let _ = writeln!(&mut result, "{}", self.mode.to_safe_string_indented());
        let _ = writeln!(&mut result, "silent: {}", self.silent);
        result
    }
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(10),
            mode: HealthCheckMode::default(),
            silent: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum HealthCheckMode {
    Grpc(Empty),
    #[cfg(feature = "kubernetes")]
    K8s(HealthCheckK8sConfig),
}

impl SafeDisplay for HealthCheckMode {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            HealthCheckMode::Grpc(_) => {
                let _ = writeln!(&mut result, "gRPC");
            }
            #[cfg(feature = "kubernetes")]
            HealthCheckMode::K8s(inner) => {
                let _ = writeln!(&mut result, "k8s:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

impl Default for HealthCheckMode {
    fn default() -> Self {
        Self::Grpc(Empty {})
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthCheckK8sConfig {
    pub namespace: String,
}

impl SafeDisplay for HealthCheckK8sConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "namespace: {}", self.namespace);
        result
    }
}

pub fn make_config_loader() -> ConfigLoader<ShardManagerConfig> {
    ConfigLoader::new_with_examples(Path::new("config/shard-manager.toml"))
}

/// Environment variables that configured the shard manager's database before `db` became
/// [`PersistenceConfig`].
const LEGACY_DB_ENV_VAR_PREFIX: &str = "GOLEM__DB__";

fn legacy_db_env_vars<I: IntoIterator<Item = String>>(names: I) -> Vec<String> {
    let mut found: Vec<String> = names
        .into_iter()
        .filter(|name| name.starts_with(LEGACY_DB_ENV_VAR_PREFIX))
        .collect();
    found.sort();
    found
}

/// Fails if the environment still configures the shard manager through the removed `db` key.
///
/// figment layers the environment over the defaults and serde then discards unknown keys, so
/// without this a deployment that was not updated starts *successfully* on the default SQLite
/// database, with an empty routing table and a quota ledger that resets on every restart.
pub fn reject_legacy_db_env_vars() -> Result<(), String> {
    let found = legacy_db_env_vars(std::env::vars().map(|(name, _)| name));
    if found.is_empty() {
        return Ok(());
    }

    Err(format!(
        "The shard manager's `db` configuration was replaced by `persistence`, which also selects \
         which backend holds the shard lease state. The following environment variables are no \
         longer read, and ignoring them would start the shard manager on the default SQLite \
         database with an empty routing table:\n  {}\nRename them to `GOLEM__PERSISTENCE__*` (for \
         example `GOLEM__DB__TYPE` becomes `GOLEM__PERSISTENCE__TYPE`).",
        found.join("\n  ")
    ))
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::config::{legacy_db_env_vars, make_config_loader};

    #[test]
    pub fn config_is_loadable() {
        let _ = make_config_loader().load().expect("Failed to load config");
    }

    #[test]
    pub fn legacy_db_env_vars_are_detected() {
        let found = legacy_db_env_vars(
            [
                "GOLEM__DB__CONFIG__HOST",
                "GOLEM__PERSISTENCE__CONFIG__HOST",
                "GOLEM__DB__TYPE",
                "GOLEM__HTTP_PORT",
                "PATH",
                "SOMETHING__GOLEM__DB__TYPE",
                "GOLEM__DBX__TYPE",
            ]
            .map(str::to_string),
        );

        assert_eq!(found, vec!["GOLEM__DB__CONFIG__HOST", "GOLEM__DB__TYPE"]);
    }

    #[test]
    pub fn an_environment_without_the_legacy_key_is_accepted() {
        let found = legacy_db_env_vars(
            ["GOLEM__PERSISTENCE__TYPE", "GOLEM__HTTP_PORT"].map(str::to_string),
        );

        assert!(found.is_empty(), "unexpected legacy variables: {found:?}");
    }
}
