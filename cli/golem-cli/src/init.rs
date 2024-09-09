// Copyright 2024 Golem Cloud
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

use crate::command::api_definition::ApiDefinitionSubcommand;
use crate::command::api_deployment::ApiDeploymentSubcommand;
use crate::command::component::ComponentSubCommand;
use crate::command::profile::{ProfileSubCommand, UniversalProfileAdd};
use crate::command::worker::{OssWorkerUriArg, WorkerSubcommand};
use crate::config::{CloudProfile, Config, OssProfile, Profile, ProfileConfig, ProfileName};
use crate::diagnose::diagnose;
use crate::model::{ComponentUriArg, Format, GolemError, GolemResult};
use crate::oss::model::OssContext;
use crate::stubgen::handle_stubgen;
use crate::{diagnose, examples};
use async_trait::async_trait;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use golem_common::uri::oss::uri::ResourceUri;
use indoc::formatdoc;
use inquire::{Confirm, CustomType, Select};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::{ParseError, Url};

#[derive(Subcommand, Debug)]
#[command()]
pub enum InitCommand<ProfileAdd: clap::Args> {
    /// Create a new default profile and switch to it
    #[command()]
    Init {},

    /// Manage profiles
    #[command()]
    Profile {
        #[command(subcommand)]
        subcommand: ProfileSubCommand<ProfileAdd>,
    },

    /// Create a new Golem component from built-in examples
    #[command(flatten)]
    Examples(golem_examples::cli::Command),

    /// WASM RPC stub generator
    #[cfg(feature = "stubgen")]
    Stubgen {
        #[command(subcommand)]
        subcommand: golem_wasm_rpc_stubgen::Command,
    },

    /// Upload and manage Golem components
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubCommand<OssContext, ComponentUriArg>,
    },

    /// Manage Golem workers
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand<ComponentUriArg, OssWorkerUriArg>,
    },

    /// Get resource by URI
    ///
    /// Use resource URN or URL to get resource metadata.
    #[command()]
    Get {
        #[arg(value_name = "URI")]
        uri: ResourceUri,
    },

    /// Manage Golem api definitions
    #[command()]
    ApiDefinition {
        #[command(subcommand)]
        subcommand: ApiDefinitionSubcommand<OssContext>,
    },

    /// Manage Golem api deployments
    #[command()]
    ApiDeployment {
        #[command(subcommand)]
        subcommand: ApiDeploymentSubcommand<OssContext>,
    },

    /// Generate shell completions
    #[command()]
    Completion {
        #[arg(long = "generate", value_enum)]
        generator: clap_complete::Shell,
    },

    /// Diagnose required tooling
    #[command()]
    Diagnose {
        #[command(flatten)]
        command: diagnose::cli::Command,
    },
}

#[derive(Parser, Debug)]
#[command(author, version = option_env ! ("VERSION").unwrap_or(env ! ("CARGO_PKG_VERSION")), about = "Your Golem is not configured. Please run `golem-cli init`", long_about = None, rename_all = "kebab-case")]
/// Your Golem is not configured. Please run `golem-cli init`
pub struct GolemInitCommand<ProfileAdd: clap::Args> {
    #[command(flatten)]
    pub verbosity: Verbosity,

    #[arg(short = 'F', long, global = true, default_value = "text")]
    pub format: Format,

    #[command(subcommand)]
    pub command: InitCommand<ProfileAdd>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum CliKind {
    Universal,
    Golem,
    Cloud,
}

#[async_trait]
pub trait ProfileAuth {
    fn auth_enabled(&self) -> bool;
    async fn auth(&self, profile_name: &ProfileName, config_dir: &Path) -> Result<(), GolemError>;
}

pub struct DummyProfileAuth {}

#[async_trait]
impl ProfileAuth for DummyProfileAuth {
    fn auth_enabled(&self) -> bool {
        false
    }

    async fn auth(
        &self,
        _profile_name: &ProfileName,
        _config_dir: &Path,
    ) -> Result<(), GolemError> {
        Ok(())
    }
}

pub trait PrintCompletion {
    fn print_completion(&self, shell: clap_complete::Shell);
}

pub async fn async_main<ProfileAdd: Into<UniversalProfileAdd> + clap::Args>(
    cmd: GolemInitCommand<ProfileAdd>,
    cli_kind: CliKind,
    config_dir: PathBuf,
    profile_auth: Box<dyn ProfileAuth + Send + Sync + 'static>,
    print_completion: Box<dyn PrintCompletion>,
) -> Result<(), Box<dyn std::error::Error>> {
    let res = match cmd.command {
        InitCommand::Init {} => {
            let profile_name = ProfileName::default(cli_kind);

            let res =
                init_profile(cli_kind, profile_name, &config_dir, profile_auth.as_ref()).await?;

            if res.auth_required {
                profile_auth.auth(&res.profile_name, &config_dir).await?
            }

            Ok(GolemResult::Str("Profile created".to_string()))
        }
        InitCommand::Profile { subcommand } => {
            subcommand
                .handle(cli_kind, &config_dir, profile_auth.as_ref())
                .await
        }
        InitCommand::Examples(golem_examples::cli::Command::New {
            name_or_language,
            package_name,
            component_name,
        }) => examples::process_new(
            name_or_language.example_name(),
            component_name,
            package_name,
        ),
        InitCommand::Examples(golem_examples::cli::Command::ListExamples {
            min_tier,
            language,
        }) => examples::process_list_examples(min_tier, language),
        InitCommand::Completion { generator } => {
            print_completion.print_completion(generator);
            Ok(GolemResult::Str("".to_string()))
        }
        InitCommand::Diagnose { command } => {
            diagnose(command);
            Ok(GolemResult::Str("".to_string()))
        }
        #[cfg(feature = "stubgen")]
        InitCommand::Stubgen { subcommand } => handle_stubgen(subcommand).await,
        _ => Err(GolemError(
            "Your Golem CLI is not configured. Please run `golem-cli init`".to_owned(),
        )),
    };

    match res {
        Ok(res) => {
            res.print(cmd.format);
            Ok(())
        }
        Err(err) => Err(Box::new(err)),
    }
}

fn validate_profile_override(
    profile_name: &ProfileName,
    config_dir: &Path,
) -> Result<(), GolemError> {
    if Config::get_profile(profile_name, config_dir).is_some() {
        let question = formatdoc!(
            "Profile '{}' already exists.
            Do you want to override it?",
            profile_name.to_string().bold()
        );

        let ans = Confirm::new(&question)
            .with_default(false)
            .prompt()
            .map_err(|err| GolemError(format!("Unexpected error: {err}")))?;

        if ans {
            Ok(())
        } else {
            Err(GolemError("Profile creation was interrupted.".to_string()))
        }
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, EnumIter)]
enum ProfileType {
    OssDefaultCompose,
    OssCustom,
    Cloud,
}

impl Display for ProfileType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileType::OssDefaultCompose => write!(
                f,
                "{}. For default docker compose configuration",
                "Golem Default".bold()
            ),
            ProfileType::OssCustom => write!(
                f,
                "{}. For stand-alone installations with custom configuration",
                "Golem".bold()
            ),
            ProfileType::Cloud => write!(
                f,
                "{}. To use cloud version provided by https://golem.cloud/",
                "Golem Cloud".bold()
            ),
        }
    }
}

fn select_type() -> Result<ProfileType, GolemError> {
    let options = ProfileType::iter().collect::<Vec<_>>();
    Select::new("Select profile type:", options)
        .prompt()
        .map_err(|err| GolemError(format!("Unexpected error: {err}")))
}

fn select_oss_type() -> Result<ProfileType, GolemError> {
    let options = vec![ProfileType::OssDefaultCompose, ProfileType::OssCustom];
    Select::new("Select profile type:", options)
        .prompt()
        .map_err(|err| GolemError(format!("Unexpected error: {err}")))
}

#[derive(Debug, Copy, Clone, EnumIter)]
enum InitFormat {
    Text,
    Yaml,
    Json,
}

impl From<InitFormat> for Format {
    fn from(value: InitFormat) -> Self {
        match value {
            InitFormat::Text => Format::Text,
            InitFormat::Yaml => Format::Yaml,
            InitFormat::Json => Format::Json,
        }
    }
}

impl Display for InitFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            InitFormat::Text => "text",
            InitFormat::Yaml => "yaml",
            InitFormat::Json => "json",
        };

        write!(f, "{}", name)
    }
}

impl FromStr for InitFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(InitFormat::Json),
            "yaml" => Ok(InitFormat::Yaml),
            "text" => Ok(InitFormat::Text),
            _ => {
                let all = InitFormat::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown format: {s}. Expected one of {all}"))
            }
        }
    }
}

fn make_profile_config() -> Result<ProfileConfig, GolemError> {
    let options = InitFormat::iter().collect::<Vec<_>>();
    let default_format = Select::new("Default output format:", options)
        .prompt()
        .map_err(|err| GolemError(format!("Unexpected error: {err}")))?
        .into();

    Ok(ProfileConfig { default_format })
}

fn make_cloud_profile() -> Result<Profile, GolemError> {
    let mut profile = CloudProfile::default();

    let config = make_profile_config()?;

    profile.config = config;

    Ok(Profile::GolemCloud(profile))
}

fn set_active_profile(
    cli_kind: CliKind,
    profile_name: &ProfileName,
    config_dir: &Path,
) -> Result<(), GolemError> {
    let active_profile = Config::get_active_profile(cli_kind, config_dir).map(|p| p.name);

    match active_profile {
        None => Config::set_active_profile_name(profile_name.clone(), cli_kind, config_dir),
        Some(active_profile) if &active_profile == profile_name => Ok(()),
        Some(active_profile) => {
            let question = formatdoc!(
                "
                Current active profile is '{active_profile}'.
                Do you want to switch active profile to '{profile_name}'?
                "
            );

            let ans = Confirm::new(&question)
                .with_default(true)
                .prompt()
                .map_err(|err| GolemError(format!("Unexpected error: {err}")))?;

            if ans {
                Config::set_active_profile_name(profile_name.clone(), cli_kind, config_dir)
            } else {
                Ok(())
            }
        }
    }
}

async fn ask_auth_cloud() -> Result<bool, GolemError> {
    let res = Confirm::new("Do you want to log in to Golem Cloud right now?")
        .with_default(false)
        .with_help_message("You can safely skip this and log in to Golem Cloud later by calling any command that requires authentication.")
        .prompt()
        .map_err(|err| GolemError(format!("Unexpected error: {err}")))?;

    Ok(res)
}

fn ask_for_component_url() -> Result<Url, GolemError> {
    CustomType::<Url>::new("Component service URL:")
        .with_error_message(&format!(
            "Please type a valid URL. For instance: {DEFAULT_OSS_URL}"
        ))
        .prompt()
        .map_err(|err| GolemError(format!("Unexpected error: {err}")))
}

#[derive(Debug, Clone)]
struct OptionalUrl(Option<Url>);

impl Display for OptionalUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            None => Ok(()),
            Some(url) => write!(f, "{}", url),
        }
    }
}

impl FromStr for OptionalUrl {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            Ok(OptionalUrl(None))
        } else {
            Ok(OptionalUrl(Some(Url::from_str(s)?)))
        }
    }
}

fn ask_for_worker_url() -> Result<Option<Url>, GolemError> {
    CustomType::<OptionalUrl>::new("Worker service URL (empty to use component service url):")
        .with_error_message("Please type a valid URL. For instance: http://localhost:9876")
        .prompt()
        .map(|o| o.0)
        .map_err(|err| GolemError(format!("Unexpected error: {err}")))
}

fn make_oss_custom_profile() -> Result<Profile, GolemError> {
    let url = ask_for_component_url()?;
    let worker_url = ask_for_worker_url()?;

    let help = formatdoc!(
        "Disables certificate validation.
        Warning! Any certificate will be trusted for use. This includes expired certificates.
        This introduces significant vulnerabilities, and should only be used as a last resort."
    );

    let allow_insecure = Confirm::new("Accept invalid certificates?")
        .with_default(false)
        .with_help_message(&help)
        .prompt()
        .map_err(|err| GolemError(format!("Unexpected error: {err}")))?;

    let config = make_profile_config()?;

    Ok(Profile::Golem(OssProfile {
        url,
        worker_url,
        allow_insecure,
        config,
    }))
}

pub const DEFAULT_OSS_URL: &str = "http://localhost:9881";

fn make_oss_default_profile() -> Result<Profile, GolemError> {
    let url = Url::parse(DEFAULT_OSS_URL).unwrap();

    let config = make_profile_config()?;

    Ok(Profile::Golem(OssProfile {
        url,
        worker_url: None,
        allow_insecure: false,
        config,
    }))
}

pub struct InitResult {
    pub profile_name: ProfileName,
    pub auth_required: bool,
}

pub async fn init_profile(
    cli_kind: CliKind,
    profile_name: ProfileName,
    config_dir: &Path,
    profile_auth: &(dyn ProfileAuth + Send + Sync),
) -> Result<InitResult, GolemError> {
    validate_profile_override(&profile_name, config_dir)?;
    let typ = match cli_kind {
        CliKind::Universal => select_type()?,
        CliKind::Golem => select_oss_type()?,
        CliKind::Cloud => ProfileType::Cloud,
    };

    let profile = match typ {
        ProfileType::OssDefaultCompose => make_oss_default_profile()?,
        ProfileType::OssCustom => make_oss_custom_profile()?,
        ProfileType::Cloud => make_cloud_profile()?,
    };

    Config::set_profile(profile_name.clone(), profile, config_dir)?;

    set_active_profile(cli_kind, &profile_name, config_dir)?;

    let auth_required = if let ProfileType::Cloud = typ {
        if profile_auth.auth_enabled() {
            ask_auth_cloud().await?
        } else {
            false
        }
    } else {
        false
    };

    Ok(InitResult {
        profile_name,
        auth_required,
    })
}
