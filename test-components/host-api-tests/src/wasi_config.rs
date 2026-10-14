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
    fn get_result(&self, key: String) -> Result<Option<String>, String>;
    fn get_all_result(&self) -> Result<Vec<(String, String)>, String>;
    fn config_probe(&self, operation: String, key: String) -> Result<(), String>;
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

    fn get_result(&self, key: String) -> Result<Option<String>, String> {
        wasi_config::get(&key).map_err(|error| format!("{error:?}"))
    }

    fn get_all_result(&self) -> Result<Vec<(String, String)>, String> {
        wasi_config::get_all().map_err(|error| format!("{error:?}"))
    }

    fn config_probe(&self, operation: String, key: String) -> Result<(), String> {
        match operation.as_str() {
            "get" => wasi_config::get(&key).map(|_| ()),
            "get-all" => wasi_config::get_all().map(|_| ()),
            _ => return Err(format!("unknown config operation: {operation}")),
        }
        .map_err(|error| format!("{error:?}"))
    }
}
