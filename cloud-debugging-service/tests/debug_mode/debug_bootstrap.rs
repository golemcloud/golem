use crate::services::auth_service::TestAuthService;
use crate::services::worker_proxy::TestWorkerProxy;
use crate::{get_component_cache_config, get_component_service_config, RegularExecutorTestContext};
use anyhow::Error;
use async_trait::async_trait;
use cloud_debugging_service::additional_deps::AdditionalDeps;
use cloud_debugging_service::auth::AuthService;
use cloud_debugging_service::debug_context::DebugContext;
use cloud_debugging_service::debug_session::{DebugSessions, DebugSessionsDefault};
use cloud_debugging_service::oplog::debug_oplog_service::DebugOplogService;
use cloud_debugging_service::{create_debug_wasmtime_linker, run_debug_server};
use golem_service_base::storage::blob::BlobStorage;
use golem_test_framework::components::worker_executor::provided::ProvidedWorkerExecutor;
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::events::Events;
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::{GolemConfig, ResourceLimitsConfig};
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor::services::oplog::OplogService;
use golem_worker_executor::services::plugins::{Plugins, PluginsObservations};
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc};
use golem_worker_executor::services::scheduler::SchedulerService;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::shard_manager::ShardManagerService;
use golem_worker_executor::services::worker::WorkerService;
use golem_worker_executor::services::worker_activator::{LazyWorkerActivator, WorkerActivator};
use golem_worker_executor::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor::services::worker_fork::DefaultWorkerFork;
use golem_worker_executor::services::worker_proxy::WorkerProxy;
use golem_worker_executor::services::All;
use golem_worker_executor::services::{component, rdbms};
use golem_worker_executor::services::{plugins, resource_limits};
use golem_worker_executor::{Bootstrap, DefaultGolemTypes};
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use wasmtime::component::Linker;
use wasmtime::Engine;

// A test bootstrap which depends on the original
// bootstrap (inner) as much as possible except for auth service
pub struct TestDebuggingServerBootStrap {
    regular_worker_executor_context: RegularExecutorTestContext,
}

impl TestDebuggingServerBootStrap {
    pub fn new(regular_worker_executor_context: RegularExecutorTestContext) -> Self {
        Self {
            regular_worker_executor_context,
        }
    }
}

#[async_trait]
impl Bootstrap<DebugContext<DefaultGolemTypes>> for TestDebuggingServerBootStrap {
    fn create_active_workers(
        &self,
        golem_config: &GolemConfig,
    ) -> Arc<ActiveWorkers<DebugContext<DefaultGolemTypes>>> {
        Arc::new(ActiveWorkers::<DebugContext<DefaultGolemTypes>>::new(
            &golem_config.memory,
        ))
    }

    fn create_plugins(
        &self,
        golem_config: &GolemConfig,
    ) -> (
        Arc<dyn Plugins<DefaultGolemTypes>>,
        Arc<dyn PluginsObservations>,
    ) {
        plugins::default_configured(&golem_config.plugin_service)
    }

    fn create_component_service(
        &self,
        golem_config: &GolemConfig,
        blob_storage: Arc<dyn BlobStorage + Send + Sync>,
        plugin_observations: Arc<dyn PluginsObservations>,
    ) -> Arc<dyn ComponentService<DefaultGolemTypes>> {
        component::configured(
            &get_component_service_config(),
            &get_component_cache_config(),
            &golem_config.compiled_component_service,
            blob_storage,
            plugin_observations,
        )
    }

    async fn run_grpc_server(
        &self,
        service_dependencies: All<DebugContext<DefaultGolemTypes>>,
        _lazy_worker_activator: Arc<LazyWorkerActivator<DebugContext<DefaultGolemTypes>>>,
        join_set: &mut JoinSet<Result<(), Error>>,
    ) -> anyhow::Result<u16> {
        run_debug_server(service_dependencies, join_set).await
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<DebugContext<DefaultGolemTypes>>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<DebugContext<DefaultGolemTypes>>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService<DefaultGolemTypes>>,
        shard_manager_service: Arc<dyn ShardManagerService>,
        worker_service: Arc<dyn WorkerService>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService>,
        promise_service: Arc<dyn PromiseService>,
        golem_config: Arc<GolemConfig>,
        shard_service: Arc<dyn ShardService>,
        key_value_service: Arc<dyn KeyValueService>,
        blob_store_service: Arc<dyn BlobStoreService>,
        rdbms_service: Arc<dyn rdbms::RdbmsService>,
        worker_activator: Arc<dyn WorkerActivator<DebugContext<DefaultGolemTypes>>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        _worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<DefaultGolemTypes>>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
    ) -> anyhow::Result<All<DebugContext<DefaultGolemTypes>>> {
        let auth_service: Arc<dyn AuthService> = Arc::new(TestAuthService);

        // The bootstrap of debug server uses a worker proxy which bypasses the worker service
        // but talks to the real regular executor directly
        let worker_proxy = Arc::new(TestWorkerProxy {
            worker_executor: Arc::new(ProvidedWorkerExecutor::new(
                "localhost".to_string(),
                self.regular_worker_executor_context.http_port(),
                self.regular_worker_executor_context.grpc_port(),
                true,
            )),
        });

        let debug_sessions: Arc<dyn DebugSessions> = Arc::new(DebugSessionsDefault::default());

        let debug_oplog_service = Arc::new(DebugOplogService::new(
            Arc::clone(&oplog_service),
            Arc::clone(&debug_sessions),
        ));

        let addition_deps = AdditionalDeps::new(auth_service, debug_sessions);
        let resource_limits = resource_limits::configured(&ResourceLimitsConfig::Disabled);

        let worker_fork = Arc::new(DefaultWorkerFork::new(
            Arc::new(RemoteInvocationRpc::new(
                worker_proxy.clone(),
                shard_service.clone(),
            )),
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            component_service.clone(),
            shard_manager_service.clone(),
            worker_service.clone(),
            worker_proxy.clone(),
            worker_enumeration_service.clone(),
            running_worker_enumeration_service.clone(),
            promise_service.clone(),
            golem_config.clone(),
            shard_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            rdbms_service.clone(),
            oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            resource_limits.clone(),
            addition_deps.clone(),
        ));

        let rpc = Arc::new(DirectWorkerInvocationRpc::new(
            Arc::new(RemoteInvocationRpc::new(
                worker_proxy.clone(),
                shard_service.clone(),
            )),
            active_workers.clone(),
            engine.clone(),
            linker.clone(),
            runtime.clone(),
            component_service.clone(),
            worker_fork.clone(),
            worker_service.clone(),
            worker_enumeration_service.clone(),
            running_worker_enumeration_service.clone(),
            promise_service.clone(),
            golem_config.clone(),
            shard_service.clone(),
            shard_manager_service.clone(),
            key_value_service.clone(),
            blob_store_service.clone(),
            rdbms_service.clone(),
            debug_oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            resource_limits.clone(),
            addition_deps.clone(),
        ));

        Ok(All::new(
            active_workers,
            engine,
            linker,
            runtime.clone(),
            component_service,
            shard_manager_service,
            worker_fork,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config.clone(),
            shard_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            debug_oplog_service,
            rpc,
            scheduler_service,
            worker_activator.clone(),
            worker_proxy.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            resource_limits,
            addition_deps,
        ))
    }

    fn create_wasmtime_linker(
        &self,
        engine: &Engine,
    ) -> anyhow::Result<Linker<DebugContext<DefaultGolemTypes>>> {
        create_debug_wasmtime_linker(engine)
    }
}
