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

use super::account_usage::AccountUsageService;
use super::account_usage::error::{AccountUsageError, LimitExceededError};
use super::application::ApplicationService;
use crate::repo::environment::{EnvironmentRepo, EnvironmentRevisionRecord};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::environment::EnvironmentRepoError;
use crate::services::application::ApplicationError;
use golem_common::model::account::AccountEmail;
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentId, EnvironmentName, EnvironmentRevision,
    EnvironmentUpdate, EnvironmentWithDetails,
};
use golem_common::{IntoAnyhow, SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AccountAction, EnvironmentAction};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tap::Pipe;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("Environment with this name already exists")]
    EnvironmentWithNameAlreadyExists,
    #[error("Environment not found for id {0}")]
    EnvironmentNotFound(EnvironmentId),
    #[error("Environment not found for name {0}")]
    EnvironmentByNameNotFound(EnvironmentName),
    #[error("Application {0} not found")]
    ParentApplicationNotFound(ApplicationId),
    #[error("Concurrent update attempt")]
    ConcurrentModification,
    #[error(transparent)]
    LimitExceeded(LimitExceededError),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for EnvironmentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::EnvironmentWithNameAlreadyExists => self.to_string(),
            Self::EnvironmentNotFound(_) => self.to_string(),
            Self::EnvironmentByNameNotFound(_) => self.to_string(),
            Self::ParentApplicationNotFound(_) => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::LimitExceeded(inner) => inner.to_safe_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    EnvironmentError,
    RepoError,
    ApplicationError,
    EnvironmentRepoError
);

impl From<AccountUsageError> for EnvironmentError {
    fn from(value: AccountUsageError) -> Self {
        match value {
            AccountUsageError::LimitExceeded(inner) => EnvironmentError::LimitExceeded(inner),
            other => Self::InternalError(other.into_anyhow()),
        }
    }
}

pub struct EnvironmentService {
    environment_repo: Arc<dyn EnvironmentRepo>,
    application_service: Arc<ApplicationService>,
    account_usage_service: Arc<AccountUsageService>,
}

impl EnvironmentService {
    pub fn new(
        environment_repo: Arc<dyn EnvironmentRepo>,
        application_service: Arc<ApplicationService>,
        account_usage_service: Arc<AccountUsageService>,
    ) -> Self {
        Self {
            environment_repo,
            application_service,
            account_usage_service,
        }
    }

    pub async fn create(
        &self,
        application_id: ApplicationId,
        data: EnvironmentCreation,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentError> {
        let application = self
            .application_service
            .get(application_id, auth)
            .await
            .map_err(|err| match err {
                ApplicationError::ApplicationNotFound(application_id) => {
                    EnvironmentError::ParentApplicationNotFound(application_id)
                }
                other => other.into(),
            })?;

        auth.authorize_account_action(application.account_id, AccountAction::CreateEnvironment)?;

        self.account_usage_service
            .ensure_environment_within_limits(application.account_id)
            .await?;

        let record = EnvironmentRevisionRecord::creation(data, auth.account_id());

        let result = self
            .environment_repo
            .create(application_id.0, record)
            .await
            .map_err(|err| match err {
                EnvironmentRepoError::EnvironmentViolatesUniqueness => {
                    EnvironmentError::EnvironmentWithNameAlreadyExists
                }
                other => other.into(),
            })?
            .try_into()?;

        Ok(result)
    }

    pub async fn update(
        &self,
        environment_id: EnvironmentId,
        update: EnvironmentUpdate,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentError> {
        let mut environment = self.get(environment_id, false, auth).await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::UpdateEnvironment,
        )?;

        if update.current_revision != environment.revision {
            return Err(EnvironmentError::ConcurrentModification);
        };

        environment.revision = environment.revision.next()?;
        if let Some(new_name) = update.name {
            environment.name = new_name
        };
        if let Some(compatibility_check) = update.compatibility_check {
            environment.compatibility_check = compatibility_check;
        }
        if let Some(version_check) = update.version_check {
            environment.version_check = version_check;
        }
        if let Some(security_overrides) = update.security_overrides {
            environment.security_overrides = security_overrides;
        }

        let audit = DeletableRevisionAuditFields::new(auth.account_id().0);
        let record = EnvironmentRevisionRecord::from_model(environment, audit);

        let result = self
            .environment_repo
            .update(record)
            .await
            .map_err(|err| match err {
                EnvironmentRepoError::ConcurrentModification => {
                    EnvironmentError::ConcurrentModification
                }
                EnvironmentRepoError::EnvironmentViolatesUniqueness => {
                    EnvironmentError::EnvironmentWithNameAlreadyExists
                }
                other => other.into(),
            })?
            .try_into()?;

        Ok(result)
    }

    pub async fn delete(
        &self,
        environment_id: EnvironmentId,
        current_revision: EnvironmentRevision,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentError> {
        let mut environment = self.get(environment_id, false, auth).await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteEnvironment,
        )?;

        if current_revision != environment.revision {
            return Err(EnvironmentError::ConcurrentModification);
        };

        environment.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.account_id().0);
        let record = EnvironmentRevisionRecord::from_model(environment, audit);

        self.environment_repo
            .delete(record)
            .await
            .map_err(|err| match err {
                EnvironmentRepoError::ConcurrentModification => {
                    EnvironmentError::ConcurrentModification
                }
                other => other.into(),
            })?;

        Ok(())
    }

    pub async fn get(
        &self,
        environment_id: EnvironmentId,
        include_deleted: bool,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentError> {
        let environment: Environment = self
            .environment_repo
            .get_by_id(
                environment_id.0,
                auth.account_id().0,
                include_deleted,
                auth.should_override_storage_visibility_rules(),
            )
            .await?
            .ok_or(EnvironmentError::EnvironmentNotFound(environment_id))?
            .try_into()?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewEnvironment,
        )
        .map_err(|_| EnvironmentError::EnvironmentNotFound(environment_id))?;

        Ok(environment)
    }

    pub async fn get_in_application(
        &self,
        application_id: ApplicationId,
        name: &EnvironmentName,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentError> {
        let result: Environment = self
            .environment_repo
            .get_by_name(
                application_id.0,
                &name.0,
                auth.account_id().0,
                auth.should_override_storage_visibility_rules(),
            )
            .await?
            .ok_or(EnvironmentError::EnvironmentByNameNotFound(name.clone()))?
            .try_into()?;

        auth.authorize_environment_action(
            result.owner_account_id,
            &result.roles_from_active_shares,
            EnvironmentAction::ViewEnvironment,
        )
        .map_err(|_| EnvironmentError::EnvironmentByNameNotFound(name.clone()))?;

        Ok(result)
    }

    pub async fn list_in_application(
        &self,
        application_id: ApplicationId,
        auth: &AuthCtx,
    ) -> Result<Vec<Environment>, EnvironmentError> {
        let mut authorized_environments = Vec::new();
        let mut application_owner_id = None;

        for record in self
            .environment_repo
            .list_by_app(
                application_id.0,
                auth.account_id().0,
                auth.should_override_storage_visibility_rules(),
            )
            .await?
        {
            let owner_account_id = record.owner_account_id();
            let environment_roles_from_shares = record.environment_roles_from_shares();

            let environment: Option<Environment> = record
                .into_revision_record()
                .map(|r| r.try_into())
                .transpose()?;

            application_owner_id.get_or_insert(owner_account_id);

            if let Some(environment) = environment
                && auth
                    .authorize_environment_action(
                        owner_account_id,
                        &environment_roles_from_shares,
                        EnvironmentAction::ViewEnvironment,
                    )
                    .is_ok()
            {
                authorized_environments.push(environment);
            }
        }

        match (application_owner_id, authorized_environments.is_empty()) {
            (Some(_), false) => {
                // checked above using the authorized environment actions -> only return authorized environments
                Ok(authorized_environments)
            }
            (Some(application_owner_id), true) => {
                // application exists but has no environments -> only leak existence if account-level permissions are present
                auth.authorize_account_action(
                    application_owner_id,
                    AccountAction::ListAllApplicationEnvironments,
                )?;

                Ok(authorized_environments)
            }
            (None, _) => {
                // parent application does not exist -> return notfound to prevent leakage
                Err(EnvironmentError::ParentApplicationNotFound(application_id))
            }
        }
    }

    pub async fn list_visible_environments(
        &self,
        account_email: Option<&AccountEmail>,
        app_name: Option<&ApplicationName>,
        env_name: Option<&EnvironmentName>,
        auth: &AuthCtx,
    ) -> Result<Vec<EnvironmentWithDetails>, EnvironmentError> {
        // When we go for an admin ui / view, this should be extended with an optional, admin-only parameter that allows listing for a different account.
        self.environment_repo
            .list_visible_to_account(
                auth.account_id().0,
                account_email.map(|ae| ae.0.as_str()),
                app_name.map(|an| an.0.as_str()),
                env_name.map(|en| en.0.as_str()),
            )
            .await?
            .into_iter()
            .map(EnvironmentWithDetails::try_from)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // Should not be necessary due to the repo already filtering, but check auth here to be on the safe side
            .filter(|e| {
                auth.authorize_environment_action(
                    e.account.id,
                    &e.environment.roles_from_active_shares,
                    EnvironmentAction::ViewEnvironment,
                )
                .is_ok()
            })
            .collect::<Vec<_>>()
            .pipe(Ok)
    }
}
