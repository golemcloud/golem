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

use crate::command::api::ApiSubcommand;
use crate::command::app::AppSubcommand;
use crate::command::cloud::CloudSubcommand;
use crate::command::component::ComponentSubcommand;
use crate::command::plugin::PluginSubcommand;
use crate::command::profile::ProfileSubcommand;
use crate::command::worker::WorkerSubcommand;
use crate::config::{BuildProfileName, ProfileName};
use crate::model::{Format, WorkerName};
use anyhow::{anyhow, bail, Context as AnyhowContext};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{self, CommandFactory, Subcommand};
use clap::{Args, Parser};
use clap_verbosity_flag::Verbosity;
use golem_client::model::ScanCursor;
use golem_wasm_rpc_stubgen::log::LogColorize;
use lenient_bool::LenientBool;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(feature = "server-commands")]
use crate::command::server::ServerSubcommand;

/// Golem Command Line Interface
#[derive(Debug, Parser)]
pub struct GolemCliCommand {
    #[command(flatten)]
    pub global_flags: GolemCliGlobalFlags,

    #[clap(subcommand)]
    pub subcommand: GolemCliSubcommand,
}

#[derive(Debug, Default, Args)]
pub struct GolemCliGlobalFlags {
    /// Output format, defaults to text, unless specified by the selected profile
    #[arg(long, short, global = true)]
    pub format: Option<Format>,

    /// Select Golem profile by name
    #[arg(long, short, global = true, conflicts_with_all = ["local", "cloud"])]
    pub profile: Option<ProfileName>,

    /// Select builtin "local" profile, to use services provided by the "golem server" command
    #[arg(long, short, global = true, conflicts_with_all = ["profile", "cloud"])]
    pub local: bool,

    /// Select builtin "cloud" profile to use Golem Cloud
    #[arg(long, short, global = true, conflicts_with_all = ["profile", "local"])]
    pub cloud: bool,

    /// Custom path to the root application manifest (golem.yaml)
    #[arg(long, short, global = true)]
    pub app_manifest_path: Option<PathBuf>,

    /// Disable automatic searching for application manifests
    #[arg(long, short = 'A', global = true)]
    pub disable_app_manifest_discovery: bool,

    /// Select build profile
    #[arg(long, short, global = true)]
    pub build_profile: Option<BuildProfileName>,

    /// Custom path to the config directory (defaults to $HOME/.golem)
    #[arg(long, global = true)]
    pub config_dir: Option<PathBuf>,

    #[command(flatten)]
    pub verbosity: Verbosity,

    // The flags below can only be set through env vars, as they are mostly
    // useful for testing, so we do not want to pollute the flag space with them
    #[arg(skip)]
    pub wasm_rpc_path: Option<PathBuf>,

    #[arg(skip)]
    pub wasm_rpc_version: Option<String>,

    #[arg(skip)]
    pub wasm_rpc_offline: bool,

    #[arg(skip)]
    pub http_batch_size: Option<u64>,
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

        if self.wasm_rpc_path.is_none() {
            if let Ok(wasm_rpc_path) = std::env::var("GOLEM_WASM_RPC_PATH") {
                self.wasm_rpc_path = Some(PathBuf::from(wasm_rpc_path));
            }
        }

        if self.wasm_rpc_version.is_none() {
            if let Ok(version) = std::env::var("GOLEM_WASM_RPC_VERSION") {
                self.wasm_rpc_version = Some(version);
            }
        }

        if let Ok(batch_size) = std::env::var("GOLEM_HTTP_BATCH_SIZE") {
            self.http_batch_size = Some(
                batch_size
                    .parse()
                    .with_context(|| {
                        format!("Failed to parse GOLEM_HTTP_BATCH_SIZE: {}", batch_size)
                    })
                    .unwrap(),
            )
        }

        self
    }

    pub fn config_dir(&self) -> PathBuf {
        self.config_dir
            .clone()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".golem"))
    }
}

#[derive(Debug, Default, Parser)]
#[command(ignore_errors = true)]
pub struct GolemCliFallbackCommand {
    #[command(flatten)]
    pub global_flags: GolemCliGlobalFlags,

    pub positional_args: Vec<String>,
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
                let fallback_command = {
                    let mut fallback_command =
                        GolemCliFallbackCommand::try_parse_from(args).unwrap_or_default();
                    if with_env_overrides {
                        fallback_command.global_flags =
                            fallback_command.global_flags.with_env_overrides()
                    }
                    fallback_command
                };

                let partial_match = match error.kind() {
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
                subcommands: vec!["app", "new"],
                found_positional_args: vec![],
                missing_positional_arg: "language",
                to_partial_match: |_| GolemCliCommandPartialMatch::AppNewMissingLanguage,
            },
            InvalidArgMatcher {
                subcommands: vec!["component", "new"],
                found_positional_args: vec![],
                missing_positional_arg: "component_template",
                to_partial_match: |_| GolemCliCommandPartialMatch::ComponentNewMissingTemplate,
            },
            InvalidArgMatcher {
                subcommands: vec!["worker", "invoke"],
                found_positional_args: vec![],
                missing_positional_arg: "worker_name",
                to_partial_match: |_| GolemCliCommandPartialMatch::WorkerInvokeMissingWorkerName,
            },
            InvalidArgMatcher {
                subcommands: vec!["worker", "invoke"],
                found_positional_args: vec!["worker_name"],
                missing_positional_arg: "function_name",
                to_partial_match: |args| {
                    GolemCliCommandPartialMatch::WorkerInvokeMissingFunctionName {
                        worker_name: args[0].clone().into(),
                    }
                },
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
            let missing_args_error_name = format!("{}...", missing_arg_error_name);
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
    AppNewMissingLanguage,
    AppMissingSubcommandHelp,
    ComponentNewMissingTemplate,
    ComponentMissingSubcommandHelp,
    WorkerInvokeMissingWorkerName,
    WorkerInvokeMissingFunctionName { worker_name: WorkerName },
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
    /// Invoke and manage workers
    Worker {
        #[clap(subcommand)]
        subcommand: WorkerSubcommand,
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
    /// Manage CLI profiles
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
    /// Diagnose possible problems
    Diagnose,
    /// Generate shell completion
    Completion,
}

pub mod shared_args {
    use crate::model::{ComponentName, WorkerName};
    use clap::Args;
    use golem_templates::model::GuestLanguage;
    use golem_wasm_rpc_stubgen::model::app::AppBuildStep;

    pub type ComponentTemplateName = String;
    pub type NewWorkerArgument = String;
    pub type WorkerFunctionArgument = String;
    pub type WorkerFunctionName = String;

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
    pub struct ComponentOptionalComponentNames {
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
        /// Optional component names, if not specified all components are selected.
        pub component_name: Vec<ComponentName>,
    }

    #[derive(Debug, Args)]
    pub struct LanguageArg {
        #[clap(long, short)]
        pub language: GuestLanguage,
    }

    #[derive(Debug, Args)]
    pub struct ComponentTemplatePositionalArg {
        /// Component template name to be used for the component
        pub component_template: ComponentTemplateName,
    }

    #[derive(Debug, Args)]
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
    pub struct WorkerNameArg {
        // DO NOT ADD EMPTY LINES TO THE DOC COMMENT
        /// Worker name, accepted formats:
        ///   - <WORKER>
        ///   - <COMPONENT>/<WORKER>
        ///   - <PROJECT>/<COMPONENT>/<WORKER>
        ///   - <ACCOUNT>/<PROJECT>/<COMPONENT>/<WORKER>
        #[arg(verbatim_doc_comment)]
        pub worker_name: WorkerName,
    }
}

pub mod app {
    use crate::command::shared_args::{AppOptionalComponentNames, BuildArgs, ForceBuildArg};
    use clap::Subcommand;
    use golem_templates::model::GuestLanguage;

    #[derive(Debug, Subcommand)]
    pub enum AppSubcommand {
        /// Create new application
        New {
            /// Application folder name where the new application should be created
            application_name: String,
            /// Languages that the application should support
            #[arg(required = true)]
            language: Vec<GuestLanguage>,
        },
        /// Build all or selected components in the application
        Build {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
            #[command(flatten)]
            build: BuildArgs,
        },
        /// Deploy all or selected components in the application, includes building
        Deploy {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
            #[command(flatten)]
            force_build: ForceBuildArg,
        },
        /// Clean all components in the application or by selection
        Clean {
            #[command(flatten)]
            component_name: AppOptionalComponentNames,
        },
        /// Run custom command
        #[clap(external_subcommand)]
        CustomCommand(Vec<String>),
    }
}

pub mod component {
    use crate::command::shared_args::{
        BuildArgs, ComponentOptionalComponentName, ComponentOptionalComponentNames,
        ComponentTemplatePositionalArg, ForceBuildArg,
    };
    use clap::Subcommand;
    use golem_templates::model::PackageName;

    #[derive(Debug, Subcommand)]
    pub enum ComponentSubcommand {
        /// Create new component in the current application
        New {
            #[command(flatten)]
            template: ComponentTemplatePositionalArg,
            /// Name of the new component package in 'package:name' form
            component_package_name: PackageName,
        },
        /// Build component(s) based on the current directory or by selection
        Build {
            #[command(flatten)]
            component_name: ComponentOptionalComponentNames,
            #[command(flatten)]
            build: BuildArgs,
        },
        // TODO: update-mode, try-update, redeploy
        /// Deploy component(s) based on the current directory or by selection
        Deploy {
            #[command(flatten)]
            component_name: ComponentOptionalComponentNames,
            #[command(flatten)]
            force_build: ForceBuildArg,
        },
        /// Clean component(s) based on the current directory or by selection
        Clean {
            #[command(flatten)]
            component_name: ComponentOptionalComponentNames,
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
    }
}

pub mod worker {
    use crate::command::parse_cursor;
    use crate::command::parse_key_val;
    use crate::command::shared_args::{
        ComponentOptionalComponentName, NewWorkerArgument, WorkerFunctionArgument,
        WorkerFunctionName, WorkerNameArg,
    };
    use crate::model::IdempotencyKey;
    use clap::Subcommand;
    use golem_client::model::ScanCursor;

    #[derive(Debug, Subcommand)]
    pub enum WorkerSubcommand {
        /// Create new worker
        New {
            #[command(flatten)]
            worker_name: WorkerNameArg,
            /// Worker arguments
            arguments: Vec<NewWorkerArgument>,
            /// Worker environment variables
            #[arg(short, long, value_parser = parse_key_val, value_name = "ENV=VAL")]
            env: Vec<(String, String)>,
        },
        // TODO: json args
        // TODO: connect
        /// Invoke (or enqueue invocation for) worker
        Invoke {
            #[command(flatten)]
            worker_name: WorkerNameArg,
            /// Worker function name to invoke
            function_name: WorkerFunctionName,
            /// Worker function arguments in WAVE format
            arguments: Vec<WorkerFunctionArgument>,
            /// Enqueue invocation, and do not wait for it
            #[clap(long, short, default_value = "false")]
            enqueue: bool,
            /// Set idempotency key for the call, use "-" for auto generated key
            #[clap(long, short)]
            idempotency_key: Option<IdempotencyKey>,
        },
        /// Get worker metadata
        Get {
            #[command(flatten)]
            worker_name: WorkerNameArg,
        },
        /// List worker metadata
        List {
            #[command(flatten)]
            component_name: ComponentOptionalComponentName,

            /// Filter for worker metadata in form of `property op value`.
            ///
            /// Filter examples: `name = worker-name`, `version >= 0`, `status = Running`, `env.var1 = value`.
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

            /// The maximum the number of returned workers, returns all values is not specified.
            /// When multiple component is selected, then the limit it is applied separately
            #[arg(long, short)]
            max_count: Option<u64>,

            /// When set to true it queries for most up-to-date status for each worker, default is false
            #[arg(long, default_value_t = false)]
            precise: bool,
        },
    }
}

pub mod api {
    use crate::command::api::cloud::ApiCloudSubcommand;
    use crate::command::api::definition::ApiDefinitionSubcommand;
    use crate::command::api::deployment::ApiDeploymentSubcommand;
    use crate::command::api::security_scheme::ApiSecuritySchemeSubcommand;
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum ApiSubcommand {
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
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiDefinitionSubcommand {}
    }

    pub mod deployment {
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiDeploymentSubcommand {}
    }

    pub mod security_scheme {
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum ApiSecuritySchemeSubcommand {}
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
            use clap::Subcommand;

            #[derive(Debug, Subcommand)]
            pub enum ApiDomainSubcommand {}
        }

        pub mod certificate {
            use clap::Subcommand;

            #[derive(Debug, Subcommand)]
            pub enum ApiCertificateSubcommand {}
        }
    }
}

pub mod plugin {
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum PluginSubcommand {}
}

pub mod profile {
    use crate::command::profile::config::ProfileConfigSubcommand;
    use crate::config::ProfileName;
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum ProfileSubcommand {
        /// Creates new profile
        #[command()]
        New {
            /// Create the new profile interactively
            #[arg(short, long)]
            interactive: bool,

            /// Switch to the profile after creation
            #[arg(short, long)]
            set_active: bool,
            // TODO: add args
        },

        /// List profiles
        #[command()]
        List {},

        /// Set the active default profile
        #[command()]
        Switch {
            /// Profile name to switch to
            profile_name: ProfileName,
        },

        /// Show profile details
        #[command()]
        Get {
            /// Name of profile to show, shows active profile if not specified.
            name: Option<ProfileName>,
        },

        /// Remove profile
        #[command()]
        Delete {
            /// Profile name to delete
            profile_name: ProfileName,
        },

        /// Profile config
        #[command()]
        Config {
            /// Profile name. Default value - active profile.
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
    use crate::command::cloud::auth_token::AuthTokenSubcommand;
    use crate::command::cloud::project::ProjectSubcommand;
    use clap::Subcommand;

    #[derive(Debug, Subcommand)]
    pub enum CloudSubcommand {
        /// Manage Cloud Auth Tokens
        AuthToken {
            #[clap(subcommand)]
            subcommand: AuthTokenSubcommand,
        },
        /// Manage Cloud Account
        Account {
            #[clap(subcommand)]
            subcommand: AccountSubcommand,
        },
        /// Manage Cloud Projects
        Project {
            #[clap(subcommand)]
            subcommand: ProjectSubcommand,
        },
    }

    pub mod auth_token {
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum AuthTokenSubcommand {}
    }

    pub mod account {
        use clap::Subcommand;

        #[derive(Debug, Subcommand)]
        pub enum AccountSubcommand {}
    }

    pub mod project {
        use crate::model::ProjectName;
        use clap::Subcommand;

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
        }
    }
}

pub mod server {
    use clap::Subcommand;
    use std::path::PathBuf;

    #[derive(Debug, Subcommand)]
    pub enum ServerSubcommand {
        /// Run golem server for local development
        Run {
            /// Address to serve the main API on
            #[clap(long, default_value = "0.0.0.0")]
            router_addr: String,

            /// Port to serve the main API on
            #[clap(long, default_value_t = 9881)]
            router_port: u16,

            /// Port to serve custom requests on
            #[clap(long, default_value_t = 9006)]
            custom_request_port: u16,

            /// Directory to store data in. Defaults to $XDG_STATE_HOME/golem
            #[clap(long)]
            data_dir: Option<PathBuf>,

            /// Clean the data directory before starting
            #[clap(long, default_value = "false")]
            clean: bool,
        },
        /// Clean the local server data directory
        Clean,
    }
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

#[cfg(test)]
mod test {
    use crate::command::GolemCliCommand;
    use assert2::assert;
    use clap::builder::StyledStr;
    use clap::{Command, CommandFactory};
    use itertools::Itertools;
    use std::collections::{BTreeMap, BTreeSet};
    use test_r::test;

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
                .map(|(error, matcher)| format!("error: {}\nmatcher: {:?}\n", error, matcher))
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
                .map(|e| format!("{:?}", e))
                .join("\n")
        );
    }
}
