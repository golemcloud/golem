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

//! Configuration of `golem server run`.
//!
//! [`GolemServerConfig`] composes the configuration types of the services that the single
//! executable runs, so each section has exactly the shape of that service's own config file. The
//! defaults are the local development setup, derived from the data directory. The layers on top
//! of them are, from lowest to highest precedence: the manifest's `localServer` section, the
//! `--config` TOML file, `GOLEM__` environment variables and the `server run` flags.

use crate::filesystem_config::{self, LocalPlatform};
use crate::registry_db_path;
#[cfg(feature = "local-observability")]
use crate::well_known_secrets::accounts_with_well_known_token;
use anyhow::{Context, anyhow, bail};
use figment::providers::{Format, Serialized, Toml};
use figment::value::{Dict, Value};
use figment::{Figment, Metadata, Profile, Provider};
use golem_cli::command::server::RunArgs;
use golem_cli::config::{
    DEFAULT_LOCAL_CUSTOM_REQUEST_PORT, DEFAULT_LOCAL_MCP_PORT, DEFAULT_LOCAL_ROUTER_ADDR,
    DEFAULT_LOCAL_ROUTER_PORT,
};
use golem_cli::fs;
use golem_cli::model::app::ResolvedLocalServer;
use golem_common::config::{DbConfig, DbPostgresConfig, DbSqliteConfig, env_config_provider};
use golem_common::model::Empty;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::model::domain_registration::Domain;
use golem_common::model::plan::{PlanId, PlanName};
use golem_common::model::tool::BUILTIN_TOOL_OWNER_ACCOUNT_EMAIL;
use golem_component_compilation_service::config::ServerConfig as ComponentCompilationServiceConfig;
use golem_registry_service::config::{
    BuiltinArtifactsConfig, BuiltinPluginsConfig, ComponentCompilationEnabledConfig, LoginConfig,
    PrecreatedAccount, PrecreatedPlan, RegistryServiceConfig,
};
use golem_service_base::config::{
    BlobStorageConfig, LocalFileSystemBlobStorageConfig, S3BlobStorageConfig,
};
use golem_service_base::service::compiled_component::{
    CompiledComponentServiceConfig, CompiledComponentServiceEnabledConfig,
};
use golem_shard_manager::config::ShardManagerConfig;
use golem_worker_executor::services::golem_config::{
    AgentWebhooksServiceConfig, EnvironmentStateServiceConfig, FilesystemStorageConfig,
    FilesystemStorageMode, GolemConfig as WorkerExecutorConfig, IndexedStorageConfig,
    IndexedStorageKVStoreMultiSqliteConfig, IndexedStoragePostgresConfig, KeyValueStorageConfig,
    KeyValueStorageInnerConfig, KeyValueStorageMultiSqliteConfig, KeyValueStoragePostgresConfig,
    SchedulerStorageConfig, SchedulerStoragePostgresConfig,
};
use golem_worker_service::config::{
    DEFAULT_POSTGRES_SESSION_STORE_SCHEMA, PostgresSessionStoreConfig, RouteResolverConfig,
    SessionStoreConfig, SqliteSessionStoreConfig, WorkerServiceConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::uuid;

/// The host the services of the single executable use to connect to each other. Every service
/// binds all its listeners to this host by default; a service whose `bind_address` is
/// unspecified (`0.0.0.0` or `::`) is reachable through it too.
pub const INTERNAL_HOST: &str = "127.0.0.1";

/// The default `bind_address` of every service, [`INTERNAL_HOST`].
const INTERNAL_BIND_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Postgres schemas of the services. They are the schemas of the per-service deployment in
/// `docker-examples/published-postgres`, so a database can move between the two.
pub const REGISTRY_SERVICE_POSTGRES_SCHEMA: &str = "golem_registry_service";
pub const SHARD_MANAGER_POSTGRES_SCHEMA: &str = "golem_shard_manager";
pub const WORKER_EXECUTOR_KEY_VALUE_POSTGRES_SCHEMA: &str = "golem_worker_executor";
pub const WORKER_EXECUTOR_INDEXED_POSTGRES_SCHEMA: &str = "golem_worker_executor_indexed";
pub const WORKER_EXECUTOR_SCHEDULER_POSTGRES_SCHEMA: &str = "golem_worker_executor_scheduler";

/// The plan limit that the local development plan uses for every quota. It is the registry's
/// unlimited value, which TOML can represent, unlike `u64::MAX`.
pub const UNLIMITED: u64 = 1_000_000_000_000_000_000;

/// The bucket of the S3 blob storage in the Postgres and S3 example.
const EXAMPLE_S3_BUCKET: &str = "golem";

/// The data directory used for the generated reference config files.
pub const REFERENCE_DATA_DIR: &str = "/var/lib/golem";

/// Config keys that `golem server run` sets itself: the ports and addresses that wire the
/// services together, the tracing settings, because the executable sets up tracing once for all
/// services, and the worker service's CORS setting, because the main API applies the registry
/// service's one to both APIs. Any layer that sets one of them is rejected.
const LAUNCHER_OWNED_KEYS: &[&str] = &[
    "registry_service.tracing",
    "registry_service.http_port",
    "registry_service.grpc.port",
    "registry_service.component_compilation.config.host",
    "registry_service.component_compilation.config.port",
    "shard_manager.tracing",
    "shard_manager.http_port",
    "shard_manager.grpc.port",
    "shard_manager.registry_service.host",
    "shard_manager.registry_service.port",
    "worker_service.tracing",
    "worker_service.port",
    "worker_service.grpc.port",
    "worker_service.shard_manager.host",
    "worker_service.shard_manager.port",
    "worker_service.registry_service.host",
    "worker_service.registry_service.port",
    "worker_service.cors_origin_regex",
    "worker_executor.tracing",
    "worker_executor.tracing_file_name_with_port",
    "worker_executor.http_port",
    "worker_executor.grpc.port",
    "worker_executor.grpc.advertised_ip",
    "worker_executor.shard_manager.host",
    "worker_executor.shard_manager.port",
    "worker_executor.registry_service.host",
    "worker_executor.registry_service.port",
    "worker_executor.public_worker_api.host",
    "worker_executor.public_worker_api.port",
    "component_compilation_service.tracing",
    "component_compilation_service.http_port",
    "component_compilation_service.grpc.port",
    "component_compilation_service.registry_service",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GolemServerConfig {
    pub data_dir: PathBuf,
    /// File to write the bound ports to as JSON once the server has started.
    pub ports_file: Option<PathBuf>,
    pub router: RouterConfig,
    pub local_observability: LocalObservabilityConfig,
    pub registry_service: RegistryServiceConfig,
    pub shard_manager: ShardManagerConfig,
    pub worker_service: WorkerServiceConfig,
    pub worker_executor: WorkerExecutorConfig,
    pub component_compilation_service: ComponentCompilationServiceConfig,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalObservabilityConfig {
    pub enabled: bool,
}

/// The listener of the main API, which serves the registry and worker service APIs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouterConfig {
    /// IP address to bind. An IPv6 address binds both IPv6 and IPv4, so `::` accepts every
    /// connection.
    pub address: String,
    pub port: u16,
    /// Host of the management API. Without it, every request is a management API request. With
    /// it, only requests for this host reach the management API, and requests for every other
    /// host reach the HTTP API and MCP deployments, like on `worker_service.custom_request_port`.
    /// The registry reserves this host, so no environment can register it as a domain.
    pub management_host: Option<String>,
}

impl GolemServerConfig {
    /// The local development configuration, which keeps all state under `data_dir`: one SQLite
    /// database per service, one SQLite database per agent in `kv-store`, and blobs in `blobs`.
    pub fn for_data_dir(data_dir: &Path) -> anyhow::Result<Self> {
        let blob_storage = BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
            root: data_dir.join("blobs"),
        });
        let sqlite = |file_name: &str, foreign_keys: bool| -> anyhow::Result<DbSqliteConfig> {
            Ok(DbSqliteConfig {
                database: fs::path_to_str(&data_dir.join(file_name))?.to_string(),
                max_connections: 4,
                foreign_keys,
            })
        };

        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            ports_file: None,
            router: RouterConfig {
                address: DEFAULT_LOCAL_ROUTER_ADDR.to_string(),
                port: DEFAULT_LOCAL_ROUTER_PORT,
                management_host: None,
            },
            local_observability: LocalObservabilityConfig::default(),
            registry_service: RegistryServiceConfig {
                bind_address: INTERNAL_BIND_ADDRESS,
                http_port: 0,
                grpc: golem_registry_service::config::GrpcApiConfig {
                    port: 0,
                    ..Default::default()
                },
                db: DbConfig::Sqlite(DbSqliteConfig {
                    database: fs::path_to_str(&registry_db_path(data_dir))?.to_string(),
                    max_connections: 4,
                    foreign_keys: true,
                }),
                login: LoginConfig::Disabled(Empty {}),
                cors_origin_regex: ".*".to_string(),
                component_compilation:
                    golem_registry_service::config::ComponentCompilationConfig::Enabled(Box::new(
                        ComponentCompilationEnabledConfig {
                            host: INTERNAL_HOST.to_string(),
                            port: 0,
                            ..Default::default()
                        },
                    )),
                component_file_upload: Default::default(),
                blob_storage: blob_storage.clone(),
                initial_plans: local_initial_plans(),
                initial_accounts: local_initial_accounts(),
                builtin_plugins: BuiltinPluginsConfig::Enabled(Empty {}),
                builtin_artifacts: BuiltinArtifactsConfig {
                    cache_dir: Some(data_dir.join("builtin-artifacts")),
                    ..Default::default()
                },
                security_scheme: golem_registry_service::config::SecuritySchemeConfig {
                    strict_issuer_url_validation: false,
                },
                ..Default::default()
            },
            shard_manager: ShardManagerConfig {
                bind_address: INTERNAL_BIND_ADDRESS,
                http_port: 0,
                grpc: golem_shard_manager::config::GrpcApiConfig {
                    port: 0,
                    ..Default::default()
                },
                persistence: golem_shard_manager::config::PersistenceConfig::Sqlite(sqlite(
                    "shard_manager.db",
                    true,
                )?),
                health_check: golem_shard_manager::config::HealthCheckConfig {
                    silent: true,
                    ..Default::default()
                },
                registry_service:
                    golem_service_base::clients::registry::GrpcRegistryServiceConfig {
                        host: INTERNAL_HOST.to_string(),
                        port: 0,
                        ..Default::default()
                    },
                ..Default::default()
            },
            worker_service: WorkerServiceConfig {
                bind_address: INTERNAL_BIND_ADDRESS,
                port: 0,
                custom_request_port: DEFAULT_LOCAL_CUSTOM_REQUEST_PORT,
                mcp_port: DEFAULT_LOCAL_MCP_PORT,
                blob_storage: blob_storage.clone(),
                grpc: golem_worker_service::config::GrpcApiConfig {
                    port: 0,
                    ..Default::default()
                },
                gateway_session_storage: SessionStoreConfig::Sqlite(SqliteSessionStoreConfig {
                    pending_login_expiration: Duration::from_hours(1),
                    cleanup_interval: Duration::from_mins(5),
                    sqlite_config: sqlite("gateway-sessions.db", false)?,
                }),
                shard_manager: golem_service_base::clients::shard_manager::GrpcShardManagerConfig {
                    host: INTERNAL_HOST.to_string(),
                    port: 0,
                    ..Default::default()
                },
                registry_service:
                    golem_service_base::clients::registry::GrpcRegistryServiceConfig {
                        host: INTERNAL_HOST.to_string(),
                        port: 0,
                        ..Default::default()
                    },
                route_resolver: RouteResolverConfig {
                    router_cache_max_capacity: 0,
                    router_cache_ttl: Default::default(),
                    router_cache_eviction_period: Default::default(),
                    trusted_ingress_addresses: Vec::new(),
                },
                ..Default::default()
            },
            worker_executor: WorkerExecutorConfig {
                bind_address: INTERNAL_BIND_ADDRESS,
                http_port: 0,
                grpc: golem_worker_executor::services::golem_config::GrpcApiConfig {
                    port: 0,
                    ..Default::default()
                },
                key_value_storage: KeyValueStorageConfig::MultiSqlite(
                    KeyValueStorageMultiSqliteConfig {
                        root_dir: data_dir.join("kv-store"),
                        max_connections: 4,
                        foreign_keys: false,
                    },
                ),
                indexed_storage: IndexedStorageConfig::KVStoreMultiSqlite(
                    IndexedStorageKVStoreMultiSqliteConfig {},
                ),
                scheduler_storage: SchedulerStorageConfig::Sqlite(sqlite("scheduler.db", false)?),
                blob_storage: blob_storage.clone(),
                compiled_component_service: CompiledComponentServiceConfig::Enabled(
                    CompiledComponentServiceEnabledConfig {},
                ),
                shard_manager: golem_service_base::clients::shard_manager::GrpcShardManagerConfig {
                    host: INTERNAL_HOST.to_string(),
                    port: 0,
                    ..Default::default()
                },
                registry_service:
                    golem_service_base::clients::registry::GrpcRegistryServiceConfig {
                        host: INTERNAL_HOST.to_string(),
                        port: 0,
                        ..Default::default()
                    },
                public_worker_api:
                    golem_worker_executor::services::golem_config::WorkerServiceGrpcConfig {
                        host: INTERNAL_HOST.to_string(),
                        port: 0,
                        client_config: Default::default(),
                    },
                environment_state_service: EnvironmentStateServiceConfig {
                    cache_capacity: 0,
                    ..Default::default()
                },
                agent_webhooks_service: AgentWebhooksServiceConfig {
                    use_https_for_webhook_url: false,
                    ..Default::default()
                },
                filesystem_storage: FilesystemStorageConfig {
                    mode: FilesystemStorageMode::Temporary,
                    ..Default::default()
                },
                ..Default::default()
            },
            component_compilation_service: ComponentCompilationServiceConfig {
                registry_service:
                    golem_component_compilation_service::config::RegistryServiceConfig::Dynamic(
                        Empty {},
                    ),
                compiled_component_service: CompiledComponentServiceConfig::Enabled(
                    CompiledComponentServiceEnabledConfig {},
                ),
                blob_storage,
                bind_address: INTERNAL_BIND_ADDRESS,
                http_port: 0,
                grpc: golem_component_compilation_service::config::GrpcApiConfig {
                    port: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
        })
    }

    /// An example that keeps the state of every service in one Postgres database, with the
    /// schemas of the per-service deployment, and the blobs in one S3 bucket.
    pub fn postgres_s3_example(data_dir: &Path) -> anyhow::Result<Self> {
        let mut config = Self::for_data_dir(data_dir)?;
        let postgres = |schema: &str| DbPostgresConfig {
            schema: Some(schema.to_string()),
            ..example_postgres()
        };
        let blob_storage = BlobStorageConfig::S3(example_s3());

        config.registry_service.db = DbConfig::Postgres(postgres(REGISTRY_SERVICE_POSTGRES_SCHEMA));
        config.registry_service.blob_storage = blob_storage.clone();
        config.shard_manager.persistence = golem_shard_manager::config::PersistenceConfig::Postgres(
            postgres(SHARD_MANAGER_POSTGRES_SCHEMA),
        );
        config.worker_service.blob_storage = blob_storage.clone();
        config.worker_service.gateway_session_storage =
            SessionStoreConfig::Postgres(PostgresSessionStoreConfig {
                pending_login_expiration: Duration::from_hours(1),
                cleanup_interval: Duration::from_mins(5),
                postgres_config: postgres(DEFAULT_POSTGRES_SESSION_STORE_SCHEMA),
            });
        config.worker_executor.key_value_storage =
            KeyValueStorageConfig::Postgres(KeyValueStoragePostgresConfig {
                postgres: postgres(WORKER_EXECUTOR_KEY_VALUE_POSTGRES_SCHEMA),
            });
        config.worker_executor.indexed_storage = IndexedStorageConfig::Postgres(
            IndexedStoragePostgresConfig::new(postgres(WORKER_EXECUTOR_INDEXED_POSTGRES_SCHEMA)),
        );
        config.worker_executor.scheduler_storage =
            SchedulerStorageConfig::Postgres(SchedulerStoragePostgresConfig {
                postgres: postgres(WORKER_EXECUTOR_SCHEDULER_POSTGRES_SCHEMA),
            });
        config.worker_executor.blob_storage = blob_storage.clone();
        config.component_compilation_service.blob_storage = blob_storage;
        Ok(config)
    }

    /// The domains that no environment may register: `registry_service.domain_registration.
    /// reserved_domains`, and the management host of the router.
    pub fn reserved_domains(&self) -> Vec<String> {
        let mut reserved_domains = self
            .registry_service
            .domain_registration
            .reserved_domains
            .clone();
        if let Some(management_host) = &self.router.management_host {
            reserved_domains.push(management_host.clone());
        }
        reserved_domains
    }

    /// Whether the registry database is the SQLite file in the data directory, which carries the
    /// compatibility version of the local data.
    pub fn uses_local_registry_db(&self) -> bool {
        match &self.registry_service.db {
            DbConfig::Sqlite(sqlite) => {
                Path::new(&sqlite.database) == registry_db_path(&self.data_dir)
            }
            DbConfig::Postgres(_) => false,
        }
    }
}

impl GolemServerConfig {
    /// The config key and the address of every setting that selects where listeners bind: the
    /// main API, and each service, whose listeners all bind its `bind_address`.
    pub fn listener_addresses(&self) -> Vec<(&'static str, String)> {
        vec![
            ("router.address", self.router.address.clone()),
            (
                "registry_service.bind_address",
                self.registry_service.bind_address.to_string(),
            ),
            (
                "shard_manager.bind_address",
                self.shard_manager.bind_address.to_string(),
            ),
            (
                "worker_service.bind_address",
                self.worker_service.bind_address.to_string(),
            ),
            (
                "worker_executor.bind_address",
                self.worker_executor.bind_address.to_string(),
            ),
            (
                "component_compilation_service.bind_address",
                self.component_compilation_service.bind_address.to_string(),
            ),
        ]
    }

    /// The config keys of the stores that keep state outside `clean_root`, the resolved data
    /// directory that `golem server clean` deletes.
    ///
    /// A store is inside only when its path, with every symlink resolved, is below
    /// `clean_root`. Deleting a symlink does not delete its target, so every store is outside
    /// when `clean_root` itself is a symlink. A path that cannot be resolved counts as outside.
    pub fn state_outside(&self, clean_root: &Path) -> Vec<&'static str> {
        let clean_root_is_symlink = std::fs::symlink_metadata(clean_root)
            .is_ok_and(|metadata| metadata.file_type().is_symlink());
        let inside = |path: &Path| {
            !clean_root_is_symlink
                && resolve_path(path).is_some_and(|path| path.starts_with(clean_root))
        };
        let sqlite_inside = |sqlite: &DbSqliteConfig| inside(Path::new(&sqlite.database));
        let key_value_inside = |config: &KeyValueStorageInnerConfig| match config {
            KeyValueStorageInnerConfig::Sqlite(sqlite) => sqlite_inside(sqlite),
            KeyValueStorageInnerConfig::MultiSqlite(config) => inside(&config.root_dir),
            KeyValueStorageInnerConfig::InMemory(_) => true,
            KeyValueStorageInnerConfig::Redis(_) | KeyValueStorageInnerConfig::Postgres(_) => false,
        };
        let blob_storage_inside = |blob_storage: &BlobStorageConfig| match blob_storage {
            BlobStorageConfig::LocalFileSystem(config) => inside(&config.root),
            BlobStorageConfig::Sqlite(sqlite) => sqlite_inside(sqlite),
            BlobStorageConfig::KVStoreSqlite(_) | BlobStorageConfig::InMemory(_) => true,
            BlobStorageConfig::S3(_) => false,
        };

        let stores = [
            (
                "registry_service.db",
                match &self.registry_service.db {
                    DbConfig::Sqlite(sqlite) => sqlite_inside(sqlite),
                    DbConfig::Postgres(_) => false,
                },
            ),
            (
                "registry_service.blob_storage",
                blob_storage_inside(&self.registry_service.blob_storage),
            ),
            (
                "shard_manager.persistence",
                match &self.shard_manager.persistence {
                    golem_shard_manager::config::PersistenceConfig::Sqlite(sqlite) => {
                        sqlite_inside(sqlite)
                    }
                    _ => false,
                },
            ),
            (
                "worker_service.gateway_session_storage",
                match &self.worker_service.gateway_session_storage {
                    SessionStoreConfig::Sqlite(sqlite) => sqlite_inside(&sqlite.sqlite_config),
                    _ => false,
                },
            ),
            (
                "worker_service.blob_storage",
                blob_storage_inside(&self.worker_service.blob_storage),
            ),
            (
                "worker_executor.key_value_storage",
                match &self.worker_executor.key_value_storage {
                    KeyValueStorageConfig::Sqlite(sqlite) => sqlite_inside(sqlite),
                    KeyValueStorageConfig::MultiSqlite(config) => inside(&config.root_dir),
                    KeyValueStorageConfig::InMemory(_) => true,
                    KeyValueStorageConfig::NamespaceRouted(config) => {
                        key_value_inside(&config.cache) && key_value_inside(&config.persistent)
                    }
                    KeyValueStorageConfig::Redis(_) | KeyValueStorageConfig::Postgres(_) => false,
                },
            ),
            (
                "worker_executor.indexed_storage",
                match &self.worker_executor.indexed_storage {
                    IndexedStorageConfig::Sqlite(sqlite) => sqlite_inside(sqlite),
                    IndexedStorageConfig::MultiSqlite(config) => inside(&config.root_dir),
                    IndexedStorageConfig::KVStoreSqlite(_)
                    | IndexedStorageConfig::KVStoreMultiSqlite(_)
                    | IndexedStorageConfig::InMemory(_) => true,
                    _ => false,
                },
            ),
            (
                "worker_executor.scheduler_storage",
                match &self.worker_executor.scheduler_storage {
                    SchedulerStorageConfig::Sqlite(sqlite) => sqlite_inside(sqlite),
                    SchedulerStorageConfig::InMemory(_) => true,
                    SchedulerStorageConfig::Postgres(_) => false,
                },
            ),
            (
                "worker_executor.blob_storage",
                blob_storage_inside(&self.worker_executor.blob_storage),
            ),
            (
                "component_compilation_service.blob_storage",
                blob_storage_inside(&self.component_compilation_service.blob_storage),
            ),
        ];
        stores
            .into_iter()
            .filter(|(_, inside)| !inside)
            .map(|(key, _)| key)
            .collect()
    }
}

/// `path` with every symlink resolved, also when its last components do not exist yet: the
/// longest existing prefix is canonicalized, and the remaining components are appended, which
/// cannot be symlinks. Returns `None` when the path cannot be resolved, which includes a
/// symlink to a target that does not exist.
fn resolve_path(path: &Path) -> Option<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut existing = path.as_path();
    let mut missing = Vec::new();
    let resolved_prefix = loop {
        match std::fs::canonicalize(existing) {
            Ok(resolved) => break resolved,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(existing).is_ok() {
                    return None;
                }
                missing.push(existing.components().next_back()?);
                existing = existing.parent()?;
            }
            Err(_) => return None,
        }
    };
    let mut resolved = resolved_prefix;
    for component in missing.into_iter().rev() {
        match component {
            std::path::Component::Normal(name) => resolved.push(name),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
        }
    }
    Some(resolved)
}

fn example_postgres() -> DbPostgresConfig {
    DbPostgresConfig {
        host: "localhost".to_string(),
        database: "golem".to_string(),
        username: "golem".to_string(),
        password: "golem".to_string(),
        port: 5432,
        max_connections: 10,
        schema: None,
        acquire_timeout: None,
    }
}

/// S3 blob storage that keeps the blobs of every namespace in one bucket.
fn example_s3() -> S3BlobStorageConfig {
    let bucket = EXAMPLE_S3_BUCKET.to_string();
    S3BlobStorageConfig {
        namespace_prefixes: true,
        compilation_cache_bucket: bucket.clone(),
        custom_data_bucket: bucket.clone(),
        oplog_payload_bucket: bucket.clone(),
        compressed_oplog_buckets: vec![bucket.clone()],
        initial_agent_files_bucket: bucket.clone(),
        components_bucket: bucket.clone(),
        filesystem_snapshots_bucket: bucket,
        ..S3BlobStorageConfig::default()
    }
}

fn local_initial_plans() -> HashMap<String, PrecreatedPlan> {
    let plan = PrecreatedPlan {
        plan_id: local_plan_id(),
        plan_name: PlanName("default".to_string()),
        app_limit: UNLIMITED,
        env_limit: UNLIMITED,
        component_limit: UNLIMITED,
        worker_connection_limit: UNLIMITED,
        storage_limit: UNLIMITED,
        blob_storage_limit: UNLIMITED,
        monthly_gas_limit: UNLIMITED,
        monthly_upload_limit: UNLIMITED,
        max_memory_per_worker: UNLIMITED,
        max_memory_per_worker_ceiling: UNLIMITED,
        max_memory_per_worker_user_configurable: true,
        monthly_memory_gb_seconds: UNLIMITED,
        monthly_memory_gb_seconds_ceiling: UNLIMITED,
        monthly_memory_gb_seconds_user_configurable: true,
        max_table_elements_per_worker: UNLIMITED,
        max_disk_space_per_worker: UNLIMITED,
        max_disk_space_per_worker_ceiling: None,
        max_disk_space_per_worker_user_configurable: true,
        per_invocation_http_call_limit: UNLIMITED,
        per_invocation_rpc_call_limit: UNLIMITED,
        monthly_http_call_limit: UNLIMITED,
        monthly_rpc_call_limit: UNLIMITED,
        max_concurrent_agents_per_executor: UNLIMITED,
        oplog_writes_per_second: UNLIMITED,
    };
    HashMap::from([("default".to_string(), plan)])
}

fn local_plan_id() -> PlanId {
    PlanId(uuid!("e808bd76-a6ab-4090-ade4-8447b8e8550f"))
}

fn local_initial_accounts() -> HashMap<String, PrecreatedAccount> {
    HashMap::from([
        (
            "root".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("51de7d7d-f286-49aa-b79a-96022f7e2df9")),
                name: "Initial User".to_string(),
                email: AccountEmail::new("initial@user"),
                token: Some(TokenSecret::trusted(
                    golem_client::LOCAL_WELL_KNOWN_TOKEN.to_string(),
                )),
                plan_id: local_plan_id(),
                role: AccountRole::Admin,
            },
        ),
        (
            "builtin_plugin_owner".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("b0a654af-d67f-4d73-a824-cf75e122bfc0")),
                name: "Builtin Plugin Owner".to_string(),
                email: AccountEmail::new("builtin-plugin-owner@golem.cloud"),
                token: None,
                plan_id: local_plan_id(),
                role: AccountRole::BuiltinPluginOwner,
            },
        ),
        (
            "builtin_tool_owner".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("58bda34c-10d4-4bfb-8abd-d5e67f09ba3c")),
                name: "Builtin Tool Owner".to_string(),
                email: AccountEmail::new(BUILTIN_TOOL_OWNER_ACCOUNT_EMAIL),
                token: None,
                plan_id: local_plan_id(),
                role: AccountRole::BuiltinPluginOwner,
            },
        ),
    ])
}

pub fn default_data_dir() -> anyhow::Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or_else(|| anyhow!("Failed to get data local dir"))?
        .join("golem"))
}

/// Settings that both the manifest's `localServer` section and the `server run` flags provide.
#[derive(Default)]
struct LocalServerSettings {
    data_dir: Option<PathBuf>,
    ports_file: Option<PathBuf>,
    router_addr: Option<String>,
    router_port: Option<u16>,
    custom_request_port: Option<u16>,
    mcp_port: Option<u16>,
    system_memory_override: Option<u64>,
    agent_filesystem_root: Option<PathBuf>,
    observability: Option<bool>,
}

impl LocalServerSettings {
    fn from_local_server(local_server: &ResolvedLocalServer) -> Self {
        Self {
            data_dir: local_server.data_dir.clone(),
            ports_file: local_server.ports_file.clone(),
            router_addr: local_server.router_addr.clone(),
            router_port: local_server.router_port,
            custom_request_port: local_server.custom_request_port,
            mcp_port: local_server.mcp_port,
            system_memory_override: local_server.system_memory_override.map(|value| value.get()),
            agent_filesystem_root: local_server.agent_filesystem_root.clone(),
            observability: local_server.observability,
        }
    }

    fn from_run_args(args: &RunArgs) -> Self {
        Self {
            data_dir: args.data_dir.clone(),
            ports_file: args.ports_file.clone(),
            router_addr: args.router_addr.clone(),
            router_port: args.router_port,
            custom_request_port: args.custom_request_port,
            mcp_port: args.mcp_port,
            system_memory_override: args.system_memory_override.map(|value| value.get()),
            agent_filesystem_root: args.agent_filesystem_root.clone(),
            observability: args.observability,
        }
    }

    fn into_figment(self, platform: LocalPlatform) -> Figment {
        fn set<T: Serialize>(figment: Figment, key: &str, value: Option<T>) -> Figment {
            match value {
                Some(value) => figment.merge(Serialized::default(key, value)),
                None => figment,
            }
        }

        let figment = Figment::new();
        let figment = set(figment, "data_dir", self.data_dir);
        let figment = set(figment, "ports_file", self.ports_file);
        let figment = set(figment, "router.address", self.router_addr);
        let figment = set(figment, "router.port", self.router_port);
        let figment = set(figment, "local_observability.enabled", self.observability);
        let figment = set(
            figment,
            "worker_service.custom_request_port",
            self.custom_request_port,
        );
        let figment = set(figment, "worker_service.mcp_port", self.mcp_port);
        let figment = set(
            figment,
            "worker_executor.memory.system_memory_override",
            self.system_memory_override,
        );
        set(
            figment,
            "worker_executor.filesystem_storage.mode",
            self.agent_filesystem_root
                .map(|root| platform.rooted_mode(&root)),
        )
    }
}

/// Where [`load_server_config`] takes the repository key of filesystem snapshots from, when the
/// configuration enables snapshots without one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotRepositoryKeySource {
    /// The key kept in the data directory, which is created on first use.
    DataDir,
    /// A fixed key that only lets the configuration be validated. Commands that do not start the
    /// server use it, so they never create the key file.
    ValidationOnly,
}

/// Loads the configuration of `golem server run` from the platform defaults, the manifest's
/// `localServer` section, the `--config` file, the `GOLEM__` environment variables and the flags.
pub fn load_server_config(
    local_server: Option<&ResolvedLocalServer>,
    args: &RunArgs,
    snapshot_repository_key: SnapshotRepositoryKeySource,
) -> anyhow::Result<GolemServerConfig> {
    load_server_config_on(
        LocalPlatform::native(),
        local_server,
        args,
        snapshot_repository_key,
    )
}

fn load_server_config_on(
    platform: LocalPlatform,
    local_server: Option<&ResolvedLocalServer>,
    args: &RunArgs,
    snapshot_repository_key: SnapshotRepositoryKeySource,
) -> anyhow::Result<GolemServerConfig> {
    let mut layers = Vec::new();
    if let Some(local_server) = local_server {
        layers.push(Layer::read(
            Metadata::named("the manifest's localServer section"),
            LocalServerSettings::from_local_server(local_server).into_figment(platform),
        )?);
    }
    if let Some(config_file) = &args.config {
        if !config_file.is_file() {
            bail!(
                "Server config file {} does not exist",
                config_file.display()
            );
        }
        let provider = Toml::file_exact(config_file);
        layers.push(Layer::read(provider.metadata(), provider)?);
    }
    let env = env_config_provider();
    layers.push(Layer::read(env.metadata(), env)?);
    layers.push(Layer::read(
        Metadata::named("the `server run` flags"),
        LocalServerSettings::from_run_args(args).into_figment(platform),
    )?);

    let data_dir = match layers_figment(&layers).extract_inner::<PathBuf>("data_dir") {
        Ok(data_dir) => data_dir,
        Err(err) if err.missing() => default_data_dir()?,
        Err(err) => return Err(err).context("Failed to load the server configuration"),
    };
    layers.insert(
        0,
        Layer::read(
            Metadata::named("the platform defaults"),
            Serialized::defaults(platform.defaults(&data_dir)),
        )?,
    );

    resolve(
        GolemServerConfig::for_data_dir(&data_dir)?,
        layers,
        |validated| match snapshot_repository_key {
            SnapshotRepositoryKeySource::DataDir => {
                filesystem_config::retained_repository_key(&validated.data_dir)
                    .map(|key| Some(key.into()))
            }
            SnapshotRepositoryKeySource::ValidationOnly => Ok(None),
        },
    )
}

/// The values one source of configuration provides, with the metadata of that source, so errors
/// name the file or the environment variables that caused them.
#[derive(Clone)]
struct Layer {
    metadata: Metadata,
    dict: Dict,
}

impl Layer {
    fn read(metadata: Metadata, provider: impl Provider) -> anyhow::Result<Self> {
        let mut data = provider
            .data()
            .map_err(|err| anyhow!("Failed to read {}: {err}", metadata.name))?;
        Ok(Self {
            metadata,
            dict: data.remove(&Profile::Default).unwrap_or_default(),
        })
    }
}

impl Provider for Layer {
    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    fn data(&self) -> figment::Result<figment::value::Map<Profile, Dict>> {
        Ok(Profile::Default.collect(self.dict.clone()))
    }
}

fn layers_figment(layers: &[Layer]) -> Figment {
    layers.iter().fold(Figment::new(), |figment, layer| {
        figment.merge(layer.clone())
    })
}

/// Merges `layers` over `defaults`, from the lowest to the highest precedence.
///
/// Enum-valued settings are tagged with `type` and carry their fields in `config`. Where a layer
/// selects a different `type` than the layers below it, the `config` of those layers is dropped,
/// so the fields of the replaced variant do not leak into the selected one. The defaults keep
/// their `config` when the selected `type` is the default one. A layer that selects a variant
/// without any `config` below or in it gets an empty one, which variants without fields need;
/// the empty `config` is removed again for unit variants, which reject it.
///
/// When the merged layers enable filesystem snapshots without a repository key, the defaults get
/// [`filesystem_config::VALIDATION_ONLY_REPOSITORY_KEY`], and the configuration is validated with
/// it. Only a valid configuration is passed to `snapshot_repository_key`; the key it returns
/// replaces the validation-only key.
fn resolve(
    defaults: GolemServerConfig,
    layers: Vec<Layer>,
    snapshot_repository_key: impl FnOnce(&GolemServerConfig) -> anyhow::Result<Option<String>>,
) -> anyhow::Result<GolemServerConfig> {
    reject_launcher_owned_keys(&layers)?;

    let Value::Dict(_, defaults) =
        Value::serialize(&defaults).context("Failed to serialize the default configuration")?
    else {
        bail!("The default configuration is not a table");
    };
    let original_defaults = defaults.clone();
    let mut dicts: Vec<Dict> = std::iter::once(defaults)
        .chain(layers.iter().map(|layer| layer.dict.clone()))
        .collect();

    let mut empty_configs = Vec::new();
    for index in 1..dicts.len() {
        let (lower, upper) = dicts.split_at_mut(index);
        let effective = lower.iter().fold(Dict::new(), |mut merged, dict| {
            merge_dict(&mut merged, dict);
            merged
        });
        let mut switched = Vec::new();
        find_switched_variants(&effective, &upper[0], &mut Vec::new(), &mut switched);
        for path in switched {
            let mut config_path = path.clone();
            config_path.push("config".to_string());
            let mut type_path = path.clone();
            type_path.push("type".to_string());
            for dict in lower.iter_mut() {
                remove_path(dict, &config_path);
            }
            if get_path(&original_defaults, &type_path) == get_path(&upper[0], &type_path)
                && let Some(default_config) = get_path(&original_defaults, &config_path)
            {
                insert_path(&mut lower[0], &config_path, default_config.clone());
            }
            if get_path(&upper[0], &config_path).is_none()
                && get_path(&lower[0], &config_path).is_none()
            {
                insert_path(&mut upper[0], &config_path, Value::from(Dict::new()));
                empty_configs.push((index, config_path));
            }
        }
    }

    let snapshots_path = |key: &str| -> Vec<String> {
        format!("worker_executor.filesystem_snapshots.{key}")
            .split('.')
            .map(str::to_string)
            .collect()
    };
    let repository_key_path = snapshots_path("config.repository_key");
    let merged = dicts.iter().fold(Dict::new(), |mut merged, dict| {
        merge_dict(&mut merged, dict);
        merged
    });
    let repository_key_needed = get_path(&merged, &snapshots_path("type")).and_then(Value::as_str)
        == Some("Enabled")
        && get_path(&merged, &repository_key_path).is_none();
    if repository_key_needed {
        insert_path(
            &mut dicts[0],
            &repository_key_path,
            Value::from(filesystem_config::VALIDATION_ONLY_REPOSITORY_KEY.to_string()),
        );
    }

    let config = extract(&layers, &mut dicts, &mut empty_configs)?;
    if repository_key_needed && let Some(key) = snapshot_repository_key(&config)? {
        insert_path(&mut dicts[0], &repository_key_path, Value::from(key));
        return extract(&layers, &mut dicts, &mut empty_configs);
    }
    Ok(config)
}

/// Extracts and validates the configuration from `dicts`, the defaults followed by `layers`.
/// Each inserted empty `config` in `empty_configs` that a unit variant rejects is removed.
fn extract(
    layers: &[Layer],
    dicts: &mut [Dict],
    empty_configs: &mut Vec<(usize, Vec<String>)>,
) -> anyhow::Result<GolemServerConfig> {
    loop {
        let figment = layers.iter().zip(&dicts[1..]).fold(
            Figment::from(Serialized::defaults(&dicts[0])),
            |figment, (layer, dict)| {
                figment.merge(Layer {
                    metadata: layer.metadata.clone(),
                    dict: dict.clone(),
                })
            },
        );
        match figment.extract::<GolemServerConfig>() {
            Ok(config) => {
                validate(&config)?;
                return Ok(config);
            }
            Err(err) => {
                let rejected = empty_configs.iter().position(|(_, config_path)| {
                    err.clone().into_iter().any(|err| {
                        matches!(
                            err.kind,
                            figment::error::Kind::InvalidType(figment::error::Actual::Map, _)
                        ) && err.path.starts_with(&config_path[..config_path.len() - 1])
                    })
                });
                match rejected {
                    Some(position) => {
                        let (index, config_path) = empty_configs.remove(position);
                        remove_path(&mut dicts[index], &config_path);
                    }
                    None => {
                        return Err(err).context("Failed to load the server configuration");
                    }
                }
            }
        }
    }
}

fn reject_launcher_owned_keys(layers: &[Layer]) -> anyhow::Result<()> {
    let env_name = env_config_provider().metadata().name;
    for key in LAUNCHER_OWNED_KEYS {
        let path: Vec<String> = key.split('.').map(str::to_string).collect();
        for layer in layers {
            if get_path(&layer.dict, &path).is_some() {
                let source = if layer.metadata.name == env_name {
                    format!(
                        "set by the environment variable GOLEM__{}",
                        key.replace('.', "__").to_uppercase()
                    )
                } else {
                    match &layer.metadata.source {
                        Some(source) => format!("set in {} {source}", layer.metadata.name),
                        None => format!("set in {}", layer.metadata.name),
                    }
                };
                bail!(
                    "`{key}` is managed by `golem server run` and cannot be configured ({source})"
                );
            }
        }
    }
    Ok(())
}

fn find_switched_variants(
    effective: &Dict,
    layer: &Dict,
    path: &mut Vec<String>,
    switched: &mut Vec<Vec<String>>,
) {
    if let (Some(effective_tag), Some(layer_tag)) = (variant_tag(effective), variant_tag(layer))
        && effective_tag != layer_tag
    {
        switched.push(path.clone());
        return;
    }
    for (key, value) in layer {
        if let (Some(Value::Dict(_, effective)), Value::Dict(_, layer)) =
            (effective.get(key), value)
        {
            path.push(key.clone());
            find_switched_variants(effective, layer, path, switched);
            path.pop();
        }
    }
}

fn variant_tag(dict: &Dict) -> Option<&str> {
    dict.get("type").and_then(Value::as_str)
}

fn merge_dict(target: &mut Dict, source: &Dict) {
    for (key, value) in source {
        match (target.get_mut(key), value) {
            (Some(Value::Dict(_, target)), Value::Dict(_, source)) => merge_dict(target, source),
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn get_path<'a>(dict: &'a Dict, path: &[String]) -> Option<&'a Value> {
    let (last, parents) = path.split_last()?;
    let mut current = dict;
    for segment in parents {
        match current.get(segment) {
            Some(Value::Dict(_, next)) => current = next,
            _ => return None,
        }
    }
    current.get(last)
}

fn remove_path(dict: &mut Dict, path: &[String]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = dict;
    for segment in parents {
        match current.get_mut(segment) {
            Some(Value::Dict(_, next)) => current = next,
            _ => return,
        }
    }
    current.remove(last);
}

fn insert_path(dict: &mut Dict, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = dict;
    for segment in parents {
        let next = current
            .entry(segment.clone())
            .or_insert_with(|| Value::from(Dict::new()));
        match next {
            Value::Dict(_, next) => current = next,
            _ => return,
        }
    }
    current.insert(last.clone(), value);
}

/// Checks the rules that span services.
fn validate(config: &GolemServerConfig) -> anyhow::Result<()> {
    if let Some(management_host) = &config.router.management_host {
        Domain::parse_host(management_host)
            .map_err(|err| anyhow!("Invalid `router.management_host`: {err}"))?;
    }

    if config.local_observability.enabled {
        #[cfg(not(feature = "local-observability"))]
        bail!(
            "Local observability is enabled, but this binary was built without local observability support; set `local_observability.enabled = false` or pass `--observability false`"
        );

        #[cfg(feature = "local-observability")]
        {
            if config.router.management_host.is_some() {
                bail!(
                    "Local observability requires the conventional local management router without `router.management_host`; set `local_observability.enabled = false` or pass `--observability false`"
                );
            }
            if accounts_with_well_known_token(config).is_empty() {
                bail!(
                    "Local observability requires the well-known local admin token; set `local_observability.enabled = false` or pass `--observability false`"
                );
            }
        }
    }

    // The store, its database and the schema it uses when the database sets none.
    let mut postgres: Vec<(&str, &DbPostgresConfig, &str)> = Vec::new();
    if let DbConfig::Postgres(db) = &config.registry_service.db {
        postgres.push(("registry_service.db", db, "public"));
    }
    if let golem_shard_manager::config::PersistenceConfig::Postgres(db) =
        &config.shard_manager.persistence
    {
        postgres.push(("shard_manager.persistence", db, "public"));
    }
    if let SessionStoreConfig::Postgres(sessions) = &config.worker_service.gateway_session_storage {
        postgres.push((
            "worker_service.gateway_session_storage",
            &sessions.postgres_config,
            DEFAULT_POSTGRES_SESSION_STORE_SCHEMA,
        ));
    }
    if let KeyValueStorageConfig::Postgres(db) = &config.worker_executor.key_value_storage {
        postgres.push(("worker_executor.key_value_storage", &db.postgres, "public"));
    }
    if let IndexedStorageConfig::Postgres(db) = &config.worker_executor.indexed_storage {
        postgres.push(("worker_executor.indexed_storage", &db.postgres, "public"));
    }
    if let SchedulerStorageConfig::Postgres(db) = &config.worker_executor.scheduler_storage {
        postgres.push(("worker_executor.scheduler_storage", &db.postgres, "public"));
    }
    let location = |db: &DbPostgresConfig, default_schema: &str| {
        (
            db.host.clone(),
            db.port,
            db.database.clone(),
            db.schema
                .clone()
                .unwrap_or_else(|| default_schema.to_string()),
        )
    };
    for (index, (key, db, default_schema)) in postgres.iter().enumerate() {
        let db_location = location(db, default_schema);
        if let Some((other_key, _, _)) = postgres[..index]
            .iter()
            .find(|(_, other, other_default)| location(other, other_default) == db_location)
        {
            bail!(
                "`{other_key}` and `{key}` use the same Postgres schema `{}` of database `{}`; \
                 each of them needs its own schema, because each one keeps its own migrations",
                db_location.3,
                db.database
            );
        }
    }

    if config.worker_service.webhook_callback_handler.hmac_key
        != config.worker_executor.agent_webhooks_service.hmac_key
    {
        bail!(
            "`worker_service.webhook_callback_handler.hmac_key` and \
             `worker_executor.agent_webhooks_service.hmac_key` must be equal: the executor signs \
             webhook URLs with its key and the worker service verifies them with its key"
        );
    }
    Ok(())
}

/// Prints the default configuration and the Postgres and S3 example for a `--dump-config*`
/// flag. Returns `false` when `flag` is not a dump flag.
///
/// The output leaves out the launcher-owned keys and the paths derived from `data_dir`, so it is
/// a valid `--config` file in which changing `data_dir` moves all local state.
pub fn dump_config(flag: &str) -> anyhow::Result<bool> {
    let figment =
        |make: fn(&Path) -> anyhow::Result<GolemServerConfig>| -> anyhow::Result<Figment> {
            Ok(Figment::from(Serialized::defaults(configurable_value(
                make,
            )?)))
        };
    Ok(golem_common::config::dump_config(
        flag,
        &figment(GolemServerConfig::for_data_dir)?,
        &[(
            "Postgres and S3",
            figment(GolemServerConfig::postgres_s3_example)?,
        )],
    ))
}

/// The configuration that `make` derives from [`REFERENCE_DATA_DIR`], without the launcher-owned
/// keys and without the values that depend on the data directory.
fn configurable_value(
    make: fn(&Path) -> anyhow::Result<GolemServerConfig>,
) -> anyhow::Result<Value> {
    let serialize = |data_dir: &str| -> anyhow::Result<Value> {
        Value::serialize(make(Path::new(data_dir))?)
            .context("Failed to serialize the configuration")
    };
    let mut value = serialize(REFERENCE_DATA_DIR)?;
    let other = serialize("/other-data-dir")?;
    remove_differing_leaves(&mut value, &other);
    if let Value::Dict(_, dict) = &mut value {
        dict.insert(
            "data_dir".to_string(),
            Value::from(REFERENCE_DATA_DIR.to_string()),
        );
        for key in LAUNCHER_OWNED_KEYS {
            let path: Vec<String> = key.split('.').map(str::to_string).collect();
            remove_path(dict, &path);
        }
    }
    Ok(value)
}

fn remove_differing_leaves(value: &mut Value, other: &Value) {
    if let (Value::Dict(_, dict), Value::Dict(_, other)) = (&mut *value, other) {
        let keys: Vec<String> = dict.keys().cloned().collect();
        for key in keys {
            let differs = match (dict.get_mut(&key), other.get(&key)) {
                (Some(child @ Value::Dict(..)), Some(other_child @ Value::Dict(..))) => {
                    remove_differing_leaves(child, other_child);
                    false
                }
                (Some(child), Some(other_child)) => child != other_child,
                _ => true,
            };
            if differs {
                dict.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::Jail;
    use golem_cli::model::app_raw::LocalServer;
    use golem_registry_service::config::ComponentCompilationConfig;
    use golem_shard_manager::config::PersistenceConfig;
    use golem_worker_executor::services::golem_config::FilesystemSnapshotsConfig;
    use test_r::test;

    /// Loads the configuration on a platform without platform defaults, with the validation-only
    /// snapshot repository key.
    fn load(
        local_server: Option<&ResolvedLocalServer>,
        args: &RunArgs,
    ) -> anyhow::Result<GolemServerConfig> {
        load_server_config_on(
            LocalPlatform::Other,
            local_server,
            args,
            SnapshotRepositoryKeySource::ValidationOnly,
        )
    }

    fn local_server(value: LocalServer) -> ResolvedLocalServer {
        ResolvedLocalServer::from_raw_with_base_dir(&value, Path::new("/tmp/test-app"))
    }

    fn run_args_with_data_dir(data_dir: &str) -> RunArgs {
        RunArgs {
            data_dir: Some(PathBuf::from(data_dir)),
            ..RunArgs::default()
        }
    }

    fn blob_root(config: &BlobStorageConfig) -> &Path {
        match config {
            BlobStorageConfig::LocalFileSystem(config) => &config.root,
            other => panic!("expected filesystem blobs, got {other:?}"),
        }
    }

    fn sqlite_path(config: &DbSqliteConfig) -> &Path {
        Path::new(&config.database)
    }

    fn jail_error(result: anyhow::Result<GolemServerConfig>) -> String {
        format!(
            "{:#}",
            result.expect_err("expected the configuration to be rejected")
        )
    }

    fn load_on(
        platform: LocalPlatform,
        local_server: Option<&ResolvedLocalServer>,
        args: &RunArgs,
        snapshot_repository_key: SnapshotRepositoryKeySource,
    ) -> anyhow::Result<GolemServerConfig> {
        load_server_config_on(platform, local_server, args, snapshot_repository_key)
    }

    fn snapshot_store(
        config: &GolemServerConfig,
    ) -> &golem_worker_executor::services::golem_config::FilesystemSnapshotStoreConfig {
        let FilesystemSnapshotsConfig::Enabled(store) =
            &config.worker_executor.filesystem_snapshots
        else {
            panic!("expected enabled filesystem snapshots")
        };
        store
    }

    #[test]
    fn local_observability_is_opt_in_and_follows_layer_precedence() {
        Jail::expect_with(|jail| {
            assert!(
                !load(None, &RunArgs::default())
                    .unwrap()
                    .local_observability
                    .enabled
            );

            let manifest = local_server(LocalServer {
                observability: Some(true),
                ..LocalServer::default()
            });
            assert!(
                load(Some(&manifest), &RunArgs::default())
                    .unwrap()
                    .local_observability
                    .enabled
            );

            jail.create_file("golem.toml", "[local_observability]\nenabled = false\n")?;
            let file_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                ..RunArgs::default()
            };
            assert!(
                !load(Some(&manifest), &file_args)
                    .unwrap()
                    .local_observability
                    .enabled
            );

            jail.set_env("GOLEM__LOCAL_OBSERVABILITY__ENABLED", "true");
            assert!(
                load(Some(&manifest), &file_args)
                    .unwrap()
                    .local_observability
                    .enabled
            );

            let cli_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                observability: Some(false),
                ..RunArgs::default()
            };
            assert!(
                !load(Some(&manifest), &cli_args)
                    .unwrap()
                    .local_observability
                    .enabled
            );
            Ok(())
        });
    }

    #[test]
    fn local_observability_rejects_non_local_router_and_token_settings() {
        Jail::expect_with(|jail| {
            let manifest = local_server(LocalServer {
                observability: Some(true),
                ..LocalServer::default()
            });

            jail.create_file(
                "management-host.toml",
                "[router]\nmanagement_host = \"api.example.com\"\n",
            )?;
            let management_host_error = jail_error(load(
                Some(&manifest),
                &RunArgs {
                    config: Some(PathBuf::from("management-host.toml")),
                    ..RunArgs::default()
                },
            ));
            assert!(
                management_host_error.contains("conventional local management router"),
                "{management_host_error}"
            );

            jail.create_file(
                "other-token.toml",
                "[registry_service.initial_accounts.root]\ntoken = \"00000000-0000-0000-0000-000000000001\"\n",
            )?;
            let token_error = jail_error(load(
                Some(&manifest),
                &RunArgs {
                    config: Some(PathBuf::from("other-token.toml")),
                    ..RunArgs::default()
                },
            ));
            assert!(
                token_error.contains("well-known local admin token"),
                "{token_error}"
            );
            Ok(())
        });
    }

    #[test]
    fn filesystem_settings_follow_the_platform_and_the_layers() {
        for platform in [LocalPlatform::MacOs, LocalPlatform::Other] {
            Jail::expect_with(|jail| {
                let data_dir = jail.directory().join("data");
                let args = RunArgs {
                    data_dir: Some(data_dir.clone()),
                    ..RunArgs::default()
                };
                let mode = |manifest: Option<&ResolvedLocalServer>, args: &RunArgs| {
                    load_on(
                        platform,
                        manifest,
                        args,
                        SnapshotRepositoryKeySource::ValidationOnly,
                    )
                    .unwrap()
                    .worker_executor
                    .filesystem_storage
                    .mode
                };

                let defaults = load_on(
                    platform,
                    None,
                    &args,
                    SnapshotRepositoryKeySource::ValidationOnly,
                )
                .unwrap();
                match platform {
                    LocalPlatform::MacOs => {
                        assert_eq!(
                            defaults.worker_executor.filesystem_storage.mode,
                            FilesystemStorageMode::Apfs {
                                root: data_dir.join("agents").into()
                            }
                        );
                        assert_eq!(snapshot_store(&defaults).repository_key().bytes(), &[0; 64]);
                    }
                    LocalPlatform::Other => {
                        assert_eq!(
                            defaults.worker_executor.filesystem_storage.mode,
                            FilesystemStorageMode::Temporary
                        );
                        assert!(matches!(
                            defaults.worker_executor.filesystem_snapshots,
                            FilesystemSnapshotsConfig::Disabled(_)
                        ));
                    }
                }

                let manifest = local_server(LocalServer {
                    agent_filesystem_root: Some(PathBuf::from("manifest-agents")),
                    ..LocalServer::default()
                });
                assert_eq!(
                    mode(Some(&manifest), &args),
                    platform.rooted_mode(Path::new("/tmp/test-app/manifest-agents"))
                );

                let flag_args = RunArgs {
                    agent_filesystem_root: Some(PathBuf::from("flag-agents")),
                    data_dir: Some(data_dir.clone()),
                    ..RunArgs::default()
                };
                assert_eq!(
                    mode(Some(&manifest), &flag_args),
                    platform.rooted_mode(Path::new("flag-agents"))
                );

                jail.create_file(
                    "golem.toml",
                    r#"
                        [worker_executor.filesystem_storage.mode]
                        type = "Directory"

                        [worker_executor.filesystem_storage.mode.config]
                        root = "file-agents"
                    "#,
                )?;
                let file_args = RunArgs {
                    config: Some(PathBuf::from("golem.toml")),
                    data_dir: Some(data_dir.clone()),
                    ..RunArgs::default()
                };
                assert_eq!(
                    mode(Some(&manifest), &file_args),
                    FilesystemStorageMode::Directory {
                        root: Path::new("file-agents").into()
                    }
                );

                jail.set_env(
                    "GOLEM__WORKER_EXECUTOR__FILESYSTEM_STORAGE__MODE__CONFIG__ROOT",
                    "env-agents",
                );
                assert_eq!(
                    mode(Some(&manifest), &file_args),
                    FilesystemStorageMode::Directory {
                        root: Path::new("env-agents").into()
                    }
                );

                let file_and_flag_args = RunArgs {
                    agent_filesystem_root: Some(PathBuf::from("flag-agents")),
                    config: Some(PathBuf::from("golem.toml")),
                    data_dir: Some(data_dir.clone()),
                    ..RunArgs::default()
                };
                assert_eq!(
                    mode(Some(&manifest), &file_and_flag_args),
                    platform.rooted_mode(Path::new("flag-agents"))
                );

                assert!(!data_dir.exists());
                Ok(())
            });
        }
    }

    #[test]
    fn environment_can_disable_storage_and_snapshots_on_macos() {
        Jail::expect_with(|jail| {
            let data_dir = jail.directory().join("data");
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__FILESYSTEM_STORAGE__MODE__TYPE",
                "Temporary",
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__FILESYSTEM_SNAPSHOTS__TYPE",
                "Disabled",
            );
            let args = RunArgs {
                data_dir: Some(data_dir.clone()),
                ..RunArgs::default()
            };

            let config = load_on(
                LocalPlatform::MacOs,
                None,
                &args,
                SnapshotRepositoryKeySource::DataDir,
            )
            .unwrap();

            assert_eq!(
                config.worker_executor.filesystem_storage.mode,
                FilesystemStorageMode::Temporary
            );
            assert!(matches!(
                config.worker_executor.filesystem_snapshots,
                FilesystemSnapshotsConfig::Disabled(_)
            ));
            assert!(!data_dir.exists());
            Ok(())
        });
    }

    #[test]
    fn temporary_storage_rejects_a_root() {
        Jail::expect_with(|jail| {
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__FILESYSTEM_STORAGE__MODE__TYPE",
                "Temporary",
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__FILESYSTEM_STORAGE__MODE__CONFIG__ROOT",
                "agents",
            );
            assert!(load(None, &run_args_with_data_dir("/data")).is_err());
            Ok(())
        });
    }

    #[test]
    fn enabled_snapshots_without_a_key_use_the_key_retained_in_the_data_dir() {
        Jail::expect_with(|jail| {
            let data_dir = jail.directory().join("data");
            jail.create_file(
                "golem.toml",
                r#"
                    [worker_executor.filesystem_snapshots]
                    type = "Enabled"
                "#,
            )?;
            let args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                data_dir: Some(data_dir.clone()),
                ..RunArgs::default()
            };
            let key_file = data_dir.join(filesystem_config::KEY_FILE);

            let validated = load_on(
                LocalPlatform::Other,
                None,
                &args,
                SnapshotRepositoryKeySource::ValidationOnly,
            )
            .unwrap();
            assert_eq!(
                snapshot_store(&validated).repository_key().bytes(),
                &[0; 64]
            );
            assert!(!data_dir.exists());

            let first = load_on(
                LocalPlatform::Other,
                None,
                &args,
                SnapshotRepositoryKeySource::DataDir,
            )
            .unwrap();
            let retained = golem_worker_executor::services::golem_config::FilesystemSnapshotRepositoryKey::parse(
                &std::fs::read_to_string(&key_file).unwrap(),
            )
            .unwrap();
            assert_eq!(
                snapshot_store(&first).repository_key().bytes(),
                retained.bytes()
            );

            let second = load_on(
                LocalPlatform::Other,
                None,
                &args,
                SnapshotRepositoryKeySource::DataDir,
            )
            .unwrap();
            assert_eq!(
                snapshot_store(&second).repository_key().bytes(),
                retained.bytes()
            );
            Ok(())
        });
    }

    #[test]
    fn a_configured_repository_key_leaves_the_retained_key_alone() {
        Jail::expect_with(|jail| {
            let data_dir = jail.directory().join("data");
            std::fs::create_dir_all(&data_dir).unwrap();
            let key_file = data_dir.join(filesystem_config::KEY_FILE);
            std::fs::write(&key_file, b"corrupt").unwrap();
            jail.create_file(
                "golem.toml",
                &format!(
                    r#"
                        [worker_executor.filesystem_snapshots]
                        type = "Enabled"

                        [worker_executor.filesystem_snapshots.config]
                        repository_key = "{}"
                        save_threads = 2
                    "#,
                    "2a".repeat(64)
                ),
            )?;
            let args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                data_dir: Some(data_dir.clone()),
                ..RunArgs::default()
            };
            let load_with_key = || {
                load_on(
                    LocalPlatform::Other,
                    None,
                    &args,
                    SnapshotRepositoryKeySource::DataDir,
                )
                .unwrap()
            };

            let from_file = load_with_key();
            assert_eq!(
                snapshot_store(&from_file).repository_key().bytes(),
                &[0x2a; 64]
            );

            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__FILESYSTEM_SNAPSHOTS__CONFIG__REPOSITORY_KEY",
                "3b".repeat(64),
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__FILESYSTEM_SNAPSHOTS__CONFIG__SAVE_THREADS",
                "3",
            );
            let from_env = load_with_key();
            let store = snapshot_store(&from_env);
            assert_eq!(store.repository_key().bytes(), &[0x3b; 64]);
            assert_eq!(store.save_threads().get(), 3);

            assert!(std::fs::read(&key_file).unwrap() == b"corrupt");
            Ok(())
        });
    }

    #[test]
    fn invalid_settings_do_not_create_the_retained_key() {
        for settings in [
            r#"
                [worker_executor.filesystem_snapshots]
                type = "Enabled"

                [worker_executor.filesystem_snapshots.config]
                save_threads = 0
            "#,
            r#"
                [worker_executor.filesystem_snapshots]
                type = "Enabled"

                [worker_executor.filesystem_storage.mode]
                type = "Directory"
            "#,
        ] {
            Jail::expect_with(|jail| {
                let data_dir = jail.directory().join("data");
                jail.create_file("golem.toml", settings)?;
                let args = RunArgs {
                    config: Some(PathBuf::from("golem.toml")),
                    data_dir: Some(data_dir.clone()),
                    ..RunArgs::default()
                };

                assert!(
                    load_on(
                        LocalPlatform::Other,
                        None,
                        &args,
                        SnapshotRepositoryKeySource::DataDir,
                    )
                    .is_err()
                );
                assert!(!data_dir.exists());
                Ok(())
            });
        }
    }

    #[test]
    fn defaults_keep_all_state_in_the_data_dir() {
        let data_dir = Path::new("/data");
        let config = GolemServerConfig::for_data_dir(data_dir).unwrap();

        assert_eq!(config.router.address, "127.0.0.1");
        assert_eq!(config.router.port, DEFAULT_LOCAL_ROUTER_PORT);
        assert!(config.uses_local_registry_db());
        let DbConfig::Sqlite(registry_db) = &config.registry_service.db else {
            panic!("expected a SQLite registry database")
        };
        assert_eq!(sqlite_path(registry_db), data_dir.join("registry.db"));
        let PersistenceConfig::Sqlite(shard_manager_db) = &config.shard_manager.persistence else {
            panic!("expected a SQLite shard manager database")
        };
        assert_eq!(
            sqlite_path(shard_manager_db),
            data_dir.join("shard_manager.db")
        );
        let SchedulerStorageConfig::Sqlite(scheduler_db) =
            &config.worker_executor.scheduler_storage
        else {
            panic!("expected a SQLite scheduler database")
        };
        assert_eq!(sqlite_path(scheduler_db), data_dir.join("scheduler.db"));
        let SessionStoreConfig::Sqlite(sessions) = &config.worker_service.gateway_session_storage
        else {
            panic!("expected a SQLite session store")
        };
        assert_eq!(
            sqlite_path(&sessions.sqlite_config),
            data_dir.join("gateway-sessions.db")
        );
        let KeyValueStorageConfig::MultiSqlite(key_value) =
            &config.worker_executor.key_value_storage
        else {
            panic!("expected one SQLite database per agent")
        };
        assert_eq!(key_value.root_dir, data_dir.join("kv-store"));
        assert!(matches!(
            config.worker_executor.indexed_storage,
            IndexedStorageConfig::KVStoreMultiSqlite(_)
        ));
        for blob_storage in [
            &config.registry_service.blob_storage,
            &config.worker_service.blob_storage,
            &config.worker_executor.blob_storage,
            &config.component_compilation_service.blob_storage,
        ] {
            assert_eq!(blob_root(blob_storage), data_dir.join("blobs"));
        }
        assert_eq!(
            config.registry_service.builtin_artifacts.cache_dir,
            Some(data_dir.join("builtin-artifacts"))
        );
        assert!(matches!(
            config.registry_service.login,
            LoginConfig::Disabled(_)
        ));
        assert_eq!(
            config.registry_service.initial_accounts["root"]
                .token
                .as_ref()
                .map(|token| token.secret()),
            Some(golem_client::LOCAL_WELL_KNOWN_TOKEN)
        );
        assert_eq!(config.worker_executor.memory.worker_memory_ratio, 0.8);
        assert!(config.worker_executor.memory.enable_measured_admission);
    }

    #[test]
    fn manifest_local_server_values_are_used_when_cli_args_are_absent() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            let manifest = local_server(LocalServer {
                system_memory_override: std::num::NonZeroU64::new(2147483648),
                router_addr: Some("127.0.0.2".to_string()),
                router_port: Some(9882),
                custom_request_port: Some(9008),
                mcp_port: Some(9009),
                ports_file: Some(PathBuf::from("/tmp/test-app/.golem/ports.json")),
                data_dir: Some(PathBuf::from("/tmp/test-app/.golem/data")),
                observability: None,
                agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
            });

            let config = load(Some(&manifest), &RunArgs::default()).unwrap();

            assert_eq!(config.router.address, "127.0.0.2");
            assert_eq!(config.router.port, 9882);
            assert_eq!(config.worker_service.custom_request_port, 9008);
            assert_eq!(config.worker_service.mcp_port, 9009);
            assert_eq!(
                config.worker_executor.memory.system_memory_override,
                Some(2147483648)
            );
            assert_eq!(
                config.ports_file,
                Some(PathBuf::from("/tmp/test-app/.golem/ports.json"))
            );
            assert_eq!(config.data_dir, PathBuf::from("/tmp/test-app/.golem/data"));
            assert_eq!(
                blob_root(&config.worker_executor.blob_storage),
                Path::new("/tmp/test-app/.golem/data/blobs")
            );
            assert_eq!(
                config.worker_executor.filesystem_storage.mode,
                FilesystemStorageMode::Directory {
                    root: Path::new("/tmp/test-app/.golem/agents").into()
                }
            );
            Ok(())
        });
    }

    #[test]
    fn cli_args_override_manifest_local_server_values() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            let manifest = local_server(LocalServer {
                system_memory_override: std::num::NonZeroU64::new(2147483648),
                router_addr: Some("127.0.0.2".to_string()),
                router_port: Some(9882),
                custom_request_port: Some(9008),
                mcp_port: Some(9009),
                ports_file: Some(PathBuf::from("/tmp/test-app/.golem/ports.json")),
                data_dir: Some(PathBuf::from("/tmp/test-app/.golem/data")),
                observability: None,
                agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
            });
            let run_args = RunArgs {
                config: None,
                system_memory_override: std::num::NonZeroU64::new(1073741824),
                router_addr: Some("0.0.0.0".to_string()),
                router_port: Some(10000),
                custom_request_port: Some(10001),
                mcp_port: Some(10002),
                ports_file: Some(PathBuf::from("cli-ports.json")),
                data_dir: Some(PathBuf::from("cli-data")),
                observability: None,
                clean: false,
                agent_filesystem_root: Some(PathBuf::from("cli-agents")),
            };

            let config = load(Some(&manifest), &run_args).unwrap();

            assert_eq!(config.router.address, "0.0.0.0");
            assert_eq!(config.router.port, 10000);
            assert_eq!(config.worker_service.custom_request_port, 10001);
            assert_eq!(config.worker_service.mcp_port, 10002);
            assert_eq!(
                config.worker_executor.memory.system_memory_override,
                Some(1073741824)
            );
            assert_eq!(config.ports_file, Some(PathBuf::from("cli-ports.json")));
            assert_eq!(config.data_dir, PathBuf::from("cli-data"));
            assert_eq!(
                config.worker_executor.filesystem_storage.mode,
                FilesystemStorageMode::Directory {
                    root: Path::new("cli-agents").into()
                }
            );
            Ok(())
        });
    }

    #[test]
    fn layers_apply_in_order_of_precedence() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "golem.toml",
                r#"
                    data_dir = "/file-data"

                    [router]
                    address = "127.0.0.3"
                    port = 7000

                    [worker_service]
                    custom_request_port = 7001
                    mcp_port = 7002
                "#,
            )?;
            jail.set_env("GOLEM__ROUTER__PORT", "8000");
            jail.set_env("GOLEM__WORKER_SERVICE__MCP_PORT", "8002");
            let manifest = local_server(LocalServer {
                router_addr: Some("127.0.0.2".to_string()),
                router_port: Some(6000),
                custom_request_port: Some(6001),
                mcp_port: Some(6002),
                data_dir: Some(PathBuf::from("/manifest-data")),
                ..LocalServer::default()
            });
            let run_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                mcp_port: Some(9002),
                ..RunArgs::default()
            };

            let config = load(Some(&manifest), &run_args).unwrap();

            assert_eq!(config.data_dir, PathBuf::from("/file-data"));
            assert_eq!(
                blob_root(&config.registry_service.blob_storage),
                Path::new("/file-data/blobs")
            );
            assert_eq!(config.router.address, "127.0.0.3");
            assert_eq!(config.router.port, 8000);
            assert_eq!(config.worker_service.custom_request_port, 7001);
            assert_eq!(config.worker_service.mcp_port, 9002);
            Ok(())
        });
    }

    #[test]
    fn missing_config_file_is_an_error() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            let run_args = RunArgs {
                config: Some(PathBuf::from("/nonexistent/golem.toml")),
                ..run_args_with_data_dir("/data")
            };

            let error = jail_error(load(None, &run_args));

            assert!(error.contains("/nonexistent/golem.toml"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn service_sections_override_individual_fields() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "golem.toml",
                r#"
                    [registry_service.initial_plans.default]
                    component_limit = 25

                    [registry_service.initial_accounts.root]
                    token = "a-secret-admin-token-of-sufficient-length"

                    [worker_executor.memory]
                    system_memory_override = 4294967296
                "#,
            )?;
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__RESOURCE_USAGE_METERING__COMPUTE",
                "true",
            );
            let run_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                ..run_args_with_data_dir("/data")
            };

            let config = load(None, &run_args).unwrap();

            let plan = &config.registry_service.initial_plans["default"];
            assert_eq!(plan.component_limit, 25);
            assert_eq!(plan.app_limit, UNLIMITED);
            assert_eq!(plan.plan_id, local_plan_id());
            let root = &config.registry_service.initial_accounts["root"];
            assert_eq!(
                root.token.as_ref().map(|token| token.secret()),
                Some("a-secret-admin-token-of-sufficient-length")
            );
            assert_eq!(root.email, AccountEmail::new("initial@user"));
            assert_eq!(config.registry_service.initial_accounts.len(), 3);
            assert_eq!(
                config.worker_executor.memory.system_memory_override,
                Some(4294967296)
            );
            assert!(config.worker_executor.resource_usage_metering.compute);
            assert!(!config.worker_executor.resource_usage_metering.memory);
            Ok(())
        });
    }

    #[test]
    fn selecting_another_variant_drops_the_fields_of_the_default_variant() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "golem.toml",
                r#"
                    [worker_executor.key_value_storage]
                    type = "Postgres"

                    [worker_executor.key_value_storage.config]
                    host = "db"
                    port = 5432
                    database = "golem"
                    username = "golem"
                    password = "golem"
                    max_connections = 20
                    schema = "golem_worker_executor"
                "#,
            )?;
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__SCHEDULER_STORAGE__TYPE",
                "Postgres",
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__HOST",
                "db",
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__PORT",
                "5432",
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__USERNAME",
                "golem",
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__PASSWORD",
                "golem",
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__MAX_CONNECTIONS",
                "5",
            );
            let run_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                ..run_args_with_data_dir("/data")
            };

            // The scheduler's default SQLite variant has a `database` field too, so without
            // dropping it the file path would become the Postgres database name.
            let error = jail_error(load(None, &run_args));
            assert!(error.contains("database"), "{error}");

            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__DATABASE",
                "golem",
            );
            let config = load(None, &run_args).unwrap();

            let KeyValueStorageConfig::Postgres(key_value) =
                &config.worker_executor.key_value_storage
            else {
                panic!("expected Postgres key-value storage")
            };
            assert_eq!(key_value.postgres.host, "db");
            assert_eq!(key_value.postgres.max_connections, 20);
            let SchedulerStorageConfig::Postgres(scheduler) =
                &config.worker_executor.scheduler_storage
            else {
                panic!("expected Postgres scheduler storage")
            };
            assert_eq!(scheduler.postgres.database, "golem");
            assert_eq!(scheduler.postgres.schema, None);
            Ok(())
        });
    }

    #[test]
    fn launcher_owned_keys_are_rejected() {
        Jail::expect_with(|jail| {
            jail.set_env("GOLEM__WORKER_EXECUTOR__GRPC__PORT", "9000");
            let error = jail_error(load(None, &run_args_with_data_dir("/data")));
            assert!(error.contains("worker_executor.grpc.port"), "{error}");
            assert!(
                error.contains("GOLEM__WORKER_EXECUTOR__GRPC__PORT"),
                "{error}"
            );
            Ok(())
        });
        Jail::expect_with(|jail| {
            jail.create_file(
                "golem.toml",
                r#"
                    [registry_service.tracing.stdout]
                    enabled = false
                "#,
            )?;
            let run_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                ..run_args_with_data_dir("/data")
            };
            let error = jail_error(load(None, &run_args));
            assert!(error.contains("registry_service.tracing"), "{error}");
            assert!(error.contains("golem.toml"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn unknown_top_level_keys_are_rejected() {
        Jail::expect_with(|jail| {
            jail.set_env("GOLEM__RESOURCE_USAGE_METERING__COMPUTE", "true");
            let error = jail_error(load(None, &run_args_with_data_dir("/data")));
            assert!(error.contains("resource_usage_metering"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn postgres_example_uses_the_schemas_of_the_per_service_deployment() {
        let compose = include_str!("../../../docker-examples/published-postgres/compose.yaml");
        let config = GolemServerConfig::postgres_s3_example(Path::new("/data")).unwrap();

        let DbConfig::Postgres(registry) = &config.registry_service.db else {
            panic!("expected a Postgres registry database")
        };
        let PersistenceConfig::Postgres(shard_manager) = &config.shard_manager.persistence else {
            panic!("expected a Postgres shard manager database")
        };
        let KeyValueStorageConfig::Postgres(key_value) = &config.worker_executor.key_value_storage
        else {
            panic!("expected Postgres key-value storage")
        };
        let IndexedStorageConfig::Postgres(indexed) = &config.worker_executor.indexed_storage
        else {
            panic!("expected Postgres indexed storage")
        };
        let SchedulerStorageConfig::Postgres(scheduler) = &config.worker_executor.scheduler_storage
        else {
            panic!("expected Postgres scheduler storage")
        };

        for (variable, postgres) in [
            ("GOLEM__DB__CONFIG__SCHEMA", registry),
            ("GOLEM__PERSISTENCE__CONFIG__SCHEMA", shard_manager),
            (
                "GOLEM__KEY_VALUE_STORAGE__CONFIG__SCHEMA",
                &key_value.postgres,
            ),
            ("GOLEM__INDEXED_STORAGE__CONFIG__SCHEMA", &indexed.postgres),
            (
                "GOLEM__SCHEDULER_STORAGE__CONFIG__SCHEMA",
                &scheduler.postgres,
            ),
        ] {
            let schema = postgres.schema.as_deref().unwrap();
            assert!(
                compose.contains(&format!("{variable}: {schema}")),
                "{variable}: {schema} is not in the per-service compose file"
            );
        }
        for blob_storage in [
            &config.registry_service.blob_storage,
            &config.worker_service.blob_storage,
            &config.worker_executor.blob_storage,
            &config.component_compilation_service.blob_storage,
        ] {
            let BlobStorageConfig::S3(s3) = blob_storage else {
                panic!("expected S3 blob storage")
            };
            assert!(s3.namespace_prefixes);
            s3.validate().unwrap();
            let mut buckets = vec![
                &s3.compilation_cache_bucket,
                &s3.custom_data_bucket,
                &s3.oplog_payload_bucket,
                &s3.initial_agent_files_bucket,
                &s3.components_bucket,
                &s3.filesystem_snapshots_bucket,
            ];
            buckets.extend(&s3.compressed_oplog_buckets);
            assert!(buckets.iter().all(|bucket| *bucket == EXAMPLE_S3_BUCKET));
        }
        assert!(!config.uses_local_registry_db());
    }

    #[test]
    fn reference_config_is_a_valid_config_file() {
        for make in [
            GolemServerConfig::for_data_dir as fn(&Path) -> anyhow::Result<GolemServerConfig>,
            GolemServerConfig::postgres_s3_example,
        ] {
            let reference = toml::to_string(&configurable_value(make).unwrap()).unwrap();
            for data_dir in [REFERENCE_DATA_DIR, "/moved"] {
                let reference = reference.replace(
                    &format!("data_dir = \"{REFERENCE_DATA_DIR}\""),
                    &format!("data_dir = \"{data_dir}\""),
                );
                Jail::expect_with(|jail| {
                    jail.create_file("golem.toml", &reference)?;
                    let run_args = RunArgs {
                        config: Some(PathBuf::from("golem.toml")),
                        ..RunArgs::default()
                    };

                    let loaded = load(None, &run_args).unwrap();

                    assert_eq!(
                        serde_json::to_value(&loaded).unwrap(),
                        serde_json::to_value(make(Path::new(data_dir)).unwrap()).unwrap()
                    );
                    Ok(())
                });
            }
        }
    }

    #[test]
    fn selecting_a_variant_without_fields_needs_no_config() {
        Jail::expect_with(|jail| {
            jail.set_env(
                "GOLEM__REGISTRY_SERVICE__COMPONENT_COMPILATION__TYPE",
                "Disabled",
            );
            jail.set_env("GOLEM__REGISTRY_SERVICE__BUILTIN_PLUGINS__TYPE", "Disabled");
            let config = load(None, &run_args_with_data_dir("/data")).unwrap();
            assert!(matches!(
                config.registry_service.component_compilation,
                ComponentCompilationConfig::Disabled(_)
            ));
            assert!(matches!(
                config.registry_service.builtin_plugins,
                BuiltinPluginsConfig::Disabled(_)
            ));
            Ok(())
        });
    }

    #[test]
    fn selecting_a_unit_variant_over_a_lower_layer_drops_its_config() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "golem.toml",
                r#"
                    [worker_executor.filesystem_storage.mode]
                    type = "Temporary"
                "#,
            )?;
            let manifest = local_server(LocalServer {
                agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
                ..LocalServer::default()
            });
            let run_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                ..run_args_with_data_dir("/data")
            };

            let config = load(Some(&manifest), &run_args).unwrap();

            assert_eq!(
                config.worker_executor.filesystem_storage.mode,
                FilesystemStorageMode::Temporary
            );
            Ok(())
        });
    }

    #[test]
    fn variant_fields_do_not_leak_between_user_layers() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "golem.toml",
                r#"
                    [worker_executor.key_value_storage]
                    type = "Postgres"

                    [worker_executor.key_value_storage.config]
                    host = "db"
                    port = 5432
                    database = "golem"
                    username = "golem"
                    password = "golem"
                    max_connections = 77
                    schema = "golem_worker_executor"
                "#,
            )?;
            jail.set_env("GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__TYPE", "Sqlite");
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__DATABASE",
                "/data/kv.db",
            );
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__FOREIGN_KEYS",
                "false",
            );
            let run_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                ..run_args_with_data_dir("/data")
            };

            let error = jail_error(load(None, &run_args));
            assert!(error.contains("max_connections"), "{error}");

            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__MAX_CONNECTIONS",
                "3",
            );
            let config = load(None, &run_args).unwrap();
            let KeyValueStorageConfig::Sqlite(sqlite) = &config.worker_executor.key_value_storage
            else {
                panic!("expected SQLite key-value storage")
            };
            assert_eq!(sqlite.max_connections, 3);
            assert_eq!(sqlite.database, "/data/kv.db");
            Ok(())
        });
    }

    #[test]
    fn the_session_store_counts_with_its_default_schema() {
        let mut config = GolemServerConfig::postgres_s3_example(Path::new("/data")).unwrap();
        let SessionStoreConfig::Postgres(sessions) =
            &mut config.worker_service.gateway_session_storage
        else {
            panic!("expected a Postgres session store")
        };
        sessions.postgres_config.schema = None;
        assert!(validate(&config).is_ok());

        let DbConfig::Postgres(registry) = &mut config.registry_service.db else {
            panic!("expected a Postgres registry database")
        };
        registry.schema = Some(DEFAULT_POSTGRES_SESSION_STORE_SCHEMA.to_string());
        let error = validate(&config).unwrap_err().to_string();
        assert!(
            error.contains("worker_service.gateway_session_storage"),
            "{error}"
        );
        assert!(
            error.contains(DEFAULT_POSTGRES_SESSION_STORE_SCHEMA),
            "{error}"
        );
    }

    #[test]
    fn postgres_stores_need_separate_schemas() {
        Jail::expect_with(|jail| {
            let postgres = |section: &str| {
                format!(
                    r#"
                        [{section}]
                        type = "Postgres"

                        [{section}.config]
                        host = "db"
                        port = 5432
                        database = "golem"
                        username = "golem"
                        password = "golem"
                        max_connections = 10
                    "#
                )
            };
            jail.create_file(
                "golem.toml",
                &format!(
                    "{}{}",
                    postgres("registry_service.db"),
                    postgres("shard_manager.persistence")
                ),
            )?;
            let run_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                ..run_args_with_data_dir("/data")
            };

            let error = jail_error(load(None, &run_args));

            assert!(error.contains("registry_service.db"), "{error}");
            assert!(error.contains("shard_manager.persistence"), "{error}");
            assert!(error.contains("public"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn webhook_keys_must_match() {
        Jail::expect_with(|jail| {
            jail.set_env(
                "GOLEM__WORKER_SERVICE__WEBHOOK_CALLBACK_HANDLER__HMAC_KEY",
                "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
            );
            let error = jail_error(load(None, &run_args_with_data_dir("/data")));
            assert!(
                error.contains("worker_executor.agent_webhooks_service.hmac_key"),
                "{error}"
            );
            Ok(())
        });
    }

    #[test]
    fn the_management_host_is_reserved() {
        Jail::expect_with(|jail| {
            jail.set_env(
                "GOLEM__REGISTRY_SERVICE__DOMAIN_REGISTRATION__RESERVED_DOMAINS",
                r#"["status.golem.example.com"]"#,
            );
            jail.set_env("GOLEM__ROUTER__MANAGEMENT_HOST", "api.golem.example.com");
            let config = load(None, &run_args_with_data_dir("/data")).unwrap();
            assert_eq!(
                config.router.management_host.as_deref(),
                Some("api.golem.example.com")
            );
            assert_eq!(
                config.reserved_domains(),
                ["status.golem.example.com", "api.golem.example.com"]
            );
            Ok(())
        });

        let config = GolemServerConfig::for_data_dir(Path::new("/data")).unwrap();
        assert!(config.reserved_domains().is_empty());
    }

    #[test]
    fn invalid_management_host_is_rejected() {
        Jail::expect_with(|jail| {
            jail.set_env(
                "GOLEM__ROUTER__MANAGEMENT_HOST",
                "https://api.golem.example.com",
            );
            let error = jail_error(load(None, &run_args_with_data_dir("/data")));
            assert!(error.contains("router.management_host"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn unknown_router_fields_are_rejected() {
        Jail::expect_with(|jail| {
            jail.set_env("GOLEM__ROUTER__ADRESS", "0.0.0.0");
            let error = jail_error(load(None, &run_args_with_data_dir("/data")));
            assert!(error.contains("adress"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn selecting_the_default_variant_again_restores_its_fields() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "golem.toml",
                r#"
                    [worker_executor.key_value_storage]
                    type = "Postgres"

                    [worker_executor.key_value_storage.config]
                    host = "db"
                    port = 5432
                    database = "golem"
                    username = "golem"
                    password = "golem"
                    max_connections = 10
                    schema = "golem_worker_executor"
                "#,
            )?;
            jail.set_env(
                "GOLEM__WORKER_EXECUTOR__KEY_VALUE_STORAGE__TYPE",
                "MultiSqlite",
            );
            let run_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                ..run_args_with_data_dir("/data")
            };

            let config = load(None, &run_args).unwrap();

            let KeyValueStorageConfig::MultiSqlite(key_value) =
                &config.worker_executor.key_value_storage
            else {
                panic!("expected one SQLite database per agent")
            };
            assert_eq!(key_value.root_dir, Path::new("/data/kv-store"));
            assert_eq!(key_value.max_connections, 4);
            Ok(())
        });
    }

    #[test]
    fn state_location_is_reported_per_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let data_dir = temp_dir.path().join("data");
        let clean_root = std::fs::canonicalize(temp_dir.path()).unwrap().join("data");

        let local = GolemServerConfig::for_data_dir(&data_dir).unwrap();
        assert!(local.state_outside(&clean_root).is_empty());

        let remote = GolemServerConfig::postgres_s3_example(&data_dir).unwrap();
        assert_eq!(
            remote.state_outside(&clean_root),
            vec![
                "registry_service.db",
                "registry_service.blob_storage",
                "shard_manager.persistence",
                "worker_service.gateway_session_storage",
                "worker_service.blob_storage",
                "worker_executor.key_value_storage",
                "worker_executor.indexed_storage",
                "worker_executor.scheduler_storage",
                "worker_executor.blob_storage",
                "component_compilation_service.blob_storage",
            ]
        );

        let mut moved_scheduler = local.clone();
        moved_scheduler.worker_executor.scheduler_storage =
            sqlite_scheduler(temp_dir.path().join("elsewhere/scheduler.db"));
        assert_eq!(
            moved_scheduler.state_outside(&clean_root),
            vec!["worker_executor.scheduler_storage"]
        );

        let local_key_value =
            KeyValueStorageInnerConfig::MultiSqlite(KeyValueStorageMultiSqliteConfig {
                root_dir: data_dir.join("kv-store"),
                max_connections: 4,
                foreign_keys: false,
            });
        let mut routed = local.clone();
        routed.worker_executor.key_value_storage = KeyValueStorageConfig::NamespaceRouted(
            golem_worker_executor::services::golem_config::KeyValueStorageNamespaceRoutedConfig {
                cache: local_key_value.clone(),
                persistent: local_key_value,
            },
        );
        assert!(routed.state_outside(&clean_root).is_empty());
        let KeyValueStorageConfig::NamespaceRouted(routed_key_value) =
            &mut routed.worker_executor.key_value_storage
        else {
            unreachable!()
        };
        routed_key_value.persistent =
            KeyValueStorageInnerConfig::Redis(golem_common::config::RedisConfig::default());
        assert_eq!(
            routed.state_outside(&clean_root),
            vec!["worker_executor.key_value_storage"]
        );
    }

    fn sqlite_scheduler(database: PathBuf) -> SchedulerStorageConfig {
        SchedulerStorageConfig::Sqlite(DbSqliteConfig {
            database: database.to_string_lossy().into_owned(),
            max_connections: 4,
            foreign_keys: false,
        })
    }

    #[test]
    fn parent_components_that_leave_the_data_directory_are_outside() {
        let temp_dir = tempfile::tempdir().unwrap();
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let clean_root = std::fs::canonicalize(&data_dir).unwrap();
        let mut config = GolemServerConfig::for_data_dir(&data_dir).unwrap();

        config.worker_executor.scheduler_storage =
            sqlite_scheduler(data_dir.join("../external/scheduler.db"));
        assert_eq!(
            config.state_outside(&clean_root),
            vec!["worker_executor.scheduler_storage"]
        );

        config.worker_executor.scheduler_storage =
            sqlite_scheduler(data_dir.join("not-created/../scheduler.db"));
        assert!(config.state_outside(&clean_root).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_that_leave_the_data_directory_are_outside() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(temp_dir.path()).unwrap();
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        let data_dir = base.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        symlink(&elsewhere, data_dir.join("kv-store")).unwrap();
        symlink(
            elsewhere.join("scheduler.db"),
            data_dir.join("scheduler.db"),
        )
        .unwrap();
        let config = GolemServerConfig::for_data_dir(&data_dir).unwrap();
        assert_eq!(
            config.state_outside(&data_dir),
            vec![
                "worker_executor.key_value_storage",
                "worker_executor.scheduler_storage",
            ]
        );

        let linked_data_dir = base.join("linked-data");
        symlink(base.join("real-data"), &linked_data_dir).unwrap();
        let config = GolemServerConfig::for_data_dir(&linked_data_dir).unwrap();
        let outside = config.state_outside(&linked_data_dir);
        assert!(outside.contains(&"registry_service.db"), "{outside:?}");
        assert_eq!(outside, config.state_outside(&base.join("unrelated")));
    }

    #[test]
    fn every_listener_binds_loopback_by_default() {
        for make in [
            GolemServerConfig::for_data_dir as fn(&Path) -> anyhow::Result<GolemServerConfig>,
            GolemServerConfig::postgres_s3_example,
        ] {
            let config = make(Path::new("/data")).unwrap();
            let addresses = config.listener_addresses();
            assert_eq!(
                addresses.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
                vec![
                    "router.address",
                    "registry_service.bind_address",
                    "shard_manager.bind_address",
                    "worker_service.bind_address",
                    "worker_executor.bind_address",
                    "component_compilation_service.bind_address",
                ]
            );
            for (key, address) in addresses {
                assert_eq!(address, "127.0.0.1", "{key}");
            }
        }
    }

    #[test]
    fn registry_wiring_is_set_by_the_launcher() {
        let config = GolemServerConfig::for_data_dir(Path::new("/data")).unwrap();
        let ComponentCompilationConfig::Enabled(compilation) =
            &config.registry_service.component_compilation
        else {
            panic!("expected component compilation to be enabled")
        };
        assert_eq!(compilation.host, INTERNAL_HOST);
    }
}
