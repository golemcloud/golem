use golem_rust::{PromiseId, agent_definition, agent_implementation, await_promise};
use std::env::{args, vars};

#[agent_definition]
pub trait Environment {
    fn new(name: String) -> Self;
    fn create_release_promise(&self) -> PromiseId;
    fn get_environment(&self) -> Result<Vec<(String, String)>, String>;
    fn get_arguments(&self) -> Result<Vec<String>, String>;
    fn get_environment_p3(&self) -> Result<Vec<(String, String)>, String>;
    async fn get_environment_after_promise(
        &self,
        release: PromiseId,
        p3: bool,
    ) -> Result<Vec<(String, String)>, String>;
    async fn get_environment_before_promise(
        &self,
        release: PromiseId,
        p3: bool,
    ) -> Result<Vec<(String, String)>, String>;
}

pub struct EnvironmentImpl {
    _name: String,
}

#[agent_implementation]
impl Environment for EnvironmentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn create_release_promise(&self) -> PromiseId {
        golem_rust::create_promise()
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

    async fn get_environment_after_promise(
        &self,
        release: PromiseId,
        p3: bool,
    ) -> Result<Vec<(String, String)>, String> {
        await_promise(&release).await;
        Ok(Self::read_environment(p3))
    }

    async fn get_environment_before_promise(
        &self,
        release: PromiseId,
        p3: bool,
    ) -> Result<Vec<(String, String)>, String> {
        let environment = Self::read_environment(p3);
        await_promise(&release).await;
        Ok(environment)
    }
}

impl EnvironmentImpl {
    fn read_environment(p3: bool) -> Vec<(String, String)> {
        let environment = if p3 {
            golem_rust::wasip3::cli::environment::get_environment()
        } else {
            wasi::cli::environment::get_environment()
        };
        environment
    }
}
