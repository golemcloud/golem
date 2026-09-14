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

use crate::model::app_raw::{BuiltinServer, Environment, Server};
use crate::model::format::Format;
use anyhow::{Context, anyhow, bail};
use chrono::{DateTime, Utc};
use golem_client::LOCAL_WELL_KNOWN_TOKEN;
use golem_client::model::TokenWithSecret;
use golem_common::model::application::ApplicationName;
use golem_common::model::environment::EnvironmentName;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::{Display, Formatter};
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

pub const DEFAULT_LOCAL_ROUTER_PORT: u16 = 9881;
pub const DEFAULT_LOCAL_CUSTOM_REQUEST_PORT: u16 = 9006;
pub const DEFAULT_LOCAL_MCP_PORT: u16 = 9007;

pub const DEFAULT_LOCAL_URL: &str = "http://localhost:9881";
pub const DEFAULT_CLOUD_URL: &str = "https://release.api.golem.cloud";

pub const CLOUD_HTTP_API_DOMAIN: &str = "apps.golem.cloud";
pub const CLOUD_MCP_DOMAIN: &str = "mcps.golem.cloud";

const BUILTIN_LOCAL_URL_ENV: &str = "GOLEM_BUILTIN_LOCAL_URL";
const PROFILE_NAME_LOCAL: &str = "local";
const PROFILE_NAME_CLOUD: &str = "cloud";

static DEFAULT_LOCAL_URL_PARSED: LazyLock<Url> =
    LazyLock::new(|| Url::parse(DEFAULT_LOCAL_URL).expect("Failed to parse DEFAULT_LOCAL_URL"));

struct BuiltinLocalUrlState {
    url: Url,
    uses_default: bool,
}

static BUILTIN_LOCAL_URL_ONCE: OnceLock<BuiltinLocalUrlState> = OnceLock::new();

fn builtin_local_url_state() -> &'static BuiltinLocalUrlState {
    BUILTIN_LOCAL_URL_ONCE.get_or_init(|| {
        if let Ok(override_url) = std::env::var(BUILTIN_LOCAL_URL_ENV) {
            return BuiltinLocalUrlState {
                url: Url::parse(&override_url).unwrap_or_else(|err| {
                    panic!("Failed to parse {BUILTIN_LOCAL_URL_ENV} ({override_url}): {err}")
                }),
                uses_default: false,
            };
        }

        BuiltinLocalUrlState {
            url: DEFAULT_LOCAL_URL_PARSED.clone(),
            uses_default: true,
        }
    })
}

pub fn builtin_local_url() -> Url {
    builtin_local_url_state().url.clone()
}

pub(crate) fn is_standard_builtin_local_url(url: &Url) -> bool {
    url == &*DEFAULT_LOCAL_URL_PARSED
}

pub(crate) fn resolved_builtin_local_url(
    router_addr: Option<&str>,
    router_port: Option<u16>,
) -> anyhow::Result<Url> {
    let state = builtin_local_url_state();
    resolve_builtin_local_url(&state.url, state.uses_default, router_addr, router_port)
}

fn resolve_builtin_local_url(
    base_url: &Url,
    uses_default: bool,
    router_addr: Option<&str>,
    router_port: Option<u16>,
) -> anyhow::Result<Url> {
    if !uses_default {
        return Ok(base_url.clone());
    }

    let mut url = base_url.clone();
    if let Some(router_addr) = router_addr {
        let bind_addr = router_addr
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| anyhow!("Invalid localServer.routerAddr: {router_addr}"))?;
        let connect_addr = if bind_addr.is_unspecified() {
            std::net::Ipv4Addr::LOCALHOST
        } else {
            bind_addr
        };
        url.set_host(Some(&connect_addr.to_string()))
            .map_err(|_| anyhow!("Invalid localServer.routerAddr: {router_addr}"))?;
    }
    if let Some(router_port) = router_port {
        url.set_port(Some(router_port))
            .map_err(|_| anyhow!("Invalid localServer.routerPort: {router_port}"))?;
    }
    Ok(url)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub profiles: HashMap<ProfileName, Profile>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default_profile: Option<ProfileName>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub application_environments: HashMap<String, ApplicationEnvironmentConfig>,
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

    /// The built-in server a built-in profile is bound to, `None` for custom profiles.
    pub fn builtin_server(&self) -> Option<Server> {
        match self.0.as_str() {
            PROFILE_NAME_LOCAL => Some(Server::Builtin(BuiltinServer::Local)),
            PROFILE_NAME_CLOUD => Some(Server::Builtin(BuiltinServer::Cloud)),
            _ => None,
        }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedProfile {
    pub name: ProfileName,
    pub profile: Profile,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub custom_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub custom_worker_url: Option<Url>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub allow_insecure: bool,
    #[serde(default)]
    pub config: ProfileConfig,
    pub auth: AuthenticationConfig,
}

impl Profile {
    /// The built-in `local` profile. Its server is always the resolved built-in local URL, so no
    /// connection fields are stored for it.
    pub fn default_local_profile() -> Self {
        Self {
            custom_url: None,
            custom_worker_url: None,
            allow_insecure: false,
            config: ProfileConfig::default(),
            auth: AuthenticationConfig::static_builtin_local(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileConfig {
    #[serde(default)]
    pub default_format: Format,
}

/// How many times replacing the config file is retried, see `Config::store_file`.
const STORE_FILE_REPLACE_ATTEMPTS: usize = 5;
const STORE_FILE_REPLACE_RETRY_DELAY: Duration = Duration::from_millis(20);

impl Config {
    fn config_path(config_dir: &Path) -> PathBuf {
        config_dir.join("config-v4.json")
    }

    fn lock_path(config_dir: &Path) -> PathBuf {
        config_dir.join("config-v4.json.lock")
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
        let config: Config = serde_json::from_reader(reader).with_context(|| {
            anyhow!(
                "Failed to deserialize config file {}",
                config_path.display(),
            )
        })?;

        // Connection fields (`custom_url`, `custom_worker_url`, `allow_insecure`) stored on the
        // built-in `local`/`cloud` profiles are ignored: those profiles are always bound to their
        // built-in servers (see `ProfileName::builtin_server`). Older config files may still
        // carry a pinned `custom_url` for `local`; it is left as-is and never read.
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

    /// Writes the config atomically: it is serialized into a temporary file next to the config
    /// file, which then replaces it, so readers never see a partially written file and a crash
    /// mid-write leaves the previous config intact.
    fn store_file(&self, config_dir: &Path) -> anyhow::Result<()> {
        create_dir_all(config_dir)
            .map_err(|err| anyhow!("Can't create config directory: {err}"))?;

        let config_path = Self::config_path(config_dir);
        let mut temporary = tempfile::NamedTempFile::new_in(config_dir).map_err(|err| {
            anyhow!(
                "Can't create temporary config file in {}: {err}",
                config_dir.display()
            )
        })?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), self)
            .map_err(|err| anyhow!("Can't save config to file: {err}"))?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|err| anyhow!("Can't flush config file: {err}"))?;

        // On Windows the replace fails while another process has the config file open (e.g. a
        // concurrent read), so it is retried a few times.
        let mut attempt = 0;
        loop {
            match temporary.persist(&config_path) {
                Ok(_) => return Ok(()),
                Err(err) if attempt < STORE_FILE_REPLACE_ATTEMPTS => {
                    attempt += 1;
                    temporary = err.file;
                    std::thread::sleep(STORE_FILE_REPLACE_RETRY_DELAY);
                }
                Err(err) => {
                    bail!(
                        "Can't replace config file {}: {}",
                        config_path.display(),
                        err.error
                    )
                }
            }
        }
    }

    /// Runs a read-modify-write of the config file under a lock shared by all CLI processes
    /// using the same config directory, so concurrent invocations (e.g. two commands logging in
    /// at the same time) do not lose each other's changes. The lock is a separate file, as the
    /// config file itself is replaced on every write.
    fn with_locked<R>(
        config_dir: &Path,
        f: impl FnOnce(&mut Config) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        create_dir_all(config_dir)
            .map_err(|err| anyhow!("Can't create config directory: {err}"))?;

        let lock_path = Self::lock_path(config_dir);
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|err| anyhow!("Can't open config lock file {}: {err}", lock_path.display()))?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _guard = lock
            .write()
            .map_err(|err| anyhow!("Can't lock config file {}: {err}", lock_path.display()))?;

        let mut config = Self::from_dir(config_dir)?;
        let result = f(&mut config)?;
        config.store_file(config_dir)?;
        Ok(result)
    }

    pub fn set_active_profile_name(
        profile_name: ProfileName,
        config_dir: &Path,
    ) -> anyhow::Result<()> {
        Self::with_locked(config_dir, |config| {
            if !config.profiles.contains_key(&profile_name) {
                bail!(
                    "No profile {profile_name} in configuration. Available profiles: [{}]",
                    config.profiles.keys().map(|n| &n.0).join(", ")
                );
            };

            config.default_profile = Some(profile_name);
            Ok(())
        })
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

    /// Creates a new profile and optionally makes it the active one, as one locked write.
    pub fn add_profile(
        name: ProfileName,
        profile: Profile,
        set_active: bool,
        config_dir: &Path,
    ) -> anyhow::Result<()> {
        Self::with_locked(config_dir, |config| {
            config.profiles.insert(name.clone(), profile);
            if set_active {
                config.default_profile = Some(name);
            }
            Ok(())
        })
    }

    /// Modifies an existing profile in place. Returns `Ok(false)` without writing when the
    /// profile does not exist.
    pub fn update_profile(
        name: &ProfileName,
        config_dir: &Path,
        f: impl FnOnce(&mut Profile),
    ) -> anyhow::Result<bool> {
        Self::with_locked(config_dir, |config| match config.profiles.get_mut(name) {
            Some(profile) => {
                f(profile);
                Ok(true)
            }
            None => Ok(false),
        })
    }

    /// Deletes a profile unless it is the active one, in which case `Ok(false)` is returned
    /// and nothing is written.
    pub fn delete_inactive_profile(name: &ProfileName, config_dir: &Path) -> anyhow::Result<bool> {
        Self::with_locked(config_dir, |config| {
            if config.default_profile_name() == *name {
                return Ok(false);
            }
            config.profiles.remove(name);
            Ok(true)
        })
    }

    pub fn application_environment(
        &self,
        env_id: &ApplicationEnvironmentConfigId,
    ) -> Option<&ApplicationEnvironmentConfig> {
        self.application_environments.get(&env_id.to_hashed_key())
    }

    pub fn get_application_environment(
        config_dir: &Path,
        env_id: &ApplicationEnvironmentConfigId,
    ) -> anyhow::Result<Option<ApplicationEnvironmentConfig>> {
        let mut config = Self::from_dir(config_dir)?;
        Ok(config
            .application_environments
            .remove(&env_id.to_hashed_key()))
    }

    /// Modifies the config of an application environment in place, creating it when missing.
    pub fn update_application_environment(
        env_id: &ApplicationEnvironmentConfigId,
        config_dir: &Path,
        f: impl FnOnce(&mut ApplicationEnvironmentConfig),
    ) -> anyhow::Result<()> {
        Self::with_locked(config_dir, |config| {
            f(config
                .application_environments
                .entry(env_id.to_hashed_key())
                .or_default());
            Ok(())
        })
    }
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub registry_url: Url,
    pub worker_url: Url,
    pub service_http_client_config: HttpClientConfig,
    pub invoke_http_client_config: HttpClientConfig,
    pub file_download_http_client_config: HttpClientConfig,
}

impl From<&Profile> for ClientConfig {
    fn from(profile: &Profile) -> Self {
        let default_cloud_url = Url::parse(DEFAULT_CLOUD_URL).unwrap();
        let registry_url = profile.custom_url.clone().unwrap_or(default_cloud_url);
        let worker_url = profile
            .custom_worker_url
            .clone()
            .unwrap_or_else(|| registry_url.clone());

        let allow_insecure = profile.allow_insecure;

        ClientConfig {
            registry_url,
            worker_url,
            service_http_client_config: HttpClientConfig::new_for_service_calls(allow_insecure),
            invoke_http_client_config: HttpClientConfig::new_for_invoke(allow_insecure),
            file_download_http_client_config: HttpClientConfig::new_for_file_download(
                allow_insecure,
            ),
        }
    }
}

impl ClientConfig {
    pub(crate) fn from_manifest_environment(environment: &Environment, local_url: &Url) -> Self {
        match environment.server.as_ref() {
            Some(server) => Self::from_server(server, local_url),
            None => Self::from_server(&Server::Builtin(BuiltinServer::Local), local_url),
        }
    }

    pub(crate) fn from_server(server: &Server, local_url: &Url) -> Self {
        struct BaseConfig {
            registry_url: Url,
            worker_url: Url,
            allow_insecure: bool,
        }

        let BaseConfig {
            registry_url,
            worker_url,
            allow_insecure,
        } = match server {
            Server::Builtin(builtin) => match builtin {
                BuiltinServer::Local => BaseConfig {
                    registry_url: local_url.clone(),
                    worker_url: local_url.clone(),
                    allow_insecure: false,
                },
                BuiltinServer::Cloud => {
                    let cloud_url =
                        Url::parse(DEFAULT_CLOUD_URL).expect("Failed to parse DEFAULT_CLOUD_URL");
                    BaseConfig {
                        registry_url: cloud_url.clone(),
                        worker_url: cloud_url.clone(),
                        allow_insecure: false,
                    }
                }
            },
            Server::Custom(custom) => BaseConfig {
                registry_url: custom.url.clone(),
                worker_url: custom
                    .worker_url
                    .clone()
                    .unwrap_or_else(|| custom.url.clone()),
                allow_insecure: custom.allow_insecure.unwrap_or(false),
            },
        };

        ClientConfig {
            registry_url,
            worker_url,
            service_http_client_config: HttpClientConfig::new_for_service_calls(allow_insecure),
            invoke_http_client_config: HttpClientConfig::new_for_invoke(allow_insecure),
            file_download_http_client_config: HttpClientConfig::new_for_file_download(
                allow_insecure,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn default_local_url() -> Url {
        DEFAULT_LOCAL_URL_PARSED.clone()
    }

    #[test]
    fn local_url_uses_manifest_router_settings() {
        let url =
            resolve_builtin_local_url(&default_local_url(), true, Some("192.0.2.10"), Some(9891))
                .unwrap();

        assert_eq!(url.as_str(), "http://192.0.2.10:9891/");
    }

    #[test]
    fn local_url_translates_unspecified_bind_address() {
        let url =
            resolve_builtin_local_url(&default_local_url(), true, Some("0.0.0.0"), Some(9891))
                .unwrap();

        assert_eq!(url.as_str(), "http://127.0.0.1:9891/");
    }

    #[test]
    fn local_url_keeps_defaults_without_manifest_overrides() {
        let url = resolve_builtin_local_url(&default_local_url(), true, None, None).unwrap();

        assert_eq!(url.as_str(), "http://localhost:9881/");
    }

    #[test]
    fn explicit_local_url_override_wins_over_manifest() {
        let override_url = Url::parse("http://custom-host:1234").unwrap();
        let url = resolve_builtin_local_url(&override_url, false, Some("192.0.2.10"), Some(9891))
            .unwrap();

        assert_eq!(url, override_url);
    }

    #[test]
    fn invalid_manifest_router_address_is_rejected() {
        let error = resolve_builtin_local_url(
            &default_local_url(),
            true,
            Some("not a valid host"),
            Some(9891),
        )
        .unwrap_err();

        assert!(error.to_string().contains("localServer.routerAddr"));
    }

    #[test]
    fn cloud_client_ignores_local_url() {
        let local_url = Url::parse("http://192.0.2.10:9891").unwrap();
        let client = ClientConfig::from_server(&Server::Builtin(BuiltinServer::Cloud), &local_url);

        let cloud_url = Url::parse(DEFAULT_CLOUD_URL).unwrap();
        assert_eq!(client.registry_url, cloud_url);
        assert_eq!(client.worker_url, cloud_url);
    }

    #[test]
    fn custom_client_ignores_local_url() {
        let local_url = Url::parse("http://192.0.2.10:9891").unwrap();
        let custom_url = Url::parse("http://custom-server:1234").unwrap();
        let server = Server::Custom(Box::new(crate::model::app_raw::CustomServer {
            url: custom_url.clone(),
            worker_url: None,
            allow_insecure: Some(true),
            auth: crate::model::app_raw::CustomServerAuth::Static {
                static_token: "token".to_string(),
            },
        }));
        let client = ClientConfig::from_server(&server, &local_url);

        assert_eq!(client.registry_url, custom_url);
        assert_eq!(client.worker_url, custom_url);
    }

    #[test]
    fn concurrent_profile_updates_are_not_lost() {
        let config_dir = tempfile::tempdir().unwrap();
        let profile_names = (0..8)
            .map(|i| ProfileName(format!("profile-{i}")))
            .collect::<Vec<_>>();

        // Each thread adds its own profile and then repeatedly switches the active profile, so
        // a lost update would show up as a missing profile or a torn config file.
        std::thread::scope(|scope| {
            for name in &profile_names {
                let config_dir = config_dir.path();
                scope.spawn(move || {
                    Config::add_profile(name.clone(), Profile::default(), false, config_dir)
                        .unwrap();
                    for _ in 0..10 {
                        Config::set_active_profile_name(name.clone(), config_dir).unwrap();
                    }
                });
            }
        });

        let config = Config::from_dir(config_dir.path()).unwrap();
        for name in &profile_names {
            assert!(config.profiles.contains_key(name), "missing profile {name}");
        }
        assert!(profile_names.contains(&config.default_profile_name()));

        // Only the config file and its lock file are left behind, no temporary files
        let mut entries = std::fs::read_dir(config_dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, vec!["config-v4.json", "config-v4.json.lock"]);
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
            timeout: Some(Duration::from_secs(120)),
            connect_timeout: Some(Duration::from_secs(10)),
            read_timeout: Some(Duration::from_secs(60)),
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

    pub fn new_for_file_download(allow_insecure: bool) -> Self {
        Self {
            allow_insecure,
            timeout: Some(Duration::from_secs(120)),
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

    pub fn static_token(token: String) -> Self {
        Self::Static(StaticAuthenticationConfig {
            secret: AuthSecret(token),
        })
    }

    pub fn static_builtin_local() -> Self {
        AuthenticationConfig::Static(StaticAuthenticationConfig {
            secret: AuthSecret(LOCAL_WELL_KNOWN_TOKEN.to_string()),
        })
    }

    pub fn from_token_with_secret(token_with_secret: TokenWithSecret) -> Self {
        Self::OAuth2(OAuth2AuthenticationConfig {
            data: Some(OAuth2AuthenticationData::from_token_with_secret(
                token_with_secret,
            )),
        })
    }
}

impl Default for AuthenticationConfig {
    fn default() -> Self {
        Self::empty_oauth2()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2AuthenticationConfig {
    pub data: Option<OAuth2AuthenticationData>,
}

impl OAuth2AuthenticationConfig {
    pub fn from_token_with_secret(token_with_secret: TokenWithSecret) -> Self {
        Self {
            data: Some(OAuth2AuthenticationData::from_token_with_secret(
                token_with_secret,
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2AuthenticationData {
    pub id: Uuid,
    pub account_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub secret: AuthSecret,
}

impl OAuth2AuthenticationData {
    pub fn from_token_with_secret(token_with_secret: TokenWithSecret) -> Self {
        Self {
            id: token_with_secret.id.0,
            account_id: token_with_secret.account_id.0,
            created_at: token_with_secret.created_at,
            expires_at: token_with_secret.expires_at,
            secret: token_with_secret.secret.secret().to_string().into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticAuthenticationConfig {
    pub secret: AuthSecret,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationEnvironmentConfig {
    pub auth: OAuth2AuthenticationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationEnvironmentConfigId {
    pub application_name: ApplicationName,
    pub environment_name: EnvironmentName,
    pub server_url: Url,
}

impl ApplicationEnvironmentConfigId {
    pub fn to_hashed_key(&self) -> String {
        blake3::hash(serde_json::to_string(self).unwrap().as_bytes())
            .to_hex()
            .to_string()
    }
}

impl Display for ApplicationEnvironmentConfigId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} - {} - {}",
            self.application_name.0, self.environment_name.0, self.server_url
        )
    }
}

pub enum AuthenticationSource {
    Profile(ProfileName),
    ApplicationEnvironment(ApplicationEnvironmentConfigId),
}

pub struct AuthenticationConfigWithSource {
    pub authentication: AuthenticationConfig,
    pub source: AuthenticationSource,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthSecret(pub String);

impl From<String> for AuthSecret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

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
