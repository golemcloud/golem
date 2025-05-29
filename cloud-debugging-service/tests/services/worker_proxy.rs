use async_trait::async_trait;
use golem_api_grpc::proto::golem::common::AccountId;
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    fork_worker_response, revert_worker_response, ForkWorkerRequest, RevertWorkerRequest,
};
use golem_common::base_model::OplogIndex;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{ComponentVersion, IdempotencyKey, OwnedWorkerId, WorkerId};
use golem_service_base::model::RevertWorkerTarget;
use golem_test_framework::components::worker_executor::WorkerExecutor;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::WitValue;
use golem_worker_executor::error::GolemError;
use golem_worker_executor::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use std::collections::HashMap;
use std::sync::Arc;

// Worker Proxy will be internally used by fork functionality,
// Fork will be resuming the worker (the target) in the real executor by
// resuming it through worker proxy. A real worker proxy
// goes through worker service and start a regular worker executor
// Here the proxy implementation bypasses the worker service
// however place it in the real executor
pub struct TestWorkerProxy {
    pub worker_executor: Arc<dyn WorkerExecutor + Send + Sync + 'static>,
}

impl TestWorkerProxy {
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
    async fn invoke_and_await(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _idempotency_key: Option<IdempotencyKey>,
        _function_name: String,
        _function_params: Vec<WitValue>,
        _caller_worker_id: WorkerId,
        _caller_args: Vec<String>,
        _caller_env: HashMap<String, String>,
        _invocation_context_stack: InvocationContextStack,
    ) -> Result<TypeAnnotatedValue, WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            GolemError::unknown(
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
        _caller_args: Vec<String>,
        _caller_env: HashMap<String, String>,
        _invocation_context_stack: InvocationContextStack,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            GolemError::unknown(
                "Not implemented in tests as debug service is not expected to call invoke and await through proxy",
            )
        ))
    }

    async fn update(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _target_version: ComponentVersion,
        _mode: UpdateMode,
    ) -> Result<(), WorkerProxyError> {
        Err(WorkerProxyError::InternalError(
            GolemError::unknown(
                "Not implemented in tests as debug service is not expected to call invoke and await through proxy",
            )
        ))
    }

    async fn resume(&self, worker_id: &WorkerId, force: bool) -> Result<(), WorkerProxyError> {
        let mut retry_count = Self::RETRY_COUNT;
        let worker_id: golem_api_grpc::proto::golem::worker::WorkerId = worker_id.clone().into();

        let result = loop {
            let result = self
                .worker_executor
                .client()
                .await
                .map_err(|e| WorkerProxyError::InternalError(GolemError::from(e)))?
                .resume_worker(workerexecutor::v1::ResumeWorkerRequest {
                    worker_id: Some(worker_id.clone()),
                    account_id: Some(AccountId {
                        name: "test-account".to_string(),
                    }),
                    force: Some(force),
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
            None => Err(WorkerProxyError::InternalError(GolemError::unknown(
                "No result in resume worker response",
            ))),
            Some(workerexecutor::v1::resume_worker_response::Result::Success(_)) => Ok(()),
            Some(workerexecutor::v1::resume_worker_response::Result::Failure(error)) => Err(
                WorkerProxyError::InternalError(GolemError::try_from(error).unwrap()),
            ),
        }
    }

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cutoff: &OplogIndex,
    ) -> Result<(), WorkerProxyError> {
        let result = self
            .worker_executor
            .client()
            .await
            .map_err(|e| WorkerProxyError::InternalError(GolemError::from(e)))?
            .fork_worker(ForkWorkerRequest {
                account_id: Some(AccountId {
                    name: "test-account".to_string(),
                }),
                source_worker_id: Some(source_worker_id.clone().into()),
                target_worker_id: Some(target_worker_id.clone().into()),
                oplog_index_cutoff: (*oplog_index_cutoff).into(),
            })
            .await?
            .into_inner()
            .result;

        match result {
            None => Err(WorkerProxyError::InternalError(GolemError::unknown(
                "No result in fork worker response",
            ))),
            Some(fork_worker_response::Result::Success(_)) => Ok(()),
            Some(fork_worker_response::Result::Failure(error)) => Err(
                WorkerProxyError::InternalError(GolemError::try_from(error).unwrap()),
            ),
        }
    }

    async fn revert(
        &self,
        worker_id: WorkerId,
        target: RevertWorkerTarget,
    ) -> Result<(), WorkerProxyError> {
        let result = self
            .worker_executor
            .client()
            .await
            .map_err(|e| WorkerProxyError::InternalError(GolemError::from(e)))?
            .revert_worker(RevertWorkerRequest {
                worker_id: Some(worker_id.into()),
                account_id: Some(AccountId {
                    name: "test-account".to_string(),
                }),
                target: Some(target.into()),
            })
            .await?
            .into_inner()
            .result;

        match result {
            None => Err(WorkerProxyError::InternalError(GolemError::unknown(
                "No result in revert worker response",
            ))),
            Some(revert_worker_response::Result::Success(_)) => Ok(()),
            Some(revert_worker_response::Result::Failure(error)) => Err(
                WorkerProxyError::InternalError(GolemError::try_from(error).unwrap()),
            ),
        }
    }
}
