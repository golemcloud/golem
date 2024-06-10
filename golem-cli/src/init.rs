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

use crate::cloud::factory::CloudServiceFactory;
use crate::command::profile::ProfileSubCommand;
use crate::config::{CloudProfile, Config, OssProfile, Profile, ProfileConfig, ProfileName};
use crate::examples;
use crate::model::{Format, GolemError, GolemResult};
use crate::stubgen::handle_stubgen;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier, PackageName};
use indoc::formatdoc;
use inquire::{Confirm, CustomType, Select};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::{ParseError, Url};

#[derive(Subcommand, Debug)]
#[command()]
pub enum InitCommand {
    /// Create a new default profile and switch to it
    #[command()]
    Init {},

    /// Manage profiles
    #[command()]
    Profile {
        #[command(subcommand)]
        subcommand: ProfileSubCommand,
    },

    /// Create a new Golem component from built-in examples
    #[command()]
    New {
        /// Name of the example to use
        #[arg(short, long)]
        example: ExampleName,

        /// The new component's name
        #[arg(short, long)]
        component_name: golem_examples::model::ComponentName,

        /// The package name of the generated component (in namespace:name format)
        #[arg(short, long)]
        package_name: Option<PackageName>,
    },

    /// Lists the built-in examples available for creating new components
    #[command()]
    ListExamples {
        /// The minimum language tier to include in the list
        #[arg(short, long)]
        min_tier: Option<GuestLanguageTier>,

        /// Filter examples by a given guest language
        #[arg(short, long)]
        language: Option<GuestLanguage>,
    },

    /// WASM RPC stub generator
    #[cfg(feature = "stubgen")]
    Stubgen {
        #[command(subcommand)]
        subcommand: golem_wasm_rpc_stubgen::Command,
    },
}

#[derive(Parser, Debug)]
#[command(author, version = option_env ! ("VERSION").unwrap_or(env ! ("CARGO_PKG_VERSION")), about = "Your Golem is not configured. Please run `golem-cli init`", long_about = None, rename_all = "kebab-case")]
/// Your Golem is not configured. Please run `golem-cli init`
pub struct GolemInitCommand {
    #[command(flatten)]
    pub verbosity: Verbosity,

    #[arg(short = 'F', long, default_value = "text")]
    pub format: Format,

    #[command(subcommand)]
    pub command: InitCommand,
}

pub async fn async_main(
    cmd: GolemInitCommand,
    config_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let res = match cmd.command {
        InitCommand::Init {} => init_profile(None, &config_dir).await,
        InitCommand::Profile { subcommand } => subcommand.handle(&config_dir).await,
        InitCommand::New {
            example,
            package_name,
            component_name,
        } => examples::process_new(example, component_name, package_name),
        InitCommand::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
        #[cfg(feature = "stubgen")]
        InitCommand::Stubgen { subcommand } => handle_stubgen(subcommand).await,
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
    profile_name: &Option<ProfileName>,
    config_dir: &Path,
) -> Result<(), GolemError> {
    let profile_name = profile_name
        .clone()
        .filter(|n| n != &ProfileName::default());

    if Config::get_profile(&profile_name.clone().unwrap_or_default(), config_dir).is_some() {
        let profile_ref = if let Some(name) = profile_name {
            format!("Profile '{}'", name.to_string().bold())
        } else {
            "Default profile".to_string()
        };

        let question = formatdoc!(
            "{profile_ref} already exists.
            Do you want to override it?"
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
    Golem,
    GolemCloud,
}

impl Display for ProfileType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileType::Golem => write!(f, "{}. For stand-alone installations", "Golem".bold()),
            ProfileType::GolemCloud => write!(
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
    profile_name: &Option<ProfileName>,
    config_dir: &Path,
) -> Result<(), GolemError> {
    let profile_name = profile_name.clone().unwrap_or_default();

    let active_profile = Config::get_active_profile(config_dir).map(|p| p.name);

    match active_profile {
        None => Config::set_active_profile_name(profile_name, config_dir),
        Some(active_profile) if active_profile == profile_name => Ok(()),
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
                Config::set_active_profile_name(profile_name, config_dir)
            } else {
                Ok(())
            }
        }
    }
}

async fn auth_cloud(profile_name: &ProfileName, config_dir: &Path) -> Result<(), GolemError> {
    let ans = Confirm::new("Do you want to log in to Golem Cloud right now?")
        .with_default(false)
        .with_help_message("You can safely skip this and log in to Golem Cloud later by calling any command that requires authentication.")
        .prompt()
        .map_err(|err| GolemError(format!("Unexpected error: {err}")))?;

    if ans {
        let profile = Config::get_profile(profile_name, config_dir)
            .ok_or(GolemError("Can't find created profile.".to_string()))?;

        let profile = if let Profile::GolemCloud(profile) = profile {
            profile
        } else {
            return Err(GolemError("Unexpected profile type.".to_string()));
        };

        let factory = CloudServiceFactory::from_profile(&profile);

        let _ = factory
            .auth()?
            .authenticate(None, profile_name, &profile, config_dir)
            .await?;

        Ok(())
    } else {
        Ok(())
    }
}

fn ask_for_component_url() -> Result<Url, GolemError> {
    CustomType::<Url>::new("Component service URL:")
        .with_error_message("Please type a valid URL. For instance: http://localhost:9876")
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

fn make_oss_profile() -> Result<Profile, GolemError> {
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

pub async fn init_profile(
    profile_name: Option<ProfileName>,
    config_dir: &Path,
) -> Result<GolemResult, GolemError> {
    validate_profile_override(&profile_name, config_dir)?;
    let typ = select_type()?;

    let profile = match typ {
        ProfileType::Golem => make_oss_profile()?,
        ProfileType::GolemCloud => make_cloud_profile()?,
    };

    Config::set_profile(
        profile_name.clone().unwrap_or_default(),
        profile,
        config_dir,
    )?;

    set_active_profile(&profile_name, config_dir)?;

    if let ProfileType::GolemCloud = typ {
        auth_cloud(&profile_name.clone().unwrap_or_default(), config_dir).await?
    }

    Ok(GolemResult::Str("Profile created.".to_string()))
}
