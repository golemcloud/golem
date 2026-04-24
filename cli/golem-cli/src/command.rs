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

use crate::app::template::AppTemplateName;
use crate::command::agent_type::AgentTypeSubcommand;
use crate::command::api::ApiSubcommand;
use crate::command::api::agent_secret::AgentSecretSubcommand;
use crate::command::cloud::CloudSubcommand;
use crate::command::component::ComponentSubcommand;
use crate::command::environment::EnvironmentSubcommand;
use crate::command::exec::ExecSubcommand;
use crate::command::plugin::PluginSubcommand;
use crate::command::profile::ProfileSubcommand;
use crate::command::resource_definition::ResourceDefinitionSubcommand;
use crate::command::retry_policy::RetryPolicySubcommand;
#[cfg(feature = "server-commands")]
use crate::command::server::ServerSubcommand;
use crate::command::shared_args::{
    BuildArgs, ForceBuildArg, OptionalComponentName, OptionalComponentNames, PostDeployArgs,
};
use crate::command::worker::AgentSubcommand;
use crate::config::ProfileName;
use crate::error::ShowClapHelpTarget;
use crate::model::GuestLanguage;
use crate::model::app::ComponentPresetName;
use crate::model::cli_command_metadata::{CliCommandMetadata, CliMetadataFilter};
use crate::model::environment::EnvironmentReference;
use crate::model::format::Format;
use crate::model::repl::ReplLanguage;
use crate::model::worker::{AgentUpdateMode, RawAgentId};
use crate::{command_name, version};
use anyhow::{Context as AnyhowContext, anyhow};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Args, Parser};
use clap::{Command, CommandFactory, Subcommand};
use clap_verbosity_flag::{ErrorLevel, LogLevel};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use lenient_bool::LenientBool;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::PathBuf;

/// Golem Command Line Interface
#[derive(Debug, Parser)]
#[command(bin_name = command_name(), display_name = command_name(), long_version = version())]
pub struct GolemCliCommand {
    #[command(flatten)]
    pub global_flags: GolemCliGlobalFlags,

    #[clap(subcommand)]
    pub subcommand: GolemCliSubcommand,
}

impl GolemCliCommand {
    pub fn collect_metadata() -> CliCommandMetadata {
        CliCommandMetadata::new(&Self::command())
    }

    pub fn collect_metadata_for_repl() -> CliCommandMetadata {
        CliCommandMetadata::new_filtered(
            &GolemCliCommand::command(),
            &CliMetadataFilter {
                command_path_prefix_exclude: vec![
                    vec!["api"], // TODO: recheck after code-first routes is implemented
                    vec!["clean"],
                    vec!["cloud"],
                    vec!["completion"],
                    vec!["generate-bridge"],
                    vec!["new"],
                    vec!["plugin"],
                    vec!["profile"],
                    vec!["repl"],
                    vec!["server"],
                ],
                arg_id_exclude: vec![
                    "app_manifest_path",
                    "cloud",
                    "config_dir",
                    "dev_mode",
                    "disable_app_manifest_discovery",
                    "environment",
                    "local",
                    "preset",
                    "profile",
                    "show_sensitive",
                ],
                exclude_hidden: true,
            },
        )
    }
}

// NOTE: inlined from clap-verbosity-flag, so we can override display order,
//       check for possible changes when upgrading clap-verbosity-flag
#[derive(clap::Args, Debug, Clone, Copy, Default)]
#[command(about = None, long_about = None)]
pub struct Verbosity<L: LogLevel = ErrorLevel> {
    #[arg(
        long,
        short = 'v',
        action = clap::ArgAction::Count,
        global = true,
        help = L::verbose_help(),
        long_help = L::verbose_long_help(),
        display_order = 201
    )]
    verbose: u8,

    #[arg(
        long,
        short = 'q',
        action = clap::ArgAction::Count,
        global = true,
        help = L::quiet_help(),
        long_help = L::quiet_long_help(),
        conflicts_with = "verbose",
        display_order = 202
    )]
    quiet: u8,

    #[arg(skip)]
    phantom: std::marker::PhantomData<L>,
}

impl Verbosity {
    pub fn as_clap_verbosity_flag(self) -> clap_verbosity_flag::Verbosity {
        clap_verbosity_flag::Verbosity::new(self.verbose, self.quiet)
    }
}

// TODO: flags for defining target server for "non-manifest" mode
#[derive(Debug, Clone, Default, Args)]
pub struct GolemCliGlobalFlags {
    /// Output format, defaults to text, unless specified by the selected profile
    #[arg(long, short = 'F', global = true, display_order = 101)]
    pub format: Option<Format>,

    /// Select Golem environment by name
    #[arg(long, short = 'E', global = true, display_order = 102)]
    pub environment: Option<EnvironmentReference>,

    /// Select "local" environment from the manifest, or "local" profile
    #[arg(long, short = 'L', global = true, conflicts_with_all = ["cloud"], display_order = 103)]
    pub local: bool,

    /// Select "cloud" environment from the manifest, or "cloud" profile
    #[arg(long, short = 'C', global = true, conflicts_with_all = ["local"], display_order = 104)]
    pub cloud: bool,

    /// Custom path to the root application manifest (golem.yaml)
    #[arg(long, short = 'A', global = true, display_order = 105)]
    pub app_manifest_path: Option<PathBuf>,

    /// Disable automatic searching for application manifests
    #[arg(long, short = 'X', global = true, display_order = 106)]
    pub disable_app_manifest_discovery: bool,

    /// Select custom component presets
    #[arg(
        long,
        short = 'P',
        global = true,
        value_delimiter = ',',
        display_order = 107
    )]
    pub preset: Vec<ComponentPresetName>,

    /// Select Golem profile by name
    #[arg(long, global = true, display_order = 108)]
    pub profile: Option<ProfileName>,

    /// Custom path to the config directory (defaults to $HOME/.golem)
    #[arg(long, global = true, display_order = 109)]
    config_dir: Option<PathBuf>,

    /// Automatically answer "yes" to any interactive confirm questions
    #[arg(long, short = 'Y', global = true, display_order = 110)]
    pub yes: bool,

    /// Disables filtering of potentially sensitive user values in text mode (e.g. component environment variable values)
    #[arg(long, global = true, display_order = 111)]
    pub show_sensitive: bool,

    /// Enable experimental, development-only features
    #[arg(long, global = true, display_order = 112)]
    pub dev_mode: bool,

    #[command(flatten)]
    verbosity: Verbosity,

    #[arg(skip)]
    pub wasm_rpc_offline: bool,

    #[arg(skip)]
    http_batch_size: Option<u64>,

    #[arg(skip)]
    http_parallelism: Option<usize>,

    #[arg(skip)]
    pub auth_token: Option<String>,

    #[arg(skip)]
    pub server_no_limit_change: bool,

    #[arg(skip)]
    pub enable_wasmtime_fs_cache: bool,
}

impl GolemCliGlobalFlags {
    pub fn with_env_overrides(mut self) -> anyhow::Result<GolemCliGlobalFlags> {
        if self.profile.is_none()
            && let Ok(profile) = std::env::var("GOLEM_PROFILE")
        {
            self.profile = Some(profile.into());
        }

        if self.environment.is_none()
            && let Ok(environment) = std::env::var("GOLEM_ENVIRONMENT")
        {
            self.environment = Some(
                EnvironmentReference::try_from(environment)
                    .map_err(|err| anyhow!(err))
                    .context("Failed to parse GOLEM_ENVIRONMENT environment variable")?,
            );
        }

        if self.app_manifest_path.is_none()
            && let Ok(app_manifest_path) = std::env::var("GOLEM_APP_MANIFEST_PATH")
        {
            self.app_manifest_path = Some(PathBuf::from(app_manifest_path));
        }

        if !self.disable_app_manifest_discovery
            && let Ok(disable) = std::env::var("GOLEM_DISABLE_APP_MANIFEST_DISCOVERY")
        {
            self.disable_app_manifest_discovery = disable
                .parse::<LenientBool>()
                .map(|b| b.into())
                .unwrap_or_default()
        }

        if self.preset.is_empty()
            && let Ok(preset) = std::env::var("GOLEM_PRESET")
        {
            self.preset = preset
                .split(',')
                .map(|preset| preset.parse())
                .collect::<Result<Vec<_>, String>>()
                .map_err(|err| anyhow!(err))?;
        }

        if let Ok(offline) = std::env::var("GOLEM_WASM_RPC_OFFLINE") {
            self.wasm_rpc_offline = offline
                .parse::<LenientBool>()
                .map(|b| b.into())
                .unwrap_or_default()
        }

        if let Ok(batch_size) = std::env::var("GOLEM_HTTP_BATCH_SIZE") {
            self.http_batch_size =
                Some(batch_size.parse().with_context(|| {
                    format!("Failed to parse GOLEM_HTTP_BATCH_SIZE: {batch_size}")
                })?)
        }

        if let Ok(parallelism) = std::env::var("GOLEM_HTTP_PARALLELISM") {
            self.http_parallelism = Some(parallelism.parse().with_context(|| {
                format!("Failed to parse GOLEM_HTTP_PARALLELISM: {parallelism}")
            })?)
        }

        if let Ok(auth_token) = std::env::var("GOLEM_AUTH_TOKEN") {
            self.auth_token = Some(
                auth_token
                    .parse()
                    .context("Failed to parse GOLEM_AUTH_TOKEN, expected uuid")?,
            );
        }

        if let Ok(server_no_limit_change) = std::env::var("GOLEM_SERVER_NO_LIMIT_CHANGE") {
            self.server_no_limit_change = server_no_limit_change
                .parse::<LenientBool>()
                .map(|b| b.into())
                .unwrap_or_default()
        }

        if let Ok(enable_wasmtime_fs_cache) = std::env::var("GOLEM_ENABLE_WASMTIME_FS_CACHE") {
            self.enable_wasmtime_fs_cache = enable_wasmtime_fs_cache
                .parse::<LenientBool>()
                .map(|b| b.into())
                .unwrap_or_default()
        }

        Ok(self)
    }

    pub fn config_dir(&self) -> PathBuf {
        self.config_dir
            .clone()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".golem"))
    }

    pub fn http_batch_size(&self) -> u64 {
        self.http_batch_size.unwrap_or(50)
    }

    pub fn http_parallelism(&self) -> usize {
        self.http_parallelism.unwrap_or(4)
    }

    pub fn verbosity(&self) -> clap_verbosity_flag::Verbosity {
        self.verbosity.as_clap_verbosity_flag()
    }
}

#[derive(Debug, Default, Parser)]
#[command(ignore_errors = true)]
pub struct GolemCliFallbackCommand {
    #[command(flatten)]
    pub global_flags: GolemCliGlobalFlags,

    pub positional_args: Vec<String>,

    #[arg(skip)]
    pub parse_error: Option<clap::Error>,
}

impl GolemCliFallbackCommand {
    fn try_parse_from<I, T>(args: I, with_env_overrides: bool) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let args = args
            .into_iter()
            .map(|arg| arg.into())
            .filter(|arg| arg != "-h" && arg != "--help")
            .collect::<Vec<OsString>>();

        let mut cmd = <Self as Parser>::try_parse_from(args).unwrap_or_else(|error| {
            GolemCliFallbackCommand {
                parse_error: Some(error),
                ..Self::default()
            }
        });

        if with_env_overrides {
            cmd.global_flags = cmd.global_flags.with_env_overrides()?;
        }

        Ok(cmd)
    }
}

impl GolemCliCommand {
    pub fn try_parse_from_lenient<I, T>(
        iterator: I,
        with_env_overrides: bool,
    ) -> GolemCliCommandParseResult
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let args = iterator
            .into_iter()
            .map(|arg| arg.into())
            .collect::<Vec<OsString>>();

        match GolemCliCommand::try_parse_from(&args) {
            Ok(mut command) => {
                if with_env_overrides {
                    match command.global_flags.with_env_overrides() {
                        Ok(global_flags) => {
                            command.global_flags = global_flags;
                        }
                        Err(err) => {
                            return GolemCliCommandParseResult::Error {
                                error: clap::Error::raw(ErrorKind::InvalidValue, err),
                                fallback_command: Default::default(),
                            };
                        }
                    }
                }
                GolemCliCommandParseResult::FullMatch(command)
            }
            Err(error) => {
                let fallback_command =
                    match GolemCliFallbackCommand::try_parse_from(&args, with_env_overrides) {
                        Ok(fallback_command) => fallback_command,
                        Err(err) => {
                            return GolemCliCommandParseResult::Error {
                                error: clap::Error::raw(ErrorKind::InvalidValue, err),
                                fallback_command: Default::default(),
                            };
                        }
                    };

                let partial_match = match error.kind() {
                    ErrorKind::DisplayHelp => {
                        let positional_args = fallback_command
                            .positional_args
                            .iter()
                            .map(|arg| arg.as_ref())
                            .collect::<Vec<_>>();
                        match positional_args.as_slice() {
                            [] => Some(GolemCliCommandPartialMatch::AppHelp),
                            ["exec"] => Some(GolemCliCommandPartialMatch::AppMissingSubcommandHelp),
                            ["component"] => Some(GolemCliCommandPartialMatch::ComponentHelp),
                            ["agent"] => Some(GolemCliCommandPartialMatch::AgentHelp),
                            _ => None,
                        }
                    }
                    ErrorKind::MissingRequiredArgument => {
                        error.context().find_map(|context| match context {
                            (ContextKind::InvalidArg, ContextValue::Strings(args)) => {
                                Self::match_invalid_arg(
                                    &fallback_command.positional_args,
                                    args,
                                    &Self::invalid_arg_matchers(),
                                )
                            }
                            _ => None,
                        })
                    }
                    ErrorKind::MissingSubcommand
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                        let positional_args = fallback_command
                            .positional_args
                            .iter()
                            .map(|arg| arg.as_ref())
                            .collect::<Vec<_>>();
                        match positional_args.as_slice() {
                            [] => Some(GolemCliCommandPartialMatch::AppMissingSubcommandHelp),
                            ["exec"] => Some(GolemCliCommandPartialMatch::AppMissingSubcommandHelp),
                            ["component"] => {
                                Some(GolemCliCommandPartialMatch::ComponentMissingSubcommandHelp)
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };

                match partial_match {
                    Some(partial_match) => GolemCliCommandParseResult::ErrorWithPartialMatch {
                        error,
                        fallback_command,
                        partial_match,
                    },
                    None => GolemCliCommandParseResult::Error {
                        error,
                        fallback_command,
                    },
                }
            }
        }
    }

    fn invalid_arg_matchers() -> Vec<InvalidArgMatcher> {
        vec![
            InvalidArgMatcher {
                subcommands: vec!["agent", "invoke"],
                found_positional_args: vec![],
                missing_positional_arg: "agent_id",
                to_partial_match: |_| GolemCliCommandPartialMatch::AgentInvokeMissingAgentName,
            },
            InvalidArgMatcher {
                subcommands: vec!["agent", "invoke"],
                found_positional_args: vec!["agent_id"],
                missing_positional_arg: "function_name",
                to_partial_match: |args| {
                    GolemCliCommandPartialMatch::AgentInvokeMissingFunctionName {
                        agent_name: args[0].clone().into(),
                    }
                },
            },
            InvalidArgMatcher {
                subcommands: vec!["profile", "switch"],
                missing_positional_arg: "profile_name",
                found_positional_args: vec![],
                to_partial_match: |_| GolemCliCommandPartialMatch::ProfileSwitchMissingProfileName,
            },
        ]
    }

    fn match_invalid_arg(
        positional_args: &[String],
        error_context_args: &[String],
        matchers: &[InvalidArgMatcher],
    ) -> Option<GolemCliCommandPartialMatch> {
        let command = Self::command();

        let positional_args = positional_args
            .iter()
            .map(|str| str.as_str())
            .collect::<Vec<_>>();

        for matcher in matchers {
            if positional_args.len() < matcher.subcommands.len() {
                continue;
            }

            let missing_arg_error_name =
                format!("<{}>", matcher.missing_positional_arg.to_uppercase());
            let missing_args_error_name = format!("{missing_arg_error_name}...");
            if !error_context_args.contains(&missing_arg_error_name)
                && !error_context_args.contains(&missing_args_error_name)
            {
                continue;
            }

            if !positional_args.starts_with(&matcher.subcommands) {
                continue;
            }

            let mut command = &command;
            for subcommand in &matcher.subcommands {
                command = command.find_subcommand(subcommand).unwrap();
            }
            let positional_arg_ids_to_idx = command
                .get_arguments()
                .filter(|arg| arg.is_positional())
                .enumerate()
                .map(|(idx, arg)| (arg.get_id().to_string(), idx))
                .collect::<HashMap<_, _>>();

            let mut found_args = Vec::<String>::with_capacity(matcher.found_positional_args.len());
            for expected_arg_name in &matcher.found_positional_args {
                let Some(idx) = positional_arg_ids_to_idx.get(*expected_arg_name) else {
                    break;
                };
                let Some(arg_value) = positional_args.get(matcher.subcommands.len() + *idx) else {
                    break;
                };
                found_args.push(arg_value.to_string());
            }
            if found_args.len() == matcher.found_positional_args.len() {
                return Some((matcher.to_partial_match)(found_args));
            }
        }

        None
    }
}

#[derive(Debug)]
struct InvalidArgMatcher {
    pub subcommands: Vec<&'static str>,
    pub found_positional_args: Vec<&'static str>,
    pub missing_positional_arg: &'static str,
    pub to_partial_match: fn(Vec<String>) -> GolemCliCommandPartialMatch,
}

#[allow(clippy::large_enum_variant)]
pub enum GolemCliCommandParseResult {
    FullMatch(GolemCliCommand),
    ErrorWithPartialMatch {
        error: clap::Error,
        fallback_command: GolemCliFallbackCommand,
        partial_match: GolemCliCommandPartialMatch,
    },
    Error {
        error: clap::Error,
        fallback_command: GolemCliFallbackCommand,
    },
}

#[derive(Debug)]
pub enum GolemCliCommandPartialMatch {
    AppHelp,
    AppMissingSubcommandHelp,
    ComponentHelp,
    ComponentMissingSubcommandHelp,
    AgentHelp,
    AgentInvokeMissingFunctionName { agent_name: RawAgentId },
    AgentInvokeMissingAgentName,
    ProfileSwitchMissingProfileName,
}

#[derive(Debug, Subcommand)]
pub enum GolemCliSubcommand {
    // App scoped root commands---------------------------------------------------------------------
    /// Create a new application, component, or agent
    New {
        /// Application folder path where the new application should be created, use `.` for the current directory or for an existing application
        application_path: Option<PathBuf>,
        /// Optional application name, defaults to the folder name (if that is a valid application name)
        #[arg(long)]
        application_name: Option<ApplicationName>,
        /// Optional existing or new component name, by default uses an existing component or name the component based on the application name and the used language
        #[arg(long)]
        component_name: Option<ComponentName>,
        /// Optional template names to apply, in non-interactive mode at least one template must be specified
        #[arg(long)]
        template: Vec<AppTemplateName>,
    },
    /// List or search application templates
    Templates {
        /// Optional filter for language or template name
        filter: Option<String>,
    },
    /// Build all or selected components in the application
    Build {
        #[command(flatten)]
        component_name: OptionalComponentNames,
        #[command(flatten)]
        build: BuildArgs,
    },
    /// Generate bridge SDK(s) for the selected agent(s)
    GenerateBridge {
        /// Selects the target language for the generated bridge SDK, defaults to the agent's language
        #[clap(long)]
        language: Option<GuestLanguage>,
        /// Optional filter for component names; can be defined multiple times
        #[clap(long)]
        component_name: Vec<ComponentName>,
        /// Optional filter for agent type names; can be defined multiple times
        #[clap(long)]
        agent_type_name: Vec<AgentTypeName>,
        /// Optional output directory for the generated SDK, when not specified, will use separate
        /// temporary directories in the application's directory
        #[clap(long)]
        output_dir: Option<PathBuf>,
    },
    /// Start REPL for a selected component
    Repl {
        /// Select the language for the REPL, defaults to the component's language
        #[arg(long)]
        language: Option<ReplLanguage>,
        #[command(flatten)]
        component_name: OptionalComponentName,
        /// Optional component revision to use, defaults to latest deployed component revision
        revision: Option<ComponentRevision>,
        #[command(flatten)]
        post_deploy_args: Option<PostDeployArgs>,
        /// Optional script to run, when defined the repl will execute the script and exit
        #[clap(long, short, conflicts_with_all = ["script_file"])]
        script: Option<String>,
        /// Optional script_file to run, when defined the repl will execute the script and exit
        #[clap(long, conflicts_with_all = ["script"])]
        script_file: Option<PathBuf>,
        /// Do not stream logs from the invoked agents. Can be also controlled with the :logs command in the REPL.
        #[clap(long)]
        disable_stream: bool,
        /// Disables automatic importing of Bridge SDK clients
        #[clap(long)]
        disable_auto_imports: bool,
    },
    /// Deploy application
    Deploy {
        /// Only plan deployment, but apply no changes to the staging area or the environment
        #[arg(long, conflicts_with_all = ["stage", "approve_staging_steps"])]
        plan: bool,
        /// Only plan and stage changes, but do not apply them to the environment; used for testing
        #[arg(long, hide=true, conflicts_with_all = ["version", "revision", "plan"])]
        stage: bool,
        /// Ask for approval for every staging step; used for testing
        #[arg(long, hide=true, conflicts_with_all = ["version", "revision", "plan"])]
        approve_staging_steps: bool,
        /// Revert to the specified version
        #[arg(long, conflicts_with_all = ["force_build", "revision", "stage", "approve_staging_steps"])]
        version: Option<String>,
        /// Revert to the specified revision
        #[arg(long, conflicts_with_all = ["force_build", "version", "stage", "approve_staging_steps"])]
        revision: Option<DeploymentRevision>,
        #[command(flatten)]
        force_build: ForceBuildArg,
        #[command(flatten)]
        post_deploy_args: PostDeployArgs,
        /// Internal flag for REPL reload
        #[arg(long, hide = true)]
        repl_bridge_sdk_target: Option<GuestLanguage>,
    },
    /// Clean all components in the application or by selection
    Clean {
        #[command(flatten)]
        component_name: OptionalComponentNames,
    },
    /// Try to automatically update all existing agents of the application to the latest version
    UpdateAgents {
        #[command(flatten)]
        component_name: OptionalComponentNames,
        /// Update mode - auto or manual, defaults to "auto"
        #[arg(long, short, default_value = "auto")]
        update_mode: AgentUpdateMode,
        /// Await the update to be completed
        #[arg(long, default_value_t = false)]
        r#await: bool,
        /// Do not wake up suspended agents, the update will be applied next time the agent wakes up
        #[arg(long, default_value_t = false)]
        disable_wakeup: bool,
    },
    /// Redeploy all agents of the application using the latest version
    RedeployAgents {
        #[command(flatten)]
        component_name: OptionalComponentNames,
    },
    // Other entities ------------------------------------------------------------------------------
    /// Execute custom, application manifest defined commands
    Exec {
        #[clap(subcommand)]
        subcommand: ExecSubcommand,
    },
    /// Manage environments
    Environment {
        #[clap(subcommand)]
        subcommand: EnvironmentSubcommand,
    },
    /// Manage components
    Component {
        #[clap(subcommand)]
        subcommand: ComponentSubcommand,
    },
    /// Invoke and manage agents
    Agent {
        #[clap(subcommand)]
        subcommand: AgentSubcommand,
    },
    /// Manage deployed agent types
    AgentType {
        #[clap(subcommand)]
        subcommand: AgentTypeSubcommand,
    },
    /// Manage API gateway objects
    Api {
        #[clap(subcommand)]
        subcommand: ApiSubcommand,
    },
    /// Manage plugins
    Plugin {
        #[clap(subcommand)]
        subcommand: PluginSubcommand,
    },
    /// Manage global CLI profiles
    Profile {
        #[clap(subcommand)]
        subcommand: ProfileSubcommand,
    },
    /// Run and manage the local Golem server
    #[cfg(feature = "server-commands")]
    Server {
        #[clap(subcommand)]
        subcommand: ServerSubcommand,
    },
    /// Manage Golem Cloud accounts and projects
    Cloud {
        #[clap(subcommand)]
        subcommand: CloudSubcommand,
    },
    /// Manage agent secrets
    AgentSecret {
        #[clap(subcommand)]
        subcommand: AgentSecretSubcommand,
    },
    /// Manage retry policies
    RetryPolicy {
        #[clap(subcommand)]
        subcommand: RetryPolicySubcommand,
    },
    /// Manage quota resource definitions
    Resource {
        #[clap(subcommand)]
        subcommand: ResourceDefinitionSubcommand,
    },
    /// Generate shell completion
    Completion {
        /// Select shell
        shell: clap_complete::Shell,
    },
}

pub mod shared_args {
    use crate::model::GuestLanguage;
    use crate::model::app::AppBuildStep;
    use crate::model::worker::{AgentUpdateMode, RawAgentId};
    use clap::Args;
    use golem_common::model::account::AccountId;
    use golem_common::model::component::ComponentName;

    pub type ComponentTemplateName = String;
    pub type NewAgentArgument = String;
    pub type AgentFunctionArgument = String;
    pub type AgentFunctionName = String;

    #[derive(Debug, Args)]
    pub struct OptionalComponentName {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Optional component name, if not specified, component is selected based on the current directory.
        #[arg(verbatim_doc_comment)]
        pub component_name: Option<ComponentName>,
    }

    #[derive(Debug, Args)]
    pub struct OptionalComponentNames {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Optional component names, if not specified, components are selected based on the current directory.
        #[arg(verbatim_doc_comment)]
        pub component_name: Vec<ComponentName>,
    }

    #[derive(Debug, Args)]
    pub struct LanguageArg {
        #[clap(long, short)]
        pub language: GuestLanguage,
    }

    #[derive(Debug, Args, Clone)]
    pub struct ForceBuildArg {
        /// When set to true will skip modification time based up-to-date checks, defaults to false
        #[clap(long, default_value = "false")]
        pub force_build: bool,
    }

    #[derive(Debug, Args)]
    pub struct BuildArgs {
        /// Select specific build step(s)
        #[clap(long, short, conflicts_with = "skip_check")]
        pub step: Vec<AppBuildStep>,
        /// Skip build-time requirement checks
        #[clap(long, default_value = "false", conflicts_with = "step")]
        pub skip_check: bool,
        #[command(flatten)]
        pub force_build: ForceBuildArg,
        /// Internal flag for REPL reload
        #[arg(long, hide = true)]
        pub repl_bridge_sdk_target: Option<GuestLanguage>,
    }

    #[derive(Debug, Args)]
    pub struct AgentIdArgs {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Agent ID, accepted formats:
        ///   - <AGENT_TYPE>(<AGENT_PARAMETERS>)
        ///   - <ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
        ///   - <APPLICATION>/<ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
        ///   - <ACCOUNT>/<APPLICATION>/<ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
        #[arg(verbatim_doc_comment)]
        pub agent_id: RawAgentId,
    }

    #[derive(Debug, Args)]
    pub struct StreamArgs {
        /// Hide log levels in stream output
        #[clap(long)]
        pub stream_no_log_level: bool,
        /// Hide timestamp in stream output
        #[clap(long)]
        pub stream_no_timestamp: bool,
        /// Only show entries coming from the agent, no output about invocation markers and stream status
        #[clap(long)]
        pub logs_only: bool,
    }

    #[derive(Debug, Args, Clone)]
    pub struct PostDeployArgs {
        /// Update existing agents with auto or manual update mode
        #[clap(long, value_name = "UPDATE_MODE", short, conflicts_with_all = ["redeploy_agents"])]
        pub update_agents: Option<AgentUpdateMode>,
        /// Delete and recreate existing agents
        #[clap(long, conflicts_with_all = ["update_agents"])]
        pub redeploy_agents: bool,
        /// Delete agents and the environment, then deploy
        #[clap(long, short, conflicts_with_all = ["update_agents", "redeploy_agents"])]
        pub reset: bool,
    }

    impl PostDeployArgs {
        pub fn is_any_set(&self, env_args: &PostDeployArgs) -> bool {
            env_args.update_agents.is_some()
                || env_args.redeploy_agents
                || env_args.reset
                || self.update_agents.is_some()
                || self.redeploy_agents
                || self.reset
        }

        pub fn none() -> Self {
            PostDeployArgs {
                update_agents: None,
                redeploy_agents: false,
                reset: false,
            }
        }

        pub fn delete_agents(&self, env_args: &PostDeployArgs) -> bool {
            (env_args.reset || self.reset) && !self.redeploy_agents && self.update_agents.is_none()
        }

        pub fn redeploy_agents(&self, env_args: &PostDeployArgs) -> bool {
            (env_args.redeploy_agents || self.redeploy_agents)
                && !self.reset
                && self.update_agents.is_none()
        }
    }

    #[derive(Debug, Args)]
    pub struct AccountIdOptionalArg {
        /// Account ID
        #[arg(long)]
        pub account_id: Option<AccountId>,
    }
}

pub mod exec {
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum ExecSubcommand {
        /// Execute custom, application manifest specified command
        #[clap(external_subcommand)]
        CustomCommand(Vec<String>),
    }
}

pub mod environment {
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum EnvironmentSubcommand {
        /// Check and optionally update environment deployment options
        SyncDeploymentOptions,
        /// List application environments on the current server
        List,
    }
}

pub mod component {
    use crate::command::shared_args::{OptionalComponentName, OptionalComponentNames};
    use crate::model::worker::AgentUpdateMode;
    use clap::Subcommand;
    use golem_common::model::component::ComponentRevision;

    #[derive(Debug, Subcommand)]
    pub enum ComponentSubcommand {
        /// List deployed component versions' metadata
        List,
        /// Get the latest or selected revision of deployed component metadata
        Get {
            #[command(flatten)]
            component_name: OptionalComponentName,
            /// Optional component revision to get
            revision: Option<ComponentRevision>,
        },
        /// Try to automatically update all existing agents of the selected component to the latest version
        UpdateAgents {
            #[command(flatten)]
            component_name: OptionalComponentName,
            /// Agent update mode - auto or manual, defaults to "auto"
            #[arg(long, short, default_value_t = AgentUpdateMode::Automatic)]
            update_mode: AgentUpdateMode,
            /// Await the update to be completed
            #[arg(long, default_value_t = false)]
            r#await: bool,
            /// Do not wake up suspended agents, the update will be applied next time the agent wakes up
            #[arg(long, default_value_t = false)]
            disable_wakeup: bool,
        },
        /// Redeploy all agents of the selected component using the latest version
        RedeployAgents {
            #[command(flatten)]
            component_name: OptionalComponentName,
        },
        /// Show component manifest properties with source trace
        ManifestTrace {
            #[command(flatten)]
            component_name: OptionalComponentNames,
        },
    }

    pub mod plugin {
        use crate::args::parse_key_val;
        use crate::command::shared_args::OptionalComponentName;
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ComponentPluginSubcommand {
            /// Install a plugin for selected component
            Install {
                #[command(flatten)]
                component_name: OptionalComponentName,
                /// The plugin to install
                #[arg(long)]
                plugin_name: String,
                /// The version of the plugin to install
                #[arg(long)]
                plugin_version: String,
                /// Priority of the plugin - largest priority is applied first
                #[arg(long)]
                priority: i32,
                /// List of parameters (key-value pairs) passed to the plugin
                #[arg(long, value_parser = parse_key_val, value_name = "KEY=VAL")]
                param: Vec<(String, String)>,
            },
            /// Get the installed plugins of the component
            Get {
                #[command(flatten)]
                component_name: OptionalComponentName,
                /// The revision of the component
                revision: Option<u64>,
            },
            /// Update component plugin
            Update {
                /// The component to update the plugin for
                #[command(flatten)]
                component_name: OptionalComponentName,
                /// Priority of the plugin to update
                #[arg(long)]
                plugin_to_update: i32,
                /// Updated priority of the plugin - largest priority is applied first
                #[arg(long)]
                priority: i32,
                /// Updated list of parameters (key-value pairs) passed to the plugin
                #[arg(long, value_parser = parse_key_val, value_name = "KEY=VAL")]
                param: Vec<(String, String)>,
            },
            /// Uninstall a plugin for selected component
            Uninstall {
                /// The component to uninstall the plugin from
                #[command(flatten)]
                component_name: OptionalComponentName,
                /// Priority of the plugin to update
                #[arg(long)]
                plugin_to_update: i32,
            },
        }
    }
}

pub mod worker {
    use crate::args::parse_agent_config;
    use crate::args::parse_cursor;
    use crate::args::parse_key_val;
    use crate::command::shared_args::{
        AgentFunctionArgument, AgentFunctionName, AgentIdArgs, PostDeployArgs, StreamArgs,
    };
    use crate::model::worker::AgentUpdateMode;
    use chrono::{DateTime, Utc};
    use clap::Subcommand;
    use golem_client::model::ScanCursor;
    use golem_common::model::IdempotencyKey;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::component::{ComponentName, ComponentRevision};
    use golem_common::model::worker::AgentConfigEntryDto;
    use uuid::Uuid;

    #[derive(Debug, Subcommand)]
    pub enum AgentSubcommand {
        /// Create new agent
        New {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Environment variables visible for the agent
            #[arg(short, long, value_parser = parse_key_val, value_name = "ENV=VAL")]
            env: Vec<(String, String)>,
            /// Configuration to be provided to the agent.
            /// This parameter can be provided multiple times in the form --config ${DOT_SEPARATED_CONFIG_PATH}=${CONFIG_VALUE}.
            /// Only configuration declared by the agent can be provided. If a given config path is not provided, the default from the manifest
            /// (agents.*.config) is used. If neither value nor default is provided and the config is non-optional, creation
            /// of the agent will fail.
            #[arg(short, long, value_parser = parse_agent_config, verbatim_doc_comment)]
            config: Vec<AgentConfigEntryDto>,
        },
        // TODO: json args
        /// Invoke (or enqueue invocation for) agent
        Invoke {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Agent function name to invoke
            function_name: AgentFunctionName,
            /// Agent function arguments specified using the agent's language's syntax
            arguments: Vec<AgentFunctionArgument>,
            /// Only trigger invocation and do not wait for it
            #[clap(long, short)]
            trigger: bool,
            /// Set idempotency key for the call, use "-" for an auto-generated key
            #[clap(long, short)]
            idempotency_key: Option<IdempotencyKey>,
            #[clap(long, short)]
            /// Disable live streaming the agent's standard output, error, and log channels
            no_stream: bool,
            #[command(flatten)]
            stream_args: StreamArgs,
            #[command(flatten)]
            post_deploy_args: Option<PostDeployArgs>,
            /// Schedule the invocation at a specific time (ISO 8601 / RFC 3339 format, e.g. 2026-03-15T10:30:00Z)
            #[clap(long, requires = "trigger")]
            schedule_at: Option<DateTime<Utc>>,
        },
        /// Get agent metadata
        Get {
            #[command(flatten)]
            agent_id: AgentIdArgs,
        },
        /// Delete an agent
        Delete {
            #[command(flatten)]
            agent_id: AgentIdArgs,
        },
        /// List agents
        List {
            /// Optional filter for a specific agent type
            #[arg(conflicts_with = "component_name")]
            agent_type_name: Option<AgentTypeName>,

            /// Optional filter for a specific component
            #[arg(long, conflicts_with = "agent_type_name")]
            component_name: Option<ComponentName>,

            /// Filter for agent metadata in form of `property op value`.
            ///
            /// Filter examples: `name = my-agent(1, 2, 3)`, `version >= 0`, `status = Running`, `env.var1 = value`.
            /// Can be used multiple times (AND condition is applied between them)
            #[arg(long)]
            filter: Vec<String>,
            /// Cursor position, if not provided, starts from the beginning.
            ///
            /// Cursor can be used to get the next page of results, use the cursor returned
            /// in the previous response.
            /// The cursor has the format 'layer/position' where both layer and position are numbers.
            #[arg(long, short, value_parser = parse_cursor)]
            scan_cursor: Option<ScanCursor>,
            /// The maximum number of returned agents; returns all values if not specified.
            /// When multiple components are selected, the limit is applied separately.
            #[arg(long, short)]
            max_count: Option<u64>,
            /// When set to true it queries for most up-to-date status for each agent, default is false
            #[arg(long, default_value_t = false)]
            precise: bool,
            /// Watch mode: periodically clear the screen and redisplay the agent list.
            /// Pass without a value to use the default interval (400ms), or --refresh=MILLIS
            /// to set a custom polling interval. Conflicts with --scan-cursor.
            #[arg(long, default_missing_value = "400", value_name = "MILLIS", num_args = 0..=1, conflicts_with = "scan_cursor")]
            refresh: Option<u64>,
        },
        /// Connect to an agent and live stream its standard output, error and log channels
        Stream {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            #[command(flatten)]
            stream_args: StreamArgs,
        },
        /// Like stream, but for helping Bridge SDK-based REPLs
        #[clap(hide = true)]
        ReplStream {
            /// Agent type name
            agent_type_name: AgentTypeName,
            /// Agent parameters in UntypedDataValue JSON format
            parameters: String,
            /// Idempotency key, used for filtering
            idempotency_key: IdempotencyKey,
            /// Phantom ID
            phantom_id: Option<Uuid>,
            #[command(flatten)]
            stream_args: StreamArgs,
        },
        /// Updates an agent
        Update {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Update mode - auto or manual (default is auto)
            mode: Option<AgentUpdateMode>,
            /// The new revision of the updated agent (default is the latest revision)
            target_revision: Option<ComponentRevision>,
            /// Await the update to be completed
            #[arg(long, default_value_t = false)]
            r#await: bool,
            /// Do not wake up suspended agents, the update will be applied next time the agent wakes up
            #[arg(long, default_value_t = false)]
            disable_wakeup: bool,
        },
        /// Interrupts a running agent
        Interrupt {
            #[command(flatten)]
            agent_id: AgentIdArgs,
        },
        /// Resume an interrupted agent
        Resume {
            #[command(flatten)]
            agent_id: AgentIdArgs,
        },
        /// Simulates a crash on an agent for testing purposes.
        ///
        /// The agent starts recovering and resuming immediately.
        SimulateCrash {
            #[command(flatten)]
            agent_id: AgentIdArgs,
        },
        /// Queries and dumps an agent's full oplog
        Oplog {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Index of the first oplog entry to get. If missing, the whole oplog is returned
            #[arg(long, conflicts_with = "query")]
            from: Option<u64>,
            /// Lucene query to look for oplog entries. If missing, the whole oplog is returned
            #[arg(long, conflicts_with = "from")]
            query: Option<String>,
        },
        /// Reverts an agent by undoing its last recorded operations
        Revert {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Revert by oplog index
            #[arg(long, conflicts_with = "number_of_invocations")]
            last_oplog_index: Option<u64>,
            /// Revert by number of invocations
            #[arg(long, conflicts_with = "last_oplog_index")]
            number_of_invocations: Option<u64>,
        },
        /// Cancels an enqueued invocation if it has not started yet
        CancelInvocation {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Idempotency key of the invocation to be cancelled
            idempotency_key: IdempotencyKey,
        },
        /// List files in an agent's directory
        Files {
            #[command(flatten)]
            agent_name: AgentIdArgs,
            /// Path to the directory to list files from
            #[arg(default_value = "/")]
            path: String,
        },
        /// Get contents of a file in an agent
        FileContents {
            #[command(flatten)]
            agent_name: AgentIdArgs,
            /// Path to the file to get contents from
            path: String,
            /// Local path (including filename) to save the file contents. Optional.
            #[arg(long)]
            output: Option<String>,
        },
        /// Activate a plugin for a specific agent instance.
        ///
        /// The plugin must be one of the installed plugins for the agent's current component version.
        /// Use `golem component plugin get` to list installed plugins with their names and priorities.
        ActivatePlugin {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Name of the plugin to activate
            #[arg(long)]
            plugin_name: String,
            /// Priority of the plugin installation to activate.
            /// Only required when multiple installations of the same plugin exist.
            #[arg(long)]
            plugin_priority: Option<i32>,
        },
        /// Deactivate a plugin for a specific agent instance.
        ///
        /// The plugin must be one of the installed plugins for the agent's current component version.
        /// Use `golem component plugin get` to list installed plugins with their names and priorities.
        DeactivatePlugin {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Name of the plugin to deactivate
            #[arg(long)]
            plugin_name: String,
            /// Priority of the plugin installation to deactivate.
            /// Only required when multiple installations of the same plugin exist.
            #[arg(long)]
            plugin_priority: Option<i32>,
        },
    }
}

pub mod agent_type {
    use clap::Subcommand;
    use golem_common::model::agent::AgentTypeName;

    #[derive(Debug, Subcommand)]
    pub enum AgentTypeSubcommand {
        /// List all deployed agent types
        List,
        /// Get deployed agent type metadata
        Get {
            /// Agent type name
            agent_type_name: AgentTypeName,
        },
    }
}

pub mod api {
    use crate::command::api::deployment::ApiDeploymentSubcommand;
    use crate::command::api::domain::ApiDomainSubcommand;
    use crate::command::api::security_scheme::ApiSecuritySchemeSubcommand;
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum ApiSubcommand {
        /// Manage API deployments
        Deployment {
            #[clap(subcommand)]
            subcommand: ApiDeploymentSubcommand,
        },
        /// Manage API Security Schemes
        SecurityScheme {
            #[clap(subcommand)]
            subcommand: ApiSecuritySchemeSubcommand,
        },
        /// Manage API Domains
        Domain {
            #[clap(subcommand)]
            subcommand: ApiDomainSubcommand,
        },
    }

    pub mod deployment {
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiDeploymentSubcommand {
            /// Get API deployment
            Get {
                /// Deployment domain
                domain: String,
            },
            /// List API deployment for API definition
            List,
        }
    }

    pub mod agent_secret {
        use crate::args::parse_agent_secret_path;
        use clap::Subcommand;
        use golem_common::model::agent_secret::{AgentSecretId, AgentSecretPath};

        #[derive(Debug, Subcommand)]
        pub enum AgentSecretSubcommand {
            /// Create Agent Secret in the environment
            Create {
                /// Path of the secret (dot-separated, e.g. "apiKey" or "db.password"). Casing is normalized during creation.
                #[arg(value_parser = parse_agent_secret_path)]
                path: AgentSecretPath,
                /// Type of the secret, using the project's language syntax (e.g. "String" for Rust, "string" for TypeScript) or JSON format
                #[arg(long)]
                secret_type: String,
                /// Value of the secret (e.g. "my-key" for strings, 42 for numbers). Uses the project's language syntax or JSON
                #[arg(long)]
                secret_value: Option<String>,
            },

            /// Get Agent Secret by path or ID
            Get {
                /// Path of the secret (dot-separated)
                #[arg(value_parser = parse_agent_secret_path, required_unless_present = "id", conflicts_with = "id")]
                path: Option<AgentSecretPath>,
                /// ID of the secret (alternative to path)
                #[arg(long, required_unless_present = "path", conflicts_with = "path")]
                id: Option<AgentSecretId>,
            },

            /// Update Agent Secret value
            UpdateValue {
                /// Path of the secret (dot-separated)
                #[arg(value_parser = parse_agent_secret_path, required_unless_present = "id", conflicts_with = "id")]
                path: Option<AgentSecretPath>,
                /// ID of the secret (alternative to path)
                #[arg(long, required_unless_present = "path", conflicts_with = "path")]
                id: Option<AgentSecretId>,
                /// Value of the secret (e.g. "my-key" for strings, 42 for numbers). Uses the project's language syntax or JSON
                #[arg(long)]
                secret_value: Option<String>,
            },

            /// Delete Agent Secret
            Delete {
                /// Path of the secret (dot-separated)
                #[arg(value_parser = parse_agent_secret_path, required_unless_present = "id", conflicts_with = "id")]
                path: Option<AgentSecretPath>,
                /// ID of the secret (alternative to path)
                #[arg(long, required_unless_present = "path", conflicts_with = "path")]
                id: Option<AgentSecretId>,
            },

            /// List Agent Secrets
            List {
                /// Include environment ID and secret ID columns in text output
                #[arg(long)]
                ids: bool,
            },
        }
    }

    pub mod security_scheme {
        use clap::Subcommand;
        use golem_common::model::security_scheme::{ProviderKind, SecuritySchemeName};

        #[derive(Debug, Subcommand)]
        pub enum ApiSecuritySchemeSubcommand {
            /// Create HTTP API Security Scheme
            Create {
                /// Security Scheme name
                security_scheme_name: SecuritySchemeName,
                /// Security Scheme provider (Google, Facebook, Gitlab, Microsoft, Custom)
                #[arg(long)]
                provider_type: ProviderKind,
                /// Custom provider display name (required when provider_type is custom)
                #[arg(long, required_if_eq("provider_type", "custom"))]
                custom_provider_name: Option<String>,
                /// Custom provider OIDC issuer URL (required when provider_type is custom)
                #[arg(long, required_if_eq("provider_type", "custom"))]
                custom_issuer_url: Option<String>,
                /// Security Scheme client ID
                #[arg(long)]
                client_id: String,
                /// Security Scheme client secret
                #[arg(long)]
                client_secret: String,
                #[arg(long)]
                /// Security Scheme Scopes, can be defined multiple times
                scope: Vec<String>,
                #[arg(long)]
                /// Security Scheme redirect URL
                redirect_url: String,
            },

            /// Get HTTP API Security Scheme
            Get {
                /// Security Scheme name
                security_scheme_name: SecuritySchemeName,
            },

            /// Update HTTP API Security Scheme
            Update {
                /// Security Scheme name
                security_scheme_name: SecuritySchemeName,
                /// Security Scheme provider (Google, Facebook, Gitlab, Microsoft, Custom)
                #[arg(long)]
                provider_type: Option<ProviderKind>,
                /// Custom provider display name (required when provider_type is custom)
                #[arg(long, required_if_eq("provider_type", "custom"))]
                custom_provider_name: Option<String>,
                /// Custom provider OIDC issuer URL (required when provider_type is custom)
                #[arg(long, required_if_eq("provider_type", "custom"))]
                custom_issuer_url: Option<String>,
                /// Security Scheme client ID
                #[arg(long)]
                client_id: Option<String>,
                /// Security Scheme client secret
                #[arg(long)]
                client_secret: Option<String>,
                /// Security Scheme Scopes (replaces existing scopes), can be defined multiple times
                #[arg(long)]
                scope: Option<Vec<String>>,
                /// Security Scheme redirect URL
                #[arg(long)]
                redirect_url: Option<String>,
            },

            /// Delete HTTP API Security Scheme
            Delete {
                /// Security Scheme name
                security_scheme_name: SecuritySchemeName,
            },

            /// List HTTP API Security Schemes
            List,
        }
    }

    pub mod domain {
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiDomainSubcommand {
            /// List domains
            List,
            /// Register a new domain
            Register {
                /// Domain name
                domain: String,
            },
            /// Delete an existing domain
            Delete {
                /// Domain name
                domain: String,
            },
        }
    }
}

pub mod resource_definition {
    use crate::model::EnforcementActionArg;
    use clap::Subcommand;
    use golem_common::model::quota::ResourceDefinitionId;

    #[derive(Debug, Subcommand)]
    pub enum ResourceDefinitionSubcommand {
        /// Create a quota resource definition in the environment
        Create {
            /// Name of the resource (unique within the environment)
            name: String,
            /// Resource limit as JSON: one of
            ///   {"type":"rate","value":N,"period":"second|minute|hour|day|month|year","max":N}
            ///   {"type":"capacity","value":N}
            ///   {"type":"concurrency","value":N}
            #[arg(long)]
            limit: String,
            /// Enforcement action when the limit is exceeded: throttle | reject | terminate
            #[arg(long, default_value_t = EnforcementActionArg::Throttle)]
            enforcement_action: EnforcementActionArg,
            /// Singular unit label (e.g. "token")
            #[arg(long, default_value = "unit")]
            unit: String,
            /// Plural unit label (e.g. "tokens")
            #[arg(long, default_value = "units")]
            units: String,
        },

        /// Update an existing quota resource definition
        Update {
            /// Name of the resource definition
            #[arg(required_unless_present = "id", conflicts_with = "id")]
            name: Option<String>,
            /// ID of the resource definition (alternative to name)
            #[arg(long, required_unless_present = "name", conflicts_with = "name")]
            id: Option<ResourceDefinitionId>,
            /// New resource limit as JSON (optional)
            #[arg(long)]
            limit: Option<String>,
            /// New enforcement action (optional): throttle | reject | terminate
            #[arg(long)]
            enforcement_action: Option<EnforcementActionArg>,
            /// New singular unit label (optional)
            #[arg(long)]
            unit: Option<String>,
            /// New plural unit label (optional)
            #[arg(long)]
            units: Option<String>,
        },

        /// Delete a quota resource definition
        Delete {
            /// Name of the resource definition
            #[arg(required_unless_present = "id", conflicts_with = "id")]
            name: Option<String>,
            /// ID of the resource definition (alternative to name)
            #[arg(long, required_unless_present = "name", conflicts_with = "name")]
            id: Option<ResourceDefinitionId>,
        },

        /// Get a quota resource definition by name or ID
        Get {
            /// Name of the resource definition
            #[arg(required_unless_present = "id", conflicts_with = "id")]
            name: Option<String>,
            /// ID of the resource definition (alternative to name)
            #[arg(long, required_unless_present = "name", conflicts_with = "name")]
            id: Option<ResourceDefinitionId>,
        },

        /// List quota resource definitions in the environment
        List,
    }
}

pub mod retry_policy {
    use clap::Subcommand;
    use golem_common::model::retry_policy::RetryPolicyId;

    #[derive(Debug, Subcommand)]
    pub enum RetryPolicySubcommand {
        /// Create a retry policy in the environment
        Create {
            /// Name of the retry policy
            name: String,
            /// Priority (higher = checked first)
            #[arg(long)]
            priority: u32,
            /// Predicate as JSON or YAML
            #[arg(long)]
            predicate: String,
            /// Policy as JSON or YAML
            #[arg(long)]
            policy: String,
        },

        /// List retry policies in the environment
        List,

        /// Get a retry policy by name or ID
        Get {
            /// Name of the retry policy
            #[arg(required_unless_present = "id", conflicts_with = "id")]
            name: Option<String>,
            /// ID of the retry policy (alternative to name)
            #[arg(long, required_unless_present = "name", conflicts_with = "name")]
            id: Option<RetryPolicyId>,
        },

        /// Update a retry policy
        Update {
            /// Name of the retry policy
            #[arg(required_unless_present = "id", conflicts_with = "id")]
            name: Option<String>,
            /// ID of the retry policy (alternative to name)
            #[arg(long, required_unless_present = "name", conflicts_with = "name")]
            id: Option<RetryPolicyId>,
            /// New priority (optional)
            #[arg(long)]
            priority: Option<u32>,
            /// New predicate as JSON or YAML (optional)
            #[arg(long)]
            predicate: Option<String>,
            /// New policy as JSON or YAML (optional)
            #[arg(long)]
            policy: Option<String>,
        },

        /// Delete a retry policy
        Delete {
            /// Name of the retry policy
            #[arg(required_unless_present = "id", conflicts_with = "id")]
            name: Option<String>,
            /// ID of the retry policy (alternative to name)
            #[arg(long, required_unless_present = "name", conflicts_with = "name")]
            id: Option<RetryPolicyId>,
        },
    }
}

pub mod plugin {
    use crate::model::PathBufOrStdin;
    use clap::Subcommand;
    use uuid::Uuid;

    #[derive(Debug, Subcommand)]
    pub enum PluginSubcommand {
        /// List account plugins
        List,
        /// Get plugin details
        Get {
            /// Plugin ID
            plugin_id: Uuid, // TODO: atomic: missing method for looking up by name
        },
        /// Register a new plugin for the account
        Register {
            /// Path to the plugin manifest JSON or '-' to use STDIN
            manifest: PathBufOrStdin,
        },
        /// Unregister a plugin
        Unregister {
            /// Plugin ID
            plugin_id: Uuid, // TODO: atomic: missing method for deleting by name
        },
    }
}

pub mod profile {
    use crate::command::profile::config::ProfileConfigSubcommand;
    use crate::config::ProfileName;
    use crate::model::format::Format;
    use clap::Subcommand;
    use url::Url;

    #[allow(clippy::large_enum_variant)]
    #[derive(Debug, Subcommand)]
    pub enum ProfileSubcommand {
        /// Create a new global profile, call without <PROFILE_NAME> for interactive setup
        New {
            /// Name of the newly created profile
            name: Option<ProfileName>,
            /// Switch to the profile after creation
            #[arg(long)]
            set_active: bool,
            /// URL of Golem Component service
            #[arg(long)]
            url: Option<Url>,
            /// URL of Golem Worker service, if not provided defaults to component-url
            #[arg(long)]
            worker_url: Option<Url>,
            /// URL of Golem Cloud service, if not provided defaults to component-url
            #[arg(long, default_value_t = Format::Text)]
            default_format: Format,
            /// Token to use for authenticating against Golem. If not provided an OAuth2 flow will be performed when authentication is needed for the first time.
            #[arg(long)]
            static_token: Option<String>,
            /// Accept invalid certificates.
            ///
            /// Disables certificate validation.
            /// Warning! Any certificate will be trusted for use.
            /// This includes expired certificates.
            /// This introduces significant vulnerabilities, and should only be used as a last resort.
            #[arg(long, hide = true)]
            allow_insecure: bool,
        },
        /// List global profiles
        List,
        /// Set the active global default profile
        Switch {
            /// Profile name to switch to
            profile_name: ProfileName,
        },
        /// Show global profile details
        Get {
            /// Name of profile to show, shows active profile if not specified.
            profile_name: Option<ProfileName>,
        },
        /// Remove global profile
        Delete {
            /// Profile name to delete
            profile_name: ProfileName,
        },
        /// Configure global profile
        Config {
            /// Profile name
            profile_name: ProfileName,
            #[command(subcommand)]
            subcommand: ProfileConfigSubcommand,
        },
    }

    pub mod config {
        use crate::model::format::Format;
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ProfileConfigSubcommand {
            /// Set default output format for the requested profile
            SetFormat {
                /// CLI output format
                format: Format,
            },
        }
    }
}

pub mod cloud {
    use crate::command::cloud::account::AccountSubcommand;
    use crate::command::cloud::token::TokenSubcommand;
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum CloudSubcommand {
        /// Manage Cloud Account
        Account {
            #[clap(subcommand)]
            subcommand: AccountSubcommand,
        },
        /// Manage Cloud Tokens
        Token {
            #[clap(subcommand)]
            subcommand: TokenSubcommand,
        },
    }

    pub mod token {
        use crate::args::parse_instant;
        use chrono::{DateTime, Utc};
        use clap::Subcommand;
        use golem_common::model::auth::TokenId;

        #[derive(Debug, Subcommand)]
        pub enum TokenSubcommand {
            /// List tokens
            List,
            /// Create new token
            New {
                /// Expiration date of the generated token
                #[arg(long, value_parser = parse_instant, default_value = "2100-01-01T00:00:00Z")]
                expires_at: DateTime<Utc>,
            },
            /// Delete an existing token
            Delete {
                /// Token ID
                token_id: TokenId,
            },
        }
    }

    pub mod account {
        use crate::command::shared_args::AccountIdOptionalArg;
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum AccountSubcommand {
            /// Get information about the account
            Get {
                #[command(flatten)]
                account_id: AccountIdOptionalArg,
            },
            /// Update some information about the account
            Update {
                #[command(flatten)]
                account_id: AccountIdOptionalArg,
                /// Set the account's name
                account_name: Option<String>,
                /// Set the account's email address
                account_email: Option<String>,
            },
            /// Add a new account
            New {
                /// The new account's name
                account_name: String,
                /// The new account's email address
                account_email: String,
            },
            /// Delete the account
            Delete {
                #[command(flatten)]
                account_id: AccountIdOptionalArg,
            },
        }
    }
}

pub mod server {
    use clap::{Args, Subcommand};
    use std::path::PathBuf;

    #[derive(Debug, Args, Default)]
    pub struct RunArgs {
        /// Address to serve the main API on, defaults to 0.0.0.0
        #[clap(long)]
        pub router_addr: Option<String>,

        /// Port to serve the main API on, defaults to 9881
        #[clap(long)]
        pub router_port: Option<u16>,

        /// Port to serve custom requests on, defaults to 9006
        #[clap(long)]
        pub custom_request_port: Option<u16>,

        /// Port to serve MCP server on, defaults to 9007
        #[clap(long)]
        pub mcp_port: Option<u16>,

        /// Write discovered startup ports to this JSON file
        #[clap(long)]
        pub ports_file: Option<PathBuf>,

        /// Directory to store data in. Defaults to $XDG_STATE_HOME/golem
        #[clap(long)]
        pub data_dir: Option<PathBuf>,

        /// Clean the data directory before starting
        #[clap(long)]
        pub clean: bool,

        /// Use deterministic agent filesystem directories rooted at the given
        /// path instead of random temp directories. The directory layout is:
        ///   <root>/<environment_id>/<component_id>/<agent_name>/
        #[clap(long)]
        pub agent_filesystem_root: Option<PathBuf>,
    }

    impl RunArgs {
        pub fn router_addr(&self) -> &str {
            self.router_addr.as_deref().unwrap_or("0.0.0.0")
        }

        pub fn router_port(&self) -> u16 {
            self.router_port.unwrap_or(9881)
        }

        pub fn custom_request_port(&self) -> u16 {
            self.custom_request_port.unwrap_or(9006)
        }
        pub fn mcp_port(&self) -> u16 {
            self.mcp_port.unwrap_or(9007)
        }
    }

    #[derive(Debug, Subcommand)]
    pub enum ServerSubcommand {
        /// Run golem server for local development
        Run {
            #[clap(flatten)]
            args: RunArgs,
        },
        /// Clean the local server data directory
        Clean,
    }
}

pub fn builtin_exec_subcommands() -> BTreeSet<String> {
    GolemCliCommand::command()
        .find_subcommand("exec")
        .unwrap()
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_string())
        .collect()
}

fn help_target_to_subcommand_names(target: ShowClapHelpTarget) -> Vec<&'static str> {
    match target {
        ShowClapHelpTarget::AppNew => vec!["new"],
    }
}

pub fn help_target_to_command(target: ShowClapHelpTarget) -> Command {
    let command = GolemCliCommand::command();
    let mut command = &command;

    for subcommand in help_target_to_subcommand_names(target) {
        command = command.find_subcommand(subcommand).unwrap();
    }

    command.clone()
}

#[cfg(test)]
mod test {

    use crate::command::{
        GolemCliCommand, builtin_exec_subcommands, help_target_to_subcommand_names,
    };
    use crate::error::ShowClapHelpTarget;
    use clap::builder::StyledStr;
    use clap::{Command, CommandFactory};
    use itertools::Itertools;
    use std::collections::{BTreeMap, BTreeSet};
    use strum::IntoEnumIterator;
    use test_r::test;

    #[test]
    fn command_debug_assert() {
        GolemCliCommand::command().debug_assert();
    }

    #[test]
    fn all_commands_and_args_has_doc() {
        fn collect_docs(
            path: &mut Vec<String>,
            doc_by_cmd_path: &mut BTreeMap<String, Option<StyledStr>>,
            command: &Command,
        ) {
            path.push(command.get_name().to_string());
            let key = path.iter().join(" ");

            doc_by_cmd_path.insert(key.clone(), command.get_about().cloned());

            for arg in command.get_arguments() {
                doc_by_cmd_path.insert(
                    if arg.is_positional() {
                        format!("{} |{}|", key, arg.get_id().to_string().to_uppercase())
                    } else {
                        format!("{} --{}", key, arg.get_id())
                    },
                    arg.get_help().cloned(),
                );
            }

            for subcommand in command.get_subcommands() {
                collect_docs(path, doc_by_cmd_path, subcommand);
            }

            path.pop();
        }

        let mut path = vec![];
        let mut doc_by_cmd_path = BTreeMap::new();
        collect_docs(&mut path, &mut doc_by_cmd_path, &GolemCliCommand::command());

        let elems_without_about = doc_by_cmd_path
            .into_iter()
            .filter_map(|(path, about)| about.is_none().then_some(path))
            .collect::<Vec<_>>();

        assert!(
            elems_without_about.is_empty(),
            "\n{}",
            elems_without_about.join("\n")
        );
    }

    #[test]
    fn invalid_arg_matchers_are_using_valid_commands_and_args_names() {
        fn collect_positional_args(
            path: &mut Vec<String>,
            positional_args_by_cmd: &mut BTreeMap<String, BTreeSet<String>>,
            command: &Command,
        ) {
            path.push(command.get_name().to_string());
            let key = path.iter().join(" ");

            positional_args_by_cmd.insert(
                key,
                command
                    .get_arguments()
                    .filter(|arg| arg.is_positional())
                    .map(|arg| arg.get_id().to_string())
                    .collect(),
            );

            for subcommand in command.get_subcommands() {
                collect_positional_args(path, positional_args_by_cmd, subcommand);
            }

            path.pop();
        }

        let mut path = vec![];
        let mut positional_args_by_cmd = BTreeMap::new();

        collect_positional_args(
            &mut path,
            &mut positional_args_by_cmd,
            &GolemCliCommand::command(),
        );

        let bad_matchers = GolemCliCommand::invalid_arg_matchers()
            .into_iter()
            .filter_map(|matcher| {
                let cmd_path = format!("golem-cli {}", matcher.subcommands.iter().join(" "));

                let Some(args) = positional_args_by_cmd.get(&cmd_path) else {
                    return Some(("command not found".to_string(), matcher));
                };

                let missing_arg = [matcher.missing_positional_arg];

                let bad_args = matcher
                    .found_positional_args
                    .iter()
                    .chain(&missing_arg)
                    .filter(|&&arg| !args.contains(arg))
                    .collect::<Vec<_>>();

                if !bad_args.is_empty() {
                    return Some((
                        format!("args not found: {}", bad_args.into_iter().join(", ")),
                        matcher,
                    ));
                }

                None
            })
            .collect::<Vec<_>>();

        assert!(
            bad_matchers.is_empty(),
            "\n{}",
            bad_matchers
                .into_iter()
                .map(|(error, matcher)| format!("error: {error}\nmatcher: {matcher:?}\n"))
                .join("\n")
        )
    }

    #[test]
    fn no_overlapping_flags() {
        fn collect_flags(
            path: &mut Vec<String>,
            flags_by_cmd_path: &mut BTreeMap<String, Vec<String>>,
            global_flags: &mut Vec<String>,
            command: &Command,
        ) {
            path.push(command.get_name().to_string());
            let key = path.iter().join(" ");

            let mut cmd_flag_names = Vec::<String>::new();
            for arg in command.get_arguments() {
                let mut arg_flag_names = Vec::<String>::new();
                if arg.is_positional() {
                    continue;
                }

                arg_flag_names.extend(
                    arg.get_long_and_visible_aliases()
                        .into_iter()
                        .flatten()
                        .map(|s| s.to_string()),
                );
                arg_flag_names.extend(
                    arg.get_short_and_visible_aliases()
                        .into_iter()
                        .flatten()
                        .map(|s| s.to_string()),
                );

                if arg.is_global_set() {
                    global_flags.extend(arg_flag_names);
                } else {
                    cmd_flag_names.extend(arg_flag_names);
                }
            }

            flags_by_cmd_path.insert(key, cmd_flag_names);

            for subcommand in command.get_subcommands() {
                collect_flags(path, flags_by_cmd_path, global_flags, subcommand);
            }

            path.pop();
        }

        let mut path = vec![];
        let mut flags_by_cmd_path = BTreeMap::<String, Vec<String>>::new();
        let mut global_flags = Vec::<String>::new();
        collect_flags(
            &mut path,
            &mut flags_by_cmd_path,
            &mut global_flags,
            &GolemCliCommand::command(),
        );

        let commands_with_conflicting_flags = flags_by_cmd_path
            .into_iter()
            .map(|(path, flags)| {
                (
                    path,
                    flags
                        .into_iter()
                        .chain(global_flags.iter().cloned())
                        .counts()
                        .into_iter()
                        .filter(|(_, count)| *count > 1)
                        .collect::<Vec<_>>(),
                )
            })
            .filter(|(_, flags)| !flags.is_empty())
            .collect::<Vec<_>>();

        assert!(
            commands_with_conflicting_flags.is_empty(),
            "\n{}",
            commands_with_conflicting_flags
                .iter()
                .map(|e| format!("{e:?}"))
                .join("\n")
        );
    }

    #[test]
    fn builtin_app_subcommands_no_panic() {
        println!("{:?}", builtin_exec_subcommands())
    }

    #[test]
    fn help_targets_to_subcommands_uses_valid_subcommands() {
        for target in ShowClapHelpTarget::iter() {
            let command = GolemCliCommand::command();
            let mut command = &command;
            let subcommands = help_target_to_subcommand_names(target);
            for subcommand in &subcommands {
                match command.find_subcommand(subcommand) {
                    Some(subcommand) => command = subcommand,
                    None => {
                        panic!("Invalid help target: {target}, {subcommands:?}, {subcommand}");
                    }
                }
            }
        }
    }
}
