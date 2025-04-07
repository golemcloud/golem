use crate::config::ProfileName;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use strum_macros::Display;

// NonSuccessfulExit is used to signal that an error got resolved with hints or error messages
// already on the command line, thus nothing should be printed in the main error handler,
// but should return non-successful exit code from the process.
#[derive(Debug)]
pub struct NonSuccessfulExit;

impl Display for NonSuccessfulExit {
    fn fmt(&self, _f: &mut Formatter<'_>) -> std::fmt::Result {
        //NOP
        Ok(())
    }
}

impl Error for NonSuccessfulExit {}

/// Errors that should be handled by the command handler with showing hints or error messages
#[derive(Debug, Display)]
pub enum HintError {
    NoApplicationManifestFound,
    ExpectedCloudProfile,
}

impl Error for HintError {}

#[derive(Debug, Display)]
pub enum ContextInitHintError {
    ProfileNotFound(ProfileName),
}

impl Error for ContextInitHintError {}

pub mod service {
    use crate::log::LogColorize;
    use crate::model::text::fmt::format_stack;
    use bytes::Bytes;
    use golem_client::model::GolemError;
    use golem_common::model::error::{
        GolemErrorComponentDownloadFailed, GolemErrorComponentParseFailed,
        GolemErrorFailedToResumeWorker, GolemErrorFileSystemError,
        GolemErrorGetLatestVersionOfComponentFailed, GolemErrorInitialComponentFileDownloadFailed,
        GolemErrorInterrupted, GolemErrorInvalidRequest, GolemErrorInvalidShardId,
        GolemErrorPromiseAlreadyCompleted, GolemErrorPromiseDropped, GolemErrorPromiseNotFound,
        GolemErrorRuntimeError, GolemErrorUnexpectedOplogEntry, GolemErrorUnknown,
        GolemErrorValueMismatch, GolemErrorWorkerAlreadyExists, GolemErrorWorkerCreationFailed,
        GolemErrorWorkerNotFound,
    };
    use golem_common::model::{PromiseId, WorkerId};
    use itertools::Itertools;
    use reqwest::StatusCode;
    use std::error::Error;
    use std::fmt::{Display, Formatter};

    #[derive(Debug)]
    pub struct ServiceErrorResponse {
        status_code: u16,
        message: String,
    }

    pub trait HasServiceName {
        fn service_name() -> &'static str;
    }

    #[derive(Debug)]
    pub struct ServiceError {
        service_name: &'static str,
        kind: ServiceErrorKind,
    }

    #[derive(Debug)]
    pub enum ServiceErrorKind {
        ErrorResponse(ServiceErrorResponse),
        ReqwestError(reqwest::Error),
        ReqwestHeaderError(reqwest::header::InvalidHeaderValue),
        SerdeError(serde_json::Error),
        UnexpectedResponse { status_code: u16, payload: Bytes },
    }

    impl Display for ServiceError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            fn format_status_code(status_code: u16) -> String {
                match StatusCode::from_u16(status_code) {
                    Ok(status_code) => status_code.to_string(),
                    Err(_) => status_code.to_string(),
                }
            }

            let service_name = format!("{} Service", self.service_name).log_color_highlight();

            match &self.kind {
                ServiceErrorKind::ErrorResponse(response) => {
                    write!(
                        f,
                        "{} - Error: {}, {}",
                        service_name,
                        format_status_code(response.status_code).log_color_error(),
                        response.message.log_color_warn()
                    )
                }
                ServiceErrorKind::ReqwestError(error) => {
                    write!(
                        f,
                        "{} - HTTP Client Error: {}",
                        service_name,
                        error.to_string().log_color_warn()
                    )
                }
                ServiceErrorKind::ReqwestHeaderError(error) => {
                    write!(
                        f,
                        "{} - HTTP Header Error: {}",
                        service_name,
                        error.to_string().log_color_warn()
                    )
                }
                ServiceErrorKind::SerdeError(error) => {
                    write!(
                        f,
                        "{} - Serialization Error: {}",
                        service_name,
                        error.to_string().log_color_warn()
                    )
                }
                ServiceErrorKind::UnexpectedResponse {
                    status_code,
                    payload,
                } => {
                    write!(
                        f,
                        "{} - Unexpected Response Error: {}, {}",
                        service_name,
                        format_status_code(*status_code).log_color_error(),
                        String::from_utf8_lossy(payload)
                            .to_string()
                            .log_color_warn()
                    )
                }
            }
        }
    }

    impl Error for ServiceError {}

    impl<T> From<golem_client::Error<T>> for ServiceError
    where
        T: Into<ServiceErrorResponse> + HasServiceName,
    {
        fn from(error: golem_client::Error<T>) -> Self {
            ServiceError {
                service_name: T::service_name(),
                kind: match error {
                    golem_client::Error::Item(error) => {
                        ServiceErrorKind::ErrorResponse(error.into())
                    }
                    golem_client::Error::Reqwest(error) => ServiceErrorKind::ReqwestError(error),
                    golem_client::Error::ReqwestHeader(error) => {
                        ServiceErrorKind::ReqwestHeaderError(error)
                    }
                    golem_client::Error::Serde(error) => ServiceErrorKind::SerdeError(error),
                    golem_client::Error::Unexpected { code, data } => {
                        ServiceErrorKind::UnexpectedResponse {
                            status_code: code,
                            payload: data,
                        }
                    }
                },
            }
        }
    }

    impl<T> From<golem_cloud_client::Error<T>> for ServiceError
    where
        T: Into<ServiceErrorResponse> + HasServiceName,
    {
        fn from(error: golem_cloud_client::Error<T>) -> Self {
            ServiceError {
                service_name: T::service_name(),
                kind: match error {
                    golem_cloud_client::Error::Item(error) => {
                        ServiceErrorKind::ErrorResponse(error.into())
                    }
                    golem_cloud_client::Error::Reqwest(error) => {
                        ServiceErrorKind::ReqwestError(error)
                    }
                    golem_cloud_client::Error::ReqwestHeader(error) => {
                        ServiceErrorKind::ReqwestHeaderError(error)
                    }
                    golem_cloud_client::Error::Serde(error) => ServiceErrorKind::SerdeError(error),
                    golem_cloud_client::Error::Unexpected { code, data } => {
                        ServiceErrorKind::UnexpectedResponse {
                            status_code: code,
                            payload: data,
                        }
                    }
                },
            }
        }
    }

    pub trait AnyhowMapServiceError<R> {
        fn map_service_error(self) -> anyhow::Result<R>;

        fn map_service_error_not_found_as_opt(self) -> anyhow::Result<Option<R>>;
    }

    impl<R, E> AnyhowMapServiceError<R> for Result<R, golem_client::Error<E>>
    where
        ServiceError: From<golem_client::Error<E>>,
    {
        fn map_service_error(self) -> anyhow::Result<R> {
            self.map_err(|err| ServiceError::from(err).into())
        }

        fn map_service_error_not_found_as_opt(self) -> anyhow::Result<Option<R>> {
            match self {
                Ok(result) => Ok(Some(result)),
                Err(err) => {
                    let service_error = ServiceError::from(err);
                    match &service_error.kind {
                        ServiceErrorKind::ErrorResponse(response)
                            if response.status_code == 404 =>
                        {
                            Ok(None)
                        }
                        _ => Err(service_error.into()),
                    }
                }
            }
        }
    }

    impl<R, E> AnyhowMapServiceError<R> for Result<R, golem_cloud_client::Error<E>>
    where
        ServiceError: From<golem_cloud_client::Error<E>>,
    {
        fn map_service_error(self) -> anyhow::Result<R> {
            self.map_err(|err| ServiceError::from(err).into())
        }

        fn map_service_error_not_found_as_opt(self) -> anyhow::Result<Option<R>> {
            match self {
                Ok(result) => Ok(Some(result)),
                Err(err) => {
                    let service_error = ServiceError::from(err);
                    match &service_error.kind {
                        ServiceErrorKind::ErrorResponse(response)
                            if response.status_code == 404 =>
                        {
                            Ok(None)
                        }
                        _ => Err(service_error.into()),
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::ComponentError {
        fn service_name() -> &'static str {
            "Component"
        }
    }

    impl From<golem_client::api::ComponentError> for ServiceErrorResponse {
        fn from(value: golem_client::api::ComponentError) -> Self {
            match value {
                golem_client::api::ComponentError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_client::api::ComponentError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::ComponentError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::ComponentError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::ComponentError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_client::api::ComponentError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ComponentError {
        fn service_name() -> &'static str {
            "Cloud Component"
        }
    }

    impl From<golem_cloud_client::api::ComponentError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ComponentError) -> Self {
            match value {
                golem_cloud_client::api::ComponentError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_cloud_client::api::ComponentError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_cloud_client::api::ComponentError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_cloud_client::api::ComponentError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_cloud_client::api::ComponentError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_cloud_client::api::ComponentError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_client::api::WorkerError {
        fn service_name() -> &'static str {
            "Worker"
        }
    }

    impl From<golem_client::api::WorkerError> for ServiceErrorResponse {
        fn from(value: golem_client::api::WorkerError) -> Self {
            match value {
                golem_client::api::WorkerError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_client::api::WorkerError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::WorkerError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::WorkerError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::WorkerError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_client::api::WorkerError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: display_golem_error(error.golem_error),
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::WorkerError {
        fn service_name() -> &'static str {
            "Cloud Worker"
        }
    }

    impl From<golem_cloud_client::api::WorkerError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::WorkerError) -> Self {
            match value {
                golem_cloud_client::api::WorkerError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_cloud_client::api::WorkerError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_cloud_client::api::WorkerError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_cloud_client::api::WorkerError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_cloud_client::api::WorkerError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_cloud_client::api::WorkerError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: display_golem_error(error.golem_error),
                },
            }
        }
    }

    impl HasServiceName for golem_client::api::PluginError {
        fn service_name() -> &'static str {
            "Plugin"
        }
    }

    impl From<golem_client::api::PluginError> for ServiceErrorResponse {
        fn from(value: golem_client::api::PluginError) -> Self {
            match value {
                golem_client::api::PluginError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_client::api::PluginError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::PluginError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::PluginError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::PluginError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_client::api::PluginError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::PluginError {
        fn service_name() -> &'static str {
            "Cloud Plugin"
        }
    }

    impl From<golem_cloud_client::api::PluginError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::PluginError) -> Self {
            match value {
                golem_cloud_client::api::PluginError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_cloud_client::api::PluginError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_cloud_client::api::PluginError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_cloud_client::api::PluginError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_cloud_client::api::PluginError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_cloud_client::api::PluginError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ProjectError {
        fn service_name() -> &'static str {
            "Cloud Project"
        }
    }

    impl From<golem_cloud_client::api::ProjectError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ProjectError) -> Self {
            match value {
                golem_cloud_client::api::ProjectError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_cloud_client::api::ProjectError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_cloud_client::api::ProjectError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_cloud_client::api::ProjectError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_cloud_client::api::ProjectError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::LoginLoginOauth2Error {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_cloud_client::api::LoginLoginOauth2Error> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::LoginLoginOauth2Error) -> Self {
            match value {
                golem_cloud_client::api::LoginLoginOauth2Error::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_cloud_client::api::LoginLoginOauth2Error::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::LoginLoginOauth2Error::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::LoginCurrentLoginTokenError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_cloud_client::api::LoginCurrentLoginTokenError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::LoginCurrentLoginTokenError) -> Self {
            match value {
                golem_cloud_client::api::LoginCurrentLoginTokenError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_cloud_client::api::LoginCurrentLoginTokenError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::LoginCurrentLoginTokenError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::LoginStartLoginOauth2Error {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_cloud_client::api::LoginStartLoginOauth2Error> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::LoginStartLoginOauth2Error) -> Self {
            match value {
                golem_cloud_client::api::LoginStartLoginOauth2Error::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_cloud_client::api::LoginStartLoginOauth2Error::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::LoginStartLoginOauth2Error::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::LoginCompleteLoginOauth2Error {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_cloud_client::api::LoginCompleteLoginOauth2Error> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::LoginCompleteLoginOauth2Error) -> Self {
            match value {
                golem_cloud_client::api::LoginCompleteLoginOauth2Error::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_cloud_client::api::LoginCompleteLoginOauth2Error::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::LoginCompleteLoginOauth2Error::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::LoginOauth2WebFlowStartError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_cloud_client::api::LoginOauth2WebFlowStartError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::LoginOauth2WebFlowStartError) -> Self {
            match value {
                golem_cloud_client::api::LoginOauth2WebFlowStartError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_cloud_client::api::LoginOauth2WebFlowStartError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::LoginOauth2WebFlowStartError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::LoginOauth2WebFlowCallbackGithubError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_cloud_client::api::LoginOauth2WebFlowCallbackGithubError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::LoginOauth2WebFlowCallbackGithubError) -> Self {
            match value {
                golem_cloud_client::api::LoginOauth2WebFlowCallbackGithubError::Error302(_) => {
                    ServiceErrorResponse {
                        status_code: 302,
                        message: "WebFlowCallbackSuccessResponse".to_string(),
                    }
                }
                golem_cloud_client::api::LoginOauth2WebFlowCallbackGithubError::Error400(
                    errors,
                ) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_cloud_client::api::LoginOauth2WebFlowCallbackGithubError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::LoginOauth2WebFlowCallbackGithubError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::LoginOauth2WebFlowPollError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_cloud_client::api::LoginOauth2WebFlowPollError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::LoginOauth2WebFlowPollError) -> Self {
            match value {
                golem_cloud_client::api::LoginOauth2WebFlowPollError::Error202(_) => {
                    ServiceErrorResponse {
                        status_code: 202,
                        message: "PendingFlowCompletionResponse".to_string(),
                    }
                }
                golem_cloud_client::api::LoginOauth2WebFlowPollError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_cloud_client::api::LoginOauth2WebFlowPollError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::LoginOauth2WebFlowPollError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::ApiDefinitionError {
        fn service_name() -> &'static str {
            "API Definition"
        }
    }

    impl From<golem_client::api::ApiDefinitionError> for ServiceErrorResponse {
        fn from(value: golem_client::api::ApiDefinitionError) -> Self {
            match value {
                golem_client::api::ApiDefinitionError::Error400(error) => error.into(),
                golem_client::api::ApiDefinitionError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::ApiDefinitionError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::ApiDefinitionError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::ApiDefinitionError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error,
                },
                golem_client::api::ApiDefinitionError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ApiDefinitionError {
        fn service_name() -> &'static str {
            "Cloud API Definition"
        }
    }

    impl From<golem_cloud_client::api::ApiDefinitionError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ApiDefinitionError) -> Self {
            match value {
                golem_cloud_client::api::ApiDefinitionError::Error400(error) => error.into(),
                golem_cloud_client::api::ApiDefinitionError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ApiDefinitionError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ApiDefinitionError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.message,
                    }
                }
                golem_cloud_client::api::ApiDefinitionError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error,
                    }
                }
                golem_cloud_client::api::ApiDefinitionError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::ApiDeploymentError {
        fn service_name() -> &'static str {
            "API Deployment"
        }
    }

    impl From<golem_client::api::ApiDeploymentError> for ServiceErrorResponse {
        fn from(value: golem_client::api::ApiDeploymentError) -> Self {
            match value {
                golem_client::api::ApiDeploymentError::Error400(error) => error.into(),
                golem_client::api::ApiDeploymentError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::ApiDeploymentError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::ApiDeploymentError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::ApiDeploymentError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error,
                },
                golem_client::api::ApiDeploymentError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ApiDeploymentError {
        fn service_name() -> &'static str {
            "Cloud API Deployment"
        }
    }

    impl From<golem_cloud_client::api::ApiDeploymentError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ApiDeploymentError) -> Self {
            match value {
                golem_cloud_client::api::ApiDeploymentError::Error400(error) => error.into(),
                golem_cloud_client::api::ApiDeploymentError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ApiDeploymentError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ApiDeploymentError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.message,
                    }
                }
                golem_cloud_client::api::ApiDeploymentError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error,
                    }
                }
                golem_cloud_client::api::ApiDeploymentError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::ApiSecurityError {
        fn service_name() -> &'static str {
            "API Security Scheme"
        }
    }

    impl From<golem_client::api::ApiSecurityError> for ServiceErrorResponse {
        fn from(value: golem_client::api::ApiSecurityError) -> Self {
            match value {
                golem_client::api::ApiSecurityError::Error400(error) => error.into(),
                golem_client::api::ApiSecurityError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::ApiSecurityError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::ApiSecurityError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::ApiSecurityError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error,
                },
                golem_client::api::ApiSecurityError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ApiSecurityError {
        fn service_name() -> &'static str {
            "Cloud API Security Scheme"
        }
    }

    impl From<golem_cloud_client::api::ApiSecurityError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ApiSecurityError) -> Self {
            match value {
                golem_cloud_client::api::ApiSecurityError::Error400(error) => error.into(),
                golem_cloud_client::api::ApiSecurityError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ApiSecurityError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ApiSecurityError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.message,
                    }
                }
                golem_cloud_client::api::ApiSecurityError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error,
                    }
                }
                golem_cloud_client::api::ApiSecurityError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::TokenError {
        fn service_name() -> &'static str {
            "Cloud Token"
        }
    }

    impl From<golem_cloud_client::api::TokenError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::TokenError) -> Self {
            match value {
                golem_cloud_client::api::TokenError::Error400(error) => ServiceErrorResponse {
                    status_code: 400,
                    message: error.errors.iter().join("\n"),
                },
                golem_cloud_client::api::TokenError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_cloud_client::api::TokenError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_cloud_client::api::TokenError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::AccountError {
        fn service_name() -> &'static str {
            "Cloud Account"
        }
    }

    impl From<golem_cloud_client::api::AccountError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::AccountError) -> Self {
            match value {
                golem_cloud_client::api::AccountError::Error400(error) => ServiceErrorResponse {
                    status_code: 400,
                    message: error.errors.iter().join("\n"),
                },
                golem_cloud_client::api::AccountError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_cloud_client::api::AccountError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_cloud_client::api::AccountError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::GrantError {
        fn service_name() -> &'static str {
            "Cloud Grant"
        }
    }

    impl From<golem_cloud_client::api::GrantError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::GrantError) -> Self {
            match value {
                golem_cloud_client::api::GrantError::Error400(error) => ServiceErrorResponse {
                    status_code: 400,
                    message: error.errors.iter().join("\n"),
                },
                golem_cloud_client::api::GrantError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_cloud_client::api::GrantError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_cloud_client::api::GrantError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ProjectPolicyError {
        fn service_name() -> &'static str {
            "Cloud Project Policy"
        }
    }

    impl From<golem_cloud_client::api::ProjectPolicyError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ProjectPolicyError) -> Self {
            match value {
                golem_cloud_client::api::ProjectPolicyError::Error400(error) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: error.errors.iter().join("\n"),
                    }
                }
                golem_cloud_client::api::ProjectPolicyError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ProjectPolicyError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ProjectPolicyError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ProjectGrantError {
        fn service_name() -> &'static str {
            "Cloud Project Grant"
        }
    }

    impl From<golem_cloud_client::api::ProjectGrantError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ProjectGrantError) -> Self {
            match value {
                golem_cloud_client::api::ProjectGrantError::Error400(error) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: error.errors.iter().join("\n"),
                    }
                }
                golem_cloud_client::api::ProjectGrantError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ProjectGrantError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ProjectGrantError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ProjectGrantError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ApiCertificateError {
        fn service_name() -> &'static str {
            "Cloud API Certificate"
        }
    }

    impl From<golem_cloud_client::api::ApiCertificateError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ApiCertificateError) -> Self {
            match value {
                golem_cloud_client::api::ApiCertificateError::Error400(error) => error.into(),
                golem_cloud_client::api::ApiCertificateError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ApiCertificateError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_cloud_client::api::ApiCertificateError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.message,
                    }
                }
                golem_cloud_client::api::ApiCertificateError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error,
                    }
                }
                golem_cloud_client::api::ApiCertificateError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_cloud_client::api::ApiDomainError {
        fn service_name() -> &'static str {
            "Cloud API Domain"
        }
    }

    impl From<golem_cloud_client::api::ApiDomainError> for ServiceErrorResponse {
        fn from(value: golem_cloud_client::api::ApiDomainError) -> Self {
            match value {
                golem_cloud_client::api::ApiDomainError::Error400(error) => error.into(),
                golem_cloud_client::api::ApiDomainError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_cloud_client::api::ApiDomainError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_cloud_client::api::ApiDomainError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.message,
                },
                golem_cloud_client::api::ApiDomainError::Error409(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error,
                },
                golem_cloud_client::api::ApiDomainError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl From<golem_client::model::WorkerServiceErrorsBody> for ServiceErrorResponse {
        fn from(error: golem_client::model::WorkerServiceErrorsBody) -> Self {
            match error {
                golem_client::model::WorkerServiceErrorsBody::Messages(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_client::model::WorkerServiceErrorsBody::Validation(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: format!("Validation error(s): {}", errors.errors.join("\n"),),
                    }
                }
            }
        }
    }

    pub fn display_golem_error(error: GolemError) -> String {
        match error {
            GolemError::InvalidRequest(GolemErrorInvalidRequest { details }) => {
                format!("Invalid request: {details}")
            }
            GolemError::WorkerAlreadyExists(GolemErrorWorkerAlreadyExists { worker_id }) => {
                format!(
                    "Worker already exists: {}",
                    display_worker_id(worker_id).log_color_highlight()
                )
            }
            GolemError::WorkerNotFound(GolemErrorWorkerNotFound { worker_id }) => {
                format!(
                    "Worker not found: {}",
                    display_worker_id(worker_id).log_color_highlight()
                )
            }
            GolemError::WorkerCreationFailed(GolemErrorWorkerCreationFailed {
                worker_id,
                details,
            }) => {
                format!(
                    "Failed to create worker {}: {}",
                    display_worker_id(worker_id).log_color_highlight(),
                    details
                )
            }
            GolemError::FailedToResumeWorker(inner) => {
                let GolemErrorFailedToResumeWorker { worker_id, reason } = inner;
                format!(
                    "Failed to resume worker {}: {}",
                    display_worker_id(worker_id).log_color_highlight(),
                    display_golem_error(*reason).log_color_warn()
                )
            }
            GolemError::ComponentDownloadFailed(GolemErrorComponentDownloadFailed {
                component_id,
                reason,
            }) => {
                format!(
                    "Failed to download component {}@{}: {}",
                    component_id.component_id.to_string().log_color_highlight(),
                    component_id.version.to_string().log_color_highlight(),
                    reason.log_color_warn()
                )
            }
            GolemError::ComponentParseFailed(GolemErrorComponentParseFailed {
                component_id,
                reason,
            }) => {
                format!(
                    "Failed to parse component {}@{}: {}",
                    component_id.component_id.to_string().log_color_highlight(),
                    component_id.version.to_string().log_color_highlight(),
                    reason.log_color_warn()
                )
            }
            GolemError::GetLatestVersionOfComponentFailed(
                GolemErrorGetLatestVersionOfComponentFailed {
                    component_id,
                    reason,
                },
            ) => {
                format!(
                    "Failed to get latest version of component {}: {}",
                    component_id.to_string().log_color_highlight(),
                    reason.log_color_warn()
                )
            }
            GolemError::PromiseNotFound(GolemErrorPromiseNotFound { promise_id }) => {
                format!(
                    "Promise not found: {}",
                    display_promise_id(promise_id).log_color_highlight()
                )
            }
            GolemError::PromiseDropped(GolemErrorPromiseDropped { promise_id }) => {
                format!(
                    "Promise dropped: {}",
                    display_promise_id(promise_id).log_color_highlight()
                )
            }
            GolemError::PromiseAlreadyCompleted(GolemErrorPromiseAlreadyCompleted {
                promise_id,
            }) => {
                format!(
                    "Promise already completed: {}",
                    display_promise_id(promise_id).log_color_highlight()
                )
            }
            GolemError::Interrupted(GolemErrorInterrupted {
                recover_immediately,
            }) => {
                if recover_immediately {
                    "Simulated crash".to_string()
                } else {
                    "Worker interrupted".to_string()
                }
            }
            GolemError::ParamTypeMismatch(_) => "Parameter type mismatch".to_string(),
            GolemError::NoValueInMessage(_) => "No value in message".to_string(),
            GolemError::ValueMismatch(GolemErrorValueMismatch { details }) => {
                format!("Parameter value mismatch: {}", details.log_color_warn())
            }
            GolemError::UnexpectedOplogEntry(GolemErrorUnexpectedOplogEntry { expected, got }) => {
                format!(
                    "Unexpected oplog entry: expected {}, got {}",
                    expected.log_color_highlight(),
                    got.log_color_warn()
                )
            }
            GolemError::RuntimeError(GolemErrorRuntimeError { details }) => {
                format!("Runtime error:\n{}", format_stack(&details))
            }
            GolemError::InvalidShardId(GolemErrorInvalidShardId {
                shard_id,
                shard_ids,
            }) => {
                format!(
                    "Invalid shard id: {} not in [{}]",
                    shard_id,
                    shard_ids.iter().join(", ")
                )
            }
            GolemError::PreviousInvocationFailed(_) => {
                "The previously invoked function failed".to_string()
            }
            GolemError::PreviousInvocationExited(_) => {
                "The previously invoked function exited".to_string()
            }
            GolemError::Unknown(GolemErrorUnknown { details }) => {
                format!("Unknown error: {}", details)
            }
            GolemError::InvalidAccount(_) => "Invalid account".to_string(),
            GolemError::ShardingNotReady(_) => "Sharding not ready".to_string(),
            GolemError::InitialComponentFileDownloadFailed(
                GolemErrorInitialComponentFileDownloadFailed { path, reason, .. },
            ) => {
                format!(
                    "Failed to download initial file {}: {}",
                    path.log_color_highlight(),
                    reason.log_color_warn()
                )
            }
            GolemError::FileSystemError(GolemErrorFileSystemError { path, reason, .. }) => {
                format!(
                    "File system error: {}, {}",
                    path.log_color_highlight(),
                    reason.log_color_warn()
                )
            }
        }
    }

    pub fn display_worker_id(worker_id: WorkerId) -> String {
        format!("{}/{}", worker_id.component_id, worker_id.worker_name)
    }

    pub fn display_promise_id(promise_id: PromiseId) -> String {
        format!(
            "{}/{}",
            display_worker_id(promise_id.worker_id),
            promise_id.oplog_idx
        )
    }
}
