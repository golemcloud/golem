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

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use golem_cli::command_handler::{ CommandHandler, CommandHandlerHooks };

#[cfg(feature = "server-commands")]
mod serve; // MCP HTTP server lives in src/serve.rs

#[cfg(feature = "server-commands")]
static SERVE_ARGS: std::sync::OnceLock<ServeArgs> = std::sync::OnceLock::new();

// -----------------------------------------------------------------------------
// Hooks
// -----------------------------------------------------------------------------
#[cfg(feature = "server-commands")]
mod hooks {
    use golem_cli::command::server::ServerSubcommand;
    use golem_cli::command_handler::CommandHandlerHooks;
    use golem_cli::context::Context;
    use clap_verbosity_flag::Verbosity;
    use std::sync::Arc;

    pub struct NoHooks;

    impl CommandHandlerHooks for NoHooks {
        fn handler_server_commands(
            &self,
            _ctx: Arc<Context>,
            _subcommand: ServerSubcommand
        ) -> impl std::future::Future<Output = anyhow::Result<()>> {
            async { Ok(()) }
        }

        fn run_server() -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
            async move {
                use crate::serve;
                // Pull the args parsed in main(); fall back to the same defaults the parser used.
                let args = crate::SERVE_ARGS.get().cloned().unwrap_or_default();

                // Mirror the original cwd fallback from main()
                let cwd = args.cwd.unwrap_or_else(|| std::env::current_dir().expect("current_dir"));

                serve::serve_http_mcp(args.port, cwd).await?;
                Ok(())
            }
        }

        fn override_verbosity(verbosity: Verbosity) -> Verbosity {
            verbosity
        }

        fn override_pretty_mode() -> bool {
            false
        }
    }
}

#[cfg(not(feature = "server-commands"))]
mod hooks {
    use golem_cli::command_handler::CommandHandlerHooks;
    pub struct NoHooks;
    impl CommandHandlerHooks for NoHooks {}
}

use hooks::NoHooks;

// -----------------------------------------------------------------------------
// Minimal serve flag handling
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Default)]
struct ServeArgs {
    enable: bool,
    port: u16,
    cwd: Option<PathBuf>,
}

fn parse_and_strip_serve(argv: &[OsString]) -> (ServeArgs, Vec<OsString>) {
    let mut forwarded: Vec<OsString> = Vec::with_capacity(argv.len());
    if let Some(first) = argv.first() {
        forwarded.push(first.clone());
    }

    let mut i = 1usize;
    let mut enable = false;
    let mut port: u16 = 1232;
    let mut cwd: Option<PathBuf> = None;

    while i < argv.len() {
        let s = argv[i].to_string_lossy();

        if s == "--serve" {
            enable = true;
            i += 1;
            continue;
        }

        if s == "--serve-port" && i + 1 < argv.len() {
            if let Ok(p) = argv[i + 1].to_string_lossy().parse::<u16>() {
                port = p;
                i += 2;
                continue;
            }
        }

        if s == "--serve-cwd" && i + 1 < argv.len() {
            cwd = Some(PathBuf::from(argv[i + 1].to_string_lossy().to_string()));
            i += 2;
            continue;
        }

        forwarded.push(argv[i].clone());
        i += 1;
    }

    (ServeArgs { enable, port, cwd }, forwarded)
}

#[cfg(feature = "server-commands")]
fn init_logging_once() {
    use tracing_subscriber::{ fmt, EnvFilter };
    let _ = fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
        )
        .try_init();
}

// -----------------------------------------------------------------------------
// Main
// -----------------------------------------------------------------------------
fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let (serve_args, forwarded) = parse_and_strip_serve(&argv);

    #[cfg(feature = "server-commands")]
    if serve_args.enable {
        init_logging_once();
        // Make the parsed args available to the hook
        let _ = SERVE_ARGS.set(serve_args.clone());

        return tokio::runtime::Builder
            ::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime for serve mode")
            .block_on(async {
                match NoHooks::run_server().await {
                    Ok(_) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("golem-cli: MCP server error: {e:#}");
                        ExitCode::FAILURE
                    }
                }
            });
    }

    // Default: old CLI behavior
    tokio::runtime::Builder
        ::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime for golem-cli main")
        .block_on(CommandHandler::handle_args(forwarded.into_iter(), Arc::new(NoHooks {})))
}
