use figment::providers::{Env, Format, Toml};
use figment::Figment;
use golem_cloud_server_base::config::TemplateStoreConfig;
use golem_cloud_server_base::routing_table::RoutingTableConfig;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct TemplatesConfig {
    pub store: TemplateStoreConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DbSqliteConfig {
    pub database: String,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CloudServiceConfig {
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub http_port: u16,
    pub grpc_port: u16,
    pub db: DbSqliteConfig,
    pub templates: TemplatesConfig,
    pub routing_table: RoutingTableConfig,
}

impl CloudServiceConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/cloud-server.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }
}

impl Default for CloudServiceConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn config_is_loadable() {
        std::env::set_var("GOLEM__ROUTING_TABLE__HOST", "localhost");
        std::env::set_var("GOLEM__ROUTING_TABLE__PORT", "1234");
        std::env::set_var("GOLEM__TEMPLATES__STORE__TYPE", "Local");
        std::env::set_var(
            "GOLEM__TEMPLATES__STORE__CONFIG__ROOT_PATH",
            "template_store",
        );
        std::env::set_var("GOLEM__TEMPLATES__STORE__CONFIG__OBJECT_PREFIX", "");
        std::env::set_var("GOLEM__HTTP_PORT", "9001");
        std::env::set_var("GOLEM__GRPC_PORT", "9002");

        // The rest can be loaded from the toml
        let _ = super::CloudServiceConfig::new();
    }
}
