use crate::config::ProfileName;
use golem_common::model::environment::EnvironmentName;
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

#[derive(Debug)]
pub struct PipedExitCode(pub u8);

impl Display for PipedExitCode {
    fn fmt(&self, _f: &mut Formatter<'_>) -> std::fmt::Result {
        //NOP
        Ok(())
    }
}

impl Error for PipedExitCode {}

#[derive(Clone, Copy, Debug, Display, EnumIter)]
pub enum ShowClapHelpTarget {
    AppNew,
    ComponentNew,
}

/// Errors that should be handled by the command handler with showing hints or error messages
#[derive(Debug, Display)]
pub enum HintError {
    NoApplicationManifestFound,
    ExpectedCloudProfile,
    EnvironmentHasNoDeployment,
    ShowClapHelp(ShowClapHelpTarget),
}

impl Error for HintError {}

#[derive(Debug, Display)]
pub enum ContextInitHintError {
    CannotUseShortEnvRefWithLocalOrCloudFlags,
    CannotSelectEnvironmentWithoutManifest {
        requested_environment_name: EnvironmentName,
    },
    EnvironmentNotFound {
        requested_environment_name: EnvironmentName,
        manifest_environment_names: Vec<EnvironmentName>,
    },
    ProfileNotFound {
        profile_name: ProfileName,
        available_profile_names: Vec<ProfileName>,
    },
}

impl Error for ContextInitHintError {}

pub mod service {
    use crate::log::LogColorize;

    use bytes::Bytes;

    use crate::model::text::fmt::{format_stack, format_stderr};
    use golem_client::api::{
        AccountError, ApiDeploymentError, ApiDomainError, ApiSecurityError, ApplicationError,
        ComponentError, EnvironmentError, LoginCompleteOauth2DeviceFlowError,
        LoginCurrentLoginTokenError, LoginLoginOauth2Error, LoginPollOauth2WebflowError,
        LoginStartOauth2DeviceFlowError, LoginStartOauth2WebflowError,
        LoginSubmitOauth2WebflowCallbackError, PluginError, TokenError, WorkerError,
    };
    use golem_common::model::{PromiseId, WorkerId};
    use itertools::Itertools;
    use reqwest::StatusCode;
    use std::error::Error;
    use std::fmt::{Display, Formatter};

    #[derive(Debug)]
    pub struct ServiceErrorResponse {
        pub status_code: u16,
        pub message: String,
    }

    pub trait HasServiceName {
        fn service_name() -> &'static str;
    }

    #[derive(Debug)]
    pub struct ServiceError {
        pub service_name: &'static str,
        pub kind: ServiceErrorKind,
    }

    impl ServiceError {
        pub fn is_domain_is_not_registered(&self) -> bool {
            match &self.kind {
                ServiceErrorKind::ErrorResponse(err) => {
                    err.status_code == 409
                        && err.message.starts_with("Domain")
                        && err.message.ends_with("is not registered")
                }
                _ => false,
            }
        }
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

    impl HasServiceName for ApplicationError {
        fn service_name() -> &'static str {
            "Application"
        }
    }

    impl From<ApplicationError> for ServiceErrorResponse {
        fn from(value: ApplicationError) -> Self {
            match value {
                ApplicationError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                ApplicationError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                ApplicationError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                ApplicationError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                ApplicationError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                ApplicationError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                ApplicationError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for EnvironmentError {
        fn service_name() -> &'static str {
            "Environment"
        }
    }

    impl From<EnvironmentError> for ServiceErrorResponse {
        fn from(value: EnvironmentError) -> Self {
            match value {
                EnvironmentError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                EnvironmentError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                EnvironmentError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                EnvironmentError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                EnvironmentError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                EnvironmentError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                EnvironmentError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for ComponentError {
        fn service_name() -> &'static str {
            "Component"
        }
    }

    impl From<ComponentError> for ServiceErrorResponse {
        fn from(value: ComponentError) -> Self {
            match value {
                ComponentError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                ComponentError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                ComponentError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                ComponentError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                ComponentError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                ComponentError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                ComponentError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for WorkerError {
        fn service_name() -> &'static str {
            "Worker"
        }
    }

    impl From<WorkerError> for ServiceErrorResponse {
        fn from(value: WorkerError) -> Self {
            match value {
                WorkerError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                WorkerError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                WorkerError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                WorkerError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                WorkerError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                WorkerError::Error500(error) => {
                    let message = match error.worker_error {
                        Some(worker_error) => {
                            let error_logs = if !worker_error.stderr.is_empty() {
                                format!("\n\nStderr:\n{}", format_stderr(&worker_error.stderr))
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

    impl HasServiceName for PluginError {
        fn service_name() -> &'static str {
            "Plugin"
        }
    }

    impl From<PluginError> for ServiceErrorResponse {
        fn from(value: PluginError) -> Self {
            match value {
                PluginError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                PluginError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                PluginError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                PluginError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                PluginError::Error409(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                PluginError::Error422(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                PluginError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for LoginLoginOauth2Error {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl From<LoginLoginOauth2Error> for ServiceErrorResponse {
        fn from(value: LoginLoginOauth2Error) -> Self {
            match value {
                LoginLoginOauth2Error::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                LoginLoginOauth2Error::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                LoginLoginOauth2Error::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                LoginLoginOauth2Error::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                LoginLoginOauth2Error::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                LoginLoginOauth2Error::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                LoginLoginOauth2Error::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for LoginCurrentLoginTokenError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl From<LoginCurrentLoginTokenError> for ServiceErrorResponse {
        fn from(value: LoginCurrentLoginTokenError) -> Self {
            match value {
                LoginCurrentLoginTokenError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                LoginCurrentLoginTokenError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                LoginCurrentLoginTokenError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                LoginCurrentLoginTokenError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                LoginCurrentLoginTokenError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                LoginCurrentLoginTokenError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                LoginCurrentLoginTokenError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for LoginStartOauth2DeviceFlowError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl From<LoginStartOauth2DeviceFlowError> for ServiceErrorResponse {
        fn from(value: LoginStartOauth2DeviceFlowError) -> Self {
            match value {
                LoginStartOauth2DeviceFlowError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                LoginStartOauth2DeviceFlowError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                LoginStartOauth2DeviceFlowError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                LoginStartOauth2DeviceFlowError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                LoginStartOauth2DeviceFlowError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                LoginStartOauth2DeviceFlowError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                LoginStartOauth2DeviceFlowError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for LoginCompleteOauth2DeviceFlowError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl From<LoginCompleteOauth2DeviceFlowError> for ServiceErrorResponse {
        fn from(value: LoginCompleteOauth2DeviceFlowError) -> Self {
            match value {
                LoginCompleteOauth2DeviceFlowError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                LoginCompleteOauth2DeviceFlowError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                LoginCompleteOauth2DeviceFlowError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                LoginCompleteOauth2DeviceFlowError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                LoginCompleteOauth2DeviceFlowError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                LoginCompleteOauth2DeviceFlowError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                LoginCompleteOauth2DeviceFlowError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for LoginStartOauth2WebflowError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl From<LoginStartOauth2WebflowError> for ServiceErrorResponse {
        fn from(value: LoginStartOauth2WebflowError) -> Self {
            match value {
                LoginStartOauth2WebflowError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                LoginStartOauth2WebflowError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                LoginStartOauth2WebflowError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                LoginStartOauth2WebflowError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                LoginStartOauth2WebflowError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                LoginStartOauth2WebflowError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                LoginStartOauth2WebflowError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for LoginSubmitOauth2WebflowCallbackError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl From<LoginSubmitOauth2WebflowCallbackError> for ServiceErrorResponse {
        fn from(value: LoginSubmitOauth2WebflowCallbackError) -> Self {
            match value {
                LoginSubmitOauth2WebflowCallbackError::Error302(_) => ServiceErrorResponse {
                    status_code: 302,
                    message: "WebFlowCallbackSuccessResponse".to_string(),
                },
                LoginSubmitOauth2WebflowCallbackError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                LoginSubmitOauth2WebflowCallbackError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                LoginSubmitOauth2WebflowCallbackError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                LoginSubmitOauth2WebflowCallbackError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                LoginSubmitOauth2WebflowCallbackError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                LoginSubmitOauth2WebflowCallbackError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                LoginSubmitOauth2WebflowCallbackError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for LoginPollOauth2WebflowError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl From<LoginPollOauth2WebflowError> for ServiceErrorResponse {
        fn from(value: LoginPollOauth2WebflowError) -> Self {
            match value {
                LoginPollOauth2WebflowError::Error202(_) => ServiceErrorResponse {
                    status_code: 202,
                    message: "PendingFlowCompletionResponse".to_string(),
                },
                LoginPollOauth2WebflowError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                LoginPollOauth2WebflowError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                LoginPollOauth2WebflowError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                LoginPollOauth2WebflowError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                LoginPollOauth2WebflowError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                LoginPollOauth2WebflowError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                LoginPollOauth2WebflowError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for ApiDeploymentError {
        fn service_name() -> &'static str {
            "API Deployment"
        }
    }

    impl From<ApiDeploymentError> for ServiceErrorResponse {
        fn from(value: ApiDeploymentError) -> Self {
            match value {
                ApiDeploymentError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                ApiDeploymentError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                ApiDeploymentError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                ApiDeploymentError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                ApiDeploymentError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                ApiDeploymentError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                ApiDeploymentError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for ApiSecurityError {
        fn service_name() -> &'static str {
            "API Security Scheme"
        }
    }

    impl From<ApiSecurityError> for ServiceErrorResponse {
        fn from(value: ApiSecurityError) -> Self {
            match value {
                ApiSecurityError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                ApiSecurityError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                ApiSecurityError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                ApiSecurityError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                ApiSecurityError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                ApiSecurityError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                ApiSecurityError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for TokenError {
        fn service_name() -> &'static str {
            "Token"
        }
    }

    impl From<TokenError> for ServiceErrorResponse {
        fn from(value: TokenError) -> Self {
            match value {
                TokenError::Error400(error) => ServiceErrorResponse {
                    status_code: 400,
                    message: error.errors.iter().join("\n"),
                },
                TokenError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                TokenError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                TokenError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                TokenError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                TokenError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                TokenError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for AccountError {
        fn service_name() -> &'static str {
            "Account"
        }
    }

    impl From<AccountError> for ServiceErrorResponse {
        fn from(value: AccountError) -> Self {
            match value {
                AccountError::Error400(error) => ServiceErrorResponse {
                    status_code: 400,
                    message: error.errors.iter().join("\n"),
                },
                AccountError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                AccountError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                AccountError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                AccountError::Error409(error) => ServiceErrorResponse {
                    status_code: 409,
                    message: error.error,
                },
                AccountError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                AccountError::Error500(error) => ServiceErrorResponse {
                    status_code: 500,
                    message: error.error,
                },
            }
        }
    }

    impl HasServiceName for ApiDomainError {
        fn service_name() -> &'static str {
            "API Domain"
        }
    }

    impl From<ApiDomainError> for ServiceErrorResponse {
        fn from(value: ApiDomainError) -> Self {
            match value {
                ApiDomainError::Error400(errors) => ServiceErrorResponse {
                    status_code: 400,
                    message: errors.errors.join("\n"),
                },
                ApiDomainError::Error401(error) => ServiceErrorResponse {
                    status_code: 401,
                    message: error.error,
                },
                ApiDomainError::Error403(error) => ServiceErrorResponse {
                    status_code: 403,
                    message: error.error,
                },
                ApiDomainError::Error404(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                ApiDomainError::Error409(error) => ServiceErrorResponse {
                    status_code: 404,
                    message: error.error,
                },
                ApiDomainError::Error422(error) => ServiceErrorResponse {
                    status_code: 422,
                    message: error.error,
                },
                ApiDomainError::Error500(error) => ServiceErrorResponse {
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
