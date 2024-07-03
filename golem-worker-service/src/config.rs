use golem_common::config::ConfigLoader;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;

pub fn make_config_loader() -> ConfigLoader<WorkerServiceBaseConfig> {
    ConfigLoader::new_with_examples("config/worker-service.toml".to_owned())
}

#[cfg(test)]
mod tests {
    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        make_config_loader().load().expect("Failed to load config");
    }
}
