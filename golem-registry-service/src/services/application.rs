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

use crate::repo::account::AccountRepo;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::error;
use golem_common::model::account::AccountId;
use crate::repo::application::ApplicationRepo;
use golem_common::model::application::{Application, ApplicationId, NewApplicationData};

#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error("Application not found for id {0}")]
    ApplicationNotFound(ApplicationId),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ApplicationError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ApplicationNotFound(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

impl From<RepoError> for ApplicationError {
    fn from(value: RepoError) -> Self {
        Self::InternalError(anyhow::Error::new(value).context("from RepoError"))
    }
}

pub struct ApplicationService {
    application_repo: Arc<dyn ApplicationRepo>,
}

impl ApplicationService {
    pub fn new(
        application_repo: Arc<dyn ApplicationRepo>,
    ) -> Self {
        Self {
            application_repo,
        }
    }

    pub async fn create(
        &self,
        account_id: AccountId,
        data: NewApplicationData,
        actor: AccountId,
    ) -> Result<Application, ApplicationError> {
        // TODO: dedicated create function
        let record = self.application_repo.ensure(&actor.0, &account_id.0, &data.name.0).await?;

        Ok(record.into())
    }

    pub async fn get(&self, application_id: &ApplicationId) -> Result<Application, ApplicationError> {
        let record = self
            .application_repo
            .get_by_id(&application_id.0)
            .await?
            .ok_or(ApplicationError::ApplicationNotFound(application_id.clone()))?;

        Ok(record.into())
    }
}
