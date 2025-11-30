// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::hooks::NoHooks;
use golem_cli::command::GolemCliCommand;
use golem_cli::command::GolemCliCommandParseResult;
use golem_cli::command_handler::CommandHandler;
use golem_cli::mcp::context::McpContext;
use golem_cli::mcp::server;
use std::process::ExitCode;
use std::sync::Arc;

#[cfg(feature = "server-commands")]
mod hooks {
    use golem_cli::command::server::ServerSubcommand;
    use golem_cli::command_handler::CommandHandlerHooks;
    use golem_cli::context::Context;

    use clap_verbosity_flag::Verbosity;
    use std::sync::Arc;

    pub struct NoHooks {}

    impl CommandHandlerHooks for NoHooks {
        #[cfg(feature = "server-commands")]
        async fn handler_server_commands(
            &self,
            _ctx: Arc<Context>,
            _subcommand: ServerSubcommand,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }

        #[cfg(feature = "server-commands")]
        async fn run_server() -> anyhow::Result<()> {
            unimplemented!()
        }

        #[cfg(feature = "server-commands")]
        fn override_verbosity(verbosity: Verbosity) -> Verbosity {
            verbosity
        }

        #[cfg(feature = "server-commands")]
        fn override_pretty_mode() -> bool {
            false
        }
    }
}

#[cfg(not(feature = "server-commands"))]
mod hooks {
    use golem_cli::command_handler::CommandHandlerHooks;

    pub struct NoHooks {}

    impl CommandHandlerHooks for NoHooks {}
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    let parse_result = GolemCliCommand::try_parse_from_lenient(&args, true);
    // Try to parse to check for serve mode
    if let GolemCliCommandParseResult::FullMatch(cli) = parse_result {
        if cli.global_flags.serve {
            let port = cli.global_flags.serve_port.unwrap_or(1232);
            println!("golem-cli running MCP Server at port {}", port);

            return tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime for MCP server")
                .block_on(async {
                    match server::serve(
                        Arc::new(McpContext::new(std::env::current_dir().unwrap())),
                        // std::env::current_dir().unwrap(),
                        port,
                    )
                    .await
                    {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(e) => {
                            eprintln!("MCP server error: {}", e);
                            ExitCode::FAILURE
                        }
                    }
                });
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime for golem-cli main")
        .block_on(CommandHandler::handle_args(
            std::env::args_os(),
            Arc::new(NoHooks {}),
        ))
}
