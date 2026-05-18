// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::repo::model::domain_registration::DomainRegistrationRepoError;
use crate::services::environment::EnvironmentError;
use golem_common::model::domain_registration::{Domain, DomainRegistrationId};
use golem_common::model::environment::EnvironmentId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::AuthorizationError;
use std::fmt::Debug;

#[derive(Debug, thiserror::Error)]
pub enum DomainRegistrationError {
    #[error("Domain {0} cannot be provisioned")]
    DomainCannotBeProvisioned(Domain),
    #[error("Registration for id {0} not found")]
    DomainRegistrationNotFound(DomainRegistrationId),
    #[error("Registration for domain {0} not found in the environment")]
    DomainRegistrationByDomainNotFound(Domain),
    #[error("Parent environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Domain is already registered: {0}")]
    DomainAlreadyExists(Domain),
    #[error(
        "Domain {domain} cannot be used for an HTTP API deployment. Only {direct_only_fragment}subdomains of {available_domain} may be used.",
        direct_only_fragment = if !allow_arbitrary_subdomains { "direct " } else { "" }
    )]
    DomainNotValidForHttpApi {
        domain: Domain,
        available_domain: String,
        allow_arbitrary_subdomains: bool,
    },
    #[error(
        "Domain {domain} cannot be used for an MCP deployment. Only {direct_only_fragment}subdomains of {available_domain} may be used.",
        direct_only_fragment = if !allow_arbitrary_subdomains { "direct " } else { "" }
    )]
    DomainNotValidForMcp {
        domain: Domain,
        available_domain: String,
        allow_arbitrary_subdomains: bool,
    },
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for DomainRegistrationError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::DomainCannotBeProvisioned(_) => self.to_string(),
            Self::DomainRegistrationNotFound(_) => self.to_string(),
            Self::DomainRegistrationByDomainNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::DomainAlreadyExists(_) => self.to_string(),
            Self::DomainNotValidForHttpApi { .. } => self.to_string(),
            Self::DomainNotValidForMcp { .. } => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    DomainRegistrationError,
    EnvironmentError,
    DomainRegistrationRepoError
);
