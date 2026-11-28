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
//! executable runs. The server settings module maps supported settings and each advanced section
//! onto these service types. The defaults are the local development setup, derived from the data
//! directory. The manifest's `localServer` section overrides these defaults. Within the supported
//! and advanced channels, environment variables override the `--config` TOML file. The advanced
//! channel overrides supported settings, even when an advanced file value conflicts with a
//! supported environment value. The `server run` flags have the highest priority.

use crate::filesystem_config::{self, LocalPlatform};
use crate::registry_db_path;
use anyhow::{Context, anyhow, bail};
use figment::providers::{Format, Serialized, Toml};
use figment::value::{Dict, Map, Tag, Value};
use figment::{Figment, Metadata, Profile, Provider};
use golem_cli::command::server::RunArgs;
use golem_cli::config::{
    DEFAULT_LOCAL_CUSTOM_REQUEST_PORT, DEFAULT_LOCAL_MCP_PORT, DEFAULT_LOCAL_ROUTER_ADDR,
    DEFAULT_LOCAL_ROUTER_PORT,
};
use golem_cli::fs;
use golem_cli::model::app::ResolvedLocalServer;
use golem_common::config::{
    DbConfig, DbPostgresConfig, DbSqliteConfig, env_config_provider, server_settings,
};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::account_usage::MonthlyPlanAmounts;
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::model::domain_registration::Domain;
use golem_common::model::golem_issuer::GolemIssuer;
use golem_common::model::plan::{PlanId, PlanName};
use golem_common::model::tool::BUILTIN_TOOL_OWNER_ACCOUNT_EMAIL;
use golem_component_compilation_service::config::ServerConfig as ComponentCompilationServiceConfig;
use golem_registry_service::config::{
    BuiltinArtifactsConfig, BuiltinPluginsConfig, ComponentCompilationEnabledConfig, LoginConfig,
    PrecreatedAccount, PrecreatedPlan, RegistryServiceConfig,
};
use golem_schema::Empty;
use golem_service_base::config::{
    BlobStorageConfig, LocalFileSystemBlobStorageConfig, S3BlobStorageConfig,
};
use golem_service_base::http_server::HttpServerConfig;
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
    "registry_service.golem_issuer",
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
    "worker_service.golem_issuer",
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
    "worker_executor.golem_issuer",
    "component_compilation_service.tracing",
    "component_compilation_service.http_port",
    "component_compilation_service.grpc.port",
    "component_compilation_service.registry_service",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GolemServerConfig {
    pub data_dir: PathBuf,
    pub logging: golem_common::tracing::TracingConfig,
    /// File to write the bound ports to as JSON once the server has started.
    pub ports_file: Option<PathBuf>,
    /// The installation's Golem issuer: an https URL on its own host, or a
    /// `urn:golem:issuer:<uuid>` value that Golem generated. It goes to the registry service
    /// only: the registry keeps the issuer in its database, and the worker service and the
    /// executor ask the registry for it. Without it, the first start generates a value and stores
    /// it in the registry database, so every later start with that database uses the same one. A
    /// value that differs from the stored one stops the start. An empty value counts as not set.
    #[serde(
        default,
        deserialize_with = "golem_common::model::golem_issuer::deserialize_setting"
    )]
    pub golem_issuer: Option<GolemIssuer>,
    pub router: RouterConfig,
    pub registry_service: RegistryServiceConfig,
    pub shard_manager: ShardManagerConfig,
    pub worker_service: WorkerServiceConfig,
    pub worker_executor: WorkerExecutorConfig,
    pub component_compilation_service: ComponentCompilationServiceConfig,
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
    /// HTTP/2 settings of the listener.
    #[serde(default)]
    pub http_server: HttpServerConfig,
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
            logging: golem_common::tracing::TracingConfig::local_dev("golem"),
            ports_file: None,
            golem_issuer: None,
            router: RouterConfig {
                address: DEFAULT_LOCAL_ROUTER_ADDR.to_string(),
                port: DEFAULT_LOCAL_ROUTER_PORT,
                management_host: None,
                http_server: HttpServerConfig::default(),
            },
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
        monthly_upload_limit: UNLIMITED,
        monthly_compute_gcu: 0,
        monthly_memory_gb_seconds: 0,
        monthly_durable_storage_gb_month: 0,
        monthly_ephemeral_storage_gb_month: 0,
        overage_eligible: false,
        max_memory_per_agent: UNLIMITED,
        max_memory_per_agent_ceiling: UNLIMITED,
        max_memory_per_agent_user_configurable: true,
        max_table_elements_per_worker: UNLIMITED,
        max_storage_per_agent_enabled: false,
        max_storage_per_agent: UNLIMITED,
        max_storage_per_agent_ceiling: None,
        max_storage_per_agent_user_configurable: false,
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

/// Refuses PostgreSQL state with Litestream without creating state or a repository key.
pub fn check_litestream_config(args: &RunArgs) -> anyhow::Result<()> {
    use golem_worker_service::mcp::session::McpSessionStorageConfig;

    let config = load_server_config(None, args, SnapshotRepositoryKeySource::ValidationOnly)?;
    let key_value = match &config.worker_executor.key_value_storage {
        KeyValueStorageConfig::Postgres(_) => Some("worker_executor.key_value_storage"),
        KeyValueStorageConfig::NamespaceRouted(stores) => [
            (
                &stores.cache,
                "worker_executor.key_value_storage.config.cache",
            ),
            (
                &stores.persistent,
                "worker_executor.key_value_storage.config.persistent",
            ),
        ]
        .into_iter()
        .find_map(|(store, path)| {
            matches!(store, KeyValueStorageInnerConfig::Postgres(_)).then_some(path)
        }),
        _ => None,
    };
    let postgres = [
        matches!(config.registry_service.db, DbConfig::Postgres(_))
            .then_some("registry_service.db"),
        matches!(
            config.shard_manager.persistence,
            golem_shard_manager::config::PersistenceConfig::Postgres(_)
        )
        .then_some("shard_manager.persistence"),
        matches!(
            config.worker_service.gateway_session_storage,
            SessionStoreConfig::Postgres(_)
        )
        .then_some("worker_service.gateway_session_storage"),
        matches!(
            config
                .worker_service
                .mcp
                .session
                .effective_session_storage(&config.worker_service.gateway_session_storage),
            McpSessionStorageConfig::Postgres(_)
        )
        .then_some("worker_service.mcp.session_storage"),
        key_value,
        matches!(
            config.worker_executor.indexed_storage,
            IndexedStorageConfig::Postgres(_)
        )
        .then_some("worker_executor.indexed_storage"),
        matches!(
            config.worker_executor.scheduler_storage,
            SchedulerStorageConfig::Postgres(_)
        )
        .then_some("worker_executor.scheduler_storage"),
    ]
    .into_iter()
    .flatten()
    .next();
    if let Some(path) = postgres {
        bail!(
            "GOLEM_LITESTREAM=true cannot use PostgreSQL state selected by `{path}`; disable Litestream or select SQLite state"
        );
    }
    Ok(())
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
    let settings_start = layers.len();
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
    layers.push(Layer::read(
        Metadata::named("the environment variables"),
        env,
    )?);
    let settings_end = layers.len();
    layers.push(Layer::read(
        Metadata::named("the `server run` flags"),
        LocalServerSettings::from_run_args(args).into_figment(platform),
    )?);

    let data_dir = match layers_figment(&layers).extract_inner::<PathBuf>("data_dir") {
        Ok(data_dir) => data_dir,
        Err(err) if err.missing() => default_data_dir()?,
        Err(err) => return Err(err).context("Failed to load the server configuration"),
    };
    let common_input = layers[settings_start..settings_end]
        .iter()
        .fold(Figment::new(), |common, layer| {
            let mut layer = layer.clone();
            layer.dict.remove("advanced");
            common.merge(layer)
        })
        .merge(Serialized::default("data_dir", &data_dir));
    let common = Layer::read(
        Metadata::named("the server settings"),
        server_settings::all_in_one(&common_input)?,
    )?;
    let (_, advanced) = layers[settings_start..settings_end].iter().try_fold(
        (common_input, Vec::new()),
        |(lower, mut projected), layer| {
            let mut advanced = layer.clone();
            advanced.dict.retain(|key, _| key == "advanced");
            let advanced = Figment::from(advanced);
            projected.push(Layer::read(
                layer.metadata.clone(),
                server_settings::server_advanced(&advanced, &lower)?,
            )?);
            Ok::<_, anyhow::Error>((lower.merge(advanced), projected))
        },
    )?;
    layers.splice(
        settings_start..settings_end,
        std::iter::once(common).chain(advanced),
    );
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
    sources: Map<Tag, Metadata>,
}

impl Layer {
    fn read(metadata: Metadata, provider: impl Provider) -> anyhow::Result<Self> {
        let provider = Figment::from(provider);
        let mut data = provider
            .data()
            .map_err(|err| anyhow!("Failed to read {}: {err}", metadata.name))?;
        Ok(Self {
            metadata,
            dict: data.remove(&Profile::Default).unwrap_or_default(),
            sources: provider.__metadata_map().unwrap_or_default(),
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

    fn __metadata_map(&self) -> Option<Map<Tag, Metadata>> {
        Some(self.sources.clone())
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
                    sources: layer.sources.clone(),
                })
            },
        );
        match figment.extract::<GolemServerConfig>() {
            Ok(config) => {
                validate(&config)?;
                validate_resource_policy(&config, &dicts[1..])?;
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
    for key in LAUNCHER_OWNED_KEYS {
        let path: Vec<String> = key.split('.').map(str::to_string).collect();
        for layer in layers {
            if get_path(&layer.dict, &path).is_some() {
                let source = if layer.metadata.name == "the environment variables" {
                    format!(
                        "set by the environment variable GOLEM_ADVANCED__{}",
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

fn validate_resource_policy(config: &GolemServerConfig, layers: &[Dict]) -> anyhow::Result<()> {
    let metering = config.worker_executor.resource_usage_metering;
    let managed_filesystem = matches!(
        config.worker_executor.filesystem_storage.mode,
        FilesystemStorageMode::ManagedXfs { .. }
    );
    for (name, plan) in &config.registry_service.initial_plans {
        if plan.max_storage_per_agent_enabled && !managed_filesystem {
            bail!("Per-agent storage limits in initial plan `{name}` require managed XFS storage");
        }
        for (field, enabled) in [
            ("monthly_compute_gcu", metering.compute),
            ("monthly_memory_gb_seconds", metering.memory),
            ("monthly_durable_storage_gb_month", metering.filesystem),
            ("monthly_ephemeral_storage_gb_month", metering.filesystem),
        ] {
            let path = vec![
                "registry_service".to_string(),
                "initial_plans".to_string(),
                name.clone(),
                field.to_string(),
            ];
            if enabled && !layers.iter().any(|layer| get_path(layer, &path).is_some()) {
                bail!(
                    "`{}` must be configured when its billing meter is enabled",
                    path.join(".")
                );
            }
        }
        MonthlyPlanAmounts {
            compute_gcu: plan.monthly_compute_gcu,
            memory_gb_seconds: plan.monthly_memory_gb_seconds,
            durable_storage_gb_month: plan.monthly_durable_storage_gb_month,
            ephemeral_storage_gb_month: plan.monthly_ephemeral_storage_gb_month,
        }
        .resolve()
        .with_context(|| format!("Invalid monthly resource amounts in initial plan `{name}`"))?;
    }
    if metering.filesystem && !managed_filesystem {
        bail!("Monthly filesystem metering requires managed XFS storage");
    }
    Ok(())
}

/// Checks the rules that span services.
fn validate(config: &GolemServerConfig) -> anyhow::Result<()> {
    if let Some(management_host) = &config.router.management_host {
        Domain::parse_host(management_host)
            .map_err(|err| anyhow!("Invalid `router.management_host`: {err}"))?;
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
/// flag. Returns `false` when `flag` is not a dump flag, and an error for an unknown
/// `--dump-config*` flag.
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
    )?)
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
    Ok(
        server_settings::reference("server", &Figment::from(Serialized::defaults(value)))
            .extract()?,
    )
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
mod tests;
