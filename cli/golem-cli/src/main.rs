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

mod serve; // MCP HTTP server in src/serve.rs

use golem_cli::command_handler::CommandHandler;
use golem_cli::command_handler::CommandHandlerHooks;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

/// We don't use any CommandHandler hooks. Keep it empty to avoid trait lifetime churn.
struct NoHooks;
impl CommandHandlerHooks for NoHooks {}

/// Minimal parser for our local flags so we don't depend on the CLI's internal clap setup.
/// Recognized flags (and stripped before delegating to CommandHandler):
///   --serve            -> run MCP HTTP server on localhost
///   --serve-port <u16> -> default 1232
///   --serve-cwd  <dir> -> default current working directory
#[derive(Debug)]
struct ServeArgs {
    enable: bool,
    port: u16,
    cwd: Option<PathBuf>,
}

impl ServeArgs {
    fn parse_and_strip(raw: Vec<OsString>) -> (ServeArgs, Vec<OsString>) {
        let mut enable = false;
        let mut port: u16 = 1232;
        let mut cwd: Option<PathBuf> = None;

        // new argv that we will pass to CommandHandler (without our flags)
        let mut forwarded: Vec<OsString> = Vec::with_capacity(raw.len());

        // keep program name as-is
        if let Some(first) = raw.first().cloned() {
            forwarded.push(first);
        }
        // walk remaining args
        let mut i = 1usize;
        while i < raw.len() {
            let s = raw[i].to_string_lossy().to_string();
            match s.as_str() {
                "--serve" => {
                    enable = true;
                    i += 1;
                }
                "--serve-port" => {
                    if i + 1 < raw.len() {
                        if let Ok(p) = raw[i + 1].to_string_lossy().parse::<u16>() {
                            port = p;
                            i += 2;
                        } else {
                            // malformed value; keep flags to forward (so user sees normal error)
                            forwarded.push(raw[i].clone());
                            forwarded.push(raw[i + 1].clone());
                            i += 2;
                        }
                    } else {
                        forwarded.push(raw[i].clone());
                        i += 1;
                    }
                }
                "--serve-cwd" => {
                    if i + 1 < raw.len() {
                        cwd = Some(PathBuf::from(raw[i + 1].to_string_lossy().to_string()));
                        i += 2;
                    } else {
                        forwarded.push(raw[i].clone());
                        i += 1;
                    }
                }
                _ => {
                    // not our flag; keep it
                    forwarded.push(raw[i].clone());
                    i += 1;
                }
            }
        }

        (
            ServeArgs { enable, port, cwd },
            forwarded,
        )
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // Parse and strip our local flags first
    let argv: Vec<OsString> = std::env::args_os().collect();
    let (serve_args, forwarded) = ServeArgs::parse_and_strip(argv);

    if serve_args.enable {
        let cwd = match serve_args.cwd {
            Some(p) => p,
            None => match std::env::current_dir() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("golem-cli: failed to get current directory: {e}");
                    return ExitCode::FAILURE;
                }
            },
        };
        if let Err(e) = serve::serve_http_mcp(serve_args.port, cwd).await {
            eprintln!("golem-cli: server error: {e:#}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    // No --serve → fall back to the original async handler
    CommandHandler::handle_args(forwarded.into_iter(), Arc::new(NoHooks)).await
}
