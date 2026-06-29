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
    DiffModelVersionMismatch {
        expected_cli_diff_model_version: u32,
        server_diff_model_version: u32,
    },
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
    use crate::model::text::fmt::{format_error, format_stderr};
    use bytes::Bytes;
    use colored::Colorize;
    use golem_common::base_model::api;
    use golem_common::model::{AgentId, PromiseId};
    use reqwest::StatusCode;
    use serde::Deserialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Map, Value};
    use std::error::Error;
    use std::fmt::{Display, Formatter};

    #[derive(Debug)]
    pub struct ServiceErrorResponse {
        pub status_code: u16,
        pub errors: Vec<String>,
        pub code: Option<String>,
        pub additional_fields: Map<String, Value>,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AgentErrorDetails {
        pub cause: String,
        pub stderr: String,
    }

    impl ServiceErrorResponse {
        pub fn error(&self) -> String {
            self.errors.join("\n")
        }

        pub fn additional_field(&self, name: &str) -> Option<&Value> {
            self.additional_fields.get(name)
        }

        pub fn additional_field_as<T>(&self, name: &str) -> Option<T>
        where
            T: DeserializeOwned,
        {
            self.additional_field(name)
                .and_then(|value| serde_json::from_value(value.clone()).ok())
        }

        pub fn agent_error(&self) -> Option<AgentErrorDetails> {
            self.additional_field_as("workerError")
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

        fn error_subcodes(&self) -> Vec<&str> {
            self.errors
                .iter()
                .filter_map(|error| error.split_once(':').map(|(subcode, _)| subcode.trim()))
                .collect()
        }

        pub fn all_error_subcodes_in(&self, allowed_codes: &[&str]) -> bool {
            let subcodes = self.error_subcodes();
            !subcodes.is_empty()
                && subcodes
                    .iter()
                    .all(|code| allowed_codes.iter().any(|allowed| allowed == code))
        }
    }

    #[derive(Debug)]
    pub struct ServiceError {
        pub service_name: &'static str,
        pub kind: ServiceErrorKind,
    }

    impl ServiceError {
        pub fn render(&self) -> String {
            match &self.kind {
                ServiceErrorKind::ErrorResponse(response) => {
                    let service_name =
                        format!("{} Service", self.service_name).log_color_highlight();
                    let status = display_status_code(response.status_code).log_color_error();

                    let code = response
                        .code
                        .as_ref()
                        .map(|code| format!(" ({})", code.log_color_highlight()))
                        .unwrap_or_default();

                    let mut result = match response.errors.as_slice() {
                        [] => format!("{} - Error: {}{}", service_name, status, code),
                        [message] => format!(
                            "{} - Error: {}, {}{}",
                            service_name,
                            status,
                            format_error(message),
                            code
                        ),
                        messages => {
                            let mut result =
                                format!("{} - Error: {}{}", service_name, status, code);
                            result.push_str("\n\n");
                            result.push_str(&"Messages:".bright_black().to_string());
                            for message in messages {
                                result.push_str(&format!("\n  - {}", format_error(message)));
                            }
                            result
                        }
                    };

                    if let Some(agent_error) = response.agent_error() {
                        if !agent_error.stderr.trim().is_empty() {
                            result.push_str("\n\n");
                            result.push_str(&"Stderr:".bright_black().to_string());
                            result.push('\n');
                            result.push_str(&format_stderr(&agent_error.stderr));
                        }

                        if !agent_error.cause.trim().is_empty() {
                            result.push_str("\n\n");
                            result.push_str(&"Cause:".bright_black().to_string());
                            result.push('\n');
                            result.push_str(&format_error(&agent_error.cause));
                        }
                    }

                    result
                }
                ServiceErrorKind::ReqwestError(error) => render_error_with_source_string(
                    &format!("{} Service", self.service_name).log_color_highlight(),
                    "HTTP Client Error",
                    error,
                ),
                ServiceErrorKind::MiddlewareError(error) => format!(
                    "{} - HTTP Middleware Error: {}",
                    format!("{} Service", self.service_name).log_color_highlight(),
                    error.to_string().log_color_warn()
                ),
                ServiceErrorKind::ReqwestHeaderError(error) => render_error_with_source_string(
                    &format!("{} Service", self.service_name).log_color_highlight(),
                    "HTTP Header Error",
                    error,
                ),
                ServiceErrorKind::SerdeError(error) => render_error_with_source_string(
                    &format!("{} Service", self.service_name).log_color_highlight(),
                    "Serialization Error",
                    error,
                ),
                ServiceErrorKind::UnexpectedResponse {
                    status_code,
                    payload,
                } => format!(
                    "{} - Unexpected Response Error: {}, {}",
                    format!("{} Service", self.service_name).log_color_highlight(),
                    display_status_code(*status_code).log_color_error(),
                    String::from_utf8_lossy(payload)
                        .to_string()
                        .log_color_warn()
                ),
            }
        }

        pub fn agent_error(&self) -> Option<AgentErrorDetails> {
            match &self.kind {
                ServiceErrorKind::ErrorResponse(response) => response.agent_error(),
                _ => None,
            }
        }

        pub fn is_auth_unauthorized(&self) -> bool {
            match &self.kind {
                ServiceErrorKind::ErrorResponse(err) => {
                    err.is_status_code(401)
                        && err.code.as_deref() == Some(api::error_code::AUTH_UNAUTHORIZED)
                }
                _ => false,
            }
        }

        pub fn is_domain_is_not_registered(&self) -> bool {
            match &self.kind {
                ServiceErrorKind::ErrorResponse(err) => {
                    err.is_status_code(409) && err.has_code(api::error_code::DOMAIN_NOT_REGISTERED)
                }
                _ => false,
            }
        }

        pub fn is_agent_config_old_config_invalid(&self) -> bool {
            match &self.kind {
                ServiceErrorKind::ErrorResponse(err) => {
                    err.is_status_code(409)
                        && err.has_code(api::error_code::AGENT_CONFIG_OLD_CONFIG_INVALID)
                }
                _ => false,
            }
        }

        pub fn is_agent_secret_not_compatible(&self) -> bool {
            match &self.kind {
                ServiceErrorKind::ErrorResponse(err) => {
                    err.has_code(api::error_code::deployment_validation::FAILED)
                        && err.all_error_subcodes_in(&[
                            api::error_code::deployment_validation::AGENT_SECRET_NOT_COMPATIBLE,
                        ])
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
                        additional_fields: error.additional_fields().unwrap_or_default(),
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
                ServiceErrorKind::ErrorResponse(_) => {
                    write!(f, "{}", self.render())
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

    fn display_status_code(status_code: u16) -> String {
        StatusCode::from_u16(status_code)
            .map(|status_code| status_code.to_string())
            .unwrap_or_else(|_| status_code.to_string())
    }

    fn render_error_with_source_string<E: std::error::Error>(
        service_name: &str,
        category: &str,
        error: &E,
    ) -> String {
        let mut result = format!(
            "{} - {}: {}",
            service_name,
            category,
            error.to_string().log_color_warn()
        );

        if let Some(source) = error.source() {
            result.push_str(&format!(
                ", caused by: {}",
                source.to_string().log_color_warn()
            ));
        }

        result
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

    pub trait MapServiceError<R> {
        fn map_service_error(self) -> Result<R, ServiceError>;

        fn map_service_error_not_found_as_opt(self) -> Result<Option<R>, ServiceError>;
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

    impl<R, E> MapServiceError<R> for Result<R, golem_client::Error<E>>
    where
        E: golem_client::ErrorInfo,
    {
        fn map_service_error(self) -> Result<R, ServiceError> {
            self.map_err(ServiceError::from)
        }

        fn map_service_error_not_found_as_opt(self) -> Result<Option<R>, ServiceError> {
            self.map_not_found_to_none().map_err(ServiceError::from)
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

    #[cfg(test)]
    mod tests {
        use super::{ServiceError, ServiceErrorKind, ServiceErrorResponse};
        use serde_json::{Map, json};
        use test_r::test;

        fn agent_error_response() -> ServiceErrorResponse {
            let mut additional_fields = Map::new();
            additional_fields.insert(
                "workerError".to_string(),
                json!({
                    "cause": "error while executing at wasm backtrace:\n    0: agent_guest.wasm!abort",
                    "stderr": "JavaScript error: BOOM!\nStack:\n    at boom (user:91:19)"
                }),
            );

            ServiceErrorResponse {
                status_code: 500,
                errors: vec!["Invocation Failed".to_string()],
                code: Some("INTERNAL_AGENT_EXECUTION_FAILED".to_string()),
                additional_fields,
            }
        }

        #[test]
        fn agent_error_deserializes_from_additional_fields() {
            let response = agent_error_response();

            let agent_error = response.agent_error().expect("agent error");

            assert!(agent_error.cause.contains("agent_guest.wasm!abort"));
            assert!(agent_error.stderr.contains("JavaScript error: BOOM!"));
        }

        #[test]
        fn service_error_render_includes_code_and_agent_details() {
            let error = ServiceError {
                service_name: "Agent",
                kind: ServiceErrorKind::ErrorResponse(agent_error_response()),
            };

            let rendered = strip_ansi_escapes::strip_str(error.render());

            assert!(rendered.contains(
                "Agent Service - Error: 500 Internal Server Error, Invocation Failed (INTERNAL_AGENT_EXECUTION_FAILED)"
            ));
            assert!(rendered.contains("Invocation Failed"));
            assert!(rendered.contains("INTERNAL_AGENT_EXECUTION_FAILED"));
            assert!(rendered.contains("Stderr:"));
            assert!(rendered.contains("JavaScript error: BOOM!"));
            assert!(rendered.contains("Cause:"));
            assert!(rendered.contains("agent_guest.wasm!abort"));
            assert!(!rendered.contains("Agent Service response:"));
            assert!(!rendered.contains("Worker stderr:"));
            assert!(!rendered.contains("Worker cause:"));
        }
    }
}
