// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

pub mod aws_config;
pub mod aws_load_balancer;
pub mod aws_provisioner;
pub mod provisioner;

use self::provisioner::DomainProvisioner;
use super::environment::{EnvironmentError, EnvironmentService};
use crate::repo::domain_registration::DomainRegistrationRepo;
use crate::repo::model::audit::ImmutableAuditFields;
use crate::repo::model::domain_registration::{
    DomainRegistrationRecord, DomainRegistrationRepoError,
};
use golem_common::model::domain_registration::{
    Domain, DomainRegistration, DomainRegistrationCreation, DomainRegistrationId,
};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::fmt::Debug;
use std::sync::Arc;

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

pub struct DomainRegistrationService {
    domain_registration_repo: Arc<dyn DomainRegistrationRepo>,
    environment_service: Arc<EnvironmentService>,
    domain_provisioner: Arc<dyn DomainProvisioner>,
}

impl DomainRegistrationService {
    pub fn new(
        domain_registration_repo: Arc<dyn DomainRegistrationRepo>,
        environment_service: Arc<EnvironmentService>,
        domain_provisioner: Arc<dyn DomainProvisioner>,
    ) -> Self {
        Self {
            domain_registration_repo,
            environment_service,
            domain_provisioner,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: DomainRegistrationCreation,
        auth: &AuthCtx,
    ) -> Result<DomainRegistration, DomainRegistrationError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DomainRegistrationError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateEnvironmentPluginGrant,
        )?;

        if !self
            .domain_provisioner
            .domain_available_to_provision(&data.domain)
        {
            return Err(DomainRegistrationError::DomainCannotBeProvisioned(
                data.domain,
            ));
        }

        let domain_registration = DomainRegistration {
            id: DomainRegistrationId::new(),
            environment_id,
            domain: data.domain.clone(),
        };

        let record = DomainRegistrationRecord::from_model(
            domain_registration,
            ImmutableAuditFields::new(auth.account_id().0),
        );

        let created: DomainRegistration = self
            .domain_registration_repo
            .create(record)
            .await
            .map_err(|err| match err {
                DomainRegistrationRepoError::DomainAlreadyExists => {
                    DomainRegistrationError::DomainAlreadyExists(data.domain)
                }
                other => other.into(),
            })?
            .into();

        // TODO: this needs to be durable in some way / we need a cron job that ensures all domains actually reflect our db state;
        self.domain_provisioner
            .provision_domain(&created.domain)
            .await?;

        Ok(created)
    }

    pub async fn delete(
        &self,
        domain_registration_id: DomainRegistrationId,
        auth: &AuthCtx,
    ) -> Result<DomainRegistration, DomainRegistrationError> {
        let (_, environment) = self
            .get_by_id_with_environment(domain_registration_id, auth)
            .await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteDomainRegistration,
        )?;

        let deleted: DomainRegistration = self
            .domain_registration_repo
            .delete(domain_registration_id.0, auth.account_id().0)
            .await?
            .ok_or(DomainRegistrationError::DomainRegistrationNotFound(
                domain_registration_id,
            ))?
            .into();

        // TODO: this needs to be durable in some way / we need a cron job that ensures all domains actually reflect our db state;
        self.domain_provisioner
            .remove_domain(&deleted.domain)
            .await?;

        Ok(deleted)
    }

    pub async fn get_by_id(
        &self,
        domain_registration_id: DomainRegistrationId,
        auth: &AuthCtx,
    ) -> Result<DomainRegistration, DomainRegistrationError> {
        Ok(self
            .get_by_id_with_environment(domain_registration_id, auth)
            .await?
            .0)
    }

    pub async fn get_in_environment(
        &self,
        environment: &Environment,
        domain: &Domain,
        auth: &AuthCtx,
    ) -> Result<DomainRegistration, DomainRegistrationError> {
        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDomainRegistration,
        )
        .map_err(|_| DomainRegistrationError::DomainRegistrationByDomainNotFound(domain.clone()))?;

        let domain_registration: DomainRegistration = self
            .domain_registration_repo
            .get_in_environment(environment.id.0, &domain.0)
            .await?
            .ok_or(DomainRegistrationError::DomainRegistrationByDomainNotFound(
                domain.clone(),
            ))?
            .into();

        Ok(domain_registration)
    }

    pub async fn list_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<DomainRegistration>, DomainRegistrationError> {
        // Optimally this is fetched together with the grant data instead of up front
        // see EnvironmentService::list_in_application for a better pattern
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    DomainRegistrationError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDomainRegistration,
        )?;

        let domain_registrations: Vec<DomainRegistration> = self
            .domain_registration_repo
            .list_by_environment(environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(domain_registrations)
    }

    async fn get_by_id_with_environment(
        &self,
        domain_registration_id: DomainRegistrationId,
        auth: &AuthCtx,
    ) -> Result<(DomainRegistration, Environment), DomainRegistrationError> {
        let domain_registration: DomainRegistration = self
            .domain_registration_repo
            .get_by_id(domain_registration_id.0)
            .await?
            .ok_or(DomainRegistrationError::DomainRegistrationNotFound(
                domain_registration_id,
            ))?
            .into();

        let environment = self
            .environment_service
            .get(domain_registration.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    DomainRegistrationError::DomainRegistrationNotFound(domain_registration_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewDomainRegistration,
        )
        .map_err(|_| DomainRegistrationError::DomainRegistrationNotFound(domain_registration_id))?;

        Ok((domain_registration, environment))
    }
}
