use golem_rust::{agent_definition, agent_implementation};
use std::io;

#[agent_definition]
pub trait Io {
    fn new(name: String) -> Self;
    fn run(&self) -> Result<String, String>;
}

pub struct IoImpl {
    _name: String,
}

#[agent_implementation]
impl Io for IoImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn run(&self) -> Result<String, String> {
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("{:?}", e))
            .map(|_| line)
    }
}
