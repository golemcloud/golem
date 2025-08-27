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

use super::account::{AccountError, AccountService};
use crate::model::auth::{AuthCtx, AuthorizationError};
use crate::repo::application::ApplicationRepo;
use crate::repo::model::application::{ApplicationRepoError, ApplicationRevisionRecord};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use golem_common::model::account::AccountId;
use golem_common::model::application::{
    Application, ApplicationId, ApplicationRevision, NewApplicationData, UpdatedApplicationData,
};
use golem_common::model::auth::AccountAction;
use golem_common::{SafeDisplay, error_forwarding};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error("Application with this name already exists")]
    ApplicationWithNameAlreadyExists,
    #[error("Application not found for id {0}")]
    ApplicationNotFound(ApplicationId),
    #[error("Parent account not found {0}")]
    ParentAccountNotFound(AccountId),
    #[error("Concurrent update attempt")]
    ConcurrentModification,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ApplicationError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ApplicationWithNameAlreadyExists => self.to_string(),
            Self::ApplicationNotFound(_) => self.to_string(),
            Self::ParentAccountNotFound(_) => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(ApplicationError, ApplicationRepoError, AccountError);

pub struct ApplicationService {
    application_repo: Arc<dyn ApplicationRepo>,
    account_service: Arc<AccountService>,
}

impl ApplicationService {
    pub fn new(
        application_repo: Arc<dyn ApplicationRepo>,
        account_service: Arc<AccountService>,
    ) -> Self {
        Self {
            application_repo,
            account_service,
        }
    }

    pub async fn create(
        &self,
        account_id: AccountId,
        data: NewApplicationData,
        auth: &AuthCtx,
    ) -> Result<Application, ApplicationError> {
        self.account_service
            .get(&account_id, auth)
            .await
            .map_err(|err| match err {
                AccountError::AccountNotFound(_) | AccountError::Unauthorized(_) => {
                    ApplicationError::ParentAccountNotFound(account_id.clone())
                }
                other => other.into(),
            })?;

        auth.authorize_account_action(&account_id, AccountAction::CreateApplication)?;

        let application = Application {
            id: ApplicationId::new_v4(),
            revision: ApplicationRevision::INITIAL,
            account_id: account_id.clone(),
            name: data.name,
        };

        let audit = DeletableRevisionAuditFields::new(auth.account_id.0);
        let record = ApplicationRevisionRecord::from_model(application, audit);

        let result = self
            .application_repo
            .create(&account_id.0, record)
            .await
            .map_err(|err| match err {
                ApplicationRepoError::ApplicationViolatesUniqueness => {
                    ApplicationError::ApplicationWithNameAlreadyExists
                }
                other => other.into(),
            })?
            .into();

        Ok(result)
    }

    pub async fn update(
        &self,
        application_id: &ApplicationId,
        update: UpdatedApplicationData,
        auth: &AuthCtx,
    ) -> Result<Application, ApplicationError> {
        let mut application = self.get(application_id, auth).await?;

        auth.authorize_account_action(&application.account_id, AccountAction::UpdateApplication)?;

        let current_revision = application.revision;
        application.revision = current_revision.next()?;

        if let Some(new_name) = update.new_name {
            application.name = new_name
        };

        let audit = DeletableRevisionAuditFields::new(auth.account_id.0);
        let record = ApplicationRevisionRecord::from_model(application, audit);

        let result = self
            .application_repo
            .update(current_revision.into(), record)
            .await
            .map_err(|err| match err {
                ApplicationRepoError::ConcurrentModification => {
                    ApplicationError::ConcurrentModification
                }
                other => other.into(),
            })?
            .into();

        Ok(result)
    }

    pub async fn delete(
        &self,
        application_id: ApplicationId,
        auth: &AuthCtx,
    ) -> Result<(), ApplicationError> {
        let mut application = self.get(&application_id, auth).await?;

        auth.authorize_account_action(&application.account_id, AccountAction::DeleteApplication)?;

        let current_revision = application.revision;
        application.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.account_id.0);
        let record = ApplicationRevisionRecord::from_model(application, audit);

        self.application_repo
            .delete(current_revision.into(), record)
            .await
            .map_err(|err| match err {
                ApplicationRepoError::ConcurrentModification => {
                    ApplicationError::ConcurrentModification
                }
                other => other.into(),
            })?;

        Ok(())
    }

    pub async fn get(
        &self,
        application_id: &ApplicationId,
        auth: &AuthCtx,
    ) -> Result<Application, ApplicationError> {
        let application: Application = self
            .application_repo
            .get_by_id(&application_id.0)
            .await?
            .ok_or(ApplicationError::ApplicationNotFound(
                application_id.clone(),
            ))?
            .into();

        auth.authorize_account_action(&application.account_id, AccountAction::ViewApplications)
            .map_err(|_| ApplicationError::ApplicationNotFound(application_id.clone()))?;

        Ok(application)
    }
}
