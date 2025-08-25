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

use crate::model::auth::{AuthCtx, AuthorizationError};
use crate::repo::application::ApplicationRepo;
use golem_common::model::account::AccountId;
use golem_common::model::application::{Application, ApplicationId, NewApplicationData};
use golem_common::model::auth::AccountAction;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error("Application not found for id {0}")]
    ApplicationNotFound(ApplicationId),
    #[error("{0}")]
    Unauthorized(AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ApplicationError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ApplicationNotFound(_) => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(ApplicationError, RepoError);

impl From<AuthorizationError> for ApplicationError {
    fn from(value: AuthorizationError) -> Self {
        Self::Unauthorized(value)
    }
}

pub struct ApplicationService {
    application_repo: Arc<dyn ApplicationRepo>,
}

impl ApplicationService {
    pub fn new(application_repo: Arc<dyn ApplicationRepo>) -> Self {
        Self { application_repo }
    }

    pub async fn create(
        &self,
        account_id: AccountId,
        data: NewApplicationData,
        auth: &AuthCtx,
    ) -> Result<Application, ApplicationError> {
        auth.authorize_account_action(&account_id, AccountAction::CreateApplication)?;

        // TODO: dedicated create function
        let record = self
            .application_repo
            .ensure(&auth.account_id.0, &account_id.0, &data.name.0)
            .await?;

        Ok(record.into())
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
