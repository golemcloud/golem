use golem_worker_service_base::app_config::WorkerServiceBaseConfig;

pub fn get_config() -> WorkerServiceBaseConfig {
    WorkerServiceBaseConfig::default()
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
        std::env::set_var("GOLEM__TEMPLATE_SERVICE__HOST", "localhost");
        std::env::set_var("GOLEM__TEMPLATE_SERVICE__PORT", "1234");
        std::env::set_var("GOLEM__CUSTOM_REQUEST_PORT", "1234");
        std::env::set_var(
            "GOLEM__TEMPLATE_SERVICE__ACCESS_TOKEN",
            "5C832D93-FF85-4A8F-9803-513950FDFDB1",
        );
        std::env::set_var("GOLEM__ROUTING_TABLE__HOST", "golem-shard-manager");
        std::env::set_var("GOLEM__ROUTING_TABLE__PORT", "1234");

        // The rest can be loaded from the toml
        let config = super::get_config();

        println!("config: {:?}", config);
    }
}
