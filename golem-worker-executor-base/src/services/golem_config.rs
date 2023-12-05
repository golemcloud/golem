use std::path::PathBuf;
use std::time::Duration;

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::{RedisConfig, RetryConfig};
use http::Uri;
use serde::Deserialize;
use url::Url;

/// The shared global Golem configuration
#[derive(Clone, Debug, Deserialize)]
pub struct GolemConfig {
    pub limits: Limits,
    pub retry: RetryConfig,
    pub template_cache: TemplateCacheConfig,
    pub template_service: TemplateServiceConfig,
    pub compiled_template_service: CompiledTemplateServiceConfig,
    pub blob_store_service: BlobStoreServiceConfig,
    pub key_value_service: KeyValueServiceConfig,
    pub promises: PromisesConfig,
    pub shard_manager_service: ShardManagerServiceConfig,
    pub workers: WorkersServiceConfig,
    pub redis: RedisConfig,
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub port: u16,
    pub http_port: u16,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Limits {
    pub max_active_instances: usize,
    pub concurrency_limit_per_connection: usize,
    pub max_concurrent_streams: u32,
    pub event_broadcast_capacity: usize,
    pub event_history_size: usize,
    pub fuel_to_borrow: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateCacheConfig {
    pub max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum TemplateServiceConfig {
    Grpc(TemplateServiceGrpcConfig),
    Local(TemplateServiceLocalConfig),
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
    pub retries: RetryConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateServiceLocalConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum CompiledTemplateServiceConfig {
    S3(CompiledTemplateServiceS3Config),
    Local(CompiledTemplateServiceLocalConfig),
    Disabled(CompiledTemplateServiceDisabledConfig),
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompiledTemplateServiceS3Config {
    pub retries: RetryConfig,
    pub region: String,
    pub bucket: String,
    pub object_prefix: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompiledTemplateServiceLocalConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompiledTemplateServiceDisabledConfig {}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum BlobStoreServiceConfig {
    S3(BlobStoreServiceS3Config),
    InMemory(BlobStoreServiceInMemoryConfig),
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlobStoreServiceS3Config {
    pub retries: RetryConfig,
    pub region: String,
    pub bucket_prefix: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BlobStoreServiceInMemoryConfig {}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ShardManagerServiceConfig {
    Grpc(ShardManagerServiceGrpcConfig),
    SingleShard,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ShardManagerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum KeyValueServiceConfig {
    Redis,
    InMemory,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum PromisesConfig {
    Redis,
    InMemory,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum WorkersServiceConfig {
    Redis,
    InMemory,
}

impl GolemConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/worker-executor.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }

    pub fn from_file(path: &str) -> Self {
        Figment::new()
            .merge(Toml::file(path))
            .extract()
            .expect("Failed to parse config")
    }
}

impl TemplateServiceGrpcConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse template service URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build template service URI")
    }
}

impl ShardManagerServiceGrpcConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse shard manager URL")
    }
}

impl Default for GolemConfig {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            retry: RetryConfig::default(),
            template_cache: TemplateCacheConfig::default(),
            template_service: TemplateServiceConfig::default(),
            compiled_template_service: CompiledTemplateServiceConfig::default(),
            blob_store_service: BlobStoreServiceConfig::default(),
            key_value_service: KeyValueServiceConfig::default(),
            promises: PromisesConfig::default(),
            shard_manager_service: ShardManagerServiceConfig::default(),
            workers: WorkersServiceConfig::default(),
            redis: RedisConfig::default(),
            enable_tracing_console: false,
            enable_json_log: false,
            port: 9000,
            http_port: 8080,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_active_instances: 1024,
            concurrency_limit_per_connection: 1024,
            max_concurrent_streams: 1024,
            event_broadcast_capacity: 16,
            event_history_size: 128,
            fuel_to_borrow: 10000,
        }
    }
}

impl Default for TemplateCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 32,
            time_to_idle: Duration::from_secs(12 * 60 * 60),
        }
    }
}

impl Default for TemplateServiceConfig {
    fn default() -> Self {
        Self::Grpc(TemplateServiceGrpcConfig::default())
    }
}

impl Default for TemplateServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9090,
            access_token: "access_token".to_string(),
            retries: RetryConfig::default(),
        }
    }
}

impl Default for CompiledTemplateServiceConfig {
    fn default() -> Self {
        Self::S3(CompiledTemplateServiceS3Config::default())
    }
}

impl Default for CompiledTemplateServiceS3Config {
    fn default() -> Self {
        Self {
            retries: RetryConfig::default(),
            region: "us-east-1".to_string(),
            bucket: "golem-compiled-components".to_string(),
            object_prefix: "".to_string(),
        }
    }
}

impl Default for BlobStoreServiceConfig {
    fn default() -> Self {
        Self::S3(BlobStoreServiceS3Config::default())
    }
}

impl Default for BlobStoreServiceS3Config {
    fn default() -> Self {
        Self {
            retries: RetryConfig::default(),
            region: "us-east-1".to_string(),
            bucket_prefix: "".to_string(),
        }
    }
}

impl Default for ShardManagerServiceConfig {
    fn default() -> Self {
        Self::Grpc(ShardManagerServiceGrpcConfig::default())
    }
}

impl Default for ShardManagerServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9020,
            retries: RetryConfig::default(),
        }
    }
}

impl Default for KeyValueServiceConfig {
    fn default() -> Self {
        Self::Redis
    }
}

impl Default for PromisesConfig {
    fn default() -> Self {
        Self::Redis
    }
}

impl Default for WorkersServiceConfig {
    fn default() -> Self {
        Self::Redis
    }
}
