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

use crate::model::Format;
use anyhow::{anyhow, bail, Context};
use chrono::{DateTime, Utc};
use golem_client::model::TokenSecret;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::{Display, Formatter};
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

pub const CLOUD_URL: &str = "https://release.api.golem.cloud";
pub const DEFAULT_OSS_URL: &str = "http://localhost:9881";
const PROFILE_NAME_LOCAL: &str = "local";
const PROFILE_NAME_CLOUD: &str = "cloud";
pub const LOCAL_WELL_KNOWN_TOKEN: Uuid = uuid::uuid!("5c832d93-ff85-4a8f-9803-513950fdfdb1");

// TODO: review and separate model, config and serialization parts
// TODO: when doing the serialization we can do a legacy migration

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub profiles: HashMap<ProfileName, Profile>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default_profile: Option<ProfileName>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct ProfileName(pub String);

impl ProfileName {
    pub fn local() -> Self {
        ProfileName(PROFILE_NAME_LOCAL.to_string())
    }

    pub fn cloud() -> Self {
        ProfileName(PROFILE_NAME_CLOUD.to_string())
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self.0.as_str(), PROFILE_NAME_LOCAL | PROFILE_NAME_CLOUD)
    }

    pub fn is_builtin_local(&self) -> bool {
        self.0.as_str() == PROFILE_NAME_LOCAL
    }

    pub fn is_builtin_cloud(&self) -> bool {
        self.0.as_str() == PROFILE_NAME_CLOUD
    }
}

impl Display for ProfileName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for ProfileName {
    fn from(name: &str) -> Self {
        Self(name.to_string())
    }
}

impl From<String> for ProfileName {
    fn from(name: String) -> Self {
        Self(name)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct BuildProfileName(pub String);

impl Display for BuildProfileName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for BuildProfileName {
    fn from(name: &str) -> Self {
        Self(name.to_string())
    }
}

impl From<String> for BuildProfileName {
    fn from(name: String) -> Self {
        Self(name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedProfile {
    pub name: ProfileName,
    pub profile: Profile,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
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
    pub auth: AuthenticationConfig,
}

impl Profile {
    pub fn default_local_profile() -> Self {
        let url = Url::parse(DEFAULT_OSS_URL).unwrap();
        Self {
            custom_url: Some(url),
            custom_worker_url: None,
            custom_cloud_url: None,
            allow_insecure: false,
            config: ProfileConfig::default(),
            auth: AuthenticationConfig::Static(StaticAuthenticationConfig {
                secret: AuthSecret(LOCAL_WELL_KNOWN_TOKEN),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, Eq, PartialEq)]
pub struct ProfileConfig {
    #[serde(default)]
    pub default_format: Format,
}

impl Config {
    fn config_path(config_dir: &Path) -> PathBuf {
        config_dir.join("config-v2.json")
    }

    pub fn default_profile_name(&self) -> ProfileName {
        self.default_profile
            .clone()
            .unwrap_or_else(ProfileName::local)
    }

    pub fn from_dir(config_dir: &Path) -> anyhow::Result<Config> {
        let config_path = Self::config_path(config_dir);

        if !config_path
            .try_exists()
            .with_context(|| anyhow!("Failed to check config file: {}", config_path.display()))?
        {
            return Ok(Config::default().with_local_and_cloud_profiles());
        }

        let file = File::open(&config_path)
            .with_context(|| anyhow!("Failed to open config file: {}", config_path.display()))?;

        let reader = BufReader::new(file);
        let mut config: Config = serde_json::from_reader(reader).with_context(|| {
            anyhow!(
                "Failed to deserialize config file {}",
                config_path.display(),
            )
        })?;

        // Detect if it was not yet migrated
        if config.default_profile.is_none() {
            // Drop old default profiles
            config.profiles.remove(&ProfileName::from("default"));
            config.profiles.remove(&ProfileName::from("cloud_default"));

            // Rename profiles that are conflicting with the new ones
            if let Some(profile) = config.profiles.remove(&ProfileName::from("local")) {
                config
                    .profiles
                    .insert(ProfileName::from("local-migrated"), profile);
            };
            if let Some(profile) = config.profiles.remove(&ProfileName::from("cloud")) {
                config
                    .profiles
                    .insert(ProfileName::from("cloud-migrated"), profile);
            }

            // Set default to local
            config.default_profile = Some(ProfileName::from("local"));

            // Save migrated config
            config.store_file(config_dir).with_context(|| {
                anyhow!(
                    "Failed to save config after migration: {}",
                    config_path.display()
                )
            })?;
        }

        Ok(config.with_local_and_cloud_profiles())
    }

    fn with_local_and_cloud_profiles(mut self) -> Self {
        self.profiles
            .entry(ProfileName::local())
            .or_insert_with(Profile::default_local_profile);

        self.profiles.entry(ProfileName::cloud()).or_default();

        if self.default_profile.is_none() {
            self.default_profile = Some(ProfileName::local())
        }

        self
    }

    fn store_file(&self, config_dir: &Path) -> anyhow::Result<()> {
        create_dir_all(config_dir)
            .map_err(|err| anyhow!("Can't create config directory: {err}"))?;

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(Self::config_path(config_dir))
            .map_err(|err| anyhow!("Can't open config file: {err}"))?;
        let writer = BufWriter::new(file);

        serde_json::to_writer_pretty(writer, self)
            .map_err(|err| anyhow!("Can't save config to file: {err}"))
    }

    pub fn set_active_profile_name(
        profile_name: ProfileName,
        config_dir: &Path,
    ) -> anyhow::Result<()> {
        let mut config = Self::from_dir(config_dir)?;

        if !config.profiles.contains_key(&profile_name) {
            bail!(
                "No profile {profile_name} in configuration. Available profiles: [{}]",
                config.profiles.keys().map(|n| &n.0).join(", ")
            );
        };

        config.default_profile = Some(profile_name);

        config.store_file(config_dir)?;

        Ok(())
    }

    pub fn get_default_profile(config_dir: &Path) -> anyhow::Result<NamedProfile> {
        let mut config = Self::from_dir(config_dir)?;
        let profile_name = config.default_profile.unwrap_or_else(ProfileName::local);
        let profile = config.profiles.remove(&profile_name).unwrap();
        Ok(NamedProfile {
            name: profile_name.clone(),
            profile: profile.clone(),
        })
    }

    pub fn get_profile(
        config_dir: &Path,
        name: &ProfileName,
    ) -> anyhow::Result<Option<NamedProfile>> {
        let mut config = Self::from_dir(config_dir)?;
        Ok(config.profiles.remove(name).map(|profile| NamedProfile {
            name: name.clone(),
            profile,
        }))
    }

    pub fn set_profile(
        name: ProfileName,
        profile: Profile,
        config_dir: &Path,
    ) -> anyhow::Result<()> {
        let mut config = Self::from_dir(config_dir)?;
        config.profiles.insert(name, profile);
        config.store_file(config_dir)
    }

    pub fn delete_profile(name: &ProfileName, config_dir: &Path) -> anyhow::Result<()> {
        let mut config = Self::from_dir(config_dir)?;
        config.profiles.remove(name);
        config.store_file(config_dir)
    }
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub component_url: Url,
    pub worker_url: Url,
    pub cloud_url: Url,
    pub service_http_client_config: HttpClientConfig,
    pub invoke_http_client_config: HttpClientConfig,
    pub health_check_http_client_config: HttpClientConfig,
    pub file_download_http_client_config: HttpClientConfig,
}

impl From<&Profile> for ClientConfig {
    fn from(profile: &Profile) -> Self {
        let default_cloud_url = Url::parse(CLOUD_URL).unwrap();
        let component_url = profile.custom_url.clone().unwrap_or(default_cloud_url);
        let cloud_url = profile
            .custom_cloud_url
            .clone()
            .unwrap_or_else(|| component_url.clone());

        let worker_url = profile
            .custom_worker_url
            .clone()
            .unwrap_or_else(|| component_url.clone());

        let allow_insecure = profile.allow_insecure;

        ClientConfig {
            component_url,
            worker_url,
            cloud_url,
            service_http_client_config: HttpClientConfig::new_for_service_calls(allow_insecure),
            invoke_http_client_config: HttpClientConfig::new_for_invoke(allow_insecure),
            health_check_http_client_config: HttpClientConfig::new_for_health_check(allow_insecure),
            file_download_http_client_config: HttpClientConfig::new_for_file_download(
                allow_insecure,
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    pub allow_insecure: bool,
    pub timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub read_timeout: Option<Duration>,
}

impl HttpClientConfig {
    pub fn new_for_service_calls(allow_insecure: bool) -> Self {
        Self {
            allow_insecure,
            timeout: Some(Duration::from_secs(10)),
            connect_timeout: Some(Duration::from_secs(10)),
            read_timeout: Some(Duration::from_secs(10)),
        }
        .with_env_overrides("GOLEM_HTTP")
    }

    pub fn new_for_invoke(allow_insecure: bool) -> Self {
        Self {
            allow_insecure,
            timeout: None,
            connect_timeout: None,
            read_timeout: None,
        }
        .with_env_overrides("GOLEM_HTTP_INVOKE")
    }

    pub fn new_for_health_check(allow_insecure: bool) -> Self {
        Self {
            allow_insecure,
            timeout: Some(Duration::from_secs(2)),
            connect_timeout: Some(Duration::from_secs(1)),
            read_timeout: Some(Duration::from_secs(1)),
        }
        .with_env_overrides("GOLEM_HTTP_HEALTHCHECK")
    }

    pub fn new_for_file_download(allow_insecure: bool) -> Self {
        Self {
            allow_insecure,
            timeout: Some(Duration::from_secs(60)),
            connect_timeout: Some(Duration::from_secs(10)),
            read_timeout: Some(Duration::from_secs(60)),
        }
        .with_env_overrides("GOLEM_HTTP_FILE_DOWNLOAD")
    }

    fn with_env_overrides(mut self, prefix: &str) -> Self {
        fn env_duration(name: &str) -> Option<Duration> {
            let duration_str = std::env::var(name).ok()?;
            Some(iso8601::duration(&duration_str).ok()?.into())
        }

        let duration_fields: Vec<(&str, &mut Option<Duration>)> = vec![
            ("TIMEOUT", &mut self.timeout),
            ("CONNECT_TIMEOUT", &mut self.connect_timeout),
            ("READ_TIMEOUT", &mut self.read_timeout),
        ];

        for (env_var_name, field) in duration_fields {
            if let Some(duration) = env_duration(&format!("{prefix}_{env_var_name}")) {
                *field = Some(duration);
            }
        }

        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthenticationConfig {
    OAuth2(OAuth2AuthenticationConfig),
    Static(StaticAuthenticationConfig),
}

impl AuthenticationConfig {
    pub fn empty_oauth2() -> Self {
        Self::OAuth2(OAuth2AuthenticationConfig { data: None })
    }

    pub fn static_token(token: Uuid) -> Self {
        Self::Static(StaticAuthenticationConfig {
            secret: AuthSecret(token),
        })
    }
}

impl Default for AuthenticationConfig {
    fn default() -> Self {
        Self::empty_oauth2()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2AuthenticationConfig {
    pub data: Option<OAuth2AuthenticationData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2AuthenticationData {
    pub id: Uuid,
    pub account_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub secret: AuthSecret,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticAuthenticationConfig {
    pub secret: AuthSecret,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct AuthSecret(pub Uuid);

impl Display for AuthSecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("*******")
    }
}

impl Debug for AuthSecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AuthSecret").field(&"*******").finish()
    }
}

impl From<AuthSecret> for TokenSecret {
    fn from(value: AuthSecret) -> Self {
        Self { value: value.0 }
    }
}
