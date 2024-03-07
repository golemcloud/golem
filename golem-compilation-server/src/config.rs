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
        format!("http://{}:{}", self.host, self.port)
            .try_into()
            .expect("Valid Template Service URI")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadWorkerConfig {
    pub num_workers: usize,
    pub retry_config: RetryConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompileWorkerConfig {
    pub retry_config: RetryConfig,
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
