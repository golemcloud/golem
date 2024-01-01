use golem_gateway_client::api::ApiCertificateError;
use golem_gateway_client::api::ApiDefinitionError;
use golem_gateway_client::api::ApiDeploymentError;
use golem_gateway_client::api::ApiDomainError;
use golem_gateway_client::api::HealthcheckError;

pub trait ResponseContentErrorMapper {
    fn map(self) -> String;
}

impl ResponseContentErrorMapper for ApiCertificateError {
    fn map(self) -> String {
        match self {
            ApiCertificateError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            ApiCertificateError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            ApiCertificateError::Error403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            ApiCertificateError::Error404(message) => {
                format!("NotFound: {message:?}")
            }
            ApiCertificateError::Error409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            ApiCertificateError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for ApiDefinitionError {
    fn map(self) -> String {
        match self {
            ApiDefinitionError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            ApiDefinitionError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            ApiDefinitionError::Error403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            ApiDefinitionError::Error404(message) => {
                format!("NotFound: {message:?}")
            }
            ApiDefinitionError::Error409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            ApiDefinitionError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for ApiDeploymentError {
    fn map(self) -> String {
        match self {
            ApiDeploymentError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            ApiDeploymentError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            ApiDeploymentError::Error403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            ApiDeploymentError::Error404(message) => {
                format!("NotFound: {message:?}")
            }
            ApiDeploymentError::Error409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            ApiDeploymentError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for ApiDomainError {
    fn map(self) -> String {
        match self {
            ApiDomainError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            ApiDomainError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            ApiDomainError::Error403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            ApiDomainError::Error404(message) => {
                format!("NotFound: {message:?}")
            }
            ApiDomainError::Error409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            ApiDomainError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for HealthcheckError {
    fn map(self) -> String {
        match self {}
    }
}
