use golem_rust::bindings::wasi::config::store as wasi_config;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait WasiConfig {
    fn new(name: String) -> Self;
    fn get(&self, key: String) -> Option<String>;
    fn get_all(&self) -> Vec<(String, String)>;
}

pub struct WasiConfigImpl {
    _name: String,
}

#[agent_implementation]
impl WasiConfig for WasiConfigImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn get(&self, key: String) -> Option<String> {
        wasi_config::get(&key).unwrap()
    }

    fn get_all(&self) -> Vec<(String, String)> {
        wasi_config::get_all().unwrap()
    }
}
