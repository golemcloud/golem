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
use golem_cli::context::Context;
use golem_cli::error::NonSuccessfulExit;
use golem_cli::fs;
use golem_cli::log::{LogColorize, log_warn_action};
use golem_cli::model::app::ResolvedLocalServer;
use golem_worker_executor::services::golem_config::ResourceUsageMeteringConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info};

use crate::compat::map_local_server_startup_error;
use crate::launch::{LaunchArgs, launch_golem_services};

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

                let launch_args = launch_args_from_run_args_and_manifest(&args, &ctx)?;
                let data_dir = launch_args.data_dir.clone();
                if args.clean && tokio::fs::metadata(&data_dir).await.is_ok() {
                    clean_data_dir(&ctx, &data_dir).await?;
                };

                // A single pinned signal future is shared by both phases below, so the
                // handlers are installed once and a signal arriving between the two
                // select! blocks is not lost.
                let shutdown = shutdown_signal();
                tokio::pin!(shutdown);

                let launch_result = tokio::select! {
                    res = launch_golem_services(&launch_args) => Some(res),
                    _ = &mut shutdown => None,
                };

                let Some(launch_result) = launch_result else {
                    info!("Received shutdown signal during startup, stopping Golem server");
                    return Ok(());
                };

                let mut join_set =
                    launch_result.map_err(|err| map_local_server_startup_error(err, &data_dir))?;

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
                        // Aborting the tasks drops the router task, which owns the worker
                        // executor's RunDetails; its Drop cancels the graph-wide shutdown
                        // token and stops the epoch thread.
                        join_set.shutdown().await;
                        Ok(())
                    }
                }
            }
            ServerSubcommand::Clean => {
                let data_dir = data_dir_from_local_server(ctx.manifest_local_server())?;
                clean_data_dir(&ctx, &data_dir).await
            }
        }
    }

    async fn run_server() -> anyhow::Result<()> {
        let args = RunArgs::default();
        let data_dir = default_data_dir()?;

        let mut join_set = launch_golem_services(&LaunchArgs {
            router_addr: args.router_addr().to_string(),
            router_port: args.router_port(),
            custom_request_port: args.custom_request_port(),
            mcp_port: args.mcp_port(),
            ports_file: args.ports_file.clone(),
            data_dir: data_dir.clone(),
            agent_filesystem_root: args.agent_filesystem_root.clone(),
            resource_usage_metering: resource_usage_metering_from_env()?,
        })
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

fn default_data_dir() -> anyhow::Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or_else(|| anyhow!("Failed to get data local dir"))?
        .join("golem"))
}

fn launch_args_from_run_args_and_manifest(
    args: &RunArgs,
    ctx: &Context,
) -> anyhow::Result<LaunchArgs> {
    launch_args_from_run_args_and_local_server(
        args,
        ctx.manifest_local_server(),
        resource_usage_metering_from_env()?,
    )
}

fn resource_usage_metering_from_env() -> anyhow::Result<ResourceUsageMeteringConfig> {
    Ok(ResourceUsageMeteringConfig {
        compute: metering_dimension_from_env("GOLEM__RESOURCE_USAGE_METERING__COMPUTE")?,
        memory: metering_dimension_from_env("GOLEM__RESOURCE_USAGE_METERING__MEMORY")?,
        filesystem: metering_dimension_from_env("GOLEM__RESOURCE_USAGE_METERING__FILESYSTEM")?,
    })
}

fn metering_dimension_from_env(name: &str) -> anyhow::Result<bool> {
    match std::env::var(name) {
        Ok(value) => parse_metering_dimension(name, &value),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("Failed to parse {name}: non-Unicode value")
        }
    }
}

fn parse_metering_dimension(name: &str, value: &str) -> anyhow::Result<bool> {
    value
        .parse()
        .with_context(|| format!("Failed to parse {name}: {value}"))
}

fn data_dir_from_local_server(
    local_server: Option<&ResolvedLocalServer>,
) -> anyhow::Result<PathBuf> {
    match local_server.and_then(|manifest| manifest.data_dir.clone()) {
        Some(data_dir) => Ok(data_dir),
        None => default_data_dir(),
    }
}

fn launch_args_from_run_args_and_local_server(
    args: &RunArgs,
    local_server: Option<&ResolvedLocalServer>,
    resource_usage_metering: ResourceUsageMeteringConfig,
) -> anyhow::Result<LaunchArgs> {
    Ok(LaunchArgs {
        router_addr: args
            .router_addr
            .clone()
            .or_else(|| local_server.and_then(|manifest| manifest.router_addr.clone()))
            .unwrap_or_else(|| args.router_addr().to_string()),
        router_port: args
            .router_port
            .or_else(|| local_server.and_then(|manifest| manifest.router_port))
            .unwrap_or_else(|| args.router_port()),
        custom_request_port: args
            .custom_request_port
            .or_else(|| local_server.and_then(|manifest| manifest.custom_request_port))
            .unwrap_or_else(|| args.custom_request_port()),
        mcp_port: args
            .mcp_port
            .or_else(|| local_server.and_then(|manifest| manifest.mcp_port))
            .unwrap_or_else(|| args.mcp_port()),
        ports_file: args
            .ports_file
            .clone()
            .or_else(|| local_server.and_then(|manifest| manifest.ports_file.clone())),
        data_dir: match &args.data_dir {
            Some(data_dir) => data_dir.clone(),
            None => data_dir_from_local_server(local_server)?,
        },
        agent_filesystem_root: args
            .agent_filesystem_root
            .clone()
            .or_else(|| local_server.and_then(|manifest| manifest.agent_filesystem_root.clone())),
        resource_usage_metering,
    })
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
    use test_r::test;

    fn local_server(value: LocalServer) -> ResolvedLocalServer {
        ResolvedLocalServer::from_raw_with_base_dir(&value, Path::new("/tmp/test-app"))
    }

    #[test]
    fn metering_dimension_values_are_validated() {
        assert!(parse_metering_dimension("METERING", "true").unwrap());
        assert!(!parse_metering_dimension("METERING", "false").unwrap());
        assert!(parse_metering_dimension("METERING", "invalid").is_err());
    }

    #[test]
    fn manifest_local_server_values_are_used_when_cli_args_are_absent() {
        let manifest = local_server(LocalServer {
            router_addr: Some("127.0.0.1".to_string()),
            router_port: Some(9882),
            custom_request_port: Some(9008),
            mcp_port: Some(9009),
            ports_file: Some(PathBuf::from("/tmp/test-app/.golem/ports.json")),
            data_dir: Some(PathBuf::from("/tmp/test-app/.golem/data")),
            agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
        });

        let args = launch_args_from_run_args_and_local_server(
            &RunArgs::default(),
            Some(&manifest),
            ResourceUsageMeteringConfig::default(),
        )
        .unwrap();

        assert_eq!(args.router_addr, "127.0.0.1");
        assert_eq!(args.router_port, 9882);
        assert_eq!(args.custom_request_port, 9008);
        assert_eq!(args.mcp_port, 9009);
        assert_eq!(
            args.ports_file,
            Some(PathBuf::from("/tmp/test-app/.golem/ports.json"))
        );
        assert_eq!(args.data_dir, PathBuf::from("/tmp/test-app/.golem/data"));
        assert_eq!(
            args.agent_filesystem_root,
            Some(PathBuf::from("/tmp/test-app/.golem/agents"))
        );
    }

    #[test]
    fn cli_args_override_manifest_local_server_values() {
        let manifest = local_server(LocalServer {
            router_addr: Some("127.0.0.1".to_string()),
            router_port: Some(9882),
            custom_request_port: Some(9008),
            mcp_port: Some(9009),
            ports_file: Some(PathBuf::from("/tmp/test-app/.golem/ports.json")),
            data_dir: Some(PathBuf::from("/tmp/test-app/.golem/data")),
            agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
        });
        let run_args = RunArgs {
            router_addr: Some("0.0.0.0".to_string()),
            router_port: Some(10000),
            custom_request_port: Some(10001),
            mcp_port: Some(10002),
            ports_file: Some(PathBuf::from("cli-ports.json")),
            data_dir: Some(PathBuf::from("cli-data")),
            clean: false,
            agent_filesystem_root: Some(PathBuf::from("cli-agents")),
        };

        let args = launch_args_from_run_args_and_local_server(
            &run_args,
            Some(&manifest),
            ResourceUsageMeteringConfig::default(),
        )
        .unwrap();

        assert_eq!(args.router_addr, "0.0.0.0");
        assert_eq!(args.router_port, 10000);
        assert_eq!(args.custom_request_port, 10001);
        assert_eq!(args.mcp_port, 10002);
        assert_eq!(args.ports_file, Some(PathBuf::from("cli-ports.json")));
        assert_eq!(args.data_dir, PathBuf::from("cli-data"));
        assert_eq!(
            args.agent_filesystem_root,
            Some(PathBuf::from("cli-agents"))
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
