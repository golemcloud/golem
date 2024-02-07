use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;

use async_trait::async_trait;
use golem_common::config::RetryConfig;
use golem_common::model::{CallingConvention, InvocationKey, PromiseId, TemplateId, WorkerId};

use golem_api_grpc::proto::golem::worker::worker_error::Error;
use golem_api_grpc::proto::golem::worker::worker_execution_error;
use golem_api_grpc::proto::golem::worker::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    get_invocation_key_response, invoke_and_await_response_json, invoke_response,
    GetInvocationKeyRequest, InvokeAndAwaitRequestJson, InvokeRequestJson,
};
use golem_common::retries::with_retries;
use http::Uri;
use poem_openapi::types::ToJSON;
use tonic::Status;
use tracing::info;
use uuid::Uuid;
use crate::app_config::TemplateServiceConfig;
use crate::UriBackConversion;

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct WorkerName(pub String);

#[async_trait]
pub trait WorkerService {
    async fn get_invocation_key(
        &self,
        name: &WorkerName,
        template_id: &TemplateId,
    ) -> Result<InvocationKey, WorkerError>;

    async fn invoke_and_await(
        &self,
        name: &WorkerName,
        template_id: &TemplateId,
        function: String,
        parameters: serde_json::Value,
        invocation_key: InvocationKey,
        use_stdio: bool,
    ) -> Result<serde_json::Value, WorkerError>;

    async fn invoke(
        &self,
        name: &WorkerName,
        template_id: &TemplateId,
        function: String,
        parameters: serde_json::Value,
    ) -> Result<(), WorkerError>;
}

#[derive(Clone)]
pub struct WorkerServiceDefault {
    pub uri: Uri,
    pub access_token: Uuid,
    pub retry_config: RetryConfig,
}

impl WorkerServiceDefault {
    pub fn new(config: &TemplateServiceConfig) -> Self {
        Self {
            uri: config.uri(),
            access_token: config.access_token,
            retry_config: config.retries.clone(),
        }
    }
}

#[async_trait]
impl WorkerService for WorkerServiceDefault {
    async fn get_invocation_key(
        &self,
        name: &WorkerName,
        template_id: &TemplateId,
    ) -> Result<InvocationKey, WorkerError> {
        let desc = format!("Getting invocation key for {}/{}", template_id.0, name.0);
        info!("{}", &desc);
        with_retries(
            &desc,
            "workers",
            "get_invocation_key",
            &self.retry_config,
            &(
                self.uri.clone(),
                name.clone(),
                template_id.clone(),
                self.access_token,
            ),
            |(uri, worker_name, template_id, access_token)| {
                Box::pin(async move {
                    let mut client = WorkerServiceClient::connect(uri.as_http_02()).await?;

                    let request = GetInvocationKeyRequest {
                            worker_id: Some(
                                WorkerId {
                                    template_id: template_id.clone(),
                                    worker_name: worker_name.0.clone(),
                                }
                                    .into(),
                            ),
                        };

                    let response = client.get_invocation_key(request).await?.into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(get_invocation_key_response::Result::Success(invocation_key)) => {
                            Ok(invocation_key.into())
                        }
                        Some(get_invocation_key_response::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            WorkerError::is_retriable,
        )
            .await
    }

    async fn invoke_and_await(
        &self,
        name: &WorkerName,
        template_id: &TemplateId,
        function: String,
        parameters: serde_json::Value,
        invocation_key: InvocationKey,
        use_stdio: bool,
    ) -> Result<serde_json::Value, WorkerError> {
        let calling_convention = if use_stdio {
            CallingConvention::Stdio
        } else {
            CallingConvention::Component
        };
        let desc = format!(
            "Invoke and await for function {} in {}/{}",
            function, template_id.0, name.0
        );
        info!("{}", &desc);

        with_retries(
            &desc,
            "workers",
            "invoke_and_await",
            &self.retry_config,
            &(
                self.uri.clone(),
                name.0.clone(),
                template_id.clone(),
                function.clone(),
                parameters.clone(),
                invocation_key.clone(),
                calling_convention,
                self.access_token,
            ),
            |(
                 uri,
                 worker_name,
                 template_id,
                 function,
                 parameters,
                 invocation_key,
                 calling_convention,
                 access_token,
             )| {
                Box::pin(async move {
                    let mut client = WorkerServiceClient::connect(uri.as_http_02()).await?;
                    let request =     
                        InvokeAndAwaitRequestJson {
                            worker_id: Some(
                                WorkerId {
                                    template_id: template_id.clone(),
                                    worker_name: worker_name.clone(),
                                }
                                    .into(),
                            ),
                            invocation_key: Some(invocation_key.clone().into()),
                            function: function.clone(),
                            invoke_parameters_json: parameters.to_json_string(),
                            calling_convention: calling_convention.clone().into(),
                        },
                        access_token,
                    );

                    let response = client.invoke_and_await_json(request).await?.into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(invoke_and_await_response_json::Result::Success(result)) => {
                            let value: Result<serde_json::Value, String> =
                                serde_json::from_str(&result.result_json).map_err(|err| {
                                    format!("Failed to deserialize result JSON: {err}").to_string()
                                });
                            Ok(value?)
                        }
                        Some(invoke_and_await_response_json::Result::Error(error)) => {
                            Err(error.into())
                        }
                    }
                })
            },
            WorkerError::is_retriable,
        )
            .await
    }

    async fn invoke(
        &self,
        name: &WorkerName,
        template_id: &TemplateId,
        function: String,
        parameters: serde_json::Value,
    ) -> Result<(), WorkerError> {
        let desc = format!(
            "Invoke function {} in {}/{}",
            function, template_id.0, name.0
        );
        info!("{}", &desc);

        with_retries(
            &desc,
            "workers",
            "invoke_and_await",
            &self.retry_config,
            &(
                self.uri.clone(),
                name.0.clone(),
                template_id.clone(),
                function.clone(),
                parameters.clone(),
                self.access_token,
            ),
            |(uri, worker_name, template_id, function, parameters, access_token)| {
                Box::pin(async move {
                    let mut client = WorkerServiceClient::connect(uri.as_http_02()).await?;
                    let request = authorised_request(
                        InvokeRequestJson {
                            worker_id: Some(
                                WorkerId {
                                    template_id: template_id.clone(),
                                    worker_name: worker_name.clone(),
                                }
                                    .into(),
                            ),
                            function: function.clone(),
                            invoke_parameters_json: parameters.to_json_string(),
                        },
                        access_token,
                    );
                    let response = client.invoke_json(request).await?.into_inner();

                    match response.result {
                        None => Err("Empty response".to_string().into()),
                        Some(invoke_response::Result::Success(_)) => Ok(()),
                        Some(invoke_response::Result::Error(error)) => Err(error.into()),
                    }
                })
            },
            WorkerError::is_retriable,
        )
            .await
    }
}

#[derive(Debug)]
pub enum WorkerError {
    Server(golem_api_grpc::proto::golem::worker::WorkerError),
    Connection(Status),
    Transport(tonic::transport::Error),
    Unknown(String),
}

impl From<golem_api_grpc::proto::golem::worker::WorkerError> for WorkerError {
    fn from(value: golem_api_grpc::proto::golem::worker::WorkerError) -> Self {
        Self::Server(value)
    }
}

impl From<tonic::transport::Error> for WorkerError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl From<Status> for WorkerError {
    fn from(value: Status) -> Self {
        Self::Connection(value)
    }
}

impl From<String> for WorkerError {
    fn from(value: String) -> Self {
        Self::Unknown(value)
    }
}

impl WorkerError {
    fn is_retriable(&self) -> bool {
        matches!(self, WorkerError::Connection(_) | WorkerError::Transport(_))
    }
}

impl Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            WorkerError::Server(err) => match &err.error {
                Some(Error::BadRequest(errors)) => {
                    write!(f, "Invalid request: {:?}", errors.errors)
                }
                Some(Error::InternalError(golem_error)) => match &golem_error.error {
                    Some(worker_execution_error::Error::InvalidRequest(
                             golem_api_grpc::proto::golem::worker::InvalidRequest { details },
                         )) => write!(f, "Invalid request: {details}"),
                    Some(worker_execution_error::Error::WorkerAlreadyExists(
                             golem_api_grpc::proto::golem::worker::WorkerAlreadyExists { worker_id },
                         )) => write!(
                        f,
                        "Worker already exists: {}",
                        worker_id
                            .as_ref()
                            .and_then(|id| WorkerId::try_from(id.clone()).ok())
                            .map(|id| id.to_string())
                            .unwrap_or("?".to_string())
                    ),
                    Some(worker_execution_error::Error::WorkerNotFound(
                             golem_api_grpc::proto::golem::worker::WorkerNotFound { worker_id },
                         )) => write!(
                        f,
                        "Worker not found: {}",
                        worker_id
                            .as_ref()
                            .and_then(|id| WorkerId::try_from(id.clone()).ok())
                            .map(|id| id.to_string())
                            .unwrap_or("?".to_string())
                    ),
                    Some(worker_execution_error::Error::WorkerCreationFailed(
                             golem_api_grpc::proto::golem::worker::WorkerCreationFailed {
                                 worker_id,
                                 details,
                             },
                         )) => write!(
                        f,
                        "Failed to create worker: {}: {details}",
                        worker_id
                            .as_ref()
                            .and_then(|id| WorkerId::try_from(id.clone()).ok())
                            .map(|id| id.to_string())
                            .unwrap_or("?".to_string())
                    ),
                    Some(worker_execution_error::Error::FailedToResumeWorker(
                             golem_api_grpc::proto::golem::worker::FailedToResumeWorker { worker_id },
                         )) => write!(
                        f,
                        "Failed to resume worker: {}",
                        worker_id
                            .as_ref()
                            .and_then(|id| WorkerId::try_from(id.clone()).ok())
                            .map(|id| id.to_string())
                            .unwrap_or("?".to_string())
                    ),
                    Some(worker_execution_error::Error::TemplateDownloadFailed(
                             golem_api_grpc::proto::golem::worker::TemplateDownloadFailed {
                                 template_id,
                                 template_version,
                                 reason,
                             },
                         )) => write!(
                        f,
                        "Failed to download template: {}#{}: {reason}",
                        template_id
                            .as_ref()
                            .and_then(|id| TemplateId::try_from(id.clone()).ok())
                            .map(|id| id.to_string())
                            .unwrap_or("?".to_string()),
                        template_version
                    ),
                    Some(worker_execution_error::Error::TemplateParseFailed(
                             golem_api_grpc::proto::golem::worker::TemplateParseFailed {
                                 template_id,
                                 template_version,
                                 reason,
                             },
                         )) => write!(
                        f,
                        "Failed to parse downloaded template: {}#{}: {reason}",
                        template_id
                            .as_ref()
                            .and_then(|id| TemplateId::try_from(id.clone()).ok())
                            .map(|id| id.to_string())
                            .unwrap_or("?".to_string()),
                        template_version
                    ),
                    Some(worker_execution_error::Error::GetLatestVersionOfTemplateFailed(
                             golem_api_grpc::proto::golem::worker::GetLatestVersionOfTemplateFailed {
                                 template_id,
                                 reason,
                             },
                         )) => write!(
                        f,
                        "Failed to get latest version of template {}: {reason}",
                        template_id
                            .as_ref()
                            .and_then(|id| TemplateId::try_from(id.clone()).ok())
                            .map(|id| id.to_string())
                            .unwrap_or("?".to_string()),
                    ),
                    Some(worker_execution_error::Error::PromiseNotFound(
                             golem_api_grpc::proto::golem::worker::PromiseNotFound { promise_id },
                         )) => {
                        let promise_id: Option<PromiseId> = promise_id
                            .as_ref()
                            .and_then(|id| id.clone().try_into().ok());
                        write!(
                            f,
                            "Promise not found: {}",
                            promise_id
                                .map(|id| id.to_string())
                                .unwrap_or("?".to_string())
                        )
                    }
                    Some(worker_execution_error::Error::PromiseDropped(
                             golem_api_grpc::proto::golem::worker::PromiseDropped { promise_id },
                         )) => {
                        let promise_id: Option<PromiseId> = promise_id
                            .as_ref()
                            .and_then(|id| id.clone().try_into().ok());
                        write!(
                            f,
                            "Promise dropped: {}",
                            promise_id
                                .map(|id| id.to_string())
                                .unwrap_or("?".to_string())
                        )
                    }
                    Some(worker_execution_error::Error::PromiseAlreadyCompleted(
                             golem_api_grpc::proto::golem::worker::PromiseAlreadyCompleted {
                                 promise_id,
                             },
                         )) => {
                        let promise_id: Option<PromiseId> = promise_id
                            .as_ref()
                            .and_then(|id| id.clone().try_into().ok());
                        write!(
                            f,
                            "Promise already completed: {}",
                            promise_id
                                .map(|id| id.to_string())
                                .unwrap_or("?".to_string())
                        )
                    }
                    Some(worker_execution_error::Error::Interrupted(
                             golem_api_grpc::proto::golem::worker::Interrupted {
                                 recover_immediately,
                             },
                         )) => {
                        if *recover_immediately {
                            write!(f, "Interrupted: simulated crash")
                        } else {
                            write!(f, "Interrupted")
                        }
                    }
                    Some(worker_execution_error::Error::ParamTypeMismatch(_)) => {
                        write!(f, "Parameter type mismatch")
                    }
                    Some(worker_execution_error::Error::NoValueInMessage(_)) => {
                        write!(f, "No value in message")
                    }
                    Some(worker_execution_error::Error::ValueMismatch(
                             golem_api_grpc::proto::golem::worker::ValueMismatch { details },
                         )) => write!(f, "Value mismatch: {details}"),
                    Some(worker_execution_error::Error::UnexpectedOplogEntry(
                             golem_api_grpc::proto::golem::worker::UnexpectedOplogEntry {
                                 expected,
                                 got,
                             },
                         )) => write!(f, "Unexpected oplog entry: expected {expected}, got {got}"),
                    Some(worker_execution_error::Error::RuntimeError(
                             golem_api_grpc::proto::golem::worker::RuntimeError { details },
                         )) => write!(f, "Runtime error: {details}"),
                    Some(worker_execution_error::Error::InvalidShardId(
                             golem_api_grpc::proto::golem::worker::InvalidShardId {
                                 shard_id,
                                 shard_ids,
                             },
                         )) => write!(
                        f,
                        "{} is not in shards {:?}",
                        shard_id
                            .as_ref()
                            .map(|id| id.value.to_string())
                            .unwrap_or("?".to_string()),
                        shard_ids
                    ),
                    Some(worker_execution_error::Error::InvalidAccount(_)) => {
                        write!(f, "Invalid account")
                    }
                    Some(worker_execution_error::Error::PreviousInvocationFailed(_)) => {
                        write!(f, "The previously invoked function failed")
                    }
                    Some(worker_execution_error::Error::PreviousInvocationExited(_)) => {
                        write!(f, "The previously invoked function exited")
                    }
                    Some(worker_execution_error::Error::Unknown(
                             golem_api_grpc::proto::golem::worker::UnknownError { details },
                         )) => {
                        write!(f, "Unknown error: {details}")
                    }
                    None => write!(f, "Unknown golem error"),
                },
                Some(Error::NotFound(error)) => write!(f, "Project not found: {}", error.error),
                Some(Error::Unauthorized(error)) => write!(f, "Unauthorized: {}", error.error),
                Some(Error::LimitExceeded(error)) => {
                    write!(f, "Worker limit reached: {}", error.error)
                }
                Some(Error::AlreadyExists(error)) => {
                    write!(f, "Worker already exists: {}", error.error)
                }
                None => write!(f, "Unknown error"),
            },
            WorkerError::Connection(status) => write!(f, "Connection error: {status}"),
            WorkerError::Transport(error) => write!(f, "Transport error: {error}"),
            WorkerError::Unknown(error) => write!(f, "Unknown error: {error}"),
        }
    }
}

impl std::error::Error for WorkerError {
    // TODO
    // fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    //     Some(&self.source)
    // }
}
