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

use crate::components::SupportedServiceLaunch;
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::component_compilation_service::provided::ProvidedComponentCompilationService;
use crate::components::component_compilation_service::spawned::SpawnedComponentCompilationService;
use crate::components::rdb::Rdb;
use crate::components::rdb::borrowed_sqlite::BorrowedSqliteRdb;
use crate::components::rdb::docker_postgres::DockerPostgresRdb;
use crate::components::rdb::provided_postgres::ProvidedPostgresRdb;
use crate::components::rdb::sqlite::SqliteRdb;
use crate::components::rdb::{DbInfo, PostgresInfo};
use crate::components::redis::Redis;
use crate::components::redis::provided::ProvidedRedis;
use crate::components::redis::spawned::SpawnedRedis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::redis_monitor::provided::ProvidedRedisMonitor;
use crate::components::redis_monitor::spawned::SpawnedRedisMonitor;
use crate::components::registry_service::RegistryService;
use crate::components::registry_service::provided::ProvidedRegistryService;
use crate::components::registry_service::spawned::SpawnedRegistryService;
use crate::components::shard_manager::ShardManager;
use crate::components::shard_manager::provided::ProvidedShardManager;
use crate::components::shard_manager::spawned::SpawnedShardManager;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_executor_cluster::provided::ProvidedWorkerExecutorCluster;
use crate::components::worker_executor_cluster::spawned::SpawnedWorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use crate::components::worker_service::provided::ProvidedWorkerService;
use crate::components::worker_service::spawned::SpawnedWorkerService;
use crate::config::{DbType, TestDependencies};
use async_trait::async_trait;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::TokenSecret;
use golem_common::model::plan::PlanId;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::fs::FileSystemBlobStorage;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tracing::Level;
use uuid::Uuid;

/// A handle to either an owned `TempDir` (parent process) or a borrowed
/// on-disk path (worker process).
///
/// Mirrors the same wrapper used by [`crate::../golem-worker-executor-test-utils`]
/// in its `Hosted` reconstruction. `EnvBasedTestDependencies` keeps its
/// `temp_directory` field behind this enum so that worker subprocesses
/// reconstructing the dep via [`AsyncHostedDep::from_descriptor`] never
/// delete the parent-owned directory tree on drop.
pub enum TestTempDir {
    /// Parent-owned: dropping this also removes the underlying directory.
    Owned(TempDir),
    /// Worker-borrowed: dropping this does NOT remove the underlying
    /// directory. Lifetime is controlled by whichever process holds the
    /// matching `Owned` value.
    Borrowed(PathBuf),
}

impl TestTempDir {
    pub fn path(&self) -> &Path {
        match self {
            Self::Owned(td) => td.path(),
            Self::Borrowed(p) => p.as_path(),
        }
    }
}

impl Debug for TestTempDir {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "TestTempDir({:?})", self.path())
    }
}

/// Selects the explicit inputs sent to the fixture's spawned service processes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TestServiceInputs {
    /// Keep the suite's configuration files, inherited environment and policy overrides.
    #[default]
    Suite,
    /// Use isolated supported settings and only advanced listener and peer addresses.
    /// Native defaults supply the accounts and the single default plan, not quota test plans.
    SupportedSettings,
}

#[derive(Clone)]
pub struct EnvBasedTestDependenciesConfig {
    pub service_inputs: TestServiceInputs,
    pub worker_executor_cluster_size: usize,
    pub environment_state_cache_capacity: Option<usize>,
    pub number_of_shards_override: Option<usize>,
    pub oplog_archive_interval: Option<Duration>,
    pub shared_client: bool,
    pub db_type: DbType,
    pub quiet: bool,
    pub redis_host: String,
    pub redis_port: u16,
    pub registry_http_port: u16,
    pub registry_grpc_port: u16,
    pub shard_manager_http_port: u16,
    pub shard_manager_grpc_port: u16,
    pub compilation_http_port: u16,
    pub compilation_grpc_port: u16,
    pub worker_http_port: u16,
    pub worker_grpc_port: u16,
    pub worker_custom_request_port: u16,
    pub worker_mcp_port: u16,
    pub executor_base_http_port: u16,
    pub executor_base_grpc_port: u16,
    pub redis_key_prefix: String,
    pub golem_repo_root: PathBuf,
    pub bin_profile: String,
    pub unique_network_id: String,
}

impl EnvBasedTestDependenciesConfig {
    pub fn new() -> Self {
        Self::default().with_env_overrides()
    }

    /// Adds `GOLEM_TEST_PORT_OFFSET` to the listener ports, then applies absolute
    /// `REDIS_PORT` and `REGISTRY_HTTP_PORT` overrides.
    pub fn with_env_overrides(self) -> Self {
        self.with_env_overrides_from(opt_env_var)
    }

    /// Applies environment values supplied by the caller, without changing the process environment.
    pub fn with_env_overrides_from(mut self, env: impl Fn(&str) -> Option<String>) -> Self {
        if let Some(offset) = env("GOLEM_TEST_PORT_OFFSET") {
            let offset: u16 = offset
                .parse()
                .expect("Failed to parse GOLEM_TEST_PORT_OFFSET");
            for port in [
                &mut self.redis_port,
                &mut self.registry_http_port,
                &mut self.registry_grpc_port,
                &mut self.shard_manager_http_port,
                &mut self.shard_manager_grpc_port,
                &mut self.compilation_http_port,
                &mut self.compilation_grpc_port,
                &mut self.worker_http_port,
                &mut self.worker_grpc_port,
                &mut self.worker_custom_request_port,
                &mut self.worker_mcp_port,
                &mut self.executor_base_http_port,
                &mut self.executor_base_grpc_port,
            ] {
                *port = port
                    .checked_add(offset)
                    .expect("GOLEM_TEST_PORT_OFFSET overflows a test port");
            }
        }

        if let Some(quiet) = env("QUIET").and_then(|value| value.parse::<bool>().ok()) {
            self.quiet = quiet;
        }

        if let Some(redis_port) = env("REDIS_PORT") {
            self.redis_port = redis_port.parse().expect("Failed to parse REDIS_PORT");
        }

        if let Some(registry_http_port) = env("REGISTRY_HTTP_PORT") {
            self.registry_http_port = registry_http_port
                .parse()
                .expect("Failed to parse REGISTRY_HTTP_PORT");
        }

        if let Some(redis_key_prefix) = env("REDIS_KEY_PREFIX") {
            self.redis_key_prefix = redis_key_prefix;
        }

        if let Some(redis_prefix) = env("REDIS_PREFIX") {
            self.redis_key_prefix = redis_prefix;
        }

        if let Some(golem_repo_root) = env("GOLEM_REPO_ROOT") {
            self.golem_repo_root = golem_repo_root.into();
        }

        if let Some(profile) = env("GOLEM_TEST_BIN_PROFILE") {
            assert!(
                matches!(profile.as_str(), "debug" | "dev-ci"),
                "GOLEM_TEST_BIN_PROFILE must be debug or dev-ci"
            );
            self.bin_profile = profile;
        }

        if let Some(last_index) = self.worker_executor_cluster_size.checked_sub(1) {
            let last_index =
                u16::try_from(last_index).expect("worker executor cluster is too large");
            self.executor_base_http_port
                .checked_add(last_index)
                .expect("worker executor HTTP port overflows");
            self.executor_base_grpc_port
                .checked_add(last_index)
                .expect("worker executor gRPC port overflows");
        }
        self
    }

    pub fn default_stdout_level(&self) -> Level {
        if self.quiet {
            Level::DEBUG
        } else {
            Level::INFO
        }
    }

    pub fn default_stderr_level(&self) -> Level {
        Level::ERROR
    }

    pub fn default_verbosity(&self) -> Level {
        if self.quiet {
            Level::WARN
        } else {
            Level::DEBUG
        }
    }

    pub fn redis_monitor_stdout_level(&self) -> Level {
        Level::TRACE
    }

    pub fn redis_monitor_stderr_level(&self) -> Level {
        Level::ERROR
    }

    fn test_components_dir(&self) -> PathBuf {
        self.golem_repo_root.join("test-components")
    }

    pub fn service_binaries_dir(&self) -> PathBuf {
        resolve_cargo_target_dir(&self.golem_repo_root).join(&self.bin_profile)
    }
}

/// Resolves the cargo target directory for the workspace rooted at `repo_root`.
///
/// Uses `CARGO_TARGET_DIR` directly when supplied, without invoking Cargo. Otherwise,
/// asks Cargo via `cargo metadata` to honor `build.target-dir` and Cargo wrappers.
/// If Cargo cannot be queried it falls back to `<repo_root>/target`.
///
/// The metadata result is memoized per `repo_root` for the lifetime of the process.
fn resolve_cargo_target_dir(repo_root: &Path) -> PathBuf {
    use std::sync::{Mutex, OnceLock};

    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        return if target_dir.is_absolute() {
            target_dir
        } else {
            std::env::current_dir()
                .expect("Failed to resolve relative CARGO_TARGET_DIR")
                .join(target_dir)
        };
    }

    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Some(cached) = cache.lock().unwrap().get(repo_root) {
        return cached.clone();
    }

    let resolved = resolve_cargo_target_dir_uncached(repo_root);
    cache
        .lock()
        .unwrap()
        .insert(repo_root.to_path_buf(), resolved.clone());
    resolved
}

fn resolve_cargo_target_dir_uncached(repo_root: &Path) -> PathBuf {
    let fallback = || repo_root.join("target");

    let manifest_path = repo_root.join("Cargo.toml");
    if !manifest_path.exists() {
        return fallback();
    }
    // `repo_root` is usually a relative path (e.g. `..`). Cargo resolves a
    // relative `--manifest-path` against the child process's working
    // directory, so a relative path combined with `current_dir(repo_root)`
    // would be applied twice and point outside the repo. Absolutize it
    // against our own working directory instead.
    let manifest_path = manifest_path.canonicalize().unwrap_or(manifest_path);

    let output = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .output();

    match output {
        Ok(output) if output.status.success() => {
            match serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                Ok(metadata) => metadata
                    .get("target_directory")
                    .and_then(|value| value.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(fallback),
                Err(_) => fallback(),
            }
        }
        _ => fallback(),
    }
}

/// Finds the executable of a Golem service for a test that spawns its own copy of the service.
///
/// The service is looked up first in the cargo profile directory of the running test binary, so a
/// `--release` run or a redirected target directory finds the binaries built with the test. When
/// that directory has no such binary, as in CI where prebuilt binaries are placed in the debug
/// directory, it falls back to the directory the shared test dependencies start services from.
pub fn service_executable(golem_repo_root: &Path, name: &str) -> PathBuf {
    let test_profile_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.to_path_buf()));
    locate_service_executable(
        test_profile_dir.as_deref(),
        &resolve_cargo_target_dir(golem_repo_root).join("debug"),
        name,
    )
}

fn locate_service_executable(
    test_profile_dir: Option<&Path>,
    shared_services_dir: &Path,
    name: &str,
) -> PathBuf {
    test_profile_dir
        .map(|dir| dir.join(name))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| shared_services_dir.join(name))
}

impl Default for EnvBasedTestDependenciesConfig {
    fn default() -> Self {
        Self {
            service_inputs: TestServiceInputs::Suite,
            worker_executor_cluster_size: 4,
            environment_state_cache_capacity: None,
            number_of_shards_override: None,
            oplog_archive_interval: None,
            shared_client: false,
            db_type: DbType::Postgres,
            quiet: false,
            redis_host: "localhost".to_string(),
            redis_port: 6379,
            registry_http_port: 8081,
            registry_grpc_port: 9091,
            shard_manager_http_port: 9021,
            shard_manager_grpc_port: 9020,
            compilation_http_port: 8083,
            compilation_grpc_port: 9094,
            worker_http_port: 8082,
            worker_grpc_port: 9092,
            worker_custom_request_port: 9093,
            worker_mcp_port: 9095,
            executor_base_http_port: 9000,
            executor_base_grpc_port: 9100,
            redis_key_prefix: "".to_string(),
            golem_repo_root: PathBuf::from(".."),
            bin_profile: "debug".to_string(),
            unique_network_id: Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Clone)]
pub struct EnvBasedTestDependencies {
    rdb: Arc<dyn Rdb>,
    redis: Arc<dyn Redis>,
    redis_monitor: Arc<dyn RedisMonitor>,
    shard_manager: Arc<dyn ShardManager>,
    component_compilation_service: Arc<dyn ComponentCompilationService>,
    worker_service: Arc<dyn WorkerService>,
    worker_executor_cluster: Arc<dyn WorkerExecutorCluster>,
    blob_storage: Arc<dyn BlobStorage>,
    initial_agent_files_service: Arc<InitialAgentFilesService>,
    temp_directory: Arc<TestTempDir>,
    registry_service: Arc<dyn RegistryService>,
    test_component_dir: PathBuf,
}

impl Debug for EnvBasedTestDependencies {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "EnvBasedTestDependencies")
    }
}

impl EnvBasedTestDependencies {
    fn supported_launch(
        config: &EnvBasedTestDependenciesConfig,
        temp_dir: &Path,
        service: &'static str,
        rdb: &dyn Rdb,
        executor_index: usize,
    ) -> Option<SupportedServiceLaunch> {
        if config.service_inputs == TestServiceInputs::Suite {
            return None;
        }
        assert!(
            config.environment_state_cache_capacity.is_none()
                && config.number_of_shards_override.is_none()
                && config.oplog_archive_interval.is_none(),
            "supported-settings fixture cannot override service policy"
        );
        let slug = service.replace('_', "-");
        let process_dir = temp_dir
            .join("processes")
            .join(format!("{slug}-{executor_index}"));
        let home = process_dir.join("home");
        let working = process_dir.join("work");
        std::fs::create_dir_all(&home).expect("create isolated home");
        std::fs::create_dir_all(working.join("config")).expect("create isolated config directory");
        std::fs::write(working.join("config").join(format!("{slug}.toml")), "")
            .expect("create required empty service config");
        if service == "worker_executor" {
            std::fs::create_dir_all(temp_dir.join("executor-logs"))
                .expect("create executor log directory");
        }
        let env = Self::supported_service_env(
            config,
            temp_dir,
            service,
            rdb.info(),
            executor_index,
            &std::env::var("GOLEM_TEST_GATEWAY_SESSION_STORAGE")
                .unwrap_or_else(|_| "redis".to_string()),
        );
        let executable = if service == "worker_executor" {
            "worker-executor".to_string()
        } else {
            format!("golem-{slug}")
        };
        Some(SupportedServiceLaunch::new(
            service,
            &config.service_binaries_dir().join(executable),
            working,
            env,
        ))
    }

    fn supported_service_env(
        config: &EnvBasedTestDependenciesConfig,
        temp_dir: &Path,
        service: &'static str,
        db: DbInfo,
        executor_index: usize,
        gateway_sessions: &str,
    ) -> HashMap<String, String> {
        let slug = service.replace('_', "-");
        let home = temp_dir
            .join("processes")
            .join(format!("{slug}-{executor_index}"))
            .join("home");
        let data_dir = match &db {
            DbInfo::Sqlite(path) => path.clone(),
            _ => temp_dir.join(&slug),
        };
        let mut env = db.env("", false);
        env.extend([
            ("HOME".to_string(), home.to_string_lossy().into_owned()),
            (
                "USERPROFILE".to_string(),
                home.to_string_lossy().into_owned(),
            ),
            (
                "GOLEM__DATA_DIR".to_string(),
                data_dir.to_string_lossy().into_owned(),
            ),
            ("GOLEM__BLOB_STORE".to_string(), "data_disk".to_string()),
            (
                "GOLEM__DATA_DISK__BLOB_ROOT".to_string(),
                temp_dir.join("blob_storage").to_string_lossy().into_owned(),
            ),
            ("GOLEM__CACHE__ENABLED".to_string(), "false".to_string()),
        ]);
        if service == "worker_service" {
            match gateway_sessions {
                "sqlite" => {
                    env.insert("GOLEM__STATE_STORE".to_string(), "sqlite".to_string());
                }
                "redis" => env.extend([
                    ("GOLEM__CACHE__ENABLED".to_string(), "true".to_string()),
                    ("GOLEM__CACHE__HOST".to_string(), config.redis_host.clone()),
                    (
                        "GOLEM__CACHE__PORT".to_string(),
                        config.redis_port.to_string(),
                    ),
                    (
                        "GOLEM__CACHE__KEY_PREFIX".to_string(),
                        config.redis_key_prefix.clone(),
                    ),
                ]),
                _ => panic!("GOLEM_TEST_GATEWAY_SESSION_STORAGE must be redis or sqlite"),
            }
        }
        let index = u16::try_from(executor_index).expect("executor index fits u16");
        let (http_key, http_port, grpc_port, peers): (&str, u16, u16, Vec<(&str, u16)>) =
            match service {
                "registry_service" => (
                    "HTTP_PORT",
                    config.registry_http_port,
                    config.registry_grpc_port,
                    vec![(
                        "COMPONENT_COMPILATION__CONFIG",
                        config.compilation_grpc_port,
                    )],
                ),
                "component_compilation_service" => (
                    "HTTP_PORT",
                    config.compilation_http_port,
                    config.compilation_grpc_port,
                    vec![("REGISTRY_SERVICE__CONFIG", config.registry_grpc_port)],
                ),
                "shard_manager" => (
                    "HTTP_PORT",
                    config.shard_manager_http_port,
                    config.shard_manager_grpc_port,
                    vec![("REGISTRY_SERVICE", config.registry_grpc_port)],
                ),
                "worker_service" => (
                    "PORT",
                    config.worker_http_port,
                    config.worker_grpc_port,
                    vec![
                        ("REGISTRY_SERVICE", config.registry_grpc_port),
                        ("SHARD_MANAGER", config.shard_manager_grpc_port),
                    ],
                ),
                "worker_executor" => (
                    "HTTP_PORT",
                    config.executor_base_http_port.checked_add(index).unwrap(),
                    config.executor_base_grpc_port.checked_add(index).unwrap(),
                    vec![
                        ("REGISTRY_SERVICE", config.registry_grpc_port),
                        ("SHARD_MANAGER", config.shard_manager_grpc_port),
                        ("PUBLIC_WORKER_API", config.worker_grpc_port),
                    ],
                ),
                _ => panic!("unknown supported-settings service {service}"),
            };
        let prefix = format!("GOLEM_ADVANCED__{}__", service.to_uppercase());
        env.extend([
            (format!("{prefix}{http_key}"), http_port.to_string()),
            (format!("{prefix}GRPC__PORT"), grpc_port.to_string()),
        ]);
        peers.into_iter().for_each(|(peer, port)| {
            env.insert(format!("{prefix}{peer}__HOST"), "localhost".to_string());
            env.insert(format!("{prefix}{peer}__PORT"), port.to_string());
        });
        if service == "worker_service" {
            env.extend([
                (
                    format!("{prefix}CUSTOM_REQUEST_PORT"),
                    config.worker_custom_request_port.to_string(),
                ),
                (
                    format!("{prefix}MCP_PORT"),
                    config.worker_mcp_port.to_string(),
                ),
            ]);
        }
        if service == "worker_executor" {
            let logs = temp_dir.join("executor-logs");
            env.extend([
                (
                    "GOLEM__LOGGING__FILE__ENABLED".to_string(),
                    "true".to_string(),
                ),
                ("GOLEM__LOGGING__FILE__JSON".to_string(), "true".to_string()),
                (
                    "GOLEM__LOGGING__FILE__JSON_FLATTEN".to_string(),
                    "false".to_string(),
                ),
                (
                    "GOLEM__LOGGING__FILE__JSON_FLATTEN_SPAN".to_string(),
                    "false".to_string(),
                ),
                (
                    "GOLEM__LOGGING__FILE__JSON_SOURCE_LOCATION".to_string(),
                    "true".to_string(),
                ),
                (
                    "GOLEM__LOGGING__FILE_DIR".to_string(),
                    logs.to_string_lossy().into_owned(),
                ),
                (
                    "GOLEM__LOGGING__FILE_NAME".to_string(),
                    format!("executor-{executor_index}.jsonl"),
                ),
            ]);
        }
        env
    }

    async fn make_rdb(config: &EnvBasedTestDependenciesConfig, temp_dir: &Path) -> Arc<dyn Rdb> {
        match config.db_type {
            DbType::Sqlite => {
                let sqlite_db_dir = &temp_dir.join("sqlite");
                Arc::new(SqliteRdb::new(sqlite_db_dir))
            }
            DbType::Postgres => {
                Arc::new(DockerPostgresRdb::new(&config.unique_network_id, false).await)
            }
        }
    }

    async fn make_redis(config: &EnvBasedTestDependenciesConfig) -> Arc<dyn Redis> {
        let prefix = config.redis_key_prefix.clone();
        let host = config.redis_host.clone();
        let port = config.redis_port;

        let running = crate::components::redis::check_if_running(&host, port);
        assert!(
            config.service_inputs != TestServiceInputs::SupportedSettings || !running,
            "supported-settings fixture requires its own Redis endpoint"
        );
        if running {
            Arc::new(ProvidedRedis::new(host, port, prefix))
        } else {
            Arc::new(SpawnedRedis::new(
                port,
                prefix,
                config.default_stdout_level(),
                config.default_stderr_level(),
            ))
        }
    }

    async fn make_redis_monitor(
        config: &EnvBasedTestDependenciesConfig,
        redis: Arc<dyn Redis>,
    ) -> Arc<dyn RedisMonitor> {
        Arc::new(SpawnedRedisMonitor::new(
            redis,
            config.redis_monitor_stdout_level(),
            config.redis_monitor_stderr_level(),
        ))
    }

    async fn make_registry_service(
        config: &EnvBasedTestDependenciesConfig,
        rdb: &Arc<dyn Rdb>,
        component_compilation_service: &Arc<dyn ComponentCompilationService>,
        temp_dir: &Path,
    ) -> Arc<dyn RegistryService> {
        if let Some(launch) =
            Self::supported_launch(config, temp_dir, "registry_service", rdb.as_ref(), 0)
        {
            return Arc::new(
                SpawnedRegistryService::new_supported(
                    launch,
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            );
        }
        Arc::new(
            SpawnedRegistryService::new(
                &config.service_binaries_dir().join("golem-registry-service"),
                &config.golem_repo_root.join("golem-registry-service"),
                config.registry_http_port,
                config.registry_grpc_port,
                rdb,
                Some(component_compilation_service),
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                false,
            )
            .await,
        )
    }

    async fn make_shard_manager(
        config: &EnvBasedTestDependenciesConfig,
        rdb: Arc<dyn Rdb>,
        registry_service: Arc<dyn RegistryService>,
        temp_dir: &Path,
    ) -> Arc<dyn ShardManager> {
        if let Some(launch) =
            Self::supported_launch(config, temp_dir, "shard_manager", rdb.as_ref(), 0)
        {
            return Arc::new(
                SpawnedShardManager::new_supported(
                    launch,
                    false,
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            );
        }
        Arc::new(
            SpawnedShardManager::new(
                &config.service_binaries_dir().join("golem-shard-manager"),
                &config.golem_repo_root.join("golem-shard-manager"),
                config.number_of_shards_override,
                None,
                false,
                config.shard_manager_http_port,
                config.shard_manager_grpc_port,
                rdb,
                registry_service,
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                false,
            )
            .await,
        )
    }

    async fn make_component_compilation_service(
        config: &EnvBasedTestDependenciesConfig,
        rdb: &Arc<dyn Rdb>,
        temp_dir: &Path,
    ) -> Arc<dyn ComponentCompilationService> {
        if let Some(launch) = Self::supported_launch(
            config,
            temp_dir,
            "component_compilation_service",
            rdb.as_ref(),
            0,
        ) {
            return Arc::new(
                SpawnedComponentCompilationService::new_supported(
                    launch,
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            );
        }
        Arc::new(
            SpawnedComponentCompilationService::new(
                &config
                    .service_binaries_dir()
                    .join("golem-component-compilation-service"),
                &config
                    .golem_repo_root
                    .join("golem-component-compilation-service"),
                config.compilation_http_port,
                config.compilation_grpc_port,
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                true,
                false,
            )
            .await,
        )
    }

    async fn make_worker_service(
        config: &EnvBasedTestDependenciesConfig,
        shard_manager: &Arc<dyn ShardManager>,
        rdb: &Arc<dyn Rdb>,
        redis: &Arc<dyn Redis>,
        registry_service: &Arc<dyn RegistryService>,
        temp_dir: &Path,
    ) -> Arc<dyn WorkerService> {
        if let Some(launch) =
            Self::supported_launch(config, temp_dir, "worker_service", rdb.as_ref(), 0)
        {
            return Arc::new(
                SpawnedWorkerService::new_supported(
                    launch,
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            );
        }
        let data_dir = match rdb.info() {
            DbInfo::Sqlite(path) => path,
            _ => temp_dir.join("worker-service"),
        };
        Arc::new(
            SpawnedWorkerService::new(
                &config.service_binaries_dir().join("golem-worker-service"),
                &config.golem_repo_root.join("golem-worker-service"),
                config.worker_http_port,
                config.worker_grpc_port,
                config.worker_custom_request_port,
                config.worker_mcp_port,
                shard_manager,
                rdb,
                redis,
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                registry_service,
                true,
                false,
                &[(
                    "GOLEM__DATA_DIR".to_string(),
                    data_dir.to_string_lossy().into_owned(),
                )],
            )
            .await,
        )
    }

    async fn make_worker_executor_cluster(
        config: &EnvBasedTestDependenciesConfig,
        shard_manager: Arc<dyn ShardManager>,
        worker_service: Arc<dyn WorkerService>,
        rdb: Arc<dyn Rdb>,
        registry_service: Arc<dyn RegistryService>,
        temp_dir: &Path,
    ) -> Arc<dyn WorkerExecutorCluster> {
        if config.service_inputs == TestServiceInputs::SupportedSettings {
            let launches = (0..config.worker_executor_cluster_size)
                .map(|index| {
                    Self::supported_launch(config, temp_dir, "worker_executor", rdb.as_ref(), index)
                        .unwrap()
                })
                .collect();
            return Arc::new(
                SpawnedWorkerExecutorCluster::new_supported(
                    launches,
                    shard_manager,
                    config.default_stdout_level(),
                    config.default_stderr_level(),
                )
                .await,
            );
        }
        Arc::new(
            SpawnedWorkerExecutorCluster::new(
                config.worker_executor_cluster_size,
                config.executor_base_http_port,
                config.executor_base_grpc_port,
                &config.service_binaries_dir().join("worker-executor"),
                &config.golem_repo_root.join("golem-worker-executor"),
                rdb,
                shard_manager,
                worker_service,
                config.default_verbosity(),
                config.default_stdout_level(),
                config.default_stderr_level(),
                registry_service,
                config.environment_state_cache_capacity,
                config.oplog_archive_interval,
                true,
                false,
            )
            .await,
        )
    }

    pub async fn new(config: EnvBasedTestDependenciesConfig) -> anyhow::Result<Self> {
        let temp_directory = Arc::new(TestTempDir::Owned(
            TempDir::new().expect("Failed to create temporary directory"),
        ));

        let blob_storage_root = &temp_directory.path().join("blob_storage");
        tokio::fs::create_dir_all(&blob_storage_root).await?;

        let blob_storage = Arc::new(FileSystemBlobStorage::new(blob_storage_root).await.unwrap());

        let initial_agent_files_service =
            Arc::new(InitialAgentFilesService::new(blob_storage.clone()));

        let redis = Self::make_redis(&config).await;
        {
            let mut connection = redis.get_connection(0);
            redis::cmd("FLUSHALL").exec(&mut connection).unwrap();
        }

        let redis_monitor = Self::make_redis_monitor(&config, redis.clone()).await;

        let rdb = Self::make_rdb(&config, temp_directory.path()).await;

        let component_compilation_service =
            Self::make_component_compilation_service(&config, &rdb, temp_directory.path()).await;

        let registry_service = Self::make_registry_service(
            &config,
            &rdb,
            &component_compilation_service,
            temp_directory.path(),
        )
        .await;

        let shard_manager = Self::make_shard_manager(
            &config,
            rdb.clone(),
            registry_service.clone(),
            temp_directory.path(),
        )
        .await;

        let worker_service = Self::make_worker_service(
            &config,
            &shard_manager,
            &rdb,
            &redis,
            &registry_service,
            temp_directory.path(),
        )
        .await;

        let worker_executor_cluster = Self::make_worker_executor_cluster(
            &config,
            shard_manager.clone(),
            worker_service.clone(),
            rdb.clone(),
            registry_service.clone(),
            temp_directory.path(),
        )
        .await;

        Ok(Self {
            rdb,
            redis,
            redis_monitor,
            blob_storage,
            initial_agent_files_service,
            registry_service,
            temp_directory,
            shard_manager,
            component_compilation_service,
            worker_executor_cluster,
            worker_service,
            test_component_dir: config.test_components_dir(),
        })
    }
}

/// Wire-form snapshot of a parent-constructed [`EnvBasedTestDependencies`].
/// Serialised by [`AsyncHostedDep::descriptor`] on the parent and consumed
/// by [`AsyncHostedDep::from_descriptor`] on each worker subprocess to
/// rebuild equivalent worker-side handles that attach to (rather than
/// re-spawn) the parent's live services.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct EnvBasedTestDependenciesDescriptor {
    temp_directory_path: PathBuf,
    test_component_dir: PathBuf,
    blob_storage_root: PathBuf,
    redis_host: String,
    redis_port: u16,
    redis_prefix: String,
    rdb: RdbDescriptor,
    shard_manager: ShardManagerDescriptor,
    component_compilation_service: ComponentCompilationServiceDescriptor,
    worker_service: WorkerServiceDescriptor,
    worker_executor_cluster: Vec<WorkerExecutorDescriptor>,
    registry_service: RegistryServiceDescriptor,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RdbDescriptor {
    Sqlite {
        path: PathBuf,
    },
    Postgres {
        public_host: String,
        public_port: u16,
        private_host: String,
        private_port: u16,
        database_name: String,
        username: String,
        password: String,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct ShardManagerDescriptor {
    host: String,
    http_port: u16,
    grpc_port: u16,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct ComponentCompilationServiceDescriptor {
    host: String,
    http_port: u16,
    grpc_port: u16,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct WorkerServiceDescriptor {
    host: String,
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    mcp_port: u16,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct WorkerExecutorDescriptor {
    host: String,
    grpc_port: u16,
    http_port: Option<u16>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct RegistryServiceDescriptor {
    host: String,
    http_port: u16,
    grpc_port: u16,
    admin_account_id: Uuid,
    admin_account_email: String,
    admin_account_token: String,
    builtin_plugin_owner_account_id: Uuid,
    default_plan_id: Uuid,
    low_fuel_plan_id: Option<Uuid>,
    low_disk_space_plan_id: Option<Uuid>,
    low_http_calls_plan_id: Option<Uuid>,
    low_rpc_calls_plan_id: Option<Uuid>,
}

impl test_r::core::AsyncHostedDep for EnvBasedTestDependencies {
    fn descriptor(&self) -> Vec<u8> {
        // Canonicalize every on-disk path so worker subprocesses
        // running with a different cwd than the parent still see
        // absolute, valid paths (the same hardening applied to
        // `WorkerExecutorTestDependencies::descriptor`).
        fn canonical(path: &Path) -> PathBuf {
            std::fs::canonicalize(path).unwrap_or_else(|e| {
                panic!(
                    "EnvBasedTestDependencies::descriptor: cannot \
                     canonicalize {path:?}: {e}"
                )
            })
        }

        let rdb = match self.rdb.info() {
            DbInfo::Sqlite(path) => RdbDescriptor::Sqlite {
                path: canonical(&path),
            },
            DbInfo::Postgres(pg) => RdbDescriptor::Postgres {
                public_host: pg.public_host.clone(),
                public_port: pg.public_port,
                private_host: pg.private_host.clone(),
                private_port: pg.private_port,
                database_name: pg.database_name.clone(),
                username: pg.username.clone(),
                password: pg.password.clone(),
            },
            DbInfo::Mysql(_) => panic!(
                "EnvBasedTestDependencies::descriptor: MySQL RDB is not yet \
                 wired through the Hosted descriptor. Add a Mysql variant \
                 to `RdbDescriptor` (and a borrowed/provided variant in \
                 `crate::components::rdb`) before opting MySQL-backed \
                 suites into `scope = Hosted`."
            ),
        };

        let cluster_endpoints: Vec<WorkerExecutorDescriptor> = self
            .worker_executor_cluster
            .to_vec()
            .iter()
            .map(|exec| WorkerExecutorDescriptor {
                host: exec.grpc_host(),
                grpc_port: exec.grpc_port(),
                http_port: exec.http_port(),
            })
            .collect();

        let descriptor = EnvBasedTestDependenciesDescriptor {
            temp_directory_path: canonical(self.temp_directory.path()),
            test_component_dir: canonical(&self.test_component_dir),
            blob_storage_root: canonical(&self.temp_directory.path().join("blob_storage")),
            redis_host: self.redis.private_host().to_string(),
            redis_port: self.redis.private_port(),
            redis_prefix: self.redis.prefix().to_string(),
            rdb,
            shard_manager: ShardManagerDescriptor {
                host: self.shard_manager.grpc_host(),
                http_port: self.shard_manager.http_port(),
                grpc_port: self.shard_manager.grpc_port(),
            },
            component_compilation_service: ComponentCompilationServiceDescriptor {
                host: self.component_compilation_service.grpc_host(),
                http_port: self.component_compilation_service.http_port(),
                grpc_port: self.component_compilation_service.grpc_port(),
            },
            worker_service: WorkerServiceDescriptor {
                host: self.worker_service.grpc_host(),
                http_port: self.worker_service.http_port(),
                grpc_port: self.worker_service.gprc_port(),
                custom_request_port: self.worker_service.custom_request_port(),
                mcp_port: self.worker_service.mcp_port(),
            },
            worker_executor_cluster: cluster_endpoints,
            registry_service: RegistryServiceDescriptor {
                host: self.registry_service.grpc_host(),
                http_port: self.registry_service.http_port(),
                grpc_port: self.registry_service.grpc_port(),
                admin_account_id: self.registry_service.admin_account_id().0,
                admin_account_email: self.registry_service.admin_account_email().to_string(),
                admin_account_token: self
                    .registry_service
                    .admin_account_token()
                    .secret()
                    .to_string(),
                builtin_plugin_owner_account_id: self
                    .registry_service
                    .builtin_plugin_owner_account_id()
                    .0,
                default_plan_id: self.registry_service.default_plan().0,
                low_fuel_plan_id: self.registry_service.low_fuel_plan().map(|id| id.0),
                low_disk_space_plan_id: self.registry_service.low_disk_space_plan().map(|id| id.0),
                low_http_calls_plan_id: self.registry_service.low_http_calls_plan().map(|id| id.0),
                low_rpc_calls_plan_id: self.registry_service.low_rpc_calls_plan().map(|id| id.0),
            },
        };

        serde_json::to_vec(&descriptor)
            .expect("serializing EnvBasedTestDependenciesDescriptor must not fail")
    }

    async fn from_descriptor(bytes: &[u8]) -> Self {
        let descriptor: EnvBasedTestDependenciesDescriptor = serde_json::from_slice(bytes)
            .expect("deserializing EnvBasedTestDependenciesDescriptor must not fail");

        let redis: Arc<dyn Redis> = Arc::new(ProvidedRedis::new(
            descriptor.redis_host,
            descriptor.redis_port,
            descriptor.redis_prefix,
        ));
        let redis_monitor: Arc<dyn RedisMonitor> = Arc::new(ProvidedRedisMonitor::new());

        let rdb: Arc<dyn Rdb> = match descriptor.rdb {
            RdbDescriptor::Sqlite { path } => Arc::new(BorrowedSqliteRdb::new(&path)),
            RdbDescriptor::Postgres {
                public_host,
                public_port,
                private_host,
                private_port,
                database_name,
                username,
                password,
            } => Arc::new(ProvidedPostgresRdb::new(PostgresInfo {
                public_host,
                public_port,
                private_host,
                private_port,
                database_name,
                username,
                password,
            })),
        };

        let blob_storage = Arc::new(
            FileSystemBlobStorage::attach_existing(&descriptor.blob_storage_root)
                .expect("attach to parent-prepared blob storage root"),
        );
        let initial_agent_files_service =
            Arc::new(InitialAgentFilesService::new(blob_storage.clone()));

        let shard_manager: Arc<dyn ShardManager> = Arc::new(ProvidedShardManager::new(
            descriptor.shard_manager.host,
            descriptor.shard_manager.http_port,
            descriptor.shard_manager.grpc_port,
        ));

        let component_compilation_service: Arc<dyn ComponentCompilationService> =
            Arc::new(ProvidedComponentCompilationService::new(
                descriptor.component_compilation_service.host,
                descriptor.component_compilation_service.http_port,
                descriptor.component_compilation_service.grpc_port,
            ));

        let worker_service: Arc<dyn WorkerService> = Arc::new(
            ProvidedWorkerService::new(
                descriptor.worker_service.host,
                descriptor.worker_service.http_port,
                descriptor.worker_service.grpc_port,
                descriptor.worker_service.custom_request_port,
                descriptor.worker_service.mcp_port,
            )
            .await,
        );

        let worker_executor_cluster: Arc<dyn WorkerExecutorCluster> =
            Arc::new(ProvidedWorkerExecutorCluster::from_endpoints(
                descriptor
                    .worker_executor_cluster
                    .into_iter()
                    .map(|w| (w.host, w.grpc_port, w.http_port))
                    .collect(),
            ));

        let registry_service: Arc<dyn RegistryService> = Arc::new(
            ProvidedRegistryService::new(
                descriptor.registry_service.host,
                descriptor.registry_service.http_port,
                descriptor.registry_service.grpc_port,
                AccountId(descriptor.registry_service.admin_account_id),
                AccountEmail::new(descriptor.registry_service.admin_account_email),
                TokenSecret::trusted(descriptor.registry_service.admin_account_token),
                AccountId(descriptor.registry_service.builtin_plugin_owner_account_id),
                PlanId(descriptor.registry_service.default_plan_id),
                descriptor.registry_service.low_fuel_plan_id.map(PlanId),
                descriptor
                    .registry_service
                    .low_disk_space_plan_id
                    .map(PlanId),
                descriptor
                    .registry_service
                    .low_http_calls_plan_id
                    .map(PlanId),
                descriptor
                    .registry_service
                    .low_rpc_calls_plan_id
                    .map(PlanId),
            )
            .await,
        );

        Self {
            rdb,
            redis,
            redis_monitor,
            blob_storage,
            initial_agent_files_service,
            registry_service,
            temp_directory: Arc::new(TestTempDir::Borrowed(descriptor.temp_directory_path)),
            shard_manager,
            component_compilation_service,
            worker_executor_cluster,
            worker_service,
            test_component_dir: descriptor.test_component_dir,
        }
    }
}

/// Parent-hosted control plane for the [`EnvBasedTestDependencies`]
/// owner.
///
/// `EnvBasedTestDependencies` already exposes its bulk-data services
/// (Redis, RDB, registry / worker / shard manager services, worker
/// executor cluster, blob storage) to worker subprocesses via the
/// descriptor path on [`AsyncHostedDep`]. That path stays unchanged.
///
/// `RedisControl` adds the small set of operations that must execute
/// **on the parent's owner instance** rather than on a worker-side
/// reconstruction:
///
/// - Health-checking the parent-owned Redis instance from a worker.
///   Workers could open their own Redis client and run `INFO` directly,
///   but routing the check through the parent guarantees the answer
///   reflects exactly the same `Arc<dyn Redis>` the parent's owner
///   holds — not just *some* Redis at the same `host:port` that might
///   have been replaced between the descriptor snapshot and the check.
/// - Flushing a logical Redis database. Workers can technically issue
///   `FLUSHDB` themselves, but doing it via the parent keeps the
///   centralised owner as the single source of truth for state-reset
///   intent and makes "who touched the suite-shared Redis" trivially
///   greppable in test logs.
///
/// This trait is intentionally small; HR3.2 only needs a credible
/// control surface to ride alongside the descriptor path. Bulk-data
/// access (worker startup, gRPC streaming, etc.) keeps going through
/// the descriptor as before.
// `#[async_trait]` is intentionally NOT used here: test-r's
// `#[hosted_rpc]` macro auto-detects async-mode by inspecting
// `sig.asyncness` on the trait methods. `#[async_trait]` would desugar
// every method to `fn ... -> Pin<Box<dyn Future + Send>>` before the
// `hosted_rpc` macro runs, defeating the detection and silently falling
// back to sync-mode dispatch. The `async_fn_in_trait` lint only warns
// that the returned future isn't pinned to `Send`; that is fine here
// because the hosted-rpc dispatcher runs on the parent's tokio
// multi-thread runtime and the future bodies only `.await` `Send` futures.
#[allow(async_fn_in_trait)]
#[test_r::hosted_rpc]
pub trait RedisControl {
    /// Returns `true` if the parent-owned Redis instance currently
    /// responds successfully to an `INFO server` ping.
    ///
    /// This is the parent-side equivalent of
    /// [`Redis::assert_valid`](crate::components::redis::Redis::assert_valid),
    /// converted to a boolean so the worker side can `assert!(...)` on
    /// the answer rather than absorb a panic across an IPC boundary.
    ///
    /// Async because test-r's `#[hosted_rpc]` macro requires the trait
    /// to be all-async or all-sync, and other methods in this trait
    /// (notably `flush_redis_db`) sit on top of blocking I/O that we
    /// keep sync at the body level but expose via an async signature for
    /// the parent IPC dispatcher.
    async fn is_redis_healthy(&self) -> bool;

    /// Flushes the given logical Redis database on the parent-owned
    /// Redis instance and returns `Ok(())` on success or an `Err`
    /// containing the underlying Redis error message.
    async fn flush_redis_db(&self, db: u16) -> Result<(), String>;

    /// Returns the parent-owned Redis instance's configured
    /// key-prefix.
    ///
    /// Workers already learn the same value via the descriptor
    /// (`EnvBasedTestDependencies.redis().prefix()`), but exposing it
    /// over the RPC channel too gives test authors a way to confirm
    /// from inside a worker that both views agree on the same
    /// parent-owned Redis without re-parsing the descriptor.
    async fn redis_prefix(&self) -> String;
}

impl RedisControl for EnvBasedTestDependencies {
    async fn is_redis_healthy(&self) -> bool {
        crate::components::redis::check_if_running(
            &self.redis.private_host(),
            self.redis.private_port(),
        )
    }

    async fn flush_redis_db(&self, db: u16) -> Result<(), String> {
        let mut connection = self
            .redis
            .try_get_connection(db)
            .map_err(|e| format!("opening parent-owned Redis connection: {e}"))?;
        redis::cmd("FLUSHDB")
            .exec(&mut connection)
            .map_err(|e| format!("FLUSHDB on db {db}: {e}"))
    }

    async fn redis_prefix(&self) -> String {
        self.redis.prefix().to_string()
    }
}

/// Parent-hosted control surface for worker-executor cluster lifecycle.
///
/// This intentionally routes lifecycle operations back to the parent-owned
/// [`EnvBasedTestDependencies`] instance. Worker-side Hosted reconstruction
/// attaches to the already-running executors, but it must not kill, restart,
/// stop, or start those parent-owned processes directly.
///
/// The Redis helpers are repeated here because today's test-r HostedRpc owner
/// type has a single worker stub. Suites that need cluster lifecycle control
/// use this aggregate control surface instead of `RedisControl`.
// See the `RedisControl` definition above for why `#[async_trait]` is
// not used here (it would break `#[test_r::hosted_rpc]` async-mode
// detection). Same reasoning applies.
#[allow(async_fn_in_trait)]
#[test_r::hosted_rpc]
pub trait WorkerExecutorClusterControl {
    async fn kill_all_and_wait(&self, timeout_millis: u64) -> Result<(), String>;
    async fn restart_all(&self);
    async fn restart_all_with_env_vars(&self, vars: Vec<(String, String)>);
    async fn stop(&self, idx: u16);
    async fn start(&self, idx: u16);
    async fn pause(&self, idx: u16);
    async fn resume(&self, idx: u16);
    async fn started_indices(&self) -> Vec<u16>;
    async fn stopped_indices(&self) -> Vec<u16>;
    async fn is_running(&self, idx: u16) -> bool;
    /// Whether the executor at `idx` answers its gRPC health check right now. Stricter than
    /// [`Self::is_running`]: a process that is aborting still counts as running until the OS has
    /// finished with it, but it no longer serves.
    async fn is_serving(&self, idx: u16) -> bool;
    async fn cluster_size(&self) -> u16;

    async fn stop_shard_manager(&self);
    async fn start_shard_manager(&self, number_of_shards: Option<u16>);
    async fn restart_shard_manager(&self);

    async fn is_redis_healthy(&self) -> bool;
    async fn flush_redis_db(&self, db: u16) -> Result<(), String>;
    async fn redis_prefix(&self) -> String;
}

impl EnvBasedTestDependencies {
    /// A view of these dependencies whose clients reach the worker service through
    /// `worker_service`, such as a private service that a test restarts or a relay that drops
    /// connections. Every other dependency is shared with `self`. The view is for the calling
    /// test only; it is not a hosted dependency.
    pub fn with_worker_service(&self, worker_service: Arc<dyn WorkerService>) -> Self {
        Self {
            worker_service,
            ..self.clone()
        }
    }

    fn usize_to_u16(idx: usize) -> u16 {
        u16::try_from(idx).expect("worker executor cluster index does not fit into u16")
    }
}

impl WorkerExecutorClusterControl for EnvBasedTestDependencies {
    async fn kill_all_and_wait(&self, timeout_millis: u64) -> Result<(), String> {
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_millis);
        self.worker_executor_cluster
            .kill_all_and_wait(deadline)
            .await
            .map_err(|error| format!("{error:#}"))
    }

    async fn restart_all(&self) {
        self.worker_executor_cluster.restart_all().await;
    }

    async fn restart_all_with_env_vars(&self, vars: Vec<(String, String)>) {
        // Per-restart overrides are passed straight into each spawned child
        // process's environment via `Command::envs(...)`. We deliberately do
        // NOT touch the parent test runner's process-wide environment here:
        // `std::env::set_var` is unsound in a multi-threaded Rust 2024
        // process and racing HostedRpc dispatches plus other test threads
        // could observe partial state otherwise.
        self.worker_executor_cluster
            .restart_all_with_extra_env_vars(vars)
            .await;
    }

    async fn stop(&self, idx: u16) {
        self.worker_executor_cluster.stop(usize::from(idx)).await;
    }

    async fn start(&self, idx: u16) {
        self.worker_executor_cluster.start(usize::from(idx)).await;
    }

    async fn pause(&self, idx: u16) {
        self.worker_executor_cluster.pause(usize::from(idx)).await;
    }

    async fn resume(&self, idx: u16) {
        self.worker_executor_cluster.resume(usize::from(idx)).await;
    }

    async fn started_indices(&self) -> Vec<u16> {
        self.worker_executor_cluster
            .started_indices()
            .await
            .into_iter()
            .map(Self::usize_to_u16)
            .collect()
    }

    async fn stopped_indices(&self) -> Vec<u16> {
        self.worker_executor_cluster
            .stopped_indices()
            .await
            .into_iter()
            .map(Self::usize_to_u16)
            .collect()
    }

    async fn is_running(&self, idx: u16) -> bool {
        let worker_executors = self.worker_executor_cluster.to_vec();
        let Some(worker_executor) = worker_executors.get(usize::from(idx)).cloned() else {
            return false;
        };
        worker_executor.is_running().await
    }

    async fn is_serving(&self, idx: u16) -> bool {
        let worker_executors = self.worker_executor_cluster.to_vec();
        let Some(worker_executor) = worker_executors.get(usize::from(idx)).cloned() else {
            return false;
        };
        crate::components::is_serving_grpc(
            &worker_executor.grpc_host(),
            worker_executor.grpc_port(),
            Duration::from_secs(5),
        )
        .await
    }

    async fn cluster_size(&self) -> u16 {
        Self::usize_to_u16(self.worker_executor_cluster.size())
    }

    async fn stop_shard_manager(&self) {
        self.shard_manager.kill().await;
    }

    async fn start_shard_manager(&self, number_of_shards: Option<u16>) {
        self.shard_manager
            .restart(number_of_shards.map(usize::from))
            .await;
    }

    async fn restart_shard_manager(&self) {
        self.shard_manager.kill().await;
        self.shard_manager.restart(None).await;
    }

    async fn is_redis_healthy(&self) -> bool {
        <Self as RedisControl>::is_redis_healthy(self).await
    }

    async fn flush_redis_db(&self, db: u16) -> Result<(), String> {
        <Self as RedisControl>::flush_redis_db(self, db).await
    }

    async fn redis_prefix(&self) -> String {
        <Self as RedisControl>::redis_prefix(self).await
    }
}

impl Clone for WorkerExecutorClusterControlStub {
    fn clone(&self) -> Self {
        Self::new(self.channel.clone())
    }
}

impl test_r::core::AsyncHostedRpcDep for EnvBasedTestDependencies {
    type Stub = WorkerExecutorClusterControlStub;

    async fn dispatch(&mut self, method_idx: u32, args: &[u8]) -> Result<Vec<u8>, String> {
        WorkerExecutorClusterControlDispatch::dispatch_worker_executor_cluster_control(
            self, method_idx, args,
        )
        .await
    }

    fn build_stub(channel: test_r::core::HostedRpcChannel) -> Self::Stub {
        WorkerExecutorClusterControlStub::new(channel)
    }
}

#[async_trait]
impl TestDependencies for EnvBasedTestDependencies {
    fn rdb(&self) -> Arc<dyn Rdb> {
        self.rdb.clone()
    }

    fn redis(&self) -> Arc<dyn Redis> {
        self.redis.clone()
    }

    fn blob_storage(&self) -> Arc<dyn BlobStorage> {
        self.blob_storage.clone()
    }

    fn redis_monitor(&self) -> Arc<dyn RedisMonitor> {
        self.redis_monitor.clone()
    }

    fn shard_manager(&self) -> Arc<dyn ShardManager> {
        self.shard_manager.clone()
    }

    fn component_directory(&self) -> &Path {
        &self.test_component_dir
    }

    fn temp_directory(&self) -> &Path {
        self.temp_directory.path()
    }

    fn component_compilation_service(&self) -> Arc<dyn ComponentCompilationService> {
        self.component_compilation_service.clone()
    }

    fn worker_service(&self) -> Arc<dyn WorkerService> {
        self.worker_service.clone()
    }

    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster> {
        self.worker_executor_cluster.clone()
    }

    fn initial_agent_files_service(&self) -> Arc<InitialAgentFilesService> {
        self.initial_agent_files_service.clone()
    }

    fn registry_service(&self) -> Arc<dyn RegistryService> {
        self.registry_service.clone()
    }
}

fn opt_env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests;
