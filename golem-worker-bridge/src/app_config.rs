use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::{RedisConfig, RetryConfig};
use http::Uri;
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
pub struct WorkerBridgeConfig {
    pub environment: String,
    pub redis: RedisConfig,
    pub component_service: TemplateServiceConfig,
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub management_port: u16,
    pub port: u16,
}

impl WorkerBridgeConfig {
    pub fn is_local_env(&self) -> bool {
        self.environment.to_lowercase() == "local"
    }
}

impl Default for WorkerBridgeConfig {
    fn default() -> Self {
        Figment::new()
            .merge(Toml::file("config/gateway-service.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateServiceConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Uuid,
    pub retries: RetryConfig,
}

impl TemplateServiceConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse ComponentService URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build ComponentService URI")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn config_is_loadable() {
        // The following settings are always coming through environment variables:
        std::env::set_var("GOLEM__REDIS__HOST", "localhost");
        std::env::set_var("GOLEM__REDIS__PORT", "1234");
        std::env::set_var("GOLEM__REDIS__DATABASE", "1");
        std::env::set_var("GOLEM__ENVIRONMENT", "dev");
        std::env::set_var("GOLEM__WORKSPACE", "release");
        std::env::set_var("GOLEM__COMPONENT_SERVICE__HOST", "localhost");
        std::env::set_var("GOLEM__COMPONENT_SERVICE__PORT", "1234");
        std::env::set_var(
            "GOLEM__TEMPLATE_SERVICE__ACCESS_TOKEN",
            "5C832D93-FF85-4A8F-9803-513950FDFDB1",
        );

        // The rest can be loaded from the toml
        let config = super::WorkerBridgeConfig::default();

        println!("config: {:?}", config);
    }
}
