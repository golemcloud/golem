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

    use golem_common::base_model::api;
    use golem_common::model::{AgentId, PromiseId};
    use reqwest::StatusCode;
    use std::error::Error;
    use std::fmt::{Display, Formatter};

    #[derive(Debug)]
    pub struct ServiceErrorResponse {
        pub status_code: u16,
        pub errors: Vec<String>,
        pub code: Option<String>,
    }

    impl ServiceErrorResponse {
        pub fn error(&self) -> String {
            self.errors.join("\n")
        }

        pub fn is_status_code(&self, status_code: u16) -> bool {
            self.status_code == status_code
        }

        pub fn is_not_found(&self) -> bool {
            self.is_status_code(404)
        }

        pub fn has_code(&self, code: &str) -> bool {
            self.code.as_deref() == Some(code)
        }
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
                    err.is_status_code(409) && err.has_code(api::error_code::DOMAIN_NOT_REGISTERED)
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

    impl ServiceErrorKind {
        fn from_golem_client_error<T: golem_client::ErrorInfo>(
            error: golem_client::Error<T>,
        ) -> Self {
            match error {
                golem_client::Error::Item(error) => {
                    ServiceErrorKind::ErrorResponse(ServiceErrorResponse {
                        status_code: error.status_code(),
                        errors: error.errors().to_vec(),
                        code: error.code().map(str::to_string),
                    })
                }
                golem_client::Error::Reqwest(error) => ServiceErrorKind::ReqwestError(error),
                golem_client::Error::Middleware(error) => ServiceErrorKind::MiddlewareError(error),
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
            }
        }
    }

    impl Display for ServiceError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            fn display_status_code(status_code: u16) -> String {
                StatusCode::from_u16(status_code)
                    .map(|status_code| status_code.to_string())
                    .unwrap_or_else(|_| status_code.to_string())
            }

            fn display_error_with_source<E: std::error::Error>(
                f: &mut Formatter<'_>,
                service_name: &str,
                category: &str,
                error: &E,
            ) -> std::fmt::Result {
                write!(
                    f,
                    "{} - {}: {}",
                    service_name,
                    category,
                    error.to_string().log_color_warn()
                )?;

                if let Some(source) = error.source() {
                    write!(f, ", caused by: {}", source.to_string().log_color_warn())?;
                }

                Ok(())
            }

            let service_name = format!("{} Service", self.service_name).log_color_highlight();

            match &self.kind {
                ServiceErrorKind::ErrorResponse(response) => {
                    write!(
                        f,
                        "{} - Error: {}, {}",
                        service_name,
                        display_status_code(response.status_code).log_color_error(),
                        response.error().log_color_warn()
                    )
                }
                ServiceErrorKind::ReqwestError(error) => {
                    display_error_with_source(f, &service_name, "HTTP Client Error", error)
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
                    display_error_with_source(f, &service_name, "HTTP Header Error", error)
                }
                ServiceErrorKind::SerdeError(error) => {
                    display_error_with_source(f, &service_name, "Serialization Error", error)
                }
                ServiceErrorKind::UnexpectedResponse {
                    status_code,
                    payload,
                } => {
                    write!(
                        f,
                        "{} - Unexpected Response Error: {}, {}",
                        service_name,
                        display_status_code(*status_code).log_color_error(),
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
        T: golem_client::ErrorInfo,
    {
        fn from(error: golem_client::Error<T>) -> Self {
            let service_name = error.service_name();

            ServiceError {
                service_name,
                kind: ServiceErrorKind::from_golem_client_error(error),
            }
        }
    }

    pub trait AnyhowMapServiceError<R> {
        fn map_service_error(self) -> anyhow::Result<R>;

        fn map_service_error_not_found_as_opt(self) -> anyhow::Result<Option<R>>;
    }

    trait ClientErrorResultExt<R, E> {
        fn map_not_found_to_none(self) -> Result<Option<R>, golem_client::Error<E>>
        where
            E: golem_client::ErrorInfo;
    }

    impl<R, E> ClientErrorResultExt<R, E> for Result<R, golem_client::Error<E>> {
        fn map_not_found_to_none(self) -> Result<Option<R>, golem_client::Error<E>>
        where
            E: golem_client::ErrorInfo,
        {
            match self {
                Ok(result) => Ok(Some(result)),
                Err(error) if error.is_not_found() => Ok(None),
                Err(error) => Err(error),
            }
        }
    }

    impl<R, E> AnyhowMapServiceError<R> for Result<R, golem_client::Error<E>>
    where
        E: golem_client::ErrorInfo,
    {
        fn map_service_error(self) -> anyhow::Result<R> {
            self.map_err(|err| ServiceError::from(err).into())
        }

        fn map_service_error_not_found_as_opt(self) -> anyhow::Result<Option<R>> {
            self.map_not_found_to_none()
                .map_err(|err| ServiceError::from(err).into())
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
