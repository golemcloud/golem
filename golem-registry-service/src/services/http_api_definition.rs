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

use super::deployment::{DeploymentError, DeploymentService};
use super::environment::{EnvironmentError, EnvironmentService};
use super::security_scheme::SecuritySchemeService;
use crate::repo::http_api_definition::HttpApiDefinitionRepo;
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::http_api_definition::{
    HttpApiDefinitionRepoError, HttpApiDefinitionRevisionRecord,
};
use crate::services::security_scheme::SecuritySchemeError;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::http_api_definition::{
    GatewayBinding, HttpApiDefinition, HttpApiDefinitionCreation, HttpApiDefinitionId,
    HttpApiDefinitionName, HttpApiDefinitionRevision, HttpApiDefinitionUpdate,
    HttpApiDefinitionVersion, HttpApiRoute,
};
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::custom_api::path_pattern::AllPathPatterns;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError, EnvironmentAction};
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum HttpApiDefinitionError {
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Http api definition for id {0} not found")]
    HttpApiDefinitionNotFound(HttpApiDefinitionId),
    #[error("Http api definition for name {0} not found")]
    HttpApiDefinitionByNameNotFound(HttpApiDefinitionName),
    #[error("Deployment revision {0} does not exist")]
    DeploymentRevisionNotFound(DeploymentRevision),
    #[error("Invalid definition: {0}")]
    InvalidDefinition(String),
    #[error("Referenced security scheme {0} does not exist")]
    SecuritySchemeDoesNotExist(SecuritySchemeName),
    #[error("Http api definition with name {0} already exists in this environment")]
    HttpApiDefinitionWithNameAlreadyExists(HttpApiDefinitionName),
    #[error("Version {0} already exists for this http api definition")]
    HttpApiDefinitionVersionAlreadyExists(HttpApiDefinitionVersion),
    #[error("Concurrent update attempt")]
    ConcurrentUpdate,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for HttpApiDefinitionError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::DeploymentRevisionNotFound(_) => self.to_string(),
            Self::HttpApiDefinitionNotFound(_) => self.to_string(),
            Self::HttpApiDefinitionByNameNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::InvalidDefinition(_) => self.to_string(),
            Self::SecuritySchemeDoesNotExist(_) => self.to_string(),
            Self::HttpApiDefinitionWithNameAlreadyExists(_) => self.to_string(),
            Self::HttpApiDefinitionVersionAlreadyExists(_) => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    HttpApiDefinitionError,
    HttpApiDefinitionRepoError,
    RepoError,
    SecuritySchemeError,
    EnvironmentError,
    DeploymentError
);

pub struct HttpApiDefinitionService {
    http_api_definition_repo: Arc<dyn HttpApiDefinitionRepo>,
    environment_service: Arc<EnvironmentService>,
    security_scheme_service: Arc<SecuritySchemeService>,
    deployment_service: Arc<DeploymentService>,
}

impl HttpApiDefinitionService {
    pub fn new(
        http_api_definition_repo: Arc<dyn HttpApiDefinitionRepo>,
        environment_service: Arc<EnvironmentService>,
        security_scheme_service: Arc<SecuritySchemeService>,
        deployment_service: Arc<DeploymentService>,
    ) -> Self {
        Self {
            http_api_definition_repo,
            environment_service,
            security_scheme_service,
            deployment_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: &EnvironmentId,
        data: HttpApiDefinitionCreation,
        auth: &AuthCtx,
    ) -> Result<HttpApiDefinition, HttpApiDefinitionError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(id) => {
                    HttpApiDefinitionError::ParentEnvironmentNotFound(id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateHttpApiDefinition,
        )?;

        Self::validate_http_api_definition_version(&data.version)?;
        self.validate_http_api_definition_routes(&environment, &data.routes, auth)
            .await?;

        let id = HttpApiDefinitionId::new_v4();
        let record = HttpApiDefinitionRevisionRecord::creation(
            id,
            data.version,
            data.routes,
            auth.account_id().clone(),
        );

        let stored_http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .create(&environment_id.0, &data.name.0, record)
            .await
            .map_err(|err| match err {
                HttpApiDefinitionRepoError::ConcurrentModification
                | HttpApiDefinitionRepoError::VersionAlreadyExists { .. } => {
                    HttpApiDefinitionError::ConcurrentUpdate
                }
                HttpApiDefinitionRepoError::ApiDefinitionViolatesUniqueness => {
                    HttpApiDefinitionError::HttpApiDefinitionWithNameAlreadyExists(data.name)
                }
                other => other.into(),
            })?
            .into();

        Ok(stored_http_api_definition)
    }

    pub async fn update(
        &self,
        http_api_definition_id: &HttpApiDefinitionId,
        update: HttpApiDefinitionUpdate,
        auth: &AuthCtx,
    ) -> Result<HttpApiDefinition, HttpApiDefinitionError> {
        let mut http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .get_staged_by_id(&http_api_definition_id.0)
            .await?
            .ok_or(HttpApiDefinitionError::HttpApiDefinitionNotFound(
                http_api_definition_id.clone(),
            ))?
            .into();

        let environment = self
            .environment_service
            .get(&http_api_definition.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    HttpApiDefinitionError::HttpApiDefinitionNotFound(
                        http_api_definition_id.clone(),
                    )
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )
        .map_err(|_| {
            HttpApiDefinitionError::HttpApiDefinitionNotFound(http_api_definition_id.clone())
        })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::UpdateHttpApiDefinition,
        )?;

        // Fast path. If the current revision does not match we will reject it later anyway
        if update.current_revision != http_api_definition.revision {
            Err(HttpApiDefinitionError::ConcurrentUpdate)?
        };

        http_api_definition.revision = http_api_definition.revision.next()?;

        if let Some(new_version) = update.version {
            Self::validate_http_api_definition_version(&new_version)?;
            http_api_definition.version = new_version;
        }
        if let Some(new_routes) = update.routes {
            self.validate_http_api_definition_routes(&environment, &new_routes, auth)
                .await?;
            http_api_definition.routes = new_routes;
        }

        let record = HttpApiDefinitionRevisionRecord::from_model(
            http_api_definition,
            DeletableRevisionAuditFields::new(auth.account_id().0),
        );

        let stored_http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .update(update.current_revision.into(), record)
            .await
            .map_err(|err| match err {
                HttpApiDefinitionRepoError::ConcurrentModification => {
                    HttpApiDefinitionError::ConcurrentUpdate
                }
                HttpApiDefinitionRepoError::VersionAlreadyExists { version } => {
                    HttpApiDefinitionError::HttpApiDefinitionVersionAlreadyExists(
                        HttpApiDefinitionVersion(version),
                    )
                }
                other => other.into(),
            })?
            .into();

        Ok(stored_http_api_definition)
    }

    pub async fn delete(
        &self,
        http_api_definition_id: &HttpApiDefinitionId,
        current_revision: HttpApiDefinitionRevision,
        auth: &AuthCtx,
    ) -> Result<(), HttpApiDefinitionError> {
        let http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .get_staged_by_id(&http_api_definition_id.0)
            .await?
            .ok_or(HttpApiDefinitionError::HttpApiDefinitionNotFound(
                http_api_definition_id.clone(),
            ))?
            .into();

        let environment = self
            .environment_service
            .get(&http_api_definition.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    HttpApiDefinitionError::HttpApiDefinitionNotFound(
                        http_api_definition_id.clone(),
                    )
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )
        .map_err(|_| {
            HttpApiDefinitionError::HttpApiDefinitionNotFound(http_api_definition_id.clone())
        })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteHttpApiDefinition,
        )?;

        if current_revision != http_api_definition.revision {
            Err(HttpApiDefinitionError::ConcurrentUpdate)?
        };

        self.http_api_definition_repo
            .delete(
                &auth.account_id().0,
                &http_api_definition_id.0,
                current_revision.into(),
            )
            .await
            .map_err(|err| match err {
                HttpApiDefinitionRepoError::ConcurrentModification => {
                    HttpApiDefinitionError::ConcurrentUpdate
                }
                other => other.into(),
            })?;

        Ok(())
    }

    pub async fn list_staged(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<HttpApiDefinition>, HttpApiDefinitionError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    HttpApiDefinitionError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        self.list_staged_for_environment(&environment, auth).await
    }

    pub async fn list_staged_for_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Vec<HttpApiDefinition>, HttpApiDefinitionError> {
        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )?;

        let http_api_definitions: Vec<HttpApiDefinition> = self
            .http_api_definition_repo
            .list_staged(&environment.id.0)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(http_api_definitions)
    }

    pub async fn list_deployed(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<HttpApiDefinition>, HttpApiDefinitionError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    HttpApiDefinitionError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )?;

        let http_api_definitions: Vec<HttpApiDefinition> = self
            .http_api_definition_repo
            .list_deployed(&environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(http_api_definitions)
    }

    pub async fn list_in_deployment(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<Vec<HttpApiDefinition>, HttpApiDefinitionError> {
        let (_, environment) = self
            .deployment_service
            .get_deployment_and_environment(environment_id, deployment_revision, auth)
            .await
            .map_err(|err| match err {
                DeploymentError::ParentEnvironmentNotFound(environment_id) => {
                    HttpApiDefinitionError::ParentEnvironmentNotFound(environment_id)
                }
                DeploymentError::DeploymentNotFound(deployment_revision) => {
                    HttpApiDefinitionError::DeploymentRevisionNotFound(deployment_revision)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )?;

        let http_api_definitions: Vec<HttpApiDefinition> = self
            .http_api_definition_repo
            .list_by_deployment(&environment_id.0, deployment_revision.into())
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(http_api_definitions)
    }

    pub async fn get_staged(
        &self,
        http_api_definition_id: &HttpApiDefinitionId,
        auth: &AuthCtx,
    ) -> Result<HttpApiDefinition, HttpApiDefinitionError> {
        let http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .get_staged_by_id(&http_api_definition_id.0)
            .await?
            .ok_or(HttpApiDefinitionError::HttpApiDefinitionNotFound(
                http_api_definition_id.clone(),
            ))?
            .into();

        let environment = self
            .environment_service
            .get(&http_api_definition.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    HttpApiDefinitionError::HttpApiDefinitionNotFound(
                        http_api_definition_id.clone(),
                    )
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )
        .map_err(|_| {
            HttpApiDefinitionError::HttpApiDefinitionNotFound(http_api_definition_id.clone())
        })?;

        Ok(http_api_definition)
    }

    pub async fn get_deployed(
        &self,
        http_api_definition_id: &HttpApiDefinitionId,
        auth: &AuthCtx,
    ) -> Result<HttpApiDefinition, HttpApiDefinitionError> {
        let http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .get_deployed_by_id(&http_api_definition_id.0)
            .await?
            .ok_or(HttpApiDefinitionError::HttpApiDefinitionNotFound(
                http_api_definition_id.clone(),
            ))?
            .into();

        let environment = self
            .environment_service
            .get(&http_api_definition.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    HttpApiDefinitionError::HttpApiDefinitionNotFound(
                        http_api_definition_id.clone(),
                    )
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )
        .map_err(|_| {
            HttpApiDefinitionError::HttpApiDefinitionNotFound(http_api_definition_id.clone())
        })?;

        Ok(http_api_definition)
    }

    pub async fn get_staged_by_name(
        &self,
        environment_id: &EnvironmentId,
        http_api_definition_name: &HttpApiDefinitionName,
        auth: &AuthCtx,
    ) -> Result<HttpApiDefinition, HttpApiDefinitionError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    HttpApiDefinitionError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        let http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .get_staged_by_name(&environment_id.0, &http_api_definition_name.0)
            .await?
            .ok_or(HttpApiDefinitionError::HttpApiDefinitionByNameNotFound(
                http_api_definition_name.clone(),
            ))?
            .into();

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )
        .map_err(|_| {
            HttpApiDefinitionError::HttpApiDefinitionByNameNotFound(
                http_api_definition_name.clone(),
            )
        })?;

        Ok(http_api_definition)
    }

    pub async fn get_deployed_by_name(
        &self,
        environment_id: &EnvironmentId,
        http_api_definition_name: &HttpApiDefinitionName,
        auth: &AuthCtx,
    ) -> Result<HttpApiDefinition, HttpApiDefinitionError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    HttpApiDefinitionError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        let http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .get_deployed_by_name(&environment_id.0, &http_api_definition_name.0)
            .await?
            .ok_or(HttpApiDefinitionError::HttpApiDefinitionByNameNotFound(
                http_api_definition_name.clone(),
            ))?
            .into();

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )
        .map_err(|_| {
            HttpApiDefinitionError::HttpApiDefinitionByNameNotFound(
                http_api_definition_name.clone(),
            )
        })?;

        Ok(http_api_definition)
    }

    pub async fn get_in_deployment_by_name(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        http_api_definition_name: &HttpApiDefinitionName,
        auth: &AuthCtx,
    ) -> Result<HttpApiDefinition, HttpApiDefinitionError> {
        let (_, environment) = self
            .deployment_service
            .get_deployment_and_environment(environment_id, deployment_revision, auth)
            .await
            .map_err(|err| match err {
                DeploymentError::ParentEnvironmentNotFound(environment_id) => {
                    HttpApiDefinitionError::ParentEnvironmentNotFound(environment_id)
                }
                DeploymentError::DeploymentNotFound(deployment_revision) => {
                    HttpApiDefinitionError::DeploymentRevisionNotFound(deployment_revision)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewHttpApiDefinition,
        )
        .map_err(|_| {
            HttpApiDefinitionError::HttpApiDefinitionByNameNotFound(
                http_api_definition_name.clone(),
            )
        })?;

        let http_api_definition: HttpApiDefinition = self
            .http_api_definition_repo
            .get_in_deployment_by_name(
                &environment_id.0,
                deployment_revision.into(),
                &http_api_definition_name.0,
            )
            .await?
            .ok_or(HttpApiDefinitionError::HttpApiDefinitionByNameNotFound(
                http_api_definition_name.clone(),
            ))?
            .into();

        Ok(http_api_definition)
    }

    fn validate_http_api_definition_version(
        version: &HttpApiDefinitionVersion,
    ) -> Result<(), HttpApiDefinitionError> {
        // empty version string is reserved for deletions
        if version.0.is_empty() {
            return Err(HttpApiDefinitionError::InvalidDefinition(
                "Empty string is not a valid version".to_string(),
            ));
        }
        Ok(())
    }

    async fn validate_http_api_definition_routes(
        &self,
        environment: &Environment,
        routes: &[HttpApiRoute],
        auth: &AuthCtx,
    ) -> Result<(), HttpApiDefinitionError> {
        for route in routes {
            AllPathPatterns::parse(&route.path).map_err(|e| {
                HttpApiDefinitionError::InvalidDefinition(format!("Invalid path ({e})"))
            })?;

            Self::validate_gateway_binding(self, &route.binding)?;

            if let Some(security_definition_name) = &route.security {
                self.security_scheme_service
                    .get_security_scheme_for_environment_and_name(
                        environment,
                        security_definition_name,
                        auth,
                    )
                    .await
                    .map_err(|err| match err {
                        SecuritySchemeError::SecuritySchemeForNameNotFound(name) => {
                            HttpApiDefinitionError::SecuritySchemeDoesNotExist(name)
                        }
                        other => other.into(),
                    })?;
            };
        }

        // We could validate existence of components here, but this will be checked during deployment anyway.

        // TODO: check that (method + pattern) are non-overlapping.
        Ok(())
    }

    fn validate_gateway_binding(
        &self,
        binding: &GatewayBinding,
    ) -> Result<(), HttpApiDefinitionError> {
        match binding {
            GatewayBinding::CorsPreflight(inner) => {
                if let Some(response) = &inner.response {
                    rib::Expr::from_text(response).map_err(|e| {
                        HttpApiDefinitionError::InvalidDefinition(format!(
                            "Invalid cors preflight response expr: {e}"
                        ))
                    })?;
                }
            }
            GatewayBinding::FileServer(inner) => {
                rib::Expr::from_text(&inner.response).map_err(|e| {
                    HttpApiDefinitionError::InvalidDefinition(format!(
                        "Invalid file server response expr: {e}"
                    ))
                })?;
            }
            GatewayBinding::HttpHandler(inner) => {
                rib::Expr::from_text(&inner.worker_name).map_err(|e| {
                    HttpApiDefinitionError::InvalidDefinition(format!(
                        "Invalid http handler worker name expr: {e}"
                    ))
                })?;
                if let Some(idempotency_key) = &inner.idempotency_key {
                    rib::Expr::from_text(idempotency_key).map_err(|e| {
                        HttpApiDefinitionError::InvalidDefinition(format!(
                            "Invalid http handler idempotency key expr: {e}"
                        ))
                    })?;
                }
            }
            GatewayBinding::Worker(inner) => {
                if let Some(idempotency_key) = &inner.idempotency_key {
                    rib::Expr::from_text(idempotency_key).map_err(|e| {
                        HttpApiDefinitionError::InvalidDefinition(format!(
                            "Invalid worker idempotency key expr: {e}"
                        ))
                    })?;
                }
                if let Some(invocation_context) = &inner.invocation_context {
                    rib::Expr::from_text(invocation_context).map_err(|e| {
                        HttpApiDefinitionError::InvalidDefinition(format!(
                            "Invalid worker invocation context expr: {e}"
                        ))
                    })?;
                }
                rib::Expr::from_text(&inner.response).map_err(|e| {
                    HttpApiDefinitionError::InvalidDefinition(format!(
                        "Invalid worker response expr: {e}"
                    ))
                })?;
            }
            GatewayBinding::SwaggerUi(_) => {}
        }

        Ok(())
    }
}
