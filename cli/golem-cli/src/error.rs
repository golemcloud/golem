use crate::config::ProfileName;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use strum_macros::{Display, EnumIter};

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

#[derive(Clone, Copy, Debug, Display, EnumIter)]
pub enum ShowClapHelpTarget {
    AppNew,
    ComponentNew,
    ComponentAddDependency,
}

/// Errors that should be handled by the command handler with showing hints or error messages
#[derive(Debug, Display)]
pub enum HintError {
    NoApplicationManifestFound,
    ExpectedCloudProfile,
    ShowClapHelp(ShowClapHelpTarget),
}

impl Error for HintError {}

#[derive(Debug, Display)]
pub enum ContextInitHintError {
    ProfileNotFound {
        profile_name: ProfileName,
        manifest_profile_names: Vec<ProfileName>,
    },
}

impl Error for ContextInitHintError {}

pub mod service {
    use crate::log::LogColorize;

    use bytes::Bytes;

    use crate::model::text::fmt::format_stack;
    use colored::Colorize;
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
                    )?;

                    if let Some(source) = error.source() {
                        write!(f, ", caused by: {}", source.to_string().log_color_warn())?
                    }

                    Ok(())
                }
                ServiceErrorKind::ReqwestHeaderError(error) => {
                    write!(
                        f,
                        "{} - HTTP Header Error: {}",
                        service_name,
                        error.to_string().log_color_warn()
                    )?;

                    if let Some(source) = error.source() {
                        write!(f, ", caused by: {}", source.to_string().log_color_warn())?
                    }

                    Ok(())
                }
                ServiceErrorKind::SerdeError(error) => {
                    write!(
                        f,
                        "{} - Serialization Error: {}",
                        service_name,
                        error.to_string().log_color_warn()
                    )?;

                    if let Some(source) = error.source() {
                        write!(f, ", caused by: {}", source.to_string().log_color_warn())?
                    }

                    Ok(())
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
                golem_client::api::ComponentError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::ComponentError::Error500(error) => ServiceErrorResponse {
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
                golem_client::api::WorkerError::Error500(error) => {
                    let message = match error.worker_error {
                        Some(worker_error) => {
                            let error_logs = if !worker_error.stderr.is_empty() {
                                format!("\n\nStderr:\n{}", worker_error.stderr.yellow())
                            } else {
                                "".to_string()
                            };

                            format!(
                                "{}:\n{}{}",
                                error.error,
                                format_stack(&worker_error.cause),
                                error_logs
                            )
                        }
                        _ => error.error,
                    };

                    ServiceErrorResponse {
                        status_code: 500,
                        message,
                    }
                }
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
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::PluginError::Error422(error) => ServiceErrorResponse {
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

    impl HasServiceName for golem_client::api::LoginLoginOauth2Error {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_client::api::LoginLoginOauth2Error> for ServiceErrorResponse {
        fn from(value: golem_client::api::LoginLoginOauth2Error) -> Self {
            match value {
                golem_client::api::LoginLoginOauth2Error::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_client::api::LoginLoginOauth2Error::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::LoginLoginOauth2Error::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::LoginLoginOauth2Error::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::LoginLoginOauth2Error::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_client::api::LoginLoginOauth2Error::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::LoginLoginOauth2Error::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_client::api::LoginCurrentLoginTokenError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_client::api::LoginCurrentLoginTokenError> for ServiceErrorResponse {
        fn from(value: golem_client::api::LoginCurrentLoginTokenError) -> Self {
            match value {
                golem_client::api::LoginCurrentLoginTokenError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_client::api::LoginCurrentLoginTokenError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCurrentLoginTokenError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCurrentLoginTokenError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCurrentLoginTokenError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCurrentLoginTokenError::Error422(error) => {
                    ServiceErrorResponse {
                        status_code: 422,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCurrentLoginTokenError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::LoginStartOauth2DeviceFlowError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_client::api::LoginStartOauth2DeviceFlowError> for ServiceErrorResponse {
        fn from(value: golem_client::api::LoginStartOauth2DeviceFlowError) -> Self {
            match value {
                golem_client::api::LoginStartOauth2DeviceFlowError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_client::api::LoginStartOauth2DeviceFlowError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2DeviceFlowError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2DeviceFlowError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2DeviceFlowError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2DeviceFlowError::Error422(error) => {
                    ServiceErrorResponse {
                        status_code: 422,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2DeviceFlowError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::LoginCompleteOauth2DeviceFlowError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_client::api::LoginCompleteOauth2DeviceFlowError> for ServiceErrorResponse {
        fn from(value: golem_client::api::LoginCompleteOauth2DeviceFlowError) -> Self {
            match value {
                golem_client::api::LoginCompleteOauth2DeviceFlowError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_client::api::LoginCompleteOauth2DeviceFlowError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCompleteOauth2DeviceFlowError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCompleteOauth2DeviceFlowError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCompleteOauth2DeviceFlowError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCompleteOauth2DeviceFlowError::Error422(error) => {
                    ServiceErrorResponse {
                        status_code: 422,
                        message: error.error,
                    }
                }
                golem_client::api::LoginCompleteOauth2DeviceFlowError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::LoginStartOauth2WebflowError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_client::api::LoginStartOauth2WebflowError> for ServiceErrorResponse {
        fn from(value: golem_client::api::LoginStartOauth2WebflowError) -> Self {
            match value {
                golem_client::api::LoginStartOauth2WebflowError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_client::api::LoginStartOauth2WebflowError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2WebflowError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2WebflowError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2WebflowError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2WebflowError::Error422(error) => {
                    ServiceErrorResponse {
                        status_code: 422,
                        message: error.error,
                    }
                }
                golem_client::api::LoginStartOauth2WebflowError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::LoginSubmitOauth2WebflowCallbackError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_client::api::LoginSubmitOauth2WebflowCallbackError> for ServiceErrorResponse {
        fn from(value: golem_client::api::LoginSubmitOauth2WebflowCallbackError) -> Self {
            match value {
                golem_client::api::LoginSubmitOauth2WebflowCallbackError::Error302(_) => {
                    ServiceErrorResponse {
                        status_code: 302,
                        message: "WebFlowCallbackSuccessResponse".to_string(),
                    }
                }
                golem_client::api::LoginSubmitOauth2WebflowCallbackError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_client::api::LoginSubmitOauth2WebflowCallbackError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_client::api::LoginSubmitOauth2WebflowCallbackError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_client::api::LoginSubmitOauth2WebflowCallbackError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.error,
                    }
                }
                golem_client::api::LoginSubmitOauth2WebflowCallbackError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error.error,
                    }
                }
                golem_client::api::LoginSubmitOauth2WebflowCallbackError::Error422(error) => {
                    ServiceErrorResponse {
                        status_code: 422,
                        message: error.error,
                    }
                }
                golem_client::api::LoginSubmitOauth2WebflowCallbackError::Error500(error) => {
                    ServiceErrorResponse {
                        status_code: 500,
                        message: error.error,
                    }
                }
            }
        }
    }

    impl HasServiceName for golem_client::api::LoginPollOauth2WebflowError {
        fn service_name() -> &'static str {
            "Cloud Login"
        }
    }

    impl From<golem_client::api::LoginPollOauth2WebflowError> for ServiceErrorResponse {
        fn from(value: golem_client::api::LoginPollOauth2WebflowError) -> Self {
            match value {
                golem_client::api::LoginPollOauth2WebflowError::Error202(_) => {
                    ServiceErrorResponse {
                        status_code: 202,
                        message: "PendingFlowCompletionResponse".to_string(),
                    }
                }
                golem_client::api::LoginPollOauth2WebflowError::Error400(errors) => {
                    ServiceErrorResponse {
                        status_code: 400,
                        message: errors.errors.join("\n"),
                    }
                }
                golem_client::api::LoginPollOauth2WebflowError::Error401(error) => {
                    ServiceErrorResponse {
                        status_code: 401,
                        message: error.error,
                    }
                }
                golem_client::api::LoginPollOauth2WebflowError::Error403(error) => {
                    ServiceErrorResponse {
                        status_code: 403,
                        message: error.error,
                    }
                }
                golem_client::api::LoginPollOauth2WebflowError::Error404(error) => {
                    ServiceErrorResponse {
                        status_code: 404,
                        message: error.error,
                    }
                }
                golem_client::api::LoginPollOauth2WebflowError::Error409(error) => {
                    ServiceErrorResponse {
                        status_code: 409,
                        message: error.error,
                    }
                }
                golem_client::api::LoginPollOauth2WebflowError::Error422(error) => {
                    ServiceErrorResponse {
                        status_code: 422,
                        message: error.error,
                    }
                }
                golem_client::api::LoginPollOauth2WebflowError::Error500(error) => {
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
                golem_client::api::ApiDefinitionError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
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
                    message: error.error,
                },
                golem_client::api::ApiDefinitionError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::ApiDefinitionError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
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
                golem_client::api::ApiDeploymentError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
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
                    message: error.error,
                },
                golem_client::api::ApiDeploymentError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::ApiDeploymentError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
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
                golem_client::api::ApiSecurityError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
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
                    message: error.error,
                },
                golem_client::api::ApiSecurityError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::ApiSecurityError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_client::api::TokenError {
        fn service_name() -> &'static str {
            "Token"
        }
    }

    impl From<golem_client::api::TokenError> for ServiceErrorResponse {
        fn from(value: golem_client::api::TokenError) -> Self {
            match value {
                golem_client::api::TokenError::Error400(error) => ServiceErrorResponse {
                    status_code: 400,
                    message: error.errors.iter().join("\n"),
                },
                golem_client::api::TokenError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::TokenError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::TokenError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::TokenError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_client::api::TokenError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::TokenError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_client::api::AccountError {
        fn service_name() -> &'static str {
            "Account"
        }
    }

    impl From<golem_client::api::AccountError> for ServiceErrorResponse {
        fn from(value: golem_client::api::AccountError) -> Self {
            match value {
                golem_client::api::AccountError::Error400(error) => ServiceErrorResponse {
                    status_code: 400,
                    message: error.errors.iter().join("\n"),
                },
                golem_client::api::AccountError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::AccountError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::AccountError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::AccountError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                golem_client::api::AccountError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::AccountError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_client::api::ApiCertificateError {
        fn service_name() -> &'static str {
            "API Certificate"
        }
    }

    impl From<golem_client::api::ApiCertificateError> for ServiceErrorResponse {
        fn from(value: golem_client::api::ApiCertificateError) -> Self {
            match value {
                golem_client::api::ApiCertificateError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_client::api::ApiCertificateError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::ApiCertificateError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::ApiCertificateError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::ApiCertificateError::Error409(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::ApiCertificateError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::ApiCertificateError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for golem_client::api::ApiDomainError {
        fn service_name() -> &'static str {
            "API Domain"
        }
    }

    impl From<golem_client::api::ApiDomainError> for ServiceErrorResponse {
        fn from(value: golem_client::api::ApiDomainError) -> Self {
            match value {
                golem_client::api::ApiDomainError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                golem_client::api::ApiDomainError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                golem_client::api::ApiDomainError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                golem_client::api::ApiDomainError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::ApiDomainError::Error409(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                golem_client::api::ApiDomainError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                golem_client::api::ApiDomainError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
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
