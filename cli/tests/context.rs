pub mod db;
pub mod golem_service;
pub mod redis;
pub mod shard_manager;
pub mod worker;

use crate::context::db::{DbInfo, Postgres};
use crate::context::golem_service::{GolemService, GolemServiceInfo};
use crate::context::redis::{Redis, RedisInfo};
use crate::context::shard_manager::{ShardManager, ShardManagerInfo};
use crate::context::worker::{WorkerExecutors, WorkerExecutorsInfo};
use libtest_mimic::Failed;
use std::path::PathBuf;
use testcontainers::clients;

const NETWORK: &str = "golem_test_network";
const TAG: &str = "v0.0.60";

#[derive(Debug, Clone)]
pub struct EnvConfig {
    pub verbose: bool,
    pub on_ci: bool,
    pub quiet: bool,
    pub redis_key_prefix: String,
    pub wasi_root: PathBuf,
    pub local_golem: bool,
}

impl EnvConfig {
    pub fn from_env() -> EnvConfig {
        EnvConfig {
            verbose: std::env::var("CI").is_err(),
            on_ci: std::env::var("CI").is_ok(),
            quiet: std::env::var("QUIET").is_ok(),
            redis_key_prefix: std::env::var("REDIS_KEY_PREFIX").unwrap_or("".to_string()),
            wasi_root: PathBuf::from(
                std::env::var("GOLEM_TEST_TEMPLATES").unwrap_or("../test-templates".to_string()),
            ),
            local_golem: std::env::var("GOLEM_DOCKER_SERVICES").is_err(),
        }
    }
}

pub struct Context<'docker_client> {
    env: EnvConfig,
    postgres: Postgres<'docker_client>,
    redis: Redis<'docker_client>,
    shard_manager: ShardManager<'docker_client>,
    golem_service: GolemService<'docker_client>,
    worker_executors: WorkerExecutors<'docker_client>,
}

impl Context<'_> {
    pub fn start(docker: &clients::Cli) -> Result<Context, Failed> {
        let env_config = EnvConfig::from_env();
        let postgres = Postgres::start(docker, &env_config)?;
        let redis = Redis::make(docker, &env_config)?;
        let shard_manager = ShardManager::start(docker, &env_config, &redis.info())?;
        let golem_service =
            GolemService::start(docker, &env_config, &shard_manager.info(), &postgres.info())?;
        let worker_executors = WorkerExecutors::start(
            docker,
            &env_config,
            &redis.info(),
            &golem_service.info(),
            &shard_manager.info(),
        )?;

        Ok(Context {
            env: env_config,
            postgres,
            redis,
            shard_manager,
            golem_service,
            worker_executors,
        })
    }

    pub fn info(&self) -> ContextInfo {
        ContextInfo {
            env: self.env.clone(),
            db: Box::new(self.postgres.info()),
            redis: self.redis.info(),
            shard_manager: self.shard_manager.info(),
            golem_service: self.golem_service.info(),
            worker_executors: self.worker_executors.info(),
        }
    }
}

impl Drop for Context<'_> {
    fn drop(&mut self) {
        println!("Stopping Context")
    }
}

pub struct ContextInfo {
    pub env: EnvConfig,
    pub db: Box<dyn DbInfo + Sync + Send>,
    pub redis: RedisInfo,
    pub shard_manager: ShardManagerInfo,
    pub golem_service: GolemServiceInfo,
    pub worker_executors: WorkerExecutorsInfo,
}
