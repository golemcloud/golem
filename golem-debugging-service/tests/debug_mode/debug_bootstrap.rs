use crate::services::auth_service::TestAuthService;
use crate::services::worker_proxy::TestWorkerProxy;
use async_trait::async_trait;
use golem_debugging_service::additional_deps::AdditionalDeps;
use golem_debugging_service::create_debugging_service_services;
use golem_debugging_service::debug_context::DebugContext;
use golem_debugging_service::debug_session::{DebugSessions, DebugSessionsDefault};
use golem_debugging_service::services::auth::AuthService;
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::service::compiled_component::DefaultCompiledComponentService;
use golem_service_base::storage::blob::BlobStorage;
use golem_worker_executor::Bootstrap;
use golem_worker_executor::services::All;
use golem_worker_executor::services::active_workers::ActiveWorkers;
use golem_worker_executor::services::agent_types::AgentTypesService;
use golem_worker_executor::services::agent_webhooks::AgentWebhooksService;
use golem_worker_executor::services::blob_store::BlobStoreService;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::direct_invocation_auth::DirectInvocationAuthService;
use golem_worker_executor::services::environment_state::EnvironmentStateService;
use golem_worker_executor::services::events::Events;
use golem_worker_executor::services::file_loader::FileLoader;
use golem_worker_executor::services::golem_config::{EnvironmentStateServiceConfig, GolemConfig};
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::oplog::OplogService;
use golem_worker_executor::services::oplog::plugin::OplogProcessorPlugin;
use golem_worker_executor::services::promise::PromiseService;
use golem_worker_executor::services::quota::QuotaService;
use golem_worker_executor::services::rdbms::RdbmsService;
use golem_worker_executor::services::resource_limits::{ResourceLimits, ResourceLimitsDisabled};
use golem_worker_executor::services::scheduler::SchedulerService;
use golem_worker_executor::services::shard::ShardService;
use golem_worker_executor::services::shard_manager::ShardManagerService;
use golem_worker_executor::services::worker::WorkerService;
use golem_worker_executor::services::worker_activator::WorkerActivator;
use golem_worker_executor::services::worker_enumeration::{
    RunningWorkerEnumerationService, WorkerEnumerationService,
};
use golem_worker_executor::services::worker_proxy::WorkerProxy;
use golem_worker_executor_test_utils::TestWorkerExecutor;
use golem_worker_executor_test_utils::agent_deployments_service::DisabledEnvironmentStateService;
use golem_worker_executor_test_utils::component_service::ComponentServiceLocalFileSystem;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use wasmtime::Engine;
use wasmtime::component::Linker;

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
    fn create_environment_state_service(
        &self,
        _config: &EnvironmentStateServiceConfig,
        _registry_service: Arc<dyn RegistryService>,
    ) -> Arc<dyn EnvironmentStateService> {
        Arc::new(DisabledEnvironmentStateService)
    }

    fn create_shard_manager_service(
        &self,
        _shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
    ) -> Arc<dyn ShardManagerService> {
        Arc::new(golem_worker_executor::services::shard_manager::ShardManagerServiceSingleShard)
    }

    fn create_quota_service(
        &self,
        _shard_manager_client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
        _golem_config: &GolemConfig,
        _shutdown_token: tokio_util::sync::CancellationToken,
    ) -> Arc<dyn golem_worker_executor::services::quota::QuotaService> {
        Arc::new(golem_worker_executor::services::quota::UnlimitedQuotaService)
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

    fn create_resource_limits(
        &self,
        _golem_config: &GolemConfig,
        _registry_service: Arc<dyn RegistryService>,
        _shutdown_token: CancellationToken,
    ) -> Arc<dyn ResourceLimits> {
        Arc::new(ResourceLimitsDisabled)
    }

    fn create_worker_proxy(&self, _golem_config: &GolemConfig) -> Arc<dyn WorkerProxy> {
        // The bootstrap of debug server uses a worker proxy which bypasses the worker service
        // but talks to the real regular executor directly
        Arc::new(TestWorkerProxy::new(
            self.regular_worker_executor_context.client.clone(),
            self.regular_worker_executor_context
                .deps
                .component_writer
                .clone(),
            self.regular_worker_executor_context.context.clone(),
        ))
    }

    fn create_additional_deps(
        &self,
        _registry_service: Arc<dyn RegistryService>,
    ) -> AdditionalDeps {
        let auth_service: Arc<dyn AuthService> = Arc::new(TestAuthService::new(
            self.regular_worker_executor_context.context.clone(),
        ));
        let debug_sessions: Arc<dyn DebugSessions> = Arc::new(DebugSessionsDefault::default());

        AdditionalDeps::new(auth_service, debug_sessions)
    }

    async fn create_services(
        &self,
        direct_invocation_auth_service: Arc<dyn DirectInvocationAuthService>,
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
        rdbms_service: Arc<dyn RdbmsService>,
        worker_activator: Arc<dyn WorkerActivator<DebugContext>>,
        oplog_service: Arc<dyn OplogService>,
        scheduler_service: Arc<dyn SchedulerService>,
        worker_proxy: Arc<dyn WorkerProxy>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
        agent_types_service: Arc<dyn AgentTypesService>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        resource_limits: Arc<dyn ResourceLimits>,
        quota_service: Arc<dyn QuotaService>,
        additional_deps: AdditionalDeps,
        shutdown_token: tokio_util::sync::CancellationToken,
        http_connection_pool: Option<wasmtime_wasi_http::HttpConnectionPool>,
        websocket_connection_pool: golem_worker_executor::durable_host::websocket::WebSocketConnectionPool,
        leak_sentinel: Arc<()>,
    ) -> anyhow::Result<All<DebugContext>> {
        create_debugging_service_services(
            direct_invocation_auth_service,
            active_workers,
            engine,
            linker,
            runtime,
            component_service,
            shard_manager_service,
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            shard_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            worker_activator,
            oplog_service,
            scheduler_service,
            worker_proxy,
            events,
            file_loader,
            oplog_processor_plugin,
            agent_types_service,
            environment_state_service,
            agent_webhooks_service,
            resource_limits,
            quota_service,
            additional_deps,
            shutdown_token,
            http_connection_pool,
            websocket_connection_pool,
            leak_sentinel,
        )
        .await
    }
}
