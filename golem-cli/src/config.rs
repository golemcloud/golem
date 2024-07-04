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

use crate::cloud::CloudAuthenticationConfig;
use crate::init::CliKind;
use crate::model::text::TextFormat;
use crate::model::{Format, GolemError};
use derive_more::FromStr;
use indoc::printdoc;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use tracing::warn;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub profiles: HashMap<ProfileName, Profile>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_profile: Option<ProfileName>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_cloud_profile: Option<ProfileName>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, FromStr)]
pub struct ProfileName(pub String);

impl Display for ProfileName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ProfileName {
    pub fn default(cli_kind: CliKind) -> ProfileName {
        match cli_kind {
            CliKind::Universal | CliKind::Golem => ProfileName("default".to_string()),
            CliKind::Cloud => ProfileName("cloud_default".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedProfile {
    pub name: ProfileName,
    pub profile: Profile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Profile {
    Golem(OssProfile),
    GolemCloud(CloudProfile),
}

impl Profile {
    pub fn config(self) -> ProfileConfig {
        match self {
            Profile::Golem(p) => p.config,
            Profile::GolemCloud(p) => p.config,
        }
    }

    pub fn get_config(&self) -> &ProfileConfig {
        match self {
            Profile::Golem(p) => &p.config,
            Profile::GolemCloud(p) => &p.config,
        }
    }

    pub fn get_config_mut(&mut self) -> &mut ProfileConfig {
        match self {
            Profile::Golem(p) => &mut p.config,
            Profile::GolemCloud(p) => &mut p.config,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CloudProfile {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub custom_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub custom_cloud_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub custom_worker_url: Option<Url>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub allow_insecure: bool,
    #[serde(default)]
    pub config: ProfileConfig,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auth: Option<CloudAuthenticationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OssProfile {
    pub url: Url,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_url: Option<Url>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub allow_insecure: bool,
    #[serde(default)]
    pub config: ProfileConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, Eq, PartialEq)]
pub struct ProfileConfig {
    #[serde(default)]
    pub default_format: Format,
}

impl TextFormat for ProfileConfig {
    fn print(&self) {
        printdoc!(
            "
            Default output format: {}
            ",
            self.default_format
        )
    }
}

impl Config {
    fn config_path(config_dir: &Path) -> PathBuf {
        config_dir.join("config.json")
    }

    fn read_from_file_opt(config_dir: &Path) -> Option<Config> {
        let file = File::open(Self::config_path(config_dir)).ok()?;
        let reader = BufReader::new(file);

        let parsed: serde_json::Result<Config> = serde_json::from_reader(reader);

        match parsed {
            Ok(conf) => Some(conf),
            Err(err) => {
                warn!("Config parsing failed: {err}");
                None
            }
        }
    }

    pub fn read_from_file(config_dir: &Path) -> Config {
        Self::read_from_file_opt(config_dir).unwrap_or_default()
    }

    fn store_file(&self, config_dir: &Path) -> Result<(), GolemError> {
        create_dir_all(config_dir)
            .map_err(|err| GolemError(format!("Can't create config directory: {err}")))?;

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(Self::config_path(config_dir))
            .map_err(|err| GolemError(format!("Can't open config file: {err}")))?;
        let writer = BufWriter::new(file);

        serde_json::to_writer_pretty(writer, self)
            .map_err(|err| GolemError(format!("Can't save config to file: {err}")))
    }

    pub fn set_active_profile_name(
        profile_name: ProfileName,
        cli_kind: CliKind,
        config_dir: &Path,
    ) -> Result<(), GolemError> {
        let mut config = Self::read_from_file(config_dir);

        if let Some(profile) = config.profiles.get(&profile_name) {
            match profile {
                Profile::Golem(_) => {
                    if cli_kind == CliKind::Cloud {
                        return Err(GolemError(format!("Profile {profile_name} is not a Cloud profile. Use `golem-cli` instead of `golem-cloud-cli` for this profile.")));
                    }
                }
                Profile::GolemCloud(_) => {
                    if cli_kind == CliKind::Golem {
                        return Err(GolemError(format!("Profile {profile_name} is a Cloud profile. Use `golem-cloud-cli` instead of `golem-cli` for this profile. You can also install universal version of `golem-cli` using `cargo install golem-cloud-cli --features universal`")));
                    }
                }
            }
        } else {
            return Err(GolemError(format!(
                "No profile {profile_name} in configuration. Available profiles: [{}]",
                config.profiles.keys().map(|n| &n.0).join(", ")
            )));
        }

        match cli_kind {
            CliKind::Universal | CliKind::Golem => config.active_profile = Some(profile_name),
            CliKind::Cloud => config.active_cloud_profile = Some(profile_name),
        }

        config.store_file(config_dir)?;

        Ok(())
    }

    pub fn get_active_profile(cli_kind: CliKind, config_dir: &Path) -> Option<NamedProfile> {
        let mut config = Self::read_from_file(config_dir);

        let name = match cli_kind {
            CliKind::Universal | CliKind::Golem => config
                .active_profile
                .unwrap_or_else(|| ProfileName::default(cli_kind)),
            CliKind::Cloud => config
                .active_cloud_profile
                .unwrap_or_else(|| ProfileName::default(cli_kind)),
        };

        Some(NamedProfile {
            name: name.clone(),
            profile: config.profiles.remove(&name)?,
        })
    }

    pub fn get_profile(name: &ProfileName, config_dir: &Path) -> Option<Profile> {
        let mut config = Self::read_from_file(config_dir);

        config.profiles.remove(name)
    }

    pub fn set_profile(
        name: ProfileName,
        profile: Profile,
        config_dir: &Path,
    ) -> Result<(), GolemError> {
        let mut config = Self::read_from_file(config_dir);

        let _ = config.profiles.insert(name, profile);

        config.store_file(config_dir)
    }

    pub fn delete_profile(name: &ProfileName, config_dir: &Path) -> Result<(), GolemError> {
        let mut config = Self::read_from_file(config_dir);

        if &config
            .active_profile
            .clone()
            .unwrap_or_else(|| ProfileName::default(CliKind::Universal))
            == name
        {
            return Err(GolemError("Can't remove active profile".to_string()));
        }

        if &config
            .active_cloud_profile
            .clone()
            .unwrap_or_else(|| ProfileName::default(CliKind::Cloud))
            == name
        {
            return Err(GolemError("Can't remove active cloud profile".to_string()));
        }

        let _ = config
            .profiles
            .remove(name)
            .ok_or(GolemError(format!("Profile {name} not found")))?;

        config.store_file(config_dir)
    }
}
