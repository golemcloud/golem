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

use self::resource_definition::ResourceDefinitionCommandHandler;
use self::retry_policy::RetryPolicyCommandHandler;
use self::secret::SecretCommandHandler;
use crate::command::agent_type::AgentTypeSubcommand;
#[cfg(feature = "server-commands")]
use crate::command::server::ServerSubcommand;
use crate::command::{
    GolemCliCommand, GolemCliCommandParseResult, GolemCliFallbackCommand, GolemCliGlobalFlags,
    GolemCliSubcommand,
};
use crate::command_handler::account::AccountCommandHandler;
use crate::command_handler::api::ApiCommandHandler;
use crate::command_handler::api::deployment::ApiDeploymentCommandHandler;
use crate::command_handler::api::domain::ApiDomainCommandHandler;
use crate::command_handler::api::security_scheme::ApiSecuritySchemeCommandHandler;
use crate::command_handler::api_token::ApiTokenCommandHandler;
use crate::command_handler::app::AppCommandHandler;
use crate::command_handler::bridge::BridgeCommandHandler;
use crate::command_handler::component::ComponentCommandHandler;
use crate::command_handler::environment::EnvironmentCommandHandler;
use crate::command_handler::interactive::InteractiveHandler;
use crate::command_handler::log::LogHandler;
use crate::command_handler::log::render_structured_document;
use crate::command_handler::partial_match::ErrorHandler;
use crate::command_handler::plugin::PluginCommandHandler;
use crate::command_handler::profile::ProfileCommandHandler;
use crate::command_handler::profile::config::ProfileConfigCommandHandler;
use crate::command_handler::repl::ReplHandler;
use crate::command_handler::worker::WorkerCommandHandler;
use crate::context::Context;
use crate::error::{ContextInitHintError, HintError, NonSuccessfulExit, PipedExitCode};
use crate::log::{Output, log_anyhow_error, logln, set_log_output};
use crate::model::format::Format;
use crate::{command_name, init_tracing};
use anyhow::anyhow;
use clap::CommandFactory;
use clap_complete::Shell;
#[cfg(feature = "server-commands")]
use clap_verbosity_flag::Verbosity;
use colored::control::SHOULD_COLORIZE;
use std::ffi::OsString;
use std::marker::PhantomData;
use std::process::ExitCode;
use std::sync::Arc;
use tracing::{Level, debug};

mod account;
mod api;
mod api_token;
mod app;
mod bridge;
mod component;
mod environment;
pub(crate) mod interactive;
mod log;
mod partial_match;
mod plugin;
mod profile;
mod repl;
mod resource_definition;
mod retry_policy;
mod secret;
pub(crate) mod template;
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

// CommandHandler is responsible for matching commands and producing CLI output.
// Context is initialized lazily only for command arms that need it.
pub struct CommandHandler<Hooks: CommandHandlerHooks> {
    _phantom: PhantomData<Hooks>,
}

impl<Hooks: CommandHandlerHooks + 'static> CommandHandler<Hooks> {
    // NOTE: setting log_output_for_help also means that we are loading the context for showing
    //       help or messages with help, meaning validation warns and confirms should be silenced
    //       for the manifest
    async fn new_context(
        global_flags: GolemCliGlobalFlags,
        log_output_for_help: Option<Output>,
    ) -> anyhow::Result<Arc<Context>> {
        Ok(Arc::new(
            Context::new(global_flags, log_output_for_help).await?,
        ))
    }

    async fn new_context_with_init_hint_error_handler(
        global_flags: GolemCliGlobalFlags,
        log_output_for_help: Option<Output>,
    ) -> anyhow::Result<Arc<Context>> {
        match Self::new_context(global_flags.clone(), log_output_for_help).await {
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

                let mut lazy_context = LazyContext::new(command.global_flags.clone(), hooks);
                let result = Self::handle_subcommand(&mut lazy_context, command.subcommand)
                    .await
                    .map(|()| ExitCode::SUCCESS);

                match result {
                    Ok(result) => Ok(result),
                    Err(error) => {
                        set_log_output(Output::Stderr);
                        if let Some(ctx) = lazy_context.initialized_context()
                            && let Some(hint_error) = error.downcast_ref::<HintError>()
                        {
                            ctx.error_handler()
                                .handle_hint_errors(hint_error)
                                .map(|()| ExitCode::FAILURE)
                        } else {
                            Err(error)
                        }
                    }
                }
            }
            GolemCliCommandParseResult::ErrorWithPartialMatch {
                error,
                fallback_command,
                partial_match,
            } => {
                init_tracing(fallback_command.global_flags.verbosity(), false);

                debug!(partial_match = ?partial_match, "Partial match");
                debug_log_parse_error(&error, &fallback_command);

                let ctx = Self::new_context_with_init_hint_error_handler(
                    fallback_command.global_flags.clone(),
                    Some(Output::Stderr),
                )
                .await;

                logln("");
                error.print().unwrap();

                match ctx {
                    Ok(ctx) => {
                        let exit_code = clamp_exit_code(error.exit_code());
                        ctx.error_handler()
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
            log_anyhow_error(&error);
            if let Some(piped) = error.downcast_ref::<PipedExitCode>() {
                ExitCode::from(piped.0)
            } else {
                ExitCode::FAILURE
            }
        })
    }

    fn handle_subcommand(
        ctx: &mut LazyContext<Hooks>,
        subcommand: GolemCliSubcommand,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + '_>> {
        Box::pin(async move {
            match subcommand {
                // App scoped root commands
                GolemCliSubcommand::New {
                    application_path,
                    application_name,
                    component_name,
                    template,
                } => {
                    ctx.get_or_init()
                        .await?
                        .app_handler()
                        .cmd_new(application_path, application_name, component_name, template)
                        .await
                }
                GolemCliSubcommand::Templates { filter } => {
                    ctx.get_or_init().await?.app_handler().cmd_templates(filter)
                }
                GolemCliSubcommand::Build {
                    component_name,
                    build: build_args,
                } => {
                    ctx.get_or_init()
                        .await?
                        .app_handler()
                        .cmd_build(component_name, build_args)
                        .await
                }
                GolemCliSubcommand::GenerateBridge {
                    language,
                    component_name,
                    agent_type_name,
                    output_dir,
                } => {
                    ctx.get_or_init()
                        .await?
                        .bridge_handler()
                        .cmd_generate_bridge(language, component_name, agent_type_name, output_dir)
                        .await
                }
                GolemCliSubcommand::Repl {
                    language,
                    component_name,
                    revision,
                    post_deploy_args,
                    script,
                    script_file,
                    disable_stream,
                    disable_auto_imports,
                } => {
                    ctx.get_or_init()
                        .await?
                        .repl_handler()
                        .cmd_repl(
                            language,
                            component_name.component_name,
                            revision,
                            post_deploy_args.as_ref(),
                            script,
                            script_file,
                            !disable_stream,
                            disable_auto_imports,
                        )
                        .await
                }
                GolemCliSubcommand::Deploy {
                    plan,
                    stage,
                    approve_staging_steps,
                    full_diff,
                    version,
                    revision,
                    force_build,
                    post_deploy_args,
                    repl_bridge_sdk_target,
                } => {
                    ctx.get_or_init()
                        .await?
                        .app_handler()
                        .cmd_deploy(
                            plan,
                            stage,
                            approve_staging_steps,
                            full_diff,
                            version,
                            revision,
                            force_build,
                            post_deploy_args,
                            repl_bridge_sdk_target,
                        )
                        .await
                }
                GolemCliSubcommand::Clean { component_name } => {
                    ctx.get_or_init()
                        .await?
                        .app_handler()
                        .cmd_clean(component_name)
                        .await
                }
                GolemCliSubcommand::UpdateAgents {
                    component_name,
                    update_mode,
                    r#await,
                    disable_wakeup,
                } => {
                    ctx.get_or_init()
                        .await?
                        .app_handler()
                        .cmd_update_workers(
                            component_name.component_name,
                            update_mode,
                            r#await,
                            disable_wakeup,
                        )
                        .await
                }
                GolemCliSubcommand::RedeployAgents { component_name } => {
                    ctx.get_or_init()
                        .await?
                        .app_handler()
                        .cmd_redeploy_workers(component_name.component_name)
                        .await
                }
                GolemCliSubcommand::Exec { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .app_handler()
                        .exec_custom_command(subcommand)
                        .await
                }

                // Other entities
                GolemCliSubcommand::Environment { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .environment_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::Component { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .component_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::Agent { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .worker_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::AgentType { subcommand } => match subcommand {
                    AgentTypeSubcommand::List => {
                        ctx.get_or_init()
                            .await?
                            .app_handler()
                            .cmd_list_agent_types()
                            .await
                    }
                    AgentTypeSubcommand::Get { agent_type_name } => {
                        ctx.get_or_init()
                            .await?
                            .app_handler()
                            .cmd_get_agent_type(agent_type_name)
                            .await
                    }
                },
                GolemCliSubcommand::Api { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .api_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::Plugin { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .plugin_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::Profile { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .profile_handler()
                        .handle_command(subcommand)
                        .await
                }
                #[cfg(feature = "server-commands")]
                GolemCliSubcommand::Server { subcommand } => {
                    let (context, hooks) = ctx.get_or_init_with_hooks().await?;
                    hooks.handler_server_commands(context, subcommand).await
                }
                GolemCliSubcommand::Account { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .account_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::ApiToken { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .api_token_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::Secret { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .secret_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::RetryPolicy { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .retry_policy_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::Resource { subcommand } => {
                    ctx.get_or_init()
                        .await?
                        .resource_definition_handler()
                        .handle_command(subcommand)
                        .await
                }
                GolemCliSubcommand::OutputSchema { types, output_type } => Self::cmd_output_schema(
                    ctx.global_flags.format.unwrap_or_default(),
                    types,
                    output_type,
                ),
                GolemCliSubcommand::Completion { shell } => Self::cmd_completion(shell),
            }
        })
    }

    fn cmd_output_schema(
        format: Format,
        types: bool,
        output_type: Vec<String>,
    ) -> anyhow::Result<()> {
        let value = if types {
            crate::model::cli_output::command_output_type_names()?
        } else if output_type.is_empty() {
            crate::model::cli_output::command_output_schema_value()?
        } else {
            crate::model::cli_output::focused_command_output_schema(&output_type)?
        };

        println!("{}", render_raw_schema_document(format, &value)?);
        Ok(())
    }

    fn cmd_completion(shell: Shell) -> anyhow::Result<()> {
        let mut command = GolemCliCommand::command();
        let command_name = command_name();
        debug!(command_name, shell=%shell, "completion");
        clap_complete::generate(shell, &mut command, command_name, &mut std::io::stdout());
        Ok(())
    }
}

fn render_raw_schema_document(format: Format, value: &serde_json::Value) -> anyhow::Result<String> {
    match format {
        Format::Text => Ok(serde_json::to_string(value)?),
        Format::Json | Format::PrettyJson | Format::Yaml | Format::PrettyYaml | Format::Toon => {
            render_structured_document(format, SHOULD_COLORIZE.should_colorize(), value)
        }
    }
}

struct LazyContext<Hooks: CommandHandlerHooks> {
    global_flags: GolemCliGlobalFlags,
    #[allow(unused)]
    hooks: Arc<Hooks>,
    ctx: Option<Arc<Context>>,
}

impl<Hooks: CommandHandlerHooks + 'static> LazyContext<Hooks> {
    fn new(global_flags: GolemCliGlobalFlags, hooks: Arc<Hooks>) -> Self {
        Self {
            global_flags,
            hooks,
            ctx: None,
        }
    }

    // Context-free commands such as `output-schema` and `completion` must not
    // call this; all contextual command arms initialize through this method.
    async fn get_or_init(&mut self) -> anyhow::Result<Arc<Context>> {
        if self.ctx.is_none() {
            self.ctx = Some(
                CommandHandler::<Hooks>::new_context_with_init_hint_error_handler(
                    self.global_flags.clone(),
                    None,
                )
                .await?,
            );
        }

        Ok(self.ctx.as_ref().expect("context initialized").clone())
    }

    #[cfg(feature = "server-commands")]
    async fn get_or_init_with_hooks(&mut self) -> anyhow::Result<(Arc<Context>, Arc<Hooks>)> {
        let ctx = self.get_or_init().await?;
        Ok((ctx, self.hooks.clone()))
    }

    fn initialized_context(&self) -> Option<Arc<Context>> {
        self.ctx.clone()
    }
}

// NOTE: for now every handler can access any other handler, but this can be restricted
//       by moving these simple factory methods into the specific handlers on-demand,
//       if the need ever arises
pub trait Handlers {
    fn account_handler(&self) -> AccountCommandHandler;
    fn secret_handler(&self) -> SecretCommandHandler;
    fn retry_policy_handler(&self) -> RetryPolicyCommandHandler;
    fn resource_definition_handler(&self) -> ResourceDefinitionCommandHandler;
    fn api_domain_handler(&self) -> ApiDomainCommandHandler;
    fn api_deployment_handler(&self) -> ApiDeploymentCommandHandler;
    fn api_handler(&self) -> ApiCommandHandler;
    fn api_security_scheme_handler(&self) -> ApiSecuritySchemeCommandHandler;
    fn api_token_handler(&self) -> ApiTokenCommandHandler;
    fn app_handler(&self) -> AppCommandHandler;
    fn bridge_handler(&self) -> BridgeCommandHandler;
    fn component_handler(&self) -> ComponentCommandHandler;
    fn environment_handler(&self) -> EnvironmentCommandHandler;
    fn error_handler(&self) -> ErrorHandler;
    fn interactive_handler(&self) -> InteractiveHandler;
    fn log_handler(&self) -> LogHandler;
    fn plugin_handler(&self) -> PluginCommandHandler;
    fn profile_config_handler(&self) -> ProfileConfigCommandHandler;
    fn profile_handler(&self) -> ProfileCommandHandler;
    fn repl_handler(&self) -> ReplHandler;
    fn worker_handler(&self) -> WorkerCommandHandler;
}

impl Handlers for Arc<Context> {
    fn account_handler(&self) -> AccountCommandHandler {
        AccountCommandHandler::new(self.clone())
    }

    fn secret_handler(&self) -> SecretCommandHandler {
        SecretCommandHandler::new(self.clone())
    }

    fn retry_policy_handler(&self) -> RetryPolicyCommandHandler {
        RetryPolicyCommandHandler::new(self.clone())
    }

    fn resource_definition_handler(&self) -> ResourceDefinitionCommandHandler {
        ResourceDefinitionCommandHandler::new(self.clone())
    }

    fn api_domain_handler(&self) -> ApiDomainCommandHandler {
        ApiDomainCommandHandler::new(self.clone())
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

    fn api_token_handler(&self) -> ApiTokenCommandHandler {
        ApiTokenCommandHandler::new(self.clone())
    }

    fn app_handler(&self) -> AppCommandHandler {
        AppCommandHandler::new(self.clone())
    }

    fn bridge_handler(&self) -> BridgeCommandHandler {
        BridgeCommandHandler::new(self.clone())
    }

    fn component_handler(&self) -> ComponentCommandHandler {
        ComponentCommandHandler::new(self.clone())
    }

    fn environment_handler(&self) -> EnvironmentCommandHandler {
        EnvironmentCommandHandler::new(self.clone())
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

    // TODO: atomic:
    /*
    fn plugin_installation_handler(&self) -> PluginInstallationHandler {
        PluginInstallationHandler::new(self.clone())
    }
    */

    fn plugin_handler(&self) -> PluginCommandHandler {
        PluginCommandHandler::new(self.clone())
    }

    fn profile_config_handler(&self) -> ProfileConfigCommandHandler {
        ProfileConfigCommandHandler::new(self.clone())
    }

    fn profile_handler(&self) -> ProfileCommandHandler {
        ProfileCommandHandler::new(self.clone())
    }

    fn repl_handler(&self) -> ReplHandler {
        ReplHandler::new(self.clone())
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
