// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::model::ResponseContentErrorMapper;
use golem_cloud_client::api::WorkerError;
use golem_cloud_client::api::{
    AccountError, ApiCertificateError, ApiDefinitionError, ApiDeploymentError, ApiDomainError,
    ComponentError, GrantError, HealthCheckError, LoginError, ProjectError, ProjectGrantError,
    ProjectPolicyError, TokenError,
};

impl ResponseContentErrorMapper for AccountError {
    fn map(self) -> String {
        match self {
            AccountError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            AccountError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            AccountError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            AccountError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for GrantError {
    fn map(self) -> String {
        match self {
            GrantError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            GrantError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            GrantError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            GrantError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for LoginError {
    fn map(self) -> String {
        match self {
            LoginError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            LoginError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            LoginError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for ProjectError {
    fn map(self) -> String {
        match self {
            ProjectError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            ProjectError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            ProjectError::Error403(error) => {
                format!("Forbidden: {error:?}")
            }
            ProjectError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            ProjectError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for ProjectGrantError {
    fn map(self) -> String {
        match self {
            ProjectGrantError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            ProjectGrantError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            ProjectGrantError::Error403(error) => {
                format!("Forbidden: {error:?}")
            }
            ProjectGrantError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            ProjectGrantError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

#[allow(unreachable_patterns)]
impl ResponseContentErrorMapper for ProjectPolicyError {
    fn map(self) -> String {
        match self {
            ProjectPolicyError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            ProjectPolicyError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            ProjectPolicyError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            ProjectPolicyError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
            _ => "UnknownError".into(),
        }
    }
}

impl ResponseContentErrorMapper for ComponentError {
    fn map(self) -> String {
        match self {
            ComponentError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            ComponentError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            ComponentError::Error403(error) => {
                format!("Forbidden: {error:?}")
            }
            ComponentError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            ComponentError::Error409(error) => {
                format!("Conflict: {error:?}")
            }
            ComponentError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for TokenError {
    fn map(self) -> String {
        match self {
            TokenError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            TokenError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            TokenError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            TokenError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for WorkerError {
    fn map(self) -> String {
        match self {
            WorkerError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            WorkerError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            WorkerError::Error403(error) => {
                format!("Forbidden: {error:?}")
            }
            WorkerError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            WorkerError::Error409(error) => {
                format!("Conflict: {error:?}")
            }
            WorkerError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
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
