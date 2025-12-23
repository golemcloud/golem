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

use super::environment::{EnvironmentError, EnvironmentService};
use crate::model::security_scheme::SecurityScheme;
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::security_scheme::{SecuritySchemeRepoError, SecuritySchemeRevisionRecord};
use crate::repo::security_scheme::SecuritySchemeRepo;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::security_scheme::{
    SecuritySchemeCreation, SecuritySchemeId, SecuritySchemeName, SecuritySchemeRevision,
    SecuritySchemeUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum SecuritySchemeError {
    #[error("There is already a scheme with name {0}")]
    SecuritySchemeWithNameAlreadyExists(SecuritySchemeName),
    #[error("Invalid redirect url provided")]
    InvalidRedirectUrl,
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Security scheme {0} not found")]
    SecuritySchemeNotFound(SecuritySchemeId),
    #[error("Security scheme for name {0} not found in environment")]
    SecuritySchemeForNameNotFound(SecuritySchemeName),
    #[error("Concurrent update attempt")]
    ConcurrentUpdateAttempt,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for SecuritySchemeError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InvalidRedirectUrl => self.to_string(),
            Self::SecuritySchemeWithNameAlreadyExists(_) => self.to_string(),
            Self::SecuritySchemeForNameNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::SecuritySchemeNotFound(_) => self.to_string(),
            Self::ConcurrentUpdateAttempt => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    SecuritySchemeError,
    SecuritySchemeRepoError,
    EnvironmentError
);

pub struct SecuritySchemeService {
    security_scheme_repo: Arc<dyn SecuritySchemeRepo>,
    environment_service: Arc<EnvironmentService>,
}

impl SecuritySchemeService {
    pub fn new(
        security_scheme_repo: Arc<dyn SecuritySchemeRepo>,
        environment_service: Arc<EnvironmentService>,
    ) -> Self {
        Self {
            security_scheme_repo,
            environment_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: SecuritySchemeCreation,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    SecuritySchemeError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateSecurityScheme,
        )?;

        let id = SecuritySchemeId::new();

        let redirect_url: RedirectUrl = RedirectUrl::new(data.redirect_url)
            .map_err(|_| SecuritySchemeError::InvalidRedirectUrl)?;
        let scopes: Vec<Scope> = data.scopes.into_iter().map(Scope::new).collect();

        let record = SecuritySchemeRevisionRecord::creation(
            id,
            data.provider_type,
            data.client_id,
            data.client_secret,
            &redirect_url,
            &scopes,
            auth.account_id(),
        );

        let result = self
            .security_scheme_repo
            .create(environment_id.0, data.name.0.clone(), record)
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(SecuritySchemeRepoError::SecuritySchemeViolatesUniqueness) => Err(
                SecuritySchemeError::SecuritySchemeWithNameAlreadyExists(data.name),
            ),
            Err(other) => Err(other.into()),
        }
    }

    pub async fn update(
        &self,
        security_scheme_id: SecuritySchemeId,
        update: SecuritySchemeUpdate,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let (mut security_scheme, environment) =
            self.get_with_environment(security_scheme_id, auth).await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::UpdateSecurityScheme,
        )?;

        if update.current_revision != security_scheme.revision {
            return Err(SecuritySchemeError::ConcurrentUpdateAttempt);
        };

        security_scheme.revision = security_scheme.revision.next()?;
        if let Some(provider_type) = update.provider_type {
            security_scheme.provider_type = provider_type;
        };
        if let Some(client_id) = update.client_id {
            security_scheme.client_id = ClientId::new(client_id);
        };
        if let Some(client_secret) = update.client_secret {
            security_scheme.client_secret = ClientSecret::new(client_secret);
        };
        if let Some(redirect_url) = update.redirect_url {
            let redirect_url: RedirectUrl = RedirectUrl::new(redirect_url)
                .map_err(|_| SecuritySchemeError::InvalidRedirectUrl)?;
            security_scheme.redirect_url = redirect_url;
        };
        if let Some(scopes) = update.scopes {
            let scopes: Vec<Scope> = scopes.into_iter().map(Scope::new).collect();
            security_scheme.scopes = scopes;
        };

        let audit = DeletableRevisionAuditFields::new(auth.account_id().0);

        let result = self
            .security_scheme_repo
            .update(SecuritySchemeRevisionRecord::from_model(
                security_scheme,
                audit,
            ))
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(SecuritySchemeRepoError::ConcurrentModification) => {
                Err(SecuritySchemeError::ConcurrentUpdateAttempt)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn delete(
        &self,
        security_scheme_id: SecuritySchemeId,
        current_revision: SecuritySchemeRevision,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let (mut security_scheme, environment) =
            self.get_with_environment(security_scheme_id, auth).await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteSecurityScheme,
        )?;

        if current_revision != security_scheme.revision {
            return Err(SecuritySchemeError::ConcurrentUpdateAttempt);
        };

        security_scheme.revision = security_scheme.revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.account_id().0);

        let result = self
            .security_scheme_repo
            .delete(SecuritySchemeRevisionRecord::from_model(
                security_scheme,
                audit,
            ))
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(SecuritySchemeRepoError::ConcurrentModification) => {
                Err(SecuritySchemeError::ConcurrentUpdateAttempt)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn get(
        &self,
        security_scheme_id: SecuritySchemeId,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let (security_scheme, _) = self.get_with_environment(security_scheme_id, auth).await?;
        Ok(security_scheme)
    }

    pub async fn get_security_schemes_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<SecurityScheme>, SecuritySchemeError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    SecuritySchemeError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewSecurityScheme,
        )?;

        let result = self
            .security_scheme_repo
            .get_for_environment(environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        Ok(result)
    }

    pub async fn get_security_scheme_for_environment_and_name(
        &self,
        environment: &Environment,
        name: &SecuritySchemeName,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewSecurityScheme,
        )?;

        let result = self
            .security_scheme_repo
            .get_for_environment_and_name(environment.id.0, &name.0)
            .await?
            .ok_or(SecuritySchemeError::SecuritySchemeForNameNotFound(
                name.clone(),
            ))?
            .try_into()?;

        Ok(result)
    }

    async fn get_with_environment(
        &self,
        security_scheme_id: SecuritySchemeId,
        auth: &AuthCtx,
    ) -> Result<(SecurityScheme, Environment), SecuritySchemeError> {
        let security_scheme: SecurityScheme = self
            .security_scheme_repo
            .get_by_id(security_scheme_id.0)
            .await?
            .ok_or(SecuritySchemeError::SecuritySchemeNotFound(
                security_scheme_id,
            ))?
            .try_into()?;

        let environment = self
            .environment_service
            .get(security_scheme.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    SecuritySchemeError::SecuritySchemeNotFound(security_scheme_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewSecurityScheme,
        )
        .map_err(|_| SecuritySchemeError::SecuritySchemeNotFound(security_scheme_id))?;

        Ok((security_scheme, environment))
    }
}
