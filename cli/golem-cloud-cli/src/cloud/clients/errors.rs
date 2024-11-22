use golem_cli::oss::clients::errors::{display_golem_error, display_worker_service_errors_body};
use golem_cloud_client::api::{
    AccountError, ApiCertificateError, ApiDefinitionError, ApiDeploymentError, ApiDomainError,
    ComponentError, GrantError, HealthCheckError, LoginCurrentLoginTokenError,
    LoginLoginOauth2Error, LoginOauth2WebFlowCallbackGithubError, LoginOauth2WebFlowPollError,
    LoginOauth2WebFlowStartError, LoginStartLoginOauth2Error, PluginError, ProjectError,
    ProjectGrantError, ProjectPolicyError, TokenError,
};
use golem_cloud_client::api::{LoginCompleteLoginOauth2Error, WorkerError};
use itertools::Itertools;

#[derive(Clone, PartialEq, Eq)]
pub struct CloudGolemError(pub String);

impl From<reqwest::Error> for CloudGolemError {
    fn from(error: reqwest::Error) -> Self {
        CloudGolemError(format!("Unexpected client error: {error:?}"))
    }
}

impl From<reqwest::header::InvalidHeaderValue> for CloudGolemError {
    fn from(value: reqwest::header::InvalidHeaderValue) -> Self {
        CloudGolemError(format!("Invalid request header: {value}"))
    }
}

impl From<CloudGolemError> for golem_cli::model::GolemError {
    fn from(value: CloudGolemError) -> Self {
        golem_cli::model::GolemError(value.0)
    }
}

pub trait ResponseContentErrorMapper {
    fn map(self) -> String;
}

impl<T: ResponseContentErrorMapper> From<golem_cloud_client::Error<T>> for CloudGolemError {
    fn from(value: golem_cloud_client::Error<T>) -> Self {
        match value {
            golem_cloud_client::Error::Reqwest(error) => CloudGolemError::from(error),
            golem_cloud_client::Error::ReqwestHeader(invalid_header) => {
                CloudGolemError::from(invalid_header)
            }
            golem_cloud_client::Error::Serde(error) => {
                CloudGolemError(format!("Unexpected serialization error: {error}"))
            }
            golem_cloud_client::Error::Item(data) => {
                let error_str = ResponseContentErrorMapper::map(data);
                CloudGolemError(error_str)
            }
            golem_cloud_client::Error::Unexpected { code, data } => {
                match String::from_utf8(Vec::from(data)) {
                    Ok(data_string) => CloudGolemError(format!(
                        "Unexpected http error. Code: {code}, content: {data_string}."
                    )),
                    Err(_) => CloudGolemError(format!(
                        "Unexpected http error. Code: {code}, can't parse content as string."
                    )),
                }
            }
        }
    }
}

impl ResponseContentErrorMapper for AccountError {
    fn map(self) -> String {
        match self {
            AccountError::Error400(errors) => errors.errors.iter().join(", "),
            AccountError::Error401(error) => error.error,
            AccountError::Error404(error) => error.error,
            AccountError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for GrantError {
    fn map(self) -> String {
        match self {
            GrantError::Error400(errors) => errors.errors.iter().join(", "),
            GrantError::Error401(error) => error.error,
            GrantError::Error404(error) => error.error,
            GrantError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for LoginLoginOauth2Error {
    fn map(self) -> String {
        match self {
            LoginLoginOauth2Error::Error400(errors) => errors.errors.iter().join(", "),
            LoginLoginOauth2Error::Error401(error) => error.error,
            LoginLoginOauth2Error::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for LoginCurrentLoginTokenError {
    fn map(self) -> String {
        match self {
            LoginCurrentLoginTokenError::Error400(errors) => errors.errors.iter().join(", "),
            LoginCurrentLoginTokenError::Error401(error) => error.error,
            LoginCurrentLoginTokenError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for LoginStartLoginOauth2Error {
    fn map(self) -> String {
        match self {
            LoginStartLoginOauth2Error::Error400(errors) => errors.errors.iter().join(", "),
            LoginStartLoginOauth2Error::Error401(error) => error.error,
            LoginStartLoginOauth2Error::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for LoginCompleteLoginOauth2Error {
    fn map(self) -> String {
        match self {
            LoginCompleteLoginOauth2Error::Error400(errors) => errors.errors.iter().join(", "),
            LoginCompleteLoginOauth2Error::Error401(error) => error.error,
            LoginCompleteLoginOauth2Error::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for LoginOauth2WebFlowStartError {
    fn map(self) -> String {
        match self {
            LoginOauth2WebFlowStartError::Error400(errors) => errors.errors.iter().join(", "),
            LoginOauth2WebFlowStartError::Error401(error) => error.error,
            LoginOauth2WebFlowStartError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for LoginOauth2WebFlowCallbackGithubError {
    fn map(self) -> String {
        match self {
            LoginOauth2WebFlowCallbackGithubError::Error302(_) => "Redirect Request".into(),
            LoginOauth2WebFlowCallbackGithubError::Error400(errors) => {
                errors.errors.iter().join(", ")
            }
            LoginOauth2WebFlowCallbackGithubError::Error401(error) => error.error,
            LoginOauth2WebFlowCallbackGithubError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for LoginOauth2WebFlowPollError {
    fn map(self) -> String {
        match self {
            LoginOauth2WebFlowPollError::Error202(_) => "Pending Web Flow".into(),
            LoginOauth2WebFlowPollError::Error400(errors) => errors.errors.iter().join(", "),
            LoginOauth2WebFlowPollError::Error401(error) => error.error,
            LoginOauth2WebFlowPollError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ProjectError {
    fn map(self) -> String {
        match self {
            ProjectError::Error400(errors) => errors.errors.iter().join(", "),
            ProjectError::Error401(error) => error.error,
            ProjectError::Error403(error) => error.error,
            ProjectError::Error404(error) => error.error,
            ProjectError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ProjectGrantError {
    fn map(self) -> String {
        match self {
            ProjectGrantError::Error400(errors) => errors.errors.iter().join(", "),
            ProjectGrantError::Error401(error) => error.error,
            ProjectGrantError::Error403(error) => error.error,
            ProjectGrantError::Error404(error) => error.error,
            ProjectGrantError::Error500(error) => error.error,
        }
    }
}

#[allow(unreachable_patterns)]
impl ResponseContentErrorMapper for ProjectPolicyError {
    fn map(self) -> String {
        match self {
            ProjectPolicyError::Error400(errors) => errors.errors.iter().join(", "),
            ProjectPolicyError::Error401(error) => error.error,
            ProjectPolicyError::Error404(error) => error.error,
            ProjectPolicyError::Error500(error) => error.error,
            _ => "UnknownError".into(),
        }
    }
}

impl ResponseContentErrorMapper for ComponentError {
    fn map(self) -> String {
        match self {
            ComponentError::Error400(errors) => errors.errors.iter().join(", "),
            ComponentError::Error401(error) => error.error,
            ComponentError::Error403(error) => error.error,
            ComponentError::Error404(error) => error.error,
            ComponentError::Error409(error) => error.error,
            ComponentError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for TokenError {
    fn map(self) -> String {
        match self {
            TokenError::Error400(errors) => errors.errors.iter().join(", "),
            TokenError::Error401(error) => error.error,
            TokenError::Error404(error) => error.error,
            TokenError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for WorkerError {
    fn map(self) -> String {
        match self {
            WorkerError::Error400(errors) => errors.errors.iter().join(", "),
            WorkerError::Error401(error) => error.error,
            WorkerError::Error403(error) => error.error,
            WorkerError::Error404(error) => error.error,
            WorkerError::Error409(error) => error.error,
            WorkerError::Error500(error) => display_golem_error(error.golem_error),
        }
    }
}

impl ResponseContentErrorMapper for HealthCheckError {
    fn map(self) -> String {
        match self {}
    }
}

impl ResponseContentErrorMapper for ApiCertificateError {
    fn map(self) -> String {
        match self {
            ApiCertificateError::Error400(errors) => display_worker_service_errors_body(errors),
            ApiCertificateError::Error401(error) => error.error,
            ApiCertificateError::Error403(error) => error.error,
            ApiCertificateError::Error404(message) => message.message,
            ApiCertificateError::Error409(error) => error.to_string(),
            ApiCertificateError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ApiDefinitionError {
    fn map(self) -> String {
        match self {
            ApiDefinitionError::Error400(errors) => display_worker_service_errors_body(errors),
            ApiDefinitionError::Error401(error) => error.error,
            ApiDefinitionError::Error403(error) => error.error,
            ApiDefinitionError::Error404(message) => message.message,
            ApiDefinitionError::Error409(error) => error.to_string(),
            ApiDefinitionError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ApiDeploymentError {
    fn map(self) -> String {
        match self {
            ApiDeploymentError::Error400(errors) => display_worker_service_errors_body(errors),
            ApiDeploymentError::Error401(error) => error.error,
            ApiDeploymentError::Error403(error) => error.error,
            ApiDeploymentError::Error404(message) => message.message,
            ApiDeploymentError::Error409(error) => error.to_string(),
            ApiDeploymentError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ApiDomainError {
    fn map(self) -> String {
        match self {
            ApiDomainError::Error400(errors) => display_worker_service_errors_body(errors),
            ApiDomainError::Error401(error) => error.error,
            ApiDomainError::Error403(error) => error.error,
            ApiDomainError::Error404(message) => message.message,
            ApiDomainError::Error409(error) => error.to_string(),
            ApiDomainError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for PluginError {
    fn map(self) -> String {
        match self {
            PluginError::Error400(errors) => errors.errors.iter().join(", "),
            PluginError::Error401(error) => error.error,
            PluginError::Error403(error) => error.error,
            PluginError::Error404(error) => error.error,
            PluginError::Error409(error) => error.error,
            PluginError::Error500(error) => error.error,
        }
    }
}
