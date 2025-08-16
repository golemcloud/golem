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

use crate::app::error::AppValidationError;
#[cfg(feature = "server-commands")]
use crate::command::server::ServerSubcommand;
use crate::command::{
    GolemCliCommand, GolemCliCommandParseResult, GolemCliFallbackCommand, GolemCliGlobalFlags,
    GolemCliSubcommand,
};
use crate::command_handler::api::cloud::certificate::ApiCloudCertificateCommandHandler;
use crate::command_handler::api::cloud::domain::ApiCloudDomainCommandHandler;
use crate::command_handler::api::cloud::ApiCloudCommandHandler;
use crate::command_handler::api::definition::ApiDefinitionCommandHandler;
use crate::command_handler::api::deployment::ApiDeploymentCommandHandler;
use crate::command_handler::api::security_scheme::ApiSecuritySchemeCommandHandler;
use crate::command_handler::api::ApiCommandHandler;
use crate::command_handler::app::AppCommandHandler;
use crate::command_handler::cloud::account::grant::CloudAccountGrantCommandHandler;
use crate::command_handler::cloud::account::CloudAccountCommandHandler;
use crate::command_handler::cloud::project::plugin::CloudProjectPluginCommandHandler;
use crate::command_handler::cloud::project::policy::CloudProjectPolicyCommandHandler;
use crate::command_handler::cloud::project::CloudProjectCommandHandler;
use crate::command_handler::cloud::token::CloudTokenCommandHandler;
use crate::command_handler::cloud::CloudCommandHandler;
use crate::command_handler::component::plugin::ComponentPluginCommandHandler;
use crate::command_handler::component::plugin_installation::PluginInstallationHandler;
use crate::command_handler::component::ComponentCommandHandler;
use crate::command_handler::interactive::InteractiveHandler;
use crate::command_handler::log::LogHandler;
use crate::command_handler::partial_match::ErrorHandler;
use crate::command_handler::plugin::PluginCommandHandler;
use crate::command_handler::profile::config::ProfileConfigCommandHandler;
use crate::command_handler::profile::ProfileCommandHandler;
use crate::command_handler::rib_repl::RibReplHandler;
use crate::command_handler::worker::WorkerCommandHandler;
use crate::context::Context;
use crate::error::{ContextInitHintError, HintError, NonSuccessfulExit};
use crate::log::{logln, set_log_output, Output};
use crate::model::text::fmt::log_error;
use crate::{command_name, init_tracing};
use anyhow::anyhow;
use clap::CommandFactory;
use clap_complete::Shell;
#[cfg(feature = "server-commands")]
use clap_verbosity_flag::Verbosity;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use std::ffi::OsString;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::{debug, Level};

mod api;
mod app;
mod cloud;
mod component;
pub(crate) mod interactive;
mod log;
mod partial_match;
mod plugin;
mod profile;
mod rib_repl;
mod worker;

// NOTE: We are explicitly not using #[async_trait] here to be able to NOT have a Send bound
// on the `handler_server_commands` method. Having a Send bound there causes "Send is not generic enough"
// error which is possibly due to a compiler bug (https://github.com/rust-lang/rust/issues/64552).
pub trait CommandHandlerHooks: Sync + Send {
    #[cfg(feature = "server-commands")]
    fn handler_server_commands(
        &self,
        ctx: Arc<Context>,
        subcommand: ServerSubcommand,
    ) -> impl std::future::Future<Output = anyhow::Result<()>>;

    // Used for auto starting the default server
    #[cfg(feature = "server-commands")]
    fn run_server() -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    #[cfg(feature = "server-commands")]
    fn override_verbosity(verbosity: Verbosity) -> Verbosity;

    #[cfg(feature = "server-commands")]
    fn override_pretty_mode() -> bool;
}

// CommandHandler is responsible for matching commands and producing CLI output using Context,
// but NOT responsible for storing state (apart from Context and Hooks itself), those should be part of Context.
pub struct CommandHandler<Hooks: CommandHandlerHooks> {
    ctx: Arc<Context>,
    #[allow(unused)]
    hooks: Arc<Hooks>,
}

impl<Hooks: CommandHandlerHooks + 'static> CommandHandler<Hooks> {
    // NOTE: setting log_output_for_help also means that we are loading the context for showing
    //       help or messages with help, meaning validation warns and confirms should be silenced
    //       for the manifest
    async fn new(
        global_flags: GolemCliGlobalFlags,
        log_output_for_help: Option<Output>,
        hooks: Arc<Hooks>,
    ) -> anyhow::Result<Self> {
        let start_local_server_yes = Arc::new(tokio::sync::RwLock::new(global_flags.yes));
        Ok(Self {
            ctx: Arc::new(
                Context::new(
                    global_flags,
                    log_output_for_help,
                    start_local_server_yes.clone(),
                    Self::start_local_server_hook(start_local_server_yes),
                )
                .await?,
            ),
            hooks,
        })
    }

    #[cfg(feature = "server-commands")]
    fn start_local_server_hook(
        yes: Arc<tokio::sync::RwLock<bool>>,
    ) -> Box<dyn Fn() -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync> {
        Box::new(move || {
            let yes = yes.clone();
            async move {
                if !InteractiveHandler::confirm_auto_start_local_server(*yes.read().await)? {
                    return Ok(());
                }

                // NOTE: using full path, so we can avoid unused imports for default features
                crate::log::log_warn_action("Starting", "local server");

                Hooks::run_server().await?;

                // NOTE: using full path, so we can avoid unused imports for default features
                crate::log::log_action("Started", "local server");

                Ok(())
            }
            .boxed()
        })
    }

    #[cfg(not(feature = "server-commands"))]
    fn start_local_server_hook(
        _yes: Arc<tokio::sync::RwLock<bool>>,
    ) -> Box<dyn Fn() -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync> {
        Box::new(|| async { Ok(()) }.boxed())
    }

    async fn new_with_init_hint_error_handler(
        global_flags: GolemCliGlobalFlags,
        log_output_for_help: Option<Output>,
        hooks: Arc<Hooks>,
    ) -> anyhow::Result<Self> {
        match Self::new(global_flags.clone(), log_output_for_help, hooks).await {
            Ok(ok) => Ok(ok),
            Err(error) => {
                set_log_output(Output::Stderr);
                if let Some(hint_error) = error.downcast_ref::<ContextInitHintError>() {
                    ErrorHandler::handle_context_init_hint_errors(&global_flags, hint_error)
                        .and_then(|()| Err(anyhow!(NonSuccessfulExit)))
                } else {
                    Err(error)
                }
            }
        }
    }

    pub async fn handle_args<I, T>(args_iterator: I, hooks: Arc<Hooks>) -> ExitCode
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let result = match GolemCliCommand::try_parse_from_lenient(args_iterator, true) {
            GolemCliCommandParseResult::FullMatch(command) => {
                #[cfg(feature = "server-commands")]
                let verbosity = if matches!(command.subcommand, GolemCliSubcommand::Server { .. }) {
                    Hooks::override_verbosity(command.global_flags.verbosity())
                } else {
                    command.global_flags.verbosity()
                };
                #[cfg(feature = "server-commands")]
                let pretty_mode = if matches!(command.subcommand, GolemCliSubcommand::Server { .. })
                {
                    Hooks::override_pretty_mode()
                } else {
                    false
                };
                #[cfg(not(feature = "server-commands"))]
                let verbosity = command.global_flags.verbosity();
                #[cfg(not(feature = "server-commands"))]
                let pretty_mode = false;

                init_tracing(verbosity, pretty_mode);

                match Self::new_with_init_hint_error_handler(
                    command.global_flags.clone(),
                    None,
                    hooks,
                )
                .await
                {
                    Ok(handler) => {
                        let result = handler
                            .handle_command(command)
                            .await
                            .map(|()| ExitCode::SUCCESS);

                        match result {
                            Ok(result) => Ok(result),
                            Err(error) => {
                                set_log_output(Output::Stderr);
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
                    Err(error) => Err(error),
                }
            }
            GolemCliCommandParseResult::ErrorWithPartialMatch {
                error,
                fallback_command,
                partial_match,
            } => {
                init_tracing(
                    fallback_command
                        .global_flags
                        .verbosity
                        .as_clap_verbosity_flag(),
                    false,
                );

                debug!(partial_match = ?partial_match, "Partial match");
                debug_log_parse_error(&error, &fallback_command);

                let handler = Self::new_with_init_hint_error_handler(
                    fallback_command.global_flags.clone(),
                    Some(Output::Stderr),
                    hooks,
                )
                .await;

                logln("");
                error.print().unwrap();

                match handler {
                    Ok(handler) => {
                        let exit_code = clamp_exit_code(error.exit_code());
                        handler
                            .ctx
                            .error_handler()
                            .handle_partial_match(partial_match)
                            .await
                            .map(|_| exit_code)
                    }
                    Err(err) => Err(err),
                }
            }
            GolemCliCommandParseResult::Error {
                error,
                fallback_command,
            } => {
                init_tracing(fallback_command.global_flags.verbosity(), false);
                debug_log_parse_error(&error, &fallback_command);
                error.print().unwrap();

                Ok(clamp_exit_code(error.exit_code()))
            }
        };

        result.unwrap_or_else(|error| {
            if error.downcast_ref::<NonSuccessfulExit>().is_some() {
                // NOP
            } else if error
                .downcast_ref::<Arc<anyhow::Error>>()
                .and_then(|err| err.downcast_ref::<AppValidationError>())
                .is_some()
                || error.downcast_ref::<AppValidationError>().is_some()
            {
                // App validation errors are already formatted and usually contain multiple
                // errors (and warns)
                logln("");
                logln(format!("{error:#}"));
            } else {
                logln("");
                log_error(format!("{error:#}"));
            }
            ExitCode::FAILURE
        })
    }

    async fn handle_command(&self, command: GolemCliCommand) -> anyhow::Result<()> {
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
            GolemCliSubcommand::Api { subcommand } => {
                self.ctx.api_handler().handle_command(subcommand).await
            }
            GolemCliSubcommand::Plugin { subcommand } => {
                self.ctx.plugin_handler().handle_command(subcommand).await
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
            GolemCliSubcommand::Repl {
                component_name,
                version,
            } => {
                self.ctx
                    .rib_repl_handler()
                    .cmd_repl(component_name.component_name, version)
                    .await
            }
            GolemCliSubcommand::Completion { shell } => self.cmd_completion(shell),
        }
    }

    fn cmd_completion(&self, shell: Shell) -> anyhow::Result<()> {
        let mut command = GolemCliCommand::command();
        let command_name = command_name();
        debug!(command_name, shell=%shell, "completion");
        clap_complete::generate(shell, &mut command, command_name, &mut std::io::stdout());
        Ok(())
    }
}

// NOTE: for now every handler can access any other handler, but this can be restricted
//       by moving these simple factory methods into the specific handlers on demand,
//       if the need ever arises
pub trait Handlers {
    fn api_cloud_certificate_handler(&self) -> ApiCloudCertificateCommandHandler;
    fn api_cloud_domain_handler(&self) -> ApiCloudDomainCommandHandler;
    fn api_cloud_handler(&self) -> ApiCloudCommandHandler;
    fn api_definition_handler(&self) -> ApiDefinitionCommandHandler;
    fn api_deployment_handler(&self) -> ApiDeploymentCommandHandler;
    fn api_handler(&self) -> ApiCommandHandler;
    fn api_security_scheme_handler(&self) -> ApiSecuritySchemeCommandHandler;
    fn app_handler(&self) -> AppCommandHandler;
    fn cloud_account_grant_handler(&self) -> CloudAccountGrantCommandHandler;
    fn cloud_account_handler(&self) -> CloudAccountCommandHandler;
    fn cloud_handler(&self) -> CloudCommandHandler;
    fn cloud_project_handler(&self) -> CloudProjectCommandHandler;
    fn cloud_project_plugin_handler(&self) -> CloudProjectPluginCommandHandler;
    fn cloud_project_policy_handler(&self) -> CloudProjectPolicyCommandHandler;
    fn cloud_token_handler(&self) -> CloudTokenCommandHandler;
    fn component_handler(&self) -> ComponentCommandHandler;
    fn component_plugin_handler(&self) -> ComponentPluginCommandHandler;
    fn error_handler(&self) -> ErrorHandler;
    fn interactive_handler(&self) -> InteractiveHandler;
    fn log_handler(&self) -> LogHandler;
    fn plugin_installation_handler(&self) -> PluginInstallationHandler;
    fn plugin_handler(&self) -> PluginCommandHandler;
    fn profile_config_handler(&self) -> ProfileConfigCommandHandler;
    fn profile_handler(&self) -> ProfileCommandHandler;
    fn rib_repl_handler(&self) -> RibReplHandler;
    fn worker_handler(&self) -> WorkerCommandHandler;
}

impl Handlers for Arc<Context> {
    fn api_cloud_certificate_handler(&self) -> ApiCloudCertificateCommandHandler {
        ApiCloudCertificateCommandHandler::new(self.clone())
    }

    fn api_cloud_domain_handler(&self) -> ApiCloudDomainCommandHandler {
        ApiCloudDomainCommandHandler::new(self.clone())
    }

    fn api_cloud_handler(&self) -> ApiCloudCommandHandler {
        ApiCloudCommandHandler::new(self.clone())
    }

    fn api_definition_handler(&self) -> ApiDefinitionCommandHandler {
        ApiDefinitionCommandHandler::new(self.clone())
    }

    fn api_deployment_handler(&self) -> ApiDeploymentCommandHandler {
        ApiDeploymentCommandHandler::new(self.clone())
    }

    fn api_handler(&self) -> ApiCommandHandler {
        ApiCommandHandler::new(self.clone())
    }

    fn api_security_scheme_handler(&self) -> ApiSecuritySchemeCommandHandler {
        ApiSecuritySchemeCommandHandler::new(self.clone())
    }

    fn app_handler(&self) -> AppCommandHandler {
        AppCommandHandler::new(self.clone())
    }

    fn cloud_account_grant_handler(&self) -> CloudAccountGrantCommandHandler {
        CloudAccountGrantCommandHandler::new(self.clone())
    }

    fn cloud_account_handler(&self) -> CloudAccountCommandHandler {
        CloudAccountCommandHandler::new(self.clone())
    }

    fn cloud_handler(&self) -> CloudCommandHandler {
        CloudCommandHandler::new(self.clone())
    }

    fn cloud_project_handler(&self) -> CloudProjectCommandHandler {
        CloudProjectCommandHandler::new(self.clone())
    }

    fn cloud_project_plugin_handler(&self) -> CloudProjectPluginCommandHandler {
        CloudProjectPluginCommandHandler::new(self.clone())
    }

    fn cloud_project_policy_handler(&self) -> CloudProjectPolicyCommandHandler {
        CloudProjectPolicyCommandHandler::new(self.clone())
    }

    fn cloud_token_handler(&self) -> CloudTokenCommandHandler {
        CloudTokenCommandHandler::new(self.clone())
    }

    fn component_handler(&self) -> ComponentCommandHandler {
        ComponentCommandHandler::new(self.clone())
    }

    fn component_plugin_handler(&self) -> ComponentPluginCommandHandler {
        ComponentPluginCommandHandler::new(self.clone())
    }

    fn error_handler(&self) -> ErrorHandler {
        ErrorHandler::new(self.clone())
    }

    fn interactive_handler(&self) -> InteractiveHandler {
        InteractiveHandler::new(self.clone())
    }

    fn log_handler(&self) -> LogHandler {
        LogHandler::new(self.clone())
    }

    fn plugin_installation_handler(&self) -> PluginInstallationHandler {
        PluginInstallationHandler::new(self.clone())
    }

    fn plugin_handler(&self) -> PluginCommandHandler {
        PluginCommandHandler::new(self.clone())
    }

    fn profile_config_handler(&self) -> ProfileConfigCommandHandler {
        ProfileConfigCommandHandler::new(self.clone())
    }

    fn profile_handler(&self) -> ProfileCommandHandler {
        ProfileCommandHandler::new(self.clone())
    }

    fn rib_repl_handler(&self) -> RibReplHandler {
        RibReplHandler::new(self.clone())
    }

    fn worker_handler(&self) -> WorkerCommandHandler {
        WorkerCommandHandler::new(self.clone())
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
