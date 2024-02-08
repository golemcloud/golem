// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod context;
pub mod services;

use std::sync::Arc;

use async_trait::async_trait;
use golem_worker_executor_base::durable_host::DurableWorkerCtx;
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_base::services::invocation_key::InvocationKeyService;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::OplogService;
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::recovery::RecoveryManagementDefault;
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::shard::ShardService;
use golem_worker_executor_base::services::shard_manager::ShardManagerService;
use golem_worker_executor_base::services::template::TemplateService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_activator::WorkerActivator;
use golem_worker_executor_base::services::All;
use golem_worker_executor_base::wasi_host::create_linker;
use golem_worker_executor_base::{durable_host, Bootstrap};
use prometheus::Registry;
use tokio::runtime::Handle;
use tracing::info;
use wasmtime::component::Linker;
use wasmtime::Engine;

use crate::context::Context;
use crate::services::AdditionalDeps;

struct ServerBootstrap {}

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
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
    ) -> anyhow::Result<All<Context>> {
        let additional_deps = AdditionalDeps {};
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
            golem_config.clone(),
            additional_deps.clone(),
        ));
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
            oplog_service,
            recovery_management,
            scheduler_service,
            additional_deps,
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<Context>> {
        let mut linker =
            create_linker::<Context, DurableWorkerCtx<Context>>(engine, |x| &mut x.durable_ctx)?;
        durable_host::host::add_to_linker::<Context, DurableWorkerCtx<Context>>(
            &mut linker,
            |x| &mut x.durable_ctx,
        )?;
        Ok(linker)
    }
}

pub async fn run(
    golem_config: GolemConfig,
    prometheus_registry: Registry,
    runtime: Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Golem Worker Executor starting up...");
    Ok(ServerBootstrap {}
        .run(golem_config, prometheus_registry, runtime)
        .await?)
}
