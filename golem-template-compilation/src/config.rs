use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::RetryConfig;
use golem_worker_executor_base::services::golem_config::CompiledTemplateServiceConfig;
use http::Uri;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    // Services.
    pub template_service: TemplateServiceConfig,
    pub worker_service: WorkerServiceGrpcConfig,
    pub compiled_template_service: CompiledTemplateServiceConfig,

    // Workers.
    pub upload_worker: UploadWorkerConfig,
    pub compile_worker: CompileWorkerConfig,

    // General.
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub grpc_port: u16,
    pub grpc_host: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl TemplateServiceConfig {
    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build TemplateServiceTemplateService  URI")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadWorkerConfig {
    pub num_workers: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompileWorkerConfig {
    pub retries: RetryConfig,
    pub max_template_size: usize,
}

impl ServerConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/compilation-server.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

#[test]
fn config_load() {
    std::env::set_var("GOLEM__TEMPLATE_SERVICE__HOST", "0.0.0.0");
    std::env::set_var("GOLEM__TEMPLATE_SERVICE__PORT", "9001");
    std::env::set_var(
        "GOLEM__TEMPLATE_SERVICE__ACCESS_TOKEN",
        "6778f06f-43ac-4e45-b501-6adb3253edf2",
    );

    std::env::set_var("GOLEM__WORKER_SERVICE__HOST", "0.0.0.0");
    std::env::set_var("GOLEM__WORKER_SERVICE__PORT", "9001");
    std::env::set_var(
        "GOLEM__WORKER_SERVICE__ACCESS_TOKEN",
        "6778f06f-43ac-4e45-b501-6adb3253edf2",
    );

    std::env::set_var(
        "GOLEM__COMPILED_TEMPLATE_SERVICE__CONFIG__REGION",
        "us-east-1",
    );
    std::env::set_var(
        "GOLEM__COMPILED_TEMPLATE_SERVICE__CONFIG__BUCKET",
        "golem-compiled-components",
    );
    std::env::set_var(
        "GOLEM__COMPILED_TEMPLATE_SERVICE__CONFIG__OBJECT_PREFIX",
        "",
    );

    let _ = ServerConfig::new();
}
