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

use anyhow::anyhow;
use clap_verbosity_flag::Verbosity;
use golem_cli::command::server::{RunArgs, ServerSubcommand};
use golem_cli::command_handler::CommandHandlerHooks;
use golem_cli::context::Context;
use golem_cli::model::app::ResolvedLocalServer;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::debug;

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
                    clean_data_dir(&data_dir).await?;
                };

                let mut join_set = launch_golem_services(&launch_args)
                    .await
                    .map_err(|err| map_local_server_startup_error(err, &data_dir))?;

                while let Some(res) = join_set.join_next().await {
                    res??;
                }

                Ok(())
            }
            ServerSubcommand::Clean => clean_data_dir(&default_data_dir()?).await,
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
    launch_args_from_run_args_and_local_server(args, ctx.manifest_local_server())
}

fn launch_args_from_run_args_and_local_server(
    args: &RunArgs,
    local_server: Option<&ResolvedLocalServer>,
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
        data_dir: args
            .data_dir
            .clone()
            .or_else(|| local_server.and_then(|manifest| manifest.data_dir.clone()))
            .map(Ok)
            .unwrap_or_else(default_data_dir)?,
        agent_filesystem_root: args
            .agent_filesystem_root
            .clone()
            .or_else(|| local_server.and_then(|manifest| manifest.agent_filesystem_root.clone())),
    })
}

async fn clean_data_dir(data_dir: &Path) -> anyhow::Result<()> {
    tokio::fs::remove_dir_all(&data_dir)
        .await
        .map_err(|err| anyhow!("Failed cleaning data dir ({}): {}", data_dir.display(), err))
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

        let args = launch_args_from_run_args_and_local_server(&RunArgs::default(), Some(&manifest))
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

        let args = launch_args_from_run_args_and_local_server(&run_args, Some(&manifest)).unwrap();

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
}
