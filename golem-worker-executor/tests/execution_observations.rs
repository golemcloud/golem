use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_worker_executor::durable_host::tool::ToolOwnerFailureMetadata;
use golem_worker_executor::services::HasActiveAgents;
use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::WorkerCtx;
use std::future::Future;
use std::pin::Pin;

/// Test conveniences over the existing execution and lifecycle APIs.
pub trait WorkerObservations {
    fn terminal_interrupt_pending_for_test(&self) -> bool;
    fn selected_owner_failure_for_test(
        &self,
    ) -> impl Future<Output = Option<ToolOwnerFailureMetadata>> + Send;
    fn await_interrupt_for_test(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>>;
    fn raw_interrupt_for_test(&self) -> tokio::sync::broadcast::Receiver<InterruptKind>;
    fn retained_cleanup_for_test(
        &self,
    ) -> impl Future<Output = Result<(), WorkerExecutorError>> + Send;
    fn join_accepted_stops_for_test(
        &self,
    ) -> impl Future<Output = Result<(), WorkerExecutorError>> + Send;
}

impl<Ctx: WorkerCtx> WorkerObservations for Worker<Ctx> {
    async fn selected_owner_failure_for_test(&self) -> Option<ToolOwnerFailureMetadata> {
        self.active_agents()
            .try_get_active_agent(&self.get_initial_worker_metadata().owned_agent_id())
            .await?
            .execution()
            .tool_operation_metadata()
            .owner_failure
    }

    fn terminal_interrupt_pending_for_test(&self) -> bool {
        *self.terminal_interrupt().borrow()
    }

    async fn retained_cleanup_for_test(&self) -> Result<(), WorkerExecutorError> {
        self.wait_for_unload_cleanup().await
    }

    async fn join_accepted_stops_for_test(&self) -> Result<(), WorkerExecutorError> {
        self.join_interrupt_establishment().await
    }

    fn await_interrupt_for_test(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.owner_runtime_resources()
            .execution_status()
            .read()
            .unwrap()
            .create_await_interrupt_signal()
    }

    fn raw_interrupt_for_test(&self) -> tokio::sync::broadcast::Receiver<InterruptKind> {
        let resources = self.owner_runtime_resources();
        let status = resources.execution_status();
        match &*status.read().unwrap() {
            golem_worker_executor::model::ExecutionStatus::Loading {
                interrupt_signal, ..
            }
            | golem_worker_executor::model::ExecutionStatus::Running {
                interrupt_signal, ..
            } => interrupt_signal.subscribe(),
            _ => panic!("expected a Loading or Running interrupt sender"),
        }
    }
}
