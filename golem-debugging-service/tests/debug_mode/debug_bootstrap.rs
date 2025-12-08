use crate::services::auth_service::TestAuthService;
use crate::services::worker_proxy::TestWorkerProxy;
use anyhow::Error;
use async_trait::async_trait;
use golem_debugging_service::additional_deps::AdditionalDeps;
use golem_debugging_service::debug_context::DebugContext;
use golem_debugging_service::debug_session::{DebugSessions, DebugSessionsDefault};
use golem_debugging_service::oplog::debug_oplog_service::DebugOplogService;
use golem_debugging_service::services::auth::AuthService;
use golem_debugging_service::{create_debug_wasmtime_linker, run_debug_worker_executor};
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::service::compiled_component::DefaultCompiledComponentService;
use golem_service_base::storage::blob::BlobStorage;
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::agent_types::AgentTypesService;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::events::Events;
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::{
    GolemConfig, ResourceLimitsConfig, ResourceLimitsDisabledConfig,
};
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor::services::oplog::OplogService;
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::rdbms;
use golem_worker_executor::services::resource_limits;
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
use golem_worker_executor::{Bootstrap, RunDetails};
use golem_worker_executor_test_utils::component_service::ComponentServiceLocalFileSystem;
use golem_worker_executor_test_utils::TestWorkerExecutor;
use prometheus::Registry;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::JoinSet;
use wasmtime::component::Linker;
use wasmtime::Engine;

// A test bootstrap which depends on the original
// bootstrap (inner) as much as possible except for auth service
pub struct TestDebuggingServerBootStrap {
    regular_worker_executor_context: TestWorkerExecutor,
}

impl TestDebuggingServerBootStrap {
    pub fn new(regular_worker_executor_context: TestWorkerExecutor) -> Self {
        Self {
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
        Arc::new(ActiveWorkers::<DebugContext>::new(&golem_config.memory))
    }

    fn create_component_service(
        &self,
        _golem_config: &GolemConfig,
        _registry_service: Arc<dyn RegistryService>,
        blob_storage: Arc<dyn BlobStorage>,
    ) -> Arc<dyn ComponentService> {
        Arc::new(ComponentServiceLocalFileSystem::new(
            &self
                .regular_worker_executor_context
                .deps
                .component_service_directory
                .clone(),
            10000,
            Duration::from_secs(3600),
            Arc::new(DefaultCompiledComponentService::new(blob_storage)),
        ))
    }

    async fn run_grpc_server(
        &self,
        _service_dependencies: All<DebugContext>,
        _lazy_worker_activator: Arc<LazyWorkerActivator<DebugContext>>,
        _join_set: &mut JoinSet<Result<(), Error>>,
    ) -> anyhow::Result<u16> {
        panic!("no debug server running")
    }

    async fn create_services(
        &self,
        active_workers: Arc<ActiveWorkers<DebugContext>>,
        engine: Arc<Engine>,
        linker: Arc<Linker<DebugContext>>,
        runtime: Handle,
        component_service: Arc<dyn ComponentService>,
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
        worker_activator: Arc<dyn WorkerActivator<DebugContext>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        _worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
        agent_types_service: Arc<dyn AgentTypesService>,
        registry_service: Arc<dyn RegistryService>,
    ) -> anyhow::Result<All<DebugContext>> {
        let auth_service: Arc<dyn AuthService> = Arc::new(TestAuthService::new(
            self.regular_worker_executor_context.context.clone(),
        ));

        // The bootstrap of debug server uses a worker proxy which bypasses the worker service
        // but talks to the real regular executor directly
        let worker_proxy = Arc::new(TestWorkerProxy::new(
            self.regular_worker_executor_context.client.clone(),
            self.regular_worker_executor_context
                .deps
                .component_writer
                .clone(),
            self.regular_worker_executor_context.context.clone(),
        ));

        let debug_sessions: Arc<dyn DebugSessions> = Arc::new(DebugSessionsDefault::default());

        let debug_oplog_service = Arc::new(DebugOplogService::new(
            Arc::clone(&oplog_service),
            Arc::clone(&debug_sessions),
        ));

        let additional_deps = AdditionalDeps::new(auth_service, debug_sessions);
        let resource_limits = resource_limits::configured(
            &ResourceLimitsConfig::Disabled(ResourceLimitsDisabledConfig {}),
            registry_service.clone(),
        );

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
            oplog_processor_plugin.clone(),
            resource_limits.clone(),
            agent_types_service.clone(),
            additional_deps.clone(),
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
            oplog_processor_plugin.clone(),
            resource_limits.clone(),
            agent_types_service.clone(),
            additional_deps.clone(),
        ));

        Ok(All::new(
            active_workers,
            agent_types_service,
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
            oplog_processor_plugin.clone(),
            resource_limits,
            additional_deps,
        ))
    }

    fn create_wasmtime_linker(&self, engine: &Engine) -> anyhow::Result<Linker<DebugContext>> {
        create_debug_wasmtime_linker(engine)
    }

    async fn run(
        &self,
        golem_config: GolemConfig,
        prometheus_registry: Registry,
        runtime: Handle,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> anyhow::Result<RunDetails> {
        run_debug_worker_executor(
            self,
            golem_config,
            ".*",
            prometheus_registry,
            runtime,
            join_set,
        )
        .await
    }
}
