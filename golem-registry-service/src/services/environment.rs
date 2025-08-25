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

use crate::model::WithEnvironmentAuth;
use crate::model::auth::{AuthCtx, AuthorizationError};
use crate::repo::environment::{EnvironmentRepo, EnvironmentRevisionRecord};
use anyhow::anyhow;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::environment::{Environment, EnvironmentId, NewEnvironmentData};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::error;
use golem_common::model::auth::EnvironmentAction;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("Environment not found for id {0}")]
    EnvironmentNotFound(EnvironmentId),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for EnvironmentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::EnvironmentNotFound(_) => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(EnvironmentError, RepoError);

pub struct EnvironmentService {
    environment_repo: Arc<dyn EnvironmentRepo>,
}

impl EnvironmentService {
    pub fn new(environment_repo: Arc<dyn EnvironmentRepo>) -> Self {
        Self { environment_repo }
    }

    pub async fn create(
        &self,
        application_id: ApplicationId,
        data: NewEnvironmentData,
        actor: AccountId,
    ) -> Result<Environment, EnvironmentError> {
        let id = EnvironmentId::new_v4();
        let name = data.name.clone();
        let record = EnvironmentRevisionRecord::from_new_model(id, data, actor);
        let result = self
            .environment_repo
            .create(&application_id.0, &name.0, record)
            .await?;

        match result {
            Some(result_record) => Ok(result_record.into()),
            None => Err(anyhow!(
                "Failed creating environment due to unique violation"
            ))?,
        }
    }

    pub async fn get(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<WithEnvironmentAuth<Environment>, EnvironmentError> {
        let result = self
            .environment_repo
            .get_by_id(
                &environment_id.0,
                &auth.account_id.0,
                auth.should_override_storage_visibility_rules(),
                false,
            )
            .await?
            .ok_or(EnvironmentError::EnvironmentNotFound(
                environment_id.clone(),
            ))?
            .into();

        Ok(result)
    }

    // Convenience method for fetching environment and checking permissions against it
    pub async fn get_and_authorize(
        &self,
        environment_id: &EnvironmentId,
        action: EnvironmentAction,
        auth: &AuthCtx,
    ) -> Result<WithEnvironmentAuth<Environment>, EnvironmentError> {
        let environment: WithEnvironmentAuth<Environment> = self
            .environment_repo
            .get_by_id(
                &environment_id.0,
                &auth.account_id.0,
                auth.should_override_storage_visibility_rules(),
                false,
            )
            .await?
            .ok_or(EnvironmentError::EnvironmentNotFound(
                environment_id.clone(),
            ))?
            .into();

        auth.authorize_environment_action(&environment.owner_account_id, &environment.roles_from_shares, action)?;

        Ok(environment)
    }
}
