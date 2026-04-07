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

    use golem_client::api::{
        AccountError, AgentError, AgentSecretsError, ApiDeploymentError, ApiDomainError,
        ApiSecurityError, ApplicationError, ComponentError, EnvironmentError,
        LoginCompleteOauth2DeviceFlowError, LoginCurrentLoginTokenError, LoginLoginOauth2Error,
        LoginPollOauth2WebflowError, LoginStartOauth2DeviceFlowError, LoginStartOauth2WebflowError,
        LoginSubmitOauth2WebflowCallbackError, McpDeploymentError, PluginError, TokenError,
        WorkerError,
    };
    use golem_common::model::{AgentId, PromiseId};
    use reqwest::StatusCode;
    use std::error::Error;
    use std::fmt::{Display, Formatter};

    #[derive(Debug)]
    pub struct ServiceErrorResponse {
        pub status_code: u16,
        pub messages: Vec<String>,
        pub code: Option<String>,
    }

    impl ServiceErrorResponse {
        pub fn message(&self) -> String {
            self.messages.join("\n")
        }

        fn first_message(&self) -> Option<&str> {
            self.messages.first().map(String::as_str)
        }
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
                    if err.code.as_deref() == Some("DOMAIN_NOT_REGISTERED") {
                        true
                    } else {
                        (err.status_code == 409 || err.status_code == 404)
                            && err.first_message().is_some_and(|message| {
                                message.starts_with("Domain")
                                    && message.ends_with("is not registered")
                            })
                    }
                }
                _ => false,
            }
        }
    }

    #[derive(Debug)]
    pub enum ServiceErrorKind {
        ErrorResponse(ServiceErrorResponse),
        ReqwestError(reqwest::Error),
        MiddlewareError(reqwest_middleware::Error),
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
                        response.message().log_color_warn()
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
                ServiceErrorKind::MiddlewareError(error) => {
                    write!(
                        f,
                        "{} - HTTP Middleware Error: {}",
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
        T: golem_client::ErrorInfo + HasServiceName,
    {
        fn from(error: golem_client::Error<T>) -> Self {
            ServiceError {
                service_name: T::service_name(),
                kind: match error {
                    golem_client::Error::Item(error) => {
                        ServiceErrorKind::ErrorResponse(ServiceErrorResponse {
                            status_code: error.status_code(),
                            messages: error.messages(),
                            code: error.code().map(str::to_string),
                        })
                    }
                    golem_client::Error::Reqwest(error) => ServiceErrorKind::ReqwestError(error),
                    golem_client::Error::Middleware(error) => {
                        ServiceErrorKind::MiddlewareError(error)
                    }
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

    impl HasServiceName for EnvironmentError {
        fn service_name() -> &'static str {
            "Environment"
        }
    }

    impl HasServiceName for ComponentError {
        fn service_name() -> &'static str {
            "Component"
        }
    }

    impl HasServiceName for AgentError {
        fn service_name() -> &'static str {
            "Agent"
        }
    }

    impl HasServiceName for WorkerError {
        fn service_name() -> &'static str {
            "Worker"
        }
    }

    impl HasServiceName for PluginError {
        fn service_name() -> &'static str {
            "Plugin"
        }
    }

    impl HasServiceName for LoginLoginOauth2Error {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl HasServiceName for LoginCurrentLoginTokenError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl HasServiceName for LoginStartOauth2DeviceFlowError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl HasServiceName for LoginCompleteOauth2DeviceFlowError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl HasServiceName for LoginStartOauth2WebflowError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl HasServiceName for LoginSubmitOauth2WebflowCallbackError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl HasServiceName for LoginPollOauth2WebflowError {
        fn service_name() -> &'static str {
            "Login"
        }
    }

    impl HasServiceName for ApiDeploymentError {
        fn service_name() -> &'static str {
            "API Deployment"
        }
    }

    impl HasServiceName for ApiSecurityError {
        fn service_name() -> &'static str {
            "API Security Scheme"
        }
    }

    impl HasServiceName for TokenError {
        fn service_name() -> &'static str {
            "Token"
        }
    }

    impl HasServiceName for AccountError {
        fn service_name() -> &'static str {
            "Account"
        }
    }

    impl HasServiceName for ApiDomainError {
        fn service_name() -> &'static str {
            "API Domain"
        }
    }

    impl HasServiceName for McpDeploymentError {
        fn service_name() -> &'static str {
            "MCP Deployment"
        }
    }

    impl HasServiceName for AgentSecretsError {
        fn service_name() -> &'static str {
            "AgentSecrets"
        }
    }

    pub fn display_agent_id(agent_id: AgentId) -> String {
        format!("{}/{}", agent_id.component_id, agent_id.agent_id)
    }

    pub fn display_promise_id(promise_id: PromiseId) -> String {
        format!(
            "{}/{}",
            display_agent_id(promise_id.agent_id),
            promise_id.oplog_idx
        )
    }
}
