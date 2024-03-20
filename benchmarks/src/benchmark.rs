use crate::context::{Context, ContextInfo};
use anyhow::Result;
use k8s_openapi::serde::{Deserialize, Serialize};
use std::time::Duration;
use testcontainers::clients;

pub struct BenchmarkConfig {
    pub iterations: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadResult {
    iterations: Vec<Duration>,
    average: Duration,
}

pub struct Benchmark<'docker_client, Config: Clone, State: Send + Sync + Clone> {
    pub make_context:
        Box<dyn FnOnce(&'docker_client clients::Cli, Config) -> Result<Context<'docker_client>>>,
    pub wait_for_startup: Box<dyn FnOnce(ContextInfo) -> Result<()>>,
    pub init_state: Box<dyn FnOnce() -> Option<State>>,
    pub prepare: Box<dyn Fn(ContextInfo, Config, Option<State>) -> Result<State>>,
    pub validate_state: Box<dyn Fn(ContextInfo, State) -> Result<()>>,
    pub warmup: Box<dyn Fn(ContextInfo, State) -> Result<()>>,
    pub workload: Box<dyn Fn(ContextInfo, State) -> Result<Duration>>,
    pub cleanup: Box<dyn FnOnce(Context) -> Result<()>>,
}

impl<'docker_client, Config: Clone, State: Send + Sync + Clone>
    Benchmark<'docker_client, Config, State>
{
    pub fn run(
        self,
        docker: &'docker_client clients::Cli,
        config: Config,
        benchmark_config: BenchmarkConfig,
    ) -> Result<WorkloadResult> {
        let Benchmark {
            make_context,
            wait_for_startup,
            init_state,
            prepare,
            validate_state,
            warmup,
            workload,
            cleanup,
        } = self;

        let context = make_context(docker, config.clone())?;
        wait_for_startup(context.info())?;
        let mut iterations: Vec<Duration> =
            Vec::with_capacity(benchmark_config.iterations as usize);
        let mut global_state = init_state();

        for _ in 0..benchmark_config.iterations {
            let state = (&prepare)(context.info(), config.clone(), global_state.clone())?;
            global_state = Some(state.clone());
            (&validate_state)(context.info(), state.clone())?;
            (&warmup)(context.info(), state.clone())?;
            let duration = (&workload)(context.info(), state.clone())?;
            iterations.push(duration);
        }

        cleanup(context)?;

        let average = iterations.iter().sum::<Duration>() / (iterations.len() as u32);

        Ok(WorkloadResult {
            iterations,
            average,
        })
    }
}
