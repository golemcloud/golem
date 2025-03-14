// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::command::{
    GolemCliCommand, GolemCliCommandParseResult, GolemCliFallbackCommand, GolemCliGlobalFlags,
    GolemCliSubcommand,
};
use crate::command_handler::app::AppCommandHandler;
use crate::command_handler::cloud::project::CloudProjectCommandHandler;
use crate::command_handler::cloud::CloudCommandHandler;
use crate::command_handler::component::ComponentCommandHandler;
use crate::command_handler::log::LogHandler;
use crate::command_handler::partial_match::ErrorHandler;
use crate::command_handler::profile::config::ProfileConfigCommandHandler;
use crate::command_handler::profile::ProfileCommandHandler;
use crate::command_handler::worker::WorkerCommandHandler;
use crate::config::{Config, ProfileName};
use crate::context::Context;
use crate::error::{HintError, NonSuccessfulExit};
use crate::init_tracing;
use crate::model::text::fmt::log_error;
use golem_wasm_rpc_stubgen::log::logln;
use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::{debug, Level};

#[cfg(feature = "server-commands")]
use crate::command::server::ServerSubcommand;

mod app;
mod cloud;
mod component;
mod log;
mod partial_match;
mod profile;
mod worker;

// NOTE: We are explicitly not using #[async_trait] here to be able to NOT have a Send bound
// on the `handler_server_commands` method. Having a Send bound there causes "Send is not generic enough"
// error which is possibly due to a compiler bug (https://github.com/rust-lang/rust/issues/64552).
pub trait CommandHandlerHooks {
    #[cfg(feature = "server-commands")]
    fn handler_server_commands(
        &self,
        ctx: Arc<Context>,
        subcommand: ServerSubcommand,
    ) -> impl std::future::Future<Output = anyhow::Result<()>>;
}

// CommandHandle is responsible for matching commands and producing CLI output using Context,
// but NOT responsible for storing state (apart from Context itself), those should be part of Context.
pub struct CommandHandler<Hooks: CommandHandlerHooks> {
    ctx: Arc<Context>,
    #[allow(unused)]
    hooks: Arc<Hooks>,
}

impl<Hooks: CommandHandlerHooks> CommandHandler<Hooks> {
    fn new(global_flags: &GolemCliGlobalFlags, hooks: Arc<Hooks>) -> anyhow::Result<Self> {
        // TODO: enum for builtin and generic profiles
        let profile_name = {
            if global_flags.local {
                Some(ProfileName::local())
            } else if global_flags.cloud {
                Some(ProfileName::cloud())
            } else {
                global_flags.profile.clone()
            }
        };

        let ctx = Arc::new(Context::new(
            global_flags,
            Config::get_active_profile(&global_flags.config_dir(), profile_name)?,
        ));
        Ok(Self {
            ctx: ctx.clone(),
            hooks,
        })
    }

    // TODO: match and enrich "-h" and "--help"
    pub async fn handle_args<I, T>(args_iterator: I, hooks: Arc<Hooks>) -> ExitCode
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let result = match GolemCliCommand::try_parse_from_lenient(args_iterator, true) {
            GolemCliCommandParseResult::FullMatch(command) => {
                init_tracing(command.global_flags.verbosity);

                match Self::new(&command.global_flags, hooks) {
                    Ok(mut handler) => {
                        // TODO: handle hint errors
                        let result = handler
                            .handle_command(command)
                            .await
                            .map(|()| ExitCode::SUCCESS);

                        match result {
                            Ok(result) => Ok(result),
                            Err(error) => {
                                if let Some(hint_error) = error.downcast_ref::<HintError>() {
                                    handler
                                        .ctx
                                        .error_handler()
                                        .handle_hint_errors(hint_error)
                                        .map(|()| ExitCode::FAILURE)
                                } else {
                                    Err(error)
                                }
                            }
                        }
                    }
                    Err(err) => Err(err),
                }
            }
            GolemCliCommandParseResult::ErrorWithPartialMatch {
                error,
                fallback_command,
                partial_match,
            } => {
                init_tracing(fallback_command.global_flags.verbosity);
                debug!(partial_match = ?partial_match, "Partial match");
                debug_log_parse_error(&error, &fallback_command);
                error.print().unwrap();

                match Self::new(&fallback_command.global_flags, hooks) {
                    Ok(handler) => handler
                        .ctx
                        .error_handler()
                        .handle_partial_match(partial_match)
                        .await
                        .map(|_| clamp_exit_code(error.exit_code())),
                    Err(err) => Err(err),
                }
            }
            GolemCliCommandParseResult::Error {
                error,
                fallback_command,
            } => {
                init_tracing(fallback_command.global_flags.verbosity);
                debug_log_parse_error(&error, &fallback_command);
                error.print().unwrap();

                Ok(clamp_exit_code(error.exit_code()))
            }
        };

        result.unwrap_or_else(|error| {
            if error.downcast_ref::<NonSuccessfulExit>().is_none() {
                // TODO: check if this should be display or debug
                logln("");
                log_error(format!("{}", error));
            }
            ExitCode::FAILURE
        })
    }

    async fn handle_command(&mut self, command: GolemCliCommand) -> anyhow::Result<()> {
        match command.subcommand {
            GolemCliSubcommand::App { subcommand } => {
                self.ctx.app_handler().handle_command(subcommand).await
            }
            GolemCliSubcommand::Component { subcommand } => {
                self.ctx
                    .component_handler()
                    .handle_command(subcommand)
                    .await
            }
            GolemCliSubcommand::Worker { subcommand } => {
                self.ctx.worker_handler().handle_command(subcommand).await
            }
            GolemCliSubcommand::Api { .. } => {
                todo!()
            }
            GolemCliSubcommand::Plugin { .. } => {
                todo!()
            }
            GolemCliSubcommand::Profile { subcommand } => {
                self.ctx.profile_handler().handle_command(subcommand).await
            }
            #[cfg(feature = "server-commands")]
            GolemCliSubcommand::Server { subcommand } => {
                self.hooks
                    .handler_server_commands(self.ctx.clone(), subcommand)
                    .await
            }
            GolemCliSubcommand::Cloud { subcommand } => {
                self.ctx.cloud_handler().handle_command(subcommand).await
            }
            GolemCliSubcommand::Diagnose => {
                todo!()
            }
            GolemCliSubcommand::Completion => {
                todo!()
            }
        }
    }
}

// NOTE: for now every handler can access any other handler, but this can be restricted
//       by moving these simple factory methods into the specific handlers on demand,
//       if the need ever arises
trait Handlers {
    fn app_handler(&self) -> AppCommandHandler;
    fn cloud_handler(&self) -> CloudCommandHandler;
    fn cloud_project_handler(&self) -> CloudProjectCommandHandler;
    fn component_handler(&self) -> ComponentCommandHandler;
    fn error_handler(&self) -> ErrorHandler;
    fn log_handler(&self) -> LogHandler;
    fn profile_config_handler(&self) -> ProfileConfigCommandHandler;
    fn profile_handler(&self) -> ProfileCommandHandler;
    fn worker_handler(&self) -> WorkerCommandHandler;
}

impl Handlers for Arc<Context> {
    fn app_handler(&self) -> AppCommandHandler {
        AppCommandHandler::new(Arc::clone(self))
    }

    fn cloud_handler(&self) -> CloudCommandHandler {
        CloudCommandHandler::new(Arc::clone(self))
    }

    fn cloud_project_handler(&self) -> CloudProjectCommandHandler {
        CloudProjectCommandHandler::new(Arc::clone(self))
    }

    fn component_handler(&self) -> ComponentCommandHandler {
        ComponentCommandHandler::new(Arc::clone(self))
    }

    fn error_handler(&self) -> ErrorHandler {
        ErrorHandler::new(Arc::clone(self))
    }

    fn log_handler(&self) -> LogHandler {
        LogHandler::new(Arc::clone(self))
    }

    fn profile_config_handler(&self) -> ProfileConfigCommandHandler {
        ProfileConfigCommandHandler::new(Arc::clone(self))
    }

    fn profile_handler(&self) -> ProfileCommandHandler {
        ProfileCommandHandler::new(Arc::clone(self))
    }

    fn worker_handler(&self) -> WorkerCommandHandler {
        WorkerCommandHandler::new(Arc::clone(self))
    }
}

fn clamp_exit_code(exit_code: i32) -> ExitCode {
    if exit_code < 0 {
        ExitCode::FAILURE
    } else if exit_code > 255 {
        ExitCode::from(255)
    } else {
        ExitCode::from(exit_code as u8)
    }
}

fn debug_log_parse_error(error: &clap::Error, fallback_command: &GolemCliFallbackCommand) {
    debug!(fallback_command = ?fallback_command, "Fallback command");
    debug!(error = ?error, "Clap error");
    if tracing::enabled!(Level::DEBUG) {
        for (kind, value) in error.context() {
            debug!(kind = %kind, value = %value, "Clap error context");
        }
    }
}
