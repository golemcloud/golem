use golem_worker_service_base::app_config::WorkerServiceBaseConfig;

pub fn get_config() -> WorkerServiceBaseConfig {
    WorkerServiceBaseConfig::new()
}

#[cfg(test)]
mod tests {
    #[test]
    pub fn config_is_loadable() {
        let _ = super::get_config();
    }
}
