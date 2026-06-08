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

use super::deployment::{DeploymentError, DeploymentService};
use super::domain_registration::{DomainRegistrationError, DomainRegistrationService};
use super::environment::{EnvironmentError, EnvironmentService};
use crate::repo::http_api_deployment::HttpApiDeploymentRepo;
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::http_api_deployment::{
    HttpApiDeploymentAuthExtRevisionRecord, HttpApiDeploymentRepoError,
    HttpApiDeploymentRevisionRecord,
};
use golem_common::model::account::AccountEmail;
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EnvironmentHttpApiDeploymentResourcePattern,
    EnvironmentHttpApiDeploymentVerb, PermissionTarget,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::{Environment, EnvironmentId, EnvironmentName};
use golem_common::model::http_api_deployment::{
    HttpApiDeployment, HttpApiDeploymentCreation, HttpApiDeploymentId, HttpApiDeploymentRevision,
    HttpApiDeploymentUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum HttpApiDeploymentError {
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Http api deployment for id {0} not found")]
    HttpApiDeploymentNotFound(HttpApiDeploymentId),
    #[error("Http api deployment for domain {0} not found")]
    HttpApiDeploymentByDomainNotFound(Domain),
    #[error("Deployment revision {0} does not exist")]
    DeploymentRevisionNotFound(DeploymentRevision),
    #[error("Http api deployment for domain {0} already exists in this environment")]
    HttpApiDeploymentForDomainAlreadyExists(Domain),
    #[error("Domain {0} is not registered")]
    DomainNotRegistered(Domain),
    #[error(
        "Domain {domain} cannot be used for an HTTP API deployment. Only {direct_only_fragment}subdomains of {available_domain} may be used.",
        direct_only_fragment = if !allow_arbitrary_subdomains { "direct " } else { "" }
    )]
    DomainNotValidForHttpApi {
        domain: Domain,
        available_domain: String,
        allow_arbitrary_subdomains: bool,
    },
    #[error("Concurrent update attempt")]
    ConcurrentUpdate,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

fn authorize_http_api_deployment_permission(
    auth: &AuthCtx,
    environment: &Environment,
    domain: Option<&Domain>,
    verb: EnvironmentHttpApiDeploymentVerb,
) -> Result<(), AuthorizationError> {
    authorize_http_api_deployment_permission_for_owner(
        auth,
        EnvironmentOwnerPattern::Environment {
            account: environment.owner_account_email.clone(),
            application: environment.application_name.clone(),
            environment: environment.name.clone(),
        },
        domain,
        verb,
    )
}

fn authorize_http_api_deployment_permission_for_owner(
    auth: &AuthCtx,
    owner: EnvironmentOwnerPattern,
    domain: Option<&Domain>,
    verb: EnvironmentHttpApiDeploymentVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentHttpApiDeployment(
        ClassPermissionTarget {
            verb: Some(verb),
            owner,
            resource: domain
                .map(
                    |domain| EnvironmentHttpApiDeploymentResourcePattern::DomainPath {
                        domain: domain.0.clone(),
                        path_glob: "/**".to_string(),
                    },
                )
                .unwrap_or(EnvironmentHttpApiDeploymentResourcePattern::Any),
        },
    ))
}

fn environment_owner_from_deployment(
    deployment: &HttpApiDeploymentAuthExtRevisionRecord,
) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: AccountEmail::new(deployment.owner_account_email.clone()),
        application: ApplicationName(deployment.application_name.clone()),
        environment: EnvironmentName(deployment.environment_name.clone()),
    }
}

impl SafeDisplay for HttpApiDeploymentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::DeploymentRevisionNotFound(_) => self.to_string(),
            Self::HttpApiDeploymentNotFound(_) => self.to_string(),
            Self::HttpApiDeploymentByDomainNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::HttpApiDeploymentForDomainAlreadyExists(_) => self.to_string(),
            Self::DomainNotRegistered(_) => self.to_string(),
            Self::DomainNotValidForHttpApi { .. } => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    HttpApiDeploymentError,
    HttpApiDeploymentRepoError,
    RepoError,
    EnvironmentError,
    DeploymentError,
    DomainRegistrationError,
);

pub struct HttpApiDeploymentService {
    http_api_deployment_repo: Arc<dyn HttpApiDeploymentRepo>,
    environment_service: Arc<EnvironmentService>,
    deployment_service: Arc<DeploymentService>,
    domain_registration_service: Arc<DomainRegistrationService>,
}

impl HttpApiDeploymentService {
    pub fn new(
        http_api_deployment_repo: Arc<dyn HttpApiDeploymentRepo>,
        environment_service: Arc<EnvironmentService>,
        deployment_service: Arc<DeploymentService>,
        domain_registration_service: Arc<DomainRegistrationService>,
    ) -> Self {
        Self {
            http_api_deployment_repo,
            environment_service,
            deployment_service,
            domain_registration_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: HttpApiDeploymentCreation,
        auth: &AuthCtx,
    ) -> Result<HttpApiDeployment, HttpApiDeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(id) => {
                    HttpApiDeploymentError::ParentEnvironmentNotFound(id)
                }
                other => other.into(),
            })?;

        authorize_http_api_deployment_permission(
            auth,
            &environment,
            Some(&data.domain),
            EnvironmentHttpApiDeploymentVerb::Create,
        )?;

        self.domain_registration_service
            .get_in_environment(&environment, &data.domain, auth)
            .await
            .map_err(|err| match err {
                DomainRegistrationError::DomainRegistrationByDomainNotFound(domain) => {
                    HttpApiDeploymentError::DomainNotRegistered(domain)
                }
                other => other.into(),
            })?;

        self.domain_registration_service
            .validate_domain_for_http_api(&data.domain)
            .map_err(|err| match err {
                DomainRegistrationError::DomainNotValidForHttpApi {
                    domain,
                    available_domain,
                    allow_arbitrary_subdomains,
                } => HttpApiDeploymentError::DomainNotValidForHttpApi {
                    domain,
                    available_domain,
                    allow_arbitrary_subdomains,
                },
                other => other.into(),
            })?;

        let id = HttpApiDeploymentId::new();
        let record = HttpApiDeploymentRevisionRecord::creation(
            id,
            HttpApiDeploymentCreation::normalize_webhooks_prefix(data.webhooks_prefix),
            HttpApiDeploymentCreation::normalize_openapi_endpoint_prefix(
                data.openapi_endpoint_prefix,
            ),
            data.agents,
            auth.actor_account_id(),
        )?;

        let stored_http_api_deployment: HttpApiDeployment = self
            .http_api_deployment_repo
            .create(environment_id.0, &data.domain.0, record)
            .await
            .map_err(|err| match err {
                HttpApiDeploymentRepoError::ConcurrentModification => {
                    HttpApiDeploymentError::ConcurrentUpdate
                }
                HttpApiDeploymentRepoError::ApiDeploymentViolatesUniqueness => {
                    HttpApiDeploymentError::HttpApiDeploymentForDomainAlreadyExists(data.domain)
                }
                other => other.into(),
            })?
            .try_into()?;

        Ok(stored_http_api_deployment)
    }

    pub async fn update(
        &self,
        http_api_deployment_id: HttpApiDeploymentId,
        update: HttpApiDeploymentUpdate,
        auth: &AuthCtx,
    ) -> Result<HttpApiDeployment, HttpApiDeploymentError> {
        let deployment_record = self
            .http_api_deployment_repo
            .get_staged_by_id(http_api_deployment_id.0)
            .await?
            .ok_or(HttpApiDeploymentError::HttpApiDeploymentNotFound(
                http_api_deployment_id,
            ))?;

        let owner = environment_owner_from_deployment(&deployment_record);
        let domain = Domain(deployment_record.deployment.domain.clone());
        let mut http_api_deployment: HttpApiDeployment = deployment_record.deployment.try_into()?;

        authorize_http_api_deployment_permission_for_owner(
            auth,
            owner.clone(),
            Some(&domain),
            EnvironmentHttpApiDeploymentVerb::View,
        )
        .map_err(|_| HttpApiDeploymentError::HttpApiDeploymentNotFound(http_api_deployment_id))?;

        authorize_http_api_deployment_permission_for_owner(
            auth,
            owner,
            Some(&domain),
            EnvironmentHttpApiDeploymentVerb::Update,
        )?;

        if update.current_revision != http_api_deployment.revision {
            Err(HttpApiDeploymentError::ConcurrentUpdate)?
        };

        http_api_deployment.revision = http_api_deployment.revision.next()?;
        if let Some(webhooks_url) = update.webhook_prefix {
            http_api_deployment.webhooks_prefix =
                HttpApiDeploymentCreation::normalize_webhooks_prefix(webhooks_url);
        };
        if let Some(openapi_endpoint) = update.openapi_endpoint_prefix {
            http_api_deployment.openapi_endpoint_prefix =
                HttpApiDeploymentCreation::normalize_openapi_endpoint_prefix(openapi_endpoint);
        };
        if let Some(api_definitions) = update.agents {
            http_api_deployment.agents = api_definitions;
        };

        let record = HttpApiDeploymentRevisionRecord::from_model(
            http_api_deployment,
            DeletableRevisionAuditFields::new(auth.actor_account_id().0),
        )?;

        let stored_http_api_deployment: HttpApiDeployment = self
            .http_api_deployment_repo
            .update(record)
            .await
            .map_err(|err| match err {
                HttpApiDeploymentRepoError::ConcurrentModification => {
                    HttpApiDeploymentError::ConcurrentUpdate
                }
                other => other.into(),
            })?
            .try_into()?;

        Ok(stored_http_api_deployment)
    }

    pub async fn delete(
        &self,
        http_api_deployment_id: HttpApiDeploymentId,
        current_revision: HttpApiDeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<(), HttpApiDeploymentError> {
        let deployment_record = self
            .http_api_deployment_repo
            .get_staged_by_id(http_api_deployment_id.0)
            .await?
            .ok_or(HttpApiDeploymentError::HttpApiDeploymentNotFound(
                http_api_deployment_id,
            ))?;

        let owner = environment_owner_from_deployment(&deployment_record);
        let domain = Domain(deployment_record.deployment.domain.clone());
        let http_api_deployment: HttpApiDeployment = deployment_record.deployment.try_into()?;

        authorize_http_api_deployment_permission_for_owner(
            auth,
            owner.clone(),
            Some(&domain),
            EnvironmentHttpApiDeploymentVerb::View,
        )
        .map_err(|_| HttpApiDeploymentError::HttpApiDeploymentNotFound(http_api_deployment_id))?;

        authorize_http_api_deployment_permission_for_owner(
            auth,
            owner,
            Some(&domain),
            EnvironmentHttpApiDeploymentVerb::Delete,
        )?;

        if current_revision != http_api_deployment.revision {
            Err(HttpApiDeploymentError::ConcurrentUpdate)?
        };

        self.http_api_deployment_repo
            .delete(
                auth.actor_account_id().0,
                http_api_deployment_id.0,
                current_revision.next()?.into(),
            )
            .await
            .map_err(|err| match err {
                HttpApiDeploymentRepoError::ConcurrentModification => {
                    HttpApiDeploymentError::ConcurrentUpdate
                }
                other => other.into(),
            })?;

        Ok(())
    }

    pub async fn get_revision(
        &self,
        http_api_deployment_id: HttpApiDeploymentId,
        revision: HttpApiDeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<HttpApiDeployment, HttpApiDeploymentError> {
        let deployment_record = self
            .http_api_deployment_repo
            .get_by_id_and_revision(http_api_deployment_id.0, revision.into())
            .await?
            .ok_or(HttpApiDeploymentError::HttpApiDeploymentNotFound(
                http_api_deployment_id,
            ))?;

        let owner = environment_owner_from_deployment(&deployment_record);
        let domain = Domain(deployment_record.deployment.domain.clone());
        let http_api_deployment: HttpApiDeployment = deployment_record.deployment.try_into()?;

        authorize_http_api_deployment_permission_for_owner(
            auth,
            owner,
            Some(&domain),
            EnvironmentHttpApiDeploymentVerb::View,
        )
        .map_err(|_| HttpApiDeploymentError::HttpApiDeploymentNotFound(http_api_deployment_id))?;

        Ok(http_api_deployment)
    }

    pub async fn list_staged(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<HttpApiDeployment>, HttpApiDeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    HttpApiDeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        self.list_staged_for_environment(&environment, auth).await
    }

    pub async fn list_staged_for_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Vec<HttpApiDeployment>, HttpApiDeploymentError> {
        let http_api_deployments: Vec<HttpApiDeployment> = self
            .http_api_deployment_repo
            .list_staged(environment.id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(http_api_deployments
            .into_iter()
            .filter(|http_api_deployment| {
                authorize_http_api_deployment_permission(
                    auth,
                    environment,
                    Some(&http_api_deployment.domain),
                    EnvironmentHttpApiDeploymentVerb::View,
                )
                .is_ok()
            })
            .collect())
    }

    pub async fn list_in_deployment(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<Vec<HttpApiDeployment>, HttpApiDeploymentError> {
        let (_, environment) = self
            .deployment_service
            .get_deployment_and_environment(environment_id, deployment_revision, auth)
            .await
            .map_err(|err| match err {
                DeploymentError::ParentEnvironmentNotFound(environment_id) => {
                    HttpApiDeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                DeploymentError::DeploymentNotFound(deployment_revision) => {
                    HttpApiDeploymentError::DeploymentRevisionNotFound(deployment_revision)
                }
                other => other.into(),
            })?;

        let http_api_deployments: Vec<HttpApiDeployment> = self
            .http_api_deployment_repo
            .list_by_deployment(environment_id.0, deployment_revision.into())
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(http_api_deployments
            .into_iter()
            .filter(|http_api_deployment| {
                authorize_http_api_deployment_permission(
                    auth,
                    &environment,
                    Some(&http_api_deployment.domain),
                    EnvironmentHttpApiDeploymentVerb::View,
                )
                .is_ok()
            })
            .collect())
    }

    pub async fn get_staged(
        &self,
        http_api_deployment_id: HttpApiDeploymentId,
        auth: &AuthCtx,
    ) -> Result<HttpApiDeployment, HttpApiDeploymentError> {
        let deployment_record = self
            .http_api_deployment_repo
            .get_staged_by_id(http_api_deployment_id.0)
            .await?
            .ok_or(HttpApiDeploymentError::HttpApiDeploymentNotFound(
                http_api_deployment_id,
            ))?;

        let owner = environment_owner_from_deployment(&deployment_record);
        let domain = Domain(deployment_record.deployment.domain.clone());
        let http_api_deployment: HttpApiDeployment = deployment_record.deployment.try_into()?;

        authorize_http_api_deployment_permission_for_owner(
            auth,
            owner,
            Some(&domain),
            EnvironmentHttpApiDeploymentVerb::View,
        )
        .map_err(|_| HttpApiDeploymentError::HttpApiDeploymentNotFound(http_api_deployment_id))?;

        Ok(http_api_deployment)
    }

    pub async fn get_staged_by_domain(
        &self,
        environment_id: EnvironmentId,
        domain: &Domain,
        auth: &AuthCtx,
    ) -> Result<HttpApiDeployment, HttpApiDeploymentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    HttpApiDeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        authorize_http_api_deployment_permission(
            auth,
            &environment,
            Some(domain),
            EnvironmentHttpApiDeploymentVerb::View,
        )
        .map_err(|_| HttpApiDeploymentError::HttpApiDeploymentByDomainNotFound(domain.clone()))?;

        let http_api_deployment: HttpApiDeployment = self
            .http_api_deployment_repo
            .get_staged_by_domain(environment_id.0, &domain.0)
            .await?
            .ok_or(HttpApiDeploymentError::HttpApiDeploymentByDomainNotFound(
                domain.clone(),
            ))?
            .try_into()?;

        Ok(http_api_deployment)
    }

    pub async fn get_in_deployment_by_domain(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        domain: &Domain,
        auth: &AuthCtx,
    ) -> Result<HttpApiDeployment, HttpApiDeploymentError> {
        let (_, environment) = self
            .deployment_service
            .get_deployment_and_environment(environment_id, deployment_revision, auth)
            .await
            .map_err(|err| match err {
                DeploymentError::ParentEnvironmentNotFound(environment_id) => {
                    HttpApiDeploymentError::ParentEnvironmentNotFound(environment_id)
                }
                DeploymentError::DeploymentNotFound(deployment_revision) => {
                    HttpApiDeploymentError::DeploymentRevisionNotFound(deployment_revision)
                }
                other => other.into(),
            })?;

        authorize_http_api_deployment_permission(
            auth,
            &environment,
            Some(domain),
            EnvironmentHttpApiDeploymentVerb::View,
        )
        .map_err(|_| HttpApiDeploymentError::HttpApiDeploymentByDomainNotFound(domain.clone()))?;

        let http_api_deployment: HttpApiDeployment = self
            .http_api_deployment_repo
            .get_in_deployment_by_domain(environment_id.0, deployment_revision.into(), &domain.0)
            .await?
            .ok_or(HttpApiDeploymentError::HttpApiDeploymentByDomainNotFound(
                domain.clone(),
            ))?
            .try_into()?;

        Ok(http_api_deployment)
    }
}
