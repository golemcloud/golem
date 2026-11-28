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

use super::{CommandHandler, CommandHandlerHooks, requires_executor_runtime};
use crate::command::GolemCliCommand;
use crate::command::server::ServerSubcommand;
use crate::context::Context;
use clap_verbosity_flag::Verbosity;
use std::cell::Cell;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use test_r::test;

#[derive(Default)]
struct RecordingServer {
    received: Mutex<Option<(bool, bool, Option<tracing::Level>)>>,
}

impl CommandHandlerHooks for RecordingServer {
    async fn handler_server_commands(
        &self,
        _ctx: Arc<Context>,
        subcommand: ServerSubcommand,
        verbosity: Verbosity,
    ) -> anyhow::Result<()> {
        *self.received.lock().unwrap() = Some((
            matches!(subcommand, ServerSubcommand::Run { .. }),
            verbosity.is_present(),
            verbosity.tracing_level(),
        ));
        Ok(())
    }

    async fn run_server() -> anyhow::Result<()> {
        panic!("dispatch must not start a service")
    }

    fn override_verbosity(_verbosity: Verbosity) -> Verbosity {
        Verbosity::new(4, 0)
    }

    fn override_pretty_mode() -> bool {
        true
    }
}

#[test]
fn server_dispatch_skips_cli_tracing_only_for_run_and_preserves_raw_verbosity() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("HOME", jail.directory().display().to_string());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        ["run", "clean"].into_iter().for_each(|subcommand| {
            [
                (&[][..], false, Some(tracing::Level::ERROR)),
                (&["-v"][..], true, Some(tracing::Level::WARN)),
                (&["-vv"][..], true, Some(tracing::Level::INFO)),
                (&["-vvv"][..], true, Some(tracing::Level::DEBUG)),
                (&["-q"][..], true, None),
            ]
            .into_iter()
            .for_each(|(flags, present, level)| {
                let command = GolemCliCommand::try_parse_from_lenient(
                    ["golem", "-X", "-L"]
                        .into_iter()
                        .chain(flags.iter().copied())
                        .chain(["server", subcommand]),
                    true,
                );
                let hooks = Arc::new(RecordingServer::default());
                let initialized = Cell::new(None);
                let result = runtime.block_on(CommandHandler::handle_with_tracing(
                    command,
                    hooks.clone(),
                    |verbosity, pretty| {
                        assert!(
                            initialized
                                .replace(Some((verbosity.tracing_level(), pretty)))
                                .is_none()
                        );
                    },
                ));
                assert_eq!(result, ExitCode::SUCCESS);
                assert_eq!(
                    *hooks.received.lock().unwrap(),
                    Some((subcommand == "run", present, level))
                );
                assert_eq!(
                    initialized.get(),
                    (subcommand == "clean").then_some((Some(tracing::Level::TRACE), true))
                );
            });
        });
        Ok(())
    });
}

fn requires_executor_runtime_for(args: &[&str]) -> bool {
    let result = GolemCliCommand::try_parse_from_lenient(args, true);
    requires_executor_runtime(&result)
}

#[test]
fn server_runtime_is_selected_only_for_parsed_server_run() {
    assert!(requires_executor_runtime_for(&["golem", "server", "run"]));
    assert!(requires_executor_runtime_for(&[
        "golem",
        "--verbose",
        "server",
        "run"
    ]));

    assert!(!requires_executor_runtime_for(&[
        "golem", "server", "clean"
    ]));
    assert!(!requires_executor_runtime_for(&[
        "golem", "server", "run", "--help"
    ]));
    assert!(!requires_executor_runtime_for(&[
        "golem", "server", "invalid"
    ]));
    assert!(!requires_executor_runtime_for(&["golem", "templates"]));
    assert!(!requires_executor_runtime_for(&["golem"]));
}
