use crate::services::auth_service::TestAuthService;
use crate::services::worker_proxy::TestWorkerProxy;
use crate::RegularExecutorTestContext;
use anyhow::Error;
use async_trait::async_trait;
use cloud_common::config::RemoteCloudServiceConfig;
use cloud_debugging_service::additional_deps::AdditionalDeps;
use cloud_debugging_service::auth::AuthService;
use cloud_debugging_service::config::AdditionalDebugConfig;
use cloud_debugging_service::debug_context::DebugContext;
use cloud_debugging_service::debug_session::{DebugSessions, DebugSessionsDefault};
use cloud_debugging_service::oplog::debug_oplog_service::DebugOplogService;
use golem_common::model::component::ComponentOwner;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_common::model::RetryConfig;
use golem_test_framework::components::worker_executor::provided::ProvidedWorkerExecutor;
use golem_worker_executor_base::services::active_workers::ActiveWorkers;
use golem_worker_executor_base::services::blob_store::BlobStoreService;
use golem_worker_executor_base::services::component::ComponentService;
use golem_worker_executor_base::services::events::Events;
use golem_worker_executor_base::services::file_loader::FileLoader;
use golem_worker_executor_base::services::golem_config::GolemConfig;
use golem_worker_executor_base::services::key_value::KeyValueService;
use golem_worker_executor_base::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor_base::services::oplog::OplogService;
use golem_worker_executor_base::services::plugins::{Plugins, PluginsObservations};
use golem_worker_executor_base::services::promise::PromiseService;
use golem_worker_executor_base::services::rpc::{DirectWorkerInvocationRpc, RemoteInvocationRpc};
use golem_worker_executor_base::services::scheduler::SchedulerService;
use golem_worker_executor_base::services::shard::ShardService;
use golem_worker_executor_base::services::shard_manager::ShardManagerService;
use golem_worker_executor_base::services::worker::WorkerService;
use golem_worker_executor_base::services::worker_activator::{
    LazyWorkerActivator, WorkerActivator,
};
use golem_worker_executor_base::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor_base::services::worker_fork::DefaultWorkerFork;
use golem_worker_executor_base::services::worker_proxy::WorkerProxy;
use golem_worker_executor_base::services::All;
use golem_worker_executor_base::workerctx::WorkerCtx;
use golem_worker_executor_base::Bootstrap;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use wasmtime::component::Linker;
use wasmtime::Engine;

// A test bootstrap which depends on the original
// bootstrap (inner) as much as possible except for auth service
pub struct TestDebuggingServerBootStrap {
    inner: cloud_debugging_service::ServerBootstrap,
    regular_worker_executor_context: RegularExecutorTestContext,
}

impl TestDebuggingServerBootStrap {
    pub fn new(regular_worker_executor_context: RegularExecutorTestContext) -> Self {
        // This will be unused, and if used, produce error.
        // We can handle this better down the line, as these
        // are used to form the auth service in the real debug bootstrap,
        // however, we are providing a dummy auth service that doesn't
        // depend on it.
        let additional_config = AdditionalDebugConfig {
            cloud_service: RemoteCloudServiceConfig {
                host: "localhost".to_string(),
                port: 8080,
                access_token: "03494299-B515-4427-8C37-4C1C915679B7".parse().unwrap(),
                retries: RetryConfig::default(),
            },
        };

        Self {
            inner: cloud_debugging_service::ServerBootstrap { additional_config },
            regular_worker_executor_context,
        }
    }
}

#[async_trait]
impl Bootstrap<DebugContext> for TestDebuggingServerBootStrap {
    fn create_active_workers(
        &self,
        golem_config: &GolemConfig,
    ) -> Arc<ActiveWorkers<DebugContext>> {
        self.inner.create_active_workers(golem_config)
    }

    fn create_plugins(
        &self,
        golem_config: &GolemConfig,
    ) -> (
        Arc<
            dyn Plugins<
                    <<DebugContext as WorkerCtx>::ComponentOwner as ComponentOwner>::PluginOwner,
                    <DebugContext as WorkerCtx>::PluginScope,
                > + Send
                + Sync,
        >,
        Arc<dyn PluginsObservations + Send + Sync>,
    ) {
        self.inner.create_plugins(golem_config)
    }

    async fn run_server(
        &self,
        service_dependencies: All<DebugContext>,
        lazy_worker_activator: Arc<LazyWorkerActivator<DebugContext>>,
        join_set: &mut JoinSet<Result<(), Error>>,
    ) -> anyhow::Result<()> {
        self.inner
            .run_server(service_dependencies, lazy_worker_activator, join_set)
            .await
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<DebugContext>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<DebugContext>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService + Send + Sync>,
        shard_manager_service: Arc<dyn ShardManagerService + Send + Sync>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<dyn WorkerEnumerationService + Send + Sync>,
        running_worker_enumeration_service: Arc<dyn RunningWorkerEnumerationService + Send + Sync>,
        promise_service: Arc<dyn PromiseService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        key_value_service: Arc<dyn KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn BlobStoreService + Send + Sync>,
        worker_activator: Arc<dyn WorkerActivator<DebugContext> + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        scheduler_service: Arc<dyn SchedulerService + Send + Sync>,
        _worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<dyn Plugins<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
    ) -> anyhow::Result<All<DebugContext>> {
        let auth_service: Arc<dyn AuthService + Send + Sync> = Arc::new(TestAuthService);

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

        let debug_sessions: Arc<dyn DebugSessions + Sync + Send> =
            Arc::new(DebugSessionsDefault::default());

        let debug_oplog_service = Arc::new(DebugOplogService::new(
            Arc::clone(&oplog_service),
            Arc::clone(&debug_sessions),
        ));

        let addition_deps = AdditionalDeps::new(auth_service, debug_sessions);

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
            oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
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
            debug_oplog_service.clone(),
            scheduler_service.clone(),
            worker_activator.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
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
            debug_oplog_service,
            rpc,
            scheduler_service,
            worker_activator.clone(),
            worker_proxy.clone(),
            events.clone(),
            file_loader.clone(),
            plugins.clone(),
            oplog_processor_plugin.clone(),
            addition_deps,
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<DebugContext>> {
        self.inner.create_wasmtime_linker(engine)
    }
}
