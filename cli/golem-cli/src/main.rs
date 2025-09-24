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

#![allow(clippy::needless_return)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use golem_cli::command_handler::{CommandHandler, CommandHandlerHooks};

use clap::{Arg, ArgAction, Command as ClapCommand};

#[cfg(feature = "mcp-server")]
mod serve; // MCP HTTP server lives in src/serve.rs and is feature-gated

// -------- Hooks ---------------------------------------------------------------
// Keep this struct EMPTY to avoid trait/lifetime churn across CommandHandler.
pub struct NoHooks;

// When `server-commands` is OFF, the trait exposes no required methods → empty impl.
#[cfg(not(feature = "server-commands"))]
impl CommandHandlerHooks for NoHooks {}

// When `server-commands` is ON, implement the required methods as no-ops.
#[cfg(feature = "server-commands")]
impl CommandHandlerHooks for NoHooks {
    fn handler_server_commands(
        &self,
        _ctx: Arc<golem_cli::command_handler::Context>,
        _subcommand: golem_cli::command_handler::ServerSubcommand,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> {
        async { Ok(()) }
    }

    fn run_server() -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async { Ok(()) }
    }

    fn override_verbosity(
        verbosity: golem_cli::command_handler::Verbosity,
    ) -> golem_cli::command_handler::Verbosity {
        verbosity
    }

    fn override_pretty_mode() -> bool {
        false
    }
}

// -------- Minimal serve flag parsing ------------------------------------------
// We use Clap to parse ONLY the MCP flags, then forward the rest unchanged.
#[derive(Debug, Default, Clone)]
struct ServeArgs {
    enable: bool,
    port: u16,
    cwd: Option<PathBuf>,
}

// Build a tiny Clap parser that recognizes only our flags.
// We *do not* attempt to parse the full CLI here; that remains with CommandHandler.
fn clap_for_serve() -> ClapCommand {
    ClapCommand::new("golem-cli-serve-flags-only")
        .disable_help_flag(true)
        .arg(
            Arg::new("serve")
                .long("serve")
                .help("Run MCP HTTP server (feature: mcp-server)")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("serve-port")
                .long("serve-port")
                .value_name("PORT")
                .num_args(1)
                .value_parser(clap::value_parser!(u16))
                .default_value("1232")
                .help("Port for MCP HTTP server"),
        )
        .arg(
            Arg::new("serve-cwd")
                .long("serve-cwd")
                .value_name("PATH")
                .num_args(1)
                .help("Working directory for tools/resources"),
        )
}

// Parse our flags and *strip them* from the argv so the rest is forwarded.
fn parse_and_strip_serve(argv: &[OsString]) -> (ServeArgs, Vec<OsString>) {
    // Keep the original argv to forward later.
    let mut forwarded: Vec<OsString> = Vec::with_capacity(argv.len());
    if let Some(first) = argv.first() {
        forwarded.push(first.clone());
    }

    // We'll do a light pass: consume known flags and reconstruct `forwarded` with the rest.
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
            // Let Clap validate; if it fails, treat as unknown and forward both tokens.
            if let Ok(m) = clap_for_serve().try_get_matches_from(
                vec!["golem", "--serve-port", &argv[i + 1].to_string_lossy()],
            ) {
                if let Some(v) = m
                    .get_one::<u16>("serve-port")
                    .copied()
                {
                    port = v;
                    i += 2;
                    continue;
                }
            }
        }

        if s == "--serve-cwd" && i + 1 < argv.len() {
            cwd = Some(PathBuf::from(argv[i + 1].to_string_lossy().to_string()));
            i += 2;
            continue;
        }

        // Unknown → forward
        forwarded.push(argv[i].clone());
        i += 1;
    }

    (ServeArgs { enable, port, cwd }, forwarded)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let (serve_args, forwarded) = parse_and_strip_serve(&argv);

    // Branch to MCP server if requested.
    if serve_args.enable {
        #[cfg(feature = "mcp-server")]
        {
            init_logging_once();
            let cwd = serve_args
                .cwd
                .unwrap_or_else(|| std::env::current_dir().expect("current_dir"));
            if let Err(e) = serve::serve_http_mcp(serve_args.port, cwd).await {
                eprintln!("golem-cli: MCP server error: {e:#}");
                return std::process::ExitCode::FAILURE;
            }
            return std::process::ExitCode::SUCCESS;
        }
        #[cfg(not(feature = "mcp-server"))]
        {
            eprintln!("golem-cli was built without MCP support. Rebuild with `--features mcp-server`.");
            return std::process::ExitCode::FAILURE;
        }
    }

    // Fall back to the canonical handler with real CLI parsing.
    CommandHandler::handle_args(forwarded.into_iter(), Arc::new(NoHooks)).await
}

#[cfg(feature = "mcp-server")]
fn init_logging_once() {
    // Use RUST_LOG if set, else default to info
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .try_init(); // ignore "set_logger once" errors
}
