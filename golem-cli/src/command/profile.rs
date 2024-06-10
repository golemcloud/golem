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

use crate::config::{
    CloudProfile, Config, NamedProfile, OssProfile, Profile, ProfileConfig, ProfileName,
};
use crate::model::text::TextFormat;
use crate::model::{Format, GolemError, GolemResult};
use clap::Subcommand;
use colored::Colorize;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::path::Path;
use url::Url;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProfileAddSubCommand {
    /// Creates a new cloud profile
    #[command()]
    GolemCloud {
        /// Name of the newly created profile
        #[arg(value_name = "profile-name")]
        name: ProfileName,

        /// URL of Golem Component service
        #[arg(long, hide = true)]
        dev_component_url: Option<Url>,

        /// URL of Golem Worker service. If not provided - use component-url.
        #[arg(long, hide = true)]
        dev_worker_url: Option<Url>,

        /// Accept invalid certificates.
        ///
        /// Disables certificate validation.
        /// Warning! Any certificate will be trusted for use.
        /// This includes expired certificates.
        /// This introduces significant vulnerabilities, and should only be used as a last resort.
        #[arg(long, default_value_t = false, hide = true)]
        dev_allow_insecure: bool,

        /// Default output format
        #[arg(short = 'f', long, default_value_t = Format::Text)]
        default_format: Format,
    },

    /// Creates a new standalone profile
    #[command()]
    Golem {
        /// Name of the newly created profile
        #[arg(value_name = "profile-name")]
        name: ProfileName,

        /// URL of Golem Component service
        #[arg(short, long)]
        component_url: Url,

        /// URL of Golem Worker service. If not provided - use component-url.
        #[arg(short, long)]
        worker_url: Option<Url>,

        /// Accept invalid certificates.
        ///
        /// Disables certificate validation.
        /// Warning! Any certificate will be trusted for use.
        /// This includes expired certificates.
        /// This introduces significant vulnerabilities, and should only be used as a last resort.
        #[arg(short, long, default_value_t = false)]
        allow_insecure: bool,

        /// Default output format
        #[arg(short = 'f', long, default_value_t = Format::Text)]
        default_format: Format,
    },
}

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProfileConfigSubCommand {
    /// Show current config
    #[command()]
    Show {},

    /// Default format
    #[command()]
    Format {
        /// Default output format
        #[arg(value_name = "default-format")]
        default_format: Format,
    },
}

#[derive(Subcommand, Debug)]
#[command()]
pub enum ProfileSubCommand {
    /// Creates a new idle profile
    #[command()]
    Add {
        /// Switch to created profile.
        #[arg(short, long, default_value_t = false)]
        set_active: bool,

        #[command(subcommand)]
        subcommand: ProfileAddSubCommand,
    },

    /// Interactively creates a new profile
    #[command()]
    Init {
        /// Name of the newly created profile
        #[arg(value_name = "profile-name")]
        name: ProfileName,
    },

    /// List all local profiles
    #[command()]
    List {},

    /// Switch active profile
    #[command()]
    Switch {
        /// Name of profile to use
        #[arg(value_name = "profile-name")]
        name: ProfileName,
    },

    /// Show profile details
    #[command()]
    Get {
        /// Name of profile to show. Use active profile if not specified.
        #[arg(value_name = "profile-name")]
        name: Option<ProfileName>,
    },

    /// Remove profile
    #[command()]
    Delete {
        /// Name of profile to remove
        #[arg(value_name = "profile-name")]
        name: ProfileName,
    },

    /// Profile config
    #[command()]
    Config {
        /// Profile name. Default value - active profile.
        #[arg(short, long, value_name = "profile-name")]
        profile: Option<ProfileName>,

        #[command(subcommand)]
        subcommand: ProfileConfigSubCommand,
    },
}

impl ProfileConfigSubCommand {
    fn handle(
        self,
        profile_name: Option<ProfileName>,
        config_dir: &Path,
    ) -> Result<GolemResult, GolemError> {
        match self {
            ProfileConfigSubCommand::Show {} => {
                let profile = match profile_name {
                    None => {
                        Config::get_active_profile(config_dir)
                            .ok_or(GolemError(
                                "No active profile. Please run `golem-cli init`".to_string(),
                            ))?
                            .profile
                    }
                    Some(profile) => Config::get_profile(&profile, config_dir)
                        .ok_or(GolemError(format!("Can't find profile {profile}")))?,
                };

                Ok(GolemResult::Ok(Box::new(profile.config())))
            }
            ProfileConfigSubCommand::Format { default_format } => {
                let NamedProfile { name, mut profile } = match profile_name {
                    None => Config::get_active_profile(config_dir).ok_or(GolemError(
                        "No active profile. Please run `golem-cli init`".to_string(),
                    ))?,
                    Some(profile_name) => {
                        let profile = Config::get_profile(&profile_name, config_dir)
                            .ok_or(GolemError(format!("Can't find profile {profile_name}")))?;
                        NamedProfile {
                            name: profile_name,
                            profile,
                        }
                    }
                };

                profile.get_config_mut().default_format = default_format;

                Config::set_profile(name, profile, config_dir)?;

                Ok(GolemResult::Str("Default format updated".to_string()))
            }
        }
    }
}

impl ProfileAddSubCommand {
    fn handle(self, set_active: bool, config_dir: &Path) -> Result<GolemResult, GolemError> {
        match self {
            ProfileAddSubCommand::Golem {
                name,
                component_url,
                worker_url,
                allow_insecure,
                default_format,
            } => {
                let profile = Profile::Golem(OssProfile {
                    url: component_url,
                    worker_url,
                    allow_insecure,
                    config: ProfileConfig { default_format },
                });

                Config::set_profile(name.clone(), profile, config_dir)?;

                if set_active {
                    Config::set_active_profile_name(name, config_dir)?;
                }

                Ok(GolemResult::Str("Profile created".to_string()))
            }
            ProfileAddSubCommand::GolemCloud {
                name,
                dev_component_url,
                dev_worker_url,
                dev_allow_insecure,
                default_format,
            } => {
                let profile = Profile::GolemCloud(CloudProfile {
                    custom_url: dev_component_url,
                    custom_worker_url: dev_worker_url,
                    allow_insecure: dev_allow_insecure,
                    config: ProfileConfig { default_format },
                    auth: None,
                });

                Config::set_profile(name.clone(), profile, config_dir)?;

                if set_active {
                    Config::set_active_profile_name(name, config_dir)?;
                }

                Ok(GolemResult::Str("Profile created".to_string()))
            }
        }
    }
}

impl ProfileSubCommand {
    pub async fn handle(self, config_dir: &Path) -> Result<GolemResult, GolemError> {
        match self {
            ProfileSubCommand::Add {
                set_active,
                subcommand,
            } => subcommand.handle(set_active, config_dir),
            ProfileSubCommand::Init { name } => {
                crate::init::init_profile(Some(name), config_dir).await
            }
            ProfileSubCommand::List {} => {
                let Config {
                    profiles,
                    active_profile,
                } = Config::read_from_file(config_dir);
                let active_profile = active_profile.unwrap_or_default();

                let res = profiles
                    .into_iter()
                    .sorted_by_key(|(n, _)| n.clone())
                    .map(|(name, profile)| NamedProfile { name, profile })
                    .map(|p| ProfileView::from_profile(&active_profile, p))
                    .collect::<Vec<_>>();

                Ok(GolemResult::Ok(Box::new(res)))
            }
            ProfileSubCommand::Switch { name } => {
                Config::set_active_profile_name(name, config_dir)?;

                Ok(GolemResult::Str("Active profile updated".to_string()))
            }
            ProfileSubCommand::Get { name } => {
                let profile = match name {
                    None => Config::get_active_profile(config_dir)
                        .ok_or(GolemError("Can't find active profile".to_string()))?,
                    Some(name) => {
                        let profile = Config::get_profile(&name, config_dir)
                            .ok_or(GolemError(format!("Can't find profile '{name}'")))?;

                        NamedProfile { name, profile }
                    }
                };

                let active = Config::get_active_profile(config_dir).map(|p| p.name);

                Ok(GolemResult::Ok(Box::new(ProfileView::from_profile(
                    &active.unwrap_or_default(),
                    profile,
                ))))
            }
            ProfileSubCommand::Delete { name } => {
                Config::delete_profile(&name, config_dir)?;

                Ok(GolemResult::Str("Profile removed".to_string()))
            }
            ProfileSubCommand::Config {
                profile,
                subcommand,
            } => subcommand.handle(profile, config_dir),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProfileView {
    pub is_active: bool,
    pub name: ProfileName,
    pub typ: ProfileType,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_url: Option<Url>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub allow_insecure: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authenticated: Option<bool>,
    pub config: ProfileConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum ProfileType {
    Golem,
    GolemCloud,
}

impl ProfileView {
    pub fn from_profile(active: &ProfileName, profile: NamedProfile) -> Self {
        let NamedProfile { name, profile } = profile;

        match profile {
            Profile::Golem(OssProfile {
                url,
                worker_url,
                allow_insecure,
                config,
            }) => ProfileView {
                is_active: &name == active,
                name,
                typ: ProfileType::Golem,
                url: Some(url),
                worker_url,
                allow_insecure,
                authenticated: None,
                config,
            },
            Profile::GolemCloud(CloudProfile {
                custom_url,
                custom_worker_url,
                allow_insecure,
                auth,
                config,
            }) => ProfileView {
                is_active: &name == active,
                name,
                typ: ProfileType::GolemCloud,
                url: custom_url,
                worker_url: custom_worker_url,
                allow_insecure,
                authenticated: Some(auth.is_some()),
                config,
            },
        }
    }
}

impl TextFormat for Vec<ProfileView> {
    fn print(&self) {
        let res = self
            .iter()
            .map(|p| {
                if p.is_active {
                    format!(" * {}", p.name.to_string().bold())
                } else {
                    format!("   {}", p.name)
                }
            })
            .join("\n");

        println!("{}", res)
    }
}

impl TextFormat for ProfileView {
    fn print(&self) {
        match self.typ {
            ProfileType::Golem => println!("Golem profile '{}':", self.name),
            ProfileType::GolemCloud => println!("Golem Cloud profile '{}':", self.name),
        }

        if let Some(url) = &self.url {
            if let Some(worker_url) = &self.worker_url {
                println!("Component service URL: {url}");
                println!("Worker service URL: {worker_url}")
            } else {
                println!("Service URL: {url}");
            }
        }

        if self.allow_insecure {
            println!("Accept invalid certificates!")
        }

        println!("Default output format: {}", self.config.default_format);

        println!("Active profile: {}", self.is_active);
    }
}
