use golem_common::config::ConfigLoader;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use std::path::Path;

pub fn make_config_loader() -> ConfigLoader<WorkerServiceBaseConfig> {
    ConfigLoader::new_with_examples(Path::new("config/worker-service.toml"))
}

#[cfg(test)]
mod tests {
    use crate::config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        make_config_loader().load().expect("Failed to load config");
    }
}
