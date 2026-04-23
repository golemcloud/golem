use golem_rust::agentic::Config;
use golem_rust::bindings::wasi::config::store as wasi_config;
use golem_rust::{ConfigSchema, agent_definition, agent_implementation};

#[derive(ConfigSchema)]
pub struct WasiConfigAgentConfig {
    pub k1: String,
    pub k2: String,
    pub k3: Option<String>,
    pub k4: Option<String>,
}

#[agent_definition]
pub trait WasiConfig {
    fn new(name: String, #[agent_config] _config: Config<WasiConfigAgentConfig>) -> Self;
    fn get(&self, key: String) -> Option<String>;
    fn get_all(&self) -> Vec<(String, String)>;
}

pub struct WasiConfigImpl {
    _name: String,
}

#[agent_implementation]
impl WasiConfig for WasiConfigImpl {
    fn new(name: String, #[agent_config] _config: Config<WasiConfigAgentConfig>) -> Self {
        Self { _name: name }
    }

    fn get(&self, key: String) -> Option<String> {
        wasi_config::get(&key).unwrap()
    }

    fn get_all(&self) -> Vec<(String, String)> {
        wasi_config::get_all().unwrap()
    }
}
