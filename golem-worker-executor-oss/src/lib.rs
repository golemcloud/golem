pub mod context;
pub mod host;
pub mod preview2;
pub mod services;

use std::sync::Arc;

use async_trait::async_trait;
use prometheus::Registry;
use tokio::runtime::Handle;
use tracing::info;
use wasmtime::component::Linker;
use wasmtime::Engine;
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_base::services::invocation_key::InvocationKeyService;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::shard::ShardService;
use golem_worker_executor_base::services::shard_manager::ShardManagerService;
use golem_worker_executor_base::services::template::TemplateService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_activator::WorkerActivator;
use golem_worker_executor_base::services::All;
use golem_worker_executor_base::Bootstrap;

use crate::context::{create_linker, Context};
use crate::services::config::AdditionalGolemConfig;
use crate::services::AdditionalDeps;

struct ServerBootstrap {
    additional_golem_config: Arc<AdditionalGolemConfig>,
}

#[async_trait]
impl Bootstrap<Context> for ServerBootstrap {
    fn create_active_workers(&self, _golem_config: &GolemConfig) -> Arc<ActiveWorkers<Context>> {
        Arc::new(ActiveWorkers::<Context>::unbounded())
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<Context>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<Context>>,
        runtime: Handle,
        template_service: Arc<dyn TemplateService + Send + Sync>,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
        invocation_key_service: Arc<dyn InvocationKeyService + Send + Sync>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        _worker_activator: Arc<dyn WorkerActivator + Send + Sync>,
    ) -> anyhow::Result<All<Context>> {
        Ok(All::new(
            active_workers,
            engine,
            linker,
            runtime.clone(),
            template_service,
            shard_manager_service,
            worker_service,
            promise_service,
            golem_config.clone(),
            invocation_key_service,
            shard_service,
            key_value_service,
            blob_store_service,
            AdditionalDeps::new(self.additional_golem_config.clone()),
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<Context>> {
        create_linker(engine)
    }
}

pub async fn run(
    golem_config: GolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
    additional_golem_config: Arc<AdditionalGolemConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Golem Worker Executor starting up...");
    Ok(ServerBootstrap {
        additional_golem_config,
    }
    .run(golem_config, prometheus_registry, runtime)
    .await?)
}
