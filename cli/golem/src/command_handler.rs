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
use golem_cli::log::{LogColorize, log_warn_action};
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
                    "rmcp=warn"
                } else {
                    "info,rmcp=warn"
                };
                // Keep the original text so span-field matchers retain their types.
                let mut ordinary_filter =
                    EnvFilter::builder().parse_lossy(format!("{defaults},{rust_log}"));
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
    let key_path = config
        .data_dir
        .join(crate::filesystem_config::KEY_FILE)
        .into_boxed_path();
    let key_metadata = match std::fs::metadata(&key_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Failed to inspect the retained repository key"),
    };
    let log_metadata = match std::fs::metadata(&log_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Failed to inspect the server log"),
    };
    #[cfg(unix)]
    let same_file = {
        use std::os::unix::fs::MetadataExt;
        key_metadata.dev() == log_metadata.dev() && key_metadata.ino() == log_metadata.ino()
    };
    #[cfg(windows)]
    let same_file = {
        use std::os::windows::fs::OpenOptionsExt;
        if key_metadata.is_file() && log_metadata.is_file() {
            let handle = |path: &Path| {
                std::fs::OpenOptions::new()
                    .access_mode(0)
                    .open(path)
                    .and_then(same_file::Handle::from_file)
            };
            handle(&key_path)? == handle(&log_path)?
        } else {
            false
        }
    };
    if same_file {
        bail!("The server log must not overwrite the retained repository key");
    }
    Ok(())
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
mod tests {
    use super::*;
    use golem_cli::model::app_raw::LocalServer;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;
    use tracing_subscriber::{Layer, layer::SubscriberExt};

    fn local_server(value: LocalServer) -> ResolvedLocalServer {
        ResolvedLocalServer::from_raw_with_base_dir(&value, Path::new("/tmp/test-app"))
    }

    #[derive(Debug)]
    struct TracingInitialized;

    #[test]
    fn server_tracing_receives_loaded_settings_after_confirmed_cleanup() {
        let off = [false; 4];
        let info = [true, true, false, false];
        let debug = [true; 4];
        let targeted = [false, false, false, true];
        [
            (None, 0, 0, "off", info, off),
            (Some("off"), 0, 0, "debug", off, debug),
            (Some("debug"), 0, 0, "off", debug, off),
            (Some("off"), 3, 0, "off", debug, off),
            (Some("debug"), 0, 1, "debug", off, debug),
            (Some("logger_probe=debug"), 0, 0, "off", targeted, off),
            (Some("off,logger_probe=debug"), 0, 1, "debug", targeted, debug),
            (Some("debug"), 1, 0, "off", [true, false, false, false], off),
            (Some("debug"), 0, 1, "off", off, off),
        ]
        .into_iter()
        .for_each(|(rust_log, verbose, quiet, otlp_filter, expected, expected_otlp)| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                jail.set_env("HOME", jail.directory().display().to_string());
                let data_dir = jail.directory().join("data");
                let log_dir = data_dir.join("logs");
                let log_path = log_dir.join("golem.log");
                let old_state = data_dir.join("old-state");
                std::fs::create_dir_all(&log_dir).unwrap();
                std::fs::write(&old_state, b"old state").unwrap();
                std::fs::write(&log_path, b"old log").unwrap();
                jail.set_env("GOLEM__LOGGING__FILE_DIR", log_dir.display().to_string());
                if let Some(rust_log) = rust_log {
                    jail.set_env("RUST_LOG", rust_log);
                }
                jail.set_env("GOLEM_OTLP_FILTER", otlp_filter);
                jail.create_file(
                    "settings.toml",
                    r#"
                [logging.stdout]
                enabled = false
                json = true
                [logging.file]
                enabled = true
                [logging]
                file_name = "golem.log"
                file_truncate = true
                [filesystem_snapshots]
                enabled = false
                [otlp]
                enabled = true
                host = "collector.startup.test"
                port = 14318
            "#,
                )?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(async {
                    let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                    flags.local = true;
                    flags.disable_app_manifest_discovery = true;
                    flags.server_no_limit_change = true;
                    flags.yes = true;
                    let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                    ServerCommandHandler
                        .handle_with_tracing(
                            ctx,
                            ServerSubcommand::Run {
                                args: RunArgs {
                                    config: Some("settings.toml".into()),
                                    data_dir: Some(data_dir.clone()),
                                    router_addr: Some("127.0.0.1".to_string()),
                                    clean: true,
                                    ..Default::default()
                                },
                            },
                            Verbosity::new(verbose, quiet),
                            |logging, make_filter| {
                                assert!(!old_state.exists(), "cleanup precedes tracing");
                                assert!(!logging.stdout.enabled);
                                assert!(logging.stdout.json);
                                assert!(logging.otlp.enabled);
                                assert_eq!(logging.otlp.host, "collector.startup.test");
                                assert_eq!(logging.otlp.port, 14318);
                                [
                                    ("stdout", Output::Stdout, expected),
                                    ("file", Output::File, expected),
                                    ("otlp", Output::Otlp, expected_otlp),
                                ]
                                .into_iter()
                                .for_each(|(output_name, output, expected)| {
                                    let subscriber = tracing_subscriber::registry().with(
                                        tracing_subscriber::fmt::layer()
                                            .with_writer(std::io::sink)
                                            .with_filter(make_filter(output)),
                                    );
                                    let observed = tracing::subscriber::with_default(subscriber, || {
                                        [
                                            tracing::enabled!(target: "unrelated_probe", tracing::Level::ERROR),
                                            tracing::enabled!(target: "unrelated_probe", tracing::Level::INFO),
                                            tracing::enabled!(target: "unrelated_probe", tracing::Level::DEBUG),
                                            tracing::enabled!(target: "logger_probe", tracing::Level::DEBUG),
                                        ]
                                    });
                                    assert_eq!(
                                        observed, expected,
                                        "{output_name}: RUST_LOG={rust_log:?}, -v={verbose}, -q={quiet}, GOLEM_OTLP_FILTER={otlp_filter}"
                                    );
                                });
                                let destination = Path::new(logging.file_dir.as_deref().unwrap_or("."))
                                    .join(logging.file_name.as_ref().unwrap());
                                assert_eq!(destination, log_path);
                                assert!(logging.file.enabled);
                                assert!(logging.file_truncate);
                                let writer = std::fs::OpenOptions::new()
                                    .write(true)
                                    .create(true)
                                    .truncate(logging.file_truncate)
                                    .open(&destination)
                                    .unwrap();
                                let subscriber = tracing_subscriber::fmt()
                                    .with_ansi(false)
                                    .with_writer(Arc::new(writer))
                                    .finish();
                                tracing::subscriber::with_default(subscriber, || {
                                    tracing::info!("post-clean-log-event");
                                });
                                let contents = std::fs::read_to_string(&destination).unwrap();
                                assert!(contents.contains("post-clean-log-event"), "{contents}");
                                assert!(!contents.contains("old log"));
                                std::panic::panic_any(TracingInitialized);
                            },
                        )
                        .await.unwrap();
                })));
                assert!(stopped.unwrap_err().is::<TracingInitialized>());
                Ok(())
            });
        });
    }

    #[test]
    fn server_tracing_preserves_target_filters_and_mcp_suppression() {
        let off = [false; 9];
        let mcp = [false, true, true, false, false, false, false, false, false];
        let warning = [false, false, true, false, false, false, false, false, false];
        let verbose = [true, false, true, true, true, true, true, true, true];
        let no_mcp = [true, false, false, true, true, true, true, true, true];
        [
            (None, None, 3, "off,rmcp=debug", verbose, mcp),
            (Some("rmcp=off"), None, 3, "off,rmcp=debug", no_mcp, mcp),
            (Some("rmcp=debug"), None, 0, "off", mcp, off),
            (Some("off"), Some("rmcp=debug"), 0, "off", mcp, off),
            (Some("debug"), Some("rmcp=off"), 0, "off,rmcp=debug", no_mcp, mcp),
            (Some(""), None, 0, "off", warning, off),
            (Some("rmcp=definitely_invalid"), None, 0, "off", warning, off),
            (Some("rmcp=debug,rmcp=definitely_invalid"), None, 0, "off", mcp, off),
            (
                Some("off"),
                Some(" , logger_probe=debug , rmcp=definitely_invalid , , rmcp=error , "),
                0,
                "off,rmcp=debug",
                [true, false, false, true, true, true, true, true, false],
                mcp,
            ),
            (
                Some("off,rmcp=off,[request{ratio=1.0}]=debug"),
                None,
                0,
                "off",
                [false, false, false, true, false, false, false, false, false],
                off,
            ),
            (
                Some("off,rmcp=off,[request{ratio=1}]=debug"),
                None,
                0,
                "off",
                [false, false, false, false, true, false, false, false, false],
                off,
            ),
            (
                Some("off,rmcp=off,field_presence_probe[{id}]=debug"),
                None,
                0,
                "off",
                [false, false, false, false, false, false, true, false, true],
                off,
            ),
            (
                Some("off,rmcp=off"),
                Some("field_presence_probe[{id}]=debug"),
                0,
                "off",
                [false, false, false, false, false, false, false, false, true],
                off,
            ),
            (
                Some("off,rmcp=off,field_presence_probe[{id}]=debug"),
                Some("field_presence_probe[{id}]=off"),
                0,
                "off",
                [false, false, false, false, false, false, true, false, false],
                off,
            ),
        ]
        .into_iter()
        .for_each(|(rust_log, extra, verbose, otlp, ordinary, exported)| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                jail.set_env("HOME", jail.directory().display().to_string());
                if let Some(value) = rust_log {
                    jail.set_env("RUST_LOG", value);
                }
                if let Some(value) = extra {
                    jail.set_env(golem_cli::LOG_FILTER_ENV_VAR, value);
                }
                jail.set_env("GOLEM_OTLP_FILTER", otlp);
                jail.create_file("settings.toml", "[filesystem_snapshots]\nenabled = false")?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async {
                        let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                        flags.local = true;
                        flags.disable_app_manifest_discovery = true;
                        flags.server_no_limit_change = true;
                        let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                        ServerCommandHandler
                            .handle_with_tracing(
                                ctx,
                                ServerSubcommand::Run {
                                    args: RunArgs {
                                        config: Some("settings.toml".into()),
                                        data_dir: Some(jail.directory().join("data")),
                                        router_addr: Some("127.0.0.1".into()),
                                        ..Default::default()
                                    },
                                },
                                Verbosity::new(verbose, 0),
                                |_, make_filter| {
                                    let layers = [
                                        ("stdout", Output::Stdout),
                                        ("stderr", Output::Stderr),
                                        ("file", Output::File),
                                        ("otlp", Output::Otlp),
                                    ]
                                    .into_iter()
                                    .map(|(name, output)| {
                                        let writer = Arc::new(
                                            std::fs::File::create(jail.directory().join(name))
                                                .unwrap(),
                                        );
                                        tracing_subscriber::fmt::layer()
                                            .without_time()
                                            .with_ansi(false)
                                            .with_writer(writer)
                                            .with_filter(make_filter(output))
                                            .boxed()
                                    })
                                    .collect::<Vec<_>>();
                                    let subscriber = tracing_subscriber::registry().with(layers);
                                    tracing::subscriber::with_default(subscriber, || {
                                        tracing::debug!(target: "logger_probe", "general-debug");
                                        tracing::debug!(target: "rmcp::service", "synthetic-mcp-body");
                                        tracing::warn!(target: "rmcp::service", "mcp-warning");
                                        {
                                            let span = tracing::info_span!("request", ratio = 1.0_f64);
                                            let _entered = span.enter();
                                            tracing::debug!(target: "logger_probe", "float-span-event");
                                        }
                                        {
                                            let span = tracing::info_span!("request", ratio = 1_u64);
                                            let _entered = span.enter();
                                            tracing::debug!(target: "logger_probe", "integer-span-event");
                                        }
                                        tracing::debug!(target: "logger_probe", "field-before");
                                        {
                                            let span = tracing::info_span!(target: "field_presence_probe", "request", id = 7_u64);
                                            let _entered = span.enter();
                                            tracing::debug!(target: "logger_probe", "field-inside");
                                        }
                                        tracing::debug!(target: "logger_probe", "field-after");
                                        tracing::debug!(target: "field_presence_probe", id = 7_u64, "field-direct");
                                    });
                                    [
                                        ("stdout", ordinary),
                                        ("stderr", ordinary),
                                        ("file", ordinary),
                                        ("otlp", exported),
                                    ]
                                    .into_iter()
                                    .for_each(|(name, expected)| {
                                        let contents = std::fs::read_to_string(
                                            jail.directory().join(name),
                                        )
                                        .unwrap();
                                        [
                                            "general-debug",
                                            "synthetic-mcp-body",
                                            "mcp-warning",
                                            "float-span-event",
                                            "integer-span-event",
                                            "field-before",
                                            "field-inside",
                                            "field-after",
                                            "field-direct",
                                        ]
                                        .into_iter()
                                        .zip(expected)
                                        .for_each(|(marker, expected)| {
                                            assert_eq!(
                                                contents.contains(marker),
                                                expected,
                                                "{name}: {marker}, RUST_LOG={rust_log:?}, GOLEM_LOG_FILTER={extra:?}, -v={verbose}, GOLEM_OTLP_FILTER={otlp}\n{contents}"
                                            );
                                        });
                                    });
                                    std::panic::panic_any(TracingInitialized);
                                },
                            )
                            .await
                            .unwrap();
                    })
                }));
                let panic = stopped.expect_err("server startup must reach tracing");
                if !panic.is::<TracingInitialized>() {
                    std::panic::resume_unwind(panic);
                }
                Ok(())
            });
        });
    }

    #[test]
    fn server_tracing_preserves_logs_when_configuration_refuses_startup() {
        [true, false].into_iter().for_each(|public_exposure| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                jail.set_env("HOME", jail.directory().display().to_string());
                let data_dir = jail.directory().join("data");
                std::fs::create_dir_all(&data_dir).unwrap();
                let log_path = data_dir.join("golem.log");
                let old_state = data_dir.join("old-state");
                let key_path = data_dir.join(crate::filesystem_config::KEY_FILE);
                std::fs::write(&log_path, b"preserve this log").unwrap();
                std::fs::write(&old_state, b"preserve this state").unwrap();
                std::fs::write(&key_path, b"corrupt").unwrap();
                jail.set_env("GOLEM__LOGGING__FILE_DIR", data_dir.display().to_string());
                jail.create_file(
                    "settings.toml",
                    &format!(
                        r#"
                    [logging]
                    file_name = "golem.log"
                    file_truncate = true
                    [logging.file]
                    enabled = true
                    [filesystem_snapshots]
                    enabled = {}
                "#,
                        !public_exposure
                    ),
                )?;
                let args = RunArgs {
                    config: Some("settings.toml".into()),
                    data_dir: Some(data_dir),
                    router_addr: Some(
                        if public_exposure {
                            "0.0.0.0"
                        } else {
                            "127.0.0.1"
                        }
                        .into(),
                    ),
                    clean: public_exposure,
                    ..Default::default()
                };
                load_server_config(None, &args, SnapshotRepositoryKeySource::ValidationOnly)
                    .unwrap();
                let called = std::cell::Cell::new(false);
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result = runtime.block_on(async {
                    let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                    flags.local = true;
                    flags.disable_app_manifest_discovery = true;
                    flags.server_no_limit_change = true;
                    flags.yes = true;
                    let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                    ServerCommandHandler
                        .handle_with_tracing(
                            ctx,
                            ServerSubcommand::Run { args },
                            Verbosity::new(0, 0),
                            |logging, _| {
                                called.set(true);
                                let destination =
                                    Path::new(logging.file_dir.as_deref().unwrap_or("."))
                                        .join(logging.file_name.as_ref().unwrap());
                                std::fs::OpenOptions::new()
                                    .write(true)
                                    .create(true)
                                    .truncate(logging.file_truncate)
                                    .open(destination)
                                    .unwrap();
                            },
                        )
                        .await
                });
                let message = format!("{:#}", result.unwrap_err());
                assert!(
                    message.contains(if public_exposure {
                        "Refusing to listen"
                    } else {
                        "repository_key must be 128 hex characters"
                    }),
                    "{message}"
                );
                assert!(!called.get(), "refusal precedes tracing");
                assert_eq!(std::fs::read(&log_path).unwrap(), b"preserve this log");
                assert_eq!(std::fs::read(&old_state).unwrap(), b"preserve this state");
                assert_eq!(std::fs::read(&key_path).unwrap(), b"corrupt");
                Ok(())
            });
        });
    }

    #[test]
    fn server_tracing_does_not_overwrite_the_retained_repository_key() {
        [
            "direct",
            "hard-link",
            "missing-parent",
            "noncolliding",
            #[cfg(unix)]
            "symlink",
        ]
        .into_iter()
        .flat_map(|alias| {
            [(false, false), (true, false), (true, true)]
                .into_iter()
                .map(move |(enabled, explicit_key)| (alias, enabled, explicit_key))
        })
        .for_each(|(alias, enabled, explicit_key)| {
            figment::Jail::expect_with(|jail| {
                jail.clear_env();
                jail.set_env("HOME", jail.directory().display().to_string());
                let data_dir = jail.directory().join("data");
                std::fs::create_dir_all(&data_dir).unwrap();
                let key_path = data_dir.join(crate::filesystem_config::KEY_FILE);
                let key = "2a".repeat(64);
                std::fs::write(&key_path, &key).unwrap();
                let log_path = match alias {
                    "direct" => key_path.clone(),
                    "missing-parent" => data_dir
                        .join("not-yet")
                        .join("..")
                        .join(crate::filesystem_config::KEY_FILE),
                    _ => data_dir.join("server.log"),
                };
                match alias {
                    "hard-link" => std::fs::hard_link(&key_path, &log_path).unwrap(),
                    #[cfg(unix)]
                    "symlink" => std::os::unix::fs::symlink(&key_path, &log_path).unwrap(),
                    _ => {}
                }
                jail.set_env("GOLEM__LOGGING__FILE_NAME", log_path.display().to_string());
                jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__ENABLED", enabled);
                if explicit_key {
                    jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__REPOSITORY_KEY", "3b".repeat(64));
                }
                jail.create_file(
                    "settings.toml",
                    r#"
                    [logging]
                    file_truncate = true
                    [logging.file]
                    enabled = true
                    [filesystem_snapshots]
                    enabled = true
                "#,
                )?;
                let called = std::cell::Cell::new(false);
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(async {
                    let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                    flags.local = true;
                    flags.disable_app_manifest_discovery = true;
                    flags.server_no_limit_change = true;
                    let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                    ServerCommandHandler
                        .handle_with_tracing(
                            ctx,
                            ServerSubcommand::Run {
                                args: RunArgs {
                                    config: Some("settings.toml".into()),
                                    data_dir: Some(data_dir),
                                    router_addr: Some("127.0.0.1".into()),
                                    ..Default::default()
                                },
                            },
                            Verbosity::new(0, 0),
                            |_, _| {
                                called.set(true);
                                std::fs::write(&log_path, b"log would replace key").unwrap();
                                std::panic::panic_any(TracingInitialized);
                            },
                        )
                        .await
                })));
                if alias == "noncolliding" {
                    assert!(result.unwrap_err().is::<TracingInitialized>());
                    assert!(called.get());
                    assert_eq!(std::fs::read(&log_path).unwrap(), b"log would replace key");
                } else {
                    let message = format!("{:#}", result.unwrap().unwrap_err());
                    assert!(message.contains("retained repository key"), "{message}");
                    assert!(
                        !called.get(),
                        "{alias}, enabled={enabled}, explicit_key={explicit_key}: rejection precedes tracing"
                    );
                }
                assert_eq!(std::fs::read_to_string(&key_path).unwrap(), key);
                Ok(())
            });
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[test_r::timeout("30s")]
    async fn server_tracing_does_not_read_open_noncolliding_sinks() {
        use rustix::fs::{CWD, Mode, mkfifoat};
        use std::os::unix::fs::PermissionsExt;

        tokio::task::spawn_blocking(|| {
            [false, true].into_iter().for_each(|fifo| {
                figment::Jail::expect_with(|jail| {
                    jail.clear_env();
                    jail.set_env("HOME", jail.directory().display().to_string());
                    let data_dir = jail.directory().join("data");
                    std::fs::create_dir_all(&data_dir).unwrap();
                    let log_path = data_dir.join("server.log");
                    if fifo {
                        mkfifoat(CWD, &log_path, Mode::RUSR | Mode::WUSR).unwrap();
                    } else {
                        std::fs::write(&log_path, b"write-only log").unwrap();
                        std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o200))
                            .unwrap();
                    }
                    jail.set_env("GOLEM__LOGGING__FILE_NAME", log_path.display().to_string());
                    jail.create_file(
                        "settings.toml",
                        r#"
                    [logging.file]
                    enabled = true
                    [filesystem_snapshots]
                    enabled = true
                "#,
                    )?;
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        runtime.block_on(async {
                            let mut flags = golem_cli::command::GolemCliGlobalFlags::default();
                            flags.local = true;
                            flags.disable_app_manifest_discovery = true;
                            flags.server_no_limit_change = true;
                            let ctx = Arc::new(Context::new(flags, None).await.unwrap());
                            ServerCommandHandler
                                .handle_with_tracing(
                                    ctx,
                                    ServerSubcommand::Run {
                                        args: RunArgs {
                                            config: Some("settings.toml".into()),
                                            data_dir: Some(data_dir.clone()),
                                            router_addr: Some("127.0.0.1".into()),
                                            ..Default::default()
                                        },
                                    },
                                    Verbosity::new(0, 0),
                                    |_, _| {
                                        assert_eq!(
                                            std::fs::read_to_string(
                                                data_dir.join(crate::filesystem_config::KEY_FILE),
                                            )
                                            .unwrap()
                                            .len(),
                                            128,
                                        );
                                        std::panic::panic_any(TracingInitialized);
                                    },
                                )
                                .await
                                .unwrap();
                        })
                    }));
                    assert!(stopped.unwrap_err().is::<TracingInitialized>());
                    Ok(())
                });
            });
        })
        .await
        .unwrap();
    }

    #[test]
    async fn server_shutdown_waits_for_tracked_cleanup() {
        let shutdown = Shutdown::new();
        let cleanup_finished = Arc::new(AtomicBool::new(false));
        let cleanup_finished_clone = cleanup_finished.clone();
        let shutdown_token = shutdown.token();
        shutdown.spawn(async move {
            shutdown_token.cancelled().await;
            tokio::task::yield_now().await;
            cleanup_finished_clone.store(true, Ordering::Release);
        });

        shutdown_golem_services(&mut JoinSet::new(), &shutdown).await;

        assert!(cleanup_finished.load(Ordering::Acquire));
    }

    #[test]
    fn subdomain_ports_match_when_bound_ports_equal_manifest_or_default_ports() {
        let manifest = local_server(LocalServer {
            custom_request_port: Some(9008),
            ..LocalServer::default()
        });
        let ports = StartupPorts {
            router_port: 9881,
            custom_request_port: 9008,
            mcp_port: DEFAULT_LOCAL_MCP_PORT,
        };

        assert_eq!(subdomain_port_mismatches(Some(&manifest), &ports), vec![]);
    }

    #[test]
    fn subdomain_ports_use_defaults_without_local_server_section() {
        // e.g. `--custom-request-port 0` in an app whose manifest has no `localServer`
        let ports = StartupPorts {
            router_port: 9881,
            custom_request_port: 41235,
            mcp_port: DEFAULT_LOCAL_MCP_PORT,
        };

        assert_eq!(
            subdomain_port_mismatches(None, &ports),
            vec![SubdomainPortMismatch {
                deployment_kind: "HTTP API",
                manifest_field: "localServer.customRequestPort",
                flag: "--custom-request-port",
                expanded_port: DEFAULT_LOCAL_CUSTOM_REQUEST_PORT,
                bound_port: 41235,
            }]
        );
    }

    #[test]
    fn subdomain_ports_mismatch_when_bound_ports_differ() {
        let manifest = local_server(LocalServer {
            custom_request_port: Some(9008),
            ..LocalServer::default()
        });
        // e.g. `--custom-request-port 0 --mcp-port 0`, OS-assigned ports
        let ports = StartupPorts {
            router_port: 9881,
            custom_request_port: 41235,
            mcp_port: 41236,
        };

        assert_eq!(
            subdomain_port_mismatches(Some(&manifest), &ports),
            vec![
                SubdomainPortMismatch {
                    deployment_kind: "HTTP API",
                    manifest_field: "localServer.customRequestPort",
                    flag: "--custom-request-port",
                    expanded_port: 9008,
                    bound_port: 41235,
                },
                SubdomainPortMismatch {
                    deployment_kind: "MCP",
                    manifest_field: "localServer.mcpPort",
                    flag: "--mcp-port",
                    expanded_port: DEFAULT_LOCAL_MCP_PORT,
                    bound_port: 41236,
                },
            ]
        );
    }

    #[test]
    fn clean_rejects_filesystem_root() {
        let current_dir = std::env::current_dir().unwrap();
        let root = current_dir.ancestors().last().unwrap();

        let error = resolve_clean_data_dir(root).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Refusing to clean filesystem root")
        );
    }

    #[test]
    fn clean_resolves_relative_data_dir() {
        let data_dir = resolve_clean_data_dir(Path::new("local-server-data")).unwrap();

        assert!(data_dir.is_absolute());
        assert!(data_dir.ends_with(Path::new("local-server-data")));

        let absolute_data_dir = std::env::current_dir().unwrap().join("local-server-data");
        assert_eq!(
            resolve_clean_data_dir(&absolute_data_dir).unwrap(),
            absolute_data_dir
        );
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use test_r::test;

        #[test]
        fn clean_resolves_intermediate_symlink_without_following_final_symlink() {
            use std::os::unix::fs::symlink;

            let test_root = std::env::temp_dir()
                .join(format!("golem-clean-symlink-test-{}", std::process::id()));
            let actual_parent = test_root.join("actual");
            let intermediate_link = test_root.join("intermediate-link");
            let final_link = actual_parent.join("final-link");
            std::fs::create_dir_all(&actual_parent).unwrap();
            symlink(&actual_parent, &intermediate_link).unwrap();
            symlink(actual_parent.join("final-target"), &final_link).unwrap();

            let resolved_intermediate =
                resolve_clean_data_dir(&intermediate_link.join("data")).unwrap();
            let resolved_final = resolve_clean_data_dir(&final_link).unwrap();
            let canonical_parent = std::fs::canonicalize(&actual_parent).unwrap();

            assert_eq!(resolved_intermediate, canonical_parent.join("data"));
            assert_eq!(resolved_final, canonical_parent.join("final-link"));

            std::fs::remove_dir_all(&test_root).unwrap();
        }
    }
}
