use golem_rust::{agent_definition, agent_implementation};
use std::env::{args, vars};

#[agent_definition]
pub trait Environment {
    fn new(name: String) -> Self;
    fn get_environment(&self) -> Result<Vec<(String, String)>, String>;
    fn get_arguments(&self) -> Result<Vec<String>, String>;
    fn get_environment_p3(&self) -> Result<Vec<(String, String)>, String>;
}

pub struct EnvironmentImpl {
    _name: String,
}

#[agent_implementation]
impl Environment for EnvironmentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn get_environment(&self) -> Result<Vec<(String, String)>, String> {
        Ok(vars().collect::<Vec<_>>())
    }

    fn get_arguments(&self) -> Result<Vec<String>, String> {
        Ok(args().collect::<Vec<_>>())
    }

    // Reads the environment through the P3-native `wasi:cli/environment@0.3` import instead of
    // std (which lowers to the P2 import), verifying the P3 host path returns the enriched
    // worker environment as well.
    fn get_environment_p3(&self) -> Result<Vec<(String, String)>, String> {
        Ok(golem_rust::wasip3::cli::environment::get_environment())
    }
}
