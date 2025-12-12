use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    fork_worker_response, revert_worker_response, ForkWorkerRequest, RevertWorkerRequest,
};
use golem_common::base_model::OplogIndex;
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::worker::RevertWorkerTarget;
use golem_common::model::{IdempotencyKey, OwnedWorkerId, PromiseId, WorkerId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};
use golem_wasm::{ValueAndType, WitValue};
use golem_worker_executor::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use golem_worker_executor_test_utils::component_writer::FileSystemComponentWriter;
use golem_worker_executor_test_utils::TestContext;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tonic::transport::Channel;

// Worker Proxy will be internally used by fork functionality,
// Fork will be resuming the worker (the target) in the real executor by
// resuming it through worker proxy. A real worker proxy
// goes through worker service and start a regular worker executor
// Here the proxy implementation bypasses the worker service
// however place it in the real executor
pub struct TestWorkerProxy {
    client: WorkerExecutorClient<Channel>,
    component_service: Arc<FileSystemComponentWriter>,
    test_ctx: TestContext,
}

impl TestWorkerProxy {
    pub fn new(
        client: WorkerExecutorClient<Channel>,
        component_service: Arc<FileSystemComponentWriter>,
        test_ctx: TestContext,
    ) -> Self {
        Self {
            client,
            component_service,
            test_ctx,
        }
    }

    fn should_retry<R>(retry_count: &mut usize, result: &Result<R, tonic::Status>) -> bool {
        if let Err(status) = result {
            if *retry_count > 0 && status.code() == tonic::Code::Unavailable {
                *retry_count -= 1;
                return true;
            }
        }
        false
    }

    const RETRY_COUNT: usize = 5;
}

#[async_trait]
impl WorkerProxy for TestWorkerProxy {
    async fn start(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _caller_env: HashMap<String, String>,
        _caller_wasi_config_vars: BTreeMap<String, String>,
        _caller_account_id: &AccountId,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            WorkerExecutorError::unknown(
                "Not implemented in tests as debug service is not expected to call start through proxy",
            )
        ))
    }

    async fn invoke_and_await(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _idempotency_key: Option<IdempotencyKey>,
        _function_name: String,
        _function_params: Vec<WitValue>,
        _caller_worker_id: WorkerId,
        _caller_env: HashMap<String, String>,
        _caller_wasi_config_vars: BTreeMap<String, String>,
        _invocation_context_stack: InvocationContextStack,
        _caller_account_id: &AccountId,
    ) -> Result<Option<ValueAndType>, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            WorkerExecutorError::unknown(
                "Not implemented in tests as debug service is not expected to call invoke and await through proxy",
            )
        ))
    }

    async fn invoke(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _idempotency_key: Option<IdempotencyKey>,
        _function_name: String,
        _function_params: Vec<WitValue>,
        _caller_worker_id: WorkerId,
        _caller_env: HashMap<String, String>,
        _caller_wasi_config_vars: BTreeMap<String, String>,
        _invocation_context_stack: InvocationContextStack,
        _caller_account_id: &AccountId,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            WorkerExecutorError::unknown(
                "Not implemented in tests as debug service is not expected to call invoke and await through proxy",
            )
        ))
    }

    async fn update(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _target_version: ComponentRevision,
        _mode: UpdateMode,
        _caller_account_id: &AccountId,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            WorkerExecutorError::unknown(
                "Not implemented in tests as debug service is not expected to call invoke and await through proxy",
            )
        ))
    }

    async fn resume(
        &self,
        worker_id: &WorkerId,
        force: bool,
        caller_account_id: &AccountId,
    ) -> Result<(), WorkerProxyError> {
        let mut retry_count = Self::RETRY_COUNT;

        let component = self
            .component_service
            .get_latest_component_metadata(&worker_id.component_id)
            .await
            .unwrap();

        assert!(*caller_account_id == self.test_ctx.account_id);

        let auth_ctx = AuthCtx::User(UserAuthCtx {
            account_id: self.test_ctx.account_id,
            account_plan_id: self.test_ctx.account_plan_id,
            account_roles: self.test_ctx.account_roles.clone(),
        });

        let result = loop {
            let result = self
                .client
                .clone()
                .resume_worker(workerexecutor::v1::ResumeWorkerRequest {
                    worker_id: Some(worker_id.clone().into()),
                    environment_id: Some(component.environment_id.into()),
                    force: Some(force),
                    auth_ctx: Some(auth_ctx.clone().into()),
                })
                .await;

            if Self::should_retry(&mut retry_count, &result) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            } else {
                break result;
            }
        };

        let result = result?.into_inner();

        match result.result {
            None => Err(WorkerProxyError::InternalError(
                WorkerExecutorError::unknown("No result in resume worker response"),
            )),
            Some(workerexecutor::v1::resume_worker_response::Result::Success(_)) => Ok(()),
            Some(workerexecutor::v1::resume_worker_response::Result::Failure(error)) => Err(
                WorkerProxyError::InternalError(WorkerExecutorError::try_from(error).unwrap()),
            ),
        }
    }

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cutoff: &OplogIndex,
        caller_account_id: &AccountId,
    ) -> Result<(), WorkerProxyError> {
        let component = self
            .component_service
            .get_latest_component_metadata(&source_worker_id.component_id)
            .await
            .unwrap();

        assert!(*caller_account_id == self.test_ctx.account_id);

        let auth_ctx = AuthCtx::User(UserAuthCtx {
            account_id: self.test_ctx.account_id,
            account_plan_id: self.test_ctx.account_plan_id,
            account_roles: self.test_ctx.account_roles.clone(),
        });

        let result = self
            .client
            .clone()
            .fork_worker(ForkWorkerRequest {
                component_owner_account_id: Some(component.account_id.into()),
                environment_id: Some(component.environment_id.into()),
                source_worker_id: Some(source_worker_id.clone().into()),
                target_worker_id: Some(target_worker_id.clone().into()),
                oplog_index_cutoff: (*oplog_index_cutoff).into(),
                auth_ctx: Some(auth_ctx.into()),
            })
            .await?
            .into_inner()
            .result;

        match result {
            None => Err(WorkerProxyError::InternalError(
                WorkerExecutorError::unknown("No result in fork worker response"),
            )),
            Some(fork_worker_response::Result::Success(_)) => Ok(()),
            Some(fork_worker_response::Result::Failure(error)) => Err(
                WorkerProxyError::InternalError(WorkerExecutorError::try_from(error).unwrap()),
            ),
        }
    }

    async fn revert(
        &self,
        worker_id: &WorkerId,
        target: RevertWorkerTarget,
        caller_account_id: &AccountId,
    ) -> Result<(), WorkerProxyError> {
        let component = self
            .component_service
            .get_latest_component_metadata(&worker_id.component_id)
            .await
            .unwrap();

        assert!(*caller_account_id == self.test_ctx.account_id);

        let auth_ctx = AuthCtx::User(UserAuthCtx {
            account_id: self.test_ctx.account_id,
            account_plan_id: self.test_ctx.account_plan_id,
            account_roles: self.test_ctx.account_roles.clone(),
        });

        let result = self
            .client
            .clone()
            .revert_worker(RevertWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(component.environment_id.into()),
                target: Some(target.into()),
                auth_ctx: Some(auth_ctx.into()),
            })
            .await?
            .into_inner()
            .result;

        match result {
            None => Err(WorkerProxyError::InternalError(
                WorkerExecutorError::unknown("No result in revert worker response"),
            )),
            Some(revert_worker_response::Result::Success(_)) => Ok(()),
            Some(revert_worker_response::Result::Failure(error)) => Err(
                WorkerProxyError::InternalError(WorkerExecutorError::try_from(error).unwrap()),
            ),
        }
    }

    async fn complete_promise(
        &self,
        _promise_id: PromiseId,
        _data: Vec<u8>,
        _caller_account_id: &AccountId,
    ) -> Result<bool, WorkerProxyError> {
        unimplemented!()
    }
}
