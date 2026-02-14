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
use golem_cli::command_handler::CommandHandler;
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
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();

    // Quick check: does --serve or --serve-port appear in args?
    let has_serve = args.iter().any(|a| a == "--serve");
    let serve_port = args
        .windows(2)
        .find(|w| w[0] == "--serve-port")
        .and_then(|w| w[1].to_str()?.parse::<u16>().ok());

    if has_serve || serve_port.is_some() {
        // MCP server mode
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime");

        let binary = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "golem-cli".to_string());

        let work_dir =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

        // Collect global flags to forward (exclude --serve and --serve-port)
        let mut global_flags = Vec::new();
        let mut skip_next = false;
        for arg in &args[1..] {
            if skip_next {
                skip_next = false;
                continue;
            }
            let s = arg.to_string_lossy();
            if s == "--serve" {
                continue;
            }
            if s == "--serve-port" {
                skip_next = true;
                continue;
            }
            if s.starts_with("--serve-port=") {
                continue;
            }
            global_flags.push(s.to_string());
        }

        let result = if let Some(port) = serve_port {
            rt.block_on(golem_cli::mcp_server::start_http_server(
                port,
                binary,
                work_dir,
                global_flags,
            ))
        } else {
            rt.block_on(golem_cli::mcp_server::start_stdio_server(
                binary,
                work_dir,
                global_flags,
            ))
        };

        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("MCP server error: {}", e);
                ExitCode::FAILURE
            }
        }
    } else {
        // Normal CLI mode
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime for golem-cli main")
            .block_on(CommandHandler::handle_args(
                std::env::args_os(),
                Arc::new(NoHooks {}),
            ))
    }
}
