use std::time::Duration;

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Default)]
pub struct AdditionalGolemConfig {
    #[serde(with = "humantime_serde")]
    pub promise_poll_interval: Duration,
}

impl AdditionalGolemConfig {
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

#[cfg(test)]
mod tests {
    use golem_worker_executor_base::services::golem_config::{GolemConfig, ShardManagerServiceConfig};

    use crate::services::config::AdditionalGolemConfig;

    #[test]
    pub fn config_is_loadable() {
        // The following settings are always coming through environment variables:
        std::env::set_var("GOLEM__REDIS__HOST", "localhost");
        std::env::set_var("GOLEM__REDIS__PORT", "1234");
        std::env::set_var("GOLEM__REDIS__DATABASE", "1");
        std::env::set_var("GOLEM__TEMPLATE_SERVICE__CONFIG__HOST", "localhost");
        std::env::set_var("GOLEM__TEMPLATE_SERVICE__CONFIG__PORT", "1234");
        std::env::set_var("GOLEM__TEMPLATE_SERVICE__CONFIG__ACCESS_TOKEN", "token");
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
        std::env::set_var("GOLEM__BLOB_STORE_SERVICE__CONFIG__REGION", "us-east-1");
        std::env::set_var(
            "GOLEM__BLOB_STORE_SERVICE__BUCKET",
            "golem-compiled-components",
        );
        std::env::set_var("GOLEM__BLOB_STORE_SERVICE__CONFIG__OBJECT_PREFIX", "");
        std::env::set_var("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__HOST", "localhost");
        std::env::set_var("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT", "4567");
        std::env::set_var("GOLEM__PORT", "1234");
        std::env::set_var("GOLEM__HTTP_PORT", "1235");
        std::env::set_var("GOLEM__ENABLE_JSON_LOG", "true");

        // The rest can be loaded from the toml
        let golem_config = GolemConfig::new();
        let _ = AdditionalGolemConfig::new();

        let shard_manager_grpc_port = match &golem_config.shard_manager_service {
            ShardManagerServiceConfig::Grpc(config) => config.port,
            _ => panic!("Expected shard manager service to be grpc"),
        };
        assert_eq!(shard_manager_grpc_port, 4567);
    }

    #[test]
    pub fn local_config_is_loadable() {
        let _ = GolemConfig::from_file("config/worker-executor-local.toml");
        let _ = AdditionalGolemConfig::from_file("config/worker-executor-local.toml");
    }
}
