use golem_gateway_client::apis::api_certificate_api::{
    V1ApiCertificatesDeleteError, V1ApiCertificatesGetError, V1ApiCertificatesPostError,
};
use golem_gateway_client::apis::api_definition_api::{
    V1ApiDefinitionsDeleteError, V1ApiDefinitionsGetError, V1ApiDefinitionsPutError,
};
use golem_gateway_client::apis::api_deployment_api::{
    V1ApiDeploymentsDeleteError, V1ApiDeploymentsGetError, V1ApiDeploymentsPutError,
};
use golem_gateway_client::apis::api_domain_api::{
    V1ApiDomainsDeleteError, V1ApiDomainsGetError, V1ApiDomainsPutError,
};
use golem_gateway_client::apis::healthcheck_api::HealthcheckGetError;

pub trait ResponseContentErrorMapper {
    fn map(self) -> String;
}

impl ResponseContentErrorMapper for HealthcheckGetError {
    fn map(self) -> String {
        match self {
            HealthcheckGetError::UnknownValue(value) => value.to_string(),
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDeploymentsGetError {
    fn map(self) -> String {
        match self {
            V1ApiDeploymentsGetError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDeploymentsGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDeploymentsGetError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDeploymentsGetError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDeploymentsGetError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDeploymentsGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDeploymentsGetError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDeploymentsPutError {
    fn map(self) -> String {
        match self {
            V1ApiDeploymentsPutError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDeploymentsPutError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDeploymentsPutError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDeploymentsPutError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDeploymentsPutError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDeploymentsPutError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDeploymentsPutError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDeploymentsDeleteError {
    fn map(self) -> String {
        match self {
            V1ApiDeploymentsDeleteError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDeploymentsDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDeploymentsDeleteError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDeploymentsDeleteError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDeploymentsDeleteError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDeploymentsDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDeploymentsDeleteError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDefinitionsGetError {
    fn map(self) -> String {
        match self {
            V1ApiDefinitionsGetError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDefinitionsGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDefinitionsGetError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDefinitionsGetError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDefinitionsGetError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDefinitionsGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDefinitionsGetError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDefinitionsPutError {
    fn map(self) -> String {
        match self {
            V1ApiDefinitionsPutError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDefinitionsPutError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDefinitionsPutError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDefinitionsPutError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDefinitionsPutError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDefinitionsPutError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDefinitionsPutError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDefinitionsDeleteError {
    fn map(self) -> String {
        match self {
            V1ApiDefinitionsDeleteError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDefinitionsDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDefinitionsDeleteError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDefinitionsDeleteError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDefinitionsDeleteError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDefinitionsDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDefinitionsDeleteError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiCertificatesGetError {
    fn map(self) -> String {
        match self {
            V1ApiCertificatesGetError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiCertificatesGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiCertificatesGetError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiCertificatesGetError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiCertificatesGetError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiCertificatesGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiCertificatesGetError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiCertificatesPostError {
    fn map(self) -> String {
        match self {
            V1ApiCertificatesPostError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiCertificatesPostError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiCertificatesPostError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiCertificatesPostError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiCertificatesPostError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiCertificatesPostError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiCertificatesPostError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiCertificatesDeleteError {
    fn map(self) -> String {
        match self {
            V1ApiCertificatesDeleteError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiCertificatesDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiCertificatesDeleteError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiCertificatesDeleteError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiCertificatesDeleteError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiCertificatesDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiCertificatesDeleteError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDomainsGetError {
    fn map(self) -> String {
        match self {
            V1ApiDomainsGetError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDomainsGetError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDomainsGetError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDomainsGetError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDomainsGetError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDomainsGetError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDomainsGetError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDomainsPutError {
    fn map(self) -> String {
        match self {
            V1ApiDomainsPutError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDomainsPutError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDomainsPutError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDomainsPutError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDomainsPutError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDomainsPutError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDomainsPutError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for V1ApiDomainsDeleteError {
    fn map(self) -> String {
        match self {
            V1ApiDomainsDeleteError::Status400(errors) => {
                // FIXME: fix schema interpretation
                format!("BadRequest: {errors:?}")
            }
            V1ApiDomainsDeleteError::Status401(error) => {
                format!("Unauthorized: {error:?}")
            }
            V1ApiDomainsDeleteError::Status403(error) => {
                format!("LimitExceeded: {error:?}")
            }
            V1ApiDomainsDeleteError::Status404(message) => {
                format!("NotFound: {message:?}")
            }
            V1ApiDomainsDeleteError::Status409(string) => {
                format!("AlreadyExists: {string:?}")
            }
            V1ApiDomainsDeleteError::Status500(error) => {
                format!("InternalError: {error:?}")
            }
            V1ApiDomainsDeleteError::UnknownValue(value) => {
                format!("Unexpected error: {value:?}")
            }
        }
    }
}
