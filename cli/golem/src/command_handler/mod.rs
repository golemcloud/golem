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

use anyhow::{Context as _, anyhow, bail};
use clap_verbosity_flag::Verbosity;
use golem_cli::LOG_FILTER_ENV_VAR;
use golem_cli::command::server::{RunArgs, ServerSubcommand};
use golem_cli::command_handler::{CommandHandlerHooks, Handlers};
use golem_cli::config::{DEFAULT_LOCAL_CUSTOM_REQUEST_PORT, DEFAULT_LOCAL_MCP_PORT};
use golem_cli::context::Context;
use golem_cli::error::NonSuccessfulExit;
use golem_cli::fs;
use golem_cli::log::{LogColorize, Output as CliOutput, log_warn_action, set_log_output};
use golem_cli::model::app::ResolvedLocalServer;
use golem_common::tracing::{Output, TracingConfig, filter};
use golem_registry_service::services::builtin_artifact::BuiltinArtifactShutdown;
use golem_worker_executor::services::shutdown::{SHUTDOWN_GRACE, Shutdown};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};
use tracing_subscriber::{EnvFilter, filter::Directive};

use crate::compat::map_local_server_startup_error;
use crate::config::{GolemServerConfig, SnapshotRepositoryKeySource, load_server_config};
use crate::launch::{StartupPorts, launch_golem_services};
use crate::well_known_secrets::check_exposure;

pub struct ServerCommandHandler;

impl ServerCommandHandler {
    async fn handle_with_tracing<G>(
        &self,
        ctx: Arc<Context>,
        subcommand: ServerSubcommand,
        verbosity: Verbosity,
        initialize: impl FnOnce(&TracingConfig, &dyn Fn(Output) -> filter::Boxed) -> G,
    ) -> anyhow::Result<()> {
        match subcommand {
            ServerSubcommand::Run { args } => {
                set_log_output(if verbosity.tracing_level().is_none() {
                    CliOutput::BufferedUntilErr
                } else if ctx.format().is_structured() {
                    CliOutput::Stderr
                } else {
                    CliOutput::Stdout
                });
                if !ctx.server_no_limit_change() {
                    let file_limit_increase_result = rlimit::increase_nofile_limit(1000000);
                    debug!(
                        "File limit increase result: {:?}",
                        file_limit_increase_result
                    );
                }

                // The first load validates the configuration without touching the data
                // directory, so `--clean` cannot delete a repository key that the started
                // server uses. The second load takes the retained key, creating it if needed.
                let config = load_server_config(
                    ctx.manifest_local_server(),
                    &args,
                    SnapshotRepositoryKeySource::ValidationOnly,
                )?;
                check_exposure(&config)?;
                if args.clean && tokio::fs::metadata(&config.data_dir).await.is_ok() {
                    clean_server_data_dir(&ctx, &config).await?;
                };
                let config = load_server_config(
                    ctx.manifest_local_server(),
                    &args,
                    SnapshotRepositoryKeySource::DataDir,
                )?;
                check_exposure(&config)?;
                prepare_server_log(&config)?;
                let rust_log = std::env::var(EnvFilter::DEFAULT_ENV).unwrap_or_default();
                let defaults = if rust_log
                    .split(',')
                    .filter(|entry| !entry.is_empty())
                    .any(|entry| entry.parse::<Directive>().is_ok())
                {
                    ""
                } else {
                    "info"
                };
                let rmcp_default = if verbosity.is_present() && verbosity.tracing_level().is_none()
                {
                    ""
                } else {
                    "rmcp=warn"
                };
                // Keep the original text so span-field matchers retain their types.
                let mut ordinary_filter = EnvFilter::builder()
                    .parse_lossy(format!("{defaults},{rmcp_default},{rust_log}"));
                if verbosity.is_present() {
                    ordinary_filter = ordinary_filter.add_directive(
                        verbosity
                            .tracing_level()
                            .map_or("off", |level| level.as_str())
                            .parse()
                            .expect("tracing levels are valid directives"),
                    );
                }
                let ordinary_filter = std::env::var(LOG_FILTER_ENV_VAR)
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .fold(ordinary_filter, |filter, entry| {
                        match entry.parse::<Directive>() {
                            Ok(directive) => filter.add_directive(directive),
                            Err(err) => {
                                let _ = writeln!(
                                    golem_cli::log::tracing_writer(),
                                    "Ignoring invalid {LOG_FILTER_ENV_VAR} directive '{entry}': {err}"
                                );
                                filter
                            }
                        }
                    });
                let make_filter = |output| -> filter::Boxed {
                    match output {
                        Output::Otlp => filter::for_all_outputs::DEFAULT_ENV(output),
                        _ => Box::new(ordinary_filter.clone()),
                    }
                };
                let _tracer = initialize(&config.logging, &make_filter);
                let data_dir = config.data_dir.clone();

                // A single pinned signal future is shared by both phases below, so the
                // handlers are installed once and a signal arriving between the two
                // select! blocks is not lost.
                let shutdown = shutdown_signal();
                tokio::pin!(shutdown);

                // Shared with `launch_golem_services` below, so a built-in artifact download or
                // metadata extraction still running inside it (on a detached `spawn_blocking`
                // task `select!` cannot reach by dropping the future) can be stopped. Cancelling
                // alone only asks it to notice; this process must also await `shutdown()` before
                // returning, so it does not drop the Tokio runtime while a blocking task is
                // still parked on a timer-bearing future that the runtime would tear down from
                // under it, which panics and aborts the process (the bug this fixes).
                let builtin_artifact_shutdown = BuiltinArtifactShutdown::new();
                let launch_result = tokio::select! {
                    res = launch_golem_services(&config, builtin_artifact_shutdown.clone()) => Some(res),
                    _ = &mut shutdown => None,
                };

                let Some(launch_result) = launch_result else {
                    builtin_artifact_shutdown.shutdown().await;
                    info!("Received shutdown signal during startup, stopping Golem server");
                    return Ok(());
                };

                let (mut join_set, startup_ports, worker_shutdown) =
                    launch_result.map_err(|err| map_local_server_startup_error(err, &data_dir))?;

                // Subdomains of the manifest's built-in local environments are expanded from
                // the `localServer` ports or their defaults, so the check applies whenever the
                // manifest has such an environment (with or without a `localServer` section);
                // which environment the CLI has selected is irrelevant to the local server.
                if ctx.manifest_has_builtin_local_environment() {
                    warn_on_subdomain_port_mismatches(ctx.manifest_local_server(), &startup_ports);
                }

                let run_result = tokio::select! {
                    res = async {
                        while let Some(res) = join_set.join_next().await {
                            res??;
                        }
                        Ok::<(), anyhow::Error>(())
                    } => Some(res),
                    _ = &mut shutdown => None,
                };

                match run_result {
                    Some(res) => res,
                    None => {
                        info!("Received shutdown signal, stopping Golem server");
                        shutdown_golem_services(&mut join_set, &worker_shutdown).await;
                        Ok(())
                    }
                }
            }
            ServerSubcommand::Clean { config } => {
                let config = load_server_config(
                    ctx.manifest_local_server(),
                    &RunArgs {
                        config,
                        ..RunArgs::default()
                    },
                    SnapshotRepositoryKeySource::ValidationOnly,
                )?;
                clean_server_data_dir(&ctx, &config).await
            }
        }
    }
}

fn prepare_server_log(config: &GolemServerConfig) -> anyhow::Result<()> {
    use golem_common::config::{DbConfig, DbSqliteConfig};
    use golem_service_base::config::BlobStorageConfig;
    use golem_shard_manager::config::PersistenceConfig;
    use golem_worker_executor::services::golem_config::{
        IndexedStorageConfig, KeyValueStorageConfig, KeyValueStorageInnerConfig,
        SchedulerStorageConfig,
    };
    use golem_worker_service::config::SessionStoreConfig;
    use golem_worker_service::mcp::session::McpSessionStorageConfig;

    let Some(file_name) = config
        .logging
        .file_name
        .as_deref()
        .filter(|_| config.logging.file.enabled)
    else {
        return Ok(());
    };
    let log_path = Path::new(config.logging.file_dir.as_deref().unwrap_or("."))
        .join(file_name)
        .into_boxed_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create the server log directory")?;
    }
    let log_metadata = match std::fs::metadata(&log_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Failed to inspect the server log"),
    };
    let check_path = |path: &Path, description: &str| -> anyhow::Result<()> {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to inspect the {description}"));
            }
        };
        #[cfg(unix)]
        let same_file = {
            use std::os::unix::fs::MetadataExt;
            metadata.dev() == log_metadata.dev() && metadata.ino() == log_metadata.ino()
        };
        #[cfg(windows)]
        let same_file = {
            use std::os::windows::fs::OpenOptionsExt;
            if metadata.is_file() && log_metadata.is_file() {
                let handle = |path: &Path| {
                    std::fs::OpenOptions::new()
                        .access_mode(0)
                        .open(path)
                        .and_then(same_file::Handle::from_file)
                };
                handle(path)? == handle(&log_path)?
            } else {
                false
            }
        };
        if same_file {
            bail!("The server log must not overwrite the {description}");
        }
        Ok(())
    };
    check_path(
        &config.data_dir.join(crate::filesystem_config::KEY_FILE),
        "retained repository key",
    )?;
    let check_sqlite_path = |path: &Path| -> anyhow::Result<()> {
        fn resolve_database(path: &Path) -> std::io::Result<PathBuf> {
            match std::fs::canonicalize(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match std::fs::read_link(path) {
                        Ok(target) => resolve_database(&path.parent().unwrap().join(target)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            Ok(path.to_path_buf())
                        }
                        Err(error) => Err(error),
                    }
                }
                result => result,
            }
        }

        let database =
            resolve_database(path).context("Failed to inspect selected SQLite database")?;
        ["", "-wal", "-shm", "-journal"]
            .into_iter()
            .try_for_each(|suffix| {
                let mut path = database.as_os_str().to_os_string();
                path.push(suffix);
                check_path(
                    Path::new(&path),
                    "selected SQLite database or recovery file",
                )
            })
    };
    let check_sqlite = |sqlite: &DbSqliteConfig| check_sqlite_path(Path::new(&sqlite.database));
    let is_agent_file_key = |key: &str| {
        key.split_once('_').is_some_and(|(environment, agent)| {
            [environment, agent].into_iter().all(|id| {
                uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id)
            })
        })
    };
    let is_key_value_database = |name: &str, persistent: bool| {
        (persistent
            && matches!(
                name,
                "kv-running_workers" | "kv-schedule.db" | "kv-user-defined.db"
            ))
            || name
                .strip_prefix("kv-worker-")
                .and_then(|key| key.strip_suffix(".db"))
                .is_some_and(is_agent_file_key)
    };
    let is_indexed_database = |name: &str| {
        name == "epochs.db"
            || name
                .strip_suffix(".db")
                .and_then(|name| {
                    name.strip_prefix("durable-")
                        .or_else(|| name.strip_prefix("ephemeral-"))
                })
                .and_then(|name| {
                    name.strip_prefix("oplog-").or_else(|| {
                        name.strip_prefix("compressed-oplog-l")
                            .or_else(|| name.strip_prefix("blob-oplog-l"))
                            .and_then(|name| name.split_once('-'))
                            .and_then(|(level, key)| {
                                level
                                    .parse::<usize>()
                                    .is_ok_and(|value| value.to_string() == level)
                                    .then_some(key)
                            })
                    })
                })
                .is_some_and(is_agent_file_key)
    };
    let check_multi_sqlite = |root: &Path, selected: &dyn Fn(&str) -> bool| -> anyhow::Result<()> {
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("Failed to inspect selected SQLite databases"),
        };
        entries.into_iter().try_for_each(|entry| {
            let entry = entry?;
            let name = entry.file_name();
            if let Some(name) = name.to_str() {
                let database = ["-wal", "-shm", "-journal"]
                    .into_iter()
                    .find_map(|suffix| name.strip_suffix(suffix))
                    .unwrap_or(name);
                if selected(database) {
                    check_sqlite_path(&root.join(database))?;
                }
            }
            Ok(())
        })
    };
    [
        match &config.registry_service.db {
            DbConfig::Sqlite(sqlite) => Some(sqlite),
            _ => None,
        },
        match &config.shard_manager.persistence {
            PersistenceConfig::Sqlite(sqlite) => Some(sqlite),
            _ => None,
        },
        match &config.worker_service.gateway_session_storage {
            SessionStoreConfig::Sqlite(sqlite) => Some(&sqlite.sqlite_config),
            _ => None,
        },
        match &config.worker_service.mcp.session.session_storage {
            Some(McpSessionStorageConfig::Sqlite(sqlite)) => Some(&sqlite.sqlite_config),
            _ => None,
        },
        match &config.worker_executor.scheduler_storage {
            SchedulerStorageConfig::Sqlite(sqlite) => Some(sqlite),
            _ => None,
        },
    ]
    .into_iter()
    .flatten()
    .try_for_each(check_sqlite)?;
    [
        &config.registry_service.blob_storage,
        &config.worker_service.blob_storage,
        &config.worker_executor.blob_storage,
        &config.component_compilation_service.blob_storage,
    ]
    .into_iter()
    .try_for_each(|blob| match blob {
        BlobStorageConfig::Sqlite(sqlite) => check_sqlite(sqlite),
        _ => Ok(()),
    })?;
    match &config.worker_executor.key_value_storage {
        KeyValueStorageConfig::Sqlite(sqlite) => check_sqlite(sqlite)?,
        KeyValueStorageConfig::MultiSqlite(sqlite) => {
            check_multi_sqlite(&sqlite.root_dir, &|name| is_key_value_database(name, true))?
        }
        KeyValueStorageConfig::NamespaceRouted(routed) => {
            [(&routed.cache, false), (&routed.persistent, true)]
                .into_iter()
                .try_for_each(|(inner, persistent)| match inner {
                    KeyValueStorageInnerConfig::Sqlite(sqlite) => check_sqlite(sqlite),
                    KeyValueStorageInnerConfig::MultiSqlite(sqlite) => {
                        check_multi_sqlite(&sqlite.root_dir, &|name| {
                            is_key_value_database(name, persistent)
                        })
                    }
                    _ => Ok(()),
                })?
        }
        _ => {}
    }
    match &config.worker_executor.indexed_storage {
        IndexedStorageConfig::Sqlite(sqlite) => check_sqlite(sqlite),
        IndexedStorageConfig::MultiSqlite(sqlite) => {
            check_multi_sqlite(&sqlite.root_dir, &is_indexed_database)
        }
        IndexedStorageConfig::KVStoreSqlite(_) => match &config.worker_executor.key_value_storage {
            KeyValueStorageConfig::Sqlite(sqlite) => check_sqlite(
                &golem_worker_executor::derive_disjoint_sqlite_config(sqlite, "indexed"),
            ),
            _ => Ok(()),
        },
        IndexedStorageConfig::KVStoreMultiSqlite(_) => {
            match &config.worker_executor.key_value_storage {
                KeyValueStorageConfig::MultiSqlite(sqlite) => {
                    check_multi_sqlite(&sqlite.root_dir, &is_indexed_database)
                }
                _ => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

impl CommandHandlerHooks for ServerCommandHandler {
    async fn handler_server_commands(
        &self,
        ctx: Arc<Context>,
        subcommand: ServerSubcommand,
        verbosity: Verbosity,
    ) -> anyhow::Result<()> {
        self.handle_with_tracing(ctx, subcommand, verbosity, |logging, make_filter| {
            golem_common::tracing::init_tracing(logging, make_filter)
        })
        .await
    }

    async fn run_server() -> anyhow::Result<()> {
        let config = load_server_config(
            None,
            &RunArgs::default(),
            SnapshotRepositoryKeySource::DataDir,
        )?;
        let data_dir = config.data_dir.clone();

        let (mut join_set, _, _) = launch_golem_services(&config, BuiltinArtifactShutdown::never())
            .await
            .map_err(|err| map_local_server_startup_error(err, &data_dir))?;

        tokio::spawn(async move {
            while let Some(res) = join_set.join_next().await {
                res.unwrap().unwrap();
            }
        });

        Ok(())
    }

    fn override_verbosity(verbosity: Verbosity) -> Verbosity {
        if verbosity.is_present() {
            verbosity
        } else {
            Verbosity::new(2, 0)
        }
    }

    fn override_pretty_mode() -> bool {
        true
    }
}

async fn shutdown_golem_services(
    join_set: &mut JoinSet<anyhow::Result<()>>,
    worker_shutdown: &Shutdown,
) {
    worker_shutdown.cancel();
    if !worker_shutdown.wait_for_tracked(SHUTDOWN_GRACE).await {
        warn!(
            grace = ?SHUTDOWN_GRACE,
            "Background tasks did not finish within the shutdown grace period"
        );
    }
    join_set.shutdown().await;
}

/// A local server port that differs from the one the manifest's deployment `subdomain`
/// expansion uses, e.g. because it was overridden with a flag or requested as `0`.
#[derive(Debug, PartialEq, Eq)]
struct SubdomainPortMismatch {
    deployment_kind: &'static str,
    manifest_field: &'static str,
    flag: &'static str,
    expanded_port: u16,
    bound_port: u16,
}

/// Deployment subdomains expand to `<label>.localhost:<port>` using the manifest's
/// `localServer.customRequestPort` / `localServer.mcpPort` (or their defaults), and a request is
/// only routed when its `Host` header matches that expansion exactly. A server bound to a
/// different port cannot serve those deployments, so the ports have to match. `local_server` is
/// `None` when the manifest has no `localServer` section, in which case the defaults apply.
fn subdomain_port_mismatches(
    local_server: Option<&ResolvedLocalServer>,
    startup_ports: &StartupPorts,
) -> Vec<SubdomainPortMismatch> {
    let checks = [
        (
            "HTTP API",
            "localServer.customRequestPort",
            "--custom-request-port",
            local_server
                .and_then(|local_server| local_server.custom_request_port)
                .unwrap_or(DEFAULT_LOCAL_CUSTOM_REQUEST_PORT),
            startup_ports.custom_request_port,
        ),
        (
            "MCP",
            "localServer.mcpPort",
            "--mcp-port",
            local_server
                .and_then(|local_server| local_server.mcp_port)
                .unwrap_or(DEFAULT_LOCAL_MCP_PORT),
            startup_ports.mcp_port,
        ),
    ];

    checks
        .into_iter()
        .filter(|(_, _, _, expanded_port, bound_port)| expanded_port != bound_port)
        .map(
            |(deployment_kind, manifest_field, flag, expanded_port, bound_port)| {
                SubdomainPortMismatch {
                    deployment_kind,
                    manifest_field,
                    flag,
                    expanded_port,
                    bound_port,
                }
            },
        )
        .collect()
}

fn warn_on_subdomain_port_mismatches(
    local_server: Option<&ResolvedLocalServer>,
    startup_ports: &StartupPorts,
) {
    for mismatch in subdomain_port_mismatches(local_server, startup_ports) {
        log_warn_action(
            "Bound",
            format!(
                "{} port {} differs from port {} used by {} deployment subdomains ({}); those deployments are not reachable on this server, use the same value for {} and {}",
                mismatch.deployment_kind,
                mismatch.bound_port.to_string().log_color_highlight(),
                mismatch.expanded_port.to_string().log_color_highlight(),
                mismatch.deployment_kind,
                mismatch.manifest_field.log_color_highlight(),
                mismatch.manifest_field.log_color_highlight(),
                mismatch.flag.log_color_highlight(),
            ),
        );
    }
}

fn resolve_clean_data_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let data_dir = fs::absolute_lexical_path(data_dir)?;
    let Some(parent) = data_dir.parent() else {
        bail!(
            "Refusing to clean filesystem root {}",
            data_dir.display().to_string().log_color_highlight()
        );
    };
    let file_name = data_dir
        .file_name()
        .ok_or_else(|| anyhow!("Data directory {} has no name", data_dir.display()))?;
    let resolved_parent = std::fs::canonicalize(parent).with_context(|| {
        format!(
            "Failed to resolve parent of local server data directory {}",
            data_dir.display()
        )
    })?;

    Ok(resolved_parent.join(file_name))
}

/// Deletes the data directory of `config`, when it holds all server state.
async fn clean_server_data_dir(
    ctx: &Arc<Context>,
    config: &GolemServerConfig,
) -> anyhow::Result<()> {
    let clean_root = resolve_clean_data_dir(&config.data_dir)?;
    let outside = config.state_outside(&clean_root);
    if !outside.is_empty() {
        bail!(
            "Refusing to clean {}: these stores keep state outside the data directory, so \
             deleting it does not delete the server state: {}",
            clean_root.display().to_string().log_color_highlight(),
            outside.join(", ")
        );
    }
    clean_data_dir(ctx, &config.data_dir).await
}

async fn clean_data_dir(ctx: &Arc<Context>, data_dir: &Path) -> anyhow::Result<()> {
    let data_dir = resolve_clean_data_dir(data_dir)?;
    if !ctx
        .interactive_handler()
        .confirm_clean_local_server_data_dir(&data_dir)?
    {
        bail!(NonSuccessfulExit);
    }

    log_warn_action(
        "Cleaning",
        format!(
            "local server data directory {}",
            data_dir.display().to_string().log_color_highlight()
        ),
    );
    tokio::fs::remove_dir_all(&data_dir)
        .await
        .map_err(|err| anyhow!("Failed cleaning data dir ({}): {}", data_dir.display(), err))
}

/// Resolves when the process receives a shutdown request.
///
/// Handlers must be installed explicitly: as PID 1 in a container the kernel
/// does not apply default signal dispositions, so without this the standalone
/// server cannot be stopped by SIGINT/SIGTERM (e.g. Ctrl+C or `docker stop`).
async fn shutdown_signal() {
    signals::shutdown_signal().await
}

#[cfg(unix)]
mod signals {
    pub(super) async fn shutdown_signal() {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");

        tokio::select! {
            _ = sigint.recv() => {},
            _ = sigterm.recv() => {},
        }
    }
}

#[cfg(not(unix))]
mod signals {
    pub(super) async fn shutdown_signal() {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    }
}

#[cfg(test)]
mod tests;
