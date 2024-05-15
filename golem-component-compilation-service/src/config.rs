use std::net::{Ipv4Addr, SocketAddrV4};

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::RetryConfig;
use golem_worker_executor_base::services::golem_config::{BlobStorageConfig, CompiledComponentServiceConfig, S3BlobStorageConfig};
use http::Uri;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    // Services.
    pub component_service: ComponentServiceConfig,
    pub compiled_component_service: CompiledComponentServiceConfig,
    pub blob_storage: BlobStorageConfig,

    // Workers.
    pub upload_worker: UploadWorkerConfig,
    pub compile_worker: CompileWorkerConfig,

    // General.
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub grpc_port: u16,
    pub grpc_host: String,

    // Metrics and healthcheck.
    pub http_host: String,
    pub http_port: u16,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComponentServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl ComponentServiceConfig {
    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build ComponentService URI")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadWorkerConfig {
    pub num_workers: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompileWorkerConfig {
    pub retries: RetryConfig,
    pub max_component_size: usize,
}

impl ServerConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/component-compilation-service.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }

    pub fn http_addr(&self) -> Option<SocketAddrV4> {
        let address = self.http_host.parse::<Ipv4Addr>().ok()?;
        Some(SocketAddrV4::new(address, self.http_port))
    }
}

#[test]
fn config_load() {
    std::env::set_var("GOLEM__COMPONENT_SERVICE__HOST", "0.0.0.0");
    std::env::set_var("GOLEM__COMPONENT_SERVICE__PORT", "9001");
    std::env::set_var(
        "GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN",
        "6778f06f-43ac-4e45-b501-6adb3253edf2",
    );

    std::env::set_var(
        "GOLEM__COMPILED_COMPONENT_SERVICE__CONFIG__REGION",
        "us-east-1",
    );
    std::env::set_var(
        "GOLEM__COMPILED_COMPONENT_SERVICE__CONFIG__BUCKET",
        "golem-compiled-components",
    );
    std::env::set_var(
        "GOLEM__COMPILED_COMPONENT_SERVICE__CONFIG__OBJECT_PREFIX",
        "",
    );

    let _ = ServerConfig::new();
}
