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

use crate::command::api::ApiSubcommand;
use crate::command::app::AppSubcommand;
use crate::command::cloud::CloudSubcommand;
use crate::command::component::ComponentSubcommand;
use crate::command::plugin::PluginSubcommand;
use crate::command::profile::ProfileSubcommand;
use crate::command::worker::AgentSubcommand;
use crate::config::{BuildProfileName, ProfileName};
use crate::log::LogColorize;
use crate::model::{Format, WorkerName};
use crate::{command_name, version};
use anyhow::{anyhow, bail, Context as AnyhowContext};
use chrono::{DateTime, Utc};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{self, Command, CommandFactory, Subcommand};
use clap::{Args, Parser};
use clap_verbosity_flag::{ErrorLevel, LogLevel};
use golem_client::model::ScanCursor;
use lenient_bool::LenientBool;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::PathBuf;
use uuid::Uuid;

#[cfg(feature = "server-commands")]
use crate::command::server::ServerSubcommand;
use crate::command::shared_args::{ComponentOptionalComponentName, DeployArgs};
use crate::error::ShowClapHelpTarget;

/// Golem Command Line Interface
#[derive(Debug, Parser)]
#[command(bin_name = command_name(), display_name = command_name(), long_version = version())]
pub struct GolemCliCommand {
    #[command(flatten)]
    pub global_flags: GolemCliGlobalFlags,

    #[clap(subcommand)]
    pub subcommand: GolemCliSubcommand,
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

#[derive(Debug, Clone, Default, Args)]
pub struct GolemCliGlobalFlags {
    /// Output format, defaults to text, unless specified by the selected profile
    #[arg(long, short, global = true, display_order = 101)]
    pub format: Option<Format>,

    /// Select Golem profile by name
    #[arg(long, short, global = true, conflicts_with_all = ["local", "cloud"], display_order = 102)]
    pub profile: Option<ProfileName>,

    /// Select builtin "local" profile, to use services provided by the "golem server" command
    #[arg(long, short, global = true, conflicts_with_all = ["profile", "cloud"], display_order = 103
    )]
    pub local: bool,

    /// Select builtin "cloud" profile to use Golem Cloud
    #[arg(long, short, global = true, conflicts_with_all = ["profile", "local"], display_order = 104
    )]
    pub cloud: bool,

    /// Custom path to the root application manifest (golem.yaml)
    #[arg(long, short, global = true, display_order = 105)]
    pub app_manifest_path: Option<PathBuf>,

    /// Disable automatic searching for application manifests
    #[arg(long, short = 'A', global = true, display_order = 106)]
    pub disable_app_manifest_discovery: bool,

    /// Select build profile
    #[arg(long, short, global = true, display_order = 107)]
    pub build_profile: Option<BuildProfileName>,

    /// Custom path to the config directory (defaults to $HOME/.golem)
    #[arg(long, global = true, display_order = 108)]
    pub config_dir: Option<PathBuf>,

    /// Automatically answer "yes" to any interactive confirm questions
    #[arg(long, short, global = true, display_order = 109)]
    pub yes: bool,

    /// Disables filtering of potentially sensitive use values in text mode (e.g. component environment variable values)
    #[arg(long, global = true, display_order = 110)]
    pub show_sensitive: bool,

    /// Enable experimental, development-only features
    #[arg(long, global = true, display_order = 111)]
    pub dev_mode: bool,

    #[command(flatten)]
    pub verbosity: Verbosity,

    // The flags below can only be set through env vars, as they are mostly
    // useful for testing, so we do not want to pollute the flag space with them
    #[arg(skip)]
    pub golem_rust_path: Option<PathBuf>,

    #[arg(skip)]
    pub golem_rust_version: Option<String>,

    #[arg(skip)]
    pub wasm_rpc_offline: bool,

    #[arg(skip)]
    pub http_batch_size: Option<u64>,

    #[arg(skip)]
    pub auth_token: Option<Uuid>,

    #[arg(skip)]
    pub local_server_auto_start: bool,

    #[arg(skip)]
    pub server_no_limit_change: bool,

    #[arg(skip)]
    pub enable_wasmtime_fs_cache: bool,
}

impl GolemCliGlobalFlags {
    pub fn with_env_overrides(mut self) -> GolemCliGlobalFlags {
        if self.profile.is_none() {
            if let Ok(profile) = std::env::var("GOLEM_PROFILE") {
                self.profile = Some(profile.into());
            }
        }

        if self.app_manifest_path.is_none() {
            if let Ok(app_manifest_path) = std::env::var("GOLEM_APP_MANIFEST_PATH") {
                self.app_manifest_path = Some(PathBuf::from(app_manifest_path));
            }
        }

        if !self.disable_app_manifest_discovery {
            if let Ok(disable) = std::env::var("GOLEM_DISABLE_APP_MANIFEST_DISCOVERY") {
                self.disable_app_manifest_discovery = disable
                    .parse::<LenientBool>()
                    .map(|b| b.into())
                    .unwrap_or_default()
            }
        }

        if self.build_profile.is_none() {
            if let Ok(build_profile) = std::env::var("GOLEM_BUILD_PROFILE") {
                self.build_profile = Some(build_profile.into());
            }
        }

        if let Ok(offline) = std::env::var("GOLEM_WASM_RPC_OFFLINE") {
            self.wasm_rpc_offline = offline
                .parse::<LenientBool>()
                .map(|b| b.into())
                .unwrap_or_default()
        }

        if self.golem_rust_path.is_none() {
            if let Ok(wasm_rpc_path) = std::env::var("GOLEM_RUST_PATH") {
                self.golem_rust_path = Some(PathBuf::from(wasm_rpc_path));
            }
        }

        if self.golem_rust_version.is_none() {
            if let Ok(version) = std::env::var("GOLEM_RUST_VERSION") {
                self.golem_rust_version = Some(version);
            }
        }

        if let Ok(batch_size) = std::env::var("GOLEM_HTTP_BATCH_SIZE") {
            self.http_batch_size = Some(
                batch_size
                    .parse()
                    .with_context(|| format!("Failed to parse GOLEM_HTTP_BATCH_SIZE: {batch_size}"))
                    .unwrap(),
            )
        }

        if let Ok(auth_token) = std::env::var("GOLEM_AUTH_TOKEN") {
            self.auth_token = Some(
                auth_token
                    .parse()
                    .context("Failed to parse GOLEM_AUTH_TOKEN, expected uuid")
                    .unwrap(),
            );
        }

        if let Ok(auto_start) = std::env::var("GOLEM_LOCAL_SERVER_AUTO_START") {
            self.local_server_auto_start = auto_start
                .parse::<LenientBool>()
                .map(|b| b.into())
                .unwrap_or_default()
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

        self
    }

    pub fn config_dir(&self) -> PathBuf {
        self.config_dir
            .clone()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".golem"))
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
    fn try_parse_from<I, T>(args: I, with_env_overrides: bool) -> Self
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
            cmd.global_flags = cmd.global_flags.with_env_overrides();
        }

        cmd
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
                    command.global_flags = command.global_flags.with_env_overrides()
                }
                GolemCliCommandParseResult::FullMatch(command)
            }
            Err(error) => {
                let fallback_command =
                    GolemCliFallbackCommand::try_parse_from(&args, with_env_overrides);

                let partial_match = match error.kind() {
                    ErrorKind::DisplayHelp => {
                        let positional_args = fallback_command
                            .positional_args
                            .iter()
                            .map(|arg| arg.as_ref())
                            .collect::<Vec<_>>();
                        match positional_args.as_slice() {
                            ["app"] => Some(GolemCliCommandPartialMatch::AppHelp),
                            ["component"] => Some(GolemCliCommandPartialMatch::ComponentHelp),
                            ["agent"] => Some(GolemCliCommandPartialMatch::WorkerHelp),
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
                            ["app"] => Some(GolemCliCommandPartialMatch::AppMissingSubcommandHelp),
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
                to_partial_match: |_| GolemCliCommandPartialMatch::WorkerInvokeMissingWorkerName,
            },
            InvalidArgMatcher {
                subcommands: vec!["agent", "invoke"],
                found_positional_args: vec!["agent_id"],
                missing_positional_arg: "function_name",
                to_partial_match: |args| {
                    GolemCliCommandPartialMatch::WorkerInvokeMissingFunctionName {
                        worker_name: args[0].clone().into(),
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
    WorkerHelp,
    WorkerInvokeMissingFunctionName { worker_name: WorkerName },
    WorkerInvokeMissingWorkerName,
    ProfileSwitchMissingProfileName,
}

#[derive(Debug, Subcommand)]
pub enum GolemCliSubcommand {
    #[command(alias = "application")]
    /// Build, deploy application
    App {
        #[clap(subcommand)]
        subcommand: AppSubcommand,
    },
    /// Build, deploy and manage components
    Component {
        #[clap(subcommand)]
        subcommand: ComponentSubcommand,
    },
    /// Invoke and manage agents
    Agent {
        #[clap(subcommand)]
        subcommand: AgentSubcommand,
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
    /// Start Rib REPL for a selected component
    Repl {
        #[command(flatten)]
        component_name: ComponentOptionalComponentName,
        /// Optional component version to use, defaults to latest component version
        version: Option<u64>,
        #[command(flatten)]
        deploy_args: Option<DeployArgs>,
        /// Optional script to run, when defined the repl will execute the script and exit
        #[clap(long, short, conflicts_with_all = ["script_file"])]
        script: Option<String>,
        /// Optional script_file to run, when defined the repl will execute the script and exit
        #[clap(long, conflicts_with_all = ["script"])]
        script_file: Option<PathBuf>,
        /// Do not stream logs from the invoked agents. Can be also controlled with the :logs command in the REPL.
        #[clap(long)]
        disable_stream: bool,
    },
    /// Generate shell completion
    Completion {
        /// Selects shell
        shell: clap_complete::Shell,
    },
}

pub mod shared_args {
    use crate::model::app::AppBuildStep;
    use crate::model::{AccountId, PluginReference};
    use crate::model::{AgentUpdateMode, ComponentName, ProjectName, ProjectReference, WorkerName};
    use clap::Args;
    use golem_templates::model::GuestLanguage;

    pub type ComponentTemplateName = String;
    pub type NewWorkerArgument = String;
    pub type WorkerFunctionArgument = String;
    pub type WorkerFunctionName = String;

    #[derive(Debug, Args)]
    pub struct ComponentMandatoryComponentName {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Optional component name, if not specified component is selected based on the current directory.
        /// Accepted formats:
        ///   - <COMPONENT>
        ///   - <PROJECT>/<COMPONENT>
        ///   - <ACCOUNT>/<PROJECT>/<COMPONENT>
        #[arg(verbatim_doc_comment)]
        pub component_name: ComponentName,
    }

    #[derive(Debug, Args)]
    pub struct ComponentOptionalComponentName {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Optional component name, if not specified component is selected based on the current directory.
        /// Accepted formats:
        ///   - <COMPONENT>
        ///   - <PROJECT>/<COMPONENT>
        ///   - <ACCOUNT>/<PROJECT>/<COMPONENT>
        #[arg(verbatim_doc_comment)]
        pub component_name: Option<ComponentName>,
    }

    #[derive(Debug, Args)]
    pub struct OptionalAgentTypeName {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Optional agent type name. If not specified, the component name must be specified.
        #[clap(long, verbatim_doc_comment)]
        pub agent_type_name: Option<String>,
    }

    #[derive(Debug, Args)]
    pub struct ComponentOptionalComponentNames {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Optional component names, if not specified components are selected based on the current directory
        /// Accepted formats:
        ///   - <COMPONENT>
        ///   - <PROJECT>/<COMPONENT>
        ///   - <ACCOUNT>/<PROJECT>/<COMPONENT>
        #[arg(verbatim_doc_comment)]
        pub component_name: Vec<ComponentName>,
    }

    #[derive(Debug, Args)]
    pub struct AppOptionalComponentNames {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Optional component names, if not specified all components are selected.
        /// Accepted formats:
        ///   - <COMPONENT>
        ///   - <PROJECT>/<COMPONENT>
        ///   - <ACCOUNT>/<PROJECT>/<COMPONENT>
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
        #[clap(long, short)]
        pub step: Vec<AppBuildStep>,
        #[command(flatten)]
        pub force_build: ForceBuildArg,
    }

    #[derive(Debug, Args)]
    pub struct AgentIdArgs {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Agent ID, accepted formats:
        ///   - <AGENT_TYPE>(<AGENT_PARAMETERS>)
        ///   - <COMPONENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
        ///   - <PROJECT>/<COMPONENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
        ///   - <ACCOUNT>/<PROJECT>/<COMPONENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
        #[arg(verbatim_doc_comment)]
        pub agent_id: WorkerName,
    }

    #[derive(Debug, Args)]
    pub struct StreamArgs {
        /// Hide log levels in stream output
        #[clap(long, short = 'L')]
        pub stream_no_log_level: bool,
        /// Hide timestamp in stream output
        #[clap(long, short = 'T')]
        pub stream_no_timestamp: bool,
        /// Only show entries coming from the agent, no output about invocation markers and stream status
        #[clap(long)]
        pub logs_only: bool,
    }

    #[derive(Debug, Args, Clone)]
    pub struct DeployArgs {
        /// Update existing agents with auto or manual update mode
        #[clap(long, value_name = "UPDATE_MODE", short, conflicts_with_all = ["redeploy_agents", "redeploy_all"], num_args = 0..=1)]
        pub update_agents: Option<AgentUpdateMode>,
        /// Delete and recreate existing agents
        #[clap(long, conflicts_with_all = ["update_agents"])]
        pub redeploy_agents: bool,
        /// Delete and recreate HTTP API definitions and deployment
        #[clap(long, conflicts_with_all = ["redeploy_all"])]
        pub redeploy_http_api: bool,
        /// Delete and recreate agents and HTTP APIs
        #[clap(long, conflicts_with_all = ["update_agents", "redeploy_agents", "redeploy_http_api"])]
        pub redeploy_all: bool,
        /// Delete agents, HTTP APIs and sites, then redeploy HTTP APIs and sites
        #[clap(long, short, conflicts_with_all = ["update_agents", "redeploy_agents", "redeploy_http_api", "redeploy_all"])]
        pub reset: bool,
    }

    impl DeployArgs {
        pub fn is_any_set(&self) -> bool {
            self.update_agents.is_some()
                || self.redeploy_agents
                || self.redeploy_http_api
                || self.redeploy_all
                || self.reset
        }

        pub fn none() -> Self {
            DeployArgs {
                update_agents: None,
                redeploy_agents: false,
                redeploy_http_api: false,
                redeploy_all: false,
                reset: false,
            }
        }

        pub fn delete_agents(&self, profile_args: &DeployArgs) -> bool {
            (profile_args.reset || self.reset)
                && !self.redeploy_agents
                && !self.redeploy_all
                && self.update_agents.is_none()
        }

        pub fn redeploy_agents(&self, profile_args: &DeployArgs) -> bool {
            (profile_args.redeploy_all
                || profile_args.redeploy_agents
                || self.redeploy_all
                || self.redeploy_agents)
                && !self.reset
                && self.update_agents.is_none()
        }

        pub fn redeploy_http_api(&self, profile_args: &DeployArgs) -> bool {
            profile_args.redeploy_all
                || profile_args.redeploy_http_api
                || profile_args.reset
                || self.redeploy_all
                || self.redeploy_http_api
                || self.reset
        }
    }

    #[derive(Debug, Args)]
    pub struct ProjectArg {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Project, accepted formats:
        ///   - <PROJECT_NAME>
        ///   - <ACCOUNT_EMAIL>/<PROJECT_NAME>
        #[arg(verbatim_doc_comment)]
        pub project: ProjectReference,
    }

    #[derive(Debug, Args)]
    pub struct ProjectOptionalFlagArg {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Project, accepted formats:
        ///   - <PROJECT_NAME>
        ///   - <ACCOUNT_EMAIL>/<PROJECT_NAME>
        #[arg(verbatim_doc_comment, long)]
        pub project: Option<ProjectReference>,
    }

    #[derive(Debug, Args)]
    pub struct AccountIdOptionalArg {
        /// Account ID
        #[arg(long)]
        pub account_id: Option<AccountId>,
    }

    #[derive(Debug, Args)]
    pub struct PluginArg {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Plugin, accepted formats:
        ///   - <PLUGIN_NAME>/<PLUGIN_VERSION>
        ///   - <ACCOUNT_EMAIL>/<PLUGIN_NAME>/<PLUGIN_VERSION>
        #[arg(verbatim_doc_comment)]
        pub plugin: PluginReference,
    }

    #[derive(clap::Args, Debug, Clone)]
    pub struct PluginScopeArgs {
        /// Global scope (plugin available for all components)
        #[arg(long, conflicts_with_all=["account", "project", "component"])]
        pub global: bool,
        /// Account id, optionally specifies the account id for the project name
        #[arg(long, conflicts_with_all = ["global"])]
        pub account: Option<String>,
        /// Project name; Required when component name is used. Without a given component, it defines a project scope.
        #[arg(long, conflicts_with_all = ["global"])]
        pub project: Option<ProjectName>,
        /// Component scope given by the component's name (plugin only available for this component)
        #[arg(long, conflicts_with_all=["global"])]
        pub component: Option<ComponentName>,
    }

    impl PluginScopeArgs {
        pub fn is_global(&self) -> bool {
            self.global
                || (self.account.is_none() && self.project.is_none() && self.component.is_none())
        }
    }
}

pub mod app {
    use crate::command::shared_args::{
        AppOptionalComponentNames, BuildArgs, DeployArgs, ForceBuildArg,
    };
    use crate::model::AgentUpdateMode;
    use clap::Subcommand;
    use golem_templates::model::GuestLanguage;

    #[derive(Debug, Subcommand)]
    pub enum AppSubcommand {
        /// Create new application
        New {
            /// Application folder name where the new application should be created
            application_name: Option<String>,
            /// Languages that the application should support
            language: Vec<GuestLanguage>,
        },
        /// Build all or selected components in the application
        Build {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
            #[command(flatten)]
            build: BuildArgs,
        },
        /// Deploy all or selected components and HTTP APIs in the application, includes building
        Deploy {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
            #[command(flatten)]
            force_build: ForceBuildArg,
            #[command(flatten)]
            deploy_args: DeployArgs,
        },
        /// Clean all components in the application or by selection
        Clean {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
        },
        /// Try to automatically update all existing agents of the application to the latest version
        UpdateAgents {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
            /// Update mode - auto or manual, defaults to "auto"
            #[arg(long, short, default_value = "auto")]
            update_mode: AgentUpdateMode,
            /// Await the update to be completed
            #[arg(long, default_value_t = false)]
            r#await: bool,
        },
        /// Redeploy all agents of the application using the latest version
        RedeployAgents {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
        },
        /// Diagnose possible tooling problems
        Diagnose {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
        },
        /// List all the deployed agent types
        ListAgentTypes {},
        /// Run custom command
        #[clap(external_subcommand)]
        CustomCommand(Vec<String>),
    }
}

pub mod component {
    use crate::command::component::plugin::ComponentPluginSubcommand;
    use crate::command::shared_args::{
        BuildArgs, ComponentOptionalComponentName, ComponentOptionalComponentNames,
        ComponentTemplateName, DeployArgs, ForceBuildArg,
    };
    use crate::model::app::DependencyType;
    use crate::model::{AgentUpdateMode, ComponentName};
    use clap::Subcommand;
    use golem_templates::model::PackageName;
    use std::path::PathBuf;
    use url::Url;

    #[derive(Debug, Subcommand)]
    pub enum ComponentSubcommand {
        /// Create new component in the current application
        New {
            /// Template to be used for the new component
            component_template: Option<ComponentTemplateName>,
            /// Name of the new component package in 'package:name' form
            component_name: Option<PackageName>,
        },
        /// List or search component templates
        Templates {
            /// Optional filter for language or template name
            filter: Option<String>,
        },
        /// Build component(s) based on the current directory or by selection
        Build {
            #[command(flatten)]
            component_name: ComponentOptionalComponentNames,
            #[command(flatten)]
            build: BuildArgs,
        },
        /// Deploy component(s) and dependent HTTP APIs based on the current directory or by selection
        Deploy {
            #[command(flatten)]
            component_name: ComponentOptionalComponentNames,
            #[command(flatten)]
            force_build: ForceBuildArg,
            #[command(flatten)]
            deploy_args: DeployArgs,
        },
        /// Clean component(s) based on the current directory or by selection
        Clean {
            #[command(flatten)]
            component_name: ComponentOptionalComponentNames,
        },
        /// Add or update a component dependency
        AddDependency {
            /// The name of the component to which the dependency should be added
            #[arg(long)]
            component_name: Option<ComponentName>,
            /// The name of the component that will be used as the target component
            #[arg(long, conflicts_with_all = ["target_component_path", "target_component_url"])]
            target_component_name: Option<ComponentName>,
            /// The path to the local component WASM that will be used as the target
            #[arg(long, conflicts_with_all = ["target_component_name", "target_component_url"])]
            target_component_path: Option<PathBuf>,
            /// The URL to the remote component WASM that will be used as the target
            #[arg(long, conflicts_with_all = ["target_component_name", "target_component_path"])]
            target_component_url: Option<Url>,
            /// The type of the dependency, defaults to wasm-rpc
            #[arg(long)]
            dependency_type: Option<DependencyType>,
        },
        /// List deployed component versions' metadata
        List {
            #[command(flatten)]
            component_name: ComponentOptionalComponentName,
        },
        /// Get latest or selected version of deployed component metadata
        Get {
            #[command(flatten)]
            component_name: ComponentOptionalComponentName,
            /// Optional component version to get
            version: Option<u64>,
        },
        /// Try to automatically update all existing agents of the selected component to the latest version
        UpdateAgents {
            #[command(flatten)]
            component_name: ComponentOptionalComponentName,
            /// Agent update mode - auto or manual, defaults to "auto"
            #[arg(long, short, default_value_t = AgentUpdateMode::Automatic)]
            update_mode: AgentUpdateMode,
            /// Await the update to be completed
            #[arg(long, default_value_t = false)]
            r#await: bool,
        },
        /// Redeploy all agents of the selected component using the latest version
        RedeployAgents {
            #[command(flatten)]
            component_name: ComponentOptionalComponentName,
        },
        /// Manage component plugin installations
        Plugin {
            #[command(subcommand)]
            subcommand: ComponentPluginSubcommand,
        },
        /// Diagnose possible tooling problems
        Diagnose {
            #[command(flatten)]
            component_name: ComponentOptionalComponentNames,
        },
    }

    pub mod plugin {
        use crate::command::parse_key_val;
        use crate::command::shared_args::ComponentOptionalComponentName;
        use clap::Subcommand;
        use golem_common::base_model::PluginInstallationId;

        #[derive(Debug, Subcommand)]
        pub enum ComponentPluginSubcommand {
            /// Install a plugin for selected component
            Install {
                #[command(flatten)]
                component_name: ComponentOptionalComponentName,
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
                component_name: ComponentOptionalComponentName,
                /// The version of the component
                version: Option<u64>,
            },
            /// Update component plugin
            Update {
                /// The component to update the plugin for
                #[command(flatten)]
                component_name: ComponentOptionalComponentName,
                /// Installation id of the plugin to update
                #[arg(long)]
                installation_id: PluginInstallationId,
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
                component_name: ComponentOptionalComponentName,
                /// Installation id of the plugin to uninstall
                #[arg(long)]
                installation_id: PluginInstallationId,
            },
        }
    }
}

pub mod worker {
    use crate::command::parse_cursor;
    use crate::command::parse_key_val;
    use crate::command::shared_args::{
        AgentIdArgs, ComponentOptionalComponentName, DeployArgs, NewWorkerArgument,
        OptionalAgentTypeName, StreamArgs, WorkerFunctionArgument, WorkerFunctionName,
    };
    use crate::model::{AgentUpdateMode, IdempotencyKey};
    use clap::Subcommand;
    use golem_client::model::ScanCursor;

    #[derive(Debug, Subcommand)]
    pub enum AgentSubcommand {
        /// Create new agent
        New {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Command-line arguments visible for the agent
            arguments: Vec<NewWorkerArgument>,
            /// Environment variables visible for the agent
            #[arg(short, long, value_parser = parse_key_val, value_name = "ENV=VAL")]
            env: Vec<(String, String)>,
        },
        // TODO: json args
        /// Invoke (or enqueue invocation for) agent
        Invoke {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Agent function name to invoke
            function_name: WorkerFunctionName,
            /// Agent function arguments in WAVE format
            arguments: Vec<WorkerFunctionArgument>,
            /// Only trigger invocation and do not wait for it
            #[clap(long, short)]
            trigger: bool,
            /// Set idempotency key for the call, use "-" for an auto generated key
            #[clap(long, short)]
            idempotency_key: Option<IdempotencyKey>,
            #[clap(long, short)]
            /// Connect to the agent before the invocation and live stream its standard output, error and log channels
            stream: bool,
            #[command(flatten)]
            stream_args: StreamArgs,
            #[command(flatten)]
            deploy_args: Option<DeployArgs>,
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
            #[command(flatten)]
            component_name: ComponentOptionalComponentName,

            #[command(flatten)]
            agent_type_name: OptionalAgentTypeName,

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
            /// The maximum the number of returned agents, returns all values is not specified.
            /// When multiple component is selected, then the limit it is applied separately
            #[arg(long, short)]
            max_count: Option<u64>,
            /// When set to true it queries for most up-to-date status for each agent, default is false
            #[arg(long, default_value_t = false)]
            precise: bool,
        },
        /// Connect to an agent and live stream its standard output, error and log channels
        Stream {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            #[command(flatten)]
            stream_args: StreamArgs,
        },
        /// Updates an agent
        Update {
            #[command(flatten)]
            agent_id: AgentIdArgs,
            /// Update mode - auto or manual (default is auto)
            mode: Option<AgentUpdateMode>,
            /// The new version of the updated agent (default is the latest version)
            target_version: Option<u64>,
            /// Await the update to be completed
            #[arg(long, default_value_t = false)]
            r#await: bool,
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
        /// List files in a worker's directory
        Files {
            #[command(flatten)]
            worker_name: AgentIdArgs,
            /// Path to the directory to list files from
            #[arg(default_value = "/")]
            path: String,
        },
        /// Get contents of a file in a worker
        FileContents {
            #[command(flatten)]
            worker_name: AgentIdArgs,
            /// Path to the file to get contents from
            path: String,
            /// Local path (including filename) to save the file contents. Optional.
            #[arg(long)]
            output: Option<String>,
        },
    }
}

pub mod api {
    use crate::command::api::cloud::ApiCloudSubcommand;
    use crate::command::api::definition::ApiDefinitionSubcommand;
    use crate::command::api::deployment::ApiDeploymentSubcommand;
    use crate::command::api::security_scheme::ApiSecuritySchemeSubcommand;
    use crate::command::shared_args::DeployArgs;
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum ApiSubcommand {
        /// Deploy API Definitions and Deployments
        Deploy {
            #[command(flatten)]
            deploy_args: DeployArgs,
        },
        /// Manage API definitions
        Definition {
            #[clap(subcommand)]
            subcommand: ApiDefinitionSubcommand,
        },
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
        /// Manage API Cloud Domains and Certificates
        Cloud {
            #[clap(subcommand)]
            subcommand: ApiCloudSubcommand,
        },
    }

    pub mod definition {
        use crate::command::shared_args::{DeployArgs, ProjectOptionalFlagArg};
        use crate::model::api::{ApiDefinitionId, ApiDefinitionVersion};
        use crate::model::app::HttpApiDefinitionName;
        use crate::model::OpenApiDefinitionOutputFormat;
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiDefinitionSubcommand {
            /// Deploy API Definitions and required components
            Deploy {
                /// API definition to deploy, if not specified, all definitions are deployed
                http_api_definition_name: Option<HttpApiDefinitionName>,
                #[command(flatten)]
                deploy_args: DeployArgs,
            },
            /// Retrieves metadata about an existing API definition
            Get {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// API definition id
                #[arg(short, long)]
                id: ApiDefinitionId,
                /// Version of the api definition
                #[arg(long)]
                version: ApiDefinitionVersion,
            },
            /// Lists all API definitions
            List {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// API definition id to get all versions. Optional.
                #[arg(short, long)]
                id: Option<ApiDefinitionId>,
            },
            /// Deletes an existing API definition
            Delete {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// API definition id
                #[arg(short, long)]
                id: ApiDefinitionId,
                /// Version of the api definition
                #[arg(long)]
                version: ApiDefinitionVersion,
            },
            /// Exports an api definition in OpenAPI format
            Export {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// Api definition id
                #[arg(short, long)]
                id: ApiDefinitionId,
                /// Version of the api definition
                #[arg(short = 'V', long)]
                version: ApiDefinitionVersion,
                /// Output format (json or yaml)
                #[arg(long = "def-format", default_value = "yaml", name = "def-format")]
                format: OpenApiDefinitionOutputFormat,
                /// Custom output file name (without extension)
                #[arg(short, long)]
                output_name: Option<String>,
            },
            /// Opens Swagger UI for an API definition
            Swagger {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// Api definition id
                #[arg(short, long)]
                id: ApiDefinitionId,
                /// Version of the api definition
                #[arg(short = 'V', long)]
                version: ApiDefinitionVersion,
                /// Port to open Swagger UI on (defaults to 9007)
                #[arg(short = 'P', long, default_value_t = 9007)]
                port: u16,
            },
        }
    }

    pub mod deployment {
        use crate::command::shared_args::{DeployArgs, ProjectOptionalFlagArg};
        use crate::model::api::ApiDefinitionId;
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiDeploymentSubcommand {
            /// Deploy API Deployments
            Deploy {
                /// Host or site to deploy, if not defined, all deployments will be deployed
                host_or_site: Option<String>,
                #[command(flatten)]
                deploy_args: DeployArgs,
            },
            /// Get API deployment
            Get {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// Deployment site
                #[arg(value_name = "subdomain.host")]
                site: String,
            },
            /// List API deployment for API definition
            List {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// API definition id
                definition: Option<ApiDefinitionId>,
            },
            /// Delete api deployment
            Delete {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// Deployment site
                #[arg(value_name = "subdomain.host")]
                site: String,
            },
        }
    }

    pub mod security_scheme {
        use crate::command::shared_args::ProjectOptionalFlagArg;
        use crate::model::api::IdentityProviderType;
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiSecuritySchemeSubcommand {
            /// Create API Security Scheme
            Create {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// Security Scheme ID
                security_scheme_id: String,
                /// Security Scheme provider (Google, Facebook, Gitlab, Microsoft)
                #[arg(long)]
                provider_type: IdentityProviderType,
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

            /// Get API security
            Get {
                #[command(flatten)]
                project: ProjectOptionalFlagArg,
                /// Security Scheme ID
                security_scheme_id: String,
            },
        }
    }

    pub mod cloud {
        use crate::command::api::cloud::certificate::ApiCertificateSubcommand;
        use crate::command::api::cloud::domain::ApiDomainSubcommand;
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiCloudSubcommand {
            /// Manage Cloud API Domains
            Domain {
                #[clap(subcommand)]
                subcommand: ApiDomainSubcommand,
            },
            /// Manage Cloud API Certificates
            Certificate {
                #[clap(subcommand)]
                subcommand: ApiCertificateSubcommand,
            },
        }

        pub mod domain {
            use crate::command::shared_args::ProjectArg;
            use clap::Subcommand;

            #[derive(Debug, Subcommand)]
            pub enum ApiDomainSubcommand {
                /// Retrieves metadata about an existing domain
                Get {
                    #[clap(flatten)]
                    project: ProjectArg,
                },
                /// Add new domain
                New {
                    #[clap(flatten)]
                    project: ProjectArg,
                    /// Domain name
                    domain_name: String,
                },
                /// Delete an existing domain
                Delete {
                    #[clap(flatten)]
                    project: ProjectArg,
                    /// Domain name
                    domain_name: String,
                },
            }
        }

        pub mod certificate {
            use crate::command::shared_args::ProjectArg;
            use crate::model::PathBufOrStdin;
            use clap::Subcommand;
            use uuid::Uuid;

            #[derive(Debug, Subcommand)]
            pub enum ApiCertificateSubcommand {
                /// Retrieves metadata about an existing certificate
                Get {
                    #[clap(flatten)]
                    project: ProjectArg,
                    /// Certificate ID
                    certificate_id: Option<Uuid>,
                },
                /// Create new certificate
                New {
                    #[clap(flatten)]
                    project: ProjectArg,
                    /// Domain name
                    #[arg(short, long)]
                    domain_name: String,
                    /// Certificate
                    #[arg(long, value_hint = clap::ValueHint::FilePath)]
                    certificate_body: PathBufOrStdin,
                    /// Certificate private key
                    #[arg(long, value_hint = clap::ValueHint::FilePath)]
                    certificate_private_key: PathBufOrStdin,
                },
                /// Delete an existing certificate
                #[command()]
                Delete {
                    #[clap(flatten)]
                    project: ProjectArg,
                    /// Certificate ID
                    certificate_id: Uuid,
                },
            }
        }
    }
}

pub mod plugin {
    use super::shared_args::PluginArg;
    use crate::command::shared_args::PluginScopeArgs;
    use crate::model::PathBufOrStdin;
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum PluginSubcommand {
        /// List component for the select scope
        List {
            /// The scope to list components from
            #[command(flatten)]
            scope: PluginScopeArgs,
        },
        /// Get information about a registered plugin
        Get {
            #[clap(flatten)]
            plugin: PluginArg,
        },
        /// Register a new plugin
        Register {
            #[command(flatten)]
            scope: PluginScopeArgs,
            /// Path to the plugin manifest JSON or '-' to use STDIN
            manifest: PathBufOrStdin,
        },
        /// Unregister a plugin
        Unregister {
            #[clap(flatten)]
            plugin: PluginArg,
        },
    }
}

pub mod profile {
    use crate::command::profile::config::ProfileConfigSubcommand;
    use crate::config::ProfileName;
    use crate::model::Format;
    use clap::Subcommand;
    use url::Url;
    use uuid::Uuid;

    #[allow(clippy::large_enum_variant)]
    #[derive(Debug, Subcommand)]
    pub enum ProfileSubcommand {
        /// Create new global profile, call without <PROFILE_NAME> for interactive setup
        New {
            /// Name of the newly created profile
            name: Option<ProfileName>,
            /// Switch to the profile after creation
            #[arg(long)]
            set_active: bool,
            /// URL of Golem Component service
            #[arg(long)]
            component_url: Option<Url>,
            /// URL of Golem Worker service, if not provided defaults to component-url
            #[arg(long)]
            worker_url: Option<Url>,
            /// URL of Golem Cloud service, if not provided defaults to component-url
            #[arg(long)]
            cloud_url: Option<Url>,
            /// Default output format
            #[arg(long, default_value_t = Format::Text)]
            default_format: Format,
            /// Token to use for authenticating against Golem. If not provided an OAuth2 flow will be performed when authentication is needed for the first time.
            #[arg(long)]
            static_token: Option<Uuid>,
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
        use crate::model::Format;
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
    use crate::command::cloud::project::ProjectSubcommand;
    use crate::command::cloud::token::TokenSubcommand;
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum CloudSubcommand {
        /// Manage Cloud Projects
        Project {
            #[clap(subcommand)]
            subcommand: ProjectSubcommand,
        },
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
        use crate::command::parse_instant;
        use crate::model::TokenId;
        use chrono::{DateTime, Utc};
        use clap::Subcommand;

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
        use crate::command::cloud::account::grant::GrantSubcommand;
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
            /// Manage the account roles
            Grant {
                #[command(subcommand)]
                subcommand: GrantSubcommand,
            },
        }

        pub mod grant {
            use crate::command::shared_args::AccountIdOptionalArg;
            use crate::model::Role;
            use clap::Subcommand;

            #[derive(Subcommand, Debug)]
            pub enum GrantSubcommand {
                /// Get the roles granted to the account
                Get {
                    #[command(flatten)]
                    account_id: AccountIdOptionalArg,
                },
                /// Grant a new role to the account
                New {
                    #[command(flatten)]
                    account_id: AccountIdOptionalArg,
                    /// The role to be granted
                    role: Role,
                },
                /// Remove a role from the account
                Delete {
                    #[command(flatten)]
                    account_id: AccountIdOptionalArg,
                    /// The role to be deleted
                    role: Role,
                },
            }
        }
    }

    pub mod project {
        use crate::command::cloud::project::plugin::ProjectPluginSubcommand;
        use crate::command::cloud::project::policy::PolicySubcommand;
        use crate::model::{ProjectName, ProjectPolicyId, ProjectReference};
        use clap::Subcommand;
        use golem_common::model::auth::ProjectPermission;

        #[derive(clap::Args, Debug)]
        #[group(required = true, multiple = false)]
        pub struct ProjectActionsOrPolicyId {
            /// The sharing policy's identifier. If not provided, use `--action` instead
            #[arg(long, required = true, group = "project_actions_or_policy")]
            pub policy_id: Option<ProjectPolicyId>,
            /// A list of actions to be granted to the recipient account. If not provided, use `--policy-id` instead
            #[arg(long, required = true, group = "project_actions_or_policy")]
            pub action: Option<Vec<ProjectPermission>>,
        }

        #[derive(Debug, Subcommand)]
        pub enum ProjectSubcommand {
            /// Create new project
            New {
                /// The new project's name
                project_name: ProjectName,
                /// The new project's description
                #[arg(short, long)]
                description: Option<String>,
            },
            /// Lists existing projects
            List {
                /// Optionally filter projects by name
                project_name: Option<ProjectName>,
            },
            /// Gets the default project which is used when no explicit project is specified
            GetDefault,
            /// Share a project with another account
            Grant {
                /// The project to be shared
                project_reference: ProjectReference,
                /// Email of the user account the project will be shared with
                recipient_email: String,
                #[command(flatten)]
                project_actions_or_policy_id: ProjectActionsOrPolicyId,
            },
            /// Manage project policies
            Policy {
                #[command(subcommand)]
                subcommand: PolicySubcommand,
            },
            /// Manage project plugins
            Plugin {
                #[command(subcommand)]
                subcommand: ProjectPluginSubcommand,
            },
        }

        pub mod policy {
            use crate::model::ProjectPolicyId;
            use clap::Subcommand;
            use golem_common::model::auth::ProjectPermission;

            #[derive(Subcommand, Debug)]
            pub enum PolicySubcommand {
                /// Creates a new project sharing policy
                New {
                    /// Name of the policy
                    policy_name: String,
                    /// List of actions allowed by the policy
                    actions: Vec<ProjectPermission>,
                },
                /// Gets the existing project sharing policies
                #[command()]
                Get {
                    /// Project policy ID
                    policy_id: ProjectPolicyId,
                },
            }
        }

        pub mod plugin {
            use crate::command::parse_key_val;
            use crate::command::shared_args::ProjectArg;
            use clap::Subcommand;
            use golem_common::base_model::PluginInstallationId;

            #[derive(Debug, Subcommand)]
            pub enum ProjectPluginSubcommand {
                /// Install a plugin for a project
                Install {
                    #[clap(flatten)]
                    project: ProjectArg,
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
                /// Get the installed plugins for the project
                Get {
                    #[clap(flatten)]
                    project: ProjectArg,
                    /* TODO: Missing from HTTP API
                    /// The version of the component
                    version: Option<u64>,
                    */
                },
                /// Update project plugin
                Update {
                    #[clap(flatten)]
                    project: ProjectArg,
                    /// Installation id of the plugin to update
                    plugin_installation_id: PluginInstallationId,
                    /// Updated priority of the plugin - largest priority is applied first
                    #[arg(long)]
                    priority: i32,
                    /// Updated list of parameters (key-value pairs) passed to the plugin
                    #[arg(long, value_parser = parse_key_val, value_name = "KEY=VAL")]
                    param: Vec<(String, String)>,
                },
                /// Uninstall a plugin for selected component
                Uninstall {
                    #[clap(flatten)]
                    project: ProjectArg,
                    /// Installation id of the plugin to uninstall
                    plugin_installation_id: PluginInstallationId,
                },
            }
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

        /// Directory to store data in. Defaults to $XDG_STATE_HOME/golem
        #[clap(long)]
        pub data_dir: Option<PathBuf>,

        /// Clean the data directory before starting
        #[clap(long)]
        pub clean: bool,
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

pub fn builtin_app_subcommands() -> BTreeSet<String> {
    GolemCliCommand::command()
        .find_subcommand("app")
        .unwrap()
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_string())
        .collect()
}

fn help_target_to_subcommand_names(target: ShowClapHelpTarget) -> Vec<&'static str> {
    match target {
        ShowClapHelpTarget::AppNew => {
            vec!["app", "new"]
        }
        ShowClapHelpTarget::ComponentNew => {
            vec!["component", "new"]
        }
        ShowClapHelpTarget::ComponentAddDependency => {
            vec!["component", "add-dependency"]
        }
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

fn parse_key_val(key_and_val: &str) -> anyhow::Result<(String, String)> {
    let pos = key_and_val.find('=').ok_or_else(|| {
        anyhow!(
            "invalid KEY=VALUE: no `=` found in `{}`",
            key_and_val.log_color_error_highlight()
        )
    })?;
    Ok((
        key_and_val[..pos].to_string(),
        key_and_val[pos + 1..].to_string(),
    ))
}

// TODO: better error context and messages
fn parse_cursor(cursor: &str) -> anyhow::Result<ScanCursor> {
    let parts = cursor.split('/').collect::<Vec<_>>();

    if parts.len() != 2 {
        bail!("Invalid cursor format: {}", cursor);
    }

    Ok(ScanCursor {
        layer: parts[0].parse()?,
        cursor: parts[1].parse()?,
    })
}

fn parse_instant(
    s: &str,
) -> Result<DateTime<Utc>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    match s.parse::<DateTime<Utc>>() {
        Ok(dt) => Ok(dt),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod test {
    use crate::command::{
        builtin_app_subcommands, help_target_to_subcommand_names, GolemCliCommand,
    };
    use crate::error::ShowClapHelpTarget;
    use assert2::assert;
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
        println!("{:?}", builtin_app_subcommands())
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
