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
use golem_cli::command::server::{RunArgs, ServerSubcommand};
use golem_cli::command_handler::{CommandHandlerHooks, Handlers};
use golem_cli::config::{DEFAULT_LOCAL_CUSTOM_REQUEST_PORT, DEFAULT_LOCAL_MCP_PORT};
use golem_cli::context::Context;
use golem_cli::error::NonSuccessfulExit;
use golem_cli::fs;
use golem_cli::log::{LogColorize, log_warn_action};
use golem_cli::model::app::ResolvedLocalServer;
use golem_worker_executor::services::shutdown::{SHUTDOWN_GRACE, Shutdown};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::compat::map_local_server_startup_error;
use crate::config::{GolemServerConfig, SnapshotRepositoryKeySource, load_server_config};
use crate::launch::{StartupPorts, launch_golem_services};
use crate::well_known_secrets::check_exposure;

pub struct ServerCommandHandler;

impl CommandHandlerHooks for ServerCommandHandler {
    async fn handler_server_commands(
        &self,
        ctx: Arc<Context>,
        subcommand: ServerSubcommand,
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
                let data_dir = config.data_dir.clone();

                // A single pinned signal future is shared by both phases below, so the
                // handlers are installed once and a signal arriving between the two
                // select! blocks is not lost.
                let shutdown = shutdown_signal();
                tokio::pin!(shutdown);

                let launch_result = tokio::select! {
                    res = launch_golem_services(&config) => Some(res),
                    _ = &mut shutdown => None,
                };

                let Some(launch_result) = launch_result else {
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

    async fn run_server() -> anyhow::Result<()> {
        let config = load_server_config(
            None,
            &RunArgs::default(),
            SnapshotRepositoryKeySource::DataDir,
        )?;
        let data_dir = config.data_dir.clone();

        let (mut join_set, _, _) = launch_golem_services(&config)
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
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");

        tokio::select! {
            _ = sigint.recv() => {},
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
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

    fn local_server(value: LocalServer) -> ResolvedLocalServer {
        ResolvedLocalServer::from_raw_with_base_dir(&value, Path::new("/tmp/test-app"))
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
    #[test]
    fn clean_resolves_intermediate_symlink_without_following_final_symlink() {
        use std::os::unix::fs::symlink;

        let test_root =
            std::env::temp_dir().join(format!("golem-clean-symlink-test-{}", std::process::id()));
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
