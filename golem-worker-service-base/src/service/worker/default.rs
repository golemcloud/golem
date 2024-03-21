use std::future::Future;
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::auth::EmptyAuthCtx;
use crate::service::template::TemplateService;
use golem_api_grpc::proto::golem::workerexecutor::{
    self, CompletePromiseRequest, ConnectWorkerRequest, CreateWorkerRequest,
    GetInvocationKeyRequest, InterruptWorkerRequest, InvokeAndAwaitWorkerRequest,
    ResumeWorkerRequest,
};
use golem_api_grpc::proto::golem::{
    common::ResourceLimits, workerexecutor::worker_executor_client::WorkerExecutorClient,
};

use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::InvokeResult as ProtoInvokeResult;
use golem_common::model::{AccountId, CallingConvention, InvocationKey, TemplateId};
use golem_service_base::model::{
    GolemErrorUnknown, PromiseId, VersionedWorkerId, WorkerId, WorkerMetadata,
};
use golem_service_base::service::auth::WithNamespace;
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

use super::{ConnectWorkerStream, WorkerServiceError};

pub type WorkerResult<T, Namespace> = Result<WithNamespace<T, Namespace>, WorkerServiceError>;

#[async_trait]
pub trait WorkerService<Namespace, AuthCtx> {
    async fn get_by_id(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<VersionedWorkerId, Namespace>;

    async fn create(
        &self,
        worker_id: &WorkerId,
        template_version: i32,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<VersionedWorkerId, Namespace>;

    async fn connect(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream, Namespace>;

    async fn delete(&self, worker_id: &WorkerId, auth_ctx: &AuthCtx)
        -> WorkerResult<(), Namespace>;

    async fn get_invocation_key(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<InvocationKey, Namespace>;

    async fn invoke_and_await_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Value,
        calling_convention: &CallingConvention,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<Value, Namespace>;

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Vec<ProtoVal>,
        calling_convention: &CallingConvention,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<ProtoInvokeResult, Namespace>;

    async fn invoke_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Value,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(), Namespace>;

    async fn invoke_fn_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Vec<ProtoVal>,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(), Namespace>;

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<bool, Namespace>;

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(), Namespace>;

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<WorkerMetadata, Namespace>;

    async fn resume(&self, worker_id: &WorkerId, auth_ctx: &AuthCtx)
        -> WorkerResult<(), Namespace>;
}

#[derive(Clone)]
pub struct WorkerServiceDefault<Namespace, AuthCtx>
where
    Namespace: Metadata + Send + Sync,
    AuthCtx: Send + Sync,
{
    auth_service: Arc<dyn AuthService<AuthCtx, Namespace, TemplatePermission> + Send + Sync>,
    worker_executor_clients: Arc<dyn WorkerExecutorClients + Send + Sync>,
    template_service: Arc<dyn TemplateService + Send + Sync>,
    routing_table_service: Arc<dyn RoutingTableService + Send + Sync>,
}

impl<Namespace, AuthCtx> WorkerServiceDefault<Namespace, AuthCtx>
where
    Namespace: Metadata + Send + Sync,
    AuthCtx: Send + Sync,
{
    pub fn new(
        auth_service: Arc<dyn AuthService<AuthCtx, Namespace, TemplatePermission> + Send + Sync>,
        worker_executor_clients: Arc<dyn WorkerExecutorClients + Send + Sync>,
        template_service: Arc<dyn TemplateService + Send + Sync>,
        routing_table_service: Arc<dyn RoutingTableService + Send + Sync>,
    ) -> Self {
        Self {
            auth_service,
            worker_executor_clients,
            template_service,
            routing_table_service,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TemplatePermission {
    pub template: TemplateId,
    pub permission: Permission,
}

impl TemplatePermission {
    pub fn new(template: TemplateId, permission: Permission) -> Self {
        Self {
            template,
            permission,
        }
    }
}

// TODO: Replace with metadata map
// Should this be async trait? or too complicated?
#[async_trait]
pub trait Metadata {
    async fn get_metadata(&self) -> anyhow::Result<NamespaceMetadata>;
}

#[derive(Clone, Debug)]
pub struct NamespaceMetadata {
    pub account_id: Option<AccountId>,
    pub limits: Option<ResourceLimits>,
}

#[async_trait]
impl<Namespace, AuthCtx> WorkerService<Namespace, AuthCtx>
    for WorkerServiceDefault<Namespace, AuthCtx>
where
    Namespace: Metadata + Send + Sync,
    AuthCtx: Send + Sync,
{
    async fn get_by_id(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<VersionedWorkerId, Namespace> {
        // TODO: More granular permisssions.
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::View);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;

        Ok(WithNamespace::new(
            VersionedWorkerId {
                worker_id: worker_id.clone(),
                template_version_used: 0,
            },
            namespace,
        ))
    }

    async fn create(
        &self,
        worker_id: &WorkerId,
        template_version: i32,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<VersionedWorkerId, Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Create);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
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

        let worker_id = VersionedWorkerId {
            worker_id: worker_id.clone(),
            template_version_used: template_version,
        };

        Ok(WithNamespace::new(worker_id, namespace))
    }

    async fn connect(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream, Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::View);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;

        let metadata = namespace.get_metadata().await?;
        let stream = self
            .retry_on_invalid_shard_id(
                worker_id,
                &(worker_id.clone(), metadata),
                |worker_executor_client, (worker_id, metadata)| {
                    Box::pin(async move {
                        let response = match worker_executor_client
                            .connect_worker(ConnectWorkerRequest {
                                worker_id: Some(worker_id.clone().into()),
                                account_id: metadata.account_id.clone().map(|id| id.into()),
                                account_limits: metadata.limits.clone(),
                            })
                            .await
                        {
                            Ok(response) => Ok(response),
                            Err(status) => {
                                if status.code() == tonic::Code::NotFound {
                                    Err(WorkerServiceError::WorkerNotFound(
                                        worker_id.clone().into(),
                                    ))
                                } else {
                                    Err(WorkerServiceError::internal(status))
                                }
                            }
                        }
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                        Ok(ConnectWorkerStream::new(response.into_inner()))
                    })
                },
            )
            .await?;

        Ok(WithNamespace::new(stream, namespace))
    }

    async fn delete(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(), Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Delete);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;

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

        Ok(WithNamespace::new((), namespace))
    }

    async fn get_invocation_key(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<InvocationKey, Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Create);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
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
        Ok(WithNamespace::new(invocation_key, namespace))
    }

    async fn invoke_and_await_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Value,
        calling_convention: &CallingConvention,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<Value, Namespace> {
        let template_details = self
            .try_get_template_for_worker(worker_id, auth_ctx)
            .await?;

        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker("Failed to find the function".to_string())
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
            .map_err(|err| WorkerServiceError::TypeChecker(err.join(", ")))?;
        let WithNamespace {
            value: results_val,
            namespace,
        } = self
            .invoke_and_await_function_proto(
                worker_id,
                function_name,
                invocation_key,
                params_val,
                calling_convention,
                auth_ctx,
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
            .map_err(|err| WorkerServiceError::TypeChecker(err.join(", ")))?;

        Ok(WithNamespace::new(invoke_response_json, namespace))
    }

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Vec<ProtoVal>,
        calling_convention: &CallingConvention,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<ProtoInvokeResult, Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Create);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;
        let metadata = namespace.get_metadata().await?;

        let template_details = self
            .try_get_template_for_worker(worker_id, auth_ctx)
            .await?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker("Failed to find the function".to_string())
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
            .map_err(|err| WorkerServiceError::TypeChecker(err.join(", ")))?;

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
        Ok(WithNamespace::new(invoke_response, namespace))
    }

    async fn invoke_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Value,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(), Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Create);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;
        let _ = namespace.get_metadata().await?;

        let template_details = self
            .try_get_template_for_worker(worker_id, auth_ctx)
            .await?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker("Failed to find the function".to_string())
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
            .map_err(|err| WorkerServiceError::TypeChecker(err.join(", ")))?;
        self.invoke_fn_proto(worker_id, function_name.clone(), params_val, auth_ctx)
            .await?;

        Ok(WithNamespace::new((), namespace))
    }

    async fn invoke_fn_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Vec<ProtoVal>,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(), Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Create);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;
        let _ = namespace.get_metadata().await?;

        let template_details = self
            .try_get_template_for_worker(worker_id, auth_ctx)
            .await?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker("Failed to find the function".to_string())
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
            .map_err(|err| WorkerServiceError::TypeChecker(err.join(", ")))?;

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
        Ok(WithNamespace::new((), namespace))
    }

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<bool, Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Create);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;
        let _ = namespace.get_metadata().await?;

        let promise_id = PromiseId {
            worker_id: worker_id.clone(),
            oplog_idx: oplog_id,
        };

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
        Ok(WithNamespace::new(result, namespace))
    }

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(), Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Update);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;

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
        ).await?;

        Ok(WithNamespace::new((), namespace))
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<WorkerMetadata, Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::View);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
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

        Ok(WithNamespace::new(metadata, namespace))
    }

    async fn resume(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(), Namespace> {
        let permission = TemplatePermission::new(worker_id.template_id.clone(), Permission::Update);
        let namespace = self
            .auth_service
            .is_authorized(permission, auth_ctx)
            .await?;
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
        Ok(WithNamespace::new((), namespace))
    }
}

impl<Namespace, AuthCtx> WorkerServiceDefault<Namespace, AuthCtx>
where
    Namespace: Metadata + Send + Sync,
    AuthCtx: Send + Sync,
{
    async fn try_get_template_for_worker(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &AuthCtx,
    ) -> Result<Template, WorkerServiceError> {
        match self.get_metadata(worker_id, auth_ctx).await {
            Ok(result) => {
                let metadata = result.value;
                let template_version = metadata.template_version;
                let template_details = self
                    .template_service
                    .get_by_version(&worker_id.template_id, template_version)
                    .await?
                    .ok_or_else(|| {
                        WorkerServiceError::VersionedTemplateIdNotFound(VersionedTemplateId {
                            template_id: worker_id.template_id.clone(),
                            version: template_version,
                        })
                    })?;

                Ok(template_details)
            }
            Err(WorkerServiceError::WorkerNotFound(_)) => Ok(self
                .template_service
                .get_latest(&worker_id.template_id)
                .await?),
            Err(WorkerServiceError::Golem(GolemError::WorkerNotFound(_))) => Ok(self
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
    ) -> Result<Out, WorkerServiceError>
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
                            return Err(WorkerServiceError::Golem(other));
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
                    // let err = anyhow::Error::new(other);
                    return Err(WorkerServiceError::internal(other));
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
    // TODO: Change to display
    #[error("Failed to get routing table: {0:?}")]
    FailedToGetRoutingTable(RoutingTableError),
    #[error("Failed to connect to pod {0}")]
    FailedToConnectToPod(String),
}

#[derive(Default)]
pub struct WorkerServiceNoOp<Namespace> {
    pub namespace: Namespace,
}

#[async_trait]
impl<Namespace> WorkerService<Namespace, EmptyAuthCtx> for WorkerServiceNoOp<Namespace>
where
    Namespace: Clone + Send + Sync,
{
    async fn get_by_id(
        &self,
        worker_id: &WorkerId,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<VersionedWorkerId, Namespace> {
        Ok(WithNamespace::new(
            VersionedWorkerId {
                worker_id: worker_id.clone(),
                template_version_used: 0,
            },
            self.namespace.clone(),
        ))
    }

    async fn create(
        &self,
        worker_id: &WorkerId,
        _: i32,
        _: Vec<String>,
        _: HashMap<String, String>,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<VersionedWorkerId, Namespace> {
        Ok(WithNamespace::new(
            VersionedWorkerId {
                worker_id: worker_id.clone(),
                template_version_used: 0,
            },
            self.namespace.clone(),
        ))
    }

    async fn connect(
        &self,
        _: &WorkerId,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<ConnectWorkerStream, Namespace> {
        Err(WorkerServiceError::Internal(anyhow::Error::msg(
            "Not supported",
        )))
    }

    async fn delete(&self, _: &WorkerId, _: &EmptyAuthCtx) -> WorkerResult<(), Namespace> {
        Ok(WithNamespace::new((), self.namespace.clone()))
    }

    async fn get_invocation_key(
        &self,
        _: &WorkerId,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<InvocationKey, Namespace> {
        Ok(WithNamespace::new(
            InvocationKey {
                value: "".to_string(),
            },
            self.namespace.clone(),
        ))
    }

    async fn invoke_and_await_function(
        &self,
        _: &WorkerId,
        _: String,
        _: &InvocationKey,
        _: Value,
        _: &CallingConvention,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<Value, Namespace> {
        Ok(WithNamespace::new(Value::Null, self.namespace.clone()))
    }

    async fn invoke_and_await_function_proto(
        &self,
        _: &WorkerId,
        _: String,
        _: &InvocationKey,
        _: Vec<ProtoVal>,
        _: &CallingConvention,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<ProtoInvokeResult, Namespace> {
        Ok(WithNamespace::new(
            ProtoInvokeResult { result: vec![] },
            self.namespace.clone(),
        ))
    }

    async fn invoke_function(
        &self,
        _: &WorkerId,
        _: String,
        _: Value,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<(), Namespace> {
        Ok(WithNamespace::new((), self.namespace.clone()))
    }

    async fn invoke_fn_proto(
        &self,
        _: &WorkerId,
        _: String,
        _: Vec<ProtoVal>,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<(), Namespace> {
        Ok(WithNamespace::new((), self.namespace.clone()))
    }

    async fn complete_promise(
        &self,
        _: &WorkerId,
        _: u64,
        _: Vec<u8>,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<bool, Namespace> {
        Ok(WithNamespace::new(true, self.namespace.clone()))
    }

    async fn interrupt(
        &self,
        _: &WorkerId,
        _: bool,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<(), Namespace> {
        Ok(WithNamespace::new((), self.namespace.clone()))
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        _: &EmptyAuthCtx,
    ) -> WorkerResult<WorkerMetadata, Namespace> {
        Ok(WithNamespace::new(
            WorkerMetadata {
                worker_id: worker_id.clone(),
                args: vec![],
                env: Default::default(),
                status: golem_common::model::WorkerStatus::Running,
                template_version: 0,
                retry_count: 0,
            },
            self.namespace.clone(),
        ))
    }

    async fn resume(&self, _: &WorkerId, _: &EmptyAuthCtx) -> WorkerResult<(), Namespace> {
        Ok(WithNamespace::new((), self.namespace.clone()))
    }
}
