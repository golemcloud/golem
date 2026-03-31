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

#![recursion_limit = "512"]

use crate::hooks::NoHooks;
use golem_cli::command_handler::CommandHandler;
use golem_cli::main_wrapper;
use std::process::ExitCode;
use std::sync::Arc;

#[cfg(feature = "server-commands")]
mod hooks {
    use golem_cli::mcp_server;
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
            ctx: Arc<Context>,
            subcommand: ServerSubcommand,
        ) -> anyhow::Result<()> {
            match subcommand {
                ServerSubcommand::Serve { port } => {
                    println!("Starting Golem CLI MCP Server on http://0.0.0.0:{}...", port);
                    mcp_server::run_mcp_server(ctx, port).await
                }
                other => {
                    println!("Server subcommand not available: {:?}", other);
                    Ok(())
                }
            }
        }

        #[cfg(feature = "server-commands")]
        async fn run_server() -> anyhow::Result<()> {
            println!("run_server not implemented");
            Ok(())
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
    main_wrapper(|| CommandHandler::handle_args(std::env::args_os(), Arc::new(NoHooks {})))
}
