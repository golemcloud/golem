use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples, RetryConfig};
use golem_common::tracing::TracingConfig;
use golem_component_service_base::config::ComponentCompilationConfig;
use golem_service_base::config::{ComponentStoreConfig, DbConfig};
use http::Uri;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceConfig {
    pub tracing: TracingConfig,
    pub environment: String,
    pub workspace: String,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbConfig,
    pub component_store: ComponentStoreConfig,
    pub compilation: ComponentCompilationConfig,
    pub cloud_service: CloudServiceConfig,
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("component-service"),
            environment: "dev".to_string(),
            workspace: "release".to_string(),
            http_port: 8081,
            grpc_port: 9091,
            db: DbConfig::default(),
            component_store: ComponentStoreConfig::default(),
            compilation: ComponentCompilationConfig::default(),
            cloud_service: CloudServiceConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CloudServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl CloudServiceConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse CloudService URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build CloudService URI")
    }
}

impl Default for CloudServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            access_token: Uuid::parse_str("5c832d93-ff85-4a8f-9803-513950fdfdb1")
                .expect("invalid UUID"),
            retries: RetryConfig::default(),
        }
    }
}

impl HasConfigExamples<ComponentServiceConfig> for ComponentServiceConfig {
    fn examples() -> Vec<ConfigExample<ComponentServiceConfig>> {
        vec![]
    }
}

pub fn make_config_loader() -> ConfigLoader<ComponentServiceConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from("config/component-service.toml"))
}

#[cfg(test)]
mod tests {
    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        make_config_loader().load().expect("Failed to load config");
    }
}
