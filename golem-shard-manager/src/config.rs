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
use golem_common::model::base64::Base64;
use golem_common::model::{Empty, RetryConfig};
use golem_common::tracing::TracingConfig;
use golem_service_base::clients::registry::GrpcRegistryServiceConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardManagerConfig {
    pub tracing: TracingConfig,
    pub persistence: PersistenceConfig,
    pub worker_executors: WorkerExecutorServiceConfig,
    pub health_check: HealthCheckConfig,
    /// The address every listener of this service binds to. `0.0.0.0` accepts IPv4 only; `::`
    /// accepts both IPv6 and IPv4.
    pub bind_address: IpAddr,
    pub http_port: u16,
    #[serde(with = "humantime_serde")]
    pub runtime_metrics_sampling_interval: Duration,
    pub grpc: GrpcApiConfig,
    pub number_of_shards: usize,
    pub rebalance_threshold: f64,
    #[serde(with = "humantime_serde")]
    pub shard_lease_duration: Duration,
    #[serde(with = "humantime_serde")]
    pub state_read_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub state_write_timeout: Duration,
    #[serde(with = "humantime_serde")]
    pub initial_health_check_timeout: Duration,
    pub registry_service: GrpcRegistryServiceConfig,
    pub resource_definition_fetcher: ResourceDefinitionFetcherConfig,
    pub quota: QuotaServiceConfig,
}

impl ShardManagerConfig {
    pub fn http_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind_address, self.http_port)
    }

    pub fn grpc_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind_address, self.grpc.port)
    }
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
        let _ = writeln!(&mut result, "bind address: {}", self.bind_address);
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
        let _ = writeln!(
            &mut result,
            "state read timeout: {:?}",
            self.state_read_timeout
        );
        let _ = writeln!(
            &mut result,
            "state write timeout: {:?}",
            self.state_write_timeout
        );
        let _ = writeln!(
            &mut result,
            "initial health check timeout: {:?}",
            self.initial_health_check_timeout
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
            bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            http_port: 8081,
            runtime_metrics_sampling_interval: Duration::from_secs(5),
            grpc: GrpcApiConfig::default(),
            number_of_shards: 1024,
            rebalance_threshold: 0.1,
            shard_lease_duration: Duration::from_secs(60),
            state_read_timeout: crate::sharding::shard_management::DEFAULT_STATE_READ_TIMEOUT,
            state_write_timeout: golem_common::base_model::shard_lease::default_state_write_timeout(
            ),
            initial_health_check_timeout:
                crate::sharding::shard_management::DEFAULT_INITIAL_HEALTH_CHECK_TIMEOUT,
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
/// * `Etcd` - **distributed mode**: the shard lease state and the quota state live in etcd,
///   each behind a compare-and-swap on a key's `mod_revision` and the leadership fence.
// Built once per process, so the size of the etcd variant does not matter.
#[allow(clippy::large_enum_variant)]
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
            database: "../data/golem_shard_manager.db".to_string(),
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
    /// Client URLs of the etcd cluster: `https://` with `tls` set, `http://` without it. The
    /// shard manager refuses to start on any other scheme.
    ///
    /// An environment override must be bracketed, otherwise it is read as a single string and
    /// fails to deserialize:
    /// `GOLEM__PERSISTENCE__CONFIG__ENDPOINTS=["http://a:2379","http://b:2379"]`
    pub endpoints: Vec<String>,
    /// Defaulted, so that selecting etcd by environment variable does not also require setting
    /// every setting: figment merges `GOLEM__PERSISTENCE__CONFIG__*` over the *default* variant's
    /// map, which is SQLite's, so these would otherwise be missing rather than inherited.
    #[serde(with = "humantime_serde", default = "default_etcd_connect_timeout")]
    pub connect_timeout: Duration,
    #[serde(with = "humantime_serde", default = "default_etcd_request_timeout")]
    pub request_timeout: Duration,
    /// How long a state read may retry retriable etcd failures before returning the last one.
    #[serde(with = "humantime_serde", default = "default_etcd_read_retry_timeout")]
    pub read_retry_timeout: Duration,
    #[serde(with = "humantime_serde", default = "default_etcd_retry_min_delay")]
    pub retry_min_delay: Duration,
    /// The longest wait between two retries of an etcd failure. With `tls` set, also how often a
    /// running replica checks whether etcd refuses its certificates.
    #[serde(with = "humantime_serde", default = "default_etcd_retry_max_delay")]
    pub retry_max_delay: Duration,
    /// How long etcd holds this replica's leadership lease without a renewal; renewed at TTL/3.
    #[serde(with = "humantime_serde", default = "default_leader_lease_ttl")]
    pub leader_lease_ttl: Duration,
    /// How many etcd revisions of history the leader keeps behind the shard lease state; older
    /// ones are compacted after each pass of its loop. The state is rewritten on every lease
    /// renewal, and quota lease changes are revisions too, so without compaction etcd grows until
    /// its space quota makes it read-only.
    /// Compaction is cluster-wide: `0` disables it, for an etcd cluster shared with tenants that
    /// keep their own history.
    #[serde(default = "default_etcd_compaction_retention_revisions")]
    pub compaction_retention_revisions: u64,
    /// TLS to etcd; see [`EtcdTlsConfig`].
    #[serde(default)]
    pub tls: Option<EtcdTlsConfig>,
    /// etcd user, for an etcd with authentication enabled. Set together with `password`.
    #[serde(default)]
    pub username: Option<String>,
    /// The password of `username`; usually set from a secret through
    /// `GOLEM__PERSISTENCE__CONFIG__PASSWORD`. Serialized as `****`, so no configuration dump holds it.
    /// Set this or `password_file`, not both.
    #[serde(default)]
    pub password: Option<EtcdPassword>,
    /// A file that holds the password of `username`, such as a key of a Kubernetes Secret mounted
    /// as files. Read once, at startup, as it is: only one trailing line ending is removed.
    #[serde(default)]
    pub password_file: Option<PathBuf>,
}

/// TLS to etcd. Set, every endpoint must be `https://`; unset, every endpoint must be `http://`.
///
/// Each item comes from a PEM file or from an inline value (the PEM, Base64-encoded), not both.
/// Files and values are read once, at startup, so a renewed certificate takes effect on a
/// restart; a running replica stops by itself when etcd refuses its certificates and the files have
/// changed since startup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EtcdTlsConfig {
    /// PEM file with the certificate of the CA that signed etcd's server certificate.
    #[serde(default)]
    pub ca_cert_file: Option<PathBuf>,
    /// `ca_cert_file` as an inline value.
    #[serde(default)]
    pub ca_cert: Option<Base64>,
    /// PEM file with this client's certificate, for an etcd that requires client certificates
    /// (mTLS). Set together with a client key.
    #[serde(default)]
    pub client_cert_file: Option<PathBuf>,
    /// `client_cert_file` as an inline value.
    #[serde(default)]
    pub client_cert: Option<Base64>,
    /// PEM file with the private key of the client certificate.
    #[serde(default)]
    pub client_key_file: Option<PathBuf>,
    /// `client_key_file` as an inline value; never shown or serialized.
    #[serde(default)]
    pub client_key: Option<EtcdTlsKey>,
    /// The name to check etcd's certificate against, when the endpoint host is an IP address or
    /// an alias that the certificate does not name.
    #[serde(default)]
    pub server_name: Option<String>,
}

/// The password of an etcd user. Neither its `Debug` form, the configuration display, a refused
/// configuration nor a serialized configuration shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct EtcdPassword(String);

/// An inline private key for etcd TLS: the PEM, Base64-encoded. Neither its `Debug` form, the
/// configuration display nor a serialized configuration shows it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct EtcdTlsKey(Base64);

impl EtcdTlsKey {
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for EtcdTlsKey {
    fn from(pem: Vec<u8>) -> Self {
        Self(Base64(pem))
    }
}

impl std::fmt::Debug for EtcdTlsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("****")
    }
}

impl Serialize for EtcdTlsKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("****")
    }
}

impl Serialize for EtcdPassword {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("****")
    }
}

const PASSWORD_NOT_TEXT: &str = "persistence.config.password must be text, but its value was read \
     as a number, a boolean or a list; in an environment variable, wrap the whole value in double \
     quotes";

impl<'de> Deserialize<'de> for EtcdPassword {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_string(PasswordVisitor)
    }
}

/// Accepts text only. The environment provider types a value of digits as a number and `true` or
/// `false` as a boolean, and serde's own message for such a value would print it, in a refusal that
/// the service prints at startup. Every other kind of value is refused with a message that does
/// not hold it; turning it back into text would guess (`0123` was read as `123`).
struct PasswordVisitor;

impl<'de> serde::de::Visitor<'de> for PasswordVisitor {
    type Value = EtcdPassword;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("the etcd password as text")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<EtcdPassword, E> {
        Ok(EtcdPassword(value.to_string()))
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<EtcdPassword, E> {
        Ok(EtcdPassword(value))
    }

    // Every arm below refuses with the same message on purpose: serde's default for each of them
    // builds a message that contains the value, and that message is printed at startup.
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<EtcdPassword, E> {
        Err(E::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<EtcdPassword, E> {
        Err(E::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_i128<E: serde::de::Error>(self, _: i128) -> Result<EtcdPassword, E> {
        Err(E::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<EtcdPassword, E> {
        Err(E::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_u128<E: serde::de::Error>(self, _: u128) -> Result<EtcdPassword, E> {
        Err(E::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<EtcdPassword, E> {
        Err(E::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_char<E: serde::de::Error>(self, _: char) -> Result<EtcdPassword, E> {
        Err(E::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_bytes<E: serde::de::Error>(self, _: &[u8]) -> Result<EtcdPassword, E> {
        Err(E::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, _: A) -> Result<EtcdPassword, A::Error> {
        Err(serde::de::Error::custom(PASSWORD_NOT_TEXT))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, _: A) -> Result<EtcdPassword, A::Error> {
        Err(serde::de::Error::custom(PASSWORD_NOT_TEXT))
    }
}

impl EtcdPassword {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<&str> for EtcdPassword {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Debug for EtcdPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("****")
    }
}

/// An inline certificate as the configuration display shows it: the SHA-256 fingerprint of its
/// first certificate, as the startup log also prints it.
fn inline_certificate(pem: &Base64) -> String {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject;

    match CertificateDer::pem_slice_iter(pem).find_map(Result::ok) {
        Some(der) => format!(
            "inline value, sha256 {}",
            crate::sharding::etcd_connection::sha256_hex(&der)
        ),
        None => "inline value without a certificate".to_string(),
    }
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

fn default_etcd_read_retry_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_etcd_retry_min_delay() -> Duration {
    Duration::from_millis(100)
}

fn default_etcd_retry_max_delay() -> Duration {
    Duration::from_secs(5)
}

/// Generous against what anything needs: nothing reads an old revision of the state or quota
/// keys, so the retention only has to outlast in-flight reads and the leader election's watches.
/// At ten executors this is about half an hour of shard lease renewals, and about 50 MB of
/// history; quota lease changes count against the same revisions, so busy quotas shorten that
/// span.
fn default_etcd_compaction_retention_revisions() -> u64 {
    1000
}

impl Default for EtcdConfig {
    fn default() -> Self {
        Self {
            endpoints: vec!["http://localhost:2379".to_string()],
            connect_timeout: default_etcd_connect_timeout(),
            request_timeout: default_etcd_request_timeout(),
            read_retry_timeout: default_etcd_read_retry_timeout(),
            retry_min_delay: default_etcd_retry_min_delay(),
            retry_max_delay: default_etcd_retry_max_delay(),
            leader_lease_ttl: default_leader_lease_ttl(),
            compaction_retention_revisions: default_etcd_compaction_retention_revisions(),
            tls: None,
            username: None,
            password: None,
            password_file: None,
        }
    }
}

impl SafeDisplay for EtcdConfig {
    fn to_safe_string(&self) -> String {
        let Self {
            endpoints,
            connect_timeout,
            request_timeout,
            read_retry_timeout,
            retry_min_delay,
            retry_max_delay,
            leader_lease_ttl,
            compaction_retention_revisions,
            tls,
            username,
            password,
            password_file,
        } = self;

        let mut result = String::new();
        let _ = writeln!(&mut result, "endpoints: {}", endpoints.join(", "));
        let _ = writeln!(&mut result, "connect timeout: {connect_timeout:?}");
        let _ = writeln!(&mut result, "request timeout: {request_timeout:?}");
        let _ = writeln!(&mut result, "read retry timeout: {read_retry_timeout:?}");
        let _ = writeln!(&mut result, "retry min delay: {retry_min_delay:?}");
        let _ = writeln!(&mut result, "retry max delay: {retry_max_delay:?}");
        let _ = writeln!(&mut result, "leader lease ttl: {leader_lease_ttl:?}");
        let _ = writeln!(
            &mut result,
            "compaction retention revisions: {compaction_retention_revisions}"
        );
        if let Some(tls) = tls {
            for (item, file, inline) in [
                (
                    "ca cert",
                    &tls.ca_cert_file,
                    tls.ca_cert.as_ref().map(inline_certificate),
                ),
                (
                    "client cert",
                    &tls.client_cert_file,
                    tls.client_cert.as_ref().map(inline_certificate),
                ),
                (
                    "client key",
                    &tls.client_key_file,
                    tls.client_key.as_ref().map(|_| "inline value".to_string()),
                ),
            ] {
                if let Some(file) = file {
                    let _ = writeln!(&mut result, "tls {item} file: {}", file.display());
                }
                if let Some(inline) = inline {
                    let _ = writeln!(&mut result, "tls {item}: {inline}");
                }
            }
            if let Some(name) = &tls.server_name {
                let _ = writeln!(&mut result, "tls server name: {name}");
            }
        }
        if let Some(username) = username {
            let _ = writeln!(&mut result, "username: {username}");
        }
        if password.is_some() {
            let _ = writeln!(&mut result, "password: ****");
        }
        if let Some(file) = password_file {
            let _ = writeln!(&mut result, "password file: {}", file.display());
        }
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

    use crate::config::{EtcdConfig, EtcdPassword, legacy_db_env_vars, make_config_loader};

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

    /// Loads an etcd configuration whose password is `raw` as the environment provider reads it:
    /// it parses every value with `Value::from_str`, which types digits as a number and `true` or
    /// `false` as a boolean.
    fn with_password_from_env(raw: &str) -> Result<EtcdConfig, figment::Error> {
        let value: figment::value::Value = raw.parse().expect("parsing a value cannot fail");
        figment::Figment::from(figment::providers::Serialized::defaults(
            EtcdConfig::default(),
        ))
        .merge(("password", value))
        .extract::<EtcdConfig>()
    }

    #[test]
    pub fn a_password_read_as_a_number_or_a_boolean_is_refused_without_showing_it() {
        for (raw, shown) in [
            ("123456", "123456"),
            ("-42", "42"),
            ("1.50", "1.5"),
            ("true", "true"),
        ] {
            let message = with_password_from_env(raw)
                .map(|_| ())
                .expect_err("a password that is not text must be refused")
                .to_string();
            assert!(
                message.contains("persistence.config.password must be text"),
                "{message}"
            );
            assert!(!message.contains(shown), "{message}");
        }
    }

    #[test]
    pub fn a_serialized_configuration_shows_the_password_as_stars() {
        let config = EtcdConfig {
            password: Some(EtcdPassword::from("s3cr3t-value")),
            ..EtcdConfig::default()
        };
        let password = figment::Figment::from(figment::providers::Serialized::defaults(&config))
            .find_value("password")
            .expect("a set password is serialized");
        assert_eq!(password.as_str(), Some("****"));
    }

    #[test]
    pub fn a_password_in_double_quotes_is_read_as_text() {
        let config = with_password_from_env("\"123456\"").expect("a quoted value is text");
        assert_eq!(config.password.expect("a password").expose(), "123456");

        let config = with_password_from_env("s3cr3t-value").expect("text is accepted");
        assert_eq!(
            config.password.expect("a password").expose(),
            "s3cr3t-value"
        );
    }
}
