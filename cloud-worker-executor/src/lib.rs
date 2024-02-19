use std::sync::Arc;

use crate::context::Context;
use async_trait::async_trait;
use golem_worker_executor_base::durable_host::DurableWorkerCtx;
use golem_worker_executor_base::preview2::golem;
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_base::services::invocation_key::InvocationKeyService;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::OplogService;
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::recovery::RecoveryManagementDefault;
use golem_worker_executor_base::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc};
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::shard::ShardService;
use golem_worker_executor_base::services::shard_manager::ShardManagerService;
use golem_worker_executor_base::services::template::TemplateService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_activator::WorkerActivator;
use golem_worker_executor_base::services::All;
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::Bootstrap;
use prometheus::Registry;
use tokio::runtime::Handle;
use tracing::info;
use uuid::Uuid;
use wasmtime::component::Linker;
use wasmtime::Engine;

use crate::services::config::AdditionalGolemConfig;
use crate::services::{resource_limits, AdditionalDeps};

pub mod context;
pub mod metrics;
pub mod services;

struct ServerBootstrap {
    additional_golem_config: Arc<AdditionalGolemConfig>,
}

#[async_trait]
impl Bootstrap<Context> for ServerBootstrap {
    fn create_active_workers(&self, golem_config: &GolemConfig) -> Arc<ActiveWorkers<Context>> {
        Arc::new(ActiveWorkers::<Context>::bounded(
            golem_config.limits.max_active_instances,
            golem_config.active_workers.drop_when_full,
            golem_config.active_workers.ttl,
        ))
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
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
    ) -> anyhow::Result<All<Context>> {
        let additional_golem_config = self.additional_golem_config.clone();
        let resource_limits =
            resource_limits::configured(&self.additional_golem_config.resource_limits);

        let extra_deps = AdditionalDeps::new(additional_golem_config, resource_limits);
        let rpc = Arc::new(DirectWorkerInvocationRpc::new(
            Arc::new(RemoteInvocationRpc::new(
                golem_config.public_worker_api.uri(),
                golem_config
                    .public_worker_api
                    .access_token
                    .parse::<Uuid>()
                    .expect("Access token must be an UUID"),
            )),
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            template_service.clone(),
            worker_service.clone(),
            promise_service.clone(),
            golem_config.clone(),
            invocation_key_service.clone(),
            shard_service.clone(),
            shard_manager_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            oplog_service.clone(),
            scheduler_service.clone(),
            extra_deps.clone(),
        ));
        let recovery_management = Arc::new(RecoveryManagementDefault::new(
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            template_service.clone(),
            worker_service.clone(),
            oplog_service.clone(),
            promise_service.clone(),
            scheduler_service.clone(),
            invocation_key_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            rpc.clone(),
            golem_config.clone(),
            extra_deps.clone(),
        ));
        rpc.set_recovery_management(recovery_management.clone());

        Ok(All::new(
            active_workers,
            engine,
            linker,
            runtime,
            template_service,
            shard_manager_service,
            worker_service,
            promise_service,
            golem_config,
            invocation_key_service,
            shard_service,
            key_value_service,
            blob_store_service,
            oplog_service,
            recovery_management,
            rpc,
            scheduler_service,
            extra_deps,
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<Context>> {
        let mut linker =
            create_linker::<Context, DurableWorkerCtx<Context>>(engine, |x| &mut x.durable_ctx)?;
        golem::api::host::add_to_linker::<Context, DurableWorkerCtx<Context>>(&mut linker, |x| {
            &mut x.durable_ctx
        })?;
        golem_wasm_rpc::golem::rpc::types::add_to_linker::<Context, DurableWorkerCtx<Context>>(
            &mut linker,
            |x| &mut x.durable_ctx,
        )?;
        Ok(linker)
    }
}

pub async fn run(
    golem_config: GolemConfig,
    additional_golem_config: Arc<AdditionalGolemConfig>,
    prometheus_registry: Registry,
    runtime: Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Golem Worker Executor starting up...");
    Ok(ServerBootstrap {
        additional_golem_config,
    }
    .run(golem_config, prometheus_registry, runtime)
    .await?)
}
