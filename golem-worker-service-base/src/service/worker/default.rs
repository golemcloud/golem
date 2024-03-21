use std::future::Future;
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::service::template::TemplateService;
use golem_api_grpc::proto::golem::workerexecutor::{
    self, CompletePromiseRequest, CreateWorkerRequest, GetInvocationKeyRequest, InterruptWorkerRequest, InvokeAndAwaitWorkerRequest, ResumeWorkerRequest
};
use golem_api_grpc::proto::golem::{
    common::ResourceLimits, workerexecutor::worker_executor_client::WorkerExecutorClient,
};

use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::InvokeResult as ProtoInvokeResult;
use golem_common::model::{AccountId, CallingConvention, InvocationKey};
use golem_service_base::model::{GolemErrorUnknown, PromiseId, VersionedWorkerId, WorkerId, WorkerMetadata};
use golem_service_base::typechecker::{TypeCheckIn, TypeCheckOut};
use golem_service_base::{
    model::{
        GolemError, GolemErrorInvalidShardId, GolemErrorRuntimeError, Template, VersionedTemplateId,
    },
    routing_table::{RoutingTableError, RoutingTableService},
    service::auth::{AuthService, Permission},
    worker_executor_clients::WorkerExecutorClients,
};
use golem_wasm_ast::analysis::AnalysedFunctionResult;
use golem_wasm_rpc::protobuf::Val as ProtoVal;
use serde_json::Value;
use tokio::time::sleep;
use tonic::transport::Channel;
use tracing::{debug, info};

use super::WorkerServiceBaseError;

#[async_trait]
pub trait WorkerService<AuthCtx> {
    async fn get_by_id(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<VersionedWorkerId, WorkerServiceBaseError>;

    async fn create(
        &self,
        worker_id: &WorkerId,
        template_version: i32,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth_ctx: &AuthCtx,
    ) -> Result<VersionedWorkerId, WorkerServiceBaseError>;

    // async fn connect(
    //     &self,
    //     worker_id: &WorkerId,
    // ) -> Result<ConnectWorkerStream, WorkerServiceBaseError>;

    async fn delete(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerServiceBaseError>;

    async fn get_invocation_key(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<InvocationKey, WorkerServiceBaseError>;

    async fn invoke_and_await_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Value,
        calling_convention: &CallingConvention,
        auth_ctx: &AuthCtx,
    ) -> Result<Value, WorkerServiceBaseError>;

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Vec<ProtoVal>,
        calling_convention: &CallingConvention,
        auth_ctx: &AuthCtx,
    ) -> Result<ProtoInvokeResult, WorkerServiceBaseError>;

    async fn invoke_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Value,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerServiceBaseError>;

    async fn invoke_fn_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Vec<ProtoVal>,
        auth_ctx: &AuthCtx
    ) -> Result<(), WorkerServiceBaseError>;

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: i32,
        data: Vec<u8>,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerServiceBaseError>;

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerServiceBaseError>;

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<WorkerMetadata, WorkerServiceBaseError>;

    async fn resume(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerServiceBaseError>;
}

#[derive(Clone)]
pub struct WorkerServiceDefault<AuthCtx, Namespace>
where
    AuthCtx: Send + Sync,
    Namespace: Metadata + Send + Sync,
{
    auth_service: Arc<dyn AuthService<AuthCtx, Namespace> + Send + Sync>,
    worker_executor_clients: Arc<dyn WorkerExecutorClients + Send + Sync>,
    template_service: Arc<dyn TemplateService + Send + Sync>,
    routing_table_service: Arc<dyn RoutingTableService + Send + Sync>,
}

// TODO: Replace with metadata map
// Should this be async trait? or too complicated?
#[async_trait]
pub trait Metadata {
    async fn get_metadata(&self) -> anyhow::Result<NamespaceMetadata>;
    // async fn record_deletion(namespace: Namespace)
    // async fn record_creation(namespace: Namespace)
}

#[derive(Clone, Debug)]
pub struct NamespaceMetadata {
    pub account_id: Option<AccountId>,
    pub limits: Option<ResourceLimits>,
}

#[async_trait]
impl<AuthCtx, Namespace> WorkerService<AuthCtx> for WorkerServiceDefault<AuthCtx, Namespace>
where
    AuthCtx: Send + Sync,
    Namespace: Metadata + Send + Sync,
{
    async fn get_by_id(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<VersionedWorkerId, WorkerServiceBaseError> {
        // TODO: More granular permisssions.
        let _ = self
            .auth_service
            .is_authorized(Permission::View, auth_ctx)
            .await?;

        Ok(VersionedWorkerId {
            worker_id: worker_id.clone(),
            template_version_used: 0,
        })
    }

    async fn create(
        &self,
        worker_id: &WorkerId,
        template_version: i32,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth_ctx: &AuthCtx,
    ) -> Result<VersionedWorkerId, WorkerServiceBaseError> {
        let namespace = self
            .auth_service
            .is_authorized(Permission::Create, auth_ctx)
            .await?;

        let metadata = namespace.get_metadata().await?;

        self.retry_on_invalid_shard_id(
            &worker_id.clone(),
            &(worker_id.clone(), template_version, arguments, environment_variables, metadata),
            |worker_executor_client, (worker_id, template_version, args, env, metadata)| {
                Box::pin(async move {
                    let response: tonic::Response<workerexecutor::CreateWorkerResponse> = worker_executor_client
                        .create_worker(
                            CreateWorkerRequest {
                                worker_id: Some(worker_id.clone().into()),
                                template_version: *template_version,
                                args: args.clone(),
                                env: env.clone(),
                                account_id: metadata.account_id.clone().map(|id| id.into()),
                                account_limits: metadata.limits.clone(), 
                            }
                        )
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;

                    match response.into_inner() {
                        workerexecutor::CreateWorkerResponse {
                            result:
                            Some(workerexecutor::create_worker_response::Result::Success(_))
                        } => Ok(()),
                        workerexecutor::CreateWorkerResponse {
                            result:
                            Some(workerexecutor::create_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::CreateWorkerResponse { .. } => Err(GolemError::Unknown(GolemErrorUnknown {
                            details: "Empty response".to_string(),
                        }))
                    }
                })
            }).await?;

        Ok(VersionedWorkerId {
            worker_id: worker_id.clone(),
            template_version_used: template_version,
        })
    }

    async fn delete(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerServiceBaseError> {
        let _ = self
            .auth_service
            .is_authorized(Permission::Delete, auth_ctx)
            .await?;

        // let plan_limit = self.check_plan_limits(&worker_id.template_id).await?;
        self.retry_on_invalid_shard_id(
                worker_id,
                worker_id,
                |worker_executor_client, worker_id| {
                    Box::pin(async move {
                        let response = worker_executor_client
                            .delete_worker(golem_api_grpc::proto::golem::worker::WorkerId::from(
                                worker_id.clone(),
                            ))
                            .await
                            .map_err(|err| {
                                GolemError::RuntimeError(GolemErrorRuntimeError {
                                    details: err.to_string(),
                                })
                            })?;
                        match response.into_inner() {
                            workerexecutor::DeleteWorkerResponse {
                                result: Some(workerexecutor::delete_worker_response::Result::Success(_)),
                            } => Ok(()),
                            workerexecutor::DeleteWorkerResponse {
                                result: Some(workerexecutor::delete_worker_response::Result::Failure(err)),
                            } => Err(err.try_into().unwrap()),
                            workerexecutor::DeleteWorkerResponse { .. } => {
                                Err(GolemError::Unknown(GolemErrorUnknown {
                                    details: "Empty response".to_string(),
                                }))
                            }
                        }
                    })
                },
            )
            .await?;
        // self.update_account_workers(&plan_limit.account_id, -1)
        //     .await?;
        Ok(())
    }

    async fn get_invocation_key(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<InvocationKey, WorkerServiceBaseError> {
        let _ = self
            .auth_service
            .is_authorized(Permission::Create, auth_ctx)
            .await?;
        let invocation_key = self
                .retry_on_invalid_shard_id(worker_id, worker_id, |worker_executor_client, worker_id| {
                    Box::pin(async move {
                        let response = worker_executor_client
                            .get_invocation_key(GetInvocationKeyRequest {
                                worker_id: Some(worker_id.clone().into()),
                            })
                            .await
                            .map_err(|err| {
                                GolemError::RuntimeError(GolemErrorRuntimeError {
                                    details: err.to_string(),
                                })
                            })?;
                        match response.into_inner() {
                            workerexecutor::GetInvocationKeyResponse {
                                result:
                                Some(workerexecutor::get_invocation_key_response::Result::Success(
                                         workerexecutor::GetInvocationKeySuccess {
                                             invocation_key: Some(invocation_key),
                                         },
                                     )),
                            } => Ok(invocation_key.into()),
                            workerexecutor::GetInvocationKeyResponse {
                                result:
                                Some(workerexecutor::get_invocation_key_response::Result::Failure(err)),
                            } => Err(err.try_into().unwrap()),
                            workerexecutor::GetInvocationKeyResponse { .. } => {
                                Err(GolemError::Unknown(GolemErrorUnknown {
                                    details: "Empty response".to_string(),
                                }))
                            }
                        }
                    })
                })
                .await?;
        Ok(invocation_key)
    }

    async fn invoke_and_await_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Value,
        calling_convention: &CallingConvention,
        auth_ctx: &AuthCtx,
    ) -> Result<Value, WorkerServiceBaseError> {
        let template_details = self
            .try_get_template_for_worker(worker_id, auth_ctx)
            .await?;

        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerServiceBaseError::TypeChecker("Failed to find the function".to_string())
            })?;
        let params_val = params
            .validate_function_parameters(
                function_type
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                calling_convention.clone(),
            )
            .map_err(|err| WorkerServiceBaseError::TypeChecker(err.join(", ")))?;
        let results_val = self
            .invoke_and_await_function_proto(
                worker_id,
                function_name,
                invocation_key,
                params_val,
                calling_convention,
                auth_ctx
            )
            .await?;

        let function_results: Vec<AnalysedFunctionResult> = function_type
            .results
            .iter()
            .map(|x| x.clone().into())
            .collect();

        let invoke_response_json = results_val
            .result
            .validate_function_result(function_results, calling_convention.clone())
            .map_err(|err| WorkerServiceBaseError::TypeChecker(err.join(", ")))?;
        Ok(invoke_response_json)
    }

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Vec<ProtoVal>,
        calling_convention: &CallingConvention,
        auth_ctx: &AuthCtx,
    ) -> Result<ProtoInvokeResult, WorkerServiceBaseError> {
        let namespace = self
            .auth_service
            .is_authorized(Permission::Create, auth_ctx)
            .await?;

        let metadata = namespace.get_metadata().await?;

        let template_details = self.try_get_template_for_worker(worker_id, auth_ctx).await?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerServiceBaseError::TypeChecker("Failed to find the function".to_string())
            })?;
        let params_val = params
            .validate_function_parameters(
                function_type
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                calling_convention.clone(),
            )
            .map_err(|err| WorkerServiceBaseError::TypeChecker(err.join(", ")))?;

        let invoke_response = self.retry_on_invalid_shard_id(
            worker_id,
            &(worker_id.clone(), function_name, params_val, invocation_key.clone(), calling_convention.clone(), metadata),
            |worker_executor_client, (worker_id, function_name, params_val, invocation_key, calling_convention, metadata)| {
                Box::pin(async move {
                    let response = worker_executor_client.invoke_and_await_worker(
                        InvokeAndAwaitWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                            name: function_name.clone(),
                            input: params_val.clone(),
                            invocation_key: Some(invocation_key.clone().into()),
                            calling_convention: calling_convention.clone().into(),
                            account_id: metadata.account_id.clone().map(|id| id.into()),
                            account_limits: metadata.limits.clone(), 
                        }
                    ).await.map_err(|err| {
                        GolemError::RuntimeError(GolemErrorRuntimeError {
                            details: err.to_string(),
                        })
                    })?;
                    match response.into_inner() {
                        workerexecutor::InvokeAndAwaitWorkerResponse {
                            result:
                            Some(workerexecutor::invoke_and_await_worker_response::Result::Success(
                                     workerexecutor::InvokeAndAwaitWorkerSuccess {
                                         output,
                                     },
                                 )),
                        } => Ok(ProtoInvokeResult { result: output }),
                        workerexecutor::InvokeAndAwaitWorkerResponse {
                            result:
                            Some(workerexecutor::invoke_and_await_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::InvokeAndAwaitWorkerResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        ).await?;
        Ok(invoke_response)
    }

    async fn invoke_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Value,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerServiceBaseError> {
        let _ = self
            .auth_service
            .is_authorized(Permission::Create, auth_ctx)
            .await?;

        let template_details = self.try_get_template_for_worker(worker_id, auth_ctx).await?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerServiceBaseError::TypeChecker("Failed to find the function".to_string())
            })?;
        let params_val = params
            .validate_function_parameters(
                function_type
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                CallingConvention::Component,
            )
            .map_err(|err| WorkerServiceBaseError::TypeChecker(err.join(", ")))?;
        self.invoke_fn_proto(worker_id, function_name.clone(), params_val, auth_ctx)
            .await?;
        Ok(())
    }

    async fn invoke_fn_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Vec<ProtoVal>,
        auth_ctx: &AuthCtx
    ) -> Result<(), WorkerServiceBaseError> {
        let namespace = self
            .auth_service
            .is_authorized(Permission::Create, auth_ctx)
            .await?;

        let template_details = self.try_get_template_for_worker(worker_id, auth_ctx).await?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerServiceBaseError::TypeChecker("Failed to find the function".to_string())
            })?;
        let params_val = params
            .validate_function_parameters(
                function_type
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                CallingConvention::Component,
            )
            .map_err(|err| WorkerServiceBaseError::TypeChecker(err.join(", ")))?;

        let metadata = namespace.get_metadata().await?;

        self.retry_on_invalid_shard_id(
            worker_id,
            &(
                worker_id.clone(),
                function_name,
                params_val,
                metadata
            ),
            |worker_executor_client,
             (worker_id, function_name, params_val, metadata)| {
                Box::pin(async move {
                    let response = worker_executor_client
                        .invoke_worker(workerexecutor::InvokeWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                            name: function_name.clone(),
                            input: params_val.clone(),
                            account_id: metadata.account_id.clone().map(|id| id.into()),
                            account_limits: metadata.limits.clone(), 
                        })
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                    match response.into_inner() {
                        workerexecutor::InvokeWorkerResponse {
                            result: Some(workerexecutor::invoke_worker_response::Result::Success(_)),
                        } => Ok(()),
                        workerexecutor::InvokeWorkerResponse {
                            result: Some(workerexecutor::invoke_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::InvokeWorkerResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        )
            .await?;
        Ok(())
    }

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: i32,
        data: Vec<u8>,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, WorkerServiceBaseError> {
        let promise_id = PromiseId {
            worker_id: worker_id.clone(),
            oplog_idx: oplog_id,
        };
        let _ = self.auth_service.is_authorized(Permission::Create, auth_ctx);
        let result = self
            .retry_on_invalid_shard_id(
                worker_id,
                &(promise_id, data),
                |worker_executor_client, (promise_id, data)| {
                    Box::pin(async move {
                        let response = worker_executor_client
                            .complete_promise(CompletePromiseRequest {
                                promise_id: Some(promise_id.clone().into()),
                                data: data.clone(),
                            })
                            .await
                            .map_err(|err| {
                                GolemError::RuntimeError(GolemErrorRuntimeError {
                                    details: err.to_string(),
                                })
                            })?;
                        match response.into_inner() {
                            workerexecutor::CompletePromiseResponse {
                                result:
                                    Some(workerexecutor::complete_promise_response::Result::Success(
                                        success,
                                    )),
                            } => Ok(success.completed),
                            workerexecutor::CompletePromiseResponse {
                                result:
                                    Some(workerexecutor::complete_promise_response::Result::Failure(
                                        err,
                                    )),
                            } => Err(err.try_into().unwrap()),
                            workerexecutor::CompletePromiseResponse { .. } => {
                                Err(GolemError::Unknown(GolemErrorUnknown {
                                    details: "Empty response".to_string(),
                                }))
                            }
                        }
                    })
                },
            )
            .await?;
        Ok(result)
    }

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerServiceBaseError> {
        self.auth_service.is_authorized(Permission::Update, auth_ctx).await?;
    self.retry_on_invalid_shard_id(
        worker_id,
        worker_id,
        |worker_executor_client, worker_id| {
            Box::pin(async move {
                let response = worker_executor_client
                    .interrupt_worker(InterruptWorkerRequest {
                        worker_id: Some(worker_id.clone().into()),
                        recover_immediately,
                    })
                    .await
                    .map_err(|err| {
                        GolemError::RuntimeError(GolemErrorRuntimeError {
                            details: err.to_string(),
                        })
                    })?;
                match response.into_inner() {
                    workerexecutor::InterruptWorkerResponse {
                        result: Some(workerexecutor::interrupt_worker_response::Result::Success(_)),
                    } => Ok(()),
                    workerexecutor::InterruptWorkerResponse {
                        result: Some(workerexecutor::interrupt_worker_response::Result::Failure(err)),
                    } => Err(err.try_into().unwrap()),
                    workerexecutor::InterruptWorkerResponse { .. } => {
                        Err(GolemError::Unknown(GolemErrorUnknown {
                            details: "Empty response".to_string(),
                        }))
                    }
                }
            })
        },
    )
    .await?;
    Ok(())

    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<WorkerMetadata, WorkerServiceBaseError> {
        let _ = self
            .auth_service
            .is_authorized(Permission::View, auth_ctx)
            .await?;

        let metadata = self.retry_on_invalid_shard_id(
            worker_id,
            worker_id,
            |worker_executor_client, worker_id| {
                Box::pin(async move {
                    let response = worker_executor_client.get_worker_metadata(
                        golem_api_grpc::proto::golem::worker::WorkerId::from(worker_id.clone())
                    ).await.map_err(|err| {
                        GolemError::RuntimeError(GolemErrorRuntimeError {
                            details: err.to_string(),
                        })
                    })?;
                    match response.into_inner() {
                        workerexecutor::GetWorkerMetadataResponse {
                            result:
                            Some(workerexecutor::get_worker_metadata_response::Result::Success(metadata)),
                        } => Ok(metadata.try_into().unwrap()),
                        workerexecutor::GetWorkerMetadataResponse {
                            result:
                            Some(workerexecutor::get_worker_metadata_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::GetWorkerMetadataResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        ).await?;
        Ok(metadata)
    }

    async fn resume(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerServiceBaseError> {
        self.auth_service.is_authorized(Permission::Update, auth_ctx).await?;
        self.retry_on_invalid_shard_id(
            worker_id,
            worker_id,
            |worker_executor_client, worker_id| {
                Box::pin(async move {
                    let response = worker_executor_client
                        .resume_worker(ResumeWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                        })
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                    match response.into_inner() {
                        workerexecutor::ResumeWorkerResponse {
                            result: Some(workerexecutor::resume_worker_response::Result::Success(_)),
                        } => Ok(()),
                        workerexecutor::ResumeWorkerResponse {
                            result: Some(workerexecutor::resume_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::ResumeWorkerResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        )
        .await?;
        Ok(())
    }

}

impl<AuthCtx, Namespace> WorkerServiceDefault<AuthCtx, Namespace>
where
    AuthCtx: Send + Sync,
    Namespace: Metadata + Send + Sync,
{
    async fn try_get_template_for_worker(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<Template, WorkerServiceBaseError> {
        match self.get_metadata(worker_id, auth_ctx).await {
            Ok(metadata) => {
                let template_version = metadata.template_version;
                let template_details = self
                    .template_service
                    .get_by_version(&worker_id.template_id, template_version)
                    .await?
                    .ok_or_else(|| {
                        WorkerServiceBaseError::VersionedTemplateIdNotFound(VersionedTemplateId {
                            template_id: worker_id.template_id.clone(),
                            version: template_version,
                        })
                    })?;

                Ok(template_details)
            }
            Err(WorkerServiceBaseError::WorkerNotFound(_)) => Ok(self
                .template_service
                .get_latest(&worker_id.template_id)
                .await?),
            Err(WorkerServiceBaseError::Golem(GolemError::WorkerNotFound(_))) => Ok(self
                .template_service
                .get_latest(&worker_id.template_id)
                .await?),
            Err(other) => Err(other),
        }
    }

    async fn get_worker_executor_client(
        &self,
        worker_id: &WorkerId,
    ) -> Result<Option<WorkerExecutorClient<Channel>>, GetWorkerExecutorClientError> {
        let routing_table = self
            .routing_table_service
            .get_routing_table()
            .await
            .map_err(GetWorkerExecutorClientError::FailedToGetRoutingTable)?;
        match routing_table.lookup(worker_id) {
            None => Ok(None),
            Some(pod) => {
                let worker_executor_client = self
                    .worker_executor_clients
                    .lookup(pod.clone())
                    .await
                    .map_err(GetWorkerExecutorClientError::FailedToConnectToPod)?;
                Ok(Some(worker_executor_client))
            }
        }
    }

    async fn retry_on_invalid_shard_id<F, In, Out>(
        &self,
        worker_id: &WorkerId,
        i: &In,
        f: F,
    ) -> Result<Out, WorkerServiceBaseError>
    where
        F: for<'b> Fn(
            &'b mut WorkerExecutorClient<Channel>,
            &'b In,
        )
            -> Pin<Box<dyn Future<Output = Result<Out, GolemError>> + 'b + Send>>,
    {
        loop {
            match self.get_worker_executor_client(worker_id).await {
                Ok(Some(mut worker_executor_client)) => {
                    match f(&mut worker_executor_client, i).await {
                        Ok(result) => return Ok(result),
                        Err(GolemError::InvalidShardId(GolemErrorInvalidShardId {
                            shard_id,
                            shard_ids,
                        })) => {
                            info!("InvalidShardId: {} not in {:?}", shard_id, shard_ids);
                            info!("Invalidating routing table");
                            self.routing_table_service.invalidate_routing_table().await;
                            sleep(Duration::from_secs(1)).await;
                        }
                        Err(GolemError::RuntimeError(GolemErrorRuntimeError { details }))
                            if is_connection_failure(&details) =>
                        {
                            info!("Worker executor unavailable");
                            info!("Invalidating routing table and retrying immediately");
                            self.routing_table_service.invalidate_routing_table().await;
                        }
                        Err(other) => {
                            debug!("Got {:?}, not retrying", other);
                            return Err(WorkerServiceBaseError::Golem(other));
                        }
                    }
                }
                Ok(None) => {
                    info!("No active shards");
                    info!("Invalidating routing table");
                    self.routing_table_service.invalidate_routing_table().await;
                    sleep(Duration::from_secs(1)).await;
                }
                Err(GetWorkerExecutorClientError::FailedToGetRoutingTable(
                    RoutingTableError::Unexpected(details),
                )) if is_connection_failure(&details) => {
                    info!("Shard manager unavailable");
                    info!("Invalidating routing table and retrying in 1 seconds");
                    self.routing_table_service.invalidate_routing_table().await;
                    sleep(Duration::from_secs(1)).await;
                }
                Err(GetWorkerExecutorClientError::FailedToConnectToPod(details))
                    if is_connection_failure(&details) =>
                {
                    info!("Worker executor unavailable");
                    info!("Invalidating routing table and retrying immediately");
                    self.routing_table_service.invalidate_routing_table().await;
                }
                Err(other) => {
                    debug!("Got {}, not retrying", other);
                    let err = anyhow::Error::new(other);
                    return Err(WorkerServiceBaseError::Internal(err));
                }
            }
        }
    }
}

fn is_connection_failure(message: &str) -> bool {
    message.contains("UNAVAILABLE")
        || message.contains("CHANNEL CLOSED")
        || message.contains("transport error")
        || message.contains("Connection refused")
}

#[derive(Debug, thiserror::Error)]
enum GetWorkerExecutorClientError {
    // TODO: Add more details
    #[error("Failed to get routing table")]
    FailedToGetRoutingTable(RoutingTableError),
    #[error("Failed to connect to pod {0}")]
    FailedToConnectToPod(String),
}
