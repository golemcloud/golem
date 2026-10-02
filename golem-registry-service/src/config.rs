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

use crate::services::domain_registration::DomainRegistrationConfig;
use anyhow::Context;
use golem_common::config::DbConfig;
use golem_common::config::{ConfigLoader, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::model::plan::{PlanId, PlanName};
use golem_common::tracing::TracingConfig;
use golem_common::{SafeDisplay, grpc_uri};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use uuid::uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuiltinArtifactSource {
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuiltinArtifactsConfig {
    pub cache_dir: Option<PathBuf>,
    /// Per-artifact source overrides. Production defaults come from the embedded release lock.
    pub source_overrides: BTreeMap<String, BuiltinArtifactSource>,
}

impl BuiltinArtifactsConfig {
    pub fn resolved_artifacts(&self) -> anyhow::Result<BTreeMap<String, BuiltinArtifactSource>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Lock {
            schema_version: u32,
            repository: String,
            artifacts: BTreeMap<String, LockEntry>,
        }

        #[derive(Deserialize)]
        struct LockEntry {
            component: String,
            version: String,
            sha256: String,
        }

        let lock: Lock = serde_json::from_str(include_str!("../../builtin-artifacts.lock.json"))
            .context("failed to parse builtin-artifacts.lock.json")?;
        anyhow::ensure!(
            lock.schema_version == 1,
            "unsupported built-in artifact lock schema version {}",
            lock.schema_version
        );
        anyhow::ensure!(
            lock.repository == "golemcloud/golem-builtins",
            "unsupported built-in artifact repository '{}'",
            lock.repository
        );

        let mut artifacts = lock
            .artifacts
            .into_iter()
            .map(|(artifact_id, entry)| {
                anyhow::ensure!(
                    valid_artifact_id(&artifact_id),
                    "invalid built-in artifact ID '{artifact_id}'"
                );
                anyhow::ensure!(
                    valid_component_name(&entry.component),
                    "invalid built-in artifact component '{}'",
                    entry.component
                );
                semver::Version::parse(&entry.version).with_context(|| {
                    format!(
                        "invalid version '{}' for built-in artifact '{artifact_id}'",
                        entry.version
                    )
                })?;
                anyhow::ensure!(
                    entry.sha256.len() == 64
                        && entry
                            .sha256
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                    "invalid SHA-256 for built-in artifact '{artifact_id}'"
                );
                let url = format!(
                    "https://github.com/{}/releases/download/{}-v{}/{}.wasm",
                    lock.repository, entry.component, entry.version, entry.component
                );
                Ok((
                    artifact_id,
                    BuiltinArtifactSource {
                        url,
                        sha256: Some(entry.sha256),
                    },
                ))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        artifacts.extend(self.source_overrides.clone());
        Ok(artifacts)
    }

    pub fn resolved_cache_dir(&self) -> anyhow::Result<PathBuf> {
        match &self.cache_dir {
            Some(path) => Ok(path.clone()),
            None => {
                let executable = std::env::current_exe().map_err(|error| {
                    anyhow::anyhow!("failed to locate registry executable: {error}")
                })?;
                let parent = executable.parent().ok_or_else(|| {
                    anyhow::anyhow!(
                        "registry executable '{}' has no parent directory",
                        executable.display()
                    )
                })?;
                Ok(parent.join("builtin-artifacts"))
            }
        }
    }
}

fn valid_artifact_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_component_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct McpImportResolverConfig {
    pub cache_entries: usize,
    pub fetch_concurrency: usize,
    #[serde(with = "humantime_serde")]
    pub refresh_interval: std::time::Duration,
    #[serde(with = "humantime_serde")]
    pub operation_timeout: std::time::Duration,
    #[serde(with = "humantime_serde")]
    pub cache_ttl: std::time::Duration,
    #[serde(with = "humantime_serde")]
    pub failure_ttl: std::time::Duration,
    pub transport: golem_mcp_import::transport::Limits,
    pub projection: golem_mcp_import::tool::Limits,
}

impl Default for McpImportResolverConfig {
    fn default() -> Self {
        Self {
            cache_entries: 128,
            fetch_concurrency: 16,
            refresh_interval: std::time::Duration::from_secs(60),
            operation_timeout: std::time::Duration::from_secs(25),
            cache_ttl: std::time::Duration::from_secs(300),
            failure_ttl: std::time::Duration::from_secs(1),
            transport: golem_mcp_import::transport::Limits {
                timeout: std::time::Duration::from_secs(20),
                ..Default::default()
            },
            projection: Default::default(),
        }
    }
}

impl McpImportResolverConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.cache_entries > 0,
            "MCP import cache_entries must be positive"
        );
        anyhow::ensure!(
            self.fetch_concurrency > 0
                && self.fetch_concurrency <= tokio::sync::Semaphore::MAX_PERMITS,
            "MCP import fetch_concurrency is out of range"
        );
        anyhow::ensure!(
            !self.refresh_interval.is_zero()
                && self.refresh_interval <= std::time::Duration::from_secs(86_400),
            "MCP import refresh_interval must be positive and at most one day"
        );
        anyhow::ensure!(
            !self.operation_timeout.is_zero()
                && self.operation_timeout <= std::time::Duration::from_secs(86_400),
            "MCP import operation_timeout must be positive and at most one day"
        );
        anyhow::ensure!(
            !self.cache_ttl.is_zero(),
            "MCP import cache_ttl must be positive"
        );
        anyhow::ensure!(
            !self.failure_ttl.is_zero(),
            "MCP import failure_ttl must be positive"
        );
        anyhow::ensure!(
            self.transport.timeout <= self.operation_timeout,
            "MCP transport timeout exceeds operation timeout"
        );
        golem_mcp_import::transport::Client::new(
            "https://config.invalid/mcp",
            None,
            self.transport,
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistryServiceConfig {
    pub tracing: TracingConfig,
    pub environment: String,
    pub workspace: String,
    pub http_port: u16,
    pub grpc: GrpcApiConfig,
    pub db: DbConfig,
    pub login: LoginConfig,
    pub blob_storage: BlobStorageConfig,
    pub cors_origin_regex: String,
    pub domain_registration: DomainRegistrationConfig,
    pub component_compilation: ComponentCompilationConfig,
    #[serde(default)]
    pub component_file_upload: ComponentFileUploadConfig,
    pub initial_accounts: HashMap<String, PrecreatedAccount>,
    pub initial_plans: HashMap<String, PrecreatedPlan>,
    #[serde(default)]
    pub builtin_plugins: BuiltinPluginsConfig,
    #[serde(default)]
    pub builtin_artifacts: BuiltinArtifactsConfig,
    #[serde(default)]
    pub deployment_events: DeploymentEventsConfig,
    #[serde(default)]
    pub resource_grants: ResourceGrantsConfig,
    #[serde(default)]
    pub security_scheme: SecuritySchemeConfig,
    pub mcp_oauth: golem_mcp_import::oauth::Limits,
    #[serde(default)]
    pub mcp_import: McpImportResolverConfig,
}

impl SafeDisplay for RegistryServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(&mut result, "environment: {}", self.environment);
        let _ = writeln!(&mut result, "workspace: {}", self.workspace);
        let _ = writeln!(&mut result, "HTTP port: {}", self.http_port);

        let _ = writeln!(&mut result, "grpc:");
        let _ = writeln!(&mut result, "{}", self.grpc.to_safe_string_indented());

        let _ = writeln!(&mut result, "db:");
        let _ = writeln!(&mut result, "{}", self.db.to_safe_string_indented());

        let _ = writeln!(&mut result, "login:");
        let _ = writeln!(&mut result, "{}", self.login.to_safe_string_indented());

        let _ = writeln!(&mut result, "blob storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.blob_storage.to_safe_string_indented()
        );

        let _ = writeln!(
            &mut result,
            "resource grant cleanup interval: {:?}",
            self.resource_grants.cleanup_interval
        );

        let _ = writeln!(&mut result, "CORS origin regex: {}", self.cors_origin_regex);

        let _ = writeln!(&mut result, "domain registration:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.domain_registration.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "component compilation:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_compilation.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "component file upload:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_file_upload.to_safe_string_indented()
        );

        let _ = writeln!(
            &mut result,
            "builtin plugins: enabled={}",
            self.builtin_plugins.enabled(),
        );
        let _ = writeln!(
            &mut result,
            "builtin artifacts: cache_dir={}, source_overrides={}",
            self.builtin_artifacts
                .cache_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<next to executable>".to_string()),
            self.builtin_artifacts.source_overrides.len(),
        );

        let _ = writeln!(&mut result, "deployment events:");
        let _ = writeln!(
            &mut result,
            "  retention: {:?}",
            self.deployment_events.retention
        );
        let _ = writeln!(
            &mut result,
            "  cleanup_interval: {:?}",
            self.deployment_events.cleanup_interval
        );

        let _ = writeln!(
            &mut result,
            "security scheme: strict_issuer_url_validation={}",
            self.security_scheme.strict_issuer_url_validation
        );
        let _ = writeln!(&mut result, "MCP OAuth limits: {:?}", self.mcp_oauth);
        let _ = writeln!(&mut result, "MCP import resolver: {:?}", self.mcp_import);

        result
    }
}

impl Default for RegistryServiceConfig {
    fn default() -> Self {
        let mut initial_accounts = HashMap::with_capacity(4);
        initial_accounts.insert(
            "root".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("e71a6160-4144-4720-9e34-e5943458d129")),
                name: "Initial User".to_string(),
                email: AccountEmail::new("initial@user"),
                token: Some(TokenSecret::trusted(
                    "lDL3DP2d7I3EbgfgJ9YEjVdEXNETpPkGYwyb36jgs28".to_string(),
                )),
                role: AccountRole::Admin,
                plan_id: PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")),
            },
        );
        initial_accounts.insert(
            "marketing".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("0e8a0431-94b9-4644-89ca-fbf403edb6e7")),
                name: "Marketing User".to_string(),
                email: AccountEmail::new("marketing@user"),
                token: Some(TokenSecret::trusted(
                    "2dwnjEdx8a_bw8TTN7r6yqcvLY2jAQuoD1N6U3uRy9I".to_string(),
                )),
                role: AccountRole::MarketingAdmin,
                plan_id: PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")),
            },
        );
        initial_accounts.insert(
            "builtin_plugin_owner".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("adb2694f-cd9f-425d-905d-ca2888c9c5de")),
                name: "Builtin Plugin Owner".to_string(),
                email: AccountEmail::new("builtin-plugin-owner@golem.cloud"),
                token: None,
                role: AccountRole::BuiltinPluginOwner,
                plan_id: PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")),
            },
        );
        initial_accounts.insert(
            "builtin_tool_owner".to_string(),
            PrecreatedAccount {
                id: AccountId(uuid!("58bda34c-10d4-4bfb-8abd-d5e67f09ba3c")),
                name: "Builtin Tool Owner".to_string(),
                email: AccountEmail::new("builtin-tool-owner@golem.cloud"),
                token: None,
                role: AccountRole::BuiltinPluginOwner,
                plan_id: PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")),
            },
        );

        let mut initial_plans = HashMap::with_capacity(1);
        initial_plans.insert(
            "default".to_string(),
            PrecreatedPlan {
                plan_id: PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")),
                plan_name: PlanName("default".to_string()),
                app_limit: 10,
                env_limit: 40,
                component_limit: 100,
                worker_connection_limit: 100,
                storage_limit: 500000000,
                monthly_gas_limit: 1000000000000000000,
                monthly_upload_limit: 1000000000,
                monthly_compute_gcu: 0,
                monthly_memory_gb_seconds: 0,
                monthly_durable_storage_gb_month: 0,
                monthly_ephemeral_storage_gb_month: 0,
                overage_eligible: false,
                max_memory_per_agent: 1024 * 1024 * 1024, // 1 GB
                max_memory_per_agent_ceiling: default_unlimited(),
                max_memory_per_agent_user_configurable: false,
                max_table_elements_per_worker: 16_384,
                max_storage_per_agent_enabled: false,
                max_storage_per_agent: default_unlimited(),
                max_storage_per_agent_ceiling: None,
                max_storage_per_agent_user_configurable: false,
                per_invocation_http_call_limit: 1_000_000_000_000_000_000,
                per_invocation_rpc_call_limit: 1_000_000_000_000_000_000,
                monthly_http_call_limit: 1_000_000_000_000_000_000,
                monthly_rpc_call_limit: 1_000_000_000_000_000_000,
                max_concurrent_agents_per_executor: 1_000_000_000_000_000_000, // unlimited sentinel
                oplog_writes_per_second: 1_000_000_000_000_000_000,            // unlimited sentinel
            },
        );

        Self {
            tracing: TracingConfig::local_dev("registry-service"),
            environment: "dev".to_string(),
            workspace: "release".to_string(),
            http_port: 8081,
            grpc: GrpcApiConfig::default(),
            db: DbConfig::Sqlite(DbSqliteConfig {
                database: "golem_registry_service.db".to_string(),
                foreign_keys: true,
                ..Default::default()
            }),
            login: LoginConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
            component_compilation: ComponentCompilationConfig::default(),
            component_file_upload: ComponentFileUploadConfig::default(),
            blob_storage: BlobStorageConfig::default(),
            domain_registration: DomainRegistrationConfig::default(),
            initial_accounts,
            initial_plans,
            builtin_plugins: BuiltinPluginsConfig::default(),
            builtin_artifacts: BuiltinArtifactsConfig::default(),
            deployment_events: DeploymentEventsConfig::default(),
            resource_grants: ResourceGrantsConfig::default(),
            security_scheme: SecuritySchemeConfig::default(),
            mcp_oauth: golem_mcp_import::oauth::Limits::default(),
            mcp_import: McpImportResolverConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ComponentFileUploadConfig {
    pub max_concurrent_files: NonZeroUsize,
    pub max_uncompressed_file_size: u64,
    pub max_uncompressed_archive_size: u64,
}

impl Default for ComponentFileUploadConfig {
    fn default() -> Self {
        Self {
            max_concurrent_files: NonZeroUsize::new(16).unwrap(),
            max_uncompressed_file_size: 536_870_912,
            max_uncompressed_archive_size: 1_073_741_824,
        }
    }
}

impl SafeDisplay for ComponentFileUploadConfig {
    fn to_safe_string(&self) -> String {
        format!(
            "max concurrent files: {}, max uncompressed file size: {}, max uncompressed archive size: {}",
            self.max_concurrent_files,
            self.max_uncompressed_file_size,
            self.max_uncompressed_archive_size,
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcApiConfig {
    pub port: u16,
    pub tls: GrpcServerTlsConfig,
}

impl SafeDisplay for GrpcApiConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "port: {}", self.port);

        let _ = writeln!(&mut result, "tls:");
        let _ = writeln!(&mut result, "{}", self.tls.to_safe_string_indented());

        result
    }
}

impl Default for GrpcApiConfig {
    fn default() -> Self {
        Self {
            port: 9090,
            tls: GrpcServerTlsConfig::disabled(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum LoginConfig {
    OAuth2(Box<OAuth2LoginSystemConfig>),
    Disabled(Empty),
}

impl SafeDisplay for LoginConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            LoginConfig::OAuth2(inner) => {
                let _ = writeln!(&mut result, "OAuth2:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            LoginConfig::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
        }
        result
    }
}

impl Default for LoginConfig {
    fn default() -> LoginConfig {
        LoginConfig::OAuth2(Box::default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OAuth2LoginSystemConfig {
    pub github: GitHubOAuth2Config,
    pub oauth2: OAuth2Config,
}

impl SafeDisplay for OAuth2LoginSystemConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "GitHub:");
        let _ = writeln!(&mut result, "{}", self.github.to_safe_string_indented());
        let _ = writeln!(&mut result, "OAuth2:");
        let _ = writeln!(&mut result, "{}", self.oauth2.to_safe_string_indented());
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OAuth2Config {
    #[serde(with = "humantime_serde")]
    pub webflow_state_expiry: std::time::Duration,
    /// The URL to redirect the browser to after a CLI web flow completes.
    /// Defaults to `https://golem.cloud`.
    pub cli_redirect: url::Url,
    /// Domains allowed as redirect targets in the browser web flow.
    /// A redirect URL is accepted if its domain equals or is a subdomain of any entry.
    /// Defaults to `["localhost", "golem.cloud"]`.
    #[serde(default = "OAuth2Config::default_allowed_redirect_domains")]
    pub allowed_redirect_domains: Vec<String>,
}

impl OAuth2Config {
    fn default_allowed_redirect_domains() -> Vec<String> {
        vec!["localhost".to_string(), "golem.cloud".to_string()]
    }
}

impl SafeDisplay for OAuth2Config {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "webflow state expiry: {:?}",
            self.webflow_state_expiry
        );
        let _ = writeln!(&mut result, "cli redirect: {}", self.cli_redirect);
        let _ = writeln!(
            &mut result,
            "allowed redirect domains: {}",
            self.allowed_redirect_domains.join(", ")
        );
        result
    }
}

impl Default for OAuth2Config {
    fn default() -> Self {
        Self {
            webflow_state_expiry: std::time::Duration::from_mins(5),
            cli_redirect: url::Url::parse("https://golem.cloud").unwrap(),
            allowed_redirect_domains: Self::default_allowed_redirect_domains(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHubOAuth2Config {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: url::Url,
}

impl SafeDisplay for GitHubOAuth2Config {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "client id: {}", self.client_id);
        let _ = writeln!(&mut result, "client secret: ****");
        let _ = writeln!(&mut result, "redirect uri: {}", self.redirect_uri);
        result
    }
}

impl Default for GitHubOAuth2Config {
    fn default() -> Self {
        Self {
            client_id: "GITHUB_CLIENT_ID".to_string(),
            client_secret: "GITHUB_CLIENT_SECRET".to_string(),
            redirect_uri: url::Url::parse("http://localhost:8080/v1/login/oauth2/web/callback")
                .unwrap(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentCompilationConfig {
    Enabled(Box<ComponentCompilationEnabledConfig>),
    Disabled(Empty),
}

impl SafeDisplay for ComponentCompilationConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            ComponentCompilationConfig::Enabled(inner) => {
                let _ = writeln!(&mut result, "enabled:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            ComponentCompilationConfig::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
        }
        result
    }
}

impl Default for ComponentCompilationConfig {
    fn default() -> Self {
        Self::Enabled(Box::default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCompilationEnabledConfig {
    pub host: String,
    pub port: u16,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
}

impl SafeDisplay for ComponentCompilationEnabledConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        result
    }
}

impl Default for ComponentCompilationEnabledConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9091,
            client_config: GrpcClientConfig::default(),
        }
    }
}

impl ComponentCompilationEnabledConfig {
    pub fn uri(&self) -> Uri {
        grpc_uri(&self.host, self.port, self.client_config.tls_enabled())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum BuiltinPluginsConfig {
    Enabled(Empty),
    Disabled(Empty),
}

impl Default for BuiltinPluginsConfig {
    fn default() -> Self {
        Self::Disabled(Empty {})
    }
}

impl BuiltinPluginsConfig {
    pub fn enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrecreatedAccount {
    pub id: AccountId,
    pub name: String,
    pub email: AccountEmail,
    #[serde(default)]
    pub token: Option<TokenSecret>,
    pub plan_id: PlanId,
    pub role: AccountRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeploymentEventsConfig {
    /// Retention period for outbox events
    #[serde(with = "humantime_serde")]
    pub retention: std::time::Duration,
    /// How often to clean up old events
    #[serde(with = "humantime_serde")]
    pub cleanup_interval: std::time::Duration,
}

impl Default for DeploymentEventsConfig {
    fn default() -> Self {
        Self {
            retention: std::time::Duration::from_secs(24 * 3600), // 24 hours
            cleanup_interval: std::time::Duration::from_secs(3600), // 1 hour
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceGrantsConfig {
    #[serde(with = "humantime_serde")]
    pub cleanup_interval: std::time::Duration,
}

impl Default for ResourceGrantsConfig {
    fn default() -> Self {
        Self {
            cleanup_interval: std::time::Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrecreatedPlan {
    pub plan_id: PlanId,
    pub plan_name: PlanName,
    pub app_limit: u64,
    pub env_limit: u64,
    pub component_limit: u64,
    pub worker_connection_limit: u64,
    pub storage_limit: u64,
    /// Executor fuel allowance. Monthly account policy uses `monthly_compute_gcu`.
    pub monthly_gas_limit: u64,
    pub monthly_upload_limit: u64,
    pub monthly_compute_gcu: u64,
    pub monthly_memory_gb_seconds: u64,
    pub monthly_durable_storage_gb_month: u64,
    pub monthly_ephemeral_storage_gb_month: u64,
    pub overage_eligible: bool,
    pub max_memory_per_agent: u64,
    #[serde(default = "default_unlimited")]
    pub max_memory_per_agent_ceiling: u64,
    #[serde(default)]
    pub max_memory_per_agent_user_configurable: bool,
    #[serde(default = "default_max_table_elements_per_worker")]
    pub max_table_elements_per_worker: u64,
    #[serde(default)]
    pub max_storage_per_agent_enabled: bool,
    #[serde(default = "default_unlimited")]
    pub max_storage_per_agent: u64,
    /// Upper bound a user may raise `max_storage_per_agent` to. Left unset it
    /// tracks `max_storage_per_agent`; see
    /// [`PrecreatedPlan::resolved_max_storage_per_agent_ceiling`].
    #[serde(default)]
    pub max_storage_per_agent_ceiling: Option<u64>,
    #[serde(default)]
    pub max_storage_per_agent_user_configurable: bool,
    #[serde(default = "default_unlimited")]
    pub per_invocation_http_call_limit: u64,
    #[serde(default = "default_unlimited")]
    pub per_invocation_rpc_call_limit: u64,
    #[serde(default = "default_unlimited")]
    pub monthly_http_call_limit: u64,
    #[serde(default = "default_unlimited")]
    pub monthly_rpc_call_limit: u64,
    #[serde(default = "default_unlimited")]
    pub max_concurrent_agents_per_executor: u64,
    #[serde(default = "default_unlimited")]
    pub oplog_writes_per_second: u64,
}

impl PrecreatedPlan {
    /// The ceiling for this plan's per-agent storage overrides.
    ///
    /// An unset ceiling tracks `max_storage_per_agent` rather than falling back to a
    /// fixed constant, keeping the plan default and its implicit override range coherent.
    pub fn resolved_max_storage_per_agent_ceiling(&self) -> u64 {
        self.max_storage_per_agent_ceiling
            .unwrap_or(self.max_storage_per_agent)
    }
}

fn default_max_table_elements_per_worker() -> u64 {
    16_384
}

fn default_unlimited() -> u64 {
    1_000_000_000_000_000_000 // 10^18, fits in i64 (TOML max), safe for SQLite REAL
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySchemeConfig {
    pub strict_issuer_url_validation: bool,
}

impl Default for SecuritySchemeConfig {
    fn default() -> Self {
        Self {
            strict_issuer_url_validation: true,
        }
    }
}

pub fn make_config_loader() -> ConfigLoader<RegistryServiceConfig> {
    ConfigLoader::new(&PathBuf::from("config/registry-service.toml"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use test_r::test;

    use crate::config::{
        BuiltinArtifactsConfig, ComponentFileUploadConfig, RegistryServiceConfig,
        make_config_loader,
    };

    #[test]
    pub fn config_is_loadable() {
        let config = make_config_loader().load().expect("Failed to load config");
        let plan = config
            .initial_plans
            .get("default")
            .expect("default plan must exist");
        assert_eq!(plan.monthly_compute_gcu, 0);
        assert_eq!(plan.monthly_memory_gb_seconds, 0);
        assert_eq!(plan.monthly_durable_storage_gb_month, 0);
        assert_eq!(plan.monthly_ephemeral_storage_gb_month, 0);
        assert!(!plan.overage_eligible);
    }

    #[test]
    pub fn builtin_artifact_defaults_are_pinned() {
        let config = BuiltinArtifactsConfig::default();
        assert!(config.source_overrides.is_empty());
        let artifacts = config.resolved_artifacts().unwrap();
        assert_eq!(artifacts.len(), 4);
        for (artifact_id, source) in artifacts {
            assert!(
                source
                    .url
                    .starts_with("https://github.com/golemcloud/golem-builtins/releases/download/"),
                "unexpected URL for {artifact_id}: {}",
                source.url
            );
            assert_eq!(
                source.sha256.as_deref().map(str::len),
                Some(64),
                "missing or invalid SHA-256 for {artifact_id}"
            );
        }
    }

    #[test]
    pub fn builtin_artifact_config_overrides_one_embedded_source() {
        let override_source = crate::config::BuiltinArtifactSource {
            url: "https://example.com/javascript-tools.wasm".to_string(),
            sha256: None,
        };
        let config = BuiltinArtifactsConfig {
            source_overrides: BTreeMap::from([(
                "javascript_tools".to_string(),
                override_source.clone(),
            )]),
            ..Default::default()
        };

        let artifacts = config.resolved_artifacts().unwrap();
        assert_eq!(artifacts.len(), 4);
        assert_eq!(artifacts["javascript_tools"].url, override_source.url);
        assert_eq!(artifacts["javascript_tools"].sha256, None);
        assert!(artifacts["typescript_tools"].sha256.is_some());
    }

    #[test]
    pub fn mcp_import_resolver_bounds_are_finite_and_validated() {
        use std::time::Duration;

        let defaults = RegistryServiceConfig::default().mcp_import;
        assert!(defaults.cache_entries > 0);
        assert!(defaults.fetch_concurrency > 0);
        assert!(!defaults.refresh_interval.is_zero());
        assert!(defaults.transport.timeout < Duration::from_secs(30));
        defaults.validate().unwrap();

        crate::config::McpImportResolverConfig {
            cache_ttl: Duration::MAX,
            failure_ttl: Duration::MAX,
            ..defaults
        }
        .validate()
        .unwrap();

        for invalid in [
            crate::config::McpImportResolverConfig {
                cache_entries: 0,
                ..defaults
            },
            crate::config::McpImportResolverConfig {
                fetch_concurrency: 0,
                ..defaults
            },
            crate::config::McpImportResolverConfig {
                refresh_interval: Duration::ZERO,
                ..defaults
            },
            crate::config::McpImportResolverConfig {
                refresh_interval: Duration::MAX,
                ..defaults
            },
            crate::config::McpImportResolverConfig {
                operation_timeout: Duration::ZERO,
                ..defaults
            },
            crate::config::McpImportResolverConfig {
                cache_ttl: Duration::ZERO,
                ..defaults
            },
            crate::config::McpImportResolverConfig {
                failure_ttl: Duration::ZERO,
                ..defaults
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    pub async fn mcp_import_resolver_rejects_unrepresentable_operation_timeout() {
        use crate::bootstrap::Services;
        use golem_common::config::{DbConfig, DbSqliteConfig};

        let mut config = RegistryServiceConfig::default();
        config.mcp_import.operation_timeout = std::time::Duration::MAX;
        assert!(config.mcp_import.validate().is_err());

        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("registry.db");
        config.db = DbConfig::Sqlite(DbSqliteConfig {
            database: database.to_str().unwrap().into(),
            ..Default::default()
        });
        let mut tasks = tokio::task::JoinSet::new();
        let error = Services::new(&config, &mut tasks).await.err().unwrap();
        assert!(error.to_string().contains("operation_timeout"));
        assert!(!database.exists());
        assert!(tasks.is_empty());
    }

    #[test]
    pub fn mcp_import_resolver_deadline_limits_are_inclusive() {
        use crate::config::McpImportResolverConfig;
        use std::time::Duration;

        let day = Duration::from_secs(24 * 60 * 60);
        let config = McpImportResolverConfig {
            refresh_interval: day,
            operation_timeout: day,
            ..Default::default()
        };
        config.validate().unwrap();
        for invalid in [
            McpImportResolverConfig {
                refresh_interval: day + Duration::from_nanos(1),
                ..config
            },
            McpImportResolverConfig {
                operation_timeout: day + Duration::from_nanos(1),
                ..config
            },
            McpImportResolverConfig {
                operation_timeout: Duration::from_secs(1),
                ..config
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    pub async fn mcp_oauth_config_validates_before_bootstrap() {
        use crate::bootstrap::Services;
        use golem_common::config::{DbConfig, DbSqliteConfig};
        use serde_json::json;
        use std::time::Duration;

        let mut config = RegistryServiceConfig::default();
        assert_eq!(
            serde_json::to_value(config.mcp_oauth).unwrap(),
            json!({
                "document_bytes": 1_048_576,
                "request_bytes": 65_536,
                "challenge_bytes": 16_384,
                "timeout": "20s"
            })
        );
        config.mcp_oauth = serde_json::from_value(json!({
            "document_bytes": 1024,
            "request_bytes": 512,
            "challenge_bytes": 128,
            "timeout": "250ms"
        }))
        .unwrap();
        assert_eq!(
            config.mcp_oauth.validate().unwrap().timeout,
            Duration::from_millis(250)
        );
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("registry.db");
        config.db = DbConfig::Sqlite(DbSqliteConfig {
            database: database.to_str().unwrap().into(),
            ..Default::default()
        });
        config.mcp_oauth.timeout = Duration::ZERO;
        let mut tasks = tokio::task::JoinSet::new();
        let error = Services::new(&config, &mut tasks).await.err().unwrap();
        assert!(error.to_string().contains("invalid OAuth limits"));
        assert!(!database.exists());
        assert!(tasks.is_empty());
    }

    #[test]
    pub fn component_file_upload_parallelism_rejects_zero() {
        let result = serde_json::from_value::<ComponentFileUploadConfig>(serde_json::json!({
            "max_concurrent_files": 0
        }));
        assert!(result.is_err());
    }

    /// An omitted ceiling follows the configured per-agent storage default.
    #[test]
    pub fn unset_ceiling_tracks_the_configured_storage_limit() {
        let mut plan = RegistryServiceConfig::default()
            .initial_plans
            .remove("default")
            .expect("default plan must exist");

        plan.max_storage_per_agent = 10 * 1024 * 1024 * 1024;
        plan.max_storage_per_agent_ceiling = None;

        assert_eq!(
            plan.resolved_max_storage_per_agent_ceiling(),
            10 * 1024 * 1024 * 1024
        );
    }

    /// An explicit ceiling still wins, including one below the plan default.
    #[test]
    pub fn explicit_ceiling_is_honoured() {
        let mut plan = RegistryServiceConfig::default()
            .initial_plans
            .remove("default")
            .expect("default plan must exist");

        plan.max_storage_per_agent = 10 * 1024 * 1024 * 1024;
        plan.max_storage_per_agent_ceiling = Some(2 * 1024 * 1024 * 1024);

        assert_eq!(
            plan.resolved_max_storage_per_agent_ceiling(),
            2 * 1024 * 1024 * 1024
        );
    }

    #[test]
    pub fn oss_monthly_plan_amounts_default_to_zero() {
        let plan = RegistryServiceConfig::default()
            .initial_plans
            .remove("default")
            .expect("default plan must exist");

        assert_eq!(plan.max_memory_per_agent_ceiling, 1_000_000_000_000_000_000);
        assert_eq!(plan.monthly_compute_gcu, 0);
        assert_eq!(plan.monthly_memory_gb_seconds, 0);
        assert_eq!(plan.monthly_durable_storage_gb_month, 0);
        assert_eq!(plan.monthly_ephemeral_storage_gb_month, 0);
        assert!(!plan.max_storage_per_agent_enabled);
        assert_eq!(plan.max_storage_per_agent, 1_000_000_000_000_000_000);
    }
}
