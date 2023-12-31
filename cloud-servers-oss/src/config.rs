use cloud_servers_base::routing_table::RoutingTableConfig;
use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct TemplateStoreLocalConfig {
    pub root_path: String,
    pub object_prefix: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TemplatesConfig {
    pub store: TemplateStoreLocalConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DbSqliteConfig {
    pub database: String,
    pub max_connections: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CloudServiceConfig {
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
