use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_common::config::RetryConfig;
use golem_component_service_base::config::ComponentCompilationConfig;
use golem_service_base::config::ComponentStoreConfig;
use http::Uri;
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
pub struct DbSqliteConfig {
    pub database: String,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComponentServiceConfig {
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbConfig,
    pub component_store: ComponentStoreConfig,
    pub compilation: ComponentCompilationConfig,
    pub cloud_service: CloudServiceConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum DbConfig {
    Postgres(DbPostgresConfig),
    Sqlite(DbSqliteConfig),
}

impl Default for DbConfig {
    fn default() -> Self {
        DbConfig::Sqlite(DbSqliteConfig {
            database: "golem_component_service.db".to_string(),
            max_connections: 10,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct DbPostgresConfig {
    pub host: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub port: u16,
    pub max_connections: u32,
    pub schema: Option<String>,
}

impl ComponentServiceConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/component-service.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self {
            enable_tracing_console: false,
            enable_json_log: false,
            http_port: 8081,
            grpc_port: 9091,
            db: DbConfig::default(),
            component_store: ComponentStoreConfig::default(),
            compilation: ComponentCompilationConfig::default(),
            cloud_service: CloudServiceConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
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
            access_token: Uuid::new_v4(),
            retries: RetryConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn config_is_loadable() {
        std::env::set_var("GOLEM__ENVIRONMENT", "dev");
        std::env::set_var("GOLEM__WORKSPACE", "test");
        std::env::set_var("GOLEM__DB__TYPE", "Postgres");
        std::env::set_var("GOLEM__DB__CONFIG__USERNAME", "postgres");
        std::env::set_var("GOLEM__DB__CONFIG__PASSWORD", "postgres");
        std::env::set_var("GOLEM__CLOUD_SERVICE__HOST", "localhost");
        std::env::set_var("GOLEM__CLOUD_SERVICE__PORT", "7899");
        std::env::set_var(
            "GOLEM__CLOUD_SERVICE__ACCESS_TOKEN",
            "5C832D93-FF85-4A8F-9803-513950FDFDB1",
        );
        let _ = super::ComponentServiceConfig::new();
    }
}
